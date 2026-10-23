//! Test cho `archctl diff`.
//!
//! Giá trị của lệnh này nằm ở chỗ **tách thay đổi thật khỏi thay đổi hình
//! thức**. Nếu một lần sắp xếp lại file cũng bị báo là đổi kiến trúc thì người
//! duyệt sẽ ngừng đọc, và cả cơ chế mất tác dụng.

use arch_core::diff::{Class, Kind, diff};
use arch_core::{Document, arch_hash, load};

fn doc(src: &str) -> Document {
    let (d, diags) = load(src);
    assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
    d
}

const BASE: &str = "\
component Web     at src/web/**      @human #locked
component Order   at src/order/**    @human #accepted
component Stock   at src/stock/**    @human #accepted
allow Web   -> Order
allow Order -> Stock
invariant data [Order] total == sum(items) verify property_test
";

#[test]
fn them_chu_thich_va_doi_thu_tu_khong_phai_thay_doi_kien_truc() {
    let base = doc(BASE);
    let head = doc("\
# Ghi chú hoàn toàn mới.

component Order   at src/order/**    @human #accepted

component Web     at src/web/**      @human #locked
component Stock   at src/stock/**        @human #accepted
allow Order -> Stock
allow Web   -> Order
invariant data [Order] total == sum(items) verify property_test
");
    let d = diff(&base, &head);
    assert!(d.is_quiet(), "nhận: {:#?}", d.changes);
    assert!(d.moved > 0, "phải đếm được số phần tử xê dịch");
    assert_eq!(arch_hash(&base), arch_hash(&head), "hash phải bỏ qua hình thức");
}

#[test]
fn them_component_la_thay_doi_cau_truc() {
    let head = doc(&format!("{BASE}component Cache at src/cache/** @ai #proposed\n"));
    let d = diff(&doc(BASE), &head);
    assert_eq!(d.changes.len(), 1);
    assert_eq!(d.changes[0].kind, Kind::Added);
    assert_eq!(d.changes[0].class, Class::Topology);
    assert_eq!(d.changes[0].key, "Cache");
}

#[test]
fn doi_pham_vi_at_la_thay_doi_cau_truc_chu_khong_phai_them_bot() {
    let head = doc(&BASE.replace("at src/order/**", "at src/order/**, src/cart/**"));
    let d = diff(&doc(BASE), &head);
    assert_eq!(d.changes.len(), 1);
    assert_eq!(d.changes[0].kind, Kind::Changed, "cùng tên thì ghép được, không tách thành +/-");
    assert_eq!(d.changes[0].class, Class::Topology);
}

#[test]
fn doi_trang_thai_la_thay_doi_quan_tri() {
    let head = doc(&BASE.replace("at src/order/**    @human #accepted", "at src/order/**    @human #locked"));
    let d = diff(&doc(BASE), &head);
    assert_eq!(d.changes.len(), 1);
    assert_eq!(d.changes[0].class, Class::Governance);
    assert!(!d.changes[0].loosening, "nâng lên mức chặt hơn không phải nới lỏng");
    assert_eq!(d.architectural(), 0, "quản trị không tính là thay đổi kiến trúc");
}

#[test]
fn ha_trang_thai_xuong_duoc_danh_dau_la_noi_long() {
    let head = doc(&BASE.replace("at src/web/**      @human #locked", "at src/web/**      @human #draft"));
    let d = diff(&doc(BASE), &head);
    let loosened: Vec<&str> = d.loosenings().map(|c| c.key.as_str()).collect();
    assert_eq!(loosened, ["Web"], "hạ #locked xuống #draft là nới lỏng");
}

/// Đổi cả nội dung lẫn trạng thái phải hiện ra cả hai. Nếu thay đổi nội dung
/// che mất việc khoá luật thì người duyệt sẽ bỏ sót hành vi quản trị.
#[test]
fn doi_ca_noi_dung_lan_trang_thai_sinh_hai_thay_doi() {
    let head = doc(&BASE.replace(
        "component Order   at src/order/**    @human #accepted",
        "component Order   at src/order/**, src/cart/**  @human #locked",
    ));
    let d = diff(&doc(BASE), &head);
    assert_eq!(d.changes.len(), 2, "nhận: {:#?}", d.changes);
    assert_eq!(d.count(Class::Topology), 1);
    assert_eq!(d.count(Class::Governance), 1);
}

#[test]
fn doi_quan_he_hien_thanh_mot_cap_them_bot() {
    // Luật không có tên riêng, nên ghép hai luật với nhau chỉ là đoán. Một cặp
    // thêm/bớt rõ ràng hơn.
    let head = doc(&BASE.replace("allow Web   -> Order", "allow Web   -> Order, Stock"));
    let d = diff(&doc(BASE), &head);
    assert_eq!(d.changes.len(), 2);
    assert!(d.changes.iter().any(|c| c.kind == Kind::Added));
    assert!(d.changes.iter().any(|c| c.kind == Kind::Removed));
    assert!(d.changes.iter().all(|c| c.class == Class::Topology));
}

#[test]
fn doi_invariant_la_thay_doi_ngu_nghia() {
    let head = doc(&BASE.replace("verify property_test", "verify property_test, tla_model"));
    let d = diff(&doc(BASE), &head);
    assert!(d.changes.iter().all(|c| c.class == Class::Semantic), "nhận: {:#?}", d.changes);
}

#[test]
fn bo_component_la_thay_doi_cau_truc() {
    let head = doc(&BASE.replace("component Stock   at src/stock/**    @human #accepted\n", "").replace("allow Order -> Stock\n", ""));
    let d = diff(&doc(BASE), &head);
    assert_eq!(d.changes.len(), 2);
    assert!(d.changes.iter().all(|c| c.kind == Kind::Removed));
}

#[test]
fn khong_doi_gi_thi_khong_bao_gi() {
    let d = diff(&doc(BASE), &doc(BASE));
    assert!(d.is_quiet());
    assert_eq!(d.moved, 0);
    assert!(d.unchanged > 0);
}

/// Khai nhận `unchecked` làm `--strict` bỏ qua một phần mã, nên thêm nó hay
/// bật nó từ `#proposed` lên là nới lỏng; gỡ nó thì không.
#[test]
fn them_unchecked_la_noi_long() {
    let line = "unchecked Web imports java because \"chỉ gọi gRPC\"";
    let base = doc(BASE);
    let added = doc(&format!("{BASE}{line}\n"));
    let keys = |a: &Document, b: &Document| -> Vec<String> { diff(a, b).loosenings().map(|c| c.key.clone()).collect() };
    assert_eq!(keys(&base, &added), ["Web imports java"]);
    assert!(keys(&added, &base).is_empty(), "gỡ khai nhận là siết lại");
    let proposed = doc(&format!("{BASE}{line} #proposed\n"));
    assert!(keys(&base, &proposed).is_empty(), "#proposed chưa có hiệu lực");
    assert_eq!(keys(&proposed, &added), ["Web imports java"]);
}
