//! Phân giải chuỗi import thành đường dẫn trong dự án hoặc tên gói.
//!
//! Đây là chỗ dễ sai nhất của cả bộ kiểm chứng, và khác nhau nhiều giữa các
//! ngôn ngữ. Nguyên tắc chung: thứ nào không phân giải được thì **báo ra**,
//! không im lặng bỏ qua. Bỏ sót âm thầm làm người dùng tin rằng không có vi
//! phạm, tệ hơn hẳn việc nói thẳng là chưa biết.

use crate::jsonc;
use crate::scan::lexer::{Tok, Token};
use crate::scan::{Import, Lang};
use std::cell::{OnceCell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// File hoặc thư mục trong dự án, tương đối so với gốc, dùng dấu `/`.
    /// Go trỏ tới thư mục vì import của Go là cả package.
    Path(String),
    /// Thư viện bên ngoài. Chỉ tham gia luật khi được khai báo `external`.
    Package(String),
    /// Trông như alias cần cấu hình build mà ta chưa đọc.
    Alias,
    /// Trỏ vào trong chính crate này nhưng không ra file nào: `mod` viết lồng
    /// trong file, hoặc một item của chính file đang xét. Không có cạnh, và
    /// cũng không có gì để báo — trình biên dịch đã bảo đảm đường dẫn này tồn
    /// tại, nên nói "không tìm thấy" chỉ là nói sai.
    Internal,
    /// Đường dẫn tương đối nhưng không tìm thấy gì ở đó.
    Missing,
}

/// Import Python có thể đang nạp một module mở rộng viết bằng ngôn ngữ khác —
/// xem [`Resolver::import_targets`]. Chỉ là ứng viên: ghép được với phía
/// native (`#[pymodule]`, `PYBIND11_MODULE`) mới thành cạnh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeImport {
    /// Tên module, đoạn cuối của đường dẫn chấm: thứ phía native đặt tên.
    pub module: String,
    /// Cách viết trong mã.
    pub shown: String,
    /// Module nằm ở đâu: đường dẫn không đuôi, tương đối với gốc. Hai import
    /// cùng chỗ là cùng một module.
    pub at: String,
    /// Có stub `.pyi` ở đúng chỗ đó.
    pub stub: bool,
}

/// Những import module mở rộng được ghép, dạng `(tên module, chỗ)`.
///
/// Phía native chỉ đặt tên module (`_core`), không nói nó nằm trong package
/// nào; điều đó do cấu hình build quyết. Nên khi nhiều chỗ cùng nạp một tên
/// thì phải chọn:
///
/// - Có stub `.pyi` thì stub nói module nằm đâu. `pkgb` import `._core` mà
///   stub nằm trong `pkga` thì `pkgb._core` là thứ khác — file sinh lúc
///   build, hay một import hỏng — và không được ghép.
/// - Không có stub thì mọi import phải trỏ cùng một chỗ. Hai package cùng nạp
///   `_core` của riêng mình thì không biết module native thuộc về bên nào.
pub fn settle_natives<'a>(
    imports: impl IntoIterator<Item = &'a NativeImport>,
) -> HashSet<(&'a str, &'a str)> {
    let mut seen: HashMap<&str, (HashSet<&str>, HashSet<&str>)> = HashMap::new();
    for n in imports {
        let (all, stubs) = seen.entry(n.module.as_str()).or_default();
        all.insert(n.at.as_str());
        if n.stub {
            stubs.insert(n.at.as_str());
        }
    }
    let mut out = HashSet::new();
    for (module, (all, stubs)) in seen {
        let keep = if !stubs.is_empty() {
            stubs
        } else if all.len() == 1 {
            all
        } else {
            HashSet::new()
        };
        out.extend(keep.into_iter().map(|at| (module, at)));
    }
    out
}

/// Một module Python `a/b` có thể là những file này, theo thứ tự. `.pxd` và
/// `.pyx` là của Cython: `cimport` lấy khai báo ở `.pxd`, và một module chỉ có
/// bản `.pyx` vẫn là module sau khi biên dịch.
const PY_MODULE_SUFFIXES: &[&str] = &[".py", "/__init__.py", ".pyi", ".pxd", ".pyx"];

const TS_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "d.ts"];
const JS_TO_TS: &[(&str, &[&str])] =
    &[(".js", &[".ts", ".tsx"]), (".jsx", &[".tsx"]), (".mjs", &[".mts"]), (".cjs", &[".cts"])];
/// Nơi thường đặt header khi `#include "..."` không nằm cạnh file gọi.
const C_INCLUDE_DIRS: &[&str] = &["", "include", "src", "lib"];

/// Header của thư viện chuẩn C, POSIX và Windows. Repo hay có bản thay thế
/// trùng tên — zstd có `wasm-shim/stdio.h`, redis có `msvc_compat/stdint.h` —
/// chỉ dùng cho một đích build. Khớp tên duy nhất vào đó thì mọi file C đều có
/// một cạnh sai tới shim: zstd từng có 363 cạnh như vậy.
const STANDARD_HEADERS: &[&str] = &[
    "assert.h", "complex.h", "ctype.h", "errno.h", "fenv.h", "float.h", "inttypes.h",
    "iso646.h", "limits.h", "locale.h", "math.h", "setjmp.h", "signal.h", "stdalign.h",
    "stdarg.h", "stdatomic.h", "stdbool.h", "stddef.h", "stdint.h", "stdio.h", "stdlib.h",
    "stdnoreturn.h", "string.h", "tgmath.h", "threads.h", "time.h", "uchar.h", "wchar.h",
    "wctype.h", "aio.h", "cpio.h", "dirent.h", "dlfcn.h", "fcntl.h", "fnmatch.h", "glob.h",
    "grp.h", "iconv.h", "langinfo.h", "libgen.h", "monetary.h", "mqueue.h", "netdb.h",
    "nl_types.h", "poll.h", "pthread.h", "pwd.h", "regex.h", "sched.h", "search.h",
    "semaphore.h", "spawn.h", "strings.h", "syslog.h", "tar.h", "termios.h", "ulimit.h",
    "unistd.h", "utime.h", "utmpx.h", "wordexp.h", "alloca.h", "malloc.h", "endian.h",
    "execinfo.h", "getopt.h", "memory.h", "windows.h", "winsock2.h", "ws2tcpip.h", "io.h",
    "process.h", "direct.h", "intrin.h",
];
/// Thư mục header của hệ thống: `<sys/types.h>`, `<netinet/in.h>`.
const STANDARD_HEADER_DIRS: &[&str] = &["sys/", "arpa/", "netinet/", "net/", "linux/", "mach/"];

fn is_standard_header(spec: &str) -> bool {
    STANDARD_HEADERS.contains(&spec) || STANDARD_HEADER_DIRS.iter().any(|d| spec.starts_with(d))
}

/// Giữ những thứ đọc một lần từ đĩa, để không mở lại `go.mod` cho từng import.
pub struct Resolver {
    root: PathBuf,
    /// Mọi module Go trong repo: đường dẫn module → thư mục, dài nhất trước.
    /// Một repo có thể chứa nhiều `go.mod` (etcd có mười bốn); chỉ đọc cái ở
    /// gốc thì mọi import sang module anh em đều thành gói ngoài.
    /// Đọc muộn: đường hook không phải quét cây tìm `go.mod` cho một file
    /// không phải Go.
    go_modules: OnceCell<Vec<(String, String)>>,
    /// Crate trong cùng workspace: tên dùng trong `use` → thư mục `src` của nó.
    /// Thiếu bản đồ này thì mọi phụ thuộc giữa các crate anh em đều bị coi là
    /// gói ngoài, và Rust workspace nào cũng hiện ra là không có quan hệ nào.
    rust_crates: Vec<(String, String)>,
    /// Mọi `tsconfig.json` trong dự án, gần nhất trước. Monorepo có nhiều cấu
    /// hình, và alias chỉ có hiệu lực trong phạm vi cấu hình chứa file gọi.
    ts_configs: Vec<TsConfig>,
    /// Gói trong workspace npm: tên trong `package.json` → thư mục của nó.
    /// Thiếu bản đồ này thì mọi phụ thuộc giữa các gói anh em đều bị coi là
    /// gói ngoài, và monorepo nào cũng hiện ra là không có quan hệ nào — chỗ
    /// mà luật kiến trúc cần nhất.
    ///
    /// Đọc muộn như `c_includes`: chỉ cần tới khi gặp một import không tương
    /// đối và không khớp alias nào, nên repo không phải TypeScript không tốn
    /// gì cả.
    npm_packages: OnceCell<Vec<(String, String)>>,
    /// Thư mục có thể là gốc chạy của một chương trình Python: gốc repo, và
    /// thư mục có `pyproject.toml`, `setup.py`, `setup.cfg`, `manage.py`. Đọc
    /// muộn, và một lượt quét cây dùng cho cả `py_roots` lẫn `extensions`.
    py_projects: OnceCell<Vec<String>>,
    /// Gốc import của Python, tra theo tên cấp cao nhất. Xem [`find_py_roots`].
    /// Đọc muộn như `npm_packages`.
    py_roots: OnceCell<PyRoots>,
    /// Extension mà `setup.py` khai — xem [`Extensions`].
    extensions: OnceCell<Extensions>,
    /// Thư mục include mà build của C/C++ khai báo. Đọc muộn: repo không có
    /// file C nào thì không phải quét tìm `Makefile` làm gì.
    c_includes: OnceCell<Vec<String>>,
    /// File C/C++ trong repo theo tên file, chỉ dựng khi build không khai thư
    /// mục include nào. Xem [`Resolver::c_unique`].
    c_files: OnceCell<HashMap<String, Vec<String>>>,
    /// File `.java` của repo, đọc muộn. Xem [`JavaIndex`].
    java: OnceCell<JavaIndex>,
    /// Package mà mỗi file Java đã hỏi khai ở câu `package`.
    java_packages: RefCell<HashMap<String, Option<String>>>,
    /// Tên thật trong những thư mục đã dò, để kiểm hoa thường. Xem [`exact_case`].
    listing: RefCell<Listing>,
    /// File gốc crate của từng gói Rust, theo thư mục gói. Đọc một lần.
    rust_roots: RefCell<HashMap<String, Rc<Vec<RootFile>>>>,
    /// Gốc crate của từng file đã hỏi. Một file có nhiều câu `use`.
    crate_of: RefCell<HashMap<String, Option<CrateRoot>>>,
    /// Tên mà một file gốc crate đưa vào phạm vi bằng `use`. Đọc một lần.
    root_bindings: RefCell<HashMap<String, Bindings>>,
}

/// `(tên, đường dẫn)` mà một file đưa vào phạm vi. Xem [`crate::scan::rust_bindings`].
type Bindings = Rc<Vec<(String, String)>>;

