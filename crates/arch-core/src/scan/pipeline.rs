//! Đồ thị pipeline khai trong mã (LangGraph, Airflow) và trong `dvc.yaml`.
//!
//! Các bước của một pipeline nối với nhau bằng lời gọi hàm hay bằng file cấu
//! hình, không bằng import: `rag/graph.py` import cả `retrieve` lẫn `generate`
//! rồi `add_edge("retrieve", "generate")`, nhưng hai file của hai bước thì
//! không import nhau. Luật cấm Retrieval nói với Llm vì thế không có tác dụng.
//! Phần này đọc các lời gọi đó trên dòng token, không chạy gì, và ra cạnh giữa
//! hàm của hai bước; bên gọi lần hàm về file qua import của chính file đồ thị
//! rồi kiểm như mọi cạnh khác (`allow`).
//!
//! Tên được tra theo phạm vi như Python: câu lệnh nằm trong `def`, `class` nào
//! thì biết nhờ thụt lề của dòng; lần gán hay import gần nhất trước dòng dùng
//! thắng. Hai hàm cùng dựng đồ thị vào một biến `workflow` là hai đồ thị.
//!
//! Chỉ đọc dạng khai tĩnh. Tên bước lấy từ biến, đồ thị dựng trong vòng lặp,
//! nhánh rẽ do hàm chọn lúc chạy (`Command(goto=…)`, `Send`) thì báo "chưa đọc
//! được" kèm dòng, không đoán (TM-63). Không kiểm thứ tự bước hay điều kiện
//! rẽ: chỉ "bước này đưa dữ liệu cho bước kia".

use std::cell::Cell;
use std::collections::{HashMap, HashSet};

use super::Import;
use super::lexer::{Tok, Token};

/// Trần số cạnh của một file (TM-62): `[a1..a1000] >> [b1..b1000]` là một
/// triệu cạnh.
pub const MAX_EDGES: usize = 10_000;
/// Trần phần tử một danh sách bước hay task, và số task của một vế.
const MAX_ITEMS: usize = 1_000;
/// Lần theo phép gán `x = y` tối đa bấy nhiêu tầng.
const MAX_FOLLOW: usize = 4;
/// Trần độ lồng của biểu thức: ngoặc lồng nhau, lời gọi lồng trong đối số.
const MAX_NEST: usize = 64;
/// Số đối số của một lời gọi bọc (`traceable(f)`, `ToolNode([a, b])`) được xét.
const MAX_ARGS: usize = 16;
/// Ngân sách bước cho một file: mỗi biểu thức xét, mỗi lần tra tên, mỗi cặp
/// task ghép tính một. Hết ngân sách thì dừng và báo (TM-62).
pub const MAX_WORK: usize = 2_000_000;
/// Trần chỗ "chưa đọc được" của một file.
const MAX_UNREAD: usize = 1_000;
/// Trần stage của một `dvc.yaml`.
pub const MAX_STAGES: usize = 2_000;
/// Trần deps cộng outs của một `dvc.yaml`.
const MAX_PATHS: usize = 50_000;
/// Trần số đoạn của một đường dẫn trong `dvc.yaml`: mỗi thư mục cha được chỉ
/// mục, nên `a/a/a/…` 80 000 đoạn thành bậc hai (TM-62).
pub const MAX_SEGMENTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    LangGraph,
    Airflow,
    Dvc,
}

impl Tool {
    pub fn as_str(self) -> &'static str {
        match self {
            Tool::LangGraph => "langgraph",
            Tool::Airflow => "airflow",
            Tool::Dvc => "dvc",
        }
    }
}

/// Hàm của một bước, như file đồ thị cho biết.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Step {
    /// Định nghĩa ngay trong file đồ thị: `def`, `class`, `lambda`, phương
    /// thức của `self`.
    Here,
    /// Lấy từ module khác. `from rag.llm import generate` rồi dùng `generate`
    /// là `rag.llm` và `["generate"]`; `import rag` rồi dùng `rag.llm.generate`
    /// là `rag` và `["llm", "generate"]`. Bên gọi thử từ tên dài nhất là
    /// module (`rag.llm.generate`, rồi `rag.llm`) về tới `module`.
    Imported { module: String, path: Vec<String> },
    /// Lời gọi bọc hàm khác: `traceable(generate)`, `partial(f, x=1)`,
    /// `ToolNode([search, fetch])`, `make_step(cfg)`. Hàm thật là `head` khi nó
    /// là mã của repo; `head` là gói ngoài thì hàm thật là các đối số. `lost`:
    /// có đối số vị trí dáng hàm mà không lần ra (`ToolNode(tools)` với `tools`
    /// là tham số): `head` là gói ngoài thì bên gọi báo chưa đọc được.
    Call { head: Box<Step>, args: Vec<Step>, lost: bool },
}

impl Step {
    /// Đối số của lời gọi bọc được trải phẳng một tầng, để bước không phình
    /// thành cây.
    fn push_flat(self, into: &mut Vec<Step>) {
        match self {
            Step::Call { head, args, .. } => {
                into.push(*head);
                into.extend(args);
            }
            s => into.push(s),
        }
    }
}

/// Một cạnh giữa hai bước.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    pub line: usize,
    pub tool: Tool,
    /// Tên bước như file viết: `"retrieve"` của LangGraph, biến `t1` hay
    /// `extract()` của Airflow. Ký tự điều khiển đã thay bằng `?`.
    pub from: String,
    pub to: String,
    pub from_step: Step,
    pub to_step: Step,
}

/// Chỗ có đồ thị mà không đọc được.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unread {
    pub line: usize,
    pub tool: Tool,
    pub why: String,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub edges: Vec<Edge>,
    pub unread: Vec<Unread>,
    /// Chạm một trần (cạnh, bước xét, chỗ chưa đọc được): phần sau không xét.
    pub cut: bool,
}

impl Graph {
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty() && self.unread.is_empty() && !self.cut
    }

    fn note(&mut self, line: usize, tool: Tool, why: String) {
        if self.unread.len() >= MAX_UNREAD {
            self.cut = true;
            return;
        }
        self.unread.push(Unread { line, tool, why: clean(&why) });
    }
}

/// Ký tự điều khiển (ESC, CR) trong tên lấy từ file thì thay bằng `?`: in thẳng
/// ra terminal thì nó xoá được dòng vi phạm (TM-66).
pub fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

fn uses(imports: &[Import], lib: &str) -> bool {
    imports
        .iter()
        .any(|i| i.spec == lib || i.spec.strip_prefix(lib).is_some_and(|r| r.starts_with('.')))
}

/// Có lời gọi `X(` mà `X` (đoạn cuối) thoả `pred`.
fn calls(toks: &[Token], pred: impl Fn(&str) -> bool) -> bool {
    toks.windows(2)
        .any(|w| matches!(&w[0].tok, Tok::Word(x) if pred(last_seg(x))) && w[1].tok == Tok::Punct('('))
}

pub fn is_langgraph_builder(name: &str) -> bool {
    matches!(name, "StateGraph" | "MessageGraph")
}

pub fn is_airflow_task_class(name: &str) -> bool {
    name == "DAG" || name.ends_with("Operator") || name.ends_with("Sensor")
}

/// Đồ thị khai trong một file Python. Chỉ xét file dùng LangGraph (import
/// `langgraph`, hay gọi `StateGraph(` nhập lại qua module khác) hay Airflow
/// (import `airflow`, hay gọi `DAG(`, `…Operator(`, `…Sensor(`): `add_edge`,
/// `>>` ở chỗ khác là việc khác. `src` là văn bản mà `toks` được tách từ đó,
/// để biết thụt lề của từng dòng.
pub fn scan(src: &str, toks: &[Token], imports: &[Import]) -> Graph {
    let langgraph = uses(imports, "langgraph") || calls(toks, is_langgraph_builder);
    let airflow = uses(imports, "airflow") || calls(toks, is_airflow_task_class);
    let mut out = Graph::default();
    if !langgraph && !airflow {
        return out;
    }
    let cx = Cx::new(src, toks);
    if langgraph {
        cx.langgraph(&mut out);
    }
    if airflow {
        cx.airflow(&mut out);
    }
    if cx.cut.get() {
        out.cut = true;
    }
    out
}

/// Tên mà một file gán ở cấp module và lấy từ module khác: `from a import x`
/// rồi `x` là `a` và `["x"]`. Bên gọi dùng để lần qua file chỉ nhập lại tên
/// (`rag/__init__.py`: `from .search import retrieve`). Tên định nghĩa ngay
/// trong file hay không có thì `None`.
pub fn module_binding(src: &str, toks: &[Token], name: &str) -> Option<Step> {
    let cx = Cx::new(src, toks);
    match cx.lookup(name, 0, usize::MAX)? {
        (_, Def::Bind(b), _) => Some(bind_step(b, name)),
        _ => None,
    }
}

/// Một tên mà import gắn vào file: `from M import x as y` gắn `y`.
#[derive(Debug, Clone)]
struct Bind {
    /// Tên trong file, có thể có chấm: `import a.b` gắn `a.b`.
    local: String,
    module: String,
    /// Tên lấy từ module (`from M import x`), không có với `import M`.
    name: Option<String>,
}

fn bind_step(b: &Bind, w: &str) -> Step {
    let segs: Vec<&str> = w.split('.').collect();
    let local: Vec<&str> = b.local.split('.').collect();
    if b.name.is_none() && (local.len() > segs.len() || segs[..local.len()] != local[..]) {
        // `import a.b` gắn cả `a`: `a.x` là thuộc tính của gói `a`.
        return Step::Imported { module: local[0].to_string(), path: segs[1..].iter().map(|s| s.to_string()).collect() };
    }
    let mut path: Vec<String> = b.name.iter().cloned().collect();
    path.extend(segs[local.len().min(segs.len())..].iter().map(|s| s.to_string()));
    Step::Imported { module: b.module.clone(), path }
}

/// Thứ một tên trỏ tới trong một phạm vi.
#[derive(Debug, Clone)]
enum Def {
    /// `def`, `class`; kèm các chuỗi của `-> Literal[...]` nếu có.
    Fn(Option<Vec<String>>),
    Bind(Bind),
    /// `x = <vế phải>`.
    Assign(Span),
    /// `with <biểu thức> as x`; câu lệnh `with` thứ mấy.
    With(Span, usize),
    /// Tham số của hàm: giá trị có lúc chạy.
    Param,
}

/// Một đoạn token `[a, b)`.
type Span = (usize, usize);

/// Một builder LangGraph: phạm vi và dòng của lần gán biến, cùng tên biến.
type Builder = (usize, String, usize);

/// Biểu thức, đủ để nhận ra tên bước và hàm của bước.
#[derive(Debug, Clone)]
enum Expr {
    Str(String),
    /// f-string: tên ghép lúc chạy.
    FStr,
    Name(String),
    /// `head(args)`, kể cả khi sau lời gọi còn `.x(...)` hay `(...)`:
    /// `f.override(task_id="x")()`, `StateGraph(S).compile()`. `args` là
    /// đối số của lời gọi đầu, `rest` của các lời gọi sau trong chuỗi.
    Call { head: String, args: Vec<Arg>, rest: Vec<Arg> },
    Seq(Vec<Span>),
    Dict(Vec<(Span, Span)>),
    /// `lambda …: <thân>`.
    Lambda(Span),
    Num,
    Other,
}

#[derive(Debug, Clone)]
struct Arg {
    kw: Option<String>,
    span: Span,
}

/// Từ khoá mở khối: `:` ở cuối phần đầu của chúng kết thúc câu lệnh.
const COMPOUND: &[&str] = &[
    "with", "for", "if", "elif", "else", "while", "try", "except", "finally", "def", "class", "async",
    "match", "case",
];

/// Từ khoá Python mở câu lệnh, trừ `return`, `yield`.
const KEYWORDS: &[&str] = &[
    "with", "for", "if", "elif", "else", "while", "try", "except", "finally", "def", "class", "async",
    "assert", "del", "raise", "import", "from", "global", "nonlocal", "pass", "break", "continue",
    "lambda", "not",
];

/// Phương thức dựng đồ thị: gọi qua biến (`edge = g.add_edge`) thì không đọc được.
const LG_METHODS: &[&str] = &["add_node", "add_edge", "add_conditional_edges", "add_sequence"];
const AF_METHODS: &[&str] = &["set_downstream", "set_upstream"];

#[derive(Debug, Clone)]
struct Stmt {
    a: usize,
    b: usize,
    /// Phạm vi (`def`, `class`, module) chứa câu lệnh.
    scope: usize,
    /// Trong một DAG Airflow: thân hàm `@dag`, hay khối `with DAG(...)`.
    dag: bool,
}

#[derive(Debug, Clone)]
struct ScopeInfo {
    parent: Option<usize>,
    class: bool,
    /// Hàm có `@dag`, hay nằm trong hàm như vậy.
    dag: bool,
}

