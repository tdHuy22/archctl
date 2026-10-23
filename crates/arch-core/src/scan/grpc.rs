//! Cạnh gRPC: lời gọi giữa hai phần mã không import nhau.
//!
//! ```text
//! // client                              // server
//! stub = rpc_pb2_grpc.KVStub(channel)    rpc_pb2_grpc.add_KVServicer_to_server(impl, s)
//! kv := pb.NewKVClient(conn)             pb.RegisterKVServer(s, impl)
//! ```
//!
//! Hai phía chỉ cùng import mã sinh ra từ một file `.proto`, nên đồ thị import
//! không thấy lời gọi. Ở đây dò chỗ gọi stub và chỗ đăng ký server theo token,
//! rồi [`ProtoIndex`] ghép chúng theo tên đầy đủ `package.Service` đọc từ
//! `.proto`: lần từ module sinh ra (`rpc_pb2_grpc` từ `rpc.proto`, import path
//! của Go là `option go_package`) về file `.proto`; không lần được thì theo tên
//! service, khi tên đó là duy nhất.
//!
//! Chỉ tính chỗ **gọi** stub và chỗ **đăng ký** server, không tính định nghĩa
//! trong mã sinh ra, lớp `XServicer` chỉ được kế thừa, gateway
//! (`RegisterXHandlerServer`), health và reflection.
//!
//! Không nhận ra (ghi trong `docs/infra.md`): stub gán vào biến rồi gọi qua
//! biến, `getattr`, reflection, client dựng động.

use super::Lang;
use super::lexer::{self, CharLit, LexConfig, Tok, Token};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Trần cho một file `.proto` (TM-13). File thật có vài chục service.
pub const MAX_SERVICES: usize = 1000;
pub const MAX_RPCS: usize = 20_000;

/// Những gì cần từ một file `.proto`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Proto {
    pub package: String,
    /// `option go_package = "path;name"`, chỉ phần đường dẫn.
    pub go_package: Option<String>,
    /// Tên package Go: phần `name` của `go_package`, không có thì đoạn cuối
    /// của đường dẫn.
    pub go_name: Option<String>,
    /// `option java_package = "com.example.shop"`.
    pub java_package: Option<String>,
    pub services: Vec<Service>,
    /// Quá trần thì phần sau không đọc.
    pub truncated: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub line: usize,
    /// Tên các `rpc`, kèm dòng.
    pub rpcs: Vec<(String, usize)>,
}

fn proto_config() -> LexConfig {
    LexConfig { char_literals: CharLit::AsString, path_separator: Some("."), ..LexConfig::base() }
}

/// Đọc `package`, `option go_package`, `service` và `rpc` ở cấp ngoài cùng.
/// Một lượt qua dòng token, đếm ngoặc, không đệ quy (TM-13).
pub fn read_proto(src: &str) -> Proto {
    let toks = lexer::lex(crate::strip_bom(src), &proto_config());
    let mut out = Proto::default();
    let word = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let punct = |i: usize, c: char| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c);
    let mut depth = 0usize;
    // Service đang đọc và độ sâu của ngoặc mở nó.
    let mut open: Option<usize> = None;
    let mut rpcs = 0usize;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i].tok {
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => {
                depth = depth.saturating_sub(1);
                if open.is_some_and(|d| depth < d) {
                    open = None;
                }
            }
            Tok::Word(w) if depth == 0 && w == "package" => {
                if let Some(name) = word(i + 1) {
                    out.package = name.to_string();
                }
            }
            Tok::Word(w) if depth == 0 && w == "option" && word(i + 1) == Some("go_package") && punct(i + 2, '=') => {
                if let Some(Tok::Str(v)) = toks.get(i + 3).map(|t| &t.tok) {
                    let (path, name) = v.split_once(';').unwrap_or((v, ""));
                    out.go_package = Some(path.to_string());
                    let name = if name.is_empty() { path.rsplit('/').next().unwrap_or(path) } else { name };
                    out.go_name = Some(name.to_string());
                }
            }
            Tok::Word(w) if depth == 0 && w == "option" && word(i + 1) == Some("java_package") && punct(i + 2, '=') => {
                if let Some(Tok::Str(v)) = toks.get(i + 3).map(|t| &t.tok) {
                    out.java_package = Some(v.clone());
                }
            }
            Tok::Word(w) if depth == 0 && w == "service" && punct(i + 2, '{') => {
                if let Some(name) = word(i + 1) {
                    if out.services.len() >= MAX_SERVICES {
                        out.truncated = true;
                        break;
                    }
                    out.services.push(Service { name: name.to_string(), line: toks[i].line, rpcs: Vec::new() });
                    open = Some(depth + 1);
                }
            }
            Tok::Word(w) if w == "rpc" && open == Some(depth) && punct(i + 2, '(') => {
                if let (Some(name), Some(s)) = (word(i + 1), out.services.last_mut()) {
                    if rpcs >= MAX_RPCS {
                        out.truncated = true;
                        break;
                    }
                    rpcs += 1;
                    s.rpcs.push((name.to_string(), toks[i].line));
                }
            }
            _ => {}
        }
        i += 1;
    }
    out
}

/// Thứ cho biết service thuộc file `.proto` nào.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    /// Không biết module (`self.pb.KVStub`, `KVStub` re-export qua
    /// `__init__.py`): chỉ tin khi một `.proto` trong repo khai service này.
    Unknown,
    /// Chỗ đăng ký Python không rõ module: `add_KVServicer_to_server` đủ đặc
    /// trưng để là gRPC, ghép theo tên.
    Loose,
    /// Python: đường dẫn module sinh ra, bỏ `_pb2_grpc`: `shop.v1.kv`,
    /// `..gen.kv`, `kv`. Đoạn cuối là tên file `.proto`.
    Stem(String),
    /// Go: import path của package sinh ra, khớp `option go_package`.
    GoPackage(String),
    /// Go: tên đứng trước `.NewXClient` không phải alias import nào trong
    /// file (package có tên khác đoạn cuối đường dẫn, như `orderv1` của
    /// `…/order/v1;orderv1`), hay package của chính file khi gọi không tiền
    /// tố: khớp tên package trong `go_package`.
    GoName(String),
    /// Tên đầy đủ viết thẳng trong chuỗi: `"/shop.v1.KV/Get"` của client
    /// generic, `"shop.v1.KV"` của `method_handlers_generic_handler`. Giữ
    /// package.
    Package(String),
    /// Node: chuỗi thuộc tính sau đối tượng nạp `.proto` (`shop.v1` trong
    /// `new p.shop.v1.KV`). Gốc của chuỗi có thể là biến gán giữa chừng
    /// (`const shop = load(def).shop`), nên chuỗi chỉ là đuôi của package:
    /// khớp package trong `.proto` bằng nó hay kết thúc bằng `.` + nó.
    Suffix(String),
    /// Java: package Java của lớp `XGrpc` sinh ra (tên đầy đủ
    /// `hipstershop.AdServiceGrpc`, câu `import`, hay `package` của chính
    /// file), khớp `option java_package` của `.proto`, không có thì `package`.
    JavaPackage(String),
    /// Rust (tonic): tên struct viết theo UpperCamel của tên service
    /// (`KV` → `KvServer`), ghép không phân biệt hoa thường với `.proto`. Như
    /// Go, chỉ tin khi một `.proto` trong repo khai service đó.
    Rust,
}

/// Một chỗ gọi stub hay đăng ký server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub service: String,
    pub origin: Origin,
    pub line: usize,
    /// Như mã viết, để báo cáo.
    pub shown: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rpc {
    /// Chỗ dùng stub: phía client. Tính cả chỗ nhắc tới mà không gọi
    /// (`S = kv_pb2_grpc.KVStub`), vì gọi qua biến thì không thấy.
    pub calls: Vec<Site>,
    /// Chỗ đăng ký server: `add_KVServicer_to_server`, `RegisterKVServer`,
    /// `KV_ServiceDesc`, `method_handlers_generic_handler("pkg.KV", …)`. Tính
    /// cả chỗ nhắc tới mà không gọi (`reg = g.add_KVServicer_to_server`).
    pub serves: Vec<Site>,
    /// Chỗ cài đặt server: lớp kế thừa `KVServicer`, struct nhúng
    /// `UnimplementedKVServer`. Có cài đặt mà không thấy chỗ đăng ký nào thì
    /// phía server đang bị giấu, và `check` nói ra (TM-12).
    pub implements: Vec<Site>,
    /// Java: tên lớp của từng chỗ cài đặt, cùng thứ tự với `implements`
    /// (rỗng với lớp vô danh và ngôn ngữ khác).
    pub java_classes: Vec<String>,
    /// Java: lớp được đăng ký trong file (`addService(new AdServiceImpl())`).
    /// Lớp cài đặt và chỗ đăng ký thường ở hai file; `check` ghép theo tên lớp.
    pub java_registered: Vec<String>,
}

impl Rpc {
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty() && self.serves.is_empty() && self.implements.is_empty()
    }
}

/// Service hạ tầng của chính gRPC, không phải service của sản phẩm.
fn infrastructure(module: &str, service: &str) -> bool {
    let last = module.rsplit(['.', '/']).next().unwrap_or(module);
    matches!(last, "health" | "reflection" | "channelz")
        || matches!(service, "ServerReflection" | "Channelz")
        || module.contains("grpc_health")
        || module.contains("grpc_reflection")
        || module.contains("grpc.health")
        || module.contains("grpc.reflection")
}