/// Một file gốc crate trong gói: `src/lib.rs`, `src/main.rs`, mỗi binary, mỗi
/// test, ví dụ, bench, và những đường dẫn khai trong `Cargo.toml`.
struct RootFile {
    file: String,
    /// Nơi các module con của nó nằm: thư mục chứa file gốc.
    dir: String,
    lib: bool,
}

/// Crate chứa một file Rust.
#[derive(Clone)]
struct CrateRoot {
    /// `None` khi gói không có file gốc nào ở chỗ quen thuộc: module vẫn tìm
    /// trong `dir` như trước, chỉ item ở gốc là không có đích.
    file: Option<String>,
    dir: String,
}

/// Re-export của re-export thì đi tiếp; `pub use crate::X` trỏ vào chính nó
/// thì dừng ở đây.
const MAX_REEXPORT_DEPTH: usize = 3;

/// Tên thật của các mục trong từng thư mục đã đọc. `None`: không đọc được.
pub(crate) type Listing = HashMap<PathBuf, Option<HashSet<OsString>>>;

/// `rel` có tồn tại với **đúng** cách viết hoa thường đó không.
///
/// `Path::is_file()` hỏi hệ thống file, và ổ mặc định của macOS (cả Windows)
/// không phân biệt hoa thường: `src/Error.rs` "có" khi trên đĩa chỉ có
/// `src/error.rs`. Linux thì không. Cùng một commit `rusqlite`, `init
/// --from-code` từng sinh 12 luật `allow` trên macOS và 11 trên CI Linux, vì
/// `use crate::{Error, Result}` được "tìm thấy" là file `src/Error.rs`. Luật
/// sinh ở máy này đem sang máy kia thành một luật không bao giờ áp được, hoặc
/// một vi phạm mà máy kia không thấy.
///
/// Mọi ngôn ngữ đã hỗ trợ đều phân biệt hoa thường khi import — Python kể cả
/// trên macOS — nên cách viết có thật trên đĩa là cách viết duy nhất được nhận.
/// Chỉ gọi sau khi `is_file`/`is_dir` đã trả "có": phần lớn lần dò trượt ngay
/// ở đó, không phải đọc thư mục nào.
pub(crate) fn exact_case(root: &Path, rel: &str, listing: &mut Listing) -> bool {
    let mut dir = root.to_path_buf();
    for part in rel.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                dir.pop();
                continue;
            }
            _ => {}
        }
        let names = listing.entry(dir.clone()).or_insert_with(|| {
            std::fs::read_dir(&dir)
                .ok()
                .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
        });
        if !names.as_ref().is_some_and(|n| n.contains(OsStr::new(part))) {
            return false;
        }
        dir.push(part);
    }
    true
}

/// Phần `paths` của một `tsconfig.json`, đã mở sẵn `extends`.
struct TsConfig {
    /// Thư mục chứa file cấu hình, tương đối so với gốc dự án.
    dir: String,
    /// Nơi các mẫu trong `paths` được tính từ đó.
    base: String,
    /// Mẫu đã tách quanh dấu `*`: tiền tố, hậu tố, danh sách đích.
    patterns: Vec<(String, String, Vec<String>)>,
}

impl Resolver {
    pub fn new(root: &Path) -> Self {
        Resolver {
            root: root.to_path_buf(),
            go_modules: OnceCell::new(),
            rust_crates: find_rust_crates(root),
            ts_configs: find_ts_configs(root),
            npm_packages: OnceCell::new(),
            py_projects: OnceCell::new(),
            py_roots: OnceCell::new(),
            extensions: OnceCell::new(),
            c_includes: OnceCell::new(),
            c_files: OnceCell::new(),
            java: OnceCell::new(),
            java_packages: RefCell::new(HashMap::new()),
            listing: RefCell::new(HashMap::new()),
            rust_roots: RefCell::new(HashMap::new()),
            crate_of: RefCell::new(HashMap::new()),
            root_bindings: RefCell::new(HashMap::new()),
        }
    }

