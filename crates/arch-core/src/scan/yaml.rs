//! Bộ đọc YAML tập con, đủ cho docker-compose và manifest Kubernetes.
//!
//! Không có dependency nào ngoài std, như phần còn lại của crate. Những gì đọc
//! được: mapping và sequence dạng khối (kể cả sequence cùng cột với khoá cha),
//! flow `[...]` và `{...}`, chuỗi thường, chuỗi nháy đơn, nháy kép, block scalar
//! `|` và `>`, anchor, alias, khoá gộp `<<`, nhiều tài liệu nối bằng `---`.
//! Cách gập dòng của chuỗi chép theo PyYAML, và bộ đo đối chiếu với nó.
//!
//! Không đọc: khoá phức `? `, tag có nghĩa (đọc rồi bỏ qua), alias trỏ ngược
//! lên node đang dựng. Gặp chúng thì trả lỗi kèm dòng, không đoán.
//!
//! File đầu vào là của người khác hoặc do agent viết, và bộ đọc chạy trong
//! hook sau mỗi lần sửa file, nên ba chốt chặn (TM-8 trong threat model):
//! - alias là tham chiếu tới node đã có, không sao chép; anchor chỉ được đăng
//!   ký khi node của nó đã dựng xong, nên không có vòng;
//! - khoá gộp `<<` sao chép mục trong một ngân sách chung cho cả file;
//! - độ lồng có giới hạn, cho cả khối lẫn flow.
//!
//! Thông báo lỗi chỉ có dòng và cột, không trích nội dung: dòng hỏng có thể
//! là `OPENAI_API_KEY=sk-…`, và thông báo đi vào log CI (TM-10).

use std::collections::{HashMap, HashSet};

/// Lồng sâu hơn mức này thì dừng, không để đệ quy làm tràn stack.
pub const MAX_DEPTH: usize = 64;
/// Ngân sách gộp `<<` cho một file: mỗi mục sao chép tính một, cộng độ dài
/// khoá của nó. Đếm cả byte vì một khoá 100 KB gộp vài nghìn lần cũng đủ ăn
/// hết bộ nhớ dù số mục còn nhỏ.
pub const MAX_MERGED: usize = 2_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub line: usize,
    pub msg: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dòng {}: {}", self.line, self.msg)
    }
}

/// Các tài liệu trong một file YAML.
#[derive(Debug, Clone, Default)]
pub struct Yaml {
    nodes: Vec<Raw>,
    /// Node là vô hướng viết trần (không nháy, không `|`/`>`): `false`, `3` là
    /// bool, số; `"false"` là chuỗi. Template Helm cần phân biệt. Mảng riêng để
    /// `Raw` không phình ra.
    plain: Vec<bool>,
    docs: Vec<usize>,
}

#[derive(Debug, Clone)]
struct Raw {
    line: usize,
    value: Value,
}

#[derive(Debug, Clone)]
enum Value {
    Null,
    Str(String),
    Seq(Vec<usize>),
    Map(Vec<Entry>),
}

#[derive(Debug, Clone)]
struct Entry {
    key: String,
    line: usize,
    value: usize,
}

/// Một node, nhìn từ bên ngoài.
#[derive(Clone, Copy)]
pub struct Node<'a> {
    yaml: &'a Yaml,
    index: usize,
}

impl std::fmt::Debug for Node<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Node@{}", self.line())
    }
}

impl Yaml {
    pub fn documents(&self) -> impl Iterator<Item = Node<'_>> {
        self.docs
            .iter()
            .map(move |&index| Node { yaml: self, index })
    }

    /// Cả cây dưới dạng JSON, mỗi tài liệu một phần tử. Mọi giá trị là chuỗi
    /// hay `null`, như bộ đọc thấy chúng. Dùng để đối chiếu với bộ đọc khác.
    ///
    /// Alias được viết ra đầy đủ ở mỗi chỗ dùng, nên một file kiểu "billion
    /// laughs" phình ra ở đây. Có trần số node viết ra; quá thì phần còn lại
    /// thành `null`.
    pub fn to_json(&self) -> String {
        const MAX_WRITTEN: usize = 1_000_000;
        fn write(y: &Yaml, i: usize, depth: usize, budget: &mut usize, out: &mut String) {
            if depth > MAX_DEPTH || *budget == 0 {
                out.push_str("null");
                return;
            }
            *budget -= 1;
            match &y.nodes[i].value {
                Value::Null => out.push_str("null"),
                Value::Str(s) => out.push_str(&crate::json::string(s)),
                Value::Seq(items) => {
                    out.push('[');
                    for (n, &item) in items.iter().enumerate() {
                        if n > 0 {
                            out.push(',');
                        }
                        write(y, item, depth + 1, budget, out);
                    }
                    out.push(']');
                }
                Value::Map(entries) => {
                    out.push('{');
                    for (n, e) in entries.iter().enumerate() {
                        if n > 0 {
                            out.push(',');
                        }
                        out.push_str(&crate::json::string(&e.key));
                        out.push(':');
                        write(y, e.value, depth + 1, budget, out);
                    }
                    out.push('}');
                }
            }
        }
        let mut out = String::from("[");
        let mut budget = MAX_WRITTEN;
        for (n, &doc) in self.docs.iter().enumerate() {
            if n > 0 {
                out.push(',');
            }
            write(self, doc, 0, &mut budget, &mut out);
        }
        out.push(']');
        out
    }
}

impl<'a> Node<'a> {
    fn raw(self) -> &'a Raw {
        &self.yaml.nodes[self.index]
    }

    /// Dòng nơi node bắt đầu, đếm từ 1.
    pub fn line(self) -> usize {
        self.raw().line
    }

    /// Chuỗi, như đã viết (số và bool cũng là chuỗi ở đây).
    pub fn as_str(self) -> Option<&'a str> {
        match &self.raw().value {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Vô hướng viết trần, không nháy: có thể là bool hay số.
    pub fn is_plain(self) -> bool {
        self.yaml.plain.get(self.index).copied().unwrap_or(false)
    }

    pub fn is_null(self) -> bool {
        matches!(self.raw().value, Value::Null)
    }

    pub fn is_map(self) -> bool {
        matches!(self.raw().value, Value::Map(_))
    }

    pub fn is_seq(self) -> bool {
        matches!(self.raw().value, Value::Seq(_))
    }

    /// Giá trị của một khoá, đã tính khoá gộp `<<`.
    pub fn get(self, key: &str) -> Option<Node<'a>> {
        match &self.raw().value {
            Value::Map(entries) => entries.iter().find(|e| e.key == key).map(|e| Node {
                yaml: self.yaml,
                index: e.value,
            }),
            _ => None,
        }
    }

    /// Mục của mapping: khoá, dòng của khoá, giá trị. Rỗng nếu không phải mapping.
    pub fn entries(self) -> impl Iterator<Item = (&'a str, usize, Node<'a>)> {
        let yaml = self.yaml;
        let entries: &'a [Entry] = match &self.raw().value {
            Value::Map(e) => e,
            _ => &[],
        };
        entries.iter().map(move |e| {
            (
                e.key.as_str(),
                e.line,
                Node {
                    yaml,
                    index: e.value,
                },
            )
        })
    }

    /// Phần tử của sequence. Rỗng nếu không phải sequence.
    pub fn items(self) -> impl Iterator<Item = Node<'a>> {
        let yaml = self.yaml;
        let items: &'a [usize] = match &self.raw().value {
            Value::Seq(i) => i,
            _ => &[],
        };
        items.iter().map(move |&index| Node { yaml, index })
    }
}

