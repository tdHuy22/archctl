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

/// Bố cục của zstd-rs và rusqlite: một bản sao thư viện C nhúng sẵn, đông file
/// hơn hẳn mã Rust thật. Chọn gốc theo số file thì gốc rơi vào trong thư viện
/// đó và toàn bộ mã của dự án nằm ngoài sơ đồ.
#[test]
fn thu_vien_c_nhung_san_khong_duoc_keo_goc_vao_trong_no() {
    let mut files = vec!["Cargo.toml|[package]\nname = \"w\"".to_string()];
    for d in ["a", "b", "c", "d"] {
        for i in 0..4 {
            files.push(format!("vendored/clib/{d}/f{i}.c|int f{i}(void) {{ return 0; }}"));
        }
    }
    for i in 0..5 {
        files.push(format!("src/f{i}.rs|pub fn f{i}() {{}}"));
        files.push(format!("wrapper/src/g{i}.rs|pub fn g{i}() {{}}"));
    }
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let names = Repo::new("vendored", &refs).names();
    assert!(names.contains(&"Src".to_string()), "{names:?}");
    assert!(names.contains(&"Wrapper".to_string()), "{names:?}");
    assert!(!names.contains(&"A".to_string()), "gốc không được rơi vào thư viện nhúng: {names:?}");
}

#[test]
fn khong_co_manifest_thi_khong_loc_theo_ngon_ngu() {
    // redis và curl là C thuần, không manifest nào. Lọc theo ngôn ngữ ở đó
    // sẽ không có gì để dựa vào.
    let repo = Repo::new("nomanifest", &[
        "src/a/x.c|int x(void) { return 0; }",
        "src/b/y.c|int y(void) { return 0; }",
        "src/c/z.c|int z(void) { return 0; }",
    ]);
    assert!(repo.names().len() >= 3, "{:?}", repo.names());
}

/// Bố cục của tokio: mười crate ở gốc, mà `tokio/src/{runtime,io,fs,…}` nhìn
/// rất giống kiến trúc. Chọn chỗ đó thì `tokio-util` và `tokio-stream` — hai
/// crate riêng — rơi hết ra ngoài sơ đồ.
#[test]
fn khong_chia_nho_hon_ranh_gioi_package_da_khai_bao() {
    let mut files = vec!["Cargo.toml|[workspace]".to_string()];
    files.push("tokio/Cargo.toml|[package]\nname = \"tokio\"".to_string());
    for m in ["runtime", "io", "fs", "sync", "net"] {
        for i in 0..4 {
            files.push(format!("tokio/src/{m}/f{i}.rs|pub fn f{i}() {{}}"));
        }
    }
    for c in ["tokio-util", "tokio-stream"] {
        files.push(format!("{c}/Cargo.toml|[package]\nname = \"{c}\""));
        for i in 0..3 {
            files.push(format!("{c}/src/f{i}.rs|pub fn f{i}() {{}}"));
        }
    }
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let repo = Repo::new("pkgbound", &refs);
    let names = repo.names();
    assert!(names.contains(&"Tokio".to_string()), "{names:?}");
    assert!(names.contains(&"TokioUtil".to_string()), "crate riêng không được rơi ra ngoài: {names:?}");
    assert!(!names.contains(&"Runtime".to_string()), "không được chia nhỏ hơn crate: {names:?}");
}

#[test]
fn package_o_goc_repo_khong_chan_viec_di_sau() {
    // `rusqlite` có package ở gốc; nếu tính nó thì mọi tiền tố đều "nằm trong
    // một package" và bản nháp không bao giờ chia nhỏ được.
    let mut files = vec!["Cargo.toml|[package]\nname = \"r\"".to_string()];
    for d in ["blob", "types", "vtab"] {
        for i in 0..4 {
            files.push(format!("src/{d}/f{i}.rs|pub fn f{i}() {{}}"));
        }
    }
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let names = Repo::new("rootpkg", &refs).names();
    assert!(names.contains(&"Blob".to_string()), "{names:?}");
    assert!(names.contains(&"Vtab".to_string()), "{names:?}");
}

#[test]
fn bao_cao_phan_repo_chua_duoc_phu() {
    // `src/` chia ra được nhiều nhất nên nó thắng, và `scripts/` nằm ngoài.
    let mut files = Vec::new();
    for d in ["a", "b", "c"] {
        for i in 0..4 {
            files.push(format!("src/{d}/f{i}.rs|pub fn f{i}() {{}}"));
        }
    }
    files.push("scripts/s1.rs|pub fn s1() {}".to_string());
    files.push("scripts/s2.rs|pub fn s2() {}".to_string());
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let repo = Repo::new("coverage", &refs);
    let d = discover(&repo.0, &[]);
    assert_eq!(d.components.len(), 3, "{:?}", d.components);
    assert_eq!(d.unmapped, 2, "hai file trong scripts/ nằm ngoài gốc đã chọn");
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

/// libbpfgo: Go ở gốc gọi vào libbpf qua cgo, không có `#include` nào trỏ tới
/// file `.c` cung cấp hàm. Bộ khám phá chỉ đọc import thì không thấy cạnh mà
/// bộ kiểm thấy, và luật sinh ra tự vi phạm chính nó.
#[test]
fn canh_cgo_giua_hai_component_co_trong_luat_sinh_ra() {
    let mut files = vec![
        "go.mod|module example.com/bpfgo".to_string(),
        "libbpf/src/bpf.h|int bpf_map_fd(int x);".to_string(),
        "libbpf/src/bpf.c|int bpf_map_fd(int x) { return x; }".to_string(),
        // Cạnh `#include` đưa Core vào diện kiểm; lời gọi cgo sang Native thì
        // không có import nào nói tới.
        "shim.c|#include \"libbpf/src/bpf.h\"".to_string(),
        "native/n.c|int native_tick(int x) { return x; }".to_string(),
        "native/m.c|int native_tock(int x) { return x; }".to_string(),
    ];
    for i in 0..10 {
        files.push(format!(
            "f{i}.go|package bpfgo\n\nimport \"C\"\n\nfunc F{i}() int {{ return int(C.bpf_map_fd(1)) + int(C.native_tick(2)) }}"
        ));
    }
    for t in ["a", "b", "c"] {
        files.push(format!("selftest/{t}/main.go|package main"));
    }
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let repo = Repo::new("cgo", &refs);
    let mut names = repo.names();
    names.retain(|n| n != "Selftest");
    assert_eq!(names, ["Core", "Libbpf", "Native"]);
    assert!(repo.self_check().is_empty(), "{:?}", repo.self_check());
}

/// `selftest/` của libbpfgo có 24 thư mục con, mỗi cái một `main.go`. Không
/// coi nó là test thì nó thắng gốc repo, và thư viện thật nằm ngoài sơ đồ.
#[test]
fn selftest_khong_duoc_chon_lam_goc() {
    let mut files = vec!["go.mod|module example.com/bpfgo".to_string()];
    for i in 0..6 {
        files.push(format!("f{i}.go|package bpfgo"));
        files.push(format!("pkg{i}/p.go|package pkg{i}"));
    }
    for i in 0..20 {
        files.push(format!("selftest/t{i}/main.go|package main"));
    }
    let refs: Vec<&str> = files.iter().map(String::as_str).collect();
    let repo = Repo::new("selftest", &refs);
    assert!(repo.names().contains(&"Core".to_string()), "{:?}", repo.globs());
}
