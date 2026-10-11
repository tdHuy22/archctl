//! Cạnh phụ thuộc đi qua ranh giới ngôn ngữ.
//!
//! Phân tích import chỉ thấy phụ thuộc trong cùng một ngôn ngữ. Rust gọi C
//! không qua câu lệnh import nào, nên cạnh đó vô hình — và trong
//! `examples/sensor` nó che đúng một vi phạm thật.
//!
//! Nhưng một phần lớn FFI viết tay là **khai báo tường minh**, không phải suy
//! đoán:
//!
//! ```text
//! // Rust gọi ra ngoài            // C cung cấp
//! extern "C" { fn parse(…); }     int parse(const char *b) { … }
//! ```
//!
//! Hai đầu mang cùng một cái tên, và cái tên đó phải khớp thì chương trình mới
//! liên kết được. Ghép chúng lại là ra cạnh, không cần đọc binary và không cần
//! bảng ký hiệu.
//!
//! Python gọi mã native cũng vậy, chỉ khác thứ mang tên chung:
//!
//! ```text
//! // Python                          // native
//! from ._core import Validator       #[pymodule] fn _core(…)       (PyO3)
//!                                    PYBIND11_MODULE(_core, m)     (pybind11)
//! torch.ops.mylib.fused_add(a, b)    TORCH_LIBRARY(mylib, m) {
//!                                      m.def("fused_add(Tensor a, …)");
//! ```
//!
//! Hai loại tên đó không lẫn với tên hàm C: khoá ghép của chúng mang tiền tố
//! `py:` và `op:`. Phía Python của module mở rộng là **import**, nên nó cần
//! bộ phân giải và được dựng ở `check`, không ở đây — xem
//! [`crate::resolve::Resolver::import_targets`].
//!
//! Những gì **không** nhận ra: `dlopen` và tên hàm dựng lúc chạy, con trỏ hàm,
//! `ctypes`, IPC, tiến trình con. Những thứ đó vẫn cần `api` — xem
//! `docs/api.md`.

use super::Lang;
use super::lexer::{Tok, Token};
use std::collections::BTreeMap;

/// Một đầu của cạnh đi qua ranh giới ngôn ngữ.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Crossing {
    /// Khoá để ghép hai đầu: tên hàm C, `py:<module>` cho module mở rộng của
    /// Python, `op:<namespace>.<op>` cho op của PyTorch.
    pub symbol: String,
    /// Cách phía gọi viết nó trong mã, để báo cáo.
    pub shown: String,
    pub line: usize,
}

/// Hàm đi qua ranh giới ngôn ngữ, nhìn từ một file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Foreign {
    /// Tên file này gọi nhưng không định nghĩa: `extern "C" { fn x; }` của
    /// Rust, `C.x` của Go, `torch.ops.ns.op` của Python.
    pub calls: Vec<Crossing>,
    /// Tên file này đưa ra cho ngôn ngữ khác gọi: định nghĩa hàm trong C,
    /// `pub extern "C" fn x` của Rust, module mở rộng và op PyTorch.
    pub provides: Vec<Crossing>,
}

impl Foreign {
    fn is_empty(&self) -> bool {
        self.calls.is_empty() && self.provides.is_empty()
    }
}

/// Đứng thay `TORCH_EXTENSION_NAME` trong khoá ghép phía native. Giá trị thật
/// do `setup.py` đặt cho từng file nguồn — xem
/// [`crate::resolve::Resolver::bind_extension`].
pub const EXTENSION_NAME: &str = "@ext";

/// Ai cung cấp `key`, trong bảng tên → component. Chuỗi rỗng là tên có hai
/// component cùng cung cấp: không biết chọn bên nào, nên không ghép.
pub fn provider<'a>(provides: &'a BTreeMap<String, String>, key: &str) -> Option<&'a String> {
    provides.get(key).filter(|owner| !owner.is_empty())
}

pub fn scan(toks: &[Token], lang: Lang) -> Foreign {
    let found = match lang {
        Lang::Rust => rust(toks),
        Lang::Go => go(toks),
        Lang::C => c(toks),
        Lang::Python => python(toks),
        Lang::TypeScript => Foreign::default(),
    };
    debug_assert!(found.is_empty() || !toks.is_empty());
    found
}

