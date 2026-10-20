//! Đọc một file docker-compose thành dữ kiện về từng service.
//!
//! Chỉ dữ kiện cần cho kiến trúc: service dựng từ đâu, chạy lệnh gì, nói
//! chuyện với ai. Giá trị biến môi trường **không bao giờ** được giữ: từ mỗi
//! giá trị chỉ lấy ra tên máy và cổng (TM-10), vì giá trị thường mang mật khẩu
//! và output của archctl đi vào log CI và context của agent.

use super::dockerfile::{Command, shell_words};
use super::yaml::{self, Node};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct File {
    pub services: Vec<Service>,
    /// `include:` ở cấp cao nhất: file compose khác, kèm dòng.
    pub includes: Vec<(String, usize)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub line: usize,
    pub image: Option<String>,
    pub build: Option<Build>,
    pub command: Option<(Command, usize)>,
    pub entrypoint: Option<(Command, usize)>,
    pub working_dir: Option<String>,
    pub env: Vec<Env>,
    pub env_files: Vec<(String, usize)>,
    pub depends_on: Vec<(String, usize)>,
    pub links: Vec<(String, usize)>,
    pub binds: Vec<Bind>,
    pub extends: Option<Extends>,
    /// Tên khác mà service khác dùng để gọi service này: `hostname`,
    /// `container_name`, alias trong `networks`.
    pub aliases: Vec<String>,
    /// Cổng publish ra máy chủ (`ports:`), và `network_mode: host`.
    pub ports: Vec<Port>,
}

/// Một cổng publish: như file viết (không có giá trị biến môi trường nào), dòng,
/// và có chỉ mở cho máy cục bộ không (`127.0.0.1:8080:80`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub shown: String,
    pub line: usize,
    pub local: bool,
}

/// Địa chỉ chỉ máy cục bộ mới tới được.
fn loopback(host: &str) -> bool {
    let h = host.trim_matches(['[', ']']);
    h == "localhost" || h == "::1" || h.starts_with("127.")
}

/// `ports:` dạng chuỗi: `"80"`, `"8080:80"`, `"127.0.0.1:8080:80"`,
/// `"[::1]:8080:80/tcp"`. Chỉ có cổng container thì Docker vẫn publish một cổng
/// máy ngẫu nhiên trên mọi giao diện, nên không phải cục bộ.
fn port_local(spec: &str) -> bool {
    let spec = if spec.contains("${") { defaults(spec) } else { spec.to_string() };
    let spec = spec.split('/').next().unwrap_or(&spec).to_string();
    if let Some(rest) = spec.strip_prefix('[') {
        return rest.split_once(']').is_some_and(|(h, _)| loopback(h));
    }
    // `host:published:target`; IPv6 không ngoặc (`::1:6000:6000`): mọi phần
    // trước hai phần cuối là địa chỉ, như Docker tách.
    let parts: Vec<&str> = spec.split(':').collect();
    parts.len() >= 3 && loopback(&parts[..parts.len() - 2].join(":"))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Build {
    /// Thư mục, như file viết (tương đối với file compose).
    pub context: String,
    pub dockerfile: Option<String>,
    /// `dockerfile_inline`: nội dung Dockerfile viết thẳng trong compose.
    pub inline: Option<String>,
    pub target: Option<String>,
    pub line: usize,
}

/// Một biến môi trường: chỉ khoá, dòng, và những tên máy trong giá trị.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Env {
    pub key: String,
    pub line: usize,
    pub hosts: Vec<Host>,
    /// Giá trị dài quá nên chỉ xét một phần, xem [`hosts_scan`].
    pub cut: bool,
    /// Giá trị, chỉ giữ cho các khoá trong [`KEPT`]: chúng quyết mã nào chạy
    /// và không mang secret.
    pub value: Option<String>,
    /// Giá trị khi mã đọc biến này làm tên topic, queue, kênh (chỉ khi được
    /// hỏi, xem [`parse_with`]), nó trông như một cái tên và khoá không mang
    /// dấu hiệu bí mật (TM-19, TM-43).
    pub word: Option<String>,
    /// File khai biến này khi khác file của đối tượng chứa nó: patch
    /// Kustomize đổi env thì vi phạm trỏ về patch.
    pub origin: Option<String>,
}

