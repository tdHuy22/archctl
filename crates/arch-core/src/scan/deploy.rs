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
use super::k8s;
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
/// Byte đọc từ các file YAML của repo để tìm manifest, cho cả lần nạp.
const MAX_K8S_READ: usize = 32 << 20;
/// File YAML mở ra (manifest, patch) cho cả lần nạp: mỗi lần mở là vài lời
/// gọi hệ thống, và repo đối kháng có hàng chục nghìn file nhỏ (TM-18).
const MAX_K8S_FILES: usize = 10_000;
/// Workload, Service, ConfigMap cho cả repo: manifest thật có vài trăm.
const MAX_K8S_OBJECTS: usize = 20_000;
const MAX_KUSTOMIZATIONS: usize = 2_000;
/// Bước so selector với nhãn pod, và bước lần `resources` của Kustomize, cho
/// cả lần nạp: hàng nghìn Service cùng chọn một nhãn mà hàng nghìn workload
/// cùng mang là bậc hai (TM-18).
const MAX_K8S_STEPS: usize = 500_000;
/// Cạnh mạng từ manifest cho cả repo. Ứng dụng thật có vài trăm; mỗi cạnh
/// thành một vi phạm có thể phải in ra (TM-18).
const MAX_K8S_EDGES: usize = 20_000;

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

/// File cấu hình triển khai của sản phẩm, không nằm trong thư mục test, ví dụ:
/// compose, Dockerfile, và mọi file YAML (manifest Kubernetes, `skaffold.yaml`,
/// `kustomization.yaml`, `Chart.yaml`), vì manifest đặt tên tuỳ ý.
pub fn is_deploy_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    (is_compose(name) || is_dockerfile(name) || is_manifest_file(name) || k8s::is_kustomization(name)) && !aside(path)
}

/// Một service lấy từ cấu hình.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    /// Service tên `name` trong các file compose ở thư mục `dir` (`""` là gốc).
    Compose { dir: String, name: String },
    /// Service dựng từ một Dockerfile, không có compose.
    Dockerfile(String),
    /// Workload Kubernetes tên `name` trong manifest ở thư mục `dir`.
    K8s { dir: String, name: String },
}

