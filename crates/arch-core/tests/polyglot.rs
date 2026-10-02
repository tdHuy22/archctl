//! Test đầu-tới-cuối trên `examples/sensor`: bốn ngôn ngữ trong một repo,
//! mỗi ngôn ngữ cài sẵn một vi phạm và vài cái bẫy báo động giả.

use arch_core::check::{Reason, Report, Via, check};
use arch_core::scan::Lang;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sensor")
}

fn report() -> Report {
    let root = root();
    let src = std::fs::read_to_string(root.join(arch_core::ARCH_FILE)).unwrap();
    let (doc, diags) = arch_core::load(&src);
    assert!(!arch_core::diag::has_errors(&diags), "file luật mẫu phải sạch: {diags:#?}");
    check(&root, &doc, &[])
}

#[test]
fn moi_ngon_ngu_bi_bat_dung_mot_vi_pham() {
    let r = report();
    let found: Vec<(&str, &str, &str)> = r
        .violations
        .iter()
        .filter(|v| v.via == Via::Import)
        .map(|v| (v.from.as_str(), v.to.as_str(), v.file.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            ("Stream", "Ctl", "internal/stream/stream.go"),
            ("Native", "LibPcap", "native/parser.c"),
            ("Capture", "Report", "src/capture/mod.rs"),
            ("ToolsLib", "Tools", "tools/lib/io.py"),
        ],
        "nhận: {:#?}",
        r.violations
    );
}

#[test]
fn bon_ngon_ngu_deu_duoc_quet() {
    let r = report();
    let langs: Vec<Lang> = r.by_lang.iter().map(|(l, _)| *l).collect();
    for want in [Lang::Rust, Lang::C, Lang::Python, Lang::Go] {
        assert!(langs.contains(&want), "thiếu {}: {:?}", want.name(), r.by_lang);
    }
    assert_eq!(r.files_scanned, 15);
}

/// `native/parser.c` có một `#include <pcap.h>` thật và ba dòng giả: hai trong
/// chú thích, một trong chuỗi. Chỉ được báo đúng một vi phạm, ở dòng của cái
/// thật.
#[test]
fn include_trong_chu_thich_va_chuoi_khong_sinh_vi_pham_gia() {
    let r = report();
    let pcap: Vec<&_> = r.violations.iter().filter(|v| v.to == "LibPcap").collect();
    assert_eq!(pcap.len(), 1, "nhận: {pcap:#?}");
    assert_eq!(pcap[0].line, 6);
}

/// Nếu lexer coi `'a` trong `fn label<'a>(...)` là mở chuỗi, nó sẽ nuốt phần
/// còn lại của file và bỏ sót dòng `use crate::report::Summary`.
#[test]
fn lifetime_cua_rust_khong_lam_bo_sot_use() {
    let r = report();
    assert!(
        r.violations.iter().any(|v| v.from == "Capture" && v.to == "Report"),
        "nhận: {:#?}",
        r.violations
    );
}

/// `tools/analyze.py` có hai dòng trông như import trong docstring và một
/// trong chuỗi thường.
#[test]
fn docstring_cua_python_khong_sinh_import_gia() {
    let r = report();
    assert!(
        !r.violations
            .iter()
            .any(|v| v.via == Via::Import && v.from == "Tools" && v.file.contains("analyze")),
        "nhận: {:#?}",
        r.violations
    );
}

#[test]
fn go_doc_module_tu_go_mod_de_phan_biet_noi_bo_voi_ben_ngoai() {
    let r = report();
    let go = r.violations.iter().find(|v| v.file.ends_with(".go")).unwrap();
    assert_eq!(go.reason, Reason::NoAllow);
    // `net` là gói ngoài và được allow; chỉ đường dẫn trong module mới thành cạnh.
    assert!(!r.violations.iter().any(|v| v.to == "GoNet"));
}

