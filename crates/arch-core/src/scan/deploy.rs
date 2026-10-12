//! Nạp mọi thứ về triển khai trong một repo: file compose, Dockerfile, và
//! những file chúng trỏ tới (`include`, `extends`, `env_file`, script khởi
//! chạy, `package.json`).
//!
//! Đường dẫn trong các file đó do người khác viết, nên mọi lần mở file đều qua
//! [`within`] và [`read_in`] (TM-9): không đường dẫn tuyệt đối, không đi ra
//! khỏi gốc dự án, không đi theo symlink, chỉ file thường, có trần dung lượng.
//! Thứ không đọc được thì vào [`Deploy::unread`], không im lặng bỏ qua (TM-12).

use super::command::{self, Launch};
use super::compose;
use super::dockerfile::{self, Command, Dockerfile, join_container};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

/// Trần cho file compose, Dockerfile, script và `env_file`. File cấu hình do
/// người viết không bao giờ tới mức này.
pub const MAX_DEPLOY_BYTES: u64 = 1 << 20;
/// `include`, `extends` và script gọi script: chuỗi người khác viết.
const MAX_HOPS: usize = 4;
/// Dockerfile thật có vài stage và vài chục `COPY`. Dockerfile đối kháng
/// (TM-8) thì bị bỏ ở đây, và báo là không đọc được.
const MAX_STAGES: usize = 64;
const MAX_MOUNTS: usize = 4096;
/// Số bước lần theo một lệnh khởi chạy (script gọi script, script npm): mỗi
/// bước có thể toả ra nhiều nhánh, nên trần là chung cho cả lệnh.
const MAX_RESOLVE: usize = 256;
/// Tổng số mục (env, depends_on, volumes…) của một project sau khi gộp
/// `extends`: mỗi service con nhận một bản sao các mục của service gốc.
const MAX_PROJECT_FACTS: usize = 200_000;
/// Byte mà các service giữ, cho cả lần nạp: mỗi file compose có trần riêng,
/// nhưng repo có nhiều file, và `extends` chép service gốc vào từng con.
const MAX_LOAD_BYTES: usize = 32 << 20;
/// Số bước lần theo lệnh khởi chạy cho cả lần nạp: mỗi lệnh có [`MAX_RESOLVE`],
/// nhưng một file có tới hàng nghìn service.
const MAX_LOAD_STEPS: usize = 20_000;
/// Byte đọc từ đĩa cho cả lần nạp: mỗi bước lần theo có thể là một script
/// 1 MB khác.
const MAX_LOAD_READ: usize = 16 << 20;

fn facts(s: &compose::Service) -> usize {
    s.env.len() + s.env_files.len() + s.depends_on.len() + s.links.len() + s.binds.len() + s.aliases.len()
}

/// Thư mục mà file compose trong đó không phải là triển khai của sản phẩm.
pub const ASIDE: &[&str] = &[
    "test", "tests", "testing", "selftest", "selftests", "spec", "specs", "__tests__", "e2e", "example", "examples",
    "bench", "benches", "doc", "docs", "deps", "vendor", "third_party", "thirdparty", "fixtures", "testdata",
    "node_modules",
];

/// File compose. Bản dành cho test (`docker-compose.test.yml`,
/// `compose.e2e.yaml`) không phải triển khai của sản phẩm và bị bỏ.
pub fn is_compose(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let stem = lower.strip_prefix("docker-").unwrap_or(&lower);
    let compose = (lower.ends_with(".yml") || lower.ends_with(".yaml"))
        && stem.strip_prefix("compose").is_some_and(|r| r.starts_with(['.', '-', '_']));
    compose && !stem.split(['.', '-', '_']).any(|part| matches!(part, "test" | "tests" | "e2e" | "ci"))
}

pub fn is_dockerfile(name: &str) -> bool {
    name == "Dockerfile"
        || name == "Containerfile"
        || name.starts_with("Dockerfile.")
        || name.starts_with("Containerfile.")
        || name.ends_with(".Dockerfile")
        || name.ends_with(".dockerfile")
}

/// Thư mục test, ví dụ, tài liệu, và thư mục ẩn (`.devcontainer`, `.github`):
/// compose trong đó là môi trường dev hay CI, không phải triển khai.
fn aside(path: &str) -> bool {
    let dirs = path.rsplit_once('/').map_or("", |(d, _)| d);
    dirs.split('/').any(|d| ASIDE.contains(&d) || d.starts_with('.'))
}

/// File cấu hình triển khai của sản phẩm, không nằm trong thư mục test, ví dụ.
pub fn is_deploy_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    (is_compose(name) || is_dockerfile(name)) && !aside(path)
}

/// Một service lấy từ cấu hình.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    /// Service tên `name` trong các file compose ở thư mục `dir` (`""` là gốc).
    Compose { dir: String, name: String },
    /// Service dựng từ một Dockerfile, không có compose.
    Dockerfile(String),
}

#[derive(Debug, Clone, Default)]
pub struct Deploy {
    pub services: Vec<Svc>,
    pub unread: Vec<Unread>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unread {
    pub file: String,
    pub why: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Svc {
    pub key: Key,
    pub file: String,
    pub line: usize,
    pub image: Option<String>,
    /// Chạy mã của repo: dựng từ build context, hay mount mã vào.
    pub code: bool,
    pub edges: Vec<Edge>,
    pub starts: Vec<Start>,
    /// Vì sao không biết service chạy mã nào, khi nó chạy mã của repo.
    pub unknown: Option<String>,
}

/// Cạnh mạng thấy trong cấu hình.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// Tên service đích trong compose.
    pub to: String,
    pub file: String,
    pub line: usize,
    /// Cách hiển thị, không bao giờ chứa giá trị gốc của biến môi trường.
    pub shown: String,
}

/// Một lệnh khởi chạy và mã mà nó nạp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Start {
    pub file: String,
    pub line: usize,
    /// Thứ được chạy (module, file), không có đối số: đối số có thể mang
    /// secret.
    pub shown: String,
    pub entries: Vec<Entry>,
    /// Thứ lệnh chạy mà không lần ra được trong repo (script không có, binary
    /// không biết dựng từ đâu). Không im lặng bỏ qua: có thể nó đang giấu mã.
    pub unresolved: Vec<String>,
    /// Chỗ thôi đọc giữa chừng: lồng quá sâu, script gọi script quá nhiều
    /// tầng, hết ngân sách, script không đọc được, hay lệnh không chạy chương
    /// trình nào nhận ra. Phần sau chỗ đó có thể chạy mã ngoài `runs` (TM-12).
    pub cut: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// File trong repo.
    File(String),
    /// Module Python: tìm trong `roots` (thư mục trong repo, theo thứ tự),
    /// rồi theo các gói của dự án. `main`: chạy bằng `-m`, gói thì qua
    /// `__main__.py`.
    Module { name: String, roots: Vec<String>, main: bool },
    /// Package Go (thư mục) trong repo.
    Dir(String),
}

