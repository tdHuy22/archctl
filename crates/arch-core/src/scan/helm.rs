//! Dựng manifest từ chart Helm, không chạy `helm`.
//!
//! Template của chart là Go template cộng thư viện hàm Sprig. archctl dựng một
//! tập con đủ cho chart thường gặp: `.Values`, `.Release`, `.Chart`,
//! `.Template`, biến, `if`/`else`/`with`/`range`, `define`/`include`/
//! `template`, và các hàm hay dùng (`toYaml`, `nindent`, `default`, `quote`,
//! `printf`…). Manifest dựng ra đi qua đúng bộ đọc Kubernetes của
//! [`super::k8s`].
//!
//! Thứ ngoài tập con (`tpl`, `lookup`, `.Files`, `.Capabilities`, hàm lạ) làm
//! cả file thành "chưa đọc được": không đoán nhánh nào, vì một nhánh đoán sai
//! là một cạnh mất hay cạnh giả (TM-37). Template là input đối kháng chạy trong
//! hook: độ sâu, số bước và số byte dựng ra đều có trần (TM-36).

use super::yaml::{self, Node};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

/// Bước dựng cho cả một chart: mỗi nút, mỗi lần lặp, mỗi lời gọi hàm là một bước.
pub const MAX_STEPS: usize = 2_000_000;
/// Byte dựng ra cho cả một chart.
pub const MAX_OUTPUT: usize = 8 << 20;
/// Độ sâu `include`/`template` lồng nhau, và độ lồng của khối điều khiển.
pub const MAX_DEPTH: usize = 64;
/// Byte giá trị trung gian do hàm tạo ra, cộng dồn cho cả cây chart.
pub const MAX_MADE: usize = 32 << 20;
/// File trong một cây chart (kể cả chart con).
pub const MAX_CHART_FILES: usize = 5000;
/// Số giá trị đọc từ các file values (alias YAML bung ra tính cả).
pub const MAX_VALUES: usize = 200_000;

/// Giá trị trong template.
#[derive(Debug, Clone, PartialEq)]
pub enum V {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    /// `Rc`: chép một giá trị (`.`, `$`, `.Values`) là O(1), không chép cả cây
    /// values ở mỗi lần truy cập (TM-36).
    List(Rc<Vec<V>>),
    /// Go duyệt map theo thứ tự khoá, `toYaml` cũng vậy.
    Map(Rc<BTreeMap<String, V>>),
}

impl V {
    fn truthy(&self) -> bool {
        match self {
            V::Nil => false,
            V::Bool(b) => *b,
            V::Int(i) => *i != 0,
            V::Float(f) => *f != 0.0,
            V::Str(s) => !s.is_empty(),
            V::List(l) => !l.is_empty(),
            V::Map(m) => !m.is_empty(),
        }
    }

