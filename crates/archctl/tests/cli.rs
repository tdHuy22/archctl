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
    assert!(p.read(".gitignore").contains(".arch/.state"));
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
