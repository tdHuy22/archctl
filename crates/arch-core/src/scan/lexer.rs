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
            if c.is_whitespace() {
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
            if self.cfg.raw_hash_strings && c == 'r' && self.raw_string() {
                continue;
            }
            if c == '"' || (c == '\'' && self.cfg.char_literals == CharLit::AsString) {
                self.quoted(c);
                continue;
            }
            if c == '\'' {
                self.char_or_lifetime();
                continue;
            }
            if c == '`' {
                self.template();
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

    /// `#include "x.h"` hoặc `#include <x.h>`. Chỉ thị khác thì bỏ qua cả dòng
    /// để `#if`, `#define` không sinh token rác.
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
        self.i = line_end;
        true
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
        let mut k = self.i + 1;
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
        if self.cfg.triple_quoted
            && self.cs.get(self.i + 1) == Some(&quote)
            && self.cs.get(self.i + 2) == Some(&quote)
        {
            let close: String = std::iter::repeat_n(quote, 3).collect();
            self.i += 3;
            while self.i < self.cs.len() && !self.rest_starts_with(&close) {
                if self.cs[self.i] == '\n' {
                    self.line += 1;
                }
                self.i += 1;
            }
            self.i = (self.i + 3).min(self.cs.len());
            self.out.push(Token { tok: Tok::Str(String::new()), line: self.line });
            self.step(Prev::Punct('"'));
            return;
        }
        let line = self.line;
        let value = self.read_until(quote);
        self.out.push(Token { tok: Tok::Str(value), line });
        self.step(Prev::Punct('"'));
    }

    fn read_until(&mut self, quote: char) -> String {
        self.i += 1;
        let mut value = String::new();
        while self.i < self.cs.len() {
            match self.cs[self.i] {
                '\\' => {
                    if let Some(&next) = self.cs.get(self.i + 1) {
                        value.push(next);
                    }
                    self.i += 2;
                }
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
                '\\' => self.i += 2,
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
        self.step(Prev::Word(word));
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '$'
}

fn is_ident_part(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}
