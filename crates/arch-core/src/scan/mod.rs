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

use lexer::{CharLit, LexConfig};

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
        let toks = lexer::lex(src, &self.config());
        match self {
            Lang::TypeScript => ts::extract(&toks),
            Lang::Python => python::extract(&toks),
            Lang::C => clang::extract(&toks),
            Lang::Rust => rustlang::extract(&toks),
            Lang::Go => golang::extract(&toks),
        }
    }
}

/// Đọc import của một file, nếu phần mở rộng thuộc ngôn ngữ đã hỗ trợ.
pub fn imports_for(path: &str, src: &str) -> Option<(Lang, Vec<Import>)> {
    let lang = Lang::for_path(path)?;
    Some((lang, lang.imports(src)))
}

pub fn is_supported(path: &str) -> bool {
    Lang::for_path(path).is_some()
}
