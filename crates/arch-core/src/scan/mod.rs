//! Trích phụ thuộc từ mã nguồn, nhiều ngôn ngữ.
//!
//! Mỗi ngôn ngữ chỉ cần khai báo hai thứ: cấu hình cho [`lexer`] dùng chung, và
//! cách khớp khuôn mẫu import trên dòng token. Không ngôn ngữ nào tự viết lại
//! phần bỏ qua chú thích và chuỗi — đó là chỗ dễ sai nhất.

pub mod lexer;

mod clang;
pub mod foreign;
mod golang;
mod notebook;
mod python;
mod rustlang;
mod ts;
pub mod yaml;
pub mod dockerfile;
pub mod command;
pub mod compose;
pub mod deploy;

use lexer::{Backtick, CharLit, LexConfig, Tok, Token};
use std::collections::HashSet;

/// Tập tên cần tìm. Dựng một lần rồi dùng cho mọi file: số tên tăng theo số
/// khai báo `flow`, `contract` và `invariant`, nên quét tuyến tính từng tên
/// một sẽ không chịu nổi.
pub type Needles<'a> = HashSet<&'a str>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    TypeScript,
    Python,
    /// C và C++ dùng chung bộ quét: khác biệt giữa chúng không nằm ở `#include`.
    C,
    Rust,
    Go,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Chuỗi như nó xuất hiện trong mã, chưa phân giải.
    pub spec: String,
    pub line: usize,
    /// `#include <...>` của C/C++. Luôn `false` ở các ngôn ngữ khác.
    pub system: bool,
    /// Tên được nhập từ module: `from pkg import a, b as c` cho `["a", "b"]`.
    /// Chỉ Python điền. Tên nào chính là module con thì cũng là một đích —
    /// xem [`crate::resolve::Resolver::submodules`].
    pub names: Vec<String>,
}

impl Import {
    fn local(spec: impl Into<String>, line: usize) -> Self {
        Import { spec: spec.into(), line, system: false, names: Vec::new() }
    }
}

impl Lang {
    pub fn for_path(path: &str) -> Option<Lang> {
        let ext = path.rsplit_once('.')?.1;
        Some(match ext {
            // `.vue`: chỉ phần `<script>`, xem [`source_view`].
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" | "vue" => Lang::TypeScript,
            // `.pyx`, `.pxd`, `.pxi`: Cython, Python cộng thêm `cimport`.
            // `.ipynb`: chỉ ô code, xem [`source_view`].
            "py" | "pyi" | "pyx" | "pxd" | "pxi" | "ipynb" => Lang::Python,
            // `.cu`, `.cuh`: CUDA, tức C++. `.inc` cố ý không có: abseil dùng
            // nó cho C++, nhưng curl dùng cho Makefile và scikit-learn cho
            // reStructuredText.
            "c" | "h" | "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "inl"
            | "cu" | "cuh" => Lang::C,
            "rs" => Lang::Rust,
            "go" => Lang::Go,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Lang::TypeScript => "TypeScript",
            Lang::Python => "Python",
            Lang::C => "C/C++",
            Lang::Rust => "Rust",
            Lang::Go => "Go",
        }
    }

    fn config(self) -> LexConfig {
        match self {
            Lang::TypeScript => LexConfig {
                char_literals: CharLit::AsString,
                regex_literals: true,
                backtick: Backtick::Template,
                ..LexConfig::base()
            },
            Lang::Python => LexConfig {
                line_comments: &["#"],
                block_comment: None,
                triple_quoted: true,
                char_literals: CharLit::AsString,
                path_separator: Some("."),
                fstrings: true,
                ..LexConfig::base()
            },
            Lang::C => LexConfig {
                preprocessor_includes: true,
                digit_separators: true,
                cpp_raw_strings: true,
                ..LexConfig::base()
            },
            Lang::Rust => LexConfig {
                nested_block_comment: true,
                raw_hash_strings: true,
                char_literals: CharLit::QuotedOrLifetime,
                path_separator: Some("::"),
                multiline_strings: true,
                ..LexConfig::base()
            },
            Lang::Go => LexConfig { backtick: Backtick::Raw, ..LexConfig::base() },
        }
    }

    /// Dòng token của `src` theo cấu hình của ngôn ngữ này. Cho phần đọc cấu
    /// hình build viết bằng chính ngôn ngữ đó, như `setup.py`.
    pub fn tokens(self, src: &str) -> Vec<Token> {
        lexer::lex(src, &self.config())
    }

    pub fn imports(self, src: &str) -> Vec<Import> {
        self.scan(src, &[]).imports
    }

    pub fn scan(self, src: &str, needles: &[&str]) -> FileScan {
        self.scan_all(src, &needles.iter().copied().collect(), &[])
    }

    /// Đọc file một lần, lấy import, ký hiệu bị theo dõi, và tên bảng dữ liệu.
    /// Lex là phần tốn nhất, nên không làm nhiều lượt.
    ///
    /// Tên bảng **chỉ** được dò trong chuỗi: `"SELECT ... FROM payments"`,
    /// `knex('payments')`, `db.collection('payments')`. Lý do ở
    /// [`find_tables`].
    pub fn scan_all(self, src: &str, needles: &Needles, tables: &[&str]) -> FileScan {
        let toks = lexer::lex(src, &self.config());
        let imports = match self {
            Lang::TypeScript => ts::extract(&toks),
            Lang::Python => python::extract(&toks),
            Lang::C => clang::extract(&toks),
            Lang::Rust => rustlang::extract(&toks),
            Lang::Go => golang::extract(&toks),
        };
        FileScan {
            imports,
            symbols: find_symbols(&toks, needles, self),
            tables: find_tables(&toks, tables),
            foreign: foreign::scan(&toks, self),
        }
    }
}

/// Một lần gặp ký hiệu bị theo dõi trong mã.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolHit {
    pub symbol: String,
    pub line: usize,
    /// Đây là chỗ **đặt ra** cái tên, không phải chỗ dùng nó.
    ///
    /// Khác biệt này quan trọng hơn vẻ ngoài của nó: `charge` được định nghĩa
    /// trong Payment là bằng chứng Payment có năng lực đó, còn `charge` xuất
    /// hiện trong Payment có thể chỉ là một lời gọi sang chỗ khác. Luôn
    /// `false` với tên bảng dữ liệu, vì chúng được dò trong chuỗi.
    pub defines: bool,
}

