//! Test đầu-tới-cuối cho giao thức agent–người, chạy chính binary.
//!
//! Vòng lặp đáng kiểm nhất là: agent đề xuất → người từ chối → **phiên sau
//! agent bị chặn không đề xuất lại**. Agent không có trí nhớ giữa các phiên,
//! nên nếu mắt xích cuối hỏng thì nó sẽ đề xuất lại mãi và người dùng sẽ tắt
//! cả hệ thống đi.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ARCH: &str = "\
component Web     at src/web/**     @human #locked
component Order   at src/order/**   @human #accepted
component Report  at src/report/**  @human #accepted
allow Web -> Order
";

struct Project(PathBuf);

impl Project {
    fn new(name: &str) -> Project {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir().join(format!("archctl-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".arch")).unwrap();
        std::fs::create_dir_all(root.join("src/web")).unwrap();
        std::fs::write(root.join(".arch/system.arch"), ARCH).unwrap();
        std::fs::write(root.join("src/web/page.ts"), "export const a = 1\n").unwrap();
        Project(root)
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_archctl"))
            .args(args)
            .arg("--root")
            .arg(&self.0)
            .output()
            .expect("chạy được archctl")
    }

    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.0.join(rel)).unwrap_or_default()
    }

    fn id_of(&self, marker: &str) -> String {
        let listing = text(&self.run(&["proposals"]));
        listing
            .lines()
            .find(|l| l.contains(marker))
            .and_then(|l| l.split_whitespace().find(|w| w.starts_with("p-")))
            .unwrap_or_else(|| panic!("không tìm thấy đề xuất `{marker}` trong:\n{listing}"))
            .to_string()
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

#[test]
fn de_xuat_roi_tu_choi_thi_phien_sau_bi_chan() {
    let p = Project::new("reject");
    let out = p.run(&["propose", "allow Order -> Report", "--reason", "cần số liệu"]);
    assert_eq!(code(&out), 3, "{}", text(&out));

    let id = p.id_of("ĐỀ XUẤT");
    let out = p.run(&["reject", &id, "--reason", "Order không được biết tới Report"]);
    assert_eq!(code(&out), 0, "{}", text(&out));

    // Phiên sau: cùng câu lệnh, lý do khác. Phải bị chặn kèm lý do cũ.
    let out = p.run(&["propose", "allow Order -> Report", "--reason", "vẫn cần"]);
    let said = text(&out);
    assert_eq!(code(&out), 3);
    assert!(said.contains("đã bị từ chối"), "{said}");
    assert!(said.contains("Order không được biết tới Report"), "lý do cũ phải hiện ra: {said}");
    assert!(!p.read(".arch/proposals.arch").contains("allow Order -> Report"));
}

#[test]
fn duyet_de_xuat_thi_luat_moi_vao_system_arch() {
    let p = Project::new("accept");
    p.run(&["propose", "allow Order -> Report", "--reason", "cần số liệu"]);
    let id = p.id_of("ĐỀ XUẤT");
    let out = p.run(&["accept", &id, "--reason", "hợp lý, Report chỉ đọc"]);
    assert_eq!(code(&out), 0, "{}", text(&out));

    let law = p.read(".arch/system.arch");
    assert!(law.contains("allow Order -> Report @ai #accepted"), "{law}");
    assert!(!p.read(".arch/proposals.arch").contains("allow Order -> Report"));
    assert!(p.read(".arch/decisions.md").contains("DUYỆT"));
    // Luật mới phải dùng được ngay.
    assert_eq!(code(&p.run(&["check"])), 0);
}

#[test]
fn de_xuat_trung_lap_la_khong_thao_tac() {
    let p = Project::new("dup");
    p.run(&["propose", "allow Order -> Report", "--reason", "lần đầu"]);
    let out = p.run(&["propose", "allow   Order  ->  Report", "--reason", "lần hai"]);
    assert!(text(&out).contains("đã có sẵn"), "{}", text(&out));
    assert_eq!(p.read(".arch/proposals.arch").matches("proposal ").count(), 1);
}

#[test]
fn cau_lenh_duoc_kiem_cung_voi_luat_hien_co() {
    let p = Project::new("validate");
    // Tham chiếu tới component đã khai báo: hợp lệ.
    assert_eq!(code(&p.run(&["propose", "allow Order -> Report", "--reason", "x"])), 3);
    // Tham chiếu tới component không tồn tại: bị chặn.
    let out = p.run(&["propose", "allow Order -> KhongCo", "--reason", "x"]);
    assert_eq!(code(&out), 64);
    assert!(text(&out).contains("KhongCo"), "{}", text(&out));
    // Sai cú pháp: bị chặn.
    let out = p.run(&["propose", "allow Order Report", "--reason", "x"]);
    assert_eq!(code(&out), 64);
    assert!(text(&out).contains("cú pháp"), "{}", text(&out));
}

#[test]
fn moi_quyet_dinh_deu_phai_co_ly_do() {
    let p = Project::new("reason");
    assert_eq!(code(&p.run(&["propose", "allow Order -> Report"])), 64);
    p.run(&["propose", "allow Order -> Report", "--reason", "x"]);
    let id = p.id_of("ĐỀ XUẤT");
    // Kể cả khi duyệt. Ma sát phải tương xứng với mức độ của hành vi.
    let out = p.run(&["accept", &id]);
    assert_eq!(code(&out), 64);
    assert!(text(&out).contains("lý do"), "{}", text(&out));
}

#[test]
fn bao_be_tac_ghi_vao_hang_cho() {
    let p = Project::new("block");
    let out = p.run(&["block", "--on", "allow Web -> Order", "--reason", "cần gọi ngược lại"]);
    assert_eq!(code(&out), 3, "{}", text(&out));
    assert!(text(&out).contains("DỪNG"), "{}", text(&out));
    assert!(p.read(".arch/proposals.arch").contains("block allow Web -> Order"));
    assert!(text(&p.run(&["proposals"])).contains("BẾ TẮC"));
}

#[test]
fn check_nhac_doc_lai_khi_kien_truc_doi() {
    let p = Project::new("hash");
    p.run(&["check"]);
    let law = p.read(".arch/system.arch");
    std::fs::write(p.0.join(".arch/system.arch"), format!("{law}allow Order -> Report\n")).unwrap();
    let said = text(&p.run(&["check"]));
    assert!(said.contains("Kiến trúc đã thay đổi"), "{said}");
    assert!(said.contains("Đọc lại"), "{said}");
}

#[test]
fn chi_them_chu_thich_thi_khong_bao_kien_truc_doi() {
    let p = Project::new("hash-quiet");
    p.run(&["check"]);
    let law = p.read(".arch/system.arch");
    std::fs::write(p.0.join(".arch/system.arch"), format!("# ghi chú mới\n\n{law}")).unwrap();
    let said = text(&p.run(&["check"]));
    assert!(!said.contains("Kiến trúc đã thay đổi"), "kêu sói: {said}");
}

#[test]
fn check_hien_de_xuat_dang_cho() {
    let p = Project::new("pending");
    p.run(&["propose", "allow Order -> Report", "--reason", "x"]);
    let said = text(&p.run(&["check"]));
    assert!(said.contains("ĐANG CHỜ DUYỆT"), "{said}");
    assert!(said.contains("đang chờ người quyết định"), "{said}");
}

#[test]
fn khong_co_de_xuat_nao_thi_noi_vay() {
    let p = Project::new("empty");
    let out = p.run(&["proposals"]);
    assert_eq!(code(&out), 0);
    assert!(text(&out).contains("Không có đề xuất nào"), "{}", text(&out));
}

#[test]
fn quyet_dinh_tren_id_khong_ton_tai_bi_tu_choi() {
    let p = Project::new("missing-id");
    let out = p.run(&["accept", "p-000000", "--reason", "x"]);
    assert_eq!(code(&out), 64);
    assert!(text(&out).contains("không có đề xuất"), "{}", text(&out));
}

#[test]
fn ngoai_du_an_co_arch_thi_bao_loi_ro_rang() {
    let out = Command::new(env!("CARGO_BIN_EXE_archctl"))
        .args(["check", "--root"])
        .arg(Path::new("/"))
        .output()
        .unwrap();
    assert_eq!(code(&out), 64);
    assert!(text(&out).contains(".arch/system.arch"), "{}", text(&out));
}

// ---------- init và baseline ----------

#[test]
fn init_dung_khung_arch_va_tro_cho_agent() {
    let p = Project::new("init");
    std::fs::remove_dir_all(p.0.join(".arch")).unwrap();
    let out = p.run(&["init"]);
    assert_eq!(code(&out), 0, "{}", text(&out));

    for f in [".arch/system.arch", ".arch/proposals.arch", ".arch/decisions.md", ".arch/ARCHITECTURE.md"] {
        assert!(p.0.join(f).is_file(), "thiếu {f}");
    }
    for f in ["AGENTS.md", "CLAUDE.md"] {
        assert!(p.read(f).contains(".arch/ARCHITECTURE.md"), "{f} không trỏ về hướng dẫn");
    }
    assert!(p.read(".gitignore").contains("**/.arch/.state"), "{}", p.read(".gitignore"));
    assert!(p.read(".gitignore").contains("**/.arch/tf-plan*"), "{}", p.read(".gitignore"));
}

