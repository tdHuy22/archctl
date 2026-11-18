//! TM-92: chữ do agent ghi (đề xuất, đường dẫn, tên, lý do) không được chèn
//! HTML hay JS vào giao diện `archctl serve`. Script chạy trong trang có khoá
//! phiên, nên một chỗ chèn chữ thô là đủ để nó tự gọi `/api/edit` sửa luật.
//!
//! Giao diện dựng HTML bằng chuỗi template rồi gán `innerHTML`, nên chốt chặn
//! là: mọi `${...}` rơi vào HTML phải qua `esc()`, hoặc chắc chắn ra số, ra
//! true/false, ra chuỗi viết sẵn trong mã. Test này đọc phần `<script>` của
//! `ui.html` bằng một bộ quét nhỏ (không dependency) và bắt mọi chỗ khác.
//!
//! Bộ quét bảo thủ: thứ nó không hiểu thì coi là không an toàn. Khi nó báo
//! nhầm một chỗ thật sự an toàn, cách sửa là bọc `esc()`, không phải nới luật.

const UI: &str = include_str!("../src/ui.html");

/// Trường của `/api/model` mà máy chủ luôn gửi dạng số JSON (Rust `usize`).
const NUMERIC: &[&str] = &["line", "length", "count", "files", "of"];

/// Chỗ ghi HTML ngoài `innerHTML`, hay chạy chuỗi thành mã. Giao diện không
/// dùng chỗ nào; dùng thì phải thêm luật kiểm cho nó trước.
const SINKS: &[&str] = &[
    "insertAdjacentHTML",
    "outerHTML",
    "document.write",
    "eval(",
    "Function(",
    "srcdoc",
    "createContextualFragment",
    "DOMParser",
    "setTimeout(S",
    "setInterval(S",
];

/// Thuộc tính mà chữ trong đó thành URL hay mã: `esc()` không đủ.
const DANGEROUS_ATTRS: &[&str] = &["href", "src", "action", "formaction", "style", "srcdoc"];

struct Template {
    line: usize,
    has_tag: bool,
    exprs: Vec<Expr>,
}

struct Expr {
    /// Chữ tĩnh của template đứng trước `${`, tính từ đầu template.
    before: String,
    /// Biểu thức đã làm phẳng: template → `T`, chuỗi → `S`, regex → `R`.
    flat: String,
    templates: Vec<Template>,
}

struct Scanner {
    c: Vec<char>,
    i: usize,
    first_line: usize,
}

impl Scanner {
    fn line(&self, at: usize) -> usize {
        self.first_line + self.c[..at].iter().filter(|&&c| c == '\n').count()
    }

    fn peek(&self, k: usize) -> Option<char> {
        self.c.get(self.i + k).copied()
    }

    /// Đọc mã tới hết (hoặc tới `}` đóng `${` nếu `in_expr`), ghi bản phẳng
    /// vào `out`, template gặp được vào `found`.
    fn code(&mut self, in_expr: bool, out: &mut String, found: &mut Vec<Template>) {
        let mut depth = 0usize;
        while let Some(c) = self.peek(0) {
            match c {
                '"' | '\'' => {
                    self.string(c);
                    out.push('S');
                }
                '`' => {
                    found.push(self.template());
                    out.push('T');
                }
                '/' if self.peek(1) == Some('/') => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.i += 1;
                    }
                }
                '/' if self.peek(1) == Some('*') => {
                    self.i += 2;
                    while self.i < self.c.len() && !(self.peek(0) == Some('*') && self.peek(1) == Some('/')) {
                        self.i += 1;
                    }
                    self.i += 2;
                }
                '/' if regex_may_start(out) => {
                    self.regex();
                    out.push('R');
                }
                '{' | '(' | '[' => {
                    depth += 1;
                    out.push(c);
                    self.i += 1;
                }
                '}' if in_expr && depth == 0 => {
                    self.i += 1;
                    return;
                }
                '}' | ')' | ']' => {
                    depth = depth.saturating_sub(1);
                    out.push(c);
                    self.i += 1;
                }
                _ => {
                    out.push(c);
                    self.i += 1;
                }
            }
        }
        assert!(!in_expr, "`${{` không đóng trong ui.html");
    }

    fn string(&mut self, quote: char) {
        self.i += 1;
        while let Some(c) = self.peek(0) {
            self.i += if c == '\\' { 2 } else { 1 };
            if c == quote {
                return;
            }
        }
    }

    fn regex(&mut self) {
        self.i += 1;
        let mut class = false;
        while let Some(c) = self.peek(0) {
            self.i += 1;
            match c {
                '\\' => self.i += 1,
                '[' => class = true,
                ']' => class = false,
                '/' if !class => break,
                _ => {}
            }
        }
        while self.peek(0).is_some_and(|c| c.is_ascii_alphabetic()) {
            self.i += 1;
        }
    }

    fn template(&mut self) -> Template {
        let line = self.line(self.i);
        self.i += 1;
        let mut text = String::new();
        let mut exprs = Vec::new();
        while let Some(c) = self.peek(0) {
            match c {
                '\\' => {
                    text.extend(self.c.get(self.i..self.i + 2).unwrap_or_default());
                    self.i += 2;
                }
                '`' => {
                    self.i += 1;
                    let has_tag = has_tag(&text);
                    return Template { line, has_tag, exprs };
                }
                '$' if self.peek(1) == Some('{') => {
                    self.i += 2;
                    let mut flat = String::new();
                    let mut templates = Vec::new();
                    self.code(true, &mut flat, &mut templates);
                    exprs.push(Expr { before: text.clone(), flat, templates });
                }
                _ => {
                    text.push(c);
                    self.i += 1;
                }
            }
        }
        panic!("template không đóng ở dòng {line} của ui.html");
    }
}