    pub fn resolve(&self, lang: Lang, importer: &str, import: &Import) -> Resolved {
        let spec = import.spec.as_str();
        match lang {
            Lang::TypeScript => self.typescript(importer, spec),
            Lang::Python => self.python(importer, spec),
            Lang::C => self.c(importer, spec, import.system),
            Lang::Rust => self.rust(importer, spec),
            Lang::Go => self.go(spec),
            Lang::Java => self.java(importer, spec),
        }
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).is_file() && self.exact(rel)
    }

    fn dir_exists(&self, rel: &str) -> bool {
        self.root.join(rel).is_dir() && self.exact(rel)
    }

    fn exact(&self, rel: &str) -> bool {
        exact_case(&self.root, rel, &mut self.listing.borrow_mut())
    }

    fn first_file(&self, candidates: impl IntoIterator<Item = String>) -> Option<String> {
        candidates.into_iter().find(|c| self.exists(c))
    }

    // ---- TypeScript ----

    fn typescript(&self, importer: &str, spec: &str) -> Resolved {
        if is_relative(spec) {
            let Some(base) = join(dir_of(importer), spec) else {
                return Resolved::Missing;
            };
            let mut candidates = vec![base.clone()];
            for (from, tos) in JS_TO_TS {
                if let Some(stem) = base.strip_suffix(from) {
                    candidates.extend(tos.iter().map(|to| format!("{stem}{to}")));
                }
            }
            candidates.extend(TS_EXTENSIONS.iter().map(|e| format!("{base}.{e}")));
            candidates.extend(TS_EXTENSIONS.iter().map(|e| format!("{base}/index.{e}")));
            return self.first_file(candidates).map_or(Resolved::Missing, Resolved::Path);
        }
        if let Some(found) = self.ts_alias(importer, spec) {
            return found;
        }
        if let Some(found) = self.npm_workspace(spec) {
            return found;
        }
        // Gói có phạm vi luôn là `@scope/name`, nên phạm vi rỗng là alias —
        // tới đây nghĩa là không có `tsconfig.json` nào khai báo nó.
        if spec.starts_with("@/") || spec.starts_with("~/") || spec.starts_with('#') {
            return Resolved::Alias;
        }
        if spec.starts_with('/') {
            return Resolved::Alias;
        }
        Resolved::Package(npm_package(spec))
    }

    /// Gói anh em trong cùng workspace. Lấy tên gói dài nhất khớp, vì
    /// `@acme/core` và `@acme/core-utils` có thể cùng tồn tại.
    fn npm_workspace(&self, spec: &str) -> Option<Resolved> {
        let (name, dir) = self
            .npm_packages
            .get_or_init(|| find_npm_packages(&self.root))
            .iter()
            .filter(|(name, _)| spec == name || spec.starts_with(&format!("{name}/")))
            .max_by_key(|(name, _)| name.len())?;

        let subpath = spec[name.len()..].trim_start_matches('/');
        let base = if subpath.is_empty() { dir.clone() } else { format!("{dir}/{subpath}") };
        let mut candidates = vec![base.clone()];
        candidates.extend(TS_EXTENSIONS.iter().map(|e| format!("{base}.{e}")));
        candidates.extend(TS_EXTENSIONS.iter().map(|e| format!("{base}/index.{e}")));
        candidates.extend(TS_EXTENSIONS.iter().map(|e| format!("{base}/src/index.{e}")));
        // Không lần ra được file vào thì đích vẫn là gói đó; thư mục của nó đủ
        // để biết nó thuộc component nào.
        Some(Resolved::Path(self.first_file(candidates).unwrap_or_else(|| dir.clone())))
    }

    /// Thử các mẫu trong `tsconfig.json` gần file gọi nhất. `None` nghĩa là
    /// không mẫu nào khớp, để nơi gọi xét tiếp như gói thường.
    fn ts_alias(&self, importer: &str, spec: &str) -> Option<Resolved> {
        for config in self.ts_configs.iter().filter(|c| within(&c.dir, importer)) {
            for (prefix, suffix, targets) in &config.patterns {
                let Some(captured) = capture(spec, prefix, suffix) else { continue };
                for target in targets {
                    let filled = target.replace('*', captured);
                    let Some(base) = join(&config.base, &filled) else { continue };
                    let mut candidates = vec![base.clone()];
                    candidates.extend(TS_EXTENSIONS.iter().map(|e| format!("{base}.{e}")));
                    candidates
                        .extend(TS_EXTENSIONS.iter().map(|e| format!("{base}/index.{e}")));
                    if let Some(found) = self.first_file(candidates) {
                        return Some(Resolved::Path(found));
                    }
                }
                // Mẫu khớp nhưng không ra file: alias có thật, đích thì không.
                return Some(Resolved::Missing);
            }
        }
        None
    }

    // ---- Python ----

    /// Import tuyệt đối tính từ gốc nào thì tuỳ chương trình chạy từ đâu.
    /// Thử trước các package chứa chính file gọi, sâu nhất trước — `tests` của
    /// một package là `tests` của nó, không phải của package bên cạnh. Rồi mới
    /// tới các package còn lại, và ở đó tên phải là duy nhất: hai package cùng
    /// có `shared` thì đoán một cái là có thể sinh cạnh sai.
    fn python(&self, importer: &str, spec: &str) -> Resolved {
        if let Some(rest) = spec.strip_prefix('.') {
            return self.python_relative(importer, rest);
        }
        let path = spec.replace('.', "/");
        let top = spec.split('.').next().unwrap_or(spec);
        let roots = self.py_roots();
        // `os`, `numpy`: không gốc nào có tên này, khỏi hỏi hệ thống file.
        let Some(bases) = roots.get(top) else {
            return Resolved::Package(top.to_string());
        };
        let (mut own, others): (Vec<_>, Vec<_>) =
            bases.iter().partition(|(project, _)| within(project, importer));
        // Ổn định: trong cùng một package giữ thứ tự gốc, `src/`, `lib/`.
        own.sort_by_key(|(project, _)| std::cmp::Reverse(project.len()));

        if let Some(found) = self.python_in(own.iter().map(|(_, b)| b.as_str()), &path) {
            return Resolved::Path(found);
        }
        let mut found: Vec<String> =
            others.iter().filter_map(|(_, b)| self.python_in([b.as_str()], &path)).collect();
        found.dedup();
        if let [one] = found.as_slice() {
            return Resolved::Path(one.clone());
        }
        // Không tìm thấy trong dự án thì đây là gói bên thứ ba. Với import
        // tuyệt đối, hai khả năng đó không phân biệt được.
        Resolved::Package(top.to_string())
    }

    /// Những tên trong `from pkg import a, b` mà chính chúng là module con.
    ///
    /// Python nạp cả `pkg/a.py`, không riêng `pkg/__init__.py`, và module con
    /// có thể nằm ở component khác: `from app import core` từ `app/web`, hay
    /// `from google.cloud import exceptions` qua namespace PEP 420 chung của
    /// nhiều distribution. Chỉ phân giải tới `pkg` thì đích thật biến mất —
    /// trên 9 repo của bộ đo, 171 lần. Tên chỉ là hàm hay lớp thì không ra file
    /// nào, và không có gì để báo.
    ///
    /// `base` là kết quả phân giải của chính `import`. Gói ngoài hay đường dẫn
    /// không tồn tại thì không có module con nào trong dự án.
    pub fn submodules(
        &self,
        lang: Lang,
        importer: &str,
        import: &Import,
        base: &Resolved,
    ) -> Vec<String> {
        self.import_targets(lang, importer, import, base).0
    }

    /// Như [`Self::submodules`], cộng thêm những module mà import này nạp nhưng
    /// trong dự án **không có mã Python nào** cho nó: không có file, hoặc chỉ
    /// có stub `.pyi`. Đó là ứng viên module mở rộng — xem [`NativeImport`]
    /// và [`settle_natives`].
    ///
    /// Chỉ xét module nằm trong một package của chính dự án: `torch._C` là
    /// module mở rộng của PyTorch, không phải `_C` của repo này.
    ///
    /// Gộp chung một hàm vì cả hai cùng dò từng tên trong `from pkg import a,
    /// b`; dò hai lượt là gấp đôi phần tốn nhất.
    pub fn import_targets(
        &self,
        lang: Lang,
        importer: &str,
        import: &Import,
        base: &Resolved,
    ) -> (Vec<String>, Vec<NativeImport>) {
        if lang != Lang::Python {
            return (Vec::new(), Vec::new());
        }
        let mut natives: Vec<NativeImport> = Vec::new();
        self.native_candidate(importer, &import.spec, base, &mut natives);
        let Resolved::Path(base) = base else { return (Vec::new(), natives) };
        // Module con nằm đâu thì tuỳ `base` là gì. Gói thường (`__init__.py`)
        // chỉ có một thư mục — `__path__` của nó — nên module con nằm ngay
        // trong đó, không phải ở project bên cạnh có gói cùng tên. Module file
        // (`utils.py`) không có module con nào. Chỉ gói không gian tên — một
        // thư mục, không có `__init__` — mới trải qua nhiều distribution.
        let file = base.rsplit('/').next().unwrap_or(base);
        let within = if file.starts_with("__init__.") {
            let dir = dir_of(base);
            Some(if dir.is_empty() { String::new() } else { format!("{dir}/") })
        } else if PY_MODULE_SUFFIXES.iter().any(|s| !s.contains('/') && file.ends_with(s)) {
            return (Vec::new(), natives);
        } else {
            None
        };
        let mut out: Vec<String> = Vec::new();
        for name in &import.names {
            let sep = if import.spec.ends_with('.') { "" } else { "." };
            let spec = format!("{}{sep}{name}", import.spec);
            let found = self.python(importer, &spec);
            self.native_candidate(importer, &spec, &found, &mut natives);
            if let Resolved::Path(found) = found
                && &found != base
                && within.as_ref().is_none_or(|w| found.starts_with(w.as_str()))
                && !out.contains(&found)
            {
                out.push(found);
            }
        }
        (out, natives)
    }

    fn py_projects(&self) -> &[String] {
        self.py_projects.get_or_init(|| find_py_projects(&self.root))
    }

    fn py_roots(&self) -> &PyRoots {
        self.py_roots.get_or_init(|| find_py_roots(&self.root, self.py_projects()))
    }

    /// Extension mà các `setup.py` của dự án khai.
    pub fn extensions(&self) -> &Extensions {
        self.extensions.get_or_init(|| Extensions::read(&self.root, self.py_projects()))
    }

    /// Khoá ghép của phía native trong `file`, sau khi thay
    /// `TORCH_EXTENSION_NAME` bằng tên thật. `None` khi không biết chắc tên đó:
    /// cạnh đoán sai còn tệ hơn không có cạnh.
    pub fn bind_extension(&self, file: &str, key: &str) -> Option<String> {
        if !key.contains(crate::scan::foreign::EXTENSION_NAME) {
            return Some(key.to_string());
        }
        let name = self.extensions().for_source(file)?;
        Some(key.replacen(crate::scan::foreign::EXTENSION_NAME, name, 1))
    }

    /// Module `spec` phân giải ra `found` có thể là module mở rộng không: chỉ
    /// có stub, không tìm thấy, hoặc nằm trong một package của dự án mà
    /// package đó không có file nào cho nó.
    fn native_candidate(&self, importer: &str, spec: &str, found: &Resolved, out: &mut Vec<NativeImport>) {
        let (at, stub) = match found {
            Resolved::Path(p) if p.ends_with(".pyi") => {
                let at = p.strip_suffix("/__init__.pyi").or_else(|| p.strip_suffix(".pyi"));
                (at.map(str::to_string), true)
            }
            Resolved::Missing => match spec.strip_prefix('.') {
                Some(rest) => (relative_stem(importer, rest), false),
                None => (None, false),
            },
            // Tên mà `setup.py` khai là extension thì là module của dự án, kể
            // cả ở cấp cao nhất (`import amp_C` của apex) — thứ mà không có
            // khai báo đó thì không phân biệt được với gói bên ngoài.
            Resolved::Package(_) if self.extensions().declares(spec) => (Some(spec.replace('.', "/")), false),
            Resolved::Package(top) if spec.contains('.') && !spec.starts_with('.') => {
                (self.absolute_stem(importer, top, spec), false)
            }
            _ => (None, false),
        };
        let module = spec.rsplit('.').next().unwrap_or_default();
        if let Some(at) = at
            && !module.is_empty()
        {
            out.push(NativeImport { module: module.to_string(), shown: spec.to_string(), at, stub });
        }
    }

    /// Chỗ module tuyệt đối `spec` sẽ nằm nếu nó là file, khi gói cấp cao nhất
    /// `top` là của dự án: dưới gốc gần file gọi nhất có gói đó. `None` khi
    /// `top` là gói bên ngoài — `torch._C` không bao giờ là module của repo này.
    fn absolute_stem(&self, importer: &str, top: &str, spec: &str) -> Option<String> {
        let roots = self.py_roots();
        let bases = roots.get(top)?;
        let base = bases
            .iter()
            .filter(|(project, _)| within(project, importer))
            .max_by_key(|(project, _)| project.len())
            .or_else(|| bases.first())?;
        Some(format!("{}{}", base.1, spec.replace('.', "/")))
    }

    /// Module `path` dưới các thư mục gốc `bases`, theo thứ tự.
    fn python_in<'a>(&self, bases: impl IntoIterator<Item = &'a str>, path: &str) -> Option<String> {
        let bases: Vec<&str> =
            bases.into_iter().filter(|b| self.may_hold_module(&format!("{b}{path}"))).collect();
        let candidates =
            bases.iter().flat_map(|b| PY_MODULE_SUFFIXES.iter().map(move |s| format!("{b}{path}{s}")));
        if let Some(found) = self.first_file(candidates) {
            return Some(found);
        }
        // Gói kiểu không gian tên (PEP 420) không có `__init__.py`, và đó là
        // mặc định của Python hiện đại. Thư mục chính là gói.
        bases.iter().map(|b| format!("{b}{path}")).find(|c| self.dir_exists(c))
    }

    fn python_relative(&self, importer: &str, rest: &str) -> Resolved {
        let Some(prefix) = relative_stem(importer, rest) else { return Resolved::Missing };
        if !self.may_hold_module(&prefix) {
            return Resolved::Missing;
        }
        let candidates = PY_MODULE_SUFFIXES.iter().map(|s| format!("{prefix}{s}"));
        if let Some(found) = self.first_file(candidates) {
            return Resolved::Path(found);
        }
        // Gói kiểu không gian tên, hoặc `from . import x` trỏ thẳng vào thư
        // mục của chính file này. Cả hai đều là thư mục chứ không phải file.
        if self.dir_exists(&prefix) {
            return Resolved::Path(prefix);
        }
        Resolved::Missing
    }

    /// Module `stem` (đường dẫn không đuôi, tương đối với gốc) có thể tồn tại
    /// không, hỏi bằng bảng liệt kê thư mục cha.
    ///
    /// Một ứng viên chỉ đúng khi tên cuối của nó có trong thư mục cha —
    /// [`exact_case`] đòi đúng điều đó — nên hỏi bảng trước không đổi kết quả,
    /// chỉ đổi giá: mỗi thư mục đọc một lần thay vì vài lần `stat` cho từng
    /// ứng viên. Phần lớn lần dò là trượt — `from pkg import a, b, c` dò từng
    /// tên xem có phải module con không — và trên namespace PEP 420 trải qua
    /// hàng chục distribution, `check` của opentelemetry-python từng chậm gấp
    /// bốn vì chúng.
    fn may_hold_module(&self, stem: &str) -> bool {
        let (parent, last) = stem.rsplit_once('/').unwrap_or(("", stem));
        if last.is_empty() {
            return true;
        }
        let dir = if parent.is_empty() { self.root.clone() } else { self.root.join(parent) };
        let mut listing = self.listing.borrow_mut();
        let names = listing.entry(dir.clone()).or_insert_with(|| {
            std::fs::read_dir(&dir)
                .ok()
                .map(|entries| entries.flatten().map(|e| e.file_name()).collect())
        });
        let Some(names) = names else { return false };
        names.contains(OsStr::new(last))
            || PY_MODULE_SUFFIXES
                .iter()
                .filter(|s| !s.contains('/'))
                .any(|s| names.contains(OsStr::new(&format!("{last}{s}"))))
    }

    // ---- C và C++ ----

    fn c(&self, importer: &str, spec: &str, system: bool) -> Resolved {
        let mut candidates = Vec::new();
        // `#include "x.h"` tìm cạnh file gọi trước. `<x.h>` thì không.
        if !system
            && let Some(local) = join(dir_of(importer), spec)
        {
            candidates.push(local);
        }
        // Thư mục mà build khai báo. Đây là đường dẫn thật, nên nó áp dụng cho
        // cả hai dạng include: `<x.h>` tìm thấy ở đây là file của dự án, đúng
        // như trình biên dịch sẽ thấy.
        let declared = self.c_includes.get_or_init(|| crate::cbuild::include_dirs(&self.root));
        candidates.extend(declared.iter().filter_map(|d| join(d, spec)));
        if let Some(found) = self.first_file(candidates) {
            return Resolved::Path(found);
        }
        // Build không khai thư mục include nào mà ta đọc được — nginx đặt
        // `-I` trong script `auto/`. Không có sự thật thì đành dựa vào tên:
        // header trùng tên với **đúng một** file trong repo thì là file đó.
        // Có build thì không đoán: `<x.h>` ngoài các thư mục nó khai là của
        // hệ thống, dù repo có một file trùng tên.
        if declared.is_empty() {
            if !system
                && let Some(found) =
                    self.first_file(C_INCLUDE_DIRS.iter().filter_map(|d| join(d, spec)))
            {
                return Resolved::Path(found);
            }
            if !is_standard_header(spec)
                && let Some(found) = self.c_unique(importer, spec)
            {
                return Resolved::Path(found);
            }
        }
        if system {
            // Header hệ thống và thư viện bên ngoài. Khai báo bằng
            // `external LibC at "stdio.h"` nếu muốn đưa vào luật.
            return Resolved::Package(spec.to_string());
        }
        Resolved::Missing
    }

    /// File duy nhất trong repo có đuôi đường dẫn là `spec`, trong số những
    /// file `importer` thấy được (xem [`vendored_owner`]). Hai file khớp thì
    /// không chọn: một cạnh sai tốn lòng tin hơn một cạnh bị bỏ sót.
    ///
    /// Có bản vendor bị ẩn thì header thuộc một thư viện ngoài, và chỉ bản
    /// vendor của chính cây `importer` mới được chọn. Một stub trong `tests/`
    /// không vì bản kia bị ẩn mà thành "duy nhất".
    fn c_unique(&self, importer: &str, spec: &str) -> Option<String> {
        let spec = normalize(spec)?;
        let name = spec.rsplit('/').next()?;
        let files = self.c_files.get_or_init(|| find_c_files(&self.root));
        let suffix = format!("/{spec}");
        let mut hidden = false;
        let mut visible = Vec::new();
        for f in files.get(name)?.iter().filter(|f| **f == spec || f.ends_with(&suffix)) {
            // Chỉ xét phần đứng trước `spec`: `<external/api.h>` ghi rõ thư
            // mục, đó là chủ ý của người viết include.
            let owner = vendored_owner(&f[..f.len() - spec.len()]);
            match owner {
                Some(o) if !importer.starts_with(o) => hidden = true,
                _ => visible.push((f, owner.is_some())),
            }
        }
        match visible.as_slice() {
            [(one, vendored)] if !hidden || *vendored => Some((*one).clone()),
            _ => None,
        }
    }

    // ---- Rust ----

    /// Không bao giờ trả `Missing`: đường dẫn Rust hoặc ra một file, hoặc là
    /// một crate bên ngoài, hoặc nằm trong `mod` viết lồng. Không có trường
    /// hợp thứ tư — trình biên dịch đã lo.
    fn rust(&self, importer: &str, spec: &str) -> Resolved {
        self.rust_at(importer, spec, 0)
    }

    fn rust_at(&self, importer: &str, spec: &str, depth: usize) -> Resolved {
        // `split` luôn trả ít nhất một phần tử, kể cả với chuỗi rỗng.
        let mut parts: Vec<&str> = spec.split("::").collect();
        let root = self.crate_root(importer);

        let base = match parts[0] {
            "crate" => {
                parts.remove(0);
                // File dùng chung cho nhiều crate — `tests/common/mod.rs` — thì
                // không biết `crate::` là crate nào. Không đoán.
                match &root {
                    Some(r) => r.dir.clone(),
                    None => return Resolved::Internal,
                }
            }
            "self" => {
                parts.remove(0);
                self.module_dir(importer)
            }
            "super" => {
                let mut d = self.module_dir(importer);
                while parts.first() == Some(&"super") {
                    parts.remove(0);
                    d = match d.rsplit_once('/') {
                        Some((up, _)) => up.to_string(),
                        None if !d.is_empty() => String::new(),
                        None => return Resolved::Internal,
                    };
                }
                d
            }
            "std" | "core" | "alloc" => return Resolved::Package(parts[0].to_string()),
            // Crate anh em trong workspace, rồi tới edition 2015 cho phép
            // `use some_module::x` trỏ vào chính crate. Hết hai cách đó mới
            // coi là crate bên ngoài.
            other => {
                if let Some((_, src)) = self.rust_crates.iter().find(|(n, _)| n == other) {
                    let src = src.clone();
                    parts.remove(0);
                    return match self.rust_module(&src, &parts, None, depth) {
                        Some(found) => found,
                        // Crate anh em có thật nhưng không lần ra module: đích
                        // vẫn là crate đó, lấy chính thư mục src của nó.
                        None => Resolved::Path(src),
                    };
                }
                // Không dùng file gốc ở đây: `serde::X` không có module file
                // nào thì là crate ngoài, không phải một item ở gốc crate.
                let base = root.map_or_else(|| self.crate_src(importer), |r| r.dir);
                return self
                    .rust_module(&base, &parts, None, depth)
                    .unwrap_or_else(|| Resolved::Package(other.to_string()));
            }
        };
        // Không ra file nào nghĩa là `mod` viết lồng trong một file không phải
        // gốc crate — không có cạnh để vẽ.
        self.rust_module(&base, &parts, root.as_ref(), depth).unwrap_or(Resolved::Internal)
    }

    /// Đoạn cuối của `use` thường là tên item chứ không phải module, và không
    /// biết trước có bao nhiêu đoạn như vậy. Thử từ đường dẫn dài nhất xuống
    /// ngắn nhất, lấy cái đầu tiên ra file.
    ///
    /// Tới chính thư mục gốc crate thì module là file gốc — `lib.rs`, không
    /// phải `src.rs` hay `src/mod.rs` — và tên còn lại là một item ở gốc. Xem
    /// [`Resolver::root_item`].
    fn rust_module(
        &self,
        base: &str,
        parts: &[&str],
        root: Option<&CrateRoot>,
        depth: usize,
    ) -> Option<Resolved> {
        // `take == 0` là chính `base`: `use super::Import` không có đoạn module
        // nào sau `super`, nên đích là chính module cha.
        for take in (0..=parts.len()).rev() {
            let joined = parts[..take].join("/");
            let path = match (base.is_empty(), take) {
                (true, 0) => String::new(),
                (true, _) => joined,
                (false, 0) => base.to_string(),
                (false, _) => format!("{base}/{joined}"),
            };
            if take == 0
                && let Some(r) = root
                && r.dir == path
            {
                return r.file.as_deref().map(|f| self.root_item(f, parts.first().copied(), depth));
            }
            if path.is_empty() {
                continue;
            }
            if let Some(found) =
                self.first_file([format!("{path}.rs"), format!("{path}/mod.rs")])
            {
                return Some(Resolved::Path(found));
            }
        }
        None
    }

    /// `name` là một item ở gốc crate. Đích là nơi nó được định nghĩa: viết
    /// thẳng trong file gốc thì là file gốc; do file gốc nhập vào bằng `use` —
    /// `pub use error::Error` — thì là nơi `use` đó trỏ tới.
    ///
    /// Không nhìn xuyên thì file gốc thành chỗ rửa phụ thuộc: `deny Scan ->
    /// Model` lách được bằng `use crate::Document`, vì `Document` đi qua
    /// `lib.rs`. Ở tokio, 56 trên 67 câu `use crate::<item gốc>` là re-export.
    fn root_item(&self, root_file: &str, name: Option<&str>, depth: usize) -> Resolved {
        if let Some(name) = name
            && depth < MAX_REEXPORT_DEPTH
        {
            let bindings = self.bindings_of(root_file);
            if let Some((_, spec)) = bindings.iter().find(|(bound, _)| bound == name) {
                let found = self.rust_at(root_file, spec, depth + 1);
                if matches!(found, Resolved::Path(_) | Resolved::Package(_)) {
                    return found;
                }
            }
        }
        Resolved::Path(root_file.to_string())
    }

    fn bindings_of(&self, file: &str) -> Bindings {
        if let Some(found) = self.root_bindings.borrow().get(file) {
            return Rc::clone(found);
        }
        let text = crate::read_config(&self.root.join(file)).unwrap_or_default();
        let found = Rc::new(crate::scan::rust_bindings(crate::strip_bom(&text)));
        self.root_bindings.borrow_mut().insert(file.to_string(), Rc::clone(&found));
        found
    }

    /// Thư mục chứa module con của file này: `src/order.rs` và
    /// `src/order/mod.rs` đều là `src/order`. File gốc crate thì khác — module
    /// con của `src/lib.rs` nằm ở `src/`, không phải `src/lib/`.
    fn module_dir(&self, importer: &str) -> String {
        match self.crate_root(importer) {
            Some(CrateRoot { file: Some(f), dir }) if f == importer => dir,
            _ => module_dir(importer),
        }
    }

    /// Crate chứa `importer`.
    ///
    /// Mỗi file gốc — thư viện, từng binary, từng test tích hợp, từng ví dụ —
    /// là một crate riêng, và `crate::` trong đó trỏ về gốc của chính nó. Một
    /// file không phải gốc thuộc về gốc có thư mục là tiền tố dài nhất của nó.
    /// `lib.rs` và `main.rs` cùng nằm trong `src/` thì module thuộc thư viện.
    /// Nhiều gốc chung một thư mục mà không cái nào là thư viện — `tests/` có
    /// mười file test và một `tests/common/` dùng chung — thì không đoán.
    fn crate_root(&self, importer: &str) -> Option<CrateRoot> {
        if let Some(found) = self.crate_of.borrow().get(importer) {
            return found.clone();
        }
        let found = self.find_crate_root(importer);
        self.crate_of.borrow_mut().insert(importer.to_string(), found.clone());
        found
    }

    fn find_crate_root(&self, importer: &str) -> Option<CrateRoot> {
        let pkg = self.crate_src(importer);
        let pkg = pkg.strip_suffix("src").unwrap_or(&pkg).trim_end_matches('/').to_string();
        let at = |rel: &str| if pkg.is_empty() { rel.to_string() } else { format!("{pkg}/{rel}") };

        // `build.rs` là một crate, nhưng không sở hữu thư mục nào.
        if importer == at("build.rs") {
            return Some(CrateRoot { file: Some(importer.to_string()), dir: pkg });
        }
        let roots = self.package_roots(&pkg);
        if let Some(r) = roots.iter().find(|r| r.file == importer) {
            return Some(CrateRoot { file: Some(r.file.clone()), dir: r.dir.clone() });
        }
        let inside = |dir: &str| dir.is_empty() || importer.starts_with(&format!("{dir}/"));
        if let Some(longest) = roots.iter().filter(|r| inside(&r.dir)).map(|r| r.dir.len()).max() {
            let same: Vec<&RootFile> =
                roots.iter().filter(|r| inside(&r.dir) && r.dir.len() == longest).collect();
            let pick = match same.as_slice() {
                [one] => Some(*one),
                many => many.iter().find(|r| r.lib).copied(),
            };
            return pick.map(|r| CrateRoot { file: Some(r.file.clone()), dir: r.dir.clone() });
        }
        // Không có gốc nào ở chỗ quen thuộc: vẫn tìm module trong `src/`.
        let src = at("src");
        inside(&src).then_some(CrateRoot { file: None, dir: src })
    }

    /// Mọi file gốc crate của gói ở `pkg`, đã lọc còn những file có thật.
    fn package_roots(&self, pkg: &str) -> Rc<Vec<RootFile>> {
        if let Some(found) = self.rust_roots.borrow().get(pkg) {
            return Rc::clone(found);
        }
        let at = |rel: &str| if pkg.is_empty() { rel.to_string() } else { format!("{pkg}/{rel}") };
        let mut candidates: Vec<(String, bool)> = Vec::new();
        // Đường dẫn khai trong `Cargo.toml` trước — ripgrep đặt binary chính ở
        // `crates/core/main.rs`, và gói đó không có `src/` nào.
        for (kind, path) in cargo_targets(&self.root.join(at("Cargo.toml"))) {
            if let Some(file) = normalize(&at(&path)) {
                candidates.push((file, kind == "lib"));
            }
        }
        candidates.push((at("src/lib.rs"), true));
        candidates.push((at("src/main.rs"), false));
        for dir in ["src/bin", "tests", "examples", "benches"] {
            let Ok(entries) = std::fs::read_dir(self.root.join(at(dir))) else { continue };
            let mut names: Vec<(String, bool)> = entries
                .flatten()
                .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path().is_dir())))
                .collect();
            // Thứ tự của `read_dir` phụ thuộc hệ thống file.
            names.sort();
            for (name, is_dir) in names {
                if is_dir {
                    candidates.push((at(&format!("{dir}/{name}/main.rs")), false));
                } else if name.ends_with(".rs") {
                    candidates.push((at(&format!("{dir}/{name}")), false));
                }
            }
        }

        let mut roots: Vec<RootFile> = Vec::new();
        for (file, lib) in candidates {
            if roots.iter().any(|r| r.file == file) || !self.exists(&file) {
                continue;
            }
            roots.push(RootFile { dir: dir_of(&file).to_string(), file, lib });
        }
        let roots = Rc::new(roots);
        self.rust_roots.borrow_mut().insert(pkg.to_string(), Rc::clone(&roots));
        roots
    }

    /// Thư mục `src` của crate chứa file này. Đi ngược lên tìm `Cargo.toml` để
    /// workspace nhiều crate cũng đúng.
    fn crate_src(&self, importer: &str) -> String {
        let mut dir: Vec<&str> = dir_of(importer).split('/').filter(|s| !s.is_empty()).collect();
        loop {
            let here = dir.join("/");
            let manifest =
                if here.is_empty() { "Cargo.toml".to_string() } else { format!("{here}/Cargo.toml") };
            if self.exists(&manifest) {
                return if here.is_empty() { "src".into() } else { format!("{here}/src") };
            }
            if dir.pop().is_none() {
                return "src".into();
            }
        }
    }

    // ---- Go ----

    fn go(&self, spec: &str) -> Resolved {
        // Dài nhất trước, nên `x/api/v3` thắng `x` khi cả hai cùng khớp.
        for (module, dir) in self.go_modules.get_or_init(|| find_go_modules(&self.root)) {
            let rest = if spec == module {
                ""
            } else if let Some(rest) = spec.strip_prefix(&format!("{module}/")) {
                rest
            } else {
                continue;
            };
            let target = match (dir.is_empty(), rest.is_empty()) {
                (true, _) => rest.to_string(),
                (false, true) => dir.clone(),
                (false, false) => format!("{dir}/{rest}"),
            };
            // Import của Go trỏ tới cả package, nên đích là một thư mục.
            return if target.is_empty() || self.dir_exists(&target) {
                Resolved::Path(target)
            } else {
                Resolved::Missing
            };
        }
        Resolved::Package(spec.to_string())
    }
}

