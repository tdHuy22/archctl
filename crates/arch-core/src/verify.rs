//! Biên nhận cho các cách kiểm đã chạy.
//!
//! `contract`, `boundary` và `invariant` đều có trường `verify`. Trước đây nó
//! trỏ tới một cái tên không dẫn tới đâu cả, và hệ thống cảnh báo "thiếu
//! `verify` thì đây chỉ là lời hứa" — ngụ ý rằng **có** `verify` thì không
//! phải. Nhưng chưa có gì chạy, nên có cũng vẫn là lời hứa, chỉ khác là không
//! còn ai nhắc.
//!
//! Biên nhận gắn kết quả với **trạng thái mã lúc chạy**. Một lần chạy đạt từ
//! trước khi sửa mã không chứng minh được gì cho mã hiện tại, nên nó phải tự
//! hết hạn.

use std::path::Path;

pub const RECEIPTS_FILE: &str = ".arch/.verified";

/// Một lần chạy đã xong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub name: String,
    pub passed: bool,
    /// Mili giây kể từ mốc Unix.
    ///
    /// Giây thì không đủ: sửa mã rồi chạy verify ngay là chuyện thường, và
    /// khi cả hai rơi vào cùng một giây thì không phân biệt được cái nào
    /// trước — biên nhận vừa ghi xong đã bị coi là cũ.
    pub at: u64,
    /// Dấu vân tay của luật lúc chạy.
    pub arch_hash: String,
    /// Băm nội dung mọi file Terraform lúc bắt đầu chạy (`None` khi repo không
    /// có). Plan JSON do lệnh này ghi chỉ được dùng khi băm còn khớp.
    pub tf: Option<String>,
    /// File plan Terraform mà chính lệnh này ghi (tạo mới hay ghi lại trong
    /// lúc chạy, đường dẫn nêu trong lệnh), kèm băm nội dung lấy sau khi chạy
    /// xong. File có sẵn mà lệnh không đụng tới thì không có ở đây (TM-57).
    pub plans: Vec<(String, String)>,
    /// Băm mã module tải về (`.terraform/modules`) lấy sau khi lệnh ghi plan
    /// chạy xong.
    pub mods: Option<String>,
}

/// Vì sao một biên nhận không còn chứng minh được gì.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stale {
    /// Chưa chạy lần nào.
    Never,
    /// Lần chạy cuối không đạt.
    Failed,
    /// Luật đã đổi sau lần chạy cuối.
    ArchChanged,
    /// Mã nguồn đã đổi sau lần chạy cuối.
    CodeChanged,
}

impl Stale {
    pub fn describe(self) -> &'static str {
        match self {
            Stale::Never => "chưa chạy lần nào",
            Stale::Failed => "lần chạy gần nhất không đạt",
            Stale::ArchChanged => "luật đã đổi sau lần chạy gần nhất",
            Stale::CodeChanged => "mã đã đổi sau lần chạy gần nhất",
        }
    }
}

pub fn parse(src: &str) -> Vec<Receipt> {
    crate::strip_bom(src)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            Some(Receipt {
                name: parts.next()?.to_string(),
                passed: parts.next()? == "đạt",
                at: parts.next()?.parse().ok()?,
                arch_hash: parts.next()?.to_string(),
                tf: None,
                plans: Vec::new(),
                mods: None,
            })
            .map(|mut r: Receipt| {
                for p in line.split_whitespace().skip(4) {
                    if let Some(v) = p.strip_prefix("tf=") {
                        r.tf = Some(v.to_string());
                    } else if let Some(v) = p.strip_prefix("plan=") {
                        if let Some((path, hash)) = v.rsplit_once('#') {
                            r.plans.push((path.to_string(), hash.to_string()));
                        }
                    } else if let Some(v) = p.strip_prefix("mods=") {
                        r.mods = Some(v.to_string());
                    }
                }
                r
            })
        })
        .collect()
}

