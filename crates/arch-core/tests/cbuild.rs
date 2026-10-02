//! Thư mục include của C/C++ đọc từ hệ thống build.
//!
//! Điều đáng kiểm nhất ở đây không phải là "đọc được `-I`" mà là **hậu quả**:
//! thiếu nó thì một vi phạm thật biến mất khỏi báo cáo, và `archctl` nói
//! "không có vi phạm" trong khi mã đang vi phạm.

use arch_core::check::{Via, check};
use std::path::PathBuf;

const ARCH: &str = "\
component App    at app/**           @human #accepted
component Crypto at vendor/crypto/** @human #accepted
component Spare  at spare/**         @human #accepted
allow App -> Spare
deny  App -> Crypto
";

struct Repo(PathBuf);

impl Repo {
    /// Mỗi test một thư mục riêng, vì cấu hình build là trạng thái toàn repo.
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir()
            .join(format!("archctl-cbuild-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = Repo(root);
        repo.write(".arch/system.arch", ARCH);
        repo.write("vendor/crypto/include/aes.h", "void aes_encrypt(char *p);\n");
        repo.write("app/main.c", "#include \"aes.h\"\nint main(void){ aes_encrypt(0); }\n");
        repo
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn mkdir(&self, rel: &str) {
        std::fs::create_dir_all(self.0.join(rel)).unwrap();
    }

    /// `(cạnh phụ thuộc bị bắt, include chưa phân giải được)`
    fn judge(&self) -> (Vec<String>, Vec<String>) {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        let (doc, diags) = arch_core::load(&src);
        assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
        let report = check(&self.0, &doc, &[]);
        (
            report
                .violations
                .iter()
                .filter(|v| v.via == Via::Import)
                .map(|v| format!("{} -> {}", v.from, v.to))
                .collect(),
            report.unresolved.iter().map(|u| u.spec.clone()).collect(),
        )
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn khong_co_file_build_thi_noi_thang_la_chua_phan_giai() {
    let repo = Repo::new("none");
    let (violations, unresolved) = repo.judge();
    assert!(violations.is_empty(), "không biết header ở đâu thì không được đoán ra cạnh");
    assert_eq!(unresolved, ["aes.h"], "nhưng phải nói ra là chưa biết");
}

#[test]
fn makefile_lam_lo_ra_vi_pham_bi_bo_sot() {
    let repo = Repo::new("make");
    repo.write("Makefile", "CFLAGS = -O2 -Ivendor/crypto/include -Wall\n");
    assert_eq!(repo.judge(), (vec!["App -> Crypto".to_string()], vec![]));
}

#[test]
fn makefile_mo_bien_truoc_khi_dung() {
    let repo = Repo::new("make-var");
    repo.write("Makefile", "INC := vendor/crypto\nCPPFLAGS += -I$(INC)/include\n");
    assert_eq!(repo.judge().0, ["App -> Crypto"]);
}

#[test]
fn cmake_voi_bien_thu_muc_nguon() {
    let repo = Repo::new("cmake");
    repo.write(
        "CMakeLists.txt",
        "add_library(app app/main.c)\n\
         target_include_directories(app PRIVATE\n  \"${CMAKE_SOURCE_DIR}/vendor/crypto/include\")\n",
    );
    assert_eq!(repo.judge().0, ["App -> Crypto"]);
}

#[test]
fn cmake_con_tinh_duong_dan_tuong_doi_tu_cho_no_dung() {
    let repo = Repo::new("cmake-sub");
    repo.mkdir("app/headers");
    repo.write("app/main.c", "#include \"local.h\"\nint main(void){}\n");
    repo.write("vendor/crypto/include/local.h", "/* bẫy: không được chọn cái này */\n");
    repo.write("app/headers/local.h", "/* đúng */\n");
    // CMakeLists ở `app/` nên `headers` là `app/headers`, không phải `headers`.
    repo.write("app/CMakeLists.txt", "include_directories(headers)\n");
    let (violations, unresolved) = repo.judge();
    assert!(violations.is_empty() && unresolved.is_empty(), "{violations:?} {unresolved:?}");
}

#[test]
fn compile_commands_duong_dan_tuyet_doi_cua_may_khac() {
    let repo = Repo::new("ccjson");
    // Sinh ra trong container, nên tiền tố tuyệt đối không khớp gốc hiện tại.
    repo.write(
        "build/compile_commands.json",
        r#"[{"directory": "/ci/ws/proj/build",
             "command": "cc -I/ci/ws/proj/vendor/crypto/include -c ../app/main.c",
             "file": "../app/main.c"}]"#,
    );
    assert_eq!(repo.judge().0, ["App -> Crypto"]);
}