/// Nối `path` (do file người khác viết) vào thư mục `base` của repo. `None`
/// nếu tuyệt đối, là URL, hay đi ra ngoài gốc dự án.
pub fn within(base: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.starts_with('~') || path.contains('\\') || path.contains("://") {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    File,
    Dir,
}

/// Loại của đường dẫn trong repo, không đi theo symlink ở bất kỳ đoạn nào.
fn kind_in(root: &Path, rel: &str) -> Option<Kind> {
    let mut p = root.to_path_buf();
    let mut meta = p.symlink_metadata().ok()?;
    for seg in rel.split('/').filter(|s| !s.is_empty()) {
        p.push(seg);
        meta = p.symlink_metadata().ok()?;
        if meta.file_type().is_symlink() {
            return None;
        }
    }
    if meta.is_file() {
        Some(Kind::File)
    } else if meta.is_dir() {
        Some(Kind::Dir)
    } else {
        None
    }
}

/// Mở để đọc mà không đi theo symlink ở đoạn cuối và không chờ trên FIFO:
/// `kind_in` đã kiểm, nhưng một tiến trình chạy song song có thể tráo file
/// giữa lúc kiểm và lúc mở (TM-9).
fn open_plain(path: &Path) -> std::io::Result<std::fs::File> {
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // Hằng số khác nhau theo hệ điều hành và cả kiến trúc (`O_NOFOLLOW` của
        // Linux ARM là `O_LARGEFILE` của x86). Chỉ đặt ở chỗ biết chắc; chỗ
        // khác vẫn có phép so `(dev, ino)` trong [`read_in`].
        #[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
        const FLAGS: i32 = 0x0100 /* O_NOFOLLOW */ | 0x0004 /* O_NONBLOCK */;
        #[cfg(all(any(target_os = "linux", target_os = "android"), any(target_arch = "x86", target_arch = "x86_64")))]
        const FLAGS: i32 = 0o400000 /* O_NOFOLLOW */ | 0o4000 /* O_NONBLOCK */;
        #[cfg(all(any(target_os = "linux", target_os = "android"), any(target_arch = "arm", target_arch = "aarch64")))]
        const FLAGS: i32 = 0o100000 /* O_NOFOLLOW */ | 0o4000 /* O_NONBLOCK */;
        #[cfg(not(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            all(
                any(target_os = "linux", target_os = "android"),
                any(target_arch = "x86", target_arch = "x86_64", target_arch = "arm", target_arch = "aarch64")
            )
        )))]
        const FLAGS: i32 = 0;
        opts.custom_flags(FLAGS);
    }
    opts.open(path)
}

/// Đọc file trong repo: chỉ file thường, không qua symlink, tối đa `max` byte.
pub fn read_in(root: &Path, rel: &str, max: u64) -> Result<String, String> {
    use std::io::Read as _;
    match kind_in(root, rel) {
        Some(Kind::File) => {}
        Some(Kind::Dir) => return Err("là thư mục".into()),
        None if root.join(rel).symlink_metadata().is_ok() => {
            return Err("không phải file thường, hay có symlink trên đường dẫn".into());
        }
        None => return Err("không có".into()),
    }
    let checked = root.join(rel).symlink_metadata().map_err(|e| e.to_string())?;
    let file = open_plain(&root.join(rel)).map_err(|e| e.to_string())?;
    // Kiểm lại trên chính file đã mở: giữa lúc kiểm đường dẫn và lúc mở, một
    // tiến trình khác có thể tráo vào FIFO, thiết bị, hay symlink ra ngoài
    // repo. Cùng `(dev, ino)` với thứ đã kiểm thì là cùng một file.
    let opened = file.metadata().map_err(|e| e.to_string())?;
    if !opened.is_file() || !same_file(&checked, &opened) {
        return Err("không phải file thường".into());
    }
    let mut text = String::new();
    file.take(max + 1).read_to_string(&mut text).map_err(|_| "không phải văn bản UTF-8".to_string())?;
    if text.len() as u64 > max {
        return Err(format!("quá {} KB", max >> 10));
    }
    Ok(text)
}

#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    a.len() == b.len() && a.modified().ok() == b.modified().ok()
}

/// Đường dẫn lấy từ cấu hình, để in trong thông báo: URL thì bỏ phần
/// `user:pass@` (`https://token@github.com/x.git` là build context hợp lệ của
/// Docker), và cắt bớt khi quá dài (TM-10).
pub fn shown_path(path: &str) -> String {
    let path = match path.split_once("://") {
        Some((scheme, rest)) => {
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            match rest[..end].rfind('@').or_else(|| rest.rfind('@')) {
                Some(at) => format!("{scheme}://…@{}", &rest[at + 1..]),
                None => path.to_string(),
            }
        }
        None => path.to_string(),
    };
    match path.char_indices().nth(120) {
        Some((at, _)) => format!("{}…", &path[..at]),
        None => path,
    }
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Biến môi trường có tên máy trong một `env_file`: dòng, khoá, các máy, và
/// giá trị có bị cắt không ([`compose::hosts_scan`]).
type EnvHosts = Vec<(usize, String, Vec<compose::Host>, bool)>;

/// Bộ đệm và ngân sách chung cho một lần nạp (TM-8). Hàng nghìn service có
/// thể cùng dựng từ một Dockerfile, cùng chạy một script, cùng nhắc một
/// `env_file`: đọc và phân tích lại cho từng service là nhân mỗi file 1 MB lên
/// hàng nghìn lần trên đường hook. Mỗi file ở đây được đọc, phân tích một lần.
struct Shared<'a> {
    root: &'a Path,
    texts: RefCell<HashMap<String, Result<Rc<str>, String>>>,
    dockerfiles: RefCell<HashMap<String, Result<Rc<Built>, String>>>,
    scripts: RefCell<HashMap<String, Rc<Vec<Launch>>>>,
    env_files: RefCell<HashMap<String, Result<Rc<EnvHosts>, String>>>,
    packages: RefCell<HashMap<String, Option<Rc<crate::jsonc::Value>>>>,
    /// Số mục (env, depends_on, dòng `env_file`…) còn được xét.
    facts: Cell<usize>,
    /// Byte còn được giữ trong các service.
    bytes: Cell<usize>,
    /// Bước lần theo lệnh khởi chạy còn lại.
    steps: Cell<usize>,
    /// Byte còn được đọc từ đĩa.
    read: Cell<usize>,
}

impl<'a> Shared<'a> {
    fn new(root: &'a Path) -> Self {
        Shared {
            root,
            texts: RefCell::default(),
            dockerfiles: RefCell::default(),
            scripts: RefCell::default(),
            env_files: RefCell::default(),
            packages: RefCell::default(),
            facts: Cell::new(MAX_PROJECT_FACTS),
            bytes: Cell::new(MAX_LOAD_BYTES),
            steps: Cell::new(MAX_LOAD_STEPS),
            read: Cell::new(MAX_LOAD_READ),
        }
    }

    /// Trừ `n` khỏi một ngân sách; không đủ thì về 0 và trả `false`.
    fn spend(budget: &Cell<usize>, n: usize) -> bool {
        match budget.get().checked_sub(n) {
            Some(rest) => {
                budget.set(rest);
                true
            }
            None => {
                budget.set(0);
                false
            }
        }
    }

