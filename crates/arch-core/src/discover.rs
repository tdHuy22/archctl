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

/// Thư mục không nói gì về kiến trúc của sản phẩm. Bỏ khi **chọn bố cục**;
/// chúng vẫn được kiểm bình thường sau đó.
const ASIDE: &[&str] = &[
    "test", "tests", "testing", "spec", "specs", "__tests__", "e2e", "example", "examples",
    "bench", "benches", "doc", "docs", "deps", "vendor", "third_party", "thirdparty",
    "fixtures", "testdata",
];

/// Tiền tố đào sâu tối đa. `src/flask` và `tokio/src` cần hai cấp.
const MAX_PREFIX_DEPTH: usize = 3;
/// Tiền tố phủ quá ít file thì không đáng xét, và xét hết mọi thư mục của một
/// repo lớn thì tốn.
const MIN_PREFIX_SHARE: usize = 8;

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
    let mut ranked: Vec<(String, usize)> = groups_under(&files, &prefix).into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let (kept, dropped) = ranked.split_at(ranked.len().min(MAX_COMPONENTS));
    out.skipped = dropped.to_vec();

    let mut names: Vec<String> = Vec::new();
    let mut owners: BTreeMap<String, String> = BTreeMap::new();
    for (dir, _) in kept {
        let base = if dir == "." { root_group_name(&prefix) } else { pascal_case(dir) };
        let name = unique_name(&base, &names);
        names.push(name.clone());
        let glob = if dir == "." {
            root_glob(&files, &prefix)
        } else {
            format!("{prefix}{dir}/**")
        };
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
        if src.len() as u64 > crate::check::MAX_FILE_BYTES {
            continue;
        }

        for import in lang.imports(&src) {
            let Resolved::Path(target) = resolver.resolve(lang, file, &import) else { continue };
            let Some(to) = group_of_target(root, &target, prefix).and_then(|g| owners.get(&g))
            else {
                continue;
            };
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

/// Nhóm mà một **đích phân giải** thuộc về.
///
/// Khác `group_of` ở chỗ đích có thể là cả một thư mục: Go phân giải import
/// thành package, và gói không gian tên của Python cũng vậy. Coi `helpers` là
/// "file nằm thẳng ở gốc" sẽ gom mọi package cấp một của Go vào chung một
/// component, và khi đó mọi cạnh giữa chúng biến mất.
fn group_of_target(root: &Path, path: &str, prefix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    match rest.split_once('/') {
        Some((dir, _)) => Some(dir.to_string()),
        None if !rest.is_empty() && root.join(path).is_dir() => Some(rest.to_string()),
        None => Some(".".to_string()),
    }
}

/// Mẫu `at` cho nhóm file nằm thẳng trong gốc.
///
/// `*` thì hỏng: nó khớp một đoạn bất kỳ, nên nó nhận luôn mọi **thư mục** con
/// chứ không riêng file. Go và Python phân giải import thành cả thư mục, nên
/// một package không thành component sẽ bị mẫu này nhận bừa — và khi đó bộ
/// khám phá với bộ kiểm chứng nói hai chuyện khác nhau về cùng một import.
///
/// Liệt kê đúng những phần mở rộng thật sự có ở đó thì mẫu chỉ còn khớp file.
fn root_glob(files: &[String], prefix: &str) -> String {
    let mut exts: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix(prefix))
        .filter(|rest| !rest.contains('/'))
        .filter_map(|rest| rest.rsplit_once('.').map(|(_, ext)| ext.to_string()))
        .collect();
    exts.sort();
    exts.dedup();
    if exts.is_empty() {
        return format!("{prefix}*");
    }
    exts.iter().map(|e| format!("{prefix}*.{e}")).collect::<Vec<_>>().join(", ")
}

/// Đoạn đường dẫn chỉ nói "đây là mã nguồn", không nói component nào.
const GENERIC_DIRS: &[&str] = &["src", "lib", "source", "crates", "packages", "internal", "pkg"];

/// Tên cho nhóm file nằm thẳng trong gốc.
///
/// `pascal_case(".")` ra chuỗi rỗng rồi rơi vào nhánh dự phòng thành `M` —
/// một cái tên không nói gì với người đọc sơ đồ. Lấy tên từ chính gốc mã
/// nguồn thì có nghĩa hơn: `src/flask` ra `Flask`, `tokio/src` ra `Tokio`.
fn root_group_name(prefix: &str) -> String {
    let name = prefix
        .trim_end_matches('/')
        .split('/')
        .rev()
        .find(|part| !part.is_empty() && !GENERIC_DIRS.contains(part))
        .unwrap_or("");
    if name.is_empty() { "Core".to_string() } else { pascal_case(name) }
}

/// Gốc mã nguồn là thư mục đầu chứa phần lớn file. Không có thư mục nào chiếm
/// ưu thế rõ rệt thì dùng luôn gốc repo — đoán sai còn tệ hơn không đoán.
/// Gốc mã nguồn: thư mục mà chia theo con của nó thì lộ ra nhiều cấu trúc
/// nhất.
///
/// Trước đây chỗ này chọn theo tên quy ước (`src`, `lib`, `internal`…) và tên
/// quy ước thắng vô điều kiện. Hugo có `internal/` với 28 file trên tổng 943,
/// nên nó thắng và 915 file còn lại nằm ngoài sơ đồ. Flask có đúng một thư mục
/// dưới `src/`, nên bản nháp ra **một** component và không luật nào cả.
///
/// Thước đo thay thế là **số file không nằm trong nhóm lớn nhất**. Nó phạt cả
/// hai kiểu hỏng cùng lúc: gốc quá sâu thì phủ ít, còn gốc quá nông thì một
/// nhóm nuốt gần hết và cũng chẳng nói lên gì. `crates/` của ripgrep thắng
/// gốc repo vì chia được thành mười một; gốc repo của hugo thắng `tpl/` vì
/// `tpl` chỉ là một phần.
fn pick_source_root(files: &[String]) -> String {
    let main = architectural(files);
    let floor = main.len() / MIN_PREFIX_SHARE;

    let mut best: Option<(usize, usize, String)> = None;
    for prefix in candidates(&main) {
        let groups = groups_under(&main, &prefix);
        if groups.len() < 2 {
            continue;
        }
        let covered: usize = groups.values().sum();
        if covered < floor {
            continue;
        }
        let biggest = groups.values().copied().max().unwrap_or(0);
        // Hoà thì gốc nông thắng, rồi tới tên nhỏ nhất: kết quả không được
        // phụ thuộc thứ tự duyệt thư mục.
        let score = (covered - biggest, covered, std::cmp::Reverse(prefix.clone()));
        if best.as_ref().is_none_or(|b| {
            (score.0, score.1, &score.2) > (b.0, b.1, &std::cmp::Reverse(b.2.clone()))
        }) {
            best = Some((score.0, score.1, prefix));
        }
    }
    best.map(|(_, _, p)| p.trim_end_matches('/').to_string()).unwrap_or_default()
}

/// File nói được điều gì đó về kiến trúc sản phẩm. Test và ví dụ thì không —
/// nhưng nếu bỏ chúng đi mà còn lại quá ít thì repo này chủ yếu **là** test,
/// và khi đó bỏ chúng chỉ làm mất thông tin.
fn architectural(files: &[String]) -> Vec<String> {
    let kept: Vec<String> = files
        .iter()
        .filter(|f| {
            let dirs = f.rsplit_once('/').map_or("", |(d, _)| d);
            !dirs.split('/').any(|d| ASIDE.contains(&d))
        })
        .cloned()
        .collect();
    if kept.len() * 4 >= files.len() { kept } else { files.to_vec() }
}

/// Tiền tố đáng thử: gốc repo, và mọi thư mục tới độ sâu `MAX_PREFIX_DEPTH`
/// chứa đủ nhiều file.
fn candidates(files: &[String]) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for f in files {
        let dirs = f.rsplit_once('/').map_or("", |(d, _)| d);
        if dirs.is_empty() {
            continue;
        }
        let parts: Vec<&str> = dirs.split('/').collect();
        for depth in 1..=parts.len().min(MAX_PREFIX_DEPTH) {
            *counts.entry(format!("{}/", parts[..depth].join("/"))).or_default() += 1;
        }
    }
    let floor = (files.len() / MIN_PREFIX_SHARE).max(2);
    let mut out = vec![String::new()];
    out.extend(counts.into_iter().filter(|(_, n)| *n >= floor).map(|(p, _)| p));
    out
}

/// Nhóm file theo thư mục con ngay dưới `prefix`. File nằm thẳng trong đó gom
/// vào nhóm `.`.
fn groups_under(files: &[String], prefix: &str) -> BTreeMap<String, usize> {
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for file in files {
        if let Some(group) = group_of(file, prefix) {
            *groups.entry(group).or_default() += 1;
        }
    }
    groups
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
