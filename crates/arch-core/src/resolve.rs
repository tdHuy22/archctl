//! Phân giải chuỗi import thành đường dẫn trong dự án hoặc tên gói.
//!
//! Đây là chỗ dễ sai nhất của cả bộ kiểm chứng, và khác nhau nhiều giữa các
//! ngôn ngữ. Nguyên tắc chung: thứ nào không phân giải được thì **báo ra**,
//! không im lặng bỏ qua. Bỏ sót âm thầm làm người dùng tin rằng không có vi
//! phạm, tệ hơn hẳn việc nói thẳng là chưa biết.

use crate::jsonc;
use crate::scan::{Import, Lang};
use std::cell::OnceCell;
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
    /// Trỏ vào trong chính crate này nhưng không ra file nào: `mod` viết lồng
    /// trong file, hoặc một item của chính file đang xét. Không có cạnh, và
    /// cũng không có gì để báo — trình biên dịch đã bảo đảm đường dẫn này tồn
    /// tại, nên nói "không tìm thấy" chỉ là nói sai.
    Internal,
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
    npm_packages: Vec<(String, String)>,
    /// Thư mục include mà build của C/C++ khai báo. Đọc muộn: repo không có
    /// file C nào thì không phải quét tìm `Makefile` làm gì.
    c_includes: OnceCell<Vec<String>>,
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
            go_module: read_go_module(root),
            rust_crates: find_rust_crates(root),
            ts_configs: find_ts_configs(root),
            npm_packages: find_npm_packages(root),
            c_includes: OnceCell::new(),
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

    fn python(&self, importer: &str, spec: &str) -> Resolved {
        if let Some(rest) = spec.strip_prefix('.') {
            return self.python_relative(importer, rest);
        }
        let path = spec.replace('.', "/");
        let roots = ["", "src/", "lib/"];
        let candidates = roots.iter().flat_map(|r| {
            [format!("{r}{path}.py"), format!("{r}{path}/__init__.py"), format!("{r}{path}.pyi")]
        });
        if let Some(found) = self.first_file(candidates) {
            return Resolved::Path(found);
        }
        // Gói kiểu không gian tên (PEP 420) không có `__init__.py`, và đó là
        // mặc định của Python hiện đại. Thư mục chính là gói.
        if let Some(found) = roots
            .iter()
            .map(|r| format!("{r}{path}"))
            .find(|candidate| self.root.join(candidate).is_dir())
        {
            return Resolved::Path(found);
        }
        // Không tìm thấy trong dự án thì đây là gói bên thứ ba. Với import
        // tuyệt đối, hai khả năng đó không phân biệt được.
        Resolved::Package(spec.split('.').next().unwrap_or(spec).to_string())
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
        if let Some(found) = self.first_file(candidates) {
            return Resolved::Path(found);
        }
        // Gói kiểu không gian tên, hoặc `from . import x` trỏ thẳng vào thư
        // mục của chính file này. Cả hai đều là thư mục chứ không phải file.
        if self.root.join(&prefix).is_dir() {
            return Resolved::Path(prefix);
        }
        Resolved::Missing
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
        if system {
            // Header hệ thống và thư viện bên ngoài. Khai báo bằng
            // `external LibC at "stdio.h"` nếu muốn đưa vào luật.
            return Resolved::Package(spec.to_string());
        }
        // Phỏng đoán cuối, cho repo không có file build nào đọc được. Chỉ dùng
        // cho `"..."`: đoán một thư mục rồi khớp `<stdio.h>` vào đó thì sai.
        self.first_file(C_INCLUDE_DIRS.iter().filter_map(|d| join(d, spec)))
            .map_or(Resolved::Missing, Resolved::Path)
    }

    // ---- Rust ----

    /// Không bao giờ trả `Missing`: đường dẫn Rust hoặc ra một file, hoặc là
    /// một crate bên ngoài, hoặc nằm trong `mod` viết lồng. Không có trường
    /// hợp thứ tư — trình biên dịch đã lo.
    fn rust(&self, importer: &str, spec: &str) -> Resolved {
        // `split` luôn trả ít nhất một phần tử, kể cả với chuỗi rỗng.
        let mut parts: Vec<&str> = spec.split("::").collect();
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
                    None => return Resolved::Internal,
                }
            }
            "std" | "core" | "alloc" => return Resolved::Package(parts[0].to_string()),
            // Crate anh em trong workspace, rồi tới edition 2015 cho phép
            // `use some_module::x` trỏ vào chính crate. Hết hai cách đó mới
            // coi là crate bên ngoài.
            other => {
                if let Some((_, src)) = self.rust_crates.iter().find(|(n, _)| n == other) {
                    let src = src.clone();
                    parts.remove(0);
                    return match self.rust_module(&src, &parts) {
                        Some(found) => Resolved::Path(found),
                        // Crate anh em có thật nhưng không lần ra module: đích
                        // vẫn là crate đó, lấy chính thư mục src của nó.
                        None => Resolved::Path(src),
                    };
                }
                return match self.rust_module(&crate_src, &parts) {
                    Some(found) => Resolved::Path(found),
                    None => Resolved::Package(other.to_string()),
                };
            }
        };
        match self.rust_module(&base, &parts) {
            Some(found) => Resolved::Path(found),
            // `crate::`, `self::` và `super::` luôn nằm trong crate này, và
            // trình biên dịch đã bảo đảm đường dẫn tồn tại. Không ra file nào
            // nghĩa là `mod` viết lồng trong file — không có cạnh để vẽ.
            None => Resolved::Internal,
        }
    }

    /// Đoạn cuối của `use` thường là tên item chứ không phải module, và không
    /// biết trước có bao nhiêu đoạn như vậy. Thử từ đường dẫn dài nhất xuống
    /// ngắn nhất, lấy cái đầu tiên ra file.
    fn rust_module(&self, base: &str, parts: &[&str]) -> Option<String> {
        // `take == 0` là chính `base`: `use super::Import` không có đoạn module
        // nào sau `super`, nên đích là chính module cha.
        for take in (0..=parts.len()).rev() {
            let joined = parts[..take].join("/");
            let path = match (base.is_empty(), take) {
                (true, 0) => continue,
                (true, _) => joined,
                (false, 0) => base.to_string(),
                (false, _) => format!("{base}/{joined}"),
            };
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
        if entry.path().is_dir() {
            let next = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            collect_manifests(root, &next, depth + 1, out);
        }
    }
}