fn hit(name: &str, line: usize) -> Crossing {
    Crossing { symbol: name.to_string(), shown: name.to_string(), line }
}

/// Module mở rộng tên `name`, phía native.
fn module(name: &str, line: usize) -> Crossing {
    Crossing { symbol: format!("py:{name}"), shown: name.to_string(), line }
}

// ---- Rust ----

/// `extern "C" { … }` khai báo hàm ở ngoài; `extern "C" fn x()` định nghĩa một
/// hàm cho ngoài gọi. Hai chiều ngược nhau, phân biệt bằng token ngay sau ABI.
fn rust(toks: &[Token]) -> Foreign {
    let mut out = Foreign::default();
    for i in 0..toks.len() {
        if !matches!(&toks[i].tok, Tok::Word(w) if w == "extern") {
            continue;
        }
        // ABI nào cũng được, không riêng "C": "system", "cdecl", "stdcall".
        // Không ghi ABI thì mặc định là "C": `extern { ... }`, `extern fn x()`.
        let after = match toks.get(i + 1).map(|t| &t.tok) {
            Some(Tok::Str(_)) => i + 2,
            _ => i + 1,
        };
        match toks.get(after).map(|t| &t.tok) {
            Some(Tok::Punct('{')) => {
                for j in after + 1..block_end(toks, after) {
                    if matches!(&toks[j].tok, Tok::Word(w) if w == "fn")
                        && let Some(Token { tok: Tok::Word(name), line }) = toks.get(j + 1)
                    {
                        out.calls.push(hit(name, *line));
                    }
                }
            }
            Some(Tok::Word(w)) if w == "fn" => {
                if let Some(Token { tok: Tok::Word(name), line }) = toks.get(after + 1) {
                    out.provides.push(hit(name, *line));
                }
            }
            _ => {}
        }
    }
    pyo3_modules(toks, &mut out);
    out
}

/// PyO3: `#[pymodule]` đặt trên `fn ten` hoặc `mod ten` là module mở rộng
/// tên `ten`, trừ khi `#[pyo3(name = "...")]` đổi tên nó.
///
/// Đọc cả dãy thuộc tính liền nhau, vì hai thuộc tính đó đứng theo thứ tự nào
/// cũng được: `#[pymodule(gil_used = false)]`, `#[pyo3(name = "_rust")]`,
/// rồi mới tới `pub mod inner`.
fn pyo3_modules(toks: &[Token], out: &mut Foreign) {
    let mut i = 0;
    while i < toks.len() {
        if !is_attribute_start(toks, i) {
            i += 1;
            continue;
        }
        let (mut is_module, mut rename) = (false, None);
        while is_attribute_start(toks, i) {
            let close = bracket_end(toks, i + 1);
            let body = &toks[i + 2..close.min(toks.len())];
            match body.first().map(|t| &t.tok) {
                Some(Tok::Word(w)) if w == "pymodule" || w == "pyo3::pymodule" => is_module = true,
                Some(Tok::Word(w)) if w == "pyo3" => rename = attribute_name(body).or(rename),
                _ => {}
            }
            i = close + 1;
        }
        if !is_module {
            continue;
        }
        // `pub`, `pub(crate)`, `unsafe` có thể đứng trước `fn`/`mod`.
        let mut j = i;
        while let Some(t) = toks.get(j) {
            match &t.tok {
                Tok::Word(w) if w == "pub" || w == "unsafe" => j += 1,
                Tok::Punct('(') if j > i => j = paren_end(toks, j) + 1,
                _ => break,
            }
        }
        if matches!(toks.get(j).map(|t| &t.tok), Some(Tok::Word(w)) if w == "fn" || w == "mod")
            && let Some(Token { tok: Tok::Word(name), line }) = toks.get(j + 1)
        {
            out.provides.push(module(rename.as_deref().unwrap_or(name), *line));
        }
    }
}

fn is_attribute_start(toks: &[Token], i: usize) -> bool {
    toks.get(i).is_some_and(|t| t.tok == Tok::Punct('#'))
        && toks.get(i + 1).is_some_and(|t| t.tok == Tok::Punct('['))
}