#[test]
fn init_lai_them_dong_plan_cho_repo_cu() {
    let p = Project::new("init-gitignore-cu");
    std::fs::remove_dir_all(p.0.join(".arch")).unwrap();
    // Repo init bằng bản trước: đã có hai dòng, chưa có dòng plan Terraform.
    std::fs::write(p.0.join(".gitignore"), "target\n**/.arch/.state\n**/.arch/.verified\n").unwrap();
    assert_eq!(code(&p.run(&["init"])), 0);
    let g = p.read(".gitignore");
    assert_eq!(g.matches("**/.arch/.state").count(), 1, "{g}");
    assert!(g.contains("**/.arch/tf-plan*"), "{g}");
}

#[test]
fn init_khong_de_len_du_an_dang_co() {
    let p = Project::new("init-guard");
    let out = p.run(&["init"]);
    assert_eq!(code(&out), 64, "{}", text(&out));
    assert!(text(&out).contains("--force"), "{}", text(&out));
    assert_eq!(p.read(".arch/system.arch"), ARCH, "file luật phải còn nguyên");
}

#[test]
fn init_khong_xoa_nhat_ky_quyet_dinh() {
    let p = Project::new("init-keep-log");
    std::fs::write(p.0.join(".arch/decisions.md"), "# giữ lại tôi\n").unwrap();
    assert_eq!(code(&p.run(&["init", "--force"])), 0);
    assert!(p.read(".arch/decisions.md").contains("giữ lại tôi"), "nhật ký không được mất");
}

