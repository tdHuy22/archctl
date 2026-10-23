//! Test đầu-tới-cuối trên `examples/ml`: một dự án ML/LLM, mỗi công thức luật
//! trong `docs/ml.md` cài sẵn đúng một vi phạm, cộng một vi phạm trong notebook. Tài liệu nói luật nào bắt được
//! gì; test này giữ cho câu đó còn đúng.

use arch_core::check::{Report, Via, check};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/ml")
}

fn report() -> Report {
    let root = root();
    let src = std::fs::read_to_string(root.join(arch_core::ARCH_FILE)).unwrap();
    let (doc, diags) = arch_core::load(&src);
    assert!(!arch_core::diag::has_errors(&diags), "file luật mẫu phải sạch: {diags:#?}");
    check(&root, &doc, &[])
}

#[test]
fn moi_cong_thuc_bat_dung_mot_vi_pham() {
    let r = report();
    let found: Vec<(&str, &str, &str, Via)> = r
        .violations
        .iter()
        .map(|v| (v.from.as_str(), v.to.as_str(), v.file.as_str(), v.via))
        .collect();
    assert_eq!(
        found,
        [
            ("Web", "Train", "dvc.yaml", Via::Pipeline),
            ("Data", "Torch", "mlapp/data/loader.py", Via::Import),
            ("Rag", "OpenAI", "mlapp/rag/retriever.py", Via::Import),
            ("Serve", "Unsafe", "mlapp/serve/api.py", Via::Symbol),
            ("Serve", "Train", "mlapp/serve/api.py", Via::Import),
            ("Train", "Kernels", "mlapp/train/loop.py", Via::Ffi),
            ("Web", "RateLimitError", "mlapp/web/app.py", Via::Symbol),
            ("Notebooks", "OpenAI", "notebooks/eda.ipynb", Via::Import),
        ],
        "nhận: {:#?}",
        r.violations
    );
}

/// Lớp bọc `Ops` gọi đúng op đó qua `torch.ops` nhưng được phép; lỗi của nhà
/// cung cấp nằm trong chính cổng LLM thì không phải rò rỉ; và `model.eval()`
/// của PyTorch không phải `eval` — công thức cấm `pickle` không được bắt nó.
#[test]
fn duong_hop_le_khong_bi_bao() {
    let r = report();
    assert!(!r.violations.iter().any(|v| v.file.starts_with("mlapp/models/")), "{:#?}", r.violations);
    assert!(!r.violations.iter().any(|v| v.file.starts_with("mlapp/ops/")), "{:#?}", r.violations);
    assert!(!r.violations.iter().any(|v| v.file.starts_with("mlapp/llm/")), "{:#?}", r.violations);
    assert_eq!(r.files_scanned, 23);
}

/// Vi phạm trong notebook trỏ vào đúng dòng của file `.ipynb`: dòng 30 là
/// chuỗi JSON chứa `import openai`. Dòng `!pip install openai` là magic, không
/// phải import.
#[test]
fn notebook_bao_dung_dong_trong_file() {
    let r = report();
    let nb: Vec<(usize, &str)> = r
        .violations
        .iter()
        .filter(|v| v.file.ends_with(".ipynb"))
        .map(|v| (v.line, v.spec.as_str()))
        .collect();
    assert_eq!(nb, [(30, "openai")]);
}