fn package_name(manifest: &Path) -> Option<String> {
    let text = std::fs::read_to_string(manifest).ok()?;
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
    collect_dirs_with(root, "", 0, "package.json", &mut dirs);
    let mut out: Vec<(String, String)> = dirs
        .iter()
        .filter(|dir| !dir.is_empty())
        .filter_map(|dir| {
            let path = root.join(dir).join("package.json");
            let text = std::fs::read_to_string(path).ok()?;
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
    collect_dirs_with(root, "", 0, "tsconfig.json", &mut dirs);
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
        let Some(value) = std::fs::read_to_string(root.join(&path)).ok().and_then(|s| jsonc::parse(&s))
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

/// Thư mục chứa một file tên cho trước, quét tới độ sâu giới hạn.
fn collect_dirs_with(root: &Path, prefix: &str, depth: usize, name: &str, out: &mut Vec<String>) {
    const MAX_DEPTH: usize = 4;
    const SKIP: &[&str] = &["target", "node_modules", ".git", ".arch", "vendor", "dist"];

    if depth > MAX_DEPTH {
        return;
    }
    let dir = if prefix.is_empty() { root.to_path_buf() } else { root.join(prefix) };
    if dir.join(name).is_file() {
        out.push(prefix.to_string());
    }
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let Ok(child) = entry.file_name().into_string() else { continue };
        if SKIP.contains(&child.as_str()) || child.starts_with('.') {
            continue;
        }
        if entry.path().is_dir() {
            let next = if prefix.is_empty() { child } else { format!("{prefix}/{child}") };
            collect_dirs_with(root, &next, depth + 1, name, out);
        }
    }
}

fn read_go_module(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("go.mod")).ok()?;
    crate::strip_bom(&text)
        .lines()
        .map(str::trim)
        .find_map(|l| l.strip_prefix("module "))
        .map(|m| m.trim().to_string())
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