    /// Như `fmt.Sprint` của Go với giá trị đã gỡ khỏi YAML.
    fn text(&self) -> String {
        match self {
            V::Nil => String::new(),
            V::Bool(b) => b.to_string(),
            V::Int(i) => i.to_string(),
            V::Float(f) => float_text(*f),
            V::Str(s) => s.clone(),
            V::List(l) => format!("[{}]", l.iter().map(V::text).collect::<Vec<_>>().join(" ")),
            V::Map(m) => format!(
                "map[{}]",
                m.iter()
                    .map(|(k, v)| format!("{k}:{}", v.text()))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        }
    }

    fn get(&self, key: &str) -> V {
        match self {
            V::Map(m) => m.get(key).cloned().unwrap_or(V::Nil),
            _ => V::Nil,
        }
    }
}

fn map(m: BTreeMap<String, V>) -> V {
    V::Map(Rc::new(m))
}

/// `%v` của Go cho float64: ngắn nhất, chuyển sang dạng mũ khi số mũ dưới -4
/// hay từ 6 trở lên (`1e+06`, `1.5e+07`).
fn float_text(f: f64) -> String {
    if f == 0.0 {
        return "0".into();
    }
    if !f.is_finite() {
        return if f.is_nan() {
            "NaN".into()
        } else if f > 0.0 {
            "+Inf".into()
        } else {
            "-Inf".into()
        };
    }
    let e = format!("{f:e}");
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if !(-4..6).contains(&exp) {
        format!("{mant}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs())
    } else {
        format!("{f}")
    }
}

/// Số trong `toJson`/`toYaml` như `encoding/json` của Go: không dùng dạng mũ
/// trong khoảng [1e-6, 1e21) (`2000000`, không phải `2e+06`).
fn json_num(f: f64) -> String {
    let a = f.abs();
    if f.is_finite() && (a == 0.0 || (1e-6..1e21).contains(&a)) {
        format!("{f}")
    } else {
        float_text(f)
    }
}

/// Giá trị YAML → giá trị template. Vô hướng trần theo YAML 1.1 như
/// `sigs.k8s.io/yaml` của Helm: `true`, `yes`, `on` là bool, số là số.
pub fn from_yaml(n: Node<'_>, budget: &mut usize) -> V {
    from_yaml_at(n, budget, 0)
}

/// Độ sâu tối đa của giá trị values. Alias cho một nút trỏ về nút có anchor,
/// nên độ sâu logic không bị trần của parser YAML giới hạn; quá trần thì cạn
/// ngân sách, file được báo chưa đọc được (claude-security F6, như
/// `kustomize::MAX_TREE_DEPTH`).
const MAX_VALUE_DEPTH: usize = 128;

fn from_yaml_at(n: Node<'_>, budget: &mut usize, depth: usize) -> V {
    if *budget == 0 {
        return V::Nil;
    }
    if depth > MAX_VALUE_DEPTH {
        *budget = 0;
        return V::Nil;
    }
    *budget -= 1;
    if n.is_map() {
        let mut m = BTreeMap::new();
        for (k, _, v) in n.entries() {
            // Khoá YAML 1.1 như `on:`, `y:` là bool, thành chuỗi `true`/`false`.
            let key = match k {
                "y" | "Y" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" | "true" | "True"
                | "TRUE" => "true",
                "n" | "N" | "no" | "No" | "NO" | "off" | "Off" | "OFF" | "false" | "False"
                | "FALSE" => "false",
                k => k,
            };
            m.insert(key.to_string(), from_yaml_at(v, budget, depth + 1));
        }
        return V::Map(Rc::new(m));
    }
    if n.is_seq() {
        return V::List(Rc::new(n.items().map(|i| from_yaml_at(i, budget, depth + 1)).collect()));
    }
    let Some(s) = n.as_str() else { return V::Nil };
    if !n.is_plain() {
        return V::Str(s.to_string());
    }
    match s {
        "true" | "True" | "TRUE" | "yes" | "Yes" | "YES" | "on" | "On" | "ON" | "y" | "Y" => {
            return V::Bool(true);
        }
        "false" | "False" | "FALSE" | "no" | "No" | "NO" | "off" | "Off" | "OFF" | "n" | "N" => {
            return V::Bool(false);
        }
        _ => {}
    }
    // Helm đọc values qua JSON: mọi số là float64.
    if let Ok(i) = s.parse::<i64>() {
        return V::Float(i as f64);
    }
    if s.contains(['.', 'e', 'E'])
        && s.bytes().any(|c| c.is_ascii_digit())
        && let Ok(f) = s.parse::<f64>()
    {
        return V::Float(f);
    }
    V::Str(s.to_string())
}

/// Gộp `over` vào `base` như Helm gộp nhiều file values: map gộp sâu, `null`
/// trong map của `over` xoá khoá đó, còn lại `over` thắng. `over` là `null` ở
/// cấp ngoài cùng (file rỗng, chart cha không có khoá của chart con) thì không
/// đổi gì.
pub fn merge(base: &mut V, over: V) {
    match (base, over) {
        (_, V::Nil) => {}
        (V::Map(b), V::Map(o)) => {
            let b = Rc::make_mut(b);
            for (k, v) in o.iter() {
                match (b.get_mut(k), v) {
                    (Some(_), V::Nil) => {
                        b.remove(k);
                    }
                    (Some(slot), v) => merge(slot, v.clone()),
                    (None, V::Nil) => {}
                    (None, v) => {
                        b.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o,
    }
}

// ---- template ----

/// Một mảnh template: chữ hay hành động `{{ … }}`.
#[derive(Debug, Clone)]
enum Item {
    Text(String, usize),
    Action(String, usize),
}

/// Lỗi dựng: ngoài tập con, hay template hỏng. Không bao giờ chứa giá trị.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub line: usize,
    pub why: String,
}

fn fail<T>(line: usize, why: impl Into<String>) -> Result<T, Unsupported> {
    Err(Unsupported {
        line,
        why: why.into(),
    })
}

/// Tách template thành chữ và hành động, xử lý `{{-` và `-}}`.
fn split(src: &str) -> Result<Vec<Item>, Unsupported> {
    let mut out = Vec::new();
    let mut rest = src;
    let mut line = 1usize;
    let mut trim_next = false;
    while let Some(at) = rest.find("{{") {
        let mut text = &rest[..at];
        if trim_next {
            let t = text.trim_start();
            line += text[..text.len() - t.len()].matches('\n').count();
            text = t;
        }
        let after = &rest[at + 2..];
        let left_trim =
            after.starts_with("- ") || after.starts_with("-\t") || after.starts_with("-\n");
        let text_owned = if left_trim { text.trim_end() } else { text };
        out.push(Item::Text(text_owned.to_string(), line));
        line += text.matches('\n').count();
        if trim_next {
            // already counted the trimmed prefix
        }
        let body_start = if left_trim { 1 } else { 0 };
        let body = &after[body_start..];
        // Chú thích `{{/* … */}}` có thể chứa `}}`.
        let close = if body.trim_start().starts_with("/*") {
            match body.find("*/") {
                Some(p) => body[p..].find("}}").map(|q| p + q),
                None => None,
            }
        } else {
            find_close(body)
        };
        let Some(end) = close else {
            return fail(line, "`{{` không đóng");
        };
        let mut inner = &body[..end];
        trim_next = false;
        if inner.ends_with(" -") || inner.ends_with("\t-") || inner.ends_with("\n-") {
            inner = &inner[..inner.len() - 1];
            trim_next = true;
        }
        let action_line = line;
        line += inner.matches('\n').count();
        let trimmed = inner.trim();
        if !trimmed.starts_with("/*") {
            out.push(Item::Action(trimmed.to_string(), action_line));
        }
        rest = &body[end + 2..];
    }
    let mut text = rest;
    if trim_next {
        text = text.trim_start();
    }
    out.push(Item::Text(text.to_string(), line));
    Ok(out)
}

/// `}}` đóng hành động, bỏ qua `}}` trong chuỗi.
fn find_close(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'`' => {
                i += 1;
                while i < b.len() && b[i] != b'`' {
                    i += 1;
                }
            }
            b'}' if b.get(i + 1) == Some(&b'}') => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Cây template.
#[derive(Debug, Clone)]
enum Node2 {
    Text(String, usize),
    Out(Pipe, usize),
    If(Vec<(Pipe, Vec<Node2>)>, Vec<Node2>, usize),
    With(Pipe, Vec<Node2>, Vec<Node2>, usize),
    Range(Vec<String>, Pipe, Vec<Node2>, Vec<Node2>, usize),
    Call(String, Option<Pipe>, usize),
    Assign(Pipe, usize),
}

#[derive(Debug, Clone)]
struct Pipe {
    /// `$x :=` hay `$i, $v :=` (chỉ trong `range`), hay `$x =`.
    decl: Vec<String>,
    assign: bool,
    cmds: Vec<Vec<Arg>>,
}

#[derive(Debug, Clone)]
enum Arg {
    /// `.a.b`, `.` (đường dẫn rỗng).
    Field(Vec<String>),
    /// `$x.a.b`; `$` là gốc.
    Var(String, Vec<String>),
    Lit(V),
    Ident(String),
    Sub(Box<Pipe>, Vec<String>),
}

/// Đọc các hành động thành cây; trả cây và các `define`.
fn parse(
    items: &[Item],
    defines: &mut HashMap<String, Vec<Node2>>,
) -> Result<Vec<Node2>, Unsupported> {
    let mut i = 0;
    let (nodes, end) = block(items, &mut i, defines, 0)?;
    if let Some((word, line)) = end {
        return fail(line, format!("`{word}` không có khối mở"));
    }
    Ok(nodes)
}

/// Đọc tới `end`/`else` của khối hiện tại. Trả nút và từ kết thúc.
#[allow(clippy::type_complexity)]
fn block(
    items: &[Item],
    i: &mut usize,
    defines: &mut HashMap<String, Vec<Node2>>,
    depth: usize,
) -> Result<(Vec<Node2>, Option<(String, usize)>), Unsupported> {
    if depth > MAX_DEPTH {
        return fail(0, format!("khối lồng sâu quá {MAX_DEPTH}"));
    }
    let mut out = Vec::new();
    while *i < items.len() {
        let item = &items[*i];
        *i += 1;
        let (a, line) = match item {
            Item::Text(t, l) => {
                if !t.is_empty() {
                    out.push(Node2::Text(t.clone(), *l));
                }
                continue;
            }
            Item::Action(a, l) => (a.as_str(), *l),
        };
        let (word, rest) = a.split_once(char::is_whitespace).unwrap_or((a, ""));
        let rest = rest.trim();
        match word {
            "end" => return Ok((out, Some(("end".into(), line)))),
            "else" => return Ok((out, Some((format!("else {rest}").trim().to_string(), line)))),
            "if" => {
                let mut arms = vec![(pipe(rest, line)?, Vec::new())];
                let mut otherwise = Vec::new();
                loop {
                    let (body, end) = block(items, i, defines, depth + 1)?;
                    let Some((end, eline)) = end else {
                        return fail(line, "`if` không có `end`");
                    };
                    if let Some(last) = arms.last_mut().filter(|_| otherwise.is_empty()) {
                        if last.1.is_empty() {
                            last.1 = body;
                        } else {
                            otherwise = body;
                        }
                    }
                    if end == "end" {
                        break;
                    }
                    if let Some(cond) = end.strip_prefix("else if ") {
                        arms.push((pipe(cond, eline)?, Vec::new()));
                        continue;
                    }
                    if end == "else" {
                        let (body, end2) = block(items, i, defines, depth + 1)?;
                        otherwise = body;
                        if end2.as_ref().map(|e| e.0.as_str()) != Some("end") {
                            return fail(eline, "`else` không có `end`");
                        }
                        break;
                    }
                    return fail(eline, format!("`{end}` không hỗ trợ"));
                }
                out.push(Node2::If(arms, otherwise, line));
            }
            "with" => {
                let p = pipe(rest, line)?;
                let (body, end) = block(items, i, defines, depth + 1)?;
                let otherwise = match end {
                    Some((e, _)) if e == "end" => Vec::new(),
                    Some((e, el)) if e == "else" => {
                        let (b, e2) = block(items, i, defines, depth + 1)?;
                        if e2.as_ref().map(|e| e.0.as_str()) != Some("end") {
                            return fail(el, "`else` không có `end`");
                        }
                        b
                    }
                    _ => return fail(line, "`with` không có `end`"),
                };
                out.push(Node2::With(p, body, otherwise, line));
            }
            "range" => {
                let p = pipe(rest, line)?;
                let vars = p.decl.clone();
                let (body, end) = block(items, i, defines, depth + 1)?;
                let otherwise = match end {
                    Some((e, _)) if e == "end" => Vec::new(),
                    Some((e, el)) if e == "else" => {
                        let (b, e2) = block(items, i, defines, depth + 1)?;
                        if e2.as_ref().map(|e| e.0.as_str()) != Some("end") {
                            return fail(el, "`else` không có `end`");
                        }
                        b
                    }
                    _ => return fail(line, "`range` không có `end`"),
                };
                out.push(Node2::Range(
                    vars,
                    Pipe {
                        decl: Vec::new(),
                        ..p
                    },
                    body,
                    otherwise,
                    line,
                ));
            }
            "define" | "block" => {
                let (name, arg) = string_lit(rest, line)?;
                let (body, end) = block(items, i, defines, depth + 1)?;
                if end.as_ref().map(|e| e.0.as_str()) != Some("end") {
                    return fail(line, format!("`{word}` không có `end`"));
                }
                defines.insert(name.clone(), body);
                if word == "block" {
                    out.push(Node2::Call(
                        name,
                        if arg.is_empty() {
                            None
                        } else {
                            Some(pipe(arg, line)?)
                        },
                        line,
                    ));
                }
            }
            "template" => {
                let (name, arg) = string_lit(rest, line)?;
                out.push(Node2::Call(
                    name,
                    if arg.is_empty() {
                        None
                    } else {
                        Some(pipe(arg, line)?)
                    },
                    line,
                ));
            }
            "break" | "continue" => return fail(line, format!("`{word}` không hỗ trợ")),
            _ => {
                let p = pipe(a, line)?;
                if p.decl.is_empty() {
                    out.push(Node2::Out(p, line));
                } else {
                    out.push(Node2::Assign(p, line));
                }
            }
        }
    }
    Ok((out, None))
}

/// `"tên" phần-còn-lại`.
fn string_lit(s: &str, line: usize) -> Result<(String, &str), Unsupported> {
    let toks = lex(s, line)?;
    match toks.first() {
        Some(Tok::Str(name)) => {
            let used = s.find(['"', '`']).map_or(0, |a| {
                let q = s.as_bytes()[a];
                let mut j = a + 1;
                while j < s.len() && s.as_bytes()[j] != q {
                    if s.as_bytes()[j] == b'\\' && q == b'"' {
                        j += 1;
                    }
                    j += 1;
                }
                j + 1
            });
            Ok((name.clone(), s.get(used..).unwrap_or("").trim()))
        }
        _ => fail(line, "thiếu tên template"),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Str(String),
    Num(V),
    Word(String),
    Field(Vec<String>),
    /// Đường dẫn dính liền sau `)`: `(…).a.b`.
    Chain(Vec<String>),
    Var(String, Vec<String>),
    Open,
    Close,
    Bar,
    Decl,
    Assign,
    Comma,
}

fn lex(s: &str, line: usize) -> Result<Vec<Tok>, Unsupported> {
    let cs: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let path = |cs: &[char], i: &mut usize| -> Vec<String> {
        let mut p = Vec::new();
        while *i < cs.len() && cs[*i] == '.' {
            *i += 1;
            let start = *i;
            while *i < cs.len() && (cs[*i].is_alphanumeric() || cs[*i] == '_') {
                *i += 1;
            }
            if *i > start {
                p.push(cs[start..*i].iter().collect());
            }
        }
        p
    };
    while i < cs.len() {
        let c = cs[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                // `(…).a.b`
                i += 1;
                let p = path(&cs, &mut i);
                out.push(Tok::Close);
                if !p.is_empty() {
                    out.push(Tok::Chain(p));
                }
            }
            '|' => {
                out.push(Tok::Bar);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            ':' if cs.get(i + 1) == Some(&'=') => {
                out.push(Tok::Decl);
                i += 2;
            }
            '=' => {
                out.push(Tok::Assign);
                i += 1;
            }
            '"' => {
                let mut v = String::new();
                i += 1;
                while i < cs.len() && cs[i] != '"' {
                    if cs[i] == '\\' && i + 1 < cs.len() {
                        i += 1;
                        v.push(match cs[i] {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            c => c,
                        });
                    } else {
                        v.push(cs[i]);
                    }
                    i += 1;
                }
                i += 1;
                out.push(Tok::Str(v));
            }
            '`' => {
                let start = i + 1;
                i += 1;
                while i < cs.len() && cs[i] != '`' {
                    i += 1;
                }
                out.push(Tok::Str(cs[start..i.min(cs.len())].iter().collect()));
                i += 1;
            }
            '.' => {
                let p = path(&cs, &mut i);
                out.push(Tok::Field(p));
            }
            '$' => {
                i += 1;
                let start = i;
                while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                    i += 1;
                }
                let name: String = cs[start..i].iter().collect();
                let p = path(&cs, &mut i);
                out.push(Tok::Var(name, p));
            }
            c if c.is_ascii_digit()
                || (c == '-' && cs.get(i + 1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                let start = i;
                i += 1;
                while i < cs.len() && (cs[i].is_ascii_alphanumeric() || cs[i] == '.') {
                    i += 1;
                }
                let t: String = cs[start..i].iter().collect();
                let v = t
                    .parse::<i64>()
                    .map(V::Int)
                    .or_else(|_| t.parse::<f64>().map(V::Float));
                match v {
                    Ok(v) => out.push(Tok::Num(v)),
                    Err(_) => return fail(line, "số không đọc được"),
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while i < cs.len() && (cs[i].is_alphanumeric() || cs[i] == '_') {
                    i += 1;
                }
                out.push(Tok::Word(cs[start..i].iter().collect()));
            }
            '\'' => return fail(line, "hằng ký tự không hỗ trợ"),
            _ => return fail(line, "ký tự lạ trong hành động"),
        }
    }
    Ok(out)
}

fn pipe(s: &str, line: usize) -> Result<Pipe, Unsupported> {
    let toks = lex(s, line)?;
    let mut i = 0;
    let p = pipe_toks(&toks, &mut i, line, 0)?;
    if i < toks.len() {
        return fail(line, "hành động có phần thừa");
    }
    Ok(p)
}

fn pipe_toks(toks: &[Tok], i: &mut usize, line: usize, depth: usize) -> Result<Pipe, Unsupported> {
    if depth > MAX_DEPTH {
        return fail(line, "ngoặc lồng quá sâu");
    }
    let mut decl = Vec::new();
    let mut assign = false;
    // `$x :=`, `$i, $v :=`, `$x =`.
    let mut j = *i;
    let mut names = Vec::new();
    while let Some(Tok::Var(n, p)) = toks.get(j) {
        if !p.is_empty() {
            break;
        }
        names.push(n.clone());
        j += 1;
        if toks.get(j) == Some(&Tok::Comma) {
            j += 1;
            continue;
        }
        break;
    }
    if !names.is_empty() && matches!(toks.get(j), Some(Tok::Decl | Tok::Assign)) {
        assign = toks.get(j) == Some(&Tok::Assign);
        decl = names;
        *i = j + 1;
    }
    let mut cmds = Vec::new();
    let mut cmd = Vec::new();
    while *i < toks.len() {
        match &toks[*i] {
            Tok::Close => break,
            Tok::Bar => {
                if cmd.is_empty() {
                    return fail(line, "`|` không có lệnh trước");
                }
                cmds.push(std::mem::take(&mut cmd));
                *i += 1;
            }
            Tok::Open => {
                *i += 1;
                let sub = pipe_toks(toks, i, line, depth + 1)?;
                if toks.get(*i) != Some(&Tok::Close) {
                    return fail(line, "ngoặc không đóng");
                }
                *i += 1;
                let path = match toks.get(*i) {
                    Some(Tok::Chain(p)) => {
                        *i += 1;
                        p.clone()
                    }
                    _ => Vec::new(),
                };
                cmd.push(Arg::Sub(Box::new(sub), path));
            }
            Tok::Str(s) => {
                cmd.push(Arg::Lit(V::Str(s.clone())));
                *i += 1;
            }
            Tok::Num(v) => {
                cmd.push(Arg::Lit(v.clone()));
                *i += 1;
            }
            Tok::Word(w) => {
                cmd.push(match w.as_str() {
                    "true" => Arg::Lit(V::Bool(true)),
                    "false" => Arg::Lit(V::Bool(false)),
                    "nil" => Arg::Lit(V::Nil),
                    _ => Arg::Ident(w.clone()),
                });
                *i += 1;
            }
            Tok::Field(p) => {
                cmd.push(Arg::Field(p.clone()));
                *i += 1;
            }
            Tok::Var(n, p) => {
                cmd.push(Arg::Var(n.clone(), p.clone()));
                *i += 1;
            }
            Tok::Chain(_) => return fail(line, "đường dẫn sau `)` sai chỗ"),
            Tok::Decl | Tok::Assign | Tok::Comma => return fail(line, "khai biến sai chỗ"),
        }
    }
    if !cmd.is_empty() {
        cmds.push(cmd);
    }
    if cmds.is_empty() {
        return fail(line, "hành động rỗng");
    }
    Ok(Pipe { decl, assign, cmds })
}

// ---- dựng ----

struct Env<'a> {
    defines: &'a HashMap<String, Vec<Node2>>,
    root: V,
    steps: usize,
    out_bytes: usize,
    /// Byte của giá trị dựng ra bởi hàm (chuỗi, list, map), cộng dồn.
    made: usize,
    depth: usize,
    /// Độ lồng của list và map do hàm dựng ra, theo địa chỉ `Rc` (giữ một bản
    /// để địa chỉ không bị dùng lại). Values từ YAML đã có trần riêng (TM-36).
    nest: HashMap<usize, (usize, V)>,
}

struct Out {
    text: String,
    /// Dòng template của từng dòng dựng ra (đếm từ 1).
    lines: Vec<usize>,
}

impl Out {
    fn new() -> Out {
        Out {
            text: String::new(),
            lines: vec![1],
        }
    }

    fn push(&mut self, s: &str, mut line: usize, advance: bool) {
        if self.lines.len() == 1 && self.text.is_empty() {
            self.lines[0] = line;
        }
        for c in s.chars() {
            self.text.push(c);
            if c == '\n' {
                if advance {
                    line += 1;
                }
                self.lines.push(line);
            }
        }
    }
}

type Vars = Vec<(String, V)>;

impl Env<'_> {
    fn step(&mut self, line: usize) -> Result<(), Unsupported> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return fail(line, format!("quá {MAX_STEPS} bước dựng"));
        }
        Ok(())
    }

    fn emit(
        &mut self,
        out: &mut Out,
        s: &str,
        line: usize,
        advance: bool,
    ) -> Result<(), Unsupported> {
        self.out_bytes += s.len();
        if self.out_bytes > MAX_OUTPUT {
            return fail(line, format!("dựng ra quá {} MB", MAX_OUTPUT >> 20));
        }
        out.push(s, line, advance);
        Ok(())
    }

    fn run(
        &mut self,
        nodes: &[Node2],
        dot: &V,
        vars: &mut Vars,
        out: &mut Out,
    ) -> Result<(), Unsupported> {
        let mark = vars.len();
        for n in nodes {
            match n {
                Node2::Text(t, l) => {
                    self.step(*l)?;
                    self.emit(out, t, *l, true)?;
                }
                Node2::Out(p, l) => {
                    self.step(*l)?;
                    let v = self.pipe(p, dot, vars, *l)?;
                    self.emit(out, &v.text(), *l, false)?;
                }
                Node2::Assign(p, l) => {
                    self.step(*l)?;
                    let v = self.pipe(
                        &Pipe {
                            decl: Vec::new(),
                            ..p.clone()
                        },
                        dot,
                        vars,
                        *l,
                    )?;
                    let name = p.decl[0].clone();
                    if p.assign {
                        match vars.iter_mut().rev().find(|(n, _)| *n == name) {
                            Some(slot) => slot.1 = v,
                            None => return fail(*l, "gán cho biến chưa khai"),
                        }
                    } else {
                        vars.push((name, v));
                    }
                }
                Node2::If(arms, otherwise, l) => {
                    self.step(*l)?;
                    let mut done = false;
                    for (cond, body) in arms {
                        if self.pipe(cond, dot, vars, *l)?.truthy() {
                            self.run(body, dot, vars, out)?;
                            done = true;
                            break;
                        }
                    }
                    if !done {
                        self.run(otherwise, dot, vars, out)?;
                    }
                }
                Node2::With(p, body, otherwise, l) => {
                    self.step(*l)?;
                    let v = self.pipe(p, dot, vars, *l)?;
                    if v.truthy() {
                        self.run(body, &v, vars, out)?;
                    } else {
                        self.run(otherwise, dot, vars, out)?;
                    }
                }
                Node2::Range(names, p, body, otherwise, l) => {
                    self.step(*l)?;
                    let v = self.pipe(p, dot, vars, *l)?;
                    // Lặp lười: `range 9223372036854775807` chạm trần bước chứ
                    // không cấp phát trước (TM-36).
                    let n = match &v {
                        V::List(list) => list.len(),
                        V::Map(m) => m.len(),
                        V::Int(n) if *n >= 0 => usize::try_from(*n).unwrap_or(usize::MAX),
                        V::Int(_) | V::Nil => 0,
                        _ => return fail(*l, "`range` trên giá trị không lặp được"),
                    };
                    if n == 0 {
                        self.run(otherwise, dot, vars, out)?;
                    }
                    let mut keys = match &v {
                        V::Map(m) => Some(m.iter()),
                        _ => None,
                    };
                    for i in 0..n {
                        self.step(*l)?;
                        let (k, x) = match (&v, keys.as_mut()) {
                            (V::List(list), _) => (V::Int(i as i64), list[i].clone()),
                            (V::Map(_), Some(it)) => match it.next() {
                                Some((k, x)) => (V::Str(k.clone()), x.clone()),
                                None => break,
                            },
                            _ => (V::Int(i as i64), V::Int(i as i64)),
                        };
                        let m = vars.len();
                        match names.as_slice() {
                            [] => {}
                            [v1] => vars.push((v1.clone(), x.clone())),
                            [k1, v1] => {
                                vars.push((k1.clone(), k));
                                vars.push((v1.clone(), x.clone()));
                            }
                            _ => return fail(*l, "`range` khai quá hai biến"),
                        }
                        self.run(body, &x, vars, out)?;
                        vars.truncate(m);
                    }
                }
                Node2::Call(name, arg, l) => {
                    self.step(*l)?;
                    let d = match arg {
                        Some(p) => self.pipe(p, dot, vars, *l)?,
                        None => V::Nil,
                    };
                    let text = self.include(name, d, *l, true)?;
                    self.emit(out, &text, *l, false)?;
                }
            }
        }
        vars.truncate(mark);
        Ok(())
    }

    /// `literal`: tên viết thẳng (`template "x"`). Tên ghép từ values (`include
    /// (printf "pw-%s" .Values.x)`) không được in ra (TM-38).
    fn include(
        &mut self,
        name: &str,
        dot: V,
        line: usize,
        literal: bool,
    ) -> Result<String, Unsupported> {
        let Some(body) = self.defines.get(name) else {
            return fail(
                line,
                if literal {
                    format!("không có template `{name}`")
                } else {
                    "`include` gọi template không có".into()
                },
            );
        };
        if self.depth >= MAX_DEPTH {
            return fail(line, format!("`include` lồng sâu quá {MAX_DEPTH}"));
        }
        self.depth += 1;
        let mut out = Out::new();
        let mut vars = vec![("".to_string(), dot.clone())];
        let r = self.run(body, &dot, &mut vars, &mut out);
        self.depth -= 1;
        r.map(|_| out.text)
    }

    fn pipe(&mut self, p: &Pipe, dot: &V, vars: &mut Vars, line: usize) -> Result<V, Unsupported> {
        let mut prev: Option<V> = None;
        for cmd in &p.cmds {
            self.step(line)?;
            prev = Some(self.command(cmd, prev, dot, vars, line)?);
        }
        Ok(prev.unwrap_or(V::Nil))
    }

    fn arg(&mut self, a: &Arg, dot: &V, vars: &mut Vars, line: usize) -> Result<V, Unsupported> {
        match a {
            Arg::Field(path) => self.field(dot, path, line),
            Arg::Var(name, path) => {
                let base = if name.is_empty() {
                    vars.first()
                        .map(|v| v.1.clone())
                        .unwrap_or_else(|| self.root.clone())
                } else {
                    match vars.iter().rev().find(|(n, _)| n == name) {
                        Some((_, v)) => v.clone(),
                        None => return fail(line, "biến chưa khai"),
                    }
                };
                self.field(&base, path, line)
            }
            Arg::Lit(v) => Ok(v.clone()),
            Arg::Sub(p, path) => {
                let v = self.pipe(p, dot, vars, line)?;
                self.field(&v, path, line)
            }
            Arg::Ident(f) => self.call(f, Vec::new(), line),
        }
    }

    fn field(&mut self, base: &V, path: &[String], line: usize) -> Result<V, Unsupported> {
        let mut v = base.clone();
        for k in path {
            // Gốc đi đường nào cũng vậy: `.Files`, `$.Capabilities`,
            // `(dict "top" $).top.Capabilities`.
            let is_root = matches!((&v, &self.root), (V::Map(a), V::Map(b)) if Rc::ptr_eq(a, b));
            if is_root && matches!(k.as_str(), "Files" | "Capabilities" | "Subcharts") {
                return fail(line, format!("`.{k}` không dựng được ngoài helm"));
            }
            v = v.get(k);
        }
        Ok(v)
    }

    fn command(
        &mut self,
        cmd: &[Arg],
        prev: Option<V>,
        dot: &V,
        vars: &mut Vars,
        line: usize,
    ) -> Result<V, Unsupported> {
        match &cmd[0] {
            Arg::Ident(f) => {
                let mut args = Vec::new();
                for a in &cmd[1..] {
                    args.push(self.arg(a, dot, vars, line)?);
                }
                if let Some(p) = prev {
                    args.push(p);
                }
                self.call(f, args, line)
            }
            first => {
                // Tính trước: `.Files.Get "a"` báo đúng `.Files`.
                let v = self.arg(first, dot, vars, line)?;
                if cmd.len() > 1 || prev.is_some() {
                    return fail(line, "gọi phương thức trên giá trị không hỗ trợ");
                }
                Ok(v)
            }
        }
    }

    fn call(&mut self, f: &str, a: Vec<V>, line: usize) -> Result<V, Unsupported> {
        let v = self.call_inner(f, a, line)?;
        // Giá trị trung gian cũng tính: `printf "%s%s" $s $s` lặp nhân đôi (TM-36).
        self.made += match &v {
            V::Str(s) => s.len(),
            V::List(l) => 16 * l.len(),
            V::Map(m) => 32 * m.len(),
            _ => 0,
        };
        if self.made > MAX_MADE {
            return fail(
                line,
                format!("giá trị dựng trong template quá {} MB", MAX_MADE >> 20),
            );
        }
        // `list $d` lặp lồng không đáy: đệ quy khi in, so sánh, giải phóng sẽ
        // tràn stack. Chặn ngay lúc dựng; chỉ duyệt con trực tiếp của giá trị
        // mới, đã tính vào `made`.
        let key = |v: &V| match v {
            V::List(l) => Some(Rc::as_ptr(l) as *const u8 as usize),
            V::Map(m) => Some(Rc::as_ptr(m) as *const u8 as usize),
            _ => None,
        };
        if let Some(k) = key(&v)
            && !self.nest.contains_key(&k)
        {
            let kids: Vec<&V> = match &v {
                V::List(l) => l.iter().collect(),
                V::Map(m) => m.values().collect(),
                _ => Vec::new(),
            };
            let d = 1 + kids
                .iter()
                .filter_map(|c| key(c))
                .map(|c| self.nest.get(&c).map_or(0, |e| e.0))
                .max()
                .unwrap_or(0);
            if d > MAX_DEPTH {
                return fail(
                    line,
                    format!("giá trị dựng trong template lồng sâu quá {MAX_DEPTH}"),
                );
            }
            self.nest.insert(k, (d, v.clone()));
        }
        Ok(v)
    }

    fn call_inner(&mut self, f: &str, a: Vec<V>, line: usize) -> Result<V, Unsupported> {
        self.step(line)?;
        let need = |n: usize| -> Result<(), Unsupported> {
            if a.len() < n {
                fail(line, format!("`{f}` thiếu đối số"))
            } else {
                Ok(())
            }
        };
        let s = |i: usize| a.get(i).map(V::text).unwrap_or_default();
        let num = |v: &V| -> i64 {
            match v {
                V::Int(i) => *i,
                V::Float(f) => *f as i64,
                V::Str(s) => s.trim().parse().unwrap_or(0),
                V::Bool(b) => *b as i64,
                _ => 0,
            }
        };
        Ok(match f {
            "include" => {
                need(1)?;
                let name = s(0);
                V::Str(self.include(&name, a.get(1).cloned().unwrap_or(V::Nil), line, false)?)
            }
            "toYaml" => {
                need(1)?;
                let mut out = String::new();
                to_yaml(&a[0], 0, &mut out);
                V::Str(out.trim_end_matches('\n').to_string())
            }
            "toJson" | "toRawJson" | "toPrettyJson" => {
                need(1)?;
                V::Str(to_json(&a[0]))
            }
            "indent" | "nindent" => {
                need(2)?;
                let pad = " ".repeat(num(&a[0]).clamp(0, 256) as usize);
                let body = a[1]
                    .text()
                    .lines()
                    .map(|l| format!("{pad}{l}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                V::Str(if f == "nindent" {
                    format!("\n{body}")
                } else {
                    body
                })
            }
            // Sprig bỏ đối số `nil`.
            "quote" => V::Str(
                a.iter()
                    .filter(|v| **v != V::Nil)
                    .map(|v| format!("{:?}", v.text()))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            "squote" => V::Str(
                a.iter()
                    .filter(|v| **v != V::Nil)
                    .map(|v| format!("'{}'", v.text()))
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            "default" => {
                need(1)?;
                match a.get(1) {
                    Some(v) if v.truthy() => v.clone(),
                    _ => a[0].clone(),
                }
            }
            "required" => {
                need(2)?;
                if a[1] == V::Nil || (matches!(&a[1], V::Str(x) if x.is_empty())) {
                    return fail(line, "`required` không có giá trị");
                }
                a[1].clone()
            }
            "fail" => return fail(line, "chart gọi `fail`"),
            "empty" => V::Bool(!a.first().is_some_and(V::truthy)),
            "not" => V::Bool(!a.first().is_some_and(V::truthy)),
            "and" => a
                .iter()
                .find(|v| !v.truthy())
                .or(a.last())
                .cloned()
                .unwrap_or(V::Nil),
            "or" => a
                .iter()
                .find(|v| v.truthy())
                .or(a.last())
                .cloned()
                .unwrap_or(V::Nil),
            "coalesce" => a.iter().find(|v| v.truthy()).cloned().unwrap_or(V::Nil),
            "ternary" => {
                need(3)?;
                if a[2].truthy() {
                    a[0].clone()
                } else {
                    a[1].clone()
                }
            }
            "eq" => {
                need(2)?;
                V::Bool(a[1..].iter().any(|x| same(&a[0], x)))
            }
            "ne" => {
                need(2)?;
                V::Bool(!same(&a[0], &a[1]))
            }
            "lt" | "le" | "gt" | "ge" => {
                need(2)?;
                // Có số thực thì so bằng f64: `gt 0.7 0.6` là đúng.
                let fl = |v: &V| {
                    if let V::Float(x) = v {
                        *x
                    } else {
                        num(v) as f64
                    }
                };
                let (x, y) = (fl(&a[0]), fl(&a[1]));
                V::Bool(match f {
                    "lt" => x < y,
                    "le" => x <= y,
                    "gt" => x > y,
                    _ => x >= y,
                })
            }
            "print" => V::Str(a.iter().map(V::text).collect::<String>()),
            "println" => V::Str(format!(
                "{}\n",
                a.iter().map(V::text).collect::<Vec<_>>().join(" ")
            )),
            "printf" => {
                need(1)?;
                V::Str(printf(&s(0), &a[1..], line)?)
            }
            "toString" => V::Str(s(0)),
            "int" | "int64" | "atoi" => V::Int(a.first().map(num).unwrap_or(0)),
            "lower" => V::Str(s(0).to_lowercase()),
            "upper" => V::Str(s(0).to_uppercase()),
            "title" => V::Str(s(0).split(' ').map(title).collect::<Vec<_>>().join(" ")),
            "trim" => V::Str(s(0).trim().to_string()),
            "trimSuffix" => {
                need(2)?;
                V::Str(
                    s(1).strip_suffix(&s(0))
                        .map(str::to_string)
                        .unwrap_or_else(|| s(1)),
                )
            }
            "trimPrefix" => {
                need(2)?;
                V::Str(
                    s(1).strip_prefix(&s(0))
                        .map(str::to_string)
                        .unwrap_or_else(|| s(1)),
                )
            }
            "trunc" => {
                need(2)?;
                let n = num(&a[0]);
                let t = s(1);
                V::Str(if n >= 0 {
                    t.chars().take(n as usize).collect()
                } else {
                    t
                })
            }
            "replace" => {
                need(3)?;
                V::Str(s(2).replace(&s(0), &s(1)))
            }
            "contains" => {
                need(2)?;
                V::Bool(s(1).contains(&s(0)))
            }
            "hasPrefix" => {
                need(2)?;
                V::Bool(s(1).starts_with(&s(0)))
            }
            "hasSuffix" => {
                need(2)?;
                V::Bool(s(1).ends_with(&s(0)))
            }
            "len" => V::Int(match a.first() {
                Some(V::List(l)) => l.len() as i64,
                Some(V::Map(m)) => m.len() as i64,
                Some(v) => v.text().chars().count() as i64,
                None => 0,
            }),
            "list" => V::List(Rc::new(a)),
            "dict" => {
                let mut m = BTreeMap::new();
                for pair in a.chunks(2) {
                    m.insert(pair[0].text(), pair.get(1).cloned().unwrap_or(V::Nil));
                }
                V::Map(Rc::new(m))
            }
            "get" => {
                need(2)?;
                a[0].get(&s(1))
            }
            "hasKey" => {
                need(2)?;
                V::Bool(matches!(&a[0], V::Map(m) if m.contains_key(&s(1))))
            }
            "keys" => V::List(Rc::new(match a.first() {
                Some(V::Map(m)) => m.keys().map(|k| V::Str(k.clone())).collect(),
                _ => Vec::new(),
            })),
            "join" => {
                need(2)?;
                V::Str(match &a[1] {
                    V::List(l) => l.iter().map(V::text).collect::<Vec<_>>().join(&s(0)),
                    v => v.text(),
                })
            }
            "first" => match a.first() {
                Some(V::List(l)) => l.first().cloned().unwrap_or(V::Nil),
                _ => V::Nil,
            },
            "kindIs" => {
                need(2)?;
                V::Bool(kind_of(&a[1]) == s(0))
            }
            "kindOf" | "typeOf" => V::Str(kind_of(a.first().unwrap_or(&V::Nil)).to_string()),
            "b64enc" => V::Str(b64(s(0).as_bytes())),
            "index" => {
                need(1)?;
                let mut v = a[0].clone();
                for k in &a[1..] {
                    v = match (&v, k) {
                        (V::List(l), V::Int(i)) => l.get(*i as usize).cloned().unwrap_or(V::Nil),
                        (_, k) => v.get(&k.text()),
                    };
                }
                v
            }
            "semverCompare" | "tpl" | "lookup" | "include_file" | "getHostByName"
            | "randAlphaNum" | "randAlpha" | "uuidv4" | "now" | "genCA" | "genPrivateKey"
            | "genSelfSignedCert" | "genSignedCert" | "derivePassword" | "htpasswd" | "env"
            | "expandenv" => {
                return fail(line, format!("`{f}` không dựng được ngoài helm"));
            }
            _ => return fail(line, format!("hàm `{f}` chưa hỗ trợ")),
        })
    }
}

fn title(w: &str) -> String {
    let mut c = w.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn kind_of(v: &V) -> &'static str {
    match v {
        V::Nil => "invalid",
        V::Bool(_) => "bool",
        V::Int(_) => "int64",
        V::Float(_) => "float64",
        V::Str(_) => "string",
        V::List(_) => "slice",
        V::Map(_) => "map",
    }
}

/// So sánh của `eq`: số với số, còn lại theo chữ.
fn same(a: &V, b: &V) -> bool {
    match (a, b) {
        (V::Int(x), V::Float(y)) | (V::Float(y), V::Int(x)) => (*x as f64) == *y,
        (V::Nil, V::Str(s)) | (V::Str(s), V::Nil) => s.is_empty(),
        _ => a == b,
    }
}

fn printf(fmt: &str, args: &[V], line: usize) -> Result<String, Unsupported> {
    let mut out = String::new();
    let mut it = args.iter();
    let mut cs = fmt.chars().peekable();
    while let Some(c) = cs.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        // Cờ và độ rộng: bỏ qua, chỉ giữ động từ.
        while cs
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.' | ' ' | '#' | '0'))
        {
            cs.next();
        }
        match cs.next() {
            Some('%') => out.push('%'),
            Some('s' | 'v' | 'd') => out.push_str(&it.next().map(V::text).unwrap_or_default()),
            Some('q') => out.push_str(&format!("{:?}", it.next().map(V::text).unwrap_or_default())),
            Some(v) => return fail(line, format!("`printf` không hỗ trợ `%{v}`")),
            None => out.push('%'),
        }
    }
    Ok(out)
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, &b)| acc | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// YAML cho `toYaml`: khoá theo thứ tự, chuỗi để trong nháy khi cần.
fn to_yaml(v: &V, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    match v {
        V::Map(m) if !m.is_empty() => {
            for (k, x) in m.iter() {
                out.push_str(&format!("{pad}{}:", scalar(&V::Str(k.clone()))));
                block_value(x, indent, out);
            }
        }
        V::List(l) if !l.is_empty() => {
            for x in l.iter() {
                out.push_str(&format!("{pad}-"));
                match x {
                    // Phần tử là map hay list khác rỗng: dòng đầu nằm ngay sau `- `.
                    V::Map(m) if !m.is_empty() => {
                        let mut inner = String::new();
                        to_yaml(x, indent + 2, &mut inner);
                        out.push(' ');
                        out.push_str(inner.trim_start());
                    }
                    V::List(l) if !l.is_empty() => {
                        let mut inner = String::new();
                        to_yaml(x, indent + 2, &mut inner);
                        out.push(' ');
                        out.push_str(inner.trim_start());
                    }
                    _ => block_value(x, indent, out),
                }
            }
        }
        _ => {
            out.push_str(&pad);
            out.push_str(&scalar(v));
            out.push('\n');
        }
    }
}

fn block_value(x: &V, indent: usize, out: &mut String) {
    match x {
        V::Map(m) if !m.is_empty() => {
            out.push('\n');
            to_yaml(x, indent + 2, out);
        }
        V::List(l) if !l.is_empty() => {
            out.push('\n');
            to_yaml(x, indent, out);
        }
        _ => {
            out.push(' ');
            out.push_str(&scalar(x));
            out.push('\n');
        }
    }
}

fn scalar(v: &V) -> String {
    match v {
        V::Nil => "null".into(),
        V::Map(_) => "{}".into(),
        V::List(_) => "[]".into(),
        V::Str(s) => {
            let plain = !s.is_empty()
                && !s.contains([
                    ':', '#', '\n', '"', '\'', '{', '}', '[', ']', ',', '&', '*', '!', '|', '>',
                    '%', '@', '`',
                ])
                && !s.starts_with([' ', '-', '?'])
                && !s.ends_with(' ')
                && from_plain(s).is_none();
            if plain { s.clone() } else { format!("{s:?}") }
        }
        V::Float(f) => json_num(*f),
        other => other.text(),
    }
}

/// Chuỗi trần mà YAML đọc thành bool, số hay null: phải để trong nháy.
fn from_plain(s: &str) -> Option<()> {
    let lower = s.to_ascii_lowercase();
    (matches!(
        lower.as_str(),
        "true" | "false" | "yes" | "no" | "on" | "off" | "y" | "n" | "null" | "~"
    ) || s.parse::<f64>().is_ok())
    .then_some(())
}

fn to_json(v: &V) -> String {
    match v {
        V::Nil => "null".into(),
        V::Bool(b) => b.to_string(),
        V::Int(i) => i.to_string(),
        V::Float(f) => json_num(*f),
        V::Str(s) => crate::json::string(s),
        V::List(l) => format!("[{}]", l.iter().map(to_json).collect::<Vec<_>>().join(",")),
        V::Map(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, x)| format!("{}:{}", crate::json::string(k), to_json(x)))
                .collect::<Vec<_>>()
                .join(",")
        ),
    }
}

// ---- chart ----

/// Một file template đã dựng.
#[derive(Debug, Clone)]
pub struct Rendered {
    /// Đường dẫn template, như `helm-chart/templates/cart.yaml`.
    pub file: String,
    pub text: String,
    /// Dòng template của dòng dựng ra thứ `i + 1`.
    pub lines: Vec<usize>,
}

/// Kết quả dựng một chart: file dựng được, và file không dựng được kèm lý do.
#[derive(Debug, Clone, Default)]
pub struct Chart {
    pub rendered: Vec<Rendered>,
    pub unread: Vec<(String, String)>,
}

/// Dựng chart ở `dir`. `read` đọc một file trong repo (đã qua chốt đường dẫn);
/// `files` là mọi file của chart, kể cả chart con; `extra` là file values khai
/// thêm trong luật.
pub fn render(
    dir: &str,
    files: &[String],
    extra: &[String],
    read: &dyn Fn(&str) -> Result<String, String>,
) -> Chart {
    let mut chart = Chart::default();
    let p = prefix(dir);
    let Some(meta) = load_yaml(&format!("{p}Chart.yaml"), read, &mut chart, false) else {
        return chart;
    };
    let mut values = load_yaml(&format!("{p}values.yaml"), read, &mut chart, true)
        .unwrap_or_else(|| map(BTreeMap::new()));
    for e in extra {
        // File khai trong luật mà không có: báo, đừng lặng lẽ dùng giá trị mặc định.
        if read(e).is_err_and(|why| why == "không có") {
            chart
                .unread
                .push((e.clone(), "file values khai trong luật không có".into()));
            continue;
        }
        if let Some(v) = load_yaml(e, read, &mut chart, false) {
            merge(&mut values, v);
        }
    }
    for f in files.iter().filter(|f| f.ends_with(".tgz")) {
        chart
            .unread
            .push((f.clone(), "chart con nén `.tgz` không mở".into()));
    }

    // Cây chart như Helm: gộp values mặc định của mọi chart con lên chart cha
    // trước, rồi mới xét `condition`/`tags` trên values đã gộp và bỏ chart tắt.
    let node = build(dir, String::new(), meta, values, files, read, &mut chart, 0);
    let mut tree: Vec<Inst> = Vec::new();
    flatten(node, &mut chart, &mut tree);

    // Mỗi file thuộc chart sâu nhất chứa nó; tính một lần (đi lên thư mục cha).
    let charts: HashSet<String> = all_charts(files).into_iter().collect();
    let owner_of = |f: &str| -> Option<String> {
        let mut d = f;
        while let Some((up, _)) = d.rsplit_once('/') {
            if charts.contains(up) {
                return Some(up.to_string());
            }
            d = up;
        }
        charts.contains("").then(String::new)
    };
    let live: HashSet<&str> = tree.iter().map(|t| t.dir.as_str()).collect();
    let mut by_owner: HashMap<String, Vec<&String>> = HashMap::new();
    for f in files.iter().filter(|f| !f.ends_with(".tgz")) {
        if let Some(c) = owner_of(f)
            && f.starts_with(&format!("{}templates/", prefix(&c)))
        {
            by_owner.entry(c).or_default().push(f);
        }
    }
    // Template của các chart bật (define dùng chung cả cây; chart tắt không
    // tham gia, như Helm), theo thứ tự của Helm: nông trước, rồi theo đường
    // dẫn; bản đầu thắng, trừ khi nó rỗng (Go không thay một define bằng
    // define rỗng, và ngược lại).
    let mut templates: Vec<&String> = by_owner
        .iter()
        .filter(|(c, _)| live.contains(c.as_str()))
        .flat_map(|(_, fs)| fs.iter().copied())
        .collect();
    templates.sort_by_key(|f| (f.matches('/').count(), (*f).clone()));
    let mut defines: HashMap<String, Vec<Node2>> = HashMap::new();
    let mut parsed: HashMap<String, Vec<Node2>> = HashMap::new();
    for f in templates {
        let base = f.rsplit('/').next().unwrap_or(f);
        if base == "NOTES.txt" {
            continue;
        }
        let text = match read(f) {
            Ok(t) => t,
            Err(why) => {
                chart.unread.push((f.clone(), why));
                continue;
            }
        };
        let mut mine = HashMap::new();
        match split(&text).and_then(|items| parse(&items, &mut mine)) {
            Ok(tree) => {
                if !base.starts_with('_') {
                    parsed.insert(f.clone(), tree);
                }
            }
            Err(e) => chart
                .unread
                .push((f.clone(), format!("dòng {}: {}", e.line, e.why))),
        }
        for (name, body) in mine {
            let empty = |b: &[Node2]| {
                b.iter()
                    .all(|n| matches!(n, Node2::Text(t, _) if t.trim().is_empty()))
            };
            match defines.get(&name) {
                Some(old) if !(empty(old) && !empty(&body)) => {}
                _ => {
                    defines.insert(name, body);
                }
            }
        }
    }

    let root_name = tree
        .first()
        .map(|t| t.meta.get("name").text())
        .unwrap_or_default();
    let mut env = Env {
        defines: &defines,
        root: V::Nil,
        steps: 0,
        out_bytes: 0,
        made: 0,
        depth: 0,
        nest: HashMap::new(),
    };
    let release = map(BTreeMap::from([
        ("Name".into(), V::Str("release-name".into())),
        ("Namespace".into(), V::Str("default".into())),
        ("Service".into(), V::Str("Helm".into())),
        ("IsInstall".into(), V::Bool(true)),
        ("IsUpgrade".into(), V::Bool(false)),
        ("Revision".into(), V::Int(1)),
    ]));
    let mut stopped = false;
    for inst in &tree {
        let cp = prefix(&inst.dir);
        let chart_v = map([
            "name",
            "version",
            "appVersion",
            "description",
            "type",
            "kubeVersion",
            "home",
            "icon",
            "keywords",
            "sources",
        ]
        .iter()
        .map(|k| (title_key(k), inst.meta.get(k)))
        .collect());
        let mut mine: Vec<&String> = by_owner
            .get(&inst.dir)
            .map(|fs| {
                fs.iter()
                    .copied()
                    .filter(|f| parsed.contains_key(*f))
                    .collect()
            })
            .unwrap_or_default();
        mine.sort();
        for f in mine {
            if stopped {
                chart
                    .unread
                    .push((f.clone(), "chưa dựng: chart đã hết ngân sách dựng".into()));
                continue;
            }
            // Đường dẫn Helm thấy: chart con theo alias.
            let rel = f.strip_prefix(&cp).unwrap_or(f);
            let root = map(BTreeMap::from([
                ("Values".to_string(), inst.values.clone()),
                ("Release".to_string(), release.clone()),
                ("Chart".to_string(), chart_v.clone()),
                (
                    "Template".to_string(),
                    map(BTreeMap::from([
                        (
                            "Name".into(),
                            V::Str(format!("{root_name}/{}{rel}", inst.shown)),
                        ),
                        (
                            "BasePath".into(),
                            V::Str(format!("{root_name}/{}templates", inst.shown)),
                        ),
                    ])),
                ),
            ]));
            env.root = root.clone();
            let mut out = Out::new();
            let mut vars = vec![(String::new(), root.clone())];
            match env.run(&parsed[f], &root, &mut vars, &mut out) {
                Ok(()) => chart.rendered.push(Rendered {
                    file: f.clone(),
                    text: out.text,
                    lines: out.lines,
                }),
                Err(e) => {
                    stopped = e.why.contains("bước dựng")
                        || e.why.contains("dựng ra quá")
                        || e.why.contains("giá trị dựng");
                    chart
                        .unread
                        .push((f.clone(), format!("dòng {}: {}", e.line, e.why)));
                }
            }
        }
    }
    chart
}

/// Một chart trong cây, trước khi xét bật tắt.
struct ChartNode {
    dir: String,
    /// Tiền tố đường dẫn Helm thấy, tính từ chart gốc: `charts/<alias>/`.
    shown: String,
    meta: V,
    values: V,
    /// (mục dependencies, khoá values, giá trị của chart cha trước khi gộp, chart con)
    kids: Vec<(Option<V>, String, V, ChartNode)>,
    /// Mục `dependencies` không có thư mục trong `charts/`.
    missing: Vec<V>,
}

/// Một chart bật, sẵn để dựng.
struct Inst {
    dir: String,
    shown: String,
    meta: V,
    values: V,
}

fn prefix(dir: &str) -> String {
    if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    }
}

/// Mọi thư mục chart (có `Chart.yaml`) trong danh sách file.
fn all_charts(files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|f| f.rsplit('/').next() == Some("Chart.yaml"))
        .map(|f| {
            f.rsplit_once('/')
                .map_or(String::new(), |(d, _)| d.to_string())
        })
        .collect()
}

/// Đọc một file YAML thành giá trị. `optional`: không có thì thôi.
fn load_yaml(
    path: &str,
    read: &dyn Fn(&str) -> Result<String, String>,
    chart: &mut Chart,
    optional: bool,
) -> Option<V> {
    let text = match read(path) {
        Ok(t) => t,
        Err(why) if why == "không có" && optional => return None,
        Err(why) => {
            chart.unread.push((path.to_string(), why));
            return None;
        }
    };
    match yaml::parse(&text) {
        Ok(y) => {
            let mut budget = MAX_VALUES;
            let v = y
                .documents()
                .next()
                .map(|n| from_yaml(n, &mut budget))
                .unwrap_or(V::Nil);
            if budget == 0 {
                chart
                    .unread
                    .push((path.to_string(), format!("quá {MAX_VALUES} giá trị, hay lồng quá {MAX_VALUE_DEPTH} tầng")));
                return None;
            }
            Some(if v == V::Nil { map(BTreeMap::new()) } else { v })
        }
        Err(e) => {
            chart
                .unread
                .push((path.to_string(), format!("YAML dòng {}", e.line)));
            None
        }
    }
}

/// Dựng cây chart ở `dir`: mỗi mục `dependencies` (kể cả cùng chart dưới
/// nhiều alias) là một chart con; values mặc định của chart con được gộp lên
/// chart cha dưới khoá alias, giá trị của chart cha thắng (`CoalesceValues`).
#[allow(clippy::too_many_arguments)]
fn build(
    dir: &str,
    shown: String,
    meta: V,
    mut values: V,
    files: &[String],
    read: &dyn Fn(&str) -> Result<String, String>,
    chart: &mut Chart,
    depth: usize,
) -> ChartNode {
    let p = prefix(dir);
    let mut subs: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix(&format!("{p}charts/")))
        .filter_map(|r| r.strip_suffix("/Chart.yaml"))
        .filter(|d| !d.contains('/'))
        .map(str::to_string)
        .collect();
    subs.sort();
    let mut node = ChartNode {
        dir: dir.to_string(),
        shown: shown.clone(),
        meta: V::Nil,
        values: V::Nil,
        kids: Vec::new(),
        missing: Vec::new(),
    };
    if depth >= 8 {
        if !subs.is_empty() {
            chart
                .unread
                .push((format!("{p}charts"), "chart con lồng quá 8 tầng".into()));
        }
        node.meta = meta;
        node.values = values;
        return node;
    }
    let metas: Vec<(String, V)> = subs
        .into_iter()
        .filter_map(|sub| {
            let sp = format!("{p}charts/{sub}");
            load_yaml(&format!("{sp}/Chart.yaml"), read, chart, false).map(|m| (sp, m))
        })
        .collect();
    let deps: Vec<V> = match meta.get("dependencies") {
        V::List(d) => d.iter().cloned().collect(),
        _ => Vec::new(),
    };
    // (mục dependencies, thư mục, Chart.yaml)
    let mut insts: Vec<(Option<V>, &String, &V)> = Vec::new();
    for d in &deps {
        let name = d.get("name").text();
        match metas.iter().find(|(_, m)| m.get("name").text() == name) {
            Some((sp, m)) => insts.push((Some(d.clone()), sp, m)),
            None => node.missing.push(d.clone()),
        }
    }
    for (sp, m) in &metas {
        let name = m.get("name").text();
        if !deps.iter().any(|d| d.get("name").text() == name) {
            insts.push((None, sp, m));
        }
    }
    for (dep, sp, sub_meta) in insts {
        let name = sub_meta.get("name").text();
        let key = dep
            .as_ref()
            .map(|d| d.get("alias").text())
            .filter(|a| !a.is_empty())
            .unwrap_or(name.clone());
        let mut sub_values = load_yaml(&format!("{sp}/values.yaml"), read, chart, true)
            .unwrap_or_else(|| map(BTreeMap::new()));
        if let over @ V::Map(_) = values.get(&key) {
            merge(&mut sub_values, over);
        }
        if let g @ V::Map(_) = values.get("global") {
            merge(
                &mut sub_values,
                map(BTreeMap::from([("global".to_string(), g)])),
            );
        }
        // `.Chart.Name` của chart con là alias.
        let mut m = sub_meta.clone();
        if let V::Map(mm) = &mut m {
            Rc::make_mut(mm).insert("name".into(), V::Str(key.clone()));
        }
        let kid = build(
            sp,
            format!("{shown}charts/{key}/"),
            m,
            sub_values,
            files,
            read,
            chart,
            depth + 1,
        );
        let orig = values.get(&key);
        if let V::Map(vm) = &mut values {
            Rc::make_mut(vm).insert(key.clone(), kid.values.clone());
        }
        node.kids.push((dep, key, orig, kid));
    }
    node.meta = meta;
    node.values = values;
    node
}

