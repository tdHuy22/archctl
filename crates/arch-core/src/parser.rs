//! Parser cho DSL kiến trúc.
//!
//! DSL theo dòng: mỗi câu lệnh một dòng, thân khối lùi vào bằng khoảng trắng.
//! Dòng có ký tự đầu tiên là `#` là chú thích. Token `#draft` và `@human` chỉ
//! được coi là chú thích trạng thái khi đứng ở cuối dòng, nên hai cách dùng
//! của `#` không nhập nhằng.

use crate::diag::Diag;
use crate::model::*;

struct Line<'a> {
    no: usize,
    indent: usize,
    text: &'a str,
}

/// Đọc nguồn thành mô hình. Luôn trả về cả hai: mô hình đọc được tới đâu và
/// danh sách chẩn đoán. Một dòng sai không làm hỏng cả file.
pub fn parse(src: &str) -> (Document, Vec<Diag>) {
    let src = crate::strip_bom(src);
    let mut doc = Document::default();
    let mut diags = Vec::new();
    let lines = collect_lines(src);

    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if line.indent > 0 {
            diags.push(
                Diag::error(line.no, "dòng lùi vào nhưng không thuộc khối nào")
                    .with_hint("bỏ khoảng trắng đầu dòng, hoặc kiểm tra khối phía trên"),
            );
            i += 1;
            continue;
        }

        let (body, meta) = split_meta(line.text, line.no, &mut diags);
        let keyword = body.split_whitespace().next().unwrap_or("");
        let rest = body[keyword.len()..].trim();

        match keyword {
            "component" | "external" => {
                parse_component(
                    rest,
                    keyword == "external",
                    meta,
                    line.no,
                    &mut doc,
                    &mut diags,
                );
                i += 1;
            }
            "store" => {
                parse_store(rest, meta, line.no, &mut doc, &mut diags);
                i += 1;
            }
            "allow" | "deny" => {
                let kind = if keyword == "allow" {
                    RuleKind::Allow
                } else {
                    RuleKind::Deny
                };
                parse_rule(rest, kind, meta, line.no, &mut doc, &mut diags);
                i += 1;
            }
            "view" => {
                parse_view(rest, meta, line.no, &mut doc, &mut diags);
                i += 1;
            }
            "api" => {
                parse_api(rest, meta, line.no, &mut doc, &mut diags);
                i += 1;
            }
            "verify" => {
                parse_verify(rest, meta, line.no, &mut doc, &mut diags);
                i += 1;
            }
            "service" => {
                parse_service(rest, meta, line.no, &mut doc, &mut diags);
                i += 1;
            }
            "link" => {
                parse_link(rest, meta, line.no, &mut doc, &mut diags);
                i += 1;
            }
            "invariant" => {
                parse_invariant(rest, meta, line.no, &mut doc, &mut diags);
                i += 1;
            }
            "flow" => {
                let (body_lines, next) = take_block(&lines, i + 1);
                parse_flow(rest, body_lines, meta, line.no, &mut doc, &mut diags);
                i = next;
            }
            "contract" => {
                let (body_lines, next) = take_block(&lines, i + 1);
                parse_contract(rest, body_lines, meta, line.no, &mut doc, &mut diags);
                i = next;
            }
            "boundary" => {
                let (body_lines, next) = take_block(&lines, i + 1);
                parse_boundary(rest, body_lines, meta, line.no, &mut doc, &mut diags);
                i = next;
            }
            "" => i += 1,
            other => {
                diags.push(
                    Diag::error(line.no, format!("từ khoá không nhận ra: `{other}`")).with_hint(
                        "từ khoá hợp lệ: component, external, store, allow, deny, \
                         flow, contract, boundary, invariant, view, api, verify, \
                         service, link",
                    ),
                );
                i += 1;
            }
        }
    }

    (doc, diags)
}

fn collect_lines(src: &str) -> Vec<Line<'_>> {
    src.lines()
        .enumerate()
        .filter_map(|(idx, raw)| {
            let trimmed = raw.trim_end();
            let text = trimmed.trim_start();
            if text.is_empty() || text.starts_with('#') {
                return None;
            }
            Some(Line {
                no: idx + 1,
                indent: trimmed.len() - text.len(),
                text,
            })
        })
        .collect()
}

