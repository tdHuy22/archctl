//! Tầng hạ tầng: `service`, `link`, và đối chiếu với docker-compose,
//! Dockerfile, lệnh khởi chạy.

use arch_core::check::{Reason, Report, Via, check, check_files};
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir().join(format!("archctl-infra-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn write(&self, rel: &str, body: &str) -> &Self {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
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

/// Ví dụ RAG trong `docs/infra.md`: ba service cùng build từ `.`, chỉ khác
/// lệnh chạy.
fn rag(name: &str) -> Repo {
    let r = Repo::new(name);
    r.write("mlapp/__init__.py", "")
        .write("mlapp/serve/__init__.py", "")
        .write("mlapp/serve/api.py", "from mlapp.rag.retriever import search\napp = None\n")
        .write("mlapp/rag/__init__.py", "")
        .write("mlapp/rag/retriever.py", "from mlapp.data.loader import load\n")
        .write("mlapp/rag/ingest.py", "from mlapp.data.loader import load\n")
        .write("mlapp/data/__init__.py", "")
        .write("mlapp/data/loader.py", "def load(x): return x\n")
        .write("mlapp/train/__init__.py", "")
        .write("mlapp/train/loop.py", "from mlapp.data.loader import load\n")
        .write("Dockerfile", "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\nCMD [\"uvicorn\", \"mlapp.serve.api:app\"]\n")
        .write(
            "docker-compose.yml",
            "services:\n\
             \x20 api:\n\
             \x20   build: .\n\
             \x20   environment:\n\
             \x20     QDRANT_URL: http://qdrant:6333\n\
             \x20     DATABASE_URL: postgres://app:hunter2@db:5432/app\n\
             \x20   depends_on: [qdrant, db]\n\
             \x20 ingest:\n\
             \x20   build: .\n\
             \x20   command: python -m mlapp.rag.ingest\n\
             \x20   environment:\n\
             \x20     - QDRANT_URL=http://qdrant:6333\n\
             \x20 trainer:\n\
             \x20   build: .\n\
             \x20   command: python -m mlapp.train.loop\n\
             \x20   environment:\n\
             \x20     DATABASE_URL: postgres://app:hunter2@db:5432/app\n\
             \x20 qdrant:\n\
             \x20   image: qdrant/qdrant:v1.12.4\n\
             \x20 db:\n\
             \x20   image: postgres:16\n",
        )
        .write(
            ".arch/system.arch",
            "component Serve at mlapp/serve/**\n\
             component Rag   at mlapp/rag/**\n\
             component Train at mlapp/train/**\n\
             component Data  at mlapp/data/**\n\
             component Root  at mlapp/*.py\n\
             service Api     from compose:api     runs Serve, Rag, Data\n\
             service Ingest  from compose:ingest  runs Rag, Data\n\
             service Trainer from compose:trainer runs Train, Data\n\
             service Qdrant  from compose:qdrant\n\
             service Db      from compose:db\n\
             link Api     -> Qdrant, Db\n\
             link Ingest  -> Qdrant\n\
             link Trainer -> Db\n",
        );
    r
}

fn violations(r: &Report) -> Vec<String> {
    r.violations
        .iter()
        .map(|v| format!("{:?} {} -> {} {}:{} {}", v.reason, v.from, v.to, v.file, v.line, v.spec))
        .collect()
}

#[test]
fn khop_thi_khong_bao_gi() {
    let r = rag("khop");
    let report = r.report();
    assert!(report.violations.is_empty(), "{:#?}", violations(&report));
    assert!(report.missing.is_empty() && report.links_unseen.is_empty() && report.services_undeclared.is_empty());
    assert!(report.entry_unknown.is_empty() && report.deploy_unread.is_empty(), "{:?} {:?}", report.entry_unknown, report.deploy_unread);
    assert!(report.deploy_checked);
}

#[test]
fn canh_mang_thieu_link() {
    let r = rag("mang");
    r.write(
        ".arch/system.arch",
        &std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap().replace("link Trainer -> Db\n", ""),
    );
    let report = r.report();
    let v = &report.violations;
    assert_eq!(v.len(), 1, "{:#?}", violations(&report));
    assert_eq!((v[0].reason, v[0].via), (Reason::NotLinked, Via::Network));
    assert_eq!((v[0].from.as_str(), v[0].to.as_str()), ("Trainer", "Db"));
    assert_eq!((v[0].file.as_str(), v[0].line), ("docker-compose.yml", 17));
    assert_eq!(v[0].spec, "DATABASE_URL → db:5432");
}

/// TM-10: mật khẩu trong biến môi trường không có mặt ở đâu trong báo cáo.
#[test]
fn bao_cao_khong_chua_mat_khau() {
    let r = rag("bimat");
    r.write(".arch/system.arch", &std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap().replace("link Api     -> Qdrant, Db\n", ""));
    let report = r.report();
    assert!(!report.violations.is_empty());
    assert!(!format!("{report:?}").contains("hunter2"), "{report:?}");
}

#[test]
fn import_keo_ma_vao_service_khong_khai() {
    let r = rag("runs");
    r.write("mlapp/serve/api.py", "from mlapp.rag.retriever import search\nfrom mlapp.train.loop import load\napp = None\n");
    let report = r.report();
    assert_eq!(
        violations(&report),
        ["NotRun Api -> Train mlapp/serve/api.py:2 mlapp.train.loop"],
        "Train chạy trong Api vì Serve import nó"
    );
    // Trong hook, chỉ file vừa sửa: kiểm được ngay, không cần đọc cấu hình.
    let hook = r.partial(&["mlapp/serve/api.py"]);
    assert_eq!(violations(&hook), violations(&report));
    assert!(!hook.deploy_checked);
}

#[test]
fn diem_vao_nam_ngoai_runs() {
    let r = rag("diemvao");
    r.write(
        ".arch/system.arch",
        &std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap().replace("runs Train, Data", "runs Data"),
    );
    let report = r.report();
    assert_eq!(violations(&report), ["NotRun Trainer -> Train docker-compose.yml:15 khởi chạy mlapp.train.loop"]);
    assert_eq!(report.violations[0].via, Via::Entry);
    // Đổi file compose thì hook kiểm cả phần hạ tầng.
    let hook = r.partial(&["docker-compose.yml"]);
    assert!(hook.deploy_checked);
    assert_eq!(violations(&hook), violations(&report));
}

#[test]
fn khai_bao_khong_con_khop() {
    let r = rag("lachau");
    let law = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(".arch/system.arch", &format!("{law}service Cache from compose:redis\nlink Ingest -> Db\n"));
    r.write("docker-compose.override.yml", "services:\n  worker:\n    image: busybox\n");
    let report = r.report();
    assert!(report.violations.is_empty(), "{:#?}", violations(&report));
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.missing[0].name, "compose:redis");
    assert_eq!(report.links_unseen, [("Ingest".to_string(), "Db".to_string(), 15)]);
    assert_eq!(report.services_undeclared, [("docker-compose.override.yml".to_string(), 2, "worker".to_string())]);
}

#[test]
fn service_tu_dockerfile_va_script_khoi_chay() {
    let r = Repo::new("dockerfile");
    r.write("svc/app/main.py", "from svc.lib import util\n")
        .write("svc/__init__.py", "")
        .write("svc/app/__init__.py", "")
        .write("svc/lib/__init__.py", "")
        .write("svc/lib/util.py", "")
        .write("deploy/start.sh", "#!/bin/sh\nset -e\nalembic upgrade head\nexec \"$@\"\n")
        .write(
            "deploy/Dockerfile",
            "FROM python:3.12 AS base\nWORKDIR /srv\nCOPY svc ./svc\nCOPY deploy/start.sh /start.sh\n\
             FROM base\nENTRYPOINT [\"/start.sh\"]\nCMD [\"python\", \"-m\", \"svc.app.main\"]\n",
        )
        .write(
            ".arch/system.arch",
            "component App at svc/app/**\ncomponent Lib at svc/lib/**\n\
             service Web from dockerfile:deploy/Dockerfile runs App\n",
        );
    // Dockerfile ở `deploy/` nên build context là `deploy/`: `COPY svc` không
    // có ở đó. Viết lại để context là gốc repo.
    r.write("Dockerfile", &std::fs::read_to_string(r.0.join("deploy/Dockerfile")).unwrap());
    r.write(".arch/system.arch", "component App at svc/app/**\ncomponent Lib at svc/lib/**\nservice Web from dockerfile:Dockerfile runs App\n");
    let report = r.report();
    assert_eq!(violations(&report), ["NotRun Web -> Lib svc/app/main.py:1 svc.lib"], "{:?}", report.entry_unknown);
}

#[test]
fn go_build_trong_dockerfile_nhieu_stage() {
    let r = Repo::new("go");
    r.write("backend/go.mod", "module example.com/backend\n\ngo 1.22\n")
        .write("backend/main.go", "package main\n\nimport \"example.com/backend/store\"\n\nfunc main() { store.Open() }\n")
        .write("backend/store/store.go", "package store\n\nfunc Open() {}\n")
        .write(
            "backend/Dockerfile",
            "FROM golang:1.22 AS builder\nWORKDIR /code\nCOPY . .\nRUN go build -o bin/backend main.go\n\
             FROM scratch\nCOPY --from=builder /code/bin/backend /usr/local/bin/backend\nCMD [\"/usr/local/bin/backend\"]\n",
        )
        .write("compose.yaml", "services:\n  backend:\n    build: backend\n")
        .write(".arch/system.arch", "component Main at backend/*.go\ncomponent Store at backend/store/**\nservice Backend from compose:backend runs Store\n");
    let report = r.report();
    // Binary chép sang stage cuối dưới tên khác vẫn ghép được với `go build`
    // ở stage dựng, qua `COPY` của chính stage đó.
    assert_eq!(violations(&report), ["NotRun Backend -> Main backend/Dockerfile:7 khởi chạy /usr/local/bin/backend"]);
}

/// TM-9: đường dẫn trong cấu hình không được đưa archctl ra ngoài dự án.
#[test]
fn duong_dan_ra_ngoai_du_an_bi_chan_va_duoc_bao() {
    let outside = Repo::new("ngoai");
    outside.write("secret.env", "DB_HOST=leaked\n").write("Dockerfile", "FROM x\nCMD [\"python\", \"leak.py\"]\n");
    let r = Repo::new("chan");
    let rel_outside = format!("../{}", outside.0.file_name().unwrap().to_string_lossy());
    r.write(
        "docker-compose.yml",
        &format!(
            "services:\n  a:\n    build: {rel_outside}\n    env_file: {rel_outside}/secret.env\n  b:\n    build:\n      context: .\n      dockerfile: /etc/passwd\n  c:\n    image: x\n    env_file: [linked.env]\n  leaked:\n    image: x\n"
        ),
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.0.join("secret.env"), r.0.join("linked.env")).unwrap();
    // Không `link` nào tới Leaked: đọc được `DB_HOST=leaked` là có vi phạm.
    r.write(".arch/system.arch", "service A from compose:a\nservice B from compose:b\nservice C from compose:c\nservice Leaked from compose:leaked\n");
    let report = r.report();
    let unread: Vec<String> = report.deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(unread.iter().any(|u| u.contains("build context") && u.contains("ngoài dự án")), "{unread:?}");
    assert!(unread.iter().any(|u| u.contains("env_file") && u.contains("ngoài dự án")), "{unread:?}");
    assert!(unread.iter().any(|u| u.contains("dockerfile: /etc/passwd")), "{unread:?}");
    #[cfg(unix)]
    assert!(unread.iter().any(|u| u.starts_with("linked.env") && u.contains("symlink")), "{unread:?}");
    // Không đọc được thì không có cạnh nào sinh ra từ đó.
    assert!(report.violations.is_empty(), "{:#?}", violations(&report));
}

/// TM-9: các nhánh chặn của bộ đọc file: quá 1 MB, FIFO (đọc là treo), và
/// symlink ở thư mục giữa đường dẫn, không chỉ ở đoạn cuối.
#[test]
fn file_cau_hinh_qua_lon_fifo_symlink_giua_bi_chan() {
    let outside = Repo::new("ngoai2");
    outside.write("conf/secret.env", "DB_HOST=leaked\n");
    let r = Repo::new("chan2");
    r.write("big.env", &format!("DB_HOST=leaked\n{}", "# x\n".repeat(300_000)));
    r.write(
        "docker-compose.yml",
        "services:\n  a:\n    image: x\n    env_file: [big.env, pipe.env, conf/secret.env]\n  leaked:\n    image: x\n",
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.0.join("conf"), r.0.join("conf")).unwrap();
        let made = std::process::Command::new("mkfifo").arg(r.0.join("pipe.env")).status();
        assert!(made.is_ok_and(|s| s.success()), "cần mkfifo");
    }
    r.write(".arch/system.arch", "service A from compose:a\nservice Leaked from compose:leaked\n");
    let start = std::time::Instant::now();
    let report = r.report();
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "FIFO không được làm treo: {:?}", start.elapsed());
    let unread: Vec<String> = report.deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(unread.iter().any(|u| u.starts_with("big.env") && u.contains("KB")), "{unread:?}");
    #[cfg(unix)]
    {
        assert!(unread.iter().any(|u| u.starts_with("pipe.env") && u.contains("file thường")), "{unread:?}");
        assert!(unread.iter().any(|u| u.starts_with("conf/secret.env") && u.contains("symlink")), "{unread:?}");
    }
    assert!(report.violations.is_empty(), "không đọc được thì không có cạnh: {:#?}", violations(&report));
}