#[test]
fn init_from_code_dung_component_va_quan_he_tu_ma_that() {
    let p = Project::new("init-discover");
    std::fs::remove_dir_all(p.0.join(".arch")).unwrap();
    for (path, body) in [
        ("src/web/page.ts", "import { make } from '../order/create'\nexport const p = make\n"),
        ("src/order/create.ts", "export const make = 1\n"),
        ("src/order/edit.ts", "export const e = 2\n"),
    ] {
        let full = p.0.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }

    assert_eq!(code(&p.run(&["init", "--from-code"])), 0);
    let law = p.read(".arch/system.arch");
    assert!(law.contains("component Web") && law.contains("at src/web/**"), "{law}");
    assert!(law.contains("component Order") && law.contains("at src/order/**"), "{law}");
    assert!(law.contains("allow Web"), "quan hệ thật phải được tìm ra: {law}");
    assert!(law.contains("-> Order"), "{law}");
    // Mọi thứ phải ở #draft: đây là mô tả hiện trạng, chưa phải luật. Phần
    // chú thích có nhắc tới #accepted để hướng dẫn, nên chỉ xét dòng luật.
    let rules: Vec<&str> = law.lines().filter(|l| !l.trim_start().starts_with('#')).collect();
    assert!(rules.iter().all(|l| !l.contains("#accepted")), "{law}");
    assert!(rules.iter().filter(|l| !l.trim().is_empty()).all(|l| l.contains("#draft")), "{law}");
    assert_eq!(code(&p.run(&["check"])), 0, "chưa luật nào có hiệu lực nên không vi phạm");
}

#[test]
fn baseline_tha_vi_pham_cu_va_chan_vi_pham_moi() {
    let p = Project::new("baseline");
    // `allow Web -> Order` có sẵn, nên Web đã vào diện kiểm.
    std::fs::write(p.0.join("src/web/page.ts"), "import { r } from '../report/x'\n").unwrap();
    std::fs::create_dir_all(p.0.join("src/report")).unwrap();
    std::fs::write(p.0.join("src/report/x.ts"), "export const r = 1\n").unwrap();
    assert_eq!(code(&p.run(&["check"])), 1, "vi phạm có sẵn");

    assert_eq!(code(&p.run(&["baseline"])), 0);
    assert_eq!(code(&p.run(&["check"])), 0, "vi phạm cũ được tha");
    assert!(text(&p.run(&["check"])).contains("đang được tha"));

    // Vi phạm mới trong file khác thì không được tha.
    std::fs::write(p.0.join("src/web/other.ts"), "import { r } from '../report/x'\n").unwrap();
    assert_eq!(code(&p.run(&["check"])), 1, "vi phạm mới phải làm hỏng build");
}

#[test]
fn baseline_siet_lai_khi_vi_pham_cu_duoc_sua() {
    let p = Project::new("ratchet");
    std::fs::create_dir_all(p.0.join("src/report")).unwrap();
    std::fs::write(p.0.join("src/report/x.ts"), "export const r = 1\n").unwrap();
    std::fs::write(p.0.join("src/web/page.ts"), "import { r } from '../report/x'\n").unwrap();
    p.run(&["baseline"]);

    std::fs::write(p.0.join("src/web/page.ts"), "export const p = 1\n").unwrap();
    assert!(text(&p.run(&["check"])).contains("đã sửa xong"), "phải nhắc siết lại");

    let out = p.run(&["baseline"]);
    assert!(text(&out).contains("không quay lại được"), "{}", text(&out));
    assert!(text(&out).contains("Không còn vi phạm nào"), "{}", text(&out));
}

// ---------- quét một phần ----------

#[test]
fn cho_thang_duong_dan_thi_chi_quet_file_do() {
    let p = Project::new("only");
    std::fs::create_dir_all(p.0.join("src/report")).unwrap();
    std::fs::write(p.0.join("src/report/x.ts"), "export const r = 1\n").unwrap();
    std::fs::write(p.0.join("src/web/bad.ts"), "import { r } from '../report/x'\n").unwrap();
    std::fs::write(p.0.join("src/web/ok.ts"), "export const o = 1\n").unwrap();

    let said = text(&p.run(&["check", "src/web/ok.ts"]));
    assert!(said.contains("chỉ quét 1 file"), "{said}");
    assert!(!said.contains("VI PHẠM"), "file sạch thì không báo gì: {said}");
    assert_eq!(code(&p.run(&["check", "src/web/ok.ts"])), 0);

    let bad = p.0.join("src/web/bad.ts");
    assert_eq!(code(&p.run(&["check", bad.to_str().unwrap()])), 1, "đường dẫn tuyệt đối");
}

#[test]
fn quet_mot_phan_khong_in_con_so_tong_gay_hieu_nham() {
    let p = Project::new("partial");
    let said = text(&p.run(&["check", "src/web/page.ts"]));
    assert!(!said.contains("component trong diện kiểm"), "{said}");
    assert!(!said.contains("CHƯA KIỂM"), "{said}");
}

// ---------- tsconfig paths ----------

#[test]
fn alias_tsconfig_duoc_phan_giai() {
    let p = Project::new("tsconfig");
    std::fs::write(
        p.0.join("tsconfig.json"),
        "{\n  // có chú thích\n  \"compilerOptions\": { \"baseUrl\": \".\", \"paths\": { \"@/*\": [\"./src/*\"] } },\n}\n",
    )
    .unwrap();
    std::fs::create_dir_all(p.0.join("src/order")).unwrap();
    std::fs::write(p.0.join("src/order/create.ts"), "export const c = 1\n").unwrap();
    std::fs::write(p.0.join("src/web/page.ts"), "import { c } from '@/order/create'\n").unwrap();

    let said = text(&p.run(&["check"]));
    assert!(!said.contains("CHƯA PHÂN GIẢI"), "alias phải phân giải được: {said}");
    assert_eq!(code(&p.run(&["check"])), 0, "Web -> Order được allow: {said}");
}

