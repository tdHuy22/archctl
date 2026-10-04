//! C và C++.
//!
//! Lexer đã nhận diện chỉ thị `#include` và phát ra cặp token `include` + chuỗi,
//! nên ở đây chỉ còn việc ghép lại.
//!
//! Chỉ thị tiền xử lý **không được diễn giải**: include nằm trong `#ifdef` vẫn
//! được tính. Đây là lựa chọn có chủ ý — phân tích phụ thuộc nên thận trọng,
//! và việc đoán xem nhánh nào được biên dịch cần biết toàn bộ cờ của build.

use super::Import;
use super::lexer::{Tok, Token};

pub fn extract(toks: &[Token]) -> Vec<Import> {
    let mut out = Vec::new();
    for pair in toks.windows(2) {
        if pair[0].tok != Tok::Word("include".into()) {
            continue;
        }
        match &pair[1].tok {
            Tok::Str(spec) if !spec.is_empty() => out.push(Import::local(spec, pair[1].line)),
            Tok::SystemStr(spec) if !spec.is_empty() => out.push(Import {
                spec: spec.clone(),
                line: pair[1].line,
                system: true,
                names: Vec::new(),
            }),
            _ => {}
        }
    }
    out
}
