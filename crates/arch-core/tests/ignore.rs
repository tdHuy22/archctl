//! Thư mục nào không quét.
//!
//! `dist`, `build`, `out`, `coverage` thường là output, nhưng không phải lúc
//! nào cũng vậy: etcd có package Go `tests/robustness/coverage/`, zstd có mã C
//! trong `build/`. Bỏ chúng theo tên là bỏ mã thật trong im lặng — và một vi
//! phạm ở đó không bao giờ hiện ra.

use arch_core::check::source_files;
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir()
            .join(format!("archctl-ignore-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn write(&self, rel: &str, body: &str) -> &Repo {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    fn files(&self) -> Vec<String> {
        let mut files = source_files(&self.0, &[]);
        files.sort();
        files
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn thu_muc_ten_output_ma_git_khong_bo_qua_la_ma_that() {
    let repo = Repo::new("tracked");
    repo.write(".gitignore", "*.log\n")
        .write("tests/robustness/coverage/c.go", "package coverage\n")
        .write("zstd/build/single.c", "")
        .write("main.go", "package main\n");
    assert_eq!(repo.files(), ["main.go", "tests/robustness/coverage/c.go", "zstd/build/single.c"]);
}

#[test]
fn gitignore_bo_qua_thi_bo_qua() {
    let repo = Repo::new("ignored");
    repo.write(".gitignore", "# output\ncoverage/\n/build\n**/out\ndist\n")
        .write("coverage/lcov.js", "")
        .write("build/x.py", "")
        .write("a/b/out/x.js", "")
        .write("web/dist/x.js", "")
        .write("main.go", "package main\n");
    assert_eq!(repo.files(), ["main.go"]);
}

#[test]
fn mau_neo_o_goc_khong_bo_thu_muc_cung_ten_o_cho_khac() {
    // `/build` chỉ là `build` ở gốc.
    let repo = Repo::new("anchored");
    repo.write(".gitignore", "/build\n")
        .write("build/x.py", "")
        .write("tools/build/y.py", "");
    assert_eq!(repo.files(), ["tools/build/y.py"]);
}

#[test]
fn dau_cham_than_dua_thu_muc_tro_lai() {
    let repo = Repo::new("negate");
    repo.write(".gitignore", "build/\n!pkg/build/\n")
        .write("build/x.py", "")
        .write("pkg/build/y.py", "");
    assert_eq!(repo.files(), ["pkg/build/y.py"]);
}

#[test]
fn gitignore_long_trong_thu_muc_con() {
    let repo = Repo::new("nested");
    repo.write(".gitignore", "*.log\n")
        .write("web/.gitignore", "dist\n")
        .write("web/dist/x.js", "")
        .write("lib/dist/y.js", "");
    assert_eq!(repo.files(), ["lib/dist/y.js"]);
}

#[test]
fn khong_co_gitignore_thi_bo_theo_ten_nhu_cu() {
    // Không có gì nói thư mục này được commit hay không. Một `dist/` sinh ra
    // mà bị quét thì cả bundle thành mã kiến trúc, nên giữ cách thận trọng.
    let repo = Repo::new("nogit");
    repo.write("build/x.py", "").write("coverage/c.js", "").write("app.py", "");
    assert_eq!(repo.files(), ["app.py"]);
}

#[test]
fn thu_muc_luon_bo_qua_van_bo_qua() {
    // `node_modules`, `vendor`, `target` là mã người khác hoặc output của
    // trình biên dịch, dù `.gitignore` có nhắc tới hay không.
    let repo = Repo::new("always");
    repo.write(".gitignore", "*.log\n")
        .write("node_modules/x/i.js", "")
        .write("vendor/y/v.go", "")
        .write("target/debug/build/z.rs", "")
        .write("src/main.rs", "");
    assert_eq!(repo.files(), ["src/main.rs"]);
}

#[test]
fn gitignore_phia_tren_goc_archctl_van_ap_dung() {
    // Monorepo: `.arch/` ở `web/`, `dist/` bị bỏ ở gốc repo git.
    let repo = Repo::new("monorepo");
    repo.write(".git/HEAD", "ref: refs/heads/main\n")
        .write(".gitignore", "dist/\n")
        .write("web/.gitignore", ".env\n")
        .write("web/src/a.ts", "")
        .write("web/dist/bundle.js", "");
    let mut files = source_files(&repo.0.join("web"), &[]);
    files.sort();
    assert_eq!(files, ["src/a.ts"]);
}

#[test]
fn khong_co_git_thi_khong_doc_gitignore_phia_tren_goc() {
    // Không có `.git` nào: `.gitignore` phía trên không phải của repo này.
    let repo = Repo::new("nogitabove");
    repo.write(".gitignore", "dist/\n").write("web/.gitignore", ".env\n").write("web/dist/x.js", "");
    assert_eq!(source_files(&repo.0.join("web"), &[]), ["dist/x.js"]);
}

#[test]
fn bo_noi_dung_thu_muc_la_bo_thu_muc() {
    let repo = Repo::new("contents");
    repo.write(".gitignore", "dist/*\n!dist/.gitkeep\nbuild/**\n")
        .write("dist/b.js", "")
        .write("build/c.py", "")
        .write("app.py", "");
    assert_eq!(repo.files(), ["app.py"]);
}

#[test]
fn gitignore_co_bom_van_doc_dung_dong_dau() {
    let repo = Repo::new("bom");
    repo.write(".gitignore", "\u{feff}dist/\n").write("dist/b.js", "").write("app.py", "");
    assert_eq!(repo.files(), ["app.py"]);
}

#[test]
fn exclude_rieng_may_khong_bien_khong_biet_thanh_quet() {
    // Không có `.gitignore`: bỏ theo tên, dù `.git/info/exclude` có mẫu khác.
    let repo = Repo::new("exclude");
    repo.write(".git/info/exclude", "*.swp\n").write("dist/b.js", "").write("app.py", "");
    assert_eq!(repo.files(), ["app.py"]);
}

#[test]
fn goc_nam_trong_thu_muc_repo_ngoai_bo_qua_thi_khong_dung_gitignore_ngoai() {
    // Bộ đo chép netbox vào `archctl/target/bench/work/`. Git không bao giờ
    // nhìn vào `target/`, nên `dist/` trong `.gitignore` của archctl không phải
    // luật của netbox — netbox commit `dist/` của nó.
    let repo = Repo::new("nestedcopy");
    repo.write(".git/HEAD", "ref: refs/heads/main\n")
        .write(".gitignore", "dist/\ntarget/\n")
        .write("target/work/p/.gitignore", "*.log\n")
        .write("target/work/p/dist/x.js", "");
    assert_eq!(source_files(&repo.0.join("target/work/p"), &[]), ["dist/x.js"]);
}
