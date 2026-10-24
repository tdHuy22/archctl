//! Java.
//!
//! Câu `import` là phần dễ: `import a.b.C;`, `import a.b.*;`, `import static
//! a.b.C.m;`. Lexer đã gộp `a.b.C` thành một từ (`path_separator`), nhưng
//! Java cho phép khoảng trắng và chú thích giữa các đoạn (`a . b /* x */ . C`),
//! nên câu được ghép lại từ mọi từ và dấu chấm tới `;`.
//!
//! Phần khó là Java không cần `import` mới dùng được lớp khác:
//!
//! - tên đầy đủ viết thẳng trong mã: `com.acme.billing.Invoice.of(x)`;
//! - tên trần của lớp cùng package, kể cả khi lớp đó nằm ở module khác
//!   (package tách đôi: `core/src/a/b/Util.java` và `app/src/a/b/Page.java`);
//! - tên trần của lớp trong package nhập bằng `import p.*`.
//!
//! Những dạng đó là cách lách luật dễ nhất, nên được trả về như tham chiếu
//! ([`Ref`]): chỉ thành cạnh khi phân giải ra file trong repo, không thì im
//! lặng — đa số là biến, trường, lớp của JDK.

use super::lexer::{Tok, Token};
use super::{Import, Ref};
use std::collections::{HashMap, HashSet};

/// Trần số câu import, số tham chiếu khác nhau của một file. File tới 4 MB
/// chạm được trần này; chạm thì `cut`, và `check` báo file đó (TM-72).
pub const MAX_IMPORTS: usize = 1 << 16;
pub const MAX_REFS: usize = 1 << 16;
/// Trần số token của một câu import: `import` mà không có `;` thì không đọc
/// cả phần còn lại của file.
const MAX_IMPORT_TOKENS: usize = 256;
/// Tên đầy đủ viết thường hết (`a.b.c.d`): thử tối đa chừng này đoạn đầu.
const MAX_LOWER_SEGMENTS: usize = 8;
/// Mỗi tên trần thử package của file rồi từng `import p.*`: số `import p.*`
/// nhân số tên là số lần phân giải. Trần cả hai, chạm thì `cut` (TM-72).
const MAX_WILDCARDS: usize = 256;
const MAX_SPECS: usize = 1 << 18;

