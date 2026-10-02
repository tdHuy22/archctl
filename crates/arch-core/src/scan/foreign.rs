//! Cạnh phụ thuộc đi qua ranh giới ngôn ngữ.
//!
//! Phân tích import chỉ thấy phụ thuộc trong cùng một ngôn ngữ. Rust gọi C
//! không qua câu lệnh import nào, nên cạnh đó vô hình — và trong
//! `examples/sensor` nó che đúng một vi phạm thật.
//!
//! Nhưng một phần lớn FFI viết tay là **khai báo tường minh**, không phải suy
//! đoán:
//!
//! ```text
//! // Rust gọi ra ngoài            // C cung cấp
//! extern "C" { fn parse(…); }     int parse(const char *b) { … }
//! ```
//!
//! Hai đầu mang cùng một cái tên, và cái tên đó phải khớp thì chương trình mới
//! liên kết được. Ghép chúng lại là ra cạnh, không cần đọc binary và không cần
//! bảng ký hiệu.
//!
//! Những gì **không** nhận ra: `dlopen` và tên hàm dựng lúc chạy, con trỏ hàm,
//! IPC, tiến trình con. Những thứ đó vẫn cần `api` — xem `docs/api.md`.

use super::lexer::{Tok, Token};
use super::{Lang, SymbolHit};

/// Hàm đi qua ranh giới ngôn ngữ, nhìn từ một file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Foreign {
    /// Tên file này gọi nhưng không định nghĩa: `extern "C" { fn x; }` của
    /// Rust, `C.x` của Go.
    pub calls: Vec<SymbolHit>,
    /// Tên file này đưa ra cho ngôn ngữ khác gọi: định nghĩa hàm trong C, và
    /// `pub extern "C" fn x` của Rust.
    pub provides: Vec<SymbolHit>,
}

impl Foreign {
    fn is_empty(&self) -> bool {
        self.calls.is_empty() && self.provides.is_empty()
    }
}

pub fn scan(toks: &[Token], lang: Lang) -> Foreign {
    let found = match lang {
        Lang::Rust => rust(toks),
        Lang::Go => go(toks),
        Lang::C => c(toks),
        _ => Foreign::default(),
    };
    debug_assert!(found.is_empty() || !toks.is_empty());
    found
}

fn hit(name: &str, line: usize) -> SymbolHit {
    SymbolHit { symbol: name.to_string(), line, defines: false }
}

// ---- Rust ----

/// `extern "C" { … }` khai báo hàm ở ngoài; `extern "C" fn x()` định nghĩa một
/// hàm cho ngoài gọi. Hai chiều ngược nhau, phân biệt bằng token ngay sau ABI.
fn rust(toks: &[Token]) -> Foreign {
    let mut out = Foreign::default();
    for i in 0..toks.len() {
        if !matches!(&toks[i].tok, Tok::Word(w) if w == "extern") {
            continue;
        }
        // ABI nào cũng được, không riêng "C": "system", "cdecl", "stdcall".
        let Some(Tok::Str(_)) = toks.get(i + 1).map(|t| &t.tok) else { continue };
        match toks.get(i + 2).map(|t| &t.tok) {
            Some(Tok::Punct('{')) => {
                for j in i + 3..block_end(toks, i + 2) {
                    if matches!(&toks[j].tok, Tok::Word(w) if w == "fn")
                        && let Some(Token { tok: Tok::Word(name), line }) = toks.get(j + 1)
                    {
                        out.calls.push(hit(name, *line));
                    }
                }
            }
            Some(Tok::Word(w)) if w == "fn" => {
                if let Some(Token { tok: Tok::Word(name), line }) = toks.get(i + 3) {
                    out.provides.push(hit(name, *line));
                }
            }
            _ => {}
        }
    }
    out
}

/// Vị trí dấu `}` đóng lại dấu `{` ở `open`, hoặc hết dãy token.
fn block_end(toks: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.tok {
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    toks.len()
}

// ---- Go ----

/// cgo: `import "C"` rồi gọi `C.ten_ham`. Những tên không phải hàm của dự án
/// (`C.int`, `C.CString`) cũng lọt vào, nhưng chúng không khớp định nghĩa nào
/// trong repo nên tự rụng ở bước ghép.
fn go(toks: &[Token]) -> Foreign {
    let imports_c = toks.windows(2).any(|w| {
        matches!(&w[0].tok, Tok::Word(k) if k == "import")
            && matches!(&w[1].tok, Tok::Str(s) if s == "C")
    });
    if !imports_c {
        return Foreign::default();
    }
    let mut out = Foreign::default();
    for w in toks.windows(3) {
        if matches!(&w[0].tok, Tok::Word(p) if p == "C")
            && matches!(w[1].tok, Tok::Punct('.'))
            && let Tok::Word(name) = &w[2].tok
        {
            out.calls.push(hit(name, w[2].line));
        }
    }
    out
}

// ---- C và C++ ----

/// Từ khoá theo sau là `(` nhưng không phải lời gọi hàm.
const C_CONTROL: &[&str] =
    &["if", "while", "for", "switch", "return", "sizeof", "catch", "defined", "static_assert"];

/// Định nghĩa hàm ở mức ngoài cùng: `kiểu ten(tham số) {`.
///
/// Chỉ xét khi độ sâu ngoặc nhọn bằng 0, nên lời gọi hàm trong thân hàm khác
/// không bị nhận nhầm. Khai báo trước (`int f(void);`) cũng không, vì nó kết
/// thúc bằng `;` chứ không phải `{`.
fn c(toks: &[Token]) -> Foreign {
    let mut out = Foreign::default();
    let mut depth = 0usize;
    let mut i = 0;
    while i < toks.len() {
        match toks[i].tok {
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => depth = depth.saturating_sub(1),
            _ => {}
        }
        if depth > 0 {
            i += 1;
            continue;
        }
        let Tok::Word(name) = &toks[i].tok else {
            i += 1;
            continue;
        };
        if C_CONTROL.contains(&name.as_str())
            || !matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Punct('(')))
        {
            i += 1;
            continue;
        }
        // Phải có kiểu trả về đứng trước, nếu không thì đây là lời gọi ở mức
        // ngoài cùng — chẳng hạn trong một macro.
        let has_type = matches!(
            toks.get(i.wrapping_sub(1)).map(|t| &t.tok),
            Some(Tok::Word(_) | Tok::Punct('*'))
        ) && i > 0;
        let close = paren_end(toks, i + 1);
        if has_type && matches!(toks.get(close + 1).map(|t| &t.tok), Some(Tok::Punct('{'))) {
            out.provides.push(hit(name, toks[i].line));
        }
        i = close.max(i) + 1;
    }
    out
}

fn paren_end(toks: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    toks.len()
}
