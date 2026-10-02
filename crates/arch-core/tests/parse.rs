//! Test cho parser. Nhóm đầu kiểm nguồn hợp lệ, nhóm sau kiểm rằng nguồn sai
//! báo đúng lỗi và đúng dòng — thông báo sai dòng còn tệ hơn không báo.

use arch_core::model::*;
use arch_core::{Severity, load};

fn errors(src: &str) -> Vec<String> {
    load(src)
        .1
        .into_iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| format!("{}:{}", d.line, d.msg))
        .collect()
}

fn ok(src: &str) -> Document {
    let (doc, diags) = load(src);
    let errs: Vec<_> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    assert!(errs.is_empty(), "không mong có lỗi, nhận được: {errs:#?}");
    doc
}

#[test]
fn doc_mau_trong_examples_doc_khong_loi() {
    let src = include_str!("../../../examples/shop/.arch/system.arch");
    let doc = ok(src);
    assert_eq!(doc.components.len(), 7, "6 component + 1 external");
    assert_eq!(doc.stores.len(), 2);
    assert_eq!(doc.rules.len(), 6);
    assert_eq!(doc.flows.len(), 1);
    assert_eq!(doc.contracts.len(), 1);
    assert_eq!(doc.boundaries.len(), 1);
    assert_eq!(doc.invariants.len(), 4);
}

#[test]
fn component_co_glob_va_chu_thich() {
    let doc = ok("component Web at src/web/**, src/pages/** @ai #locked");
    let c = &doc.components[0];
    assert_eq!(c.name, "Web");
    assert_eq!(c.paths, ["src/web/**", "src/pages/**"]);
    assert_eq!(c.meta.owner, Owner::Ai);
    assert_eq!(c.meta.status, Status::Locked);
    assert!(!c.external);
}

#[test]
fn thieu_chu_thich_thi_mac_dinh_la_human_accepted() {
    let doc = ok("component Web at src/web/**");
    assert_eq!(doc.components[0].meta.owner, Owner::Human);
    assert_eq!(doc.components[0].meta.status, Status::Accepted);
    assert!(doc.components[0].meta.status.is_enforced());
}

#[test]
fn draft_va_proposed_khong_co_hieu_luc() {
    let doc = ok("component A at a/**\ncomponent B at b/**\nallow A -> B #draft");
    assert!(!doc.rules[0].meta.status.is_enforced());
}

#[test]
fn store_owned_by_va_danh_sach_bang() {
    let doc = ok(
        "component Payment at src/payment/**\n\
         store PaymentDB owned_by Payment at \"payments\", \"refunds\"",
    );
    let s = &doc.stores[0];
    assert_eq!(s.owned_by, "Payment");
    assert_eq!(s.tables, ["payments", "refunds"]);
}

#[test]
fn deny_voi_except() {
    let doc = ok(
        "component Payment at p/**\n\
         store PaymentDB owned_by Payment\n\
         deny * -> PaymentDB except Payment",
    );
    let r = &doc.rules[0];
    assert_eq!(r.kind, RuleKind::Deny);
    assert_eq!(r.from, Target::Any);
    assert_eq!(r.to, [Target::Named("PaymentDB".into())]);
    assert_eq!(r.except, ["Payment"]);
}

#[test]
fn flow_voi_parallel_va_on_fail() {
    let doc = ok(
        "flow PlaceOrder:\n\
        \x20 reserveStock -> charge -> confirm\n\
        \x20 parallel: sendEmail, updateAnalytics\n\
        \x20 on_fail(charge): releaseStock, notify\n",
    );
    let f = &doc.flows[0];
    assert_eq!(f.lines.len(), 3);
    assert_eq!(
        f.lines[0],
        FlowLine::Sequence(vec!["reserveStock".into(), "charge".into(), "confirm".into()])
    );
    assert_eq!(
        f.lines[1],
        FlowLine::Parallel(vec!["sendEmail".into(), "updateAnalytics".into()])
    );
    assert_eq!(
        f.lines[2],
        FlowLine::OnFail {
            step: "charge".into(),
            steps: vec!["releaseStock".into(), "notify".into()]
        }
    );
}

#[test]
fn contract_gom_dong_tiep_noi_vao_dung_khoa() {
    let doc = ok(
        "component Payment at p/**\n\
         contract Payment.charge:\n\
        \x20 requires  order.status == RESERVED\n\
        \x20 ensures   ok  => CAPTURED\n\
        \x20           err => không chuyển tiền\n\
        \x20 verify    contract_test\n",
    );
    let fields = &doc.contracts[0].fields;
    assert_eq!(fields.len(), 3, "ensures và dòng tiếp nối của nó là một mục");
    assert_eq!(fields[1].key, "ensures");
    assert_eq!(fields[1].values, ["ok  => CAPTURED", "err => không chuyển tiền"]);
}

