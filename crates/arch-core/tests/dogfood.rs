//! archctl tự áp luật kiến trúc lên chính nó.
//!
//! Đây vừa là ví dụ thật nhất của công cụ, vừa là lan can cho chính nó: thêm
//! một `use` sai hướng là `cargo test` báo ngay, không đợi tới lúc ai đó nhìn
//! sơ đồ.
//!
//! Luật nằm ở `.arch/system.arch` ở gốc repo. Nó được viết tay từ thiết kế,
//! không phải do `init --from-code` đoán ra.

use arch_core::check::check;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn ma_cua_archctl_khong_vi_pham_kien_truc_cua_chinh_no() {
    let root = repo();
    let src = std::fs::read_to_string(root.join(arch_core::ARCH_FILE)).expect("có file luật");
    let (doc, diags) = arch_core::load(&src);
    assert!(!arch_core::diag::has_errors(&diags), "file luật của chính repo phải sạch: {diags:#?}");

    let report = check(&root, &doc, &arch_core::read_ignores(&root));
    let broken: Vec<String> = report
        .violations
        .iter()
        .map(|v| format!("{} -> {} ({}:{} {})", v.from, v.to, v.file, v.line, v.spec))
        .collect();
    assert!(broken.is_empty(), "vi phạm kiến trúc:\n  {}", broken.join("\n  "));
}

/// Luật chỉ có tác dụng với component **đang trong diện kiểm**. Nếu một
/// component rơi ra khỏi diện đó, luật về nó im lặng ngừng áp dụng.
#[test]
fn moi_component_deu_trong_dien_kiem() {
    let root = repo();
    let src = std::fs::read_to_string(root.join(arch_core::ARCH_FILE)).unwrap();
    let doc = arch_core::load(&src).0;
    let report = check(&root, &doc, &arch_core::read_ignores(&root));
    assert!(report.unchecked.is_empty(), "ngoài diện kiểm: {:?}", report.unchecked);
}

/// Mã của archctl phải nằm hết trong sơ đồ. Một file rơi ra ngoài thì mọi
/// luật về nó đều vô nghĩa.
#[test]
fn khong_file_nao_cua_archctl_nam_ngoai_so_do() {
    let root = repo();
    let src = std::fs::read_to_string(root.join(arch_core::ARCH_FILE)).unwrap();
    let doc = arch_core::load(&src).0;
    let report = check(&root, &doc, &arch_core::read_ignores(&root));
    assert_eq!(report.files_unmapped, 0, "{} file ngoài sơ đồ", report.files_unmapped);
}
