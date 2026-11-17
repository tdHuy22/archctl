//! Sửa `.arch/system.arch` từ giao diện `archctl serve`.
//!
//! Ý ban đầu của công cụ là **người vẽ sơ đồ, agent viết mã theo**. Trước
//! đây sơ đồ chỉ xem được: thêm một component hay một mũi tên vẫn phải mở
//! file luật ra gõ tay. Module này là phần "vẽ".
//!
//! Mọi thao tác đi cùng một khuôn:
//!
//! 1. Định vị khai báo theo **số dòng kèm chữ của dòng đó** (`line` +
//!    `expect`). Lệch thì từ chối: file đã đổi từ lúc trình duyệt tải, sửa
//!    theo số dòng cũ là ghi đè nhầm dòng khác.
//! 2. Tính bản mới bằng thao tác trên từng dòng. Dòng không đụng tới giữ
//!    nguyên từng byte, kể cả chú thích và `\r\n`.
//! 3. Chạy lại bộ đọc luật (`arch_core::load`) trên bản mới. Còn lỗi nào thì
//!    không trả bản mới: thà không làm gì còn hơn làm hỏng file luật, hay để
//!    một `allow` trỏ vào component đã xoá.
//!
//! Module không đọc ghi file: nhận chuỗi, trả chuỗi. `serve.rs` lo phần ghi
//! nguyên tử và nhật ký quyết định.

use arch_core::jsonc::Value;
use std::ops::Range;

/// Kết quả của một thao tác thành công.
#[derive(Debug)]
pub struct Edit {
    /// Toàn bộ file luật mới.
    pub text: String,
    /// Một dòng mô tả cho nhật ký quyết định.
    pub summary: String,
    /// Các dòng bị bỏ hay bị sửa, và các dòng mới, để nhật ký ghi "trước → sau".
    pub before: Vec<String>,
    pub after: Vec<String>,
    /// Đổi tên thì khoá vị trí trong `layout.json` phải đổi theo, không thì
    /// hộp nhảy về chỗ tự xếp.
    pub moved: Option<(String, String)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Yêu cầu sai hay bản mới làm hỏng file luật.
    Bad,
    /// File đã đổi từ lúc trình duyệt tải.
    Stale,
    /// Đụng tới luật `#locked` mà chưa xác nhận.
    Locked,
    /// Xoá một tên còn được chỗ khác nhắc tới; gửi lại với `cascade` để xoá kèm.
    Referenced,
}

#[derive(Debug)]
pub struct Refused {
    pub why: Why,
    pub msg: String,
    /// Những dòng gây ra việc từ chối, dạng `dòng N: …`.
    pub refs: Vec<String>,
}

fn bad(msg: impl Into<String>) -> Refused {
    Refused { why: Why::Bad, msg: msg.into(), refs: Vec::new() }
}

fn locked(refs: Vec<String>) -> Refused {
    Refused { why: Why::Locked, msg: "thao tác này đụng tới luật đã khoá (#locked)".into(), refs }
}

const KEYWORDS: &[&str] = &[
    "component", "external", "store", "allow", "deny", "flow", "contract", "boundary", "invariant", "view", "api",
    "verify", "service", "link",
];

// ---------- dòng của file ----------

#[derive(Debug, Clone)]
struct Row {
    text: String,
    /// `\n`, `\r\n`, hay rỗng ở dòng cuối không có xuống dòng.
    end: String,
    gone: bool,
}

#[derive(Clone)]
struct Rows {
    rows: Vec<Row>,
    eol: String,
    bom: bool,
}

impl Rows {
    /// Cắt giống `str::lines` mà bộ đọc luật dùng, để số dòng khớp nhau.
    fn new(src: &str) -> Rows {
        let (bom, body) = match src.strip_prefix('\u{feff}') {
            Some(rest) => (true, rest),
            None => (false, src),
        };
        let rows: Vec<Row> = body
            .split_inclusive('\n')
            .map(|piece| {
                let (text, end) = match piece.strip_suffix("\r\n") {
                    Some(t) => (t, "\r\n"),
                    None => match piece.strip_suffix('\n') {
                        Some(t) => (t, "\n"),
                        None => (piece, ""),
                    },
                };
                Row { text: text.to_string(), end: end.to_string(), gone: false }
            })
            .collect();
        let eol = rows.iter().find(|r| !r.end.is_empty()).map_or_else(|| "\n".to_string(), |r| r.end.clone());
        Rows { rows, eol, bom }
    }

    fn render(&self) -> String {
        let mut out = String::new();
        if self.bom {
            out.push('\u{feff}');
        }
        for r in self.rows.iter().filter(|r| !r.gone) {
            out.push_str(&r.text);
            out.push_str(&r.end);
        }
        out
    }

    fn row(&self, text: &str) -> Row {
        Row { text: text.to_string(), end: self.eol.clone(), gone: false }
    }

    /// Chèn các dòng mới vào trước vị trí `at`.
    fn insert(&mut self, at: usize, lines: &[String]) {
        // Chèn sau dòng cuối không có xuống dòng thì dòng đó phải có.
        if at > 0
            && let Some(prev) = self.rows.get_mut(at - 1)
            && prev.end.is_empty()
        {
            prev.end = self.eol.clone();
        }
        let new: Vec<Row> = lines.iter().map(|l| self.row(l)).collect();
        self.rows.splice(at..at, new);
    }

    fn remove(&mut self, d: &Decl) {
        for i in d.span() {
            self.rows[i].gone = true;
        }
    }

    fn lines(&self, d: &Decl) -> Vec<String> {
        self.rows[d.span()].iter().map(|r| r.text.clone()).collect()
    }
}

/// Một khai báo cấp cao nhất và các dòng thân của nó.
#[derive(Debug, Clone)]
struct Decl {
    /// Chỉ số dòng đầu (từ 0) và dòng thân cuối cùng, kể cả.
    start: usize,
    end: usize,
    keyword: String,
}

impl Decl {
    fn span(&self) -> Range<usize> {
        self.start..self.end + 1
    }
}

fn blank_or_comment(text: &str) -> bool {
    let t = text.trim();
    t.is_empty() || t.starts_with('#')
}

/// Mọi khai báo, theo cùng quy tắc với `collect_lines`/`take_block` của bộ
/// đọc: dòng trống và chú thích bị bỏ qua, thân khối là các dòng lùi vào
/// liền sau.
fn decls(rows: &Rows) -> Vec<Decl> {
    let mut out: Vec<Decl> = Vec::new();
    for (i, r) in rows.rows.iter().enumerate() {
        if r.gone || blank_or_comment(&r.text) {
            continue;
        }
        let indented = r.text.starts_with(char::is_whitespace);
        match out.last_mut() {
            Some(d) if indented => d.end = i,
            _ if indented => {}
            _ => out.push(Decl {
                start: i,
                end: i,
                keyword: r.text.split_whitespace().next().unwrap_or("").to_string(),
            }),
        }
    }
    out
}