/// File stub sinh ra: chứa định nghĩa của chính `KVStub`, `add_KVServicer_to_server`
/// và chuỗi `"/pkg.KV/Get"`, không phải chỗ dùng.
pub fn generated_stub_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.ends_with("_pb2_grpc.py")
        || name.ends_with("_grpc.pb.go")
        || name.ends_with("_grpc_pb.js")
        || name.ends_with("_grpc_pb.ts")
        || name.ends_with("_grpc_pb.d.ts")
        || name.ends_with(".grpc.pb.h")
        || name.ends_with(".grpc.pb.cc")
}

pub fn scan(toks: &[Token], lang: Lang) -> Rpc {
    match lang {
        Lang::Python => python(toks),
        Lang::Go => go(toks),
        Lang::TypeScript => node(toks),
        Lang::Rust => rust(toks),
        Lang::C => cpp(toks),
        Lang::Java => java(toks),
    }
}

/// Tên để so không phân biệt hoa thường và gạch dưới: `KV`, `Kv`, `kv`.
pub fn norm(service: &str) -> String {
    service.chars().filter(|c| *c != '_').flat_map(char::to_lowercase).collect()
}

/// Service hạ tầng của gRPC theo tên: health check, reflection.
fn infrastructure_name(service: &str) -> bool {
    matches!(service, "Health" | "ServerReflection" | "Channelz")
}

/// Node: `@grpc/grpc-js` nạp `.proto` lúc chạy qua `@grpc/proto-loader`
/// (`server.addService(proto.pkg.X.service, impl)`, `new proto.pkg.X(addr,
/// creds)`), hay mã sinh của grpc-tools (`addService(svc.XService, impl)`,
/// `new svc.XClient(addr, creds)`). Tên đi qua đối tượng nạp lúc chạy nên
/// không lần được về module; như Go, chỉ tin khi `.proto` trong repo khai
/// service đó; chuỗi thuộc tính (`proto.shop.v1.KV`) thu hẹp theo package.
fn node(toks: &[Token]) -> Rpc {
    let word = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let punct = |i: usize, c: char| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c);
    // `a.b.C` bắt đầu ở `start`: các chữ, và chỉ số của chữ cuối.
    let chain = |start: usize| -> (Vec<&str>, usize) {
        let mut out = Vec::new();
        let mut k = start;
        while let Some(w) = word(k) {
            out.push(w);
            if punct(k + 1, '.') && word(k + 2).is_some() {
                k += 2;
            } else {
                break;
            }
        }
        (out, k)
    };
    let upper = |s: &str| s.chars().next().is_some_and(char::is_uppercase);
    let mut out = Rpc::default();
    for (i, tok) in toks.iter().enumerate() {
        match word(i) {
            Some("addService") if punct(i + 1, '(') => {
                let (c, _) = chain(i + 2);
                let n = c.len();
                // `proto.pkg.X.service` (nạp lúc chạy) hay `svc.XService` (mã sinh).
                let (service, pkg) = if n >= 2 && c[n - 1] == "service" && upper(c[n - 2]) {
                    (c[n - 2].to_string(), (n >= 4).then(|| c[1..n - 2].join(".")))
                } else if let Some(s) = c.last().and_then(|w| w.strip_suffix("Service")).filter(|s| upper(s)) {
                    (s.to_string(), None)
                } else {
                    continue;
                };
                if infrastructure_name(&service) {
                    continue;
                }
                let origin = pkg.map_or(Origin::Unknown, Origin::Suffix);
                out.serves.push(Site { service, origin, line: tok.line, shown: format!("addService({})", c.join(".")) });
            }
            Some("new") => {
                let (c, last) = chain(i + 1);
                let n = c.len();
                if n == 0 || !punct(last + 1, '(') {
                    continue;
                }
                // `new svc.XClient(…)`, `new XClient(…)` (mã sinh), hay
                // `new proto.pkg.X(…)` (nạp lúc chạy; một chữ trần thì không:
                // `new Foo()` là lớp thường).
                let (service, pkg) = if let Some(s) = c[n - 1].strip_suffix("Client").filter(|s| upper(s)) {
                    (s.to_string(), None)
                } else if n >= 2 && upper(c[n - 1]) {
                    (c[n - 1].to_string(), (n >= 3).then(|| c[1..n - 1].join(".")))
                } else {
                    continue;
                };
                if infrastructure_name(&service) {
                    continue;
                }
                let origin = pkg.map_or(Origin::Unknown, Origin::Suffix);
                out.calls.push(Site { service, origin, line: tok.line, shown: format!("new {}", c.join(".")) });
            }
            _ => {}
        }
    }
    out
}

/// Rust (tonic): module sinh ra `x_server`, `x_client` theo snake_case của tên
/// service, struct `XServer`, `XClient` theo UpperCamel. Tính cả chỗ nhắc tới
/// (`use pb::kv_server::KvServer`, `use pb::kv_server::{Kv, KvServer}` rồi
/// `KvServer::new`), như Python, Go. `impl Kv for S` với `Kv` lấy từ
/// `kv_server` là chỗ cài đặt server. Mã trong `#[cfg(test)] mod` là server
/// giả. File sinh ra (khai `mod x_server` và dùng `tonic::codegen`) chứa định
/// nghĩa, không phải chỗ dùng.
fn rust(toks: &[Token]) -> Rpc {
    let word = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let punct = |i: usize, c: char| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c);
    // Lexer gộp `a::b::C` thành một từ.
    let segs = |i: usize| word(i).map(|w| w.split("::").filter(|s| !s.is_empty()).collect::<Vec<_>>()).unwrap_or_default();
    let generated = (0..toks.len()).any(|i| segs(i).windows(2).any(|w| w == ["tonic", "codegen"]))
        && (0..toks.len()).any(|i| {
            word(i) == Some("mod") && word(i + 1).is_some_and(|m| m.ends_with("_server") || m.ends_with("_client"))
        });
    if generated {
        return Rpc::default();
    }
    // Token nằm trong `#[cfg(test)] mod … { … }`.
    let mut in_test = vec![false; toks.len()];
    let mut i = 0;
    while i < toks.len() {
        let attr = punct(i, '#') && punct(i + 1, '[') && word(i + 2) == Some("cfg") && punct(i + 3, '(') && word(i + 4) == Some("test");
        if !attr {
            i += 1;
            continue;
        }
        let Some(open) = (i..toks.len().min(i + 64)).find(|&j| punct(j, '{')) else { break };
        if !(i..open).any(|j| word(j) == Some("mod")) {
            i = open;
            continue;
        }
        let mut depth = 0usize;
        let mut j = open;
        while j < toks.len() {
            if punct(j, '{') {
                depth += 1;
            } else if punct(j, '}') {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            in_test[j] = true;
            j += 1;
        }
        i = j + 1;
    }
    // Module và struct phải cùng tên service: `kv_server::KvServer`.
    let same = |prefix: &str, service: &str| norm(prefix) == norm(service);
    // Tên mà `use` lấy từ `x_server`, `x_client`: tên trong file → (vai, service).
    #[derive(Clone, Copy, PartialEq)]
    enum Role {
        Serve,
        Implement,
        Call,
    }
    let classify = |module: &str, item: &str| -> Option<(Role, String)> {
        if let Some(prefix) = module.strip_suffix("_server") {
            if let Some(service) = item.strip_suffix("Server").filter(|s| same(prefix, s)) {
                return Some((Role::Serve, service.to_string()));
            }
            if same(prefix, item) {
                return Some((Role::Implement, item.to_string()));
            }
        } else if let Some(prefix) = module.strip_suffix("_client")
            && let Some(service) = item.strip_suffix("Client").filter(|s| same(prefix, s))
        {
            return Some((Role::Call, service.to_string()));
        }
        None
    };
    let mut imported: HashMap<String, (Role, String)> = HashMap::new();
    for (name, path) in super::rustlang::uses(toks) {
        let parts: Vec<&str> = path.split("::").collect();
        if let [.., module, item] = parts.as_slice()
            && let Some(found) = classify(module, item)
        {
            imported.insert(name, found);
        }
    }
    let mut out = Rpc::default();
    for (i, tok) in toks.iter().enumerate() {
        if in_test[i] {
            continue;
        }
        let parts = segs(i);
        let mut found: Vec<(Role, String, String)> = Vec::new();
        for pair in parts.windows(2) {
            if let Some((role, service)) = classify(pair[0], pair[1]) {
                // `impl pb::kv_server::Kv for S`: chỉ khi `Kv` là đoạn cuối.
                if role != Role::Implement || (parts.last() == Some(&pair[1]) && word(i + 1) == Some("for")) {
                    found.push((role, service, format!("{}::{}", pair[0], pair[1])));
                }
            }
        }
        // Tên đã `use`: `KvServer::new(…)`, `impl Kv for S`.
        if let Some(first) = parts.first()
            && let Some((role, service)) = imported.get(*first)
            && (*role != Role::Implement || (parts.len() == 1 && word(i + 1) == Some("for")))
        {
            found.push((*role, service.clone(), (*first).to_string()));
        }
        for (role, service, shown) in found {
            if infrastructure_name(&service) {
                continue;
            }
            let site = Site { service, origin: Origin::Rust, line: tok.line, shown };
            match role {
                Role::Serve => out.serves.push(site),
                Role::Implement => out.implements.push(site),
                Role::Call => out.calls.push(site),
            }
        }
    }
    out
}

