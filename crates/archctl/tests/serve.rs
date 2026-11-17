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
    /// Khoá phiên mà máy chủ in ra lúc khởi động.
    key: String,
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

        let log = root.join("serve.log");
        let child = Command::new(env!("CARGO_BIN_EXE_archctl"))
            .args(["serve", "--port", &port.to_string(), "--root"])
            .arg(&root)
            .stdout(std::fs::File::create(&log).unwrap())
            .spawn()
            .expect("chạy được archctl serve");

        let mut server = Server { child, port, root, key: String::new() };
        for _ in 0..60 {
            if TcpStream::connect(("127.0.0.1", port)).is_ok() {
                // Khoá chỉ đi ra màn hình của người dùng, nên test cũng phải
                // lấy nó từ đó.
                let printed = std::fs::read_to_string(&log).unwrap_or_default();
                if let Some(key) = printed.split("?k=").nth(1).and_then(|r| r.split_whitespace().next())
                {
                    server.key = key.to_string();
                    return server;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("máy chủ không lên sau 3 giây");
    }

    fn request(&self, method: &str, path: &str, body: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", self.port)).expect("kết nối được");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nX-Archctl-Key: {}\r\n\
             Content-Length: {}\r\n\r\n",
            self.key,
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

    /// Gửi thẳng byte thô, để dựng được những yêu cầu mà client tử tế không
    /// bao giờ gửi.
    fn raw(&self, request: &str) -> String {
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", self.port)) else {
            return String::new();
        };
        let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(3)));
        if stream.write_all(request.as_bytes()).is_err() {
            return String::new();
        }
        let mut reply = String::new();
        let _ = BufReader::new(stream).read_line(&mut reply);
        reply.trim().to_string()
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
        ["nodes", "edges", "boundaries", "violations", "missing", "vacuous", "unproven", "proposals",
         "stats"]
    {
        assert!(out.contains(&format!(r#""{field}":"#)), "thiếu `{field}`: {out}");
    }
}

#[test]
fn khoa_mot_luat_chi_dung_mot_dong() {
    let s = Server::start("lock", 4733);
    assert!(s.post("/api/status", r##"{"name":"Report","status":"#locked","reason":"khoá lại"}"##).contains("200 OK"));

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
            let body = format!(r##"{{"name":"{name}","status":"#locked","reason":"khoá lại"}}"##);
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
    assert!(s.post("/api/status", r##"{"name":"Web","status":"#locked","reason":"khoá lại"}"##).contains("200 OK"));
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
    let out = s.post("/api/status", r##"{"name":"KhongCo","status":"#locked","reason":"khoá lại"}"##);
    assert!(out.contains("400"), "{out}");
    assert_eq!(s.law(), before, "từ chối thì không được đụng vào file");
}

#[test]
fn trang_thai_khong_hop_le_bi_tu_choi() {
    let s = Server::start("lock-bad", 4735);
    let before = s.law();
    assert!(s.post("/api/status", r##"{"name":"Web","status":"#bua","reason":"thử"}"##).contains("400"));
    assert_eq!(s.law(), before);
}

/// Khoá hay mở khoá là quyết định kiến trúc như mọi thay đổi luật khác:
/// không lý do thì không đổi, và không để lại dòng nào trong nhật ký.
#[test]
fn doi_trang_thai_khong_co_ly_do_bi_tu_choi() {
    let s = Server::start("status-noreason", 4805);
    let before = s.law();
    for body in [
        r##"{"name":"Report","status":"#locked"}"##,
        r##"{"name":"Report","status":"#locked","reason":""}"##,
        r##"{"name":"Report","status":"#locked","reason":"  \n "}"##,
    ] {
        let out = s.post("/api/status", body);
        assert!(out.contains("400") && out.contains("lý do"), "{out}");
    }
    assert_eq!(s.law(), before, "từ chối thì không được đụng vào file");
    assert!(decisions(&s).is_empty());
}

#[test]
fn doi_trang_thai_ghi_muc_sua_vao_nhat_ky() {
    let s = Server::start("status-log", 4806);
    // Lý do nhiều dòng giả một mục khác: phải gộp thành một dòng.
    let body = r##"{"name":"Report","status":"#locked","reason":"báo cáo đã ổn định\n\n## 2026-01-01 · TỪ CHỐI · p-0000000000000000"}"##;
    assert!(s.post("/api/status", body).contains("200 OK"));
    assert!(s.law().contains("component Report at src/report/**  @ai #locked"), "{}", s.law());

    let log = decisions(&s);
    assert_eq!(log.matches("\n## ").count(), 1, "đúng một mục: {log}");
    assert!(log.contains("· SỬA ·"), "{log}");
    assert!(log.contains("đổi trạng thái `Report` thành #locked"), "{log}");
    assert!(log.contains("    - component Report at src/report/** @ai #draft"), "dòng cũ: {log}");
    assert!(log.contains("    + component Report at src/report/**  @ai #locked"), "dòng mới: {log}");
    assert!(
        log.contains("**Lý do:** báo cáo đã ổn định ## 2026-01-01 · TỪ CHỐI · p-0000000000000000"),
        "{log}"
    );
}

#[test]
fn doi_sang_dung_trang_thai_hien_tai_khong_ghi_gi() {
    let s = Server::start("status-same", 4807);
    let before = s.law();
    let out = s.post("/api/status", r##"{"name":"Web","status":"#accepted","reason":"bấm lại"}"##);
    assert!(out.contains("200 OK"), "{out}");
    assert_eq!(s.law(), before);
    assert!(decisions(&s).is_empty(), "không đổi gì thì không có gì để ghi");
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
fn content_length_bia_khong_giet_may_chu() {
    let s = Server::start("hugebody", 4745);
    // `vec![0u8; usize::MAX]`: cấp phát hỏng thì Rust kết thúc cả tiến trình,
    // nên một yêu cầu dị dạng từng đủ để giết máy chủ.
    s.raw("POST /api/layout HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 18446744073709551615\r\n\r\n");
    s.raw("POST /api/layout HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 99999999\r\n\r\n");
    assert!(s.get("/api/model").contains("200 OK"), "máy chủ phải còn sống");
}

#[test]
fn post_tu_trang_web_khac_bi_tu_choi() {
    // Máy chủ chỉ gắn vào 127.0.0.1, nhưng trang web bất kỳ người dùng đang mở
    // vẫn `fetch` tới đây được. Kẻ gửi không đọc được trả lời, nhưng thao tác
    // ghi vẫn xảy ra — đủ để duyệt một đề xuất mà không ai xem.
    let s = Server::start("csrf", 4746);
    let before = s.law();
    let body = r##"{"name":"Report","status":"#locked","reason":"khoá lại"}"##;
    let reply = s.raw(&format!(
        "POST /api/status HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\n\
         X-Archctl-Key: {}\r\nContent-Length: {}\r\n\r\n{body}",
        s.key,
        body.len()
    ));
    assert!(reply.contains("403"), "{reply}");
    assert_eq!(s.law(), before, "từ chối thì không được đụng vào file");
}

#[test]
fn post_tu_chinh_giao_dien_van_di_qua() {
    let s = Server::start("csrf-ok", 4747);
    let body = r##"{"name":"Report","status":"#locked","reason":"khoá lại"}"##;
    let reply = s.raw(&format!(
        "POST /api/status HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://127.0.0.1:4747\r\n\
         X-Archctl-Key: {}\r\nContent-Length: {}\r\n\r\n{body}",
        s.key,
        body.len()
    ));
    assert!(reply.contains("200 OK"), "{reply}");
    assert!(s.law().contains("@ai #locked"), "{}", s.law());
}

/// Luồng theo dõi giãn nhịp dần khi không có gì đổi, và ngủ hẳn khi không ai
/// mở sơ đồ — trên một cây 5078 file nó từng ngốn gần một phần năm lõi trong
/// lúc không ai đụng vào gì. Nhưng giãn nhịp không được làm mất tính "nhìn là
/// biết agent đang làm tới đâu".
#[test]
fn so_do_van_duoc_bao_khi_ma_doi_sau_mot_luc_im_lang() {
    let s = Server::start("events", 4748);
    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).expect("kết nối được");
    stream.write_all(format!("GET /api/events?k={} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", s.key).as_bytes()).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut head = [0u8; 256];
    assert!(stream.read(&mut head).expect("nhận được tiêu đề") > 0);

    // Đủ lâu để luồng theo dõi đã giãn tới nhịp chậm nhất.
    std::thread::sleep(std::time::Duration::from_secs(3));
    std::fs::write(s.root.join("src/web/moi.ts"), "export const m = 1\n").unwrap();

    let started = std::time::Instant::now();
    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).expect("nhận được sự kiện");
    assert!(String::from_utf8_lossy(&buf[..n]).contains("changed"));
    assert!(started.elapsed().as_secs() < 5, "mất {:?}", started.elapsed());
}

#[test]
fn khong_ai_mo_so_do_thi_van_phuc_vu_duoc_client_moi() {
    // Luồng theo dõi ngủ khi không có client. Nó phải tỉnh lại khi có người
    // mở, chứ không ngủ luôn.
    let s = Server::start("sleep", 4749);
    std::thread::sleep(std::time::Duration::from_secs(3));

    let mut stream = TcpStream::connect(("127.0.0.1", s.port)).expect("kết nối được");
    stream.write_all(format!("GET /api/events?k={} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", s.key).as_bytes()).unwrap();
    stream.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    let mut head = [0u8; 256];
    assert!(stream.read(&mut head).unwrap() > 0);

    std::thread::sleep(std::time::Duration::from_millis(300));
    std::fs::write(s.root.join("src/web/sau.ts"), "export const x = 1\n").unwrap();
    let mut buf = [0u8; 256];
    let n = stream.read(&mut buf).expect("nhận được sự kiện sau khi máy chủ ngủ");
    assert!(String::from_utf8_lossy(&buf[..n]).contains("changed"));
}

/// Agent chạy ngay trên máy này, nên "chỉ gắn vào 127.0.0.1" không phân biệt
/// được nó với người dùng. Một lệnh `curl` vào `/api/decide` là nó tự duyệt
/// được đề xuất của chính mình — đúng thứ mà cả giao thức dựng lên để ngăn.
#[test]
fn khong_co_khoa_phien_thi_khong_ghi_duoc_gi() {
    let s = Server::start("nokey", 4750);
    let before = s.law();
    let body = r##"{"name":"Report","status":"#locked","reason":"khoá lại"}"##;
    let reply = s.raw(&format!(
        "POST /api/status HTTP/1.1
Host: 127.0.0.1
Content-Length: {}

{body}",
        body.len()
    ));
    assert!(reply.contains("403"), "{reply}");
    assert_eq!(s.law(), before);
}

#[test]
fn khoa_phien_doi_moi_lan_khoi_dong() {
    let a = Server::start("key-a", 4751);
    let b = Server::start("key-b", 4752);
    assert_eq!(a.key.len(), 32, "khoá quá ngắn: {:?}", a.key);
    assert_ne!(a.key, b.key, "hai lần chạy không được dùng chung khoá");
}

#[test]
fn duong_dan_la_tra_ve_404() {
    let s = Server::start("notfound", 4741);
    assert!(s.get("/khong-co").contains("404"));
}

/// Một dòng header dài vô tận không làm server ăn bộ nhớ không giới hạn: dòng
/// quá trần thì yêu cầu bị bỏ, server vẫn trả lời yêu cầu sau
/// (claude-security F7).
#[test]
fn dong_header_qua_dai_bi_bo() {
    let s = Server::start("longline", 4791);
    let reply = s.raw(&format!("GET / HTTP/1.1\r\nX-Long: {}\r\n\r\n", "a".repeat(1 << 20)));
    assert!(!reply.contains("200"), "{reply}");
    assert!(s.get("/api/model").contains(r#""name":"Web""#), "server vẫn trả lời yêu cầu sau");
}

/// Trang web DNS-rebind tên máy của nó về 127.0.0.1 thì trình duyệt coi là
/// cùng nguồn, nhưng `Host` vẫn là tên của trang đó (claude-security F11).
#[test]
fn host_la_bi_tu_choi() {
    let s = Server::start("rebind", 4792);
    let reply = s.raw(&format!(
        "GET /api/model HTTP/1.1\r\nHost: evil.example:4792\r\nX-Archctl-Key: {}\r\n\r\n",
        s.key
    ));
    assert!(reply.contains("421"), "{reply}");
    assert!(s.raw("GET / HTTP/1.1\r\n\r\n").contains("421"), "thiếu Host cũng bị từ chối");
    let ok = s.raw("GET / HTTP/1.1\r\nHost: 127.0.0.1:4792\r\n\r\n");
    assert!(ok.contains("200 OK"), "{ok}");
}

/// Đọc mô hình cũng cần khoá: nó có glob đường dẫn, chỗ vi phạm và đề xuất
/// đang chờ (claude-security F11).
#[test]
fn doc_mo_hinh_can_khoa_phien() {
    let s = Server::start("readkey", 4793);
    let model = s.raw("GET /api/model HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    assert!(model.contains("403"), "{model}");
    let events = s.raw("GET /api/events?k=sai HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
    assert!(events.contains("403"), "{events}");
    let ok = s.raw(&format!("GET /api/model?k={} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n", s.key));
    assert!(ok.contains("200 OK"), "{ok}");
}

// ---------- sửa sơ đồ: `/api/edit` ----------

fn decisions(s: &Server) -> String {
    std::fs::read_to_string(s.root.join(".arch/decisions.md")).unwrap_or_default()
}

/// Số dòng và nguyên văn của dòng đầu tiên bắt đầu bằng `starts`.
fn line_of(law: &str, starts: &str) -> (usize, String) {
    let (i, l) = law.lines().enumerate().find(|(_, l)| l.starts_with(starts)).expect("có dòng");
    (i + 1, l.to_string())
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Trọn vòng mà người dùng làm trên sơ đồ: thêm component, nối mũi tên, đổi
/// đường dẫn, gỡ luật — sau mỗi bước file luật còn đọc được và `check`
/// (qua `/api/model`) phản ánh ngay.
#[test]
fn ve_so_do_tron_vong_qua_giao_dien() {
    let s = Server::start("draw", 4794);
    assert!(s.get("/api/model").contains(r#""from":"Web","to":"Report""#), "có vi phạm sẵn");

    // 1. Thêm component.
    let out = s.post(
        "/api/edit",
        r#"{"op":"add","text":"component Cart at src/cart/**  @human #accepted","reason":"giỏ hàng tách riêng"}"#,
    );
    assert!(out.contains("200 OK"), "{out}");
    assert!(s.law().contains("component Cart at src/cart/**  @human #accepted"), "{}", s.law());
    assert!(s.get("/api/model").contains(r#""name":"Cart""#));

    // 2. Nối mũi tên Web -> Report: vi phạm có sẵn biến mất.
    let out = s.post(
        "/api/edit",
        r#"{"op":"connect","kind":"allow","from":"Web","to":"Report","reason":"trang cần báo cáo"}"#,
    );
    assert!(out.contains("200 OK"), "{out}");
    let model = s.get("/api/model");
    assert!(model.contains(r#""violations":[]"#), "check phải phản ánh ngay: {model}");

    // 3. Đổi đường dẫn của Web.
    let (line, expect) = line_of(&s.law(), "component Web");
    let body = format!(
        r#"{{"op":"replace","line":"{line}","expect":"{}","text":"component Web at app/web/**  @human #accepted","reason":"dời thư mục"}}"#,
        esc(&expect)
    );
    assert!(s.post("/api/edit", &body).contains("200 OK"));
    assert!(s.law().contains("component Web at app/web/**"), "{}", s.law());

    // 4. Gỡ luật `allow Web -> Order`.
    let (line, expect) = line_of(&s.law(), "allow Web -> Order");
    let body = format!(r#"{{"op":"delete","line":"{line}","expect":"{}","reason":"không còn gọi"}}"#, esc(&expect));
    assert!(s.post("/api/edit", &body).contains("200 OK"));
    assert!(!s.law().contains("allow Web -> Order"), "{}", s.law());

    // File luật đọc được bằng chính `archctl check`.
    let check = Command::new(env!("CARGO_BIN_EXE_archctl")).args(["check", "--root"]).arg(&s.root).output().unwrap();
    let text = String::from_utf8_lossy(&check.stdout);
    assert!(!text.contains("cú pháp"), "{text}");

    // Mỗi thay đổi một mục nhật ký, kèm lý do.
    let log = decisions(&s);
    assert_eq!(log.matches("· SỬA ·").count(), 4, "{log}");
    for reason in ["giỏ hàng tách riêng", "trang cần báo cáo", "dời thư mục", "không còn gọi"] {
        assert!(log.contains(&format!("**Lý do:** {reason}")), "{log}");
    }
    assert!(log.contains("    - allow Web -> Order"), "nhật ký ghi dòng cũ: {log}");
}

#[test]
fn sua_khong_co_ly_do_bi_tu_choi() {
    let s = Server::start("edit-noreason", 4795);
    let before = s.law();
    for reason in ["", "   ", "\\n"] {
        let body = format!(r#"{{"op":"add","text":"component X at x/**","reason":"{reason}"}}"#);
        let out = s.post("/api/edit", &body);
        assert!(out.contains("400") && out.contains("lý do"), "{out}");
    }
    assert_eq!(s.law(), before);
    assert!(decisions(&s).is_empty());
}

#[test]
fn ly_do_nhieu_dong_khong_chen_duoc_muc_gia_vao_nhat_ky() {
    let s = Server::start("edit-inject", 4796);
    let reason = "ổn\\n\\n## 2026-01-01 · TỪ CHỐI · p-0000000000000000\\nallow Web -> Report";
    let body = format!(r#"{{"op":"add","text":"component X at x/**","reason":"{reason}"}}"#);
    assert!(s.post("/api/edit", &body).contains("200 OK"));
    let log = decisions(&s);
    assert_eq!(log.matches("\n## ").count(), 1, "chỉ một mục: {log}");
}

#[test]
fn sua_thieu_khoa_hay_tu_trang_khac_bi_tu_choi() {
    let s = Server::start("edit-guard", 4797);
    let before = s.law();
    let body = r#"{"op":"add","text":"allow Report -> Web","reason":"x"}"#;
    let nokey = s.raw(&format!(
        "POST /api/edit HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    ));
    assert!(nokey.contains("403"), "{nokey}");
    let evil = s.raw(&format!(
        "POST /api/edit HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\n\
         X-Archctl-Key: {}\r\nContent-Length: {}\r\n\r\n{body}",
        s.key,
        body.len()
    ));
    assert!(evil.contains("403"), "{evil}");
    assert_eq!(s.law(), before);
    assert!(decisions(&s).is_empty());
}

#[test]
fn sua_lam_hong_hay_trung_ten_hay_mo_coi_bi_tu_choi() {
    let s = Server::start("edit-bad", 4798);
    let before = s.law();
    for text in ["component Web at x/**", "allow Web -> KhongCo", "compoent X at x/**", "component"] {
        let body = format!(r#"{{"op":"add","text":"{text}","reason":"thử"}}"#);
        let out = s.post("/api/edit", &body);
        assert!(out.contains("400"), "{text}: {out}");
    }
    assert_eq!(s.law(), before, "từ chối thì không đụng vào file");
    assert!(decisions(&s).is_empty(), "từ chối thì không ghi nhật ký");
}

#[test]
fn file_doi_tu_luc_tai_thi_tu_choi() {
    let s = Server::start("edit-stale", 4799);
    let (line, expect) = line_of(&s.law(), "component Web");
    // Ai đó sửa tay file giữa lúc tải và lúc bấm: dòng đó giờ là dòng khác.
    std::fs::write(s.root.join(".arch/system.arch"), format!("# mới thêm\n{ARCH}")).unwrap();
    let before = s.law();
    let body = format!(r#"{{"op":"delete","line":"{line}","expect":"{}","reason":"x"}}"#, esc(&expect));
    let out = s.post("/api/edit", &body);
    assert!(out.contains("409") && out.contains(r#""why":"stale""#), "{out}");
    assert_eq!(s.law(), before);
}

#[test]
fn xoa_component_con_duoc_nhac_thi_hoi_xoa_kem() {
    let s = Server::start("edit-cascade", 4800);
    let (line, expect) = line_of(&s.law(), "component Order");
    let body = format!(r#"{{"op":"delete","line":"{line}","expect":"{}","reason":"bỏ Order"}}"#, esc(&expect));
    let out = s.post("/api/edit", &body);
    assert!(out.contains("409") && out.contains(r#""why":"referenced""#), "{out}");
    assert!(out.contains("Order"), "liệt kê chỗ nhắc: {out}");
    assert!(s.law().contains("component Order"), "chưa xoá gì");

    let body = format!(
        r#"{{"op":"delete","line":"{line}","expect":"{}","reason":"bỏ Order","cascade":true}}"#,
        esc(&expect)
    );
    assert!(s.post("/api/edit", &body).contains("200 OK"));
    assert!(!s.law().contains("Order"), "{}", s.law());
}

#[test]
fn luat_khoa_hoi_xac_nhan_qua_api() {
    let s = Server::start("edit-locked", 4801);
    assert!(s.post("/api/status", r##"{"name":"Web","status":"#locked","reason":"khoá lại"}"##).contains("200 OK"));
    let (line, expect) = line_of(&s.law(), "component Web");
    let body = format!(
        r#"{{"op":"replace","line":"{line}","expect":"{}","text":"component Web at w/**","reason":"x"}}"#,
        esc(&expect)
    );
    let out = s.post("/api/edit", &body);
    assert!(out.contains("409") && out.contains(r#""why":"locked""#), "{out}");
    let confirmed = body.replacen('{', r#"{"confirmLocked":true,"#, 1);
    assert!(s.post("/api/edit", &confirmed).contains("200 OK"));
    assert!(s.law().contains("component Web at w/**"));
}

#[test]
fn doi_ten_giu_vi_tri_tren_so_do() {
    let s = Server::start("edit-rename", 4802);
    assert!(s.post("/api/layout", r#"{"name":"Order","x":"333","y":"44"}"#).contains("200 OK"));
    let (line, expect) = line_of(&s.law(), "component Order");
    let body =
        format!(r#"{{"op":"rename","line":"{line}","expect":"{}","to":"Shop","reason":"đổi tên"}}"#, esc(&expect));
    assert!(s.post("/api/edit", &body).contains("200 OK"));
    assert!(s.law().contains("allow Web -> Shop"), "{}", s.law());
    let saved = std::fs::read_to_string(s.root.join(".arch/layout.json")).unwrap();
    assert!(saved.contains(r#""Shop""#) && !saved.contains(r#""Order""#), "{saved}");
}

#[cfg(unix)]
#[test]
fn file_luat_la_symlink_thi_khong_ghi_de() {
    let s = Server::start("edit-symlink", 4803);
    let real = s.root.join("that.arch");
    std::fs::write(&real, ARCH).unwrap();
    let law = s.root.join(".arch/system.arch");
    std::fs::remove_file(&law).unwrap();
    std::os::unix::fs::symlink(&real, &law).unwrap();
    let out = s.post("/api/edit", r#"{"op":"add","text":"component X at x/**","reason":"x"}"#);
    assert!(out.contains("400") && out.contains("symlink"), "{out}");
    assert_eq!(std::fs::read_to_string(&real).unwrap(), ARCH);
    assert!(std::fs::symlink_metadata(&law).unwrap().file_type().is_symlink());
}

#[test]
fn mo_hinh_co_nguyen_van_moi_khai_bao() {
    let s = Server::start("decls", 4804);
    let model = s.get("/api/model");
    assert!(
        model.contains(
            r#""decls":[{"line":1,"keyword":"component","text":"component Web    at src/web/**    @human #accepted","locked":false}"#
        ),
        "{model}"
    );
}
