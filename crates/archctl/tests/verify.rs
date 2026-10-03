//! `archctl verify` — biến `verify` từ một cái tên thành bằng chứng.
//!
//! Trước đây hệ thống cảnh báo "thiếu `verify` thì đây chỉ là lời hứa", ngụ ý
//! rằng **có** `verify` thì không phải. Nhưng chưa có gì chạy, nên có cũng
//! vẫn là lời hứa — chỉ khác là không còn ai nhắc.

use std::path::PathBuf;
use std::process::{Command, Output};

const ARCH: &str = "\
component Order at src/order/** @human #accepted
component Pay   at src/pay/**   @human #accepted
allow Order -> Pay

verify money_props runs \"exit 0\"   @human #accepted

invariant money [system] tổng tiền khớp verify money_props   @human #accepted
";

struct Project(PathBuf);

impl Project {
    fn new(name: &str, arch: &str) -> Project {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir()
            .join(format!("archctl-verify-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = Project(root);
        p.write(".arch/system.arch", arch);
        p.write("src/pay/p.ts", "export const charge = 1\n");
        p.write("src/order/o.ts", "import { charge } from '../pay/p'\n");
        p
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_archctl"))
            .args(args)
            .arg("--root")
            .arg(&self.0)
            .output()
            .expect("chạy được archctl")
    }

    fn out(&self, args: &[&str]) -> String {
        let o = self.run(args);
        format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
    }

    /// Đợi qua ranh giới một giây, vì biên nhận đo bằng giây.
    fn tick(&self) {
        std::thread::sleep(std::time::Duration::from_millis(1100));
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn chua_chay_thi_check_noi_ra() {
    let p = Project::new("never", ARCH);
    let out = p.out(&["check"]);
    assert!(out.contains("CHƯA CHỨNG MINH"), "{out}");
    assert!(out.contains("chưa chạy lần nào"), "{out}");
    assert!(out.contains("archctl verify money_props"), "phải nói cách sửa: {out}");
}

#[test]
fn chay_xong_thi_het_canh_bao() {
    let p = Project::new("ran", ARCH);
    assert!(p.run(&["verify", "--quiet"]).status.success());
    assert!(!p.out(&["check"]).contains("CHƯA CHỨNG MINH"), "{}", p.out(&["check"]));
    assert!(p.0.join(".arch/.verified").exists());
}

#[test]
fn lenh_hong_thi_bien_nhan_ghi_la_hong() {
    let p = Project::new("failed", &ARCH.replace("runs \"exit 0\"", "runs \"exit 3\""));
    assert!(!p.run(&["verify", "--quiet"]).status.success(), "verify phải báo hỏng");
    let out = p.out(&["check"]);
    assert!(out.contains("lần chạy gần nhất không đạt"), "{out}");
}

#[test]
fn sua_ma_thi_bien_nhan_het_hieu_luc() {
    // Một lần chạy đạt từ trước khi sửa mã không chứng minh được gì cho mã
    // hiện tại.
    let p = Project::new("codechange", ARCH);
    p.run(&["verify", "--quiet"]);
    p.tick();
    p.write("src/order/o.ts", "import { charge } from '../pay/p'\n// đổi\n");
    let out = p.out(&["check"]);
    assert!(out.contains("mã đã đổi sau lần chạy gần nhất"), "{out}");
}

#[test]
fn sua_luat_thi_bien_nhan_het_hieu_luc() {
    let p = Project::new("archchange", ARCH);
    p.run(&["verify", "--quiet"]);
    p.write(".arch/system.arch", &format!("{ARCH}deny Pay -> Order\n"));
    let out = p.out(&["check"]);
    assert!(out.contains("luật đã đổi sau lần chạy gần nhất"), "{out}");
}

#[test]
fn them_chu_thich_khong_lam_het_hieu_luc() {
    // `arch_hash` băm mô hình chứ không băm văn bản. Biên nhận hết hạn vì một
    // dòng chú thích là kêu sói, và người dùng sẽ học cách bỏ qua.
    let p = Project::new("comment", ARCH);
    p.run(&["verify", "--quiet"]);
    p.tick();
    p.write(".arch/system.arch", &format!("{ARCH}# ghi chú thêm\n"));
    assert!(!p.out(&["check"]).contains("CHƯA CHỨNG MINH"), "{}", p.out(&["check"]));
}

#[test]
fn chay_duoc_tung_cach_kiem_mot() {
    let arch = format!("{ARCH}verify khac runs \"exit 0\"  @human #accepted\n\
                        invariant time [system] nhanh verify khac  @human #accepted\n");
    let p = Project::new("named", &arch);
    p.run(&["verify", "money_props", "--quiet"]);
    let out = p.out(&["check"]);
    assert!(!out.contains("`money_props`"), "{out}");
    assert!(out.contains("`khac`"), "cái chưa chạy vẫn phải bị nhắc: {out}");
}

#[test]
fn verify_tro_toi_ten_chua_khai_bao_thi_bao_ra() {
    let arch = ARCH.replace("verify money_props runs \"exit 0\"   @human #accepted\n", "");
    let p = Project::new("dangling", &arch);
    let out = p.out(&["check"]);
    assert!(out.contains("chưa khai báo"), "{out}");
}

/// `accept` ghi thẳng đề xuất vào file luật, và `archctl verify` chạy lệnh
/// trong đó. Nếu agent đề xuất được một cách kiểm thì một lần duyệt vội là đủ
/// để nó tự chọn thứ chạy trên máy người dùng.
#[test]
fn agent_khong_de_xuat_duoc_mot_lenh_shell() {
    let p = Project::new("noshell", ARCH);
    let o = p.run(&["propose", "verify evil runs \"touch /tmp/pwned\"", "--reason", "thử"]);
    assert!(!o.status.success());
    let out = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("lệnh shell"), "{out}");
    assert!(!p.0.join(".arch/proposals.arch").exists() ||
            !std::fs::read_to_string(p.0.join(".arch/proposals.arch")).unwrap().contains("evil"));
}

/// `check` chạy trong hook của agent sau mỗi lần sửa file. Nếu nó chạy lệnh
/// shell thì mọi ranh giới ở trên đều vô nghĩa.
#[test]
fn check_khong_bao_gio_chay_lenh() {
    let flag = std::env::temp_dir().join(format!("archctl-flag-{}", std::process::id()));
    let _ = std::fs::remove_file(&flag);
    let arch = ARCH.replace(
        "runs \"exit 0\"",
        &format!("runs \"touch {}\"", flag.display()),
    );
    let p = Project::new("nosideeffect", &arch);
    p.run(&["check"]);
    p.run(&["check", "--strict"]);
    assert!(!flag.exists(), "`check` đã chạy lệnh của verify");
    let _ = std::fs::remove_file(&flag);
}

#[test]
fn hai_cach_kiem_trung_ten_bi_bao() {
    // Cái thứ hai bị bỏ im lặng thì người viết tưởng lệnh của mình đang chạy.
    let p = Project::new("dup", &format!("{ARCH}verify money_props runs \"exit 9\"  @human #accepted\n"));
    let out = p.out(&["check"]);
    assert!(out.contains("trùng tên"), "{out}");
    assert!(!p.run(&["check"]).status.success(), "trùng tên là lỗi, không phải cảnh báo");
}

#[test]
fn lenh_chua_chu_runs_van_doc_dung() {
    let arch = ARCH.replace("runs \"exit 0\"", "runs \"sh -c 'echo runs ok'\"");
    let p = Project::new("runsword", &arch);
    assert!(p.run(&["verify", "--quiet"]).status.success(), "{}", p.out(&["verify"]));
}

#[test]
fn bien_nhan_bi_sua_tay_thanh_rac_khong_lam_sap() {
    let p = Project::new("garbage", ARCH);
    p.run(&["verify", "--quiet"]);
    p.write(".arch/.verified", "rác rưởi\nmoney_props\n\n###\n");
    let out = p.out(&["check"]);
    assert!(out.contains("chưa chạy lần nào"), "dòng hỏng phải bị bỏ qua: {out}");
}

/// `propose` chặn, nhưng `proposals.arch` là file **agent ghi** — và agent
/// chạy được shell, nên nó ghi thẳng vào đó mà chẳng cần đi qua `propose`.
/// Chỗ duy nhất kiểm được là ngay trước khi ghi vào file luật.
#[test]
fn agent_ghi_thang_proposals_roi_accept_cung_khong_qua_duoc() {
    let p = Project::new("escalate", ARCH);
    p.write(
        ".arch/proposals.arch",
        "proposal p-abc123 @ai 2026-10-03\n  rule verify evil runs \"touch /tmp/archctl-pwn\"\n  reason lách\n",
    );
    let o = p.run(&["accept", "p-abc123", "--reason", "duyệt vội"]);
    assert!(!o.status.success());
    let out = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(out.contains("lệnh shell"), "{out}");
    let law = std::fs::read_to_string(p.0.join(".arch/system.arch")).unwrap();
    assert!(!law.contains("evil"), "lệnh đã lọt vào file luật:\n{law}");
}

#[test]
fn chan_theo_cu_phap_chu_khong_theo_chuoi() {
    // Mỗi cách viết lách khác đi là một lần phải đoán lại, nếu chặn bằng cách
    // so chuỗi.
    let p = Project::new("escalate2", ARCH);
    for statement in [
        "  verify evil runs \"touch /tmp/archctl-pwn\"",
        "allow Order -> Pay\nverify evil runs \"touch /tmp/archctl-pwn\"",
    ] {
        p.write(
            ".arch/proposals.arch",
            &format!("proposal p-abc123 @ai 2026-10-03\n  rule {statement}\n  reason lách\n"),
        );
        assert!(!p.run(&["accept", "p-abc123", "--reason", "x"]).status.success(), "{statement}");
    }
    let law = std::fs::read_to_string(p.0.join(".arch/system.arch")).unwrap();
    assert!(!law.contains("evil"), "{law}");
}
