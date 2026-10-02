//! Test cho `archctl serve`, chạy máy chủ thật rồi gọi API.
//!
//! Phần đáng kiểm nhất không phải là HTTP mà là **thao tác ghi**: khoá một
//! luật chỉ được đụng đúng một dòng, và không bao giờ được để lại file luật
//! hỏng.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};

const ARCH: &str = "\
component Web    at src/web/**    @human #accepted
component Order  at src/order/**  @human #accepted
component Report at src/report/** @ai #draft
allow Web -> Order
";

struct Server {
    child: Child,
    port: u16,
    root: PathBuf,
}

impl Server {
    fn start(name: &str, port: u16) -> Server {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root =
            std::env::temp_dir().join(format!("archctl-serve-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".arch")).unwrap();
        std::fs::create_dir_all(root.join("src/web")).unwrap();
        std::fs::create_dir_all(root.join("src/report")).unwrap();
        std::fs::write(root.join(".arch/system.arch"), ARCH).unwrap();
        std::fs::write(root.join("src/report/x.ts"), "export const r = 1\n").unwrap();
        // Vi phạm có sẵn: Web đã vào diện kiểm nhưng không được gọi Report.
        std::fs::write(root.join("src/web/page.ts"), "import { r } from '../report/x'\n").unwrap();

        let child = Command::new(env!("CARGO_BIN_EXE_archctl"))
            .args(["serve", "--port", &port.to_string(), "--root"])
            .arg(&root)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("chạy được archctl serve");

        let server = Server { child, port, root };
        for _ in 0..60 {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                return server;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("máy chủ không lên sau 3 giây");
    }

    fn request(&self, method: &str, path: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("kết nối được");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n",
            body.len()
        );
        stream.write_all(head.as_bytes()).unwrap();
        stream.write_all(body.as_bytes()).unwrap();

        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).unwrap();
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).unwrap() == 0 || header.trim().is_empty() {
                break;
            }
        }
        let mut payload = String::new();
        reader.read_to_string(&mut payload).unwrap();
        format!("{}|{payload}", status.trim())
    }

    fn get(&self, path: &str) -> String {
        self.request("GET", path, "")
    }

    fn post(&self, path: &str, body: &str) -> String {
        self.request("POST", path, body)
    }

    fn law(&self) -> String {
        std::fs::read_to_string(self.root.join(".arch/system.arch")).unwrap()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn trang_chu_tra_ve_giao_dien() {
    let s = Server::start("home", 4731);
    let out = s.get("/");
    assert!(out.starts_with("HTTP/1.1 200 OK|"), "{}", &out[..60.min(out.len())]);
    assert!(out.contains("<title>archctl</title>"));
    // Giao diện phải tự chứa mọi thứ: không gọi mạng thì mở được cả khi offline.
    assert!(!out.contains("https://"), "giao diện không được tải gì từ ngoài");
}

#[test]
fn mo_hinh_co_du_node_canh_va_vi_pham() {
    let s = Server::start("model", 4732);
    let out = s.get("/api/model");
    assert!(out.contains(r#""name":"Web""#), "{out}");
    assert!(out.contains(r#""kind":"component""#), "{out}");
    assert!(out.contains(r#""from":"Web","to":"Order""#), "{out}");
    // Toạ độ có sẵn, giao diện không phải tự tính layout.
    assert!(out.contains(r#""x":"#) && out.contains(r#""y":"#), "{out}");
    assert!(out.contains(r#""from":"Web","to":"Report""#), "vi phạm phải có: {out}");
}

#[test]
fn mo_hinh_luon_co_moi_truong_ma_giao_dien_doc() {
    let s = Server::start("fields", 4742);
    let out = s.get("/api/model");
    // Thiếu một trường thì `summarize()` ném lỗi và cả sơ đồ trắng xoá, chứ
    // không chỉ mất một tab. Nên sự có mặt của chúng là một ràng buộc.
    for field in
        ["nodes", "edges", "boundaries", "violations", "missing", "vacuous", "proposals", "stats"]
    {
        assert!(out.contains(&format!(r#""{field}":"#)), "thiếu `{field}`: {out}");
    }
}

#[test]
fn khoa_mot_luat_chi_dung_mot_dong() {
    let s = Server::start("lock", 4733);
    assert!(s.post("/api/status", r##"{"name":"Report","status":"#locked"}"##).contains("200 OK"));

    let law = s.law();
    assert!(law.contains("component Report at src/report/**  @ai #locked"), "{law}");
    // Mọi dòng khác giữ nguyên từng ký tự.
    assert!(law.contains("component Web    at src/web/**    @human #accepted"), "{law}");
    assert!(law.contains("allow Web -> Order"), "{law}");
    assert_eq!(law.lines().count(), ARCH.lines().count(), "không thêm bớt dòng nào");
}

#[test]
fn hai_tab_cung_sua_mot_luc_khong_mat_thay_doi_nao() {
    // Mỗi kết nối một luồng, nên đọc-sửa-ghi của hai yêu cầu có thể xen vào
    // nhau: bản ghi sau dựa trên bản đọc trước và nuốt mất thay đổi kia.
    let s = std::sync::Arc::new(Server::start("race", 4743));
    let names = ["Web", "Order", "Report"];
    let threads: Vec<_> = names
        .iter()
        .map(|name| {
            let s = std::sync::Arc::clone(&s);
            let body = format!(r##"{{"name":"{name}","status":"#locked"}}"##);
            std::thread::spawn(move || s.post("/api/status", &body))
        })
        .collect();
    for t in threads {
        assert!(t.join().unwrap().contains("200 OK"));
    }

    let law = s.law();
    for name in names {
        assert!(law.contains(&format!("{name} ")) && law.contains("#locked"), "{law}");
        let line = law.lines().find(|l| l.contains(&format!("component {name} "))).unwrap();
        assert!(line.ends_with("#locked"), "mất thay đổi của `{name}`: {law}");
    }
    assert_eq!(law.lines().count(), ARCH.lines().count(), "không thêm bớt dòng nào");
}

#[test]
fn ghi_xong_khong_de_lai_file_tam() {
    let s = Server::start("tidy", 4744);
    assert!(s.post("/api/layout", r#"{"name":"Web","x":"10","y":"20"}"#).contains("200 OK"));
    assert!(s.post("/api/status", r##"{"name":"Web","status":"#locked"}"##).contains("200 OK"));
    for dir in [s.root.clone(), s.root.join(".arch")] {
        let leftovers: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "còn file tạm trong {}: {leftovers:?}", dir.display());
    }
}

#[test]
fn khoa_phan_tu_khong_ton_tai_bi_tu_choi() {
    let s = Server::start("lock-missing", 4734);
    let before = s.law();
    let out = s.post("/api/status", r##"{"name":"KhongCo","status":"#locked"}"##);
    assert!(out.contains("400"), "{out}");
    assert_eq!(s.law(), before, "từ chối thì không được đụng vào file");
}

#[test]
fn trang_thai_khong_hop_le_bi_tu_choi() {
    let s = Server::start("lock-bad", 4735);
    let before = s.law();
    assert!(s.post("/api/status", r##"{"name":"Web","status":"#bua"}"##).contains("400"));
    assert_eq!(s.law(), before);
}

#[test]
fn keo_tha_duoc_luu_lai() {
    let s = Server::start("layout", 4736);
    assert!(s.post("/api/layout", r#"{"name":"Web","x":"420","y":"88"}"#).contains("200 OK"));

    let saved = std::fs::read_to_string(s.root.join(".arch/layout.json")).unwrap();
    assert!(saved.contains(r#""Web""#) && saved.contains("420"), "{saved}");
    // Vị trí đã lưu phải thắng layout tự tính.
    assert!(s.get("/api/model").contains(r#""x":420"#), "{}", s.get("/api/model"));
}

#[test]
fn sap_xep_lai_xoa_vi_tri_da_luu() {
    let s = Server::start("relayout", 4737);
    s.post("/api/layout", r#"{"name":"Web","x":"999","y":"999"}"#);
    assert!(s.post("/api/relayout", "{}").contains("200 OK"));
    assert!(!s.root.join(".arch/layout.json").exists());
}

#[test]
fn duyet_de_xuat_qua_api_ghi_vao_luat_va_nhat_ky() {
    let s = Server::start("decide", 4738);
    Command::new(env!("CARGO_BIN_EXE_archctl"))
        .args(["propose", "allow Web -> Report", "--reason", "cần số liệu", "--root"])
        .arg(&s.root)
        .output()
        .unwrap();

    let model = s.get("/api/model");
    assert!(model.contains(r#""intent":"rule""#), "{model}");
    let id = model
        .split(r#""id":""#)
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("có id");

    let body = format!(r#"{{"id":"{id}","verdict":"accept","reason":"ổn"}}"#);
    assert!(s.post("/api/decide", &body).contains("200 OK"));
    assert!(s.law().contains("allow Web -> Report"), "{}", s.law());
    let log = std::fs::read_to_string(s.root.join(".arch/decisions.md")).unwrap();
    assert!(log.contains("DUYỆT") && log.contains("ổn"), "{log}");
}

#[test]
fn quyet_dinh_khong_co_ly_do_bi_tu_choi() {
    let s = Server::start("decide-noreason", 4739);
    let out = s.post("/api/decide", r#"{"id":"p-000000","verdict":"accept","reason":"  "}"#);
    assert!(out.contains("400") && out.contains("lý do"), "{out}");
}

#[test]
fn file_luat_hong_thi_bao_loi_chu_khong_sap() {
    let s = Server::start("broken", 4740);
    std::fs::write(s.root.join(".arch/system.arch"), "compoent Web at x/**\n").unwrap();
    let out = s.get("/api/model");
    assert!(out.contains("200 OK"), "{out}");
    assert!(out.contains(r#""error""#), "{out}");
    assert!(out.contains("compoent"), "phải nói rõ sai ở đâu: {out}");
}

#[test]
fn duong_dan_la_tra_ve_404() {
    let s = Server::start("notfound", 4741);
    assert!(s.get("/khong-co").contains("404"));
}
