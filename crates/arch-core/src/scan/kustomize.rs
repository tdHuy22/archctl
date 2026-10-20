//! Dựng overlay Kustomize: tập con của `kustomize build`, không chạy lệnh nào.
//!
//! Mỗi kustomization gốc (không bị kustomization khác trỏ tới) là một bản dựng.
//! Tài nguyên đọc từ `resources`, `components` (đệ quy), rồi áp theo thứ tự của
//! Kustomize: sinh ConfigMap, patch (strategic merge, JSON 6902), `namespace`,
//! `namePrefix`/`nameSuffix` (kèm tham chiếu ConfigMap, Service), nhãn (kèm
//! selector khi cần), `images`, `replicas`. Mỗi tài nguyên dựng ra được viết
//! thành văn bản YAML, dòng nào cũng nhớ nó đến từ file nào, dòng nào: giá trị
//! từ patch trỏ về patch, giá trị gốc trỏ về manifest gốc.
//!
//! Thứ ngoài tập con (`replacements`, `vars`, `helmCharts`, generator và
//! transformer plugin, directive `$patch`, target theo regex, resource từ xa)
//! làm cả bản dựng "chưa đọc được": không áp một phần rồi im (TM-45).
//! `secretGenerator` không đọc (TM-46).

use super::yaml::{self, Node};
use std::collections::{HashMap, HashSet};

/// Kustomization lồng nhau tối đa (base của base…).
pub const MAX_DEPTH: usize = 16;
/// Tài nguyên dựng ra tối đa trong một bản dựng (TM-44).
pub const MAX_RESOURCES: usize = 5_000;
/// Cỡ cây tạo ra tối đa cho cả repo (chép YAML, lấy lại base đã dựng, chép
/// patch cho từng đích), tính bằng node cộng một cho mỗi [`BYTES_PER_NODE`]
/// byte khoá và chuỗi: alias nhân bản, alias tới một chuỗi lớn, nhiều overlay
/// trên một base lớn phải dừng; bên gọi giữ ngân sách chung trong
/// [`Fs::nodes`] (TM-44).
pub const MAX_NODES: usize = 1_000_000;
pub const BYTES_PER_NODE: usize = 16;
/// Độ sâu tối đa của cây: alias trỏ tới anchor đã lồng sâu cộng dồn độ sâu
/// vượt trần của bộ đọc YAML; chép, gộp, viết cây đều đệ quy (TM-44).
pub const MAX_TREE_DEPTH: usize = 128;
/// Bước dựng tối đa (so đích patch, áp patch, đổi tên, nhãn) cho cả repo: bên
/// gọi giữ ngân sách chung trong [`Fs::budget`] (TM-44).
pub const MAX_STEPS: usize = 2_000_000;

/// Khoá kustomization làm cả bản dựng không đọc được: archctl không áp chúng.
const UNSUPPORTED: &[&str] = &[
    "replacements",
    "vars",
    "helmCharts",
    "helmChartInflationGenerator",
    "helmGlobals",
    "generators",
    "transformers",
    "validators",
    "configurations",
    "crds",
    "openapi",
    "patchesJson6902Transformers",
];

/// Loại tài nguyên không có namespace: `namespace:` không đặt cho chúng.
const CLUSTER_KINDS: &[&str] = &[
    "Namespace",
    "ClusterRole",
    "ClusterRoleBinding",
    "CustomResourceDefinition",
    "PersistentVolume",
    "StorageClass",
    "PriorityClass",
    "APIService",
    "MutatingWebhookConfiguration",
    "ValidatingWebhookConfiguration",
    "IngressClass",
    "GatewayClass",
];

/// Vị trí nguồn: chỉ số file trong [`Build::files`], dòng.
pub type At = (u32, u32);

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    Null,
    /// Vô hướng như đã viết, và nó viết trần (không nháy) không.
    Scalar(String, bool),
    Seq(Vec<N>),
    Map(Vec<Ent>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct N {
    pub v: Val,
    pub at: At,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Ent {
    pub key: String,
    pub at: At,
    pub val: N,
}

impl N {
    fn get(&self, key: &str) -> Option<&N> {
        match &self.v {
            Val::Map(es) => es.iter().find(|e| e.key == key).map(|e| &e.val),
            _ => None,
        }
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut N> {
        match &mut self.v {
            Val::Map(es) => es.iter_mut().find(|e| e.key == key).map(|e| &mut e.val),
            _ => None,
        }
    }

    fn str(&self) -> Option<&str> {
        match &self.v {
            Val::Scalar(s, _) => Some(s),
            _ => None,
        }
    }

    fn path(&self, keys: &[&str]) -> Option<&N> {
        keys.iter().try_fold(self, |n, k| n.get(k))
    }

    /// Đặt `key` thành `val` (thêm vào cuối nếu chưa có).
    fn set(&mut self, key: &str, at: At, val: N) {
        if let Val::Map(es) = &mut self.v {
            match es.iter_mut().find(|e| e.key == key) {
                Some(e) => {
                    e.val = val;
                    e.at = at;
                }
                None => es.push(Ent { key: key.to_string(), at, val }),
            }
        }
    }

    /// Map con ở `key`, tạo nếu chưa có.
    fn child_map(&mut self, key: &str, at: At) -> Option<&mut N> {
        if self.get(key).is_none_or(|n| !matches!(n.v, Val::Map(_))) {
            self.set(key, at, N { v: Val::Map(Vec::new()), at });
        }
        self.get_mut(key)
    }
}

fn scalar(s: &str, at: At) -> N {
    N { v: Val::Scalar(s.to_string(), false), at }
}

/// Đặt các mục `items` (`n` mục) vào map `m`, tra khoá qua chỉ mục (không
/// bậc hai), trừ bước và node vào ngân sách; `false` khi hết ngân sách.
fn put_all(m: &mut N, items: impl Iterator<Item = Ent>, n: usize, steps: &mut usize, nodes: &mut usize) -> bool {
    let Val::Map(es) = &mut m.v else { return true };
    let cost = es.len() + n;
    if cost > *steps {
        *steps = 0;
        return false;
    }
    *steps -= cost;
    let mut index: HashMap<String, usize> = es.iter().enumerate().map(|(i, e)| (e.key.clone(), i)).collect();
    for e in items {
        let c = 1 + e.key.len() / BYTES_PER_NODE + size(&e.val);
        if c > *nodes {
            *nodes = 0;
            return false;
        }
        *nodes -= c;
        match index.get(&e.key) {
            Some(&i) => {
                es[i].at = e.at;
                es[i].val = e.val;
            }
            None => {
                index.insert(e.key.clone(), es.len());
                es.push(e);
            }
        }
    }
    true
}

/// Cỡ của cây theo đơn vị của [`MAX_NODES`] (độ sâu đã có trần
/// [`MAX_TREE_DEPTH`] khi chép).
fn size(n: &N) -> usize {
    1 + match &n.v {
        Val::Map(es) => es.iter().map(|e| e.key.len() / BYTES_PER_NODE + size(&e.val)).sum(),
        Val::Seq(items) => items.iter().map(size).sum(),
        Val::Scalar(s, _) => s.len() / BYTES_PER_NODE,
        Val::Null => 0,
    }
}

/// Một tài nguyên của bản dựng.
#[derive(Debug, Clone)]
struct Res {
    obj: N,
    /// File nguồn của tài nguyên (chỉ số trong `files`).
    file: u32,
    /// Tên trước mọi phép đổi tên.
    original: String,
    /// Namespace lúc nạp (trống là `default`): target của patch khớp namespace
    /// gốc hay hiện tại, như Kustomize.
    orig_ns: String,
}

impl Res {
    fn kind(&self) -> &str {
        self.obj.get("kind").and_then(N::str).unwrap_or("")
    }

    fn name(&self) -> &str {
        self.obj.path(&["metadata", "name"]).and_then(N::str).unwrap_or("")
    }

    fn labels(&self) -> Vec<(String, String)> {
        match self.obj.path(&["metadata", "labels"]).map(|n| &n.v) {
            Some(Val::Map(es)) => es.iter().map(|e| (e.key.clone(), e.val.str().unwrap_or("").to_string())).collect(),
            _ => Vec::new(),
        }
    }
}

/// Một tài nguyên dựng ra, viết thành YAML.
#[derive(Debug, Clone)]
pub struct Doc {
    /// File nguồn của tài nguyên: manifest gốc, hay kustomization sinh ra nó.
    pub file: String,
    pub text: String,
    /// Nguồn của dòng thứ `i + 1`: file (chỉ số trong [`Build::files`]), dòng.
    pub lines: Vec<At>,
    /// Tên trước `namePrefix`/`nameSuffix`: `k8s:<tên>` khớp theo tên này.
    pub original: String,
}

/// Kết quả dựng một kustomization gốc.
#[derive(Debug, Clone, Default)]
pub struct Build {
    pub files: Vec<String>,
    pub docs: Vec<Doc>,
    /// Kustomization đã đi qua (thư mục), để biết patch của nó đã được áp.
    pub reached: Vec<String>,
}

/// Lý do cả bản dựng không đọc được: file (kèm dòng) và lý do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fail {
    pub file: String,
    pub why: String,
}

/// Đọc file trong repo (đã qua chốt chặn đường dẫn, trần cỡ, TM-47), và hỏi
/// một đường dẫn là thư mục hay file.
pub struct Fs<'a> {
    pub read: &'a dyn Fn(&str) -> Result<String, String>,
    /// `Some(true)`: thư mục; `Some(false)`: file; `None`: không có.
    pub kind: &'a dyn Fn(&str) -> Option<bool>,
    /// Bước dựng còn lại, dùng chung cho mọi bản dựng của repo.
    pub budget: &'a std::cell::Cell<usize>,
    /// Node cây còn được tạo, dùng chung cho mọi bản dựng của repo.
    pub nodes: &'a std::cell::Cell<usize>,
    /// YAML đã đọc, dùng chung cho mọi bản dựng: base, component dùng chung
    /// được đọc một lần (TM-44).
    pub parsed: &'a std::cell::RefCell<Parsed>,
}

/// YAML đã đọc theo đường dẫn: cây, hay dòng và lý do không đọc được.
pub type Parsed = HashMap<String, Result<std::rc::Rc<yaml::Yaml>, (usize, String)>>;

/// Một bộ nhãn: cặp khoá, giá trị kèm nguồn; có thêm vào selector, template không.
type LabelSet = (Vec<(String, String, At)>, bool, bool);

/// Luật `images`: tên, `newName`, `newTag`, `digest`, nguồn.
type ImageRule = (String, Option<String>, Option<String>, Option<String>, At);

struct Ctx<'a> {
    fs: &'a Fs<'a>,
    files: Vec<String>,
    file_ids: HashMap<String, u32>,
    steps: usize,
    reached: Vec<String>,
    /// Kết quả của kustomization (không phải component) đã dựng, theo thư mục:
    /// base dùng chung trỏ tới nhiều lần dựng một lần (TM-44). Kustomize dựng
    /// mỗi base độc lập với nơi trỏ tới nó.
    done: HashMap<String, Vec<Res>>,
}

