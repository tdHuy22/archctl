//! `archctl` trong vòng lặp của agent: hook PostToolUse của Claude Code, bộ
//! đếm lần gặp lại, và bản JSON.
//!
//! Tái hiện từ GĐ3 của agent-stack (14/11/2026): hook do `init` sinh ra chạy
//! `archctl check --changed`, có vi phạm thì thoát mã 1. Claude Code coi mã 1
//! là lỗi không chặn, nên model không bao giờ thấy vi phạm. Hook phải đưa vi
//! phạm qua `hookSpecificOutput.additionalContext` và luôn thoát 0.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

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
        let root = std::env::temp_dir().join(format!("archctl-hook-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let p = Project(root.canonicalize().unwrap_or(root));
        p.write(".arch/system.arch", ARCH);
        p.write("src/report/r.ts", "export const r = 1\n");
        p.write("src/order/o.ts", "export const o = 1\n");
        p.write("src/web/page.ts", "import { o } from '../order/o'\n");
        p
    }

    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        // Thư mục tạm có thể nằm sau symlink (`/var` → `/private/var`).
        let _ = std::fs::canonicalize(&self.0);
    }

    fn path(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().into_owned()
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_archctl"))
            .args(args)
            .arg("--root")
            .arg(&self.0)
            .current_dir(&self.0)
            .output()
            .expect("chạy được archctl")
    }

    /// `archctl check --hook` như Claude Code gọi: chạy ở thư mục project,
    /// payload qua stdin.
    fn hook(&self, payload: &[u8]) -> Output {
        hook_in(&self.0, payload)
    }

    fn edit_payload(&self, tool: &str, rel: &str) -> Vec<u8> {
        format!(
            r#"{{"session_id":"s1","transcript_path":"/tmp/t.jsonl","cwd":{},"hook_event_name":"PostToolUse","tool_name":{},"tool_input":{{"file_path":{},"old_string":"a","new_string":"b"}},"tool_response":{{"success":true}}}}"#,
            arch_core::json::string(&self.0.to_string_lossy()),
            arch_core::json::string(tool),
            arch_core::json::string(&self.path(rel)),
        )
        .into_bytes()
    }

    fn git(&self, args: &[&str]) {
        let ok = Command::new("git")
            .args(args)
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("có git")
            .success();
        assert!(ok, "git {args:?}");
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hook_in(dir: &std::path::Path, payload: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_archctl"))
        .args(["check", "--hook"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("chạy được archctl");
    let mut stdin = child.stdin.take().unwrap();
    let payload = payload.to_vec();
    // Ghi ở luồng riêng: archctl có thể thôi đọc giữa chừng (payload quá cỡ).
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&payload);
    });
    let out = child.wait_with_output().unwrap();
    let _ = writer.join();
    out
}