// ---- Java ----

impl Resolver {
    /// `a.b.C` (và `a.b.C.Inner`, `a.b.C.m` của import tĩnh) là file
    /// `a/b/C.java` khai `package a.b;`, dưới một gốc nguồn bất kỳ:
    /// `src/main/java` của Maven, Gradle, `src` của Bazel hay Ant. Không đoán
    /// gốc nguồn theo quy ước: đường dẫn phải kết thúc bằng `a/b/C.java`, và
    /// câu `package` trong file phải đúng `a.b` — `x/a/b/C.java` khai
    /// `package x.a.b;` không phải `a.b.C`. `a.b.*` là thư mục của package.
    ///
    /// Không có trong repo thì là gói ngoài (JDK, Maven). Có ở nhiều module
    /// thì `Missing`: classpath mới quyết lớp nào thắng, archctl không đoán.
    fn java(&self, importer: &str, spec: &str) -> Resolved {
        let body = spec.strip_suffix(".*").unwrap_or(spec);
        if self.java.get_or_init(|| JavaIndex::read(&self.root)).overflow {
            return Resolved::Missing;
        }
        match self.java_pick(importer, spec) {
            None => Resolved::Package(body.to_string()),
            Some(Pick::Internal) => Resolved::Internal,
            Some(Pick::One(path)) => Resolved::Path(path),
            Some(Pick::Many(_)) => Resolved::Missing,
            Some(Pick::None) => Resolved::Package(body.to_string()),
        }
    }

