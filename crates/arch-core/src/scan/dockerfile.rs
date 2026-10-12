//! Bộ đọc Dockerfile: đủ để biết image chạy lệnh gì, từ thư mục nào, và mã
//! nào được chép vào đâu.
//!
//! Đọc: chỉ thị `# escape=` đầu file, dòng nối, chú thích, heredoc của
//! BuildKit (`RUN <<EOF`), `FROM ... AS`, `WORKDIR`, `COPY`/`ADD`, `CMD`,
//! `ENTRYPOINT` (dạng JSON lẫn dạng shell), `ENV`, `ARG`, `RUN`. Biến `$X` và
//! `${X}` được thay khi biết giá trị từ `ARG`/`ENV` trước đó.
//!
//! Không chạy gì. Lệnh chỉ được giữ lại dưới dạng chữ.
//!
//! File là của người khác và được đọc trong hook, nên có trần (TM-8): giá trị
//! sau khi thay biến không dài quá [`MAX_EXPANDED`] (`ARG B=$A$A` lặp lại là
//! phình theo luỹ thừa), và không giữ quá [`MAX_COPIES`] lệnh `COPY`.

use std::collections::HashMap;

/// Độ dài tối đa của một giá trị sau khi thay biến; quá thì cắt.
pub const MAX_EXPANDED: usize = 8 << 10;
/// Tổng byte giá trị của `ARG`/`ENV` được nhớ cho cả file: 70 000 `ARG` mỗi
/// cái 8 KB là nửa GB. Quá thì biến sau đó coi như không biết.
pub const MAX_VARS: usize = 1 << 20;
/// Số lệnh `COPY`/`ADD` giữ lại cho cả file. Dockerfile thật có vài chục.
pub const MAX_COPIES: usize = 4096;

/// Lệnh khởi chạy, như Dockerfile hay compose viết.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Dạng JSON: `["python", "-m", "app"]`, chạy thẳng không qua shell.
    Exec(Vec<String>),
    /// Dạng chuỗi: chạy qua `/bin/sh -c`.
    Shell(String),
}

impl Command {
    /// Dãy đối số thật sự được chạy.
    pub fn argv(&self) -> Vec<String> {
        match self {
            Command::Exec(args) => args.clone(),
            Command::Shell(s) => vec!["/bin/sh".into(), "-c".into(), s.clone()],
        }
    }
}

/// Một lần chép mã vào image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Copy {
    /// `--from=<stage hay image>`: chép từ stage khác chứ không từ build context.
    pub from: Option<String>,
    /// Đường dẫn nguồn, tương đối với build context (hay với stage nguồn).
    pub sources: Vec<String>,
    /// Đích, đường dẫn tuyệt đối trong image.
    pub dest: String,
    /// Đích là thư mục: kết thúc bằng `/`, hay có nhiều nguồn.
    pub dest_dir: bool,
    pub line: usize,
}

/// Một lệnh `RUN`, kèm thư mục làm việc lúc đó.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub workdir: String,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// Tên sau `AS`.
    pub name: Option<String>,
    /// Image hay stage mà `FROM` trỏ tới.
    pub from: String,
    pub line: usize,
    /// Thư mục làm việc cuối cùng; `None` khi stage không đặt `WORKDIR` nào.
    pub workdir: Option<String>,
    /// Thư mục làm việc thừa hưởng từ stage mà `FROM` trỏ tới: `COPY . .`
    /// trong stage này tính đích từ đó.
    pub base_workdir: String,
    pub copies: Vec<Copy>,
    pub runs: Vec<Run>,
    pub env: Vec<(String, String)>,
    pub cmd: Option<(Command, usize)>,
    pub entrypoint: Option<(Command, usize)>,
    /// `ENTRYPOINT` đặt trong stage này xoá `CMD` thừa hưởng từ image gốc.
    pub resets_cmd: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dockerfile {
    pub stages: Vec<Stage>,
}

/// Những gì một stage thấy được khi chạy, sau khi đã thừa hưởng từ các stage
/// mà nó dựng lên trên.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effective {
    pub workdir: String,
    pub copies: Vec<Copy>,
    pub env: Vec<(String, String)>,
    pub cmd: Option<(Command, usize)>,
    pub entrypoint: Option<(Command, usize)>,
}

