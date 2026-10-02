//! `archctl serve` — sơ đồ để người thao tác và quan sát.
//!
//! Đây là phần đặc tả gọi là **chiều 3**: người nhìn sơ đồ biến đổi theo lúc
//! agent làm việc, không cần đọc dòng mã nào. Và là chỗ duy nhất người **thao
//! tác** được: kéo thả, khoá luật bằng một cú bấm, duyệt đề xuất tại chỗ.
//!
//! Máy chủ HTTP viết tay trên `TcpListener`, chỉ gắn vào `127.0.0.1`. Không
//! khung web, không gói ngoài, không gọi mạng — mở được khi không có Internet,
//! và không có bề mặt tấn công nào ngoài máy.

use crate::exit;
use crate::proto::parse_args;
use arch_core::check::{Report, Via};
use arch_core::layout::{self, Point};
use arch_core::model::{Owner, Status};
use arch_core::protocol::{self, PROPOSALS_FILE};
use arch_core::{ARCH_FILE, Document, json, jsonc};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::{Arc, Mutex};

const UI: &str = include_str!("ui.html");
const LAYOUT_FILE: &str = ".arch/layout.json";
/// Khoảng dò thay đổi file. Đủ nhanh để thấy agent làm việc gần như tức thì,
/// đủ chậm để không tốn CPU khi ngồi yên.
const POLL_MS: u64 = 400;

type Clients = Arc<Mutex<Vec<TcpStream>>>;

pub fn cmd_serve(args: &[String]) -> ExitCode {
    let parsed = match parse_args(args, &["root", "port"], &["no-open"]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("archctl: {e}");
            return ExitCode::from(exit::USAGE);
        }
    };
    let start = parsed.flags.get("root").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let Some(root) = arch_core::find_root(&start) else {
        eprintln!("archctl: không tìm thấy `{ARCH_FILE}`");
        return ExitCode::from(exit::USAGE);
    };
    let port: u16 = parsed.flags.get("port").and_then(|p| p.parse().ok()).unwrap_or(4000);

    let listener = match TcpListener::bind(("127.0.0.1", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("archctl: không mở được cổng {port}: {e}");
            return ExitCode::from(exit::USAGE);
        }
    };

    let clients: Clients = Arc::new(Mutex::new(Vec::new()));
    spawn_watcher(root.clone(), Arc::clone(&clients));

    crate::say!("archctl serve · http://127.0.0.1:{port}");
    crate::say!("Sơ đồ tự cập nhật khi mã hoặc luật thay đổi. Ctrl-C để dừng.");

    for stream in listener.incoming().flatten() {
        let root = root.clone();
        let clients = Arc::clone(&clients);
        std::thread::spawn(move || handle(stream, &root, &clients));
    }
    ExitCode::from(exit::OK)
}

// ---------- HTTP ----------

struct Request {
    method: String,
    path: String,
    body: String,
}

fn read_request(stream: &TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut length = 0usize;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).ok()? == 0 || header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some(Request { method, path, body: String::from_utf8_lossy(&body).into_owned() })
}

fn respond(stream: &mut TcpStream, status: &str, content_type: &str, body: &str) {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
}