struct Cx {
    toks: Vec<Token>,
    /// Vị trí ngoặc đóng của mỗi ngoặc mở; `n` với token khác.
    close: Vec<usize>,
    stmts: Vec<Stmt>,
    /// Câu lệnh chứa mỗi token.
    stmt_of: Vec<usize>,
    scopes: Vec<ScopeInfo>,
    /// Tên → (dòng, thứ nó trỏ tới) của từng phạm vi, theo thứ tự trong file.
    names: Vec<HashMap<String, Vec<(usize, Def)>>>,
    stars: Vec<String>,
    /// Câu lệnh `with TaskGroup(...) as tg` → biến gán trong khối của nó.
    groups: HashMap<usize, Vec<(String, usize, usize)>>,
    work: Cell<usize>,
    cut: Cell<bool>,
}

fn word(t: &Token) -> Option<&str> {
    match &t.tok {
        Tok::Word(w) => Some(w),
        _ => None,
    }
}

fn is_p(t: Option<&Token>, c: char) -> bool {
    t.is_some_and(|t| t.tok == Tok::Punct(c))
}

fn last_seg(w: &str) -> &str {
    w.rsplit('.').next().unwrap_or(w)
}

/// Độ thụt của mỗi dòng (đếm từ 1); tab nhảy tới bội của 8 như Python.
fn indents(src: &str) -> Vec<usize> {
    let mut out = vec![0];
    for line in src.split('\n') {
        let mut n = 0;
        for c in line.chars() {
            match c {
                ' ' => n += 1,
                '\t' => n = (n / 8 + 1) * 8,
                '\u{c}' => {}
                _ => break,
            }
        }
        out.push(n);
    }
    out
}

impl Cx {
    fn new(src: &str, raw: &[Token]) -> Self {
        // `\` nối dòng không phải mã: bỏ, nhưng nhớ rằng dòng sau là nối tiếp.
        let mut toks = Vec::with_capacity(raw.len());
        let mut cont = Vec::with_capacity(raw.len());
        let mut pending = false;
        for t in raw {
            if t.tok == Tok::Punct('\\') {
                pending = true;
                continue;
            }
            toks.push(t.clone());
            cont.push(pending);
            pending = false;
        }
        let n = toks.len();
        let mut close = vec![n; n];
        let mut stack: Vec<usize> = Vec::new();
        for (j, t) in toks.iter().enumerate() {
            match t.tok {
                Tok::Punct('(' | '[' | '{') => stack.push(j),
                Tok::Punct(')' | ']' | '}') => {
                    if let Some(o) = stack.pop() {
                        close[o] = j;
                    }
                }
                _ => {}
            }
        }
        let mut cx = Cx {
            toks,
            close,
            stmts: Vec::new(),
            stmt_of: vec![0; n],
            scopes: vec![ScopeInfo { parent: None, class: false, dag: false }],
            names: vec![HashMap::new()],
            stars: Vec::new(),
            groups: HashMap::new(),
            work: Cell::new(0),
            cut: Cell::new(false),
        };
        let spans = cx.statements(&cont);
        cx.structure(&spans, &indents(src));
        cx
    }

