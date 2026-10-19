//! Cạnh qua hàng đợi và pub/sub: bên gửi và bên nhận không import nhau.
//!
//! ```text
//! # api/orders.py                        # worker/main.py
//! producer.produce("orders", v)          consumer.subscribe(["orders"])
//! ch.basic_publish("", "jobs", body)     ch.basic_consume(queue="jobs", …)
//! app.send_task("shop.tasks.charge")     @app.task  def charge(…)
//! ```
//!
//! Dò chỗ gửi và chỗ nhận theo token, rồi `check` ghép chúng theo hệ và tên:
//! Kafka theo topic, RabbitMQ theo queue (exchange mặc định) hay exchange,
//! Redis pub/sub theo kênh (cả mẫu của `psubscribe`), Celery theo tên task.
//!
//! Lời gọi có tên chung chung (`send`, `publish`, `subscribe`, `Consume`) chỉ
//! được tính khi file import thư viện của hệ đó; lời gọi đặc trưng
//! (`basic_publish`, `send_task`, `sendToQueue`, `ConsumePartition`) thì luôn
//! tính. Tên topic là chuỗi cố định, hằng số khai trong cùng file, hay biến môi
//! trường (`check` lần qua cấu hình triển khai); còn lại là [`Name::Dynamic`]
//! và `check` báo "chưa ghép", không đoán (TM-40).
//!
//! Không nhận ra (ghi trong `docs/infra.md`): hàm bọc tự viết quanh producer,
//! tên topic từ module khác, aio-pika, kombu, Faust, NATS.

use super::lexer::{Tok, Token};
use super::{Import, Lang};
use std::collections::{BTreeSet, HashMap};

/// Trần số chỗ gửi, nhận trong một file (TM-42).
pub const MAX_PER_FILE: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum System {
    Kafka,
    /// RabbitMQ, AMQP 0-9-1.
    Rabbit,
    /// Redis pub/sub.
    Redis,
    Celery,
}

impl System {
    pub fn as_str(self) -> &'static str {
        match self {
            System::Kafka => "kafka",
            System::Rabbit => "rabbitmq",
            System::Redis => "redis",
            System::Celery => "celery",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Role {
    Send,
    Receive,
}

/// Tên mà chỗ gửi, nhận dùng.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Name {
    /// Chuỗi cố định, hay hằng số khai trong cùng file. RabbitMQ thêm tiền tố
    /// `q:` (queue, qua exchange mặc định) hay `x:` (exchange).
    Fixed(String),
    /// Celery: task chưa đặt `name=`, tên mặc định là `<module>.<hàm>`; `check`
    /// ghép theo module của file.
    Task(String),
    /// Mẫu glob của `psubscribe` (Redis): chỉ ở bên nhận.
    Pattern(String),
    /// Biến môi trường: `os.getenv("TOPIC")`, `process.env.TOPIC`. RabbitMQ
    /// thêm tiền tố `q:` hay `x:` như [`Name::Fixed`].
    Env(String),
    /// Không cố định: biến, f-string, nối chuỗi. Giữ cách viết để báo.
    Dynamic(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub system: System,
    pub role: Role,
    pub name: Name,
    pub line: usize,
    /// Như mã viết, để báo cáo.
    pub shown: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Queues {
    pub sites: Vec<Site>,
    /// Quá [`MAX_PER_FILE`]: phần sau không xét.
    pub cut: bool,
}

impl Queues {
    pub fn is_empty(&self) -> bool {
        self.sites.is_empty()
    }
}

/// Thư viện mỗi hệ, theo đường dẫn import.
struct Libs {
    kafka: bool,
    rabbit: bool,
    redis: bool,
    celery: bool,
}

fn libs(lang: Lang, imports: &[Import]) -> Libs {
    let has = |prefixes: &[&str]| {
        imports.iter().any(|i| {
            prefixes.iter().any(|p| {
                i.spec == *p
                    || i.spec.strip_prefix(p).is_some_and(|r| r.starts_with('.') || r.starts_with('/'))
            })
        })
    };
    match lang {
        Lang::Python => Libs {
            kafka: has(&["kafka", "confluent_kafka", "aiokafka"]),
            rabbit: has(&["pika"]),
            redis: has(&["redis", "aioredis", "coredis"]),
            celery: has(&["celery"]),
        },
        Lang::Go => Libs {
            kafka: has(&[
                "github.com/IBM/sarama",
                "github.com/Shopify/sarama",
                "github.com/segmentio/kafka-go",
                "github.com/confluentinc/confluent-kafka-go",
            ]),
            rabbit: has(&["github.com/rabbitmq/amqp091-go", "github.com/streadway/amqp"]),
            redis: has(&["github.com/redis/go-redis", "github.com/go-redis/redis"]),
            celery: false,
        },
        Lang::TypeScript => Libs {
            kafka: has(&["kafkajs", "@confluentinc/kafka-javascript"]),
            rabbit: has(&["amqplib", "amqp-connection-manager"]),
            redis: has(&["redis", "ioredis", "@redis/client"]),
            celery: false,
        },
        Lang::C | Lang::Rust => Libs { kafka: false, rabbit: false, redis: false, celery: false },
    }
}

pub fn scan(toks: &[Token], lang: Lang, imports: &[Import]) -> Queues {
    let libs = libs(lang, imports);
    if matches!(lang, Lang::C | Lang::Rust) {
        return Queues::default();
    }
    let (close, depth) = brackets(toks, lang);
    let mut cx = Cx { toks, lang, consts: HashMap::new(), close, depth, clients: HashMap::new() };
    cx.consts = consts(&cx);
    cx.clients = clients(&cx);
    let mut out = Queues::default();
    let mut push = |out: &mut Queues, system, role, names: Vec<Name>, line, shown: &str, strong: bool| {
        for name in names {
            // Lời gọi không đặc trưng mà tên cũng không cố định: không đủ chắc
            // là hàng đợi để báo. Tên lấy từ env thì lần ra được, vẫn giữ.
            if !strong && !matches!(name, Name::Fixed(_) | Name::Env(_)) {
                continue;
            }
            if out.sites.len() >= MAX_PER_FILE {
                out.cut = true;
                return;
            }
            out.sites.push(Site { system, role, name, line, shown: shown.to_string() });
        }
    };
    for i in 0..toks.len() {
        if out.cut {
            break;
        }
        match lang {
            Lang::Python => python_at(&cx, &libs, i, &mut out, &mut push),
            Lang::Go => go_at(&cx, &libs, i, &mut out, &mut push),
            Lang::TypeScript => node_at(&cx, &libs, i, &mut out, &mut push),
            Lang::C | Lang::Rust => {}
        }
    }
    out
}

type Push<'p> = dyn FnMut(&mut Queues, System, Role, Vec<Name>, usize, &str, bool) + 'p;

/// Một đối số của lời gọi: tên khoá (`topic=` của Python) và khoảng token.
struct Arg {
    key: Option<String>,
    a: usize,
    b: usize,
}

struct Cx<'a> {
    toks: &'a [Token],
    lang: Lang,
    /// Hằng số trong file: tên → giá trị; `None` khi gán nhiều giá trị khác
    /// nhau, hay tên còn được ràng buộc ở chỗ khác (tham số, biến vòng lặp).
    consts: HashMap<String, Option<Name>>,
    /// Ngoặc mở ở `i` → ngoặc đóng khớp với nó (`toks.len()` nếu không có).
    /// Tính một lần cho cả file: tìm lại mỗi lần gọi là bậc hai trên file lồng
    /// sâu (TM-42).
    close: Vec<usize>,
    /// Số ngoặc tròn, vuông (và nhọn với Python) đang mở trước token `i`: phép
    /// gán chỉ là câu lệnh khi nó bằng 0.
    depth: Vec<u32>,
    /// Biến gán từ hàm dựng producer, consumer Kafka (đoạn cuối của tên):
    /// `p = KafkaProducer(…)`, `const consumer = kafka.consumer(…)`.
    clients: HashMap<String, Role>,
}

/// Bảng ngoặc khớp và độ sâu ngoặc của từng token, một lượt bằng ngăn xếp.
/// Với Go, JS, `{` là khối lệnh: nó mở một khung độ sâu mới bằng 0, để câu
/// lệnh trong thân hàm truyền làm đối số (callback, IIFE) vẫn là câu lệnh.
/// Khối `const ( … )`, `var ( … )` của Go cũng là các câu lệnh khai.
fn brackets(toks: &[Token], lang: Lang) -> (Vec<usize>, Vec<u32>) {
    enum Open {
        /// Ngoặc tính vào độ sâu.
        Counted,
        /// Khối lệnh: độ sâu trước khi mở, khôi phục khi đóng.
        Block(u32),
        /// Không tính: `const (` của Go.
        Plain,
    }
    let n = toks.len();
    let mut close = vec![n; n];
    let mut depth = vec![0u32; n];
    let mut stack: Vec<(usize, Open)> = Vec::new();
    let mut d = 0u32;
    for (j, t) in toks.iter().enumerate() {
        depth[j] = d;
        match t.tok {
            Tok::Punct(c @ ('(' | '[' | '{')) => {
                let decl = lang == Lang::Go
                    && c == '('
                    && j > 0
                    && matches!(&toks[j - 1].tok, Tok::Word(w) if w == "const" || w == "var");
                let kind = if c == '{' && lang != Lang::Python {
                    let saved = d;
                    d = 0;
                    Open::Block(saved)
                } else if decl {
                    Open::Plain
                } else {
                    d += 1;
                    Open::Counted
                };
                stack.push((j, kind));
            }
            Tok::Punct(')' | ']' | '}') => {
                if let Some((o, kind)) = stack.pop() {
                    close[o] = j;
                    match kind {
                        Open::Counted => d = d.saturating_sub(1),
                        Open::Block(saved) => d = saved,
                        Open::Plain => {}
                    }
                }
            }
            _ => {}
        }
    }
    (close, depth)
}

impl Cx<'_> {
    fn word(&self, i: usize) -> Option<&str> {
        match self.toks.get(i).map(|t| &t.tok) {
            Some(Tok::Word(w)) => Some(w.as_str()),
            _ => None,
        }
    }

    fn punct(&self, i: usize, c: char) -> bool {
        matches!(self.toks.get(i).map(|t| &t.tok), Some(Tok::Punct(p)) if *p == c)
    }

    /// Tên hàm được gọi ở `i` (đoạn cuối), và nó có phải phương thức không:
    /// `x.produce(`, `self.p.produce(` (Python gộp đường dẫn thành một từ),
    /// `get().produce(`.
    fn callee(&self, i: usize) -> Option<(&str, bool)> {
        let w = self.word(i)?;
        if !self.punct(i + 1, '(') {
            return None;
        }
        // Định nghĩa hàm cùng tên, không phải lời gọi: `def send_task(self, m)`,
        // `func (c *C) Publish(…)`, `function publish(…)`, phương thức lớp JS
        // `publish(ch, m) {`.
        if i > 0 && matches!(self.word(i - 1), Some("def" | "func" | "function")) {
            return None;
        }
        if self.lang == Lang::Go && i > 0 && self.punct(i - 1, ')') {
            // `func (c *C) Publish(`: tìm `func` trước cụm receiver, lùi tối
            // đa 64 token (TM-42).
            let mut j = i - 1;
            let mut depth = 0usize;
            let stop = i.saturating_sub(64);
            loop {
                match &self.toks[j].tok {
                    Tok::Punct(')') => depth += 1,
                    Tok::Punct('(') => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                if j <= stop {
                    break;
                }
                j -= 1;
            }
            if j > 0 && self.word(j - 1) == Some("func") {
                return None;
            }
        }
        if self.lang == Lang::TypeScript && self.punct(self.close(i + 1) + 1, '{') && (i == 0 || !self.punct(i - 1, '.')) {
            return None;
        }
        let (last, dotted) = match w.rsplit_once('.') {
            Some((_, l)) => (l, true),
            None => (w, false),
        };
        Some((last, dotted || (i > 0 && self.punct(i - 1, '.'))))
    }

    /// Chỉ số ngoặc đóng khớp với ngoặc mở ở `open`.
    fn close(&self, open: usize) -> usize {
        self.close.get(open).copied().unwrap_or(self.toks.len())
    }

    fn opener(&self, j: usize) -> bool {
        matches!(self.toks.get(j).map(|t| &t.tok), Some(Tok::Punct('(' | '[' | '{')))
    }

    /// Tách các phần tử giữa ngoặc mở ở `open` và ngoặc đóng của nó theo dấu
    /// phẩy ở tầng ngoài cùng. Nhảy qua ngoặc lồng bằng bảng ngoặc khớp: mỗi
    /// token chỉ được duyệt bởi ngoặc gần nhất bọc nó (TM-42).
    fn items(&self, open: usize) -> Vec<Arg> {
        let end = self.close(open);
        let mut out = Vec::new();
        let mut start = open + 1;
        let mut j = open + 1;
        while j < end {
            if self.opener(j) {
                j = self.close(j).max(j) + 1;
                continue;
            }
            if self.punct(j, ',') {
                if j > start {
                    out.push(self.arg(start, j));
                }
                start = j + 1;
            }
            j += 1;
        }
        if end > start {
            out.push(self.arg(start, end.min(self.toks.len())));
        }
        out
    }

    /// Cuối giá trị bắt đầu ở `k`: dấu phẩy hay ngoặc đóng cùng tầng.
    fn value_end(&self, mut k: usize, end: usize) -> usize {
        while k < end {
            if self.opener(k) {
                k = self.close(k).max(k) + 1;
                continue;
            }
            if self.punct(k, ',') || matches!(self.toks[k].tok, Tok::Punct(')' | ']' | '}')) {
                break;
            }
            k += 1;
        }
        k.min(end)
    }

    fn arg(&self, a: usize, b: usize) -> Arg {
        // `topic="x"` của Python; `==` không phải.
        if self.lang == Lang::Python
            && b > a + 2
            && let Some(k) = self.word(a)
            && !k.contains('.')
            && self.punct(a + 1, '=')
            && !self.punct(a + 2, '=')
        {
            return Arg { key: Some(k.to_string()), a: a + 2, b };
        }
        Arg { key: None, a, b }
    }

    /// Đối số theo khoá, hay theo vị trí (chỉ tính đối số không khoá).
    fn pick<'x>(&self, args: &'x [Arg], key: &str, pos: Option<usize>) -> Option<&'x Arg> {
        args.iter()
            .find(|x| x.key.as_deref() == Some(key))
            .or_else(|| pos.and_then(|p| args.iter().filter(|x| x.key.is_none()).nth(p)))
    }

