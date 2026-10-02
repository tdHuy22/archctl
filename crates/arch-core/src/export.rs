//! Xuất sơ đồ Mermaid từ file luật.
//!
//! Đặc tả ban đầu định làm một web app React Flow. Bỏ. Mermaid render sẵn
//! trong GitHub, GitLab, VS Code, mô tả pull request và cả khung chat của
//! agent — tốn vài trăm dòng thay vì vài vạn, và tới được nhiều nơi hơn hẳn
//! một server chạy ở localhost.
//!
//! Sơ đồ vẽ **luật**, và tuỳ chọn phủ **vi phạm thật** lên trên. Nhìn một cái
//! là thấy chỗ mã đi chệch khỏi thiết kế — đúng việc mà con người cần sơ đồ
//! để làm.

use crate::check::Violation;
use crate::model::{Document, RuleKind, Status, Target};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

#[derive(Debug, Default)]
pub struct Options<'a> {
    /// Chỉ vẽ các component trong `view` này.
    pub view: Option<&'a str>,
    /// Vi phạm để phủ lên sơ đồ. Rỗng thì chỉ vẽ luật.
    pub violations: &'a [Violation],
}

pub fn mermaid(doc: &Document, opts: &Options) -> String {
    let scope = scope_of(doc, opts.view);
    let mut out = String::from("flowchart LR\n");

    let mut placed: BTreeSet<&str> = BTreeSet::new();
    for boundary in &doc.boundaries {
        let members: Vec<&str> = boundary
            .contains
            .iter()
            .map(String::as_str)
            .filter(|n| scope.contains(*n) && !placed.contains(n))
            .collect();
        if members.is_empty() {
            continue;
        }
        let _ = writeln!(out, "  subgraph {}[\"{}\"]", id(&boundary.name), boundary.name);
        for name in members {
            placed.insert(name);
            let _ = writeln!(out, "    {}", node(doc, name));
        }
        out.push_str("  end\n");
    }

    for name in &scope {
        if !placed.contains(name.as_str()) {
            let _ = writeln!(out, "  {}", node(doc, name));
        }
    }

    out.push_str(&edges(doc, &scope, opts.violations));
    out.push_str(&styles(doc, &scope, opts.violations));
    out
}

/// Bọc trong khối mã markdown để render được trên GitHub và trong mô tả pull
/// request.
pub fn markdown(doc: &Document, opts: &Options, hash: &str, date: &str) -> String {
    let title = opts.view.unwrap_or("Kiến trúc");
    format!(
        "# {title}\n\n\
         Sinh bằng `archctl export` ngày {date} · kiến trúc `{hash}`.\n\
         Đừng sửa tay — sửa `.arch/system.arch` rồi chạy lại.\n\n\
         ```mermaid\n{}```\n",
        mermaid(doc, opts)
    )
}

/// Những component sẽ được vẽ. Không có `view` thì vẽ hết.
fn scope_of(doc: &Document, view: Option<&str>) -> BTreeSet<String> {
    match view.and_then(|name| doc.views.iter().find(|v| v.name == name)) {
        Some(v) => v.components.iter().cloned().collect(),
        None => doc
            .components
            .iter()
            .map(|c| c.name.clone())
            .chain(doc.stores.iter().map(|s| s.name.clone()))
            .chain(doc.apis.iter().map(|a| a.name.clone()))
            .collect(),
    }
}

/// Hình dạng nói lên loại phần tử: hộp là component, hình trụ là kho dữ liệu,
/// lục giác là thứ bên ngoài, hình bình hành là nhóm hàm bị theo dõi.
fn node(doc: &Document, name: &str) -> String {
    let id = id(name);
    if let Some(store) = doc.store(name) {
        return format!("{id}[(\"{name} · {}\")]", store.tables.join(", "));
    }
    if doc.api(name).is_some() {
        return format!("{id}[/\"{name}\"/]");
    }
    match doc.component(name) {
        Some(c) if c.external => format!("{id}{{{{\"{name}\"}}}}"),
        _ => format!("{id}[\"{name}\"]"),
    }
}

fn edges(doc: &Document, scope: &BTreeSet<String>, violations: &[Violation]) -> String {
    let mut out = String::new();
    let mut drawn: BTreeSet<(String, String)> = BTreeSet::new();
    let mut broken: BTreeMap<(String, String), usize> = BTreeMap::new();
    for v in violations {
        if scope.contains(&v.from) && scope.contains(&v.to) {
            *broken.entry((v.from.clone(), v.to.clone())).or_default() += 1;
        }
    }

    for rule in &doc.rules {
        let Target::Named(from) = &rule.from else { continue };
        if !scope.contains(from) {
            continue;
        }
        for target in &rule.to {
            let Target::Named(to) = target else { continue };
            if !scope.contains(to) || !drawn.insert((from.clone(), to.clone())) {
                continue;
            }
            let arrow = match (rule.kind, rule.meta.status.is_enforced()) {
                (RuleKind::Allow, true) => "-->",
                // Luật chưa có hiệu lực vẽ nét đứt: nhìn là biết chưa ràng buộc.
                (RuleKind::Allow, false) => "-.->",
                (RuleKind::Deny, _) => "-.->|\"cấm\"|",
            };
            let _ = writeln!(out, "  {} {arrow} {}", id(from), id(to));
        }
    }

    // Vi phạm là cạnh có thật trong mã nhưng không có trong luật, nên phải vẽ
    // riêng chứ không chỉ tô màu cạnh đã có.
    for ((from, to), count) in &broken {
        let label = if *count > 1 { format!("{count} vi phạm") } else { "vi phạm".into() };
        let _ = writeln!(out, "  {} ==>|\"{label}\"| {}", id(from), id(to));
    }
    out
}

