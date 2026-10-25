//! Tên máy viết thẳng trong mã: `"http://payment:8080"`, `Redis(host="redis")`.
//!
//! Cạnh mạng lấy từ cấu hình có một lối lách: viết thẳng địa chỉ của service
//! khác vào mã thì không dòng cấu hình nào đổi. Ở đây đọc chuỗi trong mã theo
//! đúng luật tên máy của biến môi trường ([`compose::hosts_scan`]), với "tên
//! khoá" lấy từ chỗ chuỗi đứng: tên tham số, thuộc tính, biến được gán, hay tên
//! biến môi trường mà chuỗi là giá trị mặc định.
//!
//! - URL có `scheme://` và `máy:cổng` ở đâu cũng tính;
//! - chuỗi trần (`"redis"`) chỉ tính khi khoá nói về máy (`host`, `redisHost`,
//!   `DB_ADDR`): chữ thường như `"db"`, `"cache"` trùng tên service quá nhiều;
//! - chuỗi kết nối `Host=db;Port=5432;…` đọc từng cặp `khoá=giá trị`;
//! - khoá nói về bí mật (`password`, `apiKey`) chỉ đọc URL, như biến môi trường.
//!
//! Chuỗi gốc không bao giờ được giữ: nó có thể mang mật khẩu (TM-33). Tên máy
//! ghép lúc chạy (f-string, nối chuỗi, biến) thì không đọc: f-string bỏ phần
//! `{...}`, template JS thay `${...}` bằng `\0`.

use super::Lang;
use super::compose::{self, Host};
use super::lexer::{Tok, Token};

/// Tên máy giữ lại cho một file, và cho một chuỗi. Chuỗi được đọc hết, dài
/// bao nhiêu cũng vậy: cắt ở một độ dài nào đó là chỗ để giấu một URL (TM-32),
/// còn đọc thì tuyến tính theo cỡ file, vốn đã có trần.
pub const MAX_PER_FILE: usize = 1000;

/// Khoá mà `a:b` là nhãn hay phiên bản chứ không phải `máy:cổng`.
const LABELS: &[&str] = &["IMAGE", "IMAGES", "TAG", "TAGS", "VERSION", "REVISION", "DIGEST"];

/// Từ khoá đứng trước `(` mà không phải lời gọi hàm: `for k in ("A", "B")`.
const KEYWORDS: &[&str] = &[
    "in", "not", "and", "or", "if", "elif", "else", "return", "yield", "await", "for", "while", "assert", "is", "with",
    "case", "of", "typeof", "range",
];

/// Hàm mà đối số đầu là một tên máy.
const HOST_FUNCS: &[&str] = &["JoinHostPort", "gethostbyname", "gethostbyname_ex", "getaddrinfo", "LookupHost"];
/// Java: lớp và hàm dựng mà đối số đầu là tên máy (`new Jedis("redis", 6379)`).
const JAVA_HOST_CTORS: &[&str] = &[
    "Socket", "InetSocketAddress", "createUnresolved", "Jedis", "JedisPool", "JedisPooled", "HostAndPort", "fromParts",
    "ServerAddress",
];
/// Java: hàm ghi cấu hình theo khoá, `put("host", "db")`, `setProperty("db.host", "db")`.
const JAVA_SETTERS: &[&str] = &["put", "putIfAbsent", "setProperty", "set", "property", "withProperty"];

/// Thêm vào từ chỉ bí mật của biến môi trường, chỉ cho mã: `login = "admin:12345"`.
/// Biến môi trường thì không: `LOGIN_HOST=login` vẫn là một máy.
const CODE_SECRETS: &[&str] = &["PW", "PIN", "PASSPHRASE", "PRIVKEY", "USERINFO", "LOGIN"];

/// Đoạn cuối của khoá nói giá trị là URL hay endpoint: chuỗi trần ở đó hay là
/// tên route (`endpoint="login"`, `LOGIN_URL = "login"`), nên phải có dấu chấm.
const URLISH: &[&str] = &["URL", "URLS", "URI", "URIS", "ENDPOINT", "ENDPOINTS"];

/// Khoá nói về phía gọi vào: `ALLOWED_HOSTS`, `CORS_ORIGINS`, `bind_addr`.
const INBOUND: &[&str] = &["ALLOWED", "TRUSTED", "CORS", "ORIGIN", "ORIGINS", "BIND", "LISTEN"];

/// Hàm đọc cấu hình hay biến môi trường: `cfg.get("host", "db")` thì `"db"` là
/// giá trị của khoá `host`.
const GETTERS: &[&str] = &[
    "get", "getenv", "get_env", "getEnv", "Getenv", "GetEnv", "env", "getOrDefault", "getProperty", "GetString",
    "getString", "GetValue", "setdefault", "LookupEnv",
];

