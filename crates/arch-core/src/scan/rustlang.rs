//! Rust.
//!
//! Khó hơn các ngôn ngữ còn lại vì `use crate::a::b::c` là đường dẫn **logic**
//! trong cây module, không phải đường dẫn file. Phần đoán xem đoạn nào là
//! module và đoạn nào là tên item thuộc về bước phân giải; ở đây chỉ trích ra
//! đường dẫn đầy đủ.
//!
//! Lexer đã gộp `a::b::c` thành một từ nhờ `path_separator`, và dừng trước
//! `::{` và `::*`. Phần còn lại của một câu `use` là một cây: ngoặc nhọn lồng
//! nhau, `self`, `*`, `as`. Đọc nó như một cây, không phải một danh sách từ —
//! bản đọc theo danh sách từng coi tên sau `as` là một thành viên, và quên tiền
//! tố của ngoặc lồng.

use super::Import;
use super::lexer::{Tok, Token};

/// Một lá của cây `use`: đường dẫn tới thứ được nhập, và tên nó mang trong phạm
/// vi. `None` khi là `*` hoặc `as _` — không đưa tên nào vào.
struct Leaf {
    path: String,
    name: Option<String>,
    line: usize,
    /// Nằm ở tầng ngoài cùng của file, không trong thân hàm hay `mod` lồng nào.
    /// Chỉ những tên này vào phạm vi của module file.
    top: bool,
}

pub fn extract(toks: &[Token]) -> Vec<Import> {
    leaves(toks)
        .into_iter()
        .filter(|l| !l.path.is_empty())
        .map(|l| Import::local(l.path, l.line))
        .collect()
}

/// `(tên, đường dẫn)` cho mỗi tên mà file này đưa vào phạm vi bằng `use` hoặc
/// `extern crate`.
pub fn bindings(toks: &[Token]) -> Vec<(String, String)> {
    leaves(toks).into_iter().filter(|l| l.top).filter_map(|l| Some((l.name?, l.path))).collect()
}

fn leaves(toks: &[Token]) -> Vec<Leaf> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    // `mod tên { ... }` đang mở, kèm độ sâu ngoặc ngay bên ngoài nó.
    let mut inline: Vec<(String, usize)> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let word = match &toks[i].tok {
            Tok::Punct('{') => {
                depth += 1;
                i += 1;
                continue;
            }
            Tok::Punct('}') => {
                depth = depth.saturating_sub(1);
                if inline.last().is_some_and(|(_, outer)| *outer == depth) {
                    inline.pop();
                }
                i += 1;
                continue;
            }
            Tok::Word(word) => word.as_str(),
            _ => {
                i += 1;
                continue;
            }
        };
        let start = out.len();
        match word {
            // `mod x;` khai một module file; chỉ `mod x {` mới mở một module
            // viết lồng.
            "mod" if is_punct(toks, i + 2, '{') => {
                if let Some(name) = next_word(toks, i + 1) {
                    inline.push((name.to_string(), depth));
                }
                depth += 1;
                i += 3;
                continue;
            }
            "use" => i = tree(toks, i + 1, "", &mut out),
            // `extern crate serde;`, `extern crate foo as bar;` — cách viết cũ,
            // vẫn gặp.
            "extern" if next_word(toks, i + 1) == Some("crate") => {
                if let Some(name) = next_word(toks, i + 2) {
                    let bound = match next_word(toks, i + 3) {
                        Some("as") => next_word(toks, i + 4),
                        _ => Some(name),
                    };
                    out.push(Leaf {
                        path: name.to_string(),
                        name: bound.filter(|b| *b != "_").map(str::to_string),
                        line: toks[i].line,
                        top: false,
                    });
                }
                i += 3;
            }
            _ => i += 1,
        }
        let names: Vec<&str> = inline.iter().map(|(n, _)| n.as_str()).collect();
        for leaf in &mut out[start..] {
            leaf.path = from_file_module(&leaf.path, &names);
            leaf.top = depth == 0;
        }
    }
    out
}

