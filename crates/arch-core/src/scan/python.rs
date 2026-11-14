//! Python.
//!
//! Lexer đã gộp `a.b.c` thành một từ nhờ `path_separator`. Import của Python
//! theo dòng, nên ranh giới câu lệnh được xác định bằng **dòng logic** của
//! token: dòng vật lý, cộng các dòng nối bằng `\` — lexer không phát ra token
//! xuống dòng, nhưng có phát ra dấu `\` đứng ngoài chuỗi. Trong ngoặc đơn thì
//! xuống dòng cũng không kết thúc câu lệnh; phần đó tự đếm ngoặc ở chỗ cần.

use super::lexer::{Tok, Token};
use super::{Import, PyPath};
use std::collections::HashMap;

pub fn extract(toks: &[Token]) -> Vec<Import> {
    let ll = logical_lines(toks);
    let mut out = Vec::new();
    // `sys.path` mà file tự thêm, theo thứ tự Python tìm, và các biến cấp
    // module giữ một đường dẫn tính được (`HERE = Path(__file__).parent`).
    let mut py_path: Vec<PyPath> = Vec::new();
    let mut vars: HashMap<String, Val> = HashMap::new();
    let mut i = 0;
    while i < toks.len() {
        let Tok::Word(word) = &toks[i].tok else {
            i += 1;
            continue;
        };
        let before = out.len();
        match word.as_str() {
            "from" => i = from_import(toks, &ll, i, &mut out),
            // `cimport` của Cython đọc như `import`.
            "import" | "cimport" => i = plain_import(toks, &ll, i, &mut out),
            "sys.path.insert" | "sys.path.append" if is_punct(toks, i + 1, '(') => {
                i = sys_path(toks, &ll, i, &vars, &mut py_path);
            }
            name if statement_start(toks, &ll, i) && is_assignment(toks, &ll, i) => {
                let end = stmt_end(toks, &ll, i);
                match eval_slice(toks, i + 2, end, &vars) {
                    Some(v) if vars.len() < MAX_VARS || vars.contains_key(name) => {
                        vars.insert(name.to_string(), v);
                    }
                    _ => {
                        vars.remove(name);
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
        if !py_path.is_empty() {
            for import in &mut out[before..] {
                import.py_path = py_path.clone();
            }
        }
    }
    out
}

// ---- sys.path ----
//
// Script và hook hay nạp thư viện của chính repo bằng
// `sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "lib"))` rồi
// `import archgate`. Không đọc dòng đó thì import kia là "gói ngoài", và mọi
// luật về nó im lặng. Ở đây chỉ tính những đường dẫn suy được từ `__file__`
// bằng các phép quen thuộc của `pathlib` và `os.path`; đường dẫn lấy từ biến
// môi trường, tham số, thư mục hiện tại thì bỏ qua như trước (bạn chốt
// 14/11/2026: không báo).

/// Giá trị đường dẫn tính được.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Val {
    /// Chính `__file__`.
    File,
    /// Thư mục, tương đối với thư mục của file: các đoạn, `..` chỉ ở đầu.
    Dir(Vec<String>),
    /// Chuỗi viết thẳng (`"lib"`, `".."`): chỉ có nghĩa khi ghép vào `Dir`.
    Lit(String),
}

/// Trần cho mã đối kháng: độ sâu biểu thức, số đoạn đường dẫn, số biến.
const MAX_DEPTH: usize = 32;
const MAX_SEGMENTS: usize = 64;
const MAX_VARS: usize = 256;
const MAX_PY_PATH: usize = 32;

fn is_punct(toks: &[Token], i: usize, c: char) -> bool {
    toks.get(i).is_some_and(|t| t.tok == Tok::Punct(c))
}

/// Số token tối đa của một câu gán hay một lời gọi `sys.path` được xét.
/// Đường dẫn thật chỉ vài chục token; trần giữ cho mã đối kháng (ngoặc mở
/// không đóng trên mỗi dòng) không làm bộ quét chạy bậc hai.
const MAX_STMT_TOKENS: usize = 512;

/// Hết câu lệnh bắt đầu ở `start`: dấu `;` hay hết dòng logic khi không còn
/// ngoặc mở. Trong ngoặc thì xuống dòng không kết thúc câu — kiểu black bẻ
/// `sys.path.insert(\n    0, …\n)` qua nhiều dòng.
fn stmt_end(toks: &[Token], ll: &[usize], start: usize) -> usize {
    let mut depth = 0usize;
    let mut j = start;
    while j < toks.len() && j - start < MAX_STMT_TOKENS {
        match toks[j].tok {
            Tok::Punct('(' | '[' | '{') => depth += 1,
            Tok::Punct(')' | ']' | '}') => depth = depth.saturating_sub(1),
            Tok::Punct(';') if depth == 0 => return j,
            _ => {}
        }
        j += 1;
        if depth == 0 && j < toks.len() && ll[j] != ll[j - 1] {
            return j;
        }
    }
    j
}

/// Token `i` mở đầu một câu lệnh: đầu dòng logic, hay ngay sau `;`.
fn statement_start(toks: &[Token], ll: &[usize], i: usize) -> bool {
    i == 0 || ll[i - 1] != ll[i] || is_punct(toks, i - 1, ';')
}

/// `TÊN = …` (không phải `==`, không phải `TÊN.x = …`).
fn is_assignment(toks: &[Token], ll: &[usize], i: usize) -> bool {
    matches!(&toks[i].tok, Tok::Word(w) if !w.contains('.'))
        && is_punct(toks, i + 1, '=')
        && !is_punct(toks, i + 2, '=')
        && i + 2 < toks.len()
        && ll[i + 2] == ll[i]
}

/// `sys.path.insert(<vị trí>, <đường dẫn>)` hay `sys.path.append(<đường dẫn>)`.
fn sys_path(toks: &[Token], ll: &[usize], start: usize, vars: &HashMap<String, Val>, out: &mut Vec<PyPath>) -> usize {
    let Tok::Word(word) = &toks[start].tok else { return start + 1 };
    let front = word == "sys.path.insert";
    let end = stmt_end(toks, ll, start);
    let mut e = Eval { toks, end, vars, at: start + 2, depth: 0 };
    let args = e.args();
    let value = match (front, args.as_deref()) {
        (true, Some([_, v])) | (false, Some([v])) => v.clone(),
        _ => return start + 1,
    };
    if let Some(Val::Dir(segs)) = value
        && out.len() < MAX_PY_PATH
    {
        let entry = PyPath { dir: segs.join("/"), front };
        out.retain(|p| p.dir != entry.dir);
        if front {
            out.insert(0, entry);
        } else {
            out.push(entry);
        }
    }
    e.at.max(start + 1)
}

fn eval_slice(toks: &[Token], from: usize, end: usize, vars: &HashMap<String, Val>) -> Option<Val> {
    let mut e = Eval { toks, end, vars, at: from, depth: 0 };
    let v = e.expr()?;
    (e.at == end).then_some(v)
}

struct Eval<'a> {
    toks: &'a [Token],
    end: usize,
    vars: &'a HashMap<String, Val>,
    at: usize,
    depth: usize,
}

impl Eval<'_> {
    fn peek(&self) -> Option<&Tok> {
        (self.at < self.end).then(|| &self.toks[self.at].tok)
    }

    fn eat(&mut self, c: char) -> bool {
        if self.peek() == Some(&Tok::Punct(c)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    /// `( a, b, … )` từ ngay sau `(`… thực ra từ chính `(`: `self.at` trỏ vào
    /// token đầu tiên sau `(`. Mỗi đối số là `None` khi không tính được.
    fn args(&mut self) -> Option<Vec<Option<Val>>> {
        let mut out = Vec::new();
        if self.eat(')') {
            return Some(out);
        }
        loop {
            // Đối số có tên (`exist_ok=True`) không đổi đường dẫn: bỏ.
            if matches!(self.peek(), Some(Tok::Word(_))) && self.toks.get(self.at + 1).is_some_and(|t| t.tok == Tok::Punct('=')) {
                self.at += 2;
                self.expr();
                out.push(None);
            } else {
                let start = self.at;
                let v = self.expr();
                if v.is_none() {
                    self.skip_arg(start);
                }
                out.push(v);
            }
            if self.eat(',') {
                if self.eat(')') {
                    return Some(out);
                }
                continue;
            }
            return self.eat(')').then_some(out);
        }
    }

    /// Bỏ qua một đối số không tính được, tới `,` hay `)` cùng cấp.
    fn skip_arg(&mut self, start: usize) {
        self.at = start;
        let mut depth = 0usize;
        while let Some(t) = self.peek() {
            match t {
                Tok::Punct('(' | '[' | '{') => depth += 1,
                Tok::Punct(')' | ']' | '}') if depth == 0 => return,
                Tok::Punct(')' | ']' | '}') => depth -= 1,
                Tok::Punct(',') if depth == 0 => return,
                _ => {}
            }
            self.at += 1;
        }
    }

    fn expr(&mut self) -> Option<Val> {
        if self.depth >= MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let v = self.expr_inner();
        self.depth -= 1;
        v
    }

    fn expr_inner(&mut self) -> Option<Val> {
        let mut left = self.term()?;
        while self.eat('/') {
            let right = self.term()?;
            left = join(left, right)?;
        }
        Some(left)
    }

    fn term(&mut self) -> Option<Val> {
        let mut v = match self.peek()?.clone() {
            Tok::Str(s) => {
                self.at += 1;
                Val::Lit(s)
            }
            Tok::Punct('(') => {
                self.at += 1;
                let v = self.expr()?;
                if !self.eat(')') {
                    return None;
                }
                v
            }
            Tok::Word(w) => {
                self.at += 1;
                self.word(&w)?
            }
            _ => return None,
        };
        // `.resolve()`, `.parent`, `.parents[1]` sau một lời gọi hay ngoặc.
        while self.peek() == Some(&Tok::Punct('.')) {
            self.at += 1;
            let Some(Tok::Word(chain)) = self.peek().cloned() else { return None };
            self.at += 1;
            v = self.attrs(v, chain.split('.'))?;
        }
        Some(v)
    }

    /// Một từ (có thể là `a.b.c`), cộng lời gọi hay chỉ số ngay sau nó.
    fn word(&mut self, w: &str) -> Option<Val> {
        if self.peek() == Some(&Tok::Punct('(')) {
            let name = w.strip_prefix("pathlib.").unwrap_or(w);
            let name = name.strip_prefix("os.").unwrap_or(name);
            if let Some(f) = function(name) {
                self.at += 1;
                let args = self.args()?;
                return f(args);
            }
            // `HERE.joinpath(…)`, `Path(__file__).parent.resolve()`.
            let (base, method) = w.rsplit_once('.')?;
            let v = self.name(base)?;
            return self.attrs(v, std::iter::once(method));
        }
        self.name(w)
    }

    /// `__file__`, một biến đã gán, cộng chuỗi thuộc tính theo sau: `HERE.parent`.
    fn name(&mut self, w: &str) -> Option<Val> {
        let mut parts = w.split('.');
        let v = match parts.next()? {
            "__file__" => Val::File,
            var => self.vars.get(var)?.clone(),
        };
        self.attrs(v, parts)
    }

    /// Áp các thuộc tính; thuộc tính cuối có thể là lời gọi phương thức hay
    /// `parents[k]` khi token kế là `(` hay `[`.
    fn attrs<'s>(&mut self, mut v: Val, chain: impl Iterator<Item = &'s str>) -> Option<Val> {
        let chain: Vec<&str> = chain.collect();
        for (k, attr) in chain.iter().enumerate() {
            let last = k + 1 == chain.len();
            v = match *attr {
                "parent" => parent(v)?,
                "parents" if last && self.eat('[') => {
                    let mut n = 0usize;
                    let mut digits = 0;
                    while let Some(Tok::Punct(c)) = self.peek() {
                        let Some(d) = c.to_digit(10) else { break };
                        n = n.checked_mul(10)?.checked_add(d as usize)?;
                        digits += 1;
                        self.at += 1;
                    }
                    if digits == 0 || !self.eat(']') || n > MAX_SEGMENTS {
                        return None;
                    }
                    (0..=n).try_fold(v, |v, _| parent(v))?
                }
                "resolve" | "absolute" | "expanduser" if last && self.eat('(') => {
                    self.args()?;
                    v
                }
                "joinpath" if last && self.eat('(') => {
                    let args = self.args()?;
                    args.into_iter().try_fold(v, |v, a| join(v, a?))?
                }
                _ => return None,
            };
        }
        Some(v)
    }
}

/// Cách tính một hàm từ các đối số (`None`: đối số không tính được).
type PathFn = fn(Vec<Option<Val>>) -> Option<Val>;

/// Hàm đã biết: tên (bỏ `pathlib.`, `os.`) → cách tính từ các đối số.
fn function(name: &str) -> Option<PathFn> {
    fn one(args: Vec<Option<Val>>) -> Option<Val> {
        match <[Option<Val>; 1]>::try_from(args) {
            Ok([v]) => v,
            Err(_) => None,
        }
    }
    fn path(args: Vec<Option<Val>>) -> Option<Val> {
        let mut it = args.into_iter();
        let first = it.next()??;
        it.try_fold(first, |v, a| join(v, a?))
    }
    fn dirname(args: Vec<Option<Val>>) -> Option<Val> {
        parent(one(args)?)
    }
    Some(match name {
        "Path" | "PurePath" | "PosixPath" | "path.join" => path,
        "str" | "fspath" | "path.abspath" | "path.realpath" | "path.normpath" | "path.expanduser" => one,
        "path.dirname" => dirname,
        _ => return None,
    })
}

fn parent(v: Val) -> Option<Val> {
    match v {
        Val::File => Some(Val::Dir(Vec::new())),
        Val::Dir(segs) => push(segs, ".."),
        Val::Lit(_) => None,
    }
}

/// `a / b`, `os.path.join(a, b)`: `b` phải là chuỗi tương đối.
fn join(left: Val, right: Val) -> Option<Val> {
    let Val::Lit(right) = right else { return None };
    if right.starts_with('/') || right.starts_with('~') || right.contains('\0') {
        return None;
    }
    match left {
        Val::Dir(mut segs) => {
            for part in right.split('/') {
                segs = match push(segs, part)? {
                    Val::Dir(s) => s,
                    _ => return None,
                };
            }
            Some(Val::Dir(segs))
        }
        Val::Lit(l) => Some(Val::Lit(format!("{l}/{right}"))),
        Val::File => None,
    }
}

fn push(mut segs: Vec<String>, part: &str) -> Option<Val> {
    match part {
        "" | "." => {}
        ".." if segs.last().is_some_and(|s| s != "..") => {
            segs.pop();
        }
        p => {
            if segs.len() >= MAX_SEGMENTS || p.contains(['\\', ':']) {
                return None;
            }
            segs.push(p.to_string());
        }
    }
    Some(Val::Dir(segs))
}

/// Số thứ tự dòng logic của từng token. Sang dòng vật lý mới là sang dòng
/// logic mới, trừ khi token trước là `\` nối dòng.
fn logical_lines(toks: &[Token]) -> Vec<usize> {
    let mut out = Vec::with_capacity(toks.len());
    let mut id = 0usize;
    for (k, t) in toks.iter().enumerate() {
        if let Some(prev) = k.checked_sub(1).map(|p| &toks[p])
            && t.line != prev.line
            && prev.tok != Tok::Punct('\\')
        {
            id += 1;
        }
        out.push(id);
    }
    out
}

fn is_continuation(toks: &[Token], i: usize) -> bool {
    toks.get(i).is_some_and(|t| t.tok == Tok::Punct('\\'))
}

/// `from a.b import c` · `from . import c` · `from ..pkg import (c, d)`
///
/// Chỉ là import khi ngay sau đường dẫn module là `import`. `yield from x` và
/// `raise E from e` cũng có `from`: coi chúng là import thì vừa sinh import
/// giả, vừa bỏ qua "danh sách tên" kéo sang câu lệnh sau và nuốt mất nó.
fn from_import(toks: &[Token], ll: &[usize], start: usize, out: &mut Vec<Import>) -> usize {
    let line = toks[start].line;
    let id = ll[start];
    let here = |i: usize| i < toks.len() && ll[i] == id;
    let mut i = start + 1;
    let mut spec = String::new();

    while here(i) && (toks[i].tok == Tok::Punct('.') || is_continuation(toks, i)) {
        if toks[i].tok == Tok::Punct('.') {
            spec.push('.');
        }
        i += 1;
    }
    if here(i)
        && let Tok::Word(w) = &toks[i].tok
        && w != "import"
        && w != "cimport"
    {
        spec.push_str(w);
        i += 1;
    }
    while here(i) && is_continuation(toks, i) {
        i += 1;
    }
    let is_import =
        here(i) && matches!(&toks[i].tok, Tok::Word(w) if w == "import" || w == "cimport");
    if spec.is_empty() || !is_import {
        return start + 1;
    }
    let mut import = Import::local(spec, line);
    let end = read_names(toks, ll, i + 1, id, &mut import.names);
    out.push(import);
    end
}

/// Đọc danh sách tên sau `import`, tới hết câu lệnh: hết dòng logic hoặc tới
/// dấu `;`. Trong ngoặc đơn thì xuống dòng không tính. Tên sau `as` là bí
/// danh, không phải thứ được nhập; `*` không nhập tên cụ thể nào.
fn read_names(
    toks: &[Token],
    ll: &[usize],
    start: usize,
    id: usize,
    names: &mut Vec<String>,
) -> usize {
    let mut i = start;
    let mut depth = 0usize;
    let mut alias = false;
    while let Some(t) = toks.get(i) {
        if depth == 0 && (ll[i] != id || t.tok == Tok::Punct(';')) {
            return i;
        }
        match &t.tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => depth = depth.saturating_sub(1),
            Tok::Word(w) if w == "as" => alias = true,
            Tok::Word(w) if !alias => names.push(w.clone()),
            Tok::Word(_) => alias = false,
            _ => {}
        }
        i += 1;
    }
    i
}

/// `import a.b` · `import a.b as c, d.e` · `import a, \` xuống dòng `b`
fn plain_import(toks: &[Token], ll: &[usize], start: usize, out: &mut Vec<Import>) -> usize {
    let line = toks[start].line;
    let id = ll[start];
    let mut i = start + 1;
    let mut expect_module = true;

    while let Some(t) = toks.get(i) {
        if ll[i] != id {
            break;
        }
        match &t.tok {
            Tok::Punct('\\') => {}
            Tok::Word(w) if w == "as" => {
                i += 2; // bỏ cả bí danh
                expect_module = false;
                continue;
            }
            Tok::Word(w) if expect_module => {
                out.push(Import::local(w, line));
                expect_module = false;
            }
            Tok::Punct(',') => expect_module = true,
            _ => break,
        }
        i += 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use crate::scan::Lang;

    /// `(spec, [thư mục sys.path])` của mỗi import.
    fn paths(src: &str) -> Vec<(String, Vec<String>)> {
        super::extract(&Lang::Python.tokens(src))
            .into_iter()
            .map(|i| {
                let dirs = i.py_path.iter().map(|p| format!("{}{}", if p.front { "+" } else { "~" }, p.dir)).collect();
                (i.spec, dirs)
            })
            .collect()
    }

    fn dirs_of_last(src: &str) -> Vec<String> {
        paths(src).pop().expect("có import").1
    }

    #[test]
    fn cac_dang_pathlib_va_os_path() {
        let cases = [
            ("sys.path.insert(0, str(Path(__file__).resolve().parent.parent / 'lib'))", "+../lib"),
            ("sys.path.insert(0, str(pathlib.Path(__file__).absolute().parents[2] / 'a/b'))", "+../../a/b"),
            ("sys.path.insert(0, os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), 'lib'))", "+../lib"),
            ("sys.path.insert(1, os.path.realpath(os.path.join(os.path.dirname(__file__), '..', 'x', '.')))", "+../x"),
            ("sys.path.append(os.fspath(Path(__file__).parent.joinpath('vendor', 'pkg')))", "~vendor/pkg"),
            ("sys.path.insert(0, str(Path(__file__).parent))", "+"),
        ];
        for (line, want) in cases {
            let src = format!("import os, sys\n{line}\nimport m\n");
            assert_eq!(dirs_of_last(&src), [want], "{line}");
        }
    }

    #[test]
    fn bien_gan_truoc_va_kiem_trung() {
        let src = "\
HERE = Path(__file__).resolve().parent
LIB = HERE.parent / \"lib\"
if str(LIB) not in sys.path:
    sys.path.insert(0, str(LIB))
import a
sys.path.insert(0, str(HERE / 'b'))
import c
";
        let got = paths(src);
        assert_eq!(got[0], ("a".to_string(), vec!["+../lib".to_string()]));
        assert_eq!(got[1], ("c".to_string(), vec!["+b".to_string(), "+../lib".to_string()]));
    }

    #[test]
    fn dang_khong_tinh_duoc_thi_bo_qua() {
        for line in [
            "sys.path.insert(0, os.environ['X'])",
            "sys.path.insert(0, 'lib')",
            "sys.path.insert(0, '/opt/lib')",
            "sys.path.insert(0, str(Path.cwd() / 'lib'))",
            "sys.path.insert(0, str(Path(__file__) / 'lib'))",
            "sys.path.insert(0, f'{ROOT}/lib')",
            "sys.path.insert(0, str(Path(__file__).parents[i]))",
            "sys.path.append()",
        ] {
            let src = format!("{line}\nimport m\n");
            assert!(dirs_of_last(&src).is_empty(), "{line}");
        }
        // Biến gán lại bằng thứ không tính được thì không còn dùng được.
        let src = "D = Path(__file__).parent\nD = os.environ['X']\nsys.path.insert(0, str(D))\nimport m\n";
        assert!(dirs_of_last(src).is_empty());
    }

    #[test]
    fn chi_import_dung_sau() {
        let got = paths("import a\nsys.path.append(os.path.dirname(__file__) + '/x')\nsys.path.insert(0, os.path.dirname(__file__))\nfrom b import c\n");
        assert_eq!(got[0].1, Vec::<String>::new());
        assert_eq!(got[1].1, ["+"]);
    }

    /// Kiểu black: lời gọi và phép gán bẻ qua nhiều dòng trong ngoặc; và hai
    /// câu trên một dòng ngăn bằng `;`.
    #[test]
    fn cau_lenh_nhieu_dong_va_dau_cham_phay() {
        let src = "\
LIB = (
    Path(__file__).resolve().parent
    / \"lib\"
)
sys.path.insert(
    0,
    str(LIB),
)
import a
HERE = Path(__file__).parent; sys.path.append(str(HERE / 'x'))
import b
";
        let got = paths(src);
        assert_eq!(got[0].1, ["+lib"]);
        assert_eq!(got[1].1, ["+lib", "~x"]);
    }

    /// `a=1;` lặp 100 000 lần trên một dòng: tuyến tính, không bậc hai (review
    /// 14/11/2026: bản đầu mất 59 giây).
    #[test]
    fn nhieu_cau_tren_mot_dong_van_nhanh() {
        let src = "a=1;".repeat(100_000) + "\nimport m\n";
        let began = std::time::Instant::now();
        assert!(dirs_of_last(&src).is_empty());
        assert!(began.elapsed() < std::time::Duration::from_secs(5), "{:?}", began.elapsed());
        let src = "x = (\n".repeat(20_000) + "import m\n";
        let began = std::time::Instant::now();
        let _ = paths(&src);
        assert!(began.elapsed() < std::time::Duration::from_secs(5), "{:?}", began.elapsed());
    }

    /// Mã đối kháng: ngoặc lồng sâu không làm tràn stack, chỉ số lớn không tràn số.
    #[test]
    fn ma_doi_khang_khong_lam_sap() {
        let deep = format!("sys.path.insert(0, {}__file__{})\nimport m\n", "str(".repeat(100_000), ")".repeat(100_000));
        let handle = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || dirs_of_last(&deep))
            .unwrap();
        assert!(handle.join().unwrap().is_empty());
        let big = "sys.path.insert(0, str(Path(__file__).parents[99999999999999999999999]))\nimport m\n";
        assert!(dirs_of_last(big).is_empty());
        let many = (0..1000).map(|k| format!("sys.path.insert(0, str(Path(__file__).parent / 'd{k}'))\n")).collect::<String>() + "import m\n";
        assert!(dirs_of_last(&many).len() <= 32);
    }
}