impl Key {
    /// `compose:api`, `compose:deploy/api`, `k8s:deploy/api`, `dockerfile:x`.
    pub fn text(&self) -> String {
        let scoped = |dir: &str| if dir.is_empty() { String::new() } else { format!("{dir}/") };
        match self {
            Key::Compose { dir, name } => format!("compose:{}{name}", scoped(dir)),
            Key::K8s { dir, name } => format!("k8s:{}{name}", scoped(dir)),
            Key::Dockerfile(p) => format!("dockerfile:{p}"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Deploy {
    pub services: Vec<Svc>,
    pub unread: Vec<Unread>,
    /// Thư mục chart Helm: template trong đó không được render, nên không đọc.
    pub charts: Vec<String>,
}

/// Điều chỉnh một lần nạp.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Đọc manifest Kubernetes: chỉ khi luật có nguồn `k8s:`, hay khi `init`,
    /// `scan` (TM-18).
    pub k8s: bool,
    /// `dockerfile:` khai cùng `k8s:` trong luật: image của container chính.
    pub pins: Vec<Pin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub dir: Option<String>,
    pub name: String,
    pub dockerfile: String,
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
    /// Patch Kustomize đổi lệnh, env hay image của workload này mà archctl
    /// không áp: báo ra khi workload được khai trong luật (TM-21).
    pub patched: Vec<Unread>,
    /// Dockerfile dựng service (container chính của workload): `init` gộp
    /// service compose với workload cùng tên khi chúng dựng từ cùng một chỗ.
    pub built: Option<String>,
    /// NetworkPolicy cho workload này: ai được gọi vào, nó được gọi ra đâu.
    pub ingress: Guard,
    pub egress: Guard,
}

/// Một chiều (vào hay ra) của NetworkPolicy đối với một workload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guard {
    /// Có policy cùng chiều chọn workload này: chỉ đường trong `grants` được đi.
    pub isolated: bool,
    /// Luật của từng policy chọn workload này, dùng chung giữa các workload: một
    /// policy chọn hàng nghìn pod không nhân bản luật của nó (TM-23).
    pub grants: Vec<Rc<Vec<Grant>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// Mọi pod: luật không ghi nguồn (đích), hay `podSelector: {}`.
    Any,
    /// Không kết luận được: namespace chọn theo nhãn mà repo không cho biết.
    Unknown,
    /// Workload cụ thể mà một luật cho phép, kèm policy và dòng của luật.
    Pods { keys: Rc<Vec<Key>>, policy: String, file: String, line: usize },
}

impl Guard {
    /// Đường tới (hay từ) workload `key` có được đi không. Không bị cô lập là
    /// được; không kết luận được thì coi như được, để không báo nhầm (TM-25).
    pub fn allows(&self, key: &Key) -> bool {
        !self.isolated
            || self.grants.iter().flat_map(|g| g.iter()).any(|g| match g {
                Grant::Any | Grant::Unknown => true,
                Grant::Pods { keys, .. } => keys.contains(key),
            })
    }
}

/// Cạnh mạng thấy trong cấu hình.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edge {
    /// Tên service compose hay workload đích, để in.
    pub to: String,
    pub target: Key,
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
    match read_head(root, rel, max)? {
        (text, false) => Ok(text),
        (_, true) => Err(format!("quá {} KB", max >> 10)),
    }
}

/// Như [`read_in`], nhưng file dài hơn `max` thì trả `max` byte đầu (cắt ở
/// ranh giới ký tự) kèm cờ "còn nữa": để biết một file YAML lớn có phải
/// manifest không mà không đọc hết nó.
pub fn read_head(root: &Path, rel: &str, max: u64) -> Result<(String, bool), String> {
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
    let mut bytes = Vec::new();
    file.take(max + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    let more = bytes.len() as u64 > max;
    if more {
        bytes.truncate(max as usize);
    }
    match String::from_utf8(bytes) {
        Ok(text) => Ok((text, more)),
        // Cắt giữa một ký tự nhiều byte: bỏ mảnh cuối.
        Err(e) if more && e.utf8_error().error_len().is_none() => {
            let at = e.utf8_error().valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(at);
            Ok((String::from_utf8(bytes).unwrap_or_default(), more))
        }
        Err(_) => Err("không phải văn bản UTF-8".into()),
    }
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
    /// Byte còn được đọc để tìm manifest Kubernetes: mọi file YAML của repo
    /// là ứng viên, nên tính riêng với file cấu hình được trỏ tới.
    k8s_read: Cell<usize>,
    /// File YAML còn được mở.
    k8s_files: Cell<usize>,
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
            k8s_read: Cell::new(MAX_K8S_READ),
            k8s_files: Cell::new(MAX_K8S_FILES),
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

/// Nạp mọi project compose trong `files` (đường dẫn tương đối của repo), và
/// manifest Kubernetes khi `opts.k8s`.
pub fn load(root: &Path, files: &[String], opts: &Options) -> Deploy {
    let shared = Shared::new(root);
    let mut builds: Vec<BuildRef> = Vec::new();
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
        builds.extend(records.iter().filter_map(compose_build));
    }
    if opts.k8s {
        load_k8s(&shared, files, opts, &builds, &mut out);
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
            patched: Vec::new(),
            built: None,
            ingress: Guard::default(),
            egress: Guard::default(),
        };
        let mut seen: HashSet<(String, String, usize)> = HashSet::new();
        let mut push_edge = |to: &str, file: &str, line: usize, shown: String| {
            if let Some(&target) = names.get(&to.to_ascii_lowercase())
                && target != name
                && seen.insert((target.to_string(), file.to_string(), line))
            {
                svc.edges.push(Edge {
                    to: target.to_string(),
                    target: Key::Compose { dir: dir.to_string(), name: target.to_string() },
                    file: file.to_string(),
                    line,
                    shown,
                });
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
        patched: Vec::new(),
        built: Some(path.to_string()),
        ingress: Guard::default(),
        egress: Guard::default(),
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

/// Image dựng từ repo: artifact của skaffold, hay service compose có cả
/// `image:` lẫn `build:`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct BuildRef {
    image: String,
    /// Build context trong repo.
    context: String,
    /// Dockerfile trong repo; `None` khi dựng không qua Dockerfile (`jib`, `ko`).
    dockerfile: Option<String>,
    target: Option<String>,
    /// Chỗ khai, để in.
    at: String,
}

fn compose_build(r: &Record) -> Option<BuildRef> {
    let image = r.s.image.as_ref()?;
    let b = r.s.build.as_ref()?;
    if b.inline.is_some() {
        return None;
    }
    let context = within(&r.base, &b.context)?;
    let dockerfile = within(&context, b.dockerfile.as_deref().unwrap_or("Dockerfile"))?;
    Some(BuildRef {
        image: image.clone(),
        context,
        dockerfile: Some(dockerfile),
        target: b.target.clone(),
        at: format!("{}:{}", r.file, b.line),
    })
}

pub fn is_yaml(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".yaml") || lower.ends_with(".yml")
}

/// File có thể là manifest: YAML, hay JSON (`kubectl apply -f x.json`).
pub fn is_manifest_file(path: &str) -> bool {
    is_yaml(path) || path.to_ascii_lowercase().ends_with(".json")
}

/// Hai namespace có thể là một: thiếu một bên thì namespace đặt lúc
/// `kubectl apply -n`, không biết, nên coi như khớp.
fn same_ns(a: Option<&str>, b: Option<&str>) -> bool {
    match (a, b) {
        (Some(x), Some(y)) => x == y,
        _ => true,
    }
}

/// Một tài liệu đã đọc, kèm file chứa nó.
struct Placed<T> {
    file: String,
    dir: String,
    item: T,
}

/// Mọi thứ đọc được từ manifest của repo.
#[derive(Default)]
struct Cluster {
    workloads: Vec<Placed<k8s::Workload>>,
    services: Vec<Placed<k8s::Service>>,
    policies: Vec<Placed<k8s::NetPolicy>>,
    config_maps: Vec<Placed<k8s::ConfigMap>>,
    namespaces: HashSet<String>,
}

/// `kustomization.yaml` đã đọc.
struct Kust {
    file: String,
    dir: String,
    k: k8s::Kustomization,
}

/// Nạp manifest Kubernetes: workload thành [`Svc`] với khoá [`Key::K8s`].
fn load_k8s(shared: &Shared<'_>, files: &[String], opts: &Options, compose_builds: &[BuildRef], out: &mut Deploy) {
    let mut charts: Vec<String> = Vec::new();
    let mut skaffolds: Vec<&String> = Vec::new();
    let mut kusts: Vec<&String> = Vec::new();
    let mut candidates: Vec<&String> = Vec::new();
    for f in files {
        let name = f.rsplit('/').next().unwrap_or(f);
        if aside(f) || is_compose(name) {
            continue;
        }
        if name == "Chart.yaml" {
            charts.push(dir_of(f).to_string());
        } else if k8s::is_skaffold(name) {
            skaffolds.push(f);
        } else if k8s::is_kustomization(name) {
            kusts.push(f);
        } else if is_manifest_file(f) {
            candidates.push(f);
        }
    }
    charts.sort();
    // Template Helm không phải YAML cho tới khi render: không đọc (bạn chốt
    // "không render Helm"), và `scan` in thư mục chart ra.
    let in_chart = |f: &str| {
        charts.iter().any(|c| {
            let p = if c.is_empty() { String::new() } else { format!("{c}/") };
            f.starts_with(&format!("{p}templates/")) || f.starts_with(&format!("{p}charts/"))
        })
    };
    out.charts = charts.clone();

    // Image dựng từ đâu.
    let mut builds: Vec<BuildRef> = Vec::new();
    for f in skaffolds {
        let parsed = shared.read(f).and_then(|t| k8s::skaffold(&t).map_err(|e| format!("YAML {e}")));
        let s = match parsed {
            Ok(s) => s,
            Err(why) => {
                out.unread.push(Unread { file: f.clone(), why });
                continue;
            }
        };
        let base = dir_of(f);
        for a in &s.artifacts {
            let Some(context) = within(base, &a.context) else {
                out.unread.push(Unread {
                    file: format!("{f}:{}", a.line),
                    why: format!("`context: {}` nằm ngoài dự án", shown_path(&a.context)),
                });
                continue;
            };
            let dockerfile = match &a.dockerfile {
                None => None,
                Some(d) => match within(&context, d) {
                    Some(p) => Some(p),
                    None => {
                        out.unread.push(Unread {
                            file: format!("{f}:{}", a.line),
                            why: format!("`dockerfile: {}` nằm ngoài dự án", shown_path(d)),
                        });
                        continue;
                    }
                },
            };
            builds.push(BuildRef {
                image: a.image.clone(),
                context,
                dockerfile,
                target: a.target.clone(),
                at: format!("{f}:{}", a.line),
            });
        }
    }
    builds.extend(compose_builds.iter().cloned());
    // Tra image theo đoạn cuối của tên: mọi cách khớp của `same_image` đều
    // cần đoạn cuối bằng nhau, và quét tuyến tính cho từng container là bậc
    // hai khi skaffold có hàng chục nghìn artifact (TM-18).
    let mut by_image: HashMap<String, Vec<&BuildRef>> = HashMap::new();
    for b in &builds {
        by_image.entry(k8s::image_name(&b.image).1.to_string()).or_default().push(b);
    }

    // Kustomize: file patch không phải manifest; patch và phép biến đổi đổi
    // lệnh, env, image, tên của workload thì báo ra (TM-21).
    let steps = Cell::new(MAX_K8S_STEPS);
    let kz = kustomize(shared, &kusts, &steps, out);
    // File mà một kustomization kéo vào được đọc kể cả khi nằm trong thư mục
    // ví dụ hay ẩn: nó được triển khai.
    let listed: HashSet<&str> = candidates.iter().map(|f| f.as_str()).collect();
    let mut reached: Vec<String> =
        kz.reached.iter().filter(|f| is_manifest_file(f) && !listed.contains(f.as_str())).cloned().collect();
    reached.sort();
    let all: Vec<String> = candidates.iter().map(|f| (*f).clone()).chain(reached).collect();
    let mut cluster = read_manifests(shared, &all, &in_chart, &kz.patch_files, out);
    cluster.namespaces.extend(kz.namespaces.iter().cloned());
    cluster.namespaces.insert("default".into());
    let patched = apply_patches(&kz, &cluster, &steps);

    // Chỉ mục: Service theo tên, workload theo nhãn, ConfigMap theo tên.
    let mut svc_by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, s) in cluster.services.iter().enumerate() {
        svc_by_name.entry(s.item.name.to_ascii_lowercase()).or_default().push(i);
    }
    let mut by_label: HashMap<(&str, &str), Vec<usize>> = HashMap::new();
    for (i, w) in cluster.workloads.iter().enumerate() {
        for (k, v) in &w.item.labels {
            by_label.entry((k.as_str(), v.as_str())).or_default().push(i);
        }
    }
    let mut cm_by_name: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, c) in cluster.config_maps.iter().enumerate() {
        cm_by_name.entry(c.item.name.as_str()).or_default().push(i);
    }
    let mut selected: HashMap<usize, Rc<Vec<usize>>> = HashMap::new();
    let steps_over = Cell::new(false);
    let edges_left = Cell::new(MAX_K8S_EDGES);
    // Workload mà một Service chọn: nhãn pod chứa mọi cặp của selector, cùng
    // namespace. Ưu tiên workload cùng thư mục với Service: `release/` và
    // `kubernetes-manifests/` thường là hai bản của cùng một ứng dụng.
    let mut select = |si: usize| -> Rc<Vec<usize>> {
        if let Some(r) = selected.get(&si) {
            return r.clone();
        }
        let s = &cluster.services[si];
        let mut hits: Vec<usize> = Vec::new();
        // Nhãn do controller gắn lúc chạy (`statefulset.kubernetes.io/pod-name`,
        // `job-name`) không có trong pod template: so theo tên workload.
        let (ctl, plain): (Vec<&Label>, Vec<&Label>) =
            s.item.selector.iter().partition(|(k, _)| CONTROLLER_LABELS.contains(&k.as_str()));
        let everyone: Vec<usize>;
        let candidates: &[usize] = match plain.first() {
            Some((k, v)) => by_label.get(&(k.as_str(), v.as_str())).map_or(&[][..], Vec::as_slice),
            None if !ctl.is_empty() => {
                everyone = (0..cluster.workloads.len()).collect();
                &everyone
            }
            None => &[],
        };
        for &wi in candidates {
            if !Shared::spend(&steps, 1 + s.item.selector.len()) {
                steps_over.set(true);
                break;
            }
            let w = &cluster.workloads[wi];
            if same_ns(s.item.namespace.as_deref(), w.item.namespace.as_deref())
                && plain.iter().all(|pair| w.item.labels.contains(pair))
                && ctl.iter().all(|(k, v)| controller_label(k, v, &w.item))
            {
                hits.push(wi);
            }
        }
        let same: Vec<usize> = hits.iter().copied().filter(|&wi| cluster.workloads[wi].dir == s.dir).collect();
        let r = Rc::new(if same.is_empty() { hits } else { same });
        selected.insert(si, r.clone());
        r
    };

    let guards = network_policies(&cluster, &steps);

    let mut over = false;
    for (wi, placed) in cluster.workloads.iter().enumerate() {
        // Hết ngân sách thì dừng hẳn và báo một lần: đọc tiếp nửa vời là bỏ
        // cạnh mà không ai biết (TM-18, TM-21).
        if over {
            out.unread.push(Unread {
                file: placed.file.clone(),
                why: format!("quá {MAX_PROJECT_FACTS} mục cấu hình cho cả repo; workload từ đây trở đi không đọc"),
            });
            break;
        }
        let w = &placed.item;
        let ns = w.namespace.as_deref();
        let mut svc = Svc {
            key: Key::K8s { dir: placed.dir.clone(), name: w.name.clone() },
            file: placed.file.clone(),
            line: w.line,
            image: w.main_container().and_then(|i| w.containers[i].image.clone()),
            code: false,
            edges: Vec::new(),
            starts: Vec::new(),
            unknown: None,
            patched: patched.get(&wi).cloned().unwrap_or_default(),
            built: None,
            ingress: guards.get(&wi).map(|g| g.0.clone()).unwrap_or_default(),
            egress: guards.get(&wi).map(|g| g.1.clone()).unwrap_or_default(),
        };
        // Tên máy → Service → workload.
        let resolve_once = |host: &str| -> Vec<usize> {
            let l: Vec<&str> = host.split('.').collect();
            let find = |name: &str, ns: Option<&str>| -> Vec<usize> {
                let all: Vec<usize> = svc_by_name
                    .get(name)
                    .map(|v| v.iter().copied().filter(|&si| same_ns(cluster.services[si].item.namespace.as_deref(), ns)).collect())
                    .unwrap_or_default();
                let same: Vec<usize> = all.iter().copied().filter(|&si| cluster.services[si].dir == placed.dir).collect();
                if same.is_empty() { all } else { same }
            };
            let known = |n: &str| cluster.namespaces.contains(n);
            match l.as_slice() {
                [svc] => find(svc, ns),
                // `svc.ns.svc`, `svc.ns.svc.cluster.local`.
                [svc, n, "svc", ..] => find(svc, Some(n)),
                // Pod của Service headless: `pod.svc.ns.svc…`.
                [_, svc, n, "svc", ..] => find(svc, Some(n)),
                // `svc.ns` khi `ns` là namespace có trong manifest; không thì
                // `pod.svc`. Tên miền ngoài (`api.example`) không bị ghép.
                [svc, n] if known(n) => find(svc, Some(n)),
                [_, svc] => find(svc, ns),
                [_, svc, n] if known(n) => find(svc, Some(n)),
                _ => Vec::new(),
            }
        };
        // Service `ExternalName` là bí danh: theo nó một bước tới Service thật.
        let resolve = |host: &str| -> Vec<usize> {
            let mut found = resolve_once(host);
            let aliases: Vec<String> = found.iter().filter_map(|&si| cluster.services[si].item.external.clone()).collect();
            for a in aliases {
                found.extend(resolve_once(&a));
            }
            found
        };
        // `x.y` mà `x` là Service của repo, `y` không phải namespace nào đã
        // biết: có thể là Service ở namespace chưa khai, có thể là tên miền
        // ngoài. Không đoán, nhưng báo (TM-21).
        let doubtful = |host: &str| -> Option<String> {
            let (svc, n) = host.split_once('.')?;
            (!n.contains('.') && !cluster.namespaces.contains(n) && svc_by_name.contains_key(svc))
                .then(|| format!("tên máy `{host}` có thể là Service `{svc}` ở namespace `{n}`, mà không manifest nào khai namespace đó"))
        };
        // Cùng một tên máy lặp ở hàng nghìn biến: tính đích một lần.
        let mut memo: HashMap<String, Rc<Vec<usize>>> = HashMap::new();
        let mut seen: HashSet<(Key, String, usize)> = HashSet::new();
        let mut kept: Vec<(String, String)> = Vec::new();
        for c in &w.containers {
            // Biến môi trường, kể cả lấy từ ConfigMap.
            let mut envs: Vec<(String, compose::Env)> = c.env.iter().map(|e| (placed.file.clone(), e.clone())).collect();
            let config = |name: &str| -> Option<&Placed<k8s::ConfigMap>> {
                let list = cm_by_name.get(name)?;
                let ok = |&&i: &&usize| same_ns(cluster.config_maps[i].item.namespace.as_deref(), ns);
                list.iter().filter(ok).find(|&&i| cluster.config_maps[i].dir == placed.dir).or_else(|| list.iter().find(ok)).map(|&i| &cluster.config_maps[i])
            };
            for r in &c.refs {
                if let Some(cm) = config(&r.config_map)
                    && let Some(e) = cm.item.entry(&r.item, &r.key)
                {
                    envs.push((cm.file.clone(), e));
                }
            }
            for f in &c.env_from {
                if let Some(cm) = config(&f.config_map) {
                    envs.extend(cm.item.entries(&f.prefix).into_iter().map(|e| (cm.file.clone(), e)));
                }
            }
            if !Shared::spend(&shared.facts, envs.len()) {
                over = true;
                break;
            }
            for (file, e) in &envs {
                if let Some(v) = &e.value {
                    kept.push((e.key.clone(), v.clone()));
                }
                if e.cut {
                    out.unread.push(cut_value(file, e.line, &e.key));
                }
                for h in &e.hosts {
                    let targets = match memo.get(&h.name) {
                        Some(t) => t.clone(),
                        None => {
                            let mut t: Vec<usize> = resolve(&h.name).into_iter().flat_map(|si| select(si).as_ref().clone()).collect();
                            t.sort_unstable();
                            t.dedup();
                            let t = Rc::new(t);
                            memo.insert(h.name.clone(), t.clone());
                            t
                        }
                    };
                    if targets.is_empty()
                        && let Some(why) = doubtful(&h.name)
                    {
                        out.unread.push(Unread { file: format!("{file}:{}", e.line), why: format!("`{}`: {why}", e.key) });
                    }
                    for &ti in targets.iter() {
                        if !Shared::spend(&steps, 1) {
                            steps_over.set(true);
                            break;
                        }
                        if ti == wi {
                            continue;
                        }
                        let t = &cluster.workloads[ti];
                        let target = Key::K8s { dir: t.dir.clone(), name: t.item.name.clone() };
                        if t.item.name == w.name && t.dir == placed.dir {
                            continue;
                        }
                        if !seen.insert((target.clone(), file.clone(), e.line)) {
                            continue;
                        }
                        if !Shared::spend(&shared.facts, 1) || !Shared::spend(&edges_left, 1) {
                            over = true;
                            break;
                        }
                        svc.edges.push(Edge {
                            to: t.item.name.clone(),
                            target,
                            file: file.clone(),
                            line: e.line,
                            shown: format!("{} → {}", e.key, h.shown()),
                        });
                    }
                }
            }
        }
        if over {
            out.unread.push(Unread {
                file: placed.file.clone(),
                why: format!(
                    "quá {MAX_PROJECT_FACTS} mục cấu hình hay {MAX_K8S_EDGES} cạnh mạng cho cả repo; cạnh mạng từ đây trở đi không đọc"
                ),
            });
        }
        k8s_starts(shared, opts, &by_image, placed, &kept, &mut svc, &mut out.unread);
        out.services.push(svc);
    }
    if steps_over.get() || steps.get() == 0 {
        out.unread.push(Unread {
            file: "manifest Kubernetes".into(),
            why: format!("quá {MAX_K8S_STEPS} bước so selector hay lần `resources`; phần còn lại không đọc"),
        });
    }
}

/// Ngoài cụm hay namespace theo nhãn: phạm vi namespace của một peer.
enum Scope<'a> {
    /// Namespace của chính policy (`None`: không ghi, đặt lúc apply).
    Same(Option<&'a str>),
    Any,
    /// Namespace theo tên (`kubernetes.io/metadata.name`, nhãn Kubernetes tự gắn).
    Names(Vec<&'a str>),
    /// Theo nhãn khác: repo không cho biết namespace nào mang nhãn đó.
    Unknown,
}

/// Workload bị NetworkPolicy cô lập theo từng chiều, và đường được cho phép rõ
/// (TM-24, TM-25). Mỗi policy so selector với mọi workload một lần, tính vào
/// ngân sách bước chung (TM-23).
fn network_policies(cluster: &Cluster, steps: &Cell<usize>) -> HashMap<usize, (Guard, Guard)> {
    const NAME: &str = "kubernetes.io/metadata.name";
    let mut out: HashMap<usize, (Guard, Guard)> = HashMap::new();
    let pick = |sel: &k8s::Selector, scope: &Scope<'_>| -> Option<Vec<usize>> {
        let mut hits = Vec::new();
        for (wi, w) in cluster.workloads.iter().enumerate() {
            if !Shared::spend(steps, 1) {
                return None;
            }
            let ns = w.item.namespace.as_deref();
            let in_scope = match scope {
                Scope::Same(p) => same_ns(*p, ns),
                Scope::Any | Scope::Unknown => true,
                Scope::Names(names) => ns.is_none_or(|n| names.contains(&n)),
            };
            if in_scope && sel.matches(&w.item.labels) {
                hits.push(wi);
            }
        }
        Some(hits)
    };
    let key_of = |wi: usize| {
        let w = &cluster.workloads[wi];
        Key::K8s { dir: w.dir.clone(), name: w.item.name.clone() }
    };
    for p in &cluster.policies {
        let pol = &p.item;
        let Some(targets) = pick(&pol.pods, &Scope::Same(pol.namespace.as_deref())) else { return out };
        if targets.is_empty() {
            continue;
        }
        let mut sides: Vec<Option<Rc<Vec<Grant>>>> = Vec::new();
        for rules in [&pol.ingress, &pol.egress] {
            let Some(rules) = rules else {
                sides.push(None);
                continue;
            };
            let mut grants = Vec::new();
            // Peer cho ra cùng tập workload chỉ giữ một: policy đối kháng lặp
            // một selector hàng nghìn lần (TM-23).
            let mut kept: HashSet<Vec<usize>> = HashSet::new();
            for rule in rules {
                if rule.peers.is_empty() {
                    grants.push(Grant::Any);
                    continue;
                }
                for peer in &rule.peers {
                    if peer.pods.is_none() && peer.namespaces.is_none() {
                        // `ipBlock`: địa chỉ, không phải pod của repo.
                        continue;
                    }
                    let scope = match &peer.namespaces {
                        None => Scope::Same(pol.namespace.as_deref()),
                        Some(sel) if sel.is_empty() => Scope::Any,
                        Some(sel) if sel.exprs.is_empty() && sel.labels.len() == 1 && sel.labels[0].0 == NAME => {
                            Scope::Names(vec![sel.labels[0].1.as_str()])
                        }
                        Some(sel)
                            if sel.labels.is_empty()
                                && sel.exprs.len() == 1
                                && sel.exprs[0].0 == NAME
                                && sel.exprs[0].1 == "In" =>
                        {
                            Scope::Names(sel.exprs[0].2.iter().map(String::as_str).collect())
                        }
                        Some(_) => Scope::Unknown,
                    };
                    let pods = peer.pods.clone().unwrap_or_default();
                    // `podSelector: {}` là mọi pod: không nêu đích cụ thể nào.
                    if pods.is_empty() {
                        grants.push(if matches!(scope, Scope::Unknown) { Grant::Unknown } else { Grant::Any });
                        continue;
                    }
                    let Some(hits) = pick(&pods, &scope) else { return out };
                    if !kept.insert(hits.clone()) {
                        continue;
                    }
                    let keys: Vec<Key> = hits.into_iter().map(key_of).collect();
                    grants.push(Grant::Pods {
                        keys: Rc::new(keys),
                        policy: pol.name.clone(),
                        file: p.file.clone(),
                        line: peer.line,
                    });
                }
            }
            sides.push(Some(Rc::new(grants)));
        }
        for &wi in &targets {
            // Cô lập chỉ khi biết chắc policy và workload cùng namespace (cả hai
            // cùng ghi, hay cùng không ghi): một bên không ghi thì có thể khác
            // namespace, và coi là bị chặn sẽ báo nhầm (TM-25). Đường policy cho
            // phép thì vẫn tính theo phép khớp rộng, để không lách được (TM-24).
            let sure = cluster.workloads[wi].item.namespace == pol.namespace;
            let entry = out.entry(wi).or_default();
            for (side, guard) in sides.iter().zip([&mut entry.0, &mut entry.1]) {
                if let Some(grants) = side {
                    guard.isolated |= sure;
                    guard.grants.push(grants.clone());
                }
            }
        }
    }
    out
}

/// Nhãn mà controller gắn vào pod lúc chạy, không viết trong pod template.
const CONTROLLER_LABELS: &[&str] = &[
    "statefulset.kubernetes.io/pod-name",
    "apps.kubernetes.io/pod-index",
    "controller-revision-hash",
    "pod-template-hash",
    "job-name",
    "batch.kubernetes.io/job-name",
    "controller-uid",
    "batch.kubernetes.io/controller-uid",
    "batch.kubernetes.io/job-completion-index",
];

/// Pod của workload này có thể mang nhãn controller `k=v` không.
fn controller_label(k: &str, v: &str, w: &k8s::Workload) -> bool {
    match k {
        "statefulset.kubernetes.io/pod-name" => {
            w.kind == "StatefulSet" && v.strip_prefix(&w.name).and_then(|r| r.strip_prefix('-')).is_some_and(|i| i.parse::<u32>().is_ok())
        }
        "job-name" | "batch.kubernetes.io/job-name" => match w.kind.as_str() {
            "Job" => v == w.name,
            // Job của CronJob tên `<cronjob>-<số>`.
            "CronJob" => v.strip_prefix(&w.name).is_some_and(|r| r.starts_with('-')),
            _ => false,
        },
        // Băm, uid, chỉ số: không biết trước, nên khớp.
        _ => true,
    }
}

/// Đọc các file YAML có dáng manifest.
fn read_manifests(
    shared: &Shared<'_>,
    candidates: &[String],
    in_chart: &dyn Fn(&str) -> bool,
    patch_files: &HashSet<String>,
    out: &mut Deploy,
) -> Cluster {
    let mut c = Cluster::default();
    let mut objects = 0usize;
    for f in candidates {
        if in_chart(f) || patch_files.contains(f.as_str()) {
            continue;
        }
        let f = &f;
        if shared.k8s_read.get() == 0 || !Shared::spend(&shared.k8s_files, 1) {
            out.unread.push(Unread {
                file: (*f).clone(),
                why: format!(
                    "quá {} MB hay {MAX_K8S_FILES} file YAML cho cả repo; file này và các file sau không đọc",
                    MAX_K8S_READ >> 20
                ),
            });
            break;
        }
        let (text, more) = match read_head(shared.root, f, MAX_DEPLOY_BYTES) {
            Ok(r) => r,
            // Không phải văn bản thì không phải manifest.
            Err(why) if why == "không phải văn bản UTF-8" => continue,
            Err(why) => {
                out.unread.push(Unread { file: (*f).clone(), why });
                continue;
            }
        };
        Shared::spend(&shared.k8s_read, text.len());
        if !k8s::may_be(&text) {
            continue;
        }
        if more {
            out.unread.push(Unread { file: (*f).clone(), why: format!("quá {} KB", MAX_DEPLOY_BYTES >> 10) });
            continue;
        }
        let file = match k8s::parse(&text) {
            Ok(file) => file,
            // Chắc là manifest mà không đọc được thì báo; file chỉ tình cờ có
            // chữ `kind` (cấu hình CI, OpenAPI) thì thôi.
            Err(e) if k8s::looks_like(&text) => {
                out.unread.push(Unread { file: (*f).clone(), why: format!("YAML {e}") });
                continue;
            }
            Err(_) => continue,
        };
        let facts: usize = file
            .workloads
            .iter()
            .map(|w| w.labels.len() + w.containers.iter().map(|c| 1 + c.env.len() + c.refs.len() + c.env_from.len()).sum::<usize>())
            .sum::<usize>()
            + file.services.iter().map(|s| 1 + s.selector.len()).sum::<usize>()
            + file.config_maps.iter().map(|m| 1 + m.keys().count()).sum::<usize>();
        objects += file.workloads.len() + file.services.len() + file.config_maps.len() + file.policies.len();
        if objects > MAX_K8S_OBJECTS || !Shared::spend(&shared.facts, facts) || !Shared::spend(&shared.bytes, file.size) {
            out.unread.push(Unread {
                file: (*f).clone(),
                why: format!(
                    "quá {MAX_K8S_OBJECTS} workload, Service, ConfigMap, {MAX_PROJECT_FACTS} mục hay {} MB cho cả repo; file này và các file sau không đọc",
                    MAX_LOAD_BYTES >> 20
                ),
            });
            break;
        }
        for (kind, name, line) in &file.unknown {
            out.unread.push(Unread {
                file: format!("{f}:{line}"),
                why: format!("`{kind}` `{name}` có pod template mà archctl không đọc loại này; mã chạy trong đó không được kiểm"),
            });
        }
        let dir = dir_of(f).to_string();
        c.namespaces.extend(file.namespaces.iter().cloned());
        for w in file.workloads {
            c.namespaces.extend(w.namespace.clone());
            // Không container nào có image: mảnh patch (overlay, component),
            // không phải workload chạy được.
            if w.containers.iter().all(|c| c.image.as_deref().is_none_or(str::is_empty)) {
                continue;
            }
            c.workloads.push(Placed { file: (*f).clone(), dir: dir.clone(), item: w });
        }
        for s in file.services {
            c.namespaces.extend(s.namespace.clone());
            c.services.push(Placed { file: (*f).clone(), dir: dir.clone(), item: s });
        }
        for m in file.config_maps {
            c.namespaces.extend(m.namespace.clone());
            c.config_maps.push(Placed { file: (*f).clone(), dir: dir.clone(), item: m });
        }
        for p in file.policies {
            c.policies.push(Placed { file: (*f).clone(), dir: dir.clone(), item: p });
        }
    }
    c
}

/// Cặp nhãn `khoá=giá trị`.
type Label = (String, String);

/// Thứ một kustomization đè lên manifest mà archctl không áp: chỗ khai, đích,
/// mô tả.
type Overlay = (String, k8s::Touch, String);

/// Kết quả đọc các `kustomization.yaml`.
#[derive(Default)]
struct Kz {
    /// Với mỗi kustomization: file manifest nó kéo vào (qua `resources`,
    /// `components`), và thứ nó đè lên chúng.
    roots: Vec<(HashSet<String>, Vec<Overlay>)>,
    patch_files: HashSet<String>,
    reached: HashSet<String>,
    namespaces: Vec<String>,
}

/// Các kustomization đã đọc, theo thư mục. Thư mục được `resources` trỏ tới
/// mà không nằm trong danh sách (thư mục ví dụ, ẩn) thì đọc khi cần.
struct Kusts {
    ks: Vec<Kust>,
    by_dir: HashMap<String, Option<usize>>,
    full: bool,
    /// Loại của đường dẫn, mỗi đường dẫn hỏi đĩa một lần: hàng nghìn
    /// kustomization cùng trỏ tới nhau thì `lstat` lặp lại là chỗ tốn (TM-18).
    kinds: HashMap<String, Option<Kind>>,
}

impl Kusts {
    fn kind(&mut self, root: &Path, rel: &str) -> Option<Kind> {
        if let Some(k) = self.kinds.get(rel) {
            return *k;
        }
        let k = kind_in(root, rel);
        self.kinds.insert(rel.to_string(), k);
        k
    }
}

impl Kusts {
    fn add(&mut self, shared: &Shared<'_>, file: &str, out: &mut Deploy) -> Option<usize> {
        if self.ks.len() >= MAX_KUSTOMIZATIONS {
            if !self.full {
                self.full = true;
                out.unread.push(Unread {
                    file: file.to_string(),
                    why: format!("quá {MAX_KUSTOMIZATIONS} file kustomization; phần còn lại không đọc"),
                });
            }
            return None;
        }
        match shared.read(file).and_then(|t| k8s::kustomization(&t).map_err(|e| format!("YAML {e}"))) {
            Ok(k) => {
                self.ks.push(Kust { file: file.to_string(), dir: dir_of(file).to_string(), k });
                Some(self.ks.len() - 1)
            }
            Err(why) => {
                out.unread.push(Unread { file: file.to_string(), why });
                None
            }
        }
    }