#[test]
fn invariant_co_pham_vi_va_verify() {
    let doc = ok("invariant money [system] tổng tiền khớp verify tla_model, daily_reconcile");
    let inv = &doc.invariants[0];
    assert_eq!(inv.kind, "money");
    assert_eq!(inv.scope.as_deref(), Some("system"));
    assert_eq!(inv.expr, "tổng tiền khớp");
    assert_eq!(inv.verify, ["tla_model", "daily_reconcile"]);
}

#[test]
fn dong_bat_dau_bang_thang_la_chu_thich() {
    let doc = ok("# component KhongTonTai at x/**\ncomponent Web at src/web/**");
    assert_eq!(doc.components.len(), 1);
}

#[test]
fn quan_he_thieu_mui_ten_bao_loi_dung_dong() {
    let errs = errors("component A at a/**\nallow A B");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].starts_with("2:"), "phải báo ở dòng 2, nhận: {}", errs[0]);
    assert!(errs[0].contains("->"));
}

#[test]
fn tham_chieu_khong_ton_tai_bao_loi() {
    let errs = errors("component A at a/**\nallow A -> KhongCo");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("KhongCo"));
}

#[test]
fn trung_ten_giua_component_va_store_bao_loi() {
    let errs = errors("component A at a/**\nstore A owned_by A");
    assert!(errs.iter().any(|e| e.contains("trùng tên")), "nhận: {errs:?}");
}

#[test]
fn store_thieu_owned_by_bao_loi() {
    let errs = errors("store OrderDB");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].contains("owned_by"));
}

#[test]
fn allow_khong_dung_duoc_except() {
    let errs = errors("component A at a/**\ncomponent B at b/**\nallow A -> B except A");
    assert!(errs.iter().any(|e| e.contains("except")), "nhận: {errs:?}");
}

#[test]
fn chu_thich_trang_thai_la_khong_hop_le() {
    let errs = errors("component A at a/**  #khoa");
    assert!(errs.iter().any(|e| e.contains("#khoa")), "nhận: {errs:?}");
}

#[test]
fn tu_khoa_la_bao_loi_kem_goi_y() {
    let (_, diags) = load("compoent Web at src/**");
    let d = &diags[0];
    assert_eq!(d.severity, Severity::Error);
    assert!(d.hint.as_ref().unwrap().contains("component"));
}

#[test]
fn loai_invariant_la_bao_loi() {
    let errs = errors("invariant toposort [system] gì đó");
    assert!(errs.iter().any(|e| e.contains("toposort")), "nhận: {errs:?}");
}

#[test]
fn component_khong_co_at_bi_canh_bao() {
    let (_, diags) = load("component Web");
    assert!(
        diags.iter().any(|d| d.severity == Severity::Warning && d.msg.contains("bỏ qua")),
        "nhận: {diags:#?}"
    );
}

#[test]
fn invariant_khong_co_verify_bi_canh_bao_la_loi_hua() {
    let (_, diags) = load("invariant money [system] tổng tiền khớp");
    assert!(
        diags.iter().any(|d| d.msg.contains("lời hứa")),
        "luật không kiểm được phải bị nói ra, nhận: {diags:#?}"
    );
}

#[test]
fn invariant_algo_luon_bi_canh_bao_khong_kiem_duoc() {
    let (_, diags) = load("component S at s/**\ninvariant algo [S] O(log n) verify benchmark");
    assert!(
        diags.iter().any(|d| d.msg.contains("không kiểm được bằng máy")),
        "nhận: {diags:#?}"
    );
}

#[test]
fn mot_dong_sai_khong_lam_hong_ca_file() {
    let (doc, diags) = load("component A at a/**\nallow A B\ncomponent C at c/**");
    assert_eq!(doc.components.len(), 2, "vẫn đọc được component sau dòng lỗi");
    assert_eq!(diags.iter().filter(|d| d.severity == Severity::Error).count(), 1);
}

#[test]
fn viet_lai_chu_thich_giu_nguyen_noi_dung() {
    use arch_core::parser::rewrite_annotations;
    let out = rewrite_annotations("component Web at src/web/**   @ai #draft", Owner::Human, Status::Locked);
    assert_eq!(out, "component Web at src/web/**  @human #locked");
    // Dòng chưa có chú thích thì thêm vào.
    let out = rewrite_annotations("allow A -> B", Owner::Ai, Status::Accepted);
    assert_eq!(out, "allow A -> B  @ai #accepted");
}