/// Đọc cả file. Lỗi thì trả dòng đầu tiên không đọc được.
pub fn parse(text: &str) -> Result<Yaml, Error> {
    let text = crate::strip_bom(text);
    let mut p = Parser::new(text);
    p.documents()?;
    Ok(Yaml {
        nodes: p.nodes,
        plain: p.plains,
        docs: p.docs,
    })
}

struct Line {
    start: usize,
    end: usize,
}

struct Parser<'a> {
    text: &'a str,
    lines: Vec<Line>,
    /// Dòng đang đọc, và cột (byte) bắt đầu phần chưa đọc của dòng đó.
    i: usize,
    col: usize,
    nodes: Vec<Raw>,
    plains: Vec<bool>,
    docs: Vec<usize>,
    anchors: HashMap<String, usize>,
    merged: usize,
    depth: usize,
}

fn err<T>(line: usize, msg: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        line,
        msg: msg.into(),
    })
}

/// Dòng bắt đầu bằng `---` hay `...` ở cột 0, theo sau là khoảng trắng hay hết dòng.
fn is_marker(text: &str) -> bool {
    (text.starts_with("---") || text.starts_with("..."))
        && text[3..]
            .chars()
            .next()
            .is_none_or(|c| c == ' ' || c == '\t')
}

fn is_seq_item(text: &str) -> bool {
    text == "-" || text.starts_with("- ") || text.starts_with("-\t")
}

/// Vị trí dấu `:` kết thúc khoá nếu `text` mở đầu một cặp `khoá: giá trị`.
fn key_end(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let colon_ok = |at: usize| {
        bytes.get(at) == Some(&b':') && bytes.get(at + 1).is_none_or(|b| *b == b' ' || *b == b'\t')
    };
    match bytes.first()? {
        q @ (b'"' | b'\'') => {
            let mut j = 1;
            while j < bytes.len() {
                match bytes[j] {
                    b'\\' if *q == b'"' => j += 2,
                    b'\'' if *q == b'\'' && bytes.get(j + 1) == Some(&b'\'') => j += 2,
                    b if b == *q => {
                        let mut k = j + 1;
                        while bytes.get(k).is_some_and(|b| *b == b' ' || *b == b'\t') {
                            k += 1;
                        }
                        return colon_ok(k).then_some(k);
                    }
                    _ => j += 1,
                }
            }
            None
        }
        b'[' | b'{' | b']' | b'}' | b',' | b'#' | b'&' | b'*' | b'!' | b'|' | b'>' | b'%'
        | b'@' | b'`' | b'?' => None,
        b'-' if bytes.len() == 1 || matches!(bytes[1], b' ' | b'\t') => None,
        _ => {
            for j in 0..bytes.len() {
                if bytes[j] == b'#' && j > 0 && matches!(bytes[j - 1], b' ' | b'\t') {
                    return None;
                }
                if colon_ok(j) {
                    return Some(j);
                }
            }
            None
        }
    }
}

/// Chuỗi thường trên một dòng: tới chú thích ` #` hay hết dòng, bỏ khoảng
/// trắng cuối. Phần tử thứ hai: dòng có chú thích.
fn plain_line(text: &str) -> (&str, bool) {
    let bytes = text.as_bytes();
    for j in 1..bytes.len() {
        if bytes[j] == b'#' && matches!(bytes[j - 1], b' ' | b'\t') {
            return (text[..j].trim_end(), true);
        }
    }
    (text.trim_end(), false)
}

fn is_null_plain(s: &str) -> bool {
    matches!(s, "" | "~" | "null" | "Null" | "NULL")
}