    /// [`read_in`] với trần [`MAX_DEPLOY_BYTES`], mỗi đường dẫn một lần.
    fn read(&self, rel: &str) -> Result<Rc<str>, String> {
        if let Some(r) = self.texts.borrow().get(rel) {
            return r.clone();
        }
        let r = if self.read.get() == 0 {
            Err(format!("quá {} MB file cấu hình cho cả repo", MAX_LOAD_READ >> 20))
        } else {
            read_in(self.root, rel, MAX_DEPLOY_BYTES).map(|t| {
                Shared::spend(&self.read, t.len());
                Rc::from(t)
            })
        };
        self.texts.borrow_mut().insert(rel.to_string(), r.clone());
        r
    }

    fn dockerfile(&self, rel: &str) -> Result<Rc<Built>, String> {
        if let Some(r) = self.dockerfiles.borrow().get(rel) {
            return r.clone();
        }
        let r = self.read(rel).and_then(|text| Built::new(dockerfile::parse(&text)).map(Rc::new));
        self.dockerfiles.borrow_mut().insert(rel.to_string(), r.clone());
        r
    }

    /// Lệnh trong một script shell, tách một lần.
    fn launches(&self, rel: &str) -> Result<Rc<Vec<Launch>>, String> {
        if let Some(l) = self.scripts.borrow().get(rel) {
            return Ok(l.clone());
        }
        let l = Rc::new(command::launches_in_shell(&self.read(rel)?));
        self.scripts.borrow_mut().insert(rel.to_string(), l.clone());
        Ok(l)
    }

    /// Dòng có tên máy trong một `env_file`.
    fn env_hosts(&self, rel: &str) -> Result<Rc<EnvHosts>, String> {
        if let Some(r) = self.env_files.borrow().get(rel) {
            return r.clone();
        }
        let r = self.read(rel).map(|text| {
            let mut out = Vec::new();
            for (no, raw) in text.lines().enumerate() {
                let l = raw.trim();
                let l = l.strip_prefix("export ").unwrap_or(l);
                if l.starts_with('#') {
                    continue;
                }
                let Some((k, v)) = l.split_once('=') else { continue };
                let v = v.trim().trim_matches(['"', '\'']);
                let (hosts, cut) = compose::hosts_scan(k.trim(), v);
                if !hosts.is_empty() || cut {
                    out.push((no + 1, k.trim().to_string(), hosts, cut));
                }
            }
            Rc::new(out)
        });
        self.env_files.borrow_mut().insert(rel.to_string(), r.clone());
        r
    }

    /// `package.json` đã phân tích.
    fn package(&self, rel: &str) -> Option<Rc<crate::jsonc::Value>> {
        if let Some(j) = self.packages.borrow().get(rel) {
            return j.clone();
        }
        let j = self.read(rel).ok().and_then(|t| crate::jsonc::parse(&t)).map(Rc::new);
        self.packages.borrow_mut().insert(rel.to_string(), j.clone());
        j
    }

    /// Có phải script shell không: đuôi `.sh`, hay không có đuôi mà dòng đầu
    /// là `#!…sh`. Không đọc được thì nói lý do: script giấu được mã ở đó.
    fn is_shell_script(&self, rel: &str) -> Result<bool, String> {
        if rel.ends_with(".sh") || rel.ends_with(".bash") {
            return Ok(true);
        }
        if rel.rsplit('/').next().is_some_and(|n| n.contains('.')) {
            return Ok(false);
        }
        let text = self.read(rel)?;
        let first = text.lines().next().unwrap_or("");
        Ok(first.starts_with("#!") && first.contains("sh"))
    }
}

/// Dockerfile đã phân tích, kèm những thứ tính từ nó, mỗi thứ một lần.
struct Built {
    df: Dockerfile,
    effective: RefCell<HashMap<usize, Rc<dockerfile::Effective>>>,
    /// Mount của từng stage, theo build context.
    mounts: RefCell<HashMap<String, Rc<Vec<Vec<Mount>>>>>,
    builds: OnceCell<Vec<GoBuild>>,
}

impl Built {
    fn new(df: Dockerfile) -> Result<Built, String> {
        if df.stages.len() > MAX_STAGES {
            return Err(format!("quá {MAX_STAGES} stage"));
        }
        Ok(Built { df, effective: RefCell::default(), mounts: RefCell::default(), builds: OnceCell::new() })
    }

    fn effective(&self, stage: usize) -> Rc<dockerfile::Effective> {
        if let Some(e) = self.effective.borrow().get(&stage) {
            return e.clone();
        }
        let e = Rc::new(self.df.effective(stage));
        self.effective.borrow_mut().insert(stage, e.clone());
        e
    }

    fn mounts(&self, root: &Path, context: &str) -> Rc<Vec<Vec<Mount>>> {
        if let Some(m) = self.mounts.borrow().get(context) {
            return m.clone();
        }
        let m = Rc::new(stage_mounts(root, context, self));
        self.mounts.borrow_mut().insert(context.to_string(), m.clone());
        m
    }

    /// `go build` trong mọi `RUN`.
    fn builds(&self) -> &[GoBuild] {
        self.builds.get_or_init(|| {
            let mut builds = Vec::new();
            for (i, stage) in self.df.stages.iter().enumerate() {
                for run in &stage.runs {
                    for b in go_builds(&run.text) {
                        let pkgs = if b.1.is_empty() { vec![".".to_string()] } else { b.1 };
                        let pkgs = pkgs.into_iter().map(|p| join_container(&run.workdir, &p)).collect();
                        builds.push((b.0, pkgs, i));
                    }
                }
            }
            builds
        })
    }
}

/// Một service như một file compose viết, kèm thư mục để tính đường dẫn
/// tương đối của nó.
struct Record {
    file: String,
    base: String,
    s: compose::Service,
}

/// Nạp mọi project compose trong `files` (đường dẫn tương đối của repo).
pub fn load(root: &Path, files: &[String]) -> Deploy {
    let shared = Shared::new(root);
    let mut out = Deploy::default();
    let mut projects: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for f in files {
        let name = f.rsplit('/').next().unwrap_or(f);
        if is_compose(name) && !aside(f) {
            projects.entry(dir_of(f).to_string()).or_default().push(f.clone());
        }
    }
    for (dir, mut paths) in projects {
        // File gốc trước, file override sau.
        paths.sort_by_key(|p| {
            let n = p.rsplit('/').next().unwrap_or(p).to_ascii_lowercase();
            (!matches!(n.as_str(), "compose.yaml" | "compose.yml" | "docker-compose.yaml" | "docker-compose.yml"), p.clone())
        });
        let mut records: Vec<Record> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for p in &paths {
            read_compose(&shared, p, 0, &mut seen, &mut records, &mut out.unread);
        }
        resolve_extends(&shared, &mut records, &mut out.unread);
        project(&shared, &dir, &records, &mut out);
    }
    // Cùng một file hỏng được nhắc từ nhiều service: báo một lần.
    let mut seen: HashSet<(String, String)> = HashSet::new();
    out.unread.retain(|u| seen.insert((u.file.clone(), u.why.clone())));
    out
}