    /// Giá trị của trường `key:` trong `{…}` mở ở `open` (object JS, struct Go);
    /// cả thuộc tính viết tắt `{ topic, messages }` của JS (giá trị là biến
    /// cùng tên). `deep`: tìm cả trong ngoặc lồng (`TopicPartition{Topic: …}`),
    /// tối đa 512 token (TM-42).
    fn field(&self, open: usize, key: &str, deep: bool) -> Option<(usize, usize)> {
        let end = self.close(open).min(self.toks.len());
        let limit = if deep { end.min(open + 512) } else { end };
        let mut j = open + 1;
        while j < limit {
            if !deep && self.opener(j) {
                j = self.close(j).max(j) + 1;
                continue;
            }
            if self.word(j) == Some(key) {
                if self.punct(j + 1, ':') && !self.punct(j + 2, '=') {
                    return Some((j + 2, self.value_end(j + 2, end)));
                }
                let lone = |k: usize| self.punct(k, ',') || self.punct(k, '}');
                if self.lang == Lang::TypeScript && (self.punct(j - 1, '{') || self.punct(j - 1, ',')) && lone(j + 1) {
                    return Some((j, j + 1));
                }
            }
            j += 1;
        }
        None
    }

    /// Một hay nhiều tên: danh sách `["a", "b"]`, `[]string{"a"}`, tuple, hay
    /// một giá trị.
    fn names(&self, a: usize, b: usize) -> Vec<Name> {
        if a >= b {
            // Lexer bỏ regex của JS (`topic: /pay.*/`) mà không để token nào.
            return vec![Name::Dynamic("/…/".into())];
        }
        // Go: `[]string{…}`.
        let open = if self.punct(a, '[') && self.punct(a + 1, ']') && self.word(a + 2).is_some() && self.punct(a + 3, '{') {
            Some(a + 3)
        } else if self.punct(a, '[') || (self.lang == Lang::Python && self.punct(a, '(')) {
            Some(a)
        } else {
            None
        };
        match open {
            Some(o) if self.close(o) + 1 == b => self.items(o).iter().map(|x| self.one(x.a, x.b)).collect(),
            _ => vec![self.one(a, b)],
        }
    }

    /// Một giá trị: chuỗi, hằng số, biến môi trường, hay không cố định.
    fn one(&self, mut a: usize, b: usize) -> Name {
        // `&topic` của Go.
        if self.lang == Lang::Go && self.punct(a, '&') {
            a += 1;
        }
        // Tiền tố chuỗi của Python (`f`, `r`, `b`): lexer để nó thành một từ.
        if self.lang == Lang::Python
            && let Some(w) = self.word(a)
            && w.len() <= 2
            && w.chars().all(|c| "rRbBuUfFtT".contains(c))
            && matches!(self.toks.get(a + 1).map(|t| &t.tok), Some(Tok::Str(_)))
        {
            a += 1;
        }
        let toks = &self.toks[a.min(b)..b];
        match toks {
            [t] => match &t.tok {
                Tok::Str(s) | Tok::Template(s) if !s.contains('\0') => return Name::Fixed(s.clone()),
                Tok::Word(w) => {
                    if let Some(found) = self.consts.get(w.as_str()) {
                        return found.clone().unwrap_or_else(|| Name::Dynamic(w.clone()));
                    }
                }
                _ => {}
            },
            _ => {
                if let Some(env) = self.env(a, b) {
                    return Name::Env(env);
                }
                // `q.Name` (Go), `q.queue` (JS): lexer không gộp đường dẫn.
                if let Some(path) = self.path(a, b)
                    && let Some(found) = self.consts.get(&path)
                {
                    return found.clone().unwrap_or(Name::Dynamic(path));
                }
            }
        }
        Name::Dynamic(self.shown(a, b))
    }

    /// `a.b.c` khi khoảng token chỉ gồm từ nối bằng dấu chấm.
    fn path(&self, a: usize, b: usize) -> Option<String> {
        if b <= a || (b - a).is_multiple_of(2) {
            return None;
        }
        let mut out = String::new();
        for j in a..b {
            if (j - a).is_multiple_of(2) {
                out.push_str(self.word(j)?);
            } else if self.punct(j, '.') {
                out.push('.');
            } else {
                return None;
            }
        }
        Some(out)
    }

    /// Tên biến môi trường nếu `a..b` là phép đọc env có tên cố định.
    fn env(&self, a: usize, b: usize) -> Option<String> {
        let s = |i: usize| match self.toks.get(i).map(|t| &t.tok) {
            Some(Tok::Str(s)) if !s.contains('\0') => Some(s.clone()),
            _ => None,
        };
        match self.lang {
            Lang::Python => {
                let w = self.word(a)?;
                if matches!(w, "os.environ" | "environ") && self.punct(a + 1, '[') && b == a + 4 {
                    return s(a + 2);
                }
                if matches!(w, "os.getenv" | "getenv" | "os.environ.get" | "environ.get") && self.punct(a + 1, '(') && self.close(a + 1) + 1 == b {
                    return s(a + 2);
                }
                None
            }
            Lang::Go => {
                // os.Getenv("X")
                (self.word(a) == Some("os") && self.punct(a + 1, '.') && matches!(self.word(a + 2), Some("Getenv")) && self.punct(a + 3, '(') && self.close(a + 3) + 1 == b)
                    .then(|| s(a + 4))
                    .flatten()
            }
            Lang::TypeScript => {
                // process.env.X, process.env["X"]
                if self.word(a) != Some("process") || !self.punct(a + 1, '.') || self.word(a + 2) != Some("env") {
                    return None;
                }
                if self.punct(a + 3, '.') && b == a + 5 {
                    return self.word(a + 4).map(str::to_string);
                }
                if self.punct(a + 3, '[') && b == a + 6 {
                    return s(a + 4);
                }
                None
            }
            Lang::C | Lang::Rust => None,
        }
    }

    /// Cách viết của một khoảng token, rút gọn, để báo cáo.
    fn shown(&self, a: usize, b: usize) -> String {
        let mut out = String::new();
        for t in &self.toks[a.min(b)..b] {
            let piece = match &t.tok {
                Tok::Word(w) => w.clone(),
                Tok::Str(s) | Tok::Template(s) | Tok::Triple(s) => format!("\"{}\"", s.replace('\0', "{…}")),
                Tok::Punct(',') => ", ".to_string(),
                Tok::Punct(c) => c.to_string(),
                Tok::Define(n, _) => n.clone(),
                Tok::SystemStr(s) => s.clone(),
            };
            out.push_str(&piece);
            if out.chars().count() > 60 {
                return out.chars().take(60).collect::<String>() + "…";
            }
        }
        out
    }
}