#[test]
fn alias_tro_toi_cho_khong_co_that_thi_bao_ra() {
    let p = Project::new("tsconfig-missing");
    std::fs::write(
        p.0.join("tsconfig.json"),
        "{ \"compilerOptions\": { \"paths\": { \"@/*\": [\"./src/*\"] } } }",
    )
    .unwrap();
    std::fs::write(p.0.join("src/web/page.ts"), "import { x } from '@/khong/co'\n").unwrap();
    let said = text(&p.run(&["check"]));
    assert!(said.contains("CHƯA PHÂN GIẢI"), "{said}");
    assert!(said.contains("@/khong/co"), "{said}");
}

#[test]
fn khong_co_tsconfig_thi_alias_van_duoc_bao_la_chua_ho_tro() {
    let p = Project::new("no-tsconfig");
    std::fs::write(p.0.join("src/web/page.ts"), "import { x } from '@/bat/ky'\n").unwrap();
    assert!(text(&p.run(&["check"])).contains("alias"), "phải nói rõ vì sao");
}

#[test]
fn init_sinh_hook_cho_agent() {
    let p = Project::new("hooks");
    std::fs::remove_dir_all(p.0.join(".arch")).unwrap();
    assert_eq!(code(&p.run(&["init"])), 0);
    let settings = p.read(".claude/settings.json");
    assert!(settings.contains("PostToolUse"), "{settings}");
    assert!(settings.contains("archctl check --changed"), "{settings}");
}

/// Cấu hình sẵn có là file của người dùng; gộp vào đó là việc rủi ro.
#[test]
fn init_khong_dung_vao_cau_hinh_da_co() {
    let p = Project::new("hooks-keep");
    std::fs::remove_dir_all(p.0.join(".arch")).unwrap();
    std::fs::create_dir_all(p.0.join(".claude")).unwrap();
    std::fs::write(p.0.join(".claude/settings.json"), "{ \"cua-toi\": true }").unwrap();
    p.run(&["init"]);
    assert_eq!(p.read(".claude/settings.json"), "{ \"cua-toi\": true }");
}

#[test]
fn init_bo_qua_hook_khi_duoc_yeu_cau() {
    let p = Project::new("no-hooks");
    std::fs::remove_dir_all(p.0.join(".arch")).unwrap();
    p.run(&["init", "--no-hooks"]);
    assert!(!p.0.join(".claude/settings.json").exists());
}

/// Đoạn CI mà `init` in ra có dùng `--strict`, nên nó phải chạy thật.
#[test]
fn strict_coi_import_khong_phan_giai_duoc_la_vi_pham() {
    let p = Project::new("strict");
    std::fs::write(p.0.join("src/web/page.ts"), "import { x } from '@/khong/co'\n").unwrap();
    assert_eq!(code(&p.run(&["check"])), 0, "bình thường chỉ là cảnh báo");
    let out = p.run(&["check", "--strict"]);
    assert_eq!(code(&out), 1, "{}", text(&out));
    assert!(text(&out).contains("--strict"), "{}", text(&out));
}

