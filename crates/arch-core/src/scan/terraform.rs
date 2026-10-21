//! Đọc Terraform: service, cạnh và lối vào Internet từ file `.tf`, không chạy
//! lệnh, không gọi mạng.
//!
//! Mỗi thư mục có file `.tf` là một module. Module không bị module khác trong
//! repo gọi tới (`source = "./…"`) là một module gốc; mỗi tài nguyên được gọi
//! theo địa chỉ Terraform trong module gốc của nó (`aws_ecs_service.api`,
//! `module.app.aws_lambda_function.worker`).
//!
//! Đánh giá tĩnh: `variable` lấy giá trị mặc định hay đối số của module,
//! `locals` (phát hiện vòng), `module.x.y` qua `output` của module trong repo,
//! `path.module`, một số hàm (`jsonencode`, `merge`, `concat`, `format`…).
//! Tham chiếu tới tài nguyên giữ nguyên dạng ký hiệu: `aws_db_instance.main.address`
//! là một cạnh tới `aws_db_instance.main`.
//!
//! Service: ECS (`aws_ecs_service` cùng task definition, hay task definition
//! đứng riêng), Lambda, Cloud Run (v1, v2). Lối vào Internet: bộ cân bằng tải
//! AWS không `internal`, ECS có IP công khai và nhóm bảo mật mở `0.0.0.0/0`,
//! Lambda function URL không xác thực, API Gateway, Cloud Run cho `allUsers`.
//!
//! Chỗ cần một giá trị mà archctl không đánh giá được (module registry,
//! `count`/`for_each` không hằng, `dynamic`, biểu thức `for`, hàm lạ, file
//! `.tf.json`, file override) thì báo "chưa đọc được", không đoán (TM-49).
//! Biến không có giá trị mặc định là đầu vào lúc triển khai, như biến môi
//! trường chưa đặt: giá trị đó coi như không biết, không báo. Giá trị env chỉ
//! vào bộ tìm tên máy như compose; không in giá trị nào (TM-50). Trần bước, cỡ
//! giá trị, độ sâu, số module chung cho cả lần đọc (TM-48).

use super::compose;
use super::hcl::{self, Body, Block, Expr, Part, Seg, E};
use super::k8s;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Bước đánh giá tối đa cho cả lần đọc.
pub const MAX_STEPS: usize = 2_000_000;
/// Cỡ giá trị dựng ra tối đa (một node, cộng một cho mỗi 16 byte chuỗi).
pub const MAX_UNITS: usize = 1_000_000;
/// Độ sâu đánh giá (local trỏ local, module lồng module).
pub const MAX_EVAL_DEPTH: usize = 96;
/// Module lồng nhau tối đa, và số module dựng ra cho cả lần đọc.
pub const MAX_MODULE_DEPTH: usize = 16;
pub const MAX_INSTANCES: usize = 500;
/// Một chuỗi dựng ra dài tối đa: `local` tự nhân đôi dừng sớm (TM-48).
pub const MAX_STR: usize = 64 * 1024;

/// Thuộc tính mang địa chỉ mạng: tham chiếu tới chúng từ biến môi trường là
/// một cạnh, kể cả khi đích không chạy mã (cơ sở dữ liệu, cache, hàng đợi).
const NETWORK_ATTRS: &[&str] = &[
    "address",
    "endpoint",
    "reader_endpoint",
    "dns_name",
    "uri",
    "url",
    "function_url",
    "invoke_url",
    "api_endpoint",
    "primary_endpoint_address",
    "reader_endpoint_address",
    "configuration_endpoint_address",
    "connection_name",
    "private_ip_address",
    "public_ip_address",
    "host",
    "hostname",
    "fqdn",
    "port",
];

/// Loại tài nguyên mà mọi tham chiếu tới nó là cạnh (gửi vào hàng đợi, topic
/// theo URL, ARN, tên).
const MESSAGING: &[&str] = &["aws_sqs_queue", "aws_sns_topic", "aws_kinesis_stream", "google_pubsub_topic", "aws_mq_broker"];

/// Tham chiếu của từng tài nguyên: địa chỉ → (địa chỉ đích, thuộc tính, dòng).
type RefMap = HashMap<String, Vec<(String, Vec<String>, usize)>>;

/// Một service hay đích của cạnh trong Terraform.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// Thư mục module gốc (`""` là gốc repo).
    pub root: String,
    /// Địa chỉ Terraform trong module gốc.
    pub addr: String,
    pub ty: String,
    pub file: String,
    pub line: usize,
    /// Chạy mã: ECS, Lambda, Cloud Run.
    pub compute: bool,
    pub containers: Vec<k8s::Container>,
    pub edges: Vec<RefEdge>,
    /// Chỗ lộ ra Internet: file, dòng, cách hiển thị.
    pub exposed: Vec<(String, usize, String)>,
}

/// Cạnh qua tham chiếu: biến môi trường `key` ở `file:line` trỏ tới `target`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEdge {
    pub key: String,
    pub file: String,
    pub line: usize,
    pub target: String,
    /// Như cấu hình viết: `DB_HOST → aws_db_instance.main.address`.
    pub shown: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Out {
    pub units: Vec<Unit>,
    pub roots: Vec<String>,
    /// File (kèm dòng) và lý do không đọc được.
    pub unread: Vec<(String, String)>,
}

/// Đọc file trong repo (đã qua chốt chặn đường dẫn, trần cỡ, TM-51), hỏi một
/// đường dẫn có phải thư mục không.
pub struct Fs<'a> {
    pub read: &'a dyn Fn(&str) -> Result<String, String>,
    pub is_dir: &'a dyn Fn(&str) -> bool,
    /// Biến môi trường mà mã đọc làm tên topic: giữ giá trị dạng tên của
    /// chúng (xem `compose::parse_with`).
    pub env_words: &'a [String],
}

/// Vì sao một giá trị không biết.
#[derive(Debug, Clone, PartialEq)]
struct Why {
    file: String,
    line: usize,
    what: String,
    /// Đầu vào lúc triển khai (biến không có mặc định, `each`, `count`): không
    /// phải chỗ archctl không hiểu, không báo.
    soft: bool,
}

/// Giá trị đã đánh giá.
#[derive(Debug, Clone, PartialEq)]
enum V {
    Null,
    Bool(bool),
    Num(String),
    Str(Vec<P>),
    List(Vec<V>),
    /// Khoá, giá trị, dòng của giá trị.
    Obj(Vec<(String, V, usize)>),
    /// Tài nguyên (địa chỉ đầy đủ) hay một thuộc tính của nó, và dòng viết
    /// tham chiếu đó.
    Res(String, Vec<String>, usize),
    Unknown(Box<Why>),
}

#[derive(Debug, Clone, PartialEq)]
enum P {
    Text(String),
    Ref(String, Vec<String>, usize),
    Unknown(Box<Why>),
}

impl V {
    fn text(s: &str) -> V {
        V::Str(vec![P::Text(s.to_string())])
    }

    /// Chuỗi hằng (không tham chiếu, không phần không biết).
    fn known(&self) -> Option<String> {
        match self {
            V::Str(ps) => {
                let mut s = String::new();
                for p in ps {
                    match p {
                        P::Text(t) => s.push_str(t),
                        _ => return None,
                    }
                }
                Some(s)
            }
            V::Num(n) => Some(n.clone()),
            V::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }

    fn get(&self, key: &str) -> Option<&V> {
        match self {
            V::Obj(es) => es.iter().rev().find(|(k, _, _)| k == key).map(|(_, v, _)| v),
            _ => None,
        }
    }

    /// Dòng của giá trị ở khoá `key`.
    fn line_of(&self, key: &str) -> Option<usize> {
        match self {
            V::Obj(es) => es.iter().rev().find(|(k, _, _)| k == key).map(|(_, _, l)| *l),
            _ => None,
        }
    }

    fn items(&self) -> &[V] {
        match self {
            V::List(v) => v,
            _ => &[],
        }
    }

    /// Mọi tham chiếu tài nguyên trong giá trị: địa chỉ, thuộc tính, dòng.
    fn refs(&self, out: &mut Vec<(String, Vec<String>, usize)>) {
        match self {
            V::Res(a, p, l) => out.push((a.clone(), p.clone(), *l)),
            V::Str(ps) => {
                for p in ps {
                    if let P::Ref(a, path, l) = p {
                        out.push((a.clone(), path.clone(), *l));
                    }
                }
            }
            V::List(v) => v.iter().for_each(|x| x.refs(out)),
            V::Obj(es) => es.iter().for_each(|(_, x, _)| x.refs(out)),
            _ => {}
        }
    }

    /// Phần không biết đầu tiên mà archctl không hiểu (không phải đầu vào).
    fn hard(&self) -> Option<&Why> {
        match self {
            V::Unknown(w) if !w.soft => Some(w),
            V::Str(ps) => ps.iter().find_map(|p| match p {
                P::Unknown(w) if !w.soft => Some(&**w),
                _ => None,
            }),
            V::List(v) => v.iter().find_map(V::hard),
            V::Obj(es) => es.iter().find_map(|(_, x, _)| x.hard()),
            _ => None,
        }
    }

    fn size(&self) -> usize {
        1 + match self {
            V::Str(ps) => ps
                .iter()
                .map(|p| match p {
                    P::Text(t) => t.len() / 16,
                    _ => 1,
                })
                .sum(),
            V::List(v) => v.iter().map(V::size).sum(),
            V::Obj(es) => es.iter().map(|(k, x, _)| k.len() / 16 + x.size()).sum(),
            _ => 0,
        }
    }
}

/// Một tài nguyên khai trong một module.
#[derive(Debug)]
struct ResDecl {
    ty: String,
    name: String,
    file: usize,
    line: usize,
    body: Rc<Body>,
    /// `count`/`for_each`: biểu thức, nếu có.
    each: Option<(String, Expr)>,
}

#[derive(Debug, Default)]
struct Module {
    dir: String,
    files: Vec<String>,
    vars: HashMap<String, (usize, Option<Expr>)>,
    locals: HashMap<String, (usize, Expr)>,
    outputs: HashMap<String, (usize, Expr)>,
    resources: Vec<ResDecl>,
    /// `data "<loại>" "<tên>"`: file, dòng.
    data: HashMap<String, (usize, usize)>,
    /// `module "x" { source = … }`: tên, file, dòng, thân.
    calls: Vec<(String, usize, usize, Rc<Body>)>,
}

/// Một lần dựng module: module nào, tiền tố địa chỉ, đối số.
struct Inst {
    module: Rc<Module>,
    prefix: String,
    root: String,
    args: HashMap<String, V>,
    children: HashMap<String, usize>,
}

struct Cx<'a> {
    fs: &'a Fs<'a>,
    modules: HashMap<String, Option<Rc<Module>>>,
    tf_in: &'a dyn Fn(&str) -> Vec<String>,
    insts: Vec<Inst>,
    memo: HashMap<(usize, String), V>,
    busy: HashSet<(usize, String)>,
    steps: usize,
    units: usize,
    out: Out,
    reported: HashSet<(String, String)>,
}

