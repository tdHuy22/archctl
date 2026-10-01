//! Trích `import` từ mã TypeScript và JavaScript.
//!
//! Không dùng parser đầy đủ. Mục tiêu hẹp: tìm các chuỗi định danh module ở
//! mức câu lệnh. Cách làm là lex thành token — từ, chuỗi, dấu — trong lúc bỏ
//! qua chú thích, chuỗi và regex, rồi khớp vài khuôn mẫu trên dòng token.
//!
//! Giới hạn đã biết: regex được nhận bằng suy đoán theo token đứng trước, như
//! cách các bộ minify làm. Biểu thức trong template literal bị bỏ qua, nên
//! `import` nằm trong đó sẽ không được thấy — việc này không xảy ra trong mã
//! thật. Nếu độ chính xác không đủ, thay module này bằng tree-sitter.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// Chuỗi trong câu lệnh import, chưa phân giải.
    pub spec: String,
    pub line: usize,
}

/// Phần mở rộng mà bộ quét này hiểu.
pub const EXTENSIONS: &[&str] = &["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

pub fn is_supported(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| EXTENSIONS.contains(&ext))
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Word(String),
    Str(String),
    Punct(char),
}

struct Token {
    tok: Tok,
    line: usize,
}

/// Token đứng trước, để đoán `/` là mở đầu regex hay phép chia.
enum Prev {
    Nothing,
    Punct(char),
    Word(String),
}

/// Sau các từ khoá này, `/` chắc chắn mở đầu một regex chứ không phải phép chia.
const REGEX_AFTER_WORD: &[&str] = &[
    "return", "typeof", "case", "in", "of", "new", "delete", "void", "throw", "yield", "do",
    "else", "await",
];

pub fn imports(src: &str) -> Vec<Import> {
    extract(&lex(src))
}