/// C++: `Greeter::NewStub(channel)` (client); lớp kế thừa `Greeter::Service`,
/// `AsyncService`, `CallbackService` (cài đặt), và `RegisterService(&impl)`
/// của `ServerBuilder` trong cùng file (đăng ký: tham số là biến, không mang
/// tên service, nên tính cho mọi service cài đặt trong file). Namespace đứng
/// trước tên service (`helloworld::Greeter`) là package của `.proto`, viết
/// bằng `::`; không có namespace thì như Go, chỉ tin khi `.proto` trong repo
/// khai service đó.
fn cpp(toks: &[Token]) -> Rpc {
    let word = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let punct = |i: usize, c: char| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c);
    // Chữ đứng trước `::` ở `i`: `a::b::X` với `i` là chỉ số của `X`.
    let qualifier = |i: usize| -> (Vec<&str>, usize) {
        let mut parts = Vec::new();
        let mut j = i;
        // Trần 16 đoạn namespace (TM-13).
        // Từ khoá đứng trước `::` toàn cục (`return ::pkg::X`) không phải namespace.
        let keyword = |w: Option<&str>| {
            matches!(w, Some("return" | "co_return" | "public" | "protected" | "private" | "virtual" | "new" | "throw" | "case" | "else" | "auto"))
        };
        while parts.len() < 16 && j >= 3 && punct(j - 1, ':') && punct(j - 2, ':') && word(j - 3).is_some() && !keyword(word(j - 3)) {
            parts.push(word(j - 3).unwrap_or(""));
            j -= 3;
        }
        parts.reverse();
        (parts, j)
    };
    let origin = |ns: &[&str]| {
        let ns: Vec<&str> = ns.iter().copied().filter(|s| !s.is_empty() && *s != "std").collect();
        if ns.is_empty() { Origin::Unknown } else { Origin::Suffix(ns.join(".")) }
    };
    let mut out = Rpc::default();
    let mut implemented: Vec<(String, Origin)> = Vec::new();
    let mut register_line = None;
    for (i, tok) in toks.iter().enumerate() {
        let line = tok.line;
        match word(i) {
            Some("NewStub") if punct(i + 1, '(') => {
                let (q, _) = qualifier(i);
                if let [ns @ .., service] = q.as_slice()
                    && service.starts_with(|c: char| c.is_ascii_uppercase())
                    && !infrastructure_name(service)
                    && !ns.iter().any(|n| matches!(*n, "health" | "reflection"))
                {
                    let shown = format!("{}::NewStub", q.join("::"));
                    out.calls.push(Site { service: (*service).to_string(), origin: origin(ns), line, shown });
                }
            }
            Some("Service" | "AsyncService" | "CallbackService" | "StreamedUnaryService" | "SplitStreamedService" | "StreamedService") => {
                let (q, start) = qualifier(i);
                // Chỉ trong danh sách lớp cha: `: public X::Service`, `, X::Service`.
                let base = start >= 1
                    && (matches!(word(start - 1), Some("public" | "protected" | "private" | "virtual"))
                        || punct(start - 1, ':')
                        || punct(start - 1, ','));
                // `: public grpc::Service` là lớp cha chung, không phải service.
                if let [ns @ .., service] = q.as_slice()
                    && base
                    && service.starts_with(|c: char| c.is_ascii_uppercase())
                    && !infrastructure_name(service)
                {
                    let shown = format!("{}::{}", q.join("::"), word(i).unwrap_or(""));
                    out.implements.push(Site { service: (*service).to_string(), origin: origin(ns), line, shown });
                    implemented.push(((*service).to_string(), origin(ns)));
                }
            }
            Some("RegisterService") if punct(i + 1, '(') && register_line.is_none() => register_line = Some(line),
            _ => {}
        }
    }
    if let Some(line) = register_line {
        implemented.sort();
        implemented.dedup();
        for (service, origin) in implemented {
            out.serves.push(Site { service, origin, line, shown: "RegisterService(…)".into() });
        }
    }
    out
}