fn read_compose(
    shared: &Shared<'_>,
    path: &str,
    hops: usize,
    seen: &mut HashSet<String>,
    records: &mut Vec<Record>,
    unread: &mut Vec<Unread>,
) {
    if hops > MAX_HOPS || !seen.insert(path.to_string()) {
        return;
    }
    let text = match shared.read(path) {
        Ok(t) => t,
        Err(why) => {
            unread.push(Unread { file: path.to_string(), why });
            return;
        }
    };
    let file = match compose::parse(&text) {
        Ok(f) => f,
        Err(e) => {
            unread.push(Unread { file: path.to_string(), why: format!("YAML {e}") });
            return;
        }
    };
    let base = dir_of(path).to_string();
    for (inc, line) in &file.includes {
        match within(&base, inc) {
            Some(p) => read_compose(shared, &p, hops + 1, seen, records, unread),
            None => unread.push(Unread {
                file: format!("{path}:{line}"),
                why: format!("`include: {}` trỏ ra ngoài dự án", shown_path(inc)),
            }),
        }
    }
    let size: usize = file.services.iter().map(compose::Service::size).sum();
    let count: usize = file.services.iter().map(facts).sum();
    if !Shared::spend(&shared.bytes, size) || !Shared::spend(&shared.facts, count) {
        unread.push(Unread {
            file: path.to_string(),
            why: format!("quá {} MB hay {MAX_PROJECT_FACTS} mục cấu hình cho cả repo", MAX_LOAD_BYTES >> 20),
        });
        return;
    }
    for s in file.services {
        records.push(Record { file: path.to_string(), base: base.clone(), s });
    }
}

/// `extends`: lấy service gốc trong cùng project, hay trong file khác.
///
/// Tra tên bằng bảng băm: file 1 MB có thể khai hàng chục nghìn service cùng
/// `extends` một gốc, và tìm tuần tự thì thành bậc hai trên đường hook.
fn resolve_extends(shared: &Shared<'_>, records: &mut [Record], unread: &mut Vec<Unread>) {
    // File của `extends.file`, đọc một lần: hàng nghìn service cùng trỏ tới
    // một file thì đọc lại mỗi lần là treo hook (TM-8).
    let mut files: HashMap<String, Result<Vec<compose::Service>, String>> = HashMap::new();
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, r) in records.iter().enumerate() {
        let list = by_name.entry(r.s.name.clone()).or_default();
        if list.len() < 2 {
            list.push(i);
        }
    }
    for i in 0..records.len() {
        let mut hops = 0;
        let mut guard: HashSet<(String, String)> = HashSet::new();
        while let Some(ext) = records[i].s.extends.take() {
            hops += 1;
            if hops > MAX_HOPS || !guard.insert((ext.file.clone().unwrap_or_default(), ext.service.clone())) {
                break;
            }
            let base = match &ext.file {
                None => by_name
                    .get(&ext.service)
                    .and_then(|list| list.iter().find(|&&j| j != i))
                    .map(|&j| records[j].s.clone()),
                Some(f) => {
                    let Some(p) = within(&records[i].base, f) else {
                        unread.push(Unread {
                            file: format!("{}:{}", records[i].file, ext.line),
                            why: format!("`extends.file: {}` trỏ ra ngoài dự án", shown_path(f)),
                        });
                        break;
                    };
                    let parsed = files.entry(p.clone()).or_insert_with(|| {
                        shared.read(&p).and_then(|t| compose::parse(&t).map_err(|e| format!("YAML {e}"))).map(|f| f.services)
                    });
                    match parsed {
                        Ok(services) => services.iter().find(|s| s.name == ext.service).cloned(),
                        Err(why) => {
                            let why = why.clone();
                            if !unread.iter().any(|u| u.file == p && u.why == why) {
                                unread.push(Unread { file: p, why });
                            }
                            break;
                        }
                    }
                }
            };
            let Some(base) = base else { break };
            // Con nhận một bản sao của gốc: tính vào trần chung (TM-8).
            if !Shared::spend(&shared.facts, facts(&base)) || !Shared::spend(&shared.bytes, base.size()) {
                unread.push(Unread {
                    file: records[i].file.clone(),
                    why: format!(
                        "quá {MAX_PROJECT_FACTS} mục hay {} MB sau khi gộp `extends`; phần còn lại không đọc",
                        MAX_LOAD_BYTES >> 20
                    ),
                });
                return;
            }
            let mut child = records[i].s.clone();
            compose::inherit(&mut child, &base);
            child.extends = base.extends.clone();
            records[i].s = child;
        }
    }
}

fn project(shared: &Shared<'_>, dir: &str, records: &[Record], out: &mut Deploy) {
    // Tên mà service khác dùng để gọi: tên service, `hostname`, alias.
    // DNS không phân biệt hoa thường: `DB_HOST=DB` vẫn là service `db`.
    let mut names: HashMap<String, &str> = HashMap::new();
    for r in records {
        names.insert(r.s.name.to_ascii_lowercase(), &r.s.name);
    }
    for r in records {
        for a in &r.s.aliases {
            names.entry(a.to_ascii_lowercase()).or_insert(&r.s.name);
        }
    }
    // Gom bản ghi theo tên, giữ thứ tự xuất hiện. Bảng băm chứ không tìm tuần
    // tự: số service trong một file 1 MB có thể lên hàng chục nghìn.
    let mut order: Vec<&str> = Vec::new();
    let mut groups: HashMap<&str, Vec<&Record>> = HashMap::new();
    for r in records {
        let group = groups.entry(&r.s.name).or_default();
        if group.is_empty() {
            order.push(&r.s.name);
        }
        group.push(r);
    }
    for name in order {
        let mine = &groups[name];
        let first = mine[0];
        let mut svc = Svc {
            key: Key::Compose { dir: dir.to_string(), name: name.to_string() },
            file: first.file.clone(),
            line: first.s.line,
            image: mine.iter().find_map(|r| r.s.image.clone()),
            // Image dựng sẵn có mount vài file cấu hình (`healthchecks/`) thì
            // vẫn là mã của người khác; mount kèm lệnh khởi chạy riêng mới là
            // chạy mã của repo.
            code: mine.iter().any(|r| {
                r.s.build.is_some() || (!r.s.binds.is_empty() && (r.s.command.is_some() || r.s.entrypoint.is_some()))
            }),
            edges: Vec::new(),
            starts: Vec::new(),
            unknown: None,
        };
        let mut seen: HashSet<(String, String, usize)> = HashSet::new();
        let mut push_edge = |to: &str, file: &str, line: usize, shown: String| {
            if let Some(&target) = names.get(&to.to_ascii_lowercase())
                && target != name
                && seen.insert((target.to_string(), file.to_string(), line))
            {
                svc.edges.push(Edge { to: target.to_string(), file: file.to_string(), line, shown });
            }
        };
        for r in mine {
            for (to, line) in &r.s.depends_on {
                push_edge(to, &r.file, *line, format!("depends_on: {to}"));
            }
            for (to, line) in &r.s.links {
                push_edge(to, &r.file, *line, format!("links: {to}"));
            }
            for e in &r.s.env {
                for h in &e.hosts {
                    push_edge(&h.name, &r.file, e.line, format!("{} → {}", e.key, h.shown()));
                }
                if e.cut {
                    out.unread.push(cut_value(&r.file, e.line, &e.key));
                }
            }
            let mut env_seen: HashSet<String> = HashSet::new();
            for (path, line) in &r.s.env_files {
                let Some(p) = within(&r.base, path) else {
                    out.unread.push(Unread {
                        file: format!("{}:{line}", r.file),
                        why: format!("`env_file: {}` trỏ ra ngoài dự án", shown_path(path)),
                    });
                    continue;
                };
                if !env_seen.insert(p.clone()) {
                    continue;
                }
                match shared.env_hosts(&p) {
                    Ok(lines) => {
                        // Đọc một lần cho cả repo, nhưng mỗi service xét lại các
                        // dòng: tính vào trần chung (TM-8).
                        if !Shared::spend(&shared.facts, lines.len()) {
                            out.unread.push(Unread {
                                file: p.clone(),
                                why: format!("quá {MAX_PROJECT_FACTS} mục cấu hình cho cả repo"),
                            });
                            continue;
                        }
                        for (no, key, hosts, cut) in lines.iter() {
                            for h in hosts {
                                push_edge(&h.name, &p, *no, format!("{key} → {}", h.shown()));
                            }
                            if *cut {
                                out.unread.push(cut_value(&p, *no, key));
                            }
                        }
                    }
                    // `.env` thường không được commit: không có thì không có
                    // gì để đọc. Có mà không đọc được mới là chuyện cần báo.
                    Err(why) if why == "không có" => {}
                    Err(why) => out.unread.push(Unread { file: p, why }),
                }
            }
        }
        for r in mine {
            starts(shared, r, &mut svc, &mut out.unread);
        }
        if svc.code && svc.starts.iter().all(|s| s.entries.is_empty()) && svc.unknown.is_none() {
            svc.unknown = Some(match svc.starts.first() {
                None => "không có lệnh khởi chạy (command, CMD, ENTRYPOINT)".into(),
                Some(s) if s.shown.is_empty() => "không nhận ra lệnh khởi chạy".into(),
                Some(s) => format!("không tìm thấy trong repo mã mà `{}` nạp", s.shown),
            });
        }
        out.services.push(svc);
    }
}

