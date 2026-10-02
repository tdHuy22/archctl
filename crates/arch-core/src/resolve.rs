//! Phân giải chuỗi import thành đường dẫn trong dự án hoặc tên gói.
//!
//! Đây là chỗ dễ sai nhất của cả bộ kiểm chứng, và khác nhau nhiều giữa các
//! ngôn ngữ. Nguyên tắc chung: thứ nào không phân giải được thì **báo ra**,
//! không im lặng bỏ qua. Bỏ sót âm thầm làm người dùng tin rằng không có vi
//! phạm, tệ hơn hẳn việc nói thẳng là chưa biết.

use crate::scan::{Import, Lang};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// File hoặc thư mục trong dự án, tương đối so với gốc, dùng dấu `/`.
    /// Go trỏ tới thư mục vì import của Go là cả package.
    Path(String),
    /// Thư viện bên ngoài. Chỉ tham gia luật khi được khai báo `external`.
    Package(String),
    /// Trông như alias cần cấu hình build mà ta chưa đọc.
    Alias,
    /// Đường dẫn tương đối nhưng không tìm thấy gì ở đó.
    Missing,
}

const TS_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "d.ts"];
const JS_TO_TS: &[(&str, &[&str])] =
    &[(".js", &[".ts", ".tsx"]), (".jsx", &[".tsx"]), (".mjs", &[".mts"]), (".cjs", &[".cts"])];
/// Nơi thường đặt header khi `#include "..."` không nằm cạnh file gọi.
const C_INCLUDE_DIRS: &[&str] = &["", "include", "src", "lib"];

/// Giữ những thứ đọc một lần từ đĩa, để không mở lại `go.mod` cho từng import.
pub struct Resolver {
    root: PathBuf,
    go_module: Option<String>,
}

impl Resolver {
    pub fn new(root: &Path) -> Self {
        Resolver { root: root.to_path_buf(), go_module: read_go_module(root) }
    }

    pub fn resolve(&self, lang: Lang, importer: &str, import: &Import) -> Resolved {
        let spec = import.spec.as_str();
        match lang {
            Lang::TypeScript => self.typescript(importer, spec),
            Lang::Python => self.python(importer, spec),
            Lang::C => self.c(importer, spec, import.system),
            Lang::Rust => self.rust(importer, spec),
            Lang::Go => self.go(spec),
        }
    }

    fn exists(&self, rel: &str) -> bool {
        self.root.join(rel).is_file()
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
        // Gói có phạm vi luôn là `@scope/name`, nên phạm vi rỗng là alias.
        if spec.starts_with("@/") || spec.starts_with("~/") || spec.starts_with('#') {
            return Resolved::Alias;
        }
        if spec.starts_with('/') {
            return Resolved::Alias;
        }
        Resolved::Package(npm_package(spec))
    }

    // ---- Python ----

    fn python(&self, importer: &str, spec: &str) -> Resolved {
        if let Some(rest) = spec.strip_prefix('.') {
            return self.python_relative(importer, rest);
        }
        let path = spec.replace('.', "/");
        let roots = ["", "src/", "lib/"];
        let candidates = roots.iter().flat_map(|r| {
            [format!("{r}{path}.py"), format!("{r}{path}/__init__.py"), format!("{r}{path}.pyi")]
        });
        match self.first_file(candidates) {
            Some(found) => Resolved::Path(found),
            // Không tìm thấy trong dự án thì đây là gói bên thứ ba. Với import
            // tuyệt đối, hai khả năng đó không phân biệt được.
            None => Resolved::Package(spec.split('.').next().unwrap_or(spec).to_string()),
        }
    }

    fn python_relative(&self, importer: &str, rest: &str) -> Resolved {
        let extra_dots = rest.len() - rest.trim_start_matches('.').len();
        let module = rest.trim_start_matches('.');

        let mut base: Vec<&str> = dir_of(importer).split('/').filter(|s| !s.is_empty()).collect();
        for _ in 0..extra_dots {
            if base.pop().is_none() {
                return Resolved::Missing;
            }
        }
        let mut prefix = base.join("/");
        if !module.is_empty() {
            let tail = module.replace('.', "/");
            prefix = if prefix.is_empty() { tail } else { format!("{prefix}/{tail}") };
        }
        let candidates =
            [format!("{prefix}.py"), format!("{prefix}/__init__.py"), format!("{prefix}.pyi")];
        self.first_file(candidates).map_or(Resolved::Missing, Resolved::Path)
    }