/// Rust gọi C qua FFI chứ không qua `use`, nên cạnh Decode -> Native vô hình
/// với mọi phân tích import. Nhưng hai đầu mang cùng một cái tên — `extern "C"
/// { fn parse_ethernet }` bên Rust, `int parse_ethernet(…) {` bên C — và cái
/// tên đó phải khớp thì chương trình mới liên kết được.
#[test]
fn canh_ffi_giua_rust_va_c_duoc_ghep_lai() {
    let r = report();
    let ffi = r
        .violations
        .iter()
        .find(|v| v.from == "Decode" && v.to == "Native")
        .unwrap_or_else(|| panic!("thiếu cạnh FFI: {:#?}", r.violations));
    assert_eq!(ffi.via, Via::Ffi);
    assert_eq!(ffi.spec, "parse_ethernet");
    assert_eq!(ffi.file, "src/decode/ethernet.rs");
    assert_eq!(ffi.reason, Reason::NoAllow, "`.arch` không có allow Decode -> Native");
}

/// Ghép theo tên nên chỉ đúng khi cái tên thật sự đi qua ranh giới. `summarize`
/// được định nghĩa trong Rust và không có `extern` nào, nên nó không được sinh
/// ra cạnh nào cả.
#[test]
fn ham_khong_qua_ranh_gioi_thi_khong_sinh_canh_ffi() {
    let r = report();
    let ffi: Vec<&str> =
        r.violations.iter().filter(|v| v.via == Via::Ffi).map(|v| v.spec.as_str()).collect();
    assert_eq!(ffi, ["parse_ethernet"], "chỉ đúng một cạnh FFI trong fixture");
}

#[test]
fn header_he_thong_khong_khai_bao_thi_bo_qua() {
    let r = report();
    // `<endian.h>`, `<machine/endian.h>` không có trong `external` nào.
    assert!(r.unresolved.is_empty(), "nhận: {:#?}", r.unresolved);
}

#[test]
fn component_chua_co_allow_thi_chua_vao_dien_kiem() {
    let r = report();
    assert_eq!(r.unchecked, ["Report"]);
}

/// `use super::Import` không có đoạn module nào sau `super`, nên đích chính là
/// module cha. Trước đây bộ phân giải chỉ thử từ đoạn thứ nhất trở đi nên bỏ
/// qua chính nó — và một cạnh thật biến mất.
#[test]
fn super_tro_thang_vao_module_cha() {
    use arch_core::resolve::{Resolved, Resolver};
    use arch_core::scan::Import;

    let root = root();
    let r = Resolver::new(&root);
    let at = |spec: &str| r.resolve(Lang::Rust, "src/capture/decoder.rs", &Import { spec: spec.into(), line: 1, system: false });

    // `src/capture/mod.rs` tồn tại trong fixture; `super::Thing` phải ra nó.
    assert_eq!(at("super::Thing"), Resolved::Path("src/capture/mod.rs".into()));
    assert_eq!(at("super::*"), Resolved::Path("src/capture/mod.rs".into()));
}

/// `mod` viết lồng trong file không có file đích. Trình biên dịch đã bảo đảm
/// đường dẫn tồn tại, nên báo "không tìm thấy module" là nói sai — và nói sai
/// ở quy mô mỗi `#[cfg(test)] mod tests { use super::*; }` một lần.
#[test]
fn mod_long_trong_file_khong_bi_bao_la_thieu() {
    use arch_core::resolve::{Resolved, Resolver};
    use arch_core::scan::Import;

    let r = Resolver::new(&root());
    let at = |from: &str, spec: &str| r.resolve(Lang::Rust, from, &Import { spec: spec.into(), line: 1, system: false });

    assert_eq!(at("src/report.rs", "crate::khong_he_co_module_nay"), Resolved::Internal);
    assert_eq!(at("src/capture/mod.rs", "super::*"), Resolved::Internal, "`src` không phải module");
    // `self::helper` ra chính file đang xét; cạnh tự trỏ vào mình bị bỏ sau đó.
    assert_eq!(at("src/report.rs", "self::helper"), Resolved::Path("src/report.rs".into()));
}
