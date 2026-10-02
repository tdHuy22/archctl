//! Test cho `api` — nhóm hàm và từ khoá bị theo dõi.
//!
//! Giá trị của loại luật này so với `grep` nằm ở chỗ nó chạy trên dòng token:
//! một `strcpy` trong chú thích hay trong chuỗi không bao giờ khớp. Phần lớn
//! test ở đây là về chuyện đó.

use arch_core::check::{Reason, Via, check};
use arch_core::scan::Lang;
use std::path::{Path, PathBuf};

fn hits(lang: Lang, src: &str, needles: &[&str]) -> Vec<(String, usize)> {
    lang.scan(src, needles).symbols.into_iter().map(|h| (h.symbol, h.line)).collect()
}

// ---------- phát hiện ký hiệu ----------

#[test]
fn ky_hieu_trong_chu_thich_va_chuoi_khong_khop() {
    let src = "\
/* strcpy nằm trong chú thích */
// strcpy nữa
static const char *s = \"strcpy\";
int f(void) { strcpy(a, b); return 0; }
";
    assert_eq!(hits(Lang::C, src, &["strcpy"]), [("strcpy".into(), 4)]);
}

#[test]
fn tien_to_du_dieu_kien_van_khop() {
    // `builtins.eval` là `eval` được gọi qua tiền tố, vẫn phải bắt.
    assert_eq!(hits(Lang::Python, "x = builtins.eval(s)", &["eval"]).len(), 1);
    assert_eq!(hits(Lang::Rust, "std::ptr::copy(a, b, n);", &["copy"]).len(), 1);
}

#[test]
fn ten_dai_hon_khong_bi_nham() {
    // `strcpy_s` và `my_eval` là hàm khác, không được tính.
    assert!(hits(Lang::C, "strcpy_s(a, b, n);", &["strcpy"]).is_empty());
    assert!(hits(Lang::Python, "my_eval(s)", &["eval"]).is_empty());
}

#[test]
fn tu_khoa_cua_rust_duoc_coi_la_ky_hieu() {
    let src = "fn f() { unsafe { g() } }\n// unsafe trong chú thích\n";
    assert_eq!(hits(Lang::Rust, src, &["unsafe"]), [("unsafe".into(), 1)]);
}

#[test]
fn khong_khai_bao_api_thi_khong_quet_gi() {
    assert!(Lang::C.scan("strcpy(a, b);", &[]).symbols.is_empty());
}

// ---------- đọc luật ----------

#[test]
fn doc_duoc_khai_bao_api() {
    let (doc, diags) = arch_core::load("api Unsafe at \"strcpy\", \"gets\" @human #locked");
    assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
    assert_eq!(doc.apis[0].name, "Unsafe");
    assert_eq!(doc.apis[0].symbols, ["strcpy", "gets"]);
    assert_eq!(doc.apis[0].meta.status, arch_core::Status::Locked);
}

#[test]
fn api_khong_co_ky_hieu_nao_la_loi() {
    let (_, diags) = arch_core::load("api Unsafe at");
    assert!(arch_core::diag::has_errors(&diags));
}

#[test]
fn api_trung_ten_voi_component_la_loi() {
    let (_, diags) = arch_core::load("component Net at n/**\napi Net at \"socket\"");
    assert!(diags.iter().any(|d| d.msg.contains("trùng tên")), "{diags:#?}");
}

// ---------- đầu-tới-cuối trên examples/sensor ----------

fn report() -> arch_core::check::Report {
    let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sensor");
    let src = std::fs::read_to_string(root.join(arch_core::ARCH_FILE)).unwrap();
    let (doc, diags) = arch_core::load(&src);
    assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
    check(&root, &doc, &[])
}

#[test]
fn bat_duoc_strcpy_va_dem_dung_so_lan() {
    let r = report();
    let v = r.violations.iter().find(|v| v.to == "Unsafe").expect("phải bắt được strcpy");
    assert_eq!(v.via, Via::Symbol);
    assert_eq!(v.from, "Native");
    assert_eq!(v.spec, "strcpy");
    assert_eq!(v.count, 2, "chú thích và chuỗi không được tính — grep sẽ ra 4");
    assert_eq!(v.line, 18, "báo ở lần gặp thật đầu tiên");
}

#[test]
fn bat_duoc_eval_cua_python() {
    let r = report();
    let v = r.violations.iter().find(|v| v.to == "Danger").expect("phải bắt được eval");
    assert_eq!(v.from, "Tools");
    assert_eq!(v.spec, "eval");
}

/// `deny * -> RustUnsafe except Decode` là **miễn trừ** cho Decode, không chỉ
/// là "deny không áp dụng". Thiếu bước này thì Decode rơi xuống nhánh "không
/// có allow nào" và bị báo vi phạm, trái hẳn ý người viết luật.
#[test]
fn except_la_mien_tru_chu_khong_chi_la_khong_cam() {
    let r = report();
    assert!(
        !r.violations.iter().any(|v| v.from == "Decode" && v.to == "RustUnsafe"),
        "Decode được miễn trừ nhưng vẫn bị báo: {:#?}",
        r.violations
    );
    // Component khác dùng `unsafe` thì vẫn bị bắt.
    let capture = r.violations.iter().find(|v| v.to == "RustUnsafe").unwrap();
    assert_eq!(capture.from, "Capture");
    assert_eq!(capture.reason, Reason::Denied);
    assert!(capture.hint.contains("chỉ Decode"), "nhận: {}", capture.hint);
}

#[test]
fn vi_pham_theo_ky_hieu_va_theo_import_deu_co_mat() {
    let r = report();
    let by_symbol = r.violations.iter().filter(|v| v.via == Via::Symbol).count();
    let by_import = r.violations.iter().filter(|v| v.via == Via::Import).count();
    assert_eq!(by_symbol, 3, "strcpy, unsafe, eval");
    assert_eq!(by_import, 4, "mỗi ngôn ngữ một cái");
}
