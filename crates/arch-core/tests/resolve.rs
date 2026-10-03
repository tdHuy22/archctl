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
