//! Thư mục include của C/C++, đọc từ hệ thống build.
//!
//! `#include "aes.h"` không nói gì về nơi `aes.h` nằm — đường dẫn thật nằm
//! trong cờ `-I` của build. Đoán bằng vài tên thư mục thường gặp thì bỏ sót,
//! và bỏ sót ở đây không phải là bớt thông tin: một vi phạm thật im lặng
//! biến mất khỏi báo cáo.
//!
//! Đọc theo thứ tự tin cậy giảm dần: `compile_commands.json` (do chính trình
//! biên dịch sinh ra nên là sự thật), rồi `CMakeLists.txt`, rồi `Makefile`.
//!
//! **Không chạy `cmake` hay `make`.** Công cụ này chạy trong repo của người
//! dùng và trong vòng lặp của agent; đọc file thì an toàn, thực thi file build
//! của một repo lạ thì không.

use crate::jsonc;
use crate::resolve::{dir_of, join, normalize};
use crate::resolve::Listing;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;

/// Mỗi include không phân giải được sẽ thử lần lượt từng thư mục, nên danh
/// sách dài là một lần hỏi đĩa dài. Giới hạn để trường hợp xấu nhất bị chặn.
const MAX_DIRS: usize = 64;
/// Số file cấu hình đọc tối đa. Monorepo có thể có hàng trăm `CMakeLists.txt`.
const MAX_FILES: usize = 48;
const MAX_DEPTH: usize = 3;
const SKIP: &[&str] = &["node_modules", ".git", ".arch", "target", "dist"];

/// Cờ mở đầu một thư mục include. Không nhận `/I` của MSVC: nó không phân
/// biệt được với một đường dẫn tuyệt đối bắt đầu bằng `I`.
const FLAGS: &[&str] = &["-I", "-isystem", "-iquote", "-idirafter"];

const CMAKE_CALLS: &[&str] = &["include_directories", "target_include_directories"];
/// Từ khoá trong `target_include_directories`, không phải thư mục.
const CMAKE_KEYWORDS: &[&str] = &["PUBLIC", "PRIVATE", "INTERFACE", "SYSTEM", "BEFORE", "AFTER"];

/// Thứ tự quyết định thư mục nào được thử trước, nên nó cũng là thứ tự tin cậy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    CompileCommands,
    Cmake,
    Make,
    /// `Cargo.toml`: không có thư mục include, chỉ có thư viện `cdylib`.
    Cargo,
}

/// Mọi thư mục include mà build khai báo, tương đối so với gốc dự án, đã lọc
/// còn những thư mục thật sự tồn tại.
pub fn include_dirs(root: &Path) -> Vec<String> {
    let mut configs = Vec::new();
    walk(root, "", 0, &mut configs);
    // Thứ tự của `read_dir` phụ thuộc hệ thống file. Sắp lại để hai lần chạy
    // trên cùng một repo luôn cho cùng một kết quả.
    configs.sort();
    configs.truncate(MAX_FILES);

    let mut out: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // `compile_commands.json` lặp cùng một `-I` ở hàng nghìn dòng, nên tên
    // thật của mỗi thư mục chỉ đọc một lần.
    let mut listing = crate::resolve::Listing::new();
    for (kind, path) in &configs {
        let base = dir_of(path);
        let Ok(raw) = std::fs::read_to_string(root.join(path)) else { continue };
        let src = crate::strip_bom(&raw);
        let raws = match kind {
            Kind::CompileCommands => from_compile_commands(src),
            Kind::Cmake => from_cmake(src, base),
            Kind::Make => from_make(src, base),
            Kind::Cargo => continue,
        };
        for (at, raw) in raws {
            let Some(dir) = to_repo_dir(root, &at, &raw, &mut listing) else { continue };
            if seen.insert(dir.clone()) {
                out.push(dir);
                if out.len() >= MAX_DIRS {
                    return out;
                }
            }
        }
    }
    out
}

fn walk(root: &Path, prefix: &str, depth: usize, out: &mut Vec<(Kind, String)>) {
    if depth > MAX_DEPTH {
        return;
    }
    let dir = if prefix.is_empty() { root.to_path_buf() } else { root.join(prefix) };
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        // Không đi theo symlink: tránh vòng lặp và tránh ra ngoài gốc dự án.
        let Ok(meta) = entry.path().symlink_metadata() else { continue };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            if !SKIP.contains(&name.as_str()) {
                walk(root, &rel, depth + 1, out);
            }
        } else if let Some(kind) = kind_of(&name) {
            out.push((kind, rel));
        }
    }
}