/// Biến môi trường được giữ giá trị.
pub const KEPT: &[&str] = &["PYTHONPATH", "FLASK_APP"];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Host {
    pub name: String,
    pub port: Option<String>,
}

impl Host {
    pub fn shown(&self) -> String {
        match &self.port {
            Some(p) => format!("{}:{p}", self.name),
            None => self.name.clone(),
        }
    }
}

/// Bind mount `./src:/app/src`: mã trong repo hiện ra trong container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bind {
    pub host: String,
    pub container: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extends {
    pub file: Option<String>,
    pub service: String,
    pub line: usize,
}

/// Trần cho một file. Compose thật có vài chục service; file đối kháng thì
/// dùng alias để nhân số mục lên mà không tốn byte nào (TM-8): `services:
/// s{i}: *svc` với `*svc` có hàng nghìn `depends_on` là n·m cạnh từ vài chục KB.
pub const MAX_SERVICES: usize = 2000;
pub const MAX_FACTS: usize = 50_000;
/// Byte chép ra khỏi cây YAML cho cả file: argv, giá trị env đem đi tìm tên
/// máy, đường dẫn, tên. File 1 MB không có alias chỉ cần cỡ đó; alias thì nhân
/// một chuỗi 300 KB lên hàng nghìn lần mà không tốn thêm byte nào của file.
pub const MAX_COPIED: usize = 8 << 20;

/// Ngân sách của một file, trừ dần khi chép.
pub(crate) struct Budget {
    pub(crate) bytes: usize,
    pub(crate) over: bool,
}

impl Budget {
    /// Trừ `n` byte; hết thì ghi lại và trả `false`.
    pub(crate) fn take(&mut self, n: usize) -> bool {
        match self.bytes.checked_sub(n + 1) {
            Some(rest) => {
                self.bytes = rest;
                true
            }
            None => {
                self.bytes = 0;
                self.over = true;
                false
            }
        }
    }

    /// Chép một chuỗi nếu còn ngân sách. Hết thì trả chuỗi rỗng: `parse` bỏ
    /// cả file ngay sau service đó.
    pub(crate) fn own(&mut self, s: &str) -> String {
        if self.take(s.len()) { s.to_string() } else { String::new() }
    }
}

pub fn parse(text: &str) -> Result<File, yaml::Error> {
    parse_with(text, &[])
}

/// Như [`parse`], và giữ giá trị dạng tên của các biến môi trường trong
/// `wanted` (tên topic mà mã đọc từ env, TM-43). Giá trị khác không giữ (TM-19).
pub fn parse_with(text: &str, wanted: &[String]) -> Result<File, yaml::Error> {
    let doc = yaml::parse(text)?;
    let mut out = File::default();
    let mut budget = MAX_FACTS;
    let mut b = Budget { bytes: MAX_COPIED, over: false };
    let Some(root) = doc.documents().next() else { return Ok(out) };
    for item in root.get("include").map(|n| n.items().collect::<Vec<_>>()).unwrap_or_default() {
        if let Some(s) = item.as_str() {
            out.includes.push((b.own(s), item.line()));
        } else if let Some(path) = item.get("path") {
            if let Some(s) = path.as_str() {
                out.includes.push((b.own(s), path.line()));
            }
            for p in path.items() {
                if let Some(s) = p.as_str() {
                    out.includes.push((b.own(s), p.line()));
                }
            }
        }
    }
    // Định dạng cũ (v1) không có `services:`: mọi khoá cấp cao nhất có `image`
    // hay `build` là một service.
    let services = match root.get("services") {
        Some(s) => s,
        None if root.entries().any(|(_, _, n)| n.get("image").is_some() || n.get("build").is_some()) => root,
        None => return Ok(out),
    };
    for (name, line, node) in services.entries() {
        if name.starts_with("x-") || !node.is_map() {
            continue;
        }
        if out.services.len() >= MAX_SERVICES {
            return Err(yaml::Error { line, msg: format!("quá {MAX_SERVICES} service trong một file") });
        }
        let s = service(name, line, node, &mut b, wanted);
        if b.over {
            return Err(yaml::Error {
                line,
                msg: format!("quá {} MB dữ liệu sau khi trải alias trong một file", MAX_COPIED >> 20),
            });
        }
        let facts = s.env.len() + s.env_files.len() + s.depends_on.len() + s.links.len() + s.binds.len() + s.aliases.len() + s.ports.len();
        budget = budget.checked_sub(facts).ok_or_else(|| yaml::Error {
            line,
            msg: format!("quá {MAX_FACTS} mục (env, depends_on, volumes…) trong một file"),
        })?;
        out.services.push(s);
    }
    if b.over {
        return Err(yaml::Error { line: 1, msg: format!("quá {} MB dữ liệu sau khi trải alias trong một file", MAX_COPIED >> 20) });
    }
    Ok(out)
}

