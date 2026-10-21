//! Đọc HCL, ngôn ngữ của file Terraform `.tf`: một tập con đủ để đánh giá tĩnh
//! các thuộc tính mà tầng triển khai cần (image, biến môi trường, lối vào).
//!
//! Đọc được: khối có nhãn, thuộc tính, chú thích `#` `//` `/* */`, số, `true`
//! `false` `null`, chuỗi có `${…}` (kể cả chuỗi lồng trong `${}`), heredoc
//! `<<EOF` và `<<-EOF`, list, object (khoá là tên, chuỗi hay `(biểu thức)`),
//! tham chiếu `a.b[0].c`, splat `[*]` `.*`, gọi hàm (cả `provider::x::f`), toán
//! tử, `? :`. Biểu thức `for` và chỉ dẫn template `%{…}` được đọc qua cho đúng
//! cú pháp nhưng thành [`E::Unsupported`]: bên đánh giá báo "chưa đọc được" khi
//! cần giá trị của chúng, không đoán (TM-49).
//!
//! Đọc trực tiếp trên byte: mọi ký tự phân cách là ASCII, nên cắt chuỗi ở đó
//! luôn đúng ranh giới UTF-8. Độ lồng có trần [`MAX_DEPTH`] (TM-48).

/// Độ lồng tối đa của khối, biểu thức, template.
pub const MAX_DEPTH: usize = 64;
/// Hạng tử tối đa của một chuỗi toán tử (`1 + 1 + …`): cây lệch dài làm tràn
/// stack khi chép, huỷ (TM-48).
pub const MAX_TERMS: usize = 1000;

