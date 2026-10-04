//! Python.
//!
//! Lexer đã gộp `a.b.c` thành một từ nhờ `path_separator`. Import của Python
//! theo dòng, nên ranh giới câu lệnh được xác định bằng **dòng logic** của
//! token: dòng vật lý, cộng các dòng nối bằng `\` — lexer không phát ra token
//! xuống dòng, nhưng có phát ra dấu `\` đứng ngoài chuỗi. Trong ngoặc đơn thì
//! xuống dòng cũng không kết thúc câu lệnh; phần đó tự đếm ngoặc ở chỗ cần.

use super::Import;
use super::lexer::{Tok, Token};

pub fn extract(toks: &[Token]) -> Vec<Import> {
    let ll = logical_lines(toks);
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let Tok::Word(word) = &toks[i].tok else {
            i += 1;
            continue;
        };
        match word.as_str() {
            "from" => i = from_import(toks, &ll, i, &mut out),
            // `cimport` của Cython đọc như `import`.
            "import" | "cimport" => i = plain_import(toks, &ll, i, &mut out),
            _ => i += 1,
        }
    }
    out
}

/// Số thứ tự dòng logic của từng token. Sang dòng vật lý mới là sang dòng
/// logic mới, trừ khi token trước là `\` nối dòng.
fn logical_lines(toks: &[Token]) -> Vec<usize> {
    let mut out = Vec::with_capacity(toks.len());
    let mut id = 0usize;
    for (k, t) in toks.iter().enumerate() {
        if let Some(prev) = k.checked_sub(1).map(|p| &toks[p])
            && t.line != prev.line
            && prev.tok != Tok::Punct('\\')
        {
            id += 1;
        }
        out.push(id);
    }
    out
}

fn is_continuation(toks: &[Token], i: usize) -> bool {
    toks.get(i).is_some_and(|t| t.tok == Tok::Punct('\\'))
}

/// `from a.b import c` · `from . import c` · `from ..pkg import (c, d)`
///
/// Chỉ là import khi ngay sau đường dẫn module là `import`. `yield from x` và
/// `raise E from e` cũng có `from`: coi chúng là import thì vừa sinh import
/// giả, vừa bỏ qua "danh sách tên" kéo sang câu lệnh sau và nuốt mất nó.
fn from_import(toks: &[Token], ll: &[usize], start: usize, out: &mut Vec<Import>) -> usize {
    let line = toks[start].line;
    let id = ll[start];
    let here = |i: usize| i < toks.len() && ll[i] == id;
    let mut i = start + 1;
    let mut spec = String::new();

    while here(i) && (toks[i].tok == Tok::Punct('.') || is_continuation(toks, i)) {
        if toks[i].tok == Tok::Punct('.') {
            spec.push('.');
        }
        i += 1;
    }
    if here(i)
        && let Tok::Word(w) = &toks[i].tok
        && w != "import"
        && w != "cimport"
    {
        spec.push_str(w);
        i += 1;
    }
    while here(i) && is_continuation(toks, i) {
        i += 1;
    }
    let is_import =
        here(i) && matches!(&toks[i].tok, Tok::Word(w) if w == "import" || w == "cimport");
    if spec.is_empty() || !is_import {
        return start + 1;
    }
    let mut import = Import::local(spec, line);
    let end = read_names(toks, ll, i + 1, id, &mut import.names);
    out.push(import);
    end
}

/// Đọc danh sách tên sau `import`, tới hết câu lệnh: hết dòng logic hoặc tới
/// dấu `;`. Trong ngoặc đơn thì xuống dòng không tính. Tên sau `as` là bí
/// danh, không phải thứ được nhập; `*` không nhập tên cụ thể nào.
fn read_names(
    toks: &[Token],
    ll: &[usize],
    start: usize,
    id: usize,
    names: &mut Vec<String>,
) -> usize {
    let mut i = start;
    let mut depth = 0usize;
    let mut alias = false;
    while let Some(t) = toks.get(i) {
        if depth == 0 && (ll[i] != id || t.tok == Tok::Punct(';')) {
            return i;
        }
        match &t.tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => depth = depth.saturating_sub(1),
            Tok::Word(w) if w == "as" => alias = true,
            Tok::Word(w) if !alias => names.push(w.clone()),
            Tok::Word(_) => alias = false,
            _ => {}
        }
        i += 1;
    }
    i
}

/// `import a.b` · `import a.b as c, d.e` · `import a, \` xuống dòng `b`
fn plain_import(toks: &[Token], ll: &[usize], start: usize, out: &mut Vec<Import>) -> usize {
    let line = toks[start].line;
    let id = ll[start];
    let mut i = start + 1;
    let mut expect_module = true;

    while let Some(t) = toks.get(i) {
        if ll[i] != id {
            break;
        }
        match &t.tok {
            Tok::Punct('\\') => {}
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