fn handle(mut stream: TcpStream, root: &Path, clients: &Clients) {
    let Some(req) = read_request(&stream) else { return };
    let path = req.path.split('?').next().unwrap_or("/");

    match (req.method.as_str(), path) {
        ("GET", "/") => respond(&mut stream, "200 OK", "text/html; charset=utf-8", UI),
        ("GET", "/api/model") => {
            respond(&mut stream, "200 OK", "application/json", &model_json(root));
        }
        ("GET", "/api/events") => {
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                        Cache-Control: no-store\r\nConnection: keep-alive\r\n\r\n";
            if stream.write_all(head.as_bytes()).is_ok() {
                if let Ok(mut open) = clients.lock() {
                    open.push(stream);
                }
            }
        }
        ("POST", _) => {
            let result = match path {
                "/api/layout" => save_position(root, &req.body),
                "/api/status" => set_status(root, &req.body),
                "/api/decide" => decide(root, &req.body),
                // Chưa có file thì coi như đã xoá xong.
                "/api/relayout" => match std::fs::remove_file(root.join(LAYOUT_FILE)) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                    _ => Ok(()),
                },
                _ => Err("không có đường dẫn này".into()),
            };
            match result {
                Ok(()) => respond(&mut stream, "200 OK", "application/json", r#"{"ok":true}"#),
                Err(e) => respond(
                    &mut stream,
                    "400 Bad Request",
                    "application/json",
                    &format!(r#"{{"ok":false,"error":{}}}"#, json::string(&e)),
                ),
            }
        }
        _ => respond(&mut stream, "404 Not Found", "text/plain; charset=utf-8", "không có"),
    }
}

// ---------- theo dõi thay đổi ----------

/// Dò dấu vân tay của cây file. Dùng cách dò thay vì gọi API theo dõi của hệ
/// điều hành để khỏi thêm dependency; với chu kỳ 400ms thì chi phí không đáng
/// kể, kể cả trên repo vài nghìn file.
fn spawn_watcher(root: PathBuf, clients: Clients) {
    std::thread::spawn(move || {
        let mut previous = fingerprint(&root);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(POLL_MS));
            let current = fingerprint(&root);
            if current == previous {
                continue;
            }
            previous = current;
            let Ok(mut open) = clients.lock() else { continue };
            // Client đã đóng thì ghi hỏng; bỏ nó ra khỏi danh sách.
            open.retain_mut(|c| c.write_all(b"data: changed\n\n").and_then(|()| c.flush()).is_ok());
        }
    });
}