fn service(name: &str, line: usize, n: Node<'_>, b: &mut Budget, wanted: &[String]) -> Service {
    let mut s = Service { name: b.own(name), line, ..Service::default() };
    let text = |node: Option<Node<'_>>, b: &mut Budget| node.and_then(|i| i.as_str()).map(|v| b.own(v));
    s.image = text(n.get("image"), b);
    s.working_dir = text(n.get("working_dir"), b);
    if let Some(node) = n.get("build") {
        s.build = Some(match node.as_str() {
            Some(ctx) => Build { context: b.own(ctx), line: node.line(), ..Build::default() },
            None => Build {
                context: text(node.get("context"), b).unwrap_or_else(|| ".".to_string()),
                dockerfile: text(node.get("dockerfile"), b),
                inline: text(node.get("dockerfile_inline"), b),
                target: text(node.get("target"), b),
                line: node.line(),
            },
        });
    }
    s.command = n.get("command").and_then(|c| command(c, b));
    s.entrypoint = n.get("entrypoint").and_then(|c| command(c, b));

    if let Some(env) = n.get("environment") {
        // Giá trị không được chép, nhưng được quét tìm tên máy: tính vào ngân
        // sách như chép.
        for (key, kline, value) in env.entries() {
            let value = value.as_str().unwrap_or("");
            if b.take(key.len() + value.len()) {
                s.env.push(env_entry_with(key, kline, value, wanted));
            }
        }
        for item in env.items() {
            if let Some(text) = item.as_str()
                && b.take(text.len())
            {
                let (key, value) = text.split_once('=').unwrap_or((text, ""));
                s.env.push(env_entry_with(key, item.line(), value, wanted));
            }
        }
    }
    if let Some(files) = n.get("env_file") {
        let mut push = |node: Node<'_>| {
            let path = node.as_str().or_else(|| node.get("path").and_then(|p| p.as_str()));
            if let Some(p) = path {
                s.env_files.push((b.own(p), node.line()));
            }
        };
        if files.as_str().is_some() {
            push(files);
        }
        for f in files.items() {
            push(f);
        }
    }
    if let Some(deps) = n.get("depends_on") {
        for item in deps.items() {
            if let Some(name) = item.as_str() {
                s.depends_on.push((b.own(name), item.line()));
            }
        }
        for (name, kline, _) in deps.entries() {
            s.depends_on.push((b.own(name), kline));
        }
    }
    if let Some(links) = n.get("links") {
        for item in links.items() {
            if let Some(text) = item.as_str() {
                let target = text.split(':').next().unwrap_or(text);
                s.links.push((b.own(target), item.line()));
            }
        }
    }
    for p in n.get("ports").into_iter().flat_map(Node::items) {
        let (shown, local) = match p.as_str() {
            Some(spec) => (b.own(spec), port_local(spec)),
            // Dạng dài: `{target: 80, published: 8080, host_ip: 127.0.0.1}`.
            None => {
                let field = |k: &str| p.get(k).and_then(Node::as_str).unwrap_or("");
                let host = field("host_ip");
                let local = !host.is_empty() && loopback(&if host.contains("${") { defaults(host) } else { host.to_string() });
                let shown = format!("{}{}:{}", if host.is_empty() { String::new() } else { format!("{host}:") }, field("published"), field("target"));
                (b.own(&shown), local)
            }
        };
        s.ports.push(Port { shown, line: p.line(), local });
    }
    if let Some(m) = n.get("network_mode").filter(|m| m.as_str() == Some("host")) {
        s.ports.push(Port { shown: "network_mode: host".into(), line: m.line(), local: false });
    }
    if let Some(vols) = n.get("volumes") {
        for item in vols.items() {
            if let Some(text) = item.as_str() {
                // `host:container[:mode]`. Volume có tên (`data:/var/lib`) không
                // phải mã của repo.
                let mut parts = text.splitn(3, ':');
                if let (Some(host), Some(container)) = (parts.next(), parts.next())
                    && (host == "." || host.starts_with("./") || host.starts_with("../"))
                {
                    s.binds.push(Bind { host: b.own(host), container: b.own(container), line: item.line() });
                }
            } else if item.get("type").and_then(|t| t.as_str()) == Some("bind")
                && let (Some(src), Some(dst)) = (
                    item.get("source").and_then(|v| v.as_str()),
                    item.get("target").and_then(|v| v.as_str()),
                )
                && (src == "." || src.starts_with("./") || src.starts_with("../"))
            {
                s.binds.push(Bind { host: b.own(src), container: b.own(dst), line: item.line() });
            }
        }
    }
    if let Some(e) = n.get("extends") {
        let service = e.as_str().or_else(|| e.get("service").and_then(|v| v.as_str()));
        if let Some(service) = service {
            s.extends = Some(Extends {
                file: e.get("file").and_then(|f| f.as_str()).map(|f| b.own(f)),
                service: b.own(service),
                line: e.line(),
            });
        }
    }
    for key in ["hostname", "container_name"] {
        if let Some(v) = n.get(key).and_then(|v| v.as_str()) {
            s.aliases.push(b.own(v));
        }
    }
    if let Some(nets) = n.get("networks") {
        for (_, _, net) in nets.entries() {
            for a in net.get("aliases").map(|a| a.items().collect::<Vec<_>>()).unwrap_or_default() {
                if let Some(v) = a.as_str() {
                    s.aliases.push(b.own(v));
                }
            }
        }
    }
    s
}