/// Import, tham chiếu không qua import, và có chạm trần không.
pub fn extract(toks: &[Token]) -> (Vec<Import>, Vec<Ref>, bool) {
    let mut imports = Vec::new();
    let mut cut = false;
    let mut package: Option<String> = None;
    // Tên trần mà câu import một lớp đưa vào phạm vi: chúng che lớp cùng
    // package (JLS 6.4.1), nên không phải tham chiếu cùng package.
    let mut single: HashSet<String> = HashSet::new();
    // `import p.*` (và `import static a.b.C.*`): nơi tìm tên trần, sau package.
    let mut wildcards: Vec<String> = Vec::new();
    // Token nằm trong câu `package`, `import`: không phải mã.
    let mut header: Vec<bool> = vec![false; toks.len()];

    let mut i = 0;
    while i < toks.len() {
        let word = match &toks[i].tok {
            Tok::Word(w) => w.as_str(),
            _ => {
                i += 1;
                continue;
            }
        };
        if word != "import" && !(word == "package" && package.is_none()) {
            i += 1;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        let is_static = matches!(toks.get(j).map(|t| &t.tok), Some(Tok::Word(w)) if w == "static");
        if word == "import" && is_static {
            j += 1;
        }
        let mut spec = String::new();
        let mut ok = false;
        while let Some(t) = toks.get(j) {
            if j - start > MAX_IMPORT_TOKENS {
                break;
            }
            match &t.tok {
                Tok::Word(w) => spec.push_str(w),
                Tok::Punct('.') => spec.push('.'),
                Tok::Punct('*') => spec.push('*'),
                Tok::Punct(';') => {
                    ok = true;
                    break;
                }
                _ => break,
            }
            j += 1;
        }
        if !ok || spec.is_empty() || spec.starts_with('.') || spec.contains("..") {
            i += 1;
            continue;
        }
        for h in &mut header[start..=j] {
            *h = true;
        }
        if word == "package" {
            package = Some(spec);
        } else if imports.len() < MAX_IMPORTS {
            if let Some(p) = spec.strip_suffix(".*") {
                if wildcards.len() < MAX_WILDCARDS {
                    wildcards.push(p.to_string());
                } else {
                    cut = true;
                }
            } else if !is_static && let Some((_, name)) = spec.rsplit_once('.') {
                single.insert(name.to_string());
            }
            imports.push(Import::local(spec, toks[start].line));
        } else {
            cut = true;
        }
        i = j + 1;
    }

    let (refs, refs_cut) = refs(toks, &header, package.as_deref(), &single, &wildcards);
    (imports, refs, cut || refs_cut)
}

/// Package mà file khai ở câu `package` đầu tiên.
pub fn package(toks: &[Token]) -> Option<String> {
    let at = toks.iter().position(|t| matches!(&t.tok, Tok::Word(w) if w == "package"))?;
    let mut spec = String::new();
    for t in toks[at + 1..].iter().take(MAX_IMPORT_TOKENS) {
        match &t.tok {
            Tok::Word(w) => spec.push_str(w),
            Tok::Punct('.') => spec.push('.'),
            Tok::Punct(';') => return (!spec.is_empty()).then_some(spec),
            _ => return None,
        }
    }
    None
}

fn refs(
    toks: &[Token],
    header: &[bool],
    package: Option<&str>,
    single: &HashSet<String>,
    wildcards: &[String],
) -> (Vec<Ref>, bool) {
    let shadows = declared_types(toks);
    let shadowed = |name: &str, at: usize| shadows.get(name).is_some_and(|v| v.iter().any(|&(a, b)| a <= at && at <= b));
    let mut seen: HashSet<Vec<String>> = HashSet::new();
    let mut out = Vec::new();
    let mut specs_total = 0usize;
    let is_dot = |i: usize| matches!(toks.get(i).map(|t| &t.tok), Some(Tok::Punct('.')));
    let word = |i: usize| match toks.get(i).map(|t| &t.tok) {
        Some(Tok::Word(w)) => Some(w.as_str()),
        _ => None,
    };
    let mut i = 0;
    while i < toks.len() {
        let at = i;
        i += 1;
        if header[at] {
            continue;
        }
        let Some(first) = word(at) else { continue };
        // `f().b.C`: truy cập thành viên của một biểu thức, không phải tên.
        if at > 0 && is_dot(at - 1) {
            continue;
        }
        // Lexer chỉ gộp `a.b.C` viết liền; `com.acme\n  .core.Util` là ba
        // token, ghép lại ở đây (TM-71).
        let mut w = first.to_string();
        while is_dot(i) && !header.get(i + 1).copied().unwrap_or(true) {
            let Some(next) = word(i + 1) else { break };
            w.push('.');
            w.push_str(next);
            i += 2;
        }
        let segs: Vec<&str> = w.split('.').collect();
        let pkg = segs.iter().take_while(|s| starts_lower(s)).count();
        let (specs, qualified) = if pkg > 0 {
            if matches!(segs[0], "this" | "super") || segs.len() < 2 {
                continue;
            }
            // `com.acme.Invoice.of`: package viết thường rồi lớp viết hoa.
            let class = segs[pkg..].iter().take_while(|s| starts_upper(s)).count();
            if class > 0 {
                (vec![segs[..pkg + class].join(".")], true)
            } else {
                // Lớp tên viết thường (`com.acme.core.helper.f()`): thử các
                // đoạn đầu, dài nhất trước.
                let n = segs.len().min(MAX_LOWER_SEGMENTS);
                ((2..=n).rev().map(|k| segs[..k].join(".")).collect(), false)
            }
        } else {
            // `Util`, `Util.helper`: lớp cùng package, rồi lớp của `import p.*`
            // (JLS 6.4.1), trừ khi tên đã nhập riêng hay được khai ở phạm vi này.
            let name = segs[0];
            if !starts_upper(name) || single.contains(name) || shadowed(name, at) {
                continue;
            }
            let specs: Vec<String> =
                package.into_iter().chain(wildcards.iter().map(String::as_str)).map(|p| format!("{p}.{name}")).collect();
            if specs.is_empty() {
                continue;
            }
            (specs, false)
        };
        if seen.contains(&specs) {
            continue;
        }
        specs_total += specs.len();
        if out.len() >= MAX_REFS || specs_total > MAX_SPECS {
            return (out, true);
        }
        seen.insert(specs.clone());
        out.push(Ref { line: toks[at].line, specs, qualified });
    }
    (out, false)
}

/// Tên kiểu mà chính file khai (`class X`, `interface X`, `enum X`, `record
/// X`, `@interface X`), kèm phạm vi token mà nó che: cả file nếu khai ở mức
/// ngoài cùng, không thì khối `{…}` bao quanh chỗ khai (kiểu lồng, kiểu cục bộ).
fn declared_types(toks: &[Token]) -> HashMap<&str, Vec<(usize, usize)>> {
    // Cặp ngoặc nhọn khớp nhau.
    let mut close_of = vec![toks.len(); toks.len()];
    let mut stack: Vec<usize> = Vec::new();
    let mut enclosing: Vec<Option<usize>> = vec![None; toks.len()];
    for (k, t) in toks.iter().enumerate() {
        enclosing[k] = stack.last().copied();
        match t.tok {
            Tok::Punct('{') => stack.push(k),
            Tok::Punct('}') => {
                if let Some(open) = stack.pop() {
                    close_of[open] = k;
                }
            }
            _ => {}
        }
    }
    let mut out: HashMap<&str, Vec<(usize, usize)>> = HashMap::new();
    for k in 0..toks.len().saturating_sub(1) {
        if let (Tok::Word(kw), Tok::Word(name)) = (&toks[k].tok, &toks[k + 1].tok)
            && matches!(kw.as_str(), "class" | "interface" | "enum" | "record")
            && !name.contains('.')
        {
            // Kiểu cục bộ (khai trong thân phương thức, trong khối lệnh) che từ
            // chỗ khai trở đi (JLS 6.3); kiểu thành viên che cả thân lớp.
            let scope = match enclosing[k] {
                Some(open) if !type_body(toks, open) => (k, close_of[open]),
                Some(open) => (open, close_of[open]),
                None => (0, toks.len()),
            };
            out.entry(name.as_str()).or_default().push(scope);
        }
    }
    out
}

/// Khối `{` ở `open` là thân của một kiểu (`class X … {`, `new X() {`), không
/// phải khối lệnh: phần đứng trước nó tới `;`, `{`, `}` có `class`,
/// `interface`, `enum`, `record` hay `new`.
fn type_body(toks: &[Token], open: usize) -> bool {
    for t in toks[..open].iter().rev().take(512) {
        match &t.tok {
            Tok::Punct(';' | '{' | '}') => return false,
            Tok::Word(w) if matches!(w.as_str(), "class" | "interface" | "enum" | "record" | "new") => return true,
            _ => {}
        }
    }
    false
}

fn starts_lower(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
}

fn starts_upper(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_uppercase() || c == '$')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> (Vec<String>, Vec<String>) {
        let view = crate::scan::source_view("A.java", src);
        let (imports, refs, _) = extract(&crate::scan::Lang::Java.tokens(&view));
        (
            imports.into_iter().map(|i| format!("{}:{}", i.line, i.spec)).collect(),
            refs.into_iter().map(|r| format!("{}:{}", r.line, r.specs.join("|"))).collect(),
        )
    }

    #[test]
    fn cac_dang_import() {
        let (imports, _) = run(
            "package a.b;\nimport x.y.C;\nimport x.y.*;\nimport static x.y.C.m;\nimport static x.y.C.*;\nimport x . y /* z */ . D;\nimport x.y.\n  E;\n",
        );
        assert_eq!(imports, ["2:x.y.C", "3:x.y.*", "4:x.y.C.m", "5:x.y.C.*", "6:x.y.D", "7:x.y.E"]);
    }

    #[test]
    fn import_trong_chuoi_chu_thich_hay_khong_dong_khong_tinh() {
        let (imports, refs) = run("// import a.b.C;\nString s = \"import a.b.D;\";\n/* import a.b.E; */\nimport a.b.F\n");
        // `import a.b.F` thiếu `;` không phải câu import; tên vẫn là ứng viên tham chiếu.
        assert!(imports.is_empty(), "{imports:?}");
        assert_eq!(refs, ["4:a.b.F"]);
    }

    #[test]
    fn ten_day_du_va_ten_tran_cung_package() {
        let (_, refs) = run(
            "package a.b;\nimport x.y.List;\nimport x.y.*;\nclass Page extends Base {\n  com.acme.Invoice.Line l = com.acme.Invoice.of(Util.x(), List.of(), this.field.Name);\n  record R() {}\n  R r; Page p; String s = \"Util2\";\n}\n",
        );
        // `List` nhập riêng che lớp cùng package; `Page`, `R` do file tự khai;
        // `String` là ứng viên như mọi tên trần (cùng package, rồi `x.y.*`),
        // chỉ thành cạnh khi ra file trong repo. `this.field.Name` là trường.
        assert_eq!(
            refs,
            ["4:a.b.Base|x.y.Base", "5:com.acme.Invoice.Line", "5:com.acme.Invoice", "5:a.b.Util|x.y.Util", "7:a.b.String|x.y.String"]
        );
    }

    #[test]
    fn ten_day_du_viet_tach_dong_va_truy_cap_thanh_vien() {
        let (_, refs) = run("class A {\n  int x = com.acme\n    . core /* c */ .Util.helper();\n  int y = f().com.acme.core.Other.z;\n}\n");
        assert_eq!(refs, ["2:com.acme.core.Util"]);
    }

    #[test]
    fn hau_to_cua_so_khong_phai_ten_lop() {
        let (_, refs) = run("package a;\nclass A { long x = 42L + 0x1F + 1_000F + 1.5e-3D; }\n");
        assert!(refs.is_empty(), "{refs:?}");
    }

    #[test]
    fn ten_tran_qua_import_sao_va_pham_vi_lop_long() {
        let (_, refs) = run(
            "package a;\nimport p.q.*;\nimport static x.Y.*;\nclass S { int y = Util.f(); }\nclass O { static class Util {} Util u; }\nclass M { void m() { class Local {} Local l; } Local k; }\n",
        );
        // `O.Util` chỉ che trong thân `O`; `Local` chỉ che trong thân `m`.
        assert_eq!(refs, ["4:a.Util|p.q.Util|x.Y.Util", "6:a.Local|p.q.Local|x.Y.Local"]);
    }

    #[test]
    fn escape_xuong_dong_ket_thuc_chu_thich() {
        let (_, refs) = run("class A {\n  // x \\u000a int a = com.acme.core.Util.f();\n}\n");
        assert_eq!(refs, ["3:com.acme.core.Util"]);
    }

    #[test]
    fn lop_cuc_bo_che_tu_cho_khai() {
        let (_, refs) = run("package a;\nclass A {\n  void f() { Util.x(); class Util {} Util.y(); }\n  static class Inner {} Inner i;\n}\n");
        assert_eq!(refs, ["3:a.Util"], "trước chỗ khai là lớp cùng package; sau đó là lớp cục bộ");
    }

    #[test]
    fn nhieu_import_sao_nhan_nhieu_ten_bi_cat() {
        let mut src = String::from("package a;\n");
        for i in 0..300 {
            src.push_str(&format!("import p{i}.*;\n"));
        }
        for i in 0..2_000 {
            src.push_str(&format!("N{i} n{i};\n"));
        }
        let (_, refs, cut) = extract(&crate::scan::Lang::Java.tokens(&src));
        assert!(cut && refs.iter().all(|r| r.specs.len() == 1 + MAX_WILDCARDS));
    }

    #[test]
    fn escape_unicode_va_so_hex() {
        let (imports, refs) = run("package a;\nimport \\u0063om.acme.Util;\nclass A { int x = 0xE+com.acme.core.Util.f() + \\u0063om.acme.X.g(); String s = \"\\\\u0063\"; }\n");
        assert_eq!(imports, ["2:com.acme.Util"]);
        assert_eq!(refs, ["3:com.acme.core.Util", "3:com.acme.X", "3:a.String"]);
    }

    #[test]
    fn lop_ten_viet_thuong_hay_dau_dola() {
        let (_, refs) = run("class A { int a = com.acme.core.helper.f(); Object b = new com.acme.shared.$Gen(); }\n");
        assert_eq!(refs, ["1:com.acme.core.helper.f|com.acme.core.helper|com.acme.core|com.acme", "1:com.acme.shared.$Gen"]);
    }

    #[test]
    fn khong_package_thi_khong_co_ten_tran() {
        let (_, refs) = run("class A { Util u; }\n");
        assert!(refs.is_empty(), "{refs:?}");
    }

    #[test]
    fn file_doi_khang_khong_treo() {
        let mut src = String::from("package a;\n");
        for i in 0..200_000 {
            src.push_str(&format!("import a.b{i}.C;\nX{i} x{i};\n"));
        }
        src.push_str("import a.b.c.d.e.f");
        let clock = std::time::Instant::now();
        let (imports, refs, cut) = extract(&crate::scan::Lang::Java.tokens(&src));
        assert_eq!((imports.len(), refs.len(), cut), (MAX_IMPORTS, MAX_REFS, true));
        assert!(clock.elapsed().as_secs() < 10);
    }
}
