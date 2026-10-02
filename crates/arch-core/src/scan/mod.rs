//! Trích phụ thuộc từ mã nguồn, nhiều ngôn ngữ.
//!
//! Mỗi ngôn ngữ chỉ cần khai báo hai thứ: cấu hình cho [`lexer`] dùng chung, và
//! cách khớp khuôn mẫu import trên dòng token. Không ngôn ngữ nào tự viết lại
//! phần bỏ qua chú thích và chuỗi — đó là chỗ dễ sai nhất.

pub mod lexer;

mod clang;
mod golang;
mod python;
mod rustlang;
mod ts;

use lexer::{CharLit, LexConfig, Tok, Token};
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
}

impl Import {
    fn local(spec: impl Into<String>, line: usize) -> Self {
        Import { spec: spec.into(), line, system: false }
    }
}

impl Lang {
    pub fn for_path(path: &str) -> Option<Lang> {
        let ext = path.rsplit_once('.')?.1;
        Some(match ext {
            "ts" | "tsx" | "mts" | "cts" | "js" | "jsx" | "mjs" | "cjs" => Lang::TypeScript,
            "py" | "pyi" => Lang::Python,
            "c" | "h" | "cc" | "cpp" | "cxx" | "c++" | "hpp" | "hh" | "hxx" | "h++" | "inl" => {
                Lang::C
            }
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
                ..LexConfig::base()
            },
            Lang::Python => LexConfig {
                line_comments: &["#"],
                block_comment: None,
                triple_quoted: true,
                char_literals: CharLit::AsString,
                path_separator: Some("."),
                ..LexConfig::base()
            },
            Lang::C => LexConfig { preprocessor_includes: true, ..LexConfig::base() },
            Lang::Rust => LexConfig {
                nested_block_comment: true,
                raw_hash_strings: true,
                char_literals: CharLit::QuotedOrLifetime,
                path_separator: Some("::"),
                ..LexConfig::base()
            },
            Lang::Go => LexConfig { ..LexConfig::base() },
        }
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
            symbols: find_symbols(&toks, needles),
            tables: find_tables(&toks, tables),
        }
    }
}

/// Một lần gặp ký hiệu bị theo dõi trong mã.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolHit {
    pub symbol: String,
    pub line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FileScan {
    pub imports: Vec<Import>,
    pub symbols: Vec<SymbolHit>,
    pub tables: Vec<SymbolHit>,
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
                out.push(SymbolHit { symbol: (*needle).to_string(), line: t.line });
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

fn find_symbols(toks: &[Token], needles: &Needles) -> Vec<SymbolHit> {
    if needles.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for t in toks {
        let Tok::Word(word) = &t.tok else { continue };
        // Tra bảng băm thay vì duyệt từng tên: với vài chục tên và vài triệu
        // token thì duyệt tuyến tính là không chạy nổi.
        if let Some(found) = needles.get(word.as_str()) {
            out.push(SymbolHit { symbol: (*found).to_string(), line: t.line });
            continue;
        }
        // `builtins.eval` khớp `eval`, `std::ptr::copy` khớp `copy`.
        if let Some(tail) = last_segment(word)
            && let Some(found) = needles.get(tail)
        {
            out.push(SymbolHit { symbol: (*found).to_string(), line: t.line });
        }
    }
    out
}

fn last_segment(word: &str) -> Option<&str> {
    let dot = word.rfind('.').map(|i| i + 1);
    let colon = word.rfind("::").map(|i| i + 2);
    let at = dot.max(colon)?;
    word.get(at..).filter(|s| !s.is_empty())
}

/// Đọc import của một file, nếu phần mở rộng thuộc ngôn ngữ đã hỗ trợ.
pub fn imports_for(path: &str, src: &str) -> Option<(Lang, Vec<Import>)> {
    let lang = Lang::for_path(path)?;
    Some((lang, lang.imports(src)))
}

pub fn is_supported(path: &str) -> bool {
    Lang::for_path(path).is_some()
}