/// TM-8: thứ dùng chung giữa hàng nghìn service được đọc một lần, và thứ alias
/// nhân lên có trần theo byte. Mỗi ca từng mất hàng chục giây hay hàng GB:
/// argv 60 000 từ qua alias cho 1 500 service (3,4 GB), giá trị env 100 KB qua
/// alias cho 1 000 service (1 GB phải quét), một `env_file` 1 MB nhắc 1 000 lần (40 giây), một
/// Dockerfile lớn cho 2 000 service (37 giây).
#[test]
fn thu_dung_chung_doc_mot_lan_va_alias_co_tran() {
    let timed = |r: &Repo| {
        let start = std::time::Instant::now();
        let report = r.report();
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
        report
    };
    let unread = |report: &Report, needle: &str| report.deploy_unread.iter().any(|u| u.why.contains(needle));

    let argv = Repo::new("argv");
    let words = ["a"; 60_000].join(", ");
    let svcs: String = (0..1500).map(|i| format!("  s{i}: {{build: ., command: *c}}\n")).collect();
    argv.write("docker-compose.yml", &format!("x-c: &c [python, {words}]\nservices:\n{svcs}"));
    argv.write("Dockerfile", "FROM x\n").write(".arch/system.arch", "service S0 from compose:s0\n");
    assert!(unread(&timed(&argv), "MB"), "argv nhân qua alias phải chạm trần byte");

    let env = Repo::new("envalias");
    let keys: String = (0..10).map(|k| format!("  K{k}_HOST: *big\n")).collect();
    let svcs: String = (0..1000).map(|i| format!("  s{i}: {{image: x, environment: *e}}\n")).collect();
    env.write(
        "docker-compose.yml",
        &format!("x-big: &big \"{}\"\nx-e: &e\n{keys}services:\n{svcs}", "a ".repeat(50_000)),
    );
    env.write(".arch/system.arch", "service S0 from compose:s0\n");
    assert!(unread(&timed(&env), "MB"), "giá trị env nhân qua alias phải chạm trần byte");

    let envf = Repo::new("envfile");
    envf.write("big.env", &(0..60_000).map(|i| format!("K{i}_HOST=db\n")).collect::<String>());
    let files = ["big.env"; 10].join(", ");
    let svcs: String = (0..100).map(|i| format!("  s{i}: {{image: x, env_file: [{files}]}}\n")).collect();
    envf.write("docker-compose.yml", &format!("services:\n{svcs}  db: {{image: x}}\n"));
    envf.write(".arch/system.arch", "service S0 from compose:s0\nservice Db from compose:db\nlink S0 -> Db\n");
    timed(&envf);

    let df = Repo::new("dfchung");
    let mut text = String::from("FROM python AS a\nWORKDIR /app\n");
    text.extend((0..4000).map(|i| format!("COPY src{i} /app/src{i}\n")));
    text.extend((0..20_000).map(|i| format!("ENV K{i}=v{i}\n")));
    text.push_str("CMD [\"python\", \"-m\", \"app\"]\n");
    df.write("Dockerfile", &text).write("app.py", "x = 1\n");
    let svcs: String = (0..500).map(|i| format!("  s{i}:\n    build: .\n")).collect();
    df.write("docker-compose.yml", &format!("services:\n{svcs}"));
    df.write(".arch/system.arch", "component App at app.py\nservice S0 from compose:s0 runs App\n");
    timed(&df);
}