/// Đưa chart bật vào `tree`, chart cha trước; báo chart con khai mà thiếu
/// (Helm từ chối dựng khi thiếu, kể cả chart con tắt).
/// Trả về values cuối của chart: chart con tắt không góp values mặc định
/// (Helm bỏ chart tắt rồi gộp lại).
fn flatten(node: ChartNode, chart: &mut Chart, tree: &mut Vec<Inst>) -> V {
    let p = prefix(&node.dir);
    for d in &node.missing {
        chart.unread.push((
            format!("{p}charts/{}", d.get("name").text()),
            "chart con khai trong `dependencies` mà không có trong `charts/` (chưa `helm dependency build`)".into(),
        ));
    }
    let at = tree.len();
    tree.push(Inst {
        dir: node.dir,
        shown: node.shown,
        meta: node.meta,
        values: V::Nil,
    });
    let mut values = node.values.clone();
    for (dep, key, orig, kid) in node.kids {
        let v = if enabled(dep.as_ref(), &node.values) {
            flatten(kid, chart, tree)
        } else {
            orig
        };
        if let V::Map(vm) = &mut values {
            let vm = Rc::make_mut(vm);
            if v == V::Nil {
                vm.remove(&key);
            } else {
                vm.insert(key, v);
            }
        }
    }
    tree[at].values = values.clone();
    values
}