/// `/` ở đây mở regex hay là phép chia: nhìn ký tự có nghĩa đứng trước.
fn regex_may_start(out: &str) -> bool {
    let t = out.trim_end();
    t.ends_with("return") || t.chars().last().is_none_or(|c| "(,=:[!&|?{};+-*%<>~^".contains(c))
}

/// Chữ có thẻ HTML (`<p`, `</p`, `<br`), không tính `<tên>` trong câu chữ.
fn has_tag(text: &str) -> bool {
    let b = text.as_bytes();
    b.windows(2).any(|w| w[0] == b'<' && (w[1].is_ascii_alphabetic() || w[1] == b'/'))
}

/// Quét `<script>` của `src`, trả về các chỗ chèn không an toàn kèm dòng.
fn problems(src: &str) -> Vec<String> {
    scan(src).0
}

/// Như [`problems`], kèm số chỗ `${...}` trong HTML đã kiểm.
fn scan(src: &str) -> (Vec<String>, usize) {
    assert_eq!(src.matches("<script").count(), 1, "ui.html phải có đúng một <script>");
    let start = src.find("<script>").expect("có <script>") + "<script>".len();
    let end = src.rfind("</script>").expect("có </script>");
    let mut s = Scanner { c: src[start..end].chars().collect(), i: 0, first_line: src[..start].lines().count() };
    let mut flat = String::new();
    let mut templates = Vec::new();
    s.code(false, &mut flat, &mut templates);

    let mut bad = Vec::new();
    let mut checked = 0;
    for sink in SINKS {
        if flat.contains(sink) {
            bad.push(format!("dùng `{sink}`"));
        }
    }
    for t in &templates {
        check_template(t, false, &flat, &mut bad, &mut checked);
    }
    // `x.innerHTML = <vế phải>` và `+=`: vế phải cũng chỉ được ghép từ thứ an toàn.
    let mut from = 0;
    while let Some(k) = flat[from..].find(".innerHTML") {
        let at = from + k + ".innerHTML".len();
        let rest = flat[at..].trim_start();
        let rhs = rest.strip_prefix("+=").or_else(|| rest.strip_prefix('=').filter(|r| !r.starts_with('=')));
        if let Some(rhs) = rhs {
            let rhs = statement(rhs);
            if !safe_output(rhs, &flat, 0) {
                bad.push(format!("innerHTML = {}", rhs.trim()));
            }
        }
        from = at;
    }
    (bad, checked)
}

