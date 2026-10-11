//! Notebook Jupyter (`.ipynb`, nbformat 4): mã Python nằm trong ô code của một
//! file JSON.
//!
//! Chỉ ô code là mã. Ô markdown, output và metadata thì không: một câu
//! `import` trong ô markdown là văn bản, trong output là chữ in ra.
//!
//! Mỗi dòng mã được đặt đúng ở dòng của chuỗi JSON chứa nó, nên báo cáo trỏ
//! thẳng vào file `.ipynb`. Jupyter ghi mỗi dòng mã thành một chuỗi trên một
//! dòng riêng, nên với notebook bình thường số dòng là chính xác. File viết
//! trên một dòng thì các dòng mã xếp lần lượt từ đó, và số dòng chỉ gần đúng.
//!
//! Đọc được thì đọc, không thì thôi: JSON hỏng cho ra rỗng chứ không lỗi.
//! Bộ đọc JSON ở đây nhỏ và riêng vì nó phải giữ số dòng, thứ mà
//! [`crate::jsonc`] không giữ.

/// Phần mã Python của notebook, mỗi dòng mã ở đúng dòng của nó trong file.
pub fn code_view(src: &str) -> String {
    let src = src.strip_prefix('\u{feff}').unwrap_or(src);
    let Some(root) = Reader::new(src).document() else { return String::new() };
    if !is_python(&root) {
        return String::new();
    }
    let Some(Json::Arr(cells)) = root.get("cells") else { return String::new() };

    // Nối dần vào một chuỗi: dòng đặt mã chỉ tăng, nên chỉ cần thêm dấu xuống
    // dòng cho tới đúng chỗ. Kích thước đầu ra theo kích thước file, không
    // theo số dòng — một mảng mỗi dòng một phần tử thì một notebook vài MB toàn
    // dòng trống thành hàng trăm MB.
    let mut out = String::new();
    // Dòng đang viết (đếm từ 1). Dòng mã kế tiếp đặt ở dòng sau đó trở đi, nên
    // hai dòng mã không bao giờ chồng nhau, kể cả khi file nằm trên một dòng.
    let mut current = 0usize;
    for cell in cells {
        if !matches!(cell.get("cell_type"), Some(Json::Str(t, _)) if t == "code") {
            continue;
        }
        let code = code_lines(cell.get("source"));
        // IPython bỏ các dòng trống đầu ô trước khi tìm magic của cả ô.
        if code.iter().find(|(_, text)| !text.trim().is_empty()).is_some_and(|(_, first)| !runs_python(first)) {
            continue;
        }
        let mut state = Lexical::default();
        for (line, text) in code {
            let at = line.max(current + 1);
            // Đầu ra đang có `current` dòng; chuỗi rỗng là đang ở dòng 1.
            out.extend(std::iter::repeat_n('\n', at - current.max(1)));
            current = at;
            // Magic của IPython (`%matplotlib`, `!pip install`) không phải
            // Python — nhưng chỉ khi dòng đó mở một câu lệnh mới. Trong chuỗi
            // `"""` thì `%(id)s` là dữ liệu, trong ngoặc thì `% b` là phép
            // chia lấy dư.
            let trimmed = text.trim_start();
            if state.at_statement_start() && (trimmed.starts_with('%') || trimmed.starts_with('!')) {
                continue;
            }
            state.advance(&text);
            out.push_str(&text);
        }
    }
    out
}

/// Ô bắt đầu bằng magic của cả ô (`%%bash`, `%%html`, `%%writefile`) chứa thứ
/// khác Python, hoặc Python không chạy trong notebook. Vài magic giữ nguyên
/// Python: đo giờ, bắt output. Và vài magic chạy chính Python trong tiến trình
/// con (`%%python3`, `%%script python`): bỏ qua chúng là chừa một lối đi tắt
/// khỏi luật.
fn runs_python(first: &str) -> bool {
    const PYTHON: &[&str] = &["time", "timeit", "capture", "prun", "debug"];
    let Some(magic) = first.trim_start().strip_prefix("%%") else { return true };
    let mut words = magic.split_whitespace();
    match words.next().unwrap_or("") {
        name if PYTHON.contains(&name) || name.starts_with("python") => true,
        "script" => script_program(words).is_some_and(|program| program.starts_with("python")),
        _ => false,
    }
}