/// Một khai báo cho giao diện: số dòng, từ khoá, toàn bộ chữ, đã khoá chưa.
pub struct Declaration {
    pub line: usize,
    pub keyword: String,
    pub text: String,
    pub locked: bool,
}

pub fn declarations(src: &str) -> Vec<Declaration> {
    let rows = Rows::new(src);
    decls(&rows)
        .into_iter()
        .map(|d| Declaration {
            line: d.start + 1,
            locked: is_locked(&rows.rows[d.start].text),
            text: rows.lines(&d).join("\n"),
            keyword: d.keyword,
        })
        .collect()
}

// ---------- chú thích cuối dòng và danh sách ----------

fn inside_quotes(line: &str, at: usize) -> bool {
    let mut open = false;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if i >= at {
            break;
        }
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => open = !open,
            _ => {}
        }
    }
    open
}

/// Vị trí kết thúc phần nội dung, trước các chú thích `@owner #status` —
/// cùng quy tắc với `split_meta` của bộ đọc.
fn body_end(line: &str) -> usize {
    let mut tokens = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(b)) => {
                tokens.push((b, &line[b..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(b) = start {
        tokens.push((b, &line[b..]));
    }
    let mut cut = line.len();
    for (offset, token) in tokens.into_iter().rev() {
        if (token.starts_with('@') || token.starts_with('#')) && !inside_quotes(line, offset) {
            cut = offset;
        } else {
            break;
        }
    }
    line[..cut].trim_end().len()
}

fn is_locked(line: &str) -> bool {
    line[body_end(line)..].split_whitespace().any(|t| t == "#locked")
}

/// Thu hẹp một khoảng về phần không phải khoảng trắng.
fn trimmed(text: &str, range: Range<usize>) -> Range<usize> {
    let seg = &text[range.clone()];
    let start = range.start + (seg.len() - seg.trim_start().len());
    start..start + seg.trim().len()
}

/// Khoảng chứa danh sách đích của một dòng `allow`/`deny`/`link`, và danh
/// sách `except` nếu có.
fn rule_lists(text: &str) -> Option<(Range<usize>, Option<Range<usize>>)> {
    let end = body_end(text);
    let arrow = text[..end].find("->")? + 2;
    match text[arrow..end].find(" except ") {
        Some(e) => {
            let e = arrow + e;
            Some((trimmed(text, arrow..e), Some(trimmed(text, e + " except ".len()..end))))
        }
        None => Some((trimmed(text, arrow..end), None)),
    }
}

/// Khoảng chứa tên nguồn của `allow`/`deny`/`link`.
fn rule_from(text: &str) -> Option<Range<usize>> {
    let kw = text.split_whitespace().next()?.len();
    let arrow = text[..body_end(text)].find("->")?;
    (arrow >= kw).then(|| trimmed(text, kw..arrow))
}

/// Danh sách sau một dấu phân cách, tới hết phần nội dung (hay tới `until`).
fn list_after(text: &str, marker: &str, until: Option<&str>) -> Option<Range<usize>> {
    let end = body_end(text);
    let start = text[..end].find(marker)? + marker.len();
    let stop = until.and_then(|u| text[start..end].rfind(u).map(|p| start + p)).unwrap_or(end);
    Some(trimmed(text, start..stop))
}

fn items(seg: &str) -> Vec<String> {
    seg.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// Bỏ `name` khỏi danh sách ở khoảng `range`. Trả về dòng mới và số mục còn lại.
/// Danh sách không có `name` thì dòng giữ nguyên từng byte: viết lại `A,B`
/// thành `A, B` là đụng vào một dòng người không hề sửa, và nếu dòng đó
/// `#locked` thì còn bắt người xác nhận mở khoá vô cớ (review sonnet).
fn drop_item(text: &str, range: Range<usize>, name: &str) -> (String, usize) {
    let all = items(&text[range.clone()]);
    if !all.iter().any(|i| i == name) {
        return (text.to_string(), all.len());
    }
    let kept: Vec<String> = all.into_iter().filter(|i| i != name).collect();
    (format!("{}{}{}", &text[..range.start], kept.join(", "), &text[range.end..]), kept.len())
}

/// Bỏ cả đoạn `<marker><danh sách>` khi danh sách đã rỗng, giữ chú thích cuối dòng.
fn drop_clause(text: &str, marker: &str) -> String {
    // Danh sách rỗng thì phần nội dung kết thúc ngay sau từ khoá (`… except`).
    let end = body_end(text);
    match text[..end].rfind(marker.trim_end()) {
        Some(cut) => {
            let tail = text[end..].trim_start();
            if tail.is_empty() { text[..cut].to_string() } else { format!("{}  {tail}", &text[..cut]) }
        }
        None => text.to_string(),
    }
}

/// Gỡ `name` khỏi một dòng `allow`/`deny`/`link`. `None` nghĩa là xoá cả
/// dòng: luật xuất phát từ tên đó, hay không còn đích nào.
fn without_in_rule(text: &str, name: &str) -> Option<String> {
    if rule_from(text).is_some_and(|r| text[r] == *name) {
        return None;
    }
    let mut t = text.to_string();
    if let Some((_, Some(except))) = rule_lists(&t) {
        let (n, left) = drop_item(&t, except, name);
        t = if left == 0 { drop_clause(&n, " except ") } else { n };
    }
    let (to, _) = rule_lists(&t)?;
    let (n, left) = drop_item(&t, to, name);
    (left > 0).then_some(n)
}

/// Gỡ một component khỏi `service ... runs`. Hết thì bỏ cả đoạn `runs`.
fn without_in_runs(text: &str, name: &str) -> Option<String> {
    let range = list_after(text, " runs ", None)?;
    let (n, left) = drop_item(text, range, name);
    Some(if left == 0 { drop_clause(&n, " runs ") } else { n })
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Thay mọi tên trọn vẹn `old` bằng `new` trong đoạn chữ, bỏ qua chuỗi trong
/// nháy kép. `->` không phải một phần của tên, để `A->B` vẫn tách được.
fn rename_idents(seg: &str, old: &str, new: &str) -> String {
    let chars: Vec<(usize, char)> = seg.char_indices().collect();
    let in_name = |j: usize| {
        let c = chars[j].1;
        ident_char(c) && !(c == '-' && chars.get(j + 1).map(|p| p.1) == Some('>'))
    };
    let mut out = String::with_capacity(seg.len());
    let mut quoted = false;
    let mut i = 0;
    while i < chars.len() {
        let (at, c) = chars[i];
        if c == '"' {
            quoted = !quoted;
        }
        if quoted || !in_name(i) {
            out.push(c);
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && in_name(j) {
            j += 1;
        }
        let word = &seg[at..chars.get(j).map_or(seg.len(), |p| p.0)];
        out.push_str(if word == old { new } else { word });
        i = j;
    }
    out
}

/// Áp `rename_idents` lên một khoảng của dòng.
fn rename_in(text: &str, range: Option<Range<usize>>, old: &str, new: &str) -> String {
    match range {
        Some(r) => format!("{}{}{}", &text[..r.start], rename_idents(&text[r.clone()], old, new), &text[r.end..]),
        None => text.to_string(),
    }
}

// ---------- kiểm bản mới ----------

/// Lỗi của bộ đọc luật, dạng `dòng N: …`.
fn errors(src: &str) -> Vec<String> {
    arch_core::load(src)
        .1
        .iter()
        .filter(|d| d.severity == arch_core::Severity::Error)
        .map(|d| format!("dòng {}: {}", d.line, d.msg))
        .collect()
}

fn finish(rows: &Rows, summary: String, before: Vec<String>, after: Vec<String>) -> Result<Edit, Refused> {
    let text = rows.render();
    let errs = errors(&text);
    if !errs.is_empty() {
        return Err(Refused { why: Why::Bad, msg: "sửa xong thì file luật hỏng, không ghi gì cả".into(), refs: errs });
    }
    Ok(Edit { text, summary, before, after, moved: None })
}

/// Chữ người gõ cho một khai báo: đúng một khai báo, dòng đầu không lùi vào,
/// không ký tự điều khiển. Trả về các dòng.
fn declaration_text(text: &str) -> Result<Vec<String>, Refused> {
    if text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err(bad("chữ khai báo có ký tự điều khiển"));
    }
    let mut lines: Vec<String> = text.split('\n').map(|l| l.trim_end().to_string()).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    while lines.first().is_some_and(String::is_empty) {
        lines.remove(0);
    }
    let Some(first) = lines.first() else {
        return Err(bad("chữ khai báo rỗng"));
    };
    if first.starts_with(char::is_whitespace) || first.starts_with('#') {
        return Err(bad("dòng đầu phải là một khai báo, không lùi vào, không phải chú thích"));
    }
    let keyword = first.split_whitespace().next().unwrap_or("");
    if !KEYWORDS.contains(&keyword) {
        return Err(bad(format!("từ khoá không nhận ra: `{keyword}`")));
    }
    if let Some(extra) = lines.iter().skip(1).find(|l| !l.is_empty() && !l.starts_with(char::is_whitespace)) {
        return Err(bad(format!("mỗi lần chỉ một khai báo; dòng thừa: `{extra}`")));
    }
    Ok(lines)
}

/// Nhóm từ khoá đứng cạnh nhau trong file, để khai báo mới chen đúng chỗ.
fn group(keyword: &str) -> &str {
    match keyword {
        "external" => "component",
        "deny" => "allow",
        k => k,
    }
}

// ---------- thao tác ----------

fn text_field<'a>(req: &'a Value, key: &str) -> Option<&'a str> {
    req.get(key).and_then(Value::as_str)
}

fn flag(req: &Value, key: &str) -> bool {
    matches!(req.get(key), Some(Value::Bool(true)))
}

/// Khai báo ở dòng `line`, sau khi đối chiếu `expect` với **toàn bộ** chữ hiện
/// có của nó. Chỉ so dòng đầu thì một khối `flow` đổi thân mà giữ dòng đầu vẫn
/// lọt, và bản sửa từ tab cũ nuốt mất bước người vừa thêm (review sonnet).
fn target(rows: &Rows, req: &Value) -> Result<Decl, Refused> {
    let line: usize = text_field(req, "line")
        .and_then(|l| l.trim().parse().ok())
        .ok_or_else(|| bad("thiếu `line`"))?;
    let expect = text_field(req, "expect").ok_or_else(|| bad("thiếu `expect`"))?;
    let decl = decls(rows).into_iter().find(|d| d.start + 1 == line);
    let current = match &decl {
        Some(d) => Some(rows.lines(d).join("\n")),
        None => line.checked_sub(1).and_then(|i| rows.rows.get(i)).map(|r| r.text.clone()),
    };
    if current.as_deref() != Some(expect) {
        return Err(Refused {
            why: Why::Stale,
            msg: "file luật đã đổi từ lúc tải sơ đồ — tải lại rồi sửa".into(),
            refs: current.map(|c| vec![format!("dòng {line}: {c}")]).unwrap_or_default(),
        });
    }
    decl.ok_or_else(|| bad(format!("dòng {line} không phải một khai báo")))
}

/// Có tên trọn vẹn `name` trong đoạn chữ không (ngoài chuỗi nháy).
fn mentions_ident(seg: &str, name: &str) -> bool {
    rename_idents(seg, name, "\u{0}") != seg
}

/// Chỗ nhắc tới `name` mà bộ đọc luật không báo lỗi, nên xoá xong nó thành mồ
/// côi lặng lẽ: bước của `flow` và `retry X.y` của `boundary` (component),
/// `verify` của `invariant`, `contract`, `boundary` (cách kiểm). Không gỡ tự
/// động được, nên chỉ liệt kê để từ chối (review sonnet).
fn silent_mentions(rows: &Rows, kind: &str, name: &str) -> Vec<String> {
    let component = matches!(kind, "component" | "external");
    let mut out = Vec::new();
    for d in decls(rows) {
        for i in d.span() {
            let text = &rows.rows[i].text;
            let header = i == d.start;
            let body = text.trim_start();
            let hit = match (d.keyword.as_str(), header) {
                ("flow", false) if component => mentions_ident(text, name),
                ("boundary", false) if component && body.starts_with("retry") => mentions_ident(text, name),
                ("invariant", true) if kind == "verify" => {
                    let end = body_end(text);
                    text[..end].rfind(" verify ").is_some_and(|p| mentions_ident(&text[p + " verify ".len()..end], name))
                }
                ("contract" | "boundary", false) if kind == "verify" && body.starts_with("verify") => {
                    mentions_ident(&body["verify".len()..], name)
                }
                _ => false,
            };
            if hit {
                out.push(format!("dòng {}: {text}", i + 1));
            }
        }
    }
    out
}

/// Từ chối khi một khai báo `#locked` bị đổi mà người chưa xác nhận. Chỉ dùng
/// khi `after` giữ nguyên chỉ số dòng của `before` (dòng bỏ thì đánh `gone`).
fn guard_locked(before: &Rows, after: &Rows, confirmed: bool) -> Result<(), Refused> {
    if confirmed {
        return Ok(());
    }
    let hit: Vec<String> = decls(before)
        .into_iter()
        .filter(|d| is_locked(&before.rows[d.start].text))
        .filter(|d| d.span().any(|i| after.rows[i].gone || after.rows[i].text != before.rows[i].text))
        .map(|d| format!("dòng {}: {}", d.start + 1, before.rows[d.start].text))
        .collect();
    if hit.is_empty() { Ok(()) } else { Err(locked(hit)) }
}

/// Các dòng đã đổi giữa hai bản cùng chỉ số: bản cũ và bản mới.
fn changes(before: &Rows, after: &Rows) -> (Vec<String>, Vec<String>) {
    let mut old = Vec::new();
    let mut new = Vec::new();
    for (was, now) in before.rows.iter().zip(&after.rows) {
        if now.gone {
            old.push(was.text.clone());
        } else if now.text != was.text {
            old.push(was.text.clone());
            new.push(now.text.clone());
        }
    }
    (old, new)
}

pub fn apply(src: &str, req: &Value) -> Result<Edit, Refused> {
    let rows = Rows::new(src);
    let confirmed = flag(req, "confirmLocked");
    match text_field(req, "op").unwrap_or("") {
        "add" => add(rows, text_field(req, "text").unwrap_or("")),
        "replace" => {
            let d = target(&rows, req)?;
            let new = declaration_text(text_field(req, "text").unwrap_or(""))?;
            replace(rows, &d, new, confirmed)
        }
        "delete" => {
            let d = target(&rows, req)?;
            delete(rows, &d, flag(req, "cascade"), confirmed)
        }
        "rename" => {
            let d = target(&rows, req)?;
            rename(rows, &d, text_field(req, "to").unwrap_or("").trim(), confirmed)
        }
        "connect" => connect(
            rows,
            text_field(req, "kind").unwrap_or(""),
            text_field(req, "from").unwrap_or(""),
            text_field(req, "to").unwrap_or(""),
        ),
        "disconnect" => {
            let d = target(&rows, req)?;
            disconnect(rows, &d, text_field(req, "to").unwrap_or("").trim(), confirmed)
        }
        other => Err(bad(format!("thao tác không có: `{other}`"))),
    }
}

/// Thêm một khai báo, ngay sau khai báo cuối cùng cùng nhóm (component cạnh
/// component, luật cạnh luật), không thì ở cuối file.
fn add(mut rows: Rows, text: &str) -> Result<Edit, Refused> {
    let new = declaration_text(text)?;
    let keyword = new[0].split_whitespace().next().unwrap_or("").to_string();
    let summary = format!("thêm: {}", new[0]);
    match decls(&rows).iter().rev().find(|d| group(&d.keyword) == group(&keyword)) {
        Some(d) => rows.insert(d.end + 1, &new),
        None => {
            // Cuối file, cách khối trước một dòng trống cho dễ đọc.
            let mut lines = new.clone();
            if rows.rows.last().is_some_and(|r| !r.text.trim().is_empty()) {
                lines.insert(0, String::new());
            }
            let n = rows.rows.len();
            rows.insert(n, &lines);
        }
    }
    finish(&rows, summary, Vec::new(), new)
}

fn replace(mut rows: Rows, d: &Decl, new: Vec<String>, confirmed: bool) -> Result<Edit, Refused> {
    let old = rows.lines(d);
    if old == new {
        return Err(bad("không có gì thay đổi"));
    }
    if !confirmed && is_locked(&old[0]) {
        return Err(locked(vec![format!("dòng {}: {}", d.start + 1, old[0])]));
    }
    // Dòng cuối giữ kiểu xuống dòng của dòng cuối cũ (rỗng nếu là cuối file).
    let last_end = rows.rows[d.end].end.clone();
    let mut fresh: Vec<Row> = new.iter().map(|l| rows.row(l)).collect();
    if let Some(last) = fresh.last_mut() {
        last.end = last_end;
    }
    rows.rows.splice(d.span(), fresh);
    finish(&rows, format!("sửa dòng {}: {}", d.start + 1, old[0]), old, new)
}

/// Tên mà khai báo đặt ra và vị trí của nó trong dòng đầu.
fn declared_name(rows: &Rows, d: &Decl) -> Option<(String, Range<usize>)> {
    let text = &rows.rows[d.start].text;
    let rest = &text[d.keyword.len()..];
    let start = d.keyword.len() + (rest.len() - rest.trim_start().len());
    let len: usize = text[start..].chars().take_while(|c| ident_char(*c)).map(char::len_utf8).sum();
    (len > 0).then(|| (text[start..start + len].to_string(), start..start + len))
}

/// Xoá một khai báo. Tên của nó còn được nhắc ở chỗ khác thì từ chối và liệt
/// kê chỗ nhắc; gửi lại với `cascade` thì gỡ tên khỏi các danh sách và xoá
/// luật chỉ còn trỏ tới nó. Chỗ không gỡ tự động được (bước của flow,
/// contract, phạm vi invariant, store thuộc component đó) vẫn bị từ chối.
fn delete(rows: Rows, d: &Decl, cascade: bool, confirmed: bool) -> Result<Edit, Refused> {
    let original = rows.clone();
    let mut rows = rows;
    rows.remove(d);
    let first = original.rows[d.start].text.clone();
    let name = declared_name(&original, d).map(|n| n.0);
    let silent = name.as_deref().map(|n| silent_mentions(&rows, &d.keyword, n)).unwrap_or_default();
    let errs = errors(&rows.render());
    if !errs.is_empty() || !silent.is_empty() {
        if !cascade || name.is_none() {
            let what = name.unwrap_or_else(|| first.clone());
            let refs = errs.into_iter().chain(silent).collect();
            return Err(Refused { why: Why::Referenced, msg: format!("`{what}` còn được nhắc ở chỗ khác"), refs });
        }
        let name = name.unwrap_or_default();
        let service = d.keyword == "service";
        for other in decls(&rows) {
            let text = rows.rows[other.start].text.clone();
            // `Some(None)`: xoá cả khai báo; `Some(Some(t))`: thay dòng đầu.
            let fix: Option<Option<String>> = match (other.keyword.as_str(), service) {
                ("allow" | "deny", false) | ("link", true) => Some(without_in_rule(&text, &name)),
                ("service", false) => without_in_runs(&text, &name).map(Some),
                ("view", false) => list_after(&text, ":", None).map(|r| {
                    let (t, left) = drop_item(&text, r, &name);
                    (left > 0).then_some(t)
                }),
                ("boundary", false) => list_after(&text, " contains ", Some(":")).map(|r| Some(drop_item(&text, r, &name).0)),
                _ => None,
            };
            match fix {
                Some(Some(t)) => rows.rows[other.start].text = t,
                Some(None) => rows.remove(&other),
                None => {}
            }
        }
        let left: Vec<String> = errors(&rows.render()).into_iter().chain(silent_mentions(&rows, &d.keyword, &name)).collect();
        if !left.is_empty() {
            return Err(Refused {
                why: Why::Referenced,
                msg: format!("không gỡ tự động được mọi chỗ nhắc tới `{name}` — sửa các dòng này trước"),
                refs: left,
            });
        }
    }
    guard_locked(&original, &rows, confirmed)?;
    let (before, after) = changes(&original, &rows);
    finish(&rows, format!("xoá dòng {}: {first}", d.start + 1), before, after)
}

/// Đổi tên khai báo và mọi chỗ nhắc tới nó, đúng theo không gian tên:
/// component đổi thì `service ... runs` đổi theo, còn `link` (nối service) thì không.
fn rename(rows: Rows, d: &Decl, new: &str, confirmed: bool) -> Result<Edit, Refused> {
    if new.is_empty() || !new.chars().all(ident_char) {
        return Err(bad(format!("tên mới không hợp lệ: `{new}` — chỉ gồm chữ, số, gạch dưới và gạch nối")));
    }
    let kind = d.keyword.clone();
    let kind = kind.as_str();
    if !matches!(kind, "component" | "external" | "store" | "api" | "service" | "view" | "boundary" | "flow" | "verify") {
        return Err(bad(format!("`{kind}` không có tên để đổi — dùng Sửa")));
    }
    let Some((old, range)) = declared_name(&rows, d) else {
        return Err(bad("không đọc được tên của khai báo"));
    };
    if old == new {
        return Err(bad("tên mới trùng tên cũ"));
    }
    let original = rows.clone();
    let mut rows = rows;
    rows.rows[d.start].text.replace_range(range, new);

    let component = matches!(kind, "component" | "external");
    let named = component || matches!(kind, "store" | "api");
    for other in decls(&original) {
        for i in other.span() {
            if i == d.start {
                continue;
            }
            let text = original.rows[i].text.as_str();
            let header = i == other.start;
            let updated = match (other.keyword.as_str(), header) {
                ("allow" | "deny", true) if named => rule_renamed(text, &old, new),
                ("link", true) if kind == "service" => rule_renamed(text, &old, new),
                ("store", true) if component => rename_in(text, list_after(text, " owned_by ", Some(" at ")), &old, new),
                ("service", true) if component => rename_in(text, list_after(text, " runs ", None), &old, new),
                ("view", true) if component => rename_in(text, list_after(text, ":", None), &old, new),
                ("boundary", true) if component => rename_in(text, list_after(text, " contains ", Some(":")), &old, new),
                ("contract", true) if component => {
                    let dot = text.find('.').unwrap_or("contract".len()).max("contract".len());
                    rename_in(text, Some("contract".len()..dot), &old, new)
                }
                ("flow", false) if component => rename_idents(text, &old, new),
                // `retry Payment.charge: 3` trỏ tới contract của component.
                ("boundary", false) if component && text.trim_start().starts_with("retry") => {
                    rename_idents(text, &old, new)
                }
                ("invariant", true) if component || kind == "boundary" => {
                    let scope = text.find('[').zip(text.find(']')).filter(|(a, b)| a < b).map(|(a, b)| a + 1..b);
                    rename_in(text, scope, &old, new)
                }
                ("invariant", true) if kind == "verify" => {
                    let end = body_end(text);
                    rename_in(text, text[..end].rfind(" verify ").map(|p| p + " verify ".len()..end), &old, new)
                }
                ("contract" | "boundary", false) if kind == "verify" && text.trim_start().starts_with("verify") => {
                    let at = text.find("verify").map_or(0, |p| p + "verify".len());
                    rename_in(text, Some(at..text.len()), &old, new)
                }
                _ => continue,
            };
            rows.rows[i].text = updated;
        }
    }
    guard_locked(&original, &rows, confirmed)?;
    let (before, after) = changes(&original, &rows);
    let mut done = finish(&rows, format!("đổi tên {kind} {old} → {new}"), before, after)?;
    let key = |n: &str| if kind == "service" { format!("{}{n}", arch_core::layout::SERVICE_PREFIX) } else { n.to_string() };
    done.moved = Some((key(&old), key(new)));
    Ok(done)
}

/// Đổi tên ở nguồn, danh sách đích và `except` của một dòng luật.
fn rule_renamed(text: &str, old: &str, new: &str) -> String {
    let mut t = rename_in(text, rule_from(text), old, new);
    if let Some((_, Some(e))) = rule_lists(&t) {
        t = rename_in(&t, Some(e), old, new);
    }
    let to = rule_lists(&t).map(|(to, _)| to);
    rename_in(&t, to, old, new)
}

/// Nối mũi tên giữa hai hộp: thêm `allow`/`deny` (component) hay `link`
/// (service, tên dạng `svc:Tên`).
fn connect(rows: Rows, kind: &str, from: &str, to: &str) -> Result<Edit, Refused> {
    let prefix = arch_core::layout::SERVICE_PREFIX;
    let (from, to) = match kind {
        "allow" | "deny" => (from, to),
        "link" => (from.strip_prefix(prefix).unwrap_or(from), to.strip_prefix(prefix).unwrap_or(to)),
        other => return Err(bad(format!("loại mũi tên không có: `{other}`"))),
    };
    for name in [from, to] {
        if name.is_empty() || !name.chars().all(ident_char) {
            return Err(bad(format!("tên không hợp lệ: `{name}`")));
        }
    }
    if from == to {
        return Err(bad("mũi tên phải nối hai hộp khác nhau"));
    }
    for d in decls(&rows).iter().filter(|d| d.keyword == kind) {
        let text = &rows.rows[d.start].text;
        let same_from = rule_from(text).is_some_and(|r| &text[r] == from);
        let has_to = rule_lists(text).is_some_and(|(r, _)| items(&text[r]).iter().any(|t| t == to));
        if same_from && has_to {
            return Err(bad(format!("đã có ở dòng {}: {text}", d.start + 1)));
        }
    }
    add(rows, &format!("{kind} {from} -> {to}  @human #accepted"))
}

/// Gỡ một đích khỏi `allow`/`deny`/`link`, hay một component khỏi `service
/// ... runs`. Luật hết đích thì xoá cả dòng.
fn disconnect(mut rows: Rows, d: &Decl, to: &str, confirmed: bool) -> Result<Edit, Refused> {
    let to = to.strip_prefix(arch_core::layout::SERVICE_PREFIX).unwrap_or(to);
    let text = rows.rows[d.start].text.clone();
    let range = match d.keyword.as_str() {
        "allow" | "deny" | "link" => rule_lists(&text).map(|(r, _)| r),
        "service" => list_after(&text, " runs ", None),
        other => return Err(bad(format!("`{other}` không phải mũi tên"))),
    };
    if !range.is_some_and(|r| items(&text[r]).iter().any(|t| t == to)) {
        return Err(bad(format!("dòng {} không trỏ tới `{to}`", d.start + 1)));
    }
    if !confirmed && is_locked(&text) {
        return Err(locked(vec![format!("dòng {}: {text}", d.start + 1)]));
    }
    let summary = format!("gỡ `{to}` khỏi dòng {}: {text}", d.start + 1);
    let fixed = if d.keyword == "service" { without_in_runs(&text, to) } else { rule_lists(&text).and_then(|(r, _)| {
        let (n, left) = drop_item(&text, r, to);
        (left > 0).then_some(n)
    }) };
    match fixed {
        Some(t) => {
            rows.rows[d.start].text = t.clone();
            finish(&rows, summary, vec![text], vec![t])
        }
        None => {
            let old = rows.lines(d);
            rows.remove(d);
            finish(&rows, summary, old, Vec::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAW: &str = "\
# Kiến trúc mẫu
component Web    at src/web/**    @human #accepted
component Order  at src/order/**  @human #accepted
component Pay    at src/pay/**    @human #locked
store Orders owned_by Order at \"orders\"

allow Web   -> Order, Pay
allow Order -> Pay
deny  Pay   -> Web, Order

service Api from compose:api runs Web, Order
service Db
link Api -> Db

view Shop: Web, Order
";

    fn req(fields: &[(&str, &str)]) -> Value {
        Value::Obj(fields.iter().map(|(k, v)| ((*k).to_string(), Value::Str((*v).to_string()))).collect())
    }

    fn with(mut v: Value, key: &str) -> Value {
        if let Value::Obj(f) = &mut v {
            f.push((key.into(), Value::Bool(true)));
        }
        v
    }

    fn line_of(src: &str, starts: &str) -> (String, String) {
        let (i, l) = src.lines().enumerate().find(|(_, l)| l.starts_with(starts)).expect("có dòng");
        ((i + 1).to_string(), l.to_string())
    }

    /// Yêu cầu nhắm vào khai báo bắt đầu bằng `starts`, `expect` là nguyên văn
    /// cả khối — đúng như giao diện gửi.
    fn at(src: &str, op: &str, starts: &str, extra: &[(&str, &str)]) -> Value {
        let d = declarations(src).into_iter().find(|d| d.text.starts_with(starts)).expect("có khai báo");
        let line = d.line.to_string();
        let mut fields = vec![("op", op), ("line", line.as_str()), ("expect", d.text.as_str())];
        fields.extend_from_slice(extra);
        req(&fields)
    }

    fn run(src: &str, r: &Value) -> Result<Edit, Refused> {
        assert!(errors(src).is_empty(), "luật mẫu phải hợp lệ: {:?}", errors(src));
        apply(src, r)
    }

    /// Các dòng của `a` không còn trong `b`.
    fn missing(a: &str, b: &str) -> Vec<String> {
        a.lines().filter(|l| !b.lines().any(|x| x == *l)).map(str::to_string).collect()
    }

    #[test]
    fn them_component_chen_canh_component_cuoi() {
        let out = run(LAW, &req(&[("op", "add"), ("text", "component Cart at src/cart/**  @human #accepted")])).unwrap();
        let lines: Vec<&str> = out.text.lines().collect();
        let pay = lines.iter().position(|l| l.starts_with("component Pay")).unwrap();
        assert_eq!(lines[pay + 1], "component Cart at src/cart/**  @human #accepted");
        assert!(missing(LAW, &out.text).is_empty(), "không mất dòng nào");
        assert_eq!(out.text.lines().count(), LAW.lines().count() + 1);
    }

    #[test]
    fn them_khai_bao_moi_loai_xuong_cuoi_file() {
        let out = run(LAW, &req(&[("op", "add"), ("text", "verify unit runs \"cargo test\"")])).unwrap();
        assert!(out.text.ends_with("Order\n\nverify unit runs \"cargo test\"\n"), "{}", out.text);
    }

    #[test]
    fn them_khoi_nhieu_dong() {
        let flow = "flow Checkout:\n  Web -> Order -> Pay";
        let out = run(LAW, &req(&[("op", "add"), ("text", flow)])).unwrap();
        assert!(out.text.contains(flow), "{}", out.text);
    }

    #[test]
    fn them_tu_choi_cu_phap_hong_trung_ten_mo_coi() {
        for text in [
            "component",                                // thiếu tên
            "component Web at x/**",                    // trùng tên
            "allow Web -> KhongCo",                     // tham chiếu mồ côi
            "  component Lui at x/**",                  // lùi vào
            "component A at a/**\ncomponent B at b/**", // hai khai báo
            "componet A at a/**",                       // sai từ khoá
            "component A at a/**\r",                    // ký tự điều khiển
            "   ",
        ] {
            let r = run(LAW, &req(&[("op", "add"), ("text", text)]));
            assert_eq!(r.map(|e| e.text).unwrap_err().why, Why::Bad, "phải từ chối: {text:?}");
        }
    }

    #[test]
    fn sua_duong_dan_chi_dung_mot_dong() {
        let r = at(LAW, "replace", "component Web", &[("text", "component Web at app/web/**  @human #accepted")]);
        let out = run(LAW, &r).unwrap();
        let (_, old) = line_of(LAW, "component Web");
        assert_eq!(missing(LAW, &out.text), vec![old.clone()]);
        assert_eq!(out.before, vec![old]);
        assert_eq!(out.text.lines().count(), LAW.lines().count());
        assert!(out.text.contains("component Web at app/web/**  @human #accepted\n"));
    }

    #[test]
    fn sua_khoi_doi_so_dong() {
        let src = format!("{LAW}flow F:\n  Web -> Order\ncomponent Z at z/**");
        let r = at(&src, "replace", "flow F", &[("text", "flow F:\n  Web -> Order\n  Order -> Pay\n")]);
        let out = run(&src, &r).unwrap();
        assert!(out.text.ends_with("flow F:\n  Web -> Order\n  Order -> Pay\ncomponent Z at z/**"), "{}", out.text);
    }

    #[test]
    fn expect_lech_thi_tu_choi() {
        let (line, _) = line_of(LAW, "component Web");
        let r = req(&[("op", "replace"), ("line", &line), ("expect", "component Web at cu/**"), ("text", "component Web at x/**")]);
        assert_eq!(run(LAW, &r).unwrap_err().why, Why::Stale);
        let r = req(&[("op", "delete"), ("line", "999"), ("expect", "x")]);
        assert_eq!(run(LAW, &r).unwrap_err().why, Why::Stale);
        let r = req(&[("op", "delete"), ("line", "0"), ("expect", "x")]);
        assert_eq!(run(LAW, &r).unwrap_err().why, Why::Stale);
        // Dòng chú thích không phải khai báo.
        let r = req(&[("op", "delete"), ("line", "1"), ("expect", "# Kiến trúc mẫu")]);
        assert_eq!(run(LAW, &r).unwrap_err().why, Why::Bad);
    }

    #[test]
    fn luat_khoa_can_xac_nhan() {
        let r = at(LAW, "replace", "component Pay", &[("text", "component Pay at p/**  @human #locked")]);
        assert_eq!(run(LAW, &r).unwrap_err().why, Why::Locked);
        assert!(run(LAW, &with(r, "confirmLocked")).is_ok());
    }

    #[test]
    fn xoa_component_con_duoc_nhac_thi_liet_ke_roi_xoa_kem() {
        let r = at(LAW, "delete", "component Web", &[]);
        let refused = run(LAW, &r).unwrap_err();
        assert_eq!(refused.why, Why::Referenced);
        assert!(refused.refs.iter().any(|x| x.contains("Web")), "{:?}", refused.refs);

        let out = run(LAW, &with(r, "cascade")).unwrap();
        let t = &out.text;
        assert!(!t.contains("component Web"), "{t}");
        assert!(!t.contains("allow Web"), "luật chỉ từ Web phải mất: {t}");
        assert!(t.contains("deny  Pay   -> Order\n"), "Web gỡ khỏi danh sách đích: {t}");
        assert!(t.contains("service Api from compose:api runs Order\n"), "{t}");
        assert!(t.contains("view Shop: Order\n"), "{t}");
        assert!(out.before.iter().any(|l| l.starts_with("allow Web")), "nhật ký ghi dòng bị xoá kèm");
        assert!(out.after.contains(&"view Shop: Order".to_string()), "{:?}", out.after);
    }

    #[test]
    fn xoa_kem_khong_go_duoc_thi_tu_choi() {
        // Store thuộc Order: xoá Order thì store mồ côi, không tự xoá store được.
        let r = with(at(LAW, "delete", "component Order", &[]), "cascade");
        let refused = run(LAW, &r).unwrap_err();
        assert_eq!(refused.why, Why::Referenced);
        assert!(refused.refs.iter().any(|x| x.contains("Orders")), "{:?}", refused.refs);
    }

    #[test]
    fn xoa_kem_dung_luat_khoa_can_xac_nhan() {
        let src = "component Web at w/**\ncomponent Pay at p/**\nallow Web -> Pay  @human #locked\n";
        let r = with(at(src, "delete", "component Pay", &[]), "cascade");
        assert_eq!(run(src, &r).unwrap_err().why, Why::Locked);
        assert_eq!(run(src, &with(r, "confirmLocked")).unwrap().text, "component Web at w/**\n");
    }

    #[test]
    fn xoa_kem_bo_except_rong() {
        let src = "component A at a/**\ncomponent B at b/**\ncomponent T at t/**\ndeny * -> A except T  @human #accepted\n";
        let out = run(src, &with(at(src, "delete", "component T", &[]), "cascade")).unwrap();
        assert!(out.text.ends_with("deny * -> A  @human #accepted\n"), "{}", out.text);
    }

    #[test]
    fn xoa_khoi_nhieu_dong() {
        let src = format!("{LAW}flow F:\n  Web -> Order\n  # chú thích trong khối\n  Order -> Pay\ncomponent Z at z/**\n");
        let out = run(&src, &at(&src, "delete", "flow F", &[])).unwrap();
        assert_eq!(out.text, format!("{LAW}component Z at z/**\n"));
        assert_eq!(out.before.len(), 4);
    }

    #[test]
    fn doi_ten_component_doi_ca_tham_chieu_dung_khong_gian_ten() {
        let src = format!(
            "{LAW}contract Web.render:\n  errors Timeout\nflow F:\n  Web->Order.place -> Web\ninvariant data [Web] luôn đúng\nservice Web\n"
        );
        let out = run(&src, &at(&src, "rename", "component Web", &[("to", "Front")])).unwrap();
        let t = &out.text;
        assert!(t.contains("component Front    at src/web/**    @human #accepted"), "{t}");
        assert!(t.contains("allow Front   -> Order, Pay"), "{t}");
        assert!(t.contains("deny  Pay   -> Front, Order"), "{t}");
        assert!(t.contains("runs Front, Order"), "{t}");
        assert!(t.contains("view Shop: Front, Order"), "{t}");
        assert!(t.contains("contract Front.render:"), "{t}");
        assert!(t.contains("  Front->Order.place -> Front"), "{t}");
        assert!(t.contains("invariant data [Front]"), "{t}");
        assert!(t.contains("\nservice Web\n"), "service khác không gian tên, giữ nguyên: {t}");
        assert_eq!(out.moved, Some(("Web".into(), "Front".into())));
    }

    #[test]
    fn doi_ten_service_doi_link_va_khoa_vi_tri() {
        let out = run(LAW, &at(LAW, "rename", "service Db", &[("to", "Postgres")])).unwrap();
        assert!(out.text.contains("link Api -> Postgres"), "{}", out.text);
        assert_eq!(out.moved, Some(("svc:Db".into(), "svc:Postgres".into())));
    }

    #[test]
    fn doi_ten_trung_hay_sai_bi_tu_choi() {
        for to in ["Order", "a b", "", "Web", "Orders"] {
            assert!(run(LAW, &at(LAW, "rename", "component Web", &[("to", to)])).is_err(), "{to:?}");
        }
        assert!(run(LAW, &at(LAW, "rename", "allow Web", &[("to", "X")])).is_err(), "luật không có tên");
    }

    #[test]
    fn noi_mui_ten_them_allow_canh_luat() {
        let out = run(LAW, &req(&[("op", "connect"), ("kind", "allow"), ("from", "Order"), ("to", "Web")])).unwrap();
        let lines: Vec<&str> = out.text.lines().collect();
        let deny = lines.iter().position(|l| l.starts_with("deny")).unwrap();
        assert_eq!(lines[deny + 1], "allow Order -> Web  @human #accepted");
        // Đã có thì không thêm lần nữa.
        assert!(run(LAW, &req(&[("op", "connect"), ("kind", "allow"), ("from", "Web"), ("to", "Pay")])).is_err());
        // Nối service bằng `link`.
        let out = run(LAW, &req(&[("op", "connect"), ("kind", "link"), ("from", "svc:Db"), ("to", "svc:Api")])).unwrap();
        assert!(out.text.contains("link Api -> Db\nlink Db -> Api  @human #accepted\n"), "{}", out.text);
        // Tên chưa khai báo thì file hỏng, từ chối.
        assert!(run(LAW, &req(&[("op", "connect"), ("kind", "allow"), ("from", "Web"), ("to", "Ma")])).is_err());
        assert!(run(LAW, &req(&[("op", "connect"), ("kind", "verify"), ("from", "Web"), ("to", "Order")])).is_err());
    }

    #[test]
    fn go_mui_ten_bo_mot_dich_het_dich_thi_xoa_dong() {
        let out = run(LAW, &at(LAW, "disconnect", "allow Web", &[("to", "Pay")])).unwrap();
        assert!(out.text.contains("allow Web   -> Order\n"), "{}", out.text);

        let out = run(LAW, &at(LAW, "disconnect", "allow Order", &[("to", "Pay")])).unwrap();
        assert!(!out.text.contains("allow Order"), "{}", out.text);

        let out = run(LAW, &at(LAW, "disconnect", "service Api", &[("to", "Web")])).unwrap();
        assert!(out.text.contains("service Api from compose:api runs Order\n"), "{}", out.text);

        let out = run(LAW, &at(LAW, "disconnect", "link Api", &[("to", "svc:Db")])).unwrap();
        assert!(!out.text.contains("link"), "{}", out.text);

        assert!(run(LAW, &at(LAW, "disconnect", "allow Web", &[("to", "Web")])).is_err());
    }

    #[test]
    fn giu_nguyen_crlf_bom_va_dong_cuoi_khong_xuong_dong() {
        let src = "\u{feff}component A at a/**\r\ncomponent B at b/**\r\nallow A -> B";
        let out = run(src, &req(&[("op", "connect"), ("kind", "deny"), ("from", "B"), ("to", "A")])).unwrap();
        assert_eq!(
            out.text,
            "\u{feff}component A at a/**\r\ncomponent B at b/**\r\nallow A -> B\r\ndeny B -> A  @human #accepted\r\n"
        );
        let out = run(src, &at(src, "replace", "allow A", &[("text", "allow B -> A")])).unwrap();
        assert_eq!(out.text, "\u{feff}component A at a/**\r\ncomponent B at b/**\r\nallow B -> A");
    }

    #[test]
    fn danh_sach_khai_bao_cho_giao_dien() {
        let all = declarations(LAW);
        assert_eq!(all.len(), 11);
        assert_eq!((all[0].line, all[0].keyword.as_str()), (2, "component"));
        assert_eq!(all[0].text, "component Web    at src/web/**    @human #accepted");
        assert!(all.iter().any(|d| d.text.starts_with("component Pay") && d.locked));
    }

    #[test]
    fn xoa_kem_khong_dung_dong_khong_lien_quan() {
        // Dòng không nhắc X giữ nguyên từng byte, kể cả `A->B,C` viết sát và
        // đang khoá: không được bắt người xác nhận mở khoá vô cớ.
        let src = "component X at x/**\ncomponent A at a/**\ncomponent B at b/**\ncomponent C at c/**\n\
                   allow A->B,C  @human #locked\nallow X -> A\nview V: A,B\nservice S runs A,B\n";
        let out = run(src, &with(at(src, "delete", "component X", &[]), "cascade")).unwrap();
        assert_eq!(
            out.text,
            "component A at a/**\ncomponent B at b/**\ncomponent C at c/**\n\
             allow A->B,C  @human #locked\nview V: A,B\nservice S runs A,B\n"
        );
        assert_eq!(out.before, vec!["component X at x/**".to_string(), "allow X -> A".to_string()]);
        assert!(out.after.is_empty(), "{:?}", out.after);
    }

    #[test]
    fn than_khoi_doi_tu_luc_tai_thi_tu_choi() {
        let loaded = "component A at a/**\ncomponent B at b/**\nflow F:\n  A -> B\n";
        let now = "component A at a/**\ncomponent B at b/**\nflow F:\n  A -> B\n  B -> A\n";
        let stale = at(loaded, "replace", "flow F", &[("text", "flow F:\n  B -> A")]);
        assert_eq!(run(now, &stale).unwrap_err().why, Why::Stale);
        let stale = at(loaded, "delete", "flow F", &[]);
        assert_eq!(run(now, &stale).unwrap_err().why, Why::Stale);
    }

    #[test]
    fn xoa_ten_con_trong_flow_hay_verify_thi_tu_choi() {
        // Bộ đọc luật không báo lỗi cho hai chỗ nhắc này, nên phải tự tìm.
        let src = "component X at x/**\ncomponent A at a/**\nflow F:\n  X -> A\n";
        for r in [at(src, "delete", "component X", &[]), with(at(src, "delete", "component X", &[]), "cascade")] {
            let refused = run(src, &r).unwrap_err();
            assert_eq!(refused.why, Why::Referenced);
            assert!(refused.refs.iter().any(|x| x.contains("X -> A")), "{:?}", refused.refs);
        }

        let src = "component A at a/**\nverify v runs \"true\"\ninvariant data [A] luôn đúng verify v\ncontract A.x:\n  verify v\n";
        let refused = run(src, &at(src, "delete", "verify v", &[])).unwrap_err();
        assert_eq!(refused.why, Why::Referenced);
        assert_eq!(refused.refs.len(), 2, "{:?}", refused.refs);
        // Bước flow tên trùng mà không phải tên trọn vẹn thì không tính.
        let src = "component X at x/**\ncomponent A at a/**\nflow F:\n  A -> Xa\n";
        assert!(run(src, &at(src, "delete", "component X", &[])).is_ok());
    }

    #[test]
    fn doi_ten_component_doi_ca_retry_cua_boundary() {
        let src = "component P at p/**\ncontract P.charge:\n  errors E\nboundary Bd contains P:\n  retry P.charge: 3\n";
        let out = run(src, &at(src, "rename", "component P", &[("to", "Q")])).unwrap();
        assert!(out.text.contains("  retry Q.charge: 3\n") && out.text.contains("boundary Bd contains Q:"), "{}", out.text);
    }

    #[test]
    fn doi_ten_trong_chuoi_nhay_khong_dung() {
        assert_eq!(rename_idents("A -> B, \"A\"", "A", "Z"), "Z -> B, \"A\"");
        assert_eq!(rename_idents("A->B", "A", "Z"), "Z->B");
        assert_eq!(rename_idents("AB -> A_b, A-x", "A", "Z"), "AB -> A_b, A-x");
    }
}