fn check_template(t: &Template, html: bool, flat: &str, bad: &mut Vec<String>, checked: &mut usize) {
    let html = html || t.has_tag;
    for e in &t.exprs {
        let expr = e.flat.trim();
        if html {
            *checked += 1;
            if let Some(attr) = open_attribute(&e.before) {
                bad.push(format!("dòng {}: `${{{expr}}}` nằm trong {attr}", t.line));
            }
        }
        if is_esc(expr) {
            // Template bên trong `esc(...)` ra chữ đã thoát: không cần kiểm như HTML.
            for inner in &e.templates {
                check_template(inner, false, flat, bad, checked);
            }
            continue;
        }
        if html && !safe_output(expr, flat, 0) {
            bad.push(format!("dòng {}: `${{{expr}}}` chèn chữ chưa qua esc()", t.line));
        }
        // Template lồng trong biểu thức của một template HTML ra thẳng HTML.
        for inner in &e.templates {
            check_template(inner, html, flat, bad, checked);
        }
    }
}

/// `${` đứng ở chỗ thuộc tính không có nháy, hay trong thuộc tính nguy hiểm.
fn open_attribute(before: &str) -> Option<String> {
    let tail = &before[before.rfind('<').unwrap_or(0)..];
    if tail.contains('>') {
        return None; // đang ở chữ giữa các thẻ, không trong thẻ
    }
    if tail.trim_end().ends_with('=') {
        return Some("thuộc tính không có nháy".into());
    }
    // Trong thẻ: đếm nháy kép để biết có đang ở trong giá trị thuộc tính không.
    if tail.matches('"').count() % 2 == 1 {
        let open = tail.rfind("=\"")?;
        let name: String =
            tail[..open].chars().rev().take_while(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
        let name: String = name.chars().rev().collect::<String>().to_ascii_lowercase();
        if DANGEROUS_ATTRS.contains(&name.as_str()) || name.starts_with("on") {
            return Some(format!("thuộc tính `{name}`"));
        }
    }
    None
}

/// Vế phải tới dấu `;` ngoài mọi ngoặc.
fn statement(rhs: &str) -> &str {
    let mut depth = 0i32;
    for (k, c) in rhs.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth < 0 {
                    return &rhs[..k];
                }
            }
            ';' if depth == 0 => return &rhs[..k],
            _ => {}
        }
    }
    rhs
}

/// Vị trí đóng của ngoặc mở ở `open`.
fn closing(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (k, c) in s[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + k);
                }
            }
            _ => {}
        }
    }
    None
}

fn is_esc(e: &str) -> bool {
    let e = e.trim();
    e.starts_with("esc(") && closing(e, 3) == Some(e.len() - 1)
}

/// Bỏ cặp ngoặc tròn bao trọn biểu thức.
fn strip_parens(mut e: &str) -> &str {
    loop {
        e = e.trim();
        if e.starts_with('(') && closing(e, 0) == Some(e.len() - 1) {
            e = &e[1..e.len() - 1];
        } else {
            return e;
        }
    }
}

/// Các vị trí ở ngoài mọi ngoặc.
fn top_level(e: &str) -> Vec<(usize, char)> {
    let mut depth = 0i32;
    let mut out = Vec::new();
    for (k, c) in e.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            _ if depth == 0 => out.push((k, c)),
            _ => {}
        }
    }
    out
}

/// `điều kiện ? a : b` ở mức ngoài cùng: trả về `(a, b)`.
fn ternary(e: &str) -> Option<(&str, &str)> {
    let top = top_level(e);
    let b = e.as_bytes();
    let is_q = |k: usize| {
        b[k] == b'?' && b.get(k + 1) != Some(&b'.') && b.get(k + 1) != Some(&b'?') && (k == 0 || b[k - 1] != b'?')
    };
    let q = top.iter().map(|&(k, _)| k).find(|&k| is_q(k))?;
    let mut nested = 0;
    for &(k, c) in top.iter().filter(|&&(k, _)| k > q) {
        if c == '?' && is_q(k) {
            nested += 1;
        } else if c == ':' {
            if nested == 0 {
                return Some((&e[q + 1..k], &e[k + 1..]));
            }
            nested -= 1;
        }
    }
    None
}

/// Tách theo `+`, `||`, `??` ở mức ngoài cùng.
fn concat_parts(e: &str) -> Vec<&str> {
    let b = e.as_bytes();
    let mut parts = Vec::new();
    let mut last = 0;
    let mut skip = 0;
    for (k, _) in top_level(e) {
        if k < skip {
            continue;
        }
        let two = b.get(k..k + 2);
        if two == Some(b"||") || two == Some(b"??") {
            parts.push(&e[last..k]);
            last = k + 2;
            skip = k + 2;
        } else if b[k] == b'+' && b.get(k + 1) != Some(&b'+') && b.get(k + 1) != Some(&b'=') {
            parts.push(&e[last..k]);
            last = k + 1;
        }
    }
    parts.push(&e[last..]);
    parts
}