/// Giá trị env dài quá nên chỉ đọc một phần: cạnh tới service có thể nằm ở
/// phần không đọc (TM-12).
fn cut_value(file: &str, line: usize, key: &str) -> Unread {
    Unread {
        file: format!("{file}:{line}"),
        why: format!("`{key}` có quá nhiều phần tử hay tên máy; phần sau không đọc"),
    }
}

/// Service dựng từ một Dockerfile, không qua compose: build context là thư
/// mục chứa nó.
pub fn load_dockerfile(root: &Path, path: &str) -> Result<Svc, Unread> {
    let shared = Shared::new(root);
    let built = shared.dockerfile(path).map_err(|why| Unread { file: path.to_string(), why })?;
    let mut svc = Svc {
        key: Key::Dockerfile(path.to_string()),
        file: path.to_string(),
        line: 1,
        image: None,
        code: true,
        edges: Vec::new(),
        starts: Vec::new(),
        unknown: None,
    };
    let Some(target) = built.df.target(None) else {
        svc.unknown = Some("Dockerfile không có `FROM`".into());
        return Ok(svc);
    };
    let eff = built.effective(target);
    let ctx = Ctx::new(&shared, dir_of(path), Some(&built), Some(target), &eff, None, &[], &[]);
    let argv: Vec<String> = eff.entrypoint.iter().chain(&eff.cmd).flat_map(|(c, _)| c.argv()).collect();
    let line = eff.cmd.as_ref().or(eff.entrypoint.as_ref()).map_or(1, |(_, l)| *l);
    svc.starts.push(ctx.start(&argv, path, line));
    if svc.starts.iter().all(|s| s.entries.is_empty()) {
        svc.unknown = Some(if argv.is_empty() {
            "Dockerfile không có CMD hay ENTRYPOINT".into()
        } else {
            format!("không tìm thấy trong repo mã mà `{}` nạp", svc.starts[0].shown)
        });
    }
    Ok(svc)
}

/// Lệnh khởi chạy của một bản ghi compose, theo luật của Docker: `entrypoint`
/// của compose thay `ENTRYPOINT` và xoá `CMD`; `command` thay `CMD`.
fn starts(shared: &Shared<'_>, r: &Record, svc: &mut Svc, unread: &mut Vec<Unread>) {
    let mut df: Option<Rc<Built>> = None;
    let mut df_path = String::new();
    let mut context: Option<String> = None;
    if let Some(build) = &r.s.build {
        match within(&r.base, &build.context) {
            None => {
                let why = format!("build context `{}` nằm ngoài dự án", shown_path(&build.context));
                svc.unknown = Some(why.clone());
                unread.push(Unread { file: format!("{}:{}", r.file, build.line), why });
                return;
            }
            Some(ctx) => context = Some(ctx),
        }
        let ctx = context.clone().unwrap_or_default();
        if let Some(inline) = &build.inline {
            // Viết thẳng trong compose: nằm trong trần byte của file compose.
            match Built::new(dockerfile::parse(inline)) {
                Ok(b) => df = Some(Rc::new(b)),
                Err(why) => unread.push(Unread { file: format!("{}:{}", r.file, build.line), why }),
            }
            df_path = r.file.clone();
        } else {
            let name = build.dockerfile.as_deref().unwrap_or("Dockerfile");
            match within(&ctx, name) {
                None => unread.push(Unread {
                    file: format!("{}:{}", r.file, build.line),
                    why: format!("`dockerfile: {}` nằm ngoài dự án", shown_path(name)),
                }),
                Some(p) => match shared.dockerfile(&p) {
                    Ok(b) => {
                        df = Some(b);
                        df_path = p;
                    }
                    Err(why) => unread.push(Unread { file: p, why }),
                },
            }
        }
    }
    let target = df.as_ref().and_then(|d| d.df.target(r.s.build.as_ref().and_then(|b| b.target.as_deref())));
    let eff = match (&df, target) {
        (Some(d), Some(t)) => d.effective(t),
        _ => Rc::new(dockerfile::Effective { workdir: "/".into(), ..Default::default() }),
    };
    let (entry, entry_at): (Option<&Command>, Option<(String, usize)>) = match &r.s.entrypoint {
        Some((c, l)) => (Some(c), Some((r.file.clone(), *l))),
        None => (eff.entrypoint.as_ref().map(|(c, _)| c), eff.entrypoint.as_ref().map(|(_, l)| (df_path.clone(), *l))),
    };
    let (cmd, cmd_at): (Option<&Command>, Option<(String, usize)>) = match (&r.s.command, &r.s.entrypoint) {
        (Some((c, l)), _) => (Some(c), Some((r.file.clone(), *l))),
        (None, Some(_)) => (None, None),
        (None, None) => (eff.cmd.as_ref().map(|(c, _)| c), eff.cmd.as_ref().map(|(_, l)| (df_path.clone(), *l))),
    };
    let argv: Vec<String> = entry.iter().chain(cmd.iter()).flat_map(|c| c.argv()).collect();
    if argv.is_empty() {
        return;
    }
    let (file, line) = cmd_at.or(entry_at).unwrap_or((r.file.clone(), r.s.line));
    let binds: Vec<(String, String)> =
        r.s.binds.iter().filter_map(|b| within(&r.base, &b.host).map(|h| (h, b.container.clone()))).collect();
    let ctx = Ctx::new(
        shared,
        context.as_deref().unwrap_or(&r.base),
        df.as_ref(),
        target,
        &eff,
        r.s.working_dir.as_deref(),
        &binds,
        &r.s.env.iter().filter_map(|e| e.value.clone().map(|v| (e.key.clone(), v))).collect::<Vec<_>>(),
    );
    svc.starts.push(ctx.start(&argv, &file, line));
}