/// Đoạn cuối của khoá nói giá trị là một máy. Hẹp hơn của biến môi trường:
/// trong mã, `service = "cart"` hay `cluster = "main"` thường là nhãn.
const HOSTISH: &[&str] = &[
    "HOST", "HOSTNAME", "HOSTS", "ADDR", "ADDRESS", "SERVER", "SERVERS", "URL", "URLS", "URI", "URIS", "ENDPOINT",
    "ENDPOINTS", "DSN", "BROKER", "BROKERS",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeHost {
    pub line: usize,
    pub host: Host,
    /// Tên khoá như mã viết (`host`, `REDIS_URL`), nếu có.
    pub key: Option<String>,
}

impl CodeHost {
    /// `host → redis`, `chuỗi → payment:8080`. Không có giá trị gốc.
    pub fn shown(&self) -> String {
        format!("{} → {}", self.key.as_deref().unwrap_or("chuỗi"), self.host.shown())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeHosts {
    pub hosts: Vec<CodeHost>,
    /// Đã đủ [`MAX_PER_FILE`]: phần sau của file không được xét.
    pub cut: bool,
}

pub fn scan(toks: &[Token], lang: Lang) -> CodeHosts {
    let mut out = CodeHosts::default();
    let mut seen: std::collections::HashSet<(usize, Host)> = std::collections::HashSet::new();
    // Python và C ghép chuỗi liền nhau lúc dịch: `"http://" "db:5432"` là một chuỗi.
    let joins = matches!(lang, Lang::Python | Lang::C);
    let is_prefix = |t: &Tok| matches!(t, Tok::Word(w) if w.len() <= 2 && w.chars().all(|c| "rRbBuUfFtTL".contains(c)));
    let mut next = 0;
    for (i, t) in toks.iter().enumerate() {
        if i < next {
            continue;
        }
        let mut joined = String::new();
        let addr: String;
        let (text, key) = match &t.tok {
            Tok::Str(s) if joins => {
                let mut j = i + 1;
                joined.push_str(s);
                loop {
                    let k = if toks.get(j).is_some_and(|t| is_prefix(&t.tok)) { j + 1 } else { j };
                    match toks.get(k).map(|t| &t.tok) {
                        Some(Tok::Str(more)) => {
                            joined.push_str(more);
                            j = k + 1;
                        }
                        _ => break,
                    }
                }
                next = j;
                // Địa chỉ dạng cặp: `connect(("db", 5432))`, `addr = ("db", 5432)`.
                if let Some(port) = address_port(toks, i, j) {
                    addr = format!("{joined}:{port}");
                    (&addr, None)
                } else {
                    (&joined, None)
                }
            }
            // `@Value("${redis.host:redis}")` của Spring: giá trị mặc định của khoá.
            Tok::Str(s) if lang == Lang::Java && spring_default(s).is_some() => {
                let (k, v) = spring_default(s).unwrap_or_default();
                addr = v;
                (&addr, Some(k))
            }
            Tok::Str(s) | Tok::Template(s) => (s, None),
            // Chuỗi ba nháy chỉ khi là một giá trị: docstring thì không.
            Tok::Triple(s) if value_position(toks, i) => {
                (s, None)
            }
            Tok::Define(name, s) => (s, Some(name.clone())),
            _ => continue,
        };
        if text.is_empty() {
            continue;
        }
        let key = key.or_else(|| context(toks, i, lang));
        let (found, cut) = literal_hosts(text, key.as_deref());
        out.cut |= cut;
        for host in found {
            if out.hosts.len() >= MAX_PER_FILE {
                out.cut = true;
                return out;
            }
            if !seen.insert((t.line, host.clone())) {
                continue;
            }
            out.hosts.push(CodeHost { line: t.line, host, key: key.clone() });
        }
    }
    out
}

/// Tên máy trong một chuỗi đứng ở khoá `key`, và cờ "chưa đọc hết".
pub fn literal_hosts(text: &str, key: Option<&str>) -> (Vec<Host>, bool) {
    let k = key.map_or("", kind);
    if k == "INBOUND" {
        return (Vec::new(), false);
    }
    let urlish = key.is_some_and(|k| words(k).last().is_some_and(|w| URLISH.contains(&w.as_str())));
    let url = text.contains("://");
    // Chuỗi kết nối: `Server=db;Port=5432`, hay `host=db port=5432` của libpq.
    let libpq = !url && text.contains('=') && text.split_whitespace().all(|p| p.contains('='));
    let pairs = libpq || (!url && text.contains('=') && text.contains(';'));
    // Lọc rẻ: hầu hết chuỗi trong mã dừng ở đây.
    if !pairs && !text.contains(':') && k != "HOST" {
        return (Vec::new(), false);
    }
    // Một tên máy trần, cả chuỗi: không `:`, `/`, khoảng trắng (xét trước khi
    // bỏ chỗ giữ chỗ).
    // `" db "`, `"db/"`: vẫn là một tên máy.
    let trimmed = text.trim().trim_end_matches('/');
    let whole = !trimmed.contains([':', '/']) && !trimmed.contains(char::is_whitespace);
    let text = if whole && k == "HOST" { trimmed } else { text };
    // Tên route ở khoá URL: phải có dấu chấm mới là máy.
    let k = if urlish && k == "HOST" && whole && !trimmed.contains('.') { "" } else { k };
    let (text, stripped) = normalize(text);
    // `"db:%d" % port`, `f"db:{port}"` ở khoá kiểu máy: cả chuỗi là `máy:cổng`, chỉ
    // cổng ghép lúc chạy. Chỗ giữ chỗ khác (`f"cache:{key}"`) là khoá Redis, log:
    // chỉ tính khi khoá đã là kiểu máy, và không bao giờ ở khoá bí mật.
    let single = !text.is_empty() && text.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_'));
    let port_only = stripped != Stripped::None && !url && single && (k == "HOST" || stripped == Stripped::Port);
    let k = if port_only { "HOST" } else { k };
    let whole = whole || port_only;
    let mut found: Vec<(Host, bool)> = Vec::new();
    let mut cut = false;
    let mut add = |key: &str, value: &str, strict: bool| {
        let (hosts, c) = compose::hosts_scan_within(key, value, usize::MAX, MAX_PER_FILE);
        cut |= c;
        found.extend(hosts.into_iter().map(|h| (h, strict)));
    };
    if pairs {
        for part in text.split(|c: char| c == ';' || (libpq && c.is_whitespace())) {
            match part.split_once('=') {
                Some((pk, v)) => {
                    // `Server=tcp:db,1433`, `Server=db\SQLEXPRESS` của SQL Server.
                    let v = v.trim();
                    let v = if v.get(..4).is_some_and(|p| p.eq_ignore_ascii_case("tcp:")) { &v[4..] } else { v };
                    let v = v.split('\\').next().unwrap_or(v);
                    let pk = kind(pk.trim());
                    if pk != "INBOUND" {
                        add(pk, v, pk != "HOST");
                    }
                }
                None => add("", part.trim(), true),
            }
        }
    } else {
        // Chuỗi trần chỉ khi cả chuỗi là tên máy: `url = "data:image/png;base64,"`
        // không có máy nào tên `base64`.
        // Một câu ở khoá kiểu máy (`"host": "Hostname of the redis server"`)
        // cũng không: danh sách máy cách nhau bằng dấu phẩy, không khoảng trắng.
        let k = if k == "HOST" && !whole { "" } else { k };
        add(k, &text, !url && k != "HOST");
    }
    // Cổng phải là cổng thật. Không có URL hay khoá kiểu máy thì `a:b` dễ là thứ
    // khác (`"redis:7"` là image, `"worker:1"` là khoá Redis, `"admin:839201"`
    // là `user:mật khẩu`): đòi cổng từ 80 trở lên.
    let hosts = found
        .into_iter()
        .filter(|(h, strict)| match h.port.as_deref().map(str::parse::<u32>) {
            None => true,
            Some(Ok(p)) => (1..=65535).contains(&p) && (!strict || p >= 80),
            Some(Err(_)) => false,
        })
        .map(|(h, _)| h)
        .collect();
    (hosts, cut)
}

/// Chỗ giữ chỗ bị bỏ sau một `:`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stripped {
    None,
    /// Không biết là gì: `{}`, `%s`, `\0` của f-string và template.
    Any,
    /// Trông như cổng: `%d`, `{port}`, `%(port)s`.
    Port,
}

/// Độ dài tối đa của tên trong chỗ trống `{port}`, `%(port)s`.
const PLACEHOLDER_MAX: usize = 64;
/// Số máy tối đa của một URL nhiều máy (`mongodb://a:1,b:2/x`) được tách ra.
const MAX_URL_HOSTS: usize = 64;

/// Đưa chuỗi về dạng luật tên máy đọc được, giữ phần viết thẳng:
///
/// - cổng ghép lúc chạy bỏ đi, tên máy giữ lại: `http://db:\0/x` (f-string,
///   template), `db:%d`, `db:{}`, `db:{port}`, `http://db:` + biến;
/// - URL nhiều máy tách ra: `mongodb://a:1,b:2/x` thành `mongodb://a:1/x
///   mongodb://b:2/x`;
/// - DSN của Go MySQL: `user:pw@tcp(db:3306)/x` thành `db:3306`.
fn normalize(text: &str) -> (String, Stripped) {
    let mut stripped = Stripped::None;
    let mut s = String::with_capacity(text.len());
    let cs: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        let c = cs[i];
        if c == ':' {
            // Chỗ giữ chỗ ngay sau `:`.
            let rest = &cs[i + 1..];
            let placeholder = match rest {
                ['\0', ..] => Some(1),
                ['%', 'd' | 's' | 'v' | 'i' | 'u', ..] => Some(2),
                // Tên trong chỗ trống ngắn: chỉ dò `PLACEHOLDER_MAX` ký tự, không thì
                // `":{:{:{…"` 4 MiB làm phép dò thành bậc hai (review claude-security F2).
                ['%', '(', ..] => rest
                    .iter()
                    .take(PLACEHOLDER_MAX + 3)
                    .position(|&c| c == ')')
                    .filter(|&e| rest.get(e + 1).is_some_and(|c| c.is_ascii_alphabetic()))
                    .map(|e| e + 2),
                ['{', ..] => rest[1..]
                    .iter()
                    .take(PLACEHOLDER_MAX + 1)
                    .position(|&c| !(c.is_ascii_alphanumeric() || c == '_'))
                    .filter(|&e| rest[1 + e] == '}')
                    .map(|e| e + 2),
                [] => Some(0),
                ['/', next, ..] if *next != '/' => Some(0),
                ['/'] => Some(0),
                _ => None,
            };
            if let Some(n) = placeholder {
                // `"db:" + port` thì không chắc: `"API:"` là một nhãn.
                if n > 0 {
                    let held: String = rest[..n].iter().collect::<String>().to_ascii_lowercase();
                    let port = matches!(held.as_str(), "%d" | "%i" | "%u") || held.contains("port");
                    if port {
                        stripped = Stripped::Port;
                    } else if stripped == Stripped::None {
                        stripped = Stripped::Any;
                    }
                }
                i += 1 + n;
                continue;
            }
        }
        s.push(c);
        i += 1;
    }
    // `db:5432/x` không có `scheme://`: bỏ đường dẫn sau cổng.
    if !s.contains("://") {
        let mut t = String::with_capacity(s.len());
        let cs: Vec<char> = s.chars().collect();
        let mut i = 0;
        while i < cs.len() {
            t.push(cs[i]);
            if cs[i] == ':' {
                let n = cs[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
                if n > 0 && cs.get(i + 1 + n) == Some(&'/') {
                    t.extend(&cs[i + 1..i + 1 + n]);
                    i += 1 + n;
                    while i < cs.len() && !cs[i].is_whitespace() {
                        i += 1;
                    }
                    continue;
                }
            }
            i += 1;
        }
        s = t;
    }
    // `tcp(db:3306)` của DSN Go MySQL.
    let mut extra = String::new();
    let mut from = 0;
    while let Some(at) = s[from..].find("tcp(") {
        let start = from + at + 4;
        let Some(end) = s[start..].find(')') else { break };
        extra.push(' ');
        extra.push_str(&s[start..start + end]);
        from = start + end;
    }
    // URL nhiều máy.
    if s.contains("://") {
        let mut out = String::with_capacity(s.len());
        for part in s.split_inclusive(char::is_whitespace) {
            let Some((scheme, after)) = part.split_once("://") else {
                out.push_str(part);
                continue;
            };
            let end = after.find(['/', '?', '#']).unwrap_or(after.len());
            let (authority, path) = after.split_at(end);
            let (user, hosts) = match authority.rfind('@') {
                Some(at) => authority.split_at(at + 1),
                None => ("", authority),
            };
            // Mỗi máy chép lại cả scheme, user, path: `"<M chữ>://<N dấu phẩy>"`
            // ra N·M byte. URL thật có vài máy; nhiều hơn trần thì không tách
            // (review opus của bản sửa F2).
            if !hosts.contains(',') || hosts.matches(',').count() >= MAX_URL_HOSTS {
                out.push_str(part);
                continue;
            }
            for h in hosts.split(',') {
                out.push_str(scheme);
                out.push_str("://");
                out.push_str(user);
                out.push_str(h);
                out.push_str(path.trim_end());
                out.push(' ');
            }
        }
        s = out;
    }
    s.push_str(&extra);
    (s, stripped)
}

/// File test và mock: không chạy trong service nào mà hay có địa chỉ để thử
/// (TM-35). Thư mục ví dụ, tài liệu, fixture thì vẫn xét: mã chạy thật có thể
/// import chúng, và một ứng dụng có thể tên `docs`.
pub fn skipped_path(path: &str) -> bool {
    super::grpc::fake_server_path(path)
}

/// Khoá mà [`compose::hosts_in`] hiểu, theo loại của tên trong mã: `TOKEN`
/// (chỉ đọc URL), `HOST` (đọc cả chuỗi trần), hay rỗng (URL và `máy:cổng`).
fn kind(name: &str) -> &'static str {
    let parts = words(name);
    // Khoá bí mật, và khoá mà `a:b` là thứ khác: `image="redis:7"`.
    let other = |p: &String| {
        compose::SECRET_PARTS.contains(&p.as_str()) || CODE_SECRETS.contains(&p.as_str()) || LABELS.contains(&p.as_str())
    };
    let hostish = parts.last().is_some_and(|p| HOSTISH.contains(&p.as_str()));
    if parts.iter().any(|p| INBOUND.contains(&p.as_str())) {
        // Máy được phép gọi vào, hay địa chỉ mình nghe: không phải cạnh đi ra.
        "INBOUND"
    } else if parts.iter().any(other) && !hostish {
        // `auth_server`, `PRIVATE_DB_HOST`: đoạn cuối nói đây là máy, dù trước
        // đó có từ chỉ bí mật.
        "TOKEN"
    } else if parts.last().is_some_and(|p| HOSTISH.contains(&p.as_str())) || parts == ["DATA", "SOURCE"] {
        "HOST"
    } else {
        ""
    }
}

/// `redisHost`, `REDIS_HOST`, `redis-host`, `Data Source` → các từ viết hoa.
fn words(name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_ascii_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
            continue;
        }
        if c.is_ascii_uppercase() && prev_lower && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        cur.push(c.to_ascii_uppercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Tên mà chuỗi ở vị trí `i` là giá trị của:
///
/// - `host="db"`, `host = "db"`, `host := "db"`, `host: "db"`, `{"host": "db"}`;
/// - `getenv("DB_HOST", "db")`, `environ.get("DB_HOST", "db")`;
/// - `process.env.DB_HOST || "db"`, `?? "db"`, `os.getenv("DB_HOST") or "db"`.
///
/// So sánh (`==`, `!=`, `<=`) và `+=` không phải gán.
/// Go: tên đứng trước kiểu (`addr string = "…"`). Python: `tên: "…"` là chú
/// thích kiểu, dict thật có khoá là chuỗi.
fn context(toks: &[Token], i: usize, lang: Lang) -> Option<String> {
    let go = lang == Lang::Go;
    // `x: "Kiểu" = …`: chuỗi là chú thích kiểu, không phải giá trị.
    let annotation = matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Punct('=')))
        && !matches!(toks.get(i + 2).map(|t| &t.tok), Some(Tok::Punct('=')));
    // Tiền tố của chuỗi (`f"…"`, `rb"…"`, `L"…"`) là một từ đứng liền trước nó.
    let prefix = |w: &str| {
        matches!(
            w.to_ascii_lowercase().as_str(),
            "f" | "r" | "b" | "u" | "rb" | "br" | "rf" | "fr" | "t" | "rt" | "tr" | "l" | "u8"
        )
    };
    let mut i = match i.checked_sub(1).map(|j| &toks[j].tok) {
        Some(Tok::Word(w)) if prefix(w) => i - 1,
        _ => i,
    };
    // Ngoặc nhóm ngay sau `=`, `:`: `"url": (⏎ "http://…"⏎ )`. Chỉ khi trong
    // ngoặc toàn là chuỗi: `("host", 1234)` là tuple, không phải giá trị của khoá.
    let grouped = || {
        let mut j = i + 1;
        while j < toks.len() && j <= i + 64 {
            match &toks[j].tok {
                Tok::Punct(')') => return true,
                Tok::Str(_) | Tok::Template(_) | Tok::Word(_) => j += 1,
                _ => return false,
            }
        }
        false
    };
    if i >= 2 && toks[i - 1].tok == Tok::Punct('(') && matches!(toks[i - 2].tok, Tok::Punct('=' | ':')) && grouped() {
        i -= 1;
    }
    let at = |k: usize| -> Option<&Tok> { i.checked_sub(k).map(|j| &toks[j].tok) };
    // Lexer không giữ xuống dòng: `if not host:⏎ msg = "…"` trông như gán có
    // kiểu. Phần kiểu phải nằm cùng dòng với `=`.
    let line = |k: usize| i.checked_sub(k).map(|j| toks[j].line);
    let same_line = |k: usize| line(k).is_some() && line(k) == line(1);
    let name = |t: Option<&Tok>| -> Option<String> {
        match t? {
            Tok::Word(w) => Some(last_segment(w).to_string()),
            Tok::Str(s) => Some(s.clone()),
            _ => None,
        }
    };
    // Vế trước một toán tử "hoặc": tên biến môi trường, hay `(...)`, `[...]`
    // kết thúc bằng tên biến môi trường.
    let before = |k: usize| -> Option<String> {
        match at(k)? {
            // `process.env.DB_HOST || …`; biến thường (`x or "a" in y`) thì không.
            Tok::Word(w) if envish(last_segment(w)) => Some(last_segment(w).to_string()),
            Tok::Punct(')' | ']') => match at(k + 1)? {
                Tok::Str(s) if envish(s) => Some(s.clone()),
                Tok::Word(w) if envish(last_segment(w)) => Some(last_segment(w).to_string()),
                _ => None,
            },
            _ => None,
        }
    };
    let getter = |k: usize| {
        matches!(at(k), Some(Tok::Word(w)) if GETTERS.contains(&last_segment(w))
            || (lang == Lang::Java && JAVA_SETTERS.contains(&last_segment(w))))
    };
    // Tên được gán, khi `at(k)` là dấu `=` của phép gán: `host = …`,
    // `host: str = …`, `host := …`.
    let assigned = |k: usize| -> Option<String> {
        if at(k) != Some(&Tok::Punct('=')) || matches!(at(k + 1), Some(Tok::Punct('=' | '!' | '<' | '>' | '+' | '-' | '*' | '/'))) {
            return None;
        }
        match (at(k + 1)?, at(k + 2), at(k + 3)) {
            (Tok::Punct(':'), Some(Tok::Word(_)), _) => name(at(k + 2)),
            (Tok::Word(_), Some(Tok::Punct(':')), Some(Tok::Word(_))) => name(at(k + 3)),
            (Tok::Word(_), _, _) => name(at(k + 1)),
            _ => None,
        }
    };
    // Phép gán bao quanh, cùng dòng và cùng tầng ngoặc: `host = x or "db"`,
    // `host = x if c else "db"`, `host = Field("db")`.
    // Lời gọi bao quanh vị trí `from`: chỉ số của `(` của nó.
    let open_call = |from: usize| -> Option<usize> {
        let mut depth = 0usize;
        for k in from..from + 64 {
            match at(k)? {
                Tok::Punct(')' | ']' | '}') => depth += 1,
                Tok::Punct('(') if depth == 0 => return Some(k),
                Tok::Punct('(' | '[' | '{') if depth == 0 => return None,
                Tok::Punct('(' | '[' | '{') => depth -= 1,
                _ => {}
            }
        }
        None
    };
    let enclosing = |from: usize| -> Option<String> {
        let mut depth = 0usize;
        for k in from..from + 64 {
            if line(k)? != line(1)? {
                return None;
            }
            match at(k)? {
                Tok::Punct(')' | ']' | '}') => depth += 1,
                Tok::Punct('(' | '[' | '{') if depth == 0 => return None,
                Tok::Punct('(' | '[' | '{') => depth -= 1,
                Tok::Punct('=') if depth == 0 => return assigned(k),
                _ => {}
            }
        }
        None
    };
    // Đối số đầu của lời gọi hàm đọc cấu hình bao quanh vị trí `from`:
    // `getenv("DB_HOST", default="db")` thì `default` là giá trị của `DB_HOST`.
    let call_key = |from: usize| -> Option<String> {
        let mut depth = 0usize;
        for k in from..from + 64 {
            match at(k)? {
                Tok::Punct(')' | ']' | '}') => depth += 1,
                Tok::Punct('(') if depth == 0 => {
                    return match at(k - 1) {
                        Some(Tok::Str(s)) if getter(k + 1) && keyish(s) => Some(s.clone()),
                        _ => None,
                    };
                }
                Tok::Punct('(' | '[' | '{') if depth == 0 => return None,
                Tok::Punct('(' | '[' | '{') => depth -= 1,
                _ => {}
            }
        }
        None
    };
    let direct = match at(1)? {
        Tok::Punct('=') => match at(2) {
            Some(Tok::Punct('=' | '!' | '<' | '>' | '+' | '-' | '*' | '/' | '|' | '&')) => None,
            // `host := "db"` của Go.
            Some(Tok::Punct(':')) => name(at(3)),
            // `char host[] = "db"`, `char host[64] = "db"` của C.
            Some(Tok::Punct(']')) => {
                let mut k = 3;
                while k < 12 && matches!(at(k), Some(Tok::Word(_)) | Some(Tok::Punct('0'..='9' | '+' | '*'))) {
                    k += 1;
                }
                if at(k) == Some(&Tok::Punct('[')) { name(at(k + 1)) } else { None }
            }
            // Có kiểu: `host: str = …`, `let host: &str = …`, `host string = …`
            // của Go (tên trước kiểu); `std::string host = …` thì tên sau kiểu.
            Some(Tok::Word(_)) => match (at(3), at(4)) {
                (Some(Tok::Punct(':')), _) if same_line(4) => name(at(4)),
                (Some(Tok::Punct('&')), Some(Tok::Punct(':'))) if same_line(5) => name(at(5)),
                // `char host[64] = "db"` của C.
                (Some(Tok::Punct('[')), Some(Tok::Word(_))) if !go => name(at(4)),
                (Some(Tok::Word(w)), _) if go && same_line(3) && w != "var" && w != "const" => name(at(3)),
                // Kiểu có tên gói của Go: `addr featuregate.Feature = …`.
                (Some(Tok::Punct('.')), Some(Tok::Word(_))) if go => {
                    let mut k = 3;
                    while at(k) == Some(&Tok::Punct('.')) && matches!(at(k + 1), Some(Tok::Word(_))) {
                        k += 2;
                    }
                    if matches!(at(k), Some(Tok::Word(_))) && same_line(k) { name(at(k)) } else { name(at(2)) }
                }
                _ => name(at(2)),
            },
            t => name(t),
        },
        Tok::Punct(':') => match at(2)? {
            // `::` của Rust, C++.
            Tok::Punct(':') => None,
            _ if annotation => None,
            Tok::Word(_) if lang == Lang::Python => None,
            t => name(Some(t)),
        },
        // Tên biến đứng ngay trước, không phải giá trị trong dict
        // (`{"k": "DB_URL", "x.y": …}`).
        // Chỉ đối số đầu của một lời gọi hàm: `getenv("DB_HOST", "db")`. Trong
        // danh sách tên biến (`("HTTP_HOST", "HTTP_REFERER")`) thì không.
        Tok::Punct(',') => match (at(2)?, at(3)) {
            (Tok::Str(s), Some(Tok::Punct('('))) if envish(s) && matches!(at(4), Some(Tok::Word(w)) if !KEYWORDS.contains(&w.as_str())) => {
                Some(s.clone())
            }
            // `cfg.get("host", "db")`, `environ.get("db_host", "db")`.
            (Tok::Str(s), Some(Tok::Punct('('))) if getter(4) && keyish(s) => Some(s.clone()),
            _ => None,
        },
        Tok::Punct('|') if matches!(at(2), Some(Tok::Punct('|'))) => before(3).or_else(|| enclosing(3)),
        Tok::Punct('?') if matches!(at(2), Some(Tok::Punct('?'))) => before(3).or_else(|| enclosing(3)),
        Tok::Word(w) if w == "or" => before(2).or_else(|| enclosing(2)),
        Tok::Word(w) if w == "else" && lang == Lang::Python => enclosing(2),
        // Đối số đầu của một lời gọi.
        Tok::Punct('(') => match at(2) {
            // `net.JoinHostPort("db", …)`, `gethostbyname("db")`.
            Some(Tok::Word(w)) if HOST_FUNCS.contains(&last_segment(w)) => Some("host".to_string()),
            Some(Tok::Word(w)) if lang == Lang::Java && JAVA_HOST_CTORS.contains(&last_segment(w)) => Some("host".to_string()),
            // Java: builder, setter có tên là khoá máy (`.host("redis")`,
            // `setHost("rabbit")`, `withEndpoint(…)`).
            Some(Tok::Word(w)) if lang == Lang::Java && kind(last_segment(w)) == "HOST" => Some(last_segment(w).to_string()),
            _ => None,
        },
        // `std::string host{"db"}` của C++.
        Tok::Punct('{') if lang == Lang::C => match at(2) {
            Some(Tok::Word(w)) if !matches!(w.as_str(), "return" | "else") => name(at(2)),
            _ => None,
        },
        _ => None,
    };
    match direct.as_deref() {
        // `getenv("DB_HOST", default="db")`; `host: str = field(default="db")`.
        Some("default" | "fallback" | "defaultValue" | "default_value") if at(1) == Some(&Tok::Punct('=')) => {
            call_key(3).or_else(|| open_call(3).filter(|&k| matches!(at(k + 1), Some(Tok::Word(_)))).and_then(|k| enclosing(k + 2)))
        }
        Some(_) => direct,
        // Danh sách ở khoá số nhiều: `HOSTS = ["a", "db"]`, `servers=("db",)`.
        // Chỉ khi cả danh sách là chuỗi: `("host", 1234)` là một cặp.
        None => {
            let mut j = i + 1;
            loop {
                match &toks.get(j)?.tok {
                    Tok::Punct(',') | Tok::Str(_) | Tok::Template(_) if j < i + 256 => j += 1,
                    Tok::Punct(']' | ')') => break,
                    Tok::Punct('}') if go => break,
                    _ => return None,
                }
            }
            let mut k = 1;
            loop {
                match at(k)? {
                    Tok::Punct(',') | Tok::Str(_) | Tok::Template(_) if k < 256 => k += 1,
                    Tok::Punct('[' | '(') => break,
                    Tok::Punct('{') if go => break,
                    _ => return None,
                }
            }
            // Go: `hosts := []string{"a", "db"}`, bỏ qua phần kiểu.
            let mut m = k + 1;
            if go && at(k) == Some(&Tok::Punct('{')) {
                while m < k + 16 && matches!(at(m), Some(Tok::Word(_)) | Some(Tok::Punct('[' | ']' | '.' | '*'))) {
                    m += 1;
                }
            }
            let key = match (at(m)?, at(m + 1)) {
                (Tok::Punct('='), Some(Tok::Punct(':'))) => name(at(m + 2)),
                (Tok::Punct('='), t) if !matches!(t, Some(Tok::Punct('=' | '!' | '<' | '>'))) => name(t),
                (Tok::Punct(':'), Some(Tok::Str(s))) => Some(s.clone()),
                (Tok::Punct(':'), Some(Tok::Word(w))) if lang != Lang::Python => Some(last_segment(w).to_string()),
                _ => None,
            }?;
            matches!(kind(&key), "HOST" | "INBOUND").then_some(key)
        }
    }
}