fn kind_of(name: &str) -> Option<Kind> {
    match name {
        "compile_commands.json" => Some(Kind::CompileCommands),
        "CMakeLists.txt" => Some(Kind::Cmake),
        "Makefile" | "makefile" | "GNUmakefile" => Some(Kind::Make),
        "Cargo.toml" => Some(Kind::Cargo),
        _ if name.ends_with(".cmake") => Some(Kind::Cmake),
        _ if name.ends_with(".mk") => Some(Kind::Make),
        _ => None,
    }
}

// ---- thư viện dùng chung ----

/// Cỡ tối đa của một file build đọc để tìm thư viện dùng chung.
const MAX_BUILD_BYTES: u64 = 1 << 20;
/// Số thư viện tối đa ghi nhận cho cả repo.
const MAX_LIBRARIES: usize = 1024;

/// Thư viện dùng chung mà build của repo dựng, cho phía Python nạp bằng
/// `ctypes`/`cffi`: `(khoá lib:tên, đường dẫn trong repo cho biết component)`.
///
/// - CMake: `add_library(foo SHARED …)`, `MODULE`; component theo file nguồn
///   đầu tiên viết thẳng, không thì theo chính `CMakeLists.txt`.
/// - Makefile: `-o libfoo.so` (cả `-buildmode=c-shared` của Go), đích
///   `libfoo.so:`; component theo Makefile.
/// - Cargo: `crate-type = ["cdylib"]`; tên ở `[lib] name`, không thì tên gói
///   (`-` thành `_`); component theo `Cargo.toml`.
///
/// Tên có biến chưa biết thì bỏ: đoán tên sai sinh cạnh không có thật.
pub fn shared_libraries(root: &Path) -> Vec<(String, String)> {
    let mut configs = Vec::new();
    walk(root, "", 0, &mut configs);
    configs.sort();
    configs.truncate(MAX_FILES * 2);
    let mut out: Vec<(String, String)> = Vec::new();
    for (kind, path) in &configs {
        let base = dir_of(path);
        let Some(src) = read_capped(&root.join(path)) else { continue };
        let src = crate::strip_bom(&src);
        let found: Vec<(String, String)> = match kind {
            Kind::Cmake => cmake_libraries(src, base)
                .into_iter()
                .map(|(name, source)| {
                    let at = source
                        .and_then(|s| normalize(&join(base, &s)?))
                        .filter(|p| root.join(p).is_file())
                        .unwrap_or_else(|| path.clone());
                    (name, at)
                })
                .collect(),
            Kind::Make => make_libraries(src).into_iter().map(|n| (n, path.clone())).collect(),
            Kind::Cargo => cargo_cdylib(src).into_iter().map(|n| (n, path.clone())).collect(),
            Kind::CompileCommands => Vec::new(),
        };
        for (name, at) in found {
            if let Some(key) = crate::scan::foreign::library_key(&name) {
                out.push((key, at));
                if out.len() >= MAX_LIBRARIES {
                    return out;
                }
            }
        }
    }
    out
}

fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let file = std::fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::new();
    file.take(MAX_BUILD_BYTES).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// `add_library(tên SHARED|MODULE nguồn…)`: tên và file nguồn đầu tiên.