/// Java (grpc-java): lớp sinh ra tên `XGrpc` cho service `X`.
/// - Phía gọi: `XGrpc.newBlockingStub(ch)`, `newStub`, `newFutureStub`,
///   `newBlockingV2Stub`.
/// - Phía cài đặt: lớp `extends XGrpc.XImplBase`, hay `implements
///   XGrpc.AsyncService`.
/// - Phía đăng ký: `.addService(` trong cùng file, như C++.
///
/// Package Java của `XGrpc`: tên đầy đủ viết thẳng, câu `import`, hay
/// `package` của chính file; ghép với `java_package` của `.proto`. Như Go,
/// chỉ tin khi một `.proto` trong repo khai service đó.
fn java(toks: &[Token]) -> Rpc {
    let word = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let punct = |i: usize, c: char| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c);
    let mut file_pkg: Option<String> = None;
    // Lớp `XGrpc` nhập vào file → package của nó; phương thức nhập tĩnh →
    // (lớp, package).
    let mut imported: HashMap<String, String> = HashMap::new();
    let mut static_methods: HashMap<String, (String, String)> = HashMap::new();
    // `import static a.b.XGrpc.*;`: lớp `XGrpc` nhập mọi phương thức.
    let mut static_wild: Vec<(String, String)> = Vec::new();
    // Mọi lớp nhập (`import a.b.CartImpl;`, lớp lồng `import a.b.Outer.Inner;`)
    // → package, cho tên lớp được đăng ký; `import a.b.*;` → package `a.b`.
    let mut classes: HashMap<String, String> = HashMap::new();
    let mut wild_pkgs: Vec<String> = Vec::new();
    // Package là các đoạn đầu viết thường của một tên đầy đủ.
    let pkg_of = |segs: &[&str]| -> String {
        segs.iter().take_while(|s| s.starts_with(|c: char| c.is_ascii_lowercase())).copied().collect::<Vec<_>>().join(".")
    };
    for i in 0..toks.len() {
        match word(i) {
            Some("package") if file_pkg.is_none() => file_pkg = word(i + 1).map(str::to_string),
            Some("import") => {
                let (is_static, at) = if word(i + 1) == Some("static") { (true, i + 2) } else { (false, i + 1) };
                let Some(path) = word(at) else { continue };
                let segs: Vec<&str> = path.split('.').collect();
                // `a.b.XGrpc.*`: lexer gộp `a.b.XGrpc`, rồi `.` `*`.
                if is_static && punct(at + 1, '.') && punct(at + 2, '*') {
                    if let [pkg @ .., class] = segs.as_slice()
                        && class.ends_with("Grpc")
                    {
                        static_wild.push(((*class).to_string(), pkg.join(".")));
                    }
                    continue;
                }
                if !is_static && punct(at + 1, '.') && punct(at + 2, '*') {
                    wild_pkgs.push(pkg_of(&segs));
                    continue;
                }
                if !is_static && let Some(class) = segs.last() {
                    classes.insert((*class).to_string(), pkg_of(&segs));
                }
                if is_static {
                    if let [pkg @ .., class, method] = segs.as_slice()
                        && class.ends_with("Grpc")
                    {
                        static_methods.insert((*method).to_string(), ((*class).to_string(), pkg.join(".")));
                    }
                } else if let [pkg @ .., class] = segs.as_slice()
                    && class.ends_with("Grpc")
                {
                    imported.insert((*class).to_string(), pkg.join("."));
                }
            }
            _ => {}
        }
    }
    // `a.b.XGrpc` → (`X`, nguồn gốc). `XGrpc` không tiền tố: theo `import`,
    // rồi `package` của file.
    let service_of = |path: &[&str]| -> Option<(String, Origin)> {
        let [pkg @ .., class] = path else { return None };
        let service = class.strip_suffix("Grpc").filter(|s| s.starts_with(|c: char| c.is_ascii_uppercase()))?;
        if infrastructure_name(service) || pkg.iter().any(|p| matches!(*p, "health" | "reflection" | "channelz")) {
            return None;
        }
        let pkg = if pkg.is_empty() { imported.get(*class).cloned().or_else(|| file_pkg.clone()) } else { Some(pkg.join(".")) };
        let origin = pkg.filter(|p| !p.is_empty()).map_or(Origin::Unknown, Origin::JavaPackage);
        Some((service.to_string(), origin))
    };
    // Lớp được nhắc tới (`CartImpl`, `shop.CartImpl`, `Outer.Inner`) → các khoá
    // `package:Lớp` có thể: package viết thẳng (đoạn đầu viết thường), hay theo
    // `import` của lớp (hay lớp ngoài); không có thì `package` của file và mọi
    // package nhập bằng `*`.
    let class_keys = |name: &str| -> Vec<String> {
        let segs: Vec<&str> = name.split('.').collect();
        let last = segs.last().copied().unwrap_or("");
        let pkg = pkg_of(&segs);
        if !pkg.is_empty() {
            return vec![format!("{pkg}:{last}")];
        }
        if let Some(p) = classes.get(segs[0]) {
            return vec![format!("{p}:{last}")];
        }
        let mut keys = vec![format!("{}:{last}", file_pkg.clone().unwrap_or_default())];
        keys.extend(wild_pkgs.iter().map(|p| format!("{p}:{last}")));
        keys
    };
    // Ngoặc đóng của mỗi `(` và ngoặc mở của mỗi `)`, tính một lần cho cả file
    // (TM-69).
    let mut close = vec![usize::MAX; toks.len()];
    let mut open = vec![usize::MAX; toks.len()];
    let mut stack: Vec<usize> = Vec::new();
    for (j, t) in toks.iter().enumerate() {
        match t.tok {
            Tok::Punct('(') => stack.push(j),
            Tok::Punct(')') => {
                if let Some(o) = stack.pop() {
                    close[o] = j;
                    open[j] = o;
                }
            }
            _ => {}
        }
    }
    // `extends`, `implements` của khai báo lớp (`class`, `enum`, `record R(int
    // x)`), không của cận kiểu tham số `<S extends X>`, không của `interface`
    // (interface không cài đặt server; như tree-sitter, nó không có
    // `superclass`).
    let declares = |i: usize| -> bool {
        let mut angle = 0i32;
        let mut k = i;
        while k > 0 && i - k < 256 {
            k -= 1;
            match &toks[k].tok {
                Tok::Punct('>') => angle += 1,
                Tok::Punct('<') if angle == 0 => return false,
                Tok::Punct('<') => angle -= 1,
                // Thành phần của `record R(int x)`: nhảy qua.
                Tok::Punct(')') if open[k] != usize::MAX => k = open[k],
                Tok::Punct('{' | '}' | ';' | '(' | ')') => return false,
                Tok::Word(w) if angle == 0 && w == "interface" => return false,
                Tok::Word(w) if angle == 0 && matches!(w.as_str(), "class" | "enum" | "record") => return true,
                _ => {}
            }
        }
        false
    };
    const STUBS: &[&str] = &["newStub", "newBlockingStub", "newFutureStub", "newBlockingV2Stub"];
    let mut out = Rpc::default();
    let mut implemented: Vec<(String, Origin)> = Vec::new();
    let mut register_line = None;
    // Đang trong mệnh đề `implements A, B` (tới `{`): dấu phẩy ngăn các kiểu.
    let mut in_implements = false;
    for (i, tok) in toks.iter().enumerate() {
        let line = tok.line;
        if punct(i, '{') {
            in_implements = false;
        }
        let Some(w) = word(i) else { continue };
        if w == "implements" {
            in_implements = true;
        }
        // `XGrpc\n    .newBlockingStub(ch)`: tên lớp và phương thức là hai từ.
        let joined;
        let w = if i >= 2 && punct(i - 1, '.') && STUBS.contains(&w) && word(i - 2).is_some() {
            joined = format!("{}.{w}", word(i - 2).unwrap_or(""));
            joined.as_str()
        } else {
            w
        };
        let segs: Vec<&str> = w.split('.').collect();
        let Some((last, head)) = segs.split_last() else { continue };
        if STUBS.contains(last) && punct(i + 1, '(') {
            let found = if head.is_empty() && i > 0 && punct(i - 1, '.') {
                // `getStub().newBlockingStub(…)`: phương thức của một đối tượng.
                None
            } else if head.is_empty() && !static_methods.contains_key(*last) {
                // `import static a.XGrpc.*;`: chỉ một lớp thì là lớp đó.
                match static_wild.as_slice() {
                    [(class, pkg)] => {
                        let mut p: Vec<&str> = if pkg.is_empty() { Vec::new() } else { pkg.split('.').collect() };
                        p.push(class);
                        service_of(&p)
                    }
                    _ => None,
                }
            } else if head.is_empty() {
                static_methods.get(*last).and_then(|(class, pkg)| {
                    let mut p: Vec<&str> = if pkg.is_empty() { Vec::new() } else { pkg.split('.').collect() };
                    p.push(class);
                    service_of(&p)
                })
            } else {
                service_of(head)
            };
            if let Some((service, origin)) = found {
                out.calls.push(Site { service, origin, line, shown: w.to_string() });
            }
            continue;
        }
        // `extends XGrpc.XImplBase`, `implements XGrpc.AsyncService`, và lớp vô
        // danh `new XGrpc.XImplBase() { … }`.
        let anonymous = i > 0 && word(i - 1) == Some("new") && punct(i + 1, '(');
        let base = i > 0
            && ((matches!(word(i - 1), Some("extends" | "implements")) && declares(i - 1)) || (in_implements && punct(i - 1, ',')));
        if (base || anonymous)
            && let Some((service, origin)) = service_of(head)
            && (*last == "AsyncService" || last.strip_suffix("ImplBase") == Some(service.as_str()))
        {
            out.implements.push(Site { service: service.clone(), origin: origin.clone(), line, shown: w.to_string() });
            out.java_classes.push(if anonymous {
                String::new()
            } else {
                let c = class_before(toks, i);
                if c.is_empty() { c } else { format!("{}:{c}", file_pkg.clone().unwrap_or_default()) }
            });
            implemented.push((service, origin));
            continue;
        }
        // `server.addService(…)`, `ServerBuilder.forPort(p).addService(…)`:
        // phương thức của một đối tượng, lần đầu trong file.
        if *last == "addService" && punct(i + 1, '(') && (!head.is_empty() || (i > 0 && punct(i - 1, '.'))) {
            if register_line.is_none() {
                register_line = Some(line);
            }
            // `addService(new AdServiceImpl())`, `addService(ServerInterceptors
            // .intercept(new X(), …))`: lớp được đăng ký. Trong ngoặc của chính
            // lời gọi, tối đa 256 token (TM-69).
            let end = close[i + 1].min(i + 1 + 256).min(toks.len());
            for k in i + 2..end {
                if word(k) == Some("new")
                    && let Some(c) = word(k + 1)
                    && out.java_registered.len() < 1_000
                {
                    out.java_registered.extend(class_keys(c));
                }
            }
        }
    }
    if let Some(line) = register_line {
        implemented.sort();
        implemented.dedup();
        for (service, origin) in implemented {
            out.serves.push(Site { service, origin, line, shown: "addService(…)".into() });
        }
    }
    out
}

/// Tên lớp đang khai mà phần đầu chứa vị trí `i`: từ ngay sau `class` gần
/// nhất phía trước, trong cùng phần đầu (chưa gặp `{`, `}`, `;`).
fn class_before(toks: &[Token], i: usize) -> String {
    let mut k = i;
    while k > 0 && i - k < 256 {
        k -= 1;
        match &toks[k].tok {
            Tok::Punct('{' | '}' | ';') => break,
            Tok::Word(w) if matches!(w.as_str(), "class" | "enum" | "record") => {
                if let Some(Tok::Word(n)) = toks.get(k + 1).map(|t| &t.tok) {
                    return n.clone();
                }
                break;
            }
            _ => {}
        }
    }
    String::new()
}

/// `rpc_pb2_grpc` → `rpc`.
fn generated_stem(module: &str) -> Option<&str> {
    module.rsplit('.').next()?.strip_suffix("_pb2_grpc").filter(|s| !s.is_empty())
}

/// `a.b.rpc_pb2_grpc` → `a.b.rpc`.
fn generated_module(module: &str) -> Option<String> {
    generated_stem(module)?;
    module.strip_suffix("_pb2_grpc").map(str::to_string)
}

/// `"shop.v1.KV"` hay `"/shop.v1.KV/Get"` → (package, service).
fn full_name(s: &str) -> Option<(String, String)> {
    let s = s.strip_prefix('/').map_or(s, |r| r.split('/').next().unwrap_or(r));
    let (pkg, service) = s.rsplit_once('.')?;
    let ident = |x: &str| !x.is_empty() && x.split('.').all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_alphanumeric() || c == '_'));
    (ident(pkg) && ident(service) && service.chars().next().is_some_and(char::is_uppercase))
        .then(|| (pkg.to_string(), service.to_string()))
}