    /// Ranh giới câu lệnh: sang dòng mới ở ngoài ngoặc mà dòng trước không
    /// nối bằng `\`, sau `;`, và sau `:` kết thúc phần đầu của khối viết cùng
    /// dòng (`with X() as tg: a >> b`). Cờ: câu lệnh là thân viết cùng dòng
    /// với phần đầu của khối.
    fn statements(&self, cont: &[bool]) -> Vec<(usize, usize, bool)> {
        let toks = &self.toks;
        let mut out = Vec::new();
        let mut start = 0usize;
        let mut inline = false;
        let mut depth = 0usize;
        for j in 0..toks.len() {
            if j > start && depth == 0 {
                let prev = &toks[j - 1];
                let newline = toks[j].line != prev.line && !cont[j];
                let semi = prev.tok == Tok::Punct(';');
                let head_end = prev.tok == Tok::Punct(':')
                    && toks[j].line == prev.line
                    && word(&toks[start]).is_some_and(|w| COMPOUND.contains(&w));
                if newline || semi || head_end {
                    // `;` ngăn câu lệnh, không thuộc câu lệnh nào.
                    let end = if prev.tok == Tok::Punct(';') { j - 1 } else { j };
                    if end > start {
                        out.push((start, end, inline));
                    }
                    inline = head_end || (semi && inline);
                    start = j;
                }
            }
            match toks[j].tok {
                Tok::Punct('(' | '[' | '{') => depth += 1,
                Tok::Punct(')' | ']' | '}') => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let end = if toks.last().is_some_and(|t| t.tok == Tok::Punct(';')) { toks.len() - 1 } else { toks.len() };
        if start < end {
            out.push((start, end, inline));
        }
        out
    }

    /// Phạm vi, khối `with DAG`/`with TaskGroup`, và tên gán trong từng phạm vi.
    fn structure(&mut self, spans: &[(usize, usize, bool)], indent: &[usize]) {
        enum Kind {
            Scope(usize),
            Dag,
            Group(usize),
        }
        let mut blocks: Vec<(usize, Kind)> = Vec::new();
        let mut decorators: Vec<String> = Vec::new();
        for (k, &(a, b, inline)) in spans.iter().enumerate() {
            let line = self.toks[a].line;
            let ind = indent.get(line).copied().unwrap_or(0);
            if !inline {
                while blocks.last().is_some_and(|(i, _)| ind <= *i) {
                    blocks.pop();
                }
            }
            let inner = blocks.iter().rposition(|(_, k)| matches!(k, Kind::Scope(_)));
            let scope = inner.and_then(|i| if let Kind::Scope(s) = blocks[i].1 { Some(s) } else { None }).unwrap_or(0);
            // Trong DAG: thân hàm `@dag`, `@task_group`, hay khối `with DAG` mở
            // trong chính phạm vi này. Thân một `def` lồng bên trong (hàm của
            // `@task`, `python_callable`) là mã chạy lúc thực thi, không nối đồ thị.
            let dag = self.scopes[scope].dag || blocks[inner.map_or(0, |i| i + 1)..].iter().any(|(_, k)| matches!(k, Kind::Dag));
            let groups: Vec<usize> = blocks.iter().filter_map(|(_, k)| if let Kind::Group(g) = k { Some(*g) } else { None }).collect();
            for j in a..b {
                self.stmt_of[j] = k;
            }
            self.stmts.push(Stmt { a, b, scope, dag });
            let first = word(&self.toks[a]);
            let head = if first == Some("async") { a + 1 } else { a };
            match self.toks.get(head).and_then(word) {
                _ if self.toks[a].tok == Tok::Punct('@') => {
                    if let Some(d) = self.toks.get(a + 1).and_then(word) {
                        decorators.push(last_seg(d).to_string());
                    }
                    continue;
                }
                Some("def" | "class") => {
                    let class = word(&self.toks[head]) == Some("class");
                    let params = self.def(head + 1, b, scope);
                    let id = self.scopes.len();
                    let dag = decorators.iter().any(|d| d == "dag" || d == "task_group");
                    self.scopes.push(ScopeInfo { parent: Some(scope), class, dag });
                    self.names.push(HashMap::new());
                    for p in params {
                        self.bind(id, p, line, Def::Param);
                    }
                    blocks.push((ind, Kind::Scope(id)));
                }
                Some("with") => {
                    let (is_dag, group) = self.with_items(head + 1, b, scope, k);
                    if let Some(g) = group {
                        blocks.push((ind, Kind::Group(g)));
                    } else if is_dag {
                        blocks.push((ind, Kind::Dag));
                    }
                }
                Some("from") => self.read_from_import(a, b, scope),
                Some("import") => self.read_import(a, b, scope),
                _ => {
                    if let Some((targets, rhs)) = self.assignment(a, b) {
                        for t in targets {
                            for g in &groups {
                                self.groups.entry(*g).or_default().push((t.clone(), line, scope));
                            }
                            self.bind(scope, t, line, Def::Assign((rhs, b)));
                        }
                    }
                }
            }
            decorators.clear();
        }
    }

    fn bind(&mut self, scope: usize, name: String, line: usize, def: Def) {
        if let Some(m) = self.names.get_mut(scope) {
            m.entry(name).or_default().push((line, def));
        }
    }

    /// `x = …`, `a = b = …`, `x: T = …`: các tên ở vế trái và chỗ bắt đầu vế phải.
    /// Vế trái không phải một tên (`a, b = …`, `self.x = …`) thì bỏ tên đó.
    fn assignment(&self, a: usize, b: usize) -> Option<(Vec<String>, usize)> {
        let toks = &self.toks;
        if word(&toks[a]).is_some_and(|w| KEYWORDS.contains(&w) || w == "return" || w == "yield") {
            return None;
        }
        let mut eqs = Vec::new();
        let mut k = a;
        while k < b {
            if self.opens(k) {
                k = self.close[k] + 1;
                continue;
            }
            if word(&toks[k]) == Some("lambda") {
                break;
            }
            if is_p(toks.get(k), '=')
                && k > a
                && !is_p(toks.get(k + 1), '=')
                && !matches!(toks[k - 1].tok, Tok::Punct(c) if "=!<>+-*/%&|^@:".contains(c))
            {
                eqs.push(k);
            }
            k += 1;
        }
        let last = *eqs.last()?;
        let mut targets = Vec::new();
        let mut s = a;
        for (n, &e) in eqs.iter().enumerate() {
            let single = e == s + 1;
            let annotated = n == 0 && is_p(toks.get(s + 1), ':');
            if let Some(w) = toks.get(s).and_then(word)
                && (single || annotated)
                && !w.contains('.')
            {
                targets.push(w.to_string());
            }
            s = e + 1;
        }
        Some((targets, last + 1))
    }

    /// Mục của `with`: `X(...) as v` gắn `v`. Trả về: có `DAG(...)`; câu lệnh là
    /// `with TaskGroup(...) as v`.
    fn with_items(&mut self, a: usize, b: usize, scope: usize, stmt: usize) -> (bool, Option<usize>) {
        let line = self.toks[a.saturating_sub(1)].line;
        let mut dag = false;
        let mut group = None;
        let mut items: Vec<(Span, Option<String>)> = Vec::new();
        let mut k = a;
        let mut item = a;
        while k < b {
            if self.opens(k) {
                k = self.close[k] + 1;
                continue;
            }
            let end = is_p(self.toks.get(k), ',') || is_p(self.toks.get(k), ':');
            if word(&self.toks[k]) == Some("as") {
                let v = self.toks.get(k + 1).and_then(word).map(str::to_string);
                items.push(((item, k), v));
                k += 2;
                item = k;
                continue;
            }
            if end {
                if k > item {
                    items.push(((item, k), None));
                }
                item = k + 1;
            }
            k += 1;
        }
        for (span, v) in items {
            let head = match self.expr(span) {
                Expr::Call { head, .. } => Some(head),
                _ => None,
            };
            let kind = head.as_deref().map(last_seg);
            if kind == Some("DAG") {
                dag = true;
            }
            if kind == Some("TaskGroup") && v.is_some() {
                group = Some(stmt);
                self.groups.entry(stmt).or_default();
            }
            if let Some(v) = v {
                self.bind(scope, v, line, Def::With(span, stmt));
            }
        }
        (dag, group)
    }

    fn opens(&self, k: usize) -> bool {
        matches!(self.toks.get(k).map(|t| &t.tok), Some(Tok::Punct('(' | '[' | '{'))) && self.close[k] < self.toks.len()
    }

    /// `def`, `class`: gắn tên vào phạm vi bao ngoài; trả về tham số (của
    /// `def`), để gắn vào phạm vi của hàm.
    fn def(&mut self, k: usize, b: usize, scope: usize) -> Vec<String> {
        let Some(name) = self.toks.get(k).and_then(word).map(str::to_string) else { return Vec::new() };
        let line = self.toks[k].line;
        let literal = self.literal_return(k, b);
        let class = k > 0 && self.toks.get(k - 1).and_then(word) == Some("class");
        self.bind(scope, name, line, Def::Fn(literal));
        if class || !is_p(self.toks.get(k + 1), '(') || !self.opens(k + 1) {
            return Vec::new();
        }
        let mut params = Vec::new();
        for (x, _) in self.split(k + 2, self.close[k + 1]) {
            let mut x = x;
            while is_p(self.toks.get(x), '*') {
                x += 1;
            }
            if let Some(w) = self.toks.get(x).and_then(word) {
                params.push(w.to_string());
            }
        }
        params
    }

    /// `def route(s) -> Literal["a", "b"]:`
    fn literal_return(&self, k: usize, b: usize) -> Option<Vec<String>> {
        let toks = &self.toks;
        if !is_p(toks.get(k + 1), '(') || !self.opens(k + 1) {
            return None;
        }
        let c = self.close[k + 1];
        if !(is_p(toks.get(c + 1), '-') && is_p(toks.get(c + 2), '>')) {
            return None;
        }
        let lit = toks.get(c + 3).and_then(word).is_some_and(|w| last_seg(w) == "Literal");
        if !lit || !is_p(toks.get(c + 4), '[') || !self.opens(c + 4) || self.close[c + 4] >= b {
            return None;
        }
        let mut names = Vec::new();
        for (x, y) in self.split(c + 5, self.close[c + 4]) {
            match (&toks[x].tok, y - x) {
                (Tok::Str(s), 1) => names.push(s.clone()),
                _ => return None,
            }
        }
        Some(names)
    }

    fn read_from_import(&mut self, a: usize, b: usize, scope: usize) {
        let line = self.toks[a].line;
        let mut k = a + 1;
        let mut module = String::new();
        while k < b && is_p(self.toks.get(k), '.') {
            module.push('.');
            k += 1;
        }
        if let Some(w) = self.toks.get(k).and_then(word)
            && w != "import"
        {
            module.push_str(w);
            k += 1;
        }
        if self.toks.get(k).and_then(word) != Some("import") || module.is_empty() {
            return;
        }
        k += 1;
        let (mut x, mut y) = (k, b);
        if is_p(self.toks.get(k), '(') && self.opens(k) {
            (x, y) = (k + 1, self.close[k].min(b));
        }
        if is_p(self.toks.get(x), '*') {
            if scope == 0 {
                self.stars.push(module);
            }
            return;
        }
        for (p, q) in self.split(x, y) {
            let Some(name) = self.toks.get(p).and_then(word).map(str::to_string) else { continue };
            let local = if q == p + 3 && self.toks.get(p + 1).and_then(word) == Some("as") {
                self.toks.get(p + 2).and_then(word).map_or(name.clone(), str::to_string)
            } else {
                name.clone()
            };
            let key = local.clone();
            self.bind(scope, key, line, Def::Bind(Bind { local, module: module.clone(), name: Some(name) }));
        }
    }

    fn read_import(&mut self, a: usize, b: usize, scope: usize) {
        let line = self.toks[a].line;
        for (p, q) in self.split(a + 1, b) {
            let Some(name) = self.toks.get(p).and_then(word).map(str::to_string) else { continue };
            let local = if q == p + 3 && self.toks.get(p + 1).and_then(word) == Some("as") {
                self.toks.get(p + 2).and_then(word).map_or(name.clone(), str::to_string)
            } else {
                name.clone()
            };
            let key = local.split('.').next().unwrap_or(&local).to_string();
            self.bind(scope, key, line, Def::Bind(Bind { local, module: name, name: None }));
        }
    }

    /// Các đoạn giữa dấu phẩy ở ngoài ngoặc, bỏ đoạn rỗng.
    fn split(&self, a: usize, b: usize) -> Vec<Span> {
        let mut out = Vec::new();
        let mut start = a;
        let mut k = a;
        while k < b {
            if self.opens(k) {
                k = self.close[k] + 1;
                continue;
            }
            // `lambda s, c: f(s, c)`: dấu phẩy giữa tham số không ngăn đối số.
            if self.toks.get(k).and_then(word) == Some("lambda") {
                while k < b && !is_p(self.toks.get(k), ':') {
                    k = if self.opens(k) { self.close[k] + 1 } else { k + 1 };
                }
                continue;
            }
            if is_p(self.toks.get(k), ',') {
                if k > start {
                    out.push((start, k));
                }
                start = k + 1;
            }
            k += 1;
        }
        if b > start {
            out.push((start, b.min(self.toks.len())));
        }
        out
    }

    fn args(&self, a: usize, b: usize) -> Vec<Arg> {
        self.split(a, b)
            .into_iter()
            .map(|(x, y)| match (self.toks.get(x).and_then(word), is_p(self.toks.get(x + 1), '='), is_p(self.toks.get(x + 2), '=')) {
                (Some(kw), true, false) if y > x + 2 => Arg { kw: Some(kw.to_string()), span: (x + 2, y) },
                _ => Arg { kw: None, span: (x, y) },
            })
            .collect()
    }

    /// Trừ ngân sách bước của file; hết thì đánh dấu cắt.
    fn spend(&self, n: usize) -> Result<(), String> {
        let used = self.work.get() + n;
        if used > MAX_WORK {
            self.cut.set(true);
            return Err(format!("quá {MAX_WORK} bước xét trong file; phần sau không xét"));
        }
        self.work.set(used);
        Ok(())
    }

    fn expr(&self, (mut a, mut b): Span) -> Expr {
        let toks = &self.toks;
        if a >= b || b > toks.len() || self.spend(b - a).is_err() {
            return Expr::Other;
        }
        // `(x)` là chính `x`; `(x,)` mới là tuple.
        while is_p(toks.get(a), '(') && self.opens(a) && self.close[a] + 1 == b {
            let items = self.split(a + 1, b - 1);
            if items.len() != 1 || is_p(toks.get(b - 2), ',') {
                break;
            }
            (a, b) = items[0];
        }
        match &toks[a].tok {
            Tok::Str(s) if b == a + 1 => return Expr::Str(s.clone()),
            Tok::Punct(c) if c.is_ascii_digit() => return Expr::Num,
            Tok::Punct('-') if b > a + 1 && matches!(toks[a + 1].tok, Tok::Punct(c) if c.is_ascii_digit()) => {
                return Expr::Num;
            }
            Tok::Punct('[' | '(') if self.opens(a) && self.close[a] + 1 == b => {
                return Expr::Seq(self.split(a + 1, b - 1));
            }
            Tok::Punct('{') if self.opens(a) && self.close[a] + 1 == b => {
                let mut pairs = Vec::new();
                for (x, y) in self.split(a + 1, b - 1) {
                    let mut k = x;
                    while k < y && !is_p(toks.get(k), ':') {
                        if self.opens(k) {
                            k = self.close[k];
                        }
                        k += 1;
                    }
                    if k >= y {
                        return Expr::Other;
                    }
                    pairs.push(((x, k), (k + 1, y)));
                }
                return Expr::Dict(pairs);
            }
            _ => {}
        }
        // `"a" "b"`: Python nối hai chuỗi viết liền nhau.
        if toks[a..b].iter().all(|t| matches!(t.tok, Tok::Str(_))) {
            let mut joined = String::new();
            for t in &toks[a..b] {
                if let Tok::Str(x) = &t.tok {
                    joined.push_str(x);
                }
            }
            return Expr::Str(joined);
        }
        let Some(w) = word(&toks[a]) else { return Expr::Other };
        if w == "lambda" {
            let mut k = a + 1;
            while k < b && !is_p(toks.get(k), ':') {
                if self.opens(k) {
                    k = self.close[k];
                }
                k += 1;
            }
            return Expr::Lambda((k + 1, b));
        }
        // `f"x"`, `rb"x"`: tiền tố là một từ đứng liền trước chuỗi.
        if b == a + 2
            && let Tok::Str(s) = &toks[a + 1].tok
        {
            let lower = w.to_ascii_lowercase();
            // `b"x"`, `rb"x"` là `bytes`, không phải tên bước.
            if lower.contains('b') && lower.chars().all(|c| "rb".contains(c)) && lower.len() <= 2 {
                return Expr::Other;
            }
            if lower.chars().all(|c| "ru".contains(c)) && lower.len() <= 2 {
                return Expr::Str(s.clone());
            }
            if lower.contains('f') || lower.contains('t') {
                return Expr::FStr;
            }
        }
        if b == a + 1 {
            return match w {
                "None" | "True" | "False" => Expr::Other,
                _ => Expr::Name(w.to_string()),
            };
        }
        let mut k = a + 1;
        let mut first_args: Option<Vec<Arg>> = None;
        let mut rest: Vec<Arg> = Vec::new();
        while k < b {
            if is_p(toks.get(k), '(') && self.opens(k) {
                if first_args.is_none() {
                    first_args = Some(self.args(k + 1, self.close[k]));
                } else {
                    rest.extend(self.args(k + 1, self.close[k]));
                }
                k = self.close[k] + 1;
            } else if is_p(toks.get(k), '.') && toks.get(k + 1).and_then(word).is_some() && first_args.is_some() {
                k += 2;
            } else {
                return Expr::Other;
            }
        }
        match first_args {
            Some(args) => Expr::Call { head: w.to_string(), args, rest },
            None => Expr::Other,
        }
    }

    /// Tên bước viết bằng chuỗi cố định.
    fn literal(&self, span: Span) -> Option<String> {
        match self.expr(span) {
            Expr::Str(s) if !s.contains('\0') => Some(s),
            _ => None,
        }
    }

    /// Thứ mà tên `first` trỏ tới khi dùng ở dòng `line` trong phạm vi `scope`:
    /// trong chính phạm vi đó, lần gán, import, `def` gần nhất trước dòng dùng;
    /// rồi tới phạm vi bao ngoài (bỏ qua thân `class`, như Python), lần cuối
    /// cùng. Không có lần nào trước dòng dùng thì lấy lần đầu (hàm định nghĩa
    /// phía dưới). Trả về cả phạm vi và dòng của lần gán.
    fn lookup(&self, first: &str, scope: usize, line: usize) -> Option<(usize, &Def, usize)> {
        let _ = self.spend(1);
        let mut s = Some(scope);
        let mut own = true;
        while let Some(id) = s {
            if (own || !self.scopes[id].class)
                && let Some(list) = self.names[id].get(first)
            {
                let pick = if own { list.iter().rev().find(|(l, _)| *l <= line).or(list.first()) } else { list.last() };
                if let Some((l, d)) = pick {
                    return Some((id, d, *l));
                }
            }
            own = false;
            s = self.scopes[id].parent;
        }
        None
    }

    /// Bước mà tên `w` lấy từ import, nếu đoạn đầu của nó là tên nhập.
    fn bound(&self, w: &str, scope: usize, line: usize) -> Option<Step> {
        let first = w.split('.').next().unwrap_or(w);
        match self.lookup(first, scope, line)? {
            (_, Def::Bind(b), _) => Some(bind_step(b, w)),
            _ => None,
        }
    }

    /// Tên mà `w` nhập từ thư viện `lib`: `from langgraph.types import Command
    /// as Go` cho `Go` ra `Command`.
    fn lib_name(&self, w: &str, scope: usize, line: usize, lib: &str) -> Option<String> {
        match self.bound(w, scope, line)? {
            Step::Imported { module, path } if module == lib || module.starts_with(&format!("{lib}.")) => path.last().cloned(),
            _ => None,
        }
    }

    fn resolve_name(&self, w: &str, scope: usize, line: usize, depth: usize) -> Result<Step, String> {
        self.spend(1)?;
        let first = w.split('.').next().unwrap_or(w);
        if first == "self" || first == "cls" {
            return Ok(Step::Here);
        }
        match self.lookup(first, scope, line) {
            Some((_, Def::Fn(_), _)) => Ok(Step::Here),
            Some((_, Def::Bind(b), _)) => Ok(bind_step(b, w)),
            Some((_, Def::Param, _)) => Err(format!("`{}` là tham số của hàm, có giá trị lúc chạy", clean(w))),
            Some((sid, Def::Assign(span) | Def::With(span, _), l)) => {
                if depth >= MAX_FOLLOW {
                    return Err(format!("`{}` gán qua quá {MAX_FOLLOW} tầng", clean(w)));
                }
                // Vế phải đọc tên như trước phép gán: `generate =
                // traceable(generate)` bọc `generate` của import.
                self.resolve_expr(*span, sid, l.saturating_sub(1), depth + 1)
            }
            None => match self.stars.as_slice() {
                [module] => Ok(Step::Imported { module: module.clone(), path: w.split('.').map(str::to_string).collect() }),
                _ => Err(format!("không lần được `{}` về hàm nào trong file hay trong import", clean(w))),
            },
        }
    }

    fn resolve_expr(&self, span: Span, scope: usize, line: usize, depth: usize) -> Result<Step, String> {
        if depth > MAX_NEST {
            return Err(format!("biểu thức lồng quá {MAX_NEST} tầng"));
        }
        match self.expr(span) {
            Expr::Lambda(body) => match self.expr(body) {
                // `lambda s: generate(s)`: hàm thật là hàm được gọi; đối số là
                // tham số của lambda, không phải hàm được bọc.
                Expr::Call { head, .. } => self.resolve_name(&head, scope, line, depth + 1),
                _ => Ok(Step::Here),
            },
            Expr::Name(w) => self.resolve_name(&w, scope, line, depth),
            Expr::Call { head, args, .. } => {
                let h = self.resolve_name(&head, scope, line, depth + 1)?;
                let mut inner: Vec<Step> = Vec::new();
                // Đối số vị trí có dáng hàm mà không lần ra cái nào: hàm bọc của
                // gói ngoài sẽ không còn gì để ghép, nên báo thay vì im lặng.
                let (mut tried, mut found) = (0usize, 0usize);
                for a in args.iter().take(MAX_ARGS) {
                    // `ToolNode(tools)` với `tools = [search, fetch]` gán ở dòng khác.
                    let items: Vec<(Span, usize, usize)> = match self.expr(a.span) {
                        Expr::Seq(v) => v.into_iter().take(MAX_ARGS).map(|s| (s, scope, line)).collect(),
                        Expr::Name(w) => match self.lookup(&w, scope, line) {
                            Some((sid, Def::Assign(s), l)) if matches!(self.expr(*s), Expr::Seq(_)) => match self.expr(*s) {
                                Expr::Seq(v) => v.into_iter().take(MAX_ARGS).map(|x| (x, sid, l.saturating_sub(1))).collect(),
                                _ => Vec::new(),
                            },
                            _ => vec![(a.span, scope, line)],
                        },
                        Expr::Call { .. } | Expr::Lambda(_) => vec![(a.span, scope, line)],
                        _ => Vec::new(),
                    };
                    for (it, sc, ln) in items {
                        if a.kw.is_none() {
                            tried += 1;
                        }
                        if let Ok(s) = self.resolve_expr(it, sc, ln, depth + 1) {
                            if a.kw.is_none() {
                                found += 1;
                            }
                            s.push_flat(&mut inner);
                        }
                    }
                    if inner.len() >= MAX_ARGS {
                        break;
                    }
                }
                inner.truncate(MAX_ARGS);
                let lost = tried > 0 && found == 0 && h != Step::Here;
                Ok(if inner.is_empty() && !lost { h } else { Step::Call { head: Box::new(h), args: inner, lost } })
            }
            _ => Err(format!("hàm của bước `{}` không phải tên hay lời gọi", self.shown(span))),
        }
    }

    /// Đoạn mã như viết, ngắn gọn, cho thông báo. Nội dung chuỗi không in: nó
    /// có thể là khoá API hay lệnh có token (TM-66).
    fn shown(&self, (a, b): Span) -> String {
        let mut s = String::new();
        for t in &self.toks[a..b.min(self.toks.len())] {
            match &t.tok {
                Tok::Word(w) => {
                    if s.ends_with(|c: char| c.is_alphanumeric() || c == '_') {
                        s.push(' ');
                    }
                    s.push_str(w);
                }
                Tok::Str(_) | Tok::Template(_) | Tok::Triple(_) => s.push_str("\"…\""),
                Tok::Punct(c) => s.push(*c),
                _ => s.push('…'),
            }
            if s.chars().count() > 60 {
                return clean(&(s.chars().take(60).collect::<String>() + "…"));
            }
        }
        clean(&s)
    }

    /// Lời gọi `recv.method(...)`: vị trí, tên builder, phương thức, đối số.
    fn method_calls<'s>(&'s self, methods: &'s [&'s str]) -> impl Iterator<Item = (usize, String, &'s str, Vec<Arg>)> + 's {
        let toks = &self.toks;
        (0..toks.len()).filter_map(move |i| {
            let w = word(&toks[i])?;
            if !is_p(toks.get(i + 1), '(') || !self.opens(i + 1) {
                return None;
            }
            let (recv, m) = match w.rsplit_once('.') {
                Some((r, m)) => (r.to_string(), m),
                // `StateGraph(S).add_node(…)`: không có tên builder.
                None if i > 0 && is_p(toks.get(i - 1), '.') => ("?".to_string(), w),
                None => return None,
            };
            let m = methods.iter().find(|x| **x == m)?;
            Some((i, recv, *m, self.args(i + 2, self.close[i + 1])))
        })
    }

    /// Phương thức dựng đồ thị đứng một mình, không gọi ngay (`edge =
    /// g.add_edge`): lời gọi sau đó đi qua biến khác, không đọc được.
    fn method_aliases(&self, out: &mut Graph, methods: &[&str], tool: Tool) {
        for i in 0..self.toks.len() {
            let Some(w) = word(&self.toks[i]) else { continue };
            if !w.contains('.') || !methods.contains(&last_seg(w)) || is_p(self.toks.get(i + 1), '(') {
                continue;
            }
            out.note(self.toks[i].line, tool, format!("`{}` dùng qua biến khác, không gọi thẳng", last_seg(w)));
        }
    }

    fn arg<'b>(args: &'b [Arg], pos: usize, kw: &str) -> Option<&'b Arg> {
        args.iter().find(|a| a.kw.as_deref() == Some(kw)).or_else(|| args.iter().filter(|a| a.kw.is_none()).nth(pos))
    }

    fn scope_at(&self, i: usize) -> usize {
        self.stmts.get(self.stmt_of[i]).map_or(0, |s| s.scope)
    }

    // ---- LangGraph ----

    fn langgraph(&self, out: &mut Graph) {
        let toks = &self.toks;
        let lg = Tool::LangGraph;
        // Builder: phạm vi và dòng của lần gán biến builder, cùng tên biến. Gán
        // lại `g = StateGraph()` là builder mới; tham số `g` của hàm khác là
        // builder khác.
        let builder = |recv: &str, scope: usize, line: usize| -> Builder {
            let first = recv.split('.').next().unwrap_or(recv);
            let at = if recv == "?" { None } else { self.lookup(first, scope, line).map(|(s, _, l)| (s, l)) };
            match at {
                Some((s, l)) => (s, recv.to_string(), l),
                None if recv == "?" => (scope, recv.to_string(), line),
                None => (scope, recv.to_string(), 0),
            }
        };
        // Bước của mỗi builder: tên → hàm (lỗi đã báo thì `None`).
        let mut nodes: HashMap<(Builder, String), Option<Step>> = HashMap::new();
        // Builder có bước thêm bằng tên không cố định: tên lạ trong cạnh của
        // nó đã được báo ở chỗ thêm bước.
        let mut dynamic: HashSet<Builder> = HashSet::new();
        let mut pending: Vec<(usize, Builder, String, String)> = Vec::new();
        for (i, recv, m, args) in self.method_calls(&["add_node", "add_sequence"]) {
            let line = toks[i].line;
            let scope = self.scope_at(i);
            let key = builder(&recv, scope, line);
            if m == "add_sequence" {
                let Some(a) = Self::arg(&args, 0, "nodes") else { continue };
                let Expr::Seq(items) = self.expr(a.span) else {
                    out.note(line, lg, "danh sách bước của `add_sequence` không cố định".into());
                    dynamic.insert(key);
                    continue;
                };
                if items.len() > MAX_ITEMS {
                    out.note(line, lg, format!("`add_sequence` quá {MAX_ITEMS} bước"));
                    dynamic.insert(key);
                    continue;
                }
                let mut prev: Option<String> = None;
                for item in items {
                    let (name, step) = match self.expr(item) {
                        Expr::Name(w) => (last_seg(&w).to_string(), self.resolve_name(&w, scope, line, 0)),
                        Expr::Seq(pair) if pair.len() == 2 => match self.literal(pair[0]) {
                            Some(n) => (n, self.resolve_expr(pair[1], scope, line, 0)),
                            None => {
                                out.note(line, lg, "tên bước không cố định".into());
                                dynamic.insert(key.clone());
                                prev = None;
                                continue;
                            }
                        },
                        _ => {
                            out.note(line, lg, format!("bước `{}` không đọc được", self.shown(item)));
                            dynamic.insert(key.clone());
                            prev = None;
                            continue;
                        }
                    };
                    let step = step.map_err(|why| out.note(line, lg, why)).ok();
                    nodes.insert((key.clone(), name.clone()), step);
                    if let Some(p) = prev.replace(name.clone()) {
                        pending.push((line, key.clone(), p, name));
                    }
                }
                continue;
            }
            let node = Self::arg(&args, 0, "node");
            let action = Self::arg(&args, 1, "action");
            let (name, step) = match (node, action) {
                (Some(n), Some(a)) => match self.literal(n.span) {
                    Some(name) => (name, self.resolve_expr(a.span, scope, line, 0)),
                    None => {
                        out.note(line, lg, format!("tên bước `{}` không cố định", self.shown(n.span)));
                        dynamic.insert(key);
                        continue;
                    }
                },
                // `add_node(retrieve)`: tên bước là tên hàm.
                (Some(n), None) => match self.expr(n.span) {
                    Expr::Name(w) => (last_seg(&w).to_string(), self.resolve_name(&w, scope, line, 0)),
                    _ => {
                        out.note(line, lg, format!("tên bước `{}` không đọc được", self.shown(n.span)));
                        dynamic.insert(key);
                        continue;
                    }
                },
                _ => continue,
            };
            let step = step.map_err(|why| out.note(line, lg, why)).ok();
            nodes.insert((key, name), step);
        }

        let edge = |out: &mut Graph, line: usize, key: &Builder, a: &str, b: &str| {
            if a == b {
                return;
            }
            if out.edges.len() >= MAX_EDGES {
                out.cut = true;
                return;
            }
            let mut step = |name: &str| match nodes.get(&(key.clone(), name.to_string())) {
                Some(s) => s.clone(),
                None => {
                    if !dynamic.contains(key) {
                        out.note(line, lg, format!("bước `{}` không khai bằng `add_node` cho builder này", clean(name)));
                    }
                    None
                }
            };
            let (Some(f), Some(t)) = (step(a), step(b)) else { return };
            out.edges.push(Edge { line, tool: lg, from: clean(a), to: clean(b), from_step: f, to_step: t });
        };
        for (line, key, a, b) in pending {
            edge(out, line, &key, &a, &b);
        }
        let is_start = |e: &Expr| matches!(e, Expr::Name(w) if last_seg(w) == "START") || matches!(e, Expr::Str(s) if s == "__start__");
        let is_end = |e: &Expr| matches!(e, Expr::Name(w) if last_seg(w) == "END") || matches!(e, Expr::Str(s) if s == "__end__");
        for (i, recv, m, args) in self.method_calls(&["add_edge", "add_conditional_edges"]) {
            let line = toks[i].line;
            let scope = self.scope_at(i);
            let key = builder(&recv, scope, line);
            let Some(src) = Self::arg(&args, 0, if m == "add_edge" { "start_key" } else { "source" }) else { continue };
            let src_e = self.expr(src.span);
            if is_start(&src_e) {
                continue;
            }
            // Nguồn: một bước, hay danh sách bước cùng chờ (`add_edge(["a", "b"], "c")`).
            let sources: Vec<String> = match &src_e {
                Expr::Str(s) if !s.contains('\0') => vec![s.clone()],
                Expr::Seq(items) if m == "add_edge" && items.len() <= MAX_ITEMS => {
                    let names: Option<Vec<String>> = items.iter().map(|s| self.literal(*s)).collect();
                    match names {
                        Some(n) => n,
                        None => {
                            out.note(line, lg, "tên bước không cố định".into());
                            continue;
                        }
                    }
                }
                _ => {
                    out.note(line, lg, format!("tên bước `{}` không cố định", self.shown(src.span)));
                    continue;
                }
            };
            let targets: Vec<String> = if m == "add_edge" {
                let Some(dst) = Self::arg(&args, 1, "end_key") else { continue };
                if is_end(&self.expr(dst.span)) {
                    continue;
                }
                match self.literal(dst.span) {
                    Some(n) => vec![n],
                    None => {
                        out.note(line, lg, format!("tên bước `{}` không cố định", self.shown(dst.span)));
                        continue;
                    }
                }
            } else {
                match self.branch_targets(&args, scope, line) {
                    Ok(t) => t,
                    Err(why) => {
                        out.note(line, lg, why);
                        continue;
                    }
                }
            };
            for a in &sources {
                for b in &targets {
                    edge(out, line, &key, a, b);
                }
            }
        }
        self.method_aliases(out, LG_METHODS, lg);
        // Bước rẽ lúc chạy: không thấy đích trong file đồ thị.
        for i in 0..toks.len() {
            let Some(w) = word(&toks[i]) else { continue };
            if !is_p(toks.get(i + 1), '(') || !self.opens(i + 1) {
                continue;
            }
            let (line, scope) = (toks[i].line, self.scope_at(i));
            match self.lib_name(w, scope, line, "langgraph").as_deref() {
                Some("Command") => {
                    let args = self.args(i + 2, self.close[i + 1]);
                    let spread = args.iter().any(|a| is_p(toks.get(a.span.0), '*') && is_p(toks.get(a.span.0 + 1), '*'));
                    let positional = args.iter().filter(|a| a.kw.is_none()).count();
                    if spread || positional >= 4 || args.iter().any(|a| a.kw.as_deref() == Some("goto")) {
                        out.note(line, lg, "`Command(goto=…)` chọn bước tiếp theo lúc chạy".into());
                    }
                }
                Some("Send") => out.note(line, lg, "`Send(…)` chọn bước tiếp theo lúc chạy".into()),
                _ => {}
            }
        }
    }

    /// Đích của `add_conditional_edges`: giá trị của `path_map`, hay kiểu trả
    /// về `Literal[...]` của hàm rẽ, hay `tools_condition` (rẽ sang `tools`).
    fn branch_targets(&self, args: &[Arg], scope: usize, line: usize) -> Result<Vec<String>, String> {
        let names = |spans: Vec<Span>| -> Result<Vec<String>, String> {
            if spans.len() > MAX_ITEMS {
                return Err(format!("quá {MAX_ITEMS} đích rẽ"));
            }
            let mut out = Vec::new();
            for s in spans {
                match self.expr(s) {
                    Expr::Name(w) if last_seg(&w) == "END" => {}
                    Expr::Str(x) if x == "__end__" => {}
                    _ => match self.literal(s) {
                        Some(n) => out.push(n),
                        None => return Err(format!("đích rẽ `{}` không cố định", self.shown(s))),
                    },
                }
            }
            Ok(out)
        };
        if let Some(map) = Self::arg(args, 2, "path_map") {
            let mut e = self.expr(map.span);
            if let Expr::Name(w) = &e {
                e = match self.lookup(w, scope, line) {
                    Some((_, Def::Assign(s), _)) => self.expr(*s),
                    _ => Expr::Other,
                };
            }
            return match e {
                Expr::Dict(pairs) => names(pairs.into_iter().map(|(_, v)| v).collect()),
                Expr::Seq(items) => names(items),
                _ => Err(format!("`path_map` `{}` không cố định", self.shown(map.span))),
            };
        }
        let Some(path) = Self::arg(args, 1, "path") else {
            return Err("thiếu hàm rẽ".into());
        };
        if let Expr::Name(w) = self.expr(path.span) {
            if last_seg(&w) == "tools_condition" {
                return Ok(vec!["tools".into()]);
            }
            if let Some((_, Def::Fn(Some(list)), _)) = self.lookup(&w, scope, line) {
                return Ok(list.iter().filter(|n| *n != "__end__").cloned().collect());
            }
        }
        Err(format!(
            "nhánh do `{}` chọn lúc chạy; thêm `path_map` hay kiểu trả về `Literal[...]` để đọc được",
            self.shown(path.span)
        ))
    }

    // ---- Airflow ----

    fn airflow(&self, out: &mut Graph) {
        let toks = &self.toks;
        let af = Tool::Airflow;
        for st in &self.stmts {
            if self.cut.get() {
                break;
            }
            let (a, b, scope) = (st.a, st.b, st.scope);
            let line = toks[a].line;
            // Câu lệnh mở bằng từ khoá (`if a >> b:`, `assert`) không khai
            // cạnh; `return a >> b`, `yield a >> b` thì có.
            let first = word(&toks[a]);
            if first.is_some_and(|w| KEYWORDS.contains(&w)) {
                continue;
            }
            // `x = a >> b`, `x += a >> b`: chỉ xét vế phải.
            let mut start = if first.is_some_and(|w| matches!(w, "return" | "yield")) { a + 1 } else { a };
            let mut k = a;
            while k + 1 < b {
                if self.opens(k) {
                    k = self.close[k] + 1;
                    continue;
                }
                if word(&toks[k]) == Some("lambda") {
                    break;
                }
                if toks[k].tok == Tok::Punct('=')
                    && toks[k + 1].tok != Tok::Punct('=')
                    && k > a
                    && !matches!(toks[k - 1].tok, Tok::Punct(c) if "=!<>".contains(c))
                {
                    start = k + 1;
                }
                k += 1;
            }
            let mut end = b;
            // `(a >> b)` là chính `a >> b`.
            while start < end && is_p(toks.get(start), '(') && self.opens(start) && self.close[start] + 1 == end {
                start += 1;
                end -= 1;
            }
            let shift = |k: usize| match (&toks[k].tok, toks.get(k + 1).map(|t| &t.tok)) {
                (Tok::Punct('>'), Some(Tok::Punct('>'))) if !is_p(toks.get(k + 2), '=') => Some(true),
                (Tok::Punct('<'), Some(Tok::Punct('<'))) if !is_p(toks.get(k + 2), '=') => Some(false),
                _ => None,
            };
            let mut ops: Vec<(usize, bool)> = Vec::new();
            let mut k = start;
            while k + 1 < end {
                if self.opens(k) {
                    k = self.close[k] + 1;
                    continue;
                }
                if let Some(down) = shift(k) {
                    ops.push((k, down));
                    k += 2;
                    continue;
                }
                k += 1;
            }
            if !ops.is_empty() {
                self.chain_statement(out, &ops, start, end, scope, line);
            } else if st.dag {
                self.taskflow_statement(out, st, start, end);
            }
            // `>>` nằm sâu trong biểu thức (`log(t1 >> t2)`), vế trái là task:
            // có cạnh mà không đọc được.
            let top: HashSet<usize> = ops.iter().map(|(k, _)| *k).collect();
            let mut k = a;
            while k + 1 < b {
                if !top.contains(&k)
                    && shift(k).is_some()
                    && k > a
                    && let Some(w) = word(&toks[k - 1])
                    && !w.contains('.')
                    && matches!(self.task_name(w, scope, line, 0), Ok(Some(v)) if !v.is_empty())
                {
                    out.note(line, af, "`>>` nằm trong biểu thức khác, chưa đọc được".into());
                    break;
                }
                k += if shift(k).is_some() { 2 } else { 1 };
            }
        }
        // `a.set_downstream(b)`, `a.set_upstream(b)`.
        for (i, recv, m, args) in self.method_calls(&["set_downstream", "set_upstream", "set_dependency"]) {
            let line = toks[i].line;
            let scope = self.scope_at(i);
            if m == "set_dependency" {
                out.note(line, af, "`set_dependency` nối task theo `task_id`".into());
                continue;
            }
            let Some(arg) = Self::arg(&args, 0, "task_or_task_list") else { continue };
            let left = self.task_name(&recv, scope, line, 0);
            let right = self.tasks(arg.span, scope, line, 0);
            match (left, right) {
                (Ok(Some(l)), Ok(Some(r))) => {
                    if m == "set_downstream" {
                        self.cross(out, line, &l, &r);
                    } else {
                        self.cross(out, line, &r, &l);
                    }
                }
                (Err(why), _) | (_, Err(why)) => out.note(line, af, why),
                _ => {}
            }
        }
        self.method_aliases(out, AF_METHODS, af);
        // `chain(a, b, [c, d])`, `chain_linear(...)`, `cross_downstream(...)` của
        // Airflow, cả khi nhập với tên khác (không phải `itertools.chain`).
        for i in 0..toks.len() {
            let Some(w) = word(&toks[i]) else { continue };
            if !is_p(toks.get(i + 1), '(') || !self.opens(i + 1) {
                continue;
            }
            let (line, scope) = (toks[i].line, self.scope_at(i));
            let Some(kind) = self.lib_name(w, scope, line, "airflow") else { continue };
            if !matches!(kind.as_str(), "chain" | "chain_linear" | "cross_downstream") {
                continue;
            }
            let args = self.args(i + 2, self.close[i + 1]);
            if kind == "cross_downstream" {
                let (Some(f), Some(t)) = (Self::arg(&args, 0, "from_tasks"), Self::arg(&args, 1, "to_tasks")) else { continue };
                match (self.tasks(f.span, scope, line, 0), self.tasks(t.span, scope, line, 0)) {
                    (Ok(Some(l)), Ok(Some(r))) => self.cross(out, line, &l, &r),
                    (Err(why), _) | (_, Err(why)) => out.note(line, af, why),
                    _ => {}
                }
                continue;
            }
            let mut sides: Vec<(Vec<Task>, bool)> = Vec::new();
            let mut bad = false;
            for a in args.iter().filter(|a| a.kw.is_none()) {
                let list = matches!(self.expr(a.span), Expr::Seq(_));
                match self.tasks(a.span, scope, line, 0) {
                    Ok(Some(t)) => sides.push((t, list)),
                    Ok(None) => {
                        bad = true;
                        break;
                    }
                    Err(why) => {
                        out.note(line, af, why);
                        bad = true;
                        break;
                    }
                }
            }
            if bad {
                continue;
            }
            for pair in sides.windows(2) {
                let ((l, ll), (r, rl)) = (&pair[0], &pair[1]);
                if kind == "chain" && *ll && *rl {
                    // Hai danh sách: ghép từng cặp theo vị trí.
                    if l.len() != r.len() {
                        out.note(line, af, "`chain` nối hai danh sách khác độ dài".into());
                        continue;
                    }
                    for (x, y) in l.iter().zip(r) {
                        self.cross(out, line, std::slice::from_ref(x), std::slice::from_ref(y));
                    }
                } else {
                    self.cross(out, line, l, r);
                }
            }
        }
    }

    /// `a >> b << c`: mọi vế phải là task, không thì câu lệnh không khai cạnh
    /// (phép dịch bit) hay báo chưa đọc được. Tính hai lượt để không phải giữ
    /// task của mọi vế cùng lúc (TM-62).
    fn chain_statement(&self, out: &mut Graph, ops: &[(usize, bool)], start: usize, end: usize, scope: usize, line: usize) {
        let mut pieces: Vec<Span> = Vec::new();
        let mut s = start;
        for (k, _) in ops {
            pieces.push((s, *k));
            s = k + 2;
        }
        pieces.push((s, end));
        for p in &pieces {
            match self.tasks(*p, scope, line, 0) {
                Ok(Some(_)) => {}
                // Một vế không phải task: phép dịch bit, không phải cạnh.
                Ok(None) => return,
                Err(why) => {
                    out.note(line, Tool::Airflow, why);
                    return;
                }
            }
        }
        let Ok(Some(mut prev)) = self.tasks(pieces[0], scope, line, 0) else { return };
        for (n, (_, down)) in ops.iter().enumerate() {
            let Ok(Some(cur)) = self.tasks(pieces[n + 1], scope, line, 0) else { return };
            if *down {
                self.cross(out, line, &prev, &cur);
            } else {
                self.cross(out, line, &cur, &prev);
            }
            prev = cur;
        }
    }

    /// TaskFlow trong một DAG: `load(transform(extract()))`, `t = transform(e)`
    /// là cạnh từ task cho đối số tới task được gọi.
    fn taskflow_statement(&self, out: &mut Graph, st: &Stmt, start: usize, b: usize) {
        let line = self.toks[st.a].line;
        let label = self.assignment(st.a, st.b).and_then(|(t, _)| (t.len() == 1).then(|| t[0].clone()));
        self.taskflow(out, (start, b), st.scope, line, label, 0);
    }

    fn taskflow(&self, out: &mut Graph, span: Span, scope: usize, line: usize, label: Option<String>, depth: usize) -> Option<Task> {
        if depth > MAX_NEST || self.cut.get() {
            return None;
        }
        let Expr::Call { head, args, rest } = self.expr(span) else { return None };
        let f = strip_task_suffix(&head);
        if is_airflow_task_class(last_seg(f)) || last_seg(f) == "TaskGroup" {
            return None;
        }
        // Chỉ hàm `def` của file hay tên nhập: `t1.set_downstream(...)`,
        // `dag.get_task(...)` là phương thức của đối tượng, không phải task.
        let first = f.split('.').next().unwrap_or(f);
        if !matches!(self.lookup(first, scope, line), Some((_, Def::Fn(_) | Def::Bind(_), _))) {
            return None;
        }
        let mut callee = self.task_call(&head, &args, scope, line).ok()?;
        if let Some(l) = label {
            callee.label = l;
        }
        let one = std::slice::from_ref(&callee);
        for a in args.iter().chain(&rest).take(MAX_ARGS) {
            let items = match self.expr(a.span) {
                Expr::Seq(v) => v.into_iter().take(MAX_ITEMS).collect(),
                _ => vec![a.span],
            };
            for it in items {
                match self.expr(it) {
                    Expr::Call { .. } => {
                        if let Some(t) = self.taskflow(out, it, scope, line, None, depth + 1) {
                            self.cross(out, line, std::slice::from_ref(&t), one);
                        }
                    }
                    Expr::Name(w) => {
                        if let Ok(Some(ts)) = self.task_name(&w, scope, line, 0) {
                            self.cross(out, line, &ts, one);
                        }
                    }
                    _ => {}
                }
            }
        }
        Some(callee)
    }

    fn cross(&self, out: &mut Graph, line: usize, l: &[Task], r: &[Task]) {
        for x in l {
            for y in r {
                if self.spend(1).is_err() {
                    return;
                }
                let (Some(f), Some(t)) = (&x.step, &y.step) else { continue };
                if out.edges.len() >= MAX_EDGES {
                    out.cut = true;
                    return;
                }
                out.edges.push(Edge {
                    line,
                    tool: Tool::Airflow,
                    from: clean(&x.label),
                    to: clean(&y.label),
                    from_step: f.clone(),
                    to_step: t.clone(),
                });
            }
        }
    }

    /// Task của một vế. `Ok(None)`: vế này không phải task (số, chuỗi), nên cả
    /// câu lệnh không phải khai cạnh.
    fn tasks(&self, span: Span, scope: usize, line: usize, depth: usize) -> Result<Option<Vec<Task>>, String> {
        self.spend(1)?;
        if depth > MAX_NEST {
            return Err(format!("biểu thức lồng quá {MAX_NEST} tầng"));
        }
        match self.expr(span) {
            Expr::Num | Expr::Str(_) | Expr::FStr | Expr::Lambda(_) => Ok(None),
            Expr::Seq(items) => {
                if items.len() > MAX_ITEMS {
                    return Err(format!("danh sách quá {MAX_ITEMS} task"));
                }
                let mut all = Vec::new();
                for it in items {
                    match self.tasks(it, scope, line, depth + 1)? {
                        Some(t) => all.extend(t),
                        None => return Ok(None),
                    }
                    if all.len() > MAX_ITEMS {
                        return Err(format!("một vế quá {MAX_ITEMS} task"));
                    }
                }
                Ok(Some(all))
            }
            Expr::Name(w) => self.task_name(&w, scope, line, depth + 1),
            Expr::Call { head, args, .. } => self.task_call(&head, &args, scope, line).map(|t| Some(vec![t])),
            _ => Err(format!("task `{}` không phải tên hay lời gọi", self.shown(span))),
        }
    }

    fn task_name(&self, w: &str, scope: usize, line: usize, depth: usize) -> Result<Option<Vec<Task>>, String> {
        self.spend(1)?;
        if w.contains('.') {
            return Err(format!("không lần được task `{}`", clean(w)));
        }
        if depth > MAX_NEST {
            return Err(format!("task `{}` lồng quá {MAX_NEST} tầng", clean(w)));
        }
        let Some((sid, def, l)) = self.lookup(w, scope, line) else {
            return Err(format!("không lần được task `{}` về chỗ tạo nó trong file", clean(w)));
        };
        let (span, with) = match def {
            Def::Param => return Err(format!("task `{}` là tham số của hàm", clean(w))),
            Def::Fn(_) => return Ok(None),
            Def::Bind(_) => return Err(format!("task `{}` nhập từ module khác", clean(w))),
            Def::Assign(s) => (*s, None),
            Def::With(s, stmt) => (*s, Some(*stmt)),
        };
        match self.expr(span) {
            Expr::Call { head, .. } if last_seg(&head) == "TaskGroup" => {
                // `with TaskGroup("g") as tg:`: cạnh tới `tg` là cạnh tới các task
                // gán trong khối.
                let members = with.and_then(|s| self.groups.get(&s)).filter(|m| !m.is_empty());
                let Some(members) = members else {
                    return Err(format!("nhóm task `{}` không có task nào gán trong khối `with`", clean(w)));
                };
                let mut all = Vec::new();
                for (m, ml, ms) in members.iter().take(MAX_ITEMS) {
                    if let Some(t) = self.task_name(m, *ms, *ml, depth + 1)? {
                        all.extend(t);
                    }
                    if all.len() > MAX_ITEMS {
                        return Err(format!("nhóm task quá {MAX_ITEMS} task"));
                    }
                }
                Ok(Some(all))
            }
            Expr::Call { head, args, .. } => {
                let mut t = self.task_call(&head, &args, sid, l)?;
                t.label = w.to_string();
                Ok(Some(vec![t]))
            }
            Expr::Name(_) | Expr::Seq(_) if depth < MAX_FOLLOW * 4 => self.tasks(span, sid, l, depth + 1),
            Expr::Num | Expr::Str(_) | Expr::FStr | Expr::Dict(_) | Expr::Lambda(_) => Ok(None),
            _ if with.is_some() => Ok(None),
            _ => Err(format!("không lần được task `{}`", clean(w))),
        }
    }

    /// Task tạo bằng một lời gọi. Operator, sensor lấy hàm từ
    /// `python_callable`; không có thì không có mã Python của repo
    /// (`BashOperator`), trừ khi lệnh shell của nó chạy script. Lời gọi khác là
    /// task TaskFlow (`extract()`, `f.override(...)()`): hàm của task là hàm
    /// được gọi.
    fn task_call(&self, head: &str, args: &[Arg], scope: usize, line: usize) -> Result<Task, String> {
        let label = format!("{head}()");
        let f = strip_task_suffix(head);
        let last = last_seg(f);
        if last.ends_with("Operator") || last.ends_with("Sensor") {
            if let Some(a) = args.iter().find(|a| a.kw.as_deref() == Some("python_callable")) {
                return Ok(Task { label, step: Some(self.resolve_expr(a.span, scope, line, 0)?) });
            }
            if let Some(a) = args.iter().find(|a| a.kw.as_deref() == Some("bash_command"))
                && let Some(cmd) = self.literal(a.span)
                && runs_script(&cmd)
            {
                return Err(format!("`{}` chạy script của repo qua lệnh shell, chưa lần về file", clean(last)));
            }
            return Ok(Task { label, step: None });
        }
        if last == "TaskGroup" {
            return Err("nhóm task không tạo bằng `with … as`".into());
        }
        let step = self.resolve_name(f, scope, line, 0)?;
        if let Step::Imported { module, .. } = &step
            && (module == "airflow" || module.starts_with("airflow."))
        {
            return Err(format!("task `{}` do `airflow` tạo lúc chạy", clean(head)));
        }
        Ok(Task { label, step: Some(step) })
    }
}