/// Đọc mọi file Terraform trong `files` (đường dẫn trong repo, đã lọc `.tf`).
pub fn load(files: &[String], fs: &Fs<'_>) -> Out {
    let mut by_dir: HashMap<String, Vec<String>> = HashMap::new();
    let mut out = Out::default();
    for f in files {
        let name = f.rsplit('/').next().unwrap_or(f);
        if name.ends_with(".tf.json") {
            out.unread.push((f.clone(), "Terraform dạng JSON (`.tf.json`) chưa được đọc".into()));
            continue;
        }
        if !name.ends_with(".tf") {
            continue;
        }
        if name == "override.tf" || name.ends_with("_override.tf") {
            out.unread.push((f.clone(), "file override của Terraform chưa được áp".into()));
            continue;
        }
        by_dir.entry(dir_of(f).to_string()).or_default().push(f.clone());
    }
    for v in by_dir.values_mut() {
        v.sort();
    }
    let lookup = move |d: &str| -> Vec<String> { by_dir.get(d).cloned().unwrap_or_default() };
    let mut cx = Cx {
        fs,
        modules: HashMap::new(),
        tf_in: &lookup,
        insts: Vec::new(),
        memo: HashMap::new(),
        busy: HashSet::new(),
        steps: 0,
        units: 0,
        out,
        reported: HashSet::new(),
    };
    // Thư mục có `.tf`: đọc trước để biết module nào được gọi tới.
    let mut dirs: Vec<String> = files.iter().filter(|f| f.ends_with(".tf")).map(|f| dir_of(f).to_string()).collect();
    dirs.sort();
    dirs.dedup();
    let mut called: HashSet<String> = HashSet::new();
    for d in &dirs {
        let Some(m) = cx.module(d) else { continue };
        for (_, file, _, body) in &m.calls {
            if let Some(src) = body.attr("source").and_then(|a| a.expr.literal())
                && let Some(p) = local_source(&m.dir, &src)
            {
                called.insert(p);
            }
            let _ = file;
        }
    }
    let roots: Vec<String> = dirs.into_iter().filter(|d| !called.contains(d) && cx.modules.get(d).is_some_and(Option::is_some)).collect();
    for r in &roots {
        if cx.insts.len() >= MAX_INSTANCES {
            cx.unread(r.clone(), format!("quá {MAX_INSTANCES} module cho cả repo; module gốc này không đọc"));
            continue;
        }
        let Some(m) = cx.module(r) else { continue };
        let root = cx.instantiate(m, String::new(), r.clone(), HashMap::new(), 0, &mut Vec::new());
        if let Some(i) = root {
            cx.collect(i);
        }
    }
    cx.out.roots = roots;
    cx.out
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// `source` của module trong repo: `./x`, `../x`. Module registry, git, URL
/// thì `None`.
fn local_source(dir: &str, src: &str) -> Option<String> {
    if !(src.starts_with("./") || src.starts_with("../")) {
        return None;
    }
    super::deploy::within(dir, src)
}

impl Cx<'_> {
    fn unread(&mut self, file: String, why: String) {
        if self.reported.insert((file.clone(), why.clone())) {
            self.out.unread.push((file, why));
        }
    }

    fn step(&mut self, n: usize) -> bool {
        self.steps += n;
        self.steps <= MAX_STEPS
    }

    /// Đọc module ở `dir` (mọi `.tf` trong thư mục), nhớ kết quả.
    fn module(&mut self, dir: &str) -> Option<Rc<Module>> {
        if let Some(m) = self.modules.get(dir) {
            return m.clone();
        }
        let mut m = Module { dir: dir.to_string(), ..Default::default() };
        let mut ok = false;
        for f in (self.tf_in)(dir) {
            let text = match (self.fs.read)(&f) {
                Ok(t) => t,
                Err(why) => {
                    self.unread(f.clone(), why);
                    continue;
                }
            };
            let body = match hcl::parse(&text) {
                Ok(b) => b,
                Err(e) => {
                    self.unread(format!("{f}:{}", e.line), format!("HCL: {}", e.msg));
                    continue;
                }
            };
            ok = true;
            let fid = m.files.len();
            m.files.push(f.clone());
            for item in body.items {
                let hcl::Item::Block(b) = item else { continue };
                add_block(&mut m, fid, b);
            }
        }
        let m = ok.then(|| Rc::new(m));
        self.modules.insert(dir.to_string(), m.clone());
        m
    }

    /// Dựng một module: đối số đã đánh giá ở module gọi. Module con trong repo
    /// dựng theo; module registry báo "chưa đọc được".
    fn instantiate(&mut self, m: Rc<Module>, prefix: String, root: String, args: HashMap<String, V>, depth: usize, stack: &mut Vec<String>) -> Option<usize> {
        if depth > MAX_MODULE_DEPTH || stack.contains(&m.dir) {
            let why = if depth > MAX_MODULE_DEPTH { format!("module lồng sâu quá {MAX_MODULE_DEPTH} tầng") } else { "module gọi vòng".to_string() };
            self.unread(m.files.first().cloned().unwrap_or_else(|| m.dir.clone()), why);
            return None;
        }
        if self.insts.len() >= MAX_INSTANCES {
            self.unread(m.files.first().cloned().unwrap_or_default(), format!("quá {MAX_INSTANCES} module cho cả repo"));
            return None;
        }
        let idx = self.insts.len();
        self.insts.push(Inst { module: m.clone(), prefix: prefix.clone(), root: root.clone(), args, children: HashMap::new() });
        stack.push(m.dir.clone());
        for (name, fid, line, body) in &m.calls {
            let file = &m.files[*fid];
            let at = format!("{file}:{line}");
            if body.attr("count").is_some() || body.attr("for_each").is_some() {
                self.unread(at, format!("module `{name}` dùng `count`/`for_each` chưa được đọc"));
                continue;
            }
            let src = body.attr("source").and_then(|a| a.expr.literal()).unwrap_or_default();
            let Some(dir) = local_source(&m.dir, &src) else {
                // Chỉ phần trước `?`: query của nguồn S3, HTTP có thể mang khoá (TM-50).
                let shown = super::deploy::shown_path(src.split('?').next().unwrap_or(""));
                self.unread(at, format!("module `{name}` từ registry hay URL (`{shown}`) không tải"));
                continue;
            };
            if !(self.fs.is_dir)(&dir) {
                self.unread(at, format!("module `{name}`: thư mục `{}` không có", super::deploy::shown_path(&src)));
                continue;
            }
            let Some(child) = self.module(&dir) else {
                self.unread(at, format!("module `{name}`: không có file `.tf` đọc được"));
                continue;
            };
            let mut cargs = HashMap::new();
            for item in &body.items {
                if let hcl::Item::Attr(a) = item
                    && !matches!(a.name.as_str(), "source" | "version" | "providers" | "depends_on")
                {
                    let v = self.eval(idx, *fid, &a.expr, 0);
                    cargs.insert(a.name.clone(), v);
                }
            }
            let cprefix = format!("{prefix}module.{name}.");
            if let Some(ci) = self.instantiate(child, cprefix, root.clone(), cargs, depth + 1, stack) {
                self.insts[idx].children.insert(name.clone(), ci);
            }
        }
        stack.pop();
        Some(idx)
    }

    fn soft(&self, inst: usize, fid: usize, line: usize, what: impl Into<String>) -> V {
        let m = &self.insts[inst].module;
        V::Unknown(Box::new(Why { file: m.files[fid].clone(), line, what: what.into(), soft: true }))
    }

    fn hard(&self, inst: usize, fid: usize, line: usize, what: impl Into<String>) -> V {
        let m = &self.insts[inst].module;
        V::Unknown(Box::new(Why { file: m.files[fid].clone(), line, what: what.into(), soft: false }))
    }

    fn eval(&mut self, inst: usize, fid: usize, e: &Expr, depth: usize) -> V {
        if depth > MAX_EVAL_DEPTH {
            return self.hard(inst, fid, e.line, format!("đánh giá lồng sâu quá {MAX_EVAL_DEPTH} tầng"));
        }
        if !self.step(1) || self.units > MAX_UNITS {
            return self.hard(inst, fid, e.line, format!("quá {MAX_STEPS} bước hay {MAX_UNITS} đơn vị giá trị đọc Terraform cho cả repo"));
        }
        let v = match &e.e {
            E::Null => V::Null,
            E::Bool(b) => V::Bool(*b),
            E::Num(n) => V::Num(n.clone()),
            E::Template(parts) => {
                let mut ps = Vec::new();
                for p in parts {
                    match p {
                        Part::Lit(t) => self.splice(&mut ps, V::text(t), inst, fid, e.line),
                        Part::Interp(x) => {
                            let v = self.eval(inst, fid, x, depth + 1);
                            self.splice(&mut ps, v, inst, fid, x.line);
                        }
                    }
                }
                let len: usize = ps
                    .iter()
                    .map(|p| match p {
                        P::Text(t) => t.len(),
                        _ => 1,
                    })
                    .sum();
                if len > MAX_STR {
                    return self.hard(inst, fid, e.line, format!("chuỗi dài quá {} KB", MAX_STR >> 10));
                }
                V::Str(ps)
            }
            E::Ref(root, segs) => self.reference(inst, fid, e.line, root, segs, depth),
            E::Call(name, args, expand) => self.call(inst, fid, e.line, (name, args, *expand), depth),
            E::Tuple(items) => V::List(items.iter().map(|x| self.eval(inst, fid, x, depth + 1)).collect()),
            E::Object(items) => {
                let mut es = Vec::new();
                for (k, x) in items {
                    let key = match self.eval(inst, fid, k, depth + 1).known() {
                        Some(k) => k,
                        None => return self.hard(inst, fid, k.line, "khoá object không đánh giá được"),
                    };
                    es.push((key, self.eval(inst, fid, x, depth + 1), x.line));
                }
                V::Obj(es)
            }
            E::Cond(c, a, b) => match self.eval(inst, fid, c, depth + 1) {
                V::Bool(true) => self.eval(inst, fid, a, depth + 1),
                V::Bool(false) => self.eval(inst, fid, b, depth + 1),
                V::Unknown(w) if w.soft => {
                    // Điều kiện là đầu vào: nhánh nào cũng có thể được triển
                    // khai, nên giữ tham chiếu của cả hai (cạnh nào cũng cần
                    // `link`), không bỏ cả hai.
                    let (x, y) = (self.eval(inst, fid, a, depth + 1), self.eval(inst, fid, b, depth + 1));
                    join(x, y, &w)
                }
                V::Unknown(w) => V::Unknown(w),
                _ => self.hard(inst, fid, e.line, "điều kiện không phải bool"),
            },
            E::Op(op, xs) => {
                let vs: Vec<V> = xs.iter().map(|x| self.eval(inst, fid, x, depth + 1)).collect();
                self.op(inst, fid, e.line, op, vs)
            }
            E::Get(base, segs) => {
                let b = self.eval(inst, fid, base, depth + 1);
                self.walk(inst, fid, e.line, b, segs, depth)
            }
            E::Unsupported(what) => self.hard(inst, fid, e.line, *what),
        };
        self.units += v.size();
        v
    }

    /// Ghép giá trị vào chuỗi đang dựng.
    fn splice(&self, ps: &mut Vec<P>, v: V, inst: usize, fid: usize, line: usize) {
        let push = |ps: &mut Vec<P>, p: P| match (ps.last_mut(), p) {
            // Đoạn chữ liền nhau gộp một: giá trị không phình theo số đoạn.
            (Some(P::Text(a)), P::Text(b)) => a.push_str(&b),
            (_, p) => ps.push(p),
        };
        match v {
            V::Str(more) => more.into_iter().for_each(|p| push(ps, p)),
            V::Num(n) => push(ps, P::Text(n)),
            V::Bool(b) => push(ps, P::Text(b.to_string())),
            V::Null => {}
            V::Res(a, p, l) => ps.push(P::Ref(a, p, l)),
            V::Unknown(w) => ps.push(P::Unknown(w)),
            V::List(_) | V::Obj(_) => {
                if let V::Unknown(w) = self.hard(inst, fid, line, "list hay object trong `${}`") {
                    ps.push(P::Unknown(w));
                }
            }
        }
    }

    fn op(&self, inst: usize, fid: usize, line: usize, op: &str, vs: Vec<V>) -> V {
        if let Some(w) = vs.iter().find_map(|v| match v {
            V::Unknown(w) => Some(w.clone()),
            _ => None,
        }) {
            return V::Unknown(w);
        }
        match (op, vs.as_slice()) {
            ("!", [V::Bool(b)]) => V::Bool(!b),
            ("==", [a, b]) | ("!=", [a, b]) => match (a.known(), b.known(), a, b) {
                (Some(x), Some(y), _, _) => V::Bool((x == y) == (op == "==")),
                (_, _, V::Null, V::Null) => V::Bool(op == "=="),
                (_, _, V::Null, _) | (_, _, _, V::Null) => V::Bool(op == "!="),
                _ => self.soft(inst, fid, line, "so sánh giá trị chưa biết"),
            },
            ("&&", [V::Bool(a), V::Bool(b)]) => V::Bool(*a && *b),
            ("||", [V::Bool(a), V::Bool(b)]) => V::Bool(*a || *b),
            _ => {
                let nums: Option<Vec<f64>> = vs.iter().map(|v| v.known().and_then(|s| s.parse::<f64>().ok())).collect();
                match (op, nums.as_deref()) {
                    ("-", Some([a])) => V::Num(fmt_num(-a)),
                    ("+", Some([a, b])) => V::Num(fmt_num(a + b)),
                    ("-", Some([a, b])) => V::Num(fmt_num(a - b)),
                    ("*", Some([a, b])) => V::Num(fmt_num(a * b)),
                    ("<", Some([a, b])) => V::Bool(a < b),
                    (">", Some([a, b])) => V::Bool(a > b),
                    ("<=", Some([a, b])) => V::Bool(a <= b),
                    (">=", Some([a, b])) => V::Bool(a >= b),
                    _ => self.soft(inst, fid, line, format!("toán tử `{op}` trên giá trị chưa biết")),
                }
            }
        }
    }

    fn reference(&mut self, inst: usize, fid: usize, line: usize, root: &str, segs: &[Seg], depth: usize) -> V {
        let m = self.insts[inst].module.clone();
        let attr = |i: usize| match segs.get(i) {
            Some(Seg::Attr(a)) => Some(a.as_str()),
            _ => None,
        };
        match root {
            "var" => {
                let Some(name) = attr(0) else { return self.hard(inst, fid, line, "`var` không có tên") };
                let v = if let Some(v) = self.insts[inst].args.get(name).cloned() {
                    v
                } else {
                    match m.vars.get(name) {
                        Some((vf, Some(d))) => {
                            let (vf, d) = (*vf, d.clone());
                            self.eval(inst, vf, &d, depth + 1)
                        }
                        Some((_, None)) => self.soft(inst, fid, line, format!("biến `{name}` không có giá trị mặc định")),
                        None => self.hard(inst, fid, line, format!("biến `{name}` không khai")),
                    }
                };
                self.walk(inst, fid, line, v, &segs[1..], depth)
            }
            "local" => {
                let Some(name) = attr(0) else { return self.hard(inst, fid, line, "`local` không có tên") };
                let key = (inst, name.to_string());
                let v = if let Some(v) = self.memo.get(&key) {
                    v.clone()
                } else if self.busy.contains(&key) {
                    self.hard(inst, fid, line, format!("`local.{name}` tham chiếu vòng"))
                } else {
                    match m.locals.get(name) {
                        Some((lf, x)) => {
                            let (lf, x) = (*lf, x.clone());
                            self.busy.insert(key.clone());
                            let v = self.eval(inst, lf, &x, depth + 1);
                            self.busy.remove(&key);
                            self.memo.insert(key, v.clone());
                            v
                        }
                        None => self.hard(inst, fid, line, format!("`local.{name}` không khai")),
                    }
                };
                self.walk(inst, fid, line, v, &segs[1..], depth)
            }
            "module" => {
                let (Some(name), Some(out)) = (attr(0), attr(1)) else {
                    return self.hard(inst, fid, line, "`module` cần tên và output");
                };
                let Some(&ci) = self.insts[inst].children.get(name) else {
                    return self.hard(inst, fid, line, format!("module `{name}` không đọc được"));
                };
                let cm = self.insts[ci].module.clone();
                let v = match cm.outputs.get(out) {
                    Some((of, x)) => {
                        let (of, x) = (*of, x.clone());
                        self.eval(ci, of, &x, depth + 1)
                    }
                    None => self.hard(inst, fid, line, format!("module `{name}` không có output `{out}`")),
                };
                self.walk(inst, fid, line, v, &segs[2..], depth)
            }
            "path" => match attr(0) {
                Some("module") => V::text(&m.dir_or_dot()),
                Some("root") | Some("cwd") => V::text(&dot(&self.insts[inst].root)),
                _ => self.hard(inst, fid, line, "`path` lạ"),
            },
            "terraform" => self.soft(inst, fid, line, "`terraform.workspace`"),
            "each" | "count" | "self" => self.soft(inst, fid, line, format!("`{root}` chỉ biết lúc plan")),
            "data" => {
                let (Some(t), Some(n)) = (attr(0), attr(1)) else {
                    return self.hard(inst, fid, line, "`data` cần loại và tên");
                };
                let addr = format!("{}data.{t}.{n}", self.insts[inst].prefix);
                self.res_path(inst, fid, line, addr, &segs[2..])
            }
            ty => {
                let Some(n) = attr(0) else {
                    return self.hard(inst, fid, line, format!("`{ty}` không phải tham chiếu đọc được"));
                };
                if !m.resources.iter().any(|r| r.ty == ty && r.name == n) {
                    return self.hard(inst, fid, line, format!("`{ty}.{n}` không khai trong module"));
                }
                let addr = format!("{}{ty}.{n}", self.insts[inst].prefix);
                self.res_path(inst, fid, line, addr, &segs[1..])
            }
        }
    }

    /// Tham chiếu tới tài nguyên: giữ dạng ký hiệu, chỉ lấy các bước là tên
    /// thuộc tính (chỉ số `[0]` của `count` bỏ qua).
    fn res_path(&self, _inst: usize, _fid: usize, line: usize, addr: String, segs: &[Seg]) -> V {
        let path = segs
            .iter()
            .filter_map(|s| match s {
                Seg::Attr(a) => Some(a.clone()),
                _ => None,
            })
            .collect();
        V::Res(addr, path, line)
    }

    /// Bước truy cập trên một giá trị đã biết.
    fn walk(&mut self, inst: usize, fid: usize, line: usize, mut v: V, segs: &[Seg], depth: usize) -> V {
        for s in segs {
            v = match (v, s) {
                (V::Res(a, mut p, l), Seg::Attr(x)) => {
                    p.push(x.clone());
                    V::Res(a, p, l)
                }
                (V::Res(a, p, l), _) => V::Res(a, p, l),
                (u @ V::Unknown(_), _) => u,
                (V::Obj(es), Seg::Attr(k)) => match es.into_iter().rev().find(|(x, _, _)| x == k) {
                    Some((_, x, _)) => x,
                    None => return self.hard(inst, fid, line, format!("object không có khoá `{k}`")),
                },
                (V::Obj(es), Seg::Index(i)) => {
                    let k = self.eval(inst, fid, i, depth + 1);
                    match k.known() {
                        Some(k) => match es.into_iter().rev().find(|(x, _, _)| *x == k) {
                            Some((_, x, _)) => x,
                            None => return self.hard(inst, fid, line, format!("object không có khoá `{k}`")),
                        },
                        None => return k,
                    }
                }
                (V::List(items), Seg::Index(i)) => {
                    let k = self.eval(inst, fid, i, depth + 1);
                    match k.known().and_then(|s| s.parse::<usize>().ok()) {
                        Some(n) => match items.into_iter().nth(n) {
                            Some(x) => x,
                            None => return self.hard(inst, fid, line, "chỉ số vượt độ dài list"),
                        },
                        None => return self.soft(inst, fid, line, "chỉ số list chưa biết"),
                    }
                }
                _ => return self.hard(inst, fid, line, "truy cập giá trị không đọc được"),
            };
        }
        v
    }

    fn call(&mut self, inst: usize, fid: usize, line: usize, (name, args, expand): (&str, &[Expr], bool), depth: usize) -> V {
        if expand {
            return self.hard(inst, fid, line, format!("`{name}(… ...)` chưa được đọc"));
        }
        let mut vs: Vec<V> = args.iter().map(|a| self.eval(inst, fid, a, depth + 1)).collect();
        let first_unknown = |vs: &[V]| {
            vs.iter().find_map(|v| match v {
                V::Unknown(w) => Some(V::Unknown(w.clone())),
                _ => None,
            })
        };
        match (name, vs.len()) {
            // Mã hoá giữ nguyên cấu trúc: người đọc (container_definitions) cần nó.
            ("jsonencode" | "yamlencode" | "tolist" | "toset" | "tomap" | "sensitive" | "nonsensitive", 1) => vs.remove(0),
            ("tostring" | "trimspace", 1) => vs.remove(0),
            ("lower" | "upper", 1) => match vs[0].known() {
                Some(s) => V::text(&if name == "lower" { s.to_lowercase() } else { s.to_uppercase() }),
                None => first_unknown(&vs).unwrap_or_else(|| self.soft(inst, fid, line, format!("`{name}` của giá trị chưa biết"))),
            },
            // Đối số là đầu vào chưa biết (`var.extra_env`) thì bỏ qua nó nhưng
            // giữ phần đã biết: không để một biến làm mất cạnh của các khoá khác.
            ("concat", _) => {
                if let Some(u) = vs.iter().find(|v| matches!(v, V::Unknown(w) if !w.soft)) {
                    return u.clone();
                }
                V::List(vs.into_iter().flat_map(|v| v.items().to_vec()).collect())
            }
            ("merge", _) => {
                if let Some(u) = vs.iter().find(|v| matches!(v, V::Unknown(w) if !w.soft)) {
                    return u.clone();
                }
                // Khoá trùng: giá trị sau thắng, như Terraform.
                let mut es: Vec<(String, V, usize)> = Vec::new();
                for v in vs {
                    if let V::Obj(more) = v {
                        for (k, x, l) in more {
                            es.retain(|(e, _, _)| *e != k);
                            es.push((k, x, l));
                        }
                    }
                }
                V::Obj(es)
            }
            ("lookup", 2 | 3) => match (&vs[0], vs[1].known()) {
                (V::Obj(_), Some(k)) => match vs[0].get(&k).cloned() {
                    Some(v) => v,
                    None if vs.len() == 3 => vs.remove(2),
                    None => self.hard(inst, fid, line, format!("`lookup` không có khoá `{k}`")),
                },
                _ => first_unknown(&vs).unwrap_or_else(|| self.soft(inst, fid, line, "`lookup` chưa biết")),
            },
            ("coalesce" | "try", _) => vs
                .iter()
                .find(|v| !matches!(v, V::Null | V::Unknown(_)) && v.known().is_none_or(|s| !s.is_empty()))
                .cloned()
                .or_else(|| first_unknown(&vs))
                .unwrap_or(V::Null),
            ("join", 2) => match (vs[0].known(), &vs[1]) {
                (Some(sep), V::List(items)) => {
                    let mut ps = Vec::new();
                    for (i, it) in items.clone().into_iter().enumerate() {
                        if i > 0 {
                            ps.push(P::Text(sep.clone()));
                        }
                        self.splice(&mut ps, it, inst, fid, line);
                    }
                    V::Str(ps)
                }
                _ => first_unknown(&vs).unwrap_or_else(|| self.soft(inst, fid, line, "`join` chưa biết")),
            },
            ("format", n) if n >= 1 => match vs[0].known() {
                Some(f) => {
                    let mut ps = Vec::new();
                    let mut rest = vs[1..].iter().cloned();
                    let mut chars = f.chars().peekable();
                    let mut lit = String::new();
                    while let Some(c) = chars.next() {
                        if c != '%' {
                            lit.push(c);
                            continue;
                        }
                        match chars.next() {
                            Some('%') => lit.push('%'),
                            Some('s' | 'd' | 'v') => {
                                ps.push(P::Text(std::mem::take(&mut lit)));
                                match rest.next() {
                                    Some(v) => self.splice(&mut ps, v, inst, fid, line),
                                    None => return self.hard(inst, fid, line, "`format` thiếu đối số"),
                                }
                            }
                            _ => return self.hard(inst, fid, line, "`format` với định dạng chưa đọc được"),
                        }
                    }
                    ps.push(P::Text(lit));
                    V::Str(ps)
                }
                None => first_unknown(&vs).unwrap_or_else(|| self.soft(inst, fid, line, "`format` chưa biết")),
            },
            ("element", 2) => match (&vs[0], vs[1].known().and_then(|s| s.parse::<usize>().ok())) {
                (V::List(items), Some(i)) if !items.is_empty() => items[i % items.len()].clone(),
                _ => first_unknown(&vs).unwrap_or_else(|| self.soft(inst, fid, line, "`element` chưa biết")),
            },
            ("file" | "templatefile" | "filebase64", _) => self.hard(inst, fid, line, format!("hàm `{name}` (đọc file) chưa được đọc")),
            _ => self.hard(inst, fid, line, format!("hàm `{name}` chưa được đọc")),
        }
    }

    /// Đánh giá một thuộc tính của tài nguyên.
    fn attr(&mut self, inst: usize, fid: usize, body: &Body, name: &str) -> Option<(V, usize)> {
        let a = body.attr(name)?;
        let (x, line) = (a.expr.clone(), a.line);
        Some((self.eval(inst, fid, &x, 0), line))
    }

    /// Thu service, cạnh, lối vào từ một module đã dựng và các module con.
    fn collect(&mut self, root_inst: usize) {
        let mut all: Vec<usize> = vec![root_inst];
        let mut k = 0;
        while k < all.len() {
            let children: Vec<usize> = self.insts[all[k]].children.values().copied().collect();
            all.extend(children);
            k += 1;
        }
        let root = self.insts[root_inst].root.clone();
        // Mọi tài nguyên của mọi module trong cây, với thân đã có.
        let mut decls: Vec<(usize, usize)> = Vec::new();
        for &i in &all {
            for (r, d) in self.insts[i].module.resources.iter().enumerate() {
                // `count = 0`: tài nguyên không được tạo, không nối gì.
                if matches!(&d.each, Some((k, Expr { e: E::Num(n), .. })) if k == "count" && n == "0") {
                    continue;
                }
                decls.push((i, r));
            }
        }
        let addr_of = |cx: &Cx<'_>, i: usize, r: usize| {
            let d = &cx.insts[i].module.resources[r];
            format!("{}{}.{}", cx.insts[i].prefix, d.ty, d.name)
        };
        // Giá trị của các thuộc tính hay dùng để nối tài nguyên, theo địa chỉ.
        let mut refs_of: RefMap = HashMap::new();
        let mut by_addr: HashMap<String, (usize, usize)> = HashMap::new();
        for &(i, r) in &decls {
            let a = addr_of(self, i, r);
            by_addr.insert(a.clone(), (i, r));
            let d = &self.insts[i].module.resources[r];
            let (body, fid) = (d.body.clone(), d.file);
            let mut out = Vec::new();
            for key in [
                "task_definition", "load_balancer_arn", "target_group_arn", "listener_arn", "function_name", "api_id",
                "registry_arn", "name", "service", "location", "integration_uri", "uri", "security_group_id",
            ] {
                if let Some((v, _)) = self.attr(i, fid, &body, key) {
                    v.refs(&mut out);
                }
            }
            for b in body.items.iter().filter_map(|it| match it {
                hcl::Item::Block(b) => Some(b),
                _ => None,
            }) {
                for key in ["target_group_arn", "registry_arn"] {
                    if let Some((v, _)) = self.attr(i, fid, &b.body, key) {
                        v.refs(&mut out);
                    }
                }
                for inner in b.body.blocks("forward").chain(b.body.blocks("target_group")) {
                    if let Some((v, _)) = self.attr(i, fid, &inner.body, "arn") {
                        v.refs(&mut out);
                    }
                }
            }
            refs_of.insert(a, out);
        }
        // Chỉ mục ngược: đích → tài nguyên trỏ tới nó. Tra "ai trỏ tới X" một
        // lần, không quét mọi tài nguyên cho mỗi service (TM-48).
        let back = back_index(&refs_of);
        let points_to = |refs_of: &RefMap, from: &str, ty: &str| -> Vec<String> {
            refs_of.get(from).into_iter().flatten().filter(|(a, _, _)| type_of(a) == ty).map(|(a, _, _)| a.clone()).collect()
        };
        let referencing = |_: &RefMap, ty: &str, to: &str| -> Vec<String> { referencing_in(&back, ty, to) };
        // Service chạy mã: địa chỉ → (module, tài nguyên chứa container, tài nguyên được gọi tên).
        let mut units: Vec<Unit> = Vec::new();
        let mut unit_of: HashMap<String, Vec<String>> = HashMap::new();
        let mut task_used: HashSet<String> = HashSet::new();
        let mut sorted: Vec<(usize, usize)> = decls.clone();
        sorted.sort_by_key(|&(i, r)| addr_of(self, i, r));
        for &(i, r) in &sorted {
            let d = &self.insts[i].module.resources[r];
            if d.ty != "aws_ecs_service" {
                continue;
            }
            let a = addr_of(self, i, r);
            for td in points_to(&refs_of, &a, "aws_ecs_task_definition") {
                task_used.insert(td.clone());
                if let Some(&(ti, tr)) = by_addr.get(&td)
                    && let Some(u) = self.compute_unit(&root, &a, (i, r), (ti, tr))
                {
                    unit_of.entry(td.clone()).or_default().push(a.clone());
                    unit_of.entry(a.clone()).or_default().push(a.clone());
                    units.push(u);
                }
            }
        }
        for &(i, r) in &sorted {
            let d = &self.insts[i].module.resources[r];
            let a = addr_of(self, i, r);
            let standalone = match d.ty.as_str() {
                "aws_ecs_task_definition" => !task_used.contains(&a),
                "aws_lambda_function" | "google_cloud_run_v2_service" | "google_cloud_run_service" => true,
                _ => false,
            };
            if standalone && let Some(u) = self.compute_unit(&root, &a, (i, r), (i, r)) {
                unit_of.entry(a.clone()).or_default().push(a.clone());
                units.push(u);
            }
        }
        // Tài nguyên đứng trước service (cân bằng tải, URL, cổng API, Cloud Map):
        // tham chiếu tới chúng là cạnh tới service phía sau.
        let mut fronts: HashMap<String, Vec<String>> = HashMap::new();
        for (svc, _) in unit_of.clone().iter().filter(|(k, v)| v.contains(k)) {
            match type_of(svc) {
                "aws_ecs_service" => {
                    for tg in points_to(&refs_of, svc, "aws_lb_target_group") {
                        fronts.entry(tg.clone()).or_default().push(svc.clone());
                        for l in referencing(&refs_of, "aws_lb_listener", &tg).into_iter().chain(referencing(&refs_of, "aws_lb_listener_rule", &tg)) {
                            for lb in points_to(&refs_of, &l, "aws_lb") {
                                fronts.entry(lb).or_default().push(svc.clone());
                            }
                            // Luật của listener trỏ listener, listener trỏ bộ cân bằng tải.
                            for l2 in points_to(&refs_of, &l, "aws_lb_listener") {
                                for lb in points_to(&refs_of, &l2, "aws_lb") {
                                    fronts.entry(lb).or_default().push(svc.clone());
                                }
                            }
                        }
                    }
                    for sd in points_to(&refs_of, svc, "aws_service_discovery_service") {
                        fronts.entry(sd).or_default().push(svc.clone());
                    }
                }
                "aws_lambda_function" => {
                    for ty in ["aws_lambda_function_url", "aws_lambda_alias"] {
                        for u in referencing(&refs_of, ty, svc) {
                            fronts.entry(u).or_default().push(svc.clone());
                        }
                    }
                }
                _ => {}
            }
        }
        for v in fronts.values_mut() {
            v.sort();
            v.dedup();
        }
        // Lối vào Internet.
        for u in &mut units {
            let mut exposed = Vec::new();
            self.exposure(u, &refs_of, &back, &by_addr, &mut exposed);
            u.exposed = exposed;
        }
        // Cạnh: biến môi trường của từng container.
        let compute: HashSet<String> = units.iter().map(|u| u.addr.clone()).collect();
        let mut targets: Vec<(String, usize, usize)> = Vec::new();
        // Đích là data source (cơ sở dữ liệu có sẵn, không do repo tạo).
        let mut data_targets: Vec<(String, (String, usize))> = Vec::new();
        for u in &mut units {
            let raw = std::mem::take(&mut u.edges);
            let mut kept = Vec::new();
            for e in raw {
                let tgt_ty = type_of(&e.target).to_string();
                let path: Vec<&str> = e.shown.rsplit(" → ").next().unwrap_or("").split('.').collect();
                let attr = path.iter().rev().find(|p| !p.is_empty()).copied().unwrap_or("");
                let resolved: Vec<String> = if let Some(svcs) = unit_of.get(&e.target) {
                    svcs.clone()
                } else if let Some(svcs) = fronts.get(&e.target) {
                    svcs.clone()
                } else if NETWORK_ATTRS.contains(&attr) || MESSAGING.contains(&tgt_ty.as_str()) {
                    if let Some(&(i, r)) = by_addr.get(&e.target) {
                        targets.push((e.target.clone(), i, r));
                    } else if let Some(place) = self.data_place(&all, &e.target) {
                        data_targets.push((e.target.clone(), place));
                    }
                    vec![e.target.clone()]
                } else {
                    Vec::new()
                };
                for t in resolved {
                    if t != u.addr {
                        kept.push(RefEdge { target: t, ..e.clone() });
                    }
                }
            }
            kept.dedup();
            u.edges = kept;
        }
        targets.sort_by(|a, b| a.0.cmp(&b.0));
        targets.dedup_by(|a, b| a.0 == b.0);
        for (addr, i, r) in targets {
            if compute.contains(&addr) {
                continue;
            }
            let d = &self.insts[i].module.resources[r];
            units.push(Unit {
                root: root.clone(),
                addr,
                ty: d.ty.clone(),
                file: self.insts[i].module.files[d.file].clone(),
                line: d.line,
                compute: false,
                containers: Vec::new(),
                edges: Vec::new(),
                exposed: Vec::new(),
            });
        }
        data_targets.sort();
        data_targets.dedup();
        for (addr, (file, line)) in data_targets {
            units.push(Unit {
                root: root.clone(),
                ty: type_of(&addr).to_string(),
                addr,
                file,
                line,
                compute: false,
                containers: Vec::new(),
                edges: Vec::new(),
                exposed: Vec::new(),
            });
        }
        self.out.units.extend(units);
        // Hết ngân sách: các giá trị sau đó thành "không biết", tham chiếu và
        // lối vào trong chúng mất. Báo, không im lặng (TM-48).
        if self.steps > MAX_STEPS || self.units > MAX_UNITS {
            let file = self.insts[root_inst].module.files.first().cloned().unwrap_or_else(|| dot(&root));
            self.unread(file, format!("quá {MAX_STEPS} bước hay {MAX_UNITS} đơn vị giá trị đọc Terraform cho cả repo; phần sau không đọc"));
        }
    }

    /// Service chạy mã: `named` là tài nguyên được gọi tên (ECS service, hay
    /// chính tài nguyên), `holder` là tài nguyên khai container.
    fn compute_unit(&mut self, root: &str, addr: &str, named: (usize, usize), holder: (usize, usize)) -> Option<Unit> {
        let (ni, nr) = named;
        let (hi, hr) = holder;
        let nd = &self.insts[ni].module.resources[nr];
        let (nty, nline, nfile) = (nd.ty.clone(), nd.line, self.insts[ni].module.files[nd.file].clone());
        let n_each = nd.each.clone();
        let hd = &self.insts[hi].module.resources[hr];
        let (hty, hbody, hfid) = (hd.ty.clone(), hd.body.clone(), hd.file);
        let hfile = self.insts[hi].module.files[hfid].clone();
        let h_each = hd.each.clone();
        for (each, fid_file) in [(n_each, nfile.clone()), (h_each, hfile.clone())] {
            if let Some((kind, x)) = each {
                match (kind.as_str(), &x.e) {
                    ("count", E::Num(n)) if n == "0" => return None,
                    ("count", E::Num(_)) => {}
                    _ => self.unread(format!("{fid_file}:{}", x.line), format!("`{kind}` không hằng: số bản của tài nguyên chưa biết, cạnh có thể khác giữa các bản")),
                }
            }
        }
        let mut containers = Vec::new();
        let mut edges = Vec::new();
        let ctx = (hi, hfid, hfile.clone());
        match hty.as_str() {
            "aws_ecs_task_definition" => {
                let Some((v, line)) = self.attr(hi, hfid, &hbody, "container_definitions") else {
                    self.unread(format!("{hfile}:{}", self.insts[hi].module.resources[hr].line), "task definition không có `container_definitions`".into());
                    return None;
                };
                let v = match v {
                    V::Str(ps) => match parse_json_template(&ps, line) {
                        Some(v) => v,
                        None => {
                            self.unread(format!("{hfile}:{line}"), "`container_definitions` không phải JSON đọc được".into());
                            return None;
                        }
                    },
                    other => other,
                };
                if let Some(w) = v.hard() {
                    self.unread(format!("{}:{}", w.file, w.line), format!("`container_definitions`: {}", w.what));
                }
                for c in v.items() {
                    let name = c.get("name").and_then(V::known).unwrap_or_default();
                    let image = c.get("image").and_then(V::known);
                    let list = |key: &str| c.get(key).map(|v| (v.items().iter().filter_map(V::known).collect::<Vec<_>>(), c.line_of(key).unwrap_or(line)));
                    let mut env = Vec::new();
                    for e in c.get("environment").map(V::items).unwrap_or(&[]) {
                        let Some(k) = e.get("name").and_then(V::known) else { continue };
                        let val = e.get("value").cloned().unwrap_or(V::Null);
                        let at = e.line_of("value").unwrap_or(line);
                        self.env_var(&ctx, &k, at, &val, &mut env, &mut edges);
                    }
                    containers.push(k8s::Container {
                        name,
                        line,
                        init: false,
                        image,
                        command: list("entryPoint"),
                        args: list("command"),
                        working_dir: c.get("workingDirectory").and_then(V::known),
                        env,
                        env_from: Vec::new(),
                        refs: Vec::new(),
                        command_origin: None,
                        args_origin: None,
                    });
                }
            }
            "aws_lambda_function" => {
                let image = self.attr(hi, hfid, &hbody, "image_uri").and_then(|(v, _)| v.known());
                let mut env = Vec::new();
                for b in hbody.blocks("environment") {
                    self.dynamic_in(&b.body, &hfile, "variables");
                    if let Some((V::Obj(vars), _)) = self.attr(hi, hfid, &b.body, "variables") {
                        for (k, v, at) in vars {
                            self.env_var(&ctx, &k, at, &v, &mut env, &mut edges);
                        }
                    } else if let Some((v, line)) = self.attr(hi, hfid, &b.body, "variables") {
                        if let Some(w) = v.hard() {
                            self.unread(format!("{}:{}", w.file, w.line), format!("biến môi trường của Lambda: {}", w.what));
                        }
                        // Không ra object mà vẫn có tham chiếu: vẫn là cạnh,
                        // không để mất (TM-49).
                        self.env_var(&ctx, "variables", line, &v, &mut env, &mut edges);
                    }
                }
                self.dynamic_in(&hbody, &hfile, "environment");
                containers.push(k8s::Container {
                    name: self.insts[hi].module.resources[hr].name.clone(),
                    line: self.insts[hi].module.resources[hr].line,
                    init: false,
                    image,
                    command: None,
                    args: None,
                    working_dir: None,
                    env,
                    env_from: Vec::new(),
                    refs: Vec::new(),
                    command_origin: None,
                    args_origin: None,
                });
            }
            "google_cloud_run_v2_service" | "google_cloud_run_service" => {
                let spec_blocks: Vec<Block> = if hty == "google_cloud_run_v2_service" {
                    hbody.blocks("template").cloned().collect()
                } else {
                    hbody.blocks("template").flat_map(|t| t.body.blocks("spec").cloned().collect::<Vec<_>>()).collect()
                };
                for t in &spec_blocks {
                    self.dynamic_in(&t.body, &hfile, "containers");
                    for c in t.body.blocks("containers") {
                        self.dynamic_in(&c.body, &hfile, "env");
                        let image = self.attr(hi, hfid, &c.body, "image").and_then(|(v, _)| v.known());
                        let list = |cx: &mut Self, key: &str| {
                            cx.attr(hi, hfid, &c.body, key).map(|(v, l)| (v.items().iter().filter_map(V::known).collect::<Vec<_>>(), l))
                        };
                        let command = list(self, "command");
                        let args = list(self, "args");
                        let mut env = Vec::new();
                        for e in c.body.blocks("env") {
                            let Some((k, _)) = self.attr(hi, hfid, &e.body, "name") else { continue };
                            let Some(k) = k.known() else { continue };
                            let (val, line) = self.attr(hi, hfid, &e.body, "value").unwrap_or((V::Null, e.line));
                            self.env_var(&ctx, &k, line, &val, &mut env, &mut edges);
                        }
                        containers.push(k8s::Container {
                            name: self.attr(hi, hfid, &c.body, "name").and_then(|(v, _)| v.known()).unwrap_or_default(),
                            line: c.line,
                            init: false,
                            image,
                            command,
                            args,
                            working_dir: self.attr(hi, hfid, &c.body, "working_dir").and_then(|(v, _)| v.known()),
                            env,
                            env_from: Vec::new(),
                            refs: Vec::new(),
                            command_origin: None,
                            args_origin: None,
                        });
                    }
                }
            }
            _ => return None,
        }
        let _ = nty;
        Some(Unit {
            root: root.to_string(),
            addr: addr.to_string(),
            ty: self.insts[ni].module.resources[nr].ty.clone(),
            file: nfile,
            line: nline,
            compute: true,
            containers,
            edges,
            exposed: Vec::new(),
        })
    }

    /// File, dòng khai data source `addr` (`module.x.data.<loại>.<tên>`) trong
    /// các module của cây.
    fn data_place(&self, all: &[usize], addr: &str) -> Option<(String, usize)> {
        all.iter().find_map(|&i| {
            let inst = &self.insts[i];
            let rest = addr.strip_prefix(&inst.prefix)?.strip_prefix("data.")?;
            if rest.contains("module.") {
                return None;
            }
            inst.module.data.get(rest).map(|&(f, l)| (inst.module.files[f].clone(), l))
        })
    }

    /// `dynamic "<kind>"` trong thân: archctl không sinh khối từ nó (TM-49).
    fn dynamic_in(&mut self, body: &Body, file: &str, kind: &str) {
        for d in body.blocks("dynamic") {
            if d.labels.first().is_some_and(|l| l == kind) {
                self.unread(format!("{file}:{}", d.line), format!("khối `dynamic \"{kind}\"` chưa được đọc"));
            }
        }
    }

    /// Một biến môi trường: tên máy trong giá trị (như compose), tham chiếu tài
    /// nguyên thành cạnh. Không giữ giá trị (TM-50).
    fn env_var(&mut self, ctx: &(usize, usize, String), key: &str, line: usize, v: &V, env: &mut Vec<compose::Env>, edges: &mut Vec<RefEdge>) {
        let (_, _, file) = ctx;
        if let Some(w) = v.hard() {
            self.unread(format!("{}:{}", w.file, w.line), format!("biến môi trường `{key}`: {}", w.what));
        }
        // Giá trị biết được (không tham chiếu) đi qua bộ tìm tên máy.
        let wanted = self.fs.env_words;
        if let Some(text) = v.known() {
            env.push(compose::env_entry_with(key, line, &text, wanted));
        } else if let V::Str(ps) = v {
            let text: String = ps
                .iter()
                .map(|p| match p {
                    P::Text(t) => t.as_str(),
                    _ => "\u{0}",
                })
                .collect();
            env.push(compose::env_entry_with(key, line, &text, wanted));
        }
        if compose::secret_key(key) {
            return;
        }
        let mut refs = Vec::new();
        v.refs(&mut refs);
        // Dòng là dòng của giá trị biến môi trường, không phải dòng mà tham
        // chiếu được viết: `local.db` có thể nằm ở file khác.
        for (addr, path, _) in refs {
            let shown = if path.is_empty() { format!("{key} → {addr}") } else { format!("{key} → {addr}.{}", path.join(".")) };
            edges.push(RefEdge { key: key.to_string(), file: file.clone(), line, target: addr, shown });
        }
    }

    fn exposure(&mut self, u: &Unit, refs_of: &RefMap, back: &Back, by_addr: &HashMap<String, (usize, usize)>, out: &mut Vec<(String, usize, String)>) {
        let referencing = |ty: &str, to: &str| -> Vec<String> { referencing_in(back, ty, to) };
        let points_to = |from: &str, ty: &str| -> Vec<String> {
            refs_of.get(from).into_iter().flatten().filter(|(a, _, _)| type_of(a) == ty).map(|(a, _, _)| a.clone()).collect()
        };
        let place = |cx: &Self, addr: &str| -> (String, usize) {
            match by_addr.get(addr) {
                Some(&(i, r)) => {
                    let d = &cx.insts[i].module.resources[r];
                    (cx.insts[i].module.files[d.file].clone(), d.line)
                }
                None => (u.file.clone(), u.line),
            }
        };
        match u.ty.as_str() {
            "aws_ecs_service" => {
                for tg in points_to(&u.addr, "aws_lb_target_group") {
                    let mut listeners = referencing("aws_lb_listener", &tg);
                    for rule in referencing("aws_lb_listener_rule", &tg) {
                        listeners.extend(points_to(&rule, "aws_lb_listener"));
                    }
                    for l in listeners {
                        for lb in points_to(&l, "aws_lb") {
                            let Some(&(i, r)) = by_addr.get(&lb) else { continue };
                            let d = &self.insts[i].module.resources[r];
                            let (body, fid) = (d.body.clone(), d.file);
                            let internal = self.attr(i, fid, &body, "internal").map(|(v, _)| v);
                            if let Some(w) = internal.as_ref().and_then(V::hard) {
                                self.unread(format!("{}:{}", w.file, w.line), format!("`internal` của `{lb}` quyết định lối vào mà chưa đọc được: {}", w.what));
                                continue;
                            }
                            if matches!(internal, Some(V::Bool(true))) || internal.as_ref().and_then(V::known).is_some_and(|s| s == "true") {
                                continue;
                            }
                            if matches!(internal, Some(V::Unknown(_))) {
                                continue;
                            }
                            let (f, line) = place(self, &lb);
                            out.push((f, line, format!("`{lb}` (không `internal`) → `{tg}`")));
                        }
                    }
                }
                // IP công khai cùng nhóm bảo mật mở cho mọi địa chỉ.
                if let Some(&(i, r)) = by_addr.get(&u.addr) {
                    let d = &self.insts[i].module.resources[r];
                    let (body, fid) = (d.body.clone(), d.file);
                    for nc in body.blocks("network_configuration") {
                        let ip = self.attr(i, fid, &nc.body, "assign_public_ip");
                        if let Some(w) = ip.as_ref().and_then(|(v, _)| v.hard()) {
                            self.unread(format!("{}:{}", w.file, w.line), format!("`assign_public_ip` của `{}` chưa đọc được: {}", u.addr, w.what));
                        }
                        let public = ip.is_some_and(|(v, _)| matches!(v, V::Bool(true)) || v.known().is_some_and(|s| s == "true"));
                        if !public {
                            continue;
                        }
                        let mut sgs = Vec::new();
                        if let Some((v, _)) = self.attr(i, fid, &nc.body, "security_groups") {
                            v.refs(&mut sgs);
                        }
                        for (sg, _, _) in sgs {
                            if type_of(&sg) == "aws_security_group" && self.open_world(&sg, back, by_addr) {
                                let (f, line) = place(self, &sg);
                                out.push((f, line, format!("IP công khai, `{sg}` mở `0.0.0.0/0`")));
                            }
                        }
                    }
                }
            }
            "aws_lambda_function" => {
                // URL gắn thẳng vào hàm, hay qua một alias của nó.
                let mut urls = referencing("aws_lambda_function_url", &u.addr);
                for alias in referencing("aws_lambda_alias", &u.addr) {
                    urls.extend(referencing("aws_lambda_function_url", &alias));
                }
                for url in urls {
                    let Some(&(i, r)) = by_addr.get(&url) else { continue };
                    let d = &self.insts[i].module.resources[r];
                    let (body, fid) = (d.body.clone(), d.file);
                    let auth = self.attr(i, fid, &body, "authorization_type").map(|(v, _)| v);
                    if let Some(w) = auth.as_ref().and_then(V::hard) {
                        self.unread(format!("{}:{}", w.file, w.line), format!("`authorization_type` của `{url}` chưa đọc được: {}", w.what));
                    }
                    if auth.and_then(|v| v.known()).is_some_and(|s| s == "NONE") {
                        let (f, line) = place(self, &url);
                        out.push((f, line, format!("`{url}` không xác thực")));
                    }
                }
                for ty in ["aws_apigatewayv2_integration", "aws_api_gateway_integration"] {
                    for a in referencing(ty, &u.addr) {
                        let (f, line) = place(self, &a);
                        out.push((f, line, format!("cổng API `{a}`")));
                    }
                }
            }
            "google_cloud_run_v2_service" | "google_cloud_run_service" => {
                if let Some(&(i, r)) = by_addr.get(&u.addr) {
                    let d = &self.insts[i].module.resources[r];
                    let (body, fid) = (d.body.clone(), d.file);
                    if self.attr(i, fid, &body, "invoker_iam_disabled").is_some_and(|(v, _)| matches!(v, V::Bool(true))) {
                        out.push((u.file.clone(), u.line, "`invoker_iam_disabled = true`".into()));
                    }
                }
                let iam = if u.ty == "google_cloud_run_v2_service" {
                    ["google_cloud_run_v2_service_iam_member", "google_cloud_run_v2_service_iam_binding"]
                } else {
                    ["google_cloud_run_service_iam_member", "google_cloud_run_service_iam_binding"]
                };
                let policy = if u.ty == "google_cloud_run_v2_service" { "google_cloud_run_v2_service_iam_policy" } else { "google_cloud_run_service_iam_policy" };
                for p in referencing(policy, &u.addr) {
                    let (f, line) = place(self, &p);
                    self.unread(format!("{f}:{line}"), format!("`{p}` (IAM policy) chưa được đọc: không biết có cho `allUsers` gọi `{}` không", u.addr));
                }
                for ty in iam {
                    for m in referencing(ty, &u.addr) {
                        let Some(&(i, r)) = by_addr.get(&m) else { continue };
                        let d = &self.insts[i].module.resources[r];
                        let (body, fid) = (d.body.clone(), d.file);
                        let role = self.attr(i, fid, &body, "role").and_then(|(v, _)| v.known());
                        if role.as_deref().is_some_and(|r| r != "roles/run.invoker") {
                            continue;
                        }
                        let mut members = Vec::new();
                        if let Some((v, _)) = self.attr(i, fid, &body, "member") {
                            members.push(v);
                        }
                        match self.attr(i, fid, &body, "members") {
                            Some((V::List(vs), _)) => members.extend(vs),
                            Some((v, _)) => members.push(v),
                            None => {}
                        }
                        if let Some(w) = members.iter().find_map(V::hard) {
                            self.unread(format!("{}:{}", w.file, w.line), format!("thành viên của `{m}` chưa đọc được: {}", w.what));
                        }
                        if members.iter().any(|m| m.known().is_some_and(|s| s == "allUsers" || s == "allAuthenticatedUsers")) {
                            let (f, line) = place(self, &m);
                            out.push((f, line, format!("`{m}` cho `allUsers`")));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Nhóm bảo mật cho phép vào từ `0.0.0.0/0` (khối `ingress`, hay luật
    /// riêng trỏ tới nó).
    fn open_world(&mut self, sg: &str, back: &Back, by_addr: &HashMap<String, (usize, usize)>) -> bool {
        let world = |v: &V| {
            let mut all = vec![v.clone()];
            all.extend(v.items().iter().cloned());
            all.iter().any(|x| x.known().is_some_and(|s| s == "0.0.0.0/0" || s == "::/0"))
        };
        if let Some(&(i, r)) = by_addr.get(sg) {
            let d = &self.insts[i].module.resources[r];
            let (body, fid) = (d.body.clone(), d.file);
            for ing in body.blocks("ingress") {
                for key in ["cidr_blocks", "ipv6_cidr_blocks"] {
                    let v = self.attr(i, fid, &ing.body, key);
                    if let Some(w) = v.as_ref().and_then(|(v, _)| v.hard()) {
                        self.unread(format!("{}:{}", w.file, w.line), format!("`{key}` của `{sg}` chưa đọc được: {}", w.what));
                    }
                    if v.is_some_and(|(v, _)| world(&v)) {
                        return true;
                    }
                }
            }
            self.dynamic_in(&body, &self.insts[i].module.files[fid].clone(), "ingress");
        }
        let mut rules = referencing_in(back, "aws_security_group_rule", sg);
        rules.extend(referencing_in(back, "aws_vpc_security_group_ingress_rule", sg));
        for rule in rules {
            let Some(&(i, r)) = by_addr.get(&rule) else { continue };
            let d = &self.insts[i].module.resources[r];
            let (body, fid) = (d.body.clone(), d.file);
            let mut rs = Vec::new();
            if let Some((v, _)) = self.attr(i, fid, &body, "security_group_id") {
                v.refs(&mut rs);
            }
            if !rs.iter().any(|(t, _, _)| t == sg) {
                continue;
            }
            if type_of(&rule) == "aws_security_group_rule" && self.attr(i, fid, &body, "type").and_then(|(v, _)| v.known()).is_some_and(|s| s != "ingress") {
                continue;
            }
            for key in ["cidr_blocks", "ipv6_cidr_blocks", "cidr_ipv4", "cidr_ipv6"] {
                if self.attr(i, fid, &body, key).is_some_and(|(v, _)| world(&v)) {
                    return true;
                }
            }
        }
        false
    }
}

impl Module {
    fn dir_or_dot(&self) -> String {
        dot(&self.dir)
    }
}

fn dot(d: &str) -> String {
    if d.is_empty() { ".".to_string() } else { d.to_string() }
}

/// Đích → các tài nguyên trỏ tới nó.
type Back = HashMap<String, Vec<String>>;

fn back_index(refs_of: &RefMap) -> Back {
    let mut back: Back = HashMap::new();
    for (from, rs) in refs_of {
        for (to, _, _) in rs {
            back.entry(to.clone()).or_default().push(from.clone());
        }
    }
    for v in back.values_mut() {
        v.sort();
        v.dedup();
    }
    back
}

/// Tài nguyên loại `ty` trỏ tới `to`.
fn referencing_in(back: &Back, ty: &str, to: &str) -> Vec<String> {
    back.get(to).into_iter().flatten().filter(|a| type_of(a) == ty).cloned().collect()
}

/// Loại tài nguyên của một địa chỉ: `module.a.aws_lb.x` → `aws_lb`. Tên cũ
/// `aws_alb*` là bí danh của `aws_lb*` trong provider AWS.
fn type_of(addr: &str) -> &str {
    let mut parts: Vec<&str> = addr.split('.').collect();
    while parts.first() == Some(&"module") && parts.len() > 2 {
        parts.drain(..2);
    }
    let ty = if parts.first() == Some(&"data") { parts.get(1).copied() } else { parts.first().copied() }.unwrap_or("");
    match ty {
        "aws_alb" => "aws_lb",
        "aws_alb_listener" => "aws_lb_listener",
        "aws_alb_listener_rule" => "aws_lb_listener_rule",
        "aws_alb_target_group" => "aws_lb_target_group",
        t => t,
    }
}

fn add_block(m: &mut Module, fid: usize, b: Block) {
    let line = b.line;
    match (b.kind.as_str(), b.labels.as_slice()) {
        ("variable", [name]) => {
            let d = b.body.attr("default").map(|a| a.expr.clone());
            m.vars.insert(name.clone(), (fid, d));
        }
        ("locals", []) => {
            for item in b.body.items {
                if let hcl::Item::Attr(a) = item {
                    m.locals.insert(a.name, (fid, a.expr));
                }
            }
        }
        ("output", [name]) => {
            if let Some(a) = b.body.attr("value") {
                m.outputs.insert(name.clone(), (fid, a.expr.clone()));
            }
        }
        ("resource", [ty, name]) => {
            let each = b
                .body
                .attr("count")
                .map(|a| ("count".to_string(), a.expr.clone()))
                .or_else(|| b.body.attr("for_each").map(|a| ("for_each".to_string(), a.expr.clone())));
            m.resources.push(ResDecl { ty: ty.clone(), name: name.clone(), file: fid, line, body: Rc::new(b.body), each });
        }
        ("data", [ty, name]) => {
            m.data.insert(format!("{ty}.{name}"), (fid, line));
        }
        ("module", [name]) => m.calls.push((name.clone(), fid, line, Rc::new(b.body))),
        _ => {}
    }
}

/// Hai giá trị có thể (hai nhánh của `? :` với điều kiện là đầu vào): object
/// gộp theo khoá, list cùng độ dài gộp theo vị trí; còn lại thành chuỗi không
/// biết mà vẫn giữ mọi tham chiếu của cả hai.
fn join(x: V, y: V, w: &Why) -> V {
    if x == y {
        return x;
    }
    match (x, y) {
        (V::Obj(a), V::Obj(b)) => {
            let mut out: Vec<(String, V, usize)> = Vec::new();
            for (k, xv, l) in a {
                let other = b.iter().rev().find(|(kb, _, _)| *kb == k).map(|(_, v, _)| v.clone());
                out.push((k, match other {
                    Some(yv) => join(xv, yv, w),
                    None => xv,
                }, l));
            }
            for (k, yv, l) in b {
                if !out.iter().any(|(e, _, _)| *e == k) {
                    out.push((k, yv, l));
                }
            }
            V::Obj(out)
        }
        (V::List(a), V::List(b)) if a.len() == b.len() => V::List(a.into_iter().zip(b).map(|(p, q)| join(p, q, w)).collect()),
        // `cond ? [x] : []`: mục nào cũng có thể có mặt.
        (V::List(mut a), V::List(b)) => {
            a.extend(b);
            V::List(a)
        }
        // `cond ? {…} : null`: giữ phía có giá trị.
        (v @ (V::Obj(_) | V::List(_)), V::Null) | (V::Null, v @ (V::Obj(_) | V::List(_))) => v,
        (x, y) => {
            let mut refs = Vec::new();
            x.refs(&mut refs);
            y.refs(&mut refs);
            let mut ps = vec![P::Unknown(Box::new(w.clone()))];
            ps.extend(refs.into_iter().map(|(a, p, l)| P::Ref(a, p, l)));
            V::Str(ps)
        }
    }
}

/// Chuỗi JSON (heredoc) có `${}`: thay mỗi phần không phải chữ bằng một dấu
/// giữ chỗ, đọc JSON, rồi trả các phần về chỗ trong chuỗi.
fn parse_json_template(ps: &[P], line: usize) -> Option<V> {
    let mut text = String::new();
    let mut holes: Vec<P> = Vec::new();
    for p in ps {
        match p {
            P::Text(t) => text.push_str(t),
            other => {
                text.push_str(&format!("\u{1}{}\u{1}", holes.len()));
                holes.push(other.clone());
            }
        }
    }
    let j = crate::jsonc::parse(&text)?;
    Some(from_json(&j, &holes, line))
}

fn from_json(j: &crate::jsonc::Value, holes: &[P], line: usize) -> V {
    use crate::jsonc::Value as J;
    match j {
        J::Null => V::Null,
        J::Bool(b) => V::Bool(*b),
        J::Str(s) => {
            let mut ps = Vec::new();
            for (k, piece) in s.split('\u{1}').enumerate() {
                if k % 2 == 1 {
                    match piece.parse::<usize>().ok().and_then(|n| holes.get(n)) {
                        Some(h) => ps.push(h.clone()),
                        None => ps.push(P::Text(piece.to_string())),
                    }
                } else if !piece.is_empty() {
                    ps.push(P::Text(piece.to_string()));
                }
            }
            if ps.is_empty() {
                ps.push(P::Text(String::new()));
            }
            V::Str(ps)
        }
        J::Arr(v) => V::List(v.iter().map(|x| from_json(x, holes, line)).collect()),
        J::Obj(es) => V::Obj(es.iter().map(|(k, x)| (k.clone(), from_json(x, holes, line), line)).collect()),
    }
}

/// Thư mục của các module gốc mà `files` tạo ra (cho `scan`, `init`).
pub fn fmt_num(x: f64) -> String {
    if x.fract() == 0.0 && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{x}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn run(files: &[(&str, &str)]) -> Out {
        let map: BTreeMap<String, String> = files.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let read = |p: &str| map.get(p).cloned().ok_or_else(|| "không có".to_string());
        let is_dir = |p: &str| map.keys().any(|k| k.starts_with(&format!("{p}/")));
        let names: Vec<String> = map.keys().cloned().collect();
        load(&names, &Fs { read: &read, is_dir: &is_dir, env_words: &[] })
    }

    fn unit<'a>(o: &'a Out, addr: &str) -> &'a Unit {
        o.units.iter().find(|u| u.addr == addr).unwrap_or_else(|| panic!("không có {addr}: {:?}", o.units.iter().map(|u| &u.addr).collect::<Vec<_>>()))
    }

    const CLOUD_RUN: &str = r#"
resource "google_cloud_run_v2_service" "public" {
  name     = "public-service"
  location = "us-central1"
  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
      env {
        name  = "URL"
        value = google_cloud_run_v2_service.private.uri
      }
    }
  }
}

resource "google_cloud_run_v2_service" "private" {
  name     = "private-service"
  location = "us-central1"
  template {
    containers {
      image = "us-docker.pkg.dev/cloudrun/container/hello"
    }
  }
}

resource "google_cloud_run_v2_service_iam_member" "public" {
  location = google_cloud_run_v2_service.public.location
  name     = google_cloud_run_v2_service.public.name
  role     = "roles/run.invoker"
  member   = "allUsers"
}
"#;

    #[test]
    fn cloud_run_goi_nhau_va_cho_allusers() {
        let o = run(&[("run/main.tf", CLOUD_RUN)]);
        assert_eq!(o.roots, ["run"]);
        assert!(o.unread.is_empty(), "{:?}", o.unread);
        let p = unit(&o, "google_cloud_run_v2_service.public");
        assert_eq!(p.edges.len(), 1);
        assert_eq!(p.edges[0].target, "google_cloud_run_v2_service.private");
        assert_eq!((p.edges[0].file.as_str(), p.edges[0].line), ("run/main.tf", 10));
        assert_eq!(p.edges[0].shown, "URL → google_cloud_run_v2_service.private.uri");
        assert_eq!(p.exposed.len(), 1, "{:?}", p.exposed);
        assert_eq!(p.exposed[0].1, 26);
        assert!(unit(&o, "google_cloud_run_v2_service.private").exposed.is_empty());
    }

    #[test]
    fn ecs_jsonencode_va_heredoc_toi_db_qua_lb() {
        let main = r#"
locals {
  db = aws_db_instance.main.address
}
variable "tag" {
  default = "1.4"
}
resource "aws_db_instance" "main" {
  engine   = "postgres"
  password = var.db_password
}
variable "db_password" {}
resource "aws_ecs_task_definition" "api" {
  family = "api"
  container_definitions = jsonencode([{
    name  = "api"
    image = "acme/api:${var.tag}"
    command = ["python", "-m", "api"]
    environment = [
      { name = "DB_HOST", value = local.db },
      { name = "DB_PASSWORD", value = aws_db_instance.main.password },
      { name = "MODE", value = "fast" },
    ]
  }])
}
resource "aws_ecs_service" "api" {
  task_definition = aws_ecs_task_definition.api.arn
  load_balancer {
    target_group_arn = aws_lb_target_group.api.arn
  }
}
resource "aws_lb" "public" {
  internal = false
}
resource "aws_lb_target_group" "api" {}
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.public.arn
  default_action {
    target_group_arn = aws_lb_target_group.api.arn
  }
}
resource "aws_ecs_task_definition" "web" {
  container_definitions = <<EOF
[{"name": "web", "image": "acme/web",
  "environment": [{"name": "API_URL", "value": "http://${aws_lb.public.dns_name}/v1"}]}]
EOF
}
"#;
        let o = run(&[("main.tf", main)]);
        assert!(o.unread.is_empty(), "{:?}", o.unread);
        assert_eq!(o.roots, [""]);
        let api = unit(&o, "aws_ecs_service.api");
        assert_eq!(api.containers[0].image.as_deref(), Some("acme/api:1.4"));
        assert_eq!(api.containers[0].args.as_ref().map(|a| a.0.clone()), Some(vec!["python".to_string(), "-m".into(), "api".into()]));
        // Mật khẩu không thành cạnh, không giữ giá trị.
        assert_eq!(api.edges.iter().map(|e| e.target.as_str()).collect::<Vec<_>>(), ["aws_db_instance.main"]);
        assert_eq!(api.edges[0].shown, "DB_HOST → aws_db_instance.main.address");
        assert_eq!(api.exposed.len(), 1, "{:?}", api.exposed);
        let web = unit(&o, "aws_ecs_task_definition.web");
        assert_eq!(web.edges.iter().map(|e| e.target.as_str()).collect::<Vec<_>>(), ["aws_ecs_service.api"]);
        let db = unit(&o, "aws_db_instance.main");
        assert!(!db.compute);
        assert!(!format!("{o:?}").contains("fast"));
    }

    #[test]
    fn module_trong_repo_va_registry() {
        let o = run(&[
            ("envs/prod/main.tf", "module \"app\" {\n  source = \"../../modules/app\"\n  image  = \"acme/app:2\"\n}\nmodule \"vpc\" {\n  source = \"terraform-aws-modules/vpc/aws\"\n}\n"),
            (
                "modules/app/main.tf",
                "variable \"image\" {}\nresource \"aws_lambda_function\" \"worker\" {\n  image_uri = var.image\n  environment {\n    variables = { QUEUE_URL = aws_sqs_queue.jobs.url, TOKEN = \"x\" }\n  }\n}\nresource \"aws_sqs_queue\" \"jobs\" {}\noutput \"url\" { value = aws_sqs_queue.jobs.url }\n",
            ),
        ]);
        assert_eq!(o.roots, ["envs/prod"]);
        let w = unit(&o, "module.app.aws_lambda_function.worker");
        assert_eq!(w.root, "envs/prod");
        assert_eq!(w.containers[0].image.as_deref(), Some("acme/app:2"));
        assert_eq!(w.edges[0].target, "module.app.aws_sqs_queue.jobs");
        assert!(o.unread.iter().any(|(f, why)| f == "envs/prod/main.tf:5" && why.contains("registry")), "{:?}", o.unread);
    }

    #[test]
    fn ngoai_tap_con_thi_bao() {
        let o = run(&[(
            "main.tf",
            "resource \"aws_lambda_function\" \"f\" {\n  for_each = var.fns\n  environment {\n    variables = { A = [for x in var.l : x] }\n  }\n}\nresource \"google_cloud_run_v2_service\" \"s\" {\n  template {\n    containers {\n      dynamic \"env\" {\n        for_each = var.e\n        content {}\n      }\n      env {\n        name = \"B\"\n        value = file(\"x\")\n      }\n    }\n  }\n}\n",
        ), ("a.tf.json", "{}"), ("x_override.tf", "")]);
        let why: Vec<String> = o.unread.iter().map(|(f, w)| format!("{f} {w}")).collect();
        for want in ["main.tf:2 `for_each` không hằng", "main.tf:4 biến môi trường `A`: biểu thức `for`", "main.tf:10 khối `dynamic \"env\"`", "main.tf:16 biến môi trường `B`: hàm `file`", "a.tf.json Terraform dạng JSON", "x_override.tf file override"] {
            assert!(why.iter().any(|w| w.starts_with(want)), "thiếu {want}: {why:?}");
        }
    }

    #[test]
    fn local_vong_va_long_sau_khong_treo() {
        let mut locals = String::from("locals {\n  a0 = \"x\"\n");
        for i in 1..300 {
            locals.push_str(&format!("  a{i} = \"${{local.a{}}}${{local.a{}}}\"\n", i - 1, i - 1));
        }
        locals.push_str("  c1 = local.c2\n  c2 = local.c1\n}\n");
        let src = format!("{locals}resource \"aws_lambda_function\" \"f\" {{\n  environment {{\n    variables = {{ A = local.a299, B = local.c1 }}\n  }}\n}}\n");
        let t = crate::testclock::Clock::start();
        let o = run(&[("main.tf", &src)]);
        assert!(t.elapsed().as_secs_f64() < 2.0, "{:?}", t.elapsed());
        let why: Vec<&String> = o.unread.iter().map(|(_, w)| w).collect();
        assert!(why.iter().any(|w| w.contains("dài quá")), "{why:?}");
        // Vòng `local` riêng (ca trên đã hết ngân sách).
        let src = "locals {\n  c1 = local.c2\n  c2 = local.c1\n}\nresource \"aws_lambda_function\" \"f\" {\n  environment {\n    variables = { B = local.c1 }\n  }\n}\n";
        let o = run(&[("main.tf", src)]);
        assert!(o.unread.iter().any(|(_, w)| w.contains("vòng")), "{:?}", o.unread);
        // Module gọi vòng.
        let o = run(&[("a/main.tf", "module \"b\" { source = \"../b\" }\n"), ("b/main.tf", "module \"a\" { source = \"../a\" }\n")]);
        assert!(o.roots.is_empty() || o.unread.iter().any(|(_, w)| w.contains("vòng")), "{o:?}");
    }

    #[test]
    fn review_vong_1_ngu_nghia() {
        let src = r#"
resource "aws_db_instance" "main" {}
resource "aws_db_instance" "replica" {}
resource "aws_db_instance" "old" {}
variable "use_replica" {}
variable "extra" {}
resource "aws_lambda_function" "f" {
  image_uri = "x"
  environment {
    variables = merge(var.extra, { DB_HOST = var.use_replica ? aws_db_instance.replica.address : aws_db_instance.main.address }, { OLD = aws_db_instance.old.address }, { OLD = "localhost" })
  }
}
resource "aws_lambda_function_url" "f" {
  function_name      = aws_lambda_alias.live.arn
  authorization_type = "NONE"
}
resource "aws_lambda_alias" "live" {
  function_name = aws_lambda_function.f.function_name
}
resource "aws_lambda_function_url" "gone" {
  count              = 0
  function_name      = aws_lambda_function.g.function_name
  authorization_type = "NONE"
}
resource "aws_lambda_function" "g" {
  image_uri = "x"
  environment {
    variables = { H = terraform.workspace == "prod" ? aws_db_instance.main.address : data.aws_db_instance.shared.address }
  }
}
data "aws_db_instance" "shared" {}
resource "aws_ecs_task_definition" "api" {
  container_definitions = jsonencode([{ name = "api", image = "a" }])
}
resource "aws_ecs_service" "api" {
  task_definition = aws_ecs_task_definition.api.arn
  load_balancer {
    target_group_arn = aws_alb_target_group.api.arn
  }
}
resource "aws_alb" "public" {
  internal = tobool("false")
}
resource "aws_lb" "other" {}
resource "aws_alb_target_group" "api" {}
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.other.arn
}
resource "aws_lb_listener_rule" "r" {
  listener_arn = aws_lb_listener.http.arn
  action {
    target_group_arn = aws_alb_target_group.api.arn
  }
}
resource "aws_alb_listener" "x" {
  load_balancer_arn = aws_alb.public.arn
  default_action {
    target_group_arn = aws_alb_target_group.api.arn
  }
}
"#;
        let o = run(&[("main.tf", src)]);
        let edges = |a: &str| unit(&o, a).edges.iter().map(|e| e.target.clone()).collect::<Vec<_>>();
        // `? :` với điều kiện là đầu vào: cạnh của cả hai nhánh; `merge` giữ phần
        // đã biết, khoá ghi đè không tạo cạnh ma.
        assert_eq!(edges("aws_lambda_function.f"), ["aws_db_instance.replica", "aws_db_instance.main"]);
        assert_eq!(edges("aws_lambda_function.g"), ["aws_db_instance.main", "data.aws_db_instance.shared"]);
        assert!(o.units.iter().any(|u| u.addr == "data.aws_db_instance.shared"));
        // URL qua alias: lộ ra; URL có `count = 0`: không.
        assert_eq!(unit(&o, "aws_lambda_function.f").exposed.len(), 1);
        assert!(unit(&o, "aws_lambda_function.g").exposed.is_empty());
        // Listener rule, bí danh `aws_alb*`; `internal` không đọc được thì báo.
        let api = unit(&o, "aws_ecs_service.api");
        assert!(api.exposed.iter().any(|e| e.2.contains("aws_lb.other")), "{:?}", api.exposed);
        assert!(o.unread.iter().any(|(_, w)| w.contains("`internal` của `aws_alb.public`")), "{:?}", o.unread);
    }

    #[test]
    fn nhanh_khac_dang_van_giu_canh() {
        // `cond ? [x] : []`, `cond ? {…} : null` trong `concat`, `merge`, biến
        // môi trường của Lambda: tham chiếu không mất (TM-49).
        let src = r#"
resource "aws_db_instance" "main" {}
variable "with_db" {}
resource "aws_ecs_task_definition" "api" {
  container_definitions = jsonencode([{
    name  = "api"
    image = "a"
    environment = concat([{ name = "MODE", value = "x" }], var.with_db ? [{ name = "DB_HOST", value = aws_db_instance.main.address }] : [])
  }])
}
resource "aws_lambda_function" "a" {
  image_uri = "x"
  environment {
    variables = var.with_db ? { DB_HOST = aws_db_instance.main.address } : null
  }
}
resource "aws_lambda_function" "b" {
  image_uri = "x"
  environment {
    variables = merge({ A = "1" }, var.with_db ? { DB_HOST = aws_db_instance.main.address } : null)
  }
}
"#;
        let o = run(&[("main.tf", src)]);
        for a in ["aws_ecs_task_definition.api", "aws_lambda_function.a", "aws_lambda_function.b"] {
            assert_eq!(unit(&o, a).edges.iter().map(|e| e.target.as_str()).collect::<Vec<_>>(), ["aws_db_instance.main"], "{a}");
        }
    }

    #[test]
    fn nam_nghin_lambda_khong_bac_hai() {
        let mut src = String::new();
        for i in 0..5_000 {
            src.push_str(&format!(
                "resource \"aws_lambda_function\" \"f{i}\" {{\n  image_uri = \"x\"\n}}\nresource \"aws_apigatewayv2_integration\" \"i{i}\" {{\n  integration_uri = aws_lambda_function.f{i}.invoke_arn\n}}\n"
            ));
        }
        let t = crate::testclock::Clock::start();
        let o = run(&[("main.tf", &src)]);
        assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
        assert_eq!(o.units.iter().filter(|u| !u.exposed.is_empty()).count(), 5_000);
    }

    #[test]
    fn module_ra_ngoai_repo_khong_doc() {
        let o = run(&[("main.tf", "module \"x\" { source = \"../../etc\" }\n")]);
        assert!(o.unread.iter().any(|(_, w)| w.contains("không tải") || w.contains("không có")), "{:?}", o.unread);
    }
}