/// `("db", 5432)`: cặp địa chỉ trong ngoặc tròn, là đối số (`connect((…))`)
/// hay giá trị được gán (`addr = (…)`); chuỗi ở `i`, dấu phẩy ở `j`. Trả cổng.
/// Phần tử của một danh sách cặp (`[("light", 300), ("book", 380)]`) thì không.
fn address_port(toks: &[Token], i: usize, j: usize) -> Option<String> {
    let before = |k: usize| i.checked_sub(k).map(|x| &toks[x].tok);
    if before(1) != Some(&Tok::Punct('(')) || !matches!(before(2), Some(Tok::Punct('(' | '='))) {
        return None;
    }
    // Sau `=`: chỉ khi tên được gán là kiểu máy (`addr = ("db", 5432)`), không
    // phải `CACHE = ("cache", 3600)`.
    if before(2) == Some(&Tok::Punct('=')) && !matches!(before(3), Some(Tok::Word(w)) if kind(last_segment(w)) == "HOST") {
        return None;
    }
    if toks.get(j)?.tok != Tok::Punct(',') {
        return None;
    }
    let mut port = String::new();
    let mut k = j + 1;
    while let Some(Tok::Punct(c @ '0'..='9')) = toks.get(k).map(|t| &t.tok) {
        port.push(*c);
        k += 1;
    }
    (!port.is_empty() && port.len() <= 5 && toks.get(k)?.tok == Tok::Punct(')')).then_some(port)
}

