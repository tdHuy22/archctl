//! Khớp glob: giữ nguyên nghĩa, bỏ thời gian chạy theo hàm mũ.
//!
//! Bản đệ quy tự nhiên thử mọi cách chia nên số nhánh tăng theo hàm mũ. Mẫu
//! là do người dùng — hoặc agent, qua `accept` — viết ra, nên một mẫu vô tình
//! treo cả `archctl check` trong vòng lặp của agent. Bản mới chỉ giữ một điểm
//! quay lui.
//!
//! Viết lại một bộ khớp là đúng chỗ để hành vi lặng lẽ đổi khác, nên bản đệ
//! quy cũ nằm lại đây làm mốc so sánh.

use arch_core::glob::matches;

// Đồng hồ thời gian CPU của luồng, dùng chung với test trong lib: máy tải nặng
// (CI, test chạy song song) không làm test thời gian trượt.
#[path = "../src/testclock.rs"]
#[allow(dead_code)]
mod testclock;

fn old(pattern: &str, path: &str) -> bool {
    fn segments(pat: &[&str], path: &[&str]) -> bool {
        match pat.split_first() {
            None => path.is_empty(),
            Some((&"**", rest)) => (0..=path.len()).any(|skip| segments(rest, &path[skip..])),
            Some((head, rest)) => match path.split_first() {
                Some((first, tail)) => one(head, first) && segments(rest, tail),
                None => false,
            },
        }
    }
    fn one(pattern: &str, segment: &str) -> bool {
        chars(&pattern.chars().collect::<Vec<_>>(), &segment.chars().collect::<Vec<_>>())
    }
    fn chars(pat: &[char], s: &[char]) -> bool {
        match pat.split_first() {
            None => s.is_empty(),
            Some(('*', rest)) => (0..=s.len()).any(|i| chars(rest, &s[i..])),
            Some(('?', rest)) => !s.is_empty() && chars(rest, &s[1..]),
            Some((c, rest)) => match s.split_first() {
                Some((first, tail)) if first == c => chars(rest, tail),
                _ => false,
            },
        }
    }
    segments(
        &pattern.trim_matches('/').split('/').collect::<Vec<_>>(),
        &path.trim_matches('/').split('/').collect::<Vec<_>>(),
    )
}

/// Bộ sinh số giả ngẫu nhiên nhỏ. Hạt cố định để lần chạy nào cũng như nhau —
/// một test lúc đổ lúc không thì không ai sửa.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, xs: &'a [&'a str]) -> &'a str {
        xs[(self.next() % xs.len() as u64) as usize]
    }
}

#[test]
fn nghia_giong_het_ban_de_quy() {
    const ROUNDS: usize = 60_000;
    let pattern_atoms =
        ["a", "b", "ab", "*", "**", "?", "a*", "*a", "*a*", "a?b", "", "*.ts", "x.ts"];
    let path_atoms = ["a", "b", "ab", "aab", "x.ts", "a.ts", "", "aaa"];
    let mut rng = Rng(0x2026_1002);

    for _ in 0..ROUNDS {
        let np = 1 + (rng.next() % 4) as usize;
        let ns = 1 + (rng.next() % 4) as usize;
        let pattern: Vec<&str> = (0..np).map(|_| rng.pick(&pattern_atoms)).collect();
        let path: Vec<&str> = (0..ns).map(|_| rng.pick(&path_atoms)).collect();
        let (pattern, path) = (pattern.join("/"), path.join("/"));
        assert_eq!(matches(&pattern, &path), old(&pattern, &path), "mẫu {pattern:?} ↔ {path:?}");
    }
}

#[test]
fn mau_nhieu_sao_khong_treo() {
    // Bản cũ chạy quá mười lăm giây trên đúng trường hợp này.
    let path = format!("src/{}.ts", "a".repeat(40));
    let pattern = format!("src/{}*b.ts", "*a".repeat(12));
    let start = testclock::Clock::start();
    assert!(!matches(&pattern, &path));
    assert!(start.elapsed().as_millis() < 100, "mất {:?}", start.elapsed());
}

#[test]
fn nhieu_hai_sao_khong_treo() {
    let path = "a/".repeat(60) + "x.ts";
    let pattern = "**/".repeat(12) + "*.rs";
    let start = testclock::Clock::start();
    assert!(!matches(&pattern, &path));
    assert!(start.elapsed().as_millis() < 100, "mất {:?}", start.elapsed());
}

#[test]
fn hai_sao_khop_ca_khi_khong_co_doan_nao() {
    assert!(matches("**", ""));
    assert!(matches("**/**", "a"));
    assert!(matches("src/**", "src"));
    assert!(!matches("src/**", ""));
}