/// Import Java không được đọc (TM-68), nên `--strict` chặn; người khai nhận
/// qua `propose`/`accept` thì cho qua, kèm lý do vẫn in ra; bỏ file Java đi
/// thì khai nhận lạc hậu và `--strict` lại chặn (TM-70).
#[test]
fn strict_va_khai_nhan_import_java() {
    let p = Project::new("strict-java");
    std::fs::write(p.0.join("src/web/Page.java"), "package web;\nimport order.Store;\nclass Page {}\n").unwrap();
    let out = p.run(&["check", "--strict"]);
    assert_eq!(code(&out), 1, "{}", text(&out));
    assert!(text(&out).contains("archctl propose 'unchecked Web imports java because \"<lý do>\"' --reason"), "{}", text(&out));

    let out = p.run(&["propose", "unchecked Web imports java because \"Web chỉ gọi Order qua gRPC\"", "--reason", "Java không đọc import"]);
    assert_eq!(code(&out), 3, "{}", text(&out));
    assert_eq!(code(&p.run(&["check", "--strict"])), 1, "đề xuất chưa duyệt chưa có hiệu lực");
    let id = p.id_of("ĐỀ XUẤT");
    assert_eq!(code(&p.run(&["accept", &id, "--reason", "chấp nhận"])), 0);

    let out = p.run(&["check", "--strict"]);
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(text(&out).contains("ĐÃ KHAI NHẬN") && text(&out).contains("Web chỉ gọi Order qua gRPC"), "{}", text(&out));
    let json = text(&p.run(&["check", "--json"]));
    assert!(json.contains(r#""importsAccepted":[{"component":"Web","lang":"Java","files":1,"reason":"Web chỉ gọi Order qua gRPC"}]"#), "{json}");

    std::fs::remove_file(p.0.join("src/web/Page.java")).unwrap();
    let out = p.run(&["check", "--strict"]);
    assert_eq!(code(&out), 1, "{}", text(&out));
    assert!(text(&out).contains("KHAI NHẬN LẠC HẬU"), "{}", text(&out));
    let json = text(&p.run(&["check", "--json"]));
    assert!(json.contains(r#""uncheckedUnused":[{"component":"Web","lang":"Java","line":5}]"#), "{json}");
}

/// `ARCH` có `allow Web -> Order` và không có gì cho `Order`, `Report`, nên dự
/// án này *có* cảnh báo — loại "chưa khai báo". `--strict` phải cho qua: áp
/// dần là cách dùng được thiết kế, và chặn ở đây nghĩa là không ai bật được
/// `--strict` cho tới khi đã mô tả xong cả repo.
#[test]
fn strict_khong_chan_du_an_moi_ap_luat_mot_phan() {
    let p = Project::new("strict-partial");
    let out = p.run(&["check", "--strict"]);
    assert!(text(&out).contains("CHƯA KIỂM"), "mong có cảnh báo chưa khai báo: {}", text(&out));
    assert_eq!(code(&out), 0, "{}", text(&out));
}

/// Một luật không ai kiểm được là loại "chưa kiểm được" tệ nhất: nó *có* mặt
/// trong file, đọc lên y như luật đang được áp. `--strict` chạy trên nhánh
/// chính nên nó phải chặn, chứ không phải in ra rồi đi tiếp.
#[test]
fn strict_coi_canh_bao_cua_file_luat_la_vi_pham() {
    let p = Project::new("strict-law");
    std::fs::write(
        p.0.join(".arch/system.arch"),
        format!("{ARCH}invariant money [system] tổng tiền khớp   @human #accepted\n"),
    )
    .unwrap();
    let out = p.run(&["check"]);
    assert_eq!(code(&out), 0, "không có --strict thì vẫn chỉ là cảnh báo: {}", text(&out));
    assert!(text(&out).contains("lời hứa"), "{}", text(&out));

    let out = p.run(&["check", "--strict"]);
    assert_eq!(code(&out), 1, "{}", text(&out));
}

/// In ba dòng ⚠ rồi kết luận "0 cảnh báo" thì dòng cuối — dòng duy nhất nhiều
/// người đọc — nói ngược lại những dòng ngay trên nó.
///
/// So hai lần chạy thay vì khẳng định một con số: con số đúng tình cờ không
/// chứng minh được gì, còn "thêm một luật không kiểm được thì đếm tăng đúng
/// một" thì có.
#[test]
fn tong_ket_dem_ca_canh_bao_cua_file_luat() {
    fn dem(out: &str) -> usize {
        out.lines()
            .find_map(|l| l.split_once(" cảnh báo"))
            .and_then(|(head, _)| head.rsplit(' ').next())
            .and_then(|n| n.parse().ok())
            .unwrap_or_else(|| panic!("không thấy dòng tổng kết trong:\n{out}"))
    }

    let p = Project::new("count-law");
    let truoc = dem(&text(&p.run(&["check"])));
    std::fs::write(
        p.0.join(".arch/system.arch"),
        format!("{ARCH}invariant money [system] tổng tiền khớp   @human #accepted\n"),
    )
    .unwrap();
    let sau = text(&p.run(&["check"]));
    assert!(sau.contains("lời hứa"), "{sau}");
    assert_eq!(dem(&sau), truoc + 1, "{sau}");
}

/// Bản người đọc cắt ở mười hai mục và bảo "xem `--json`". Nếu `--json` thiếu
/// hẳn một loại thì mã thoát và nội dung JSON nói hai chuyện khác nhau.
#[test]
fn json_co_canh_bao_cua_file_luat() {
    let p = Project::new("json-law");
    std::fs::write(
        p.0.join(".arch/system.arch"),
        format!("{ARCH}invariant money [system] tổng tiền khớp   @human #accepted\n"),
    )
    .unwrap();
    let out = text(&p.run(&["check", "--json"]));
    assert!(out.contains(r#""lawWarnings":[{"#), "{out}");
    assert!(out.contains("lời hứa"), "{out}");
}

/// `--changed` là đường mà hook của agent chạy sau mỗi lần sửa file. Bỏ sót ở
/// đây nghĩa là agent nghe "sạch" trong khi mã đang vi phạm — chế độ hỏng tệ
/// nhất mà công cụ này có thể mắc.
mod changed {
    use super::*;

    fn git(root: &PathBuf, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .expect("chạy được git");
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// Repo đã commit, rồi `src/web/<name>` được sửa thành một vi phạm
    /// `Web -> Report`. Luật không nằm trong danh sách thay đổi, nên
    /// `--changed` thật sự chỉ xét file vừa sửa.
    fn project(test: &str, name: &str) -> Option<Project> {
        if Command::new("git").arg("--version").output().is_err() {
            return None;
        }
        let p = Project::new(test);
        std::fs::create_dir_all(p.0.join("src/report")).unwrap();
        std::fs::write(p.0.join("src/report/x.ts"), "export const r = 1\n").unwrap();
        std::fs::write(p.0.join(format!("src/web/{name}")), "export const a = 1\n").unwrap();
        git(&p.0, &["init", "-q", "."]);
        git(&p.0, &["add", "-A"]);
        git(&p.0, &["commit", "-qm", "init"]);
        std::fs::write(p.0.join(format!("src/web/{name}")), "import { r } from '../report/x'\n")
            .unwrap();
        Some(p)
    }

    #[test]
    fn ten_file_khong_thuan_ascii_van_duoc_kiem() {
        // git bọc nháy và thoát bát phân những tên này, nên đường dẫn đọc ra
        // không có thật trên đĩa và file bị bỏ qua trong im lặng.
        let Some(p) = project("changed-unicode", "trang-chủ.ts") else { return };
        let out = text(&p.run(&["check", "--changed"]));
        assert!(out.contains("Web -> Report"), "{out}");
    }

    #[test]
    fn ten_file_co_khoang_trang_van_duoc_kiem() {
        let Some(p) = project("changed-space", "trang chủ.ts") else { return };
        let out = text(&p.run(&["check", "--changed"]));
        assert!(out.contains("Web -> Report"), "{out}");
    }

    #[test]
    fn doi_ten_thi_xet_ten_moi() {
        let Some(p) = project("changed-rename", "cũ.ts") else { return };
        git(&p.0, &["add", "-A"]);
        git(&p.0, &["commit", "-qm", "wip"]);
        git(&p.0, &["mv", "src/web/cũ.ts", "src/web/mới.ts"]);
        let out = text(&p.run(&["check", "--changed"]));
        assert!(out.contains("Web -> Report"), "{out}");
    }

    #[test]
    fn xoa_file_khong_lam_hong_gi() {
        let Some(p) = project("changed-delete", "a.ts") else { return };
        git(&p.0, &["add", "-A"]);
        git(&p.0, &["commit", "-qm", "wip"]);
        git(&p.0, &["rm", "-q", "src/web/a.ts"]);
        let out = text(&p.run(&["check", "--changed"]));
        assert!(out.contains("0 vi phạm mới"), "{out}");
    }

    #[test]
    fn sua_luat_thi_kiem_lai_ca_repo() {
        let Some(p) = project("changed-law", "a.ts") else { return };
        git(&p.0, &["add", "-A"]);
        git(&p.0, &["commit", "-qm", "wip"]);
        // Chỉ luật đổi, không file nguồn nào đổi — mà vi phạm vẫn phải hiện.
        std::fs::write(p.0.join(".arch/system.arch"), format!("{ARCH}# ghi chú\n")).unwrap();
        let out = text(&p.run(&["check", "--changed"]));
        assert!(out.contains("Web -> Report"), "{out}");
    }
}

/// Trên một cây 5078 file, một lần `check` từng in ra 7247 dòng — và danh
/// sách dài tới mức đó thì người đọc bỏ qua cả những dòng đáng đọc.
#[test]
fn canh_bao_lap_lai_duoc_gop_va_cat_bot() {
    let p = Project::new("flood");
    std::fs::create_dir_all(p.0.join("src/web")).unwrap();
    // Một header không có thật, include ở ba mươi chỗ.
    for i in 0..30 {
        std::fs::write(
            p.0.join(format!("src/web/f{i}.c")),
            "#include \"khong-co-that.h\"\nint main(void){return 0;}\n",
        )
        .unwrap();
    }
    let out = text(&p.run(&["check"]));
    assert!(out.contains("(30 chỗ)"), "phải gộp theo tên: {out}");
    assert_eq!(out.matches("CHƯA PHÂN GIẢI").count(), 1, "{out}");
    assert!(out.lines().count() < 30, "{} dòng là quá nhiều", out.lines().count());
}

#[test]
fn danh_sach_dai_thi_noi_ro_con_bao_nhieu() {
    let p = Project::new("capped");
    std::fs::create_dir_all(p.0.join("src/web")).unwrap();
    for i in 0..20 {
        std::fs::write(
            p.0.join(format!("src/web/f{i}.c")),
            format!("#include \"thieu-{i}.h\"\nint main(void){{return 0;}}\n"),
        )
        .unwrap();
    }
    let out = text(&p.run(&["check"]));
    assert!(out.contains("còn 8 tên nữa"), "{out}");
    // `--json` vẫn phải đưa đủ.
    let json = text(&p.run(&["check", "--json"]));
    assert_eq!(json.matches("\"spec\"").count(), 20, "{json}");
}

#[test]
fn scan_in_import_doc_duoc_va_dich_phan_giai() {
    // Bộ đo đối chiếu đầu ra này với parser chuẩn của từng ngôn ngữ, nên nó
    // phải là đúng thứ `check` đọc: cùng import, cùng dòng, cùng đích.
    let p = Project::new("scan");
    std::fs::create_dir_all(p.0.join("src/order")).unwrap();
    std::fs::write(
        p.0.join("src/order/create.py"),
        "import os\nfrom . import util\ntry:\n    import fast\nexcept ImportError as e:\n    raise E from e\n",
    )
    .unwrap();
    std::fs::write(p.0.join("src/order/util.py"), "").unwrap();

    let out = p.run(&["scan", "--json", "src/order/create.py"]);
    assert_eq!(code(&out), 0, "{}", text(&out));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains(r#""file":"src/order/create.py","lang":"Python""#), "{s}");
    assert!(s.contains(r#""line":1,"spec":"os""#), "{s}");
    assert!(s.contains(r#""resolved":"package","target":"os""#), "{s}");
    assert!(s.contains(r#""line":2,"spec":".""#), "{s}");
    assert!(s.contains(r#""submodules":["src/order/util.py"]"#), "{s}");
    assert!(s.contains(r#""line":4,"spec":"fast""#), "{s}");
    assert!(!s.contains(r#""spec":"e""#), "`raise ... from e` không phải import: {s}");

    let human = text(&p.run(&["scan", "src/order/create.py"]));
    assert!(human.contains("src/order/create.py") && human.contains("os"), "{human}");
}

#[test]
fn scan_in_ca_canh_lien_ngon_ngu_cua_tung_file() {
    // Bộ đo đối chiếu cả phần này: lời gọi `torch.ops` phía Python với `ast`,
    // module và op phía C++ với lexer thô của clang. Khoá in ra là khoá thô,
    // trước khi `TORCH_EXTENSION_NAME` được thay bằng tên trong `setup.py`.
    let p = Project::new("scan-crossings");
    std::fs::create_dir_all(p.0.join("app")).unwrap();
    std::fs::create_dir_all(p.0.join("native")).unwrap();
    std::fs::write(p.0.join("app/k.py"), "import torch\n\ny = torch.ops.mylib.fused(x)\n").unwrap();
    std::fs::write(
        p.0.join("native/ops.cpp"),
        "TORCH_LIBRARY(mylib, m) {\n  m.def(\"fused(Tensor x) -> Tensor\");\n}\n\
         PYBIND11_MODULE(TORCH_EXTENSION_NAME, m) {}\n",
    )
    .unwrap();

    let out = p.run(&["scan", "--json", "app/k.py", "native/ops.cpp"]);
    assert_eq!(code(&out), 0, "{}", text(&out));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains(r#""calls":[{"line":3,"key":"op:mylib.fused","shown":"torch.ops.mylib.fused"}]"#), "{s}");
    assert!(s.contains(r#"{"line":2,"key":"op:mylib.fused","shown":"mylib::fused"}"#), "{s}");
    assert!(s.contains(r#"{"line":4,"key":"py:@ext","shown":"@ext"}"#), "{s}");

    let human = text(&p.run(&["scan", "app/k.py", "native/ops.cpp"]));
    assert!(human.contains("torch.ops.mylib.fused") && human.contains("qua ranh giới ngôn ngữ"), "{human}");
}

#[test]
fn scan_in_dich_vu_tu_compose_khong_in_gia_tri_env() {
    // Bộ đo đối chiếu phần `deploy` với PyYAML. Giá trị biến môi trường không
    // bao giờ được in, chỉ tên máy trong đó.
    let p = Project::new("scan-deploy");
    std::fs::create_dir_all(p.0.join("app")).unwrap();
    std::fs::write(p.0.join("app/main.py"), "x = 1\n").unwrap();
    std::fs::write(p.0.join("Dockerfile"), "FROM python:3.12\nWORKDIR /srv\nCOPY app ./app\nCMD [\"python\", \"-m\", \"app.main\"]\n").unwrap();
    std::fs::write(
        p.0.join("docker-compose.yml"),
        "services:\n  api:\n    build: .\n    environment:\n      DATABASE_URL: postgres://u:hunter2@db:5432/x\n    depends_on: [db]\n  db:\n    image: postgres:16\n",
    )
    .unwrap();

    let out = p.run(&["scan", "--json", "docker-compose.yml", "Dockerfile"]);
    assert_eq!(code(&out), 0, "{}", text(&out));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains(r#""env":[{"key":"DATABASE_URL","line":5}]"#), "{s}");
    assert!(s.contains(r#""dependsOn":[["db",6]]"#), "{s}");
    assert!(s.contains(r#"{"line":4,"keyword":"CMD"}"#), "mặc định không in đối số: {s}");
    assert!(s.contains(r#""entries":["app/main.py"]"#), "lệnh khởi chạy dẫn tới file: {s}");
    assert!(!s.contains("hunter2"), "{s}");
    let verbatim = p.run(&["scan", "--json", "--verbatim", "Dockerfile"]);
    let v = String::from_utf8_lossy(&verbatim.stdout);
    assert!(v.contains(r#""keyword":"CMD","args":"[\"python\", \"-m\", \"app.main\"]""#), "{v}");

    let human = text(&p.run(&["scan", "docker-compose.yml"]));
    assert!(human.contains("mạng → db") && !human.contains("hunter2"), "{human}");
}

#[test]
fn check_bao_canh_mang_thieu_link_va_strict_tinh_canh_bao_ha_tang() {
    let p = Project::new("check-deploy");
    std::fs::write(
        p.0.join("docker-compose.yml"),
        "services:\n  api:\n    image: x\n    environment:\n      REDIS_URL: redis://:secret@cache:6379/0\n  cache:\n    image: redis:7\n",
    )
    .unwrap();
    std::fs::write(
        p.0.join(".arch/system.arch"),
        format!("{ARCH}\nservice Api from compose:api\nservice Cache from compose:cache\nservice Db from compose:db\nlink Api -> Db\n"),
    )
    .unwrap();

    let out = p.run(&["check", "--json"]);
    assert_eq!(code(&out), 1, "{}", text(&out));
    let s = String::from_utf8_lossy(&out.stdout);
    assert!(s.contains(r#""from":"Api","to":"Cache""#) && s.contains(r#""via":"network""#), "{s}");
    assert!(s.contains(r#""reason":"not_linked""#) && s.contains(r#""spec":"REDIS_URL → cache:6379""#), "{s}");
    assert!(s.contains(r#""missing":[{"name":"compose:db""#), "{s}");
    for key in ["\"deployUnread\":[", "\"entryUnknown\":[", "\"servicesUndeclared\":[", "\"linksUnseen\":["] {
        assert!(s.contains(key), "thiếu `{key}`: {s}");
    }
    assert!(!s.contains("secret"), "{s}");

    let human = text(&p.run(&["check"]));
    assert!(human.contains("✗ VI PHẠM  Api -> Cache  (cạnh mạng không có link"), "{human}");
    assert!(human.contains("archctl propose 'link Api -> Cache'") || !human.contains("đã gặp lại"), "{human}");
}

/// TM-10: `scan` mặc định không in thứ gì có thể là secret. Các chuỗi đánh dấu
/// là những chỗ từng lọt ra: `ENV` của Dockerfile, argv của lệnh, URL build
/// context có token, giá trị của khoá trông như tên máy.
#[test]
fn scan_mac_dinh_khong_in_secret() {
    let p = Project::new("scan-secret");
    std::fs::write(
        p.0.join("Dockerfile"),
        "FROM python\nENV OPENAI_API_KEY=sk-danhdau1\nARG TOKEN=danhdau2\nRUN curl -H 'Authorization: Bearer danhdau3' x\n\
         ADD https://deploy:danhdau8@git.example.com/x.tar.gz /opt/\n\
         COPY <<EOF /app/settings.py\nSTRIPE_KEY=\"danhdau9\"\nEOF\n\
         CMD [\"python\", \"app.py\"]\n",
    )
    .unwrap();
    std::fs::write(
        p.0.join("docker-compose.yml"),
        "include:\n  - https://user:danhdauA@example.com/c.yml\nservices:\n  cache:\n    image: redis\n    command: redis-server --requirepass danhdau4\n    environment:\n      SUPABASE_SERVICE_ROLE_KEY: eyJdanhdau5\n      SECURITY_PASSWORD_SALT: danhdau6\n  web:\n    build: https://ghp_danhdau7@github.com/x/y.git\n",
    )
    .unwrap();
    for args in [&["scan", "--json"][..], &["scan"][..], &["scan", "--json", "Dockerfile", "docker-compose.yml"][..]] {
        let out = text(&p.run(args));
        for marker in
            ["danhdau1", "danhdau2", "danhdau3", "danhdau4", "danhdau5", "danhdau6", "danhdau7", "danhdau8", "danhdau9", "danhdauA"]
        {
            assert!(!out.contains(marker), "{args:?} lộ `{marker}`:\n{out}");
        }
    }
    // `--verbatim` là lựa chọn có chủ ý, cho bộ đo: đối số được in (kể cả
    // `ENV` của Dockerfile), nhưng giá trị biến môi trường của compose thì vẫn
    // không.
    let out = text(&p.run(&["scan", "--json", "--verbatim", "Dockerfile", "docker-compose.yml"]));
    assert!(out.contains("danhdau4") && !out.contains("danhdau5") && !out.contains("danhdau6"), "{out}");
}


#[test]
fn scan_chi_in_ten_may_la_service() {
    // Chuỗi dạng `a:b` khác có thể là `user:mật khẩu` (TM-33): mặc định `scan` chỉ
    // in tên máy khớp một service của repo; `--all-hosts` in mọi ứng viên.
    let p = Project::new("scan-hosts");
    std::fs::write(p.0.join("docker-compose.yml"), "services:\n  db:\n    image: postgres:16\n").unwrap();
    std::fs::write(p.0.join("src/web/conn.py"), "DSN = 'postgres://app:hunter2@db:5432/x'\nx = 'alice:31337'\n").unwrap();
    for args in [&["scan", "--json", "src/web/conn.py"][..], &["scan", "src/web/conn.py"][..]] {
        let out = text(&p.run(args));
        assert!(out.contains("db"), "{out}");
        assert!(!out.contains("alice") && !out.contains("31337") && !out.contains("hunter2"), "{out}");
    }
    let all = text(&p.run(&["scan", "--json", "--all-hosts", "src/web/conn.py"]));
    assert!(all.contains("alice") && !all.contains("hunter2"), "{all}");
}

/// `scan`, `check` không in `cmd` của dvc hay đối số của operator Airflow:
/// lệnh có thể mang token (TM-66).
#[test]
fn pipeline_khong_in_bi_mat() {
    let p = Project::new("pipeline-secret");
    let write = |rel: &str, body: &str| {
        let path = p.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    };
    write("src/order/prep.py", "def run():\n    pass\n");
    write("src/report/train.py", "def run():\n    pass\n");
    write(
        "dvc.yaml",
        "stages:\n  prep:\n    cmd: python src/order/prep.py --token hunter2-dvc\n    outs: [d.csv]\n  train:\n    cmd: python src/report/train.py\n    deps: [d.csv]\n",
    );
    write(
        "src/web/dag.py",
        "from airflow.operators.bash import BashOperator\nfrom airflow.operators.python import PythonOperator\nfrom src.order.prep import run\nb = BashOperator(task_id='b', bash_command='curl -u x:hunter2-af http://h')\na = PythonOperator(task_id='a', python_callable=run, op_kwargs={'k': 'hunter2-kw'})\na >> b\n",
    );
    for args in [&["scan", "--json"][..], &["scan"], &["check", "--json"], &["check"]] {
        let out = p.run(args);
        let all = format!("{}{}", text(&out), String::from_utf8_lossy(&out.stderr));
        for secret in ["hunter2-dvc", "hunter2-af", "hunter2-kw"] {
            assert!(!all.contains(secret), "`{args:?}` lọt {secret}:\n{all}");
        }
    }
    let scan = text(&p.run(&["scan", "--json"]));
    assert!(scan.contains(r#""from":"prep","to":"train","path":"d.csv""#), "{scan}");
}

/// Tên lấy từ file của repo không điều khiển được terminal: ESC, CR, ký tự
/// đảo chiều chữ thành `?` ở đầu ra chữ; JSON giữ nguyên giá trị (đã thoát)
/// (TM-67).
#[test]
fn ky_tu_dieu_khien_khong_ra_terminal() {
    let p = Project::new("ctrl-chars");
    std::fs::write(
        p.0.join("src/web/a.ts"),
        "import x from \"./\\u001b[2K\\rgone\\u202e\";\nimport y from \"./\\u0085\\u009b2K\\u007fz\";\n",
    )
    .unwrap();
    std::fs::write(p.0.join("docker-compose.yml"), "services:\n  \"a\\e[8mb\":\n    build: .\n").unwrap();
    for args in [&["check"][..], &["scan"]] {
        let out = p.run(args);
        let all = format!("{}{}", text(&out), String::from_utf8_lossy(&out.stderr));
        for c in ['\u{1b}', '\r', '\u{202e}', '\u{85}', '\u{9b}', '\u{7f}'] {
            assert!(!all.contains(c), "`{args:?}` in ra {c:?}:\n{all}");
        }
    }
    assert!(text(&p.run(&["check"])).contains("./?[2K?gone?"));
    // JSON thoát cả C1, DEL, ký tự đổi hướng thành `\uXXXX`: đúng giá trị, sạch
    // với terminal.
    let json = text(&p.run(&["check", "--json"]));
    assert!(json.contains(r"\u001b[2K\rgone\u202e") && json.contains(r"\u0085\u009b2K\u007fz"), "{json}");
    assert!(!json.contains('?'), "{json}");
}