fn lex(src: &str) -> Vec<Token> {
    let cs: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1;
    let mut prev = Prev::Nothing;

    while i < cs.len() {
        let c = cs[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && cs.get(i + 1) == Some(&'/') {
            while i < cs.len() && cs[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && cs.get(i + 1) == Some(&'*') {
            i += 2;
            while i < cs.len() && !(cs[i] == '*' && cs.get(i + 1) == Some(&'/')) {
                if cs[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 2).min(cs.len());
            continue;
        }
        if c == '/' && regex_allowed(&prev) {
            i = skip_regex(&cs, i, &mut line);
            prev = Prev::Punct('/');
            continue;
        }
        if c == '"' || c == '\'' {
            let (value, next) = read_quoted(&cs, i + 1, c, &mut line);
            out.push(Token { tok: Tok::Str(value), line });
            i = next;
            prev = Prev::Punct('"');
            continue;
        }
        if c == '`' {
            i = skip_template(&cs, i + 1, &mut line);
            prev = Prev::Punct('`');
            continue;
        }
        if is_ident_start(c) {
            let start = i;
            while i < cs.len() && is_ident_part(cs[i]) {
                i += 1;
            }
            let word: String = cs[start..i].iter().collect();
            out.push(Token { tok: Tok::Word(word.clone()), line });
            prev = Prev::Word(word);
            continue;
        }
        out.push(Token { tok: Tok::Punct(c), line });
        prev = Prev::Punct(c);
        i += 1;
    }
    out
}

fn regex_allowed(prev: &Prev) -> bool {
    match prev {
        Prev::Nothing => true,
        Prev::Word(w) => REGEX_AFTER_WORD.contains(&w.as_str()),
        Prev::Punct(c) => "(,=:[!&|?{};+-*%~^<>".contains(*c),
    }
}

fn skip_regex(cs: &[char], mut i: usize, line: &mut usize) -> usize {
    i += 1; // dấu `/` mở đầu
    let mut in_class = false;
    while i < cs.len() {
        match cs[i] {
            '\\' => i += 1,
            '\n' => {
                // Regex không bắc qua dòng: đây hoá ra là phép chia, bỏ qua.
                *line += 1;
                return i + 1;
            }
            '[' => in_class = true,
            ']' => in_class = false,
            '/' if !in_class => return i + 1,
            _ => {}
        }
        i += 1;
    }
    i
}

fn read_quoted(cs: &[char], mut i: usize, quote: char, line: &mut usize) -> (String, usize) {
    let mut value = String::new();
    while i < cs.len() {
        match cs[i] {
            '\\' => {
                if let Some(&next) = cs.get(i + 1) {
                    value.push(next);
                    i += 2;
                    continue;
                }
                i += 1;
            }
            '\n' => {
                *line += 1;
                i += 1;
            }
            c if c == quote => return (value, i + 1),
            c => {
                value.push(c);
                i += 1;
            }
        }
    }
    (value, i)
}

/// Bỏ qua template literal, kể cả biểu thức `${...}` lồng trong đó.
fn skip_template(cs: &[char], mut i: usize, line: &mut usize) -> usize {
    while i < cs.len() {
        match cs[i] {
            '\\' => i += 2,
            '\n' => {
                *line += 1;
                i += 1;
            }
            '`' => return i + 1,
            '$' if cs.get(i + 1) == Some(&'{') => {
                let mut depth = 1;
                i += 2;
                while i < cs.len() && depth > 0 {
                    match cs[i] {
                        '\n' => *line += 1,
                        '{' => depth += 1,
                        '}' => depth -= 1,
                        '"' | '\'' => {
                            let quote = cs[i];
                            let (_, next) = read_quoted(cs, i + 1, quote, line);
                            i = next;
                            continue;
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    i
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Từ khoá mở đầu câu lệnh mới — gặp phải thì dừng việc tìm `from`.
const STATEMENT_START: &[&str] =
    &["import", "export", "const", "let", "var", "function", "class", "return", "type"];

fn extract(toks: &[Token]) -> Vec<Import> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let Tok::Word(word) = &toks[i].tok else {
            i += 1;
            continue;
        };
        match word.as_str() {
            // `import 'x'` · `import('x')` · `import ... from 'x'`
            "import" => match toks.get(i + 1).map(|t| &t.tok) {
                Some(Tok::Str(spec)) => {
                    push(&mut out, spec, toks[i + 1].line);
                    i += 2;
                }
                Some(Tok::Punct('(')) => {
                    if let Some(t) = toks.get(i + 2)
                        && let Tok::Str(spec) = &t.tok
                    {
                        push(&mut out, spec, t.line);
                    }
                    i += 2;
                }
                _ => i = scan_from(toks, i + 1, &mut out),
            },
            // `export { a } from 'x'` · `export * from 'x'`
            "export" => i = scan_from(toks, i + 1, &mut out),
            // `require('x')`
            "require" if toks.get(i + 1).map(|t| &t.tok) == Some(&Tok::Punct('(')) => {
                if let Some(t) = toks.get(i + 2)
                    && let Tok::Str(spec) = &t.tok
                {
                    push(&mut out, spec, t.line);
                }
                i += 3;
            }
            _ => i += 1,
        }
    }
    out
}

/// Tìm `from '...'` từ vị trí `start`, dừng ở dấu `;` hoặc câu lệnh mới.
///
/// `type` ngay đầu là của `import type {T} from 'x'` và `export type {T} from
/// 'x'`, nên phải bỏ qua — ở vị trí khác nó mở đầu khai báo `type X = ...` và
/// là mốc dừng.
fn scan_from(toks: &[Token], start: usize, out: &mut Vec<Import>) -> usize {
    let mut i = match toks.get(start).map(|t| &t.tok) {
        Some(Tok::Word(w)) if w == "type" => start + 1,
        _ => start,
    };
    while i < toks.len() {
        match &toks[i].tok {
            Tok::Punct(';') => return i + 1,
            Tok::Word(w) if w == "from" => {
                if let Some(t) = toks.get(i + 1)
                    && let Tok::Str(spec) = &t.tok
                {
                    push(out, spec, t.line);
                    return i + 2;
                }
                return i + 1;
            }
            Tok::Word(w) if STATEMENT_START.contains(&w.as_str()) => return i,
            _ => i += 1,
        }
    }
    i
}

fn push(out: &mut Vec<Import>, spec: &str, line: usize) {
    if !spec.is_empty() {
        out.push(Import { spec: spec.to_string(), line });
    }
}