/// Đường dẫn trong container ứng với file hay thư mục trong repo.
#[derive(Debug, Clone)]
struct Mount {
    container: String,
    repo: String,
}

/// Mount của từng stage, tính một lần cho cả Dockerfile.
///
/// `COPY` từ build context: nguồn là thư mục thì nội dung của nó nằm ở đích;
/// nguồn là file thì nằm ở đích, hay trong đích nếu đích là thư mục. Nguồn có
/// ký tự đại diện bị bỏ qua.
///
/// `COPY --from=<stage>` đi theo mount của stage nguồn: `COPY --from=build
/// /app/backend .` là mã mà stage `build` đã chép vào `/app/backend`. Đường dẫn
/// nguồn khi đó tính từ gốc của stage. Chỉ stage đứng trước được trỏ tới, nên
/// mỗi stage dùng lại kết quả đã tính, không đệ quy — tính lại cho từng dòng
/// `COPY --from` từng làm một Dockerfile 4 KB mất 23 giây. `--from` trỏ tới
/// image bên ngoài thì không phải mã của repo.
fn stage_mounts(root: &Path, context: &str, built: &Built) -> Vec<Vec<Mount>> {
    let df = &built.df;
    let mut out: Vec<Vec<Mount>> = Vec::with_capacity(df.stages.len());
    for i in 0..df.stages.len() {
        let mut here: Vec<Mount> = Vec::new();
        for c in &built.effective(i).copies {
            let source: Option<&Vec<Mount>> = match &c.from {
                None => None,
                Some(name) => {
                    let index = df
                        .stages
                        .iter()
                        .position(|s| s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
                        .or_else(|| name.parse::<usize>().ok());
                    match index {
                        Some(j) if j < i => Some(&out[j]),
                        _ => continue,
                    }
                }
            };
            for src in &c.sources {
                if here.len() >= MAX_MOUNTS {
                    break;
                }
                if src.contains(['*', '?', '[']) {
                    continue;
                }
                let repo = match source {
                    None => within(context, src),
                    Some(stage) => map_to_repo(stage, &join_container("/", src)),
                };
                let Some(repo) = repo else { continue };
                match kind_in(root, &repo) {
                    Some(Kind::Dir) => here.push(Mount { container: c.dest.clone(), repo }),
                    Some(Kind::File) => {
                        let name = repo.rsplit('/').next().unwrap_or(&repo).to_string();
                        let container = if c.dest_dir { join_container(&c.dest, &name) } else { c.dest.clone() };
                        here.push(Mount { container, repo });
                    }
                    None => {}
                }
            }
        }
        out.push(here);
    }
    out
}

/// Đường dẫn trong container → đường dẫn trong repo. Mount sau và cụ thể hơn
/// thắng.
fn map_to_repo(mounts: &[Mount], container: &str) -> Option<String> {
    let mut best: Option<&Mount> = None;
    for m in mounts.iter().rev() {
        let hit = container == m.container
            || container.starts_with(&format!("{}/", m.container.trim_end_matches('/')))
            || m.container == "/";
        if hit && best.is_none_or(|b| m.container.len() > b.container.len()) {
            best = Some(m);
        }
    }
    let m = best?;
    let rest = container[m.container.len().min(container.len())..].trim_start_matches('/');
    within(&m.repo, rest)
}

/// Một lần `go build` trong Dockerfile: binary ra đâu, package nào (đường dẫn
/// trong container), ở stage nào.
type GoBuild = (Option<String>, Vec<String>, usize);

/// Bối cảnh để đổi gợi ý của [`command::launches`] thành mã trong repo.
struct Ctx<'a> {
    shared: &'a Shared<'a>,
    root: &'a Path,
    /// Build context (hay thư mục của file compose khi không có build).
    context: String,
    built: Option<Rc<Built>>,
    workdir: String,
    /// Mount của stage được dựng, cộng bind mount.
    mounts: Vec<Mount>,
    /// Mount của từng stage, cho binary dựng ở stage khác.
    stages: Rc<Vec<Vec<Mount>>>,
    pythonpath: Option<String>,
    flask_app: Option<String>,
    /// Số bước lần theo còn lại cho lệnh đang xét.
    budget: std::cell::Cell<usize>,
    /// Chỗ thôi đọc, xem [`Start::cut`].
    cut: std::cell::RefCell<Vec<String>>,
    /// Số chỗ thôi đọc không ghi chi tiết vì đã đủ [`MAX_CUT`].
    cut_more: Cell<usize>,
}

/// Số chỗ thôi đọc ghi chi tiết cho một lệnh; phần còn lại chỉ đếm, để
/// output (đi vào context của agent) không dài theo cấu hình đối kháng.
const MAX_CUT: usize = 5;