/// Lệnh shell của `BashOperator` chạy mã của repo: có từ là file `.py`,
/// `.sh`, `.js`, `.ts`, hay `-m` ngay sau `python…`.
fn runs_script(cmd: &str) -> bool {
    let words: Vec<&str> = cmd.split(|c: char| c.is_whitespace() || ";&|()<>".contains(c)).filter(|w| !w.is_empty()).collect();
    words.iter().any(|w| [".py", ".sh", ".js", ".ts"].iter().any(|e| w.ends_with(e)))
        || words.windows(2).any(|p| p[0].rsplit('/').next().is_some_and(|b| b.starts_with("python")) && p[1] == "-m")
}

/// `f.override(...)`, `f.partial(...).expand(...)`: task vẫn là `f`.
fn strip_task_suffix(head: &str) -> &str {
    let mut f = head;
    while let Some((rest, tail)) = f.rsplit_once('.')
        && matches!(tail, "override" | "partial" | "expand" | "expand_kwargs")
    {
        f = rest;
    }
    f
}

#[derive(Debug, Clone)]
struct Task {
    label: String,
    /// `None`: task không chạy hàm Python nào của repo.
    step: Option<Step>,
}

// ---- dvc ----

/// Một stage của `dvc.yaml`. Đường dẫn đã tính theo `wdir`, tương đối với gốc
/// repo; đường dẫn ra ngoài repo giữ nguyên chữ, chỉ để so khớp, không đọc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// `dvc.yaml` khai stage này.
    pub file: String,
    pub name: String,
    pub line: usize,
    /// Thư mục chạy `cmd`, tương đối với gốc; `None` khi `wdir` ra ngoài repo.
    pub dir: Option<String>,
    pub cmds: Vec<String>,
    pub deps: Vec<(String, usize)>,
    pub outs: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Default)]