fn python(toks: &[Token]) -> Rpc {
    let word = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let string = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Str(s)) => Some(s.as_str()),
        _ => None,
    };
    let punct = |i: usize, c: char| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c);
    // Tên trong file → module sinh ra: `import a.rpc_pb2_grpc as g`, `from a
    // import rpc_pb2_grpc`. Và tên trong file → (module, tên gốc): `from
    // a.rpc_pb2_grpc import KVStub as S`.
    let mut modules: HashMap<String, String> = HashMap::new();
    let mut names: HashMap<String, (String, String)> = HashMap::new();
    // `from a.rpc_pb2_grpc import *`: tên trần `KVStub` có thể đến từ đây.
    let mut star: Vec<String> = Vec::new();
    // Token nằm trong câu import: tên ở đó là khai báo, không phải chỗ dùng.
    let mut in_import = vec![false; toks.len()];
    // Bỏ `\` nối dòng nếu lexer để lại nó.
    let skip = |mut j: usize| {
        while punct(j, '\\') {
            j += 1;
        }
        j
    };
    let mut i = 0;
    while i < toks.len() {
        let at_stmt = i == 0 || toks[i - 1].line != toks[i].line || punct(i - 1, ';');
        let begin = i;
        match word(i) {
            Some("import") if at_stmt => {
                // import a.b as c, d
                let mut j = skip(i + 1);
                while let Some(m) = word(j) {
                    let (alias, next) = if word(skip(j + 1)) == Some("as") {
                        (word(skip(skip(j + 1) + 1)).unwrap_or(m), skip(skip(j + 1) + 1) + 1)
                    } else {
                        (m, j + 1)
                    };
                    if generated_stem(m).is_some() {
                        modules.insert(alias.to_string(), m.to_string());
                    }
                    j = skip(next);
                    if !punct(j, ',') {
                        break;
                    }
                    j = skip(j + 1);
                }
                in_import[begin..j.min(toks.len())].iter_mut().for_each(|x| *x = true);
                i = j;
                continue;
            }
            Some("from") if at_stmt => {
                // `from a.b`, `from .a`, `from ..a.b`, `from .`: dấu chấm đứng
                // đầu là import tương đối, lexer tách chúng thành dấu câu.
                let mut j = i + 1;
                let mut base = String::new();
                while punct(j, '.') {
                    base.push('.');
                    j += 1;
                }
                if let Some(w) = word(j).filter(|w| *w != "import") {
                    base.push_str(w);
                    j += 1;
                }
                j = skip(j);
                if base.is_empty() || word(j) != Some("import") {
                    i += 1;
                    continue;
                }
                j = skip(j + 1);
                if punct(j, '*') {
                    if generated_stem(&base).is_some() {
                        star.push(base);
                    }
                    in_import[begin..=j].iter_mut().for_each(|x| *x = true);
                    i = j + 1;
                    continue;
                }
                let paren = punct(j, '(');
                if paren {
                    j = skip(j + 1);
                }
                while let Some(n) = word(j) {
                    let (alias, next) = if word(skip(j + 1)) == Some("as") {
                        (word(skip(skip(j + 1) + 1)).unwrap_or(n), skip(skip(j + 1) + 1) + 1)
                    } else {
                        (n, j + 1)
                    };
                    let full = if base.ends_with('.') { format!("{base}{n}") } else { format!("{base}.{n}") };
                    if generated_stem(n).is_some() {
                        modules.insert(alias.to_string(), full);
                    } else if generated_stem(&base).is_some() {
                        names.insert(alias.to_string(), (base.clone(), n.to_string()));
                    }
                    j = skip(next);
                    if !punct(j, ',') {
                        break;
                    }
                    j = skip(j + 1);
                }
                in_import[begin..j.min(toks.len())].iter_mut().for_each(|x| *x = true);
                i = j;
                continue;
            }
            _ => {}
        }
        i += 1;
    }

    let mut out = Rpc::default();
    for i in 0..toks.len() {
        let Some(w) = word(i) else { continue };
        if in_import[i] {
            continue;
        }
        // `def add_XServicer_to_server(`, `class XStub(` là định nghĩa.
        if i > 0 && matches!(word(i - 1), Some("def" | "class")) {
            continue;
        }
        let (module, name): (Option<String>, &str) = match w.rsplit_once('.') {
            Some((qual, last)) => {
                let module = modules.get(qual).cloned().or_else(|| generated_stem(qual).map(|_| qual.to_string()));
                (module, last)
            }
            None => match names.get(w) {
                Some((m, original)) => (Some(m.clone()), original.as_str()),
                None => (star.first().cloned(), w),
            },
        };
        let generated = module.as_deref().and_then(generated_module);
        let infra = |service: &str| infrastructure(generated.as_deref().unwrap_or(""), service);
        let site = |service: &str, origin: Origin| Site { service: service.to_string(), origin, line: toks[i].line, shown: w.to_string() };
        let stem_or = |fallback: Origin| generated.clone().map_or(fallback, Origin::Stem);
        // Generic: tên đầy đủ nằm trong chuỗi.
        if punct(i + 1, '(') && name == "method_handlers_generic_handler" {
            if let Some((pkg, service)) = string(i + 2).and_then(full_name)
                && !infrastructure(&pkg, &service)
            {
                out.serves.push(site(&service, Origin::Package(pkg)));
            }
            continue;
        }
        if punct(i + 1, '(') && matches!(name, "unary_unary" | "unary_stream" | "stream_unary" | "stream_stream") {
            if let Some((pkg, service)) = string(i + 2).and_then(full_name)
                && !infrastructure(&pkg, &service)
            {
                out.calls.push(site(&service, Origin::Package(pkg)));
            }
            continue;
        }
        if let Some(service) = name.strip_prefix("add_").and_then(|n| n.strip_suffix("Servicer_to_server")) {
            if !service.is_empty() && !infra(service) {
                out.serves.push(site(service, stem_or(Origin::Loose)));
            }
        } else if let Some(service) = name.strip_suffix("Servicer") {
            // Lớp cha của server: chỉ tin khi đến từ module sinh ra.
            if !service.is_empty() && generated.is_some() && !infra(service) {
                out.implements.push(site(service, stem_or(Origin::Unknown)));
            }
        } else if let Some(service) = name.strip_suffix("Stub") {
            // Đến từ module sinh ra thì chắc là gRPC; không thì chỉ tin khi
            // một `.proto` trong repo khai service đó (`QuantStub()` của
            // PyTorch thì không).
            if !service.is_empty() && service.chars().next().is_some_and(char::is_uppercase) && !infra(service) {
                out.calls.push(site(service, stem_or(Origin::Unknown)));
            }
        }
    }
    out
}

fn go(toks: &[Token]) -> Rpc {
    let word = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let punct = |i: usize, c: char| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c);
    // Package của chính file: gọi không tiền tố trong package sinh ra.
    let own = (0..toks.len()).find(|&i| word(i) == Some("package")).and_then(|i| word(i + 1)).unwrap_or("").to_string();
    // Alias của từng import: `pb "go.etcd.io/etcd/api/v3/etcdserverpb"`, hay
    // tên mặc định là đoạn cuối của đường dẫn (bỏ `v3`).
    let mut aliases: HashMap<String, String> = HashMap::new();
    // `import . "x/kvpb"`: tên của package đó dùng không tiền tố.
    let mut dotted: Vec<String> = Vec::new();
    let mut add = |alias: Option<&str>, path: &str| {
        if alias == Some(".") {
            dotted.push(path.to_string());
            return;
        }
        let default = || {
            let mut segs = path.rsplit('/');
            let last = segs.next().unwrap_or(path);
            let versioned = last.len() > 1 && last.starts_with('v') && last[1..].bytes().all(|b| b.is_ascii_digit());
            if versioned { segs.next().unwrap_or(last).to_string() } else { last.to_string() }
        };
        let name = alias.map_or_else(default, str::to_string);
        if name != "_" && name != "." {
            aliases.insert(name, path.to_string());
        }
    };
    let mut i = 0;
    while i < toks.len() {
        if word(i) != Some("import") {
            i += 1;
            continue;
        }
        let one = |j: usize| -> (Option<String>, Option<(String, usize)>) {
            match (toks.get(j).map(|t| &t.tok), toks.get(j + 1).map(|t| &t.tok)) {
                (Some(Tok::Word(a)), Some(Tok::Str(p))) => (Some(a.clone()), Some((p.clone(), j + 2))),
                (Some(Tok::Punct('.')), Some(Tok::Str(p))) => (Some(".".into()), Some((p.clone(), j + 2))),
                (Some(Tok::Str(p)), _) => (None, Some((p.clone(), j + 1))),
                _ => (None, None),
            }
        };
        if punct(i + 1, '(') {
            let mut j = i + 2;
            while j < toks.len() && !punct(j, ')') {
                match one(j) {
                    (alias, Some((path, next))) => {
                        add(alias.as_deref(), &path);
                        j = next;
                    }
                    _ => j += 1,
                }
            }
            i = j + 1;
        } else {
            match one(i + 1) {
                (alias, Some((path, next))) => {
                    add(alias.as_deref(), &path);
                    i = next;
                }
                _ => i += 1,
            }
        }
    }

    // `RegisterKVHandlerServer` của grpc-gateway chạy handler trong cùng
    // process, không phải server gRPC.
    let service_of = |f: &str, prefix: &str, suffix: &str| -> Option<String> {
        f.strip_prefix(prefix)
            .and_then(|r| r.strip_suffix(suffix))
            .filter(|s| !s.is_empty() && !s.ends_with("Handler") && s.chars().next().is_some_and(char::is_uppercase))
            .map(str::to_string)
    };
    let mut out = Rpc::default();
    for i in 0..toks.len() {
        let Some(f) = word(i) else { continue };
        let qualified = punct(i.wrapping_sub(1), '.') && i >= 2;
        let (origin, shown, module) = if qualified {
            let Some(pkg) = word(i - 2) else { continue };
            // `a.pb.NewKVClient`: không phải tên package.
            if punct(i.wrapping_sub(3), '.') && i >= 3 {
                continue;
            }
            match aliases.get(pkg) {
                Some(path) => (Origin::GoPackage(path.clone()), format!("{pkg}.{f}"), path.clone()),
                None => (Origin::GoName(pkg.to_string()), format!("{pkg}.{f}"), pkg.to_string()),
            }
        } else {
            // Gọi không tiền tố: chỉ tính lời gọi, không tính định nghĩa
            // `func NewKVClient(` hay method `func (c *x) NewKVClient(`.
            if !punct(i + 1, '(') || (i > 0 && (word(i - 1) == Some("func") || punct(i - 1, ')'))) {
                continue;
            }
            (Origin::GoName(own.clone()), f.to_string(), own.clone())
        };
        // Không tiền tố: có thể từ package của chính file, hay từ một package
        // dot-import. Ghi mỗi khả năng một chỗ; bảng tra `.proto` bỏ chỗ sai.
        let mut origins = vec![origin];
        if !qualified {
            origins.extend(dotted.iter().map(|p| Origin::GoPackage(p.clone())));
        }
        let site = |service: &str, origin: &Origin| Site {
            service: service.to_string(),
            origin: origin.clone(),
            line: toks[i].line,
            shown: shown.clone(),
        };
        if let Some(service) = service_of(f, "Register", "Server") {
            if !infrastructure(&module, &service) {
                out.serves.extend(origins.iter().map(|o| site(&service, o)));
            }
        } else if let Some(service) = service_of(f, "New", "Client") {
            if !infrastructure(&module, &service) {
                out.calls.extend(origins.iter().map(|o| site(&service, o)));
            }
        } else if qualified {
            let origin = origins.swap_remove(0);
            let site = |service: String, origin: Origin| Site { service, origin, line: toks[i].line, shown: shown.clone() };
            if let Some(service) = f.strip_suffix("_ServiceDesc").filter(|s| !s.is_empty()).map(str::to_string) {
                if !infrastructure(&module, &service) {
                    out.serves.push(site(service, origin));
                }
            } else if let Some(service) = service_of(f, "Unimplemented", "Server")
                && !infrastructure(&module, &service)
            {
                out.implements.push(site(service, origin));
            } else if matches!(f, "Invoke" | "NewStream") && punct(i + 1, '(') {
                // Client generic: `conn.Invoke(ctx, "/pkg.KV/Range", …)`.
                for j in i + 2..(i + 10).min(toks.len()) {
                    if let Some(Tok::Str(s)) = toks.get(j).map(|t| &t.tok)
                        && let Some((pkg, service)) = full_name(s)
                    {
                        if !infrastructure(&pkg, &service) {
                            out.calls.push(Site { service, origin: Origin::Package(pkg), line: toks[i].line, shown: shown.clone() });
                        }
                        break;
                    }
                }
            }
        }
    }
    out
}