    fn java_pick(&self, importer: &str, spec: &str) -> Option<Pick> {
        let (body, wild) = match spec.strip_suffix(".*") {
            Some(body) => (body, true),
            None => (spec, false),
        };
        let segs: Vec<&str> = body.split('.').collect();
        if segs.iter().any(|s| s.is_empty()) {
            return None;
        }
        // Package là các đoạn đầu viết thường, như quy ước đặt tên của Java.
        let lower = segs.iter().take_while(|s| s.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')).count();
        let (package, class) = if lower < segs.len() {
            (&segs[..lower], Some(segs[lower]))
        } else if wild {
            (&segs[..], None)
        } else {
            // `import a.b.c;`: lớp tên viết thường.
            (&segs[..segs.len() - 1], segs.last().copied())
        };
        // Không import được lớp của package mặc định.
        if package.is_empty() {
            return None;
        }
        let dotted = package.join(".");
        let dir = package.join("/");
        let index = self.java.get_or_init(|| JavaIndex::read(&self.root));
        let found = match class {
            Some(class) => {
                let tail = format!("{dir}/{class}.java");
                let matched: Vec<(String, String)> = index
                    .by_name
                    .get(&format!("{class}.java"))
                    .into_iter()
                    .flatten()
                    .filter_map(|f| Some((f.clone(), source_root(f, &tail)?)))
                    .collect();
                let packages: Vec<Option<String>> = matched.iter().map(|(f, _)| self.java_package(f)).collect();
                // Đường dẫn khớp mà không đọc được câu `package`: không biết có
                // phải lớp này không, nên không coi là gói ngoài.
                let unknown = packages.iter().any(Option::is_none);
                let candidates: Vec<(String, String)> = matched
                    .into_iter()
                    .zip(packages)
                    .filter(|(_, p)| p.as_deref() == Some(dotted.as_str()))
                    .map(|(c, _)| c)
                    .collect();
                match pick_java(importer, candidates) {
                    Pick::None if unknown => Pick::Many(Vec::new()),
                    pick => pick,
                }
            }
            None => {
                let last = package[package.len() - 1];
                let candidates: Vec<(String, String)> = index
                    .by_dir
                    .get(last)
                    .into_iter()
                    .flatten()
                    .filter_map(|(d, sample)| Some((d.clone(), sample, source_root_dir(d, &dir)?)))
                    .filter(|(_, sample, _)| self.java_package(sample).as_deref() == Some(dotted.as_str()))
                    .map(|(_, sample, root)| (sample.clone(), root))
                    .collect();
                // Đích là một file của package (không phải thư mục), để luật
                // `at core/**/*.java` cũng khớp. Package có ở nhiều module thì
                // import này không tự thành cạnh: tên trần dùng qua nó là tham
                // chiếu, phân giải từng lớp (TM-71).
                match pick_java(importer, candidates) {
                    Pick::Many(_) => Pick::Internal,
                    pick => pick,
                }
            }
        };
        Some(found)
    }

    /// Các file mà `spec` có thể trỏ tới khi [`Resolver::java`] ra `Missing`
    /// vì có ở nhiều chỗ: nơi gọi xem chúng có cùng một component không
    /// (biến thể `src/debug`, `src/release` của Android).
    pub fn java_candidates(&self, importer: &str, spec: &str) -> Vec<String> {
        match self.java_pick(importer, spec) {
            Some(Pick::Many(c)) => c,
            _ => Vec::new(),
        }
    }

    /// Ứng viên đầu tiên của một tham chiếu Java ra file trong repo; tên đầy
    /// đủ (`a.b.Lớp`) ra gói ngoài thì cũng trả, để so với `external`.
    pub fn java_ref(&self, importer: &str, r: &crate::scan::Ref) -> Option<String> {
        // `Missing` (có ở nhiều module) cũng trả: nơi gọi báo nó như với
        // import, không bỏ im lặng; trừ khi chỉ mục chạm trần (mọi tên đều thế).
        if self.java.get_or_init(|| JavaIndex::read(&self.root)).overflow {
            return None;
        }
        for spec in &r.specs {
            match self.java(importer, spec) {
                Resolved::Path(_) | Resolved::Missing => return Some(spec.clone()),
                Resolved::Package(_) if r.qualified => return Some(spec.clone()),
                _ => {}
            }
        }
        None
    }

    /// Câu `package` của một file Java. Đọc một lần.
    fn java_package(&self, file: &str) -> Option<String> {
        if let Some(known) = self.java_packages.borrow().get(file) {
            return known.clone();
        }
        // Thường câu `package` ở đầu; license dài thì đọc tiếp tới trần cỡ file.
        let read = |max: u64| read_prefix(&self.root.join(file), max).and_then(|src| crate::scan::java_package(&Lang::Java.tokens(&crate::scan::source_view(file, &src))));
        let found = read(1 << 16).or_else(|| {
            let big = std::fs::metadata(self.root.join(file)).is_ok_and(|m| m.len() > 1 << 16);
            if big { read(crate::check::MAX_FILE_BYTES) } else { None }
        });
        self.java_packages.borrow_mut().insert(file.to_string(), found.clone());
        found
    }
}

enum Pick {
    One(String),
    /// Nhiều ứng viên còn lại sau khi lọc.
    Many(Vec<String>),
    /// `import p.*` của package có ở nhiều module: không tự thành cạnh.
    Internal,
    None,
}

/// Chọn một trong các file (hay thư mục) cùng tên đầy đủ, kèm gốc nguồn của
/// từng cái. Gốc test (`src/test/java`) thường không nằm trên classpath của
/// module khác, nên có bản ngoài test thì bỏ bản test của module khác; chỉ có
/// bản test (tiện ích test dùng chung, như `core/test/…/Asserts.java` của
/// guice) thì vẫn là nó. Còn nhiều hơn một thì ưu tiên gốc chứa file gọi; vẫn
/// nhiều thì không đoán.
fn pick_java(importer: &str, mut candidates: Vec<(String, String)>) -> Pick {
    let inside = |root: &str| root.is_empty() || importer.starts_with(&format!("{root}/"));
    let is_test = |root: &str| root.split('/').any(|p| p == "test" || p == "tests");
    if candidates.iter().any(|(_, root)| !is_test(root) || inside(root)) {
        candidates.retain(|(_, root)| !is_test(root) || inside(root));
    }
    candidates.sort();
    candidates.dedup();
    if candidates.len() > 1 {
        let own: Vec<_> = candidates.iter().filter(|(_, root)| inside(root)).cloned().collect();
        if own.len() == 1 {
            candidates = own;
        }
    }
    match candidates.len() {
        0 => Pick::None,
        1 => Pick::One(candidates.remove(0).0),
        _ => Pick::Many(candidates.into_iter().map(|(f, _)| f).collect()),
    }
}

/// Gốc nguồn của `file` nếu nó kết thúc đúng bằng `tail` (theo đoạn đường dẫn).
fn source_root(file: &str, tail: &str) -> Option<String> {
    if file == tail {
        return Some(String::new());
    }
    file.strip_suffix(tail)?.strip_suffix('/').map(str::to_string)
}

fn source_root_dir(dir: &str, tail: &str) -> Option<String> {
    source_root(dir, tail)
}

/// File `.java` của repo: theo tên file, và thư mục chứa file Java theo tên
/// thư mục (kèm một file mẫu để đọc câu `package`). Chạm trần thì rỗng và
/// `overflow`: dừng giữa chừng thì kết quả tuỳ thứ tự `read_dir`.
#[derive(Default)]
struct JavaIndex {
    by_name: HashMap<String, Vec<String>>,
    by_dir: HashMap<String, Vec<(String, String)>>,
    /// Chạm trần: không biết lớp nào có trong repo, nên mọi import là chưa
    /// phân giải được, không phải gói ngoài (TM-72).
    overflow: bool,
}

impl JavaIndex {
    /// Cùng danh sách file mà `check` quét (bỏ `target/`, `build/` khi
    /// `.gitignore` bỏ, mẫu của `.archignore`…): bản sao do build sinh ra
    /// không được biến một import thật thành "có ở nhiều module".
    fn read(root: &Path) -> Self {
        const MAX_FILES: usize = 200_000;
        let mut index = JavaIndex::default();
        let files = crate::check::source_files(root, &crate::read_ignores(root));
        let mut seen = 0;
        let mut samples: HashMap<String, String> = HashMap::new();
        for rel in files.into_iter().filter(|f| f.ends_with(".java")) {
            seen += 1;
            if seen > MAX_FILES {
                return JavaIndex { overflow: true, ..JavaIndex::default() };
            }
            let (dir, name) = match rel.rsplit_once('/') {
                Some((d, n)) => (d.to_string(), n.to_string()),
                None => (String::new(), rel.clone()),
            };
            let sample = samples.entry(dir).or_insert_with(|| rel.clone());
            if rel < *sample {
                *sample = rel.clone();
            }
            index.by_name.entry(name).or_default().push(rel);
        }
        let mut dirs: Vec<(String, String)> = samples.into_iter().collect();
        dirs.sort();
        for (dir, sample) in dirs {
            let last = dir.rsplit('/').next().unwrap_or("").to_string();
            index.by_dir.entry(last).or_default().push((dir, sample));
        }
        index
    }
}

/// Quét `Cargo.toml` để biết workspace có những crate nào. Chỉ cần tên gói,
/// nên đọc bằng cách dò dòng thay vì kéo thêm một thư viện TOML.
fn find_rust_crates(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    collect_manifests(root, "", 0, &mut out);
    out
}

fn collect_manifests(root: &Path, prefix: &str, depth: usize, out: &mut Vec<(String, String)>) {
    const MAX_DEPTH: usize = 4;
    const SKIP: &[&str] = &["target", "node_modules", ".git", ".arch", "vendor"];

    if depth > MAX_DEPTH {
        return;
    }
    let dir = if prefix.is_empty() { root.to_path_buf() } else { root.join(prefix) };
    if let Some(name) = package_name(&dir.join("Cargo.toml")) {
        let src = if prefix.is_empty() { "src".to_string() } else { format!("{prefix}/src") };
        if root.join(&src).is_dir() {
            // `use` viết tên crate với gạch dưới, Cargo.toml viết gạch nối.
            out.push((name.replace('-', "_"), src));
        }
    }
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        if SKIP.contains(&name.as_str()) || name.starts_with('.') {
            continue;
        }
        // `file_type` không theo symlink: `x -> /` không được kéo việc đọc ra ngoài repo.
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            let next = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            collect_manifests(root, &next, depth + 1, out);
        }
    }
}

