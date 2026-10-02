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

/// Khớp tiến dần, chỉ giữ **một** điểm quay lui: điểm sao gần nhất.
///
/// Thử mọi cách chia như cách viết đệ quy tự nhiên thì số nhánh tăng theo hàm
/// mũ: `*a*a*a*a*a*a*a*a*a*a*a*a*b` đối với bốn mươi chữ `a` chạy quá mười
/// lăm giây. Mẫu là do người dùng — hoặc agent, qua `accept` — viết ra, nên
/// một mẫu vô tình treo cả `archctl check` trong vòng lặp của agent.
///
/// Một điểm quay lui là đủ, vì giữa hai dấu sao mọi thứ phải khớp đúng một
/// đoạn liên tiếp: chỉ còn một ẩn số là dấu sao trước đó ăn tới đâu.
macro_rules! wildcard_match {
    ($pat:expr, $input:expr, $star:expr, $eq:expr) => {{
        let (pat, input) = ($pat, $input);
        let (mut p, mut i) = (0usize, 0usize);
        // `(vị trí sao trong mẫu, vị trí đã thử trong đầu vào)`
        let mut backtrack: Option<(usize, usize)> = None;
        loop {
            if i < input.len() && p < pat.len() && $eq(&pat[p], &input[i]) {
                p += 1;
                i += 1;
            } else if p < pat.len() && $star(&pat[p]) {
                backtrack = Some((p, i));
                p += 1;
            } else if let Some((sp, si)) = backtrack
                && si < input.len()
            {
                // Cho dấu sao ăn thêm một đơn vị rồi thử lại từ sau nó.
                backtrack = Some((sp, si + 1));
                p = sp + 1;
                i = si + 1;
            } else {
                return false;
            }
            if i == input.len() {
                return pat[p..].iter().all(|x| $star(x));
            }
        }
    }};
}

fn match_segments(pat: &[&str], path: &[&str]) -> bool {
    // `**` khớp cả khi không có đoạn nào, nên đường dẫn rỗng vẫn có thể khớp.
    if path.is_empty() {
        return pat.iter().all(|p| *p == "**");
    }
    wildcard_match!(pat, path, |p: &&str| *p == "**", |p: &&str, s: &&str| *p != "**"
        && match_one(p, s))
}

fn match_one(pattern: &str, segment: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let s: Vec<char> = segment.chars().collect();
    match_chars(&p, &s)
}

fn match_chars(pat: &[char], s: &[char]) -> bool {
    if s.is_empty() {
        return pat.iter().all(|c| *c == '*');
    }
    wildcard_match!(pat, s, |c: &char| *c == '*', |c: &char, x: &char| *c == '?' || c == x)
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