impl Dockerfile {
    /// Stage được dựng: `target` nếu có, không thì stage cuối.
    pub fn target(&self, target: Option<&str>) -> Option<usize> {
        match target {
            Some(t) => self.stages.iter().position(|s| s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(t))),
            None => self.stages.len().checked_sub(1),
        }
    }

    /// Trạng thái lúc chạy của stage `index`, đi ngược theo chuỗi `FROM`.
    pub fn effective(&self, index: usize) -> Effective {
        // Chuỗi stage từ gốc tới `index`. Một stage chỉ dựng được trên stage
        // đứng trước nó, nên chuỗi luôn hữu hạn.
        let mut chain = vec![index];
        let mut at = index;
        while let Some(parent) = self.stages[..at]
            .iter()
            .rposition(|s| s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&self.stages[at].from)))
        {
            chain.push(parent);
            at = parent;
        }
        chain.reverse();
        let mut out = Effective { workdir: "/".into(), ..Effective::default() };
        for &i in &chain {
            let s = &self.stages[i];
            if let Some(w) = &s.workdir {
                out.workdir = w.clone();
            }
            out.copies.extend(s.copies.iter().cloned());
            out.env.extend(s.env.iter().cloned());
            if s.resets_cmd {
                out.cmd = None;
            }
            if s.entrypoint.is_some() {
                out.entrypoint = s.entrypoint.clone();
            }
            if s.cmd.is_some() {
                out.cmd = s.cmd.clone();
            }
        }
        out
    }

    /// Mọi lệnh `RUN` ở mọi stage: tìm `go build` thì cần cả stage dựng.
    pub fn all_runs(&self) -> impl Iterator<Item = &Run> {
        self.stages.iter().flat_map(|s| &s.runs)
    }
}

/// Các chỉ thị như bộ đọc thấy: `(dòng, TỪ KHOÁ, đối số)`, sau khi đã nối
/// dòng, bỏ chú thích, và nối thân heredoc vào. Để đối chiếu với bộ đọc khác.
pub fn instructions_of(text: &str) -> Vec<(usize, String, String)> {
    instructions(crate::strip_bom(text)).into_iter().map(|i| (i.line, i.keyword, i.args)).collect()
}

/// Một chỉ thị, sau khi đã nối dòng và bỏ chú thích.
struct Instruction {
    keyword: String,
    args: String,
    line: usize,
}

