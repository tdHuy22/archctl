//! Cạnh phụ thuộc đi qua ranh giới ngôn ngữ.
//!
//! Rust gọi C không qua câu lệnh import nào, nên cạnh đó vô hình với phân tích
//! import. Nhưng hai đầu mang cùng một cái tên, và cái tên đó phải khớp thì
//! chương trình mới liên kết được.
//!
//! Ghép theo tên nên cái đáng kiểm nhất là **chỗ không được ghép**: một lời
//! gọi hàm bình thường, một khai báo trước, một biến tên `C`.

use arch_core::check::{Via, check};
use std::path::PathBuf;

const ARCH: &str = "\
component App    at app/**    @human #accepted
component Native at native/** @human #accepted
component O      at o/**      @human #accepted
allow App -> O
allow Native -> O
";

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root =
            std::env::temp_dir().join(format!("archctl-ffi-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = Repo(root);
        repo.write(".arch/system.arch", ARCH);
        repo
    }

    fn write(&self, rel: &str, body: &str) -> &Repo {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    /// Các cạnh liên ngôn ngữ bắt được, dạng `từ -> tới qua tên`.
    fn edges(&self) -> Vec<String> {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        let (doc, diags) = arch_core::load(&src);
        assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
        let mut out: Vec<String> = check(&self.0, &doc, &[])
            .violations
            .iter()
            .filter(|v| v.via == Via::Ffi)
            .map(|v| format!("{} -> {} qua {}", v.from, v.to, v.spec))
            .collect();
        out.sort();
        out
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn rust_goi_c_qua_extern_block() {
    let repo = Repo::new("rust-c");
    repo.write(
        "app/decode.rs",
        "unsafe extern \"C\" {\n    fn aes_encrypt(buf: *const u8, len: usize) -> i32;\n}\n\
         pub fn go(b: &[u8]) -> i32 { unsafe { aes_encrypt(b.as_ptr(), b.len()) } }\n",
    )
    .write(
        "native/crypto.c",
        "int aes_encrypt(const char *buf, unsigned long len) { return (int)len; }\n",
    );
    assert_eq!(repo.edges(), ["App -> Native qua aes_encrypt"]);
}

#[test]
fn go_goi_c_qua_cgo() {
    let repo = Repo::new("go-c");
    repo.write(
        "app/main.go",
        "package app\n\n/*\n#include \"crypto.h\"\n*/\nimport \"C\"\n\n\
         func Encrypt(n int) int { return int(C.aes_encrypt(C.CString(\"x\"), C.ulong(n))) }\n",
    )
    .write("native/crypto.c", "int aes_encrypt(const char *b, unsigned long n) { return 0; }\n");
    // `C.CString` và `C.ulong` là của chính cgo, không khớp định nghĩa nào
    // trong repo nên tự rụng ở bước ghép.
    assert_eq!(repo.edges(), ["App -> Native qua aes_encrypt"]);
}

#[test]
fn go_goi_ham_rust_dua_ra_cho_c() {
    let repo = Repo::new("go-rust");
    repo.write("app/lib.rs", "#[unsafe(no_mangle)]\npub extern \"C\" fn app_tick(n: u32) -> u32 { n + 1 }\n")
        .write(
            "native/call.go",
            "package native\n\n/*\nextern unsigned app_tick(unsigned n);\n*/\nimport \"C\"\n\n\
             func Bump(n uint) uint { return uint(C.app_tick(C.uint(n))) }\n",
        );
    assert_eq!(repo.edges(), ["Native -> App qua app_tick"]);
}

