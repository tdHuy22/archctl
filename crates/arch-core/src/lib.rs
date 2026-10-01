//! Lõi của archctl: đọc và kiểm tra file `system.arch`.
//!
//! Sơ đồ kiến trúc là luật. Crate này biến luật dạng text thành mô hình mà
//! bộ kiểm chứng và app sơ đồ cùng dùng. Không có dependency ngoài std.

pub mod diag;
pub mod model;
pub mod parser;
pub mod validate;

pub use diag::{Diag, Severity};
pub use model::{Document, Owner, Status};

/// Đọc nguồn rồi kiểm tra luôn. Chẩn đoán của cả hai bước gộp làm một.
pub fn load(src: &str) -> (Document, Vec<Diag>) {
    let (doc, mut diags) = parser::parse(src);
    diags.extend(validate::validate(&doc));
    diags.sort_by_key(|d| d.line);
    (doc, diags)
}