    fn at(&mut self, shared: &Shared<'_>, dir: &str, out: &mut Deploy) -> Option<usize> {
        if let Some(i) = self.by_dir.get(dir) {
            return *i;
        }
        let found = ["kustomization.yaml", "kustomization.yml", "Kustomization"]
            .iter()
            .map(|n| if dir.is_empty() { (*n).to_string() } else { format!("{dir}/{n}") })
            .find(|p| self.kind(shared.root, p) == Some(Kind::File));
        let i = found.and_then(|p| self.add(shared, &p, out));
        self.by_dir.insert(dir.to_string(), i);
        i
    }
}

/// Đọc mọi kustomization của repo và những cái chúng trỏ tới: file nào được
/// kéo vào, patch và phép biến đổi nào đè lên chúng.
fn kustomize(shared: &Shared<'_>, listed: &[&String], steps: &Cell<usize>, out: &mut Deploy) -> Kz {
    let mut kb = Kusts { ks: Vec::new(), by_dir: HashMap::new(), full: false, kinds: HashMap::new() };
    for f in listed {
        let dir = dir_of(f).to_string();
        if kb.by_dir.contains_key(&dir) {
            continue;
        }
        let i = kb.add(shared, f, out);
        kb.by_dir.insert(dir, i);
    }
    let mut kz = Kz::default();
    let mut touch_cache: HashMap<String, Rc<Vec<k8s::Touch>>> = HashMap::new();
    // Kustomization đọc thêm khi lần cũng là gốc: patch của một base trỏ vào
    // thư mục ví dụ vẫn áp lên base đó.
    let mut root = 0;
    while root < kb.ks.len() {
        let mut files: HashSet<String> = HashSet::new();
        let mut patches: Vec<Overlay> = Vec::new();
        let mut visited: HashSet<usize> = HashSet::from([root]);
        let mut stack: Vec<(usize, usize, bool)> = vec![(root, 0, true)];
        while let Some((i, depth, own)) = stack.pop() {
            let dir = kb.ks[i].dir.clone();
            let refs: Vec<(String, bool)> = kb.ks[i]
                .k
                .resources
                .iter()
                .map(|(r, _)| (r.clone(), false))
                // Patch của component áp ở kustomization dùng nó.
                .chain(kb.ks[i].k.components.iter().map(|(r, _)| (r.clone(), own)))
                .collect();
            for (rel, patches_too) in refs {
                if !Shared::spend(steps, 1) {
                    return kz;
                }
                let Some(p) = within(&dir, &rel) else { continue };
                match kb.kind(shared.root, &p) {
                    Some(Kind::File) => {
                        files.insert(p);
                    }
                    Some(Kind::Dir) => {
                        if depth < MAX_HOPS
                            && let Some(j) = kb.at(shared, &p, out)
                            && visited.insert(j)
                        {
                            stack.push((j, depth + 1, patches_too));
                        }
                    }
                    None => {}
                }
            }
            if !own {
                continue;
            }
            let k = &kb.ks[i];
            for (key, line) in &k.k.transforms {
                patches.push((
                    format!("{}:{line}", k.file),
                    k8s::Touch { kind: None, name: None, touches: true },
                    format!("`{key}` của Kustomize đổi tên, image hay giá trị env"),
                ));
            }
            for p in &k.k.patches {
                let at = match &p.path {
                    Some(path) => match within(&k.dir, path) {
                        Some(full) => full,
                        None => {
                            out.unread.push(Unread {
                                file: format!("{}:{}", k.file, p.line),
                                why: format!("patch `{}` trỏ ra ngoài dự án", shown_path(path)),
                            });
                            continue;
                        }
                    },
                    None => format!("{}:{}", k.file, p.line),
                };
                let touched: Rc<Vec<k8s::Touch>> = match (&p.inline, &p.path) {
                    (Some(t), _) => Rc::new(t.clone()),
                    (None, Some(_)) => touch_cache
                        .entry(at.clone())
                        .or_insert_with(|| {
                            if !Shared::spend(&shared.k8s_files, 1) {
                                out.unread.push(Unread {
                                    file: at.clone(),
                                    why: format!("quá {MAX_K8S_FILES} file YAML cho cả repo; patch này không đọc"),
                                });
                                // Không đọc được thì coi như đổi mọi thứ.
                                return Rc::new(vec![k8s::Touch { kind: None, name: None, touches: true }]);
                            }
                            match shared.read(&at) {
                                Ok(text) => Rc::new(k8s::touches(&text)),
                                Err(why) => {
                                    out.unread.push(Unread { file: at.clone(), why: format!("patch Kustomize: {why}") });
                                    Rc::new(Vec::new())
                                }
                            }
                        })
                        .clone(),
                    // Patch viết thẳng mà không đọc được (quá trần): coi như
                    // đổi mọi thứ.
                    (None, None) => Rc::new(vec![k8s::Touch { kind: None, name: None, touches: true }]),
                };
                let what = "patch Kustomize đổi lệnh, biến môi trường hay image".to_string();
                match &p.target {
                    // `target` thắng đích viết trong nội dung patch.
                    Some((kind, name)) if touched.iter().any(|t| t.touches) => {
                        patches.push((at, k8s::Touch { kind: kind.clone(), name: name.clone(), touches: true }, what));
                    }
                    Some(_) => {}
                    None => patches.extend(touched.iter().filter(|t| t.touches).map(|t| (at.clone(), t.clone(), what.clone()))),
                }
            }
        }
        kz.reached.extend(files.iter().cloned());
        kz.roots.push((files, patches));
        root += 1;
    }
    for k in &kb.ks {
        kz.namespaces.extend(k.k.namespace.clone());
        for p in &k.k.patches {
            if let Some(path) = &p.path
                && let Some(full) = within(&k.dir, path)
            {
                kz.patch_files.insert(full);
            }
        }
    }
    kz
}

/// Patch và phép biến đổi của Kustomize đè lên workload nào, theo chỉ số
/// workload.
fn apply_patches(kz: &Kz, cluster: &Cluster, steps: &Cell<usize>) -> HashMap<usize, Vec<Unread>> {
    let mut by_file: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, w) in cluster.workloads.iter().enumerate() {
        by_file.entry(w.file.as_str()).or_default().push(i);
    }
    let mut result: HashMap<usize, Vec<Unread>> = HashMap::new();
    let mut seen: HashSet<(usize, &str)> = HashSet::new();
    for (files, patches) in &kz.roots {
        for (at, t, what) in patches {
            for f in files {
                // Patch × file: hàng nghìn patch nhắm hàng nghìn file (TM-18).
                if !Shared::spend(steps, 1) {
                    return result;
                }
                for &wi in by_file.get(f.as_str()).map_or(&[][..], Vec::as_slice) {
                    if !Shared::spend(steps, 1) {
                        return result;
                    }
                    let w = &cluster.workloads[wi].item;
                    if t.kind.as_ref().is_none_or(|k| *k == w.kind)
                        && t.name.as_ref().is_none_or(|n| *n == w.name)
                        && seen.insert((wi, at.as_str()))
                    {
                        result.entry(wi).or_default().push(Unread {
                            file: at.clone(),
                            why: format!("{what} của {} `{}`; archctl không áp", w.kind, w.name),
                        });
                    }
                }
            }
        }
    }
    result
}

