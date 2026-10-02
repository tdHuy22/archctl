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