fn text(out: &Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

/// `additionalContext` trong stdout của hook, sau khi kiểm hình dạng JSON mà
/// Claude Code đòi. Không in gì thì `None`.
fn context(out: &Output) -> Option<String> {
    let stdout = String::from_utf8(out.stdout.clone()).expect("stdout là UTF-8");
    if stdout.trim().is_empty() {
        return None;
    }
    let v = arch_core::jsonc::parse(&stdout).unwrap_or_else(|| panic!("stdout không phải JSON: {stdout}"));
    let event = v.get_path(&["hookSpecificOutput", "hookEventName"]).and_then(|x| x.as_str());
    assert_eq!(event, Some("PostToolUse"), "{stdout}");
    Some(
        v.get_path(&["hookSpecificOutput", "additionalContext"])
            .and_then(|x| x.as_str())
            .unwrap_or_else(|| panic!("thiếu additionalContext: {stdout}"))
            .to_string(),
    )
}

// ---- 1. Đầu ra cho hook ----

#[test]
fn hook_dua_vi_pham_toi_model_qua_additional_context() {
    let p = Project::new("ctx");
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let out = p.hook(&p.edit_payload("Edit", "src/web/page.ts"));
    // Mã 1 là "lỗi không chặn": Claude Code chỉ ghi vào transcript, model không thấy.
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(out.stderr.is_empty(), "{}", text(&out));
    let ctx = context(&out).expect("có vi phạm thì phải có additionalContext");
    assert!(ctx.contains("✗ VI PHẠM  Web -> Report"), "{ctx}");
    assert!(ctx.contains("src/web/page.ts:1"), "{ctx}");
    // Nói rõ đây là dữ liệu, không phải chỉ dẫn (TM-88).
    assert!(ctx.contains("dữ liệu"), "{ctx}");
}

#[test]
fn hook_sach_thi_khong_in_gi() {
    let p = Project::new("clean");
    let out = p.hook(&p.edit_payload("Write", "src/web/page.ts"));
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{}", text(&out));
}

#[test]
fn hook_multiedit_cung_duoc_kiem() {
    let p = Project::new("multi");
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let ctx = context(&p.hook(&p.edit_payload("MultiEdit", "src/web/page.ts")));
    assert!(ctx.is_some_and(|c| c.contains("Web -> Report")));
}

/// Bash không có `file_path`: kiểm những file đổi so với HEAD, như `--changed`.
#[test]
fn hook_sau_lenh_bash_kiem_file_vua_doi() {
    let p = Project::new("bash");
    p.git(&["init", "-q"]);
    p.git(&["add", "-A"]);
    p.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let payload = format!(
        r#"{{"session_id":"s","cwd":{},"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{{"command":"sed -i x"}},"tool_response":{{}}}}"#,
        arch_core::json::string(&p.0.to_string_lossy())
    );
    let out = p.hook(payload.as_bytes());
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(context(&out).is_some_and(|c| c.contains("Web -> Report")), "{}", text(&out));
}

/// Lỗi của hook không bao giờ được làm kẹt phiên (TM-86, TM-89): mọi đường
/// đều mã 0, stderr rỗng, stdout rỗng hoặc đúng một JSON.
#[test]
fn hook_loi_khong_bao_gio_lam_ket_phien() {
    let p = Project::new("errors");
    for payload in [&b""[..], b"khong phai json", b"{\"tool_input\": 5}", b"[[[[[[", b"\xff\xfe"] {
        let out = p.hook(payload);
        assert_eq!(code(&out), 0, "{payload:?}: {}", text(&out));
        assert!(out.stderr.is_empty(), "{payload:?}: {}", text(&out));
        let _ = context(&out);
    }
    // Payload quá cỡ: thôi đọc, im lặng.
    let mut big = p.edit_payload("Edit", "src/web/page.ts");
    big.extend(std::iter::repeat_n(b' ', 17 << 20));
    let out = p.hook(&big);
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());

    // File luật hỏng: báo cho model, vẫn mã 0.
    p.write(".arch/system.arch", "component ??? at\n");
    let out = p.hook(&p.edit_payload("Edit", "src/web/page.ts"));
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(context(&out).is_some_and(|c| c.contains("system.arch")), "{}", text(&out));

    // Thư mục không có `.arch`: im lặng.
    let bare = std::env::temp_dir().join(format!("archctl-hook-bare-{}", std::process::id()));
    std::fs::create_dir_all(&bare).unwrap();
    let out = hook_in(&bare, br#"{"cwd":"/","tool_name":"Edit","tool_input":{"file_path":"/etc/hosts"}}"#);
    let _ = std::fs::remove_dir_all(&bare);
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{}", text(&out));
}

/// File ngoài gốc project không được kiểm (TM-87).
#[test]
fn hook_bo_qua_file_ngoai_goc() {
    let p = Project::new("outside");
    let other = Project::new("outside-other");
    other.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let mut payload = p.edit_payload("Edit", "src/web/page.ts");
    let needle = p.path("src/web/page.ts");
    let s = String::from_utf8(payload).unwrap().replace(&needle, &other.path("src/web/page.ts"));
    payload = s.into_bytes();
    let out = p.hook(&payload);
    assert_eq!(code(&out), 0, "{}", text(&out));
    assert!(out.stdout.is_empty(), "{}", text(&out));
}

/// Chữ lấy từ repo đi thẳng vào context của model: ký tự đảo chiều, mã thoát
/// terminal không được lọt qua (TM-88).
#[test]
fn hook_loc_ky_tu_an_trong_context() {
    let p = Project::new("bidi");
    p.write("src/report/r\u{202e}x.ts", "export const r = 1\n");
    p.write("src/web/page.ts", "import { r } from '../report/r\u{202e}x'\n");
    let ctx = context(&p.hook(&p.edit_payload("Edit", "src/web/page.ts"))).expect("có vi phạm");
    assert!(!ctx.contains('\u{202e}'), "{ctx:?}");
    assert!(!ctx.chars().any(|c| c.is_control() && c != '\n' && c != '\t'), "{ctx:?}");
}

// ---- 2. Matcher của hook do `init` sinh ----

#[test]
fn init_sinh_hook_bat_ca_multiedit_va_bash() {
    let p = Project::new("init");
    std::fs::remove_dir_all(p.0.join(".arch")).unwrap();
    assert_eq!(code(&p.run(&["init"])), 0);
    let settings = std::fs::read_to_string(p.0.join(".claude/settings.json")).unwrap();
    let v = arch_core::jsonc::parse(&settings).expect("settings là JSON");
    let post = v.get_path(&["hooks", "PostToolUse"]).and_then(|x| x.as_array()).expect("có PostToolUse");
    let matcher = post[0].get("matcher").and_then(|x| x.as_str()).unwrap();
    let mut tools: Vec<&str> = matcher.split('|').collect();
    tools.sort_unstable();
    assert_eq!(tools, ["Bash", "Edit", "MultiEdit", "Write"], "{settings}");
    let cmd = post[0].get_path(&["hooks"]).and_then(|x| x.as_array()).unwrap()[0]
        .get("command")
        .and_then(|x| x.as_str())
        .unwrap();
    assert_eq!(cmd, "archctl check --hook", "{settings}");
}

// ---- 4. Bộ đếm không mất khi kiểm một file ----

/// Đúng cách agent-stack gọi: `archctl check <file>` sau mỗi lần sửa. Agent
/// sửa xen kẽ hai file đều vi phạm; bộ đếm của file kia không được về 0, nếu
/// không agent không bao giờ tới mốc "đã gặp lại N lần" để đề xuất hay báo
/// bế tắc.
#[test]
fn check_mot_file_khong_lam_mat_bo_dem_file_khac() {
    let p = Project::new("counter");
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    p.write("src/web/list.ts", "import { r } from '../report/r'\n");
    let a = p.path("src/web/page.ts");
    let b = p.path("src/web/list.ts");
    p.run(&["check", &a]);
    p.run(&["check", &a]);
    p.run(&["check", &b]);
    let said = text(&p.run(&["check", &a]));
    assert!(said.contains("đã gặp lại 3 lần"), "{said}");

    // Kiểm cả repo sau đó vẫn thấy đúng số của từng file.
    let said = text(&p.run(&["check"]));
    assert!(said.contains("đã gặp lại 4 lần") && said.contains("(lần thứ 2)"), "{said}");

    // Sửa xong file vừa kiểm thì bộ đếm của nó về 0, của file kia giữ nguyên.
    p.write("src/web/page.ts", "import { o } from '../order/o'\n");
    p.run(&["check", &a]);
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let said = text(&p.run(&["check", &a]));
    assert!(!said.contains("lần thứ"), "{said}");
    let said = text(&p.run(&["check", &b]));
    assert!(said.contains("đã gặp lại 3 lần"), "{said}");
}

// ---- 5. JSON có baseline và số đề xuất đang chờ ----

#[test]
fn json_co_baseline_va_de_xuat_dang_cho() {
    let p = Project::new("json");
    p.write("src/web/old.ts", "import { r } from '../report/r'\n");
    assert_eq!(code(&p.run(&["baseline"])), 0);
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let out = p.run(&["propose", "allow Order -> Report", "--reason", "cần số liệu"]);
    assert_eq!(code(&out), 3, "{}", text(&out));
    p.run(&["check"]);

    let out = p.run(&["check", "--json"]);
    assert_eq!(code(&out), 1, "{}", text(&out));
    let s = String::from_utf8(out.stdout).unwrap();
    let v = arch_core::jsonc::parse(&s).unwrap_or_else(|| panic!("không phải JSON: {s}"));
    assert!(s.contains(r#""fresh":1"#), "{s}");
    assert!(s.contains(r#""pending":1"#), "{s}");
    assert!(s.contains(r#""baselineStale":0"#), "{s}");
    assert!(v.get("previousHash").and_then(|x| x.as_str()).is_some_and(|h| h.len() == 32), "{s}");
    let vs = v.get("violations").and_then(|x| x.as_array()).unwrap();
    assert_eq!(vs.len(), 2, "{s}");
    for item in vs {
        let file = item.get("file").and_then(|x| x.as_str()).unwrap();
        let baselined = file.ends_with("old.ts");
        assert_eq!(item.get("baselined"), Some(&arch_core::jsonc::Value::Bool(baselined)), "{item:?}");
        // Lần check thứ hai cho cả hai vi phạm.
        assert_eq!(item.get("attempts").and_then(|x| x.as_str()), Some("2"), "{item:?}");
    }
}

// ---- Định dạng agent-stack đang đọc không đổi ----

/// `agent-stack/plugin/lib/archgate.py` đọc chữ người đọc: dòng mở đầu bằng
/// `✗`, dòng thụt lề theo sau, dòng tổng `N vi phạm mới`, bộ đếm
/// `(lần thứ N)` / `đã gặp lại N lần`, và mã thoát 0/1/2.
#[test]
fn dinh_dang_agent_stack_dang_doc_khong_doi() {
    let p = Project::new("format");
    let out = p.run(&["check", &p.path("src/web/page.ts")]);
    assert_eq!(code(&out), 0, "{}", text(&out));
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let f = p.path("src/web/page.ts");
    let out = p.run(&["check", &f]);
    assert_eq!(code(&out), 1, "{}", text(&out));
    let said = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = said.lines().collect();
    let at = lines.iter().position(|l| l.starts_with("✗ VI PHẠM  Web -> Report")).expect(&said);
    assert!(lines[at + 1].starts_with("  src/web/page.ts:1  import '../report/r'"), "{said}");
    assert!(lines.iter().any(|l| l.starts_with("1 vi phạm mới")), "{said}");
    assert!(text(&p.run(&["check", &f])).contains("(lần thứ 2)"));
    p.run(&["check", &f]);
    p.run(&["check", &f]);
    assert!(text(&p.run(&["check", &f])).contains("! đã gặp lại 5 lần"));
    p.write(".arch/system.arch", "component ??? at\n");
    assert_eq!(code(&p.run(&["check", &f])), 2);
}

impl Project {
    fn bash_payload(&self) -> Vec<u8> {
        format!(
            r#"{{"session_id":"s","cwd":{},"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{{"command":"ls"}},"tool_response":{{}}}}"#,
            arch_core::json::string(&self.0.to_string_lossy())
        )
        .into_bytes()
    }

    fn commit_all(&self) {
        self.git(&["init", "-q"]);
        self.git(&["add", "-A"]);
        self.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    }
}

/// Bash có trong matcher, nên hook chạy sau **mọi** lệnh shell. Lệnh không
/// đổi file nào (`ls`, chạy test) không được báo lại vi phạm cũ, cũng không
/// được đếm thành một lần "gặp lại": nếu không, agent chạy test ba lần là bị
/// bảo dừng lại và đề xuất đổi luật.
#[test]
fn hook_sau_bash_khong_doi_file_thi_im_lang_va_khong_dem() {
    let p = Project::new("bash-quiet");
    p.commit_all();
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    assert!(context(&p.hook(&p.bash_payload())).is_some(), "lần đầu phải báo");
    for _ in 0..3 {
        let out = p.hook(&p.bash_payload());
        assert_eq!(code(&out), 0);
        assert!(out.stdout.is_empty(), "không đổi gì mà báo lại: {}", text(&out));
    }
    // Chỉ một lần được đếm.
    let said = text(&p.run(&["check", &p.path("src/web/page.ts")]));
    assert!(said.contains("(lần thứ 2)"), "{said}");

    // Lệnh shell sửa file: kiểm lại.
    std::thread::sleep(std::time::Duration::from_millis(20));
    p.write("src/web/page.ts", "import { r } from '../report/r'\nexport const x = 1\n");
    assert!(context(&p.hook(&p.bash_payload())).is_some_and(|c| c.contains("Web -> Report")));
}

/// Edit vừa báo xong thì lệnh Bash ngay sau đó không báo lại cùng file.
#[test]
fn hook_edit_roi_bash_khong_bao_trung() {
    let p = Project::new("edit-bash");
    p.commit_all();
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    assert!(context(&p.hook(&p.edit_payload("Edit", "src/web/page.ts"))).is_some());
    let out = p.hook(&p.bash_payload());
    assert!(out.stdout.is_empty(), "{}", text(&out));
}

/// Không phải repo git thì không biết lệnh shell đã đổi gì: im lặng (hook
/// sau Edit/Write vẫn kiểm từng file).
#[test]
fn hook_bash_ngoai_git_thi_im_lang() {
    let p = Project::new("bash-nogit");
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let out = p.hook(&p.bash_payload());
    assert_eq!(code(&out), 0);
    assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{}", text(&out));
}

// ---- Các lỗi review tìm ra (14/11/2026) ----

/// Monorepo: `.arch/` ở `svc/`, git ở thư mục cha. `git status` in đường dẫn
/// tính từ gốc git (`svc/src/…`); trước bản sửa hook sau Bash im lặng.
#[test]
fn hook_bash_trong_monorepo_co_arch_o_thu_muc_con() {
    let outer = Project::new("mono");
    let svc = outer.0.join("svc");
    std::fs::create_dir_all(&svc).unwrap();
    for entry in [".arch", "src"] {
        std::fs::rename(outer.0.join(entry), svc.join(entry)).unwrap();
    }
    outer.commit_all();
    std::fs::write(svc.join("src/web/page.ts"), "import { r } from '../report/r'\n").unwrap();
    let payload = format!(
        r#"{{"cwd":{},"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{{"command":"sed"}}}}"#,
        arch_core::json::string(&svc.to_string_lossy())
    );
    let out = hook_in(&svc, payload.as_bytes());
    assert!(context(&out).is_some_and(|c| c.contains("Web -> Report")), "{}", text(&out));
}

/// Ngay sau `archctl init` file luật còn chưa commit. Hook sau Bash vẫn phải
/// kiểm file vừa đổi, không im lặng và không quét lại cả repo mỗi lệnh.
#[test]
fn hook_bash_khi_file_luat_chua_commit() {
    let p = Project::new("law-untracked");
    p.git(&["init", "-q"]);
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    assert!(context(&p.hook(&p.bash_payload())).is_some_and(|c| c.contains("Web -> Report")));
    assert!(p.hook(&p.bash_payload()).stdout.is_empty(), "lệnh thứ hai không đổi gì");
}

/// `git checkout --` hoàn tác file vi phạm: bộ đếm của nó về 0, nên thêm lại
/// đúng import đó là lần đầu, không phải "đã gặp lại 3 lần".
#[test]
fn hoan_tac_bang_git_thi_bo_dem_ve_khong() {
    let p = Project::new("revert");
    p.commit_all();
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    p.hook(&p.edit_payload("Edit", "src/web/page.ts"));
    p.hook(&p.edit_payload("Edit", "src/web/page.ts"));
    p.git(&["checkout", "--", "src/web/page.ts"]);
    assert!(p.hook(&p.bash_payload()).stdout.is_empty());
    p.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let ctx = context(&p.hook(&p.edit_payload("Edit", "src/web/page.ts"))).unwrap();
    assert!(!ctx.contains("(lần thứ") && !ctx.contains("! đã gặp lại"), "{ctx}");
}

/// TM-87: `cwd/../dự-án-khác/…` không lọt qua phép so chữ.
#[test]
fn hook_khong_kiem_du_an_khac_qua_cham_cham() {
    let p = Project::new("dotdot");
    let other = Project::new("dotdot-other");
    other.write("src/web/page.ts", "import { r } from '../report/r'\n");
    let name = other.0.file_name().unwrap().to_string_lossy().into_owned();
    let sneaky = format!("{}/../{name}/src/web/page.ts", p.0.display());
    let payload = format!(
        r#"{{"cwd":{},"tool_name":"Edit","tool_input":{{"file_path":{}}}}}"#,
        arch_core::json::string(&p.0.to_string_lossy()),
        arch_core::json::string(&sneaky)
    );
    let before = std::fs::read_to_string(other.0.join(".arch/.state")).unwrap_or_default();
    let out = p.hook(payload.as_bytes());
    assert!(out.stdout.is_empty(), "{}", text(&out));
    assert_eq!(std::fs::read_to_string(other.0.join(".arch/.state")).unwrap_or_default(), before);
}