/// `(loại, đường dẫn)` của mỗi target khai đường dẫn riêng trong `Cargo.toml`:
/// `[lib]`, `[[bin]]`, `[[test]]`, `[[example]]`, `[[bench]]`.
fn cargo_targets(manifest: &Path) -> Vec<(&'static str, String)> {
    let Some(text) = crate::read_config(manifest) else { return Vec::new() };
    let mut kind = None;
    let mut out = Vec::new();
    for line in crate::strip_bom(&text).lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            kind = match line {
                "[lib]" => Some("lib"),
                "[[bin]]" => Some("bin"),
                "[[test]]" => Some("test"),
                "[[example]]" => Some("example"),
                "[[bench]]" => Some("bench"),
                _ => None,
            };
            continue;
        }
        if let Some(kind) = kind
            && let Some(value) = line.strip_prefix("path")
            && let Some(value) = value.trim_start().strip_prefix('=')
        {
            let value = value.trim().trim_matches(|c| c == '"' || c == '\'');
            if !value.is_empty() {
                out.push((kind, value.to_string()));
            }
        }
    }
    out
}

fn package_name(manifest: &Path) -> Option<String> {
    let text = crate::read_config(manifest)?;
    let text = crate::strip_bom(&text);
    let mut in_package = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_package = line == "[package]";
            continue;
        }
        if in_package && let Some(value) = line.strip_prefix("name") {
            return value
                .trim_start()
                .strip_prefix('=')
                .map(|v| v.trim().trim_matches('"').to_string())
                .filter(|v| !v.is_empty());
        }
    }
    None
}

/// Phần giữa mà `*` bắt được. Mẫu không có `*` thì phải khớp đúng y nguyên.
fn capture<'a>(spec: &'a str, prefix: &str, suffix: &str) -> Option<&'a str> {
    if prefix.len() + suffix.len() > spec.len() {
        return None;
    }
    spec.strip_prefix(prefix)?.strip_suffix(suffix)
}

/// File có nằm trong phạm vi của thư mục cấu hình không.
fn within(dir: &str, path: &str) -> bool {
    dir.is_empty() || path.starts_with(&format!("{dir}/"))
}

/// Tên gói trong mọi `package.json` của dự án. Bỏ qua gói ở ngay gốc: tên đó
/// ứng với cả repo nên nó sẽ nuốt mọi import.
fn find_npm_packages(root: &Path) -> Vec<(String, String)> {
    let mut dirs = Vec::new();
    collect_dirs_with(root, "", 0, &["package.json"], &mut dirs);
    let mut out: Vec<(String, String)> = dirs
        .iter()
        .filter(|dir| !dir.is_empty())
        .filter_map(|dir| {
            let path = root.join(dir).join("package.json");
            let text = crate::read_config(&path)?;
            let name = jsonc::parse(&text)?.get("name")?.as_str()?.to_string();
            (!name.is_empty()).then_some((name, dir.clone()))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

fn find_ts_configs(root: &Path) -> Vec<TsConfig> {
    let mut dirs = Vec::new();
    collect_dirs_with(root, "", 0, &["tsconfig.json"], &mut dirs);
    let mut out: Vec<TsConfig> =
        dirs.iter().filter_map(|dir| read_ts_config(root, dir)).collect();
    // Gần nhất trước: thư mục sâu hơn thắng thư mục nông hơn.
    out.sort_by_key(|c| std::cmp::Reverse(c.dir.len()));
    out
}

fn read_ts_config(root: &Path, dir: &str) -> Option<TsConfig> {
    const MAX_EXTENDS: usize = 4;

    // `extends` tạo chuỗi cấu hình; con thắng cha. Đi từ gốc chuỗi xuống.
    let mut chain: Vec<jsonc::Value> = Vec::new();
    let mut current = dir.to_string();
    for _ in 0..MAX_EXTENDS {
        let path = if current.is_empty() {
            "tsconfig.json".to_string()
        } else {
            format!("{current}/tsconfig.json")
        };
        let Some(value) = crate::read_config(&root.join(&path)).and_then(|s| jsonc::parse(&s))
        else {
            break;
        };
        let parent = value.get("extends").and_then(jsonc::Value::as_str).map(str::to_string);
        chain.push(value);
        match parent.and_then(|p| join(&current, p.trim_end_matches(".json"))) {
            Some(next) => current = next.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_string()),
            None => break,
        }
    }

    let mut base = dir.to_string();
    let mut patterns = Vec::new();
    for value in chain.iter().rev() {
        if let Some(url) = value.get_path(&["compilerOptions", "baseUrl"]).and_then(jsonc::Value::as_str)
            && let Some(joined) = join(dir, url)
        {
            base = joined;
        }
        if let Some(fields) =
            value.get_path(&["compilerOptions", "paths"]).and_then(jsonc::Value::as_object)
        {
            patterns.clear();
            for (pattern, targets) in fields {
                let (prefix, suffix) = match pattern.split_once('*') {
                    Some((p, s)) => (p.to_string(), s.to_string()),
                    None => (pattern.clone(), String::new()),
                };
                let targets: Vec<String> = targets
                    .as_array()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_string))
                    .collect();
                if !targets.is_empty() {
                    patterns.push((prefix, suffix, targets));
                }
            }
        }
    }
    (!patterns.is_empty()).then_some(TsConfig { dir: dir.to_string(), base, patterns })
}

/// Thư mục chứa ít nhất một file có tên trong `names`, quét tới độ sâu giới
/// hạn. Nhiều tên trong một lượt: đường hook `check <file>` trả cho mỗi lượt
/// quét cây, và bốn lượt cho bốn dấu hiệu của Python từng làm nó chậm gấp ba.
fn collect_dirs_with(root: &Path, prefix: &str, depth: usize, names: &[&str], out: &mut Vec<String>) {
    const MAX_DEPTH: usize = 4;
    const SKIP: &[&str] = &["target", "node_modules", ".git", ".arch", "vendor", "dist"];

    if depth > MAX_DEPTH {
        return;
    }
    let dir = if prefix.is_empty() { root.to_path_buf() } else { root.join(prefix) };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    let mut marked = false;
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let Ok(child) = entry.file_name().into_string() else { continue };
        // `file_type` không theo symlink và không tốn thêm một lần `stat`.
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_file() && names.contains(&child.as_str()) {
            marked = true;
        } else if kind.is_dir() && !SKIP.contains(&child.as_str()) && !child.starts_with('.') {
            children.push(child);
        }
    }
    if marked {
        out.push(prefix.to_string());
    }
    for child in children {
        let next = if prefix.is_empty() { child } else { format!("{prefix}/{child}") };
        collect_dirs_with(root, &next, depth + 1, names, out);
    }
}