/// Chuỗi ba nháy đứng ở chỗ một giá trị: sau `(`, `,`, `=` (bỏ qua tiền tố
/// `r`, `f`…). Docstring thì không.
fn value_position(toks: &[Token], i: usize) -> bool {
    let mut j = i;
    if let Some(Tok::Word(w)) = j.checked_sub(1).map(|k| &toks[k].tok)
        && w.len() <= 2
        && w.chars().all(|c| "rRbBuUfFtT".contains(c))
    {
        j -= 1;
    }
    matches!(j.checked_sub(1).map(|k| &toks[k].tok), Some(Tok::Punct('(' | ',' | '=')))
}

/// `${khoá:mặc định}` của Spring, cả chuỗi: `(khoá, mặc định)`.
fn spring_default(s: &str) -> Option<(String, String)> {
    let inner = s.strip_prefix("${")?.strip_suffix('}')?;
    let (k, v) = inner.split_once(':')?;
    (keyish(k) && !v.contains(['{', '}', '$'])).then(|| (k.to_string(), v.to_string()))
}

/// Tên khoá cấu hình: chữ, số, `_`, `.`, `-`.
fn keyish(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'))
}

/// `os.environ.get` → `get`, `self.host` → `host`, `cfg::ADDR` → `ADDR`.
fn last_segment(w: &str) -> &str {
    w.rsplit(['.', ':']).next().unwrap_or(w)
}