/// `name = "..."` trong `#[pyo3(...)]`.
fn attribute_name(body: &[Token]) -> Option<String> {
    body.windows(3).find_map(|w| match (&w[0].tok, &w[1].tok, &w[2].tok) {
        (Tok::Word(k), Tok::Punct('='), Tok::Str(v)) if k == "name" => Some(v.clone()),
        _ => None,
    })
}

/// Vị trí dấu `]` đóng lại dấu `[` ở `open`, hoặc hết dãy token.
fn bracket_end(toks: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.tok {
            Tok::Punct('[') => depth += 1,
            Tok::Punct(']') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    toks.len()
}

/// Vị trí dấu `}` đóng lại dấu `{` ở `open`, hoặc hết dãy token.
fn block_end(toks: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.tok {
            Tok::Punct('{') => depth += 1,
            Tok::Punct('}') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    toks.len()
}

// ---- Go ----

/// cgo: `import "C"` rồi gọi `C.ten_ham`. Những tên không phải hàm của dự án
/// (`C.int`, `C.CString`) cũng lọt vào, nhưng chúng không khớp định nghĩa nào
/// trong repo nên tự rụng ở bước ghép.
fn go(toks: &[Token]) -> Foreign {
    let imports_c = toks.windows(2).any(|w| {
        matches!(&w[0].tok, Tok::Word(k) if k == "import")
            && matches!(&w[1].tok, Tok::Str(s) if s == "C")
    });
    if !imports_c {
        return Foreign::default();
    }
    let mut out = Foreign::default();
    for w in toks.windows(3) {
        if matches!(&w[0].tok, Tok::Word(p) if p == "C")
            && matches!(w[1].tok, Tok::Punct('.'))
            && let Tok::Word(name) = &w[2].tok
        {
            out.calls.push(hit(name, w[2].line));
        }
    }
    out
}

// ---- Python ----

/// `torch.ops.ns.op(...)`: lời gọi một op PyTorch, thứ có thể do C++ của chính
/// repo này đăng ký. Lexer của Python đã gộp cả đường dẫn chấm thành một từ.
///
/// Gán `ops = torch.ops._C` rồi gọi `ops.x` thì không nhận ra: bỏ sót, chứ
/// không đoán.
fn python(toks: &[Token]) -> Foreign {
    let mut out = Foreign::default();
    for t in toks {
        let Tok::Word(w) = &t.tok else { continue };
        let Some(rest) = w.strip_prefix("torch.ops.") else { continue };
        let mut parts = rest.split('.');
        if let (Some(ns), Some(op)) = (parts.next(), parts.next())
            && !ns.is_empty()
            && !op.is_empty()
        {
            out.calls.push(Crossing {
                symbol: format!("op:{ns}.{op}"),
                shown: format!("torch.ops.{ns}.{op}"),
                line: t.line,
            });
        }
    }
    out
}

// ---- C và C++ ----

/// Từ khoá theo sau là `(` nhưng không phải lời gọi hàm.
const C_CONTROL: &[&str] =
    &["if", "while", "for", "switch", "return", "sizeof", "catch", "defined", "static_assert"];

/// Định nghĩa hàm ở mức ngoài cùng: `kiểu ten(tham số) {`.
///
/// Chỉ xét khi độ sâu ngoặc nhọn bằng 0, nên lời gọi hàm trong thân hàm khác
/// không bị nhận nhầm. Khai báo trước (`int f(void);`) cũng không, vì nó kết
/// thúc bằng `;` chứ không phải `{`.
///
/// Khối `extern "C" { ... }` không tính là một tầng: mã C++ đưa hàm ra cho C
/// gọi thường bọc chúng trong đó, và chính khối đó giữ tên không bị mangle.
/// `namespace` thì có tính — hàm C++ trong đó mang tên đã mangle, Rust hay Go
/// không liên kết tới được.
fn c(toks: &[Token]) -> Foreign {
    let mut out = Foreign::default();
    let mut depth = 0usize;
    // Mỗi `{` đang mở: có phải khối `extern "C"` không.
    let mut open: Vec<bool> = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        match toks[i].tok {
            Tok::Punct('{') => {
                let extern_block = i >= 2
                    && matches!(&toks[i - 2].tok, Tok::Word(w) if w == "extern")
                    && matches!(toks[i - 1].tok, Tok::Str(_));
                open.push(extern_block);
                if !extern_block {
                    depth += 1;
                }
            }
            // Đóng khối `extern "C"` thì độ sâu không đổi.
            Tok::Punct('}') => depth = depth.saturating_sub(usize::from(open.pop() == Some(false))),
            _ => {}
        }
        if depth > 0 {
            i += 1;
            continue;
        }
        let Tok::Word(name) = &toks[i].tok else {
            i += 1;
            continue;
        };
        if C_CONTROL.contains(&name.as_str())
            || !matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Punct('(')))
        {
            i += 1;
            continue;
        }
        // Phải có kiểu trả về đứng trước, nếu không thì đây là lời gọi ở mức
        // ngoài cùng — chẳng hạn trong một macro.
        let has_type = matches!(
            toks.get(i.wrapping_sub(1)).map(|t| &t.tok),
            Some(Tok::Word(_) | Tok::Punct('*'))
        ) && i > 0;
        let close = paren_end(toks, i + 1);
        if has_type && !is_macro_call(toks, i) && matches!(toks.get(close + 1).map(|t| &t.tok), Some(Tok::Punct('{'))) {
            out.provides.push(hit(name, toks[i].line));
            // Mọi module mở rộng viết bằng C API của CPython bắt đầu ở
            // `PyInit_<tên>`; pybind11 và nanobind cũng sinh ra đúng hàm đó.
            if let Some(module_name) = name.strip_prefix("PyInit_").filter(|m| !m.is_empty()) {
                out.provides.push(module(module_name, toks[i].line));
            }
        }
        i = close.max(i) + 1;
    }
    binding_modules(toks, &mut out);
    torch_ops(toks, &mut out);
    out
}

