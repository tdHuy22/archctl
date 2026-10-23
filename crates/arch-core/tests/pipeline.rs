//! Cạnh qua bước pipeline: LangGraph, Airflow, `dvc.yaml`.

use arch_core::check::{Report, UnresolvedReason, Via, check, check_files};
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir().join(format!("archctl-pipe-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn write(&self, rel: &str, body: &str) -> &Self {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    fn arch(&self, extra: &str) -> &Self {
        let old = std::fs::read_to_string(self.0.join(".arch/system.arch")).unwrap_or_default();
        self.write(".arch/system.arch", &format!("{old}{extra}"))
    }

    fn doc(&self) -> arch_core::Document {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        let (doc, diags) = arch_core::load(&src);
        assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
        doc
    }

    fn report(&self) -> Report {
        check(&self.0, &self.doc(), &[])
    }

    fn partial(&self, files: &[&str]) -> Report {
        let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
        check_files(&self.0, &self.doc(), &files, true)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn pipe(r: &Report) -> Vec<String> {
    r.violations
        .iter()
        .filter(|v| v.via == Via::Pipeline)
        .map(|v| format!("{:?} {} -> {} {}:{} {}", v.reason, v.from, v.to, v.file, v.line, v.spec))
        .collect()
}

fn unread(r: &Report) -> Vec<String> {
    let mut out: Vec<String> = r
        .unresolved
        .iter()
        .filter(|u| u.reason == UnresolvedReason::Pipeline)
        .map(|u| format!("{}:{} {}", u.file, u.line, u.spec))
        .collect();
    out.extend(r.deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)));
    out
}

const LAW: &str = "component Graph at rag/graph.py\ncomponent Retrieval at rag/retrieval/**\ncomponent Llm at rag/llm.py\ncomponent Store at store/**\nallow Graph -> Retrieval, Llm\nallow Retrieval -> Store\n";

/// Ví dụ 2 của trang quyết định: bước tìm tài liệu và bước gọi mô hình ở hai
/// component, nối trong file đồ thị.
fn rag(name: &str) -> Repo {
    let r = Repo::new(name);
    r.write("rag/__init__.py", "")
        .write("rag/retrieval/__init__.py", "from .search import retrieve\n")
        .write("rag/retrieval/search.py", "def retrieve(state):\n    return state\n")
        .write("rag/llm.py", "def generate(state):\n    return state\n")
        .write("store/__init__.py", "")
        .write(
            "rag/graph.py",
            "from langgraph.graph import StateGraph, START, END\nfrom rag.retrieval.search import retrieve\nfrom rag import llm\n\nbuilder = StateGraph(dict)\nbuilder.add_node(\"retrieve\", retrieve)\nbuilder.add_node(\"generate\", llm.generate)\nbuilder.add_edge(START, \"retrieve\")\nbuilder.add_edge(\"retrieve\", \"generate\")\nbuilder.add_edge(\"generate\", END)\n",
        )
        .arch(LAW);
    r
}

#[test]
fn langgraph_hai_buoc_hai_component_can_allow() {
    let r = rag("lg");
    assert_eq!(pipe(&r.report()), ["NoAllow Retrieval -> Llm rag/graph.py:9 retrieve → generate (langgraph)"]);
    r.arch("allow Retrieval -> Llm\n");
    assert!(pipe(&r.report()).is_empty());
}

#[test]
fn deny_chan_canh_pipeline() {
    let r = rag("deny");
    r.arch("allow Retrieval -> Llm\ndeny Retrieval -> Llm\n");
    let v = pipe(&r.report());
    assert_eq!(v.len(), 1, "{v:?}");
    assert!(v[0].starts_with("Denied"), "{v:?}");
}

/// Hook sửa đúng file đồ thị: thấy cạnh ngay trong lượt sửa.
#[test]
fn hook_sua_file_do_thi() {
    let r = rag("hook");
    assert_eq!(pipe(&r.partial(&["rag/graph.py"])).len(), 1);
    assert!(pipe(&r.partial(&["rag/llm.py"])).is_empty());
}