/// Ký tự kết thúc chuỗi thường trong flow.
fn flow_stop(c: char) -> bool {
    matches!(c, ',' | '[' | ']' | '{' | '}')
}

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for (idx, b) in text.bytes().enumerate() {
            if b == b'\n' {
                let end = if idx > start && text.as_bytes()[idx - 1] == b'\r' {
                    idx - 1
                } else {
                    idx
                };
                lines.push(Line { start, end });
                start = idx + 1;
            }
        }
        if start < text.len() {
            lines.push(Line {
                start,
                end: text.len(),
            });
        }
        Parser {
            text,
            lines,
            i: 0,
            col: 0,
            nodes: Vec::new(),
            plains: Vec::new(),
            docs: Vec::new(),
            anchors: HashMap::new(),
            merged: 0,
            depth: 0,
        }
    }

    fn line_text(&self, i: usize) -> &'a str {
        let l = &self.lines[i];
        &self.text[l.start..l.end]
    }

    fn line_no(&self) -> usize {
        self.i.min(self.lines.len().saturating_sub(1)) + 1
    }

    /// Phần chưa đọc của dòng hiện tại.
    fn rest(&self) -> &'a str {
        if self.i >= self.lines.len() {
            return "";
        }
        let t = self.line_text(self.i);
        &t[self.col.min(t.len())..]
    }

    fn next_line(&mut self) {
        self.i += 1;
        self.col = 0;
    }

    fn skip_spaces(&mut self) {
        let rest = self.rest();
        self.col += rest.len() - rest.trim_start_matches([' ', '\t']).len();
    }

    /// Phần còn lại của dòng phải rỗng hay là chú thích; rồi sang dòng sau.
    fn end_of_line(&mut self) -> Result<(), Error> {
        self.skip_spaces();
        let rest = self.rest();
        if !rest.is_empty() && !rest.starts_with('#') {
            return err(
                self.line_no(),
                format!("thừa nội dung sau giá trị, ở cột {}", self.col + 1),
            );
        }
        self.next_line();
        Ok(())
    }

    /// Dòng có nội dung tiếp theo, tính từ dòng hiện tại (từ đầu dòng), và cột
    /// của nội dung đó. Không dịch vị trí đọc.
    fn peek(&self) -> Result<Option<(usize, usize)>, Error> {
        let mut i = self.i;
        while i < self.lines.len() {
            let t = self.line_text(i);
            let indent = t.len() - t.trim_start_matches(' ').len();
            let after = &t[indent..];
            let content = after.trim_start_matches([' ', '\t']);
            if content.is_empty() || content.starts_with('#') {
                i += 1;
                continue;
            }
            if after.starts_with('\t') {
                return err(i + 1, "tab trong phần thụt lề — YAML chỉ nhận dấu cách");
            }
            return Ok(Some((i, indent)));
        }
        Ok(None)
    }

    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return err(self.line_no(), format!("lồng sâu quá {MAX_DEPTH} cấp"));
        }
        Ok(())
    }

    fn push(&mut self, line: usize, value: Value) -> usize {
        self.nodes.push(Raw { line, value });
        self.plains.push(false);
        self.nodes.len() - 1
    }

    fn push_plain(&mut self, line: usize, value: Value) -> usize {
        self.nodes.push(Raw { line, value });
        self.plains.push(true);
        self.nodes.len() - 1
    }

    fn documents(&mut self) -> Result<(), Error> {
        loop {
            let Some((i, indent)) = self.peek()? else {
                return Ok(());
            };
            self.i = i;
            self.col = 0;
            let t = self.line_text(i);
            if indent == 0 && t.starts_with('%') {
                self.next_line();
                continue;
            }
            if indent == 0 && t.starts_with("...") && is_marker(t) {
                self.next_line();
                continue;
            }
            let root = if indent == 0 && t.starts_with("---") && is_marker(t) {
                self.col = 3;
                self.value(-1, None)?
            } else {
                self.block(-1, None)?
            };
            self.docs.push(root);
            // Sau một tài liệu chỉ có thể là dấu `---`, `...` hay hết file.
            if let Some((i, indent)) = self.peek()? {
                let t = self.line_text(i);
                if !(indent == 0 && is_marker(t)) {
                    return err(i + 1, "nội dung không thuộc cấu trúc nào");
                }
            }
        }
    }

    /// Node bắt đầu ở các dòng sau, thụt sâu hơn `parent`. `compact` cho phép
    /// sequence nằm đúng cột đó: giá trị của một khoá mapping viết kiểu
    /// `khoá:` rồi `- a` cùng cột.
    fn block(&mut self, parent: isize, compact: Option<usize>) -> Result<usize, Error> {
        let line = self.line_no();
        let Some((i, indent)) = self.peek()? else {
            return Ok(self.push(line, Value::Null));
        };
        let t = self.line_text(i);
        if indent == 0 && is_marker(t) {
            return Ok(self.push(line, Value::Null));
        }
        let deeper = indent as isize > parent;
        let compact_seq = compact == Some(indent) && is_seq_item(&t[indent..]);
        if !deeper && !compact_seq {
            return Ok(self.push(line, Value::Null));
        }
        self.i = i;
        self.col = indent;
        self.inline(parent)
    }

    /// Giá trị bắt đầu ở vị trí đọc hiện tại: phần còn lại của dòng (sau
    /// `khoá:` hay `- `), hoặc các dòng sau nếu dòng này đã hết.
    ///
    /// Sau `khoá:` thì cùng dòng chỉ có thể là một giá trị đơn: `a: - x` hay
    /// `a: b: c` là lỗi, như PyYAML. Sau `- ` thì được (`- - a`, `- k: v`).
    fn value(&mut self, parent: isize, compact: Option<usize>) -> Result<usize, Error> {
        self.enter()?;
        let (anchor, tagged) = self.props()?;
        self.skip_spaces();
        let rest = self.rest();
        let node = if rest.is_empty() || rest.starts_with('#') {
            self.next_line();
            self.block(parent, compact)?
        } else {
            if compact.is_some() && (is_seq_item(rest) || key_end(rest).is_some()) {
                return err(
                    self.line_no(),
                    "sau `khoá:` không mở được sequence hay mapping trên cùng dòng",
                );
            }
            self.inline(parent)?
        };
        self.untag_null(node, tagged);
        if let Some(name) = anchor {
            self.anchors.insert(name, node);
        }
        self.depth -= 1;
        Ok(node)
    }

    /// Giá trị rỗng có tag (`!!python/name:x` không có gì sau) là chuỗi rỗng,
    /// không phải null: tag đã nói nó là gì.
    fn untag_null(&mut self, node: usize, tagged: bool) {
        // Có tag (`!!str false`) thì không còn là vô hướng trần: không đọc
        // thành bool hay số.
        if tagged {
            self.plains[node] = false;
        }
        if tagged && matches!(self.nodes[node].value, Value::Null) {
            self.nodes[node].value = Value::Str(String::new());
        }
    }

    /// `&anchor` và `!tag` trước một giá trị. Tag chỉ được ghi nhận là có,
    /// trừ `!!null`.
    fn props(&mut self) -> Result<(Option<String>, bool), Error> {
        let mut anchor = None;
        let mut tagged = false;
        loop {
            self.skip_spaces();
            let rest = self.rest();
            let Some(first) = rest.chars().next() else {
                break;
            };
            if first != '&' && first != '!' {
                break;
            }
            let len = rest.find([' ', '\t']).unwrap_or(rest.len());
            if first == '&' {
                let name = &rest[1..len];
                if name.is_empty() {
                    return err(self.line_no(), "anchor thiếu tên");
                }
                anchor = Some(name.to_string());
            } else {
                tagged = &rest[..len] != "!!null";
            }
            self.col += len;
        }
        Ok((anchor, tagged))
    }

    /// Node bắt đầu ngay tại vị trí đọc, trên dòng hiện tại.
    fn inline(&mut self, parent: isize) -> Result<usize, Error> {
        self.skip_spaces();
        let rest = self.rest();
        let line = self.line_no();
        let column = self.col;
        let Some(first) = rest.chars().next() else {
            return Ok(self.push(line, Value::Null));
        };
        match first {
            '*' => {
                let len = rest.find([' ', '\t']).unwrap_or(rest.len());
                let name = &rest[1..len];
                let Some(&node) = self.anchors.get(name) else {
                    return err(line, "alias trỏ tới anchor chưa có".to_string());
                };
                self.col += len;
                self.end_of_line()?;
                Ok(node)
            }
            '|' | '>' => self.block_scalar(parent),
            '[' | '{' => {
                let at = self.lines[self.i].start + self.col;
                let (node, end) = self.flow(at)?;
                self.seek(end);
                self.end_of_line()?;
                Ok(node)
            }
            '?' if rest.len() == 1 || rest[1..].starts_with([' ', '\t']) => {
                err(line, "khoá phức `? ` không được hỗ trợ")
            }
            _ if is_seq_item(rest) => self.sequence(column),
            _ if key_end(rest).is_some() => self.mapping(column),
            '"' | '\'' => {
                let at = self.lines[self.i].start + self.col;
                let (text, end) = self.quoted(at)?;
                self.seek(end);
                self.end_of_line()?;
                Ok(self.push(line, Value::Str(text)))
            }
            _ => self.plain(parent),
        }
    }

    /// Đặt vị trí đọc vào byte `at` của cả văn bản.
    fn seek(&mut self, at: usize) {
        let i = self
            .lines
            .partition_point(|l| l.start <= at)
            .saturating_sub(1);
        self.i = i;
        self.col = at.saturating_sub(self.lines.get(i).map_or(0, |l| l.start));
    }

    fn sequence(&mut self, column: usize) -> Result<usize, Error> {
        self.enter()?;
        let line = self.line_no();
        let mut items = Vec::new();
        loop {
            self.col = column + 1; // sau dấu `-`
            items.push(self.value(column as isize, None)?);
            let Some((i, indent)) = self.peek()? else {
                break;
            };
            let t = self.line_text(i);
            if indent == 0 && is_marker(t) {
                break;
            }
            if indent != column || !is_seq_item(&t[indent..]) {
                if indent > column {
                    return err(i + 1, "thụt lề sai trong sequence");
                }
                break;
            }
            self.i = i;
            self.col = column;
        }
        self.depth -= 1;
        Ok(self.push(line, Value::Seq(items)))
    }

    fn mapping(&mut self, column: usize) -> Result<usize, Error> {
        self.enter()?;
        let line = self.line_no();
        let mut entries: Vec<Entry> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut merges: Vec<(usize, usize)> = Vec::new();
        loop {
            let key_line = self.line_no();
            let (key, plain) = self.key()?;
            let value = self.value(column as isize, Some(column))?;
            if plain && key == "<<" {
                merges.push((value, key_line));
            } else if let Some(&at) = index.get(&key) {
                // Khoá trùng: khoá sau thắng, như PyYAML.
                entries[at] = Entry {
                    key,
                    line: key_line,
                    value,
                };
            } else {
                index.insert(key.clone(), entries.len());
                entries.push(Entry {
                    key,
                    line: key_line,
                    value,
                });
            }
            let Some((i, indent)) = self.peek()? else {
                break;
            };
            let t = self.line_text(i);
            if indent == 0 && is_marker(t) {
                break;
            }
            if indent < column {
                break;
            }
            if indent > column {
                return err(i + 1, "thụt lề sai trong mapping");
            }
            let content = &t[indent..];
            if is_seq_item(content) {
                break; // sequence cùng cột thuộc về khoá của mapping cha
            }
            if content == "?" || content.starts_with("? ") {
                return err(i + 1, "khoá phức `? ` không được hỗ trợ");
            }
            if key_end(content).is_none() {
                return err(i + 1, format!("cần một khoá `tên:` ở cột {}", indent + 1));
            }
            self.i = i;
            self.col = column;
        }
        self.merge(&mut entries, merges)?;
        self.depth -= 1;
        Ok(self.push(line, Value::Map(entries)))
    }

    /// Gộp mục của các mapping mà `<<` trỏ tới. Mục viết thẳng thắng mục gộp,
    /// mapping đứng trước trong danh sách `<<` thắng mapping đứng sau.
    fn merge(
        &mut self,
        entries: &mut Vec<Entry>,
        merges: Vec<(usize, usize)>,
    ) -> Result<(), Error> {
        if merges.is_empty() {
            return Ok(());
        }
        let mut seen: HashSet<String> = entries.iter().map(|e| e.key.clone()).collect();
        for (node, line) in merges {
            let sources: Vec<usize> = match &self.nodes[node].value {
                Value::Map(_) => vec![node],
                Value::Seq(items) => items.clone(),
                _ => return err(line, "`<<` phải trỏ tới mapping hay danh sách mapping"),
            };
            for source in sources {
                let Value::Map(from) = &self.nodes[source].value else {
                    return err(line, "`<<` phải trỏ tới mapping hay danh sách mapping");
                };
                self.merged += from.iter().map(|e| 1 + e.key.len()).sum::<usize>();
                if self.merged > MAX_MERGED {
                    return err(line, "gộp `<<` vượt ngân sách của một file");
                }
                for e in from {
                    if seen.insert(e.key.clone()) {
                        entries.push(e.clone());
                    }
                }
            }
        }
        Ok(())
    }

    /// Đọc khoá ở vị trí hiện tại và dấu `:` sau nó. Phần tử thứ hai: khoá
    /// viết không nháy.
    fn key(&mut self) -> Result<(String, bool), Error> {
        let rest = self.rest();
        let Some(end) = key_end(rest) else {
            return err(
                self.line_no(),
                format!("cần một khoá `tên:` ở cột {}", self.col + 1),
            );
        };
        let raw = rest[..end].trim_end();
        let (key, plain) = if raw.starts_with(['"', '\'']) {
            let at = self.lines[self.i].start + self.col;
            (self.quoted(at)?.0, false)
        } else {
            (raw.to_string(), true)
        };
        self.col += end + 1;
        Ok((key, plain))
    }

    /// Chuỗi thường, có thể kéo sang các dòng sau thụt sâu hơn `parent`.
    fn plain(&mut self, parent: isize) -> Result<usize, Error> {
        let line = self.line_no();
        let (first, commented) = plain_line(self.rest());
        let mut out = first.to_string();
        self.next_line();
        // Chú thích kết thúc chuỗi thường: dòng sau không nối vào nữa.
        if !commented {
            let mut breaks = 0usize;
            while self.i < self.lines.len() {
                let t = self.line_text(self.i);
                let content = t.trim_start_matches([' ', '\t']);
                if content.is_empty() {
                    breaks += 1;
                    self.i += 1;
                    continue;
                }
                let indent = t.len() - t.trim_start_matches(' ').len();
                if indent as isize <= parent
                    || content.starts_with('#')
                    || (indent == 0 && is_marker(t))
                {
                    break;
                }
                if key_end(content).is_some() {
                    return err(self.i + 1, "khoá `tên:` nằm giữa một chuỗi nhiều dòng");
                }
                let (piece, commented) = plain_line(content);
                if breaks == 0 {
                    out.push(' ');
                } else {
                    out.extend(std::iter::repeat_n('\n', breaks));
                }
                out.push_str(piece);
                breaks = 0;
                self.i += 1;
                if commented {
                    break;
                }
            }
            self.col = 0;
        }
        let value = if is_null_plain(&out) {
            Value::Null
        } else {
            Value::Str(out)
        };
        Ok(self.push_plain(line, value))
    }

    /// Chuỗi trong nháy đơn hay nháy kép bắt đầu ở byte `at`, có thể nhiều
    /// dòng. Trả chuỗi và vị trí ngay sau dấu nháy đóng. Gập dòng như PyYAML.
    fn quoted(&self, at: usize) -> Result<(String, usize), Error> {
        let text = self.text;
        let bytes = text.as_bytes();
        let double = bytes[at] == b'"';
        let quote = bytes[at] as char;
        let line_of = |pos: usize| self.lines.partition_point(|l| l.start <= pos).max(1);
        let mut out = String::new();
        let mut pos = at + 1;
        loop {
            // Phần không phải khoảng trắng.
            loop {
                let Some(c) = text[pos..].chars().next() else {
                    return err(line_of(at), "chuỗi trong nháy chưa đóng");
                };
                match c {
                    '\'' if !double && text[pos + 1..].starts_with('\'') => {
                        out.push('\'');
                        pos += 2;
                    }
                    c if c == quote => return Ok((out, pos + 1)),
                    '\\' if double => {
                        let Some(next) = text[pos + 1..].chars().next() else {
                            return err(line_of(pos), "chuỗi trong nháy chưa đóng");
                        };
                        if next == '\n' || next == '\r' {
                            pos = skip_break(text, pos + 1);
                            let (breaks, after) = flow_breaks(text, pos);
                            out.extend(std::iter::repeat_n('\n', breaks));
                            pos = after;
                            continue;
                        }
                        let simple = match next {
                            '0' => Some('\0'),
                            'a' => Some('\u{07}'),
                            'b' => Some('\u{08}'),
                            't' | '\t' => Some('\t'),
                            'n' => Some('\n'),
                            'v' => Some('\u{0B}'),
                            'f' => Some('\u{0C}'),
                            'r' => Some('\r'),
                            'e' => Some('\u{1B}'),
                            ' ' => Some(' '),
                            '"' => Some('"'),
                            '/' => Some('/'),
                            '\\' => Some('\\'),
                            'N' => Some('\u{85}'),
                            '_' => Some('\u{A0}'),
                            'L' => Some('\u{2028}'),
                            'P' => Some('\u{2029}'),
                            _ => None,
                        };
                        if let Some(c) = simple {
                            out.push(c);
                            pos += 1 + next.len_utf8();
                            continue;
                        }
                        let width = match next {
                            'x' => 2,
                            'u' => 4,
                            'U' => 8,
                            _ => return err(line_of(pos), "ký tự thoát không hợp lệ".to_string()),
                        };
                        let hex = text.get(pos + 2..pos + 2 + width).unwrap_or("");
                        let code = (hex.len() == width
                            && hex.bytes().all(|b| b.is_ascii_hexdigit()))
                        .then(|| u32::from_str_radix(hex, 16).ok())
                        .flatten()
                        .and_then(char::from_u32);
                        let Some(c) = code else {
                            return err(line_of(pos), "mã thoát không hợp lệ".to_string());
                        };
                        out.push(c);
                        pos += 2 + width;
                    }
                    ' ' | '\t' | '\n' | '\r' => break,
                    c => {
                        out.push(c);
                        pos += c.len_utf8();
                    }
                }
            }
            // Khoảng trắng: giữ nguyên trong dòng; ngắt dòng thì gập.
            let start = pos;
            while matches!(bytes.get(pos), Some(b' ' | b'\t')) {
                pos += 1;
            }
            match bytes.get(pos) {
                None => return err(line_of(at), "chuỗi trong nháy chưa đóng"),
                Some(b'\n' | b'\r') => {
                    pos = skip_break(text, pos);
                    let (breaks, after) = flow_breaks(text, pos);
                    if breaks == 0 {
                        out.push(' ');
                    } else {
                        out.extend(std::iter::repeat_n('\n', breaks));
                    }
                    pos = after;
                }
                Some(_) => out.push_str(&text[start..pos]),
            }
        }
    }

    /// Block scalar `|` hay `>` ở vị trí hiện tại. Chép thuật toán
    /// `scan_block_scalar` của PyYAML, kể cả cách nó tìm độ thụt lề.
    fn block_scalar(&mut self, parent: isize) -> Result<usize, Error> {
        let line = self.line_no();
        let header = self.rest();
        let folded = header.starts_with('>');
        let mut chomp: Option<bool> = None; // None: giữ một ngắt dòng; false: bỏ hết; true: giữ hết
        let mut increment: Option<usize> = None;
        let mut used = 1;
        for c in header[1..].chars() {
            match c {
                '+' | '-' if chomp.is_none() => chomp = Some(c == '+'),
                '1'..='9' if increment.is_none() => increment = Some(c as usize - '0' as usize),
                _ => break,
            }
            used += 1;
        }
        let tail = header[used..].trim_start_matches([' ', '\t']);
        if !tail.is_empty() && !tail.starts_with('#') {
            return err(line, format!("thừa nội dung sau `{}`", &header[..used]));
        }
        let text = self.text;
        let bytes = text.as_bytes();
        let min_indent = (parent + 1).max(1) as usize;
        // Bắt đầu ở đầu dòng sau.
        let mut pos = self.lines.get(self.i + 1).map_or(text.len(), |l| l.start);
        let column = |pos: usize| {
            pos - self.lines[self
                .lines
                .partition_point(|l| l.start <= pos)
                .saturating_sub(1)]
            .start
        };
        let mut breaks: Vec<&str> = Vec::new();
        let indent = match increment {
            None => {
                // Bỏ các dòng trống ở đầu, ghi lại cột sâu nhất của dấu cách.
                let mut max_indent = 0;
                loop {
                    match bytes.get(pos) {
                        Some(b' ') => {
                            pos += 1;
                            max_indent = max_indent.max(column(pos));
                        }
                        Some(b'\n' | b'\r') => {
                            pos = skip_break(text, pos);
                            breaks.push("\n");
                        }
                        _ => break,
                    }
                }
                max_indent.max(min_indent)
            }
            Some(n) => {
                let indent = min_indent + n - 1;
                pos = block_breaks(text, pos, indent, &column, &mut breaks);
                indent
            }
        };
        let mut chunks = String::new();
        let mut line_break = "";
        while pos < text.len() && column(pos) == indent {
            for b in breaks.drain(..) {
                chunks.push_str(b);
            }
            let leading_non_space = !matches!(bytes[pos], b' ' | b'\t');
            let end = text[pos..]
                .find(['\n', '\r'])
                .map_or(text.len(), |n| pos + n);
            chunks.push_str(&text[pos..end]);
            pos = end;
            line_break = if pos < text.len() {
                pos = skip_break(text, pos);
                "\n"
            } else {
                ""
            };
            pos = block_breaks(text, pos, indent, &column, &mut breaks);
            if pos < text.len() && column(pos) == indent {
                if folded
                    && line_break == "\n"
                    && leading_non_space
                    && !matches!(bytes[pos], b' ' | b'\t')
                {
                    if breaks.is_empty() {
                        chunks.push(' ');
                    }
                } else {
                    chunks.push_str(line_break);
                }
            } else {
                break;
            }
        }
        if chomp != Some(false) {
            chunks.push_str(line_break);
        }
        if chomp == Some(true) {
            for b in &breaks {
                chunks.push_str(b);
            }
        }
        // Dòng chưa đọc tiếp theo là dòng chứa `pos`, đọc lại từ đầu.
        self.i = if pos >= text.len() {
            self.lines.len()
        } else {
            self.lines
                .partition_point(|l| l.start <= pos)
                .saturating_sub(1)
                .max(self.i + 1)
        };
        self.col = 0;
        Ok(self.push(line, Value::Str(chunks)))
    }

    /// Collection flow `[...]` hay `{...}` bắt đầu ở byte `at`. Trả node và vị
    /// trí ngay sau dấu đóng.
    fn flow(&mut self, at: usize) -> Result<(usize, usize), Error> {
        self.enter()?;
        let text = self.text;
        let line = self.lines.partition_point(|l| l.start <= at).max(1);
        let seq = text.as_bytes()[at] == b'[';
        let close = if seq { ']' } else { '}' };
        let mut pos = at + 1;
        let mut items: Vec<usize> = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut merges: Vec<(usize, usize)> = Vec::new();
        loop {
            pos = flow_space(text, pos);
            match text[pos..].chars().next() {
                None => return err(line, format!("thiếu `{close}`")),
                Some(c) if c == close => {
                    pos += 1;
                    break;
                }
                _ => {}
            }
            let item_line = self.lines.partition_point(|l| l.start <= pos).max(1);
            let (first, after) = self.flow_node(pos)?;
            pos = flow_space(text, after);
            let paired = text[pos..].starts_with(':')
                && text[pos + 1..]
                    .chars()
                    .next()
                    .is_none_or(|c| c.is_whitespace() || flow_stop(c));
            if paired || !seq {
                // Cặp `khoá: giá trị`. Trong `[...]` nó thành mapping một mục.
                let key = match &self.nodes[first].value {
                    Value::Str(s) => s.clone(),
                    Value::Null => String::new(),
                    _ => return err(item_line, "khoá trong flow phải là chuỗi"),
                };
                let value = if paired {
                    pos = flow_space(text, pos + 1);
                    match text[pos..].chars().next() {
                        Some(c) if c == ',' || c == close => self.push(item_line, Value::Null),
                        _ => {
                            let (v, after) = self.flow_node(pos)?;
                            pos = flow_space(text, after);
                            v
                        }
                    }
                } else {
                    self.push(item_line, Value::Null)
                };
                if seq {
                    let entry = Entry {
                        key,
                        line: item_line,
                        value,
                    };
                    items.push(self.push(item_line, Value::Map(vec![entry])));
                } else if key == "<<" {
                    merges.push((value, item_line));
                } else if let Some(&i) = index.get(&key) {
                    entries[i] = Entry {
                        key,
                        line: item_line,
                        value,
                    };
                } else {
                    index.insert(key.clone(), entries.len());
                    entries.push(Entry {
                        key,
                        line: item_line,
                        value,
                    });
                }
            } else {
                items.push(first);
            }
            match text[pos..].chars().next() {
                Some(',') => pos += 1,
                Some(c) if c == close => {
                    pos += 1;
                    break;
                }
                _ => return err(line, format!("cần `,` hay `{close}`")),
            }
        }
        self.depth -= 1;
        let value = if seq {
            Value::Seq(items)
        } else {
            self.merge(&mut entries, merges)?;
            Value::Map(entries)
        };
        Ok((self.push(line, value), pos))
    }

    /// Một node trong flow, bắt đầu ở byte `pos`.
    fn flow_node(&mut self, mut pos: usize) -> Result<(usize, usize), Error> {
        let text = self.text;
        let line = self.lines.partition_point(|l| l.start <= pos).max(1);
        let mut anchor = None;
        let mut tagged = false;
        while let Some(c @ ('&' | '!')) = text[pos..].chars().next() {
            let len = text[pos..]
                .find(|c: char| c.is_whitespace() || flow_stop(c))
                .unwrap_or(text.len() - pos);
            if c == '&' {
                anchor = Some(text[pos + 1..pos + len].to_string());
            } else {
                tagged = &text[pos..pos + len] != "!!null";
            }
            pos = flow_space(text, pos + len);
        }
        let (node, end) = match text[pos..].chars().next() {
            Some('[' | '{') => self.flow(pos)?,
            Some('"' | '\'') => {
                let (s, end) = self.quoted(pos)?;
                (self.push(line, Value::Str(s)), end)
            }
            Some('*') => {
                let len = text[pos..]
                    .find(|c: char| c.is_whitespace() || flow_stop(c))
                    .unwrap_or(text.len() - pos);
                let name = &text[pos + 1..pos + len];
                let Some(&node) = self.anchors.get(name) else {
                    return err(line, "alias trỏ tới anchor chưa có".to_string());
                };
                (node, pos + len)
            }
            _ => {
                let (s, end) = flow_plain(text, pos);
                let value = if is_null_plain(&s) {
                    Value::Null
                } else {
                    Value::Str(s)
                };
                (self.push_plain(line, value), end)
            }
        };
        self.untag_null(node, tagged);
        if let Some(name) = anchor {
            self.anchors.insert(name, node);
        }
        Ok((node, end))
    }
}