/// `PYBIND11_MODULE(ten, m)` của pybind11 và `NB_MODULE(ten, m)` của
/// nanobind: macro sinh ra `PyInit_ten`, nên tên đó là tên module mà Python
/// import.
fn binding_modules(toks: &[Token], out: &mut Foreign) {
    for w in toks.windows(4) {
        if matches!(&w[0].tok, Tok::Word(m) if m == "PYBIND11_MODULE" || m == "NB_MODULE")
            && w[1].tok == Tok::Punct('(')
            && w[3].tok == Tok::Punct(',')
            && let Tok::Word(name) = &w[2].tok
        {
            // `PYBIND11_MODULE(TORCH_EXTENSION_NAME, m)`: tên do `setup.py` đặt.
            let name = if name == "TORCH_EXTENSION_NAME" { EXTENSION_NAME } else { name };
            out.provides.push(module(name, w[2].line));
        }
    }
}

/// Op của PyTorch: `m.def("ten(...)")` trong khối `TORCH_LIBRARY(ns, m) { }`.
///
/// Mọi biến thể đều tính: `TORCH_LIBRARY_FRAGMENT`, `STABLE_TORCH_LIBRARY`,
/// cả macro bọc lại của dự án như `TORCH_LIBRARY_EXPAND` của vllm. Trừ
/// `TORCH_LIBRARY_IMPL`: nó chỉ gắn kernel cho một op đã định nghĩa ở chỗ
/// khác, và tính nó thì hai component cùng "cung cấp" một op.
///
/// Namespace là đối số đầu. Viết thẳng thì ghép theo nó. `TORCH_EXTENSION_NAME`
/// thì giá trị do `setup.py` cho biết, nếu cho biết chắc — xem
/// [`crate::resolve::Resolver::bind_extension`]. Macro khác
/// (`CONCAT(TORCH_EXTENSION_NAME, _cache_ops)`) thì chỉ hệ thống build biết, và
/// cả khối bị bỏ qua. Đã thử đoán macro là một module mở rộng mà Python import:
/// trên vllm, nơi có cả chục module như vậy,
/// `torch.ops._vllm_fa3_C.get_scheduler_metadata` (của flash-attention, tải lúc
/// build) ghép nhầm vào op CPU cùng tên.
fn torch_ops(toks: &[Token], out: &mut Foreign) {
    for i in 0..toks.len() {
        let Tok::Word(w) = &toks[i].tok else { continue };
        if !w.contains("TORCH_LIBRARY") || w.contains("IMPL") {
            continue;
        }
        if toks.get(i + 1).map(|t| &t.tok) != Some(&Tok::Punct('(')) {
            continue;
        }
        let close = paren_end(toks, i + 1);
        if toks.get(close + 1).map(|t| &t.tok) != Some(&Tok::Punct('{')) {
            continue; // `#define TORCH_LIBRARY_EXPAND(NAME, M) TORCH_LIBRARY(NAME, M)`
        }
        let namespace = match &toks[i + 2..close.min(toks.len())] {
            [Token { tok: Tok::Word(ns), .. }, Token { tok: Tok::Punct(','), .. }, ..]
                if !is_macro_name(ns) =>
            {
                ns.as_str()
            }
            // Riêng `TORCH_EXTENSION_NAME` thì `setup.py` có thể cho biết giá trị.
            [Token { tok: Tok::Word(ns), .. }, Token { tok: Tok::Punct(','), .. }, ..]
                if ns == "TORCH_EXTENSION_NAME" =>
            {
                EXTENSION_NAME
            }
            _ => continue,
        };
        let end = block_end(toks, close + 1);
        for j in close + 2..end {
            if toks[j].tok != Tok::Punct('.')
                || !matches!(toks.get(j + 1).map(|t| &t.tok), Some(Tok::Word(d)) if d == "def")
                || toks.get(j + 2).map(|t| &t.tok) != Some(&Tok::Punct('('))
            {
                continue;
            }
            // Schema có thể bọc trong một macro: `m.def(TORCH_SELECTIVE_SCHEMA("..."))`.
            let schema = match (toks.get(j + 3), toks.get(j + 4), toks.get(j + 5)) {
                (Some(Token { tok: Tok::Str(s), line }), _, _) => Some((s, *line)),
                (Some(Token { tok: Tok::Word(_), .. }), Some(Token { tok: Tok::Punct('('), .. }), Some(Token { tok: Tok::Str(s), line })) => {
                    Some((s, *line))
                }
                _ => None,
            };
            if let Some((schema, line)) = schema
                && let Some((ns, op)) = op_name(schema)
            {
                let ns = ns.unwrap_or(namespace);
                out.provides.push(Crossing {
                    symbol: format!("op:{ns}.{op}"),
                    shown: format!("{ns}::{op}"),
                    line,
                });
            }
        }
    }
}