/// Hằng số chuỗi trong file: `TOPIC = "orders"` (Python, cả `self.topic =`),
/// `const Topic = "orders"`, `topic := "orders"` (Go), `const TOPIC = "orders"`
/// (JS); giá trị cũng có thể là phép đọc env. Chỉ nhận phép gán đứng đầu câu
/// lệnh và không nằm trong ngoặc: `f(topic="x")`, kể cả xuống dòng, không phải
/// hằng số. Một tên gán nhiều giá trị khác nhau, hay còn được ràng buộc ở chỗ
/// khác (tham số hàm, biến vòng lặp, gán nhiều vế, `as`), thì không cố định:
/// hằng số không theo phạm vi, và tên trùng ở phạm vi khác làm sai tên (TM-40).
fn consts(cx: &Cx) -> HashMap<String, Option<Name>> {
    let (toks, lang) = (cx.toks, cx.lang);
    let mut out: HashMap<String, Option<Name>> = HashMap::new();
    let set = |out: &mut HashMap<String, Option<Name>>, key: String, value: Option<Name>| match out.get(&key) {
        Some(old) if *old != value => {
            out.insert(key, None);
        }
        Some(_) => {}
        None => {
            out.insert(key, value);
        }
    };
    for i in 0..toks.len() {
        let Some(name) = cx.word(i) else { continue };
        let starts = cx.depth[i] == 0
            && (i == 0
                || toks[i - 1].line != toks[i].line
                || cx.punct(i - 1, ';')
                || matches!(cx.word(i - 1), Some("const" | "let" | "var")));
        if !starts {
            continue;
        }
        // Vị trí dấu `=`.
        let eq = if cx.punct(i + 1, '=') {
            i + 1
        } else if lang == Lang::Go && cx.punct(i + 1, ':') && cx.punct(i + 2, '=') {
            i + 2
        } else if lang == Lang::Go && cx.punct(i + 1, ',') && cx.word(i + 2).is_some() && cx.punct(i + 3, ':') && cx.punct(i + 4, '=') {
            // `q, err := ch.QueueDeclare(…)`: chỉ để lần tên queue.
            i + 4
        } else if lang == Lang::Go && cx.punct(i + 1, ',') && cx.word(i + 2).is_some() && cx.punct(i + 3, '=') {
            i + 3
        } else if cx.word(i + 1).is_some() && cx.punct(i + 2, '=') && (lang == Lang::Go || lang == Lang::TypeScript) {
            // `const Topic string = "x"`
            i + 2
        } else if lang == Lang::Python && cx.punct(i + 1, ':') && cx.word(i + 2).is_some() && cx.punct(i + 3, '=') {
            // `TOPIC: str = "x"`
            i + 3
        } else if lang == Lang::TypeScript && cx.punct(i + 1, ':') && cx.word(i + 2).is_some() && cx.punct(i + 3, '=') {
            i + 3
        } else {
            continue;
        };
        if cx.punct(eq + 1, '=') {
            continue;
        }
        // Queue khai rồi dùng qua biến: `q, err := ch.QueueDeclare("hello", …)`
        // rồi `q.Name` (Go), `const q = await ch.assertQueue("hello")` rồi
        // `q.queue` (JS), `r = ch.queue_declare(queue="hello")` rồi
        // `r.method.queue` (Python). Tên rỗng là queue do server đặt tên.
        if let Some((field, call)) = declared_queue(cx, eq) {
            let args = cx.items(call + 1);
            let q = cx.pick(&args, "queue", Some(0)).map(|x| cx.one(x.a, x.b));
            let value = match q {
                Some(Name::Fixed(n)) if !n.is_empty() => Some(Name::Fixed(n)),
                _ => None,
            };
            set(&mut out, format!("{name}.{field}"), value);
        }
        // `a, b := …`: không tên nào cố định.
        if cx.punct(i + 1, ',') {
            set(&mut out, name.to_string(), None);
            continue;
        }
        let line = toks[eq].line;
        let mut end = eq + 1;
        // Giá trị nhận được (chuỗi, hằng số, env) ngắn: quét tối đa 32 token,
        // dài hơn thì không cố định (TM-42: dòng dài nhiều phép gán).
        let cap = eq + 1 + 32;
        while end < toks.len() && end < cap && toks[end].line == line && !cx.punct(end, ';') {
            end += 1;
        }
        if end == eq + 1 {
            continue;
        }
        if end == cap && toks.get(end).is_some_and(|t| t.line == line) && !cx.punct(end, ';') {
            set(&mut out, name.to_string(), None);
            continue;
        }
        let value = match cx.one(eq + 1, end) {
            v @ (Name::Fixed(_) | Name::Env(_)) => Some(v),
            _ => None,
        };
        set(&mut out, name.to_string(), value);
    }
    for name in bound(cx) {
        out.insert(name, None);
    }
    out
}

/// Tên được ràng buộc không qua phép gán đơn: tham số hàm, biến vòng lặp,
/// `as`, gán nhiều vế, gán kèm phép toán (`+=`), walrus `:=` của Python.
fn bound(cx: &Cx) -> Vec<String> {
    let toks = cx.toks;
    let mut out = Vec::new();
    // Tên tham số trong danh sách mở ở `open`: tên đứng đầu mỗi mục (Python
    // `topic: str = X` là `topic`, không phải `str` hay `X`); mục JS phân rã
    // `{ topic }` thì mọi tên trong đó; Go `a, b string` thì `a`, `b`, còn
    // `(string, int)` không có tên. Danh sách dài bất thường chỉ xét 256 mục.
    let words_in = |out: &mut Vec<String>, open: usize| {
        let items = cx.items(open);
        let items = &items[..items.len().min(256)];
        // Từ ở tầng ngoài của mục (nhảy qua ngoặc lồng), tối đa 2: đủ để biết
        // tên đứng đầu và mục Go có kiểu đi kèm không. Mục phân rã `{ … }`,
        // `[ … ]` của JS thì mọi tên trong đó, tối đa 256 token (TM-42).
        let words = |x: &Arg| {
            let mut found = Vec::new();
            if cx.punct(x.a, '{') || cx.punct(x.a, '[') {
                for j in x.a..x.b.min(x.a + 256) {
                    if let Some(w) = cx.word(j) {
                        found.push(w);
                    }
                }
                return found;
            }
            let mut j = x.a;
            while j < x.b && found.len() < 2 {
                if cx.opener(j) {
                    j = cx.close(j).max(j) + 1;
                    continue;
                }
                if let Some(w) = cx.word(j) {
                    found.push(w);
                }
                j += 1;
            }
            found
        };
        match cx.lang {
            Lang::Go => {
                let named = items.iter().any(|x| words(x).len() >= 2);
                if named {
                    out.extend(items.iter().filter_map(|x| words(x).first().map(|w| w.to_string())));
                }
            }
            _ => {
                for x in items {
                    if let Some(k) = &x.key {
                        out.push(k.clone());
                    } else if cx.punct(x.a, '{') || cx.punct(x.a, '[') {
                        out.extend(words(x).into_iter().map(str::to_string));
                    } else if let Some(w) = words(x).first() {
                        out.push(w.to_string());
                    }
                }
            }
        }
    };
    for i in 0..toks.len() {
        let Some(w) = cx.word(i) else { continue };
        match (cx.lang, w) {
            (Lang::Python, "def") if cx.word(i + 1).is_some() && cx.punct(i + 2, '(') => words_in(&mut out, i + 2),
            (Lang::Python, "lambda") => {
                let mut j = i + 1;
                while j < toks.len() && j < i + 64 && !cx.punct(j, ':') {
                    if let Some(x) = cx.word(j) {
                        out.push(x.to_string());
                    }
                    j += 1;
                }
            }
            (Lang::Python, "for") | (Lang::Go, "for") => {
                // `for a, b in …`, `for k, v := range …`.
                let mut j = i + 1;
                while j < toks.len()
                    && j < i + 64
                    && !matches!(cx.word(j), Some("in"))
                    && !cx.punct(j, '=')
                    && !cx.punct(j, ';')
                    && !cx.punct(j, '{')
                {
                    if let Some(x) = cx.word(j) {
                        out.push(x.to_string());
                    }
                    j += 1;
                }
            }
            (Lang::Python, "as") => {
                if let Some(x) = cx.word(i + 1) {
                    out.push(x.to_string());
                }
            }
            (Lang::Go, "func") => {
                // `func (r *T) Name(params)`, `func Name(params)`, `func(params)`.
                let mut j = i + 1;
                if cx.punct(j, '(') && cx.word(cx.close(j) + 1).is_some() {
                    words_in(&mut out, j);
                    j = cx.close(j) + 1;
                }
                if cx.word(j).is_some() {
                    j += 1;
                }
                if cx.punct(j, '(') {
                    words_in(&mut out, j);
                }
            }
            (Lang::TypeScript, "function") => {
                let mut j = if cx.word(i + 1).is_some() { i + 2 } else { i + 1 };
                // `function send<T>(topic)`.
                if cx.punct(j, '<') {
                    j = skip_angle(cx, j);
                }
                if cx.punct(j, '(') {
                    words_in(&mut out, j);
                }
            }
            (Lang::TypeScript, "catch") if cx.punct(i + 1, '(') => words_in(&mut out, i + 1),
            (Lang::TypeScript, "for") if cx.punct(i + 1, '(') => {
                // `for (const x of xs)`: tên ngay sau `const`, `let`, `var`.
                for j in i + 2..cx.close(i + 1).min(i + 64) {
                    if matches!(cx.word(j), Some("const" | "let" | "var"))
                        && let Some(x) = cx.word(j + 1)
                    {
                        out.push(x.to_string());
                    }
                }
            }
            _ => {}
        }
        // Gán kèm phép toán: `x += 1`, `x |= …`.
        if toks.get(i + 1).is_some_and(|t| matches!(t.tok, Tok::Punct('+' | '-' | '*' | '/' | '%' | '|' | '&' | '^')))
            && cx.punct(i + 2, '=')
        {
            out.push(w.to_string());
        }
        // Walrus của Python: `(x := …)`.
        if cx.lang == Lang::Python && cx.punct(i + 1, ':') && cx.punct(i + 2, '=') {
            out.push(w.to_string());
        }
        // `a, b = …`, `a, b := …` ở đầu câu lệnh.
        if cx.depth[i] == 0
            && (i == 0 || toks[i - 1].line != toks[i].line)
            && cx.punct(i + 1, ',')
        {
            // Chỉ khi thật gặp `=`: dòng `topic,` trong literal nhiều dòng
            // (`send({\n topic,\n …})`, `[]string{\n Topic,\n}`) không phải phép gán.
            let mut j = i;
            let mut names = Vec::new();
            while j < toks.len() && j < i + 64 && !cx.punct(j, '=') && toks[j].line == toks[i].line {
                if let Some(x) = cx.word(j) {
                    names.push(x.to_string());
                }
                j += 1;
            }
            if cx.punct(j, '=') {
                out.extend(names);
            }
        }
    }
    // JS: tham số của hàm mũi tên `(a, b) =>`, `a =>`, và của phương thức
    // `send(producer, TOPIC) { … }`.
    if cx.lang == Lang::TypeScript {
        for i in 0..toks.len() {
            if cx.punct(i, '(') {
                // Sau `)` có thể là kiểu trả về: `): Promise<void> =>`.
                let c = after_return_type(cx, cx.close(i));
                let arrow = cx.punct(c + 1, '=') && cx.punct(c + 2, '>');
                // Tên đứng trước `(`, bỏ qua generic `name<T>(`.
                let name_at = if i > 0 && cx.punct(i - 1, '>') { back_angle(cx, i - 1) } else { i.checked_sub(1) };
                let method = cx.punct(c + 1, '{')
                    && name_at.is_some_and(|k| {
                        cx.word(k).is_some_and(|w| !matches!(w, "if" | "while" | "switch" | "for" | "with" | "catch"))
                            && !cx.punct(k.saturating_sub(1), '.')
                    });
                if arrow || method {
                    words_in(&mut out, i);
                }
            } else if let Some(w) = cx.word(i)
                && cx.punct(i + 1, '=')
                && cx.punct(i + 2, '>')
            {
                out.push(w.to_string());
            }
        }
    }
    out
}