#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub imports: Vec<Import>,
    pub symbols: Vec<SymbolHit>,
    pub tables: Vec<SymbolHit>,
    /// Hàm đi qua ranh giới ngôn ngữ — thứ mà câu lệnh import không nói gì.
    pub foreign: foreign::Foreign,
}

/// Tìm tên bảng, **chỉ trong chuỗi**.
///
/// Dò cả trong định danh nghe hấp dẫn hơn — nó sẽ bắt được `prisma.payments`.
/// Nhưng một trường tên `packets` trong struct cũng khớp, mà đó không phải
/// truy cập dữ liệu. Phân biệt hai thứ đó cần thông tin kiểu, và đoán sai sẽ
/// sinh báo động giả ngay trong chính mã của người dùng.
///
/// Chuỗi thì gần như không nhập nhằng: SQL thô, query builder và tên
/// collection đều nằm trong chuỗi. Muốn bắt cả cách truy cập theo thuộc tính
/// của ORM thì khai báo nó thành một nhóm `api` — cỗ máy đó vốn để dò định
/// danh.
fn find_tables(toks: &[Token], needles: &[&str]) -> Vec<SymbolHit> {
    if needles.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for t in toks {
        let (Tok::Str(text) | Tok::SystemStr(text)) = &t.tok else { continue };
        for needle in needles {
            if in_text(text, needle) {
                out.push(SymbolHit {
                    symbol: (*needle).to_string(),
                    line: t.line,
                    defines: false,
                });
            }
        }
    }
    out
}

/// Tìm tên bảng trong một chuỗi, theo ranh giới định danh. Nếu không có ranh
/// giới thì bảng `payments` sẽ khớp nhầm trong `payments_archive`.
fn in_text(text: &str, needle: &str) -> bool {
    let bytes = text.as_bytes();
    let n = needle.len();
    let boundary = |c: u8| !(c.is_ascii_alphanumeric() || c == b'_');
    text.match_indices(needle).any(|(at, _)| {
        let before = at == 0 || boundary(bytes[at - 1]);
        let after = at + n == bytes.len() || boundary(bytes[at + n]);
        before && after
    })
}

fn find_symbols(toks: &[Token], needles: &Needles, lang: Lang) -> Vec<SymbolHit> {
    if needles.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        let Tok::Word(word) = &t.tok else { continue };
        // Tra bảng băm thay vì duyệt từng tên: với vài chục tên và vài triệu
        // token thì duyệt tuyến tính là không chạy nổi.
        if let Some(found) = needles.get(word.as_str()) {
            out.push(SymbolHit {
                symbol: (*found).to_string(),
                line: t.line,
                defines: declares(toks, i, lang),
            });
            continue;
        }
        // `builtins.eval` khớp `eval`, `std::ptr::copy` khớp `copy`. Tên có
        // đường dẫn phía trước thì không bao giờ là chỗ khai báo.
        if let Some(tail) = last_segment(word)
            && let Some(found) = needles.get(tail)
        {
            out.push(SymbolHit { symbol: (*found).to_string(), line: t.line, defines: false });
        }
    }
    out
}