impl Ctx<'_> {
    fn file_id(&mut self, f: &str) -> u32 {
        if let Some(&i) = self.file_ids.get(f) {
            return i;
        }
        let i = self.files.len() as u32;
        self.files.push(f.to_string());
        self.file_ids.insert(f.to_string(), i);
        i
    }

    fn fail(&self, file: u32, line: usize, why: impl Into<String>) -> Fail {
        let f = self.files.get(file as usize).cloned().unwrap_or_default();
        Fail { file: if line > 0 { format!("{f}:{line}") } else { f }, why: why.into() }
    }

    fn step(&mut self, n: usize, file: u32) -> Result<(), Fail> {
        self.steps += n;
        let left = self.fs.budget.get();
        if n > left {
            self.fs.budget.set(0);
            return Err(self.fail(file, 0, format!("quá {MAX_STEPS} bước dựng Kustomize cho cả repo")));
        }
        self.fs.budget.set(left - n);
        Ok(())
    }

    /// Tính `n` node mới vào ngân sách node chung (TM-44).
    fn grow(&mut self, n: usize, file: u32) -> Result<(), Fail> {
        let left = self.fs.nodes.get();
        if n > left {
            self.fs.nodes.set(0);
            return Err(self.fail(file, 0, format!("quá {MAX_NODES} node YAML dựng Kustomize cho cả repo")));
        }
        self.fs.nodes.set(left - n);
        Ok(())
    }

    /// Chép node YAML thành cây, có trần số node (alias nhân bản) và trần độ
    /// sâu (alias tới anchor đã lồng sâu, TM-44).
    fn tree(&mut self, n: Node<'_>, file: u32, line_offset: usize) -> Result<N, Fail> {
        self.tree_at(n, file, line_offset, 0)
    }

    fn tree_at(&mut self, n: Node<'_>, file: u32, line_offset: usize, depth: usize) -> Result<N, Fail> {
        // Chuỗi chép theo byte: alias tới một chuỗi lớn dùng lại nghìn lần.
        let bytes = n.as_str().map_or(0, str::len) + if n.is_map() { n.entries().map(|(k, _, _)| k.len()).sum() } else { 0 };
        self.grow(1 + bytes / BYTES_PER_NODE, file)?;
        if depth > MAX_TREE_DEPTH {
            return Err(self.fail(file, n.line() + line_offset, format!("YAML lồng sâu quá {MAX_TREE_DEPTH} cấp (kể cả qua alias)")));
        }
        let at = (file, (n.line() + line_offset) as u32);
        let v = if n.is_map() {
            let mut es = Vec::new();
            for (k, l, v) in n.entries() {
                es.push(Ent { key: k.to_string(), at: (file, (l + line_offset) as u32), val: self.tree_at(v, file, line_offset, depth + 1)? });
            }
            Val::Map(es)
        } else if n.is_seq() {
            let mut items = Vec::new();
            for v in n.items() {
                items.push(self.tree_at(v, file, line_offset, depth + 1)?);
            }
            Val::Seq(items)
        } else if let Some(s) = n.as_str() {
            Val::Scalar(s.to_string(), n.is_plain())
        } else {
            Val::Null
        };
        Ok(N { v, at })
    }

    fn read_yaml(&mut self, path: &str) -> Result<(u32, std::rc::Rc<yaml::Yaml>), Fail> {
        let id = self.file_id(path);
        let cached = self.fs.parsed.borrow().get(path).cloned();
        let r = match cached {
            Some(r) => r,
            None => {
                let r = match (self.fs.read)(path) {
                    Ok(text) => yaml::parse(&text).map(std::rc::Rc::new).map_err(|e| (e.line, format!("YAML: {}", e.msg))),
                    Err(why) => Err((0, why)),
                };
                self.fs.parsed.borrow_mut().insert(path.to_string(), r.clone());
                r
            }
        };
        r.map(|y| (id, y)).map_err(|(line, why)| self.fail(id, line, why))
    }

    fn kustomization_in(&self, dir: &str) -> Option<String> {
        ["kustomization.yaml", "kustomization.yml", "Kustomization"]
            .iter()
            .map(|n| if dir.is_empty() { (*n).to_string() } else { format!("{dir}/{n}") })
            .find(|p| (self.fs.kind)(p) == Some(false))
    }
}

/// Đường dẫn trong repo của `rel` tính từ `dir`; `None` nếu ra ngoài gốc,
/// tuyệt đối, hay là URL (TM-47).
fn join(dir: &str, rel: &str) -> Option<String> {
    super::deploy::within(dir, rel)
}

fn remote(rel: &str) -> bool {
    rel.contains("://") || rel.starts_with("github.com/") || rel.starts_with("git@") || rel.contains("?ref=")
}

/// Resource từ xa khi báo: chỉ máy chủ. URL có thể mang token
/// (`https://oauth2:TOKEN@…`, `?access_token=…`): không in (TM-46, TM-10).
fn remote_shown(rel: &str) -> String {
    let rest = rel.split_once("://").map_or(rel, |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    format!("{}/…", super::deploy::shown_path(host))
}

/// Dựng kustomization ở thư mục `dir`.
pub fn build(dir: &str, fs: &Fs<'_>) -> Result<Build, Fail> {
    let mut cx = Ctx { fs, files: Vec::new(), file_ids: HashMap::new(), steps: 0, reached: Vec::new(), done: HashMap::new() };
    let mut stack = Vec::new();
    let res = kustomize(&mut cx, dir, 0, &mut stack, false)?;
    let mut docs = Vec::new();
    for r in res {
        let mut text = String::new();
        let mut lines = Vec::new();
        write(&r.obj, 0, &mut text, &mut lines);
        docs.push(Doc { file: cx.files[r.file as usize].clone(), text, lines, original: r.original });
    }
    Ok(Build { files: cx.files, docs, reached: cx.reached })
}

/// Một kustomization (hay component, `component`): tài nguyên của nó sau mọi
/// phép biến đổi của chính nó.
fn kustomize(cx: &mut Ctx<'_>, dir: &str, depth: usize, stack: &mut Vec<String>, component: bool) -> Result<Vec<Res>, Fail> {
    if let Some(r) = cx.done.get(dir) {
        // Bản chép của base đã dựng là node mới: tính vào ngân sách node.
        let n: usize = r.iter().map(|x| size(&x.obj)).sum();
        let len = r.len();
        cx.step(1 + len, 0)?;
        cx.grow(n, 0)?;
        return Ok(cx.done[dir].clone());
    }
    let Some(path) = cx.kustomization_in(dir) else {
        let id = cx.file_id(dir);
        return Err(cx.fail(id, 0, "thư mục không có kustomization"));
    };
    if depth > MAX_DEPTH {
        let id = cx.file_id(&path);
        return Err(cx.fail(id, 0, format!("kustomization lồng sâu quá {MAX_DEPTH}")));
    }
    if stack.iter().any(|d| d == dir) {
        let id = cx.file_id(&path);
        return Err(cx.fail(id, 0, "kustomization trỏ vòng"));
    }
    stack.push(dir.to_string());
    if !cx.reached.iter().any(|d| d == dir) {
        cx.reached.push(dir.to_string());
    }
    let (kid, y) = cx.read_yaml(&path)?;
    // Mỗi lần vào một kustomization tính một bước: base dùng chung trỏ tới
    // nhiều lần (kim cương) nhân theo cấp số mà không có tài nguyên nào (TM-44).
    cx.step(1, kid)?;
    let Some(k) = y.documents().next() else {
        stack.pop();
        return Ok(Vec::new());
    };
    for (key, line, _) in k.entries() {
        if UNSUPPORTED.contains(&key) {
            return Err(cx.fail(kid, line, format!("`{key}` của Kustomize chưa được hỗ trợ")));
        }
    }
    allowed(cx, kid, k)?;
    let mut res: Vec<Res> = Vec::new();
    // `resources` (và `bases` cũ).
    for key in ["resources", "bases"] {
        for item in k.get(key).into_iter().flat_map(Node::items) {
            let Some(rel) = item.as_str() else { continue };
            if remote(rel) {
                return Err(cx.fail(kid, item.line(), format!("resource từ xa `{}` không tải", remote_shown(rel))));
            }
            let Some(p) = join(dir, rel) else {
                return Err(cx.fail(kid, item.line(), format!("resource `{}` trỏ ra ngoài dự án", super::deploy::shown_path(rel))));
            };
            match (cx.fs.kind)(&p) {
                Some(true) => res.extend(kustomize(cx, &p, depth + 1, stack, false)?),
                Some(false) => res.extend(load(cx, &p)?),
                None => return Err(cx.fail(kid, item.line(), format!("resource `{}` không có", super::deploy::shown_path(rel)))),
            }
            check_count(cx, &res, kid)?;
        }
    }
    // Như Kustomize: sinh ConfigMap sau `resources`, trước component (patch
    // của component nhắm được ConfigMap sinh ra ở đây).
    generate(cx, dir, kid, k, &mut res)?;
    // Component: tài nguyên và phép biến đổi của nó áp lên tập đang có.
    for item in k.get("components").into_iter().flat_map(Node::items) {
        let Some(rel) = item.as_str() else { continue };
        if remote(rel) {
            return Err(cx.fail(kid, item.line(), format!("component từ xa `{}` không tải", remote_shown(rel))));
        }
        let Some(p) = join(dir, rel) else {
            return Err(cx.fail(kid, item.line(), "component trỏ ra ngoài dự án"));
        };
        if (cx.fs.kind)(&p) != Some(true) {
            return Err(cx.fail(kid, item.line(), format!("component `{}` không phải thư mục", super::deploy::shown_path(rel))));
        }
        res = with_component(cx, &p, depth + 1, stack, res)?;
        check_count(cx, &res, kid)?;
    }
    let _ = component;
    check_dups(cx, &res, kid)?;
    transform(cx, dir, kid, k, &mut res)?;
    stack.pop();
    cx.done.insert(dir.to_string(), res.clone());
    Ok(res)
}

/// Áp component ở `dir` lên `base`: tài nguyên của component cộng vào, rồi
/// phép biến đổi của nó áp lên cả tập.
fn with_component(cx: &mut Ctx<'_>, dir: &str, depth: usize, stack: &mut Vec<String>, base: Vec<Res>) -> Result<Vec<Res>, Fail> {
    let Some(path) = cx.kustomization_in(dir) else {
        let id = cx.file_id(dir);
        return Err(cx.fail(id, 0, "component không có kustomization"));
    };
    if stack.iter().any(|d| d == dir) {
        let id = cx.file_id(&path);
        return Err(cx.fail(id, 0, "kustomization trỏ vòng"));
    }
    if depth > MAX_DEPTH {
        let id = cx.file_id(&path);
        return Err(cx.fail(id, 0, format!("kustomization lồng sâu quá {MAX_DEPTH}")));
    }
    stack.push(dir.to_string());
    if !cx.reached.iter().any(|d| d == dir) {
        cx.reached.push(dir.to_string());
    }
    let (kid, y) = cx.read_yaml(&path)?;
    // Mỗi lần vào một kustomization tính một bước: base dùng chung trỏ tới
    // nhiều lần (kim cương) nhân theo cấp số mà không có tài nguyên nào (TM-44).
    cx.step(1, kid)?;
    let Some(k) = y.documents().next() else {
        stack.pop();
        return Ok(base);
    };
    for (key, line, _) in k.entries() {
        if UNSUPPORTED.contains(&key) {
            return Err(cx.fail(kid, line, format!("`{key}` của Kustomize chưa được hỗ trợ")));
        }
    }
    allowed(cx, kid, k)?;
    let mut res = base;
    for key in ["resources", "bases"] {
        for item in k.get(key).into_iter().flat_map(Node::items) {
            let Some(rel) = item.as_str() else { continue };
            if remote(rel) {
                return Err(cx.fail(kid, item.line(), format!("resource từ xa `{}` không tải", remote_shown(rel))));
            }
            let Some(p) = join(dir, rel) else {
                return Err(cx.fail(kid, item.line(), "resource trỏ ra ngoài dự án"));
            };
            match (cx.fs.kind)(&p) {
                Some(true) => res.extend(kustomize(cx, &p, depth + 1, stack, false)?),
                Some(false) => res.extend(load(cx, &p)?),
                None => return Err(cx.fail(kid, item.line(), format!("resource `{}` không có", super::deploy::shown_path(rel)))),
            }
            check_count(cx, &res, kid)?;
        }
    }
    generate(cx, dir, kid, k, &mut res)?;
    for item in k.get("components").into_iter().flat_map(Node::items) {
        let Some(rel) = item.as_str() else { continue };
        if remote(rel) {
            return Err(cx.fail(kid, item.line(), format!("component từ xa `{}` không tải", remote_shown(rel))));
        }
        let Some(p) = join(dir, rel).filter(|p| (cx.fs.kind)(p) == Some(true)) else {
            return Err(cx.fail(kid, item.line(), format!("component `{}` không đọc được", super::deploy::shown_path(rel))));
        };
        res = with_component(cx, &p, depth + 1, stack, res)?;
    }
    check_dups(cx, &res, kid)?;
    transform(cx, dir, kid, k, &mut res)?;
    stack.pop();
    Ok(res)
}

fn check_count(cx: &mut Ctx<'_>, res: &[Res], kid: u32) -> Result<(), Fail> {
    if res.len() > MAX_RESOURCES {
        return Err(cx.fail(kid, 0, format!("quá {MAX_RESOURCES} tài nguyên trong bản dựng")));
    }
    Ok(())
}

fn check_dups(cx: &mut Ctx<'_>, res: &[Res], kid: u32) -> Result<(), Fail> {
    // Tài nguyên trùng id (loại, namespace, tên): Kustomize từ chối; base kéo
    // vào hai lần thì bản dựng không phải thứ được triển khai.
    cx.step(res.len(), kid)?;
    let mut seen = HashSet::new();
    for r in res {
        let ns = r.obj.path(&["metadata", "namespace"]).and_then(N::str).unwrap_or("");
        if !seen.insert((r.kind(), ns, r.name())) {
            return Err(cx.fail(r.file, 0, format!("tài nguyên trùng: {} `{}`", r.kind(), r.name())));
        }
    }
    Ok(())
}

/// Tài nguyên trong một file manifest: mỗi tài liệu có `kind`; `kind: List`
/// thì từng mục.
fn load(cx: &mut Ctx<'_>, path: &str) -> Result<Vec<Res>, Fail> {
    let (id, y) = cx.read_yaml(path)?;
    let mut out = Vec::new();
    for d in y.documents() {
        let items: Vec<Node<'_>> = if d.get("kind").and_then(Node::as_str).is_some_and(|k| k.ends_with("List")) && d.get("items").is_some() {
            d.get("items").into_iter().flat_map(Node::items).collect()
        } else {
            vec![d]
        };
        for n in items {
            if !n.is_map() || n.get("kind").is_none() {
                continue;
            }
            let obj = cx.tree(n, id, 0)?;
            let original = obj.path(&["metadata", "name"]).and_then(N::str).unwrap_or("").to_string();
            let orig_ns = obj.path(&["metadata", "namespace"]).and_then(N::str).unwrap_or("").to_string();
            out.push(Res { obj, file: id, original, orig_ns });
        }
    }
    Ok(out)
}