/// TM-12: chỗ không đọc hết thì báo, không im lặng. Điểm vào thuộc component
/// mà không biết ngôn ngữ (`./mlapp/serve/run` có `#!/usr/bin/env python3`);
/// điểm vào không đọc được; giá trị env quá dài nên chỉ xét một phần (cạnh tới
/// `db` nằm sau phần tử thứ 256, hay sau tên máy thứ 64).
#[test]
fn diem_vao_va_bien_doc_do_dang_thi_bao() {
    let r = rag("dodang2");
    r.write("mlapp/serve/run", "#!/usr/bin/env python3\nfrom mlapp.train import loop\n");
    r.write("main.py", "import mlapp.serve.api\n");
    std::fs::write(r.0.join("main.py"), b"import mlapp.serve.api\n\xff\xfe\n").unwrap();
    let compose = std::fs::read_to_string(r.0.join("docker-compose.yml")).unwrap();
    let many: Vec<String> = (0..260).map(|i| format!("x{i}")).collect();
    let ports: Vec<String> = (0..70).map(|i| format!("h{i}:1")).collect();
    let compose = compose
        .replace("  api:\n    build: .\n", "  api:\n    build: .\n    command: ./mlapp/serve/run\n")
        .replace("  ingest:\n    build: .\n    command: python -m mlapp.rag.ingest\n", "  ingest:\n    build: .\n    command: python main.py\n")
        .replace(
            "  trainer:\n    build: .\n    command: python -m mlapp.train.loop\n    environment:\n",
            &format!(
                "  trainer:\n    build: .\n    command: python -m mlapp.train.loop\n    environment:\n      DB_HOSTS: \"{} db\"\n      KAFKA: \"{},db:5432\"\n",
                many.join(" "),
                ports.join(",")
            ),
        );
    r.write("docker-compose.yml", &compose);
    let report = r.report();
    let unknown = |name: &str, needle: &str| report.entry_unknown.iter().any(|(s, why)| s == name && why.contains(needle));
    assert!(unknown("Api", "không biết ngôn ngữ"), "{:?}", report.entry_unknown);
    assert!(unknown("Ingest", "main.py"), "{:?}", report.entry_unknown);
    assert!(
        report.deploy_unread.iter().filter(|u| u.why.contains("phần sau không đọc")).count() >= 2,
        "{:?}",
        report.deploy_unread
    );
}