    // ---- C và C++ ----

    fn c(&self, importer: &str, spec: &str, system: bool) -> Resolved {
        if system {
            // Header hệ thống và thư viện bên ngoài. Khai báo bằng
            // `external LibC at "stdio.h"` nếu muốn đưa vào luật.
            return Resolved::Package(spec.to_string());
        }
        // `#include "x.h"` ưu tiên thư mục của file gọi, rồi tới các thư mục
        // include thường gặp. Đường dẫn thật nằm trong CMake hoặc Makefile mà
        // ta chưa đọc, nên đây là xấp xỉ.
        let mut candidates = Vec::new();
        if let Some(local) = join(dir_of(importer), spec) {
            candidates.push(local);
        }
        candidates.extend(C_INCLUDE_DIRS.iter().filter_map(|d| join(d, spec)));
        self.first_file(candidates).map_or(Resolved::Missing, Resolved::Path)
    }

    // ---- Rust ----

    fn rust(&self, importer: &str, spec: &str) -> Resolved {
        let mut parts: Vec<&str> = spec.split("::").collect();
        if parts.is_empty() {
            return Resolved::Missing;
        }
        let crate_src = self.crate_src(importer);

        let base = match parts[0] {
            "crate" => {
                parts.remove(0);
                crate_src
            }
            "self" => {
                parts.remove(0);
                module_dir(importer)
            }
            "super" => {
                parts.remove(0);
                let d = module_dir(importer);
                match d.rsplit_once('/') {
                    Some((up, _)) => up.to_string(),
                    None => return Resolved::Missing,
                }
            }
            "std" | "core" | "alloc" => return Resolved::Package(parts[0].to_string()),
            // Edition 2015 cho phép `use some_module::x` trỏ vào chính crate.
            // Thử trong crate trước, không có thì coi là crate bên ngoài.
            other => match self.rust_module(&crate_src, &parts) {
                Some(found) => return Resolved::Path(found),
                None => return Resolved::Package(other.to_string()),
            },
        };
        match self.rust_module(&base, &parts) {
            Some(found) => Resolved::Path(found),
            None => Resolved::Missing,
        }
    }

    /// Đoạn cuối của `use` thường là tên item chứ không phải module, và không
    /// biết trước có bao nhiêu đoạn như vậy. Thử từ đường dẫn dài nhất xuống
    /// ngắn nhất, lấy cái đầu tiên ra file.
    fn rust_module(&self, base: &str, parts: &[&str]) -> Option<String> {
        for take in (1..=parts.len()).rev() {
            let joined = parts[..take].join("/");
            let path = if base.is_empty() { joined } else { format!("{base}/{joined}") };
            if let Some(found) =
                self.first_file([format!("{path}.rs"), format!("{path}/mod.rs")])
            {
                return Some(found);
            }
        }
        None
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
        let Some(module) = &self.go_module else {
            return Resolved::Package(spec.to_string());
        };
        if spec == module {
            return Resolved::Path(String::new());
        }
        match spec.strip_prefix(&format!("{module}/")) {
            // Import của Go trỏ tới cả package, nên đích là một thư mục.
            Some(rest) if self.root.join(rest).is_dir() => Resolved::Path(rest.to_string()),
            Some(_) => Resolved::Missing,
            None => Resolved::Package(spec.to_string()),
        }
    }
}

fn read_go_module(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("go.mod")).ok()?;
    text.lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("module "))
        .map(|m| m.trim().to_string())
}

fn is_relative(spec: &str) -> bool {
    spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".."
}

fn dir_of(path: &str) -> &str {
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

fn join(dir: &str, spec: &str) -> Option<String> {
    let joined = if dir.is_empty() { spec.to_string() } else { format!("{dir}/{spec}") };
    normalize(&joined)
}

/// Gộp `.` và `..`. Trả về `None` nếu vượt ra ngoài gốc dự án.
fn normalize(path: &str) -> Option<String> {
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