/// Thu các dòng lùi vào liên tiếp làm thân khối. Trả về chỉ số dòng kế tiếp.
fn take_block<'a, 'b>(lines: &'b [Line<'a>], start: usize) -> (&'b [Line<'a>], usize) {
    let end = lines[start..]
        .iter()
        .position(|l| l.indent == 0)
        .map_or(lines.len(), |p| start + p);
    (&lines[start..end], end)
}

/// Cắt các chú thích `@owner` và `#status` ở cuối dòng ra khỏi phần nội dung.
/// Vị trí này có nằm trong một chuỗi nháy kép không — đếm số dấu nháy chưa
/// bị gạch chéo ở phía trước.
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

fn split_meta<'a>(line: &'a str, no: usize, diags: &mut Vec<Diag>) -> (&'a str, Meta) {
    let mut cut = line.len();
    let mut annotations = Vec::new();
    for (offset, token) in whitespace_tokens(line).into_iter().rev() {
        // Bên trong một chuỗi thì `@` và `#` là nội dung, không phải chú thích.
        // `verify x runs "npm test -- --grep @slow"` từng làm hỏng cả file luật.
        if (token.starts_with('@') || token.starts_with('#')) && !inside_quotes(line, offset) {
            annotations.push(token);
            cut = offset;
        } else {
            break;
        }
    }
    annotations.reverse();

    let mut meta = Meta {
        line: no,
        ..Meta::default()
    };
    let mut saw_owner = false;
    let mut saw_status = false;
    for token in annotations {
        match token {
            "@human" | "@ai" => {
                if saw_owner {
                    diags.push(Diag::warning(no, "có nhiều hơn một chú thích chủ sở hữu"));
                }
                meta.owner = if token == "@ai" {
                    Owner::Ai
                } else {
                    Owner::Human
                };
                saw_owner = true;
            }
            "#draft" | "#proposed" | "#accepted" | "#locked" => {
                if saw_status {
                    diags.push(Diag::warning(no, "có nhiều hơn một chú thích trạng thái"));
                }
                meta.status = match token {
                    "#draft" => Status::Draft,
                    "#proposed" => Status::Proposed,
                    "#accepted" => Status::Accepted,
                    _ => Status::Locked,
                };
                saw_status = true;
            }
            other => diags.push(
                Diag::error(no, format!("chú thích không nhận ra: `{other}`")).with_hint(
                    "chủ sở hữu: @human hoặc @ai; \
                     trạng thái: #draft, #proposed, #accepted, #locked",
                ),
            ),
        }
    }

    (line[..cut].trim_end(), meta)
}

/// Viết lại chú thích `@owner #status` ở cuối một dòng luật, giữ nguyên phần
/// nội dung và cách căn lề. Dùng khi người bấm khoá một luật trên sơ đồ.
pub fn rewrite_annotations(line: &str, owner: Owner, status: Status) -> String {
    let mut cut = line.len();
    for (offset, token) in whitespace_tokens(line).into_iter().rev() {
        if token.starts_with('@') || token.starts_with('#') {
            cut = offset;
        } else {
            break;
        }
    }
    let body = line[..cut].trim_end();
    let owner = if owner == Owner::Ai { "@ai" } else { "@human" };
    format!("{body}  {owner} {}", status.as_str())
}

