//! Phân giải import: những cách bố trí thật mà bộ phân giải hay trượt.
//!
//! Trượt ở đây không ồn ào. Một import không ra được file thì cạnh phụ thuộc
//! biến mất, và `archctl check` nói "không có vi phạm" về đúng chỗ đang vi
//! phạm.

use arch_core::check::{Via, check};
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir()
            .join(format!("archctl-resolve-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn write(&self, rel: &str, body: &str) -> &Repo {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    /// `(cạnh bắt được, import chưa phân giải được)`
    fn judge(&self) -> (Vec<String>, Vec<String>) {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        let (doc, diags) = arch_core::load(&src);
        assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
        let r = check(&self.0, &doc, &[]);
        let mut edges: Vec<String> = r
            .violations
            .iter()
            .filter(|v| v.via == Via::Import)
            .map(|v| format!("{} -> {} ({})", v.from, v.to, v.line))
            .collect();
        edges.sort();
        (edges, r.unresolved.iter().map(|u| u.spec.clone()).collect())
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const PY_ARCH: &str = "\
component Web  at app/web/**  @human #accepted
component Core at app/core/** @human #accepted
component O    at o/**        @human #accepted
allow Web -> O
";

/// `app/web/views.py` gọi sang `app/core` bằng cả hai lối, tuyệt đối và tương
/// đối, cộng một `from . import` chỉ trỏ vào chính thư mục của nó.
fn python(name: &str) -> Repo {
    let repo = Repo::new(name);
    repo.write(".arch/system.arch", PY_ARCH)
        .write("app/core/models.py", "class Order: pass\n")
        .write("app/web/helpers.py", "def fmt(): pass\n")
        .write(
            "app/web/views.py",
            "from app.core.models import Order\n\
             from ..core import models\n\
             from . import helpers\n",
        );
    repo
}

#[test]
fn python_goi_kieu_khong_gian_ten_khong_can_init_py() {
    // PEP 420: gói không cần `__init__.py`, và đó là mặc định của Python hiện
    // đại. Trước đây `from ..core import models` không ra file nào nên cạnh
    // `Web -> Core` biến mất hẳn.
    let (edges, unresolved) = python("pep420").judge();
    assert_eq!(edges, ["Web -> Core (1)", "Web -> Core (2)"], "{edges:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

#[test]
fn python_goi_co_init_py_van_nhu_cu() {
    let repo = python("initpy");
    for dir in ["app", "app/web", "app/core"] {
        repo.write(&format!("{dir}/__init__.py"), "");
    }
    let (edges, unresolved) = repo.judge();
    assert_eq!(edges, ["Web -> Core (1)", "Web -> Core (2)"], "{edges:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

#[test]
fn python_from_cham_import_tro_vao_chinh_thu_muc_minh() {
    // `from . import helpers` không bao giờ là cạnh liên component, nên nó
    // không được là vi phạm, mà cũng không được là cảnh báo.
    let (edges, unresolved) = python("dot").judge();
    assert!(!edges.iter().any(|e| e.ends_with("(3)")), "{edges:?}");
    assert!(!unresolved.iter().any(|u| u == "."), "{unresolved:?}");
}

const TS_ARCH: &str = "\
component Web  at packages/web/**  @human #accepted
component Core at packages/core/** @human #accepted
component O    at o/**             @human #accepted
allow Web -> O
";

/// Monorepo npm: `packages/web` gọi sang `packages/core` bằng tên gói.
fn monorepo(name: &str) -> Repo {
    let repo = Repo::new(name);
    repo.write(".arch/system.arch", TS_ARCH)
        .write("package.json", r#"{"name":"root","workspaces":["packages/*"]}"#)
        .write("packages/core/package.json", r#"{"name":"@acme/core"}"#)
        .write("packages/core/src/index.ts", "export const thing = 1\n")
        .write("packages/core/src/types.ts", "export type T = number\n")
        .write("packages/web/package.json", r#"{"name":"@acme/web"}"#)
        .write("packages/web/src/util.ts", "export const u = 1\n")
        .write(
            "packages/web/src/page.ts",
            "import { thing } from '@acme/core'\n\
             export * from './util'\n\
             import type { T } from '@acme/core/src/types'\n",
        );
    repo
}

#[test]
fn goi_anh_em_trong_workspace_npm_la_phu_thuoc_that() {
    // Trước đây `@acme/core` bị coi là gói bên ngoài chưa khai báo `external`,
    // nên bị bỏ **trong im lặng**: không cạnh, không cảnh báo, không gì cả.
    // Monorepo là đúng chỗ luật kiến trúc cần nhất.
    let (edges, unresolved) = monorepo("npmws").judge();
    assert_eq!(edges, ["Web -> Core (1)", "Web -> Core (3)"], "{edges:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

#[test]
fn ten_goi_dai_hon_thang() {
    // `@acme/core` và `@acme/core-utils` cùng tồn tại thì phải chọn đúng cái.
    let repo = monorepo("longest");
    repo.write(".arch/system.arch", &format!("{TS_ARCH}component Utils at packages/core-utils/** @human #accepted\n"))
        .write("packages/core-utils/package.json", r#"{"name":"@acme/core-utils"}"#)
        .write("packages/core-utils/src/index.ts", "export const v = 1\n")
        .write("packages/web/src/page.ts", "import { v } from '@acme/core-utils'\n");
    assert_eq!(repo.judge().0, ["Web -> Utils (1)"]);
}

#[test]
fn goi_ngoai_van_la_goi_ngoai() {
    let repo = monorepo("external");
    repo.write("packages/web/src/page.ts", "import x from 'lodash'\n");
    assert_eq!(repo.judge(), (vec![], vec![]), "gói npm thật không được thành cạnh nội bộ");
}

#[test]
fn go_doc_duoc_duong_dan_module_co_so_phien_ban() {
    let repo = Repo::new("gomod");
    repo.write(".arch/system.arch", "\
component Web  at web/**  @human #accepted
component Core at core/** @human #accepted
component O    at o/**    @human #accepted
allow Web -> O
")
        .write("go.mod", "module example.com/proj/v2\n\ngo 1.22\n")
        .write("core/c.go", "package core\n")
        .write("core/sub/s.go", "package sub\n")
        .write(
            "web/h.go",
            "package web\n\n\
             import (\n\t\"fmt\"\n\t\
             _ \"example.com/proj/v2/core\"\n\t\
             alias \"example.com/proj/v2/core/sub\"\n\t\
             \"github.com/x/y\"\n)\n",
        );
    let (edges, unresolved) = repo.judge();
    // Import trống `_` và import có bí danh đều là phụ thuộc thật.
    assert_eq!(edges, ["Web -> Core (5)", "Web -> Core (6)"], "{edges:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

/// Hoa thường phải khớp đúng, trên mọi hệ thống file.
///
/// Ổ mặc định của macOS không phân biệt hoa thường, nên `Path::is_file()` trả
/// "có" cho `src/Error.rs` khi trên đĩa chỉ có `src/error.rs`. Linux thì
/// không. Cùng một commit `rusqlite`, `init --from-code` từng sinh 12 luật
/// `allow` trên macOS và 11 trên CI Linux, vì `use crate::{Error, Result}`
/// được "tìm thấy" là file `src/Error.rs`. Luật sinh ở máy này đem sang máy
/// kia thành một luật không bao giờ áp được, hoặc một vi phạm máy kia không
/// thấy. Mọi ngôn ngữ đã hỗ trợ đều phân biệt hoa thường khi import.
///
/// Trên ổ phân biệt hoa thường (Linux) các test này đạt cả khi chưa sửa; chúng
/// cắn trên macOS, nơi job `nền tảng khác` của CI chạy `cargo test`.
mod hoa_thuong {
    use arch_core::resolve::{Resolved, Resolver};
    use arch_core::scan::{Import, Lang};

    use super::Repo;

    fn resolve(repo: &Repo, lang: Lang, importer: &str, spec: &str) -> Resolved {
        let import = Import { spec: spec.to_string(), line: 1, system: false };
        Resolver::new(&repo.0).resolve(lang, importer, &import)
    }

    /// Ghi chú cho người đọc log: trên ổ phân biệt hoa thường, phần âm của
    /// test không chứng minh được gì về bản sửa.
    fn ghi_chu_he_thong_file(repo: &Repo) {
        repo.write("probe.txt", "");
        if !repo.0.join("PROBE.txt").is_file() {
            eprintln!("ổ này phân biệt hoa thường: test hoa_thuong chỉ chứng minh trên macOS");
        }
    }

    #[test]
    fn rust_ten_item_khong_phai_file_module_viet_khac_hoa() {
        let repo = Repo::new("case-rust");
        repo.write("Cargo.toml", "[package]\nname = \"x\"\n")
            .write("src/lib.rs", "mod error;\nmod blob;\npub use error::Error;\n")
            .write("src/error.rs", "pub struct Error;\n")
            .write("src/blob/mod.rs", "use crate::Error;\n");
        ghi_chu_he_thong_file(&repo);
        let got = resolve(&repo, Lang::Rust, "src/blob/mod.rs", "crate::Error");
        assert_ne!(got, Resolved::Path("src/Error.rs".into()));
        // Đúng hoa thường thì vẫn ra file như cũ.
        let got = resolve(&repo, Lang::Rust, "src/blob/mod.rs", "crate::error::Error");
        assert_eq!(got, Resolved::Path("src/error.rs".into()));
    }

    #[test]
    fn typescript_import_tuong_doi_sai_hoa() {
        let repo = Repo::new("case-ts");
        repo.write("src/util.ts", "export const x = 1\n")
            .write("src/app.ts", "import { x } from './Util'\n");
        ghi_chu_he_thong_file(&repo);
        assert_eq!(resolve(&repo, Lang::TypeScript, "src/app.ts", "./Util"), Resolved::Missing);
        assert_eq!(
            resolve(&repo, Lang::TypeScript, "src/app.ts", "./util"),
            Resolved::Path("src/util.ts".into())
        );
    }

    #[test]
    fn python_module_sai_hoa() {
        let repo = Repo::new("case-py");
        repo.write("pkg/__init__.py", "")
            .write("pkg/helpers.py", "x = 1\n")
            .write("pkg/app.py", "from pkg import Helpers\n");
        ghi_chu_he_thong_file(&repo);
        let got = resolve(&repo, Lang::Python, "pkg/app.py", "pkg.Helpers");
        assert_ne!(got, Resolved::Path("pkg/Helpers.py".into()));
        assert_eq!(
            resolve(&repo, Lang::Python, "pkg/app.py", "pkg.helpers"),
            Resolved::Path("pkg/helpers.py".into())
        );
    }

    #[test]
    fn python_thu_muc_khong_gian_ten_sai_hoa() {
        let repo = Repo::new("case-py-ns");
        repo.write("ns/mod.py", "x = 1\n").write("app.py", "import NS\n");
        ghi_chu_he_thong_file(&repo);
        assert_ne!(resolve(&repo, Lang::Python, "app.py", "NS"), Resolved::Path("NS".into()));
        assert_eq!(resolve(&repo, Lang::Python, "app.py", "ns"), Resolved::Path("ns".into()));
    }

    #[test]
    fn c_include_sai_hoa() {
        let repo = Repo::new("case-c");
        repo.write("src/config.h", "#define X 1\n")
            .write("src/main.c", "#include \"Config.h\"\nint main(void) { return 0; }\n");
        ghi_chu_he_thong_file(&repo);
        assert_ne!(
            resolve(&repo, Lang::C, "src/main.c", "Config.h"),
            Resolved::Path("src/Config.h".into())
        );
        assert_eq!(
            resolve(&repo, Lang::C, "src/main.c", "config.h"),
            Resolved::Path("src/config.h".into())
        );
    }

    #[test]
    fn go_package_sai_hoa() {
        let repo = Repo::new("case-go");
        repo.write("go.mod", "module example.com/m\n\ngo 1.22\n")
            .write("internal/x.go", "package internal\n")
            .write("main.go", "package main\n\nimport _ \"example.com/m/Internal\"\n");
        ghi_chu_he_thong_file(&repo);
        assert_eq!(resolve(&repo, Lang::Go, "main.go", "example.com/m/Internal"), Resolved::Missing);
        assert_eq!(
            resolve(&repo, Lang::Go, "main.go", "example.com/m/internal"),
            Resolved::Path("internal".into())
        );
    }
}

/// `crate::X` khi `X` không phải module file: một item ở gốc crate.
///
/// Bản cũ chỉ thử `src.rs` và `src/mod.rs` cho gốc crate — gốc crate không bao
/// giờ tên như vậy — rồi trả `Internal`: không cạnh, không cảnh báo. Ở năm repo
/// Rust của bộ đo có khoảng 470 câu `use` như vậy. Với `deny Blob -> Core` trên
/// `rusqlite`, `use crate::{Connection, Name, Result}` ở `src/blob/mod.rs` cho 0
/// vi phạm và 0 chưa phân giải: luật có mặt mà không được áp.
///
/// Đích đúng là nơi item được định nghĩa. `X` viết thẳng trong `lib.rs` thì là
/// `lib.rs`; `X` do `lib.rs` re-export (`pub use error::Error`) thì là
/// `error.rs` — không thì `lib.rs` thành chỗ rửa phụ thuộc: một `deny Scan ->
/// Model` lách được bằng `use crate::Document`.
mod goc_crate {
    use arch_core::resolve::{Resolved, Resolver};
    use arch_core::scan::{Import, Lang};

    use super::Repo;

    fn at(repo: &Repo, importer: &str, spec: &str) -> Resolved {
        let import = Import { spec: spec.to_string(), line: 1, system: false };
        Resolver::new(&repo.0).resolve(Lang::Rust, importer, &import)
    }

    fn path(p: &str) -> Resolved {
        Resolved::Path(p.to_string())
    }

    fn lib_crate(name: &str) -> Repo {
        let repo = Repo::new(name);
        repo.write("Cargo.toml", "[package]\nname = \"x\"\n")
            .write(
                "src/lib.rs",
                "mod blob;\nmod error;\nmod model;\n\
                 pub use error::Error;\n\
                 pub use self::model::{Document, Owner as Holder};\n\
                 pub use libsqlite3_sys as ffi;\n\
                 pub use crate::Loop;\n\
                 pub struct Connection;\n",
            )
            .write("src/error.rs", "pub struct Error;\n")
            .write("src/model.rs", "pub struct Document;\npub struct Owner;\n")
            .write("src/blob/mod.rs", "use crate::Connection;\n")
            .write("src/blob/pos_io.rs", "use super::super::Connection;\n");
        repo
    }

    #[test]
    fn item_viet_thang_trong_lib_rs() {
        let repo = lib_crate("root-item");
        assert_eq!(at(&repo, "src/blob/mod.rs", "crate::Connection"), path("src/lib.rs"));
    }

    #[test]
    fn re_export_tro_toi_noi_dinh_nghia() {
        let repo = lib_crate("root-reexport");
        assert_eq!(at(&repo, "src/blob/mod.rs", "crate::Error"), path("src/error.rs"));
        assert_eq!(at(&repo, "src/blob/mod.rs", "crate::Document"), path("src/model.rs"));
        // Tên mới sau `as`.
        assert_eq!(at(&repo, "src/blob/mod.rs", "crate::Holder"), path("src/model.rs"));
        // Biến thể của enum, hay item lồng trong item: đoạn đầu quyết định.
        assert_eq!(at(&repo, "src/blob/mod.rs", "crate::Error::Kind"), path("src/error.rs"));
    }

    #[test]
    fn re_export_crate_ngoai_van_la_goi_ngoai() {
        let repo = lib_crate("root-extern");
        assert_eq!(
            at(&repo, "src/blob/mod.rs", "crate::ffi::sqlite3_open"),
            Resolved::Package("libsqlite3_sys".into())
        );
    }

    #[test]
    fn re_export_vong_tron_khong_treo() {
        let repo = lib_crate("root-loop");
        assert_eq!(at(&repo, "src/blob/mod.rs", "crate::Loop"), path("src/lib.rs"));
    }

    #[test]
    fn super_va_self_toi_goc_crate() {
        let repo = lib_crate("root-super");
        assert_eq!(at(&repo, "src/blob/pos_io.rs", "super::super::Connection"), path("src/lib.rs"));
        assert_eq!(at(&repo, "src/error.rs", "super::Error"), path("src/error.rs"));
        // Trong chính `lib.rs`, `self::` là gốc crate: module con nằm ở `src/`,
        // không phải `src/lib/`. Bản cũ trả về chính `lib.rs`.
        assert_eq!(at(&repo, "src/lib.rs", "self::model::Document"), path("src/model.rs"));
    }

    #[test]
    fn crate_chi_co_main_rs() {
        let repo = Repo::new("root-bin-only");
        repo.write("Cargo.toml", "[package]\nname = \"tool\"\n")
            .write("src/main.rs", "mod cli;\npub struct Args;\nfn main() {}\n")
            .write("src/cli.rs", "use crate::Args;\n");
        assert_eq!(at(&repo, "src/cli.rs", "crate::Args"), path("src/main.rs"));
    }

    /// Mỗi binary là một crate riêng. `crate::` trong `src/bin/tool/` là gốc của
    /// binary đó, không phải thư viện cùng gói.
    #[test]
    fn binary_trong_src_bin_la_crate_rieng() {
        let repo = Repo::new("root-src-bin");
        repo.write("Cargo.toml", "[package]\nname = \"x\"\n")
            .write("src/lib.rs", "pub struct Config;\n")
            .write("src/bin/tool/main.rs", "mod helper;\npub struct Config;\nfn main() {}\n")
            .write("src/bin/tool/helper.rs", "use crate::Config;\n");
        assert_eq!(
            at(&repo, "src/bin/tool/helper.rs", "crate::Config"),
            path("src/bin/tool/main.rs")
        );
    }

    /// Cách ripgrep đặt binary chính: không có `src/`, đường dẫn khai trong
    /// `Cargo.toml`. Bản cũ tìm module trong `src/` không tồn tại, nên mọi
    /// `crate::` của binary đó mất hết.
    #[test]
    fn binary_khai_duong_dan_trong_cargo_toml() {
        let repo = Repo::new("root-bin-path");
        repo.write(
            "Cargo.toml",
            "[package]\nname = \"ripgrep\"\n\n[[bin]]\nname = \"rg\"\npath = \"crates/core/main.rs\"\n",
        )
        .write("crates/core/main.rs", "mod flags;\npub struct Thing;\nfn main() {}\n")
        .write("crates/core/flags/mod.rs", "use crate::Thing;\n");
        assert_eq!(at(&repo, "crates/core/main.rs", "crate::flags"), path("crates/core/flags/mod.rs"));
        assert_eq!(at(&repo, "crates/core/flags/mod.rs", "crate::Thing"), path("crates/core/main.rs"));
    }

    /// Test tích hợp là một crate: `crate::common` của nó là `tests/common/`,
    /// không phải `src/common.rs` của thư viện cùng gói.
    #[test]
    fn test_tich_hop_la_crate_rieng() {
        let repo = Repo::new("root-it");
        repo.write("Cargo.toml", "[package]\nname = \"x\"\n")
            .write("src/lib.rs", "pub mod common;\n")
            .write("src/common.rs", "pub fn decoy() {}\n")
            .write("tests/it.rs", "mod common;\nuse crate::common::helper;\n")
            .write("tests/other.rs", "mod common;\n")
            .write("tests/common/mod.rs", "pub fn helper() {}\nuse crate::Thing;\n");
        assert_eq!(at(&repo, "tests/it.rs", "crate::common::helper"), path("tests/common/mod.rs"));
        // `tests/common/` dùng chung cho nhiều crate test: không biết gốc nào,
        // nên không đoán.
        assert_eq!(at(&repo, "tests/common/mod.rs", "crate::Thing"), Resolved::Internal);
    }

    #[test]
    fn crate_thanh_vien_cua_workspace() {
        let repo = Repo::new("root-ws");
        repo.write("Cargo.toml", "[workspace]\nmembers = [\"crates/a\"]\n")
            .write("crates/a/Cargo.toml", "[package]\nname = \"a\"\n")
            .write("crates/a/src/lib.rs", "mod inner;\npub use inner::Thing;\n")
            .write("crates/a/src/inner.rs", "pub struct Thing;\n")
            .write("crates/a/src/x.rs", "use crate::Thing;\n");
        assert_eq!(at(&repo, "crates/a/src/x.rs", "crate::Thing"), path("crates/a/src/inner.rs"));
    }

    /// Kịch bản của `rusqlite`, chạy qua `check`: `deny Blob -> Core` phải bắt
    /// được `use crate::{Connection, Result}`.
    #[test]
    fn deny_bat_duoc_import_item_goc_crate() {
        let repo = lib_crate("root-deny");
        repo.write("other/x.rs", "")
            .write(
                ".arch/system.arch",
                "component Core  at src/*.rs   @human #accepted\n\
                 component Blob  at src/blob/** @human #accepted\n\
                 component Other at other/**    @human #accepted\n\
                 allow Blob -> Other\n\
                 deny  Blob -> Core\n",
            );
        let (edges, _) = repo.judge();
        assert!(edges.iter().any(|e| e.starts_with("Blob -> Core")), "nhận: {edges:#?}");
    }
}

const MULTI_ARCH: &str = "\
component Web  at web/**  @human #accepted
component Api  at api/**  @human #accepted
component Core at core/** @human #accepted
component O    at o/**    @human #accepted
allow Web -> O
";

#[test]
fn go_nhieu_module_trong_mot_repo() {
    // etcd: `api/go.mod` khai `go.etcd.io/etcd/api/v3`, gốc khai
    // `go.etcd.io/etcd/v3`. Chỉ đọc `go.mod` ở gốc thì mọi import sang module
    // anh em đều thành gói ngoài: repo hiện ra là không có quan hệ nào.
    let repo = Repo::new("gomulti");
    repo.write(".arch/system.arch", MULTI_ARCH)
        .write("go.mod", "module example.com/proj/v3\n\ngo 1.22\n")
        .write("core/c.go", "package core\n")
        .write("api/go.mod", "module example.com/proj/api/v3\n\ngo 1.22\n")
        .write("api/pb/p.go", "package pb\n")
        // Go bỏ qua `testdata`: một `go.mod` trong đó không phải module thật.
        .write("api/testdata/go.mod", "module example.com/proj/api/v3/pb\n")
        .write(
            "web/h.go",
            "package web\n\n\
             import (\n\t\
             \"example.com/proj/api/v3/pb\"\n\t\
             \"example.com/proj/v3/core\"\n\t\
             \"example.com/other/v3\"\n)\n",
        );
    let (edges, unresolved) = repo.judge();
    assert_eq!(edges, ["Web -> Api (4)", "Web -> Core (5)"], "{edges:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

#[test]
fn python_moi_package_trong_monorepo_la_mot_goc() {
    // langchain: `libs/core/langchain_core`, `libs/partners/x/langchain_x`.
    // Mỗi thư mục có `pyproject.toml` là một gốc import riêng.
    let repo = Repo::new("pymono");
    repo.write(
        ".arch/system.arch",
        "\
component Core at libs/core/** @human #accepted
component Web  at libs/web/**  @human #accepted
component O    at o/**         @human #accepted
allow Web -> O
allow Core -> O
",
    )
    .write("libs/core/pyproject.toml", "[project]\nname = \"acme-core\"\n")
    .write("libs/core/acme_core/msg.py", "class M: pass\n")
    .write("libs/core/tests/helpers.py", "")
    .write("libs/core/tests/test_msg.py", "from tests.helpers import h\n")
    .write("libs/web/pyproject.toml", "[project]\nname = \"acme-web\"\n")
    .write("libs/web/src/acme_web/view.py", "from acme_core.msg import M\n")
    // Cùng tên `tests` ở package kia: `tests` của chính package thắng.
    .write("libs/web/tests/helpers.py", "")
    .write("libs/web/tests/test_view.py", "from tests.helpers import h\n");
    let (edges, unresolved) = repo.judge();
    assert_eq!(edges, ["Web -> Core (1)"], "{edges:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

#[test]
fn python_goc_la_thu_muc_co_manage_py() {
    // netbox: mã nằm trong `netbox/`, chạy từ đó, nên `from dcim.models`
    // tính từ `netbox/` chứ không phải từ gốc repo.
    let repo = Repo::new("django");
    repo.write(
        ".arch/system.arch",
        "\
component Dcim at site/dcim/** @human #accepted
component Ipam at site/ipam/** @human #accepted
component O    at o/**         @human #accepted
allow Ipam -> O
",
    )
    .write("site/manage.py", "")
    .write("site/dcim/models.py", "class Device: pass\n")
    .write("site/ipam/views.py", "from dcim.models import Device\nimport django\n");
    let (edges, unresolved) = repo.judge();
    assert_eq!(edges, ["Ipam -> Dcim (1)"], "{edges:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

#[test]
fn python_ten_trung_o_hai_package_khac_thi_khong_doan() {
    // Không package nào chứa file gọi, mà hai package cùng có `shared`: đoán
    // một cái là có thể sinh cạnh sai. Coi như gói ngoài, như trước.
    let repo = Repo::new("pyamb");
    repo.write(
        ".arch/system.arch",
        "\
component A   at a/**   @human #accepted
component B   at b/**   @human #accepted
component App at app/** @human #accepted
component O   at o/**   @human #accepted
allow App -> O
",
    )
    .write("a/pyproject.toml", "")
    .write("a/shared/x.py", "")
    .write("b/pyproject.toml", "")
    .write("b/shared/x.py", "")
    .write("app/main.py", "from shared.x import y\n");
    assert_eq!(repo.judge(), (vec![], vec![]));
}

const C_ARCH: &str = "\
component Http at src/http/** @human #accepted
component Core at src/core/** @human #accepted
component O    at o/**        @human #accepted
allow Http -> O
";

#[test]
fn c_khong_co_file_build_thi_khop_ten_duy_nhat_trong_repo() {
    // nginx: `-I src/core` nằm trong script `auto/`, không trong CMake hay
    // Makefile. `#include <ngx_core.h>` từng bị coi là header hệ thống, và cả
    // repo không có cạnh nào.
    let repo = Repo::new("cnobuild");
    repo.write(".arch/system.arch", C_ARCH)
        .write("src/core/ngx_core.h", "")
        .write("src/core/event/ngx_event.h", "")
        .write("src/http/a/config.h", "")
        .write("src/core/config.h", "")
        .write(
            "src/http/h.c",
            "#include <stdio.h>\n\
             #include <ngx_core.h>\n\
             #include \"event/ngx_event.h\"\n\
             #include <config.h>\n",
        );
    let (edges, unresolved) = repo.judge();
    // `config.h` có ở hai nơi: không đoán.
    assert_eq!(edges, ["Http -> Core (2)", "Http -> Core (3)"], "{edges:?}");
    assert!(unresolved.is_empty(), "{unresolved:?}");
}

#[test]
fn c_co_file_build_thi_khong_doan() {
    // Build đã khai `-I` thì đó là sự thật; `<ngx_core.h>` ngoài các thư mục
    // đó là header của hệ thống, dù repo có một file trùng tên.
    let repo = Repo::new("cbuild");
    repo.write(".arch/system.arch", C_ARCH)
        .write("Makefile", "CFLAGS = -Isrc/http\n")
        .write("src/core/ngx_core.h", "")
        .write("src/http/h.c", "#include <ngx_core.h>\n");
    assert_eq!(repo.judge(), (vec![], vec![]));
}

#[test]
fn c_header_chuan_khong_khop_vao_shim_trong_repo() {
    // zstd có `wasm-shim/stdio.h` chỉ dùng khi build wasm. Không có file build
    // nào khai `-I`, nhưng `<stdio.h>` vẫn là của hệ thống.
    let repo = Repo::new("cshim");
    repo.write(".arch/system.arch", C_ARCH)
        .write("src/core/shim/stdio.h", "")
        .write("src/core/shim/sys/types.h", "")
        .write("src/http/h.c", "#include <stdio.h>\n#include <sys/types.h>\n");
    assert_eq!(repo.judge(), (vec![], vec![]));
}