/// Đường dẫn viết bên trong các `mod` lồng `inline`, đổi về đường dẫn tính từ
/// module của chính file.
///
/// Mỗi `super` đi ra một tầng lồng trước, hết tầng lồng mới tới module cha của
/// file. `self` là tầng lồng trong cùng — không có file riêng, nên về sau nó
/// phân giải ra chính file này.
fn from_file_module(path: &str, inline: &[&str]) -> String {
    if inline.is_empty() {
        return path.to_string();
    }
    let segs: Vec<&str> = path.split("::").collect();
    let (supers, rest) = match segs[0] {
        "super" => {
            let n = segs.iter().take_while(|s| **s == "super").count();
            (n, &segs[n..])
        }
        "self" => (0, &segs[1..]),
        _ => return path.to_string(),
    };
    let out: Vec<&str> = if supers > inline.len() {
        std::iter::repeat_n("super", supers - inline.len()).chain(rest.iter().copied()).collect()
    } else {
        std::iter::once("self")
            .chain(inline[..inline.len() - supers].iter().copied())
            .chain(rest.iter().copied())
            .collect()
    };
    out.join("::")
}

/// Một cây `use` bắt đầu ở `i`, nối sau `prefix`. Trả về vị trí ngay sau cây.
fn tree(toks: &[Token], mut i: usize, prefix: &str, out: &mut Vec<Leaf>) -> usize {
    // `use ::std::io` — hai chấm dẫn đầu chỉ nói "crate ngoài".
    if is_punct(toks, i, ':') && is_punct(toks, i + 1, ':') {
        i += 2;
    }
    let Some(tok) = toks.get(i) else { return i };
    let word = match &tok.tok {
        Tok::Punct('{') => return group(toks, i, prefix, out),
        Tok::Word(w) => w.as_str(),
        _ => return i + 1,
    };
    let line = tok.line;
    // `{self}` là chính tiền tố.
    let path = match (word, prefix.is_empty()) {
        ("self", false) => prefix.to_string(),
        (_, true) => word.to_string(),
        (_, false) => format!("{prefix}::{word}"),
    };

    if is_punct(toks, i + 1, ':') && is_punct(toks, i + 2, ':') {
        match toks.get(i + 3).map(|t| &t.tok) {
            Some(Tok::Punct('{')) => return group(toks, i + 3, &path, out),
            Some(Tok::Punct('*')) => {
                out.push(Leaf { path, name: None, line, top: false });
                return i + 4;
            }
            _ => {}
        }
    }
    if next_word(toks, i + 1) == Some("as") {
        let alias = next_word(toks, i + 2);
        out.push(Leaf { path, name: alias.filter(|a| *a != "_").map(str::to_string), line, top: false });
        return i + 3;
    }
    let name = path.rsplit("::").next().filter(|n| !n.is_empty()).map(str::to_string);
    out.push(Leaf { path, name, line, top: false });
    i + 1
}

/// `{ a, b::{c, d}, e as f }` mở ở dấu `{` tại `open`.
fn group(toks: &[Token], open: usize, prefix: &str, out: &mut Vec<Leaf>) -> usize {
    let mut i = open + 1;
    while let Some(t) = toks.get(i) {
        match &t.tok {
            Tok::Punct('}') => return i + 1,
            Tok::Punct(',') => i += 1,
            // Luôn tiến về phía trước, kể cả gặp token không hiểu.
            _ => i = tree(toks, i, prefix, out).max(i + 1),
        }
    }
    i
}

fn is_punct(toks: &[Token], at: usize, c: char) -> bool {
    toks.get(at).map(|t| &t.tok) == Some(&Tok::Punct(c))
}

fn next_word(toks: &[Token], at: usize) -> Option<&str> {
    match toks.get(at).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    }
}
