//! Khớp glob cho mẫu trong `at`. Đủ dùng cho đường dẫn, không nhiều hơn.
//!
//! `**` khớp số đoạn đường dẫn bất kỳ kể cả không có đoạn nào, `*` khớp trong
//! một đoạn, `?` khớp một ký tự. Đường dẫn luôn dùng `/` kể cả trên Windows.

/// Khớp mẫu với đường dẫn tương đối so với gốc dự án.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.trim_matches('/').split('/').collect();
    let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
    match_segments(&pat, &seg)
}

/// Đúng khi đường dẫn khớp bất kỳ mẫu nào trong danh sách.
pub fn matches_any(patterns: &[String], path: &str) -> bool {
    patterns.iter().any(|p| matches(p, path))
}

fn match_segments(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => (0..=path.len()).any(|skip| match_segments(rest, &path[skip..])),
        Some((head, rest)) => match path.split_first() {
            Some((first, tail)) => match_one(head, first) && match_segments(rest, tail),
            None => false,
        },
    }
}

fn match_one(pattern: &str, segment: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = segment.chars().collect();
    match_chars(&p, &s)
}

fn match_chars(pat: &[char], s: &[char]) -> bool {
    match pat.split_first() {
        None => s.is_empty(),
        Some(('*', rest)) => (0..=s.len()).any(|i| match_chars(rest, &s[i..])),
        Some(('?', rest)) => !s.is_empty() && match_chars(rest, &s[1..]),
        Some((c, rest)) => match s.split_first() {
            Some((first, tail)) if first == c => match_chars(rest, tail),
            _ => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn hai_sao_khop_nhieu_doan() {
        assert!(matches("src/web/**", "src/web/page.ts"));
        assert!(matches("src/web/**", "src/web/a/b/c.ts"));
        assert!(matches("src/web/**", "src/web"));
        assert!(!matches("src/web/**", "src/order/page.ts"));
    }

    #[test]
    fn mot_sao_khong_vuot_qua_gach_cheo() {
        assert!(matches("src/*.ts", "src/main.ts"));
        assert!(!matches("src/*.ts", "src/web/main.ts"));
    }

    #[test]
    fn hai_sao_o_giua() {
        assert!(matches("src/**/*.test.ts", "src/web/page.test.ts"));
        assert!(matches("src/**/*.test.ts", "src/page.test.ts"));
        assert!(!matches("src/**/*.test.ts", "src/web/page.ts"));
    }

    #[test]
    fn dau_hoi_khop_mot_ky_tu() {
        assert!(matches("src/v?.ts", "src/v1.ts"));
        assert!(!matches("src/v?.ts", "src/v12.ts"));
    }
}
