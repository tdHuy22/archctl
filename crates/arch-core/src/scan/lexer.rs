//! Lexer dùng chung cho mọi ngôn ngữ.
//!
//! Việc khó khi trích import không phải là tìm từ khoá, mà là **không tìm thấy
//! nó ở chỗ không nên tìm**: trong chú thích, trong chuỗi, trong regex. Phần đó
//! giống nhau giữa các ngôn ngữ, chỉ khác cú pháp, nên viết một lần rồi cấu
//! hình cho từng ngôn ngữ.
//!
//! Đổi lại, lexer này không hiểu ngữ pháp. Nó cho ra dòng token thô — từ,
//! chuỗi, dấu — và mỗi ngôn ngữ tự khớp khuôn mẫu import trên dòng đó.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Word(String),
    Str(String),
    /// `<stdio.h>` trong chỉ thị `#include` của C/C++.
    SystemStr(String),
    Punct(char),
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: usize,
}

/// Cách một ngôn ngữ viết char literal. Quan trọng vì `'` vừa có thể mở char
/// literal vừa có thể mở chuỗi, và trong Rust còn là lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharLit {
    /// `'` mở chuỗi, như Python và JavaScript.
    AsString,
    /// `'a'`, `'\n'` — luôn đóng lại.
    Quoted,
    /// Như trên nhưng `'a` không đóng là lifetime, không phải chuỗi.
    QuotedOrLifetime,
}

/// Dấu backtick nghĩa là gì.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backtick {
    /// Không nghĩa gì: Python, C, Rust. Coi nó là mở chuỗi thì một dấu lạc
    /// nuốt cả phần sau của file.
    Punct,
    /// Template của JavaScript: có ký tự thoát và nội suy `${...}`.
    Template,
    /// Chuỗi thô của Go: không thoát, không nội suy, đóng ở backtick kế tiếp.
    Raw,
}

#[derive(Debug, Clone)]
pub struct LexConfig {
    pub line_comments: &'static [&'static str],
    pub block_comment: Option<(&'static str, &'static str)>,
    /// Rust cho phép `/* /* */ */`.
    pub nested_block_comment: bool,
    /// Python `'''` và `"""`.
    pub triple_quoted: bool,
    /// Rust `r#"..."#`.
    pub raw_hash_strings: bool,
    pub char_literals: CharLit,
    /// Chỉ JavaScript và TypeScript.
    pub regex_literals: bool,
    /// Gộp `a.b.c` hoặc `a::b::c` thành một từ, để đường dẫn module không bị
    /// vỡ thành nhiều token.
    pub path_separator: Option<&'static str>,
    /// Nhận `#include "x"` và `#include <x>` của C/C++.
    pub preprocessor_includes: bool,
    /// Chuỗi trong nháy kép được bắc qua dòng mà không cần `\`. Chỉ Rust.
    pub multiline_strings: bool,
    /// `1'000'000`, `0xFFFF'FFFF` của C++14 và C23: `'` giữa các chữ số là
    /// dấu phân cách, không mở char literal.
    pub digit_separators: bool,
    /// Chuỗi thô C++11 `R"delim(...)delim"`.
    pub cpp_raw_strings: bool,
    pub backtick: Backtick,
    /// f-string và t-string của Python: `{...}` bên trong là mã.
    pub fstrings: bool,
}

impl LexConfig {
    pub const fn base() -> Self {
        LexConfig {
            line_comments: &["//"],
            block_comment: Some(("/*", "*/")),
            nested_block_comment: false,
            triple_quoted: false,
            raw_hash_strings: false,
            char_literals: CharLit::Quoted,
            regex_literals: false,
            path_separator: None,
            preprocessor_includes: false,
            multiline_strings: false,
            digit_separators: false,
            cpp_raw_strings: false,
            backtick: Backtick::Punct,
            fstrings: false,
        }
    }
}

/// Sau các từ khoá này, `/` chắc chắn mở đầu regex chứ không phải phép chia.
const REGEX_AFTER_WORD: &[&str] = &[
    "return", "typeof", "case", "in", "of", "new", "delete", "void", "throw", "yield", "do",
    "else", "await",
];

