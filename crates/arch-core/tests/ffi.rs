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