/// `command`: chuỗi thì compose tách từ như shell rồi chạy thẳng, không qua
/// `sh -c`; danh sách thì giữ nguyên.
fn command(n: Node<'_>, b: &mut Budget) -> Option<(Command, usize)> {
    if let Some(s) = n.as_str() {
        let words = if b.take(s.len()) { shell_words(s) } else { Vec::new() };
        return Some((Command::Exec(words), n.line()));
    }
    if n.is_seq() {
        let items: Vec<String> = n.items().map(|i| b.own(i.as_str().unwrap_or(""))).collect();
        return Some((Command::Exec(items), n.line()));
    }
    None
}

pub fn env_entry(key: &str, line: usize, value: &str) -> Env {
    env_entry_with(key, line, value, &[])
}

/// Như [`env_entry`], và giữ giá trị dạng tên khi khoá nằm trong `wanted`.
pub fn env_entry_with(key: &str, line: usize, value: &str, wanted: &[String]) -> Env {
    let key = key.trim();
    let (hosts, cut) = hosts_scan(key, value);
    Env {
        key: key.to_string(),
        line,
        hosts,
        cut,
        value: KEPT.contains(&key).then(|| value.to_string()),
        word: if wanted.iter().any(|w| w == key) { word_value(key, value) } else { None },
        origin: None,
    }
}

/// Khoá có một đoạn nói về bí mật: `DB_PASSWORD`, `KAFKA_TOKEN`.
pub(crate) fn secret_key(key: &str) -> bool {
    key.to_ascii_uppercase().split(['_', '-', '.']).any(|p| SECRET_PARTS.contains(&p))
}

/// Giá trị dạng tên: chữ, số, `.`, `_`, `-`, `:`, tối đa 249 ký tự (trần tên
/// topic của Kafka). `${X:-orders}` lấy giá trị mặc định. Khoá có đoạn nói về
/// bí mật (`…_KEY`, `…_TOKEN`) thì không giữ gì.
pub fn word_value(key: &str, value: &str) -> Option<String> {
    if secret_key(key) {
        return None;
    }
    let value: std::borrow::Cow<'_, str> = if value.contains("${") { defaults(value).into() } else { value.into() };
    let v = value.trim();
    let ok = !v.is_empty()
        && v.len() <= 249
        && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':'));
    ok.then(|| v.to_string())
}