/// TM-8: compose đối kháng không làm treo hook.
#[test]
fn compose_doi_khang_khong_lam_treo() {
    let r = Repo::new("doikhang");
    let mut text = String::from("x0: &x0 [a, a, a, a, a, a, a, a, a, a]\n");
    for d in 1..20 {
        text.push_str(&format!("x{d}: &x{d} [{}]\n", vec![format!("*x{}", d - 1); 10].join(", ")));
    }
    text.push_str("services:\n  a:\n    image: x\n    environment: *x19\n    depends_on: *x19\n");
    r.write("docker-compose.yml", &text);
    r.write("deep/docker-compose.yml", &format!("services: {}", "[".repeat(200_000)));
    r.write(".arch/system.arch", "service A from compose:a\n");
    let start = std::time::Instant::now();
    let report = r.report();
    assert!(start.elapsed() < std::time::Duration::from_secs(10), "{:?}", start.elapsed());
    assert!(report.deploy_unread.iter().any(|u| u.file == "deep/docker-compose.yml" && u.why.contains("lồng sâu")), "{:?}", report.deploy_unread);
}

#[test]
fn khong_khai_service_thi_khong_doc_cau_hinh() {
    let r = rag("tat");
    r.write(".arch/system.arch", "component Serve at mlapp/serve/**\n");
    let report = r.report();
    assert!(!report.deploy_checked, "chưa mô tả triển khai thì không tốn công đọc compose");
}

