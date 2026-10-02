//! Python.
//!
//! Lexer đã gộp `a.b.c` thành một từ nhờ `path_separator`. Import của Python
//! theo dòng, nên ranh giới câu lệnh được xác định bằng số dòng của token —
//! lexer không phát ra token xuống dòng.

use super::Import;
use super::lexer::{Tok, Token};

pub fn extract(toks: &[Token]) -> Vec<Import> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let Tok::Word(word) = &toks[i].tok else {
            i += 1;
            continue;
        };
        match word.as_str() {
            "from" => i = from_import(toks, i, &mut out),
            "import" => i = plain_import(toks, i, &mut out),
            _ => i += 1,
        }
    }
    out
}

/// `from a.b import c` · `from . import c` · `from ..pkg import c`
fn from_import(toks: &[Token], start: usize, out: &mut Vec<Import>) -> usize {
    let line = toks[start].line;
    let mut i = start + 1;
    let mut spec = String::new();

    while matches!(toks.get(i), Some(t) if t.tok == Tok::Punct('.') && t.line == line) {
        spec.push('.');
        i += 1;
    }
    if let Some(t) = toks.get(i)
        && t.line == line
        && let Tok::Word(w) = &t.tok
        && w != "import"
    {
        spec.push_str(w);
        i += 1;
    }
    if !spec.is_empty() {
        out.push(Import::local(spec, line));
    }

    // Bỏ phần tên được import. Nó có thể trải nhiều dòng khi nằm trong ngoặc.
    skip_names(toks, i)
}

/// Nhảy qua danh sách tên sau `import`, tôn trọng ngoặc đơn nhiều dòng.
fn skip_names(toks: &[Token], start: usize) -> usize {
    let mut i = start;
    let mut depth = 0usize;
    let mut last_line = toks.get(start).map_or(0, |t| t.line);
    while let Some(t) = toks.get(i) {
        match t.tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => depth = depth.saturating_sub(1),
            _ => {
                if depth == 0 && t.line > last_line {
                    return i;
                }
            }
        }
        last_line = t.line;
        i += 1;
    }
    i
}

/// `import a.b` · `import a.b as c, d.e`
fn plain_import(toks: &[Token], start: usize, out: &mut Vec<Import>) -> usize {
    let line = toks[start].line;
    let mut i = start + 1;
    let mut expect_module = true;

    while let Some(t) = toks.get(i) {
        if t.line != line {
            break;
        }
        match &t.tok {
            Tok::Word(w) if w == "as" => {
                i += 2; // bỏ cả bí danh
                expect_module = false;
                continue;
            }
            Tok::Word(w) if expect_module => {
                out.push(Import::local(w, line));
                expect_module = false;
            }
            Tok::Punct(',') => expect_module = true,
            _ => break,
        }
        i += 1;
    }
    i
}
