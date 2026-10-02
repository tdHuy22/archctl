//! Dựng bản nháp kiến trúc từ mã nguồn có sẵn.
//!
//! Đây là lối vào cho codebase đã có. Không có nó, người dùng đứng trước hai
//! lựa chọn đều tệ: một file luật rỗng không ràng buộc gì, hoặc gõ tay cả
//! trăm dòng trước khi thấy được giá trị nào.
//!
//! Phải nói thẳng một điều, và đầu ra cũng nói thẳng: kết quả ở đây là **mô tả
//! hiện trạng**, không phải thiết kế. Nó chép lại cả những chỗ đang lộn xộn.
//! Vì vậy mọi thứ sinh ra đều mang `#draft` — chưa luật nào có hiệu lực cho
//! tới khi người đọc lại và nâng lên.

use crate::check;
use crate::resolve::{Resolved, Resolver};
use crate::scan::Lang;
use std::collections::BTreeMap;
use std::path::Path;

/// Thư mục gốc mã nguồn thường gặp, xét theo thứ tự này.
const SOURCE_ROOTS: &[&str] = &["src", "lib", "app", "internal", "pkg", "source"];

/// Quá nhiều component thì sơ đồ vô dụng và file luật không ai đọc. Phần dư
/// được liệt kê riêng để người tự quyết.
const MAX_COMPONENTS: usize = 24;

#[derive(Debug, Clone, Default)]
pub struct Discovery {
    /// Thư mục gốc mã nguồn, rỗng nghĩa là gốc repo.
    pub source_root: String,
    /// Tên component và mẫu `at` của nó.
    pub components: Vec<(String, String)>,
    /// Quan hệ phụ thuộc quan sát được trong mã.
    pub edges: Vec<(String, String)>,
    /// Thư mục không thành component vì đã quá số lượng, kèm số file.
    pub skipped: Vec<(String, usize)>,
    pub files: usize,
    pub by_lang: Vec<(Lang, usize)>,
}

pub fn discover(root: &Path, ignores: &[String]) -> Discovery {
    let files = check::source_files(root, ignores);
    let mut out = Discovery { files: files.len(), ..Discovery::default() };
    if files.is_empty() {
        return out;
    }

    out.by_lang = tally(files.iter().filter_map(|f| Lang::for_path(f)));
    out.source_root = pick_source_root(&files);
    let prefix = if out.source_root.is_empty() {
        String::new()
    } else {
        format!("{}/", out.source_root)
    };

    // Mỗi thư mục con ngay dưới gốc mã nguồn là một component ứng viên. File
    // nằm thẳng trong gốc gom vào một component riêng.
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for file in &files {
        if let Some(group) = group_of(file, &prefix) {
            *groups.entry(group).or_default() += 1;
        }
    }

    let mut ranked: Vec<(String, usize)> = groups.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let (kept, dropped) = ranked.split_at(ranked.len().min(MAX_COMPONENTS));
    out.skipped = dropped.to_vec();

    let mut names: Vec<String> = Vec::new();
    let mut owners: BTreeMap<String, String> = BTreeMap::new();
    for (dir, _) in kept {
        let name = unique_name(&pascal_case(dir), &names);
        names.push(name.clone());
        let glob = if dir == "." { format!("{prefix}*") } else { format!("{prefix}{dir}/**") };
        owners.insert(dir.clone(), name.clone());
        out.components.push((name, glob));
    }
    out.components.sort();

    out.edges = observe_edges(root, &files, &prefix, &owners);
    out
}

/// Quan hệ phụ thuộc thật trong mã. Chỉ quan hệ giữa hai component, bỏ qua
/// thư viện bên ngoài — liệt kê mọi gói npm hay header hệ thống sẽ làm bản
/// nháp dài tới mức không ai đọc.
fn observe_edges(
    root: &Path,
    files: &[String],
    prefix: &str,
    owners: &BTreeMap<String, String>,
) -> Vec<(String, String)> {
    let resolver = Resolver::new(root);
    let mut edges: Vec<(String, String)> = Vec::new();

    for file in files {
        let Some(lang) = Lang::for_path(file) else { continue };
        let Some(from) = group_of(file, prefix).and_then(|g| owners.get(&g)) else { continue };
        let Ok(src) = std::fs::read_to_string(root.join(file)) else { continue };

        for import in lang.imports(&src) {
            let Resolved::Path(target) = resolver.resolve(lang, file, &import) else { continue };
            let Some(to) = group_of(&target, prefix).and_then(|g| owners.get(&g)) else { continue };
            if to != from {
                edges.push((from.clone(), to.clone()));
            }
        }
    }
    edges.sort();
    edges.dedup();
    edges
}

/// Thư mục con chứa file này, tính từ gốc mã nguồn. `"."` nghĩa là nằm thẳng
/// trong gốc.
fn group_of(path: &str, prefix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    Some(rest.split_once('/').map_or_else(|| ".".to_string(), |(dir, _)| dir.to_string()))
}

