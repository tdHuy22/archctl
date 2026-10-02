//! Giới hạn tài nguyên: một file trong repo không được làm bộ kiểm chứng chết.
//!
//! Bộ dò giữ cả dãy token trong bộ nhớ, nên một bundle đã minify 32 MB ngốn
//! hơn một gigabyte. Trong CI có hạn mức bộ nhớ thì đó là một lần bị giết, và
//! triệu chứng không chỉ ra được nguyên nhân.

use arch_core::check::{MAX_FILE_BYTES, check};
use std::path::PathBuf;

const ARCH: &str = "\
component Web at src/web/** @human #accepted
component Lib at src/lib/** @human #accepted
component O   at o/**       @human #accepted
allow Web -> O
";

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir()
            .join(format!("archctl-limits-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = Repo(root);
        repo.write(".arch/system.arch", ARCH);
        repo.write("src/lib/n.ts", "export const n = 1\n");
        repo
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn report(&self) -> arch_core::check::Report {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        check(&self.0, &arch_core::load(&src).0, &[])
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Một file chứa import thật, độn cho tới kích thước mong muốn.
fn padded(bytes: usize) -> String {
    let head = "import { n } from '../lib/n'\n";
    let filler = "var a=1;";
    format!("{head}{}", filler.repeat(bytes.saturating_sub(head.len()) / filler.len()))
}

#[test]
fn file_qua_lon_bi_bo_qua_nhung_duoc_bao_ra() {
    let repo = Repo::new("over");
    repo.write("src/web/bundle.min.js", &padded(MAX_FILE_BYTES as usize + (1 << 20)));
    let report = repo.report();
    assert_eq!(report.oversized.len(), 1, "{:?}", report.oversized);
    assert_eq!(report.oversized[0].0, "src/web/bundle.min.js");
    // Bỏ qua thì không bắt được vi phạm trong đó — đúng, và đó là lý do phải
    // báo ra thay vì im lặng.
    assert!(report.violations.is_empty());
}

#[test]
fn file_duoi_nguong_van_duoc_do_day_du() {
    let repo = Repo::new("under");
    repo.write("src/web/big.ts", &padded(MAX_FILE_BYTES as usize - (1 << 20)));
    let report = repo.report();
    assert!(report.oversized.is_empty(), "{:?}", report.oversized);
    assert_eq!(report.violations.len(), 1, "{:?}", report.violations);
}

#[test]
fn file_qua_lon_khong_lam_lech_mau_so_cua_do_pho_bien() {
    // File bị bỏ qua không được tính vào tổng số file đã dò, nếu không ngưỡng
    // "có mặt khắp nơi" sẽ bị pha loãng bởi thứ chưa hề được dò.
    let repo = Repo::new("denominator");
    repo.write("src/web/bundle.min.js", &padded(MAX_FILE_BYTES as usize + 1024));
    let report = repo.report();
    assert_eq!(report.files_scanned, 2, "vẫn đếm là đã gặp");
    assert_eq!(report.oversized.len(), 1);
}