/// Lệnh khởi chạy của từng container dựng từ repo. Container chính dùng
/// `dockerfile:` của luật nếu có; còn lại lần image về skaffold, compose.
fn k8s_starts(
    shared: &Shared<'_>,
    opts: &Options,
    by_image: &HashMap<String, Vec<&BuildRef>>,
    placed: &Placed<k8s::Workload>,
    kept: &[(String, String)],
    svc: &mut Svc,
    unread: &mut Vec<Unread>,
) {
    let w = &placed.item;
    let main = w.main_container();
    let pin = opts.pins.iter().find(|p| p.name == w.name && p.dir.as_ref().is_none_or(|d| *d == placed.dir));
    let mut why_main: Option<String> = None;
    for (ci, c) in w.containers.iter().enumerate() {
        let image = c.image.as_deref().unwrap_or("");
        let pinned;
        let build: &BuildRef = match pin {
            Some(p) if Some(ci) == main => {
                pinned = BuildRef {
                    image: image.to_string(),
                    context: dir_of(&p.dockerfile).to_string(),
                    dockerfile: Some(p.dockerfile.clone()),
                    target: None,
                    at: "luật".into(),
                };
                &pinned
            }
            _ => {
                let candidates = by_image.get(k8s::image_name(image).1).map_or(&[][..], Vec::as_slice);
                let mut hits: Vec<&BuildRef> = candidates.iter().copied().filter(|b| k8s::same_image(image, &b.image)).collect();
                hits.sort_by(|a, b| (&a.context, &a.dockerfile, &a.target).cmp(&(&b.context, &b.dockerfile, &b.target)));
                hits.dedup_by(|a, b| (&a.context, &a.dockerfile, &a.target) == (&b.context, &b.dockerfile, &b.target));
                match hits.as_slice() {
                    [] => {
                        if Some(ci) == main {
                            why_main = Some(format!(
                                "image `{}` không lần được về nơi dựng trong repo (skaffold, compose có `image:` và `build:`); \
                                 image dựng từ repo thì khai thêm `dockerfile:` trong luật",
                                shown_path(image)
                            ));
                        }
                        continue;
                    }
                    [one] => one,
                    many => {
                        // Không chọn hộ (TM-22).
                        svc.code = true;
                        svc.unknown = Some(format!(
                            "image `{}` khớp nhiều nơi dựng ({}); khai `dockerfile:` trong luật để chọn",
                            shown_path(image),
                            many.iter().map(|b| b.at.as_str()).collect::<Vec<_>>().join(", ")
                        ));
                        continue;
                    }
                }
            }
        };
        svc.code = true;
        if Some(ci) == main {
            svc.built.clone_from(&build.dockerfile);
        }
        let Some(df_path) = &build.dockerfile else {
            svc.unknown.get_or_insert_with(|| {
                format!("image `{}` dựng không qua Dockerfile ({}), nên không biết lệnh khởi chạy", shown_path(image), build.at)
            });
            continue;
        };
        let built = match shared.dockerfile(df_path) {
            Ok(b) => b,
            Err(why) => {
                unread.push(Unread { file: df_path.clone(), why });
                svc.unknown.get_or_insert_with(|| format!("không đọc được Dockerfile `{df_path}`"));
                continue;
            }
        };
        let target = built.df.target(build.target.as_deref());
        let eff = match target {
            Some(t) => built.effective(t),
            None => Rc::new(dockerfile::Effective { workdir: "/".into(), ..Default::default() }),
        };
        // Kubernetes: `command` thay `ENTRYPOINT`, `args` thay `CMD`; có
        // `command` mà không có `args` thì `CMD` của image bị bỏ.
        let from_image = |x: &Option<(Command, usize)>| x.as_ref().map(|(c, l)| (c.argv(), (df_path.clone(), *l)));
        let from_pod = |x: &Option<(Vec<String>, usize)>| x.as_ref().map(|(a, l)| (a.clone(), (placed.file.clone(), *l)));
        let entry = from_pod(&c.command).or_else(|| from_image(&eff.entrypoint));
        let cmd = match (&c.args, &c.command) {
            (Some(_), _) => from_pod(&c.args),
            (None, Some(_)) => None,
            (None, None) => from_image(&eff.cmd),
        };
        let argv: Vec<String> = entry.iter().chain(cmd.iter()).flat_map(|(a, _)| a.iter().cloned()).collect();
        if argv.is_empty() {
            continue;
        }
        let (file, line) = cmd.as_ref().or(entry.as_ref()).map(|(_, at)| at.clone()).unwrap_or((placed.file.clone(), c.line));
        let ctx = Ctx::new(shared, &build.context, Some(&built), target, &eff, c.working_dir.as_deref(), &[], kept);
        svc.starts.push(ctx.start(&argv, &file, line));
    }
    // Container chính không lần được thì luôn nói ra, kể cả khi một sidecar
    // lần được: thêm một sidecar không được là cách tắt cảnh báo.
    if let Some(why) = why_main {
        if svc.code {
            svc.unknown.get_or_insert(why);
        } else {
            svc.unknown = Some(why);
        }
    } else if svc.starts.iter().all(|s| s.entries.is_empty()) && svc.unknown.is_none() {
        svc.unknown = Some(match svc.starts.first() {
            None => "không có lệnh khởi chạy (command, args, CMD, ENTRYPOINT)".into(),
            Some(s) if s.shown.is_empty() => "không nhận ra lệnh khởi chạy".into(),
            Some(s) => format!("không tìm thấy trong repo mã mà `{}` nạp", s.shown),
        });
    }
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
    if df.is_some() && svc.built.is_none() {
        svc.built = Some(df_path.clone());
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
    is_compose(name)
        || is_dockerfile(name)
        || is_manifest_file(name)
        || k8s::is_kustomization(name)
        || name.starts_with(".env")
        || name.ends_with(".sh")
        || name == "package.json"
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
