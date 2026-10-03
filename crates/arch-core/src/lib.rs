//! Lõi của archctl: đọc file luật kiến trúc và đối chiếu mã nguồn với nó.
//!
//! Sơ đồ kiến trúc là luật. Crate này biến luật dạng text thành mô hình mà bộ
//! kiểm chứng và app sơ đồ cùng dùng. Không có dependency ngoài std — công cụ
//! này chạy trong repo của người dùng và trong vòng lặp của agent, nên mỗi
//! dependency thêm vào là một bề mặt tấn công thêm.

pub mod baseline;
pub mod cbuild;
pub mod check;
pub mod discover;
pub mod export;
pub mod diff;
pub mod diag;
pub mod glob;
pub mod json;
pub mod jsonc;
pub mod layout;
pub mod model;
pub mod parser;
pub mod protocol;
pub mod resolve;
pub mod scan;
pub mod validate;
pub mod verify;

pub use diag::{Diag, Severity};
pub use model::{Document, Owner, Status};

use std::path::{Path, PathBuf};

/// Đường dẫn file luật, tính từ gốc dự án.
pub const ARCH_FILE: &str = ".arch/system.arch";
pub const IGNORE_FILE: &str = ".arch/.archignore";

/// Ghi file một cách nguyên tử: ra file tạm cùng thư mục, rồi đổi tên.
///
/// `fs::write` cắt file về rỗng trước khi ghi nội dung mới. Ai đọc đúng khoảng
/// giữa đó sẽ thấy một file luật rỗng hoặc dở dang — và "ai đó" ở đây là agent
/// đang chạy `archctl check`, hay `archctl serve` đang theo dõi file, hay một
/// lệnh `archctl` khác chạy song song. Đổi tên trong cùng một hệ thống file là
/// thao tác nguyên tử, nên người đọc chỉ thấy bản cũ hoặc bản mới.
///
/// File tạm mang số hiệu tiến trình và thời điểm, để hai tiến trình ghi cùng
/// lúc không giẫm lên file tạm của nhau.
pub fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let stem = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let tmp = dir.join(format!(".{stem}.{}.{nanos}.tmp", std::process::id()));
    std::fs::write(&tmp, body)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Bỏ dấu BOM đầu file.
///
/// Trình soạn thảo trên Windows hay ghi UTF-8 kèm BOM. Ba byte đó dính vào
/// token đầu tiên và làm mọi bộ đọc hỏng ngay dòng 1 — tệ nhất là thông báo
/// lỗi khi đó tự mâu thuẫn: "từ khoá không nhận ra: `component`".
///
/// Áp ở từng bộ đọc chứ không ở từng chỗ gọi `read_to_string`: như vậy nó
/// đúng cả khi chuỗi tới từ nơi khác, không chỉ từ đĩa.
pub fn strip_bom(src: &str) -> &str {
    src.strip_prefix('\u{feff}').unwrap_or(src)
}

/// Đọc nguồn rồi kiểm tra luôn. Chẩn đoán của cả hai bước gộp làm một.
pub fn load(src: &str) -> (Document, Vec<Diag>) {
    let (doc, mut diags) = parser::parse(src);
    diags.extend(validate::validate(&doc));
    diags.sort_by_key(|d| d.line);
    (doc, diags)
}

/// Dấu vân tay của luật. Mọi đầu ra đều in nó ra, để agent biết kiến trúc đã
/// đổi kể từ lần kiểm trước và cần đọc lại.
///
/// Băm **mô hình đã chuẩn hoá**, không phải văn bản: thêm một dòng chú thích
/// hay đổi thứ tự khai báo thì hash phải giữ nguyên. Báo "kiến trúc đã đổi"
/// khi không có gì đổi là kêu sói, và agent sẽ học cách phớt lờ cảnh báo.
pub fn arch_hash(doc: &Document) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in doc.canonical().bytes() {
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
    let text = std::fs::read_to_string(root.join(IGNORE_FILE))
        .unwrap_or_default();
    strip_bom(&text)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{arch_hash, load};

    fn hash(src: &str) -> String {
        arch_hash(&load(src).0)
    }

    #[test]
    fn hash_doi_khi_luat_doi() {
        assert_ne!(hash("component A at a/**"), hash("component B at b/**"));
        assert_eq!(hash("component A at a/**").len(), 8);
    }

    #[test]
    fn hash_khong_doi_vi_chu_thich_hay_khoang_trang() {
        let a = "component A at a/**\ncomponent B at b/**\n";
        let b = "# ghi chú mới\ncomponent A    at a/**\n\ncomponent B at b/**\n";
        assert_eq!(hash(a), hash(b), "chú thích và khoảng trắng không phải là luật");
    }

    #[test]
    fn hash_khong_doi_vi_thu_tu_khai_bao() {
        let a = "component A at a/**\ncomponent B at b/**\n";
        let b = "component B at b/**\ncomponent A at a/**\n";
        assert_eq!(hash(a), hash(b));
    }

    #[test]
    fn hash_doi_khi_trang_thai_doi() {
        let a = "component A at a/** #accepted";
        let b = "component A at a/** #locked";
        assert_ne!(hash(a), hash(b), "đổi trạng thái là đổi luật");
    }
}