/// Phép biến đổi của một kustomization, theo thứ tự của Kustomize (sinh
/// ConfigMap đã làm trước component, ở bên gọi).
fn transform(cx: &mut Ctx<'_>, dir: &str, kid: u32, k: Node<'_>, res: &mut [Res]) -> Result<(), Fail> {
    // Thứ tự của Kustomize: patch strategic merge kiểu cũ, `patches`,
    // `namespace`, tiền tố/hậu tố, nhãn, `patchesJson6902`, `replicas`,
    // `images`. JSON 6902 áp sau đổi tên: đích viết theo tên, namespace mới.
    for item in k.get("patchesStrategicMerge").into_iter().flat_map(Node::items) {
        let Some(s) = item.as_str() else {
            return Err(cx.fail(kid, item.line(), "mục `patchesStrategicMerge` không phải chuỗi"));
        };
        let patches = patch_docs(cx, dir, kid, s, item.line())?;
        for p in patches {
            apply_smp(cx, kid, item.line(), None, p, res)?;
        }
    }
    for item in k.get("patches").into_iter().flat_map(Node::items) {
        let target = target(cx, kid, item.get("target"), item.line())?;
        let docs = match (item.get("path").and_then(Node::as_str), item.get("patch").and_then(Node::as_str)) {
            (Some(path), _) => patch_docs(cx, dir, kid, path, item.line())?,
            (None, Some(inline)) => inline_docs(cx, kid, inline, key_line(item, "patch"))?,
            _ => return Err(cx.fail(kid, item.line(), "mục `patches` không có `path` hay `patch` đọc được")),
        };
        for d in docs {
            if matches!(d.v, Val::Seq(_)) {
                apply_json6902(cx, kid, item.line(), target.as_ref(), d, res)?;
            } else {
                apply_smp(cx, kid, item.line(), target.as_ref(), d, res)?;
            }
        }
    }
    if let Some(ns) = k.get("namespace").and_then(Node::as_str) {
        let at = (kid, k.get("namespace").map_or(0, Node::line) as u32);
        for r in res.iter_mut() {
            if CLUSTER_KINDS.contains(&r.kind()) {
                continue;
            }
            if let Some(meta) = r.obj.child_map("metadata", at) {
                meta.set("namespace", at, scalar(ns, at));
            }
        }
    }
    let prefix = k.get("namePrefix").and_then(Node::as_str).unwrap_or("");
    let suffix = k.get("nameSuffix").and_then(Node::as_str).unwrap_or("");
    if !prefix.is_empty() || !suffix.is_empty() {
        let line = k.get("namePrefix").or_else(|| k.get("nameSuffix")).map_or(0, Node::line);
        rename(cx, kid, line, prefix, suffix, res)?;
    }
    labels(cx, kid, k, res)?;
    for item in k.get("patchesJson6902").into_iter().flat_map(Node::items) {
        let target = target(cx, kid, item.get("target"), item.line())?;
        let docs = match (item.get("path").and_then(Node::as_str), item.get("patch").and_then(Node::as_str)) {
            (Some(path), _) => patch_docs(cx, dir, kid, path, item.line())?,
            (None, Some(inline)) => inline_docs(cx, kid, inline, key_line(item, "patch"))?,
            _ => return Err(cx.fail(kid, item.line(), "mục `patchesJson6902` không có `path` hay `patch` đọc được")),
        };
        for d in docs {
            apply_json6902(cx, kid, item.line(), target.as_ref(), d, res)?;
        }
    }
    for item in k.get("replicas").into_iter().flat_map(Node::items) {
        let (Some(name), Some(count)) = (item.get("name").and_then(Node::as_str), item.get("count").and_then(Node::as_str)) else { continue };
        let at = (kid, item.line() as u32);
        cx.step(res.len(), kid)?;
        for r in res.iter_mut().filter(|r| r.name() == name || r.original == name) {
            if let Some(spec) = r.obj.child_map("spec", at) {
                spec.set("replicas", at, N { v: Val::Scalar(count.to_string(), true), at });
            }
        }
    }
    images(cx, kid, k, res)
}

/// Dòng của khoá `key` trong một mục (dòng `patch: |-`, không phải `- target:`).
fn key_line(item: Node<'_>, key: &str) -> usize {
    item.entries().find(|(k, _, _)| *k == key).map_or(item.line(), |(_, l, _)| l)
}

/// Khoá của kustomization và của từng mục mà archctl áp được (TM-45). Khoá
/// khác, kể cả khoá mới của Kustomize, làm cả bản dựng không đọc được: không
/// bỏ qua im lặng một thứ có thể đổi tên, env, image.
fn allowed(cx: &Ctx<'_>, kid: u32, k: Node<'_>) -> Result<(), Fail> {
    const ROOT: &[&str] = &[
        "apiVersion", "kind", "metadata", "resources", "bases", "components", "patches", "patchesStrategicMerge",
        "patchesJson6902", "namespace", "namePrefix", "nameSuffix", "commonLabels", "labels", "commonAnnotations",
        "images", "configMapGenerator", "secretGenerator", "generatorOptions", "replicas", "buildMetadata", "sortOptions",
    ];
    let items: &[(&str, &[&str])] = &[
        ("patches", &["path", "patch", "target"]),
        ("patchesJson6902", &["path", "patch", "target"]),
        ("labels", &["pairs", "includeSelectors", "includeTemplates"]),
        ("images", &["name", "newName", "newTag", "digest"]),
        ("configMapGenerator", &["name", "namespace", "behavior", "literals", "envs", "env", "files", "options"]),
        ("replicas", &["name", "count"]),
    ];
    for (key, line, v) in k.entries() {
        if !ROOT.contains(&key) {
            return Err(cx.fail(kid, line, format!("`{key}` của Kustomize chưa được hỗ trợ")));
        }
        if let Some((_, keys)) = items.iter().find(|(n, _)| *n == key) {
            for item in v.items() {
                for (k2, l2, _) in item.entries() {
                    if !keys.contains(&k2) {
                        return Err(cx.fail(kid, l2, format!("`{key}[].{k2}` của Kustomize chưa được hỗ trợ")));
                    }
                }
            }
        }
    }
    Ok(())
}

/// `configMapGenerator`: ConfigMap từ `literals`, `envs`/`env`, `files`. Tên
/// không thêm đuôi băm (thước đo bỏ đuôi đó ở phía kustomize). `behavior:
/// merge`/`replace` sửa ConfigMap đã có. `secretGenerator` không đọc (TM-46).
fn generate(cx: &mut Ctx<'_>, dir: &str, kid: u32, k: Node<'_>, res: &mut Vec<Res>) -> Result<(), Fail> {
    for g in k.get("configMapGenerator").into_iter().flat_map(Node::items) {
        let Some(name) = g.get("name").and_then(Node::as_str) else { continue };
        let at = (kid, g.line() as u32);
        let mut data: Vec<Ent> = Vec::new();
        for lit in g.get("literals").into_iter().flat_map(Node::items) {
            if let Some((key, val)) = lit.as_str().and_then(|s| s.split_once('=')) {
                let a = (kid, lit.line() as u32);
                // Như `ParseLiteralSource` của Kustomize: cặp nháy bao quanh bị bỏ
                // (file env thì giữ nguyên).
                let val = ['"', '\''].iter().find_map(|q| val.strip_prefix(*q)?.strip_suffix(*q)).unwrap_or(val);
                data.push(Ent { key: key.trim().to_string(), at: a, val: N { v: Val::Scalar(val.to_string(), false), at: a } });
            }
        }
        let envs: Vec<Node<'_>> = g.get("envs").into_iter().flat_map(Node::items).chain(g.get("env")).collect();
        for e in envs {
            let Some(rel) = e.as_str() else { continue };
            let Some(p) = join(dir, rel) else {
                return Err(cx.fail(kid, e.line(), "file env của configMapGenerator trỏ ra ngoài dự án"));
            };
            let id = cx.file_id(&p);
            let text = (cx.fs.read)(&p).map_err(|why| cx.fail(id, 0, why))?;
            for (n, raw) in text.lines().enumerate() {
                let l = raw.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                if let Some((key, val)) = l.split_once('=') {
                    let a = (id, n as u32 + 1);
                    data.push(Ent { key: key.trim().to_string(), at: a, val: N { v: Val::Scalar(val.to_string(), false), at: a } });
                }
            }
        }
        for f in g.get("files").into_iter().flat_map(Node::items) {
            let Some(spec) = f.as_str() else { continue };
            let (key, rel) = spec.split_once('=').unwrap_or((spec.rsplit('/').next().unwrap_or(spec), spec));
            let Some(p) = join(dir, rel) else {
                return Err(cx.fail(kid, f.line(), "file của configMapGenerator trỏ ra ngoài dự án"));
            };
            let id = cx.file_id(&p);
            let text = (cx.fs.read)(&p).map_err(|why| cx.fail(id, 0, why))?;
            let a = (kid, f.line() as u32);
            data.push(Ent { key: key.to_string(), at: a, val: N { v: Val::Scalar(text, false), at: a } });
        }
        cx.step(data.len() + 1 + res.len(), kid)?;
        let n: usize = data.iter().map(|e| 1 + e.key.len() / BYTES_PER_NODE + size(&e.val)).sum();
        cx.grow(n, kid)?;
        let behavior = g.get("behavior").and_then(Node::as_str).unwrap_or("create");
        let existing = res.iter_mut().find(|r| r.kind() == "ConfigMap" && (r.original == name || r.name() == name));
        match (behavior, existing) {
            ("merge", Some(r)) => {
                let d = r.obj.child_map("data", at).expect("map");
                // Đã trừ node của `data` ở trên: chỉ trừ bước tra khoá.
                let (mut steps, mut nodes) = (cx.fs.budget.get(), usize::MAX);
                let n = data.len();
                let ok = put_all(d, data.into_iter(), n, &mut steps, &mut nodes);
                cx.fs.budget.set(steps);
                if !ok {
                    return Err(cx.fail(kid, g.line(), format!("quá {MAX_STEPS} bước dựng Kustomize cho cả repo")));
                }
            }
            ("replace", Some(r)) => {
                r.obj.set("data", at, N { v: Val::Map(data), at });
            }
            ("merge" | "replace", None) => {
                return Err(cx.fail(kid, g.line(), format!("configMapGenerator `{name}` gộp vào ConfigMap không có")));
            }
            _ => {
                let mut meta = vec![Ent { key: "name".into(), at, val: scalar(name, at) }];
                if let Some(ns) = g.get("namespace").and_then(Node::as_str) {
                    meta.push(Ent { key: "namespace".into(), at, val: scalar(ns, at) });
                }
                let obj = N {
                    v: Val::Map(vec![
                        Ent { key: "apiVersion".into(), at, val: scalar("v1", at) },
                        Ent { key: "kind".into(), at, val: scalar("ConfigMap", at) },
                        Ent { key: "metadata".into(), at, val: N { v: Val::Map(meta), at } },
                        Ent { key: "data".into(), at, val: N { v: Val::Map(data), at } },
                    ]),
                    at,
                };
                let orig_ns = g.get("namespace").and_then(Node::as_str).unwrap_or("").to_string();
                res.push(Res { obj, file: kid, original: name.to_string(), orig_ns });
            }
        }
    }
    Ok(())
}