#[derive(Debug, Clone, PartialEq)]
pub struct Body {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Attr(Attr),
    Block(Block),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Attr {
    pub name: String,
    pub line: usize,
    pub expr: Expr,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub kind: String,
    pub labels: Vec<String>,
    pub line: usize,
    pub body: Body,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Expr {
    pub line: usize,
    pub e: E,
}

#[derive(Debug, Clone, PartialEq)]
pub enum E {
    Null,
    Bool(bool),
    Num(String),
    /// Chuỗi trong nháy hay heredoc: đoạn chữ và đoạn `${…}`.
    Template(Vec<Part>),
    /// Tham chiếu: tên gốc (`var`, `local`, `aws_db_instance`…) rồi các bước.
    Ref(String, Vec<Seg>),
    /// Gọi hàm: tên, đối số, đối số cuối có `...` không.
    Call(String, Vec<Expr>, bool),
    Tuple(Vec<Expr>),
    Object(Vec<(Expr, Expr)>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
    /// Toán tử một hay hai ngôi.
    Op(&'static str, Vec<Expr>),
    /// Bước truy cập sau một biểu thức không phải tham chiếu: `f(x)[0]`, `(a).b`.
    Get(Box<Expr>, Vec<Seg>),
    /// Cú pháp HCL hợp lệ mà archctl không đánh giá: lý do.
    Unsupported(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Part {
    Lit(String),
    Interp(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Seg {
    Attr(String),
    Index(Expr),
    Splat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub msg: String,
}

impl Body {
    pub fn attr(&self, name: &str) -> Option<&Attr> {
        self.items.iter().find_map(|i| match i {
            Item::Attr(a) if a.name == name => Some(a),
            _ => None,
        })
    }

    pub fn blocks<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = &'a Block> + 'a {
        self.items.iter().filter_map(move |i| match i {
            Item::Block(b) if b.kind == kind => Some(b),
            _ => None,
        })
    }
}

impl Expr {
    /// Chuỗi hằng không có `${}`.
    pub fn literal(&self) -> Option<String> {
        match &self.e {
            E::Template(parts) => {
                let mut s = String::new();
                for p in parts {
                    match p {
                        Part::Lit(t) => s.push_str(t),
                        Part::Interp(_) => return None,
                    }
                }
                Some(s)
            }
            _ => None,
        }
    }
}

/// Đọc một file HCL.
pub fn parse(src: &str) -> Result<Body, Error> {
    let mut p = Parser { s: src.as_bytes(), src, i: 0, lines: line_starts(src) };
    let body = p.body(0, false)?;
    p.ws(true);
    if p.i < p.s.len() {
        return Err(p.err("ký tự thừa sau cùng"));
    }
    Ok(body)
}

fn line_starts(src: &str) -> Vec<usize> {
    std::iter::once(0).chain(src.bytes().enumerate().filter(|(_, b)| *b == b'\n').map(|(i, _)| i + 1)).collect()
}

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
    lines: Vec<usize>,
}

fn ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b >= 0x80
}

fn ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b >= 0x80
}

impl Parser<'_> {
    fn line(&self) -> usize {
        self.line_at(self.i)
    }

    fn line_at(&self, pos: usize) -> usize {
        self.lines.partition_point(|&s| s <= pos)
    }

    fn err(&self, msg: impl Into<String>) -> Error {
        Error { line: self.line(), msg: msg.into() }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn at(&self, lit: &str) -> bool {
        self.s[self.i..].starts_with(lit.as_bytes())
    }

    /// Bỏ khoảng trắng và chú thích; `nl` thì bỏ cả xuống dòng.
    fn ws(&mut self, nl: bool) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\r' => self.i += 1,
                b'\n' if nl => self.i += 1,
                b'#' => self.skip_line(),
                b'/' if self.at("//") => self.skip_line(),
                b'/' if self.at("/*") => {
                    // Chú thích khối được chứa xuống dòng: coi như khoảng trắng.
                    match self.src[self.i + 2..].find("*/") {
                        Some(k) => self.i += 2 + k + 2,
                        None => self.i = self.s.len(),
                    }
                }
                _ => break,
            }
        }
    }

    fn skip_line(&mut self) {
        while let Some(b) = self.peek() {
            if b == b'\n' {
                break;
            }
            self.i += 1;
        }
    }

    fn ident(&mut self) -> Option<String> {
        let start = self.i;
        if !self.peek().is_some_and(ident_start) {
            return None;
        }
        while self.peek().is_some_and(ident_char) {
            self.i += 1;
        }
        Some(self.src[start..self.i].to_string())
    }

    fn expect(&mut self, c: u8, what: &str) -> Result<(), Error> {
        if self.peek() == Some(c) {
            self.i += 1;
            Ok(())
        } else {
            Err(self.err(format!("thiếu `{what}`")))
        }
    }

    fn deep(&self, depth: usize) -> Result<(), Error> {
        if depth > MAX_DEPTH {
            Err(self.err(format!("HCL lồng sâu quá {MAX_DEPTH} cấp")))
        } else {
            Ok(())
        }
    }

    /// Thân khối: thuộc tính và khối con, tới `}` (khi `inner`) hay hết file.
    fn body(&mut self, depth: usize, inner: bool) -> Result<Body, Error> {
        self.deep(depth)?;
        let mut items = Vec::new();
        loop {
            self.ws(true);
            match self.peek() {
                None if inner => return Err(self.err("thiếu `}`")),
                None => break,
                Some(b'}') if inner => break,
                _ => {}
            }
            let line = self.line();
            let Some(name) = self.ident() else {
                return Err(self.err("cần tên thuộc tính hay khối"));
            };
            self.ws(false);
            if self.peek() == Some(b'=') && !self.at("==") {
                self.i += 1;
                let expr = self.expr(depth + 1, false)?;
                items.push(Item::Attr(Attr { name, line, expr }));
                self.end_item()?;
                continue;
            }
            let mut labels = Vec::new();
            loop {
                self.ws(false);
                match self.peek() {
                    Some(b'"') => {
                        let e = self.quoted(depth + 1)?;
                        match e.literal() {
                            Some(l) => labels.push(l),
                            None => return Err(self.err("nhãn của khối không được có `${}`")),
                        }
                    }
                    Some(b) if ident_start(b) => labels.push(self.ident().unwrap_or_default()),
                    Some(b'{') => break,
                    _ => return Err(self.err(format!("cần `=` hay `{{` sau `{name}`"))),
                }
            }
            self.i += 1;
            let body = self.body(depth + 1, true)?;
            self.expect(b'}', "}")?;
            items.push(Item::Block(Block { kind: name, labels, line, body }));
            self.end_item()?;
        }
        Ok(Body { items })
    }

    /// Sau một mục của thân khối: xuống dòng, `}` hay hết file.
    fn end_item(&mut self) -> Result<(), Error> {
        self.ws(false);
        match self.peek() {
            None | Some(b'\n') | Some(b'}') => Ok(()),
            _ => Err(self.err("cần xuống dòng sau thuộc tính hay khối")),
        }
    }

    /// Biểu thức. `nl`: đang trong ngoặc, xuống dòng là khoảng trắng.
    fn expr(&mut self, depth: usize, nl: bool) -> Result<Expr, Error> {
        self.deep(depth)?;
        let cond = self.binary(0, depth, nl)?;
        // Không gặp `?` thì trả vị trí lại: xuống dòng sau giá trị là dấu ngăn
        // mục của object.
        let save = self.i;
        self.ws(nl);
        if self.peek() == Some(b'?') {
            self.i += 1;
            let a = self.expr(depth + 1, nl)?;
            self.ws(nl);
            self.expect(b':', ":")?;
            let b = self.expr(depth + 1, nl)?;
            return Ok(Expr { line: cond.line, e: E::Cond(Box::new(cond), Box::new(a), Box::new(b)) });
        }
        self.i = save;
        Ok(cond)
    }

    fn binary(&mut self, level: usize, depth: usize, nl: bool) -> Result<Expr, Error> {
        const LEVELS: &[&[&str]] = &[&["||"], &["&&"], &["==", "!="], &["<=", ">=", "<", ">"], &["+", "-"], &["*", "/", "%"]];
        // Độ sâu chỉ tăng khi lồng vào cấu trúc con, không theo tầng ưu tiên.
        if level == LEVELS.len() {
            return self.unary(depth, nl);
        }
        let mut left = self.binary(level + 1, depth, nl)?;
        let mut terms = 1usize;
        loop {
            let save = self.i;
            self.ws(nl);
            let Some(op) = LEVELS[level].iter().find(|op| self.at(op)).copied() else {
                self.i = save;
                break;
            };
            // `a => b` là khoá của `for`, không phải `>`.
            if (op == ">" || op == "<") && self.at("=>") {
                self.i = save;
                break;
            }
            self.i += op.len();
            terms += 1;
            if terms > MAX_TERMS {
                return Err(self.err(format!("biểu thức quá {MAX_TERMS} hạng tử")));
            }
            let right = self.binary(level + 1, depth, nl)?;
            left = Expr { line: left.line, e: E::Op(op, vec![left, right]) };
        }
        Ok(left)
    }

    fn unary(&mut self, depth: usize, nl: bool) -> Result<Expr, Error> {
        self.ws(nl);
        let line = self.line();
        for op in ["!", "-"] {
            if self.at(op) && !self.at("!=") {
                self.deep(depth + 1)?;
                self.i += 1;
                let x = self.unary(depth + 1, nl)?;
                return Ok(Expr { line, e: E::Op(op, vec![x]) });
            }
        }
        let base = self.primary(depth, nl)?;
        self.postfix(base, depth, nl)
    }

    /// Bước truy cập sau một giá trị: `.tên`, `.0`, `[biểu thức]`, `[*]`, `.*`.
    fn postfix(&mut self, base: Expr, depth: usize, nl: bool) -> Result<Expr, Error> {
        let mut segs = Vec::new();
        loop {
            // Xuống dòng không nối bước truy cập, kể cả trong ngoặc: `[a\n.b]` hiếm,
            // và HCL cũng không cho.
            let save = self.i;
            self.ws(false);
            if self.peek() == Some(b'.') && !self.at("...") {
                self.i += 1;
                if self.peek() == Some(b'*') {
                    self.i += 1;
                    segs.push(Seg::Splat);
                } else if self.peek().is_some_and(|b| b.is_ascii_digit()) {
                    let line = self.line();
                    let start = self.i;
                    while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                        self.i += 1;
                    }
                    let n = self.src[start..self.i].to_string();
                    segs.push(Seg::Index(Expr { line, e: E::Num(n) }));
                } else if let Some(name) = self.ident() {
                    segs.push(Seg::Attr(name));
                } else {
                    return Err(self.err("cần tên sau `.`"));
                }
            } else if self.peek() == Some(b'[') {
                self.i += 1;
                self.ws(true);
                if self.peek() == Some(b'*') {
                    self.i += 1;
                    self.ws(true);
                    self.expect(b']', "]")?;
                    segs.push(Seg::Splat);
                } else {
                    let idx = self.expr(depth + 1, true)?;
                    self.ws(true);
                    self.expect(b']', "]")?;
                    segs.push(Seg::Index(idx));
                }
            } else {
                self.i = save;
                break;
            }
            let _ = nl;
        }
        if segs.is_empty() {
            return Ok(base);
        }
        let line = base.line;
        Ok(match base.e {
            E::Ref(root, mut s) => {
                s.extend(segs);
                Expr { line, e: E::Ref(root, s) }
            }
            e => Expr { line, e: E::Get(Box::new(Expr { line, e }), segs) },
        })
    }

    fn primary(&mut self, depth: usize, nl: bool) -> Result<Expr, Error> {
        self.deep(depth)?;
        self.ws(nl);
        let line = self.line();
        match self.peek() {
            None => Err(self.err("thiếu biểu thức")),
            Some(b'"') => self.quoted(depth + 1),
            Some(b'<') if self.at("<<") => self.heredoc(depth + 1),
            Some(b'(') => {
                self.i += 1;
                let x = self.expr(depth + 1, true)?;
                self.ws(true);
                self.expect(b')', ")")?;
                Ok(x)
            }
            Some(b'[') => self.tuple(depth + 1),
            Some(b'{') => self.object(depth + 1),
            Some(b) if b.is_ascii_digit() => {
                let start = self.i;
                while self.peek().is_some_and(|b| b.is_ascii_digit() || b == b'.') {
                    self.i += 1;
                }
                if self.peek().is_some_and(|b| b == b'e' || b == b'E') {
                    self.i += 1;
                    if self.peek().is_some_and(|b| b == b'+' || b == b'-') {
                        self.i += 1;
                    }
                    while self.peek().is_some_and(|b| b.is_ascii_digit()) {
                        self.i += 1;
                    }
                }
                Ok(Expr { line, e: E::Num(self.src[start..self.i].to_string()) })
            }
            Some(b) if ident_start(b) => {
                let mut name = self.ident().unwrap_or_default();
                // Hàm của provider: `provider::aws::arn_parse(…)`.
                while self.at("::") {
                    self.i += 2;
                    match self.ident() {
                        Some(n) => {
                            name.push_str("::");
                            name.push_str(&n);
                        }
                        None => return Err(self.err("cần tên sau `::`")),
                    }
                }
                let save = self.i;
                self.ws(false);
                if self.peek() == Some(b'(') {
                    self.i += 1;
                    return self.call(name, line, depth + 1);
                }
                self.i = save;
                Ok(Expr {
                    line,
                    e: match name.as_str() {
                        "true" => E::Bool(true),
                        "false" => E::Bool(false),
                        "null" => E::Null,
                        _ => E::Ref(name, Vec::new()),
                    },
                })
            }
            Some(_) => Err(self.err("biểu thức không đọc được")),
        }
    }

    fn call(&mut self, name: String, line: usize, depth: usize) -> Result<Expr, Error> {
        let mut args = Vec::new();
        let mut expand = false;
        loop {
            self.ws(true);
            if self.peek() == Some(b')') {
                self.i += 1;
                break;
            }
            args.push(self.expr(depth, true)?);
            self.ws(true);
            if self.at("...") {
                self.i += 3;
                expand = true;
                self.ws(true);
            }
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b')') => {}
                _ => return Err(self.err(format!("cần `,` hay `)` trong `{name}(…)`"))),
            }
        }
        Ok(Expr { line, e: E::Call(name, args, expand) })
    }

    /// `for` đứng ngay sau `[` hay `{`: đọc qua tới ngoặc đóng.
    fn is_for(&self) -> bool {
        let rest = &self.s[self.i..];
        rest.starts_with(b"for") && rest.get(3).is_some_and(|b| !ident_char(*b))
    }

    fn tuple(&mut self, depth: usize) -> Result<Expr, Error> {
        let line = self.line();
        self.i += 1;
        self.ws(true);
        if self.is_for() {
            self.skip_balanced(b'[', b']', depth + 1)?;
            return Ok(Expr { line, e: E::Unsupported("biểu thức `for`") });
        }
        let mut items = Vec::new();
        loop {
            self.ws(true);
            if self.peek() == Some(b']') {
                self.i += 1;
                break;
            }
            items.push(self.expr(depth, true)?);
            self.ws(true);
            match self.peek() {
                Some(b',') => self.i += 1,
                Some(b']') => {}
                _ => return Err(self.err("cần `,` hay `]`")),
            }
        }
        Ok(Expr { line, e: E::Tuple(items) })
    }

    fn object(&mut self, depth: usize) -> Result<Expr, Error> {
        let line = self.line();
        self.i += 1;
        self.ws(true);
        if self.is_for() {
            self.skip_balanced(b'{', b'}', depth + 1)?;
            return Ok(Expr { line, e: E::Unsupported("biểu thức `for`") });
        }
        let mut items = Vec::new();
        loop {
            self.ws(true);
            if self.peek() == Some(b'}') {
                self.i += 1;
                break;
            }
            // Khoá: tên trần là chuỗi hằng (không phải tham chiếu), như HCL.
            let kline = self.line();
            let key = match self.peek() {
                Some(b) if ident_start(b) => {
                    let save = self.i;
                    let name = self.ident().unwrap_or_default();
                    self.ws(false);
                    if self.peek() == Some(b'=') && !self.at("==") || self.peek() == Some(b':') {
                        Expr { line: kline, e: E::Template(vec![Part::Lit(name)]) }
                    } else {
                        self.i = save;
                        self.expr(depth, true)?
                    }
                }
                _ => self.expr(depth, true)?,
            };
            self.ws(true);
            match self.peek() {
                Some(b'=') | Some(b':') => self.i += 1,
                _ => return Err(self.err("cần `=` hay `:` trong object")),
            }
            let val = self.expr(depth, true)?;
            items.push((key, val));
            self.ws(false);
            match self.peek() {
                Some(b',') | Some(b'\n') => self.i += 1,
                Some(b'}') => {}
                _ => {
                    self.ws(true);
                    if self.peek() != Some(b'}') {
                        return Err(self.err("cần `,`, xuống dòng hay `}` trong object"));
                    }
                }
            }
        }
        Ok(Expr { line, e: E::Object(items) })
    }

    /// Bỏ qua tới ngoặc đóng tương ứng (đang đứng sau ngoặc mở), qua chuỗi,
    /// heredoc và chú thích lồng bên trong: ngoặc trong chúng không được đếm,
    /// nếu không một heredoc có `[` giấu được cả các khối sau (TM-49).
    fn skip_balanced(&mut self, open: u8, close: u8, depth: usize) -> Result<(), Error> {
        self.deep(depth)?;
        let mut level = 1usize;
        while let Some(b) = self.peek() {
            match b {
                b'"' => {
                    self.quoted(depth + 1)?;
                    continue;
                }
                // Heredoc hỏng thì cả file hỏng: quay về đếm ngoặc trên chữ thô
                // thì một heredoc cố ý làm hỏng giấu được các khối sau (TM-49).
                b'<' if self.at("<<") => {
                    self.heredoc(depth + 1)?;
                    continue;
                }
                b'/' if self.at("/*") => {
                    match self.src[self.i + 2..].find("*/") {
                        Some(k) => self.i += 2 + k + 2,
                        None => self.i = self.s.len(),
                    }
                    continue;
                }
                b'#' => self.skip_line(),
                b'/' if self.at("//") => self.skip_line(),
                _ if b == open => level += 1,
                _ if b == close => {
                    level -= 1;
                    if level == 0 {
                        self.i += 1;
                        return Ok(());
                    }
                }
                _ => {}
            }
            self.i += 1;
        }
        Err(self.err(format!("thiếu `{}`", close as char)))
    }

    /// Chuỗi trong nháy kép, có `${…}`.
    fn quoted(&mut self, depth: usize) -> Result<Expr, Error> {
        self.deep(depth)?;
        let line = self.line();
        self.i += 1;
        let mut parts = Vec::new();
        let mut lit = String::new();
        let mut unsupported = false;
        loop {
            let Some(b) = self.peek() else {
                return Err(Error { line, msg: "chuỗi thiếu `\"` đóng".into() });
            };
            match b {
                b'"' => {
                    self.i += 1;
                    break;
                }
                b'\n' => return Err(Error { line, msg: "chuỗi thiếu `\"` đóng".into() }),
                b'\\' => {
                    self.i += 1;
                    match self.peek() {
                        Some(b'n') => lit.push('\n'),
                        Some(b't') => lit.push('\t'),
                        Some(b'r') => lit.push('\r'),
                        Some(b'"') => lit.push('"'),
                        Some(b'\\') => lit.push('\\'),
                        Some(b'u') | Some(b'U') => {
                            let n = if self.peek() == Some(b'u') { 4 } else { 8 };
                            let hex = self.src.get(self.i + 1..self.i + 1 + n).unwrap_or("");
                            let c = u32::from_str_radix(hex, 16).ok().and_then(char::from_u32);
                            match c {
                                Some(c) if hex.len() == n => {
                                    lit.push(c);
                                    self.i += n;
                                }
                                _ => return Err(self.err("`\\u` không hợp lệ")),
                            }
                        }
                        _ => return Err(self.err("ký tự thoát không hợp lệ")),
                    }
                    self.i += 1;
                }
                b'$' if self.at("$${") => {
                    lit.push_str("${");
                    self.i += 3;
                }
                b'%' if self.at("%%{") => {
                    lit.push_str("%{");
                    self.i += 3;
                }
                b'$' if self.at("${") => {
                    self.i += 2;
                    if !lit.is_empty() {
                        parts.push(Part::Lit(std::mem::take(&mut lit)));
                    }
                    parts.push(Part::Interp(self.interp(depth)?));
                }
                b'%' if self.at("%{") => {
                    self.i += 2;
                    self.skip_balanced(b'{', b'}', depth + 1)?;
                    unsupported = true;
                }
                _ => {
                    let start = self.i;
                    while self.peek().is_some_and(|c| !matches!(c, b'"' | b'\\' | b'$' | b'%' | b'\n')) {
                        self.i += 1;
                    }
                    if self.i == start {
                        // `$` hay `%` không mở `{`: ký tự thường.
                        self.i += 1;
                    }
                    lit.push_str(&self.src[start..self.i]);
                }
            }
        }
        if unsupported {
            return Ok(Expr { line, e: E::Unsupported("chỉ dẫn template `%{…}`") });
        }
        if !lit.is_empty() || parts.is_empty() {
            parts.push(Part::Lit(lit));
        }
        Ok(Expr { line, e: E::Template(parts) })
    }

    /// Phần trong `${…}` (đã qua `${`), có thể có `~` cắt khoảng trắng.
    fn interp(&mut self, depth: usize) -> Result<Expr, Error> {
        self.ws(true);
        if self.peek() == Some(b'~') {
            self.i += 1;
        }
        let x = self.expr(depth, true)?;
        self.ws(true);
        if self.peek() == Some(b'~') {
            self.i += 1;
        }
        self.expect(b'}', "}")?;
        Ok(x)
    }

    /// Heredoc: `<<EOF` hay `<<-EOF` (bỏ thụt lề chung), nội dung là template.
    fn heredoc(&mut self, depth: usize) -> Result<Expr, Error> {
        self.deep(depth)?;
        let line = self.line();
        self.i += 2;
        let strip = self.peek() == Some(b'-');
        if strip {
            self.i += 1;
        }
        let Some(marker) = self.ident() else {
            return Err(self.err("heredoc thiếu tên đánh dấu"));
        };
        self.ws(false);
        if self.peek() != Some(b'\n') {
            return Err(self.err("heredoc: cần xuống dòng sau tên đánh dấu"));
        }
        self.i += 1;
        let start = self.i;
        // Tìm dòng chỉ có tên đánh dấu (có thể thụt lề).
        let mut pos = start;
        let end = loop {
            let eol = self.src[pos..].find('\n').map_or(self.s.len(), |k| pos + k);
            if self.src[pos..eol].trim() == marker {
                break pos;
            }
            if eol >= self.s.len() {
                return Err(Error { line, msg: format!("heredoc thiếu dòng đóng `{marker}`") });
            }
            pos = eol + 1;
        };
        let content = &self.src[start..end];
        let indent = if strip {
            content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| l.len() - l.trim_start_matches([' ', '\t']).len())
                .min()
                .unwrap_or(0)
        } else {
            0
        };
        // Đọc nội dung như template, dòng theo vị trí thật trong file.
        let mut parts = Vec::new();
        let mut lit = String::new();
        let mut unsupported = false;
        let mut at_line_start = true;
        self.i = start;
        while self.i < end {
            if at_line_start && indent > 0 {
                let mut k = 0;
                while k < indent && self.i < end && matches!(self.s[self.i], b' ' | b'\t') {
                    self.i += 1;
                    k += 1;
                }
                at_line_start = false;
                continue;
            }
            if self.at("$${") {
                lit.push_str("${");
                self.i += 3;
            } else if self.at("%%{") {
                lit.push_str("%{");
                self.i += 3;
            } else if self.at("${") {
                self.i += 2;
                if !lit.is_empty() {
                    parts.push(Part::Lit(std::mem::take(&mut lit)));
                }
                parts.push(Part::Interp(self.interp(depth)?));
                if self.i > end {
                    return Err(self.err("`${` của heredoc vượt dòng đóng"));
                }
            } else if self.at("%{") {
                self.i += 2;
                self.skip_balanced(b'{', b'}', depth + 1)?;
                unsupported = true;
            } else {
                let c_start = self.i;
                while self.i < end && !matches!(self.s[self.i], b'$' | b'%' | b'\n') {
                    self.i += 1;
                }
                if self.i < end && self.s[self.i] == b'\n' {
                    self.i += 1;
                    at_line_start = true;
                } else if self.i == c_start {
                    self.i += 1;
                }
                lit.push_str(&self.src[c_start..self.i]);
            }
        }
        // Qua dòng đóng.
        self.i = self.src[end..].find('\n').map_or(self.s.len(), |k| end + k);
        if unsupported {
            return Ok(Expr { line, e: E::Unsupported("chỉ dẫn template `%{…}`") });
        }
        if !lit.is_empty() || parts.is_empty() {
            parts.push(Part::Lit(lit));
        }
        Ok(Expr { line, e: E::Template(parts) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(e: &Expr) -> String {
        e.literal().unwrap_or_else(|| panic!("không phải chuỗi hằng: {e:?}"))
    }

    #[test]
    fn khoi_thuoc_tinh_va_tham_chieu() {
        let b = parse(
            "# chú thích\nresource \"aws_ecs_task_definition\" \"api\" {\n  family = \"api\" // cuối dòng\n  cpu    = 256\n  /* khối */ tags = { Name = \"x\", \"a-b\" = 1 }\n  depends_on = [aws_db_instance.main]\n  env = aws_db_instance.main.address\n}\n",
        )
        .unwrap();
        let Item::Block(r) = &b.items[0] else { panic!() };
        assert_eq!((r.kind.as_str(), r.labels.clone(), r.line), ("resource", vec!["aws_ecs_task_definition".into(), "api".into()], 2));
        assert_eq!(lit(&r.body.attr("family").unwrap().expr), "api");
        assert_eq!(r.body.attr("cpu").unwrap().expr.e, E::Num("256".into()));
        let env = &r.body.attr("env").unwrap();
        assert_eq!(env.line, 7);
        assert_eq!(env.expr.e, E::Ref("aws_db_instance".into(), vec![Seg::Attr("main".into()), Seg::Attr("address".into())]));
        let E::Object(tags) = &r.body.attr("tags").unwrap().expr.e else { panic!() };
        assert_eq!(lit(&tags[1].0), "a-b");
    }

    #[test]
    fn template_long_va_heredoc() {
        let b = parse(
            "a = \"http://${aws_lb.x.dns_name}:${var.port}/${lower(\"A${local.b}\")}\"\nb = <<-EOF\n    [{\"name\": \"api\",\n      \"value\": \"${aws_db_instance.main.address}\"}]\n    EOF\nc = \"$${literal} 100%\"\n",
        )
        .unwrap();
        let E::Template(parts) = &b.attr("a").unwrap().expr.e else { panic!() };
        assert_eq!(parts.len(), 6);
        assert_eq!(parts[0], Part::Lit("http://".into()));
        let E::Template(h) = &b.attr("b").unwrap().expr.e else { panic!() };
        assert_eq!(h[0], Part::Lit("[{\"name\": \"api\",\n  \"value\": \"".into()));
        assert!(matches!(&h[1], Part::Interp(Expr { line: 4, .. })));
        assert_eq!(lit(&b.attr("c").unwrap().expr), "${literal} 100%");
    }

    #[test]
    fn ham_toan_tu_va_for() {
        let b = parse(
            "a = jsonencode([{ name = \"x\", n = 1 + 2 * 3 }])\nb = var.on ? \"y\" : \"n\"\nc = [for s in var.list : upper(s) if s != \"\"]\nd = { for k, v in var.m : k => v }\ne = provider::aws::arn_parse(var.arn).region\nf = \"%{ if var.x }a%{ endif }\"\ng = aws_instance.web[*].id\nh = merge(local.a, local.b...)\n",
        )
        .unwrap();
        assert!(matches!(&b.attr("a").unwrap().expr.e, E::Call(n, _, false) if n == "jsonencode"));
        assert!(matches!(&b.attr("b").unwrap().expr.e, E::Cond(..)));
        assert_eq!(b.attr("c").unwrap().expr.e, E::Unsupported("biểu thức `for`"));
        assert_eq!(b.attr("d").unwrap().expr.e, E::Unsupported("biểu thức `for`"));
        assert!(matches!(&b.attr("e").unwrap().expr.e, E::Get(..)));
        assert!(matches!(&b.attr("f").unwrap().expr.e, E::Unsupported(_)));
        assert!(matches!(&b.attr("g").unwrap().expr.e, E::Ref(_, s) if s.contains(&Seg::Splat)));
        assert!(matches!(&b.attr("h").unwrap().expr.e, E::Call(_, _, true)));
    }

    #[test]
    fn khoi_mot_dong_va_object_nhieu_dong() {
        let b = parse("variable \"x\" { default = 1 }\nlocals {\n  m = {\n    a = 1\n    b = \"2\"\n  }\n  l = [\n    \"a\",\n    \"b\",\n  ]\n}\n").unwrap();
        assert_eq!(b.items.len(), 2);
        let Item::Block(l) = &b.items[1] else { panic!() };
        assert!(matches!(&l.body.attr("m").unwrap().expr.e, E::Object(v) if v.len() == 2));
        assert!(matches!(&l.body.attr("l").unwrap().expr.e, E::Tuple(v) if v.len() == 2));
    }

    #[test]
    fn loi_co_dong() {
        for (src, line) in [("a = \"x\n", 1), ("a {\n b = 1\n", 3), ("a = (1\n", 2), ("x = <<EOF\nabc\n", 1), ("a = 1 b = 2\n", 1)] {
            let e = parse(src).unwrap_err();
            assert_eq!(e.line, line, "{src:?}: {e:?}");
        }
    }

    #[test]
    fn long_sau_co_tran() {
        let deep = format!("a = {}1{}\n", "[".repeat(200), "]".repeat(200));
        assert!(parse(&deep).unwrap_err().msg.contains("lồng sâu"));
        let deep = format!("a = \"{}\"\n", "${\"".repeat(100) + &"\"}".repeat(100));
        assert!(parse(&deep).is_err());
        let blocks = format!("{}{}", "a {\n".repeat(100), "}\n".repeat(100));
        assert!(parse(&blocks).unwrap_err().msg.contains("lồng sâu"));
    }

    #[test]
    fn duong_tran_stack_cua_review_thanh_loi() {
        // `!` lặp, `%{` lồng trong chuỗi, chuỗi toán tử rất dài: lỗi có dòng,
        // không tràn stack (TM-48).
        for src in [
            format!("a = {}true\n", "!".repeat(20_000)),
            format!("a = {}\n", "\"%{".repeat(20_000)),
            format!("a = [for x in [] : {}]\n", "\"%{".repeat(20_000)),
            format!("a = 1{}\n", "+1".repeat(200_000)),
        ] {
            assert!(parse(&src).is_err());
        }
        assert!(parse(&format!("a = 1{}\n", "+1".repeat(500))).is_ok());
    }

    #[test]
    fn for_bo_qua_heredoc_va_chu_thich() {
        // Ngoặc trong heredoc, chú thích khối nằm trong `for` không được đếm:
        // khối sau vẫn được đọc (TM-49).
        let src = "locals {\n  x = [for s in [] : <<EOF\n[\nEOF\n  ]\n  y = { for k, v in {} : k => v /* } */ }\n}\nresource \"aws_lambda_function\" \"hidden\" {\n  image_uri = \"x\"\n}\n";
        let b = parse(src).unwrap();
        assert!(b.blocks("resource").any(|r| r.labels[1] == "hidden"), "{b:?}");
        // Heredoc cố ý làm hỏng trong `for`: cả file lỗi, không quay về đếm
        // ngoặc trên chữ thô rồi giấu khối sau.
        let bad = format!("locals {{\n  x = [for s in [] : <<EOF\n${{1{}}}\n[\nEOF\n  ]\n}}\nresource \"aws_lambda_function\" \"hidden\" {{}}\n", "+1".repeat(1100));
        assert!(parse(&bad).is_err());
    }

    #[test]
    fn utf8_va_dau_cuoi_khong_panic() {
        for src in ["a = \"chữ việt ${x}\"\n", "tên = 1\n", "a = \"\\u00e9\"\n", "a = <<EOF\nđ\nEOF", "a = \"$", "a = \"%", "a = [", "a = {", "a = -", "a"] {
            let _ = parse(src);
        }
        assert_eq!(lit(&parse("a = \"\\u00e9\"\n").unwrap().attr("a").unwrap().expr), "é");
    }
}