/// Sau `<` ở `j`: chỉ số ngay sau `>` khớp, tối đa 64 token.
fn skip_angle(cx: &Cx, j: usize) -> usize {
    let mut depth = 0usize;
    for k in j..(j + 64).min(cx.toks.len()) {
        if cx.punct(k, '<') {
            depth += 1;
        } else if cx.punct(k, '>') {
            depth -= 1;
            if depth == 0 {
                return k + 1;
            }
        }
    }
    j
}

/// Từ `>` ở `j` lùi về `<` khớp, tối đa 64 token: chỉ số từ đứng trước `<`.
fn back_angle(cx: &Cx, j: usize) -> Option<usize> {
    let mut depth = 0usize;
    let stop = j.saturating_sub(64);
    let mut k = j;
    loop {
        if cx.punct(k, '>') {
            depth += 1;
        } else if cx.punct(k, '<') {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return k.checked_sub(1);
            }
        }
        if k <= stop {
            return None;
        }
        k -= 1;
    }
}

/// TS: `)` ở `c` có kiểu trả về đi sau (`): Promise<void>`): chỉ số token cuối
/// của kiểu, để xét `=>` hay `{` ngay sau nó; không có thì chính `c`. Kiểu là
/// object (`): { ok: boolean } =>`) thì nhảy qua ngoặc. Tối đa 32 token.
fn after_return_type(cx: &Cx, c: usize) -> usize {
    if cx.lang != Lang::TypeScript || !cx.punct(c + 1, ':') {
        return c;
    }
    let mut k = c + 2;
    let stop = (c + 34).min(cx.toks.len());
    // Độ sâu `<…>`: dấu phẩy trong `Record<string, number>` không kết thúc kiểu.
    let mut angle = 0usize;
    while k < stop {
        if cx.punct(k, '<') {
            angle += 1;
            k += 1;
            continue;
        }
        if cx.punct(k, '>') && angle > 0 && !cx.punct(k - 1, '=') {
            angle -= 1;
            k += 1;
            continue;
        }
        if cx.punct(k, '(') || cx.punct(k, '[') {
            k = cx.close(k).max(k) + 1;
            continue;
        }
        if angle > 0 {
            k += 1;
            continue;
        }
        if cx.punct(k, '=') && cx.punct(k + 1, '>') {
            return k - 1;
        }
        if cx.punct(k, '{') {
            let end = cx.close(k);
            // `{ … } =>` là kiểu object; `{` khác là thân hàm.
            if cx.punct(end + 1, '=') && cx.punct(end + 2, '>') {
                return end;
            }
            return k - 1;
        }
        if cx.punct(k, ';') || cx.punct(k, ',') || cx.punct(k, ')') {
            return c;
        }
        k += 1;
    }
    c
}

/// Biến gán từ hàm dựng producer, consumer Kafka: `X = KafkaProducer(`,
/// `self.p = Producer(` (Python), `const X = kafka.producer(`,
/// `this.consumer = kafka.consumer(` (JS). Tên lấy đoạn cuối.
fn clients(cx: &Cx) -> HashMap<String, Role> {
    let mut out = HashMap::new();
    if cx.lang == Lang::Go {
        // `msg := &sarama.ProducerMessage{}`, `cfg := kafka.ReaderConfig{…}`:
        // biến gán từ struct Kafka, để `msg.Topic = …` biết vai.
        for i in 2..cx.toks.len() {
            let role = match cx.word(i) {
                Some("ProducerMessage" | "Message" | "Writer" | "WriterConfig") => Role::Send,
                Some("ReaderConfig") => Role::Receive,
                _ => continue,
            };
            if !cx.punct(i - 1, '.') || !cx.punct(i + 1, '{') || cx.word(i - 2).is_none() {
                continue;
            }
            let mut k = i - 2;
            if k > 0 && cx.punct(k - 1, '&') {
                k -= 1;
            }
            if k < 2 || !cx.punct(k - 1, '=') {
                continue;
            }
            let at = if cx.punct(k - 2, ':') { k.checked_sub(3) } else { k.checked_sub(2) };
            if let Some(name) = at.and_then(|a| cx.word(a)) {
                out.insert(name.to_string(), role);
            }
        }
        return out;
    }
    if cx.lang != Lang::Python && cx.lang != Lang::TypeScript {
        return out;
    }
    for i in 0..cx.toks.len() {
        if !cx.punct(i, '=') || cx.punct(i + 1, '=') || i == 0 {
            continue;
        }
        // `self.client: KafkaProducer = …`: tên đứng trước chú thích kiểu.
        let at = if i >= 3 && cx.punct(i - 2, ':') && cx.word(i - 3).is_some() { i - 3 } else { i - 1 };
        let Some(name) = cx.word(at) else { continue };
        let name = name.rsplit('.').next().unwrap_or(name);
        let limit = (i + 8).min(cx.toks.len());
        let Some(call) = (i + 1..limit).find(|&j| cx.callee(j).is_some()) else { continue };
        // Hàm dựng phải đứng ngay sau `=` (qua `await`, `new`, `a.b.`): `x if c
        // else KafkaProducer()` thì không chắc.
        let direct = (i + 1..call).all(|k| {
            cx.punct(k, '.') || matches!(cx.word(k), Some("await" | "new")) || (cx.word(k).is_some() && cx.punct(k + 1, '.'))
        });
        if !direct {
            continue;
        }
        let Some((m, _)) = cx.callee(call) else { continue };
        let role = match (cx.lang, m) {
            (Lang::Python, "KafkaProducer" | "AIOKafkaProducer" | "Producer") | (Lang::TypeScript, "producer") => Role::Send,
            (Lang::Python, "KafkaConsumer" | "AIOKafkaConsumer" | "Consumer") | (Lang::TypeScript, "consumer") => Role::Receive,
            _ => continue,
        };
        out.insert(name.to_string(), role);
    }
    out
}

impl Cx<'_> {
    /// Biến đứng trước phương thức ở `i` (đoạn cuối): `producer` của
    /// `this.producer.send(`, `self.p.send(`.
    fn receiver(&self, i: usize) -> Option<&str> {
        let w = self.word(i)?;
        match w.rsplit_once('.') {
            Some((head, _)) => Some(head.rsplit('.').next().unwrap_or(head)),
            None if i >= 2 && self.punct(i - 1, '.') => self.word(i - 2),
            None => None,
        }
    }

    /// Biến ở `i` chắc là client Kafka vai `role`: tên nói vậy (`producer`,
    /// `consumer`), hay được gán từ hàm dựng. `res.send(…)`, `ws.send(…)`,
    /// `obs.subscribe({ next })` thì không.
    fn kafka_client(&self, i: usize, role: Role) -> bool {
        let Some(r) = self.receiver(i) else { return false };
        let lower = r.to_ascii_lowercase();
        let named = match role {
            Role::Send => lower.contains("produc"),
            Role::Receive => lower.contains("consum"),
        };
        named || self.clients.get(r) == Some(&role)
    }
}

/// Phép gán ở `eq` là khai queue: trường chứa tên queue trên
/// biến, và chỉ số tên hàm khai.
fn declared_queue(cx: &Cx, eq: usize) -> Option<(&'static str, usize)> {
    let limit = (eq + 8).min(cx.toks.len());
    let call = (eq + 1..limit).find(|&j| cx.callee(j).is_some())?;
    let (m, _) = cx.callee(call)?;
    match (cx.lang, m) {
        (Lang::Go, "QueueDeclare") => Some(("Name", call)),
        (Lang::TypeScript, "assertQueue") => Some(("queue", call)),
        (Lang::Python, "queue_declare") => Some(("method.queue", call)),
        _ => None,
    }
}

/// RabbitMQ: exchange mặc định (`""`) đưa thẳng tới queue mang tên routing key.
fn rabbit_target(exchange: Name, key: Option<Name>) -> Name {
    match exchange {
        Name::Fixed(x) if x.is_empty() => match key {
            Some(k) => queue_name(k),
            None => Name::Dynamic(String::new()),
        },
        other => exchange_name(other),
    }
}

fn queue_name(n: Name) -> Name {
    match n {
        Name::Fixed(q) => Name::Fixed(format!("q:{q}")),
        Name::Env(v) => Name::Env(format!("q:{v}")),
        other => other,
    }
}

fn exchange_name(n: Name) -> Name {
    match n {
        Name::Fixed(x) => Name::Fixed(format!("x:{x}")),
        Name::Env(v) => Name::Env(format!("x:{v}")),
        other => other,
    }
}