/// Đồ thị dựng lúc chạy: báo "chưa đọc được", `--strict` tính. Đồ thị trong
/// file test nối bước thật nên cạnh vẫn tính; chỗ không đọc được của nó thì
/// không báo, như bên gửi của hàng đợi.
#[test]
fn do_thi_dong_thi_bao_test_thi_bo() {
    let r = rag("dyn");
    r.write(
        "rag/dyn.py",
        "from langgraph.graph import StateGraph\nfrom rag.llm import generate\ng = StateGraph(dict)\nfor name, fn in STEPS:\n    g.add_node(name, fn)\ng.add_node(\"gen\", generate)\ng.add_conditional_edges(\"gen\", pick)\n",
    )
    .write(
        "tests/test_graph.py",
        "from langgraph.graph import StateGraph\nfrom rag.llm import generate\nfrom rag.retrieval.search import retrieve\ng = StateGraph(dict)\ng.add_node(\"r\", retrieve)\ng.add_node(\"g\", generate)\ng.add_edge(\"r\", \"g\")\ng.add_node(n, x)\n",
    );
    let rep = r.report();
    let u = unread(&rep);
    assert!(u.iter().any(|s| s.starts_with("rag/dyn.py:5 langgraph: tên bước")), "{u:?}");
    assert!(u.iter().any(|s| s.starts_with("rag/dyn.py:7") && s.contains("`pick`")), "{u:?}");
    assert!(!u.iter().any(|s| s.contains("tests/")), "{u:?}");
    assert_eq!(pipe(&rep).len(), 2, "{:?}", pipe(&rep));
    assert!(pipe(&rep)[1].contains("tests/test_graph.py:7"), "{:?}", pipe(&rep));
}

#[test]
fn airflow_operator_va_taskflow() {
    let r = Repo::new("af");
    r.write("etl/__init__.py", "")
        .write("etl/extract.py", "def pull():\n    pass\n")
        .write("etl/load.py", "def push():\n    pass\n")
        .write("dags/daily.py", "from airflow import DAG\nfrom airflow.operators.python import PythonOperator\nfrom airflow.operators.bash import BashOperator\nfrom etl.extract import pull\nfrom etl import load\n\nwith DAG(\"daily\") as dag:\n    a = PythonOperator(task_id=\"a\", python_callable=pull)\n    b = PythonOperator(task_id=\"b\", python_callable=load.push)\n    c = BashOperator(task_id=\"c\", bash_command=\"curl -H 'Authorization: hunter2-af' x\")\n    a >> c >> b\n    a >> b\n")
        .arch("component Dags at dags/**\ncomponent Extract at etl/extract.py\ncomponent Load at etl/load.py\nallow Dags -> Extract, Load\nallow Extract -> Extract\n");
    let rep = r.report();
    assert_eq!(pipe(&rep), ["NoAllow Extract -> Load dags/daily.py:12 a → b (airflow)"]);
    assert!(!format!("{rep:?}").contains("hunter2-af"), "lọt bí mật");
}

/// Ví dụ 3: stage ghi file mà stage sau đọc; script của hai stage ở hai
/// component.
#[test]
fn dvc_stage_qua_file_du_lieu() {
    let r = Repo::new("dvc");
    r.write("src/prepare.py", "print(1)\n")
        .write("src/train/__init__.py", "")
        .write("src/train/__main__.py", "print(2)\n")
        .write(
            "dvc.yaml",
            "stages:\n  prepare:\n    cmd: python src/prepare.py --token hunter2-dvc\n    deps: [data/raw.csv]\n    outs: [data/clean.csv]\n  train:\n    cmd: cd src && python -m train\n    deps:\n      - data/clean.csv\n    outs: [model.pkl]\n  odd:\n    cmd: bash run.sh\n    deps: [model.pkl]\n",
        )
        .arch("component Prepare at src/prepare.py\ncomponent Train at src/train/**\nallow Prepare -> Prepare\n");
    let rep = r.report();
    assert_eq!(pipe(&rep), ["NoAllow Prepare -> Train dvc.yaml:9 stage prepare → train qua data/clean.csv (dvc)"]);
    let u = unread(&rep);
    assert!(u.iter().any(|s| s.contains("script shell `run.sh`")), "{u:?}");
    assert!(!format!("{rep:?}").contains("hunter2-dvc"), "lọt bí mật");
    // Hook sửa `dvc.yaml`: đọc lại ngay.
    assert_eq!(pipe(&r.partial(&["dvc.yaml"])).len(), 1);
}