pub struct Dvc {
    pub stages: Vec<Stage>,
    pub unread: Vec<Unread>,
}

/// Stage `from` ghi `path` mà stage `to` đọc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataEdge {
    pub from: usize,
    pub to: usize,
    pub path: String,
    /// Dòng của mục deps bên đọc.
    pub line: usize,
}

pub fn is_dvc_file(path: &str) -> bool {
    path.rsplit('/').next() == Some("dvc.yaml")
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Đường dẫn của mục deps, outs: tính từ `dir`, bỏ `./`, `/` cuối.
fn data_path(dir: Option<&str>, p: &str) -> String {
    let p = p.trim_end_matches('/');
    match dir.and_then(|d| super::deploy::within(d, p)) {
        Some(r) => r,
        None => p.to_string(),
    }
}

/// Đọc stage của một `dvc.yaml`. Không chạy `cmd`, không đọc file nào khác.
pub fn dvc(text: &str, file: &str) -> Dvc {
    let mut out = Dvc::default();
    let unread = |out: &mut Dvc, line: usize, why: String| out.unread.push(Unread { line, tool: Tool::Dvc, why: clean(&why) });
    let yaml = match super::yaml::parse(text) {
        Ok(y) => y,
        Err(e) => {
            unread(&mut out, e.line, format!("YAML không đọc được: {}", e.msg));
            return out;
        }
    };
    let Some(doc) = yaml.documents().next() else { return out };
    let Some(stages) = doc.get("stages") else { return out };
    if !stages.is_map() {
        unread(&mut out, stages.line(), "`stages` không phải mapping".into());
        return out;
    }
    let base = dir_of(file);
    let paths = Cell::new(0usize);
    for (name, line, node) in stages.entries() {
        if out.stages.len() >= MAX_STAGES || paths.get() >= MAX_PATHS {
            unread(&mut out, line, format!("quá {MAX_STAGES} stage hay {MAX_PATHS} đường dẫn; phần sau không xét"));
            break;
        }
        if node.get("foreach").is_some() || node.get("matrix").is_some() {
            unread(&mut out, line, format!("stage `{name}` dựng bằng `foreach`/`matrix`"));
            continue;
        }
        let templated = |s: &str| s.contains("${");
        let dir = match node.get("wdir").and_then(|w| w.as_str()) {
            Some(w) if templated(w) => {
                unread(&mut out, line, format!("`wdir` của stage `{name}` có `${{…}}`"));
                continue;
            }
            Some(w) => {
                let d = super::deploy::within(base, w.trim_end_matches('/'));
                if d.is_none() {
                    unread(&mut out, line, format!("`wdir` của stage `{name}` ra ngoài repo"));
                }
                d
            }
            None => Some(base.to_string()),
        };
        let mut cmds = Vec::new();
        match node.get("cmd") {
            Some(c) if c.is_seq() => cmds.extend(c.items().filter_map(|i| i.as_str()).map(str::to_string)),
            Some(c) => cmds.extend(c.as_str().map(str::to_string)),
            None => {}
        }
        if cmds.iter().any(|c| templated(c)) {
            unread(&mut out, line, format!("`cmd` của stage `{name}` có `${{…}}`"));
            cmds.clear();
        }
        // Mỗi đường dẫn một lần: `deps: [d, d, d, …]` lặp lại không tốn thêm (TM-62).
        let list = |out: &mut Dvc, keys: &[&str]| -> Vec<(String, usize)> {
            let mut v = Vec::new();
            let mut seen: HashSet<String> = HashSet::new();
            for key in keys {
                let Some(n) = node.get(key) else { continue };
                for item in n.items() {
                    if paths.get() >= MAX_PATHS {
                        unread(out, item.line(), format!("quá {MAX_PATHS} đường dẫn; phần sau không xét"));
                        return v;
                    }
                    // `- data/x.csv` hay `- data/x.csv: {cache: false}`.
                    let raw = item.as_str().or_else(|| item.entries().next().map(|(k, _, _)| k));
                    let Some(raw) = raw else { continue };
                    if templated(raw) {
                        unread(out, item.line(), format!("`{key}` của stage `{name}` có `${{…}}`"));
                        continue;
                    }
                    if raw.contains("://") {
                        continue;
                    }
                    let p = data_path(dir.as_deref(), raw);
                    if p.split('/').count() > MAX_SEGMENTS {
                        unread(out, item.line(), format!("`{key}` của stage `{name}` có đường dẫn quá {MAX_SEGMENTS} tầng"));
                        continue;
                    }
                    if seen.insert(p.clone()) {
                        paths.set(paths.get() + 1);
                        v.push((p, item.line()));
                    }
                }
            }
            v
        };
        let deps = list(&mut out, &["deps"]);
        let outs = list(&mut out, &["outs", "metrics", "plots"]);
        out.stages.push(Stage { file: file.to_string(), name: clean(name), line, dir, cmds, deps, outs });
    }
    out
}

/// Cạnh dữ liệu giữa các stage, của một hay nhiều `dvc.yaml` (dvc dựng một đồ
/// thị chung cho mọi `dvc.yaml` của repo): stage sau đọc đúng file, hay file
/// trong thư mục, hay thư mục chứa file mà stage trước ghi. So theo đoạn
/// đường dẫn: `data/a` không khớp `data/ab` (TM-65). Chỉ mục theo đường dẫn,
/// không so từng cặp, và có ngân sách bước (TM-62).
pub fn dvc_edges(stages: &[Stage]) -> (Vec<DataEdge>, bool) {
    // Stage ghi mỗi đường dẫn, và stage ghi vào bên dưới mỗi thư mục.
    let mut exact: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut under: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut work = 0usize;
    for (s, st) in stages.iter().enumerate() {
        for (p, _) in &st.outs {
            let e = exact.entry(p.as_str()).or_default();
            if e.last() != Some(&s) {
                e.push(s);
            }
            let mut d = p.as_str();
            while let Some((parent, _)) = d.rsplit_once('/') {
                work += 1;
                let u = under.entry(parent).or_default();
                if u.last() != Some(&s) {
                    u.push(s);
                }
                d = parent;
            }
            if work > MAX_WORK {
                return (Vec::new(), true);
            }
        }
    }
    let mut out = Vec::new();
    let mut seen: HashSet<(usize, usize, &str)> = HashSet::new();
    for (t, st) in stages.iter().enumerate() {
        for (p, line) in &st.deps {
            // Out là chính dep, hay thư mục cha của dep; hay dep là thư mục cha của out.
            let mut from: Vec<usize> = Vec::new();
            let mut d = p.as_str();
            from.extend(exact.get(d).into_iter().flatten());
            while let Some((parent, _)) = d.rsplit_once('/') {
                from.extend(exact.get(parent).into_iter().flatten());
                d = parent;
            }
            from.extend(under.get(p.as_str()).into_iter().flatten());
            work += from.len() + 1;
            if work > MAX_WORK {
                return (out, true);
            }
            for f in from {
                if f == t || !seen.insert((f, t, p.as_str())) {
                    continue;
                }
                if out.len() >= MAX_EDGES {
                    return (out, true);
                }
                out.push(DataEdge { from: f, to: t, path: p.clone(), line: *line });
            }
        }
    }
    (out, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Lang;

    fn graph(src: &str) -> Graph {
        let toks = Lang::Python.tokens(src);
        let imports = Lang::Python.imports(src);
        scan(src, &toks, &imports)
    }

    fn pairs(g: &Graph) -> Vec<(usize, &str, &str)> {
        g.edges.iter().map(|e| (e.line, e.from.as_str(), e.to.as_str())).collect()
    }

    fn lines(g: &Graph) -> Vec<usize> {
        g.unread.iter().map(|u| u.line).collect()
    }

    fn imp(module: &str, path: &[&str]) -> Step {
        Step::Imported { module: module.into(), path: path.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn langgraph_mau_dung() {
        let g = graph(
            r#"from langgraph.graph import StateGraph, START, END
from rag.retrieval import retrieve
from rag import llm
import rag.grade as gr

def route(s) -> Literal["generate", "__end__"]:
    return "generate"

builder = StateGraph(State)
builder.add_node("retrieve", retrieve)
builder.add_node("generate", llm.generate)
builder.add_node(gr.grade)
builder.add_node("local", lambda s: s)
builder.add_edge(START, "retrieve")
builder.add_edge("retrieve", "grade")
builder.add_conditional_edges("grade", route)
builder.add_conditional_edges("generate", pick, {"a": "local", "b": END})
builder.add_edge("local", END)
"#,
        );
        assert_eq!(pairs(&g), vec![(15, "retrieve", "grade"), (16, "grade", "generate"), (17, "generate", "local")]);
        assert!(g.unread.is_empty(), "{:?}", g.unread);
        assert_eq!(g.edges[0].from_step, imp("rag.retrieval", &["retrieve"]));
        assert_eq!(g.edges[0].to_step, imp("rag.grade", &["grade"]));
        assert_eq!(g.edges[1].to_step, imp("rag", &["llm", "generate"]));
        assert_eq!(g.edges[2].to_step, Step::Here);
    }

    #[test]
    fn langgraph_dung_dong_thi_bao() {
        let g = graph(
            r#"from langgraph.graph import StateGraph
from langgraph.types import Command, Send
g = StateGraph(S)
for name, fn in STEPS.items():
    g.add_node(name, fn)
g.add_edge("a", "b")
h = StateGraph(S)
h.add_node("x", make)
h.add_edge("x", "y")
h.add_conditional_edges("x", decide)
h.add_node(f"n_{i}", run)
def node(s):
    return Command(goto="x")
def fan(s):
    return [Send("x", s)]
"#,
        );
        let why: Vec<(usize, &str)> = g.unread.iter().map(|u| (u.line, u.why.as_str())).collect();
        assert!(g.edges.is_empty(), "{:?}", g.edges);
        assert!(why.iter().any(|(l, w)| *l == 5 && w.contains("không cố định")), "{why:?}");
        // Builder `g` có bước tên động: cạnh của nó không báo thêm.
        assert!(!why.iter().any(|(l, _)| *l == 6), "{why:?}");
        assert!(why.iter().any(|(l, w)| *l == 8 && w.contains("`make`")), "{why:?}");
        assert!(why.iter().any(|(l, w)| *l == 10 && w.contains("`decide`")), "{why:?}");
        assert!(why.iter().any(|(l, _)| *l == 11), "{why:?}");
        assert!(why.iter().any(|(l, w)| *l == 13 && w.contains("Command")), "{why:?}");
        assert!(why.iter().any(|(l, w)| *l == 15 && w.contains("Send")), "{why:?}");
    }

    #[test]
    fn langgraph_sequence_va_tools_condition() {
        let g = graph(
            r#"from langgraph.graph import StateGraph
from langgraph.prebuilt import tools_condition, ToolNode
from app.steps import a, b
from app.agent import agent
from app.tools import search
g = StateGraph(S).add_sequence([a, ("bee", b)])
w = StateGraph(S)
w.add_node("agent", agent)
w.add_node("tools", ToolNode([search]))
w.add_conditional_edges("agent", tools_condition)
w.add_edge(["agent", "tools"], "agent")
"#,
        );
        assert_eq!(pairs(&g), vec![(6, "a", "bee"), (10, "agent", "tools"), (11, "tools", "agent")]);
        assert!(g.unread.is_empty(), "{:?}", g.unread);
        // `ToolNode` của thư viện: hàm thật là các tool trong danh sách.
        assert_eq!(
            g.edges[1].to_step,
            Step::Call { head: Box::new(imp("langgraph.prebuilt", &["ToolNode"])), args: vec![imp("app.tools", &["search"])], lost: false }
        );
    }

    /// Hai hàm cùng dựng đồ thị vào biến `workflow`: hai đồ thị, không ghi đè
    /// nhau (review 2026-10-23).
    #[test]
    fn hai_builder_cung_ten_o_hai_ham() {
        let g = graph(
            r#"from langgraph.graph import StateGraph
from rag.retrieval import retrieve
from rag import llm, other
def build_a():
    workflow = StateGraph(S)
    workflow.add_node("retrieve", retrieve)
    workflow.add_node("generate", llm.generate)
    workflow.add_edge("retrieve", "generate")
def build_b():
    workflow = StateGraph(S)
    workflow.add_node("retrieve", llm.generate)
    workflow.add_node("generate", other.x)
"#,
        );
        assert_eq!(pairs(&g), vec![(8, "retrieve", "generate")]);
        assert_eq!(g.edges[0].from_step, imp("rag.retrieval", &["retrieve"]));
        assert_eq!(g.edges[0].to_step, imp("rag", &["llm", "generate"]));
    }

    /// Tên tra theo phạm vi và thứ tự: phương thức cùng tên trong lớp không che
    /// import; import hay phép gán sau thắng cái trước.
    #[test]
    fn ten_theo_pham_vi_va_lan_gan_gan_nhat() {
        let g = graph(
            r#"from langgraph.graph import StateGraph
from app.safe import generate
from app.llm.core import generate
from app.r import retrieve
class Helper:
    def generate(self):
        pass
    def retrieve(self):
        pass
retrieve = Helper().retrieve
g = StateGraph(S)
g.add_node("r", retrieve)
g.add_node("g", generate)
g.add_edge("r", "g")
"#,
        );
        assert_eq!(pairs(&g), vec![(14, "r", "g")]);
        assert_eq!(g.edges[0].from_step, Step::Here);
        assert_eq!(g.edges[0].to_step, imp("app.llm.core", &["generate"]));
    }

    /// Lớp bọc và `lambda`: hàm thật là hàm được bọc.
    #[test]
    fn lop_boc_lambda_va_bi_danh() {
        let g = graph(
            r#"from langgraph.graph import StateGraph
from langgraph.types import Command as Go
from langsmith import traceable
from app.llm import generate
from app.r import retrieve
g = StateGraph(S)
g.add_node("r", lambda s: retrieve(s))
g.add_node("g", traceable(generate))
g.add_edge("r", "g")
node, edge = g.add_node, g.add_edge
def step(s):
    return Go(goto="g")
def other(s):
    return Go(None, {}, None, "g")
"#,
        );
        assert_eq!(pairs(&g), vec![(9, "r", "g")]);
        assert_eq!(g.edges[0].from_step, imp("app.r", &["retrieve"]));
        assert_eq!(
            g.edges[0].to_step,
            Step::Call { head: Box::new(imp("langsmith", &["traceable"])), args: vec![imp("app.llm", &["generate"])], lost: false }
        );
        assert_eq!(lines(&g), vec![10, 10, 12, 14], "{:?}", g.unread);
    }

    #[test]
    fn airflow_toan_tu_va_operator() {
        let g = graph(
            r#"from airflow import DAG
from airflow.operators.python import PythonOperator
from airflow.operators.bash import BashOperator
from airflow.models.baseoperator import chain, cross_downstream
from itertools import chain as ichain
from etl.extract import pull
from etl import load
with DAG("d") as dag:
    t1 = PythonOperator(task_id="pull", python_callable=pull)
    t2 = PythonOperator(task_id="load", python_callable=load.run)
    t3 = BashOperator(task_id="b", bash_command="echo $SECRET_TOKEN")
    t4 = PythonOperator(task_id="x", python_callable=local_fn)
    t1 >> t2
    t3 >> t2 << t1
    t1.set_downstream([t2, t4])
    chain(t1, [t2, t4])
    ichain(t1, t2)
    n = 0x1f >> 2
    cross_downstream(to_tasks=[t1], from_tasks=[t2])
def local_fn():
    pass
"#,
        );
        assert_eq!(
            pairs(&g),
            vec![
                (13, "t1", "t2"),
                (14, "t1", "t2"),
                // `itertools.chain` gọi với task: gói ngoài, bước kiểm không có file nào để ghép.
                (17, "t1", "ichain()"),
                (17, "t2", "ichain()"),
                (15, "t1", "t2"),
                (15, "t1", "t4"),
                (16, "t1", "t2"),
                (16, "t1", "t4"),
                (19, "t2", "t1")
            ]
        );
        assert!(g.unread.is_empty(), "{:?}", g.unread);
        assert_eq!(g.edges[0].to_step, imp("etl", &["load", "run"]));
        assert_eq!(g.edges[5].to_step, Step::Here);
    }

    #[test]
    fn airflow_taskflow_nhom_task_va_cho_khong_doc_duoc() {
        let g = graph(
            r#"from airflow.decorators import dag, task
from airflow.utils.task_group import TaskGroup
from airflow.operators.python import PythonOperator
from airflow.operators.bash import BashOperator
from airflow.models.baseoperator import chain as link
from etl.steps import extract, transform, load
@dag()
def pipe():
    e = extract()
    t = transform.override(task_id="t")(e)
    e >> t
    tasks[0] >> t
    unknown >> t
    with TaskGroup("g") as tg:
        a = PythonOperator(task_id="a", python_callable=extract)
        b = PythonOperator.partial(task_id="b", python_callable=load).expand(op_args=[[1]])
    tg >> t
    load(transform(extract()))
    link(e, b)
    x = dag.get_task("a")
    x >> t
    sh = BashOperator(task_id="s", bash_command="python etl/run.py")
    sh >> t
    print(e >> t)
pipe()
"#,
        );
        assert_eq!(
            pairs(&g),
            vec![
                (10, "e", "t"),
                (11, "e", "t"),
                (17, "a", "t"),
                (17, "b", "t"),
                (18, "extract()", "transform()"),
                (18, "transform()", "load()"),
                (19, "e", "b"),
            ]
        );
        assert_eq!(g.edges[3].from_step, imp("etl.steps", &["load"]));
        assert_eq!(lines(&g), vec![12, 13, 21, 23, 24], "{:?}", g.unread);
    }

    /// Dạng mà Python đọc khác cách tách từ thô: ngoặc bọc cả câu lệnh, ngoặc
    /// quanh một tên bước, hai chuỗi viết liền, `return a >> b`, `lambda`, nối
    /// dòng bằng `\`.
    #[test]
    fn ngoac_chuoi_noi_va_return() {
        let g = graph(
            "from airflow.operators.python import PythonOperator\nfrom langgraph.graph import StateGraph\nfrom m import f, h\na = PythonOperator(task_id=\"a\", python_callable=f)\nb = PythonOperator(task_id=\"b\", python_callable=h)\n(a >> [b])\ndef wire():\n    return a >> b\nlam = lambda: 1\nlam >> b\nif a >> b:\n    pass\ns = StateGraph(dict)\ns.add_node(\"ret\" \"rieve\", f)\ns.add_node(\"gen\", h)\ns.add_edge((\"retrieve\"), \"gen\")\ns.add_edge((\"retrieve\",), \"gen\")\na \\\n    >> b\n",
        );
        assert_eq!(
            pairs(&g),
            vec![(16, "retrieve", "gen"), (17, "retrieve", "gen"), (6, "a", "b"), (8, "a", "b"), (18, "a", "b")]
        );
        assert!(g.unread.is_empty(), "{:?}", g.unread);
    }

    /// Review vòng 2 (2026-10-23), lỗi 1: thân hàm `@task` và hàm làm
    /// `python_callable` là mã chạy lúc thực thi, không nối đồ thị.
    #[test]
    fn than_task_khong_phai_cho_noi_do_thi() {
        let g = graph(
            r#"from airflow.decorators import dag, task
from airflow import DAG
from airflow.operators.python import PythonOperator
from app.r import retrieve
from app.llm import generate
@dag()
def pipe():
    @task
    def answer(q):
        return generate(retrieve(q))
    a = answer("x")
    b = generate(a)
with DAG("d"):
    def _run():
        docs = retrieve(1)
        return generate(docs)
    t = PythonOperator(task_id="t", python_callable=_run)
"#,
        );
        assert_eq!(pairs(&g), vec![(12, "a", "b")]);
    }

    /// Lỗi 2: gán lại builder trong cùng phạm vi, và builder là tham số của
    /// hàm khác, là builder khác.
    #[test]
    fn gan_lai_builder_va_builder_la_tham_so() {
        let g = graph(
            r#"from langgraph.graph import StateGraph
from app.r import retrieve, normalize
from app.llm import generate
g = StateGraph(S)
g.add_node("r", retrieve)
g.add_node("g", generate)
g.add_edge("r", "g")
app = g.compile()
g = StateGraph(S)
g.add_node("r", normalize)
def add_demo(g):
    g.add_node("r", normalize)
"#,
        );
        assert_eq!(pairs(&g), vec![(7, "r", "g")]);
        assert_eq!(g.edges[0].from_step, imp("app.r", &["retrieve"]));
    }

    /// Lỗi 3 và 5: `generate = traceable(generate)` bọc hàm của import;
    /// `ToolNode(tools)` với `tools` gán bằng danh sách; `tools` là tham số thì
    /// đánh dấu `lost`.
    #[test]
    fn boc_lai_cung_ten_va_danh_sach_qua_bien() {
        let g = graph(
            r#"from langgraph.graph import StateGraph
from langgraph.prebuilt import ToolNode
from langsmith import traceable
from app.llm import generate
from app.tools import search, fetch
generate = traceable(generate)
tools = [search, fetch]
g = StateGraph(S)
g.add_node("g", generate)
g.add_node("t", ToolNode(tools))
g.add_edge("g", "t")
def build(tools):
    h = StateGraph(S)
    h.add_node("g", generate)
    h.add_node("t", ToolNode(tools))
    h.add_edge("g", "t")
"#,
        );
        assert_eq!(pairs(&g), vec![(11, "g", "t"), (16, "g", "t")]);
        let traced = Step::Call { head: Box::new(imp("langsmith", &["traceable"])), args: vec![imp("app.llm", &["generate"])], lost: false };
        assert_eq!(g.edges[0].from_step, traced);
        assert_eq!(
            g.edges[0].to_step,
            Step::Call {
                head: Box::new(imp("langgraph.prebuilt", &["ToolNode"])),
                args: vec![imp("app.tools", &["search"]), imp("app.tools", &["fetch"])],
                lost: false
            }
        );
        assert!(matches!(&g.edges[1].to_step, Step::Call { lost: true, args, .. } if args.is_empty()), "{:?}", g.edges[1]);
    }

    /// Lỗi 4: dấu phẩy giữa tham số của `lambda` không ngăn đối số.
    #[test]
    fn lambda_nhieu_tham_so() {
        let g = graph(
            r#"from langgraph.graph import StateGraph
from airflow.operators.python import PythonOperator
from app.llm import generate
from app.r import retrieve
g = StateGraph(S)
g.add_node("r", lambda state, config: retrieve(state, config))
g.add_node("g", lambda s, c=1: generate(s))
g.add_edge("r", "g")
a = PythonOperator(task_id="a", python_callable=lambda ds, ti: retrieve(ds))
b = PythonOperator(task_id="b", python_callable=generate)
a >> b
"#,
        );
        assert_eq!(pairs(&g), vec![(8, "r", "g"), (11, "a", "b")]);
        assert_eq!(g.edges[0].from_step, imp("app.r", &["retrieve"]));
        assert_eq!(g.edges[1].from_step, imp("app.r", &["retrieve"]));
    }

    /// Lỗi 6, 10, 11: file kết thúc bằng `async` không làm sập; `;` không thuộc
    /// câu lệnh; thân `case` viết cùng dòng.
    #[test]
    fn async_cuoi_file_dau_cham_phay_va_case() {
        graph("import airflow\nasync");
        let g = graph(
            "from airflow.operators.python import PythonOperator\nfrom m import f, h\na = PythonOperator(task_id='a', python_callable=f); b = PythonOperator(task_id='b', python_callable=h); a >> b\nmatch x:\n    case 1: a >> b\nwhile x: b >> a;\n",
        );
        assert_eq!(pairs(&g), vec![(3, "a", "b"), (5, "a", "b"), (6, "b", "a")]);
        assert!(g.unread.is_empty(), "{:?}", g.unread);
    }

    #[test]
    fn file_khong_dung_thu_vien_do_thi_thi_bo_qua() {
        let g = graph("g.add_edge('a', 'b')\nx >> y\n");
        assert!(g.is_empty());
    }

    /// Không import LangGraph mà gọi `StateGraph(` nhập lại qua module của repo.
    #[test]
    fn statgraph_nhap_lai_van_doc() {
        let g = graph("from rag.lg import StateGraph\nfrom a import f, h\ng = StateGraph(S)\ng.add_node('x', f)\ng.add_node('y', h)\ng.add_edge('x', 'y')\n");
        assert_eq!(pairs(&g), vec![(6, "x", "y")]);
    }

    /// Lời báo không in nội dung chuỗi, và không mang ký tự điều khiển (TM-66).
    #[test]
    fn loi_bao_khong_in_chuoi_va_ky_tu_dieu_khien() {
        let g = graph(
            "from airflow.operators.bash import BashOperator\nfrom langgraph.graph import StateGraph\na = BashOperator(task_id='a')\na >> (BashOperator(bash_command='curl -u x:hunter2-af h') if P else a)\ng = StateGraph(S)\ng.add_node('llm', prompt | ChatOpenAI(api_key='sk-proj-SECRET'))\ng.add_node('\\x1b[2Kx', f)\n",
        );
        let all = format!("{:?}{:?}", g.unread, g.edges);
        assert!(!all.contains("hunter2") && !all.contains("SECRET"), "{all}");
        assert!(!g.unread.iter().any(|u| u.why.contains('\u{1b}')), "{:?}", g.unread);
    }

    fn timed(src: &str) -> Graph {
        let t = crate::testclock::Clock::start();
        let g = graph(src);
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "quá chậm: {:?}", t.elapsed());
        g
    }

    #[test]
    fn airflow_qua_tran_canh_thi_cat() {
        let names: Vec<String> = (0..200).map(|i| format!("a{i}")).collect();
        let mut src = String::from("from airflow.operators.python import PythonOperator\nfrom m import f\n");
        for n in &names {
            src.push_str(&format!("{n} = PythonOperator(task_id='{n}', python_callable=f)\n"));
        }
        let list = names.join(", ");
        src.push_str(&format!("[{list}] >> [{list}]\n"));
        let g = timed(&src);
        assert!(g.cut);
        assert_eq!(g.edges.len(), MAX_EDGES);
    }

    /// Đầu vào đối kháng của review 2026-10-23: danh sách lồng nổ cấp số mũ,
    /// chuỗi `L >> L >> …`, ngoặc lồng 50 000 tầng, 40 000 import.
    #[test]
    fn dau_vao_doi_khang_khong_treo_khong_sap() {
        let mut lists = String::from("from airflow.operators.python import PythonOperator\nt = PythonOperator(task_id='t', python_callable=f)\n");
        let n = 300;
        lists.push_str(&format!("c = [{}]\n", vec!["t"; n].join(",")));
        lists.push_str(&format!("b = [{}]\n", vec!["c"; n].join(",")));
        lists.push_str(&format!("a = [{}]\n", vec!["b"; n].join(",")));
        lists.push_str("a >> t\n");
        let g = timed(&lists);
        assert!(!g.unread.is_empty());

        let mut chain = String::from("from airflow.operators.bash import BashOperator\n");
        for i in 0..1000 {
            chain.push_str(&format!("b{i} = BashOperator(task_id='b{i}')\n"));
        }
        chain.push_str(&format!("L = [{}]\n", (0..1000).map(|i| format!("b{i}")).collect::<Vec<_>>().join(",")));
        chain.push_str(&format!("{}\n", vec!["L"; 20_000].join(">>")));
        let g = timed(&chain);
        assert!(g.cut);

        let deep = format!("from airflow.operators.python import PythonOperator\nt1 >> {}t1{}\n", "(".repeat(50_000), ")".repeat(50_000));
        timed(&deep);
        let deep = format!("from langgraph.graph import StateGraph\ng = StateGraph(S)\ng.add_node('x', {}f{})\n", "[".repeat(50_000), "]".repeat(50_000));
        timed(&deep);

        let mut binds = String::from("from airflow.operators.python import PythonOperator\n");
        for i in 0..40_000 {
            binds.push_str(&format!("from m{i} import a{i}\n"));
        }
        binds.push_str("t = PythonOperator(task_id='t', python_callable=a1)\n");
        binds.push_str(&"t >> t\n".repeat(40_000));
        timed(&binds);

        // Lỗi 7 của vòng 2: vế phải dài bị đọc lại ở mỗi lần tra tên.
        let mut long = String::from("from airflow.decorators import task\nfrom app import generate\nt = generate");
        long.push_str(&"()".repeat(100_000));
        long.push('\n');
        long.push_str(&"t >> t\n".repeat(8_000));
        timed(&long);
    }

    #[test]
    fn dvc_canh_qua_file() {
        let d = dvc(
            "stages:\n  prepare:\n    cmd: python src/prepare.py\n    deps: [src/prepare.py, data/raw]\n    outs:\n      - data/clean\n  train:\n    cmd: python src/train.py --token abc\n    wdir: .\n    deps:\n      - data/clean/a.csv\n      - data/cleaner.csv\n    outs:\n      - model.pkl: {cache: false}\n  eval:\n    cmd: python src/eval.py\n    deps: [model.pkl, data]\n    metrics: [eval.json]\n  loop:\n    foreach: [1, 2]\n    do: {cmd: x}\n",
            "ml/dvc.yaml",
        );
        let (edges, cut) = dvc_edges(&d.stages);
        assert!(!cut);
        let got: Vec<(&str, &str, &str, usize)> = edges
            .iter()
            .map(|e| (d.stages[e.from].name.as_str(), d.stages[e.to].name.as_str(), e.path.as_str(), e.line))
            .collect();
        assert_eq!(
            got,
            vec![("prepare", "train", "ml/data/clean/a.csv", 11), ("train", "eval", "ml/model.pkl", 17), ("prepare", "eval", "ml/data", 17)]
        );
        assert_eq!(d.unread.len(), 1);
        assert!(d.unread[0].why.contains("foreach"));
    }

    #[test]
    fn dvc_wdir_ra_ngoai_va_template() {
        let d = dvc(
            "stages:\n  a:\n    wdir: ../..\n    cmd: python x.py\n  b:\n    cmd: python ${item}.py\n    outs: [\"${out}\"]\n",
            "dvc.yaml",
        );
        assert_eq!(d.stages[0].dir, None);
        let why: Vec<&str> = d.unread.iter().map(|u| u.why.as_str()).collect();
        assert!(why[0].contains("ra ngoài"), "{why:?}");
        assert!(why.iter().any(|w| w.contains("`cmd`")), "{why:?}");
        assert!(why.iter().any(|w| w.contains("`outs`")), "{why:?}");
    }

    #[test]
    fn dvc_nhieu_stage_va_duong_dan_trung_khong_bac_hai() {
        let mut y = String::from("stages:\n");
        for i in 0..1_500 {
            y.push_str(&format!("  s{i}:\n    cmd: python s.py\n    deps: [d/x]\n    outs: [d/x/o{i}]\n"));
        }
        let t = crate::testclock::Clock::start();
        let d = dvc(&y, "dvc.yaml");
        let (edges, cut) = dvc_edges(&d.stages);
        assert!(cut && edges.len() == MAX_EDGES);
        // `outs: [d, d, …]`, `deps: [d/x, d/x, …]`: mỗi đường dẫn một lần.
        let y = format!(
            "stages:\n  a:\n    cmd: x\n    outs: [{}]\n  b:\n    cmd: y\n    deps: [{}]\n",
            vec!["d"; 49_000].join(","),
            vec!["d/x"; 49_000].join(",")
        );
        let d = dvc(&y, "dvc.yaml");
        let (edges, _) = dvc_edges(&d.stages);
        assert_eq!(edges.len(), 1);
        // Lỗi 8 của vòng 2: đường dẫn `a/a/a/…` 80 000 đoạn.
        let deep = vec!["a"; 80_000].join("/");
        let d = dvc(&format!("stages:\n  x:\n    cmd: y\n    outs: [\"{deep}\"]\n  z:\n    cmd: y\n    deps: [\"{deep}\"]\n"), "dvc.yaml");
        assert!(d.unread.iter().any(|u| u.why.contains("tầng")), "{:?}", d.unread);
        dvc_edges(&d.stages);
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
    }
}