#[test]
fn luat_service_va_link_sai_bi_bao() {
    let errors = |src: &str| {
        let (_, diags) = arch_core::load(src);
        diags.iter().filter(|d| d.severity == arch_core::Severity::Error).map(|d| d.msg.clone()).collect::<Vec<_>>()
    };
    assert!(errors("service A runs X\n")[0].contains("chưa khai báo"));
    assert!(errors("external T at \"torch\"\nservice A runs T\n")[0].contains("external"));
    assert!(errors("service A\nlink A -> B\n")[0].contains("`B`"));
    assert!(errors("service A\nservice A\n")[0].contains("trùng tên"));
    assert!(errors("service A from compose:x\nservice B from compose:x\n")[0].contains("đã dùng"));
    assert!(errors("service A from dockerfile:../x/Dockerfile\n")[0].contains("trong dự án"));
    assert!(errors("service A from compose:../x/api\n")[0].contains("trong dự án"));
    assert!(errors("service A from helm:x\n")[0].contains("không nhận ra"));
    assert!(errors("component Api at api/**\nservice Api from compose:api runs Api\n").is_empty(), "không gian tên riêng");
    let (_, diags) = arch_core::load("service A\nservice B from compose:b\nlink A -> B\n");
    assert!(diags.iter().any(|d| d.msg.contains("không có `from`")), "{diags:?}");
}

#[test]
fn doi_service_hay_link_la_doi_kien_truc() {
    let hash = |s: &str| arch_core::arch_hash(&arch_core::load(s).0);
    let base = "service A from compose:a runs X\ncomponent X at x/**\nservice B\nlink A -> B\n";
    assert_ne!(hash(base), hash(&base.replace("link A -> B\n", "")));
    assert_ne!(hash(base), hash(&base.replace("runs X", "")));
    assert_ne!(hash(base), hash(&base.replace("compose:a", "compose:b")));
    let d = arch_core::diff::diff(&arch_core::load(base).0, &arch_core::load(&base.replace("link A -> B\n", "")).0);
    assert_eq!(d.count(arch_core::diff::Class::Topology), 1);
}

/// TM-8: file compose dưới 1 MB mà nhiều mục không được làm bộ nạp chạy bậc
/// hai. Hai ca này từng mất 37 và 9 giây trong hook.
#[test]
fn compose_nhieu_muc_doc_tuyen_tinh() {
    let r = Repo::new("nhieumuc");
    let mut env = String::from("services:\n  a:\n    image: x\n    environment:\n");
    for i in 0..40_000 {
        env.push_str(&format!("      H{i}: b:1\n"));
    }
    env.push_str("  b:\n    image: y\n");
    r.write("docker-compose.yml", &env);
    let mut many = String::from("services:\n");
    for i in 0..25_000 {
        many.push_str(&format!("  s{i}: {{image: x, extends: s0}}\n"));
    }
    r.write("deploy/docker-compose.yml", &many);
    r.write(".arch/system.arch", "service A from compose:a\nservice B from compose:b\nlink A -> B\n");
    let start = std::time::Instant::now();
    let report = r.report();
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    // Quá trần số service thì file bị bỏ và được báo, không đọc dở.
    assert_eq!(report.deploy_unread.len(), 1, "{:?}", report.deploy_unread);
    assert!(report.deploy_unread[0].file == "deploy/docker-compose.yml" && report.deploy_unread[0].why.contains("quá"));
}

/// TM-8: alias nhân số mục lên n·m mà không tốn byte nào; trần số mục của file
/// dừng nó lại. Bản chưa có trần: 46 KB ăn 1,35 GB.
#[test]
fn alias_nhan_so_muc_dung_o_tran() {
    let r = Repo::new("aliasnhan");
    let deps: Vec<String> = (0..2000).map(|i| format!("s{i}")).collect();
    let mut text = format!("x-deps: &deps [{}]\nx-svc: &svc {{image: busybox, depends_on: *deps}}\nservices:\n", deps.join(", "));
    for i in 0..2000 {
        text.push_str(&format!("  s{i}: *svc\n"));
    }
    r.write("docker-compose.yml", &text);
    r.write(".arch/system.arch", "service A from compose:s0\n");
    let start = std::time::Instant::now();
    let report = r.report();
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
    assert!(report.deploy_unread.iter().any(|u| u.why.contains("quá")), "{:?}", report.deploy_unread);
}