/// Đích của patch: `kind`, `name` (chữ, không regex), `namespace`,
/// `labelSelector` dạng `k=v,…`. Thứ khác thì không áp được (TM-45).
#[derive(Debug, Clone, Default)]
struct Target {
    kind: Option<String>,
    name: Option<String>,
    namespace: Option<String>,
    labels: Vec<(String, String)>,
}

fn literal(s: &str) -> bool {
    s.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ':'))
}

fn target(cx: &Ctx<'_>, kid: u32, t: Option<Node<'_>>, line: usize) -> Result<Option<Target>, Fail> {
    let Some(t) = t else { return Ok(None) };
    let mut out = Target::default();
    for (key, l, v) in t.entries() {
        let s = v.as_str().unwrap_or("");
        match key {
            "kind" => out.kind = Some(s.to_string()),
            "group" | "version" => {}
            "name" | "namespace" => {
                if !literal(s) {
                    return Err(cx.fail(kid, l, format!("target `{key}: {s}` theo regex chưa được hỗ trợ")));
                }
                if key == "name" {
                    out.name = Some(s.to_string());
                } else {
                    out.namespace = Some(s.to_string());
                }
            }
            "labelSelector" => {
                for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    match part.split_once('=') {
                        Some((a, b)) if literal(a.trim()) && literal(b.trim().trim_start_matches('=')) => {
                            out.labels.push((a.trim().to_string(), b.trim().trim_start_matches('=').trim().to_string()));
                        }
                        _ => return Err(cx.fail(kid, l, format!("labelSelector `{s}` chưa được hỗ trợ"))),
                    }
                }
            }
            other => return Err(cx.fail(kid, l, format!("target `{other}` chưa được hỗ trợ"))),
        }
    }
    let _ = line;
    Ok(Some(out))
}

fn matches(r: &Res, t: &Target) -> bool {
    t.kind.as_deref().is_none_or(|k| k == r.kind())
        && t.name.as_deref().is_none_or(|n| n == r.name() || n == r.original)
        && t.namespace.as_deref().is_none_or(|ns| {
            // Kustomize khớp namespace gốc hay hiện tại; trống là `default`.
            let norm = |x: &str| if x.is_empty() { "default".to_string() } else { x.to_string() };
            let cur = r.obj.path(&["metadata", "namespace"]).and_then(N::str).unwrap_or("");
            norm(ns) == norm(cur) || norm(ns) == norm(&r.orig_ns)
        })
        && t.labels.iter().all(|(k, v)| r.labels().iter().any(|(a, b)| a == k && b == v))
}

/// Nội dung patch từ file (mỗi tài liệu một patch).
fn patch_docs(cx: &mut Ctx<'_>, dir: &str, kid: u32, rel: &str, line: usize) -> Result<Vec<N>, Fail> {
    // `patchesStrategicMerge` cho phép viết thẳng nội dung thay đường dẫn.
    if rel.contains('\n') || rel.contains(": ") {
        return inline_docs(cx, kid, rel, line);
    }
    let Some(p) = join(dir, rel) else {
        return Err(cx.fail(kid, line, format!("patch `{}` trỏ ra ngoài dự án", super::deploy::shown_path(rel))));
    };
    let (id, y) = cx.read_yaml(&p)?;
    let mut out = Vec::new();
    for d in y.documents() {
        out.push(cx.tree(d, id, 0)?);
    }
    Ok(out)
}

/// Patch viết thẳng trong kustomization: dòng trỏ về kustomization.
fn inline_docs(cx: &mut Ctx<'_>, kid: u32, text: &str, line: usize) -> Result<Vec<N>, Fail> {
    let y = yaml::parse(text).map_err(|e| cx.fail(kid, line + e.line, format!("patch YAML: {}", e.msg)))?;
    let mut out = Vec::new();
    for d in y.documents() {
        out.push(cx.tree(d, kid, line)?);
    }
    Ok(out)
}

/// Strategic merge: đích theo `target`, hay theo `kind` và `metadata.name` của
/// patch.
fn apply_smp(cx: &mut Ctx<'_>, kid: u32, line: usize, t: Option<&Target>, patch: N, res: &mut [Res]) -> Result<(), Fail> {
    // Kustomize từ chối patch không phải map, và patch không `target` mà
    // thiếu `kind`, `metadata.name`: áp nó lên mọi tài nguyên là sai lặng lẽ.
    match &patch.v {
        Val::Map(_) => {}
        Val::Null => return Ok(()),
        _ => return Err(cx.fail(kid, line, "patch strategic merge không phải map")),
    }
    if t.is_none() && (patch.get("kind").and_then(N::str).is_none() || patch.path(&["metadata", "name"]).and_then(N::str).is_none()) {
        return Err(cx.fail(kid, line, "patch không có `target` thì phải có `kind` và `metadata.name`"));
    }
    let derived;
    let derived_target = t.is_none();
    let t = match t {
        Some(t) => t,
        None => {
            derived = Target {
                kind: patch.get("kind").and_then(N::str).map(str::to_string),
                name: patch.path(&["metadata", "name"]).and_then(N::str).map(str::to_string),
                namespace: patch.path(&["metadata", "namespace"]).and_then(N::str).map(str::to_string),
                labels: Vec::new(),
            };
            &derived
        }
    };
    cx.step(res.len(), kid)?;
    let mut hit = false;
    for r in res.iter_mut().filter(|r| matches(r, t)) {
        hit = true;
        cx.step(1, kid)?;
        // Bản chép patch cho từng đích là node mới (TM-44).
        cx.grow(size(&patch), kid)?;
        let mut steps = 0usize;
        // Tên trong patch không đổi tên tài nguyên.
        let mut p = patch.clone();
        if let Some(meta) = p.get_mut("metadata")
            && let Val::Map(es) = &mut meta.v
        {
            es.retain(|e| e.key != "name");
        }
        merge(&mut r.obj, &p, "", &mut steps).map_err(|why| cx.fail(kid, line, why))?;
        // `merge` tự dừng ở MAX_STEPS; trừ phần đã dùng vào ngân sách chung.
        cx.step(steps, kid)?;
    }
    // Patch không `target` mà không khớp tài nguyên nào: Kustomize báo lỗi;
    // bỏ qua thì một patch gõ sai tên lặng lẽ không áp.
    if !hit && derived_target {
        return Err(cx.fail(kid, line, format!(
            "patch không khớp tài nguyên nào ({} `{}`)",
            t.kind.as_deref().unwrap_or("?"),
            t.name.as_deref().unwrap_or("?")
        )));
    }
    Ok(())
}

/// Khoá gộp của danh sách theo tên trường, như Kubernetes.
fn merge_key(field: &str, parent: &str) -> Option<&'static str> {
    match field {
        "containers" | "initContainers" | "ephemeralContainers" | "env" | "volumes" | "imagePullSecrets" => Some("name"),
        "volumeMounts" => Some("mountPath"),
        "ports" if parent == "service" => Some("port"),
        "ports" => Some("containerPort"),
        _ => None,
    }
}

/// Gộp strategic merge `p` vào `base`. `field`: tên trường của `base`.
fn merge(base: &mut N, p: &N, field: &str, steps: &mut usize) -> Result<(), String> {
    *steps += 1;
    if *steps > MAX_STEPS {
        return Err(format!("quá {MAX_STEPS} bước gộp patch"));
    }
    match (&mut base.v, &p.v) {
        (Val::Map(bes), Val::Map(pes)) => {
            let parent = if bes.iter().any(|e| e.key == "selector" || e.key == "clusterIP" || e.key == "type") && field == "spec" {
                "service"
            } else {
                ""
            };
            for pe in pes {
                // Tìm khoá là tuyến tính theo số khoá: tính vào bước (TM-44).
                *steps += bes.len();
                if *steps > MAX_STEPS {
                    return Err(format!("quá {MAX_STEPS} bước gộp patch"));
                }
                if pe.key.starts_with('$') {
                    return Err(format!("directive `{}` của strategic merge chưa được hỗ trợ", pe.key));
                }
                match (bes.iter_mut().find(|e| e.key == pe.key), &pe.val.v) {
                    (Some(_), Val::Null) => bes.retain(|e| e.key != pe.key),
                    (None, Val::Null) => {}
                    (Some(be), _) => {
                        let key = merge_key(&pe.key, parent);
                        match (key, &mut be.val.v, &pe.val.v) {
                            (Some(k), Val::Seq(bi), Val::Seq(pi)) => merge_list(bi, pi, k, &pe.key, steps)?,
                            (_, Val::Map(_), Val::Map(_)) => merge(&mut be.val, &pe.val, &pe.key, steps)?,
                            // Giá trị bị thay cả: dòng là dòng của patch.
                            _ => {
                                merge(&mut be.val, &pe.val, &pe.key, steps)?;
                                be.at = pe.at;
                            }
                        }
                    }
                    (None, _) => bes.push(pe.clone()),
                }
            }
            Ok(())
        }
        _ => {
            *base = p.clone();
            Ok(())
        }
    }
}