#[test]
fn duong_dan_he_thong_khong_bi_keo_vao_trong_repo() {
    let repo = Repo::new("syspath");
    // `/opt/brew/share` có hậu tố một đoạn trùng với `share/` của repo. Khớp
    // theo hậu tố phải đòi ít nhất hai đoạn, không thì mọi repo có một thư mục
    // tên trùng đều nhận bừa thư mục hệ thống thành của mình.
    repo.mkdir("share");
    repo.write("share/aes.h", "/* bẫy */\n");
    repo.write(
        "compile_commands.json",
        r#"[{"directory": "/elsewhere/build", "arguments": ["cc", "-I/opt/brew/share", "-c", "a.c"]}]"#,
    );
    assert!(!arch_core::cbuild::include_dirs(&repo.0).contains(&"share".to_string()));
    let (violations, unresolved) = repo.judge();
    assert!(violations.is_empty(), "{violations:?}");
    assert_eq!(unresolved, ["aes.h"], "không phân giải được thì phải nói ra");
}

#[test]
fn header_he_thong_van_la_goi_ngoai() {
    let repo = Repo::new("external");
    repo.write("Makefile", "CFLAGS = -Ivendor/crypto/include\n");
    repo.write("app/main.c", "#include <stdio.h>\n#include \"aes.h\"\nint main(void){}\n");
    let src = format!("{ARCH}external LibC at \"stdio.h\" @human #accepted\ndeny App -> LibC\n");
    repo.write(".arch/system.arch", &src);
    let (violations, unresolved) = repo.judge();
    assert!(violations.contains(&"App -> LibC".to_string()), "{violations:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

#[test]
fn header_cua_du_an_tim_bang_dau_ngoac_nhon_van_la_file_du_an() {
    let repo = Repo::new("angle");
    repo.write("Makefile", "CFLAGS = -Ivendor/crypto/include\n");
    // `-I` nói thẳng thư mục này nằm trên đường tìm kiếm, nên `<aes.h>` ở đây
    // là file của dự án, đúng như trình biên dịch sẽ thấy.
    repo.write("app/main.c", "#include <aes.h>\nint main(void){}\n");
    assert_eq!(repo.judge().0, ["App -> Crypto"]);
}

#[test]
fn khong_chay_make_hay_cmake() {
    let repo = Repo::new("noexec");
    let flag = repo.0.join("DA_CHAY");
    // Nếu có lúc nào đó bộ đọc gọi `make`, test này sẽ thấy.
    repo.write("Makefile", &format!("CFLAGS = -Ivendor/crypto/include\nall:\n\ttouch {}\n", flag.display()));
    assert_eq!(repo.judge().0, ["App -> Crypto"]);
    assert!(!flag.exists(), "đọc file build thì được, thực thi nó thì không");
}

#[test]
fn khong_di_theo_symlink_ra_ngoai_repo() {
    let repo = Repo::new("symlink");
    let outside = std::env::temp_dir().join("archctl-cbuild-outside");
    let _ = std::fs::create_dir_all(&outside);
    std::fs::write(outside.join("Makefile"), "CFLAGS = -Ivendor/crypto/include\n").unwrap();
    repo.mkdir("sub");
    if std::os::unix::fs::symlink(&outside, repo.0.join("sub/link")).is_ok() {
        assert!(repo.judge().0.is_empty(), "cấu hình ngoài repo không phải luật của repo");
    }
    let _ = std::fs::remove_dir_all(&outside);
}

#[test]
fn thu_tu_khong_phu_thuoc_thu_tu_doc_thu_muc() {
    let repo = Repo::new("stable");
    repo.write("a/CMakeLists.txt", "include_directories(${CMAKE_SOURCE_DIR}/vendor/crypto/include)\n");
    repo.write("b/CMakeLists.txt", "include_directories(${CMAKE_SOURCE_DIR}/spare)\n");
    repo.mkdir("spare");
    let first = arch_core::cbuild::include_dirs(&repo.0);
    for _ in 0..5 {
        assert_eq!(arch_core::cbuild::include_dirs(&repo.0), first);
    }
    assert_eq!(first, ["vendor/crypto/include", "spare"]);
}