/// Bỏ một ngắt dòng (`\n` hay `\r\n`) ở `pos`.
fn skip_break(text: &str, pos: usize) -> usize {
    match text.as_bytes().get(pos) {
        Some(b'\r') if text.as_bytes().get(pos + 1) == Some(&b'\n') => pos + 2,
        Some(b'\r' | b'\n') => pos + 1,
        _ => pos,
    }
}

/// Sau một ngắt dòng trong chuỗi có nháy: đếm dòng trống tiếp theo, bỏ khoảng
/// trắng đầu dòng có nội dung.
fn flow_breaks(text: &str, mut pos: usize) -> (usize, usize) {
    let bytes = text.as_bytes();
    let mut breaks = 0;
    loop {
        while matches!(bytes.get(pos), Some(b' ' | b'\t')) {
            pos += 1;
        }
        if matches!(bytes.get(pos), Some(b'\n' | b'\r')) {
            pos = skip_break(text, pos);
            breaks += 1;
        } else {
            return (breaks, pos);
        }
    }
}

/// `scan_block_scalar_breaks` của PyYAML: dòng trống sau nội dung, mỗi dòng
/// đã bỏ dấu cách tới cột `indent`.
fn block_breaks<'t>(
    text: &'t str,
    mut pos: usize,
    indent: usize,
    column: &dyn Fn(usize) -> usize,
    breaks: &mut Vec<&'t str>,
) -> usize {
    let bytes = text.as_bytes();
    breaks.clear();
    while pos < text.len() && column(pos) < indent && bytes[pos] == b' ' {
        pos += 1;
    }
    while matches!(bytes.get(pos), Some(b'\n' | b'\r')) {
        pos = skip_break(text, pos);
        breaks.push("\n");
        while pos < text.len() && column(pos) < indent && bytes[pos] == b' ' {
            pos += 1;
        }
    }
    pos
}