/// Gốc mã nguồn là thư mục đầu chứa phần lớn file. Không có thư mục nào chiếm
/// ưu thế rõ rệt thì dùng luôn gốc repo — đoán sai còn tệ hơn không đoán.
fn pick_source_root(files: &[String]) -> String {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for f in files {
        if let Some((head, _)) = f.split_once('/') {
            *counts.entry(head).or_default() += 1;
        }
    }
    // Hoà thì lấy tên nhỏ nhất, để kết quả không phụ thuộc thứ tự duyệt.
    fn best(pool: Vec<(&str, usize)>) -> Option<(String, usize)> {
        pool.into_iter()
            .max_by_key(|(name, n)| (*n, std::cmp::Reverse(*name)))
            .map(|(name, n)| (name.to_string(), n))
    }

    // Tên quy ước thắng trước; trong số đó lấy cái nhiều file nhất.
    let conventional: Vec<(&str, usize)> =
        counts.iter().filter(|(n, _)| SOURCE_ROOTS.contains(n)).map(|(n, c)| (*n, *c)).collect();
    if let Some((name, _)) = best(conventional) {
        return name;
    }
    // Không có tên quy ước thì phải chiếm từ 60% số file trở lên. Nhân chéo
    // thay vì chia, để phép chia nguyên không làm ngưỡng tụt về 1.
    best(counts.into_iter().collect())
        .filter(|(_, n)| n * 5 >= files.len() * 3)
        .map_or(String::new(), |(name, _)| name)
}

fn tally(langs: impl Iterator<Item = Lang>) -> Vec<(Lang, usize)> {
    let mut counts: Vec<(Lang, usize)> = Vec::new();
    for lang in langs {
        match counts.iter_mut().find(|(l, _)| *l == lang) {
            Some((_, n)) => *n += 1,
            None => counts.push((lang, 1)),
        }
    }
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    counts
}

/// `create-order` → `CreateOrder`. Ký tự không hợp lệ bị bỏ, vì tên component
/// phải là định danh.
fn pascal_case(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c == '-' || c == '_' || c == '.' || c == ' ' {
            upper = true;
        } else if c.is_alphanumeric() {
            if upper {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            upper = false;
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_numeric()) {
        out.insert(0, 'M');
    }
    out
}

fn unique_name(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}{n}")).find(|c| !taken.contains(c)).unwrap_or_default()
}

/// Viết bản nháp ra dạng DSL. Mọi thứ `@ai #draft`: chưa có hiệu lực, và ghi
/// rõ nguồn gốc là máy sinh chứ không phải người thiết kế.
pub fn render(d: &Discovery, date: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let langs: Vec<String> =
        d.by_lang.iter().map(|(l, n)| format!("{n} {}", l.name())).collect();
    let _ = writeln!(out, "# Sinh bằng `archctl init --from-code` ngày {date}.");
    let _ = writeln!(out, "# Đã đọc {} file ({}).", d.files, langs.join(", "));
    out.push_str(
        "#\n\
         # ĐÂY LÀ MÔ TẢ HIỆN TRẠNG, KHÔNG PHẢI THIẾT KẾ. Nó chép lại cả những chỗ\n\
         # đang lộn xộn. Mọi thứ đang `#draft` nên chưa luật nào có hiệu lực.\n\
         #\n\
         # Cách dùng: xoá những component không có thật, gộp những cái nên gộp,\n\
         # bỏ những quan hệ không nên tồn tại, rồi nâng dần lên `#accepted`.\n\
         # Component nào có `allow` đang có hiệu lực thì component đó bắt đầu bị\n\
         # kiểm — không cần sửa xong cả file mới chạy được.\n",
    );

    if !d.components.is_empty() {
        out.push('\n');
        let width = d.components.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        for (name, glob) in &d.components {
            let _ = writeln!(out, "component {name:<width$} at {glob}   @ai #draft");
        }
    }
    if !d.edges.is_empty() {
        out.push_str("\n# Quan hệ quan sát được trong mã. Cái nào không nên tồn tại thì xoá.\n");
        let width = d.edges.iter().map(|(f, _)| f.len()).max().unwrap_or(0);
        for (from, to) in &d.edges {
            let _ = writeln!(out, "allow {from:<width$} -> {to}   @ai #draft");
        }
    }
    if !d.skipped.is_empty() {
        let _ = write!(
            out,
            "\n# Còn {} thư mục nữa chưa thành component, xếp theo số file:\n",
            d.skipped.len()
        );
        for (dir, n) in &d.skipped {
            let _ = writeln!(out, "#   {dir} ({n} file)");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doi_ten_thu_muc_thanh_dinh_danh() {
        assert_eq!(pascal_case("create-order"), "CreateOrder");
        assert_eq!(pascal_case("web_ui"), "WebUi");
        assert_eq!(pascal_case("api"), "Api");
        assert_eq!(pascal_case("2fa"), "M2fa", "tên không được bắt đầu bằng số");
        assert_eq!(pascal_case("@scope"), "Scope");
    }

    #[test]
    fn ten_trung_duoc_danh_so() {
        let taken = vec!["Api".to_string()];
        assert_eq!(unique_name("Api", &taken), "Api2");
        assert_eq!(unique_name("Web", &taken), "Web");
    }

    #[test]
    fn goc_ma_nguon_theo_ten_quy_uoc() {
        let files: Vec<String> =
            ["src/a/x.ts", "src/b/y.ts", "docs/z.ts"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_source_root(&files), "src");
    }

    #[test]
    fn khong_co_thu_muc_nao_chiem_uu_the_thi_dung_goc_repo() {
        let files: Vec<String> =
            ["aa/x.ts", "bb/y.ts", "cc/z.ts"].iter().map(|s| s.to_string()).collect();
        assert_eq!(pick_source_root(&files), "", "đoán sai còn tệ hơn không đoán");
    }

    #[test]
    fn nhom_file_theo_thu_muc_con() {
        assert_eq!(group_of("src/web/page.ts", "src/").as_deref(), Some("web"));
        assert_eq!(group_of("src/main.ts", "src/").as_deref(), Some("."));
        assert_eq!(group_of("docs/x.md", "src/"), None);
    }
}