enum Prev {
    Nothing,
    Punct(char),
    Word(String),
}

struct Lexer<'a> {
    cs: Vec<char>,
    i: usize,
    line: usize,
    /// Đúng khi từ đầu dòng tới đây chỉ có khoảng trắng — cần cho `#include`.
    at_line_start: bool,
    cfg: &'a LexConfig,
    prev: Prev,
    /// Vị trí ngay sau từ vừa đọc. `f"..."` là f-string chỉ khi `f` liền nháy.
    word_end: usize,
    out: Vec<Token>,
}

pub fn lex(src: &str, cfg: &LexConfig) -> Vec<Token> {
    let mut lx = Lexer {
        cs: src.chars().collect(),
        i: 0,
        line: 1,
        at_line_start: true,
        cfg,
        prev: Prev::Nothing,
        word_end: usize::MAX,
        out: Vec::new(),
    };
    lx.run();
    lx.out
}

impl Lexer<'_> {
    fn run(&mut self) {
        while self.i < self.cs.len() {
            let c = self.cs[self.i];

            if c == '\n' {
                self.line += 1;
                self.i += 1;
                self.at_line_start = true;
                continue;
            }
            // BOM không phải khoảng trắng theo Unicode, nhưng cũng không phải
            // mã: coi nó là token thì `#` ở dòng 1 hết "ở đầu dòng".
            if c.is_whitespace() || c == '\u{feff}' {
                self.i += 1;
                continue;
            }
            if self.skip_comment() {
                continue;
            }
            if self.cfg.preprocessor_includes && self.at_line_start && c == '#' && self.include() {
                continue;
            }
            if self.cfg.regex_literals && c == '/' && self.regex_allowed() {
                self.skip_regex();
                self.step(Prev::Punct('/'));
                continue;
            }
            if self.cfg.raw_hash_strings && matches!(c, 'r' | 'b' | 'c') && self.raw_string() {
                continue;
            }
            if c == '"' || (c == '\'' && self.cfg.char_literals == CharLit::AsString) {
                if self.cfg.fstrings && self.word_end == self.i && self.after_fstring_prefix() {
                    self.fstring(c);
                } else {
                    self.quoted(c);
                }
                continue;
            }
            if c == '\'' {
                self.char_or_lifetime();
                continue;
            }
            if c == '`' && self.cfg.backtick == Backtick::Template {
                self.template();
                continue;
            }
            if c == '`' && self.cfg.backtick == Backtick::Raw {
                self.raw_backtick();
                continue;
            }
            if self.cfg.digit_separators && c.is_ascii_digit() {
                self.number();
                continue;
            }
            if self.cfg.cpp_raw_strings && matches!(c, 'R' | 'L' | 'u' | 'U') && self.cpp_raw_string()
            {
                continue;
            }
            if is_ident_start(c) {
                self.word();
                continue;
            }
            self.out.push(Token { tok: Tok::Punct(c), line: self.line });
            self.i += 1;
            self.step(Prev::Punct(c));
        }
    }

    fn step(&mut self, prev: Prev) {
        self.prev = prev;
        self.at_line_start = false;
    }

    fn rest_starts_with(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(k, c)| self.cs.get(self.i + k) == Some(&c))
    }

    fn skip_comment(&mut self) -> bool {
        // So ký tự đầu trước khi so cả chuỗi: hàm này chạy ở mọi ký tự của mọi
        // file, nên một phép so sánh thừa ở đây là đáng kể.
        let c = self.cs[self.i];
        for marker in self.cfg.line_comments {
            if marker.starts_with(c) && self.rest_starts_with(marker) {
                while self.i < self.cs.len() && self.cs[self.i] != '\n' {
                    self.i += 1;
                }
                return true;
            }
        }
        let Some((open, close)) = self.cfg.block_comment else {
            return false;
        };
        if !open.starts_with(c) || !self.rest_starts_with(open) {
            return false;
        }
        self.i += open.chars().count();
        let mut depth = 1usize;
        while self.i < self.cs.len() && depth > 0 {
            if self.cs[self.i] == '\n' {
                self.line += 1;
            }
            if self.rest_starts_with(close) {
                depth -= 1;
                self.i += close.chars().count();
                continue;
            }
            if self.cfg.nested_block_comment && self.rest_starts_with(open) {
                depth += 1;
                self.i += open.chars().count();
                continue;
            }
            self.i += 1;
        }
        true
    }

    /// `#include "x.h"` hoặc `#include <x.h>`. Chỉ thị khác thì bỏ qua trọn
    /// chỉ thị để `#if`, `#define` không sinh token rác.
    fn include(&mut self) -> bool {
        let line_end =
            self.cs[self.i..].iter().position(|&c| c == '\n').map_or(self.cs.len(), |p| self.i + p);
        let text: String = self.cs[self.i..line_end].iter().collect();
        let body = text.trim_start_matches('#').trim_start();

        if let Some(rest) = body.strip_prefix("include") {
            let rest = rest.trim_start();
            let parsed = rest
                .strip_prefix('"')
                .and_then(|r| r.split_once('"'))
                .map(|(spec, _)| Tok::Str(spec.to_string()))
                .or_else(|| {
                    rest.strip_prefix('<')
                        .and_then(|r| r.split_once('>'))
                        .map(|(spec, _)| Tok::SystemStr(spec.to_string()))
                });
            if let Some(tok) = parsed {
                self.out.push(Token { tok: Tok::Word("include".into()), line: self.line });
                self.out.push(Token { tok, line: self.line });
            }
        }
        self.i += 1;
        self.skip_directive();
        true
    }

    /// Đi tới hết chỉ thị tiền xử lý đang đứng, dừng ngay trước `\n` kết thúc
    /// để vòng ngoài đếm dòng như mọi dòng khác.
    ///
    /// "Hết" là hết dòng nối cuối cùng: `#error "... \` hay một macro nhiều
    /// dòng kéo sang dòng sau, và phần đó không phải mã. Chú thích được bỏ qua
    /// như chú thích — một `/*` mở trên dòng này có thể đóng ở dòng sau — còn
    /// chuỗi được đọc như chuỗi, để `#define S "/*"` không mở chú thích nào.
    fn skip_directive(&mut self) {
        while self.i < self.cs.len() {
            if self.skip_comment() {
                continue;
            }
            match self.cs[self.i] {
                '\n' => return,
                '\\' if self.cs.get(self.i + 1) == Some(&'\n') => {
                    self.line += 1;
                    self.i += 2;
                }
                '\\' if self.rest_starts_with_at(self.i + 1, "\r\n") => {
                    self.line += 1;
                    self.i += 3;
                }
                q @ ('"' | '\'') => {
                    self.read_until(q);
                }
                _ => self.i += 1,
            }
        }
    }

    fn rest_starts_with_at(&self, at: usize, s: &str) -> bool {
        s.chars().enumerate().all(|(k, c)| self.cs.get(at + k) == Some(&c))
    }

    fn regex_allowed(&self) -> bool {
        match &self.prev {
            Prev::Nothing => true,
            Prev::Word(w) => REGEX_AFTER_WORD.contains(&w.as_str()),
            Prev::Punct(c) => "(,=:[!&|?{};+-*%~^<>".contains(*c),
        }
    }

    fn skip_regex(&mut self) {
        self.i += 1;
        let mut in_class = false;
        while self.i < self.cs.len() {
            match self.cs[self.i] {
                // Regex không bắc qua dòng, kể cả bằng `\`: để vòng ngoài đếm
                // dòng đó.
                '\\' if self.cs.get(self.i + 1) == Some(&'\n') => return,
                '\\' => self.i += 1,
                // Regex không bắc qua dòng: hoá ra đây là phép chia.
                '\n' => return,
                '[' => in_class = true,
                ']' => in_class = false,
                '/' if !in_class => {
                    self.i += 1;
                    return;
                }
                _ => {}
            }
            self.i += 1;
        }
    }

    /// Rust `r"..."`, `r#"..."#`, `r##"..."##`.
    fn raw_string(&mut self) -> bool {
        // Bản byte và bản C cũng thô: `br"..."`, `cr#"..."#`.
        let mut k = self.i;
        if matches!(self.cs[k], 'b' | 'c') {
            k += 1;
        }
        if self.cs.get(k) != Some(&'r') {
            return false;
        }
        k += 1;
        let mut hashes = 0;
        while self.cs.get(k) == Some(&'#') {
            hashes += 1;
            k += 1;
        }
        if self.cs.get(k) != Some(&'"') {
            return false;
        }
        let close: String = std::iter::once('"').chain(std::iter::repeat_n('#', hashes)).collect();
        self.i = k + 1;
        let start = self.i;
        while self.i < self.cs.len() && !self.rest_starts_with(&close) {
            if self.cs[self.i] == '\n' {
                self.line += 1;
            }
            self.i += 1;
        }
        let value: String = self.cs[start..self.i.min(self.cs.len())].iter().collect();
        self.i = (self.i + close.chars().count()).min(self.cs.len());
        self.out.push(Token { tok: Tok::Str(value), line: self.line });
        self.step(Prev::Punct('"'));
        true
    }

    fn quoted(&mut self, quote: char) {
        if self.cfg.triple_quoted && self.opens_triple(quote) {
            self.skip_triple(quote);
            self.out.push(Token { tok: Tok::Str(String::new()), line: self.line });
            self.step(Prev::Punct('"'));
            return;
        }
        let line = self.line;
        let value = self.read_until(quote);
        self.out.push(Token { tok: Tok::Str(value), line });
        self.step(Prev::Punct('"'));
    }

    /// Đọc tới dấu nháy đóng.
    ///
    /// Chỉ chuỗi của Rust được bắc qua dòng. Ở mọi ngôn ngữ khác, và với char
    /// literal ở mọi ngôn ngữ, gặp cuối dòng là dừng: dấu nháy ở đó là một dấu
    /// nháy lạc — chữ trong JSX, `0xFFFF'FFFF` của C++14 — và đọc tiếp thì nó
    /// nuốt hoặc lộn ngược cả phần còn lại của file.
    fn opens_triple(&self, quote: char) -> bool {
        self.cs.get(self.i + 1) == Some(&quote) && self.cs.get(self.i + 2) == Some(&quote)
    }

    /// Chuỗi ba nháy, từ ba nháy mở tới hết ba nháy đóng.
    fn skip_triple(&mut self, quote: char) {
        let close: String = std::iter::repeat_n(quote, 3).collect();
        self.i += 3;
        while self.i < self.cs.len() && !self.rest_starts_with(&close) {
            match self.cs[self.i] {
                // `\"` không đóng chuỗi, kể cả chuỗi thô của Python.
                '\\' => {
                    if self.cs.get(self.i + 1) == Some(&'\n') {
                        self.line += 1;
                    }
                    self.i += 2;
                    continue;
                }
                '\n' => self.line += 1,
                _ => {}
            }
            self.i += 1;
        }
        self.i = (self.i + 3).min(self.cs.len());
    }

    /// Từ vừa đọc là tiền tố của f-string hay t-string: `f`, `rf`, `fr`, `t`,
    /// `rt`, `tr`, hoa thường tuỳ ý.
    fn after_fstring_prefix(&self) -> bool {
        matches!(&self.prev, Prev::Word(w) if is_fstring_prefix(w))
    }

    /// f-string (PEP 701, Python 3.12) và t-string (PEP 750, 3.14).
    ///
    /// Biểu thức trong `{...}` là mã thật: được chứa chính loại nháy bọc
    /// ngoài, chuỗi ba nháy, f-string lồng, được xuống dòng và có chú thích.
    /// Đọc như chuỗi thường thì một nháy bên trong đóng chuỗi sớm, và một
    /// chuỗi ba nháy lạc nuốt phần sau của file.
    fn fstring(&mut self, quote: char) {
        let line = self.line;
        let triple = self.opens_triple(quote);
        self.i += if triple { 3 } else { 1 };
        let mut value = String::new();
        self.fstring_body(quote, triple, &mut value);
        // Như chuỗi thường: chuỗi ba nháy không giữ nội dung.
        let (value, line) = if triple { (String::new(), self.line) } else { (value, line) };
        self.out.push(Token { tok: Tok::Str(value), line });
        self.step(Prev::Punct('"'));
    }

    fn closes(&self, quote: char, triple: bool) -> bool {
        self.cs.get(self.i) == Some(&quote) && (!triple || self.opens_triple(quote))
    }

    /// Phần chữ của f-string, tới hết nháy đóng. f-string một nháy gặp cuối
    /// dòng ở phần chữ thì dừng, không ăn `\n`, như mọi chuỗi một dòng.
    fn fstring_body(&mut self, quote: char, triple: bool, value: &mut String) {
        while self.i < self.cs.len() {
            if self.closes(quote, triple) {
                self.i += if triple { 3 } else { 1 };
                return;
            }
            let c = self.cs[self.i];
            match c {
                '\\' => {
                    if let Some(&next) = self.cs.get(self.i + 1) {
                        value.push(next);
                        if next == '\n' {
                            self.line += 1;
                        }
                    }
                    self.i += 2;
                }
                '\n' if !triple => return,
                '\n' => {
                    self.line += 1;
                    self.i += 1;
                }
                '{' | '}' if self.cs.get(self.i + 1) == Some(&c) => {
                    value.push(c);
                    self.i += 2;
                }
                '{' => {
                    self.i += 1;
                    self.fstring_field(quote, triple);
                }
                _ => {
                    value.push(c);
                    self.i += 1;
                }
            }
        }
    }

    /// Một trường `{...}`, từ sau `{` tới hết `}` đóng nó. Đây là mã: ngoặc
    /// lồng nhau, chuỗi, chú thích. `:` ở tầng ngoài cùng mở phần định dạng.
    fn fstring_field(&mut self, quote: char, triple: bool) {
        let mut depth = 0usize;
        while self.i < self.cs.len() {
            match self.cs[self.i] {
                '\n' => {
                    self.line += 1;
                    self.i += 1;
                }
                '#' => {
                    while self.i < self.cs.len() && self.cs[self.i] != '\n' {
                        self.i += 1;
                    }
                }
                '(' | '[' | '{' => {
                    depth += 1;
                    self.i += 1;
                }
                ')' | ']' => {
                    depth = depth.saturating_sub(1);
                    self.i += 1;
                }
                '}' if depth == 0 => {
                    self.i += 1;
                    return;
                }
                '}' => {
                    depth -= 1;
                    self.i += 1;
                }
                ':' if depth == 0 => {
                    self.i += 1;
                    self.fstring_spec(quote, triple);
                    return;
                }
                '"' | '\'' => self.nested_string(),
                _ => self.i += 1,
            }
        }
    }

    /// Phần định dạng sau `:`, tới hết `}` đóng trường. Đây lại là chữ:
    /// `f"{x:'>10}"` lấp đầy bằng dấu nháy, và `{...}` lồng là một trường.
    /// Gặp nháy đóng của f-string bọc ngoài thì để nó đóng.
    fn fstring_spec(&mut self, quote: char, triple: bool) {
        while self.i < self.cs.len() && !self.closes(quote, triple) {
            match self.cs[self.i] {
                '{' => {
                    self.i += 1;
                    self.fstring_field(quote, triple);
                }
                '}' => {
                    self.i += 1;
                    return;
                }
                '\n' if !triple => return,
                '\n' => {
                    self.line += 1;
                    self.i += 1;
                }
                '\\' => {
                    if self.cs.get(self.i + 1) == Some(&'\n') {
                        self.line += 1;
                    }
                    self.i += 2;
                }
                _ => self.i += 1,
            }
        }
    }

    /// Một chuỗi nằm trong biểu thức của trường f-string. Nó có thể chính là
    /// một f-string, nhận ra bằng tiền tố đứng liền trước dấu nháy.
    fn nested_string(&mut self) {
        let quote = self.cs[self.i];
        let mut k = self.i;
        while k > 0 && self.i - k < 2 && self.cs[k - 1].is_ascii_alphabetic() {
            k -= 1;
        }
        let prefix: String = self.cs[k..self.i].iter().collect();
        let boundary = k == 0 || !is_ident_part(self.cs[k - 1]);
        let triple = self.opens_triple(quote);
        if boundary && is_fstring_prefix(&prefix) {
            self.i += if triple { 3 } else { 1 };
            self.fstring_body(quote, triple, &mut String::new());
        } else if triple {
            self.skip_triple(quote);
        } else {
            self.read_until(quote);
        }
    }

    fn read_until(&mut self, quote: char) -> String {
        let multiline = self.cfg.multiline_strings && quote == '"';
        self.i += 1;
        let mut value = String::new();
        while self.i < self.cs.len() {
            match self.cs[self.i] {
                '\\' => {
                    // `\` cuối dòng nối chuỗi sang dòng sau, kể cả khi file
                    // xuống dòng kiểu Windows. Dòng đó vẫn phải được đếm,
                    // không thì mọi token phía sau lệch số dòng.
                    let crlf = self.cs.get(self.i + 1) == Some(&'\r')
                        && self.cs.get(self.i + 2) == Some(&'\n');
                    if crlf {
                        self.i += 1;
                    }
                    if let Some(&next) = self.cs.get(self.i + 1) {
                        // Cặp `\` và xuống dòng biến mất khỏi giá trị, như
                        // trình biên dịch C, Python và JS: `"eig\⏎ht"` là
                        // `"eight"`.
                        if next == '\n' {
                            self.line += 1;
                        } else {
                            value.push(next);
                        }
                    }
                    self.i += 2;
                }
                '\n' if !multiline => return value,
                '\n' => {
                    self.line += 1;
                    self.i += 1;
                }
                c if c == quote => {
                    self.i += 1;
                    return value;
                }
                c => {
                    value.push(c);
                    self.i += 1;
                }
            }
        }
        value
    }

    /// `'a'` là char literal; `'a` không đóng trong Rust là lifetime, và phải
    /// được bỏ qua chứ không nuốt phần còn lại của file làm chuỗi.
    fn char_or_lifetime(&mut self) {
        if self.cfg.char_literals == CharLit::QuotedOrLifetime && !self.looks_like_char() {
            self.i += 1;
            self.step(Prev::Punct('\''));
            return;
        }
        self.read_until('\'');
        self.step(Prev::Punct('\''));
    }

    fn looks_like_char(&self) -> bool {
        match self.cs.get(self.i + 1) {
            Some('\\') => true,
            Some(_) => self.cs.get(self.i + 2) == Some(&'\''),
            None => false,
        }
    }

    fn template(&mut self) {
        self.i += 1;
        while self.i < self.cs.len() {
            match self.cs[self.i] {
                '\\' => {
                    if self.cs.get(self.i + 1) == Some(&'\n') {
                        self.line += 1;
                    }
                    self.i += 2;
                }
                '\n' => {
                    self.line += 1;
                    self.i += 1;
                }
                '`' => {
                    self.i += 1;
                    break;
                }
                '$' if self.cs.get(self.i + 1) == Some(&'{') => self.skip_interpolation(),
                _ => self.i += 1,
            }
        }
        self.step(Prev::Punct('`'));
    }

    /// Chuỗi thô của Go. Được bắc qua dòng; `\` và `${` chỉ là chữ.
    fn raw_backtick(&mut self) {
        let line = self.line;
        self.i += 1;
        let start = self.i;
        while self.i < self.cs.len() && self.cs[self.i] != '`' {
            if self.cs[self.i] == '\n' {
                self.line += 1;
            }
            self.i += 1;
        }
        let value: String = self.cs[start..self.i].iter().collect();
        self.i = (self.i + 1).min(self.cs.len());
        self.out.push(Token { tok: Tok::Str(value), line });
        self.step(Prev::Punct('`'));
    }

    fn skip_interpolation(&mut self) {
        let mut depth = 1;
        self.i += 2;
        while self.i < self.cs.len() && depth > 0 {
            match self.cs[self.i] {
                '\n' => self.line += 1,
                '{' => depth += 1,
                '}' => depth -= 1,
                '"' | '\'' => {
                    let quote = self.cs[self.i];
                    self.read_until(quote);
                    continue;
                }
                _ => {}
            }
            self.i += 1;
        }
    }

    /// Chuỗi thô C++11: `R"delim(...)delim"`, có thể kèm tiền tố `u8`, `u`,
    /// `U`, `L`. Không có ký tự thoát, được chứa nháy kép và xuống dòng, chỉ
    /// đóng ở `)delim"`. `delim` dài tối đa 16 ký tự, không có khoảng trắng,
    /// ngoặc đơn hay `\`; sai dạng thì đây không phải chuỗi thô.
    fn cpp_raw_string(&mut self) -> bool {
        let mut k = self.i;
        for prefix in ["u8", "u", "U", "L"] {
            if self.rest_starts_with_at(k, prefix) && self.cs.get(k + prefix.len()) == Some(&'R') {
                k += prefix.len();
                break;
            }
        }
        if !self.rest_starts_with_at(k, "R\"") {
            return false;
        }
        k += 2;
        let delim_start = k;
        while let Some(&c) = self.cs.get(k) {
            if c == '(' {
                break;
            }
            if c.is_whitespace() || c == ')' || c == '\\' || k - delim_start >= 16 {
                return false;
            }
            k += 1;
        }
        if self.cs.get(k) != Some(&'(') {
            return false;
        }
        let delim: String = self.cs[delim_start..k].iter().collect();
        let close = format!("){delim}\"");
        let line = self.line;
        k += 1;
        let start = k;
        while k < self.cs.len() && !self.rest_starts_with_at(k, &close) {
            if self.cs[k] == '\n' {
                self.line += 1;
            }
            k += 1;
        }
        let value: String = self.cs[start..k].iter().collect();
        self.i = (k + close.chars().count()).min(self.cs.len());
        self.out.push(Token { tok: Tok::Str(value), line });
        self.step(Prev::Punct('"'));
        true
    }

    /// Một hằng số, đọc liền một mạch kể cả dấu phân cách `'` và dấu của số
    /// mũ (`1.5e+3`, `0x1p-3`). Chỉ phát một token: không ai cần từng chữ số.
    fn number(&mut self) {
        let first = self.cs[self.i];
        while let Some(&c) = self.cs.get(self.i) {
            let separator = c == '\''
                && self.cs.get(self.i + 1).is_some_and(|n| n.is_ascii_alphanumeric());
            let exponent_sign = matches!(c, '+' | '-')
                && self.i > 0
                && matches!(self.cs[self.i - 1], 'e' | 'E' | 'p' | 'P');
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || separator || exponent_sign {
                self.i += 1;
            } else {
                break;
            }
        }
        self.out.push(Token { tok: Tok::Punct(first), line: self.line });
        self.step(Prev::Punct(first));
    }

    fn word(&mut self) {
        let start = self.i;
        while self.i < self.cs.len() && is_ident_part(self.cs[self.i]) {
            self.i += 1;
        }
        // Gộp `a.b.c` hoặc `a::b::c` thành một từ.
        if let Some(sep) = self.cfg.path_separator {
            loop {
                let save = self.i;
                if !self.rest_starts_with(sep) {
                    break;
                }
                self.i += sep.chars().count();
                if !self.cs.get(self.i).copied().is_some_and(is_ident_start) {
                    self.i = save;
                    break;
                }
                while self.i < self.cs.len() && is_ident_part(self.cs[self.i]) {
                    self.i += 1;
                }
            }
        }
        let word: String = self.cs[start..self.i].iter().collect();
        self.out.push(Token { tok: Tok::Word(word.clone()), line: self.line });
        self.word_end = self.i;
        self.step(Prev::Word(word));
    }
}

fn is_fstring_prefix(w: &str) -> bool {
    matches!(w.to_ascii_lowercase().as_str(), "f" | "rf" | "fr" | "t" | "rt" | "tr")
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}