/// Khoảng trắng, ngắt dòng và chú thích giữa các phần tử của flow.
fn flow_space(text: &str, mut pos: usize) -> usize {
    let bytes = text.as_bytes();
    loop {
        match bytes.get(pos) {
            Some(b' ' | b'\t' | b'\n' | b'\r') => pos += 1,
            Some(b'#') if pos == 0 || matches!(bytes[pos - 1], b' ' | b'\t' | b'\n' | b'\r') => {
                while !matches!(bytes.get(pos), None | Some(b'\n')) {
                    pos += 1;
                }
            }
            _ => return pos,
        }
    }
}

/// Chuỗi thường trong flow: tới `, [ ] { }`, tới `:` theo sau là khoảng
/// trắng hay dấu flow, hay tới chú thích. Ngắt dòng gập thành dấu cách.
fn flow_plain(text: &str, start: usize) -> (String, usize) {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut pos = start;
    loop {
        let seg_start = pos;
        while let Some(c) = text[pos..].chars().next() {
            if flow_stop(c) || c == '\n' || c == '\r' {
                break;
            }
            if c == ':'
                && text[pos + 1..]
                    .chars()
                    .next()
                    .is_none_or(|n| n.is_whitespace() || flow_stop(n))
            {
                break;
            }
            if c == '#' && pos > seg_start && matches!(bytes[pos - 1], b' ' | b'\t') {
                break;
            }
            pos += c.len_utf8();
        }
        out.push_str(text[seg_start..pos].trim_end_matches([' ', '\t']));
        // Gập sang dòng sau nếu chuỗi chưa kết thúc.
        if !matches!(bytes.get(pos), Some(b'\n' | b'\r')) {
            return (out, pos);
        }
        let (breaks, after) = flow_breaks(text, skip_break(text, pos));
        match text[after..].chars().next() {
            None => return (out, pos),
            Some(c) if flow_stop(c) || c == '#' || c == ':' => return (out, pos),
            _ => {
                if breaks == 0 {
                    out.push(' ');
                } else {
                    out.extend(std::iter::repeat_n('\n', breaks));
                }
                pos = after;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn doc(text: &str) -> Yaml {
        parse(text).unwrap_or_else(|e| panic!("{e}\n{text}"))
    }

    fn json(text: &str) -> String {
        doc(text).to_json()
    }

    fn fails(text: &str) -> Error {
        match parse(text) {
            Ok(y) => panic!("lẽ ra phải lỗi, đọc được: {}", y.to_json()),
            Err(e) => e,
        }
    }

    #[test]
    fn mapping_va_sequence_long_nhau() {
        let y = doc(
            "services:\n  api:\n    image: x\n    ports:\n      - \"8080:80\"\n  db:\n    image: postgres\n",
        );
        let root = y.documents().next().unwrap();
        let api = root.get("services").unwrap().get("api").unwrap();
        assert_eq!(api.get("image").unwrap().as_str(), Some("x"));
        let ports: Vec<_> = api
            .get("ports")
            .unwrap()
            .items()
            .map(|n| n.as_str().unwrap())
            .collect();
        assert_eq!(ports, ["8080:80"]);
        let services: Vec<_> = root
            .get("services")
            .unwrap()
            .entries()
            .map(|(k, line, _)| (k, line))
            .collect();
        assert_eq!(services, [("api", 2), ("db", 6)], "dòng của khoá");
    }

    #[test]
    fn sequence_cung_cot_voi_khoa() {
        assert_eq!(json("a:\n- x\n- y\nb: z\n"), r#"[{"a":["x","y"],"b":"z"}]"#);
    }

    /// Giá trị kỳ vọng lấy từ PyYAML 6.0.3 trên đúng đoạn văn bản này.
    #[test]
    fn block_scalar_gap_dong_nhu_pyyaml() {
        let text = "lit: |\n  line one\n    indented\n  line three\n\nfold: >\n  a b\n  c\n\n  d\n   more\n  e\nstrip: |-\n  x\nkeep: |+\n  y\n\n\nclip_last: |\n  z\nind: |2\n    two extra\nafter: ok\n";
        assert_eq!(
            json(text),
            r#"[{"lit":"line one\n  indented\nline three\n","fold":"a b c\nd\n more\ne\n","strip":"x","keep":"y\n\n\n","clip_last":"z\n","ind":"  two extra\n","after":"ok"}]"#
        );
    }

    #[test]
    fn chuoi_trong_nhay_gap_dong_nhu_pyyaml() {
        let text = "a: \"one\n  two\n\n  three\"\nb: 'it''s\n   folded'\nc: \"esc \\t \\x41 \\u00e9 \\\"q\\\" \\\\ end\"\nd: \"line\\\n   joined\"\ne: \"trail   \n  next\"\nf: ''\ng: \"\"\n";
        assert_eq!(
            json(text),
            r#"[{"a":"one two\nthree","b":"it's folded","c":"esc \t A é \"q\" \\ end","d":"linejoined","e":"trail next","f":"","g":""}]"#
        );
    }

    #[test]
    fn anchor_alias_va_khoa_gop() {
        let text = "x: &common\n  image: base\n  environment: &env\n    A: \"1\"\n    B: two\napi:\n  <<: *common\n  environment:\n    <<: *env\n    B: override\nworker:\n  <<: [*common, {restart: always}]\n  command: *env\n";
        let y = doc(text);
        let root = y.documents().next().unwrap();
        let api = root.get("api").unwrap();
        assert_eq!(
            api.get("image").unwrap().as_str(),
            Some("base"),
            "lấy từ `<<`"
        );
        assert_eq!(
            api.get("environment").unwrap().get("B").unwrap().as_str(),
            Some("override"),
            "viết thẳng thắng gộp"
        );
        assert_eq!(
            api.get("environment").unwrap().get("A").unwrap().as_str(),
            Some("1")
        );
        let worker = root.get("worker").unwrap();
        assert_eq!(worker.get("restart").unwrap().as_str(), Some("always"));
        assert_eq!(
            worker.get("command").unwrap().get("B").unwrap().as_str(),
            Some("two")
        );
        assert_eq!(
            api.get("image").unwrap().line(),
            2,
            "dòng của node gộp là dòng gốc của nó"
        );
    }

    #[test]
    fn flow_nhieu_dong_va_cap_trong_sequence() {
        assert_eq!(
            json(
                "b: {x: 1,\n    y: [a,\n        b], z: }\nd: [a: b, c]\ne: {url: http://x:1/y}\nh: [ multi\n     line, x ]\n"
            ),
            r#"[{"b":{"x":"1","y":["a","b"],"z":null},"d":[{"a":"b"},"c"],"e":{"url":"http://x:1/y"},"h":["multi line","x"]}]"#
        );
    }

    #[test]
    fn chu_thich_null_va_nhieu_tai_lieu() {
        assert_eq!(
            json(
                "key: # c\n  sub: 1   # t\nempty:\nn: [~, null, '', \"null\"]\nurl: http://e.com/a#frag\nhash: a #c\nnc: a#b\n"
            ),
            r#"[{"key":{"sub":"1"},"empty":null,"n":[null,null,"","null"],"url":"http://e.com/a#frag","hash":"a","nc":"a#b"}]"#
        );
        assert_eq!(
            json("%YAML 1.1\n---\na: 1\n...\n---\n- x\n--- |\n  text doc\n---\n"),
            r#"[{"a":"1"},["x"],"text doc\n",null]"#
        );
        assert_eq!(
            json("a: 1\r\nb:\r\n  - \"y\r\n    z\"\r\n"),
            r#"[{"a":"1","b":["y z"]}]"#,
            "CRLF"
        );
    }

    #[test]
    fn tag_bo_qua_nhung_gia_tri_rong_co_tag_la_chuoi_rong() {
        assert_eq!(
            json("a: !!python/name:x.y\nb: !!null\nc: !reset []\n"),
            r#"[{"a":"","b":null,"c":[]}]"#
        );
    }

    #[test]
    fn loi_co_dong_va_khong_doan() {
        assert_eq!(fails("a:\n\tb: 1\n").line, 2);
        assert_eq!(fails("a: - x\n").line, 1);
        assert_eq!(fails("a: b: c\n").line, 1);
        assert_eq!(fails("? k\n: v\n").line, 1);
        assert_eq!(fails("a: \"open\nb: c\n").line, 1);
        assert!(fails("a: *missing\n").msg.contains("anchor chưa có"));
        assert!(
            fails("a: &x [*x]\n").msg.contains("anchor chưa có"),
            "alias không trỏ ngược lên node đang dựng"
        );
    }

    /// TM-8: lồng sâu không làm tràn stack.
    #[test]
    fn long_rat_sau_khong_tran_stack() {
        let flow = format!("a: {}", "[".repeat(100_000));
        assert!(fails(&flow).msg.contains("lồng sâu"));
        let flow_map = format!("a: {}", "{b: ".repeat(100_000));
        assert!(fails(&flow_map).msg.contains("lồng sâu"));
        let block: String = (0..200)
            .map(|d| format!("{}k{d}:\n", "  ".repeat(d)))
            .collect();
        assert!(fails(&block).msg.contains("lồng sâu"));
        let seq: String = (0..200)
            .map(|d| format!("{}- \n", "  ".repeat(d)))
            .collect();
        assert!(fails(&seq).msg.contains("lồng sâu"));
    }

    /// TM-8: alias là tham chiếu, nên "billion laughs" đọc tức thì và không
    /// phình bộ nhớ; chỉ chỗ viết ra cả cây mới có trần.
    #[test]
    fn alias_long_nhau_khong_phinh() {
        let mut text = String::from("l0: &l0 [x, x, x, x, x, x, x, x, x, x]\n");
        for d in 1..12 {
            let refs = vec![format!("*l{}", d - 1); 10].join(", ");
            text.push_str(&format!("l{d}: &l{d} [{refs}]\n"));
        }
        let start = crate::testclock::Clock::start();
        let y = doc(&text);
        assert!(y.nodes.len() < 1_000, "{} node", y.nodes.len());
        let out = y.to_json();
        assert!(out.len() < 32 << 20, "{} byte", out.len());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    /// TM-8: `<<` toả nhánh dừng ở ngân sách.
    #[test]
    fn khoa_gop_toa_nhanh_dung_o_ngan_sach() {
        let keys: Vec<String> = (0..1_000).map(|k| format!("k{k}: v")).collect();
        let mut text = format!("base: &b {{{}}}\n", keys.join(", "));
        for m in 0..200 {
            text.push_str(&format!("m{m}:\n  <<: [*b, *b, *b]\n"));
        }
        let start = crate::testclock::Clock::start();
        assert!(fails(&text).msg.contains("gộp `<<`"));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    /// TM-8: khoá rất dài gộp nhiều lần cũng dừng ở ngân sách, vì ngân sách
    /// đếm cả byte.
    #[test]
    fn khoa_dai_gop_nhieu_lan_dung_o_ngan_sach() {
        let mut text = format!("x: &b {{{}: v}}\nl:\n", "k".repeat(100_000));
        for _ in 0..4000 {
            text.push_str("- {<<: *b}\n");
        }
        assert!(fails(&text).msg.contains("ngân sách"));
    }

    /// TM-10: thông báo lỗi không chép lại nội dung dòng hỏng.
    #[test]
    fn loi_khong_trich_noi_dung() {
        for text in [
            "a: 1\nOPENAI_API_KEY=sk-proj-bimat\n",
            "a: \"x\" sk-proj-bimat\n",
            "a:\n  b: 1\n c: sk-proj-bimat\n",
            // Tên alias là chữ người viết chọn, có khi chính là mật khẩu.
            "POSTGRES_PASSWORD: *Hunter2bimat\n",
            "a: [*Hunter2bimat]\n",
            "a: \"\\qbimat\"\n",
        ] {
            let e = fails(text);
            assert!(!e.msg.contains("bimat"), "{e}");
        }
    }

    /// File 1 MB bình thường đọc trong thời gian tuyến tính.
    #[test]
    fn file_lon_doc_tuyen_tinh() {
        let mut text = String::from("services:\n");
        let mut n = 0;
        while text.len() < 1 << 20 {
            text.push_str(&format!("  s{n}:\n    image: \"img:{n}\"\n    environment:\n      - A=b\n    command: [python, -m, app]\n"));
            n += 1;
        }
        let start = crate::testclock::Clock::start();
        let y = doc(&text);
        assert_eq!(
            y.documents()
                .next()
                .unwrap()
                .get("services")
                .unwrap()
                .entries()
                .count(),
            n
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }
}