/// Đoạn cuối của tên khoá nói giá trị là địa chỉ máy: `DB_HOST`, `REDIS_URL`.
/// Chỉ đoạn cuối: `SERVER_TOKEN` là token, không phải máy.
const HOST_SUFFIXES: &[&str] = &[
    "HOST", "HOSTNAME", "HOSTS", "ADDR", "ADDRESS", "SERVER", "SERVERS", "URL", "URLS", "URI", "URIS", "ENDPOINT",
    "ENDPOINTS", "DSN", "BROKER", "BROKERS", "PEERS", "NODES", "CLUSTER", "SERVICE",
];
/// Đoạn của tên khoá nói giá trị là bí mật. Khoá như vậy không bao giờ được
/// đọc ra tên máy: `SUPABASE_SERVICE_ROLE_KEY` là một JWT trông y như tên máy.
pub(crate) const SECRET_PARTS: &[&str] = &[
    "KEY", "KEYS", "TOKEN", "TOKENS", "SECRET", "SECRETS", "PASSWORD", "PASSWD", "PASS", "PWD", "SALT", "CREDENTIAL",
    "CREDENTIALS", "CREDS", "AUTH", "PRIVATE", "CERT", "SIGNATURE", "JWT", "APIKEY", "OTP",
];

/// Tên máy (kèm cổng) trong giá trị của một biến môi trường.
///
/// - `scheme://[user[:pass]@]host[:port]/...`: phần xác thực bị bỏ;
/// - `host:port`;
/// - giá trị trần `db`, chỉ khi đoạn cuối tên khoá nói về máy (`DB_HOST`).
///
/// Khoá có một đoạn nói về bí mật (`…_KEY`, `…_TOKEN`) thì chỉ đọc URL có
/// `scheme://`: `AUTH_SERVICE_URL=http://auth:8000` là cạnh tới service `auth`,
/// còn giá trị trần hay `a:b` của khoá đó có thể là chính secret. Tên máy viết thường, bỏ dấu chấm cuối: DNS không phân biệt hoa thường, và
/// `db.` là `db`. `${X:-mặc định}` lấy giá trị mặc định; biến không có mặc
/// định thì không biết, và không đoán.
pub fn hosts_in(key: &str, value: &str) -> Vec<Host> {
    hosts_scan(key, value).0
}

/// Như [`hosts_in`], kèm cờ "đã cắt": giá trị quá [`MAX_PARTS`] phần tử hay
/// quá [`MAX_HOSTS`] tên máy thì phần sau không được xét, và bên gọi phải báo
/// ra, vì một cạnh tới service có thể nằm ở đó (TM-12).
pub fn hosts_scan(key: &str, value: &str) -> (Vec<Host>, bool) {
    hosts_scan_within(key, value, MAX_PARTS, MAX_HOSTS)
}

/// Như [`hosts_scan`] với trần khác: chuỗi trong mã có thể dài cả trang mà
/// vẫn chỉ là chữ, nên không cắt theo số phần tử, chỉ theo số tên máy.
pub fn hosts_scan_within(key: &str, value: &str, max_parts: usize, max_hosts: usize) -> (Vec<Host>, bool) {
    let upper = key.to_ascii_uppercase();
    let parts: Vec<&str> = upper.split(['_', '-', '.']).filter(|p| !p.is_empty()).collect();
    let secret = parts.iter().any(|p| SECRET_PARTS.contains(p));
    let hostish = !secret && parts.last().is_some_and(|p| HOST_SUFFIXES.contains(p));
    let value: std::borrow::Cow<'_, str> = if value.contains("${") { defaults(value).into() } else { value.into() };
    let mut out = Found { max: max_hosts, ..Found::default() };
    // Danh sách máy thật có vài chục phần tử; giá trị dài hơn thế không phải
    // danh sách máy, và tách hết nó là tốn theo cỡ giá trị (TM-8).
    let mut parts = value.split([',', ' ', ';', '\t', '\n']).filter(|p| !p.is_empty());
    for part in parts.by_ref().take(max_parts) {
        if let Some((_, after)) = part.split_once("://") {
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            let authority = &after[..end];
            match (authority.rfind('@'), after.rfind('@')) {
                (Some(at), _) => host_port(&mut out, &authority[at + 1..], true),
                // `@` nằm sau một `/`: mật khẩu có `/` chưa mã hoá
                // (`postgres://u:ab/cd@db/x`, `redis://hunt/er2@redis`), hay `@`
                // trong đường dẫn. Thử cả hai; vế đầu khi đó có thể là `user:mật
                // khẩu`, nên không lấy cổng của nó. Vế đầu là tên miền có dấu
                // chấm mà không có `:` thì nó là máy, và `@` là của đường dẫn
                // (`https://youtube.com/@excalidraw`).
                (None, Some(at)) if authority.contains(':') || !authority.contains('.') => {
                    let rest = &after[at + 1..];
                    host_port(&mut out, &rest[..rest.find(['/', '?', '#']).unwrap_or(rest.len())], true);
                    host_port(&mut out, authority, false);
                }
                (None, _) => host_port(&mut out, authority, true),
            }
        } else if secret {
            continue;
        } else if let Some((h, p)) = part.rsplit_once(':') {
            if !h.contains(':') && !h.contains('@') && !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) {
                push_host(&mut out, h, Some(p));
            }
        } else if hostish {
            push_host(&mut out, part, None);
        }
    }
    let cut = out.cut || parts.next().is_some();
    (out.hosts, cut)
}

