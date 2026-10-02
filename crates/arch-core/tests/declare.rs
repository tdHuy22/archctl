//! Kiểm các khai báo hành vi: bước của flow trỏ vào đâu, và khi nào một lần
//! "pass" thật ra không chứng minh được gì.
//!
//! Hai chế độ hỏng ở đây đối xứng nhau, và cái thứ hai tệ hơn: báo nhầm thì
//! người dùng mất lòng tin vào công cụ, còn im lặng sai thì họ tin vào một sơ
//! đồ đã lạc hậu.

use arch_core::check::check;
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str, arch: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir()
            .join(format!("archctl-declare-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = Repo(root);
        repo.write(".arch/system.arch", arch);
        repo
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    /// `(tên bị báo lạc hậu, tên bị báo kiểm không kết luận)`
    fn judge(&self) -> (Vec<String>, Vec<String>) {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        let (doc, diags) = arch_core::load(&src);
        assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
        let report = check(&self.0, &doc, &[]);
        (
            report.missing.iter().map(|m| m.name.clone()).collect(),
            report.vacuous.iter().map(|v| v.name.clone()).collect(),
        )
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

const ARCH: &str = "\
component Web   at src/web/**   @human #accepted
component Order at src/order/** @human #accepted
allow Web -> Order
";

#[test]
fn ten_component_lam_nguoi_thuc_hien_khong_phai_la_ky_hieu_trong_ma() {
    // `Web -> charge` nghĩa là Web gọi charge. Không có hàm nào tên `Web`, nên
    // đòi phải có là báo nhầm — và báo nhầm ở ngay dòng đầu của flow.
    let repo = Repo::new("actor", &format!("{ARCH}\nflow F:\n  Web -> charge\n"));
    repo.write("src/order/pay.ts", "export function charge() {}\n");
    repo.write("src/web/page.ts", "import { charge } from '../order/pay'\n");
    assert_eq!(repo.judge(), (vec![], vec![]));
}

#[test]
fn buoc_dang_component_cham_nang_luc_duoc_tach_ra() {
    // Dò cả chuỗi `Order.charge` thì không bao giờ khớp: bộ quét làm việc trên
    // từng định danh một.
    let repo = Repo::new("dotted", &format!("{ARCH}\nflow F:\n  Web -> Order.charge\n"));
    repo.write("src/order/pay.ts", "export function charge() {}\n");
    repo.write("src/web/page.ts", "import { charge } from '../order/pay'\n");
    assert_eq!(repo.judge(), (vec![], vec![]));
}

#[test]
fn buoc_dang_component_cham_nang_luc_doi_dung_component() {
    // Tách ra còn kiểm được nhiều hơn: năng lực phải nằm trong đúng component.
    let repo = Repo::new("dotted-wrong", &format!("{ARCH}\nflow F:\n  Web -> Web.charge\n"));
    repo.write("src/order/pay.ts", "export function charge() {}\n");
    let (missing, _) = repo.judge();
    assert_eq!(missing, ["charge"]);
}

#[test]
fn buoc_dang_bien_cham_phuong_thuc_khong_rang_buoc_noi_chua() {
    // `order.charge` là một biến, không phải component nào cả.
    let repo = Repo::new("varcall", &format!("{ARCH}\nflow F:\n  Web -> order.charge\n"));
    repo.write("src/order/pay.ts", "export function charge() {}\n");
    assert_eq!(repo.judge(), (vec![], vec![]));
}

#[test]
fn ten_co_mat_khap_noi_thi_noi_thang_la_khong_ket_luan_duoc() {
    let repo = Repo::new("vacuous", &format!("{ARCH}\nflow F:\n  Web -> Order.run\n"));
    for i in 0..6 {
        repo.write(&format!("src/order/f{i}.ts"), "export function run() {}\n");
        repo.write(&format!("src/web/f{i}.ts"), "export function run() {}\n");
    }
    let (missing, vacuous) = repo.judge();
    assert!(missing.is_empty(), "tên có thật nên không phải lạc hậu: {missing:?}");
    assert_eq!(vacuous, ["run"], "nhưng tìm thấy nó không chứng minh được gì");
}

#[test]
fn ten_cu_the_thi_khong_bi_coi_la_khong_ket_luan() {
    let repo = Repo::new("specific", &format!("{ARCH}\nflow F:\n  Web -> Order.reserveStock\n"));
    for i in 0..6 {
        repo.write(&format!("src/order/f{i}.ts"), "export function run() {}\n");
        repo.write(&format!("src/web/f{i}.ts"), "export function run() {}\n");
    }
    repo.write("src/order/stock.ts", "export function reserveStock() {}\n");
    assert_eq!(repo.judge(), (vec![], vec![]));
}

#[test]
fn repo_nho_thi_khong_ket_luan_gi_ve_do_pho_bien() {
    // Trong hai file thì tên nào cũng "có mặt ở một nửa repo". Nói rằng kiểm
    // không kết luận được ở đây mới là cảnh báo rác.
    let repo = Repo::new("small", &format!("{ARCH}\nflow F:\n  Web -> Order.run\n"));
    repo.write("src/order/a.ts", "export function run() {}\n");
    repo.write("src/web/b.ts", "export function run() {}\n");
    assert_eq!(repo.judge(), (vec![], vec![]));
}

/// Chỗ **khai báo** là bằng chứng mạnh hơn hẳn một lần gặp bất kỳ. `charge`
/// được định nghĩa trong Payment nói rằng Payment có năng lực đó; `charge`
/// xuất hiện trong Web có thể chỉ là một lời gọi sang Payment.
mod khai_bao {
    use super::*;

    const ARCH: &str = "\
component Web     at src/web/**     @human #accepted
component Payment at src/payment/** @human #accepted
allow Web -> Payment
";

    #[test]
    fn nang_luc_phai_duoc_dinh_nghia_dung_cho_chu_khong_chi_duoc_goi() {
        let repo = Repo::new(
            "declared",
            &format!("{ARCH}\ncontract Web.charge:  @human #accepted\n  verify x\n"),
        );
        repo.write("src/payment/pay.ts", "export function charge() {}\n");
        // Web import và gọi `charge`, nên nó *xuất hiện* ở đây — nhưng năng
        // lực thì nằm ở Payment.
        repo.write(
            "src/web/page.ts",
            "import { charge } from '../payment/pay'\nexport function checkout() { return charge() }\n",
        );
        let (missing, _) = repo.judge();
        assert_eq!(missing, ["charge"], "chỉ gọi thì không phải là có năng lực đó");
    }

    #[test]
    fn dinh_nghia_dung_cho_thi_khong_bi_bao() {
        let repo = Repo::new(
            "declared-ok",
            &format!("{ARCH}\ncontract Payment.charge:  @human #accepted\n  verify x\n"),
        );
        repo.write("src/payment/pay.ts", "export function charge() {}\n");
        repo.write("src/web/page.ts", "import { charge } from '../payment/pay'\n");
        assert_eq!(repo.judge(), (vec![], vec![]));
    }

    #[test]
    fn khong_nhan_ra_dang_khai_bao_thi_quay_ve_cach_dem_cu() {
        // `Declined` là một khoá trong object literal: nó là định danh thật,
        // nhưng không có từ khoá khai báo nào đứng trước. Đòi một bằng chứng
        // mình không biết cách tìm là tự sinh báo động giả, nên phải nới ra.
        let repo = Repo::new(
            "no-decl-form",
            &format!("{ARCH}\ncontract Web.checkout:  @human #accepted\n  errors Declined\n  verify x\n"),
        );
        repo.write("src/web/page.ts", "export function checkout() {}\n");
        repo.write("src/web/errors.ts", "export const codes = { Declined: 402 }\n");
        assert_eq!(repo.judge(), (vec![], vec![]));
    }

    #[test]
    fn ten_khai_bao_khap_noi_van_la_khong_ket_luan_duoc() {
        let repo = Repo::new("decl-vacuous", &format!("{ARCH}\nflow F:\n  Web -> Payment.run\n"));
        for i in 0..6 {
            repo.write(&format!("src/payment/f{i}.ts"), "export function run() {}\n");
            repo.write(&format!("src/web/f{i}.ts"), "export function run() {}\n");
        }
        let (_, vacuous) = repo.judge();
        assert_eq!(vacuous, ["run"]);
    }

    #[test]
    fn goi_khap_noi_nhung_dinh_nghia_mot_cho_thi_ket_luan_duoc() {
        // Đây là chỗ bằng chứng khai báo thắng hẳn: `run` bị gọi trong mọi
        // file, nhưng chỉ được đặt ra ở một chỗ.
        let repo = Repo::new("decl-sharp", &format!("{ARCH}\nflow F:\n  Web -> Payment.run\n"));
        repo.write("src/payment/core.ts", "export function run() {}\n");
        for i in 0..6 {
            repo.write(&format!("src/web/f{i}.ts"), "import { run } from '../payment/core'\nexport const a{i} = run()\n".replace("{i}", &i.to_string()).as_str());
        }
        let (missing, vacuous) = repo.judge();
        assert!(missing.is_empty(), "{missing:?}");
        assert!(vacuous.is_empty(), "chỉ một chỗ khai báo thì không thể vô nghĩa: {vacuous:?}");
    }
}