/// File test hay mock: server đăng ký ở đó là server giả cho test
/// (`client/v3/mock/mockserver` của etcd), không phải nơi service chạy thật.
/// Lời gọi từ test thì vẫn là lời gọi thật.
pub fn fake_server_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let dirs = path.rsplit_once('/').map_or("", |(d, _)| d);
    let script = |n: &str| [".js", ".ts", ".mjs", ".cjs"].iter().any(|e| n.ends_with(e));
    name.ends_with("_test.go")
        || name.starts_with("test_")
        || name.ends_with("_test.py")
        || name == "conftest.py"
        || name == "tests.rs"
        || name.ends_with("_test.rs")
        || (script(name) && (name.contains(".test.") || name.contains(".spec.")))
        || dirs.split('/').any(|d| {
            matches!(d, "test" | "tests" | "testdata" | "testutil" | "testutils" | "mock" | "mocks" | "grpctesting" | "__tests__")
        })
}

/// Tổng số service đưa vào bảng tra (TM-13): repo thật có vài trăm.
pub const MAX_INDEX: usize = 100_000;
/// Số ứng viên tối đa xét khi phân biệt theo đường dẫn module.
const MAX_SUFFIX_CANDIDATES: usize = 64;

/// Mọi file `.proto` của repo, tra theo tên service bằng bảng băm (TM-13).
#[derive(Debug, Clone, Default)]
pub struct ProtoIndex {
    by_service: HashMap<String, Known>,
    /// Tên service không phân biệt hoa thường → tên trong `.proto`: Rust viết
    /// `KV` thành `Kv`.
    by_norm: HashMap<String, BTreeSet<String>>,
    /// Tên mọi file `.proto` (không đuôi), kể cả file không có service.
    stems: std::collections::HashSet<String>,
    count: usize,
    /// Quá [`MAX_INDEX`] service thì phần sau không vào bảng.
    pub truncated: bool,
}

/// Một tên service: nó có ở đâu.
#[derive(Debug, Clone, Default)]
struct Known {
    packages: BTreeSet<String>,
    /// Tên file → (đường dẫn file bằng dấu chấm, không đuôi; package).
    by_stem: HashMap<String, Vec<(String, String)>>,
    by_go_package: HashMap<String, BTreeSet<String>>,
    by_go_name: HashMap<String, BTreeSet<String>>,
    /// Package Java của lớp sinh ra → package `.proto`.
    by_java: HashMap<String, BTreeSet<String>>,
}

/// Kết quả ghép một chỗ gọi hay đăng ký với `.proto`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    /// `grpc:package.Service`, hay `grpc:Service` khi không có package, hay
    /// khi module sinh ra không ứng với `.proto` nào trong repo.
    Found(String),
    /// Tên service có ở nhiều package mà không lần được về file nào.
    Ambiguous(Vec<String>),
    /// Không phải gRPC: `NewXClient` của Go, `XStub` không rõ module, mà không
    /// `.proto` nào trong repo khai service `X`.
    NotRpc,
}

impl ProtoIndex {
    pub fn add(&mut self, path: &str, proto: &Proto) {
        let file = path.strip_suffix(".proto").unwrap_or(path);
        let stem = file.rsplit('/').next().unwrap_or(file).to_string();
        let dotted = file.replace('/', ".");
        self.stems.insert(stem.clone());
        for s in &proto.services {
            if self.count >= MAX_INDEX {
                self.truncated = true;
                return;
            }
            self.count += 1;
            self.by_norm.entry(norm(&s.name)).or_default().insert(s.name.clone());
            let k = self.by_service.entry(s.name.clone()).or_default();
            k.packages.insert(proto.package.clone());
            k.by_stem.entry(stem.clone()).or_default().push((dotted.clone(), proto.package.clone()));
            if let Some(g) = &proto.go_package {
                k.by_go_package.entry(g.clone()).or_default().insert(proto.package.clone());
            }
            if let Some(n) = &proto.go_name {
                k.by_go_name.entry(n.clone()).or_default().insert(proto.package.clone());
            }
            let java = proto.java_package.clone().unwrap_or_else(|| proto.package.clone());
            k.by_java.entry(java).or_default().insert(proto.package.clone());
        }
    }

    fn full(package: &str, service: &str) -> String {
        if package.is_empty() { format!("grpc:{service}") } else { format!("grpc:{package}.{service}") }
    }

    fn one(set: &BTreeSet<String>, service: &str) -> Option<Key> {
        (set.len() == 1).then(|| Key::Found(Self::full(set.first().map_or("", String::as_str), service)))
    }

    pub fn key(&self, site: &Site) -> Key {
        if site.origin == Origin::Rust {
            let names = self.by_norm.get(&norm(&site.service)).map(|n| n.iter().collect::<Vec<_>>()).unwrap_or_default();
            return match names.as_slice() {
                [] => Key::NotRpc,
                [one] => self.key(&Site { service: (*one).clone(), origin: Origin::Unknown, ..site.clone() }),
                many => Key::Ambiguous(many.iter().flat_map(|n| self.by_service[*n].packages.iter().map(|p| Self::full(p, n))).collect()),
            };
        }
        let service = site.service.as_str();
        let empty = Known::default();
        let known = self.by_service.get(service).unwrap_or(&empty);
        // Chỉ theo tên: duy nhất thì ghép, nhiều thì nhập nhằng.
        let by_name = |none: Key| match known.packages.len() {
            0 => none,
            1 => Key::Found(Self::full(known.packages.first().map_or("", String::as_str), service)),
            _ => Key::Ambiguous(known.packages.iter().map(|p| Self::full(p, service)).collect()),
        };
        match &site.origin {
            Origin::Package(pkg) => Key::Found(Self::full(pkg, service)),
            Origin::Suffix(tail) => {
                let dotted = format!(".{tail}");
                let hits: BTreeSet<String> =
                    known.packages.iter().filter(|p| *p == tail || p.ends_with(&dotted)).cloned().collect();
                match hits.len() {
                    0 => by_name(Key::NotRpc),
                    1 => Key::Found(Self::full(hits.first().map_or("", String::as_str), service)),
                    _ => Key::Ambiguous(hits.iter().map(|p| Self::full(p, service)).collect()),
                }
            }
            Origin::GoName(name) => {
                known.by_go_name.get(name).and_then(|set| Self::one(set, service)).unwrap_or(Key::NotRpc)
            }
            Origin::GoPackage(path) => match known.by_go_package.get(path).and_then(|set| Self::one(set, service)) {
                Some(k) => k,
                None => by_name(Key::NotRpc),
            },
            Origin::JavaPackage(pkg) => match known.by_java.get(pkg).and_then(|set| Self::one(set, service)) {
                Some(k) => k,
                None => by_name(Key::NotRpc),
            },
            // `Rust` đã đổi về tên trong `.proto` ở trên.
            Origin::Unknown | Origin::Rust => by_name(Key::NotRpc),
            Origin::Loose => by_name(Key::Found(Self::full("", service))),
            Origin::Stem(module) => {
                let last = module.rsplit('.').next().unwrap_or(module);
                // Không file `.proto` nào trong repo mang tên này: module đến
                // từ thư viện ngoài, hay `.proto` để ở repo khác. Ghép theo tên
                // trần, không lấy service cùng tên của repo này.
                if !self.stems.contains(last) {
                    return Key::Found(Self::full("", service));
                }
                let Some(cands) = known.by_stem.get(last) else { return by_name(Key::Found(Self::full("", service))) };
                let pkgs: BTreeSet<String> = cands.iter().map(|(_, p)| p.clone()).collect();
                if let Some(k) = Self::one(&pkgs, service) {
                    return k;
                }
                // `shop.v1.kv` và `shop.v2.kv`: phân biệt theo đường dẫn module.
                if cands.len() <= MAX_SUFFIX_CANDIDATES {
                    let want: Vec<&str> = module.trim_start_matches('.').split('.').collect();
                    let matched: BTreeSet<String> = cands
                        .iter()
                        .filter(|(path, _)| path.split('.').collect::<Vec<_>>().ends_with(&want))
                        .map(|(_, p)| p.clone())
                        .collect();
                    if let Some(k) = Self::one(&matched, service) {
                        return k;
                    }
                }
                Key::Ambiguous(pkgs.iter().map(|p| Self::full(p, service)).collect())
            }
        }
    }
}

