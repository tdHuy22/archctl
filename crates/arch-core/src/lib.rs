//! Lõi của archctl: đọc file luật kiến trúc và đối chiếu mã nguồn với nó.
//!
//! Sơ đồ kiến trúc là luật. Crate này biến luật dạng text thành mô hình mà bộ
//! kiểm chứng và app sơ đồ cùng dùng. Không có dependency ngoài std — công cụ
//! này chạy trong repo của người dùng và trong vòng lặp của agent, nên mỗi
//! dependency thêm vào là một bề mặt tấn công thêm.

pub mod check;
pub mod diag;
pub mod glob;
pub mod json;
pub mod model;
pub mod parser;
pub mod resolve;
pub mod scan;
pub mod validate;

pub use diag::{Diag, Severity};
pub use model::{Document, Owner, Status};

use std::path::{Path, PathBuf};

/// Đường dẫn file luật, tính từ gốc dự án.
pub const ARCH_FILE: &str = ".arch/system.arch";
pub const IGNORE_FILE: &str = ".arch/.archignore";

/// Đọc nguồn rồi kiểm tra luôn. Chẩn đoán của cả hai bước gộp làm một.
pub fn load(src: &str) -> (Document, Vec<Diag>) {
    let (doc, mut diags) = parser::parse(src);
    diags.extend(validate::validate(&doc));
    diags.sort_by_key(|d| d.line);
    (doc, diags)
}

/// Dấu vân tay của file luật. Mọi đầu ra đều in nó ra, để agent biết kiến trúc
/// đã đổi kể từ lần kiểm trước và cần đọc lại.
pub fn arch_hash(src: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in src.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (h as u32) ^ ((h >> 32) as u32))
}

/// Tìm gốc dự án: thư mục gần nhất đi lên có chứa `.arch/system.arch`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start.canonicalize().ok()?;
    loop {
        if dir.join(ARCH_FILE).is_file() {
            return Some(dir);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Mẫu glob trong `.arch/.archignore`. Thiếu file thì danh sách rỗng.
pub fn read_ignores(root: &Path) -> Vec<String> {
    std::fs::read_to_string(root.join(IGNORE_FILE))
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::arch_hash;

    #[test]
    fn hash_doi_khi_noi_dung_doi() {
        assert_ne!(arch_hash("component A"), arch_hash("component B"));
        assert_eq!(arch_hash("component A"), arch_hash("component A"));
        assert_eq!(arch_hash("component A").len(), 8);
    }
}