impl<'a> Ctx<'a> {
    /// `eff`: stage được dựng, đã thừa hưởng. `working_dir` của compose thay
    /// `WORKDIR`; bind mount phủ lên nội dung image nên đứng sau và thắng.
    #[allow(clippy::too_many_arguments)]
    fn new(
        shared: &'a Shared<'a>,
        context: &str,
        built: Option<&Rc<Built>>,
        target: Option<usize>,
        eff: &dockerfile::Effective,
        working_dir: Option<&str>,
        binds: &[(String, String)],
        kept: &[(String, String)],
    ) -> Self {
        let root = shared.root;
        let stages = built.map(|b| b.mounts(root, context)).unwrap_or_default();
        let mut mounts = target.and_then(|t| stages.get(t)).cloned().unwrap_or_default();
        // Biến của compose thắng `ENV` của Dockerfile.
        let var = |key: &str| {
            kept.iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .or_else(|| eff.env.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
        };
        mounts.extend(binds.iter().map(|(h, c)| Mount { container: join_container("/", c), repo: h.clone() }));
        Ctx {
            shared,
            root,
            context: context.to_string(),
            built: built.cloned(),
            workdir: working_dir.map_or_else(|| eff.workdir.clone(), |w| join_container(&eff.workdir, w)),
            mounts,
            stages,
            pythonpath: var("PYTHONPATH"),
            flask_app: var("FLASK_APP"),
            budget: std::cell::Cell::new(MAX_RESOLVE),
            cut: std::cell::RefCell::new(Vec::new()),
            cut_more: Cell::new(0),
        }
    }

    fn to_repo(&self, container: &str) -> Option<String> {
        map_to_repo(&self.mounts, container)
    }

    /// Đường dẫn như lệnh viết → file trong repo, qua mount; không có mount nào
    /// thì thử tương đối với build context.
    fn locate(&self, path: &str) -> Option<String> {
        let container = join_container(&self.workdir, path);
        if let Some(repo) = self.to_repo(&container)
            && kind_in(self.root, &repo).is_some()
        {
            return Some(repo);
        }
        if !path.starts_with('/') {
            let repo = within(&self.context, path)?;
            return kind_in(self.root, &repo).is_some().then_some(repo);
        }
        None
    }

    /// Đường dẫn trong repo mà lệnh trỏ tới, có trên đĩa nhưng không đọc được
    /// vì symlink hay không phải file thường. Chỉ hỏi có tồn tại, không đọc.
    fn unreadable(&self, path: &str) -> Option<String> {
        let mut candidates: Vec<String> = self.to_repo(&join_container(&self.workdir, path)).into_iter().collect();
        if !path.starts_with('/') {
            candidates.extend(within(&self.context, path));
        }
        candidates.into_iter().find(|r| kind_in(self.root, r).is_none() && self.root.join(r).symlink_metadata().is_ok())
    }

    fn start(&self, argv: &[String], file: &str, line: usize) -> Start {
        let launches = command::launches(argv);
        let mut entries = Vec::new();
        let mut shown = Vec::new();
        let mut unresolved = Vec::new();
        if launches.is_empty() {
            self.stop("lệnh không chạy chương trình nào archctl nhận ra".into());
        }
        for l in &launches {
            shown.push(describe(l));
            let before = entries.len();
            self.resolve(l, 0, &mut entries);
            if entries.len() == before && !matches!(l, Launch::Opaque(_)) {
                unresolved.push(describe(l));
            }
        }
        entries.dedup();
        let mut cut = self.cut.take();
        if self.cut_more.get() > 0 {
            cut.push(format!("và {} chỗ khác", self.cut_more.get()));
        }
        Start { file: file.to_string(), line, shown: shown.join(", "), entries, unresolved, cut }
    }

    /// Ghi một chỗ thôi đọc, mỗi lý do một lần.
    fn stop(&self, why: String) {
        let mut cut = self.cut.borrow_mut();
        if cut.contains(&why) {
            return;
        }
        if cut.len() < MAX_CUT {
            cut.push(why);
        } else {
            self.cut_more.set(self.cut_more.get() + 1);
        }
    }

    fn resolve(&self, launch: &Launch, hops: usize, out: &mut Vec<Entry>) {
        if hops > MAX_HOPS {
            self.stop(format!("script gọi script quá {MAX_HOPS} tầng"));
            return;
        }
        if self.budget.get() == 0 {
            self.stop(format!("quá {MAX_RESOLVE} bước lần theo"));
            return;
        }
        if !Shared::spend(&self.shared.steps, 1) {
            self.stop(format!("quá {MAX_LOAD_STEPS} bước lần theo cho cả cấu hình"));
            return;
        }
        self.budget.set(self.budget.get() - 1);
        match launch {
            Launch::Module { name, main, dir } => {
                let mut roots_c = vec![join_container(&self.workdir, dir.as_deref().unwrap_or("."))];
                for p in self.pythonpath.iter().flat_map(|p| p.split(':')).filter(|p| !p.is_empty()) {
                    roots_c.push(join_container(&self.workdir, p));
                }
                let roots: Vec<String> = roots_c
                    .iter()
                    .filter_map(|c| self.to_repo(c))
                    .filter(|r| kind_in(self.root, r) == Some(Kind::Dir))
                    .collect();
                out.push(Entry::Module { name: name.clone(), roots, main: *main });
            }
            Launch::Script(path) => {
                let Some(repo) = self.locate(path) else {
                    if let Some(r) = self.unreadable(path) {
                        self.stop(format!("{}: có symlink trên đường dẫn, hay không phải file thường", shown_path(&r)));
                    }
                    return;
                };
                if kind_in(self.root, &repo) != Some(Kind::File) {
                    return;
                }
                match self.shared.is_shell_script(&repo) {
                    Ok(true) => match self.shared.launches(&repo) {
                        Ok(launches) => self.nested(&launches, hops, out),
                        Err(why) => self.stop(format!("{}: {why}", shown_path(&repo))),
                    },
                    Ok(false) => out.push(Entry::File(repo)),
                    Err(why) => self.stop(format!("{}: {why}", shown_path(&repo))),
                }
            }
            Launch::Npm(name) => {
                let Some(pkg) = self.locate("package.json") else { return };
                let Some(json) = self.shared.package(&pkg) else { return };
                let Some(script) = json.get_path(&["scripts", name]).and_then(|v| v.as_str()) else { return };
                self.nested(&command::launches_in_shell(script), hops, out);
            }
            Launch::OneOf(candidates) => {
                if let Some(repo) = candidates.iter().find_map(|c| self.locate(c).filter(|r| kind_in(self.root, r) == Some(Kind::File))) {
                    out.push(Entry::File(repo));
                }
            }
            Launch::FlaskApp => {
                let targets: Vec<String> = match &self.flask_app {
                    Some(app) => vec![app.split(':').next().unwrap_or(app).to_string()],
                    None => vec!["wsgi.py".into(), "app.py".into()],
                };
                for t in targets {
                    if t.ends_with(".py") {
                        if let Some(repo) = self.locate(&t).filter(|r| kind_in(self.root, r) == Some(Kind::File)) {
                            out.push(Entry::File(repo));
                            return;
                        }
                    } else {
                        self.resolve(&Launch::Module { name: t, main: false, dir: None }, hops + 1, out);
                        return;
                    }
                }
            }
            Launch::GoRun(pkg) => {
                if let Some(repo) = self.locate(pkg)
                    && kind_in(self.root, &repo) == Some(Kind::Dir)
                {
                    out.push(Entry::Dir(repo));
                }
            }
            Launch::Program(path) => {
                if let Some(repo) = self.locate(path)
                    && self.shared.is_shell_script(&repo) == Ok(true)
                {
                    self.resolve(&Launch::Script(path.clone()), hops, out);
                    return;
                }
                self.go_binary(path, out);
            }
            Launch::Opaque(why) => self.stop(why.clone()),
        }
    }

    /// Lệnh trong một script (shell, hay script npm). Lệnh nào không lần ra
    /// mã thì ghi lại: lệnh ở tầng ngoài cùng đã được [`Ctx::start`] báo, còn
    /// lệnh lồng trong script thì chỉ có ở đây (TM-12).
    fn nested(&self, launches: &[Launch], hops: usize, out: &mut Vec<Entry>) {
        for l in launches {
            let before = out.len();
            self.resolve(l, hops + 1, out);
            if out.len() == before && !matches!(l, Launch::Opaque(_)) {
                self.stop(format!("`{}` không lần ra mã trong repo", describe(l)));
            }
        }
    }

    /// Binary dựng bằng `go build` trong Dockerfile: package nào ra binary này.
    /// Không khớp tên được mà Dockerfile chỉ có một lần `go build` thì lấy nó.
    fn go_binary(&self, program: &str, out: &mut Vec<Entry>) {
        let Some(built) = &self.built else { return };
        let name = program.rsplit('/').next().unwrap_or(program);
        let builds = built.builds();
        let named: Vec<&GoBuild> =
            builds.iter().filter(|b| b.0.as_deref().is_some_and(|o| o.rsplit('/').next() == Some(name))).collect();
        let chosen: Vec<&GoBuild> = match (named.is_empty(), builds.len()) {
            (false, _) => named,
            (true, 1) => builds.iter().collect(),
            _ => Vec::new(),
        };
        for (_, pkgs, stage) in chosen {
            // Đường dẫn trong stage dựng, qua `COPY` của chính stage đó.
            let Some(mounts) = self.stages.get(*stage) else { continue };
            for p in pkgs {
                // `go build main.go` truyền file: package là thư mục chứa nó.
                let Some(repo) = map_to_repo(mounts, p) else { continue };
                match kind_in(self.root, &repo) {
                    Some(Kind::Dir) => out.push(Entry::Dir(repo)),
                    Some(Kind::File) if repo.ends_with(".go") => out.push(Entry::Dir(dir_of(&repo).to_string())),
                    _ => {}
                }
            }
        }
    }
}

/// File trong repo mà một điểm vào nạp trực tiếp. Module không có trong
/// `roots` thì trả `Err(tên)`, để bên gọi tra theo các gói của dự án.
pub fn entry_files(root: &Path, entry: &Entry) -> Result<Vec<String>, String> {
    match entry {
        Entry::File(f) => Ok(vec![f.clone()]),
        Entry::Dir(d) => {
            let mut out: Vec<String> = std::fs::read_dir(root.join(d))
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|e| e.file_name().into_string().ok())
                        .map(|n| if d.is_empty() { n } else { format!("{d}/{n}") })
                        .filter(|f| {
                            super::Lang::for_path(f).is_some()
                                && !f.ends_with("_test.go")
                                && kind_in(root, f) == Some(Kind::File)
                        })
                        .collect()
                })
                .unwrap_or_default();
            out.sort();
            Ok(out)
        }
        Entry::Module { name, roots, main } => {
            let path = name.replace('.', "/");
            for r in roots {
                let base = if r.is_empty() { path.clone() } else { format!("{r}/{path}") };
                let file = format!("{base}.py");
                if kind_in(root, &file) == Some(Kind::File) {
                    return Ok(vec![file]);
                }
                let mut found: Vec<String> = Vec::new();
                for f in [format!("{base}/__init__.py"), format!("{base}/__main__.py")] {
                    if (*main || f.ends_with("__init__.py")) && kind_in(root, &f) == Some(Kind::File) {
                        found.push(f);
                    }
                }
                if !found.is_empty() {
                    return Ok(found);
                }
            }
            Err(name.clone())
        }
    }
}