#[cfg(unix)]
#[test]
fn dvc_wdir_ra_ngoai_va_symlink_khong_doc() {
    let r = Repo::new("dvcout");
    r.write("src/a.py", "")
        .write("dvc.yaml", "stages:\n  a:\n    wdir: ../..\n    cmd: python etc/x.py\n    outs: [o]\n  b:\n    cmd: python src/a.py\n    deps: [o]\n")
        .write("outside/dvc.yaml.real", "stages: {}\n")
        .arch("component A at src/**\nallow A -> A\n");
    std::fs::create_dir_all(r.0.join("sub")).unwrap();
    std::os::unix::fs::symlink(r.0.join("outside/dvc.yaml.real"), r.0.join("sub/dvc.yaml")).unwrap();
    let u = unread(&r.report());
    assert!(u.iter().any(|s| s.starts_with("dvc.yaml") && s.contains("ra ngoài repo")), "{u:?}");
    // Kiểm cả repo không đi theo symlink; hook nhận thẳng đường dẫn thì báo.
    assert!(!u.iter().any(|s| s.starts_with("sub/")), "{u:?}");
    let u = unread(&r.partial(&["sub/dvc.yaml"]));
    assert!(u.iter().any(|s| s.starts_with("sub/dvc.yaml") && s.contains("symlink")), "{u:?}");
}

