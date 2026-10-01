//! Xuất JSON cho agent đọc. Viết tay vì chỉ cần escape chuỗi.

/// Escape một chuỗi thành literal JSON, kèm hai dấu ngoặc kép.
pub fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Mảng các chuỗi.
pub fn strings<'a>(items: impl IntoIterator<Item = &'a str>) -> String {
    let body: Vec<String> = items.into_iter().map(string).collect();
    format!("[{}]", body.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_dau_ngoac_va_xuong_dong() {
        assert_eq!(string("a\"b\\c\nd"), r#""a\"b\\c\nd""#);
    }

    #[test]
    fn giu_nguyen_utf8() {
        assert_eq!(string("vi phạm"), "\"vi phạm\"");
    }
}