fn whitespace_tokens(s: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in s.char_indices() {
        match (c.is_whitespace(), start) {
            (false, None) => start = Some(i),
            (true, Some(b)) => {
                out.push((b, &s[b..i]));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(b) = start {
        out.push((b, &s[b..]));
    }
    out
}

/// Gỡ `\"` thành `"` và `\\` thành `\`.
///
/// Chỉ hai cái đó. Mọi dấu gạch chéo khác giữ nguyên để shell nhận được —
/// `grep '^\[dependencies\]'` phải tới được grep đúng như đã viết.
fn unescape_quotes(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some(next @ ('"' | '\\')) => out.push(next),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `verify property_test runs "cargo test --test props"`
///
/// Lệnh nằm trong dấu nháy kép, vì nó gần như luôn có khoảng trắng và vì dấu
/// nháy làm rõ đâu là ranh giới của nó.
fn parse_verify(rest: &str, meta: Meta, line: usize, doc: &mut Document, diags: &mut Vec<Diag>) {
    let Some((name, command)) = rest.split_once(" runs ") else {
        diags.push(
            Diag::error(line, "thiếu `runs` trong khai báo verify")
                .with_hint("dạng đúng: verify <tên> runs \"<lệnh>\""),
        );
        return;
    };
    let name = name.trim();
    if !check_ident(name, "tên cách kiểm", line, diags) {
        return;
    }
    let command = command.trim();
    let Some(command) = command.strip_prefix('"').and_then(|c| c.strip_suffix('"')) else {
        diags.push(
            Diag::error(line, "lệnh phải nằm trong dấu nháy kép")
                .with_hint(format!("viết: verify {name} runs \"{command}\"")),
        );
        return;
    };
    if command.trim().is_empty() {
        diags.push(Diag::error(
            line,
            format!("cách kiểm `{name}` có lệnh rỗng"),
        ));
        return;
    }
    doc.verifies.push(Verify {
        name: name.to_string(),
        command: unescape_quotes(command),
        meta,
    });
}

/// Tách danh sách phân cách bằng dấu phẩy, tôn trọng dấu ngoặc kép.
fn split_list(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in s.chars() {
        match c {
            '"' => quoted = !quoted,
            ',' if !quoted => {
                push_item(&mut out, &mut current);
            }
            _ => current.push(c),
        }
    }
    push_item(&mut out, &mut current);
    out
}

fn push_item(out: &mut Vec<String>, current: &mut String) {
    let item = current.trim();
    if !item.is_empty() {
        out.push(item.to_string());
    }
    current.clear();
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

fn check_ident(name: &str, what: &str, no: usize, diags: &mut Vec<Diag>) -> bool {
    if is_ident(name) {
        return true;
    }
    diags.push(
        Diag::error(no, format!("{what} không hợp lệ: `{name}`"))
            .with_hint("tên chỉ gồm chữ, số, gạch dưới và gạch nối"),
    );
    false
}

fn parse_component(
    rest: &str,
    external: bool,
    meta: Meta,
    no: usize,
    doc: &mut Document,
    diags: &mut Vec<Diag>,
) {
    let (name, paths) = match rest.split_once(" at ") {
        Some((n, p)) => (n.trim(), split_list(p)),
        None => (rest.trim(), Vec::new()),
    };
    if name.is_empty() {
        diags.push(Diag::error(no, "thiếu tên component"));
        return;
    }
    if !check_ident(name, "tên component", no, diags) {
        return;
    }
    doc.components.push(Component {
        name: name.to_string(),
        paths,
        external,
        meta,
    });
}

fn parse_store(rest: &str, meta: Meta, no: usize, doc: &mut Document, diags: &mut Vec<Diag>) {
    let Some((name, tail)) = rest.split_once(" owned_by ") else {
        diags.push(
            Diag::error(no, "`store` thiếu `owned_by`")
                .with_hint("dạng đúng: store <Tên> owned_by <Component> [at \"bảng\", ...]"),
        );
        return;
    };
    let (owner, tables) = match tail.split_once(" at ") {
        Some((o, t)) => (o.trim(), split_list(t)),
        None => (tail.trim(), Vec::new()),
    };
    if !check_ident(name.trim(), "tên store", no, diags)
        || !check_ident(owner, "tên component trong owned_by", no, diags)
    {
        return;
    }
    doc.stores.push(Store {
        name: name.trim().to_string(),
        owned_by: owner.to_string(),
        tables,
        meta,
    });
}

fn parse_rule(
    rest: &str,
    kind: RuleKind,
    meta: Meta,
    no: usize,
    doc: &mut Document,
    diags: &mut Vec<Diag>,
) {
    let (main, except) = match rest.split_once(" except ") {
        Some((m, e)) => (m, split_list(e)),
        None => (rest, Vec::new()),
    };
    let Some((from, to)) = main.split_once("->") else {
        diags.push(
            Diag::error(no, "quan hệ thiếu `->`")
                .with_hint("dạng đúng: allow <Nguồn> -> <Đích>[, <Đích>...]"),
        );
        return;
    };
    if kind == RuleKind::Allow && !except.is_empty() {
        diags.push(
            Diag::error(no, "`allow` không dùng được `except`")
                .with_hint("`except` chỉ có nghĩa với `deny`"),
        );
        return;
    }

    let items = split_list(to);
    if items.is_empty() {
        diags.push(Diag::error(no, "quan hệ không có đích nào"));
        return;
    }
    let Some(from) = parse_target(from.trim(), no, diags) else {
        return;
    };
    let mut to_targets = Vec::with_capacity(items.len());
    for item in &items {
        let Some(target) = parse_target(item, no, diags) else {
            return;
        };
        to_targets.push(target);
    }
    doc.rules.push(Rule {
        kind,
        from,
        to: to_targets,
        except,
        meta,
    });
}

fn parse_target(s: &str, no: usize, diags: &mut Vec<Diag>) -> Option<Target> {
    if s == "*" {
        return Some(Target::Any);
    }
    if !check_ident(s, "tên trong quan hệ", no, diags) {
        return None;
    }
    Some(Target::Named(s.to_string()))
}

/// `service Api from compose:api runs Serve, Rag`,
/// `service Vote from compose:vote, k8s:vote runs Vote`
///
/// `from` trước `runs`, cả hai đều tuỳ chọn. Thứ tự cố định để một dòng chỉ
/// có một cách đọc.
fn parse_service(rest: &str, meta: Meta, no: usize, doc: &mut Document, diags: &mut Vec<Diag>) {
    let (head, runs) = match rest.split_once(" runs ") {
        Some((h, r)) => (h, split_list(r)),
        None => (rest, Vec::new()),
    };
    let (name, from) = match head.split_once(" from ") {
        Some((n, f)) => (n.trim(), Some(f.trim())),
        None => (head.trim(), None),
    };
    let from: Vec<&str> = from
        .map(|f| f.split(',').map(str::trim).collect())
        .unwrap_or_default();
    if name.contains(" runs") || from.iter().any(|f| f.is_empty() || f.contains(' ')) {
        diags.push(
            Diag::error(no, "`service` viết sai thứ tự hay thừa chữ")
                .with_hint("dạng đúng: service <Tên> [from <nguồn>, ...] [runs <Component>, ...]"),
        );
        return;
    }
    if !check_ident(name, "tên service", no, diags) {
        return;
    }
    for c in &runs {
        if !check_ident(c, "tên component trong runs", no, diags) {
            return;
        }
    }
    let mut sources = Vec::new();
    for text in from {
        match parse_source(text) {
            Ok(s) => sources.push(s),
            Err(why) => {
                diags.push(Diag::error(no, format!("`from {text}`: {why}")).with_hint(
                    "dạng đúng: from compose:<service>, compose:<thư mục>/<service>, \
                     k8s:<workload>, k8s:<thư mục>/<workload>, tf:<loại>.<tên>, tf:<thư mục>/<loại>.<tên>, \
                     dockerfile:<đường dẫn>, values:<đường dẫn>",
                ));
                return;
            }
        }
    }
    doc.services.push(Service {
        name: name.to_string(),
        sources,
        runs,
        meta,
    });
}

/// Đường dẫn trong `from` là tương đối với gốc dự án và không được ra khỏi đó:
/// file luật là thứ agent đề xuất được, và một `from dockerfile:../../x` là
/// cách nhờ archctl đọc hộ một file nằm ngoài repo.
fn parse_source(text: &str) -> Result<Source, String> {
    let relative = |path: &str| {
        !path.is_empty()
            && !path.starts_with('/')
            && !path.contains('\\')
            && path
                .split('/')
                .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
    };
    // `compose:<thư mục>/<tên>` và `k8s:<thư mục>/<tên>`: `./` là gốc dự án.
    let scoped = |rest: &str, what: &str| -> Result<(Option<String>, String), String> {
        let (dir, name) = match rest.rsplit_once('/') {
            Some((d, n)) => (Some(d), n),
            None => (None, rest),
        };
        if !is_compose_name(name) {
            return Err(format!("tên {what} không hợp lệ: `{name}`"));
        }
        let dir = match dir {
            Some(".") => Some(""),
            Some(d) if !relative(d) => {
                return Err(format!(
                    "thư mục `{d}` phải là đường dẫn tương đối nằm trong dự án"
                ));
            }
            d => d,
        };
        Ok((dir.map(str::to_string), name.to_string()))
    };
    if let Some(rest) = text.strip_prefix("compose:") {
        let (dir, name) = scoped(rest, "service compose")?;
        return Ok(Source::Compose { dir, name });
    }
    if let Some(rest) = text.strip_prefix("k8s:") {
        let (dir, name) = scoped(rest, "workload")?;
        return Ok(Source::K8s { dir, name });
    }
    if let Some(rest) = text.strip_prefix("tf:") {
        // Địa chỉ có dấu `.`, thư mục module gốc đứng trước `/` cuối.
        let (dir, addr) = match rest.rsplit_once('/') {
            Some((".", a)) => (Some(String::new()), a),
            Some((d, a)) if relative(d) => (Some(d.to_string()), a),
            Some((d, _)) => {
                return Err(format!(
                    "thư mục `{d}` phải là đường dẫn tương đối nằm trong dự án"
                ));
            }
            None => (None, rest),
        };
        if !is_tf_addr(addr) {
            return Err(format!(
                "địa chỉ Terraform không hợp lệ: `{addr}` (dạng `<loại>.<tên>` hay \
                 `data.<loại>.<tên>`, trong module thì `module.<tên>.<loại>.<tên>`)"
            ));
        }
        return Ok(Source::Tf { dir, addr: addr.to_string() });
    }
    if let Some(path) = text.strip_prefix("dockerfile:") {
        if !relative(path) {
            return Err(format!(
                "`{path}` phải là đường dẫn tương đối nằm trong dự án"
            ));
        }
        return Ok(Source::Dockerfile(path.to_string()));
    }
    if let Some(path) = text.strip_prefix("values:") {
        if !relative(path) {
            return Err(format!(
                "`{path}` phải là đường dẫn tương đối nằm trong dự án"
            ));
        }
        return Ok(Source::Values(path.to_string()));
    }
    Err("không nhận ra nguồn cấu hình".into())
}

/// Địa chỉ tài nguyên Terraform: `(module.<tên>.)*<loại>.<tên>`.
fn is_tf_addr(s: &str) -> bool {
    let parts: Vec<&str> = s.split('.').collect();
    let ok = |p: &str| {
        !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
    };
    if parts.len() < 2 || !parts.iter().all(|p| ok(p)) {
        return false;
    }
    // Đuôi là `<loại>.<tên>`, hay `data.<loại>.<tên>` cho data source.
    let tail = if parts.len() >= 3 && parts[parts.len() - 3] == "data" { 3 } else { 2 };
    let head = &parts[..parts.len() - tail];
    head.len().is_multiple_of(2)
        && head.chunks(2).all(|c| c[0] == "module")
        && parts[parts.len() - 2] != "module"
        && (tail == 3 || parts[parts.len() - 2] != "data")
}

/// Tên service mà compose chấp nhận.
fn is_compose_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// `link Api -> Qdrant, Db`
fn parse_link(rest: &str, meta: Meta, no: usize, doc: &mut Document, diags: &mut Vec<Diag>) {
    let Some((from, to)) = rest.split_once("->") else {
        diags.push(
            Diag::error(no, "`link` thiếu `->`")
                .with_hint("dạng đúng: link <Service> -> <Service>[, <Service>...]"),
        );
        return;
    };
    let from = from.trim();
    let to = split_list(to);
    if to.is_empty() {
        diags.push(Diag::error(no, "`link` không có đích nào"));
        return;
    }
    if !check_ident(from, "tên service trong link", no, diags) {
        return;
    }
    for t in &to {
        if !check_ident(t, "tên service trong link", no, diags) {
            return;
        }
    }
    doc.links.push(Link {
        from: from.to_string(),
        to,
        meta,
    });
}

fn parse_view(rest: &str, meta: Meta, no: usize, doc: &mut Document, diags: &mut Vec<Diag>) {
    let Some((name, members)) = rest.split_once(':') else {
        diags.push(
            Diag::error(no, "`view` thiếu dấu hai chấm")
                .with_hint("dạng đúng: view <Tên>: <Component>[, <Component>...]"),
        );
        return;
    };
    if !check_ident(name.trim(), "tên view", no, diags) {
        return;
    }
    doc.views.push(View {
        name: name.trim().to_string(),
        components: split_list(members),
        meta,
    });
}

fn parse_api(rest: &str, meta: Meta, no: usize, doc: &mut Document, diags: &mut Vec<Diag>) {
    let Some((name, symbols)) = rest.split_once(" at ") else {
        diags.push(
            Diag::error(no, "`api` thiếu `at`")
                .with_hint("dạng đúng: api <Tên> at \"strcpy\", \"gets\""),
        );
        return;
    };
    if !check_ident(name.trim(), "tên api", no, diags) {
        return;
    }
    let symbols = split_list(symbols);
    if symbols.is_empty() {
        diags.push(Diag::error(
            no,
            format!("api `{}` không liệt kê ký hiệu nào", name.trim()),
        ));
        return;
    }
    doc.apis.push(Api {
        name: name.trim().to_string(),
        symbols,
        meta,
    });
}

fn parse_invariant(rest: &str, meta: Meta, no: usize, doc: &mut Document, diags: &mut Vec<Diag>) {
    let mut cursor = rest.trim();
    let Some(kind_end) = cursor.find(char::is_whitespace) else {
        diags.push(
            Diag::error(no, "`invariant` thiếu nội dung").with_hint(
                "dạng đúng: invariant <loại> [<phạm vi>] <mệnh đề> [verify <cách kiểm>]",
            ),
        );
        return;
    };
    let kind = cursor[..kind_end].to_string();
    cursor = cursor[kind_end..].trim_start();

    let mut scope = None;
    if let Some(stripped) = cursor.strip_prefix('[') {
        match stripped.find(']') {
            Some(end) => {
                scope = Some(stripped[..end].trim().to_string());
                cursor = stripped[end + 1..].trim_start();
            }
            None => {
                diags.push(Diag::error(no, "phạm vi invariant thiếu dấu `]`"));
                return;
            }
        }
    }

    let (expr, verify) = match cursor.split_once(" verify ") {
        Some((e, v)) => (e.trim(), split_list(v.trim_start().trim_start_matches(':'))),
        None => (cursor, Vec::new()),
    };
    if expr.is_empty() {
        diags.push(Diag::error(no, "`invariant` thiếu mệnh đề"));
        return;
    }
    doc.invariants.push(Invariant {
        kind,
        scope,
        expr: expr.to_string(),
        verify,
        meta,
    });
}

fn parse_flow(
    rest: &str,
    body: &[Line<'_>],
    meta: Meta,
    no: usize,
    doc: &mut Document,
    diags: &mut Vec<Diag>,
) {
    let name = rest.trim().trim_end_matches(':').trim();
    if !check_ident(name, "tên flow", no, diags) {
        return;
    }
    if !rest.trim_end().ends_with(':') {
        diags.push(Diag::error(no, "`flow` thiếu dấu hai chấm ở cuối"));
    }
    if body.is_empty() {
        diags.push(Diag::warning(
            no,
            format!("flow `{name}` không có bước nào"),
        ));
    }

    let lines = body
        .iter()
        .filter_map(|l| parse_flow_line(l, diags))
        .collect();
    doc.flows.push(Flow {
        name: name.to_string(),
        lines,
        meta,
    });
}

fn parse_flow_line(line: &Line<'_>, diags: &mut Vec<Diag>) -> Option<FlowLine> {
    let text = line.text;
    if let Some(rest) = text.strip_prefix("parallel:") {
        return Some(FlowLine::Parallel(split_list(rest)));
    }
    if let Some(rest) = text.strip_prefix("on_fail(") {
        let Some((step, tail)) = rest.split_once(')') else {
            diags.push(Diag::error(line.no, "`on_fail` thiếu dấu `)`"));
            return None;
        };
        let Some(steps) = tail.trim_start().strip_prefix(':') else {
            diags.push(Diag::error(line.no, "`on_fail(...)` thiếu dấu hai chấm"));
            return None;
        };
        return Some(FlowLine::OnFail {
            step: step.trim().to_string(),
            steps: split_list(steps),
        });
    }
    if let Some(rest) = text.strip_prefix("if ") {
        let Some((cond, steps)) = rest.split_once(':') else {
            diags.push(Diag::error(line.no, "`if` thiếu dấu hai chấm"));
            return None;
        };
        return Some(FlowLine::Cond {
            cond: cond.trim().to_string(),
            steps: split_list(steps),
        });
    }
    if text.contains("->") {
        let steps: Vec<String> = text
            .split("->")
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if steps.len() < 2 {
            diags.push(Diag::error(line.no, "chuỗi bước cần ít nhất hai bước"));
            return None;
        }
        return Some(FlowLine::Sequence(steps));
    }
    diags.push(
        Diag::error(line.no, format!("không hiểu dòng trong flow: `{text}`"))
            .with_hint("dạng hợp lệ: `a -> b`, `parallel: a, b`, `if <đk>: a`, `on_fail(a): b`"),
    );
    None
}

fn parse_contract(
    rest: &str,
    body: &[Line<'_>],
    meta: Meta,
    no: usize,
    doc: &mut Document,
    diags: &mut Vec<Diag>,
) {
    let head = rest.trim().trim_end_matches(':').trim();
    let Some((component, capability)) = head.split_once('.') else {
        diags.push(
            Diag::error(no, "`contract` thiếu dấu chấm")
                .with_hint("dạng đúng: contract <Component>.<năng-lực>:"),
        );
        return;
    };
    if !check_ident(component, "tên component trong contract", no, diags)
        || !check_ident(capability, "tên năng lực trong contract", no, diags)
    {
        return;
    }
    doc.contracts.push(Contract {
        component: component.to_string(),
        capability: capability.to_string(),
        fields: parse_fields(body),
        meta,
    });
}

fn parse_boundary(
    rest: &str,
    body: &[Line<'_>],
    meta: Meta,
    no: usize,
    doc: &mut Document,
    diags: &mut Vec<Diag>,
) {
    let head = rest.trim().trim_end_matches(':').trim();
    let Some((name, members)) = head.split_once(" contains ") else {
        diags.push(
            Diag::error(no, "`boundary` thiếu `contains`")
                .with_hint("dạng đúng: boundary <Tên> contains <Component>[, ...]:"),
        );
        return;
    };
    if !check_ident(name.trim(), "tên boundary", no, diags) {
        return;
    }
    doc.boundaries.push(Boundary {
        name: name.trim().to_string(),
        contains: split_list(members),
        fields: parse_fields(body),
        meta,
    });
}

/// Thân `contract` và `boundary`: `khoá giá trị`, dòng lùi sâu hơn là tiếp nối
/// giá trị của khoá liền trước. Giai đoạn này chưa hiểu ngữ nghĩa từng khoá.
fn parse_fields(body: &[Line<'_>]) -> Vec<Field> {
    let base = body.first().map_or(0, |l| l.indent);
    let mut fields: Vec<Field> = Vec::new();
    for line in body {
        if line.indent > base
            && let Some(last) = fields.last_mut()
        {
            last.values.push(line.text.to_string());
            continue;
        }
        let (key, value) = match line.text.split_once(char::is_whitespace) {
            Some((k, v)) => (k, v.trim()),
            None => (line.text, ""),
        };
        fields.push(Field {
            key: key.trim_end_matches(':').to_string(),
            values: if value.is_empty() {
                Vec::new()
            } else {
                vec![value.to_string()]
            },
            line: line.no,
        });
    }
    fields
}
