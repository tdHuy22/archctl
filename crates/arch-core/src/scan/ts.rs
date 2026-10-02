//! TypeScript và JavaScript.

use super::Import;
use super::lexer::{Tok, Token};

/// Từ khoá mở đầu câu lệnh mới — gặp phải thì dừng việc tìm `from`.
const STATEMENT_START: &[&str] =
    &["import", "export", "const", "let", "var", "function", "class", "return", "type"];

pub fn extract(toks: &[Token]) -> Vec<Import> {
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
        out.push(Import::local(spec, line));
    }
}