/// Tên máy tìm được, và có tên nào bị bỏ vì đã đủ [`MAX_HOSTS`] không.
#[derive(Default)]
struct Found {
    hosts: Vec<Host>,
    cut: bool,
    max: usize,
}

/// `host[:port]`; `with_port` sai thì bỏ cổng.
fn host_port(out: &mut Found, text: &str, with_port: bool) {
    match text.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
            push_host(out, h, with_port.then_some(p));
        }
        _ => push_host(out, text, None),
    }
}

/// Giá trị thật có vài tên máy; trần giữ cho một giá trị 1 MB toàn `a1:1,a2:1,…`
/// không thành bậc hai.
const MAX_HOSTS: usize = 64;
/// Số phần tử của một giá trị được xét.
const MAX_PARTS: usize = 256;

fn push_host(out: &mut Found, name: &str, port: Option<&str>) {
    // Kiểm trên chuỗi mượn trước khi cấp phát: một giá trị 1 MB toàn `a a a`
    // là nửa triệu lần gọi tới đây.
    let name = name.trim_end_matches('.');
    let ok = !name.is_empty()
        && name.len() <= 253
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'))
        && name.bytes().any(|c| c.is_ascii_alphabetic());
    if !ok || out.hosts.iter().any(|h| h.name.eq_ignore_ascii_case(name) && h.port.as_deref() == port) {
        return;
    }
    if out.hosts.len() >= out.max {
        out.cut = true;
        return;
    }
    out.hosts.push(Host { name: name.to_ascii_lowercase(), port: port.map(str::to_string) });
}

/// Thay `${X:-d}` và `${X-d}` bằng `d`. `${X}` không có mặc định giữ nguyên.
fn defaults(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find("${") {
        out.push_str(&rest[..at]);
        let body = &rest[at + 2..];
        let Some(end) = body.find('}') else {
            out.push_str(&rest[at..]);
            return out;
        };
        let inner = &body[..end];
        match inner.split_once(":-").or_else(|| inner.split_once('-')) {
            Some((_, d)) => out.push_str(d),
            None => out.push_str(&rest[at..at + 2 + end + 1]),
        }
        rest = &body[end + 1..];
    }
    out.push_str(rest);
    out
}

impl Service {
    /// Byte mà service này giữ, xấp xỉ: để bên gộp `extends` (mỗi con một bản
    /// sao của gốc) có trần theo byte, không chỉ theo số mục.
    pub fn size(&self) -> usize {
        let argv = |c: &Option<(Command, usize)>| match c {
            Some((Command::Exec(words), _)) => words.iter().map(|w| w.len() + 1).sum(),
            Some((Command::Shell(text), _)) => text.len(),
            None => 0,
        };
        let strs = |v: &[(String, usize)]| v.iter().map(|(s, _)| s.len() + 1).sum::<usize>();
        argv(&self.command)
            + argv(&self.entrypoint)
            + self.build.as_ref().map_or(0, |b| b.context.len() + b.inline.as_ref().map_or(0, String::len))
            + self.env.iter().map(|e| e.key.len() + e.value.as_ref().map_or(0, String::len) + 16 * e.hosts.len()).sum::<usize>()
            + strs(&self.env_files)
            + strs(&self.depends_on)
            + strs(&self.links)
            + self.binds.iter().map(|b| b.host.len() + b.container.len()).sum::<usize>()
            + self.ports.iter().map(|p| p.shown.len() + 8).sum::<usize>()
            + self.aliases.iter().map(String::len).sum::<usize>()
    }
}