/// Phương thức hàng đợi theo tên, khi thư viện của hệ đó được import: để báo
/// tham chiếu không gọi (`send = p.produce`) và `getattr(p, "produce")`, chỗ
/// mà lời gọi thật không thấy được tên (TM-40).
fn queue_method(lang: Lang, libs: &Libs, m: &str) -> Option<(System, Role)> {
    use {Role::*, System::*};
    match (lang, m) {
        (Lang::Python, "produce" | "send" | "send_and_wait") if libs.kafka => Some((Kafka, Send)),
        (Lang::Python, "subscribe") if libs.kafka => Some((Kafka, Receive)),
        (Lang::Python, "subscribe" | "psubscribe") if libs.redis => Some((Redis, Receive)),
        (Lang::Python, "publish") if libs.redis => Some((Redis, Send)),
        (Lang::Python, "basic_publish") if libs.rabbit => Some((Rabbit, Send)),
        (Lang::Python, "basic_consume" | "queue_bind") if libs.rabbit => Some((Rabbit, Receive)),
        (Lang::Python, "send_task") if libs.celery => Some((Celery, Send)),
        (Lang::TypeScript, "send" | "sendBatch") if libs.kafka => Some((Kafka, Send)),
        (Lang::TypeScript, "subscribe") if libs.kafka => Some((Kafka, Receive)),
        (Lang::TypeScript, "subscribe" | "psubscribe" | "pSubscribe") if libs.redis => Some((Redis, Receive)),
        (Lang::TypeScript, "publish" | "sendToQueue") if libs.rabbit => Some((Rabbit, Send)),
        (Lang::TypeScript, "publish") if libs.redis => Some((Redis, Send)),
        (Lang::TypeScript, "consume" | "bindQueue") if libs.rabbit => Some((Rabbit, Receive)),
        _ => None,
    }
}

fn python_at(cx: &Cx, libs: &Libs, i: usize, out: &mut Queues, push: &mut Push) {
    let line = cx.toks[i].line;
    // `getattr(p, "produce")`: tên phương thức trong chuỗi.
    if cx.word(i).is_some_and(|w| w == "getattr" || w.ends_with(".getattr")) && cx.punct(i + 1, '(') {
        let args = cx.items(i + 1);
        if let Some(Name::Fixed(m)) = args.get(1).map(|x| cx.one(x.a, x.b))
            && let Some((system, role)) = queue_method(Lang::Python, libs, &m)
        {
            let shown = cx.shown(i, (cx.close(i + 1) + 1).min(cx.toks.len()));
            push(out, system, role, vec![Name::Dynamic(shown.clone())], line, &shown, true);
        }
        return;
    }
    // `send = p.produce`: tham chiếu phương thức không gọi.
    if let Some(w) = cx.word(i)
        && let Some((_, m)) = w.rsplit_once('.')
        && !cx.punct(i + 1, '(')
        && !cx.punct(i + 1, '=')
        && !(cx.punct(i + 1, ':') && cx.punct(i + 2, '='))
        && let Some((system, role)) = queue_method(Lang::Python, libs, m)
        && (!matches!(m, "send" | "send_and_wait" | "subscribe") || system != System::Kafka || cx.kafka_client(i, role))
    {
        push(out, system, role, vec![Name::Dynamic(w.to_string())], line, w, true);
        return;
    }
    // Celery: `@app.task`, `@shared_task(name="x")` rồi `def f`.
    if cx.punct(i, '@')
        && let Some(d) = cx.word(i + 1)
        && (d == "shared_task" || d.ends_with(".shared_task") || d.ends_with(".task") || (d == "task" && libs.celery))
    {
        let mut j = i + 2;
        let mut explicit = None;
        if cx.punct(j, '(') {
            let args = cx.items(j);
            if let Some(n) = cx.pick(&args, "name", None) {
                explicit = Some(cx.one(n.a, n.b));
            }
            j = cx.close(j) + 1;
        }
        // Bỏ qua các decorator khác, tới `def`.
        let limit = (j + 64).min(cx.toks.len());
        while j < limit && cx.word(j) != Some("def") {
            j += 1;
        }
        if let (Some("def"), Some(f)) = (cx.word(j), cx.word(j + 1)) {
            let name = explicit.unwrap_or_else(|| Name::Task(f.to_string()));
            push(out, System::Celery, Role::Receive, vec![name], line, &format!("@{d} def {f}"), true);
        }
        return;
    }
    let Some((m, method)) = cx.callee(i) else { return };
    let open = i + 1;
    let shown = || cx.shown(i, (cx.close(open) + 1).min(cx.toks.len()));
    match m {
        "send_task" if method => {
            // `app.send_task("x")` với `app` import từ module khác: tên cố định
            // vẫn tính; tên không cố định chỉ khi file import celery.
            let args = cx.items(open);
            if let Some(a) = cx.pick(&args, "name", Some(0)) {
                push(out, System::Celery, Role::Send, vec![cx.one(a.a, a.b)], line, &shown(), libs.celery);
            }
        }
        "produce" if method => {
            let args = cx.items(open);
            if let Some(a) = cx.pick(&args, "topic", Some(0)) {
                push(out, System::Kafka, Role::Send, vec![cx.one(a.a, a.b)], line, &shown(), libs.kafka);
            }
        }
        "send" | "send_and_wait" if method && libs.kafka => {
            // `send` là tên chung (`ws.send`, `sock.send`): tên không cố định chỉ
            // báo khi biến chắc là producer.
            let args = cx.items(open);
            if let Some(a) = cx.pick(&args, "topic", Some(0)) {
                let strong = cx.kafka_client(i, Role::Send);
                push(out, System::Kafka, Role::Send, vec![cx.one(a.a, a.b)], line, &shown(), strong);
            }
        }
        "KafkaConsumer" | "AIOKafkaConsumer" => {
            let args = cx.items(open);
            let names: Vec<Name> = args.iter().filter(|x| x.key.is_none()).map(|x| cx.one(x.a, x.b)).collect();
            push(out, System::Kafka, Role::Receive, names, line, &shown(), libs.kafka);
        }
        "subscribe" if method => {
            let args = cx.items(open);
            let first = cx.pick(&args, "topics", Some(0));
            // Danh sách là Kafka; chuỗi rời là kênh Redis.
            if let Some(a) = first
                && (cx.punct(a.a, '[') || cx.punct(a.a, '(') || a.key.is_some())
            {
                if libs.kafka {
                    push(out, System::Kafka, Role::Receive, cx.names(a.a, a.b), line, &shown(), true);
                }
            } else if let Some(pattern) = cx.pick(&args, "pattern", None) {
                if libs.kafka {
                    push(out, System::Kafka, Role::Receive, vec![Name::Dynamic(cx.shown(pattern.a, pattern.b))], line, &shown(), true);
                }
            } else if libs.redis {
                // `p.subscribe("news", alerts=handler)`: khoá cũng là tên kênh.
                let names = args
                    .iter()
                    .map(|x| match &x.key {
                        Some(k) => Name::Fixed(k.clone()),
                        None => cx.one(x.a, x.b),
                    })
                    .collect();
                push(out, System::Redis, Role::Receive, names, line, &shown(), true);
            }
        }
        "psubscribe" if method && libs.redis => {
            let args = cx.items(open);
            let names = args
                .iter()
                .map(|x| match (&x.key, cx.one(x.a, x.b)) {
                    (Some(k), _) => Name::Pattern(k.clone()),
                    (None, Name::Fixed(p)) => Name::Pattern(p),
                    (None, other) => other,
                })
                .collect();
            push(out, System::Redis, Role::Receive, names, line, &shown(), true);
        }
        "publish" if method && libs.redis => {
            let args = cx.items(open);
            if args.len() >= 2
                && let Some(a) = cx.pick(&args, "channel", Some(0))
            {
                push(out, System::Redis, Role::Send, vec![cx.one(a.a, a.b)], line, &shown(), true);
            }
        }
        "basic_publish" => {
            let args = cx.items(open);
            let ex = cx.pick(&args, "exchange", Some(0)).map(|x| cx.one(x.a, x.b));
            let key = cx.pick(&args, "routing_key", Some(1)).map(|x| cx.one(x.a, x.b));
            if let Some(ex) = ex {
                push(out, System::Rabbit, Role::Send, vec![rabbit_target(ex, key)], line, &shown(), true);
            }
        }
        "basic_consume" => {
            let args = cx.items(open);
            if let Some(q) = cx.pick(&args, "queue", Some(0)) {
                push(out, System::Rabbit, Role::Receive, vec![queue_name(cx.one(q.a, q.b))], line, &shown(), true);
            }
        }
        "queue_bind" => {
            let args = cx.items(open);
            if let Some(x) = cx.pick(&args, "exchange", Some(1)) {
                push(out, System::Rabbit, Role::Receive, vec![exchange_name(cx.one(x.a, x.b))], line, &shown(), true);
            }
        }
        _ => {}
    }
}