/// `init --from-code` sinh `allow` cho cạnh pipeline; nâng lên `#accepted`
/// thì không tự vi phạm.
#[test]
fn init_sinh_allow_cho_canh_pipeline() {
    let r = Repo::new("init");
    r.write("app/__init__.py", "")
        .write("app/graph.py", "from langgraph.graph import StateGraph\nfrom retrieval.search import find\nfrom llm.call import ask\ng = StateGraph(dict)\ng.add_node(find)\ng.add_node(ask)\ng.add_edge(\"find\", \"ask\")\n")
        .write("retrieval/__init__.py", "")
        .write("retrieval/search.py", "def find(s):\n    return s\n")
        .write("llm/__init__.py", "")
        .write("llm/call.py", "def ask(s):\n    return s\n");
    let found = arch_core::discover::discover(&r.0, &[]);
    let name = |dir: &str| found.components.iter().find(|(_, g)| g.starts_with(dir)).map(|(c, _)| c.clone()).expect(dir);
    assert_eq!(found.pipeline_edges, [(name("retrieval"), name("llm"))]);
    let law = arch_core::discover::render(&found, "2026-10-23").replace("#draft", "#accepted");
    assert!(law.contains("# Bước pipeline"), "{law}");
    r.write(".arch/system.arch", &law);
    let rep = r.report();
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

/// Bước nhập qua `__init__.py` chỉ nhập lại tên: lần tới file định nghĩa
/// (review 2026-10-23).
#[test]
fn lan_qua_file_chi_nhap_lai_ten() {
    let r = rag("reexport");
    r.write("store/__init__.py", "from rag.llm import generate as gen\n").write(
        "rag/graph.py",
        "from langgraph.graph import StateGraph\nfrom rag.retrieval import retrieve\nfrom store import gen\nb = StateGraph(dict)\nb.add_node('r', retrieve)\nb.add_node('g', gen)\nb.add_edge('r', 'g')\n",
    );
    r.write("rag/retrieval/__init__.py", "from .search import retrieve\n");
    assert_eq!(pipe(&r.report()), ["NoAllow Retrieval -> Llm rag/graph.py:7 r → g (langgraph)"]);
}

/// dvc dựng một đồ thị chung cho mọi `dvc.yaml`; `python -c`, `make` thì báo,
/// không im lặng (review 2026-10-23).
#[test]
fn dvc_hai_file_va_cmd_khong_nhan_ra() {
    let r = Repo::new("dvc2");
    r.write("src/a/prep.py", "")
        .write("src/b/train.py", "")
        .write("p1/dvc.yaml", "stages:\n  prep:\n    cmd: python ../src/a/prep.py\n    outs: [../data/x.csv]\n")
        .write(
            "p2/dvc.yaml",
            "stages:\n  train:\n    cmd: python ../src/b/train.py\n    deps: [../data/x.csv]\n  sneaky:\n    cmd: python -c \"import src.b.train\"\n    deps: [../data/x.csv]\n",
        )
        .arch("component A at src/a/**\ncomponent B at src/b/**\nallow A -> A\n");
    let rep = r.report();
    assert_eq!(pipe(&rep), ["NoAllow A -> B p2/dvc.yaml:4 stage prep → train qua data/x.csv (dvc)"]);
    let u = unread(&rep);
    assert!(u.iter().any(|s| s.starts_with("p2/dvc.yaml") && s.contains("`sneaky`") && s.contains("không trỏ tới file mã")), "{u:?}");
    assert!(!u.iter().any(|s| s.contains("import src")), "không in `cmd`: {u:?}");
    // Hook sửa một `dvc.yaml`: đọc lại cả hai.
    assert_eq!(pipe(&r.partial(&["p1/dvc.yaml"])).len(), 1);
}

/// File đồ thị không thuộc component nào mà quá cỡ thì báo quá lớn, không bỏ
/// im lặng.
#[test]
fn file_do_thi_ngoai_component_qua_co_thi_bao() {
    let r = rag("big");
    let mut big = String::from("from langgraph.graph import StateGraph\n");
    while big.len() < 4 * 1024 * 1024 + 10 {
        big.push_str("# độn cho quá ngưỡng 4 MB của mỗi file\n");
    }
    r.write("pipelines/graph.py", &big);
    let rep = r.report();
    assert!(rep.oversized.iter().any(|(f, _)| f == "pipelines/graph.py"), "{:?}", rep.oversized);
}

/// Review vòng 2, lỗi 9: `cmd` trộn lệnh nhận ra với lệnh không nhận ra thì
/// lệnh sau vẫn được báo; tiện ích (`mkdir`, `cd`) thì không.
#[test]
fn dvc_cmd_tron_lenh() {
    let r = Repo::new("dvcmix");
    r.write("src/a/prep.py", "")
        .write("src/b/noop.py", "")
        .write(
            "dvc.yaml",
            "stages:\n  prep:\n    cmd: mkdir -p data && python src/a/prep.py\n    outs: [data/x.csv]\n  b:\n    cmd: python src/b/noop.py && python -c \"import src.b.job\"\n    deps: [data/x.csv]\n  c:\n    cmd: [python src/b/noop.py, make generate]\n    deps: [data/x.csv]\n",
        )
        .arch("component A at src/a/**\ncomponent B at src/b/**\nallow A -> A\n");
    let u = unread(&r.report());
    assert!(u.iter().any(|s| s.contains("`b`") && s.contains("`python`")), "{u:?}");
    assert!(u.iter().any(|s| s.contains("`c`") && s.contains("`make`")), "{u:?}");
    assert!(!u.iter().any(|s| s.contains("`prep`")), "{u:?}");
    assert!(!u.iter().any(|s| s.contains("import src")), "không in `cmd`: {u:?}");
}

/// Bước bọc bởi gói ngoài mà hàm được bọc không lần ra (`ToolNode(tools)`,
/// `tools` là tham số): báo, không lặng lẽ mất cạnh.
#[test]
fn boc_goi_ngoai_khong_lan_ra_thi_bao() {
    let r = rag("lost");
    r.write(
        "rag/graph.py",
        "from langgraph.graph import StateGraph\nfrom langgraph.prebuilt import ToolNode\nfrom rag.retrieval.search import retrieve\ndef build(tools):\n    b = StateGraph(dict)\n    b.add_node('r', retrieve)\n    b.add_node('t', ToolNode(tools))\n    b.add_edge('r', 't')\n",
    );
    let u = unread(&r.report());
    assert!(u.iter().any(|s| s.starts_with("rag/graph.py:8") && s.contains("bọc")), "{u:?}");
}