/// File mà khi đổi thì phần hạ tầng phải kiểm lại: compose, Dockerfile,
/// `env_file`, script khởi chạy, `package.json`.
pub fn touches_deploy(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    is_compose(name) || is_dockerfile(name) || name.starts_with(".env") || name.ends_with(".sh") || name == "package.json"
}

/// `go build [cờ] [-o out] [package...]` trong một lệnh `RUN`.
fn go_builds(run: &str) -> Vec<(Option<String>, Vec<String>)> {
    const VALUE_FLAGS: &[&str] = &["-o", "-ldflags", "-gcflags", "-asmflags", "-tags", "-mod", "-p", "-pkgdir", "-buildmode", "-compiler", "-installsuffix", "-modfile", "-overlay", "-pgo", "-C"];
    let mut out = Vec::new();
    for segment in run.split(['&', ';', '|', '\n']) {
        let words = dockerfile::shell_words(segment);
        let Some(at) = words.windows(2).position(|w| w[0].rsplit('/').next() == Some("go") && w[1] == "build") else {
            continue;
        };
        let mut output = None;
        let mut pkgs = Vec::new();
        let mut i = at + 2;
        while let Some(w) = words.get(i) {
            if let Some((flag, v)) = w.split_once('=')
                && flag.starts_with('-')
            {
                if flag == "-o" {
                    output = Some(v.to_string());
                }
            } else if VALUE_FLAGS.contains(&w.as_str()) {
                if w == "-o" {
                    output = words.get(i + 1).cloned();
                }
                i += 1;
            } else if !w.starts_with('-') {
                pkgs.push(w.clone());
            }
            i += 1;
        }
        out.push((output, pkgs));
    }
    out
}

fn describe(l: &Launch) -> String {
    match l {
        Launch::Module { name, .. } => name.clone(),
        Launch::Script(p) | Launch::GoRun(p) | Launch::Program(p) => p.clone(),
        Launch::Npm(n) => format!("npm run {n}"),
        Launch::OneOf(c) => c.first().map_or_else(String::new, |f| f.rsplit_once('.').map_or(f.clone(), |(s, _)| format!("{s}.*"))),
        Launch::FlaskApp => "flask run".into(),
        Launch::Opaque(why) => why.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duong_dan_khong_ra_ngoai_goc() {
        assert_eq!(within("a/b", "../c").as_deref(), Some("a/c"));
        assert_eq!(within("a", "./x/./y").as_deref(), Some("a/x/y"));
        assert_eq!(within("a", "../.."), None);
        assert_eq!(within("", ".."), None);
        assert_eq!(within("a", "/etc/passwd"), None);
        assert_eq!(within("a", "~/x"), None);
        assert_eq!(within("a", "https://github.com/x/y.git"), None);
        assert_eq!(within("", "."), Some(String::new()));
    }

    #[test]
    fn nhan_ra_ten_file() {
        for yes in ["docker-compose.yml", "compose.yaml", "docker-compose.prod.yml", "compose.override.yml", "docker-compose-dev.yaml"] {
            assert!(is_compose(yes), "{yes}");
        }
        for no in ["composer.yml", "docker-compose.json", "my-compose.yml", "compose.txt", "docker-compose.test.yml", "compose.e2e.yaml"] {
            assert!(!is_compose(no), "{no}");
        }
        assert!(is_dockerfile("Dockerfile") && is_dockerfile("Dockerfile.dev") && is_dockerfile("api.Dockerfile"));
        assert!(!is_deploy_file("tests/docker-compose.yml") && is_deploy_file("deploy/docker-compose.yml"));
        assert!(!is_deploy_file(".devcontainer/docker-compose.yaml") && !is_deploy_file(".github/compose.yml"));
    }

    #[test]
    fn lenh_go_build() {
        assert_eq!(go_builds("CGO_ENABLED=0 go build -ldflags=\"-s\" -o /go/bin/app ./cmd/app"), [(Some("/go/bin/app".to_string()), vec!["./cmd/app".to_string()])]);
        assert_eq!(go_builds("cd src && go build -o server ."), [(Some("server".to_string()), vec![".".to_string()])]);
        assert_eq!(go_builds("go mod download"), []);
    }
}