/// Trông như tên biến môi trường: chữ hoa, số, `_`, có chữ cái.
fn envish(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'_')
        && s.bytes().any(|c| c.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(lang: Lang, src: &str) -> Vec<String> {
        scan(&lang.tokens(src), lang).hosts.iter().map(|h| format!("{}:{}", h.line, h.shown())).collect()
    }

    #[test]
    fn url_va_may_cong_o_dau_cung_tinh() {
        assert_eq!(found(Lang::Python, "x = requests.get('http://result:80/api')"), ["1:chuỗi → result:80"]);
        assert_eq!(found(Lang::Python, "connect('redis:6379')"), ["1:chuỗi → redis:6379"]);
        assert_eq!(
            found(Lang::TypeScript, "const c = new Pool({ connectionString: 'postgres://postgres:postgres@db/postgres' })"),
            ["1:connectionString → db"]
        );
        // `@` trong đường dẫn không phải ranh giới mật khẩu.
        assert_eq!(found(Lang::TypeScript, "a = 'https://youtube.com/@excalidraw'"), ["1:a → youtube.com"]);
        // Mật khẩu không bao giờ ra ngoài.
        let all = found(Lang::TypeScript, "u = 'postgres://admin:s3cret@db:5432/x'").join(" ");
        assert!(!all.contains("s3cret") && !all.contains("admin"), "{all}");
    }

    #[test]
    fn chuoi_tran_chi_tinh_o_khoa_kieu_may() {
        assert_eq!(found(Lang::Python, "g.redis = Redis(host=\"redis\", db=0)"), ["1:host → redis"]);
        assert_eq!(found(Lang::Python, "cfg = {\"hostname\": \"db\"}"), ["1:hostname → db"]);
        assert_eq!(found(Lang::Go, "opt := redis.Options{Addr: \"redis\"}\nredisHost := \"cache\""), ["1:Addr → redis", "2:redisHost → cache"]);
        assert_eq!(found(Lang::Rust, "let server = \"db\";"), ["1:server → db"]);
        // Java: khai báo có kiểu, trường, lời gọi đọc cấu hình, annotation; số
        // đọc trọn (`digit_separators`) không làm lệch khoá.
        assert_eq!(found(Lang::Java, "private static final String DB_HOST = \"db\";"), ["1:DB_HOST → db"]);
        assert_eq!(found(Lang::Java, "this.host = \"cache\"; hosts[0] = \"queue\";"), ["1:host → cache", "1:hosts → queue"]);
        assert_eq!(
            found(Lang::Java, "String a = System.getenv().getOrDefault(\"REDIS_ADDR\", \"redis:6379\");"),
            ["1:REDIS_ADDR → redis:6379"]
        );
        assert_eq!(found(Lang::Java, "@Endpoint(host = \"metrics\", port = 9090)"), ["1:host → metrics"]);
        assert_eq!(
            found(Lang::Java, "RedisClient.builder().host(\"redis\"); cf.setHost(\"rabbit\"); new Jedis(\"redis3\", 6379);"),
            ["1:host → redis", "1:setHost → rabbit", "1:host → redis3"]
        );
        assert_eq!(
            found(Lang::Java, "props.setProperty(\"db.host\", \"pghost\"); m.put(\"host\", \"mapped\"); log.info(\"cart\");"),
            ["1:db.host → pghost", "1:host → mapped"]
        );
        assert_eq!(found(Lang::Java, "@Value(\"${redis.host:redis}\") String h;"), ["1:redis.host → redis"]);
        // `":{:{:{…"` dài không làm phép dò chỗ trống thành bậc hai (claude-security F2).
        let many = format!("x = \"{}://{}\"", "a".repeat(40_000), ",".repeat(40_000));
        let t = crate::testclock::Clock::start();
        let _ = found(Lang::Python, &many);
        assert!(t.elapsed().as_secs() < 5);
        let big = format!("x = \"{}\"", ":{".repeat(1_000_000));
        let t = crate::testclock::Clock::start();
        let _ = found(Lang::Python, &big);
        assert!(t.elapsed().as_secs() < 5);
        assert_eq!(found(Lang::Python, &format!("u = \"http://db:{{{}}}/x\"", "p".repeat(64))), ["1:u → db"]);
        // Khoá bí mật không đọc, chỉ in tên và cổng.
        assert_eq!(found(Lang::Java, "String pw = \"http://user:hunter2@vault:8200\";"), ["1:pw → vault:8200"]);
        // Có kiểu.
        assert_eq!(found(Lang::Python, "host: str = \"db\""), ["1:host → db"]);
        assert_eq!(found(Lang::TypeScript, "let host: string = 'db'"), ["1:host → db"]);
        assert_eq!(found(Lang::Rust, "let host: &str = \"db\";"), ["1:host → db"]);
        assert_eq!(found(Lang::Go, "var host string = \"db\""), ["1:host → db"]);
        assert_eq!(
            found(Lang::Go, "const (\n\tSetMemberLocalAddr featuregate.Feature = \"x\"\n\tDbHost Feature = \"db\"\n)"),
            ["2:SetMemberLocalAddr → x", "3:DbHost → db"]
        );
        // Không có xuống dòng giữa khối và lệnh gán sau nó.
        assert!(found(Lang::Python, "if not hostname:\n    msg = 'missing hostname'").is_empty());
        // Ngoặc nhóm không làm mất khoá; tuple thì không phải giá trị của khoá.
        assert_eq!(found(Lang::Python, "d = {'host': (\n  'redis'\n)}"), ["2:host → redis"]);
        // Cặp `(máy, cổng)` là một địa chỉ; tuple khác thì không phải giá trị của khoá.
        assert_eq!(found(Lang::Python, "addr: Addr = ('host', 1234)\nd = {'X_URL': ('x_url', y)}"), ["1:chuỗi → host:1234"]);
        assert!(found(Lang::Python, "def f(tool_server: 'ToolServer | None' = None): pass").is_empty());
        assert!(found(Lang::Python, "def f(tool_server: 'ToolServer | None', x): pass").is_empty());
        assert_eq!(found(Lang::TypeScript, "const o = {host: 'db', port: 1}"), ["1:host → db"]);
        // `x or "..."` với biến thường không phải giá trị mặc định.
        assert!(found(Lang::Python, "if not endpoint or '.mtls.' in e: pass").is_empty());
        assert_eq!(found(Lang::TypeScript, "const host = 'db'"), ["1:host → db"]);
        assert_eq!(found(Lang::C, "const char *host = \"db\";"), ["1:host → db"]);
        // Không phải khoá kiểu máy, hay là so sánh, hay là nhãn.
        assert!(found(Lang::Python, "name = \"db\"\nif host == \"db\": pass\nservice = \"cart\"\nOpen(\"redis\")").is_empty());
        assert!(found(Lang::Rust, "let x = Kind::Host(\"db\");").is_empty());
    }

    #[test]
    fn mac_dinh_cua_bien_moi_truong() {
        assert_eq!(found(Lang::Python, "h = os.getenv(\"REDIS_HOST\", \"redis\")"), ["1:REDIS_HOST → redis"]);
        assert_eq!(found(Lang::Python, "h = os.environ.get('DB_ADDR') or 'db'"), ["1:DB_ADDR → db"]);
        assert_eq!(found(Lang::TypeScript, "const h = process.env.DB_HOST || 'db'"), ["1:DB_HOST → db"]);
        assert_eq!(found(Lang::TypeScript, "const h = process.env['DB_HOST'] ?? 'db'"), ["1:DB_HOST → db"]);
        // Danh sách tên biến môi trường không phải giá trị mặc định.
        assert!(found(Lang::Python, "SENSITIVE = ('HTTP_HOST', 'HTTP_REFERER')\nf(metavar=('URL', 'bootstrap_port'))").is_empty());
        assert!(found(Lang::Python, "for k in ('OPENAI_BASE_URL', 'OPENAI_API_BASE'): pass\nprint('API:')").is_empty());
        // Giá trị trong dict không phải tên biến cho chuỗi sau nó.
        assert!(found(Lang::Python, "M = {'a.b': 'X_BASE_URL',\n 'c.d': 'Y'}").is_empty());
        // Tên biến không phải kiểu máy: chỉ là một lựa chọn.
        assert!(found(Lang::Python, "os.getenv('OPTION_A', 'Cats')").is_empty());
    }

    #[test]
    fn chuoi_ket_noi_va_khoa_bi_mat() {
        assert_eq!(
            found(Lang::Rust, "let c = \"Server=db;Username=postgres;Password=db;\";"),
            ["1:c → db"]
        );
        assert_eq!(found(Lang::Python, "c = 'Data Source=sql,1433;Initial Catalog=x'"), ["1:c → sql"]);
        // Khoá bí mật: chỉ URL. `password = "db:5432"` có thể là chính mật khẩu.
        assert!(found(Lang::Python, "password = 'db:5432'\napi_key = 'cache'").is_empty());
        assert_eq!(found(Lang::Python, "auth_url = 'http://auth:8000'"), ["1:auth_url → auth:8000"]);
    }

    #[test]
    fn ten_ghep_luc_chay_khong_doc() {
        assert!(found(Lang::TypeScript, "fetch(`http://${svc}:80`)\nfetch(`http://api-${env}:80`)").is_empty());
        assert_eq!(found(Lang::TypeScript, "fetch(`http://payment:80/${id}`)"), ["1:chuỗi → payment:80"]);
        assert!(found(Lang::Python, "url = f'http://{host}:8000'").is_empty());
        // Chuỗi ba nháy (docstring) không giữ nội dung.
        assert!(found(Lang::Python, "\"\"\"Gọi http://api:8000\"\"\"").is_empty());
    }

    #[test]
    fn ky_tu_thoat_chuoi_tho_va_f_string() {
        // `\n` là xuống dòng, không phải chữ `n`.
        assert_eq!(found(Lang::Python, "x = 'db:5432\\n'"), ["1:x → db:5432"]);
        assert_eq!(found(Lang::Go, "x := \"cart:7070\\n\""), ["1:x → cart:7070"]);
        // Chuỗi thô và thoát lạ của Python giữ `\`: đây là regex, không phải máy.
        assert!(found(Lang::Python, "re.match(r'https://github\\.com/(.*)', u)").is_empty());
        assert!(found(Lang::Python, "re.match('https://github\\.com/(.*)', u)").is_empty());
        // f-string: khoá là thứ đứng trước chuỗi, không phải mã trong `{...}`.
        assert_eq!(found(Lang::Python, "d = {'url': f'http://cart:7070/{path}'}"), ["1:url → cart:7070"]);
        assert!(found(Lang::Python, "d = {'url': f'data:{t};base64,{x}'}").is_empty());
        assert!(found(Lang::Python, "data_uri = 'data:image/png;base64,{}'.format(b)").is_empty());
    }

    fn names(lang: Lang, src: &str) -> Vec<String> {
        scan(&lang.tokens(src), lang).hosts.iter().map(|h| h.host.shown()).collect()
    }

    #[test]
    fn cong_ghep_luc_chay_van_giu_ten_may() {
        for (lang, src) in [
            (Lang::Python, "u = f'http://db:{settings.PORT}/api'"),
            (Lang::Python, "u = 'http://db:%d/api' % p"),
            (Lang::Python, "u = 'http://db:%(port)s/api' % d"),
            (Lang::Python, "u = 'http://db:{}/api'.format(p)"),
            (Lang::Python, "u = 'http://db:' + str(p)"),
            (Lang::Python, "u = 'db:%d' % p"),
            (Lang::Python, "u = 'db:{port}'.format(port=p)"),
            (Lang::Python, "db_host = f'db:{p}'"),
            (Lang::TypeScript, "const u = `http://db:${port}/x`"),
            (Lang::Go, "u := fmt.Sprintf(\"http://db:%d/api\", p)"),
        ] {
            assert_eq!(names(lang, src), ["db"], "{src}");
        }
        // Tên ghép lúc chạy thì không thành tên nào.
        assert!(names(Lang::Python, "u = f'http://worker{i}:8000'\nv = f'http://{env}db:5432'").is_empty());
    }

    #[test]
    fn cac_dang_viet_khac_van_doc_duoc() {
        let one = |lang: Lang, src: &str, want: &[&str]| assert_eq!(names(lang, src), want, "{src}");
        one(Lang::Go, "var dbHost = \"db\"\nconst cacheAddr = \"cache\"", &["db", "cache"]);
        one(Lang::Python, "connect(\"\"\"http://db:5432/x\"\"\")", &["db:5432"]);
        one(Lang::Python, "def f():\n    \"\"\"Gọi http://db:5432 để thử.\"\"\"", &[]);
        one(Lang::C, "#define DB_URL \"http://db:5432\"\n#define DB_HOST \"cache\"\n#define F(x) \"q:8080\"", &["db:5432", "cache"]);
        one(Lang::C, "char host[] = \"db\";\nchar addr[64] = \"cache\";", &["db", "cache"]);
        one(Lang::Go, "sql.Open(\"postgres\", \"host=db port=5432 user=x password=hunter2 sslmode=disable\")", &["db"]);
        one(Lang::Python, "c = 'db:5432;sslmode=require'", &["db:5432"]);
        one(Lang::Python, "c = 'Server=tcp:sql,1433;Database=x'\nd = 'Server=db\\\\SQLEXPRESS;'", &["sql", "db"]);
        one(Lang::Python, "u = 'mongodb://mongo1:27017,db:5432/app'", &["mongo1:27017", "db:5432"]);
        one(Lang::Go, "dsn := \"root:pw@tcp(db:3306)/app\"", &["db:3306"]);
        one(Lang::Python, "u = 'http://d\\x62:5432'\nv = 'http://\\u0063ache:6379'", &["db:5432", "cache:6379"]);
        one(Lang::TypeScript, "const u = `db:5432\\n`", &["db:5432"]);
        one(Lang::Python, "HOSTS = ['a', 'db']\nservers = ('cache',)", &["a", "db", "cache"]);
        one(Lang::Go, "hosts := []string{\"a\", \"db\"}\nx := []string{\"cache\"}", &["a", "db"]);
        one(Lang::Python, "connect(r'''http://db:5432/x''')", &["db:5432"]);
        one(Lang::Python, "h = os.getenv('DB_HOST', default='db')", &["db"]);
        one(Lang::Python, "h = cfg.get('host', 'db')\ni = os.environ.get('cache_host', 'cache')", &["db", "cache"]);
        one(Lang::TypeScript, "const h = (process.env.DB_HOST) || 'db'", &["db"]);
        one(Lang::Python, "u = 'redis://hunt/er2@redis:6379'", &["redis:6379", "hunt"]);
        // Chữ nhiều byte ở mọi chỗ cắt chuỗi: không panic.
        one(Lang::Go, "x := \"a=b’c;d=’’’;Server=’;’=’\"", &[]);
    }

    #[test]
    fn them_cac_dang_lach() {
        let one = |lang: Lang, src: &str, want: &[&str]| assert_eq!(names(lang, src), want, "{src}");
        one(Lang::Python, "requests.get('http://' 'db:5432/x')\nrequests.get('http://d' 'b:5432/x')", &["db:5432", "db:5432"]);
        one(Lang::C, "const char *u = \"http://\" \"db:5432\";", &["db:5432"]);
        one(Lang::Python, "u = 'db:5432/x'", &["db:5432"]);
        one(Lang::Python, "s.connect(('db', 5432))\naddr = ('cache', 6379)\nprint('Count', 100)", &["db:5432", "cache:6379"]);
        one(Lang::Python, "W = [\n  ('light', 300),\n  ('book', 380),\n]", &[]);
        one(Lang::Python, "host = x or 'db'\ndb_host = x if x else 'cache'", &["db", "cache"]);
        one(Lang::Python, "class C:\n    host: str = field(default='db')", &["db"]);
        one(Lang::Go, "a := net.JoinHostPort(\"db\", port)", &["db"]);
        one(Lang::Python, "auth_server = 'db:5432'\nPRIVATE_DB_HOST = 'cache'", &["db:5432", "cache"]);
        one(Lang::Python, "host = ' db '\nh = {'host': 'cache/'}", &["db", "cache"]);
        one(Lang::C, "std::string host{\"db\"};", &["db"]);
        one(Lang::Python, "db_host = os.getenv('DB_HOST', 'db')", &["db"]);
        // Không phải gán: lời gọi trên dòng riêng, `else` trong đối số.
        one(Lang::Python, "print('db')\nf(x if c else 'db')", &[]);
    }

    #[test]
    fn khong_bao_nham() {
        for (lang, src) in [
            (Lang::Python, "c = Container(image='redis:7')"),
            (Lang::Python, "r.set('worker:1', 1)"),
            (Lang::Python, "HELP = {'host': 'Hostname of the redis server'}"),
            (Lang::Python, "x = b64encode(b'admin:839201')"),
            (Lang::Python, "ALLOWED_HOSTS = ['api', 'db']"),
            (Lang::Python, "TAG = 'postgres:16'"),
            (Lang::Python, "print('host', 'db')"),
            // Chỗ giữ chỗ không phải cổng: khoá Redis, log, image, mật khẩu.
            (Lang::Python, "r.get(f'cache:{key}')"),
            (Lang::Python, "r.get('login:%s' % uid)"),
            (Lang::Python, "r.get('cache:{}'.format(k))"),
            (Lang::Python, "log.error('db:%s', e)"),
            (Lang::Python, "c.run(image=f'db:{v}')"),
            (Lang::Python, "password = f'db:{u}'"),
            (Lang::TypeScript, "r.get(`cache:${id}`)"),
            (Lang::Go, "k := fmt.Sprintf(\"cache:%s\", id)"),
            (Lang::Python, "CACHE = ('cache', 3600)"),
            // Tên route ở khoá URL.
            (Lang::Python, "@app.route('/login', endpoint='login')\nLOGIN_URL = 'login'\nSTATIC_URL = 'cache/'"),
            // Khoá chiều vào.
            (Lang::Python, "CORS_ALLOWED_ORIGINS = ['http://login:3000']\nLISTEN_ADDR = 'login:8080'"),
        ] {
            assert!(names(lang, src).is_empty(), "{src}: {:?}", names(lang, src));
        }
    }

    #[test]
    fn f_string_long_sau_khong_tran_stack() {
        let deep = format!("x = f\"{}\"", "{f\"".repeat(150_000));
        let t = crate::testclock::Clock::start();
        let _ = names(Lang::Python, &deep);
        let spec = format!("x = f\"{}\"", "{x:".repeat(150_000));
        let _ = names(Lang::Python, &spec);
        assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    }

    #[test]
    fn file_doi_khang_van_nhanh() {
        let many: String = (0..200_000).map(|i| format!("h{i} = 'h{i}:8080'\n")).collect();
        let t = crate::testclock::Clock::start();
        let r = scan(&Lang::Python.tokens(&many), Lang::Python);
        assert!(r.cut);
        assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    }

    #[test]
    fn chuoi_dai_va_qua_nhieu_ten_co_tran() {
        let long = format!("x = 'http://pay:80/{}'", "a".repeat(100_000));
        assert_eq!(found(Lang::Python, &long), ["1:x → pay:80"]);
        let many: String = (0..5000).map(|i| format!("h{i} = 'n{i}:8080'\n")).collect();
        let r = scan(&Lang::Python.tokens(&many), Lang::Python);
        assert_eq!(r.hosts.len(), MAX_PER_FILE);
        assert!(r.cut);
    }
}