/// Gộp service `base` vào `child` theo `extends`: trường đơn của `child` thắng,
/// danh sách thì nối.
pub fn inherit(child: &mut Service, base: &Service) {
    if child.image.is_none() {
        child.image = base.image.clone();
    }
    if child.build.is_none() {
        child.build = base.build.clone();
    }
    if child.command.is_none() {
        child.command = base.command.clone();
    }
    if child.entrypoint.is_none() {
        child.entrypoint = base.entrypoint.clone();
    }
    if child.working_dir.is_none() {
        child.working_dir = base.working_dir.clone();
    }
    child.env.extend(base.env.iter().cloned());
    child.env_files.extend(base.env_files.iter().cloned());
    child.depends_on.extend(base.depends_on.iter().cloned());
    child.links.extend(base.links.iter().cloned());
    child.binds.extend(base.binds.iter().cloned());
    child.ports.extend(base.ports.iter().cloned());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, port: Option<&str>) -> Host {
        Host { name: name.into(), port: port.map(str::to_string) }
    }

    #[test]
    fn ten_may_trong_gia_tri_env() {
        assert_eq!(hosts_in("DATABASE_URL", "postgres://u:mat:khau@db:5432/app"), [host("db", Some("5432"))]);
        assert_eq!(hosts_in("REDIS_URL", "redis://:secret@redis:6379/0"), [host("redis", Some("6379"))]);
        assert_eq!(hosts_in("QDRANT_URL", "http://qdrant:6333"), [host("qdrant", Some("6333"))]);
        assert_eq!(hosts_in("KAFKA", "k1:9092,k2:9092"), [host("k1", Some("9092")), host("k2", Some("9092"))]);
        assert_eq!(hosts_in("DB_HOST", "db"), [host("db", None)]);
        assert_eq!(hosts_in("MODE", "worker"), [], "khoá không nói về máy");
        assert_eq!(hosts_in("DB_HOST", "${DB_HOST:-postgres}"), [host("postgres", None)]);
        assert_eq!(hosts_in("DB_HOST", "${DB_HOST}"), [], "không có mặc định thì không đoán");
        assert_eq!(hosts_in("PORT", "8080"), []);
        assert_eq!(hosts_in("TIME", "12:30"), [], "tên máy phải có chữ cái");
        // Khoá nói về bí mật thì không đọc giá trị trần hay `a:b`, kể cả khi có
        // đoạn trông như máy; chỉ đọc URL có `scheme://`.
        assert_eq!(hosts_in("SUPABASE_SERVICE_ROLE_KEY", "eyJhbGciOiJIUzI1NiJ9.eyJyb2xlIjoic2VydmljZSJ9.abc"), []);
        assert_eq!(hosts_in("STRIPE_WEBHOOK_ENDPOINT_SECRET", "whsec_abc"), []);
        assert_eq!(hosts_in("SERVER_TOKEN", "abc"), [], "đoạn cuối là TOKEN");
        assert_eq!(hosts_in("SECURITY_PASSWORD_SALT", "abc"), []);
        assert_eq!(hosts_in("API_KEY", "user:12345"), [], "`a:số` của khoá bí mật có thể là chính secret");
        assert_eq!(hosts_in("AUTH_SERVICE_URL", "http://auth:8000"), [host("auth", Some("8000"))]);
        assert_eq!(hosts_in("AUTH_TOKEN_URL", "https://u:pw@idp/token"), [host("idp", None)]);
        // Mật khẩu có `/` chưa mã hoá vẫn ra đúng máy, và không lấy cổng của
        // vế `user:mật khẩu`.
        let hosts = hosts_in("DATABASE_URL", "postgres://admin:2024/0101@db:5432/app");
        assert!(hosts.contains(&host("db", Some("5432"))), "{hosts:?}");
        assert!(!hosts.iter().any(|h| h.port.as_deref() == Some("2024")), "{hosts:?}");
        assert_eq!(hosts_in("DB_HOST", "DB."), [host("db", None)], "viết thường, bỏ chấm cuối");
    }

    #[test]
    fn service_day_du() {
        let text = r#"
x-env: &env
  REDIS_HOST: redis
services:
  api:
    build:
      context: ./api
      dockerfile: Dockerfile.prod
      target: runtime
    command: uvicorn app.main:app --port 8000
    environment:
      <<: *env
      DATABASE_URL: postgres://u:p@db:5432/x
      PYTHONPATH: /app/src
    env_file: .env
    depends_on:
      db:
        condition: service_healthy
    volumes:
      - ./api:/app
      - data:/data
    networks:
      back:
        aliases: [api-internal]
  worker:
    extends: api
    command: ["celery", "-A", "app.tasks", "worker"]
    environment:
      - "BROKER=redis://redis:6379"
    links: ["db:database"]
  db:
    image: postgres:16
"#;
        let f = parse(text).unwrap();
        assert_eq!(f.services.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["api", "worker", "db"]);
        let api = &f.services[0];
        let build = api.build.as_ref().unwrap();
        assert_eq!((build.context.as_str(), build.dockerfile.as_deref(), build.target.as_deref()), ("./api", Some("Dockerfile.prod"), Some("runtime")));
        assert_eq!(api.command.as_ref().unwrap().0, Command::Exec(shell_words("uvicorn app.main:app --port 8000")));
        let keys: Vec<_> = api.env.iter().map(|e| (e.key.as_str(), e.hosts.clone())).collect();
        assert!(keys.contains(&("DATABASE_URL", vec![host("db", Some("5432"))])), "{keys:?}");
        assert!(keys.contains(&("REDIS_HOST", vec![host("redis", None)])), "lấy qua `<<`: {keys:?}");
        assert_eq!(api.env.iter().find(|e| e.key == "PYTHONPATH").unwrap().value.as_deref(), Some("/app/src"));
        assert_eq!(api.env.iter().find(|e| e.key == "DATABASE_URL").unwrap().value, None);
        assert_eq!(api.env_files, [(".env".to_string(), 15)]);
        assert_eq!(api.depends_on, [("db".to_string(), 17)]);
        assert_eq!(api.binds.len(), 1, "volume có tên không phải mã");
        assert_eq!(api.aliases, ["api-internal"]);
        let worker = &f.services[1];
        assert_eq!(worker.extends.as_ref().unwrap().service, "api");
        assert_eq!(worker.links, [("db".to_string(), 30)]);
        assert_eq!(worker.env[0].hosts, [host("redis", Some("6379"))]);
        assert_eq!(f.services[2].image.as_deref(), Some("postgres:16"));
    }

    /// TM-10: không trường nào giữ giá trị gốc của biến môi trường.
    #[test]
    fn khong_giu_gia_tri_env() {
        let f = parse("services:\n  a:\n    environment:\n      DATABASE_URL: postgres://u:hunter2@db/x\n      OPENAI_API_KEY: sk-live-abc\n").unwrap();
        let dump = format!("{:?}", f);
        assert!(!dump.contains("hunter2") && !dump.contains("sk-live-abc"), "{dump}");
    }

    #[test]
    fn chi_giu_gia_tri_cua_bien_duoc_hoi() {
        let text = "services:\n  a:\n    environment:\n      TOPIC: orders\n      OTHER: secret-ish-but-name-like\n      KAFKA_TOKEN: orders\n";
        let wanted = vec!["TOPIC".to_string(), "KAFKA_TOKEN".to_string()];
        let f = parse_with(text, &wanted).unwrap();
        let words: Vec<_> = f.services[0].env.iter().map(|e| (e.key.as_str(), e.word.as_deref())).collect();
        assert_eq!(words, [("TOPIC", Some("orders")), ("OTHER", None), ("KAFKA_TOKEN", None)]);
        // Không hỏi gì thì không giữ gì (TM-19).
        assert!(!format!("{:?}", parse(text).unwrap()).contains("orders"));
    }
}