pub fn render(receipts: &[Receipt]) -> String {
    let mut out = String::from(
        "# archctl sinh tự động. Không cần đưa vào git.\n\
         # <tên> <đạt|hỏng> <mili giây unix> <dấu vân tay luật> [tf=<băm .tf>] [plan=<file>#<băm>]… [mods=<băm>]\n",
    );
    let mut sorted: Vec<&Receipt> = receipts.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for r in sorted {
        let verdict = if r.passed { "đạt" } else { "hỏng" };
        out.push_str(&format!("{} {verdict} {} {}", r.name, r.at, r.arch_hash));
        if let Some(tf) = &r.tf {
            out.push_str(&format!(" tf={tf}"));
        }
        for (path, hash) in &r.plans {
            out.push_str(&format!(" plan={path}#{hash}"));
        }
        if let Some(mods) = &r.mods {
            out.push_str(&format!(" mods={mods}"));
        }
        out.push('\n');
    }
    out
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Lần sửa gần nhất trong mã nguồn, tính bằng mili giây unix.
///
/// Một biên nhận đạt từ trước lần sửa đó không nói gì về mã hiện tại.
pub fn last_source_change(root: &Path, files: &[String]) -> u64 {
    files
        .iter()
        .filter_map(|f| std::fs::metadata(root.join(f)).ok())
        .filter_map(|m| m.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .max()
        .unwrap_or(0)
}

/// Cách kiểm nào không còn chứng minh được gì, và vì sao.
pub fn stale(
    names: &[String],
    receipts: &[Receipt],
    arch_hash: &str,
    last_change: u64,
) -> Vec<(String, Stale)> {
    let mut out: Vec<(String, Stale)> = names
        .iter()
        .filter_map(|name| {
            let reason = match receipts.iter().find(|r| &r.name == name) {
                None => Stale::Never,
                Some(r) if !r.passed => Stale::Failed,
                Some(r) if r.arch_hash != arch_hash => Stale::ArchChanged,
                Some(r) if r.at < last_change => Stale::CodeChanged,
                Some(_) => return None,
            };
            Some((name.clone(), reason))
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(name: &str, passed: bool, at: u64, hash: &str) -> Receipt {
        Receipt { name: name.into(), passed, at, arch_hash: hash.into(), tf: None, plans: Vec::new(), mods: None }
    }

    #[test]
    fn ghi_roi_doc_lai_duoc_nguyen_ven() {
        let rs = vec![receipt("b", true, 42, "aaaa"), receipt("a", false, 7, "bbbb")];
        let back = parse(&render(&rs));
        assert_eq!(back.len(), 2);
        assert_eq!(back[0], receipt("a", false, 7, "bbbb"));
        // Băm Terraform đi kèm khi có; biên nhận cũ không có cột đó vẫn đọc được.
        let with_tf = Receipt { tf: Some("0123abcd".into()), plans: vec![(".arch/tf-plan.json".into(), "ffee".into())], mods: Some("aa".into()), ..receipt("p", true, 9, "cccc") };
        assert_eq!(parse(&render(std::slice::from_ref(&with_tf))), vec![with_tf]);
        assert_eq!(parse("x đạt 1 aaaa\n")[0].tf, None);
    }

    #[test]
    fn dong_hong_bi_bo_qua_chu_khong_lam_sap() {
        assert!(parse("rác\nthiếu cột\n").is_empty());
    }

    #[test]
    fn moi_ly_do_het_han() {
        let names = vec!["a".to_string()];
        let fresh = vec![receipt("a", true, 100, "h1")];
        assert_eq!(stale(&names, &[], "h1", 0), [("a".into(), Stale::Never)]);
        assert_eq!(
            stale(&names, &[receipt("a", false, 100, "h1")], "h1", 0),
            [("a".into(), Stale::Failed)]
        );
        assert_eq!(stale(&names, &fresh, "h2", 0), [("a".into(), Stale::ArchChanged)]);
        assert_eq!(stale(&names, &fresh, "h1", 150), [("a".into(), Stale::CodeChanged)]);
        assert!(stale(&names, &fresh, "h1", 50).is_empty());
    }
}