fn go_at(cx: &Cx, libs: &Libs, i: usize, out: &mut Queues, push: &mut Push) {
    let line = cx.toks[i].line;
    // Struct Kafka: `sarama.ProducerMessage{Topic: …}`, `kafka.Message{…}`,
    // `kafka.Writer{…}`, `kafka.ReaderConfig{Topic: …, GroupTopics: …}`.
    if libs.kafka
        && i > 0
        && cx.punct(i - 1, '.')
        && cx.punct(i + 1, '{')
        && let Some(t) = cx.word(i)
    {
        let shown = cx.shown(i.saturating_sub(2), i + 1);
        match t {
            "ProducerMessage" | "Message" | "Writer" | "WriterConfig" => {
                if let Some((a, b)) = cx.field(i + 1, "Topic", t == "Message") {
                    push(out, System::Kafka, Role::Send, cx.names(a, b), line, &shown, true);
                }
            }
            "ReaderConfig" => {
                for key in ["Topic", "GroupTopics"] {
                    if let Some((a, b)) = cx.field(i + 1, key, false) {
                        push(out, System::Kafka, Role::Receive, cx.names(a, b), line, &shown, true);
                    }
                }
            }
            _ => {}
        }
        return;
    }
    // `msg := &sarama.ProducerMessage{}` rồi `msg.Topic = "orders"`; vai theo
    // struct mà biến được gán từ đó. Biến không rõ (struct của miền) thì thôi.
    if libs.kafka
        && cx.word(i) == Some("Topic")
        && i >= 2
        && cx.punct(i - 1, '.')
        && cx.punct(i + 1, '=')
        && !cx.punct(i + 2, '=')
    {
        let Some(&role) = cx.word(i - 2).and_then(|v| cx.clients.get(v)) else { return };
        let line_end = (i + 2..(i + 34).min(cx.toks.len()))
            .find(|&k| cx.toks[k].line != line || cx.punct(k, ';') || cx.punct(k, '}'))
            .unwrap_or((i + 34).min(cx.toks.len()));
        if line_end > i + 2 {
            push(out, System::Kafka, role, vec![cx.one(i + 2, line_end)], line, &cx.shown(i.saturating_sub(2), line_end), true);
        }
        return;
    }
    let Some((m, true)) = cx.callee(i) else { return };
    let open = i + 1;
    let shown = || cx.shown(i.saturating_sub(2), (cx.close(open) + 1).min(cx.toks.len()));
    let args = || cx.items(open);
    let nth = |args: &[Arg], n: usize| args.get(n).map(|x| cx.one(x.a, x.b));
    match m {
        "ConsumePartition" => {
            if let Some(t) = nth(&args(), 0) {
                push(out, System::Kafka, Role::Receive, vec![t], line, &shown(), libs.kafka);
            }
        }
        "SubscribeTopics" if libs.kafka => {
            if let Some(a) = args().first() {
                push(out, System::Kafka, Role::Receive, cx.names(a.a, a.b), line, &shown(), true);
            }
        }
        "Consume" if libs.kafka || libs.rabbit => {
            let args = args();
            // sarama: `group.Consume(ctx, []string{…}, h)`; amqp: `ch.Consume("q", …)`.
            if libs.kafka && args.len() == 3 && args.get(1).is_some_and(|a| cx.punct(a.a, '[')) {
                push(out, System::Kafka, Role::Receive, cx.names(args[1].a, args[1].b), line, &shown(), true);
            } else if libs.rabbit && args.len() >= 6
                && let Some(q) = nth(&args, 0)
            {
                push(out, System::Rabbit, Role::Receive, vec![queue_name(q)], line, &shown(), true);
            }
        }
        "Subscribe" | "PSubscribe" if libs.redis || libs.kafka => {
            let args = args();
            if libs.redis && args.len() >= 2 {
                let names = args[1..]
                    .iter()
                    .map(|x| match cx.one(x.a, x.b) {
                        Name::Fixed(p) if m == "PSubscribe" => Name::Pattern(p),
                        other => other,
                    })
                    .collect();
                push(out, System::Redis, Role::Receive, names, line, &shown(), true);
            } else if libs.kafka && m == "Subscribe" && args.len() == 2 {
                // confluent: `c.Subscribe("t", nil)`.
                push(out, System::Kafka, Role::Receive, vec![cx.one(args[0].a, args[0].b)], line, &shown(), true);
            }
        }
        "Publish" if libs.rabbit || libs.redis => {
            let args = args();
            if libs.rabbit && args.len() == 5 {
                if let (Some(ex), key) = (nth(&args, 0), nth(&args, 1)) {
                    push(out, System::Rabbit, Role::Send, vec![rabbit_target(ex, key)], line, &shown(), true);
                }
            } else if libs.redis && args.len() == 3
                && let Some(ch) = nth(&args, 1)
            {
                push(out, System::Redis, Role::Send, vec![ch], line, &shown(), true);
            }
        }
        "PublishWithContext" | "PublishWithDeferredConfirmWithContext" if libs.rabbit => {
            let args = args();
            if let (Some(ex), key) = (nth(&args, 1), nth(&args, 2)) {
                push(out, System::Rabbit, Role::Send, vec![rabbit_target(ex, key)], line, &shown(), true);
            }
        }
        "QueueBind" if libs.rabbit => {
            if let Some(ex) = nth(&args(), 2) {
                push(out, System::Rabbit, Role::Receive, vec![exchange_name(ex)], line, &shown(), true);
            }
        }
        _ => {}
    }
}

fn node_at(cx: &Cx, libs: &Libs, i: usize, out: &mut Queues, push: &mut Push) {
    let line = cx.toks[i].line;
    // `const send = producer.send`, `producer.send.bind(producer)`: tham chiếu
    // phương thức không gọi.
    if i > 0
        && cx.punct(i - 1, '.')
        && let Some(m) = cx.word(i)
        && !cx.punct(i + 1, '(')
        && !(cx.punct(i + 1, '=') && !cx.punct(i + 2, '='))
        && let Some((system, role)) = queue_method(Lang::TypeScript, libs, m)
        && (system != System::Kafka || cx.kafka_client(i, role))
    {
        let shown = cx.shown(i.saturating_sub(2), i + 1);
        push(out, system, role, vec![Name::Dynamic(shown.clone())], line, &shown, true);
        return;
    }
    let Some((m, true)) = cx.callee(i) else { return };
    let open = i + 1;
    let shown = || cx.shown(i.saturating_sub(2), (cx.close(open) + 1).min(cx.toks.len()));
    let args = || cx.items(open);
    let nth = |args: &[Arg], n: usize| args.get(n).map(|x| cx.one(x.a, x.b));
    let object = |args: &[Arg]| args.first().filter(|a| cx.punct(a.a, '{')).map(|a| a.a);
    match m {
        "send" if libs.kafka => {
            // `producer.send({ topic, messages })`. Không tìm ra tên (`send(rec)`)
            // thì báo, không im lặng (TM-40), khi biến chắc là producer:
            // `res.send(…)`, `ws.send(…)` thì không.
            let args = args();
            let strong = cx.kafka_client(i, Role::Send);
            let name = object(&args)
                .and_then(|o| cx.field(o, "topic", false))
                .map_or_else(|| Name::Dynamic(shown()), |(a, b)| cx.one(a, b));
            push(out, System::Kafka, Role::Send, vec![name], line, &shown(), strong);
        }
        "sendBatch" if libs.kafka => {
            // `{ topicMessages: [{ topic: "a", … }, …] }`: đọc theo cấu trúc,
            // không quét cả khoảng lời gọi (TM-42).
            let args = args();
            let strong = cx.kafka_client(i, Role::Send);
            let mut names = Vec::new();
            if let Some(o) = object(&args)
                && let Some((a, b)) = cx.field(o, "topicMessages", false)
            {
                if cx.punct(a, '[') && cx.close(a) + 1 == b {
                    for item in cx.items(a) {
                        names.push(match cx.punct(item.a, '{').then(|| cx.field(item.a, "topic", false)).flatten() {
                            Some((x, y)) => cx.one(x, y),
                            None => Name::Dynamic(cx.shown(item.a, item.b)),
                        });
                    }
                } else {
                    names.push(Name::Dynamic(cx.shown(a, b)));
                }
            } else {
                names.push(Name::Dynamic(shown()));
            }
            push(out, System::Kafka, Role::Send, names, line, &shown(), strong);
        }
        "subscribe" => {
            let args = args();
            if let Some(o) = object(&args) {
                if libs.kafka {
                    // `obs.subscribe({ next })` của RxJS không phải Kafka.
                    let strong = cx.kafka_client(i, Role::Receive);
                    let mut found = false;
                    for key in ["topic", "topics"] {
                        if let Some((a, b)) = cx.field(o, key, false) {
                            found = true;
                            push(out, System::Kafka, Role::Receive, cx.names(a, b), line, &shown(), strong);
                        }
                    }
                    if !found {
                        push(out, System::Kafka, Role::Receive, vec![Name::Dynamic(shown())], line, &shown(), strong);
                    }
                }
            } else if libs.redis {
                // Đối số cuối có thể là hàm nhận tin.
                let names = args
                    .iter()
                    .map(|x| cx.one(x.a, x.b))
                    .filter(|n| !matches!(n, Name::Dynamic(_)) || args.len() == 1)
                    .collect();
                push(out, System::Redis, Role::Receive, names, line, &shown(), true);
            }
        }
        "psubscribe" | "pSubscribe" if libs.redis => {
            let names = args()
                .iter()
                .filter_map(|x| match cx.one(x.a, x.b) {
                    Name::Fixed(p) => Some(Name::Pattern(p)),
                    _ => None,
                })
                .collect();
            push(out, System::Redis, Role::Receive, names, line, &shown(), true);
        }
        "publish" => {
            let args = args();
            if libs.rabbit && args.len() >= 3 {
                if let (Some(ex), key) = (nth(&args, 0), nth(&args, 1)) {
                    push(out, System::Rabbit, Role::Send, vec![rabbit_target(ex, key)], line, &shown(), true);
                }
            } else if libs.redis && args.len() == 2
                && let Some(ch) = nth(&args, 0)
            {
                push(out, System::Redis, Role::Send, vec![ch], line, &shown(), true);
            }
        }
        "sendToQueue" => {
            if let Some(q) = nth(&args(), 0) {
                push(out, System::Rabbit, Role::Send, vec![queue_name(q)], line, &shown(), libs.rabbit);
            }
        }
        "consume" if libs.rabbit => {
            if let Some(q) = nth(&args(), 0) {
                push(out, System::Rabbit, Role::Receive, vec![queue_name(q)], line, &shown(), true);
            }
        }
        "bindQueue" if libs.rabbit => {
            if let Some(ex) = nth(&args(), 1) {
                push(out, System::Rabbit, Role::Receive, vec![exchange_name(ex)], line, &shown(), true);
            }
        }
        _ => {}
    }
}

/// Tên đầy đủ mặc định của task Celery: `<module>.<hàm>`, module theo đường
/// dẫn file (`shop/tasks.py` → `shop.tasks`).
pub fn celery_default(file: &str, func: &str) -> String {
    let module = file.rsplit_once('.').map_or(file, |(m, _)| m).replace('/', ".");
    let module = module.strip_suffix(".__init__").unwrap_or(&module).to_string();
    format!("{module}.{func}")
}

/// Bên nhận của mọi hệ, tra theo tên bên gửi. Dùng chung cho `check` và
/// `init`.
#[derive(Debug, Default)]
pub struct Index {
    /// (hệ, tên) → chủ (component).
    fixed: HashMap<(System, String), BTreeSet<String>>,
    /// Mẫu `psubscribe` của Redis.
    patterns: Vec<(String, String)>,
    /// Task Celery chưa đặt tên: tên đầy đủ mặc định → chủ.
    tasks: Vec<(String, String)>,
    /// Quá [`MAX_PATTERNS`] mẫu: mẫu sau không xét, bên gọi báo ra.
    pub patterns_cut: bool,
}

/// Mẫu `psubscribe` tối đa trong một lần ghép: mỗi tên bên gửi khớp với mọi
/// mẫu (TM-42). Repo thật có vài chục.
pub const MAX_PATTERNS: usize = 1000;

impl Index {
    /// Thêm một chỗ nhận của `owner` ở `file`. `false` khi tên không cố định
    /// (env, không cố định): bên gọi tự báo.
    pub fn add(&mut self, site: &Site, file: &str, owner: &str) -> bool {
        match &site.name {
            Name::Fixed(n) => {
                self.fixed.entry((site.system, n.clone())).or_default().insert(owner.to_string());
            }
            Name::Pattern(p) => {
                if self.patterns.len() >= MAX_PATTERNS {
                    self.patterns_cut = true;
                } else {
                    self.patterns.push((p.clone(), owner.to_string()));
                }
            }
            Name::Task(f) => self.tasks.push((celery_default(file, f), owner.to_string())),
            Name::Env(_) | Name::Dynamic(_) => return false,
        }
        true
    }

