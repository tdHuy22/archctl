//! Test cho `store owned_by` — kiểm truy cập dữ liệu.
//!
//! Chỉ dò trong chuỗi, có chủ ý. Nhóm test thứ hai khoá lại đúng quyết định
//! đó: một trường struct trùng tên bảng **không** được tính là truy cập.

use arch_core::scan::Lang;

fn tables(lang: Lang, src: &str, needles: &[&str]) -> Vec<(String, usize)> {
    lang.scan_all(src, &Default::default(), needles).tables.into_iter().map(|h| (h.symbol, h.line)).collect()
}

#[test]
fn sql_tho_trong_chuoi_duoc_bat() {
    let src = "const q = \"SELECT ts FROM packets WHERE len > 0\"\n";
    assert_eq!(tables(Lang::TypeScript, src, &["packets"]), [("packets".into(), 1)]);
}

#[test]
fn query_builder_va_ten_collection() {
    assert_eq!(tables(Lang::TypeScript, "knex('payments').insert(x)", &["payments"]).len(), 1);
    assert_eq!(
        tables(Lang::TypeScript, "db.collection('payments').find()", &["payments"]).len(),
        1
    );
}

#[test]
fn ranh_gioi_tu_duoc_ton_trong() {
    // `payments_archive` và `old_payments` là bảng khác.
    let src = "const a = \"FROM payments_archive\"\nconst b = \"FROM old_payments\"\n";
    assert!(tables(Lang::TypeScript, src, &["payments"]).is_empty());
}

/// Trường struct tên trùng bảng không phải truy cập dữ liệu. Phân biệt được
/// hai thứ đó cần thông tin kiểu; đoán sai sẽ báo động giả ngay trong mã của
/// người dùng, nên dứt khoát không dò trong định danh.
#[test]
fn truong_struct_trung_ten_bang_khong_duoc_tinh() {
    assert!(tables(Lang::Rust, "struct S { packets: u64 }", &["packets"]).is_empty());
    assert!(tables(Lang::TypeScript, "const packets = load()", &["packets"]).is_empty());
    assert!(tables(Lang::Python, "self.packets = 0", &["packets"]).is_empty());
}

#[test]
fn ten_bang_trong_chu_thich_khong_duoc_tinh() {
    let src = "// SELECT * FROM packets\n/* FROM packets */\n";
    assert!(tables(Lang::TypeScript, src, &["packets"]).is_empty());
}

#[test]
fn khong_khai_bao_store_thi_khong_quet_gi() {
    assert!(Lang::TypeScript.scan_all("\"FROM packets\"", &Default::default(), &[]).tables.is_empty());
}

// ---------- ngữ nghĩa quyền sở hữu ----------

use arch_core::check::{Reason, check};
use std::path::{Path, PathBuf};

fn sensor() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sensor")
}

fn report() -> arch_core::check::Report {
    let root = sensor();
    let src = std::fs::read_to_string(root.join(arch_core::ARCH_FILE)).unwrap();
    let (doc, diags) = arch_core::load(&src);
    assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
    check(&root, &doc, &[])
}

#[test]
fn cham_bang_cua_component_khac_la_vi_pham() {
    let r = report();
    let v = r.violations.iter().find(|v| v.to == "Captures").expect("phải bắt được");
    assert_eq!(v.reason, Reason::NotOwner);
    assert_eq!(v.from, "ToolsLib");
    assert_eq!(v.spec, "packets");
    assert!(v.hint.contains("gọi Capture"), "nhận: {}", v.hint);
}

#[test]
fn chi_dung_mot_vi_pham_bang_tren_ca_du_an() {
    let r = report();
    // `report.rs` có trường `packets` và `io.py` có `packets_archive`; cả hai
    // không được tính. Chỉ câu SQL thật mới tính.
    let hits: Vec<&str> = r
        .violations
        .iter()
        .filter(|v| v.to == "Captures")
        .map(|v| v.file.as_str())
        .collect();
    assert_eq!(hits, ["tools/lib/io.py"], "nhận: {hits:?}");
}

#[test]
fn quyen_so_huu_khong_cho_component_vao_dien_kiem() {
    // ToolsLib có `allow ToolsLib -> NumPy` nên đã vào diện kiểm, nhưng luật
    // sở hữu phải áp cả với component chưa vào diện kiểm — nó là `deny` ngầm.
    let (doc, _) = arch_core::load(
        "component A at a/**\ncomponent B at b/**\nstore Db owned_by B at \"t\"",
    );
    assert!(doc.store("Db").is_some());
    assert!(doc.knows("Db"), "store phải là đích hợp lệ của allow/deny");
}
