//! Bánh răng một chiều cho vi phạm có sẵn.
//!
//! Áp luật lên codebase đã có thì ngày đầu tiên sẽ có hàng trăm vi phạm. Bắt
//! sửa hết trước khi bật là không thực tế, mà bỏ qua hết thì công cụ vô dụng.
//!
//! Lối ra là ghi lại hiện trạng rồi chỉ chặn những gì **mới**: vi phạm cũ được
//! tha, vi phạm mới làm hỏng build. Đây là cách người ta áp `strict` của
//! TypeScript hay một luật lint mới lên dự án đang chạy.

use crate::check::Violation;
use std::collections::BTreeSet;

pub const BASELINE_FILE: &str = ".arch/baseline";

/// Danh tính của một vi phạm. Đổi tên file sẽ làm mục cũ hết hiệu lực, và đó
/// là hành vi đúng: file được viết lại thì vi phạm trong đó không còn được tha.
pub fn key(v: &Violation) -> String {
    format!("{}->{}|{}|{}", v.from, v.to, v.file, v.spec)
}

pub fn parse(src: &str) -> BTreeSet<String> {
    let src = crate::strip_bom(src);
    src.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

pub fn render(keys: &BTreeSet<String>, date: &str) -> String {
    let mut out = format!(
        "# Vi phạm đã có sẵn khi bắt đầu áp luật, ghi ngày {date}.\n\
         # Chúng được tha; vi phạm MỚI thì không. Chạy `archctl baseline` sau khi\n\
         # sửa bớt để siết lại — một khi đã bỏ khỏi đây thì không quay lại được.\n\
         # {} mục.\n",
        keys.len()
    );
    for k in keys {
        out.push_str(k);
        out.push('\n');
    }
    out
}

/// Tách vi phạm thành phần mới và phần đã được tha.
pub struct Split<'a> {
    pub fresh: Vec<&'a Violation>,
    pub grandfathered: Vec<&'a Violation>,
}

pub fn split<'a>(violations: &'a [Violation], baseline: &BTreeSet<String>) -> Split<'a> {
    let (grandfathered, fresh) =
        violations.iter().partition(|v| baseline.contains(&key(v)));
    Split { fresh, grandfathered }
}

/// Mục trong baseline không còn vi phạm nào tương ứng. Chạy `baseline` lại sẽ
/// dọn chúng, và bánh răng siết thêm một nấc.
pub fn stale(violations: &[Violation], baseline: &BTreeSet<String>) -> usize {
    let present: BTreeSet<String> = violations.iter().map(key).collect();
    baseline.difference(&present).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ghi_ra_roi_doc_lai_duoc() {
        let keys: BTreeSet<String> =
            ["A->B|src/a.ts|../b", "C->D|src/c.ts|d"].iter().map(|s| s.to_string()).collect();
        assert_eq!(parse(&render(&keys, "2026-10-02")), keys);
    }

    #[test]
    fn dong_chu_thich_va_dong_rong_bi_bo_qua() {
        assert_eq!(parse("# ghi chú\n\nA->B|f|s\n").len(), 1);
    }
}