/// Đường dẫn không đuôi mà import tương đối `from .<rest>` trỏ tới, tính từ
/// file gọi. `None` khi số dấu chấm đi lên quá gốc repo.
fn relative_stem(importer: &str, rest: &str) -> Option<String> {
    let extra_dots = rest.len() - rest.trim_start_matches('.').len();
    let module = rest.trim_start_matches('.');
    let mut base: Vec<&str> = dir_of(importer).split('/').filter(|s| !s.is_empty()).collect();
    for _ in 0..extra_dots {
        base.pop()?;
    }
    let mut prefix = base.join("/");
    if !module.is_empty() {
        let tail = module.replace('.', "/");
        prefix = if prefix.is_empty() { tail } else { format!("{prefix}/{tail}") };
    }
    Some(prefix)
}

/// Tên cấp cao nhất → `(package, thư mục gốc)` có tên đó, theo thứ tự
/// package rồi gốc, `src/`, `lib/` của nó.
type PyRoots = HashMap<String, Vec<(String, String)>>;

/// Thư mục mà một chương trình Python trong repo có thể chạy từ đó: gốc repo,
/// và mọi thư mục đánh dấu một package (`pyproject.toml`, `setup.py`,
/// `setup.cfg`) hay một dự án Django (`manage.py`).
///
/// Đọc mỗi thư mục gốc một lần rồi lập bảng theo tên: monorepo có hàng chục
/// gốc, mỗi file hàng chục import, và phần lớn import là thư viện ngoài —
/// hỏi hệ thống file cho từng cặp thì `check` của langchain chậm gấp năm.
fn find_py_projects(root: &Path) -> Vec<String> {
    let mut projects = vec![String::new()];
    let markers = ["pyproject.toml", "setup.py", "setup.cfg", "manage.py"];
    collect_dirs_with(root, "", 0, &markers, &mut projects);
    projects.sort();
    projects.dedup();
    projects
}

fn find_py_roots(root: &Path, projects: &[String]) -> PyRoots {
    let mut out: PyRoots = HashMap::new();
    for project in projects {
        let p = if project.is_empty() { String::new() } else { format!("{project}/") };
        let mut bases = vec![p.clone(), format!("{p}src/"), format!("{p}lib/")];
        let declared = read_bounded(&root.join(format!("{p}pyproject.toml")), 1 << 20)
            .map(|text| pyproject_bases(&text))
            .unwrap_or_default();
        for d in declared {
            let base = format!("{p}{d}/");
            if !bases.contains(&base) {
                bases.push(base);
            }
        }
        for base in bases {
            let Ok(entries) = std::fs::read_dir(root.join(&base)) else { continue };
            let mut names: Vec<String> = entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().into_string().ok()?;
                    let stem = PY_MODULE_SUFFIXES
                        .iter()
                        .filter(|s| !s.contains('/'))
                        .find_map(|s| name.strip_suffix(s))
                        .map(str::to_string);
                    stem.or_else(|| e.file_type().ok()?.is_dir().then_some(name))
                })
                .collect();
            names.sort();
            names.dedup();
            for name in names {
                out.entry(name).or_default().push((project.clone(), base.clone()));
            }
        }
    }
    out
}

/// Thư mục chứa gói mà `pyproject.toml` khai, tương đối với project: mã không
/// nằm ở gốc, `src/` hay `lib/` thì cấu hình build nói nó ở đâu.
///
/// - hatch: `[tool.hatch.build.targets.wheel]` `sources = ["backend"]`, hay
///   `packages = ["backend/pkg"]` (thư mục cha của gói);
/// - setuptools: `[tool.setuptools.packages.find]` `where = ["backend"]`, và
///   `package-dir = {"" = "backend"}`;
/// - poetry: `packages = [{ include = "pkg", from = "backend" }]`.
///
/// Đọc theo dòng, đủ cho cách viết thường gặp; không phải bộ đọc TOML. Đường
/// dẫn ra khỏi project hay tuyệt đối thì bỏ.
fn pyproject_bases(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut section = String::new();
    let strings = |s: &str| -> Vec<String> {
        let mut v = Vec::new();
        let mut rest = s;
        while let Some(a) = rest.find(['"', '\'']) {
            let q = rest.as_bytes()[a] as char;
            let Some(b) = rest[a + 1..].find(q) else { break };
            v.push(rest[a + 1..a + 1 + b].to_string());
            rest = &rest[a + 1 + b + 1..];
        }
        v
    };
    for raw in text.lines() {
        let line = raw.split(" #").next().unwrap_or(raw).trim();
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = name.trim_matches(['[', ']']).trim().to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let key = key.trim();
        let found: Vec<String> = match (section.as_str(), key) {
            ("tool.hatch.build.targets.wheel" | "tool.hatch.build", "sources") => {
                // `sources = ["backend"]`, hay bảng `{"src/x" = "x"}`: lấy vế trái.
                let items = strings(value);
                if value.trim_start().starts_with('{') { items.into_iter().step_by(2).collect() } else { items }
            }
            ("tool.hatch.build.targets.wheel", "packages") => {
                strings(value).iter().filter_map(|p| p.rsplit_once('/').map(|(d, _)| d.to_string())).collect()
            }
            ("tool.setuptools.packages.find", "where") => strings(value),
            ("tool.setuptools", "package-dir") | ("tool.setuptools.package-dir", "\"\"") => {
                let items = strings(value);
                match key {
                    "package-dir" => items.chunks(2).filter(|c| c[0].is_empty()).filter_map(|c| c.get(1).cloned()).collect(),
                    _ => items,
                }
            }
            ("tool.poetry", "packages") => {
                let mut v = Vec::new();
                let mut rest = value;
                while let Some(at) = rest.find("from") {
                    let tail = &rest[at + 4..];
                    if let Some(s) = tail.trim_start().strip_prefix('=').and_then(|t| strings(t).into_iter().next()) {
                        v.push(s);
                    }
                    rest = tail;
                }
                v
            }
            _ => continue,
        };
        for d in found {
            // Kiểm đường dẫn tuyệt đối trước khi bỏ `/` cuối: `/etc` không
            // được thành `etc`.
            let absolute = d.starts_with('/');
            let d = d.trim_end_matches('/').to_string();
            let safe = !absolute
                && !d.is_empty()
                && !d.contains('\\')
                && d.split('/').all(|seg| !seg.is_empty() && seg != "." && seg != "..");
            if safe && !out.contains(&d) {
                out.push(d);
            }
        }
    }
    out
}

/// Extension Python mà `setup.py` khai: `CppExtension("pkg._C", ["a.cpp"])`,
/// `CUDAExtension(name="amp_C", sources=[…])`, cả qua biến (`extension =
/// CUDAExtension` rồi `extension("pkg._C", sources)`).
///
/// Đó là cách duy nhất biết `TORCH_EXTENSION_NAME` mang tên gì: khi biên dịch
/// từng file nguồn, `torch.utils.cpp_extension` đặt nó bằng đoạn cuối của tên
/// extension chứa file đó. Tên đặt qua biến (`name=ext_name`) thì không đọc.
#[derive(Debug, Default)]
pub struct Extensions {
    /// Tên đầy đủ → file nguồn (tương đối với gốc repo) khi danh sách nguồn
    /// viết thẳng; `None` khi nó tính lúc chạy (`glob`, biến).
    declared: HashMap<String, Option<Vec<String>>>,
    /// File nguồn → tên đầy đủ của extension liệt kê nó. Chuỗi rỗng là file
    /// có trong hai extension: không biết mang tên nào.
    by_source: HashMap<String, String>,
}

impl Extensions {
    fn read(root: &Path, projects: &[String]) -> Self {
        // `setup.py` thật vài chục KB. Trần mỗi file và trần tổng: đọc nó nằm
        // trên đường hook, và số file theo số project, không bị chặn.
        const MAX_SETUP_BYTES: u64 = 1 << 20;
        const MAX_TOTAL_BYTES: u64 = 8 << 20;
        let mut out = Extensions::default();
        let mut total = 0u64;
        for project in projects {
            let dir = if project.is_empty() { String::new() } else { format!("{project}/") };
            let Some(src) = read_bounded(&root.join(format!("{dir}setup.py")), MAX_SETUP_BYTES) else {
                continue;
            };
            total += src.len() as u64;
            if total > MAX_TOTAL_BYTES {
                break;
            }
            for (name, sources) in parse_setup(&Lang::Python.tokens(&src)) {
                let sources = sources.map(|s| {
                    s.iter().map(|f| format!("{dir}{}", f.trim_start_matches("./"))).collect::<Vec<_>>()
                });
                match out.declared.entry(name) {
                    // Cùng một tên khai ở hai nhánh `if`: gộp nguồn, và một
                    // bên không biết nguồn thì cả hai không biết.
                    std::collections::hash_map::Entry::Occupied(mut e) => match (e.get_mut(), sources) {
                        (Some(known), Some(more)) => known.extend(more),
                        (known, _) => *known = None,
                    },
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(sources);
                    }
                }
            }
        }
        for (name, sources) in &out.declared {
            for file in sources.iter().flatten() {
                out.by_source
                    .entry(file.clone())
                    .and_modify(|owner| {
                        if owner != name {
                            owner.clear();
                        }
                    })
                    .or_insert_with(|| name.clone());
            }
        }
        out
    }

    /// `full` (`amp_C`, `pkg._C`) có phải tên một extension của dự án không.
    pub fn declares(&self, full: &str) -> bool {
        self.declared.contains_key(full)
    }

    /// Tên mà `TORCH_EXTENSION_NAME` mang khi biên dịch `file`: đoạn cuối của
    /// tên extension có `file` trong danh sách nguồn. Không có thì chỉ khi dự
    /// án khai đúng một extension và nguồn của nó tính lúc chạy. Nhiều
    /// extension mà không biết file thuộc cái nào thì không đoán.
    pub fn for_source(&self, file: &str) -> Option<&str> {
        let full = match self.by_source.get(file) {
            Some(owner) if owner.is_empty() => return None,
            Some(owner) => owner.as_str(),
            None => match self.declared.iter().next() {
                Some((name, None)) if self.declared.len() == 1 => name.as_str(),
                _ => return None,
            },
        };
        Some(full.rsplit('.').next().unwrap_or(full))
    }
}

/// Nội dung một file thường, nếu nó không quá `max` byte. Không theo được tới
/// thiết bị hay FIFO: `setup.py -> /dev/zero` có kích thước 0 theo metadata
/// nhưng đọc mãi không hết, và git commit được symlink.
fn read_bounded(path: &Path, max: u64) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let mut src = String::new();
    file.take(max + 1).read_to_string(&mut src).ok()?;
    (src.len() as u64 <= max).then_some(src)
}