/// TM-12: lệnh khởi chạy không lần ra mã thì phải báo — `cd` rồi `python -m`
/// từng lọt qua `--strict` mà không một dòng nào.
#[test]
fn diem_vao_khong_lan_ra_ma_thi_bao() {
    let r = rag("cd");
    let compose = std::fs::read_to_string(r.0.join("docker-compose.yml")).unwrap();
    // `cd` giờ được hiểu: lệnh này chạy Train trong Api.
    r.write("docker-compose.yml", &compose.replace("  api:\n    build: .\n", "  api:\n    build: .\n    command: sh -c \"cd mlapp/train && python -m loop\"\n"));
    let report = r.report();
    assert!(violations(&report).iter().any(|v| v.starts_with("NotRun Api -> Train")), "{:#?}", violations(&report));
    // Thư mục là biến không biết thì không đoán, nhưng phải nói ra.
    r.write("docker-compose.yml", &compose.replace("  api:\n    build: .\n", "  api:\n    build: .\n    command: sh -c \"cd $APP_DIR && python -m loop\"\n"));
    let report = r.report();
    assert!(report.entry_unknown.iter().any(|(s, why)| s == "Api" && why.contains("loop")), "{:?}", report.entry_unknown);
}

/// TM-12: cạnh tới service compose chưa khai không bị bỏ qua.
#[test]
fn canh_toi_service_chua_khai_la_vi_pham() {
    let r = rag("chuakhai");
    let compose = std::fs::read_to_string(r.0.join("docker-compose.yml")).unwrap();
    r.write("docker-compose.yml", &format!("{compose}  pgproxy:\n    image: pgbouncer\n").replace("DATABASE_URL: postgres://app:hunter2@db:5432/app\n    depends_on", "DATABASE_URL: postgres://app:hunter2@PgProxy.:6432/app\n    depends_on"));
    let report = r.report();
    assert!(
        violations(&report).iter().any(|v| v.starts_with("NotLinked Api -> pgproxy")),
        "viết hoa và chấm cuối vẫn khớp, và service chưa khai không làm cạnh biến mất: {:#?}",
        violations(&report)
    );
}

/// TM-8: Dockerfile và script khởi chạy toả nhánh không làm treo hook.
/// `COPY --from` lồng bốn stage, mỗi stage bốn mươi dòng, từng mất 23 giây.
#[test]
fn dockerfile_va_script_toa_nhanh_khong_lam_treo() {
    let r = Repo::new("toanhanh");
    let mut df = String::from("FROM python AS s0\nCOPY src /src\n");
    for k in 1..5 {
        df.push_str(&format!("FROM python AS s{k}\n"));
        for j in 0..40 {
            df.push_str(&format!("COPY --from=s{} /src /src{j}\n", k - 1));
        }
    }
    df.push_str("COPY run /run\nCMD [\"/run/start.sh\"]\n");
    r.write("Dockerfile", &df).write("src/app.py", "x = 1\n");
    // Mỗi script gọi mười script khác, lồng nhiều tầng.
    for d in 0..6 {
        let calls: String = (0..10).map(|k| format!("/run/s{}_{k}.sh\n", d + 1)).collect();
        for k in 0..10 {
            r.write(&format!("run/s{d}_{k}.sh"), &format!("#!/bin/sh\n{calls}"));
        }
    }
    r.write("run/start.sh", &(0..10).map(|k| format!("/run/s0_{k}.sh\n")).collect::<String>());
    r.write("compose.yaml", "services:\n  a:\n    build: .\n");
    r.write(".arch/system.arch", "component Src at src/**\nservice A from compose:a runs Src\n");
    let start = std::time::Instant::now();
    let report = r.report();
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
    assert!(
        report.entry_unknown.iter().any(|(_, why)| why.contains("không tìm thấy")),
        "không tìm ra mã thì nói ra: {:?}",
        report.entry_unknown
    );
    assert!(
        report.entry_unknown.iter().any(|(_, why)| why.contains("không đọc hết")),
        "thôi đọc giữa chừng cũng nói ra: {:?}",
        report.entry_unknown
    );
}

