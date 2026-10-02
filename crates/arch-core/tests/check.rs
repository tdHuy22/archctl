//! Test đầu-tới-cuối trên dự án mẫu trong `examples/shop`.
//!
//! Dự án mẫu cố ý chứa hai vi phạm, hai import không phân giải được, một file
//! ngoài sơ đồ, và vài chuỗi trông giống import trong chú thích. Nhờ vậy một
//! lần chạy kiểm được cả đường đúng lẫn đường sai.

use arch_core::check::{Reason, check};
use std::path::{Path, PathBuf};

fn shop() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/shop")
}

fn report() -> arch_core::check::Report {
    let root = shop();
    let src = std::fs::read_to_string(root.join(arch_core::ARCH_FILE)).unwrap();
    let (doc, diags) = arch_core::load(&src);
    assert!(!arch_core::diag::has_errors(&diags), "file luật mẫu phải sạch: {diags:#?}");
    check(&root, &doc, &[])
}

#[test]
fn tim_dung_hai_vi_pham() {
    let r = report();
    let found: Vec<(&str, &str)> =
        r.violations.iter().map(|v| (v.from.as_str(), v.to.as_str())).collect();
    assert_eq!(found, [("Stock", "Payment"), ("Web", "Stripe")], "nhận: {:#?}", r.violations);
}

#[test]
fn deny_duoc_neu_ten_va_goi_y_di_qua_dung_component() {
    let r = report();
    let v = r.violations.iter().find(|v| v.to == "Stripe").unwrap();
    assert_eq!(v.reason, Reason::Denied);
    assert_eq!(v.rule, "deny * -> Stripe except Payment");
    assert!(v.hint.contains("gọi Payment"), "nhận: {}", v.hint);
}

#[test]
fn khong_co_allow_thi_goi_y_liet_ke_dich_hop_le() {
    let r = report();
    let v = r.violations.iter().find(|v| v.from == "Stock").unwrap();
    assert_eq!(v.reason, Reason::NoAllow);
    assert!(v.hint.contains("Notify"), "nhận: {}", v.hint);
}

/// `src/payment/charge.ts` có một import bị chú thích, một trong chuỗi và một
/// trong template literal, tất cả đều trỏ tới `../user/profile`. Payment đã vào
/// diện kiểm, nên nếu bộ quét tính bất kỳ dòng nào trong số đó thì test này đổ.
#[test]
fn import_trong_chu_thich_va_chuoi_khong_sinh_vi_pham() {
    let r = report();
    assert!(
        !r.violations.iter().any(|v| v.from == "Payment"),
        "bộ quét tính cả chú thích hoặc chuỗi: {:#?}",
        r.violations
    );
}

#[test]
fn import_khong_phan_giai_duoc_thi_bao_ra_chu_khong_im_lang() {
    let r = report();
    let specs: Vec<&str> = r.unresolved.iter().map(|u| u.spec.as_str()).collect();
    assert_eq!(specs, ["../shared/cart-types", "@/lib/format"]);
}

#[test]
fn component_chua_co_allow_thi_chua_vao_dien_kiem() {
    let r = report();
    assert_eq!(r.unchecked, ["Notify", "User"]);
    assert_eq!(r.checked, ["Web", "Order", "Payment", "Stock"]);
}

#[test]
fn file_ngoai_so_do_duoc_dem_chu_khong_bi_coi_la_loi() {
    let r = report();
    assert_eq!(r.files_unmapped, 1, "src/shared/money.ts không thuộc component nào");
    assert!(!r.violations.iter().any(|v| v.file.contains("shared")));
}

/// `store OrderDB owned_by Order` là một `deny` ngầm. Nó không chờ component
/// vào diện kiểm, và không cần viết `deny` tay.
#[test]
fn chi_chu_so_huu_duoc_cham_vao_kho_du_lieu() {
    let r = report();
    assert!(
        !r.violations.iter().any(|v| v.to == "OrderDB" || v.to == "PaymentDB"),
        "dự án mẫu shop không có truy vấn SQL nào: {:#?}",
        r.violations
    );
}

#[test]
fn khong_quet_vao_node_modules_hay_arch() {
    let r = report();
    assert_eq!(r.files_scanned, 7, "chỉ các file .ts trong src/");
}

#[test]
fn import_hop_le_khong_sinh_vi_pham() {
    let r = report();
    // Order -> Stock, Payment, Notify đều có allow; Web -> Order, User cũng vậy.
    assert!(!r.violations.iter().any(|v| v.from == "Order"), "nhận: {:#?}", r.violations);
    assert!(
        !r.violations.iter().any(|v| v.from == "Web" && v.to == "Order"),
        "nhận: {:#?}",
        r.violations
    );
}