fn cmake_libraries(src: &str, _base: &str) -> Vec<(String, Option<String>)> {
    const TYPES: &[&str] = &["SHARED", "MODULE"];
    const SKIP: &[&str] = &["STATIC", "OBJECT", "INTERFACE", "IMPORTED", "ALIAS", "EXCLUDE_FROM_ALL", "GLOBAL"];
    let clean = strip_hash_comments(src);
    let lower = clean.to_ascii_lowercase();
    let name = "add_library";
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = lower[at..].find(name) {
        let start = at + found;
        at = start + name.len();
        if start > 0 && is_ident(lower.as_bytes()[start - 1]) {
            continue;
        }
        let Some(open) = lower[at..].find('(') else { break };
        if clean[at..at + open].chars().any(|c| !c.is_whitespace()) {
            continue;
        }
        let body_start = at + open + 1;
        let Some(len) = balanced(&clean[body_start..]) else { break };
        at = body_start + len + 1;
        let mut words = clean[body_start..body_start + len].split_whitespace().map(|w| w.trim_matches(['"', '\'']));
        let Some(target) = words.next() else { continue };
        let rest: Vec<&str> = words.collect();
        // `SHARED IMPORTED` là thư viện dựng sẵn, `ALIAS` là tên khác: repo không dựng.
        let has = |set: &[&str]| rest.iter().any(|w| set.contains(&w.to_ascii_uppercase().as_str()));
        if !has(TYPES) || has(&["IMPORTED", "ALIAS"]) || target.contains('$') {
            continue;
        }
        let source = rest
            .iter()
            .find(|w| {
                let up = w.to_ascii_uppercase();
                !TYPES.contains(&up.as_str()) && !SKIP.contains(&up.as_str()) && !w.contains('$') && w.contains('.')
            })
            .map(|w| w.to_string());
        out.push((target.to_string(), source));
    }
    out
}

fn is_library_file(name: &str) -> bool {
    name.ends_with(".so") || name.contains(".so.") || name.ends_with(".dylib") || name.ends_with(".dll")
}