/// TM-12: chỗ bộ đọc thôi đọc giữa chừng là chỗ giấu mã được. Lồng `sh -c`
/// quá sâu, script gọi qua symlink, lệnh không chạy chương trình nào nhận ra:
/// cả ba từng qua `--strict` mà không một dòng nào, dù lệnh nhìn thấy được
/// (uvicorn) nằm gọn trong `runs`.
#[test]
fn lenh_doc_do_dang_thi_bao() {
    let r = rag("dodang");
    let mut deep = "python -m mlapp.train.loop".to_string();
    for _ in 0..6 {
        deep = format!("sh -c '{}'", deep.replace('\'', "'\\''"));
    }
    r.write("deep.sh", &format!("uvicorn mlapp.serve.api:app &\n{deep}\n"))
        .write("link.sh", "uvicorn mlapp.serve.api:app &\n./hidden.sh\n")
        .write("mlapp/train/run.sh", "python -m mlapp.train.loop\n");
    #[cfg(unix)]
    std::os::unix::fs::symlink("mlapp/train/run.sh", r.0.join("hidden.sh")).unwrap();
    let compose = std::fs::read_to_string(r.0.join("docker-compose.yml")).unwrap();
    let mut extra = String::from("  deep:\n    build: .\n    command: sh deep.sh\n  java:\n    build: .\n    command: sh -c \"java -jar app.jar\"\n");
    if cfg!(unix) {
        extra.push_str("  link:\n    build: .\n    command: sh link.sh\n");
    }
    r.write("docker-compose.yml", &format!("{compose}{extra}"));
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(
        ".arch/system.arch",
        &format!(
            "{arch}service Deep from compose:deep runs Serve, Rag, Data\n\
             service Java from compose:java runs Serve, Rag, Data\n\
             service Link from compose:link runs Serve, Rag, Data\n"
        ),
    );
    let report = r.report();
    let cut = |name: &str, needle: &str| {
        report.entry_unknown.iter().any(|(s, why)| s == name && why.contains("không đọc hết") && why.contains(needle))
    };
    assert!(cut("Deep", "lồng quá"), "{:?}", report.entry_unknown);
    assert!(cut("Java", "không chạy chương trình nào"), "{:?}", report.entry_unknown);
    #[cfg(unix)]
    assert!(cut("Link", "hidden.sh"), "{:?}", report.entry_unknown);
    // Service dựng như vậy không được `init` sinh `runs`: luật sinh ra không
    // được tự cảnh báo.
    let found = arch_core::discover::discover(&r.0, &[]);
    for name in ["Deep", "Java"] {
        let svc = found.services.iter().find(|s| s.name == name).expect(name);
        assert!(svc.runs.is_empty(), "{name}: {:?}", svc.runs);
    }
}

/// Dockerfile quá nhiều stage bị bỏ và được báo, không im lặng.
#[test]
fn dockerfile_qua_nhieu_stage_duoc_bao() {
    let r = Repo::new("nhieustage");
    let df: String = (0..100).map(|i| format!("FROM x AS s{i}\n")).collect();
    r.write("Dockerfile", &df).write("compose.yaml", "services:\n  a:\n    build: .\n");
    r.write(".arch/system.arch", "service A from compose:a\n");
    let report = r.report();
    assert!(report.deploy_unread.iter().any(|u| u.file == "Dockerfile" && u.why.contains("stage")), "{:?}", report.deploy_unread);
}

/// Cổng compose publish ra mọi giao diện là lối vào từ Internet; cổng chỉ mở
/// cho máy cục bộ thì không.
#[test]
fn cong_compose_la_loi_vao() {
    for (name, ports, want) in [
        ("hai-cong", "    ports: [\"8080:80\"]\n", true),
        ("chi-container", "    ports: [\"80\"]\n", true),
        ("cuc-bo", "    ports: [\"127.0.0.1:8080:80\", \"[::1]:9000:9000\", \"::1:6000:6000\"]\n", false),
        ("bien-cuc-bo", "    ports: [\"${BIND:-127.0.0.1}:8080:80\"]\n", false),
        ("dang-dai-cuc-bo", "    ports:\n      - {target: 80, published: 8080, host_ip: 127.0.0.1}\n", false),
        ("dang-dai", "    ports:\n      - {target: 80, published: 8080}\n", true),
        ("host", "    network_mode: host\n", true),
        ("expose", "    expose: [\"80\"]\n", false),
    ] {
        let r = Repo::new(&format!("cong-{name}"));
        r.write("compose.yaml", &format!("services:\n  web:\n    image: nginx\n{ports}"))
            .write(".arch/system.arch", "service Web from compose:web\n");
        let rep = check(&r.0, &r.doc(), &[]);
        let got: Vec<String> = rep.violations.iter().map(|v| format!("{} -> {}", v.from, v.to)).collect();
        assert_eq!(!got.is_empty(), want, "{name}: {got:?}");
    }
}

/// Hai service compose; mã của Api gọi Db bằng chuỗi viết thẳng (TM-32, TM-33).
fn code_hosts(name: &str, code: &str, rules: &str) -> Repo {
    let r = Repo::new(name);
    r.write("api/main.py", code)
        .write("api/Dockerfile", "FROM python:3.12-slim\nWORKDIR /app\nCOPY . .\nCMD [\"python\", \"main.py\"]\n")
        .write("tests/test_api.py", "URL = 'http://cache:6379'\n")
        .write(
            "docker-compose.yml",
            "services:\n  api:\n    build: api\n  db:\n    image: postgres:16\n  cache:\n    image: redis:7\n",
        )
        .write(
            ".arch/system.arch",
            &format!("component Api at api/**\ncomponent Tests at tests/**\nservice Api from compose:api runs Api, Tests\nservice Db from compose:db\nservice Cache from compose:cache\n{rules}"),
        );
    r
}

