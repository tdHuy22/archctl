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
#[cfg(test)]
pub(crate) mod testclock;

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
///
/// 128 bit đầu của SHA-256: biên nhận `verify` và `diff` dựa vào nó, nên phải
/// không dò được bản luật khác cùng hash. FNV gập 32 bit thì vài phút trên
/// laptop (claude-security F8, F10).
pub fn arch_hash(doc: &Document) -> String {
    let mut digest = protocol::sha256::hex(doc.canonical().as_bytes());
    digest.truncate(32);
    digest
}

/// Tìm gốc dự án: thư mục gần nhất đi lên có chứa `.arch/system.arch`.
///
/// Dừng ở ranh giới tin cậy, vì `verify` chạy lệnh trong file luật tìm được:
/// không đi lên quá gốc repo git (thư mục có `.git`), và chỉ nhận file luật ở
/// thư mục tổ tiên khi nó là của chính người chạy (hay root) và không phải
/// chỗ ai cũng ghi được. Người dùng khác trên máy đặt `/tmp/.arch/system.arch`
/// thì `archctl verify` chạy trong `/tmp/work` từng chạy lệnh của họ
/// (claude-security F15). Thư mục người dùng tự chỉ (`--root`, thư mục hiện
/// tại) là lựa chọn của họ, nên không kiểm: checkout trên ổ mà mọi thứ hiện
/// quyền 0777 (WSL `/mnt/c`) vẫn chạy được với `--root`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    let start = start.canonicalize().ok()?;
    let mut dir = start.clone();
    loop {
        if dir.join(ARCH_FILE).is_file() {
            return (dir == start || trusted_root(&dir)).then_some(dir);
        }
        if dir.join(".git").exists() || !dir.pop() {
            return None;
        }
    }
}

/// Gốc, `.arch/` và file luật đều là của người chạy hay root, và không phải
/// thứ mọi người ghi được: như `safe.directory` của git.
#[cfg(unix)]
fn trusted_root(dir: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    // SAFETY: `geteuid` không nhận đối số, không bao giờ lỗi.
    let me = unsafe { geteuid() };
    [dir.to_path_buf(), dir.join(".arch"), dir.join(ARCH_FILE)]
        .iter()
        .all(|p| std::fs::metadata(p).is_ok_and(|m| m.mode() & 0o002 == 0 && (m.uid() == me || m.uid() == 0)))
}

#[cfg(not(unix))]
fn trusted_root(_dir: &Path) -> bool {
    true
}

/// Một file cấu hình do repo kiểm soát (`Cargo.toml`, `go.mod`, `tsconfig.json`,
/// `.archignore`…): chỉ file thường, không qua symlink, tối đa trần cỡ file
/// của bộ quét. Git commit được symlink, nên `Cargo.toml -> /dev/zero` từng làm
/// mọi lần `check` đọc mãi không dừng (claude-security F14).
pub fn read_config(path: &Path) -> Option<String> {
    use std::io::Read as _;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > check::MAX_FILE_BYTES {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path).ok()?.take(check::MAX_FILE_BYTES + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= check::MAX_FILE_BYTES).then_some(text)
}

/// Đọc một file sắp được ghi lại. Chưa có thì rỗng; có mà [`read_config`]
/// không nhận (symlink, không phải file thường, quá cỡ) thì lỗi, để nơi gọi
/// không ghi đè nội dung nó chưa đọc được (review opus: `system.arch` là
/// symlink từng bị thay bằng file chỉ còn luật vừa duyệt).
pub fn read_for_update(path: &Path) -> Result<String, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("không đọc được `{}`: {e}", path.display())),
        Ok(_) => read_config(path).ok_or_else(|| {
            format!("`{}` là symlink, không phải file thường hay quá 4 MB: không ghi đè", path.display())
        }),
    }
}

/// Mẫu glob trong `.arch/.archignore`. Thiếu file thì danh sách rỗng.
pub fn read_ignores(root: &Path) -> Vec<String> {
    let text = read_config(&root.join(IGNORE_FILE)).unwrap_or_default();
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
        assert_eq!(hash("component A at a/**").len(), 32);
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