/// `"mylib::scaled.out(Tensor a) -> Tensor"` ra `(Some("mylib"), "scaled")`:
/// tên đứng trước `(`, bỏ namespace nếu có và tên overload sau dấu chấm.
fn op_name(schema: &str) -> Option<(Option<&str>, &str)> {
    let head = schema.split('(').next()?.trim();
    let (ns, name) = match head.rsplit_once("::") {
        Some((ns, name)) => (Some(ns), name),
        None => (None, head),
    };
    let name = name.split('.').next()?;
    let ident = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
    (ident(name) && ns.is_none_or(ident)).then_some((ns, name))
}

/// Tên viết hoa toàn bộ, có từ hai chữ cái trở lên: quy ước của macro.
/// `_C` thì không — đó là namespace thật mà nhiều extension dùng.
fn is_macro_name(name: &str) -> bool {
    !name.bytes().any(|b| b.is_ascii_lowercase()) && name.bytes().filter(u8::is_ascii_uppercase).count() >= 2
}

/// `s32 BPF_STRUCT_OPS(name, …) {` trong chương trình eBPF trông y như một
/// định nghĩa hàm, nhưng hàm thật là `name`, còn `BPF_STRUCT_OPS` ở phía Go lại
/// là một hằng enum. Hai dấu hiệu cùng lúc: tên viết hoa toàn bộ (quy ước của
/// macro), và đối số đầu là một tên trần chứ không phải tham số có kiểu.
/// `SHA256(const unsigned char *d, …)` của OpenSSL là hàm thật, và vẫn được
/// tính.
fn is_macro_call(toks: &[Token], i: usize) -> bool {
    let Tok::Word(name) = &toks[i].tok else { return false };
    let upper = name.bytes().any(|b| b.is_ascii_uppercase())
        && !name.bytes().any(|b| b.is_ascii_lowercase());
    upper
        && matches!(toks.get(i + 2).map(|t| &t.tok), Some(Tok::Word(w)) if w != "void")
        && matches!(toks.get(i + 3).map(|t| &t.tok), Some(Tok::Punct(',' | ')')))
}

fn paren_end(toks: &[Token], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    toks.len()
}