/// Phần đầu của một file thường (không theo symlink ở bước liệt kê), tối đa
/// `max` byte; byte lạ hay ký tự bị cắt đôi ở cuối thành ký tự thay thế.
fn read_prefix(path: &Path, max: u64) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(max).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// Các lời gọi dựng extension trong một `setup.py`: `(tên, nguồn nếu viết thẳng)`.
///
/// Tuyến tính theo số token, kể cả với `E(E(E(…)))` lồng sâu: `setup.py` là
/// input không tin được, và đọc nó nằm trên đường hook.
fn parse_setup(toks: &[Token]) -> Vec<(String, Option<Vec<String>>)> {
    let is_ext = |w: &str| w.rsplit('.').next().is_some_and(|last| last.ends_with("Extension"));
    let closing = matching_brackets(toks);
    // `extension = CUDAExtension`
    let aliases: HashSet<&str> = toks
        .windows(3)
        .filter_map(|w| match (&w[0].tok, &w[1].tok, &w[2].tok) {
            (Tok::Word(a), Tok::Punct('='), Tok::Word(b)) if is_ext(b) => Some(a.as_str()),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    for i in 0..toks.len() {
        let Tok::Word(callee) = &toks[i].tok else { continue };
        if !(is_ext(callee) || aliases.contains(&callee.as_str()))
            || toks.get(i + 1).map(|t| &t.tok) != Some(&Tok::Punct('('))
            // `class CMakeExtension(Extension):` là định nghĩa, không phải lời gọi.
            || i.checked_sub(1).is_some_and(|k| matches!(&toks[k].tok, Tok::Word(w) if w == "class" || w == "def"))
        {
            continue;
        }
        let (mut name, mut sources, mut position) = (None, None, 0);
        for arg in call_args(toks, &closing, i + 1) {
            match arg {
                [Token { tok: Tok::Word(key), .. }, Token { tok: Tok::Punct('='), .. }, value @ ..] => {
                    match key.as_str() {
                        "name" => name = single_str(value),
                        "sources" => sources = str_list(value),
                        _ => {}
                    }
                }
                _ => {
                    match position {
                        0 => name = name.or_else(|| single_str(arg)),
                        1 => sources = sources.or_else(|| str_list(arg)),
                        _ => {}
                    }
                    position += 1;
                }
            }
        }
        if let Some(name) = name {
            out.push((name, sources));
        }
    }
    out
}

/// Với mỗi dấu mở ngoặc, vị trí dấu đóng của nó; `usize::MAX` khi không đóng.
/// Một lượt, bằng stack.
fn matching_brackets(toks: &[Token]) -> Vec<usize> {
    let mut out = vec![usize::MAX; toks.len()];
    let mut open: Vec<usize> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        match t.tok {
            Tok::Punct('(' | '[' | '{') => open.push(i),
            Tok::Punct(')' | ']' | '}') => {
                if let Some(o) = open.pop() {
                    out[o] = i;
                }
            }
            _ => {}
        }
    }
    out
}

/// Đối số cấp ngoài cùng của lời gọi có `(` ở `open`, cắt theo dấu phẩy. Nhảy
/// qua cả nhóm ngoặc lồng bên trong thay vì đi từng token, nên tổng công của
/// mọi lời gọi là tuyến tính. Lời gọi không đóng thì không có đối số nào.
fn call_args<'a>(toks: &'a [Token], closing: &[usize], open: usize) -> Vec<&'a [Token]> {
    let close = closing[open];
    if close == usize::MAX {
        return Vec::new();
    }
    let mut out = Vec::new();
    let (mut i, mut start) = (open + 1, open + 1);
    while i < close {
        match toks[i].tok {
            Tok::Punct('(' | '[' | '{') if closing[i] != usize::MAX => i = closing[i] + 1,
            Tok::Punct(',') => {
                out.push(&toks[start..i]);
                i += 1;
                start = i;
            }
            _ => i += 1,
        }
    }
    if start < close {
        out.push(&toks[start..close]);
    }
    out
}

fn single_str(arg: &[Token]) -> Option<String> {
    match arg {
        [Token { tok: Tok::Str(s), .. }] => Some(s.clone()),
        _ => None,
    }
}

/// `["a.cpp", "b.cu"]`, chỉ khi mọi phần tử là chuỗi viết thẳng.
fn str_list(arg: &[Token]) -> Option<Vec<String>> {
    let [Token { tok: Tok::Punct('['), .. }, inner @ .., Token { tok: Tok::Punct(']'), .. }] = arg else {
        return None;
    };
    let mut out = Vec::new();
    for (k, t) in inner.iter().enumerate() {
        match (&t.tok, k % 2) {
            (Tok::Str(s), 0) => out.push(s.clone()),
            (Tok::Punct(','), 1) => {}
            _ => return None,
        }
    }
    Some(out)
}

/// Thư mục chứa mã vendor của bên thứ ba.
const VENDOR_DIRS: &[&str] = &["deps", "third_party", "thirdparty", "3rdparty", "external"];

/// Phần đường dẫn đứng trước thư mục vendor đầu tiên, kèm `/` cuối: cây sở
/// hữu bản vendor đó. `None` nếu `path` không nằm trong thư mục vendor nào.
///
/// Header vendor chỉ thuộc về cây chứa nó. glfw đặt mỗi phiên bản ở một thư
/// mục; v3.2 vendor `deps/vulkan/vulkan.h`, v3.3 thì dùng bản của hệ thống.
/// Không có giới hạn này thì `<vulkan/vulkan.h>` của v3.3 khớp tên duy nhất
/// vào bản của v3.2. `deps/` ở gốc như redis có cây sở hữu là cả repo.
fn vendored_owner(path: &str) -> Option<&str> {
    let mut at = 0;
    for part in path.split('/') {
        if VENDOR_DIRS.contains(&part) {
            return Some(&path[..at]);
        }
        at += part.len() + 1;
    }
    None
}

/// File C/C++ trong repo, gom theo tên file. Chạm giới hạn thì trả bảng rỗng,
/// tức là không đoán: dừng giữa chừng thì bảng tuỳ thứ tự `read_dir`, và
/// "duy nhất" trên máy này có thể là "hai file" trên máy kia.
fn find_c_files(root: &Path) -> HashMap<String, Vec<String>> {
    const MAX_FILES: usize = 200_000;
    const SKIP: &[&str] = &["target", "node_modules", ".git", ".arch", "dist"];
    const EXTS: &[&str] =
        &["h", "hh", "hpp", "hxx", "inl", "inc", "c", "cc", "cpp", "cxx", "def", "ipp", "tcc", "cu", "cuh"];

    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    let mut stack = vec![String::new()];
    let mut seen = 0;
    while let Some(prefix) = stack.pop() {
        let dir = if prefix.is_empty() { root.to_path_buf() } else { root.join(&prefix) };
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else { continue };
            if name.starts_with('.') || SKIP.contains(&name.as_str()) {
                continue;
            }
            let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                stack.push(rel);
            } else if kind.is_file()
                && name.rsplit_once('.').is_some_and(|(_, ext)| EXTS.contains(&ext))
            {
                out.entry(name).or_default().push(rel);
                seen += 1;
                if seen >= MAX_FILES {
                    return HashMap::new();
                }
            }
        }
    }
    out
}

/// Mọi `go.mod` trong repo. Bỏ những cái nằm trong `testdata`: công cụ Go bỏ
/// qua thư mục đó, nên module khai trong ấy không phải module thật.
fn find_go_modules(root: &Path) -> Vec<(String, String)> {
    let mut dirs = Vec::new();
    collect_dirs_with(root, "", 0, &["go.mod"], &mut dirs);
    let mut out: Vec<(String, String)> = dirs
        .into_iter()
        .filter(|dir| !dir.split('/').any(|part| part == "testdata"))
        .filter_map(|dir| {
            let text = crate::read_config(&root.join(&dir).join("go.mod"))?;
            let module = crate::strip_bom(&text)
                .lines()
                .map(str::trim)
                .find_map(|l| l.strip_prefix("module "))?
                .trim()
                .trim_matches('"')
                .to_string();
            (!module.is_empty()).then_some((module, dir))
        })
        .collect();
    out.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.cmp(b)));
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

fn is_relative(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".."
}

pub(crate) fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// Thư mục mà module của file này sở hữu: `a/b.rs` và `a/b/mod.rs` đều là
/// `a/b`.
fn module_dir(importer: &str) -> String {
    match importer.strip_suffix("/mod.rs") {
        Some(dir) => dir.to_string(),
        None => importer.strip_suffix(".rs").unwrap_or(importer).to_string(),
    }
}

pub(crate) fn join(dir: &str, spec: &str) -> Option<String> {
    let joined = if dir.is_empty() { spec.to_string() } else { format!("{dir}/{spec}") };
    normalize(&joined)
}

/// Gộp `.` và `..`. Trả về `None` nếu vượt ra ngoài gốc dự án.
pub(crate) fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    Some(out.join("/"))
}

/// `@scope/name/sub` → `@scope/name`; `pkg/sub` → `pkg`.
fn npm_package(spec: &str) -> String {
    let mut parts = spec.split('/');
    let first = parts.next().unwrap_or(spec);
    match (first.starts_with('@'), parts.next()) {
        (true, Some(second)) => format!("{first}/{second}"),
        _ => first.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goc_goi_khai_trong_pyproject() {
        let hatch = "[project]\nname = \"x\"\n[tool.hatch.build.targets.wheel]\nsources = [\"backend\"]\n";
        assert_eq!(pyproject_bases(hatch), ["backend"]);
        let hatch_pkgs = "[tool.hatch.build.targets.wheel]\npackages = [\"python/pkg\", \"python/other\"]\n";
        assert_eq!(pyproject_bases(hatch_pkgs), ["python"]);
        let find = "[tool.setuptools.packages.find]\nwhere = [\"py\"]  # comment\n";
        assert_eq!(pyproject_bases(find), ["py"]);
        let dir = "[tool.setuptools]\npackage-dir = {\"\" = \"code\"}\n";
        assert_eq!(pyproject_bases(dir), ["code"]);
        let poetry = "[tool.poetry]\npackages = [{ include = \"app\", from = \"service\" }]\n";
        assert_eq!(pyproject_bases(poetry), ["service"]);
        let escape = "[tool.hatch.build.targets.wheel]\nsources = [\"../outside\", \"/etc\", \"ok/./x\"]\n";
        assert!(pyproject_bases(escape).is_empty(), "đường dẫn ra ngoài project bị bỏ");
        assert!(pyproject_bases("[tool.other]\nsources = [\"x\"]\n").is_empty());
    }

    #[test]
    fn goi_npm_thuong_va_co_pham_vi() {
        assert_eq!(npm_package("stripe/lib/webhook"), "stripe");
        assert_eq!(npm_package("@scope/pkg/sub"), "@scope/pkg");
    }

    #[test]
    fn gop_dau_cham() {
        assert_eq!(normalize("src/a/../b/./c").as_deref(), Some("src/b/c"));
        assert_eq!(normalize("src/../.."), None);
    }

    #[test]
    fn thu_muc_module_cua_rust() {
        assert_eq!(module_dir("src/order/mod.rs"), "src/order");
        assert_eq!(module_dir("src/order/create.rs"), "src/order/create");
    }
}