/// Từ khoá mở đầu một khai báo: token ngay sau chúng là tên vừa được đặt ra.
///
/// Danh sách cố ý hẹp. Nhận sót một dạng khai báo thì chỉ mất độ chặt, vì nơi
/// gọi quay về cách đếm cũ; nhận nhầm một lời gọi thành khai báo thì sinh báo
/// động giả ngay trong mã của người dùng.
fn declaring_keywords(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::TypeScript => {
            &["function", "class", "interface", "type", "enum", "const", "let", "var", "namespace"]
        }
        Lang::Python => &["def", "class"],
        // Hàm C không có từ khoá mở đầu, nên chỉ nhận được các kiểu dữ liệu.
        Lang::C => &["struct", "union", "enum", "typedef", "class", "namespace"],
        Lang::Rust => &[
            "fn", "struct", "enum", "trait", "type", "const", "static", "union", "mod",
            "macro_rules",
        ],
        Lang::Go => &["func", "type", "const", "var", "package"],
    }
}

/// Token thứ `at` có phải là một cái tên vừa được đặt ra không.
fn declares(toks: &[Token], at: usize, lang: Lang) -> bool {
    let Some(prev) = at.checked_sub(1).and_then(|i| toks.get(i)) else { return false };
    match &prev.tok {
        Tok::Word(keyword) => declaring_keywords(lang).contains(&keyword.as_str()),
        // `func (r *Repo) Save()` — tên phương thức nằm sau ngoặc nhận.
        Tok::Punct(')') => lang == Lang::Go && after_go_receiver(toks, at - 1),
        _ => false,
    }
}

/// Dấu `)` ở vị trí `close` có đóng lại ngoặc nhận của một `func` không.
fn after_go_receiver(toks: &[Token], close: usize) -> bool {
    let mut depth = 0usize;
    for i in (0..=close).rev() {
        match toks[i].tok {
            Tok::Punct(')') => depth += 1,
            Tok::Punct('(') => {
                depth -= 1;
                if depth == 0 {
                    return matches!(
                        i.checked_sub(1).map(|p| &toks[p].tok),
                        Some(Tok::Word(w)) if w == "func"
                    );
                }
            }
            _ => {}
        }
    }
    false
}

fn last_segment(word: &str) -> Option<&str> {
    let dot = word.rfind('.').map(|i| i + 1);
    let colon = word.rfind("::").map(|i| i + 2);
    let at = dot.max(colon)?;
    word.get(at..).filter(|s| !s.is_empty())
}

/// Đọc import của một file, nếu phần mở rộng thuộc ngôn ngữ đã hỗ trợ.
/// Tên mà các câu `use` của một file Rust đưa vào phạm vi, kèm đường dẫn mỗi
/// tên trỏ tới: `pub use error::Error as Failure` cho `("Failure",
/// "error::Error")`. `*` và `as _` không đưa tên nào vào.
///
/// Bộ phân giải đọc chúng ở file gốc crate, để biết `crate::Error` thật ra là
/// `crate::error::Error`.
pub fn rust_bindings(src: &str) -> Vec<(String, String)> {
    rustlang::bindings(&lexer::lex(src, &Lang::Rust.config()))
}

pub fn imports_for(path: &str, src: &str) -> Option<(Lang, Vec<Import>)> {
    let lang = Lang::for_path(path)?;
    Some((lang, lang.imports(&source_view(path, src))))
}

/// Phần mã của một file, để quét. Gần như luôn là cả file.
///
/// Component một file của Vue (`.vue`) là ngoại lệ: mã nằm trong các khối
/// `<script>`, còn `<template>` và `<style>` là HTML và CSS — có chữ như
/// "Don't import './x'", và không có import nào. Mọi thứ ngoài khối script
/// thành khoảng trắng, giữ nguyên dấu xuống dòng để số dòng vẫn đúng với file
/// gốc.
///
/// Notebook (`.ipynb`) cũng vậy: chỉ ô code, mỗi dòng mã ở đúng dòng của nó
/// trong file JSON — xem `notebook.rs`.
pub fn source_view<'a>(path: &str, src: &'a str) -> std::borrow::Cow<'a, str> {
    if path.ends_with(".ipynb") {
        return std::borrow::Cow::Owned(notebook::code_view(src));
    }
    if !path.ends_with(".vue") {
        return std::borrow::Cow::Borrowed(src);
    }
    let blank = |s: &str| s.chars().map(|c| if c == '\n' { '\n' } else { ' ' }).collect::<String>();
    let mut out = String::with_capacity(src.len());
    let mut rest = src;
    while let Some(open) = rest.find("<script") {
        // Thẻ mở kết thúc ở `>` đầu tiên sau nó.
        let Some(tag_end) = rest[open..].find('>').map(|k| open + k + 1) else { break };
        out.push_str(&blank(&rest[..tag_end]));
        let body_end = rest[tag_end..].find("</script").map_or(rest.len(), |k| tag_end + k);
        out.push_str(&rest[tag_end..body_end]);
        rest = &rest[body_end..];
    }
    out.push_str(&blank(rest));
    std::borrow::Cow::Owned(out)
}

pub fn is_supported(path: &str) -> bool {
    Lang::for_path(path).is_some()
}