#[test]
fn khai_bao_truoc_khong_phai_la_cung_cap() {
    // `int f(void);` kết thúc bằng `;` chứ không mở thân hàm, nên nó không
    // phải là nơi hàm được định nghĩa.
    let repo = Repo::new("prototype");
    repo.write("app/decode.rs", "unsafe extern \"C\" {\n    fn only_declared(n: u32);\n}\n")
        .write("native/crypto.c", "void only_declared(unsigned n);\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn loi_goi_trong_than_ham_khong_phai_la_dinh_nghia() {
    let repo = Repo::new("innercall");
    repo.write("app/decode.rs", "unsafe extern \"C\" {\n    fn helper(n: u32);\n}\n")
        .write("native/crypto.c", "int outer(void) {\n    helper(1);\n    return 0;\n}\n");
    assert!(repo.edges().is_empty(), "lời gọi lồng bên trong không tạo ra cạnh");
}

#[test]
fn tu_khoa_dieu_khien_khong_phai_ten_ham() {
    let repo = Repo::new("control");
    repo.write("app/decode.rs", "unsafe extern \"C\" {\n    fn r#while(n: u32);\n}\n")
        .write("native/crypto.c", "#define M(x) while (x) { }\nint f(void) { return 0; }\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn go_khong_import_cgo_thi_khong_co_canh() {
    let repo = Repo::new("notcgo");
    repo.write(
        "app/main.go",
        "package app\n\ntype box struct{ aes_encrypt func() }\n\nfunc Run(C box) { C.aes_encrypt() }\n",
    )
    .write("native/crypto.c", "int aes_encrypt(void) { return 0; }\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn hai_component_cung_cung_cap_mot_ten_thi_bo_ca_hai() {
    // Không biết chọn đâu, mà đoán sai thì sinh ra một cạnh không có thật.
    let repo = Repo::new("ambiguous");
    repo.write(".arch/system.arch", &format!("{ARCH}component Other at other/** @human #accepted\n"))
        .write("app/decode.rs", "unsafe extern \"C\" {\n    fn tick(n: u32);\n}\n")
        .write("native/a.c", "int tick(unsigned n) { return 0; }\n")
        .write("other/b.c", "int tick(unsigned n) { return 1; }\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn ffi_trong_cung_mot_component_khong_phai_la_canh() {
    let repo = Repo::new("samecomp");
    repo.write("app/decode.rs", "unsafe extern \"C\" {\n    fn tick(n: u32);\n}\n")
        .write("app/impl.c", "int tick(unsigned n) { return 0; }\n");
    assert!(repo.edges().is_empty());
}

#[test]
fn ham_he_thong_khong_ghep_duoc_thi_im_lang() {
    // `memcpy` nằm trong libc, không có định nghĩa nào trong repo. Không có
    // cạnh, và cũng không có cảnh báo: FFI tới thư viện hệ thống là chuyện
    // thường, nên báo ra sẽ chỉ là tiếng ồn.
    let repo = Repo::new("libc");
    repo.write("app/decode.rs", "unsafe extern \"C\" {\n    fn memcpy(d: *mut u8, s: *const u8, n: usize);\n}\n");
    assert!(repo.edges().is_empty());
}

#[test]
fn macro_dang_dinh_nghia_ham_khong_phai_la_cung_cap() {
    // libbpfgo: chương trình eBPF viết `s32 BPF_STRUCT_OPS(simple_enqueue, …) {`,
    // một macro bọc định nghĩa hàm. Phía Go có `C.BPF_STRUCT_OPS`, nhưng đó là
    // hằng enum của kernel. Tên viết hoa toàn bộ là quy ước của macro.
    let repo = Repo::new("macro");
    repo.write(
        "app/prog.go",
        "package app\n\n// #include <linux/bpf.h>\nimport \"C\"\n\nconst T = C.BPF_STRUCT_OPS\n",
    )
    .write("native/sched.bpf.c", "s32 BPF_STRUCT_OPS(simple_enqueue, u64 flags) { return 0; }\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn ham_c_viet_hoa_van_la_cung_cap() {
    // OpenSSL có `SHA256`, `MD5`, `HMAC`: hàm thật, tên viết hoa. Khác macro
    // ở chỗ trong ngoặc là tham số có kiểu, không phải một tên trần.
    let repo = Repo::new("upper");
    repo.write(
        "app/hash.rs",
        "unsafe extern \"C\" {\n    fn SHA256(d: *const u8, n: usize, md: *mut u8) -> *mut u8;\n    fn RAND(v: u32) -> u32;\n}\n",
    )
    .write(
        "native/sha.c",
        "unsigned char *SHA256(const unsigned char *d, size_t n, unsigned char *md) { return md; }\n\
         unsigned RAND(void) { return 4; }\n",
    );
    assert_eq!(repo.edges(), ["App -> Native qua RAND", "App -> Native qua SHA256"]);
}

#[test]
fn ham_ben_trong_doi_so_macro_khong_phai_la_cung_cap() {
    let repo = Repo::new("wrap");
    repo.write("app/decode.rs", "unsafe extern \"C\" {\n    fn inner(n: u32);\n}\n")
        .write("native/w.c", "WRAP(static int inner(int x) { return x; })\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn c_ham_trong_khoi_extern_c_van_la_cung_cap() {
    // Mã C++ đưa hàm ra cho C gọi thường bọc trong `extern "C" { ... }`. Khối
    // đó không phải thân hàm: hàm định nghĩa bên trong vẫn ở tầng ngoài cùng,
    // và tên của chúng không bị mangle nên Rust gọi được.
    let repo = Repo::new("extern-c-block");
    repo.write(
        "app/decode.rs",
        "unsafe extern \"C\" {\n    fn wrapped_parse(b: *const u8) -> i32;\n}\n",
    )
    .write(
        "native/wrap.cc",
        "#include \"wrap.h\"\nextern \"C\" {\nint wrapped_parse(const char *b) { return 0; }\n}\n",
    );
    assert_eq!(repo.edges(), ["App -> Native qua wrapped_parse"]);
}

#[test]
fn c_ham_trong_namespace_khong_phai_cung_cap() {
    // Ngược lại, hàm C++ trong namespace mang tên đã mangle: Rust khai
    // `fn helper` thì không liên kết được tới `ns::helper`. Không ghép.
    let repo = Repo::new("cpp-namespace");
    repo.write("app/decode.rs", "unsafe extern \"C\" {\n    fn helper(x: i32) -> i32;\n}\n")
        .write("native/ns.cc", "namespace ns {\nint helper(int x) { return x; }\n}\n");
    assert_eq!(repo.edges(), Vec::<String>::new());
}

#[test]
fn rust_extern_khong_ghi_abi_van_la_goi() {
    // `extern { ... }` không ghi ABI mặc định là "C".
    let repo = Repo::new("rust-extern-noabi");
    repo.write("app/decode.rs", "extern {\n    fn legacy_init() -> i32;\n}\n")
        .write("native/legacy.c", "int legacy_init(void) { return 0; }\n");
    assert_eq!(repo.edges(), ["App -> Native qua legacy_init"]);
}

// ---- Python gọi mã native ----
//
// Python không khai báo tên hàm native như `extern "C"`. Thứ hai đầu cùng
// mang là **tên module mở rộng**: phía native đặt tên nó (`#[pymodule] fn
// _core`, `PYBIND11_MODULE(_core, m)`), phía Python import đúng tên đó. Và
// với op của PyTorch là tên op: `m.def("op(...)")` ↔ `torch.ops.ns.op`.
//
// Chỉ tính import mà trong repo không có mã Python nào trả lời: không có file,
// hoặc chỉ có stub `.pyi`. Import trỏ tới file `.py` là cạnh Python thường;
// import gói bên ngoài (`torch._C`) không bao giờ là module của repo này.

impl Repo {
    /// Cảnh báo "chưa phân giải", dạng `file:dòng spec`.
    fn unresolved(&self) -> Vec<String> {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        let (doc, _) = arch_core::load(&src);
        check(&self.0, &doc, &[])
            .unresolved
            .iter()
            .map(|u| format!("{}:{} {}", u.file, u.line, u.spec))
            .collect()
    }
}

#[test]
fn pyo3_module_ghep_voi_import_python() {
    let repo = Repo::new("pyo3-fn");
    repo.write(
        "native/src/lib.rs",
        "use pyo3::prelude::*;\n\n#[pymodule]\nfn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {\n    Ok(())\n}\n",
    )
    .write("app/pkg/__init__.py", "from ._core import Validator\n");
    assert_eq!(repo.edges(), ["App -> Native qua ._core"]);
}

#[test]
fn pyo3_module_dang_mod_co_doi_ten() {
    // `#[pyo3(name = "...")]` đặt tên module mà Python thấy; tên `mod` trong
    // Rust khi đó không còn là tên module.
    let repo = Repo::new("pyo3-mod");
    repo.write(
        "native/src/lib.rs",
        "#[pymodule(gil_used = false)]\n#[pyo3(name = \"_rust\")]\npub mod inner {\n    #[pymodule_export]\n    use super::Thing;\n}\n",
    )
    .write("app/__init__.py", "")
    .write("app/use.py", "import app._rust\nimport app.inner\n");
    assert_eq!(repo.edges(), ["App -> Native qua app._rust"]);
}

#[test]
fn stub_pyi_khong_che_module_native() {
    // pydantic-core: `_pydantic_core.pyi` nằm cạnh `__init__.py`, nên import
    // phân giải tới stub. Stub chỉ mô tả kiểu; mã chạy thật ở phía Rust.
    let repo = Repo::new("pyo3-stub");
    repo.write("native/src/lib.rs", "#[pymodule]\nmod _core {}\n")
        .write("app/pkg/__init__.py", "from ._core import Validator\n")
        .write("app/pkg/_core.pyi", "class Validator: ...\n");
    assert_eq!(repo.edges(), ["App -> Native qua ._core"]);
}

#[test]
fn module_python_cung_ten_khong_phai_module_native() {
    let repo = Repo::new("pyo3-shadow");
    repo.write("native/src/lib.rs", "#[pymodule]\nfn _core(m: &Bound<'_, PyModule>) {}\n")
        .write("app/pkg/__init__.py", "from ._core import Validator\n")
        .write("app/pkg/_core.py", "class Validator: ...\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn goi_ngoai_cung_ten_khong_phai_module_cua_repo() {
    // `torch._C` là module mở rộng của PyTorch, không phải `_C` của repo này.
    let repo = Repo::new("pybind-external");
    repo.write("native/bind.cpp", "PYBIND11_MODULE(_C, m) {\n    m.def(\"add\", &add);\n}\n")
        .write("app/x.py", "import torch._C\nfrom torch import _C\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn pybind11_module_ghep_voi_from_import() {
    let repo = Repo::new("pybind-from");
    repo.write(
        "native/bind.cpp",
        "#include <pybind11/pybind11.h>\nint add(int a, int b) { return a + b; }\n\
         PYBIND11_MODULE(_ext, m, py::mod_gil_not_used()) {\n    m.def(\"add\", &add);\n}\n",
    )
    .write("app/__init__.py", "")
    .write("app/x.py", "from app import _ext\n");
    assert_eq!(repo.edges(), ["App -> Native qua app._ext"]);
}

#[test]
fn nanobind_module_ghep_voi_import() {
    let repo = Repo::new("nanobind");
    repo.write("native/nb.cpp", "NB_MODULE(_nb, m) {\n    m.def(\"add\", [](int a) { return a; });\n}\n")
        .write("app/__init__.py", "")
        .write("app/x.py", "import app._nb as nb\n");
    assert_eq!(repo.edges(), ["App -> Native qua app._nb"]);
}

#[test]
fn c_api_pyinit_la_module_mo_rong() {
    // Mọi module mở rộng, viết tay hay qua pybind11, cuối cùng là `PyInit_<tên>`.
    let repo = Repo::new("pyinit");
    repo.write(
        "native/spam.c",
        "#include <Python.h>\nPyMODINIT_FUNC PyInit__spam(void) {\n    return NULL;\n}\n",
    )
    .write("app/__init__.py", "")
    .write("app/x.py", "from app import _spam\n");
    assert_eq!(repo.edges(), ["App -> Native qua app._spam"]);
}

#[test]
fn module_native_ghep_duoc_thi_khong_con_bao_chua_phan_giai() {
    let repo = Repo::new("pyo3-unresolved");
    repo.write("app/pkg/__init__.py", "from ._core import Validator\nfrom ._version import v\n");
    // Chưa có phía native: cả hai đều chưa phân giải.
    assert_eq!(
        repo.unresolved(),
        ["app/pkg/__init__.py:1 ._core", "app/pkg/__init__.py:2 ._version"]
    );
    repo.write("native/src/lib.rs", "#[pymodule]\nfn _core(m: &Bound<'_, PyModule>) {}\n");
    assert_eq!(repo.unresolved(), ["app/pkg/__init__.py:2 ._version"]);
}

#[test]
fn torch_op_ghep_theo_namespace_va_ten() {
    let repo = Repo::new("torch-op");
    repo.write(
        "native/ops.cpp",
        "#include <torch/library.h>\nTORCH_LIBRARY(mylib, m) {\n  m.def(\"fused_add(Tensor a, Tensor b) -> Tensor\");\n}\n",
    )
    .write("app/k.py", "import torch\n\ndef f(a, b):\n    return torch.ops.mylib.fused_add(a, b)\n");
    assert_eq!(repo.edges(), ["App -> Native qua torch.ops.mylib.fused_add"]);
}

#[test]
fn torch_op_namespace_la_macro_thi_khong_ghep() {
    // vllm: namespace là `TORCH_EXTENSION_NAME`, do CMake định nghĩa lúc build
    // và khác nhau giữa các module mở rộng. Không đọc được giá trị thì không
    // ghép: `vllm_ir::rms_norm` đăng ký bằng Python, `oink::` của plugin ngoài,
    // `_vllm_fa3_C` của flash-attention tải lúc build đều trùng tên với op
    // viết dưới macro đó.
    let repo = Repo::new("torch-op-macro");
    repo.write(
        "native/torch_bindings.cpp",
        "TORCH_LIBRARY_EXPAND(TORCH_EXTENSION_NAME, ops) {\n\
         \x20 ops.def(\"rms_norm(Tensor! result, Tensor input,\"\n\
         \x20         \"Tensor weight, float epsilon) -> ()\");\n}\n\
         TORCH_LIBRARY_EXPAND(CONCAT(TORCH_EXTENSION_NAME, _cache_ops), cache_ops) {\n\
         \x20 cache_ops.def(\"swap_blocks(Tensor src, Tensor! dst) -> ()\");\n}\n",
    )
    .write(
        "app/ops.py",
        "import torch\nimport app._C\nfrom torch.library import Library\nlib = Library(\"vllm_ir\", \"FRAGMENT\")\n\
         torch.ops._C.rms_norm(o, x, w, e)\ntorch.ops._C_cache_ops.swap_blocks(s, d)\n\
         torch.ops.vllm_ir.rms_norm(x)\ntorch.ops.oink.rms_norm(x)\n",
    );
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn torch_op_namespace_viet_thang_van_ghep_canh_khoi_macro() {
    // Cùng một repo có cả hai dạng: khối viết thẳng `_C` vẫn ghép được.
    let repo = Repo::new("torch-op-mixed");
    repo.write(
        "native/a.cpp",
        "TORCH_LIBRARY_EXPAND(TORCH_EXTENSION_NAME, ops) {\n  ops.def(\"rms_norm(Tensor x) -> ()\");\n}\n",
    )
    .write(
        "native/b.cpp",
        "STABLE_TORCH_LIBRARY_FRAGMENT(_C, ops) {\n  ops.def(\"rotary_embedding(Tensor q) -> ()\");\n}\n",
    )
    .write("app/ops.py", "import torch\ntorch.ops._C.rms_norm(x)\ntorch.ops._C.rotary_embedding(q)\n");
    assert_eq!(repo.edges(), ["App -> Native qua torch.ops._C.rotary_embedding"]);
}

#[test]
fn torch_op_khac_namespace_khong_ghep() {
    // `aten::add` của PyTorch không phải `mylib::add` của repo này.
    let repo = Repo::new("torch-op-ns");
    repo.write("native/ops.cpp", "TORCH_LIBRARY(mylib, m) {\n  m.def(\"add(Tensor a) -> Tensor\");\n}\n")
        .write("app/k.py", "import torch\nx = torch.ops.aten.add(a)\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn torch_op_chi_co_impl_thi_khong_phai_cung_cap() {
    // `TORCH_LIBRARY_IMPL` chỉ gắn kernel cho op đã định nghĩa ở chỗ khác.
    let repo = Repo::new("torch-op-impl");
    repo.write(
        "native/cuda.cpp",
        "TORCH_LIBRARY_IMPL(mylib, CUDA, m) {\n  m.impl(\"fused_add\", &fused_add_cuda);\n}\n",
    )
    .write("app/k.py", "import torch\ntorch.ops.mylib.fused_add(a, b)\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}

#[test]
fn torch_op_co_namespace_va_overload_trong_schema() {
    let repo = Repo::new("torch-op-schema");
    repo.write(
        "native/ops.cpp",
        "TORCH_LIBRARY_FRAGMENT(mylib, m) {\n  m.def(\"mylib::scaled.out(Tensor a, *, Tensor(a!) out) -> Tensor(a!)\");\n}\n",
    )
    .write("app/k.py", "import torch\ntorch.ops.mylib.scaled.out(a, out=b)\n");
    assert_eq!(repo.edges(), ["App -> Native qua torch.ops.mylib.scaled"]);
}

#[test]
fn import_tuong_doi_cua_goi_khac_khong_ghep_khi_stub_noi_module_o_cho_khac() {
    // `._core` của `pkgb` là `pkgb._core`. Stub `_core.pyi` nằm trong `pkga`
    // nói module native thuộc về `pkga`; `pkgb._core` là thứ khác — file sinh
    // lúc build, hoặc một import hỏng — và vẫn phải bị báo.
    let repo = Repo::new("pyo3-other-pkg");
    repo.write("native/src/lib.rs", "#[pymodule]\nfn _core(m: &Bound<'_, PyModule>) {}\n")
        .write("app/pkga/__init__.py", "from ._core import X\n")
        .write("app/pkga/_core.pyi", "class X: ...\n")
        .write("app/pkgb/api.py", "from ._core import Y\n");
    assert_eq!(repo.edges(), ["App -> Native qua ._core"]);
    assert_eq!(repo.unresolved(), ["app/pkgb/api.py:1 ._core"]);
}

#[test]
fn hai_goi_cung_import_module_native_khong_stub_thi_khong_ghep() {
    // Không có stub, hai package cùng nạp một `_core` của riêng mình: không
    // biết module native thuộc về package nào.
    let repo = Repo::new("pyo3-two-pkgs");
    repo.write("native/src/lib.rs", "#[pymodule]\nfn _core(m: &Bound<'_, PyModule>) {}\n")
        .write("app/pkga/x.py", "from ._core import A\n")
        .write("app/pkgb/y.py", "from ._core import B\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
    // Cùng một module, gọi từ hai file trong cùng package, thì vẫn ghép.
    repo.write("app/pkgb/y.py", "from app.pkga import _core\n");
    assert_eq!(repo.edges(), ["App -> Native qua ._core", "App -> Native qua app.pkga._core"]);
}

#[test]
fn hai_component_cung_cung_cap_module_hay_op_thi_bo_ca_hai() {
    let repo = Repo::new("binding-ambiguous");
    repo.write("native/src/lib.rs", "#[pymodule]\nfn _core(m: &Bound<'_, PyModule>) {}\n")
        .write("o/bind.cpp", "PYBIND11_MODULE(_core, m) {}\nTORCH_LIBRARY(mylib, m) {\n  m.def(\"f(Tensor a) -> Tensor\");\n}\n")
        .write("native/ops.cpp", "TORCH_LIBRARY_FRAGMENT(mylib, m) {\n  m.def(\"f(Tensor a) -> Tensor\");\n}\n")
        .write("app/__init__.py", "")
        .write("app/x.py", "import torch\nimport app._core\ntorch.ops.mylib.f(a)\n");
    assert!(repo.edges().is_empty(), "{:?}", repo.edges());
}