/// Chart con có bật không, theo `condition` rồi `tags` của `dependencies`,
/// như Helm: `condition` là danh sách đường dẫn cách nhau bằng dấu phẩy, đường
/// đầu tiên ra bool quyết định; không đường nào ra bool thì xét `tags`.
fn enabled(dep: Option<&V>, values: &V) -> bool {
    let Some(dep) = dep else { return true };
    for path in dep
        .get("condition")
        .text()
        .split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        let mut v = values.clone();
        for k in path.split('.') {
            v = v.get(k);
        }
        if let V::Bool(b) = v {
            return b;
        }
    }
    if let V::List(tags) = dep.get("tags") {
        let set: Vec<bool> = tags
            .iter()
            .filter_map(|t| {
                if let V::Bool(b) = values.get("tags").get(&t.text()) {
                    Some(b)
                } else {
                    None
                }
            })
            .collect();
        if !set.is_empty() {
            return set.iter().any(|b| *b);
        }
    }
    true
}

fn title_key(k: &str) -> String {
    let mut c = k.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Chuỗi alias nối nhau dựng giá trị sâu tuỳ ý trong vài dòng: không tràn
    /// stack, ngân sách cạn để file được báo (claude-security F6).
    #[test]
    fn values_alias_long_sau_co_tran() {
        let mut text = String::from("a0: &a0 [x]\n");
        for i in 1..20_000 {
            text.push_str(&format!("a{i}: &a{i} [*a{}]\n", i - 1));
        }
        let budget = std::thread::Builder::new()
            .stack_size(512 << 10)
            .spawn(move || {
                let y = yaml::parse(&text).expect("đọc được");
                let mut budget = MAX_VALUES;
                let _ = y.documents().next().map(|n| from_yaml(n, &mut budget));
                budget
            })
            .unwrap()
            .join()
            .expect("không tràn stack");
        assert_eq!(budget, 0);
    }

    fn chart(files: &[(&str, &str)]) -> Chart {
        let map: HashMap<String, String> = files
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let names: Vec<String> = map.keys().cloned().collect();
        render("c", &names, &[], &|p: &str| {
            map.get(p).cloned().ok_or_else(|| "không có".to_string())
        })
    }

    const META: (&str, &str) = (
        "c/Chart.yaml",
        "apiVersion: v2\nname: shop\nversion: 1.2.3\n",
    );

    #[test]
    fn dung_template_thuong_gap() {
        let c = chart(&[
            META,
            (
                "c/values.yaml",
                "cart:\n  create: true\n  name: cartservice\n  port: 7070\nredis:\n  create: false\nannotations:\n  a: \"1\"\n  b: x\n",
            ),
            (
                "c/templates/_helpers.tpl",
                "{{- define \"shop.labels\" -}}\napp: {{ .Values.cart.name }}\nchart: {{ $.Chart.Name }}\n{{- end }}",
            ),
            (
                "c/templates/cart.yaml",
                "{{- if .Values.cart.create }}\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {{ .Values.cart.name }}\n  namespace: {{ .Release.Namespace }}\n  labels:\n    {{- include \"shop.labels\" . | nindent 4 }}\n  annotations:\n    {{- toYaml .Values.annotations | nindent 4 }}\nspec:\n  template:\n    spec:\n      containers:\n      - name: server\n        env:\n        {{- if .Values.redis.create }}\n        - name: REDIS_ADDR\n          value: \"redis:6379\"\n        {{- else }}\n        - name: REDIS_ADDR\n          value: {{ printf \"%s:%d\" \"cache\" 6379 | quote }}\n        {{- end }}\n{{- end }}\n",
            ),
        ]);
        assert!(c.unread.is_empty(), "{:?}", c.unread);
        let r = &c.rendered[0];
        assert!(r.text.contains("name: cartservice"), "{}", r.text);
        assert!(
            r.text.contains("app: cartservice\n    chart: shop"),
            "{}",
            r.text
        );
        assert!(r.text.contains("a: \"1\"\n    b: x"), "{}", r.text);
        assert!(r.text.contains("value: \"cache:6379\""), "{}", r.text);
        assert!(!r.text.contains("redis:6379"), "{}", r.text);
        // Dòng dựng ra trỏ về dòng template.
        let at = r
            .text
            .lines()
            .position(|l| l.contains("name: cartservice"))
            .unwrap();
        assert_eq!(r.lines[at], 5);
    }

    #[test]
    fn range_with_bien() {
        let c = chart(&[
            META,
            (
                "c/values.yaml",
                "svcs:\n  - name: a\n    port: 80\n  - name: b\n    port: 81\nopt: {}\n",
            ),
            (
                "c/templates/s.yaml",
                "{{- range $i, $s := .Values.svcs }}\n---\nkind: Service\nmetadata:\n  name: {{ $s.name }}-{{ $i }}\n{{- end }}\n{{- with .Values.opt }}\nnope: 1\n{{- else }}\nyes: {{ $.Chart.Version }}\n{{- end }}\n",
            ),
        ]);
        let t = &c.rendered[0].text;
        assert!(
            t.contains("name: a-0")
                && t.contains("name: b-1")
                && t.contains("yes: 1.2.3")
                && !t.contains("nope"),
            "{t}"
        );
    }

    #[test]
    fn ngoai_tap_con_thi_chua_doc() {
        for (src, why) in [
            ("{{ tpl .Values.x . }}", "tpl"),
            ("{{ .Files.Get \"a\" }}", ".Files"),
            ("{{ lookup \"v1\" \"Secret\" \"\" \"x\" }}", "lookup"),
            ("{{ frobnicate 1 }}", "frobnicate"),
            ("{{ if .Values.a }}", "end"),
        ] {
            let c = chart(&[
                META,
                ("c/values.yaml", "a: 1\nx: y\n"),
                ("c/templates/t.yaml", src),
            ]);
            assert!(c.rendered.is_empty(), "{src}");
            assert!(
                c.unread.iter().any(|(_, w)| w.contains(why)),
                "{src}: {:?}",
                c.unread
            );
        }
    }

    #[test]
    fn template_doi_khang_co_tran() {
        let t = crate::testclock::Clock::start();
        // `include` gọi chính nó.
        let c = chart(&[
            META,
            (
                "c/templates/a.yaml",
                "{{ define \"x\" }}{{ include \"x\" . }}{{ end }}{{ include \"x\" . }}",
            ),
        ]);
        assert!(
            c.unread.iter().any(|(_, w)| w.contains("lồng sâu")),
            "{:?}",
            c.unread
        );
        // Lặp lồng bùng nổ.
        let c = chart(&[
            META,
            ("c/values.yaml", "count: 100000\nbig: 9223372036854775807\n"),
            (
                "c/templates/a.yaml",
                "{{ range int .Values.count }}{{ range int $.Values.count }}x{{ end }}{{ end }}",
            ),
        ]);
        assert!(
            c.unread
                .iter()
                .any(|(_, w)| w.contains("bước") || w.contains("MB")),
            "{:?}",
            c.unread
        );
        // `range` trên số rất lớn không cấp phát trước; giá trị trung gian có trần.
        for src in [
            "{{ range int .Values.big }}x{{ end }}",
            "{{ $s := \"xxxxxxxx\" }}{{ range int .Values.count }}{{ $s = printf \"%s%s\" $s $s }}{{ end }}{{ len $s }}",
        ] {
            let c = chart(&[
                META,
                ("c/values.yaml", "count: 100000\nbig: 9223372036854775807\n"),
                ("c/templates/a.yaml", src),
            ]);
            assert!(!c.unread.is_empty(), "{src}");
        }
        // Khối lồng sâu.
        let deep = "{{ if 1 }}".repeat(10_000) + &"{{ end }}".repeat(10_000);
        let c = chart(&[META, ("c/templates/a.yaml", &deep)]);
        assert!(!c.unread.is_empty());
        assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    }

    #[test]
    fn chart_con_define_va_dieu_kien() {
        let c = chart(&[
            (
                "c/Chart.yaml",
                "apiVersion: v2\nname: shop\nversion: 1\ndependencies:\n- {name: redis}\n- {name: kv, alias: cache, condition: 'cache.enabled,global.on'}\n- {name: extra, tags: [opt]}\n",
            ),
            (
                "c/values.yaml",
                "cache:\n  enabled: false\ntags:\n  opt: false\n",
            ),
            (
                "c/charts/redis/Chart.yaml",
                "apiVersion: v2\nname: redis\nversion: 1\n",
            ),
            ("c/charts/redis/values.yaml", "image: redis:7\n"),
            (
                "c/charts/redis/templates/d.yaml",
                "image: {{ .Values.image }}\n",
            ),
            (
                "c/charts/kv/Chart.yaml",
                "apiVersion: v2\nname: kv\nversion: 1\n",
            ),
            ("c/charts/kv/templates/d.yaml", "kv: 1\n"),
            (
                "c/charts/extra/Chart.yaml",
                "apiVersion: v2\nname: extra\nversion: 1\n",
            ),
            ("c/charts/extra/templates/d.yaml", "extra: 1\n"),
            ("c/templates/_a.tpl", "{{ define \"x\" }}A{{ end }}"),
            ("c/templates/_b.tpl", "{{ define \"x\" }}B{{ end }}"),
            (
                "c/templates/w.tpl",
                "x: {{ include \"x\" . }}\nn: {{ 1000000.0 }}\nq: [{{ quote nil }}]\n",
            ),
            ("c/charts/x.tgz", ""),
        ]);
        let text: Vec<(&str, &str)> = c
            .rendered
            .iter()
            .map(|r| (r.file.as_str(), r.text.as_str()))
            .collect();
        // Chart con không có khoá ở chart cha vẫn giữ values của nó.
        assert!(
            text.contains(&("c/charts/redis/templates/d.yaml", "image: redis:7\n")),
            "{text:?}"
        );
        // `condition` dạng danh sách và `tags` tắt chart con.
        assert!(
            !text
                .iter()
                .any(|(f, _)| f.contains("/kv/") || f.contains("/extra/")),
            "{text:?}"
        );
        // `define` trùng tên: bản của file đứng trước thắng, như Helm.
        assert!(
            text.contains(&("c/templates/w.tpl", "x: A\nn: 1e+06\nq: []\n")),
            "{text:?}"
        );
        assert!(
            c.unread.iter().any(|(f, _)| f == "c/charts/x.tgz"),
            "{:?}",
            c.unread
        );
    }

    #[test]
    fn khong_lo_ten_va_goc() {
        let c = chart(&[
            META,
            ("c/values.yaml", "pw: hunter2\n"),
            (
                "c/templates/a.yaml",
                "{{ include (printf \"pw-%s\" .Values.pw) . }}",
            ),
            (
                "c/templates/b.yaml",
                "{{ $d := dict \"top\" $ }}{{ $d.top.Capabilities.KubeVersion }}",
            ),
        ]);
        assert!(
            c.unread.iter().all(|(_, w)| !w.contains("hunter2")),
            "{:?}",
            c.unread
        );
        assert!(
            c.unread
                .iter()
                .any(|(f, w)| f == "c/templates/b.yaml" && w.contains("Capabilities")),
            "{:?}",
            c.unread
        );
        let mut out = String::new();
        to_yaml(
            &V::List(Rc::new(vec![
                V::List(Rc::new(vec![V::Str("a".into()), V::Str("b".into())])),
                V::List(Rc::new(vec![V::Str("c".into())])),
            ])),
            0,
            &mut out,
        );
        assert_eq!(out, "- - a\n  - b\n- - c\n");
    }

    #[test]
    fn values_lon_khong_cham() {
        let values: String = (0..50_000).map(|i| format!("k{i}: v{i}\n")).collect();
        let tpl: String = (0..20_000)
            .map(|i| format!("{{{{ .Values.k{} }}}}\n", i % 50_000))
            .collect();
        let t = crate::testclock::Clock::start();
        let c = chart(&[
            META,
            ("c/values.yaml", &values),
            ("c/templates/a.yaml", &tpl),
        ]);
        assert_eq!(c.rendered.len(), 1, "{:?}", c.unread);
        assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    }

    #[test]
    fn bool_va_so_theo_yaml() {
        let c = chart(&[
            META,
            ("c/values.yaml", "a: false\nb: \"false\"\nc: 0\nd: \"0\"\n"),
            (
                "c/templates/t.yaml",
                "a: {{ if .Values.a }}y{{ else }}n{{ end }}\nb: {{ if .Values.b }}y{{ else }}n{{ end }}\nc: {{ if .Values.c }}y{{ else }}n{{ end }}\nd: {{ if .Values.d }}y{{ else }}n{{ end }}\n",
            ),
        ]);
        assert_eq!(c.rendered[0].text, "a: n\nb: y\nc: n\nd: y\n");
    }

    fn texts(c: &Chart) -> Vec<(&str, &str)> {
        c.rendered
            .iter()
            .map(|r| (r.file.as_str(), r.text.as_str()))
            .collect()
    }

    #[test]
    fn ngoac_roi_truong_tach_roi() {
        let c = chart(&[
            META,
            (
                "c/values.yaml",
                "l:\n- {name: a}\nratio: 0.7\nmem: 2000000\n",
            ),
            (
                "c/templates/a.yaml",
                "x: {{ default (printf \"y\") .Values.sa }}\ny: {{ (index .Values.l 0).name }}\nz: {{ gt .Values.ratio 0.6 }}\nj: {{ toJson .Values.mem }}\n",
            ),
        ]);
        assert_eq!(
            texts(&c),
            vec![("c/templates/a.yaml", "x: y\ny: a\nz: true\nj: 2000000\n")],
            "{:?}",
            c.unread
        );
    }

    #[test]
    fn gia_tri_long_sau_khong_tran_stack() {
        let c = chart(&[
            META,
            ("c/values.yaml", "count: 300000\n"),
            (
                "c/templates/a.yaml",
                "{{ $d := dict }}{{ range int .Values.count }}{{ $d = list $d }}{{ end }}{{ toYaml $d }}",
            ),
        ]);
        assert!(
            c.unread
                .iter()
                .any(|(f, w)| f == "c/templates/a.yaml" && w.contains("lồng sâu")),
            "{:?}",
            c.unread
        );
    }

    #[test]
    fn chart_con_alias_gop_values_va_thieu() {
        let c = chart(&[
            (
                "c/Chart.yaml",
                "apiVersion: v2\nname: shop\nversion: 1\ndependencies:\n- {name: redis, alias: cache}\n- {name: redis, alias: queue}\n- {name: db, condition: db.enabled}\n- {name: gone}\n- {name: idle, condition: idle.enabled}\n",
            ),
            ("c/values.yaml", "queue:\n  port: 2\n"),
            (
                "c/charts/redis/Chart.yaml",
                "apiVersion: v2\nname: redis\nversion: 1\n",
            ),
            ("c/charts/redis/values.yaml", "port: 1\n"),
            (
                "c/charts/redis/templates/s.yaml",
                "name: {{ .Chart.Name }}-{{ .Values.port }}\n",
            ),
            (
                "c/charts/db/Chart.yaml",
                "apiVersion: v2\nname: db\nversion: 1\n",
            ),
            ("c/charts/db/values.yaml", "enabled: false\n"),
            (
                "c/charts/db/templates/_n.tpl",
                "{{ define \"n\" }}db{{ end }}",
            ),
            ("c/charts/db/templates/d.yaml", "db: 1\n"),
            (
                "c/templates/p.yaml",
                "p: {{ .Values.cache.port }} {{ include \"n\" . }}\n",
            ),
        ]);
        let t = texts(&c);
        // Mỗi alias là một thể hiện; `.Chart.Name` là alias.
        assert!(
            t.contains(&("c/charts/redis/templates/s.yaml", "name: cache-1\n")),
            "{t:?}"
        );
        assert!(
            t.contains(&("c/charts/redis/templates/s.yaml", "name: queue-2\n")),
            "{t:?}"
        );
        // `condition` thấy values mặc định của chart con; chart cha đọc được
        // values mặc định của chart con; define của chart tắt không tồn tại.
        assert!(!t.iter().any(|(f, _)| f.contains("/db/")), "{t:?}");
        assert!(
            c.unread
                .iter()
                .any(|(f, w)| f == "c/templates/p.yaml" && w.contains("không có")),
            "{:?} {t:?}",
            c.unread
        );
        // Khai mà thiếu thư mục: báo, kể cả khi tắt (Helm từ chối dựng).
        assert!(
            c.unread.iter().any(|(f, _)| f == "c/charts/gone"),
            "{:?}",
            c.unread
        );
        assert!(
            c.unread.iter().any(|(f, _)| f == "c/charts/idle"),
            "{:?}",
            c.unread
        );
    }
}