fn merge_list(base: &mut Vec<N>, patch: &[N], key: &str, field: &str, steps: &mut usize) -> Result<(), String> {
    for item in patch {
        if let Val::Map(es) = &item.v
            && es.iter().any(|e| e.key.starts_with('$'))
        {
            return Err(format!("directive strategic merge trong `{field}` chưa được hỗ trợ"));
        }
        *steps += base.len();
        if *steps > MAX_STEPS {
            return Err(format!("quá {MAX_STEPS} bước gộp patch"));
        }
        let id = item.get(key).and_then(N::str);
        match id.and_then(|id| base.iter_mut().find(|b| b.get(key).and_then(N::str) == Some(id))) {
            // Phần tử bị patch sửa: dòng của nó là dòng của patch (env đổi giá
            // trị thì vi phạm trỏ về patch).
            Some(b) => {
                merge(b, item, field, steps)?;
                if field == "env" {
                    b.at = item.at;
                }
            }
            None => base.push(item.clone()),
        }
    }
    Ok(())
}

/// JSON 6902: `add`, `replace`, `remove`. `copy`, `move`, `test` thì không
/// áp được (TM-45).
fn apply_json6902(cx: &mut Ctx<'_>, kid: u32, line: usize, t: Option<&Target>, ops: N, res: &mut [Res]) -> Result<(), Fail> {
    let Some(t) = t else {
        return Err(cx.fail(kid, line, "patch JSON 6902 không có `target`"));
    };
    let list = match &ops.v {
        Val::Seq(list) => list,
        Val::Null => return Ok(()),
        _ => return Err(cx.fail(kid, line, "patch JSON 6902 không phải danh sách phép")),
    };
    cx.step(res.len(), kid)?;
    for r in res.iter_mut().filter(|r| matches(r, t)) {
        for op in list {
            cx.step(1, kid)?;
            let kind = op.get("op").and_then(N::str).unwrap_or("");
            let path = op.get("path").and_then(N::str).unwrap_or("");
            let at_line = op.at.1 as usize;
            let parts: Vec<String> = path
                .split('/')
                .skip(1)
                .map(|p| p.replace("~1", "/").replace("~0", "~"))
                .collect();
            if !path.starts_with('/') || parts.len() > yaml::MAX_DEPTH {
                return Err(cx.fail(op.at.0, at_line, format!("đường dẫn JSON 6902 `{path}` không hợp lệ")));
            }
            let value = op.get("value").cloned();
            if let Some(v) = &value {
                cx.grow(size(v), kid)?;
            }
            let at = value.as_ref().map_or(op.at, |v| v.at);
            let ok = match kind {
                "add" | "replace" => match value {
                    Some(v) => {
                        let ok = pointer_set(&mut r.obj, &parts, v, kind == "add");
                        if ok {
                            mark_env(&mut r.obj, &parts, at);
                        }
                        ok
                    }
                    None => false,
                },
                "remove" => pointer_remove(&mut r.obj, &parts),
                other => {
                    return Err(cx.fail(op.at.0, at_line, format!("phép JSON 6902 `{other}` chưa được hỗ trợ")));
                }
            };
            if !ok {
                return Err(cx.fail(op.at.0, at_line, format!("JSON 6902 `{kind} {path}` không áp được")));
            }
        }
    }
    Ok(())
}

fn pointer_set(n: &mut N, parts: &[String], v: N, add: bool) -> bool {
    let Some((last, head)) = parts.split_last() else {
        *n = v;
        return true;
    };
    let mut cur = n;
    for p in head {
        cur = match &mut cur.v {
            Val::Map(es) => match es.iter_mut().find(|e| &e.key == p) {
                Some(e) => &mut e.val,
                None => return false,
            },
            Val::Seq(items) => match p.parse::<usize>().ok().and_then(|i| items.get_mut(i)) {
                Some(x) => x,
                None => return false,
            },
            _ => return false,
        };
    }
    match &mut cur.v {
        Val::Map(es) => match es.iter_mut().find(|e| &e.key == last) {
            Some(e) => {
                e.at = v.at;
                e.val = v;
                true
            }
            None if add => {
                let at = v.at;
                es.push(Ent { key: last.clone(), at, val: v });
                true
            }
            None => false,
        },
        Val::Seq(items) => {
            if last == "-" && add {
                items.push(v);
                return true;
            }
            match last.parse::<usize>() {
                Ok(i) if add && i <= items.len() => {
                    items.insert(i, v);
                    true
                }
                Ok(i) if !add && i < items.len() => {
                    items[i] = v;
                    true
                }
                _ => false,
            }
        }
        _ => false,
    }
}

/// Mục env nằm trên đường dẫn JSON 6902 vừa sửa: dòng của nó là dòng của
/// patch (vi phạm từ env bị patch đổi trỏ về patch). Chỉ mục env: dòng của
/// container, workload đi cùng file manifest gốc.
fn mark_env(n: &mut N, parts: &[String], at: At) {
    let mut cur = n;
    let mut prev = "";
    for p in parts {
        let next = match &mut cur.v {
            Val::Map(es) => es.iter_mut().find(|e| &e.key == p).map(|e| &mut e.val),
            Val::Seq(items) => p.parse::<usize>().ok().and_then(|i| items.get_mut(i)),
            _ => None,
        };
        let Some(next) = next else { return };
        if prev == "env" {
            next.at = at;
            return;
        }
        prev = p;
        cur = next;
    }
}

fn pointer_remove(n: &mut N, parts: &[String]) -> bool {
    let Some((last, head)) = parts.split_last() else { return false };
    let mut cur = n;
    for p in head {
        cur = match &mut cur.v {
            Val::Map(es) => match es.iter_mut().find(|e| &e.key == p) {
                Some(e) => &mut e.val,
                None => return false,
            },
            Val::Seq(items) => match p.parse::<usize>().ok().and_then(|i| items.get_mut(i)) {
                Some(x) => x,
                None => return false,
            },
            _ => return false,
        };
    }
    match &mut cur.v {
        Val::Map(es) => {
            let before = es.len();
            es.retain(|e| &e.key != last);
            es.len() != before
        }
        Val::Seq(items) => match last.parse::<usize>() {
            Ok(i) if i < items.len() => {
                items.remove(i);
                true
            }
            _ => false,
        },
        _ => false,
    }
}

/// Mọi `spec` của pod trong một tài nguyên (workload, CronJob, Pod).
fn pod_specs(obj: &mut N) -> Vec<&mut N> {
    let kind = obj.get("kind").and_then(N::str).unwrap_or("").to_string();
    match kind.as_str() {
        "Pod" => obj.get_mut("spec").into_iter().collect(),
        "CronJob" => obj
            .get_mut("spec")
            .and_then(|s| s.get_mut("jobTemplate"))
            .and_then(|j| j.get_mut("spec"))
            .and_then(|s| s.get_mut("template"))
            .and_then(|t| t.get_mut("spec"))
            .into_iter()
            .collect(),
        _ => obj.get_mut("spec").and_then(|s| s.get_mut("template")).and_then(|t| t.get_mut("spec")).into_iter().collect(),
    }
}

/// `namePrefix`/`nameSuffix`: đổi tên mọi tài nguyên có namespace (trừ
/// `Namespace`, CRD), và cập nhật tham chiếu tới ConfigMap, Secret, Service,
/// ServiceAccount như Kustomize. Tên trong biến môi trường (`redis-cart:6379`)
/// thì không đổi, như Kustomize.
fn rename(cx: &mut Ctx<'_>, kid: u32, line: usize, prefix: &str, suffix: &str, res: &mut [Res]) -> Result<(), Fail> {
    let at = (kid, line as u32);
    let mut renamed: HashMap<(String, String), String> = HashMap::new();
    for r in res.iter_mut() {
        let kind = r.kind().to_string();
        if matches!(kind.as_str(), "Namespace" | "CustomResourceDefinition") {
            continue;
        }
        let old = r.name().to_string();
        let new = format!("{prefix}{old}{suffix}");
        if let Some(meta) = r.obj.get_mut("metadata") {
            meta.set("name", at, scalar(&new, at));
        }
        renamed.insert((kind, old), new);
    }
    cx.step(res.len(), kid)?;
    let to = |kind: &str, name: &str| renamed.get(&(kind.to_string(), name.to_string())).cloned();
    for r in res.iter_mut() {
        let kind = r.kind().to_string();
        // Ingress: backend Service.
        if kind == "Ingress" {
            fix_ingress(&mut r.obj, &to);
        }
        for spec in pod_specs(&mut r.obj) {
            if let Some(sa) = spec.get("serviceAccountName").and_then(N::str).and_then(|n| to("ServiceAccount", n)) {
                let a = spec.get("serviceAccountName").map_or(at, |n| n.at);
                spec.set("serviceAccountName", a, scalar(&sa, a));
            }
            for list in ["containers", "initContainers"] {
                let Some(Val::Seq(cs)) = spec.get_mut(list).map(|n| &mut n.v) else { continue };
                for c in cs {
                    if let Some(Val::Seq(env)) = c.get_mut("env").map(|n| &mut n.v) {
                        for e in env {
                            for (src, k) in [("configMapKeyRef", "ConfigMap"), ("secretKeyRef", "Secret")] {
                                if let Some(r) = e.get_mut("valueFrom").and_then(|v| v.get_mut(src)) {
                                    fix_name(r, k, &to);
                                }
                            }
                        }
                    }
                    if let Some(Val::Seq(from)) = c.get_mut("envFrom").map(|n| &mut n.v) {
                        for e in from {
                            for (src, k) in [("configMapRef", "ConfigMap"), ("secretRef", "Secret")] {
                                if let Some(r) = e.get_mut(src) {
                                    fix_name(r, k, &to);
                                }
                            }
                        }
                    }
                }
            }
            if let Some(Val::Seq(vols)) = spec.get_mut("volumes").map(|n| &mut n.v) {
                for v in vols {
                    if let Some(c) = v.get_mut("configMap") {
                        fix_name(c, "ConfigMap", &to);
                    }
                }
            }
        }
    }
    Ok(())
}

fn fix_name(n: &mut N, kind: &str, to: &dyn Fn(&str, &str) -> Option<String>) {
    if let Some(name) = n.get("name").and_then(N::str).and_then(|x| to(kind, x)) {
        let a = n.get("name").map_or(n.at, |x| x.at);
        n.set("name", a, scalar(&name, a));
    }
}

fn fix_ingress(obj: &mut N, to: &dyn Fn(&str, &str) -> Option<String>) {
    fn walk(n: &mut N, to: &dyn Fn(&str, &str) -> Option<String>, depth: usize) {
        if depth > 16 {
            return;
        }
        if let Some(svc) = n.get_mut("service") {
            fix_name(svc, "Service", to);
        }
        if let Some(name) = n.get("serviceName").and_then(N::str).and_then(|x| to("Service", x)) {
            let a = n.get("serviceName").map_or(n.at, |x| x.at);
            n.set("serviceName", a, scalar(&name, a));
        }
        match &mut n.v {
            Val::Map(es) => es.iter_mut().for_each(|e| walk(&mut e.val, to, depth + 1)),
            Val::Seq(items) => items.iter_mut().for_each(|x| walk(x, to, depth + 1)),
            _ => {}
        }
    }
    if let Some(spec) = obj.get_mut("spec") {
        walk(spec, to, 0);
    }
}

