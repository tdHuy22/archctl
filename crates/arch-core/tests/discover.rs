//! `init --from-code` trên các bố cục repo thật.
//!
//! Bất biến mạnh nhất ở đây: luật được sinh ra **từ chính mã này**, nên nâng
//! mọi thứ lên `#accepted` rồi kiểm lại phải ra 0 vi phạm. Mỗi vi phạm còn lại
//! là một chỗ bộ khám phá và bộ kiểm chứng nhìn khác nhau — tức là một lỗi,
//! không phải một phát hiện.

use arch_core::check::check;
use arch_core::discover::discover;
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str, files: &[&str]) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir()
            .join(format!("archctl-disc-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for f in files {
            let (path, body) = f.split_once('|').unwrap_or((f, ""));
            let full = root.join(path);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, format!("{body}\n")).unwrap();
        }
        Repo(root)
    }

    fn names(&self) -> Vec<String> {
        let d = discover(&self.0, &[]);
        let mut v: Vec<String> = d.components.iter().map(|(n, _)| n.clone()).collect();
        v.sort();
        v
    }

    fn globs(&self) -> Vec<(String, String)> {
        discover(&self.0, &[]).components
    }

    /// Luật sinh ra, nâng hết lên `#accepted`, rồi kiểm lại chính mã đó.
    fn self_check(&self) -> Vec<String> {
        let d = discover(&self.0, &[]);
        let src = arch_core::discover::render(&d, "2026-01-01").replace("#draft", "#accepted");
        let (doc, diags) = arch_core::load(&src);
        assert!(!arch_core::diag::has_errors(&diags), "{src}\n{diags:#?}");
        check(&self.0, &doc, &[])
            .violations
            .iter()
            .map(|v| format!("{}->{} {}:{}", v.from, v.to, v.file, v.line))
            .collect()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Bố cục của hugo: `internal/` có đúng hai file, mã thật nằm ở các thư mục
/// cấp một khác. Chọn gốc theo tên quy ước thì `internal` thắng vô điều kiện
/// và phần còn lại của repo rơi ra ngoài sơ đồ.
#[test]
fn ten_quy_uoc_khong_duoc_thang_khi_no_chi_la_mot_goc_nho() {
    let mut files = vec!["internal/a/a.go|package a".to_string(), "internal/b/b.go|package b".to_string()];
    for dir in ["tpl", "resources", "hugolib", "common"] {
        for i in 0..6 {
            files.push(format!("{dir}/f{i}.go|package {dir}"));
        }
    }
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let repo = Repo::new("conventional", &refs);
    let names = repo.names();
    assert!(names.contains(&"Tpl".to_string()), "{names:?}");
    assert!(names.contains(&"Hugolib".to_string()), "{names:?}");
    assert!(names.len() >= 4, "lẽ ra phải chia theo thư mục cấp một: {names:?}");
}

/// Bố cục của flask: đúng một thư mục dưới `src/`, nên dừng ở đó thì bản nháp
/// chỉ có một component và không luật nào cả.
#[test]
fn mot_thu_muc_duy_nhat_thi_di_sau_them_mot_cap() {
    let repo = Repo::new("single", &[
        "src/flask/app.py|x = 1",
        "src/flask/helpers.py|y = 2",
        "src/flask/json/tag.py|z = 3",
        "src/flask/sansio/app.py|w = 4",
    ]);
    let names = repo.names();
    assert!(names.len() >= 2, "lẽ ra phải chia nhỏ hơn: {names:?}");
    assert!(names.contains(&"Json".to_string()), "{names:?}");
}

/// Bố cục của ripgrep và vue: gốc repo chia ra thì một nhóm nuốt gần hết, nên
/// phải đi vào trong nó.
#[test]
fn mot_nhom_nuot_gan_het_thi_di_sau_vao_no() {
    let mut files = vec!["build.rs|fn main() {}".to_string()];
    for c in ["core", "printer", "searcher", "ignore"] {
        for i in 0..5 {
            files.push(format!("crates/{c}/f{i}.rs|pub fn f{i}() {{}}"));
        }
    }
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let names = Repo::new("nested", &refs).names();
    assert!(names.contains(&"Printer".to_string()), "{names:?}");
    assert!(names.contains(&"Searcher".to_string()), "{names:?}");
}

#[test]
fn nhom_goc_khong_duoc_ten_vo_nghia() {
    // `pascal_case(".")` ra chuỗi rỗng rồi rơi vào nhánh dự phòng thành `M`.
    let repo = Repo::new("rootname", &[
        "main.go|package main",
        "cache/c.go|package cache",
        "helpers/h.go|package helpers",
    ]);
    let names = repo.names();
    assert!(!names.contains(&"M".to_string()), "{names:?}");
    assert!(names.contains(&"Core".to_string()), "{names:?}");
}

#[test]
fn mau_cua_nhom_goc_chi_khop_file_chu_khong_khop_thu_muc() {
    // `*` khớp một đoạn bất kỳ nên nó nhận luôn mọi thư mục con. Go phân giải
    // import thành cả thư mục, nên một package không thành component sẽ bị
    // mẫu này nhận bừa.
    let repo = Repo::new("rootglob", &[
        "main.go|package main",
        "cache/c.go|package cache",
        "helpers/h.go|package helpers",
    ]);
    let core = repo.globs().into_iter().find(|(n, _)| n == "Core").expect("có nhóm gốc");
    assert_eq!(core.1, "*.go", "{core:?}");
    assert!(!arch_core::glob::matches(&core.1, "helpers"), "không được nhận cả thư mục");
}

/// Go phân giải import thành cả package, nên đích có thể là một thư mục.
#[test]
fn canh_toi_package_cap_mot_cua_go_khong_bi_gom_vao_nhom_goc() {
    let repo = Repo::new("gopkg", &[
        "go.mod|module example.com/p",
        "main.go|package main",
        "cache/c.go|package cache\n\nimport \"example.com/p/helpers\"",
        "helpers/h.go|package helpers",
    ]);
    let d = discover(&repo.0, &[]);
    let edges: Vec<String> = d.edges.iter().map(|(a, b)| format!("{a}->{b}")).collect();
    assert!(edges.contains(&"Cache->Helpers".to_string()), "{edges:?}");
}

#[test]
fn luat_sinh_ra_tu_ma_nay_thi_kiem_lai_phai_sach() {
    for (name, files) in [
        ("go", vec![
            "go.mod|module example.com/p",
            "main.go|package main",
            "cache/c.go|package cache\n\nimport \"example.com/p/helpers\"",
            "helpers/h.go|package helpers",
            "htesting/t.go|package htesting",
        ]),
        ("ts", vec![
            "packages/core/index.ts|export const a = 1",
            "packages/web/page.ts|import { a } from '../core/index'",
            "scripts/build.ts|export const b = 2",
        ]),
        ("py", vec![
            "src/app/main.py|from app.util import helper",
            "src/app/util.py|def helper(): pass",
            "src/app/sub/x.py|v = 1",
        ]),
    ] {
        let repo = Repo::new(&format!("selfcheck-{name}"), &files);
        assert!(repo.self_check().is_empty(), "{name}: {:?}", repo.self_check());
    }
}