pub fn parse(text: &str) -> Dockerfile {
    let text = crate::strip_bom(text);
    let mut out = Dockerfile::default();
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut held = 0usize;
    // Nhớ một biến nếu còn trong trần [`MAX_VARS`].
    let mut remember = |vars: &mut HashMap<String, String>, k: &str, v: &str| -> bool {
        if held + k.len() + v.len() > MAX_VARS {
            return false;
        }
        held += k.len() + v.len();
        vars.insert(k.to_string(), v.to_string());
        true
    };
    let mut copies = 0usize;
    for ins in instructions(text) {
        let args = ins.args.trim();
        match ins.keyword.as_str() {
            "FROM" => {
                let words = shell_words(args);
                let words: Vec<&String> = words.iter().filter(|w| !w.starts_with("--")).collect();
                let from = words.first().map(|w| expand(w, &vars)).unwrap_or_default();
                let name = match words.as_slice() {
                    [_, kw, name, ..] if kw.eq_ignore_ascii_case("as") => Some((*name).clone()),
                    _ => None,
                };
                let base_workdir = out
                    .stages
                    .iter()
                    .rev()
                    .find(|s| s.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(&from)))
                    .map_or_else(|| "/".to_string(), stage_workdir);
                out.stages.push(Stage {
                    name,
                    from,
                    line: ins.line,
                    workdir: None,
                    base_workdir,
                    copies: Vec::new(),
                    runs: Vec::new(),
                    env: Vec::new(),
                    cmd: None,
                    entrypoint: None,
                    resets_cmd: false,
                });
            }
            "ARG" => {
                for w in shell_words(args) {
                    if let Some((k, v)) = w.split_once('=') {
                        let v = expand(v, &vars);
                        remember(&mut vars, k, &v);
                    }
                }
            }
            _ => {
                // Chỉ thị trước `FROM` đầu tiên (ngoài ARG) không thuộc stage nào.
                let Some(stage) = out.stages.last_mut() else { continue };
                let workdir = || stage_workdir(stage);
                match ins.keyword.as_str() {
                    "WORKDIR" => {
                        let path = expand(&unquote(args), &vars);
                        let joined = join_container(&workdir(), &path);
                        stage.workdir = Some(joined);
                    }
                    "COPY" | "ADD" if copies < MAX_COPIES => {
                        if let Some(copy) = parse_copy(args, &workdir(), &vars, ins.line) {
                            copies += 1;
                            stage.copies.push(copy);
                        }
                    }
                    "RUN" => {
                        let wd = workdir();
                        stage.runs.push(Run { text: args.to_string(), workdir: wd, line: ins.line });
                    }
                    "ENV" => {
                        for (k, v) in parse_env(args) {
                            let v = expand(&v, &vars);
                            if remember(&mut vars, &k, &v) {
                                stage.env.push((k, v));
                            }
                        }
                    }
                    "CMD" => stage.cmd = Some((parse_command(args), ins.line)),
                    "ENTRYPOINT" => {
                        stage.entrypoint = Some((parse_command(args), ins.line));
                        if stage.cmd.is_none() {
                            stage.resets_cmd = true;
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    out
}

fn stage_workdir(stage: &Stage) -> String {
    stage.workdir.clone().unwrap_or_else(|| stage.base_workdir.clone())
}

/// Nối dòng, bỏ chú thích, tách từ khoá. Heredoc (`<<EOF` … `EOF`) được nối
/// vào thân chỉ thị.
fn instructions(text: &str) -> Vec<Instruction> {
    let lines: Vec<&str> = text.lines().map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    // Chỉ thị parser chỉ có ở đầu file, trước mọi dòng khác.
    let mut escape = '\\';
    for line in &lines {
        let t = line.trim();
        let Some(body) = t.strip_prefix('#') else { break };
        let Some((key, value)) = body.split_once('=') else { break };
        let key = key.trim().to_ascii_lowercase();
        if key == "escape" {
            escape = value.trim().chars().next().unwrap_or('\\');
        } else if key != "syntax" && key != "check" {
            break;
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let first = lines[i].trim_start();
        if first.is_empty() || first.starts_with('#') {
            i += 1;
            continue;
        }
        let start = i + 1;
        let mut body = String::new();
        loop {
            let line = lines[i];
            let trimmed = line.trim_end();
            match trimmed.strip_suffix(escape) {
                Some(head) if i + 1 < lines.len() => {
                    body.push_str(head);
                    i += 1;
                    // Dòng chú thích và dòng trống giữa các dòng nối bị bỏ.
                    while i < lines.len() && {
                        let t = lines[i].trim_start();
                        t.is_empty() || t.starts_with('#')
                    } {
                        i += 1;
                    }
                    if i >= lines.len() {
                        break;
                    }
                }
                _ => {
                    body.push_str(trimmed);
                    i += 1;
                    break;
                }
            }
        }
        let body = body.trim_start();
        let (keyword, args) = match body.split_once(char::is_whitespace) {
            Some((k, a)) => (k.to_ascii_uppercase(), a.to_string()),
            None => (body.to_ascii_uppercase(), String::new()),
        };
        let mut args = args;
        // Heredoc: thân nằm ở các dòng sau, tới dòng chỉ có từ kết thúc.
        for word in heredoc_words(&args) {
            let (strip_tabs, word) = word;
            let mut content = String::new();
            while i < lines.len() {
                let l = if strip_tabs { lines[i].trim_start_matches('\t') } else { lines[i] };
                i += 1;
                if l == word {
                    break;
                }
                content.push('\n');
                content.push_str(l);
            }
            args.push_str(&content);
        }
        out.push(Instruction { keyword, args, line: start });
    }
    out
}

/// Từ kết thúc của các heredoc trong một dòng: `<<EOF`, `<<-EOF`, `<<"EOF"`.
fn heredoc_words(args: &str) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    let mut rest = args;
    while let Some(at) = rest.find("<<") {
        let mut tail = &rest[at + 2..];
        let strip = tail.starts_with('-');
        if strip {
            tail = &tail[1..];
        }
        let unquoted = tail.trim_start_matches(['"', '\'']);
        let word: String = unquoted.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !word.is_empty() {
            out.push((strip, word));
        }
        rest = &rest[at + 2..];
    }
    out
}

/// `CMD ["a", "b"]` hay `CMD a b`. JSON hỏng thì Docker coi là dạng shell.
pub fn parse_command(args: &str) -> Command {
    let t = args.trim();
    if t.starts_with('[')
        && let Some(items) = json_strings(t)
    {
        return Command::Exec(items);
    }
    Command::Shell(t.to_string())
}

/// Mảng JSON chỉ gồm chuỗi. `None` nếu không đúng dạng đó.
pub fn json_strings(t: &str) -> Option<Vec<String>> {
    let inner = t.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut out = Vec::new();
    let mut chars = inner.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        match chars.next() {
            None if out.is_empty() => return Some(out),
            Some('"') => {}
            _ => return None,
        }
        let mut s = String::new();
        loop {
            match chars.next()? {
                '"' => break,
                '\\' => match chars.next()? {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    'b' => s.push('\u{08}'),
                    'f' => s.push('\u{0C}'),
                    'u' => {
                        let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                        s.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                    }
                    c => s.push(c),
                },
                c => s.push(c),
            }
        }
        out.push(s);
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        match chars.next() {
            Some(',') => continue,
            None => return Some(out),
            _ => return None,
        }
    }
}

fn parse_copy(args: &str, workdir: &str, vars: &HashMap<String, String>, line: usize) -> Option<Copy> {
    let mut from = None;
    let mut rest: Vec<String> = Vec::new();
    // Thân heredoc được nối vào sau dấu xuống dòng: không phải nguồn hay đích.
    let t = args.split('\n').next().unwrap_or("").trim();
    let words = if t.starts_with('[') {
        // `COPY ["a b", "dest/"]`, có thể có cờ đứng trước — không có ở dạng này.
        json_strings(t)?
    } else {
        shell_words(t)
    };
    for w in words {
        if let Some(flag) = w.strip_prefix("--") {
            if let Some(v) = flag.strip_prefix("from=") {
                from = Some(v.to_string());
            }
            continue;
        }
        rest.push(expand(&w, vars));
    }
    // Heredoc của `COPY <<EOF /đích`: nguồn là nội dung, không phải file.
    rest.retain(|w| !w.starts_with("<<"));
    let dest = rest.pop()?;
    if rest.is_empty() {
        return None;
    }
    let dest_dir = dest.ends_with('/') || rest.len() > 1;
    Some(Copy { from, sources: rest, dest: join_container(workdir, &dest), dest_dir, line })
}

/// `ENV A=1 B="x y"` hay dạng cũ `ENV A 1`.
fn parse_env(args: &str) -> Vec<(String, String)> {
    let words = shell_words(args);
    if words.first().is_some_and(|w| w.contains('=')) {
        words
            .into_iter()
            .filter_map(|w| w.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect()
    } else {
        let mut it = args.trim().splitn(2, char::is_whitespace);
        match (it.next(), it.next()) {
            (Some(k), Some(v)) if !k.is_empty() => vec![(k.to_string(), unquote(v.trim()))],
            _ => Vec::new(),
        }
    }
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    for q in ['"', '\''] {
        if let Some(inner) = s.strip_prefix(q).and_then(|r| r.strip_suffix(q)) {
            return inner.to_string();
        }
    }
    s.to_string()
}

/// Thay `$X`, `${X}`, `${X:-d}` bằng giá trị đã biết. Biến không biết thì
/// giữ nguyên chữ. Kết quả dài tối đa [`MAX_EXPANDED`] byte: chạm trần thì cắt
/// ở đó (`ARG B=$A$A` lặp lại là phình theo luỹ thừa).
pub fn expand(s: &str, vars: &HashMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(at) = rest.find('$') {
        if !push_capped(&mut out, &rest[..at]) {
            return out;
        }
        let tail = &rest[at + 1..];
        if let Some(body) = tail.strip_prefix('{')
            && let Some(end) = body.find('}')
        {
            let inner = &body[..end];
            let (name, default) = match inner.split_once(":-").or_else(|| inner.split_once('-')) {
                Some((n, d)) => (n, Some(d)),
                None => (inner, None),
            };
            let piece = match (vars.get(name), default) {
                (Some(v), _) => v.as_str(),
                (None, Some(d)) => d,
                (None, None) => &rest[at..at + 2 + end + 1],
            };
            if !push_capped(&mut out, piece) {
                return out;
            }
            rest = &body[end + 1..];
            continue;
        }
        let len = tail.find(|c: char| !(c.is_alphanumeric() || c == '_')).unwrap_or(tail.len());
        let name = &tail[..len];
        let piece = match vars.get(name) {
            Some(v) if !name.is_empty() => v.as_str(),
            _ => &rest[at..at + 1 + len],
        };
        if !push_capped(&mut out, piece) {
            return out;
        }
        rest = &tail[len..];
    }
    push_capped(&mut out, rest);
    out
}

/// Nối `s` vào `out` trong trần [`MAX_EXPANDED`]; chạm trần thì cắt ở ranh
/// giới ký tự và trả `false`.
fn push_capped(out: &mut String, s: &str) -> bool {
    let room = MAX_EXPANDED.saturating_sub(out.len());
    if s.len() <= room {
        out.push_str(s);
        return true;
    }
    let mut cut = room;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    out.push_str(&s[..cut]);
    false
}

/// Ghép đường dẫn trong container: tuyệt đối thì giữ, tương đối thì nối vào
/// `base`; chuẩn hoá `.` và `..` (không lên quá `/`).
pub fn join_container(base: &str, path: &str) -> String {
    let joined = if path.starts_with('/') { path.to_string() } else { format!("{base}/{path}") };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Tách từ kiểu shell: khoảng trắng phân cách, nháy đơn và nháy kép gom
/// lại, `\` thoát ký tự sau nó ngoài nháy đơn.
pub fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut has = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    cur.push(c);
                }
            }
            '"' => {
                has = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' if chars.peek().is_some_and(|n| matches!(n, '"' | '\\' | '$' | '`')) => {
                            cur.push(chars.next().unwrap_or('\\'));
                        }
                        c => cur.push(c),
                    }
                }
            }
            '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                    has = true;
                }
            }
            c if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_workdir_copy_cmd() {
        let d = parse(
            "# syntax=docker/dockerfile:1\nARG PY=3.12\nFROM python:${PY}-slim AS base\nWORKDIR /app\nCOPY requirements.txt .\nCOPY src/ ./src/\nENV PYTHONPATH=/app/src\nCMD [\"uvicorn\", \"app.main:app\"]\n\nFROM base\nENTRYPOINT [\"python\", \"-m\", \"worker\"]\n",
        );
        assert_eq!(d.stages.len(), 2);
        assert_eq!(d.stages[0].from, "python:3.12-slim");
        assert_eq!(d.stages[0].name.as_deref(), Some("base"));
        let e = d.effective(1);
        assert_eq!(e.workdir, "/app");
        let inherit = parse("FROM python AS base\nWORKDIR /usr/local/app\nFROM base AS final\nCOPY . .\n");
        assert_eq!(inherit.stages[1].copies[0].dest, "/usr/local/app", "WORKDIR thừa hưởng qua `FROM base`");
        assert_eq!(e.copies[1].dest, "/app/src");
        assert!(e.copies[1].dest_dir);
        assert_eq!(e.cmd, None, "ENTRYPOINT ở stage con xoá CMD thừa hưởng");
        assert_eq!(e.entrypoint.unwrap().0, Command::Exec(vec!["python".into(), "-m".into(), "worker".into()]));
        assert_eq!(e.env, [("PYTHONPATH".to_string(), "/app/src".to_string())]);
        let base = d.effective(0);
        assert_eq!(base.cmd.unwrap(), (Command::Exec(vec!["uvicorn".into(), "app.main:app".into()]), 8));
    }

    #[test]
    fn noi_dong_chu_thich_va_escape() {
        let d = parse("FROM x\nRUN go build \\\n  # chú thích giữa dòng nối\n  -o /bin/app \\\n  ./cmd/app\nCMD python app.py\n");
        assert_eq!(d.stages[0].runs[0].text, "go build   -o /bin/app   ./cmd/app");
        assert_eq!(d.stages[0].runs[0].line, 2);
        assert_eq!(d.stages[0].cmd.as_ref().unwrap().1, 6);
        let w = parse("# escape=`\nFROM x\nRUN a `\n  b\n");
        assert_eq!(w.stages[0].runs[0].text, "a   b");
    }

    #[test]
    fn heredoc_noi_vao_than_chi_thi() {
        let d = parse("FROM x\nRUN <<EOF\ncd /src\ngo build -o /app .\nEOF\nCMD [\"/app\"]\n");
        assert!(d.stages[0].runs[0].text.contains("go build -o /app ."));
        assert_eq!(d.stages[0].cmd.as_ref().unwrap().1, 6);
    }

    /// TM-8: `ARG B=$A$A` lặp lại không làm phình bộ nhớ theo luỹ thừa, và
    /// hàng chục nghìn `ARG` không làm tra biến thành bậc hai.
    #[test]
    fn thay_bien_co_tran() {
        let mut text = String::from("FROM x\nARG V0=aaaaaaaa\n");
        for i in 1..200 {
            text.push_str(&format!("ARG V{i}=$V{p}$V{p}\n", p = i - 1));
        }
        text.push_str("WORKDIR /$V199\n");
        let start = std::time::Instant::now();
        let d = parse(&text);
        assert!(d.stages[0].workdir.as_ref().unwrap().len() <= MAX_EXPANDED + 1);
        let mut many = String::from("FROM x\n");
        for i in 0..70_000 {
            many.push_str(&format!("ARG V{i}=$V0\n"));
        }
        parse(&many);
        // Mỗi `ARG` sát trần 8 KB, 41 000 cái trong 1 MB: từng giữ 4 GB.
        let mut wide = format!("FROM x\nARG V0={}\n", "a".repeat(8300));
        for i in 1..41_000 {
            wide.push_str(&format!("ARG V{i}=$V{p}$V{p}\n", p = i - 1));
        }
        wide.push_str("ENV PYTHONPATH=/app\nWORKDIR /$V40999\n");
        let d = parse(&wide);
        assert!(d.stages[0].workdir.as_ref().unwrap().len() <= MAX_EXPANDED + 1);
        assert!(d.stages[0].env.is_empty(), "quá trần thì biến sau coi như không biết");
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
        // Cắt ở ranh giới ký tự, không panic.
        let vars: HashMap<String, String> = [("A".to_string(), "ệ".repeat(MAX_EXPANDED))].into();
        assert!(expand("x$A", &vars).len() <= MAX_EXPANDED);
    }

    #[test]
    fn json_hong_la_dang_shell() {
        assert_eq!(parse_command("[python, app.py]"), Command::Shell("[python, app.py]".into()));
        assert_eq!(parse_command("[\"a\", \"b\\\"c\"]"), Command::Exec(vec!["a".into(), "b\"c".into()]));
    }

    #[test]
    fn tach_tu_va_bien() {
        assert_eq!(shell_words("a 'b c' \"d \\\"e\" f\\ g"), ["a", "b c", "d \"e", "f g"]);
        let vars = HashMap::from([("A".to_string(), "1".to_string())]);
        assert_eq!(expand("x$A ${A} ${B:-def} ${C} $D", &vars), "x1 1 def ${C} $D");
        assert_eq!(join_container("/app", "../x/./y"), "/x/y");
        assert_eq!(join_container("/app", "/abs"), "/abs");
    }
}
