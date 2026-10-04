//! Go.
//!
//! Dễ nhất trong các ngôn ngữ ở đây: đường dẫn import luôn là một chuỗi, và
//! luôn nằm ngay sau `import` hoặc trong khối ngoặc của nó.

use super::Import;
use super::lexer::{Tok, Token};

pub fn extract(toks: &[Token]) -> Vec<Import> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if toks[i].tok != Tok::Word("import".into()) {
            i += 1;
            continue;
        }
        // import bolt "go.etcd.io/bbolt", import . "strings", import _ "embed"
        let renamed = matches!(
            toks.get(i + 1).map(|t| &t.tok),
            Some(Tok::Word(_) | Tok::Punct('.'))
        ) && matches!(toks.get(i + 2).map(|t| &t.tok), Some(Tok::Str(_)));
        if renamed {
            i += 1;
        }
        match toks.get(i + 1).map(|t| &t.tok) {
            // import "fmt"
            Some(Tok::Str(spec)) => {
                push(&mut out, spec, toks[i + 1].line);
                i += 2;
            }
            // import ( "fmt" \n alias "os" )
            Some(Tok::Punct('(')) => {
                i += 2;
                while let Some(t) = toks.get(i) {
                    match &t.tok {
                        Tok::Punct(')') => break,
                        Tok::Str(spec) => push(&mut out, spec, t.line),
                        _ => {}
                    }
                    i += 1;
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

fn push(out: &mut Vec<Import>, spec: &str, line: usize) {
    if !spec.is_empty() {
        out.push(Import::local(spec, line));
    }
}
