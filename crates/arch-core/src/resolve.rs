//! Phân giải chuỗi định danh module thành đường dẫn file hoặc tên gói.
//!
//! Đây là chỗ dễ sai nhất của cả bộ kiểm chứng. Nguyên tắc: thứ nào không phân
//! giải được thì **báo ra**, không im lặng bỏ qua. Bỏ sót âm thầm làm người
//! dùng tin rằng không có vi phạm, tệ hơn hẳn việc nói thẳng là chưa biết.

use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// File trong dự án, đường dẫn tương đối so với gốc, dùng dấu `/`.
    File(String),
    /// Gói bên ngoài. Chỉ tham gia luật khi được khai báo bằng `external`.
    Package(String),
    /// Trông như alias của tsconfig `paths`. Chưa hỗ trợ.
    Alias,
    /// Là đường dẫn tương đối nhưng không tìm thấy file nào.
    Missing,
}

/// Thứ tự thử phần mở rộng, theo cách TypeScript tự phân giải.
const TRY_EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs", "d.ts"];

/// Import `./x.js` trong TypeScript ESM thực chất trỏ tới `./x.ts`.
const JS_TO_TS: &[(&str, &[&str])] =
    &[(".js", &[".ts", ".tsx"]), (".jsx", &[".tsx"]), (".mjs", &[".mts"]), (".cjs", &[".cts"])];

pub fn resolve(root: &Path, importer: &str, spec: &str) -> Resolved {
    if spec.starts_with("./") || spec.starts_with("../") || spec == "." || spec == ".." {
        return resolve_relative(root, importer, spec);
    }
    // `@/...` và `~/...` trông như gói nhưng là alias. Gói có phạm vi luôn có
    // dạng `@scope/name`, nên phạm vi rỗng là dấu hiệu đủ rõ.
    if spec.starts_with("@/") || spec.starts_with("~/") || spec.starts_with('#') {
        return Resolved::Alias;
    }
    if spec.starts_with('/') {
        return Resolved::Alias;
    }
    Resolved::Package(package_name(spec))
}

fn resolve_relative(root: &Path, importer: &str, spec: &str) -> Resolved {
    let dir = importer.rsplit_once('/').map_or("", |(d, _)| d);
    let joined = if dir.is_empty() { spec.to_string() } else { format!("{dir}/{spec}") };
    let Some(base) = normalize(&joined) else {
        return Resolved::Missing;
    };

    for candidate in candidates(&base) {
        if root.join(&candidate).is_file() {
            return Resolved::File(candidate);
        }
    }
    Resolved::Missing
}

fn candidates(base: &str) -> Vec<String> {
    let mut out = Vec::new();
    out.push(base.to_string());
    for (from, tos) in JS_TO_TS {
        if let Some(stem) = base.strip_suffix(from) {
            out.extend(tos.iter().map(|to| format!("{stem}{to}")));
        }
    }
    out.extend(TRY_EXTENSIONS.iter().map(|ext| format!("{base}.{ext}")));
    out.extend(TRY_EXTENSIONS.iter().map(|ext| format!("{base}/index.{ext}")));
    out
}

/// Gộp `.` và `..` trong đường dẫn. Trả về `None` nếu vượt ra ngoài gốc.
fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    Some(out.join("/"))
}

/// `@scope/name/sub` → `@scope/name`; `pkg/sub` → `pkg`.
fn package_name(spec: &str) -> String {
    let mut parts = spec.split('/');
    let first = parts.next().unwrap_or(spec);
    if first.starts_with('@') {
        match parts.next() {
            Some(second) => format!("{first}/{second}"),
            None => first.to_string(),
        }
    } else {
        first.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goi_thuong_va_goi_co_pham_vi() {
        let root = Path::new("/khong-ton-tai");
        assert_eq!(resolve(root, "a.ts", "stripe"), Resolved::Package("stripe".into()));
        assert_eq!(
            resolve(root, "a.ts", "stripe/lib/webhook"),
            Resolved::Package("stripe".into())
        );
        assert_eq!(
            resolve(root, "a.ts", "@scope/pkg/sub"),
            Resolved::Package("@scope/pkg".into())
        );
    }

    #[test]
    fn alias_khong_bi_nham_la_goi() {
        let root = Path::new("/khong-ton-tai");
        assert_eq!(resolve(root, "a.ts", "@/lib/format"), Resolved::Alias);
        assert_eq!(resolve(root, "a.ts", "~/lib/format"), Resolved::Alias);
    }

    #[test]
    fn duong_dan_tuong_doi_khong_co_file_thi_bao_thieu() {
        let root = Path::new("/khong-ton-tai");
        assert_eq!(resolve(root, "src/a/b.ts", "../c/d"), Resolved::Missing);
    }

    #[test]
    fn gop_dau_cham() {
        assert_eq!(normalize("src/a/../b/./c").as_deref(), Some("src/b/c"));
        assert_eq!(normalize("src/../.."), None);
    }
}