fn is_ident(s: &str) -> bool {
    let mut cs = s.chars();
    cs.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_' || c == '$')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

/// Biểu thức này chỉ ra được chữ an toàn để đặt vào HTML.
fn safe_output(e: &str, flat: &str, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    let e = strip_parens(e);
    if e.is_empty() {
        return false;
    }
    // Điều kiện của `?:` không ra HTML, chỉ hai nhánh ra.
    if let Some((a, b)) = ternary(e) {
        return safe_output(a, flat, depth + 1) && safe_output(b, flat, depth + 1);
    }
    let parts = concat_parts(e);
    if parts.len() > 1 {
        return parts.iter().all(|p| safe_output(p, flat, depth + 1));
    }
    safe_atom(e, flat, depth)
}

fn safe_atom(e: &str, flat: &str, depth: usize) -> bool {
    let e = e.trim();
    if e == "T" || e == "S" || is_esc(e) || e.parse::<f64>().is_ok() {
        return true;
    }
    // Phép tính, so sánh, phủ định: ra số hay true/false, không ra chữ.
    let top = top_level(e);
    let b = e.as_bytes();
    let arithmetic = top.iter().any(|&(k, c)| {
        matches!(c, '-' | '*' | '/' | '%') && k > 0
            || (c == '<' || c == '>') && b.get(k + 1) != Some(&b'>') && (k == 0 || b[k - 1] != b'=')
            || c == '=' && (b.get(k + 1) == Some(&b'=') || k > 0 && b[k - 1] == b'!')
    });
    if arithmetic || e.starts_with('!') {
        return true;
    }
    // `a.b.line`: trường số của mô hình.
    let path: Vec<&str> = e.split('.').map(|p| p.trim_end_matches('?')).collect();
    if path.len() > 1 && path.iter().all(|p| is_ident(p)) {
        return NUMERIC.contains(path.last().unwrap());
    }
    // `x.map(f).join("")`: an toàn nếu `f` trả về chữ an toàn.
    if let Some(head) = e.strip_suffix(".join(S)").or_else(|| e.strip_suffix(".join()")) {
        let open = head.rfind(".map(").map(|k| k + 4);
        return match open {
            Some(open) if head.ends_with(')') && closing(head, open) == Some(head.len() - 1) => {
                safe_function(&head[open + 1..head.len() - 1], flat, depth + 1)
            }
            _ => false,
        };
    }
    // Biến: mọi chỗ gán cho nó (trong cả file) đều phải an toàn.
    if is_ident(e) {
        let rhs = assignments(flat, e);
        return !rhs.is_empty() && rhs.iter().all(|r| safe_output(r, flat, depth + 1));
    }
    false
}

/// Hàm mũi tên trả về chữ an toàn, hay tên một hàm như thế.
fn safe_function(f: &str, flat: &str, depth: usize) -> bool {
    let f = f.trim();
    if is_ident(f) {
        let rhs = assignments(flat, f);
        return !rhs.is_empty() && rhs.iter().all(|r| safe_function(r, flat, depth + 1));
    }
    let Some(arrow) = f.find("=>") else { return false };
    let params = f[..arrow].trim();
    let params_ok = is_ident(params) || (params.starts_with('(') && closing(params, 0) == Some(params.len() - 1));
    let body = f[arrow + 2..].trim();
    params_ok && !body.starts_with('{') && safe_output(body, flat, depth + 1)
}

/// Vế phải của mọi `name = …` và `name += …` trong mã đã làm phẳng.
fn assignments<'a>(flat: &'a str, name: &str) -> Vec<&'a str> {
    let b = flat.as_bytes();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(k) = flat[from..].find(name) {
        let at = from + k;
        from = at + name.len();
        let before_ok = at == 0 || {
            let p = b[at - 1];
            !(p.is_ascii_alphanumeric() || p == b'_' || p == b'$' || p == b'.')
        };
        let after = &flat[at + name.len()..];
        let after_ok = after.chars().next().is_some_and(|c| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'));
        if !before_ok || !after_ok {
            continue;
        }
        let rest = after.trim_start();
        let rhs = rest.strip_prefix("+=").or_else(|| {
            rest.strip_prefix('=').filter(|r| !r.starts_with('=') && !r.starts_with('>'))
        });
        if let Some(rhs) = rhs {
            out.push(statement(rhs));
        }
    }
    out
}