/// Chương trình mà `%%script [tuỳ chọn] chương_trình` chạy, chỉ lấy tên file:
/// `--bg --out log python3`, `/usr/bin/python3`, `env python3 -u` đều ra
/// `python3`.
fn script_program<'a>(mut words: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    const WITH_VALUE: &[&str] = &["--out", "--err", "--proc"];
    let mut program = loop {
        let word = words.next()?;
        if WITH_VALUE.contains(&word) {
            words.next();
        } else if !word.starts_with('-') {
            break word;
        }
    };
    loop {
        let name = program.rsplit('/').next().unwrap_or(program);
        if name != "env" {
            return Some(name);
        }
        program = words.by_ref().find(|w| !w.starts_with('-'))?;
    }
}

/// Đủ để biết một dòng có mở câu lệnh mới không: đang ở trong chuỗi ba nháy,
/// trong ngoặc, hay sau dấu `\` nối dòng. Không cần hơn thế; phần còn lại là
/// việc của lexer Python.
#[derive(Default)]
struct Lexical {
    triple: Option<&'static [u8]>,
    depth: usize,
    continued: bool,
}

impl Lexical {
    fn at_statement_start(&self) -> bool {
        self.triple.is_none() && self.depth == 0 && !self.continued
    }

    fn advance(&mut self, line: &str) {
        let b = line.as_bytes();
        let mut i = 0;
        while i < b.len() {
            if let Some(quote) = self.triple {
                if b[i..].starts_with(quote) {
                    self.triple = None;
                    i += 3;
                } else {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                continue;
            }
            match b[i] {
                b'#' => break,
                q @ (b'"' | b'\'') => {
                    let triple: &'static [u8] = if q == b'"' { b"\"\"\"" } else { b"'''" };
                    if b[i..].starts_with(triple) {
                        self.triple = Some(triple);
                        i += 3;
                        continue;
                    }
                    // Chuỗi một dòng: tới dấu nháy đóng, bỏ qua `\x`.
                    i += 1;
                    while i < b.len() && b[i] != q {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                b'(' | b'[' | b'{' => {
                    self.depth += 1;
                    i += 1;
                }
                b')' | b']' | b'}' => {
                    self.depth = self.depth.saturating_sub(1);
                    i += 1;
                }
                _ => i += 1,
            }
        }
        self.continued = self.triple.is_none() && line.trim_end().ends_with('\\');
    }
}

/// Các dòng mã của một ô, kèm dòng JSON nơi mỗi dòng bắt đầu. `source` là một
/// mảng chuỗi (mỗi dòng một chuỗi) hoặc một chuỗi duy nhất.
fn code_lines(source: Option<&Json>) -> Vec<(usize, String)> {
    let pieces: Vec<(&str, usize)> = match source {
        Some(Json::Str(s, line)) => vec![(s.as_str(), *line)],
        Some(Json::Arr(items)) => items
            .iter()
            .filter_map(|v| match v {
                Json::Str(s, line) => Some((s.as_str(), *line)),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    let mut out: Vec<(usize, String)> = Vec::new();
    // Dòng mã cuối chưa có `\n`: chuỗi kế tiếp viết tiếp vào nó.
    let mut open = false;
    for (piece, line) in pieces {
        for (k, segment) in piece.split_inclusive('\n').enumerate() {
            let text = segment.strip_suffix('\n').unwrap_or(segment);
            match out.last_mut() {
                Some((_, last)) if open => last.push_str(text),
                _ => out.push((line + k, text.to_string())),
            }
            open = !segment.ends_with('\n');
        }
    }
    out
}

/// Notebook khai ngôn ngữ khác Python (R, Julia) thì không có gì để đọc.
/// Không khai thì coi là Python, như Jupyter.
fn is_python(root: &Json) -> bool {
    let meta = root.get("metadata");
    let declared = [
        meta.and_then(|m| m.get("kernelspec")).and_then(|k| k.get("language")),
        meta.and_then(|m| m.get("language_info")).and_then(|l| l.get("name")),
    ];
    declared.into_iter().flatten().all(|v| matches!(v, Json::Str(s, _) if s.eq_ignore_ascii_case("python")))
}

enum Json {
    Obj(Vec<(String, Json)>),
    Arr(Vec<Json>),
    /// Chuỗi đã giải ký tự thoát, kèm dòng (đếm từ 1) nơi nó mở.
    Str(String, usize),
    /// Số, `true`, `false`, `null`: không cần tới giá trị.
    Other,
}

impl Json {
    fn get(&self, key: &str) -> Option<&Json> {
        match self {
            // Khoá trùng thì lấy khoá cuối, như `json.load` của Python — tức
            // như Jupyter. Lấy khoá đầu là chừa một lối giấu ô code.
            Json::Obj(fields) => fields.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// Đủ sâu cho mọi notebook thật, đủ nông để một file `[[[[…` không làm tràn
/// stack: archctl đọc file của repo người khác.
const MAX_DEPTH: usize = 64;

struct Reader<'a> {
    src: &'a [u8],
    at: usize,
    line: usize,
}

impl<'a> Reader<'a> {
    fn new(src: &'a str) -> Self {
        Reader { src: src.as_bytes(), at: 0, line: 1 }
    }

    /// Cả file là đúng một giá trị JSON.
    fn document(mut self) -> Option<Json> {
        let value = self.value(0)?;
        self.space();
        (self.at == self.src.len()).then_some(value)
    }

    fn space(&mut self) {
        while let Some(&b) = self.src.get(self.at) {
            match b {
                b'\n' => self.line += 1,
                b' ' | b'\t' | b'\r' => {}
                _ => return,
            }
            self.at += 1;
        }
    }

    fn eat(&mut self, b: u8) -> bool {
        self.space();
        let hit = self.src.get(self.at) == Some(&b);
        if hit {
            self.at += 1;
        }
        hit
    }

    fn value(&mut self, depth: usize) -> Option<Json> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.space();
        match *self.src.get(self.at)? {
            b'{' => {
                self.at += 1;
                let mut fields = Vec::new();
                if self.eat(b'}') {
                    return Some(Json::Obj(fields));
                }
                loop {
                    self.space();
                    let Json::Str(key, _) = self.string()? else { return None };
                    if !self.eat(b':') {
                        return None;
                    }
                    fields.push((key, self.value(depth + 1)?));
                    if self.eat(b'}') {
                        return Some(Json::Obj(fields));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'[' => {
                self.at += 1;
                let mut items = Vec::new();
                if self.eat(b']') {
                    return Some(Json::Arr(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    if self.eat(b']') {
                        return Some(Json::Arr(items));
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            b'"' => self.string(),
            _ => {
                let start = self.at;
                while self.src.get(self.at).is_some_and(|b| b.is_ascii_alphanumeric() || b"+-.".contains(b)) {
                    self.at += 1;
                }
                (self.at > start).then_some(Json::Other)
            }
        }
    }

    fn string(&mut self) -> Option<Json> {
        if self.src.get(self.at) != Some(&b'"') {
            return None;
        }
        let line = self.line;
        self.at += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let b = *self.src.get(self.at)?;
            self.at += 1;
            match b {
                b'"' => break,
                b'\\' => {
                    let e = *self.src.get(self.at)?;
                    self.at += 1;
                    match e {
                        b'"' | b'\\' | b'/' => out.push(e),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let c = self.unicode()?;
                            out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        _ => return None,
                    }
                }
                b'\n' => {
                    // JSON không cho xuống dòng thật trong chuỗi; vẫn đếm để
                    // số dòng phía sau không lệch.
                    self.line += 1;
                    out.push(b);
                }
                _ => out.push(b),
            }
        }
        Some(Json::Str(String::from_utf8(out).ok()?, line))
    }

    /// `\uXXXX`, kể cả cặp surrogate cho ký tự ngoài BMP.
    fn unicode(&mut self) -> Option<char> {
        let hi = self.hex4()?;
        if (0xD800..0xDC00).contains(&hi) {
            if self.src.get(self.at..self.at + 2) != Some(b"\\u") {
                return None;
            }
            self.at += 2;
            let lo = self.hex4()?;
            if !(0xDC00..0xE000).contains(&lo) {
                return None;
            }
            return char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00));
        }
        char::from_u32(hi)
    }

    fn hex4(&mut self) -> Option<u32> {
        let digits = self.src.get(self.at..self.at + 4)?;
        if !digits.iter().all(u8::is_ascii_hexdigit) {
            return None; // `from_str_radix` còn nhận cả dấu `+`
        }
        let n = u32::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
        self.at += 4;
        Some(n)
    }
}