/// `-o libfoo.so` và đích `libfoo.so:` của Makefile, sau khi mở biến.
fn make_libraries(src: &str) -> Vec<String> {
    const ROUNDS: usize = 3;
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut out = Vec::new();
    for raw in src.lines() {
        let line = match raw.find('#') {
            Some(i) => &raw[..i],
            None => raw,
        };
        if let Some((name, value)) = assignment(line)
            && !line.contains("-o ")
        {
            let value = expand(value, &vars, ROUNDS);
            vars.insert(name.to_string(), value);
            continue;
        }
        let line = expand(line, &vars, ROUNDS);
        let args = split_command(&line);
        for pair in args.windows(2) {
            if pair[0] == "-o" && is_library_file(&pair[1]) && !pair[1].contains('$') {
                out.push(pair[1].clone());
            }
        }
        // Đích của luật: `libfoo.so: a.o b.o` (dòng không bắt đầu bằng tab).
        if !raw.starts_with('\t')
            && let Some((targets, _)) = line.split_once(':')
            && !line.contains(":=")
        {
            out.extend(targets.split_whitespace().filter(|t| is_library_file(t) && !t.contains('$')).map(str::to_string));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Tên thư viện `cdylib` của một `Cargo.toml`, đọc theo dòng như phần Rust
/// của bộ phân giải.
fn cargo_cdylib(src: &str) -> Option<String> {
    let mut section = String::new();
    let (mut package, mut lib_name, mut cdylib) = (None, None, false);
    let mut lines = src.lines().map(|l| l.split('#').next().unwrap_or("").trim());
    while let Some(line) = lines.next() {
        if line.starts_with('[') && !line.contains('=') {
            section = line.to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, mut value) = (key.trim(), value.trim().to_string());
        // `crate-type = [` viết nhiều dòng.
        if value.starts_with('[') && !value.contains(']') {
            for more in lines.by_ref().take(64) {
                value.push_str(more);
                if more.contains(']') {
                    break;
                }
            }
        }
        let text = || value.trim_matches(['"', '\'']).to_string();
        match (section.as_str(), key) {
            ("[package]", "name") => package = Some(text()),
            ("[lib]", "name") => lib_name = Some(text()),
            ("[lib]", "crate-type" | "crate_type") => cdylib = value.contains("cdylib"),
            _ => {}
        }
    }
    cdylib.then(|| lib_name.or(package).map(|n| n.replace('-', "_")))?
}

// ---- compile_commands.json ----

/// Mỗi mục có `directory` riêng, và đường dẫn trong `-I` tính từ đó — không
/// phải từ nơi đặt `compile_commands.json`.
fn from_compile_commands(src: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(value) = jsonc::parse(src) else { return out };
    let Some(entries) = value.as_array() else { return out };
    for entry in entries {
        let at =
            entry.get("directory").and_then(jsonc::Value::as_str).unwrap_or_default().to_string();
        let args: Vec<String> = match entry.get("arguments").and_then(jsonc::Value::as_array) {
            Some(items) => items.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            None => entry
                .get("command")
                .and_then(jsonc::Value::as_str)
                .map(split_command)
                .unwrap_or_default(),
        };
        out.extend(include_flags(&args).into_iter().map(|dir| (at.clone(), dir)));
    }
    out
}

/// Tách một dòng lệnh thành các tham số, tôn trọng dấu nháy vì đường dẫn có
/// thể chứa khoảng trắng.
fn split_command(cmd: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for c in cmd.chars() {
        if escaped {
            current.push(c);
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if let Some(q) = quote {
            if c == q {
                quote = None;
            } else {
                current.push(c);
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c.is_whitespace() {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// `-Idir` và `-I dir` đều hợp lệ, nên phải xét cả hai dạng.
fn include_flags(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        for flag in FLAGS {
            if arg == *flag {
                if let Some(next) = args.get(i + 1) {
                    out.push(next.clone());
                    i += 1;
                }
                break;
            }
            if let Some(rest) = arg.strip_prefix(flag)
                && !rest.is_empty()
            {
                out.push(rest.to_string());
                break;
            }
        }
        i += 1;
    }
    out
}

// ---- CMakeLists.txt ----

fn from_cmake(src: &str, base: &str) -> Vec<(String, String)> {
    let clean = strip_hash_comments(src);
    // Tên lệnh của CMake không phân biệt chữ hoa chữ thường. `to_ascii_lowercase`
    // giữ nguyên độ dài theo byte nên hai chuỗi dùng chung được chỉ số.
    let lower = clean.to_ascii_lowercase();
    let mut out = Vec::new();

    for name in CMAKE_CALLS {
        let mut at = 0;
        while let Some(found) = lower[at..].find(name) {
            let start = at + found;
            at = start + name.len();
            // `include_directories` là hậu tố của `target_include_directories`,
            // nên phải chắc đây là đầu một định danh.
            if start > 0 && is_ident(lower.as_bytes()[start - 1]) {
                continue;
            }
            let Some(open) = lower[at..].find('(') else { continue };
            if clean[at..at + open].chars().any(|c| !c.is_whitespace()) {
                continue;
            }
            let body_start = at + open + 1;
            let Some(len) = balanced(&clean[body_start..]) else { continue };
            at = body_start + len + 1;
            out.extend(cmake_args(&clean[body_start..body_start + len], base, *name != "include_directories"));
        }
    }
    out
}

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Độ dài phần thân tới dấu `)` khớp với dấu `(` đã mở.
fn balanced(src: &str) -> Option<usize> {
    let mut depth = 1usize;
    for (i, c) in src.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn cmake_args(body: &str, base: &str, is_target: bool) -> Vec<(String, String)> {
    let mut words = body
        .split_whitespace()
        .map(|w| w.trim_matches(['"', '\'']))
        .filter(|w| !w.is_empty());
    if is_target {
        // Tham số đầu của `target_include_directories` là tên target.
        words.next();
    }
    words
        .filter(|w| !CMAKE_KEYWORDS.contains(&w.to_ascii_uppercase().as_str()))
        .filter_map(|w| cmake_path(w, base))
        .collect()
}

/// Mở các biến CMake hay gặp. Trả `None` khi còn biến chưa biết: một đường
/// dẫn đoán sai còn tệ hơn không có đường dẫn nào.
fn cmake_path(word: &str, base: &str) -> Option<(String, String)> {
    // `$<INSTALL_INTERFACE:...>` nói về cây đã cài đặt, không phải cây nguồn.
    if word.contains("$<INSTALL_INTERFACE:") {
        return None;
    }
    let mut text = word.to_string();
    if let Some(inner) = text.strip_prefix("$<BUILD_INTERFACE:") {
        text = inner.trim_end_matches('>').to_string();
    }
    // Thư mục nguồn hiện tại là nơi đặt chính file CMakeLists này.
    for var in ["${CMAKE_CURRENT_SOURCE_DIR}", "${CMAKE_CURRENT_LIST_DIR}"] {
        text = text.replace(var, if base.is_empty() { "." } else { base });
    }
    for var in ["${CMAKE_SOURCE_DIR}", "${PROJECT_SOURCE_DIR}", "${CMAKE_HOME_DIRECTORY}"] {
        text = text.replace(var, ".");
    }
    if text.contains('$') {
        return None;
    }
    // Đã mở biến thì đường dẫn tính từ gốc dự án, không từ `base` nữa.
    if word.contains("${") {
        return Some((String::new(), normalize(&text)?));
    }
    Some((base.to_string(), text))
}

fn strip_hash_comments(src: &str) -> String {
    src.lines()
        .map(|line| match line.find('#') {
            Some(i) => &line[..i],
            None => line,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---- Makefile ----

/// Biến của make được mở gần đúng: coi mọi phép gán là tức thời. Đủ cho
/// `INCDIR = include` rồi `-I$(INCDIR)`, là dạng thực tế hay gặp.
fn from_make(src: &str, base: &str) -> Vec<(String, String)> {
    const ROUNDS: usize = 3;

    let mut vars: HashMap<String, String> = HashMap::new();
    let mut raws: Vec<String> = Vec::new();

    for line in src.lines() {
        let line = match line.find('#') {
            Some(i) => &line[..i],
            None => line,
        };
        if let Some((name, value)) = assignment(line) {
            let value = expand(value, &vars, ROUNDS);
            match vars.get_mut(name) {
                // `+=` nối thêm; mọi dạng khác ghi đè.
                Some(slot) if line.contains("+=") => {
                    slot.push(' ');
                    slot.push_str(&value);
                }
                _ => {
                    vars.insert(name.to_string(), value);
                }
            }
        }
        raws.extend(include_flags(&split_command(line)));
    }

    raws.into_iter()
        .map(|raw| expand(&raw, &vars, ROUNDS))
        .filter(|raw| !raw.contains('$'))
        .map(|raw| (base.to_string(), raw))
        .collect()
}

/// `NAME = v`, `NAME := v`, `NAME ?= v`, `NAME += v`. Tên phải là định danh,
/// để không bắt phải `-DFOO=1` hay một dòng recipe nào đó.
fn assignment(line: &str) -> Option<(&str, &str)> {
    let eq = line.find('=')?;
    let (mut name, value) = (line[..eq].trim_end(), line[eq + 1..].trim());
    name = name.trim_end_matches([':', '?', '+', '!']).trim();
    if name.is_empty() || !name.bytes().all(|b| is_ident(b) || b == b'.') {
        return None;
    }
    Some((name, value))
}

fn expand(text: &str, vars: &HashMap<String, String>, rounds: usize) -> String {
    let mut out = text.to_string();
    for _ in 0..rounds {
        if !out.contains('$') {
            break;
        }
        out = expand_once(&out, vars);
    }
    out
}

fn expand_once(text: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(text.len());
    let cs: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let close = match (cs[i], cs.get(i + 1)) {
            ('$', Some('(')) => Some(')'),
            ('$', Some('{')) => Some('}'),
            _ => None,
        };
        let Some(close) = close else {
            out.push(cs[i]);
            i += 1;
            continue;
        };
        let Some(end) = cs[i + 2..].iter().position(|c| *c == close) else {
            out.push(cs[i]);
            i += 1;
            continue;
        };
        let name: String = cs[i + 2..i + 2 + end].iter().collect();
        match vars.get(name.as_str()) {
            Some(value) => out.push_str(value),
            // Biến chưa biết: giữ nguyên để nơi gọi thấy `$` mà loại bỏ.
            None => out.extend(&cs[i..i + 3 + end]),
        }
        i += 3 + end;
    }
    out
}

// ---- đưa về đường dẫn trong repo ----

/// `at` là thư mục mà `raw` tính từ đó, tương đối so với gốc dự án hoặc tuyệt
/// đối. Kết quả chỉ được trả về khi nó thật sự là một thư mục trong repo, viết
/// đúng hoa thường — xem [`crate::resolve::exact_case`].
fn to_repo_dir(root: &Path, at: &str, raw: &str, listing: &mut Listing) -> Option<String> {
    let raw = raw.replace('\\', "/");
    let at = at.replace('\\', "/");
    let combined = if raw.starts_with('/') {
        raw
    } else if at.starts_with('/') {
        format!("{}/{raw}", at.trim_end_matches('/'))
    } else {
        let rel = join(&at, &raw)?;
        return is_repo_dir(root, &rel, listing).then_some(rel);
    };

    let rel = relativize(root, &combined, listing)?;
    is_repo_dir(root, &rel, listing).then_some(rel)
}

fn is_repo_dir(root: &Path, rel: &str, listing: &mut Listing) -> bool {
    root.join(rel).is_dir() && crate::resolve::exact_case(root, rel, listing)
}

/// Đường dẫn tuyệt đối → tương đối so với gốc dự án.
///
/// `compile_commands.json` thường được sinh ở máy khác hoặc trong container,
/// nên tiền tố tuyệt đối trong đó có thể không khớp gốc hiện tại. Khi không
/// khớp, lấy hậu tố dài nhất mà thật sự là thư mục trong repo — và đòi ít nhất
/// hai đoạn, vì một đoạn như `include` thì `/usr/include` cũng khớp.
fn relativize(root: &Path, abs: &str, listing: &mut Listing) -> Option<String> {
    let clean = normalize(abs)?;
    for prefix in [Some(root.to_path_buf()), root.canonicalize().ok()] {
        let Some(prefix) = prefix else { continue };
        let prefix = prefix.to_string_lossy().trim_start_matches('/').to_string();
        if prefix.is_empty() {
            continue;
        }
        if let Some(rest) = clean.strip_prefix(&prefix).and_then(|r| r.strip_prefix('/')) {
            return Some(rest.to_string());
        }
    }

    let segments: Vec<&str> = clean.split('/').filter(|s| !s.is_empty()).collect();
    (0..segments.len().saturating_sub(1))
        .map(|start| segments[start..].join("/"))
        .find(|candidate| is_repo_dir(root, candidate, listing))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raws(pairs: Vec<(String, String)>) -> Vec<String> {
        pairs.into_iter().map(|(at, raw)| if at.is_empty() { raw } else { format!("{at}|{raw}") }).collect()
    }

    #[test]
    fn co_include_dinh_lien_va_tach_roi() {
        let args = split_command("cc -O2 -Iinclude -I vendor/x -isystem sys -c a.c");
        assert_eq!(include_flags(&args), ["include", "vendor/x", "sys"]);
    }

    #[test]
    fn duong_dan_co_khoang_trang_trong_dau_nhay() {
        let args = split_command(r#"cc "-I/opt/My Libs/include" -c a.c"#);
        assert_eq!(include_flags(&args), ["/opt/My Libs/include"]);
    }

    #[test]
    fn khong_nham_co_khac_thanh_include() {
        // `-include` nạp sẵn một file, không phải thêm thư mục tìm kiếm.
        let args = split_command("cc -include config.h -idirafter late -c a.c");
        assert_eq!(include_flags(&args), ["late"]);
    }

    #[test]
    fn doc_duoc_ca_arguments_va_command() {
        let src = r#"[
          {"directory": "/w/b", "arguments": ["cc", "-I../inc", "-c", "a.c"]},
          {"directory": "/w/b", "command": "cc -I/w/other -c b.c"}
        ]"#;
        assert_eq!(raws(from_compile_commands(src)), ["/w/b|../inc", "/w/b|/w/other"]);
    }

    #[test]
    fn cmake_bo_ten_target_va_tu_khoa() {
        let src = "target_include_directories(mylib PUBLIC include PRIVATE src)";
        assert_eq!(raws(from_cmake(src, "lib")), ["lib|include", "lib|src"]);
    }

    #[test]
    fn cmake_mo_bien_thu_muc_nguon() {
        // CURRENT_SOURCE_DIR là nơi đặt file CMakeLists; SOURCE_DIR là gốc dự án.
        let src = "include_directories(${CMAKE_CURRENT_SOURCE_DIR}/inc ${CMAKE_SOURCE_DIR}/vendor/h)";
        assert_eq!(raws(from_cmake(src, "lib")), ["lib/inc", "vendor/h"]);
    }

    #[test]
    fn cmake_bo_bien_chua_biet_va_cay_da_cai_dat() {
        let src = "include_directories(${SOMETHING_ELSE}/inc $<INSTALL_INTERFACE:include> ${CMAKE_BINARY_DIR}/gen)";
        assert!(raws(from_cmake(src, "")).is_empty(), "đoán sai còn tệ hơn không đoán");
    }

    #[test]
    fn cmake_lay_phan_build_cua_generator_expression() {
        let src = "target_include_directories(t PUBLIC $<BUILD_INTERFACE:${CMAKE_CURRENT_SOURCE_DIR}/inc>)";
        assert_eq!(raws(from_cmake(src, "lib")), ["lib/inc"]);
    }

    #[test]
    fn cmake_khong_doc_chu_thich() {
        let src = "# include_directories(ma)\ninclude_directories(that)";
        assert_eq!(raws(from_cmake(src, "")), ["that"]);
    }

    #[test]
    fn cmake_khong_nham_hai_lenh_long_nhau() {
        // `include_directories` là hậu tố của `target_include_directories`:
        // nhận nhầm thì tham số đầu bị coi là thư mục thay vì tên target.
        let src = "target_include_directories(mylib PUBLIC inc)";
        assert_eq!(raws(from_cmake(src, "")), ["inc"]);
    }

    #[test]
    fn cmake_ten_lenh_khong_phan_biet_chu_hoa() {
        assert_eq!(raws(from_cmake("INCLUDE_DIRECTORIES(Inc)", "")), ["Inc"]);
    }

    #[test]
    fn cmake_doc_duoc_lenh_tren_nhieu_dong() {
        let src = "include_directories(\n  a\n  b\n)\ninclude_directories(c)";
        assert_eq!(raws(from_cmake(src, "")), ["a", "b", "c"]);
    }

    #[test]
    fn make_mo_bien_va_noi_them() {
        let src = "INC := vendor\nCFLAGS = -I$(INC)/h\nCFLAGS += -I${INC}/h2\n";
        assert_eq!(raws(from_make(src, "n")), ["n|vendor/h", "n|vendor/h2"]);
    }

    #[test]
    fn make_bo_bien_chua_dinh_nghia() {
        assert!(raws(from_make("CFLAGS = -I$(UNKNOWN)/h\n", "")).is_empty());
    }

    #[test]
    fn make_khong_doc_chu_thich() {
        assert_eq!(raws(from_make("#CFLAGS = -Ima\nCFLAGS = -Ithat\n", "")), ["that"]);
    }

    #[test]
    fn make_chi_nhan_phep_gan_that() {
        assert_eq!(assignment("CFLAGS := -O2"), Some(("CFLAGS", "-O2")));
        assert_eq!(assignment("A.B ?= x"), Some(("A.B", "x")));
        // Một dòng recipe không phải phép gán, dù có dấu `=`.
        assert_eq!(assignment("\t$(CC) -DFOO=1 -o x x.c"), None);
    }

    #[test]
    fn make_doc_duoc_co_trong_dong_recipe() {
        assert_eq!(raws(from_make("all:\n\t$(CC) -Iinc -o x x.c\n", "")), ["inc"]);
    }

    #[test]
    fn thu_vien_dung_chung_cua_build() {
        assert_eq!(
            cmake_libraries("# add_library(no SHARED x.c)\nadd_library(a SHARED src/a.c)\nADD_LIBRARY(b MODULE)\nadd_library(c STATIC c.c)\nadd_library(${N} SHARED n.c)\n", ""),
            [("a".to_string(), Some("src/a.c".to_string())), ("b".to_string(), None)]
        );
        assert_eq!(make_libraries("OUT := build/libx.so.1\nlibz.dylib: z.o\n\t$(CC) -shared -o $(OUT) x.o\n\t$(CC) -o $(UNKNOWN).so y.o\n"), ["build/libx.so.1", "libz.dylib"]);
        assert_eq!(cargo_cdylib("[package]\nname = \"my-ext\"\n[lib]\ncrate-type = [\"cdylib\", \"rlib\"]\n").as_deref(), Some("my_ext"));
        assert_eq!(cargo_cdylib("[package]\nname = \"x\"\n[lib]\nname = \"y\"\n"), None);
        assert_eq!(cargo_cdylib("[package]\nname = \"x\"\n[lib]\ncrate-type = [\n  \"rlib\",\n  \"cdylib\",\n]\n").as_deref(), Some("x"));
        assert!(cmake_libraries("add_library(cudart SHARED IMPORTED)\nadd_library(a::b ALIAS a)\n", "").is_empty());
        assert_eq!(crate::scan::foreign::library_key("build/libx.so.1").as_deref(), Some("lib:x"));
        assert_eq!(crate::scan::foreign::library_key("C:\\x\\foo.dll").as_deref(), Some("lib:foo"));
        assert_eq!(crate::scan::foreign::library_key("lib"), Some("lib:lib".to_string()));
        assert_eq!(crate::scan::foreign::library_key("a b.so"), None);
    }

}