fn styles(doc: &Document, scope: &BTreeSet<String>, violations: &[Violation]) -> String {
    let mut out = String::from(
        "\n  classDef draft stroke-dasharray:5 3,opacity:0.55\n\
           \u{20} classDef locked stroke-width:3px\n\
           \u{20} classDef broken stroke:#d33,stroke-width:2px,color:#d33\n",
    );

    let mut by_class: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for name in scope {
        let status = doc
            .component(name)
            .map(|c| c.meta.status)
            .or_else(|| doc.store(name).map(|s| s.meta.status))
            .or_else(|| doc.api(name).map(|a| a.meta.status));
        match status {
            Some(Status::Draft | Status::Proposed) => {
                by_class.entry("draft").or_default().push(id(name))
            }
            Some(Status::Locked) => by_class.entry("locked").or_default().push(id(name)),
            _ => {}
        }
    }
    for v in violations {
        if scope.contains(&v.from) {
            by_class.entry("broken").or_default().push(id(&v.from));
        }
    }

    for (class, mut names) in by_class {
        names.sort();
        names.dedup();
        let _ = writeln!(out, "  class {} {class}", names.join(","));
    }
    out
}

/// Mermaid nhận định danh hẹp hơn DSL, nên gạch nối phải đổi thành gạch dưới.
/// Nhãn hiển thị vẫn giữ tên gốc.
fn id(name: &str) -> String {
    name.replace('-', "_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Document {
        let (d, diags) = crate::load(src);
        assert!(!crate::diag::has_errors(&diags), "{diags:#?}");
        d
    }

    #[test]
    fn hinh_dang_noi_len_loai_phan_tu() {
        let d = doc(
            "component Web at w/**\n\
             external Stripe at \"stripe\"\n\
             store Db owned_by Web at \"t\"\n\
             api Unsafe at \"gets\"",
        );
        let out = mermaid(&d, &Options::default());
        assert!(out.contains(r#"Web["Web"]"#), "{out}");
        assert!(out.contains(r#"Stripe{{"Stripe"}}"#), "{out}");
        assert!(out.contains(r#"Db[("Db · t")]"#), "{out}");
        assert!(out.contains(r#"Unsafe[/"Unsafe"/]"#), "{out}");
    }

    #[test]
    fn luat_chua_co_hieu_luc_ve_net_dut() {
        let d = doc("component A at a/**\ncomponent B at b/**\nallow A -> B #draft");
        assert!(mermaid(&d, &Options::default()).contains("A -.-> B"));
    }

    /// Nét mờ theo trạng thái của **phần tử**, không phải của luật nối chúng.
    #[test]
    fn component_nhap_ve_mo_component_da_khoa_ve_dam() {
        let d = doc("component A at a/** #draft\ncomponent B at b/** #locked");
        let out = mermaid(&d, &Options::default());
        assert!(out.contains("class A draft"), "{out}");
        assert!(out.contains("class B locked"), "{out}");
    }

    #[test]
    fn deny_ve_rieng_va_co_nhan() {
        let d = doc("component A at a/**\ncomponent B at b/**\ndeny A -> B");
        assert!(mermaid(&d, &Options::default()).contains(r#"A -.->|"cấm"| B"#));
    }

    #[test]
    fn boundary_thanh_subgraph() {
        let d = doc(
            "component A at a/**\ncomponent B at b/**\n\
             boundary Core contains A, B:\n  verify x",
        );
        let out = mermaid(&d, &Options::default());
        assert!(out.contains(r#"subgraph Core["Core"]"#), "{out}");
        assert!(out.contains("    A[\"A\"]"), "{out}");
    }

    #[test]
    fn view_cat_bot_phan_khong_lien_quan() {
        let d = doc(
            "component A at a/**\ncomponent B at b/**\ncomponent C at c/**\n\
             allow A -> B\nallow A -> C\nview Nho: A, B",
        );
        let out = mermaid(&d, &Options { view: Some("Nho"), ..Options::default() });
        assert!(out.contains("A --> B"), "{out}");
        assert!(!out.contains('C'), "{out}");
    }

    #[test]
    fn ten_co_gach_noi_doi_thanh_gach_duoi() {
        let d = doc("component web-ui at w/**");
        let out = mermaid(&d, &Options::default());
        assert!(out.contains(r#"web_ui["web-ui"]"#), "nhãn giữ tên gốc: {out}");
    }

    #[test]
    fn markdown_boc_trong_khoi_ma() {
        let d = doc("component A at a/**");
        let md = markdown(&d, &Options::default(), "abcd1234", "2026-10-02");
        assert!(md.starts_with("# Kiến trúc"));
        assert!(md.contains("```mermaid\nflowchart LR"));
        assert!(md.trim_end().ends_with("```"));
        assert!(md.contains("abcd1234"));
    }
}