/// Ai phục vụ mỗi khoá: mọi component đăng ký server cho nó. Một service có
/// thể có nhiều server (proxy của etcd cũng đăng ký `KV`), và trùng không làm
/// cạnh biến mất (TM-15).
pub type Servers = BTreeMap<String, BTreeSet<String>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn py(src: &str) -> Rpc {
        scan(&Lang::Python.tokens(src), Lang::Python)
    }

    fn golang(src: &str) -> Rpc {
        scan(&Lang::Go.tokens(src), Lang::Go)
    }

    fn names(sites: &[Site]) -> Vec<(String, Origin)> {
        sites.iter().map(|s| (s.service.clone(), s.origin.clone())).collect()
    }

    fn stem(s: &str) -> Origin {
        Origin::Stem(s.into())
    }

    #[test]
    fn doc_proto() {
        let p = read_proto(
            "syntax = \"proto3\";\npackage etcdserverpb;\noption go_package = \"go.etcd.io/etcd/api/v3/etcdserverpb;pb\";\n\
             // service Fake {\nmessage M { string service = 1; message N { } }\n\
             service KV {\n  rpc Range(RangeRequest) returns (RangeResponse) {\n    option (google.api.http) = { post: \"/v3/kv/range\" };\n  }\n  rpc Put(PutRequest) returns (PutResponse) {}\n}\n\
             service Watch { rpc Watch(stream WatchRequest) returns (stream WatchResponse); }\n",
        );
        assert_eq!(p.package, "etcdserverpb");
        assert_eq!(p.go_package.as_deref(), Some("go.etcd.io/etcd/api/v3/etcdserverpb"));
        let s: Vec<(&str, Vec<&str>)> =
            p.services.iter().map(|s| (s.name.as_str(), s.rpcs.iter().map(|(r, _)| r.as_str()).collect())).collect();
        assert_eq!(s, [("KV", vec!["Range", "Put"]), ("Watch", vec!["Watch"])]);
        assert_eq!(p.services[0].line, 6);
        assert_eq!(read_proto("package a.b.c;\n").package, "a.b.c");
    }

    #[test]
    fn proto_doi_khang_co_tran() {
        let deep = format!("service S {{ {} rpc A(B) returns (C); }}", "{".repeat(200_000));
        let start = crate::testclock::Clock::start();
        let p = read_proto(&deep);
        assert_eq!(p.services.len(), 1);
        assert!(p.services[0].rpcs.is_empty(), "rpc lồng trong ngoặc khác không phải rpc của service");
        let many: String = (0..2000).map(|i| format!("service S{i} {{}}\n")).collect();
        let p = read_proto(&many);
        assert!(p.truncated && p.services.len() == MAX_SERVICES);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
    }

    #[test]
    fn cpp_client_server() {
        let toks = Lang::C.tokens(
            "#include \"helloworld.grpc.pb.h\"\nclass GreeterServiceImpl final : public helloworld::Greeter::Service {};\n\
             class Async : public Greeter::AsyncService, public grpc::Service {};\n\
             void run() {\n  GreeterServiceImpl service;\n  builder.RegisterService(&service);\n\
               auto stub = helloworld::Greeter::NewStub(channel);\n  auto h = grpc::health::v1::Health::NewStub(ch);\n  auto s = Shop::NewStub(c);\n}\n\
             std::unique_ptr<Greeter::Stub> Greeter::NewStub(x);\n",
        );
        let rpc = scan(&toks, Lang::C);
        let names = |v: &[Site]| v.iter().map(|s| (s.service.clone(), s.origin.clone(), s.line)).collect::<Vec<_>>();
        let suffix = |p: &str| Origin::Suffix(p.into());
        assert_eq!(names(&rpc.implements), vec![("Greeter".into(), suffix("helloworld"), 2), ("Greeter".into(), Origin::Unknown, 3)]);
        assert_eq!(names(&rpc.serves), vec![("Greeter".into(), Origin::Unknown, 6), ("Greeter".into(), suffix("helloworld"), 6)]);
        assert_eq!(
            names(&rpc.calls),
            vec![("Greeter".into(), suffix("helloworld"), 7), ("Shop".into(), Origin::Unknown, 9), ("Greeter".into(), Origin::Unknown, 11)]
        );
        assert!(generated_stub_path("gen/helloworld.grpc.pb.cc"));
        // `::` toàn cục sau từ khoá: từ khoá không phải namespace.
        let toks = Lang::C.tokens("auto f() { return ::shop::KV::NewStub(ch); }\nclass I : public ::shop::KV::Service {};\n");
        let rpc = scan(&toks, Lang::C);
        assert_eq!(rpc.calls[0].origin, Origin::Suffix("shop".into()));
        assert_eq!(rpc.implements[0].origin, Origin::Suffix("shop".into()));
    }

    #[test]
    fn python_client_server() {
        let r = py("import grpc\nfrom vllm.grpc import rpc_pb2_grpc\nimport a.b.other_pb2_grpc as og\n\
                    from a.c.inf_pb2_grpc import InferenceStub as Inf, add_ControlServicer_to_server\n\
                    stub = rpc_pb2_grpc.KVStub(channel)\nx = og.WatchStub(ch)\ny = Inf(ch)\n\
                    add_ControlServicer_to_server(impl, server)\nrpc_pb2_grpc.add_KVServicer_to_server(impl, s)\n\
                    q = torch.ao.quantization.QuantStub()\nz = MarkingStub()\n");
        assert_eq!(
            names(&r.calls),
            [
                ("KV".into(), stem("vllm.grpc.rpc")),
                ("Watch".into(), stem("a.b.other")),
                ("Inference".into(), stem("a.c.inf")),
                // Không rõ module: ghi lại, bảng tra `.proto` quyết (không có
                // `.proto` nào khai `Quant` thì không phải gRPC).
                ("Quant".into(), Origin::Unknown),
                ("Marking".into(), Origin::Unknown),
            ]
        );
        assert_eq!(names(&r.serves), [("Control".into(), stem("a.c.inf")), ("KV".into(), stem("vllm.grpc.rpc"))]);
    }

    #[test]
    fn python_import_tuong_doi_noi_dong_va_sao() {
        let kv = |m: &str| ("KV".to_string(), stem(m));
        assert_eq!(names(&py("from ..gen.kv_pb2_grpc import KVStub\nKVStub(ch)\n").calls), [kv("..gen.kv")]);
        assert_eq!(names(&py("from ..gen import kv_pb2_grpc as pb\npb.KVStub(ch)\n").calls), [kv("..gen.kv")]);
        assert_eq!(names(&py("from . import kv_pb2_grpc\nkv_pb2_grpc.KVStub(ch)\n").calls), [kv(".kv")]);
        assert_eq!(names(&py("from app.gen.kv_pb2_grpc import \\\n    KVStub\nKVStub(ch)\n").calls), [kv("app.gen.kv")]);
        assert_eq!(names(&py("from app.gen.kv_pb2_grpc import (\n    KVStub,\n)\nKVStub(ch)\n").calls), [kv("app.gen.kv")]);
        assert_eq!(names(&py("from app.gen.kv_pb2_grpc import *\nKVStub(ch)\n").calls), [kv("app.gen.kv")]);
        assert_eq!(names(&py("import app.gen.kv_pb2_grpc \\\n    as g\ng.KVStub(ch)\n").calls), [kv("app.gen.kv")]);
        // Gọi qua biến: nhắc tới là đủ, không cần dấu ngoặc.
        assert_eq!(names(&py("from app.gen import kv_pb2_grpc\nS = kv_pb2_grpc.KVStub\ns = S(ch)\n").calls), [kv("app.gen.kv")]);
        let r = py("from app.gen import kv_pb2_grpc as g\nreg = g.add_KVServicer_to_server\nreg(impl, s)\n");
        assert_eq!(names(&r.serves), [kv("app.gen.kv")]);
    }

    #[test]
    fn python_generic_va_cai_dat() {
        let r = py("import grpc\nfrom app.gen import kv_pb2_grpc\n\
                    class Impl(kv_pb2_grpc.KVServicer):\n    pass\n\
                    h = grpc.method_handlers_generic_handler(\"shop.v1.KV\", handlers)\n\
                    m = ch.unary_unary(\"/shop.v1.KV/Get\")\n");
        let pkg = || Origin::Package("shop.v1".into());
        assert_eq!(names(&r.implements), [("KV".into(), stem("app.gen.kv"))]);
        assert_eq!(names(&r.serves), [("KV".into(), pkg())]);
        assert_eq!(names(&r.calls), [("KV".into(), pkg())]);
    }

    #[test]
    fn python_bo_dinh_nghia_va_ha_tang() {
        let r = py("class KVStub(object):\n    pass\ndef add_KVServicer_to_server(servicer, server):\n    pass\n\
                    from grpc_health.v1 import health_pb2_grpc\nhealth_pb2_grpc.add_HealthServicer_to_server(h, s)\n\
                    s = health_pb2_grpc.HealthStub(ch)\n");
        assert!(r.is_empty(), "{r:?}");
    }

    #[test]
    fn go_client_server() {
        let r = golang("package x\nimport (\n  pb \"go.etcd.io/etcd/api/v3/etcdserverpb\"\n  \"google.golang.org/grpc\"\n  \"example.com/api/v2\"\n)\n\
                        func f() {\n  kv := pb.NewKVClient(conn)\n  pb.RegisterKVServer(s, impl)\n  pb.RegisterKVHandlerServer(ctx, mux, srv)\n  c := api.NewOrderClient(conn)\n  grpc.NewServer()\n}\n");
        let go = |p: &str| Origin::GoPackage(p.into());
        assert_eq!(
            names(&r.calls),
            [("KV".into(), go("go.etcd.io/etcd/api/v3/etcdserverpb")), ("Order".into(), go("example.com/api/v2"))]
        );
        assert_eq!(names(&r.serves), [("KV".into(), go("go.etcd.io/etcd/api/v3/etcdserverpb"))]);
        assert_eq!(r.calls[0].line, 8);
        let r = golang("package x\nimport pb \"a/kvpb\"\ntype s struct { pb.UnimplementedKVServer }\n\
                        func g(srv int) { srv.RegisterService(&pb.KV_ServiceDesc, 0); f := pb.RegisterKVServer; f(srv, 0)\n\
                        conn.Invoke(ctx, \"/shop.v1.KV/Range\", in, out) }\n");
        assert_eq!(names(&r.implements), [("KV".into(), go("a/kvpb"))]);
        assert_eq!(names(&r.serves), [("KV".into(), go("a/kvpb")), ("KV".into(), go("a/kvpb"))]);
        assert_eq!(names(&r.calls), [("KV".into(), Origin::Package("shop.v1".into()))]);
        // Gọi không tiền tố trong package sinh ra: theo tên package của file.
        let r = golang("package kvpb\nfunc NewKVClient(c int) int { return 0 }\nfunc Dial(c int) int { return NewKVClient(c) }\n");
        assert_eq!(names(&r.calls), [("KV".into(), Origin::GoName("kvpb".into()))]);
        // Dot-import: thử cả package của file lẫn package dot-import.
        let r = golang("package server\nimport . \"a/kvpb\"\nfunc g(s int) { RegisterKVServer(s, 0) }\n");
        assert_eq!(names(&r.serves), [("KV".into(), Origin::GoName("server".into())), ("KV".into(), go("a/kvpb"))]);
    }

    #[test]
    fn ghep_theo_package_roi_theo_ten() {
        let mut idx = ProtoIndex::default();
        idx.add("api/rpc.proto", &read_proto("package etcdserverpb;\noption go_package = \"go.etcd.io/api/pb\";\nservice KV {}\n"));
        idx.add("a/x.proto", &read_proto("package a;\nservice Dup {}\n"));
        idx.add("b/y.proto", &read_proto("package b;\nservice Dup {}\n"));
        let site = |service: &str, origin: Origin| Site { service: service.into(), origin, line: 1, shown: String::new() };
        assert_eq!(idx.key(&site("KV", stem("rpc"))), Key::Found("grpc:etcdserverpb.KV".into()));
        assert_eq!(idx.key(&site("KV", Origin::GoPackage("go.etcd.io/api/pb".into()))), Key::Found("grpc:etcdserverpb.KV".into()));
        assert_eq!(idx.key(&site("KV", Origin::Unknown)), Key::Found("grpc:etcdserverpb.KV".into()), "tên duy nhất");
        assert_eq!(idx.key(&site("Dup", stem("x"))), Key::Found("grpc:a.Dup".into()), "lần được về file");
        assert_eq!(
            idx.key(&site("Dup", Origin::Unknown)),
            Key::Ambiguous(vec!["grpc:a.Dup".into(), "grpc:b.Dup".into()])
        );
        assert_eq!(idx.key(&site("VllmEngine", stem("vllm_engine"))), Key::Found("grpc:VllmEngine".into()), "không có .proto");
        // Module của thư viện ngoài (`etcd3.etcdrpc.rpc_pb2_grpc`) mà repo không
        // có `rpc.proto`: không ghép vào service cùng tên của repo.
        idx.add("proto/kv.proto", &read_proto("package shop.v1;\nservice Ext {}\n"));
        assert_eq!(idx.key(&site("Ext", stem("etcd3.etcdrpc.rpc_ext"))), Key::Found("grpc:Ext".into()));
        // API có phiên bản: cùng tên file, phân biệt theo đường dẫn module.
        idx.add("proto/shop/v1/kv2.proto", &read_proto("package shop.v1;\nservice Ver {}\n"));
        idx.add("proto/shop/v2/kv2.proto", &read_proto("package shop.v2;\nservice Ver {}\n"));
        assert_eq!(idx.key(&site("Ver", stem("shop.v2.kv2"))), Key::Found("grpc:shop.v2.Ver".into()));
        assert!(matches!(idx.key(&site("Ver", stem("kv2"))), Key::Ambiguous(_)));
        assert_eq!(idx.key(&site("Quant", Origin::Unknown)), Key::NotRpc, "XStub không rõ module cần .proto");
        assert_eq!(idx.key(&site("KVFromKV", Origin::GoPackage("go.etcd.io/x/adapter".into()))), Key::NotRpc, "Go cần .proto");
        idx.add("order/v1/order.proto", &read_proto("package acme.order.v1;\noption go_package = \"example.com/gen/order/v1;orderv1\";\nservice OrderService {}\n"));
        assert_eq!(idx.key(&site("OrderService", Origin::GoName("orderv1".into()))), Key::Found("grpc:acme.order.v1.OrderService".into()));
        assert_eq!(idx.key(&site("OrderService", Origin::GoName("client".into()))), Key::NotRpc, "biến tên `client` không phải package");
    }

    #[test]
    fn java_package_va_ghep_theo_package_java() {
        let p = read_proto("package shop.v1;\noption java_package = \"com.acme.shop.v1\";\nservice Cart {}\n");
        assert_eq!(p.java_package.as_deref(), Some("com.acme.shop.v1"));
        let mut idx = ProtoIndex::default();
        idx.add("a/shop.proto", &p);
        idx.add("b/other.proto", &read_proto("package other;\nservice Cart {}\n"));
        let site = |o: Origin| Site { service: "Cart".into(), origin: o, line: 1, shown: String::new() };
        assert_eq!(idx.key(&site(Origin::JavaPackage("com.acme.shop.v1".into()))), Key::Found("grpc:shop.v1.Cart".into()));
        // Không `java_package`: package Java là package `.proto`.
        assert_eq!(idx.key(&site(Origin::JavaPackage("other".into()))), Key::Found("grpc:other.Cart".into()));
        assert!(matches!(idx.key(&site(Origin::JavaPackage("x.y".into()))), Key::Ambiguous(_)));
    }

    #[test]
    fn java_goi_cai_dat_dang_ky() {
        let src = "package a.b;\nimport static x.KVGrpc.newStub;\nclass S extends x.KVGrpc.KVImplBase implements Runnable, x.LogGrpc.AsyncService {\n  void f(Object c) { newStub(c); KVGrpc\n    .newFutureStub(c); g().newBlockingStub(c); }\n  void s(io.grpc.ServerBuilder<?> b) { b.addService(this); }\n}\n";
        let r = java(&crate::scan::Lang::Java.tokens(src));
        let show = |v: &[Site]| v.iter().map(|s| format!("{}:{}:{:?}", s.line, s.service, s.origin)).collect::<Vec<_>>();
        assert_eq!(show(&r.calls), ["4:KV:JavaPackage(\"x\")", "5:KV:JavaPackage(\"a.b\")"]);
        assert_eq!(show(&r.implements), ["3:KV:JavaPackage(\"x\")", "3:Log:JavaPackage(\"x\")"]);
        assert_eq!(show(&r.serves), ["6:KV:JavaPackage(\"x\")", "6:Log:JavaPackage(\"x\")"]);
    }

}