#[test]
fn ten_may_viet_trong_ma_doi_link() {
    let code = "import os\nDSN = 'postgres://app:hunter2@db:5432/app'\n";
    let r = code_hosts("ma-thieu-link", code, "");
    let rep = r.report();
    let got: Vec<String> = rep.violations.iter().map(|v| format!("{} -> {} {}:{} {}", v.from, v.to, v.file, v.line, v.spec)).collect();
    // File test không chạy trong service nào: `cache` ở đó không phải cạnh.
    assert_eq!(got, ["Api -> Db api/main.py:2 DSN → db:5432"]);
    assert_eq!(rep.violations[0].reason, Reason::NotLinked);
    // Mật khẩu trong chuỗi không ra ngoài, ở mọi trường.
    assert!(!format!("{:?}", rep.violations).contains("hunter2"));
    // Hook sửa đúng file mã đó cũng thấy, dù không file cấu hình nào đổi.
    let hook = r.partial(&["api/main.py"]);
    assert_eq!(hook.violations.len(), 1, "{:?}", hook.violations);
    // Có `link` thì yên, và `link` đó không bị coi là "không thấy trong cấu hình".
    let r = code_hosts("ma-co-link", code, "link Api -> Db\n");
    let rep = r.report();
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
    assert!(rep.links_unseen.is_empty(), "{:?}", rep.links_unseen);
}

#[test]
fn ten_may_trong_ma_toi_service_chua_khai() {
    let r = Repo::new("ma-chua-khai");
    r.write("api/main.py", "r = Redis(host='queue')\n")
        .write("docker-compose.yml", "services:\n  api:\n    build: api\n  queue:\n    image: redis:7\n")
        .write(".arch/system.arch", "component Api at api/**\nservice Api from compose:api runs Api\n");
    let rep = r.report();
    let got: Vec<String> = rep.violations.iter().map(|v| format!("{} -> {} {}", v.from, v.to, v.rule)).collect();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(got[0].starts_with("Api -> compose:queue") && got[0].contains("chưa khai"), "{got:?}");
}

#[test]
fn ten_may_trong_ma_khong_doan() {
    // Chữ trùng tên service ở chỗ không phải máy, tên ghép lúc chạy, tên ngoài
    // repo, chính mình: không cạnh nào.
    let code = "name = 'db'\nif x == 'db': pass\nurl = f'http://{svc}:80'\nu = 'https://api.github.com'\nme = 'http://api:8000'\n";
    let r = code_hosts("ma-khong-doan", code, "");
    assert!(r.report().violations.is_empty(), "{:?}", r.report().violations);
}

#[test]
fn ten_may_trong_ma_lach_bang_file_va_so_luong() {
    // Một byte không phải UTF-8 không làm cả file bị bỏ qua.
    let r = code_hosts("ma-latin1", "", "");
    std::fs::write(r.0.join("api/main.py"), b"# -*- coding: latin-1 -*-\n# caf\xe9\nrequests.get('http://db:5432')\n").unwrap();
    let got: Vec<String> = r.report().violations.iter().map(|v| format!("{} -> {} {}:{}", v.from, v.to, v.file, v.line)).collect();
    assert_eq!(got, ["Api -> Db api/main.py:3"]);
    // 1 000 chuỗi tới cùng một service là một vi phạm, đếm số chỗ.
    let many: String = (0..1000).map(|i| format!("h{i} = 'db:5432'\n")).collect();
    let r = code_hosts("ma-nhieu", &many, "");
    let rep = r.report();
    assert_eq!(rep.violations.len(), 1, "{:?}", rep.violations.len());
    assert_eq!(rep.violations[0].count, 1000);
    assert!(rep.violations[0].hint.contains("khỏi mã"), "{}", rep.violations[0].hint);
    // Thư mục ví dụ vẫn được xét: mã chạy thật có thể import nó.
    let r = code_hosts("ma-vi-du", "from examples.conn import URL\n", "");
    r.write("api/examples/conn.py", "URL = 'http://db:5432'\n");
    assert_eq!(r.report().violations.len(), 1, "{:?}", r.report().violations);
}

#[test]
fn bien_moi_truong_login_van_la_may() {
    // Từ chỉ bí mật thêm cho mã (`LOGIN`, `PIN`) không làm mất cạnh từ biến môi trường.
    let r = Repo::new("env-login");
    r.write("docker-compose.yml", "services:\n  api:\n    image: x\n    environment:\n      LOGIN_HOST: login\n      PIN_SERVICE_HOST: db\n  login:\n    image: y\n  db:\n    image: postgres:16\n")
        .write(".arch/system.arch", "service Api from compose:api\nservice Login from compose:login\nservice Db from compose:db\n");
    let mut got: Vec<String> = r.report().violations.iter().map(|v| format!("{} -> {}", v.from, v.to)).collect();
    got.sort();
    assert_eq!(got, ["Api -> Db", "Api -> Login"]);
}
