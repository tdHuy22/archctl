//! Rust.
//!
//! Khó hơn các ngôn ngữ còn lại vì `use crate::a::b::c` là đường dẫn **logic**
//! trong cây module, không phải đường dẫn file. Phần đoán xem đoạn nào là
//! module và đoạn nào là tên item thuộc về bước phân giải; ở đây chỉ trích ra
//! đường dẫn đầy đủ.
//!
//! Lexer đã gộp `a::b::c` thành một từ nhờ `path_separator`, nên `use` với
//! ngoặc nhọn là trường hợp duy nhất cần ghép tay.

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
            "use" => i = use_item(toks, i + 1, &mut out),
            // `extern crate serde;` — cách viết cũ, vẫn gặp.
            "extern" if next_word(toks, i + 1) == Some("crate") => {
                if let Some(name) = next_word(toks, i + 2) {
                    out.push(Import::local(name, toks[i].line));
                }
                i += 3;
            }
            _ => i += 1,
        }
    }
    out
}

fn use_item(toks: &[Token], start: usize, out: &mut Vec<Import>) -> usize {
    let Some(Tok::Word(prefix)) = toks.get(start).map(|t| &t.tok) else {
        return start + 1;
    };
    let line = toks[start].line;

    // `use crate::a::{b, c::d};` — lexer dừng ở `::{`, phần còn lại là token rời.
    if toks.get(start + 1).map(|t| &t.tok) == Some(&Tok::Punct(':'))
        && toks.get(start + 3).map(|t| &t.tok) == Some(&Tok::Punct('{'))
    {
        let mut i = start + 4;
        let mut depth = 1usize;
        let mut any = false;
        while let Some(t) = toks.get(i) {
            match &t.tok {
                Tok::Punct('{') => depth += 1,
                Tok::Punct('}') => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                Tok::Word(member) if member != "as" && member != "self" => {
                    out.push(Import::local(format!("{prefix}::{member}"), t.line));
                    any = true;
                }
                _ => {}
            }
            i += 1;
        }
        // `use crate::a::{self};` không có thành viên nào: bản thân tiền tố là
        // phụ thuộc.
        if !any {
            out.push(Import::local(prefix, line));
        }
        return i + 1;
    }

    out.push(Import::local(prefix, line));
    start + 1
}

fn next_word(toks: &[Token], at: usize) -> Option<&str> {
    match toks.get(at).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    }
}