/// `commonLabels` (cả selector, template), `labels` (theo `includeSelectors`,
/// `includeTemplates`).
fn labels(cx: &mut Ctx<'_>, kid: u32, k: Node<'_>, res: &mut [Res]) -> Result<(), Fail> {
    let mut sets: Vec<LabelSet> = Vec::new();
    if let Some(c) = k.get("commonLabels") {
        let pairs = c.entries().map(|(a, l, v)| (a.to_string(), v.as_str().unwrap_or("").to_string(), (kid, l as u32))).collect();
        sets.push((pairs, true, true));
    }
    for item in k.get("labels").into_iter().flat_map(Node::items) {
        let pairs = item
            .get("pairs")
            .into_iter()
            .flat_map(Node::entries)
            .map(|(a, l, v)| (a.to_string(), v.as_str().unwrap_or("").to_string(), (kid, l as u32)))
            .collect();
        let sel = item.get("includeSelectors").and_then(Node::as_str) == Some("true");
        let tpl = sel || item.get("includeTemplates").and_then(Node::as_str) == Some("true");
        sets.push((pairs, sel, tpl));
    }
    if sets.is_empty() {
        return Ok(());
    }
    let workload = |k: &str| matches!(k, "Deployment" | "StatefulSet" | "DaemonSet" | "ReplicaSet" | "Job" | "ReplicationController");
    for r in res.iter_mut() {
        cx.step(1, kid)?;
        let kind = r.kind().to_string();
        // Mỗi map nhãn ghi qua chỉ mục khoá, trừ bước và node ngay khi ghi:
        // hàng chục nghìn cặp nhãn × nhiều map không được treo hook (TM-44).
        let (mut steps, mut nodes, mut ok) = (cx.fs.budget.get(), cx.fs.nodes.get(), true);
        for (pairs, sel, tpl) in &sets {
            let mut add = |n: &mut N| {
                let items = pairs.iter().map(|(a, b, at)| Ent { key: a.clone(), at: *at, val: scalar(b, *at) });
                ok = ok && put_all(n, items, pairs.len(), &mut steps, &mut nodes);
            };
            let at = r.obj.at;
            if let Some(meta) = r.obj.child_map("metadata", at) {
                let at = meta.at;
                if let Some(l) = meta.child_map("labels", at) {
                    add(l);
                }
            }
            if *tpl && (workload(&kind) || kind == "CronJob") {
                let tmpl = if kind == "CronJob" {
                    r.obj.get_mut("spec").and_then(|s| s.get_mut("jobTemplate")).and_then(|j| j.get_mut("spec")).and_then(|s| s.get_mut("template"))
                } else {
                    r.obj.get_mut("spec").and_then(|s| s.get_mut("template"))
                };
                if let Some(t) = tmpl {
                    let at = t.at;
                    if let Some(meta) = t.child_map("metadata", at) {
                        let at = meta.at;
                        if let Some(l) = meta.child_map("labels", at) {
                            add(l);
                        }
                    }
                }
            }
            if *sel {
                match kind.as_str() {
                    "Service" => {
                        if let Some(spec) = r.obj.get_mut("spec") {
                            let at = spec.at;
                            if let Some(s) = spec.child_map("selector", at) {
                                add(s);
                            }
                        }
                    }
                    k if workload(k) && k != "Job" => {
                        if let Some(spec) = r.obj.get_mut("spec") {
                            let at = spec.at;
                            if let Some(s) = spec.child_map("selector", at) {
                                let at = s.at;
                                if let Some(m) = s.child_map("matchLabels", at) {
                                    add(m);
                                }
                            }
                        }
                    }
                    // Như Kustomize (`create: false`): chỉ thêm vào selector đã
                    // có `matchLabels`, cả ở `ingress.from`, `egress.to`;
                    // `podSelector: {}` (mọi pod) giữ nguyên.
                    "NetworkPolicy" => {
                        if let Some(spec) = r.obj.get_mut("spec") {
                            if let Some(m) = spec.get_mut("podSelector").and_then(|s| s.get_mut("matchLabels")) {
                                add(m);
                            }
                            for (side, peers) in [("ingress", "from"), ("egress", "to")] {
                                let Some(Val::Seq(rules)) = spec.get_mut(side).map(|n| &mut n.v) else { continue };
                                for rule in rules {
                                    let Some(Val::Seq(list)) = rule.get_mut(peers).map(|n| &mut n.v) else { continue };
                                    for peer in list {
                                        if let Some(m) = peer.get_mut("podSelector").and_then(|s| s.get_mut("matchLabels")) {
                                            add(m);
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        cx.fs.budget.set(steps);
        cx.fs.nodes.set(nodes);
        if !ok {
            return Err(cx.fail(kid, 0, format!("quá {MAX_STEPS} bước hay {MAX_NODES} node dựng Kustomize cho cả repo (nhãn)")));
        }
    }
    Ok(())
}

/// `images`: đổi tên, tag, digest của image trong container khớp `name`.
/// Luật tra theo tên (luật đầu tiên thắng), mỗi container một bước (TM-44).
fn images(cx: &mut Ctx<'_>, kid: u32, k: Node<'_>, res: &mut [Res]) -> Result<(), Fail> {
    let rules: Vec<ImageRule> = k
        .get("images")
        .into_iter()
        .flat_map(Node::items)
        .filter_map(|i| {
            let name = i.get("name").and_then(Node::as_str)?.to_string();
            let s = |k: &str| i.get(k).and_then(Node::as_str).map(str::to_string);
            Some((name, s("newName"), s("newTag"), s("digest"), (kid, i.line() as u32)))
        })
        .collect();
    if rules.is_empty() {
        return Ok(());
    }
    cx.step(rules.len(), kid)?;
    // Như Kustomize, các luật áp lần lượt theo thứ tự: luật sau khớp tên mà
    // luật trước để lại (hai luật cùng tên thì luật sau thắng tag).
    let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, r) in rules.iter().enumerate() {
        by_name.entry(r.0.as_str()).or_default().push(i);
    }
    for r in res.iter_mut() {
        for spec in pod_specs(&mut r.obj) {
            for list in ["containers", "initContainers"] {
                let Some(Val::Seq(cs)) = spec.get_mut(list).map(|n| &mut n.v) else { continue };
                for c in cs {
                    cx.step(1, kid)?;
                    let Some(mut image) = c.get("image").and_then(N::str).map(str::to_string) else { continue };
                    let mut next = 0usize;
                    let mut last = None;
                    loop {
                        let (full, _) = super::k8s::image_name(&image);
                        let Some(list) = by_name.get(full) else { break };
                        let Some(&i) = list.get(list.partition_point(|&i| i < next)) else { break };
                        cx.step(1, kid)?;
                        let (_, new_name, tag, digest, at) = &rules[i];
                        let name = new_name.clone().unwrap_or_else(|| full.to_string());
                        let rest = &image[full.len()..];
                        image = match (digest, tag) {
                            (Some(d), Some(t)) => format!("{name}:{t}@{d}"),
                            (Some(d), None) => format!("{name}@{d}"),
                            (None, Some(t)) => format!("{name}:{t}"),
                            (None, None) => format!("{name}{rest}"),
                        };
                        last = Some(*at);
                        next = i + 1;
                    }
                    if let Some(at) = last {
                        c.set("image", at, scalar(&image, at));
                    }
                }
            }
        }
    }
    Ok(())
}

/// Viết cây thành YAML khối, ghi nguồn của từng dòng.
fn write(n: &N, indent: usize, out: &mut String, lines: &mut Vec<At>) {
    match &n.v {
        Val::Map(es) => {
            for e in es {
                write_entry(e, indent, out, lines);
            }
        }
        _ => {
            out.push_str(&" ".repeat(indent));
            out.push_str(&inline(n));
            out.push('\n');
            lines.push(n.at);
        }
    }
}

fn write_entry(e: &Ent, indent: usize, out: &mut String, lines: &mut Vec<At>) {
    let pad = " ".repeat(indent);
    let key = quote_key(&e.key);
    match &e.val.v {
        Val::Map(es) if !es.is_empty() => {
            out.push_str(&format!("{pad}{key}:\n"));
            lines.push(e.at);
            write(&e.val, indent + 2, out, lines);
        }
        Val::Seq(items) if !items.is_empty() => {
            out.push_str(&format!("{pad}{key}:\n"));
            lines.push(e.at);
            for item in items {
                write_item(item, indent + 2, out, lines);
            }
        }
        _ => {
            out.push_str(&format!("{pad}{key}: {}\n", inline(&e.val)));
            lines.push(e.at);
        }
    }
}

fn write_item(n: &N, indent: usize, out: &mut String, lines: &mut Vec<At>) {
    let pad = " ".repeat(indent);
    match &n.v {
        Val::Map(es) if !es.is_empty() => {
            // Mục đầu trên dòng `- `, các mục sau thụt vào.
            let first = &es[0];
            let mut head = String::new();
            let mut head_lines = Vec::new();
            write_entry(first, 0, &mut head, &mut head_lines);
            let mut iter = head.lines();
            if let Some(l) = iter.next() {
                out.push_str(&format!("{pad}- {l}\n"));
            }
            for l in iter {
                out.push_str(&format!("{pad}  {l}\n"));
            }
            // Dòng của mục đầu là dòng của cả phần tử: bộ đọc lấy dòng phần tử.
            if let Some(f) = head_lines.first_mut() {
                *f = n.at;
            }
            lines.extend(head_lines);
            for e in &es[1..] {
                write_entry(e, indent + 2, out, lines);
            }
        }
        Val::Seq(items) if !items.is_empty() => {
            out.push_str(&format!("{pad}-\n"));
            lines.push(n.at);
            for item in items {
                write_item(item, indent + 2, out, lines);
            }
        }
        _ => {
            out.push_str(&format!("{pad}- {}\n", inline(n)));
            lines.push(n.at);
        }
    }
}

fn inline(n: &N) -> String {
    match &n.v {
        Val::Null => "null".into(),
        Val::Map(_) => "{}".into(),
        Val::Seq(_) => "[]".into(),
        // Viết trần khi đọc lại vẫn là đúng chuỗi đó: chuỗi gốc có nháy mà
        // trông như bool, số thì giữ nháy.
        Val::Scalar(s, plain) => {
            if safe_plain(s) && (*plain || !typed(s)) {
                s.clone()
            } else {
                crate::json::string(s)
            }
        }
    }
}

/// Chuỗi mà YAML đọc trần thành bool, số hay null.
fn typed(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "true" | "false" | "yes" | "no" | "on" | "off" | "y" | "n" | "null" | "~")
        || s.parse::<f64>().is_ok()
}

fn safe_plain(s: &str) -> bool {
    !s.is_empty()
        && !s.contains(['\n', '#', '"', '\'', '{', '}', '[', ']', ',', '&', '*', '!', '|', '>', '%', '@', '`'])
        && !s.contains(": ")
        && !s.ends_with(':')
        && !s.starts_with([' ', '-', '?', ':'])
        && !s.ends_with(' ')
}

fn quote_key(k: &str) -> String {
    if safe_plain(k) && !k.contains(':') { k.to_string() } else { crate::json::string(k) }
}

/// Kustomization là component (`kind: Component`): chỉ dùng qua `components`
/// của kustomization khác, không dựng một mình.
pub fn is_component(text: &str) -> bool {
    yaml::parse(text)
        .ok()
        .and_then(|y| y.documents().next().and_then(|d| d.get("kind")).and_then(Node::as_str).map(|k| k == "Component"))
        .unwrap_or(false)
}

/// Kustomization gốc: không bị kustomization nào khác trỏ tới qua `resources`,
/// `bases`, `components`. `dirs`: thư mục có kustomization; `refs(d)`: các
/// thư mục mà kustomization ở `d` trỏ tới.
pub fn roots(dirs: &[String], refs: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {
    let referenced: HashSet<String> = dirs.iter().flat_map(|d| refs(d)).collect();
    dirs.iter().filter(|d| !referenced.contains(*d)).cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn fs_of(files: &BTreeMap<String, String>) -> (impl Fn(&str) -> Result<String, String> + '_, impl Fn(&str) -> Option<bool> + '_) {
        let read = move |p: &str| files.get(p).cloned().ok_or_else(|| "không có".to_string());
        let kind = move |p: &str| {
            if files.contains_key(p) {
                Some(false)
            } else if files.keys().any(|k| k.starts_with(&format!("{p}/"))) {
                Some(true)
            } else {
                None
            }
        };
        (read, kind)
    }

    fn run(files: &[(&str, &str)], dir: &str) -> Result<Build, Fail> {
        let map: BTreeMap<String, String> = files.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        let (read, kind) = fs_of(&map);
        let budget = std::cell::Cell::new(MAX_STEPS);
        let nodes = std::cell::Cell::new(MAX_NODES);
        let parsed = std::cell::RefCell::new(HashMap::new());
        build(dir, &Fs { read: &read, kind: &kind, budget: &budget, nodes: &nodes, parsed: &parsed })
    }

    const BASE: &[(&str, &str)] = &[
        ("base/kustomization.yaml", "resources:\n- app.yaml\n"),
        (
            "base/app.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: cart\n  labels: {app: cart}\nspec:\n  selector:\n    matchLabels: {app: cart}\n  template:\n    metadata:\n      labels: {app: cart}\n    spec:\n      containers:\n      - name: server\n        image: cart:1\n        env:\n        - name: REDIS_ADDR\n          value: redis-cart:6379\n        envFrom:\n        - configMapRef: {name: cfg}\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: cart\nspec:\n  selector: {app: cart}\n  ports:\n  - port: 80\n",
        ),
    ];

    fn with(extra: &[(&'static str, &'static str)]) -> Vec<(&'static str, &'static str)> {
        BASE.iter().copied().chain(extra.iter().copied()).collect()
    }

    #[test]
    fn overlay_patch_ten_nhan_image() {
        let files = with(&[
            (
                "prod/kustomization.yaml",
                "resources:\n- ../base\nnamePrefix: prod-\ncommonLabels:\n  env: prod\nimages:\n- name: cart\n  newName: reg/cart\n  newTag: \"2\"\nconfigMapGenerator:\n- name: cfg\n  literals:\n  - MODE=fast\npatches:\n- path: env.yaml\n",
            ),
            (
                "prod/env.yaml",
                "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: cart\nspec:\n  template:\n    spec:\n      containers:\n      - name: server\n        env:\n        - name: REDIS_ADDR\n          value: memorystore:6379\n",
            ),
        ]);
        let b = run(&files, "prod").unwrap();
        let dep = b.docs.iter().find(|d| d.text.contains("kind: Deployment")).unwrap();
        assert_eq!(dep.original, "cart");
        assert!(dep.text.contains("name: prod-cart"), "{}", dep.text);
        assert!(dep.text.contains("image: reg/cart:2"), "{}", dep.text);
        assert!(dep.text.contains("value: memorystore:6379"), "{}", dep.text);
        assert!(dep.text.contains("env: prod"), "{}", dep.text);
        // Tham chiếu ConfigMap đổi theo tên mới.
        assert!(dep.text.contains("name: prod-cfg"), "{}", dep.text);
        // Dòng của giá trị từ patch trỏ về file patch.
        let line = dep.text.lines().position(|l| l.contains("memorystore")).unwrap();
        let (f, l) = dep.lines[line];
        assert_eq!((b.files[f as usize].as_str(), l), ("prod/env.yaml", 12));
        // Dòng giá trị gốc trỏ về manifest gốc.
        let line = dep.text.lines().position(|l| l.contains("image:")).unwrap();
        assert_eq!(b.files[dep.lines[line].0 as usize], "prod/kustomization.yaml");
        let svc = b.docs.iter().find(|d| d.text.contains("kind: Service")).unwrap();
        assert!(svc.text.contains("name: prod-cart") && svc.text.contains("env: prod"), "{}", svc.text);
        assert!(b.docs.iter().any(|d| d.text.contains("kind: ConfigMap") && d.text.contains("MODE: fast")));
    }

    #[test]
    fn json6902_va_component() {
        let files = with(&[
            ("comp/kustomization.yaml", "apiVersion: kustomize.config.k8s.io/v1alpha1\nkind: Component\npatches:\n- target: {kind: Deployment, name: cart}\n  patch: |-\n    - op: replace\n      path: /spec/template/spec/containers/0/env/0/value\n      value: other:1\n"),
            ("o/kustomization.yaml", "resources:\n- ../base\ncomponents:\n- ../comp\n"),
        ]);
        let b = run(&files, "o").unwrap();
        assert!(b.docs.iter().any(|d| d.text.contains("value: other:1")), "{:?}", b.docs);
        assert!(b.reached.contains(&"comp".to_string()));
    }

    #[test]
    fn ngoai_tap_con_thi_khong_dung() {
        for (k, want) in [
            ("resources:\n- ../base\nreplacements:\n- path: r.yaml\n", "replacements"),
            ("resources:\n- github.com/x/y?ref=v1\n", "từ xa"),
            ("resources:\n- ../../etc\n", "ngoài dự án"),
            ("resources:\n- ../base\npatches:\n- target: {kind: Deployment, name: \"ca.*\"}\n  patch: |-\n    - op: remove\n      path: /spec\n", "regex"),
            ("resources:\n- ../base\npatches:\n- patch: |-\n    apiVersion: apps/v1\n    kind: Deployment\n    metadata: {name: cart}\n    spec:\n      $patch: replace\n", "directive"),
            ("resources:\n- ../loop\n", "vòng"),
        ] {
            let files = with(&[("o/kustomization.yaml", k), ("loop/kustomization.yaml", "resources:\n- ../loop\n")]);
            let err = run(&files, "o").unwrap_err();
            assert!(err.why.contains(want), "{k}: {err:?}");
        }
    }

    #[test]
    fn alias_nhan_ban_co_tran() {
        // Mỗi tầng alias nhân đôi: cây chép ra có trần node (TM-44).
        let mut y = String::from("a0: &a0 [x, x]\n");
        for i in 1..40 {
            y.push_str(&format!("a{i}: &a{i} [*a{}, *a{}]\n", i - 1, i - 1));
        }
        let doc = format!("apiVersion: v1\nkind: ConfigMap\nmetadata: {{name: c}}\ndata:\n  big: x\nextra:\n{}", y.lines().map(|l| format!("  {l}\n")).collect::<String>());
        let files = [("k/kustomization.yaml", "resources:\n- c.yaml\n"), ("k/c.yaml", doc.as_str())];
        let t = crate::testclock::Clock::start();
        let err = run(&files, "k");
        assert!(t.elapsed().as_secs_f64() < 2.0);
        assert!(err.is_err_and(|e| e.why.contains("node")));
    }

    #[test]
    fn alias_long_sau_thi_dung_khong_tran_stack() {
        // Alias tới anchor đã lồng sâu cộng dồn độ sâu quá trần của bộ đọc YAML:
        // chép cây phải dừng ở trần độ sâu, không tràn stack (TM-44).
        let mut y = format!("a0: &a0 {}z{}\n", "[".repeat(60), "]".repeat(60));
        for i in 1..40 {
            y.push_str(&format!("a{i}: &a{i} {}*a{}{}\n", "[".repeat(60), i - 1, "]".repeat(60)));
        }
        let doc = format!("{y}---\napiVersion: v1\nkind: ConfigMap\nmetadata: {{name: x}}\ndata:\n  x: *a39\n");
        let files = [("k/kustomization.yaml", "resources:\n- c.yaml\n"), ("k/c.yaml", doc.as_str())];
        let err = run(&files, "k").unwrap_err();
        assert!(err.why.contains("lồng sâu"), "{err:?}");
    }

    #[test]
    fn base_dung_lai_tinh_vao_ngan_sach_node() {
        // Base 2^17 node trỏ tới từ 8 overlay trong một gốc: mỗi bản chép từ bộ
        // nhớ đệm là node mới, vượt trần node chung thì dừng (TM-44).
        let mut y = String::from("a0: &a0 [x, x]\n");
        for i in 1..17 {
            y.push_str(&format!("  a{i}: &a{i} [*a{}, *a{}]\n", i - 1, i - 1));
        }
        let base = format!("apiVersion: v1\nkind: ConfigMap\nmetadata: {{name: big}}\ndata:\n  {y}");
        let mut files: Vec<(String, String)> = vec![
            ("base/kustomization.yaml".into(), "resources:\n- c.yaml\n".into()),
            ("base/c.yaml".into(), base),
        ];
        let mut root = String::from("resources:\n");
        for n in 0..8 {
            files.push((format!("o{n}/kustomization.yaml"), format!("resources:\n- ../base\nnamePrefix: p{n}-\n")));
            root.push_str(&format!("- ../o{n}\n"));
        }
        files.push(("all/kustomization.yaml".into(), root));
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let err = run(&refs, "all").unwrap_err();
        assert!(err.why.contains("node"), "{err:?}");
        // Một overlay thì dưới trần.
        assert!(run(&refs, "o0").is_ok());
    }

    #[test]
    fn target_namespace_khop_goc_hay_default() {
        // Kustomize khớp namespace gốc hay hiện tại; trống là `default`.
        let k = "resources:\n- ../base\npatches:\n- target: {kind: Deployment, name: cart, namespace: default}\n  patch: |-\n    - op: replace\n      path: /spec/template/spec/containers/0/env/0/value\n      value: pg:5432\n";
        let files = with(&[("o/kustomization.yaml", k)]);
        let b = run(&files, "o").unwrap();
        assert!(b.docs.iter().any(|d| d.text.contains("value: pg:5432")));
        let files = with(&[
            ("mid/kustomization.yaml", "resources:\n- ../base\nnamespace: dev\n"),
            ("top/kustomization.yaml", "resources:\n- ../mid\npatches:\n- target: {kind: Deployment, name: cart, namespace: default}\n  patch: |-\n    - op: replace\n      path: /spec/template/spec/containers/0/env/0/value\n      value: pg:5432\n"),
        ]);
        let b = run(&files, "top").unwrap();
        assert!(b.docs.iter().any(|d| d.text.contains("value: pg:5432")));
    }

    #[test]
    fn configmap_sinh_truoc_component_va_bo_nhay_literal() {
        let files = with(&[
            ("comp/kustomization.yaml", "apiVersion: kustomize.config.k8s.io/v1alpha1\nkind: Component\npatches:\n- target: {kind: ConfigMap, name: cfg2}\n  patch: |-\n    - op: replace\n      path: /data/REDIS\n      value: pg:5432\n"),
            ("o/kustomization.yaml", "resources:\n- ../base\nconfigMapGenerator:\n- name: cfg2\n  literals:\n  - REDIS=redis-cart:6379\n  - 'Q=\"pg:5432\"'\ncomponents:\n- ../comp\n"),
        ]);
        let b = run(&files, "o").unwrap();
        let cm = b.docs.iter().find(|d| d.original == "cfg2").unwrap();
        assert!(cm.text.contains("REDIS: pg:5432"), "{}", cm.text);
        assert!(cm.text.contains("Q: pg:5432") || cm.text.contains("Q: 'pg:5432'") || cm.text.contains("Q: \"pg:5432\""), "{}", cm.text);
        assert!(!cm.text.contains("\\\""), "{}", cm.text);
    }

    #[test]
    fn patch_sai_dang_thi_khong_dung() {
        for (k, want) in [
            ("resources:\n- ../base\npatches:\n- patch: \"spec: {replicas: 3}\"\n", "kind"),
            ("resources:\n- ../base\npatches:\n- patch: hello\n", "không phải map"),
            ("resources:\n- ../base\npatches:\n- target: {kind: Deployment}\n", "path"),
            ("resources:\n- ../base\npatchesJson6902:\n- target: {kind: Deployment, name: cart}\n  patch: \"op: remove\"\n", "danh sách"),
        ] {
            let files = with(&[("o/kustomization.yaml", k)]);
            let err = run(&files, "o").unwrap_err();
            assert!(err.why.contains(want), "{k}: {err:?}");
        }
    }

    #[test]
    fn url_tu_xa_khong_in_token() {
        for k in [
            "resources:\n- https://oauth2:glpat-SECRET123@gitlab.example.com/org/infra.git//base?ref=v1\n",
            "resources:\n- ../base\ncomponents:\n- https://x:SECRET123@git.example.com/c?ref=1\n",
        ] {
            let files = with(&[("o/kustomization.yaml", k)]);
            let err = run(&files, "o").unwrap_err();
            assert!(err.why.contains("từ xa") && !err.why.contains("SECRET123"), "{err:?}");
        }
    }

    #[test]
    fn nhieu_luat_images_khong_bac_hai() {
        let mut k = String::from("resources:\n- c.yaml\nimages:\n");
        for i in 0..20_000 {
            k.push_str(&format!("- name: img{i}\n  newTag: v{i}\n"));
        }
        let cs: Vec<String> = (0..100).map(|i| format!("{{name: c{i}, image: img{i}}}")).collect();
        let mut c = format!("kind: X\napiVersion: v1\nmetadata: {{name: a}}\nx: &cs [{}]\n", cs.join(", "));
        for d in 0..200 {
            c.push_str(&format!("---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: d{d}}}\nspec:\n  template:\n    spec:\n      containers: *cs\n"));
        }
        let files = [("k/kustomization.yaml", k.as_str()), ("k/c.yaml", c.as_str())];
        let t = crate::testclock::Clock::start();
        let b = run(&files, "k").unwrap();
        assert!(t.elapsed().as_secs_f64() < 2.0);
        assert!(b.docs.iter().any(|d| d.text.contains("image: img7:v7")));
    }

    #[test]
    fn alias_toi_chuoi_lon_tinh_theo_byte() {
        // Alias tới một chuỗi 700 KB dùng lại 900 lần: trần tính cả byte chuỗi,
        // không chỉ số node (TM-44).
        let mut c = format!("defs: &s {}\n---\napiVersion: v1\nkind: ConfigMap\nmetadata: {{name: m}}\ndata:\n", "x".repeat(700_000));
        for i in 0..900 {
            c.push_str(&format!("  k{i}: *s\n"));
        }
        let files = [("k/kustomization.yaml", "resources:\n- c.yaml\n"), ("k/c.yaml", c.as_str())];
        let err = run(&files, "k").unwrap_err();
        assert!(err.why.contains("node"), "{err:?}");
    }

    #[test]
    fn hai_luat_images_cung_ten_ap_lan_luot() {
        // Như Kustomize: luật áp lần lượt, luật sau cùng tên thắng tag; luật sau
        // khớp tên mà luật trước đổi ra.
        let d = "apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: cart}\nspec:\n  template:\n    spec:\n      containers: [{name: s, image: cart:1}]\n";
        for (k, want) in [
            ("images:\n- {name: cart, newTag: \"2\"}\n- {name: cart, newTag: \"3\"}\n", "image: cart:3"),
            ("images:\n- {name: cart, newName: reg/cart}\n- {name: reg/cart, newTag: \"4\"}\n", "image: reg/cart:4"),
        ] {
            let kz = format!("resources:\n- d.yaml\n{k}");
            let files = [("k/kustomization.yaml", kz.as_str()), ("k/d.yaml", d)];
            let b = run(&files, "k").unwrap();
            assert!(b.docs[0].text.contains(want), "{k}: {}", b.docs[0].text);
        }
    }

    #[test]
    fn nhieu_nhan_va_merge_khong_bac_hai() {
        // 10 000 cặp `commonLabels` trên nhiều tài nguyên, và `behavior: merge`
        // 10 000 mục: ghi qua chỉ mục khoá, không bậc hai (TM-44; bậc hai là
        // khoảng 10^9 phép so).
        let mut k = String::from("resources:\n- c.yaml\ncommonLabels:\n");
        for i in 0..10_000 {
            k.push_str(&format!("  l{i}: v\n"));
        }
        k.push_str("configMapGenerator:\n- name: m0\n  behavior: merge\n  literals:\n");
        for i in 0..10_000 {
            k.push_str(&format!("  - k{i}=v\n"));
        }
        for (n, fits) in [(10, true), (200, false)] {
            let mut c = String::new();
            for d in 0..n {
                c.push_str(&format!("---\napiVersion: v1\nkind: ConfigMap\nmetadata: {{name: m{d}}}\ndata: {{a: b}}\n"));
            }
            let files = [("k/kustomization.yaml", k.as_str()), ("k/c.yaml", c.as_str())];
            let t = crate::testclock::Clock::start();
            let r = run(&files, "k");
            assert!(t.elapsed().as_secs_f64() < 3.0, "{n}: {:?}", t.elapsed());
            match r {
                Ok(b) => {
                    assert!(fits);
                    assert!(b.docs.iter().any(|d| d.text.contains("l9999: v") && d.text.contains("k9999: v")));
                }
                // Vượt trần thì cả bản dựng "chưa đọc được", không treo.
                Err(e) => assert!(!fits && e.why.contains("nhãn"), "{e:?}"),
            }
        }
    }

    #[test]
    fn goc_la_khong_bi_tro_toi() {
        let dirs = vec!["base".to_string(), "prod".to_string(), "comp".to_string()];
        let refs = |d: &str| match d {
            "prod" => vec!["base".to_string(), "comp".to_string()],
            _ => Vec::new(),
        };
        assert_eq!(roots(&dirs, &refs), ["prod"]);
    }

    #[test]
    fn het_ngan_sach_buoc_thi_dung() {
        // 2 000 tài nguyên × 2 000 patch: vượt ngân sách bước chung (TM-44).
        let res: String = (0..2000).map(|i| format!("---\napiVersion: v1\nkind: ConfigMap\nmetadata: {{name: c{i}}}\ndata: {{a: b}}\n")).collect();
        let patches: String = (0..2000).map(|i| format!("- patch: |-\n    apiVersion: v1\n    kind: ConfigMap\n    metadata: {{name: c{i}}}\n    data: {{x: y}}\n")).collect();
        let k = format!("resources: [r.yaml]\npatches:\n{patches}");
        let files = [("k/r.yaml", res.as_str()), ("k/kustomization.yaml", k.as_str())];
        let t = crate::testclock::Clock::start();
        let err = run(&files, "k").unwrap_err();
        assert!(t.elapsed().as_secs_f64() < 3.0, "{:?}", t.elapsed());
        assert!(err.why.contains("bước dựng"), "{err:?}");
    }

    #[test]
    fn secret_generator_khong_doc() {
        let files = with(&[(
            "o/kustomization.yaml",
            "resources:\n- ../base\nsecretGenerator:\n- name: db\n  literals:\n  - password=hunter2\n",
        )]);
        let b = run(&files, "o").unwrap();
        assert!(b.docs.iter().all(|d| !d.text.contains("hunter2") && !d.text.contains("kind: Secret")), "{:?}", b.docs);
    }

    #[test]
    fn json6902_ap_sau_doi_ten_va_namespace() {
        // Đích viết theo tên đã thêm tiền tố, namespace mới: như Kustomize.
        let files = with(&[(
            "o/kustomization.yaml",
            "resources:\n- ../base\nnamespace: shop\nnamePrefix: prod-\npatchesJson6902:\n- target: {kind: Deployment, name: prod-cart, namespace: shop}\n  patch: |-\n    - op: replace\n      path: /spec/template/spec/containers/0/env/0/value\n      value: db:5432\n",
        )]);
        let b = run(&files, "o").unwrap();
        assert!(b.docs.iter().any(|d| d.text.contains("value: db:5432")), "{:?}", b.docs);
    }

    #[test]
    fn kim_cuong_base_ton_ngan_sach() {
        // l0 → l1 ×3 → … → l14: 3^14 lần dựng nếu không tính bước (TM-44).
        let mut files: Vec<(String, String)> = (0..14).map(|i| (format!("l{i}/kustomization.yaml"), format!("resources: [../l{0}, ../l{0}, ../l{0}]\n", i + 1))).collect();
        files.push(("l14/kustomization.yaml".into(), "resources: []\n".into()));
        let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let t = crate::testclock::Clock::start();
        // Base dựng một lần rồi dùng lại: không nhân theo cấp số. Base rỗng nên
        // không trùng tài nguyên, như Kustomize.
        assert!(run(&refs, "l0").is_ok());
        assert!(t.elapsed().as_secs_f64() < 1.0, "{:?}", t.elapsed());
        // Cùng base kéo vào hai lần: tài nguyên trùng, như Kustomize.
        let files = with(&[("o/kustomization.yaml", "resources:\n- ../base\n- ../base/app.yaml\n")]);
        assert!(run(&files, "o").unwrap_err().why.contains("trùng"));
    }

    #[test]
    fn khoa_la_va_patch_khong_khop_thi_khong_dung() {
        for (k, want) in [
            ("resources:\n- ../base\npatches:\n- path: p.yaml\n  options: {allowNameChange: true}\n", "options"),
            ("resources:\n- ../base\nlabels:\n- pairs: {a: b}\n  fields: [{path: x}]\n", "fields"),
            ("resources:\n- ../base\nvars:\n- name: X\n", "vars"),
            ("resources:\n- ../base\nhelmCharts:\n- name: x\n", "helmCharts"),
            ("resources:\n- ../base\nsomethingNew: true\n", "somethingNew"),
            ("resources:\n- ../base\npatches:\n- patch: |-\n    apiVersion: apps/v1\n    kind: Deployment\n    metadata: {name: carts}\n    spec: {replicas: 2}\n", "không khớp"),
            ("resources:\n- ../base\npatches:\n- target: {kind: Deployment, name: cart}\n  patch: |-\n    - op: copy\n      from: /a\n      path: /b\n", "copy"),
        ] {
            let files = with(&[("o/kustomization.yaml", k), ("o/p.yaml", "apiVersion: v1\nkind: Service\nmetadata: {name: cart}\n")]);
            let err = run(&files, "o").unwrap_err();
            assert!(err.why.contains(want), "{k}: {err:?}");
        }
    }

    #[test]
    fn dong_patch_viet_thang_nhan_va_image() {
        let files = with(&[(
            "o/kustomization.yaml",
            "resources:\n- ../base\ncommonLabels:\n  env: prod\nimages:\n- name: cart\n  newTag: \"2\"\n  digest: sha256:abc\npatches:\n- target:\n    kind: Deployment\n    name: cart\n  patch: |-\n    - op: replace\n      path: /spec/template/spec/containers/0/env/0/value\n      value: db:5432\n",
        ), (
            "o/np.yaml",
            "",
        )]);
        let b = run(&files, "o").unwrap();
        let dep = b.docs.iter().find(|d| d.text.contains("kind: Deployment")).unwrap();
        // `value:` của patch viết thẳng: dòng 16 của kustomization (dòng 3 của patch).
        let line = dep.text.lines().position(|l| l.contains("db:5432")).unwrap();
        assert_eq!((b.files[dep.lines[line].0 as usize].as_str(), dep.lines[line].1), ("o/kustomization.yaml", 16));
        assert!(dep.text.contains("cart:2@sha256:abc"), "{}", dep.text);
    }

    #[test]
    fn nhan_tren_network_policy_nhu_kustomize() {
        let files = with(&[
            ("base2/kustomization.yaml", "resources: [np.yaml]\n"),
            (
                "base2/np.yaml",
                "apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata: {name: a}\nspec:\n  podSelector: {}\n  ingress:\n  - from:\n    - podSelector:\n        matchLabels: {app: x}\n",
            ),
            ("o/kustomization.yaml", "resources: [../base2]\ncommonLabels:\n  env: prod\n"),
        ]);
        let b = run(&files, "o").unwrap();
        let np = &b.docs[0].text;
        // `podSelector: {}` giữ nguyên (mọi pod); selector đã có thì thêm nhãn.
        assert!(np.contains("podSelector: {}"), "{np}");
        assert!(np.contains("app: x") && np.matches("env: prod").count() == 2, "{np}");
    }
}