    /// Chủ của mọi bên nhận khớp tên `name` của hệ `system`.
    pub fn targets(&self, system: System, name: &str) -> BTreeSet<String> {
        let mut out = self.fixed.get(&(system, name.to_string())).cloned().unwrap_or_default();
        if system == System::Redis {
            out.extend(self.patterns.iter().filter(|(p, _)| glob(p, name)).map(|(_, c)| c.clone()));
        }
        if system == System::Celery {
            // Bên gửi viết tên tính từ gốc package, đường dẫn file có thể dài
            // hơn (`src/shop/tasks.py`).
            let tail = format!(".{name}");
            out.extend(self.tasks.iter().filter(|(full, _)| full == name || full.ends_with(&tail)).map(|(_, c)| c.clone()));
        }
        out
    }
}

/// Khớp glob kiểu Redis (`*`, `?`) trên byte, như Redis: không đệ quy, không
/// cấp phát, thời gian O(|mẫu|·|tên|).
pub fn glob(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.as_bytes(), name.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while j < n.len() {
        if i < p.len() && (p[i] == b'?' || p[i] == n[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == b'*' {
            star = Some((i, j));
            i += 1;
        } else if let Some((si, sj)) = star {
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == b'*' {
        i += 1;
    }
    i == p.len()
}

/// Phần chữ cố định đứng đầu mẫu glob: mọi tên khớp mẫu đều bắt đầu bằng nó.
pub fn glob_prefix(pattern: &str) -> &str {
    let end = pattern.find(['*', '?', '[']).unwrap_or(pattern.len());
    &pattern[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(path: &str, src: &str) -> Vec<(System, Role, Name)> {
        let lang = Lang::for_path(path).unwrap();
        let toks = lang.tokens(src);
        let imports = lang.imports(src);
        scan(&toks, lang, &imports).sites.into_iter().map(|s| (s.system, s.role, s.name)).collect()
    }

    fn fixed(s: &str) -> Name {
        Name::Fixed(s.into())
    }

    #[test]
    fn python_kafka_rabbit_redis_celery() {
        let got = run(
            "a.py",
            "from confluent_kafka import Producer\nimport pika, redis, os\nfrom celery import shared_task\n\
             TOPIC = 'orders'\nENV_T = os.getenv('PAY_TOPIC', 'pay')\n\
             p.produce(TOPIC, v)\np.produce(topic='audit')\nself.p.produce(f'x-{n}')\n\
             c.subscribe(['orders', ENV_T])\n\
             ch.basic_publish(exchange='', routing_key='jobs', body=b)\nch.basic_publish('logs', '', b)\n\
             ch.basic_consume(queue='jobs', on_message_callback=cb)\nch.queue_bind(exchange='logs', queue=q)\n\
             r.publish('news', m)\nps.subscribe('news', 'alerts')\nps.psubscribe('news.*')\n\
             app.send_task('shop.tasks.charge', args=[1])\n\
             @shared_task\ndef refund(x): pass\n@app.task(name='custom.bill')\ndef bill(): pass\n",
        );
        use Role::*;
        use System::*;
        let want = vec![
            (Kafka, Send, fixed("orders")),
            (Kafka, Send, fixed("audit")),
            (Kafka, Send, Name::Dynamic("\"x-{…}\"".into())),
            (Kafka, Receive, fixed("orders")),
            (Kafka, Receive, Name::Env("PAY_TOPIC".into())),
            (Rabbit, Send, fixed("q:jobs")),
            (Rabbit, Send, fixed("x:logs")),
            (Rabbit, Receive, fixed("q:jobs")),
            (Rabbit, Receive, fixed("x:logs")),
            (Redis, Send, fixed("news")),
            (Redis, Receive, fixed("news")),
            (Redis, Receive, fixed("alerts")),
            (Redis, Receive, Name::Pattern("news.*".into())),
            (Celery, Send, fixed("shop.tasks.charge")),
            (Celery, Receive, Name::Task("refund".into())),
            (Celery, Receive, fixed("custom.bill")),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn python_khong_import_thi_khong_doan() {
        // `send`, `publish`, `subscribe` của thư viện khác.
        let got = run("a.py", "ws.send('hi')\npub.publish(topic, data)\nbus.subscribe('x')\np.produce(name)\n");
        assert!(got.is_empty(), "{got:?}");
        // `produce` với chuỗi cố định vẫn tính: đủ đặc trưng.
        assert_eq!(run("a.py", "p.produce('orders', v)\n"), vec![(System::Kafka, Role::Send, fixed("orders"))]);
    }

    #[test]
    fn go_sarama_kafkago_amqp_redis() {
        let got = run(
            "a.go",
            "package a\nimport (\n\"github.com/IBM/sarama\"\nkafka \"github.com/segmentio/kafka-go\"\namqp \"github.com/rabbitmq/amqp091-go\"\n\"github.com/redis/go-redis/v9\"\n\"os\"\n)\n\
             const Topic = \"orders\"\n\
             func f() {\n\
             m := &sarama.ProducerMessage{Topic: Topic, Value: v}\n\
             w := &kafka.Writer{Addr: a, Topic: \"audit\"}\n\
             r := kafka.NewReader(kafka.ReaderConfig{Brokers: b, GroupTopics: []string{\"orders\", \"x\"}})\n\
             pc, _ := c.ConsumePartition(\"orders\", 0, o)\n\
             g.Consume(ctx, []string{\"pay\"}, h)\n\
             ch.Publish(\"\", \"jobs\", false, false, msg)\n\
             ch.PublishWithContext(ctx, \"logs\", \"\", false, false, msg)\n\
             ch.Consume(\"jobs\", \"\", true, false, false, false, nil)\n\
             ch.QueueBind(q.Name, \"\", \"logs\", false, nil)\n\
             rdb.Publish(ctx, \"news\", m)\n\
             rdb.Subscribe(ctx, os.Getenv(\"CH\"))\n\
             }\n",
        );
        use Role::*;
        use System::*;
        let want = vec![
            (Kafka, Send, fixed("orders")),
            (Kafka, Send, fixed("audit")),
            (Kafka, Receive, fixed("orders")),
            (Kafka, Receive, fixed("x")),
            (Kafka, Receive, fixed("orders")),
            (Kafka, Receive, fixed("pay")),
            (Rabbit, Send, fixed("q:jobs")),
            (Rabbit, Send, fixed("x:logs")),
            (Rabbit, Receive, fixed("q:jobs")),
            (Rabbit, Receive, fixed("x:logs")),
            (Redis, Send, fixed("news")),
            (Redis, Receive, Name::Env("CH".into())),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn node_kafkajs_amqplib_redis() {
        let got = run(
            "a.js",
            "const { Kafka } = require('kafkajs')\nconst amqp = require('amqplib')\nimport { createClient } from 'redis'\n\
             const TOPIC = 'orders'\n\
             await producer.send({ topic: TOPIC, messages })\n\
             await producer.sendBatch({ topicMessages: [{ topic: 'a', messages }, { topic: 'b', messages }] })\n\
             await consumer.subscribe({ topics: ['orders', process.env.EXTRA] })\n\
             await consumer.subscribe({ topic: /pay.*/ })\n\
             ch.sendToQueue('jobs', buf)\nch.publish('logs', '', buf)\nch.consume('jobs', cb)\nch.bindQueue(q.queue, 'logs', '')\n\
             await pub.publish('news', msg)\nawait sub.subscribe('news', (m) => {})\nawait sub.pSubscribe('news.*', cb)\n",
        );
        use Role::*;
        use System::*;
        let want = vec![
            (Kafka, Send, fixed("orders")),
            (Kafka, Send, fixed("a")),
            (Kafka, Send, fixed("b")),
            (Kafka, Receive, fixed("orders")),
            (Kafka, Receive, Name::Env("EXTRA".into())),
            (Kafka, Receive, Name::Dynamic("/…/".into())),
            (Rabbit, Send, fixed("q:jobs")),
            (Rabbit, Send, fixed("x:logs")),
            (Rabbit, Receive, fixed("q:jobs")),
            (Rabbit, Receive, fixed("x:logs")),
            (Redis, Send, fixed("news")),
            (Redis, Receive, fixed("news")),
            (Redis, Receive, Name::Pattern("news.*".into())),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn dinh_nghia_ham_khong_phai_loi_goi() {
        assert!(run("a.py", "def send_task(self, m):\n    self.send_task(m)\n").is_empty());
        assert!(run("a.go", "package a\nimport \"github.com/redis/go-redis/v9\"\nfunc (c *C) Publish(ctx context.Context, ch string, m any) error { return nil }\n").is_empty());
        assert!(run("a.js", "const amqp = require('amqplib')\nclass Bus { publish(ex, key, m) { } consume(q) { } }\nfunction sendToQueue(q, b) {}\n").is_empty());
    }

    #[test]
    fn queue_khai_roi_dung_qua_bien() {
        let got = run(
            "a.go",
            "package a\nimport amqp \"github.com/rabbitmq/amqp091-go\"\nfunc f() {\nq, err := ch.QueueDeclare(\n\"hello\", false, false, false, false, nil)\nch.Publish(\"\", q.Name, false, false, m)\nr, _ := ch.QueueDeclare(\"\", false, false, true, false, nil)\nch.Consume(r.Name, \"\", true, false, false, false, nil)\n}\n",
        );
        assert_eq!(got[0], (System::Rabbit, Role::Send, fixed("q:hello")));
        assert!(matches!(got[1], (System::Rabbit, Role::Receive, Name::Dynamic(_))), "{got:?}");
        let got = run("a.js", "const amqp = require('amqplib')\nconst q = await ch.assertQueue('jobs', {durable: true})\nch.consume(q.queue, cb)\n");
        assert_eq!(got, vec![(System::Rabbit, Role::Receive, fixed("q:jobs"))]);
        let got = run("a.py", "import pika\nr = ch.queue_declare(queue='jobs')\nch.basic_consume(queue=r.method.queue, on_message_callback=cb)\n");
        assert_eq!(got, vec![(System::Rabbit, Role::Receive, fixed("q:jobs"))]);
    }

    #[test]
    fn hang_so_gan_hai_gia_tri_thi_khong_co_dinh() {
        let got = run("a.py", "import kafka\nT = 'a'\nT = 'b'\nproducer.send(T, v)\nf(T='c')\n");
        assert_eq!(got, vec![(System::Kafka, Role::Send, Name::Dynamic("T".into()))]);
    }

    #[test]
    fn glob_redis() {
        assert!(glob("news.*", "news.sport"));
        assert!(glob("n?ws", "news"));
        assert!(glob("*", ""));
        assert!(!glob("news.*", "new"));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(!glob("a*b*c", "aXXbYY"));
    }

    #[test]
    fn tran_moi_file() {
        let src: String = "import redis\n".to_string() + &"r.publish('a', m)\n".repeat(MAX_PER_FILE + 5);
        let lang = Lang::Python;
        let q = scan(&lang.tokens(&src), lang, &lang.imports(&src));
        assert!(q.cut);
        assert_eq!(q.sites.len(), MAX_PER_FILE);
    }

    #[test]
    fn hang_so_khong_theo_pham_vi_thi_khong_co_dinh() {
        // Tham số hàm, biến vòng lặp trùng tên hằng số: không cố định (TM-40).
        let got = run(
            "a.py",
            "import kafka\n\ndef setup(p):\n    topic = 'audit'\n\ndef send(producer, topic):\n    producer.send(topic, 1)\n\nTOPIC2 = 'audit'\nfor TOPIC2 in ['payments']:\n    producer.send(TOPIC2, 1)\n",
        );
        assert!(got.iter().all(|(_, _, n)| matches!(n, Name::Dynamic(_))), "{got:?}");
        // Keyword argument xuống dòng không phải phép gán.
        let got = run("a.py", "import kafka\nx = dict(\n    topic='audit'\n)\ndef f(producer, topic):\n    producer.send(topic, 1)\n");
        assert!(matches!(got[0].2, Name::Dynamic(_)), "{got:?}");
        let got = run(
            "a.go",
            "package a\nimport \"github.com/IBM/sarama\"\nfunc g() { topic := \"heartbeat\"; _ = topic }\nfunc send(topic string) { m := &sarama.ProducerMessage{Topic: topic}; _ = m }\n",
        );
        assert!(matches!(got[0].2, Name::Dynamic(_)), "{got:?}");
        let got = run(
            "a.js",
            "const { Kafka } = require('kafkajs')\nconst TOPIC = 'heartbeat'\nasync function send(producer, TOPIC) { await producer.send({ topic: TOPIC, messages }) }\n",
        );
        assert!(matches!(got[0].2, Name::Dynamic(_)), "{got:?}");
        // Hằng số thật vẫn cố định.
        let got = run("a.js", "const { Kafka } = require('kafkajs')\nconst TOPIC = 'orders'\nasync function send(producer) { await producer.send({ topic: TOPIC, messages }) }\n");
        assert_eq!(got, vec![(System::Kafka, Role::Send, fixed("orders"))]);
    }

    #[test]
    fn nhan_ra_loi_goi_ma_khong_thay_ten_thi_bao() {
        // Thuộc tính viết tắt lấy theo hằng số cùng tên; `send(rec)` không thấy tên.
        let got = run(
            "a.js",
            "const { Kafka } = require('kafkajs')\nconst topic = 'orders'\nawait producer.send({ topic, messages })\nawait producer.send(rec)\nawait consumer.subscribe({ fromBeginning: true })\nconst send = producer.send\n",
        );
        assert_eq!(got[0], (System::Kafka, Role::Send, fixed("orders")));
        assert!(got[1..].iter().all(|(_, _, n)| matches!(n, Name::Dynamic(_))), "{got:?}");
        assert_eq!(got.len(), 4, "{got:?}");
        // Go: gán trường sau khi dựng struct.
        let got = run("a.go", "package a\nimport \"github.com/IBM/sarama\"\nfunc f() { msg := &sarama.ProducerMessage{}\nmsg.Topic = \"payments\" }\n");
        assert_eq!(got, vec![(System::Kafka, Role::Send, fixed("payments"))]);
        // redis-py: kênh trong keyword; getattr, tham chiếu phương thức.
        let got = run(
            "a.py",
            "import redis\nfrom confluent_kafka import Producer\np.subscribe(payments=handler)\np.psubscribe(**{'a': h})\ngetattr(prod, 'produce')('payments', v)\nsend = prod.produce\n",
        );
        assert_eq!(got[0], (System::Redis, Role::Receive, fixed("payments")));
        assert!(got.iter().any(|(s, r, n)| *s == System::Kafka && *r == Role::Send && matches!(n, Name::Dynamic(d) if d.contains("getattr"))), "{got:?}");
        assert!(got.iter().any(|(s, _, n)| *s == System::Kafka && matches!(n, Name::Dynamic(d) if d == "prod.produce")), "{got:?}");
    }

    #[test]
    fn file_long_sau_khong_bac_hai() {
        // Lồng 50 000 tầng: bảng ngoặc khớp tính một lần (TM-42).
        let n = 50_000;
        let cases = [
            ("a.ts", "a(", ")"),
            ("a.py", "producer.send(", ")"),
            ("a.go", "x.Publish(", ")"),
            // Tham số lồng: `func(a func(…`, `(a, (a, … ) => 0`.
            ("a.go", "func(a func(", "))"),
            ("a.ts", "(a, ", ") => 0"),
            ("a.ts", "producer.sendBatch(", ")"),
        ];
        for (path, unit, tail) in cases {
            let head = match path {
                "a.py" => "import kafka\n",
                "a.go" => "package a\nimport \"github.com/redis/go-redis/v9\"\n",
                _ => "const { Kafka } = require('kafkajs')\n",
            };
            let src = format!("{head}{}{}", unit.repeat(n), tail.repeat(n));
            let lang = Lang::for_path(path).unwrap();
            let toks = lang.tokens(&src);
            let imports = lang.imports(&src);
            let t = crate::testclock::Clock::start();
            let _ = scan(&toks, lang, &imports);
            assert!(t.elapsed().as_secs_f64() < 2.0, "{path}: {:?}", t.elapsed());
        }
    }

    #[test]
    fn vong_2_pham_vi_va_nhieu() {
        // Hằng số trong callback, IIFE vẫn là hằng số.
        let got = run(
            "a.js",
            "const { Kafka } = require('kafkajs')\napp.post('/o', async (req, res) => { const topic = 'orders'; await producer.send({ topic, messages }); res.send('ok') })\n",
        );
        assert_eq!(got, vec![(System::Kafka, Role::Send, fixed("orders"))]);
        let got = run(
            "a.go",
            "package a\nimport \"github.com/IBM/sarama\"\nfunc f() {\nhttp.HandleFunc(\"/\", func(w W, r R) {\ntopic := \"orders\"\n_ = &sarama.ProducerMessage{Topic: topic}\n})\n}\n",
        );
        assert_eq!(got, vec![(System::Kafka, Role::Send, fixed("orders"))]);
        // Tham số TS có kiểu trả về, generic: không lấy hằng số cùng tên.
        let got = run(
            "a.ts",
            "import { Kafka } from 'kafkajs'\nconst topic = 'heartbeat'\nexport const publish = async (topic: string, msg: string): Promise<void> => { await producer.send({ topic, messages: [] }) }\nasync function emit<T>(topic: T) { await producer.send({ topic, messages: [] }) }\n",
        );
        assert!(got.iter().all(|(_, _, n)| matches!(n, Name::Dynamic(_))), "{got:?}");
        // `res.send`, `ws.send`, RxJS `subscribe`, tham chiếu trong test jest:
        // không phải Kafka, không báo.
        let got = run(
            "a.js",
            "const { Kafka } = require('kafkajs')\nres.send('ok')\nws.send(JSON.stringify(x))\nres.status(200).send({ ok: true })\nobs.subscribe({ next })\nexpect(api.send).toHaveBeenCalled()\nconst p = kafka.producer()\nawait p.send(rec)\n",
        );
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(matches!(got[0], (System::Kafka, Role::Send, Name::Dynamic(_))), "{got:?}");
        // Go `cfg.Topic =` của ReaderConfig là bên nhận; struct của miền thì không.
        let got = run(
            "a.go",
            "package a\nimport kafka \"github.com/segmentio/kafka-go\"\nfunc f() {\ncfg := kafka.ReaderConfig{Brokers: b}\ncfg.Topic = \"orders\"\ne.Topic = \"payments\"\n}\n",
        );
        assert_eq!(got, vec![(System::Kafka, Role::Receive, fixed("orders"))]);
    }

    #[test]
    fn vong_3() {
        // Literal nhiều dòng (prettier, Go `[]string{`) không phải gán nhiều vế.
        let got = run(
            "a.js",
            "const { Kafka } = require('kafkajs')\nconst topic = 'orders'\nawait producer.send({\n  topic,\n  messages: [],\n})\n",
        );
        assert_eq!(got, vec![(System::Kafka, Role::Send, fixed("orders"))]);
        let got = run(
            "a.go",
            "package a\nimport kafka \"github.com/segmentio/kafka-go\"\nconst OrdersTopic = \"orders\"\nfunc f() {\nr := kafka.NewReader(kafka.ReaderConfig{GroupTopics: []string{\nOrdersTopic,\n\"pay\",\n}})\n_ = r\n}\n",
        );
        assert_eq!(got, vec![(System::Kafka, Role::Receive, fixed("orders")), (System::Kafka, Role::Receive, fixed("pay"))]);
        // Tên từ env vẫn giữ dù biến không rõ là producer.
        let got = run("a.py", "import kafka, os\np = make()\np.send(os.getenv('ORDERS_TOPIC'), v)\np.send(t, v)\n");
        assert_eq!(got, vec![(System::Kafka, Role::Send, Name::Env("ORDERS_TOPIC".into()))]);
        // Client có chú thích kiểu; ternary thì không chắc.
        let got = run("a.py", "import kafka\nclass A:\n    def __init__(self):\n        self.client: KafkaProducer = KafkaProducer()\n    def f(self, t):\n        self.client.send(t, 1)\n");
        assert!(matches!(got[..], [(System::Kafka, Role::Send, Name::Dynamic(_))]), "{got:?}");
        let got = run("a.py", "import kafka\np = x if a else KafkaProducer()\np.send(t, 1)\n");
        assert!(got.is_empty(), "{got:?}");
        // Kiểu trả về generic có dấu phẩy.
        let got = run(
            "a.ts",
            "import { Kafka } from 'kafkajs'\nconst topic = 'heartbeat'\nconst f = async (topic: string): Promise<Record<string, number>> => { await producer.send({ topic, messages: [] }) }\n",
        );
        assert!(matches!(got[..], [(System::Kafka, Role::Send, Name::Dynamic(_))]), "{got:?}");
        // Dòng dài nhiều phép gán: không bậc hai (TM-42).
        for (path, unit) in [("a.js", "let a = "), ("a.go", "var a = ")] {
            let src = format!("{}1\n", unit.repeat(30_000));
            let lang = Lang::for_path(path).unwrap();
            let t = crate::testclock::Clock::start();
            let _ = scan(&lang.tokens(&src), lang, &[]);
            assert!(t.elapsed().as_secs_f64() < 2.0, "{path}: {:?}", t.elapsed());
        }
    }
}