#[test]
fn moi_cho_chen_vao_html_deu_qua_esc() {
    let (bad, checked) = scan(UI);
    assert!(bad.is_empty(), "ui.html có chỗ chèn chữ chưa thoát:\n{}", bad.join("\n"));
    // Bộ quét lạc nhịp (regex, chuỗi lạ) thì có thể lặng lẽ bỏ qua cả nửa file.
    assert!(checked >= 60, "chỉ kiểm được {checked} chỗ chèn: bộ quét đọc sai ui.html?");
}

/// Bộ quét phải bắt được đúng những lỗi mà TM-92 lo: gài từng lỗi vào bản
/// sao của `ui.html` và đòi nó báo.
#[test]
fn bo_quet_bat_duoc_cho_chen_chu_tho() {
    let replace = |old: &str, new: &str| {
        assert!(UI.contains(old), "ui.html không còn `{old}`: cập nhật ca gài lỗi này");
        UI.replacen(old, new, 1)
    };
    let append = |code: &str| UI.replacen("</script>", &format!("\n{code}\n</script>"), 1);
    let cases = [
        ("lý do đề xuất chèn thẳng", replace("${esc(p.reason)}", "${p.reason}")),
        ("gợi ý chèn thẳng", replace("${esc(m.hint)}", "${m.hint}")),
        ("gọi hàm không phải esc", replace("${esc(label(v.from))}", "${label(v.from)}")),
        ("template lồng không có thẻ", replace("` · ${v.count} lần`", "` · ${v.spec} lần`")),
        ("thuộc tính có esc nhưng thiếu nháy", replace(r#"data-reason="${esc(p.id)}""#, "data-reason=${esc(p.id)}")),
        ("href lấy từ dữ liệu", append(r#"const bad1 = (u) => `<a href="${esc(u)}">x</a>`;"#)),
        ("onclick lấy từ dữ liệu", append(r#"const bad2 = (u) => `<b onclick="go('${esc(u)}')">x</b>`;"#)),
        ("innerHTML gán chữ thô", append("function bad3(box, p) { box.innerHTML = p.reason; }")),
        ("biến trùng tên được gán chữ thô", append("function bad4(box, p) { const head = p.reason; box.innerHTML = head; }")),
        ("cộng dồn chữ thô", append(r#"function bad5(box, p) { let acc = ""; acc += p.reason; box.innerHTML = acc; }"#)),
        ("map với hàm không thoát", append("function bad6(box, xs) { box.innerHTML = xs.map((x) => x.name).join(\"\"); }")),
        ("insertAdjacentHTML", append(r#"function bad7(el, p) { el.insertAdjacentHTML("beforeend", p.reason); }"#)),
        ("eval", append("function bad8(p) { eval(p.reason); }")),
        ("tra bảng hằng bằng khoá từ dữ liệu", replace("${esc(kinds[n.kind])}", "${kinds[n.kind]}")),
    ];
    for (what, src) in cases {
        assert!(!problems(&src).is_empty(), "bộ quét không bắt được: {what}");
    }
}

/// Hộp `confirm()` trả "không" thì form phải báo đã huỷ, chưa ghi gì; trước
/// đây nó im lặng và người dùng không biết luật khoá đã đổi hay chưa.
#[test]
fn tra_khong_o_hop_xac_nhan_thi_bao_da_huy() {
    let mut seen = 0;
    for (at, _) in UI.match_indices("confirm(") {
        let rest = &UI[at..];
        let until = rest.find("return;").expect("sau confirm( phải có return;");
        assert!(
            rest[..until].contains("showError(\"Đã huỷ"),
            "nhánh trả \"không\" sau confirm( không báo gì:\n{}",
            &rest[..until]
        );
        seen += 1;
    }
    assert!(seen >= 1, "ui.html không còn confirm( nào: cập nhật test này");
}