fn fingerprint(root: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |n: u64| {
        h ^= n;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    let mut files = arch_core::check::source_files(root, &arch_core::read_ignores(root));
    files.extend([ARCH_FILE.into(), PROPOSALS_FILE.into(), LAYOUT_FILE.into()]);
    for file in &files {
        let Ok(meta) = std::fs::metadata(root.join(file)) else { continue };
        for b in file.bytes() {
            mix(u64::from(b));
        }
        mix(meta.len());
        if let Ok(time) = meta.modified()
            && let Ok(since) = time.duration_since(std::time::UNIX_EPOCH)
        {
            mix(since.as_millis() as u64);
        }
    }
    h
}

// ---------- mô hình gửi cho giao diện ----------

fn load(root: &Path) -> Option<(Document, Report, String)> {
    let src = std::fs::read_to_string(root.join(ARCH_FILE)).ok()?;
    let (doc, diags) = arch_core::load(&src);
    if arch_core::diag::has_errors(&diags) {
        return None;
    }
    let hash = arch_core::arch_hash(&doc);
    let ignores = arch_core::read_ignores(root);
    let report = arch_core::check::check(root, &doc, &ignores);
    Some((doc, report, hash))
}

fn model_json(root: &Path) -> String {
    let Some((doc, report, hash)) = load(root) else {
        let src = std::fs::read_to_string(root.join(ARCH_FILE)).unwrap_or_default();
        let messages: Vec<String> = arch_core::load(&src)
            .1
            .iter()
            .filter(|d| d.severity == arch_core::Severity::Error)
            .map(|d| format!("dòng {}: {}", d.line, d.msg))
            .collect();
        return format!(
            r#"{{"error":{},"errors":{}}}"#,
            json::string("file luật đang có lỗi cú pháp"),
            json::strings(messages.iter().map(String::as_str)),
        );
    };

    let saved = layout::parse_saved(
        &std::fs::read_to_string(root.join(LAYOUT_FILE)).unwrap_or_default(),
    );
    let places = layout::place(&doc, &saved);
    let baseline = arch_core::baseline::parse(
        &std::fs::read_to_string(root.join(arch_core::baseline::BASELINE_FILE)).unwrap_or_default(),
    );

    let mut nodes = Vec::new();
    for c in &doc.components {
        let kind = if c.external { "external" } else { "component" };
        nodes.push(node_json(&c.name, kind, &c.paths.join(", "), &c.meta, &places));
    }
    for s in &doc.stores {
        nodes.push(node_json(&s.name, "store", &s.tables.join(", "), &s.meta, &places));
    }
    for a in &doc.apis {
        nodes.push(node_json(&a.name, "api", &a.symbols.join(", "), &a.meta, &places));
    }

    let edges: Vec<String> = doc
        .rules
        .iter()
        .flat_map(|r| {
            let kind = if r.kind == arch_core::model::RuleKind::Allow { "allow" } else { "deny" };
            let from = r.from.as_str().to_string();
            let status = r.meta.status.as_str();
            let line = r.meta.line;
            r.to.iter().map(move |t| {
                format!(
                    r#"{{"from":{},"to":{},"kind":{},"status":{},"line":{line}}}"#,
                    json::string(&from),
                    json::string(t.as_str()),
                    json::string(kind),
                    json::string(status),
                )
            })
        })
        .collect();

    let boundaries: Vec<String> = doc
        .boundaries
        .iter()
        .map(|b| {
            format!(
                r#"{{"name":{},"contains":{}}}"#,
                json::string(&b.name),
                json::strings(b.contains.iter().map(String::as_str)),
            )
        })
        .collect();

    let violations: Vec<String> = report
        .violations
        .iter()
        .map(|v| {
            format!(
                r#"{{"from":{},"to":{},"file":{},"line":{},"spec":{},"via":{},"count":{},"rule":{},"hint":{},"baselined":{}}}"#,
                json::string(&v.from),
                json::string(&v.to),
                json::string(&v.file),
                v.line,
                json::string(&v.spec),
                json::string(match v.via {
                    Via::Import => "import",
                    Via::Symbol => "symbol",
                    Via::Table => "table",
                }),
                v.count,
                json::string(&v.rule),
                json::string(&v.hint),
                baseline.contains(&arch_core::baseline::key(v)),
            )
        })
        .collect();

    let missing: Vec<String> = report
        .missing
        .iter()
        .map(|m| {
            format!(
                r#"{{"name":{},"source":{},"line":{},"hint":{}}}"#,
                json::string(&m.name),
                json::string(&m.source),
                m.line,
                json::string(&m.hint),
            )
        })
        .collect();

    let (proposals, _) = protocol::parse_proposals(
        &std::fs::read_to_string(root.join(PROPOSALS_FILE)).unwrap_or_default(),
    );
    let pending: Vec<String> = proposals
        .iter()
        .map(|p| {
            format!(
                r#"{{"id":{},"intent":{},"statement":{},"reason":{},"origin":{},"date":{}}}"#,
                json::string(&p.id),
                json::string(p.intent.keyword()),
                json::string(&p.statement),
                json::string(&p.reason),
                json::string(p.origin.as_deref().unwrap_or("")),
                json::string(&p.date),
            )
        })
        .collect();

    format!(
        r#"{{"archHash":{},"nodes":[{}],"edges":[{}],"boundaries":[{}],"violations":[{}],"missing":[{}],"proposals":[{}],"stats":{{"files":{},"checked":{},"unchecked":{}}}}}"#,
        json::string(&hash),
        nodes.join(","),
        edges.join(","),
        boundaries.join(","),
        violations.join(","),
        missing.join(","),
        pending.join(","),
        report.files_scanned,
        json::strings(report.checked.iter().map(String::as_str)),
        json::strings(report.unchecked.iter().map(String::as_str)),
    )
}

fn node_json(
    name: &str,
    kind: &str,
    sub: &str,
    meta: &arch_core::model::Meta,
    places: &std::collections::BTreeMap<String, Point>,
) -> String {
    let point = places.get(name).copied().unwrap_or(Point { x: 48.0, y: 48.0 });
    format!(
        r#"{{"name":{},"kind":{},"sub":{},"status":{},"owner":{},"line":{},"x":{:.0},"y":{:.0}}}"#,
        json::string(name),
        json::string(kind),
        json::string(sub),
        json::string(meta.status.as_str()),
        json::string(if meta.owner == Owner::Ai { "ai" } else { "human" }),
        meta.line,
        point.x,
        point.y,
    )
}

// ---------- thao tác ----------

fn field(body: &str, key: &str) -> Option<String> {
    jsonc::parse(body)?.get(key)?.as_str().map(str::to_string)
}

fn save_position(root: &Path, body: &str) -> Result<(), String> {
    let name = field(body, "name").ok_or("thiếu `name`")?;
    let x: f64 = field(body, "x").ok_or("thiếu `x`")?.parse().map_err(|_| "x không phải số")?;
    let y: f64 = field(body, "y").ok_or("thiếu `y`")?.parse().map_err(|_| "y không phải số")?;

    let path = root.join(LAYOUT_FILE);
    let mut saved =
        layout::parse_saved(&std::fs::read_to_string(&path).unwrap_or_default());
    saved.insert(name, Point { x, y });
    std::fs::write(&path, layout::render_saved(&saved)).map_err(|e| e.to_string())
}

/// Đổi trạng thái một phần tử ngay trên sơ đồ — đây là nút khoá.
///
/// Sửa đúng dòng của phần tử đó trong `system.arch`, giữ nguyên mọi dòng khác.
fn set_status(root: &Path, body: &str) -> Result<(), String> {
    let name = field(body, "name").ok_or("thiếu `name`")?;
    let wanted = field(body, "status").ok_or("thiếu `status`")?;
    let status = match wanted.as_str() {
        "#draft" => Status::Draft,
        "#proposed" => Status::Proposed,
        "#accepted" => Status::Accepted,
        "#locked" => Status::Locked,
        other => return Err(format!("trạng thái không hợp lệ: {other}")),
    };

    let path = root.join(ARCH_FILE);
    let src = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let (doc, _) = arch_core::load(&src);
    let element = doc
        .elements()
        .into_iter()
        .find(|e| e.key == name)
        .ok_or_else(|| format!("không có phần tử nào tên `{name}`"))?;

    let mut lines: Vec<String> = src.lines().map(str::to_string).collect();
    let index = element.line.checked_sub(1).ok_or("phần tử không có số dòng")?;
    let line = lines.get(index).ok_or("số dòng vượt ngoài file")?;
    lines[index] = arch_core::parser::rewrite_annotations(line, element.owner, status);

    let updated = format!("{}\n", lines.join("\n"));
    // Không ghi nếu kết quả hỏng: thà không làm gì còn hơn làm hỏng file luật.
    if let Some(d) = arch_core::load(&updated)
        .1
        .iter()
        .find(|d| d.severity == arch_core::Severity::Error)
    {
        return Err(format!("sửa xong thì file luật hỏng: {}", d.msg));
    }
    std::fs::write(&path, updated).map_err(|e| e.to_string())
}

/// Duyệt hoặc từ chối một đề xuất, dùng lại đúng lệnh của `archctl`.
fn decide(root: &Path, body: &str) -> Result<(), String> {
    let id = field(body, "id").ok_or("thiếu `id`")?;
    let verdict = field(body, "verdict").ok_or("thiếu `verdict`")?;
    let reason = field(body, "reason").unwrap_or_default();
    if reason.trim().is_empty() {
        return Err("phải ghi lý do, kể cả khi duyệt".into());
    }
    let command = match verdict.as_str() {
        "accept" => "accept",
        "reject" => "reject",
        other => return Err(format!("phán quyết không hợp lệ: {other}")),
    };

    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let out = std::process::Command::new(exe)
        .args([command, &id, "--reason", &reason, "--root"])
        .arg(root)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}
