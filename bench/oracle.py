#!/usr/bin/env python3
"""Đối chiếu import archctl đọc được với parser chuẩn của từng ngôn ngữ.

    python3 bench/oracle.py [--archctl <binary>] [--only flask,curl] [--require clang,node,go,syn]

Phép nhất quán của `bench.py` so archctl với chính nó: bộ khám phá và bộ kiểm
dùng chung một bộ trích, nên một lỗi trong bộ trích làm cả hai sai giống nhau
và vẫn ra "0 bất nhất". Đối chiếu ngày 2026-10-04 tìm ra như vậy: Vue core mất
21% import tĩnh, Python có 1 045 import giả từ `raise ... from e`, một dòng
`#error` nối dòng làm cả `redis/src/tls.c` bị đọc ngược. Script này giữ cho
chúng không quay lại.

Thước đo, chạy trên repo mà `bench.py` đã tải về `target/bench/repos/`:

- Python: `ast` của chính trình thông dịch đang chạy script này. File không
  parse được (cú pháp mới hơn, Python 2) được đếm riêng, không đối chiếu.
- C/C++: lexer thô của clang (`-dump-raw-tokens`), không tiền xử lý — giống
  archctl, `#include` trong nhánh `#if` nào cũng tính.
- JS/TS: parser module của Node (`vm.SourceTextModule`); TS gỡ kiểu bằng
  `module.stripTypeScriptTypes` trước. Chỉ import tĩnh, nên chỉ kiểm phía
  thiếu: `import()`, `require()` và `import type` archctl đọc thêm là đúng.
  `.tsx`, `.jsx`, `.vue` không có thước đo.
- Go: `go/parser` ở chế độ `ImportsOnly`, dựng một lần thành binary tạm. Như
  archctl, không xét build tag: file nào cũng tính, `import "C"` cũng là import.

- Tên máy viết thẳng trong chuỗi (`scan --json`, mục `hosts`) của file Python
  và Go: chuỗi và chỗ nó đứng theo `ast`, `tokenize`, `go/ast`, luật đổi thành
  tên máy viết lại theo đặc tả — xem `code_hosts.py`. JS/TS chưa có thước đo.

- Rust: `syn` (bản ghim cứng trong SYN_MANIFEST), dựng thành binary dưới
  `target/bench/syn-oracle/`. Phải tải crate từ crates.io, nên chỉ chạy khi
  `--require syn`; CI bật nó, máy dev mặc định BỎ QUA. Cây `use` được trải ra
  giống archctl (`{self}` là tiền tố, `*` là chính tiền tố, `self`/`super`
  trong `mod x { }` đổi về module của file). `syn` không đọc vào thân macro,
  nên import archctl đọc được bên trong `macro_rules!`, `quote! { }`... không
  tính là thừa; chúng được đếm riêng.

- Notebook (`.ipynb`): `TransformerManager` của IPython biến đổi từng ô code
  đúng như Jupyter, rồi `ast` đọc import; dòng của từng phần tử `source` lấy
  khi duyệt JSON bằng `scanstring` chuẩn — xem `notebook_truth.py`. IPython
  (ghim cứng kèm hash trong `ipython-requirements.txt`) cài vào venv dưới
  `target/bench/ipython-oracle/`, nên chỉ chạy khi `--require ipython`; CI bật
  nó. Số dòng chỉ so khi mỗi dòng mã là một chuỗi JSON riêng, ở những dòng JSON
  tăng dần — như mọi notebook Jupyter ghi ra — và IPython không gộp dòng; còn
  lại archctl chỉ hứa gần đúng, nên chỉ so tên.

- Binding (cạnh Python ↔ native): `scan --json` in khoá thô của từng file
  (`py:tên`, `op:ns.tên`, `@ext` thay cho `TORCH_EXTENSION_NAME`). Phía C/C++
  đọc lại trên token thô của clang, ngoài chỉ thị tiền xử lý:
  `PYBIND11_MODULE`/`NB_MODULE`, định nghĩa `PyInit_x` ở mức ngoài cùng,
  `m.def("…")` trong khối `TORCH_LIBRARY*` (chuỗi kề nhau được nối như trình
  biên dịch). Phía Python là chuỗi thuộc tính `torch.ops.ns.op` theo `ast`.
  PyO3 là `#[pymodule]` theo `syn`, nên chỉ khi `--require syn`. Hàm C của FFI
  (`extern "C"`, cgo) không thuộc phép này.

Python, notebook, binding, C/C++, Go, Rust: thiếu, thừa và lệch số dòng đều là lệch. Thiếu clang, Node hay Go
thì phần đó in BỎ QUA và không tính là đạt; `--require` biến việc thiếu thành
lỗi, và với Node thì đòi cả khả năng gỡ kiểu TS. Một thước đo không parse được
quá nửa số file của một repo là thước đo hỏng chứ không phải mã hỏng: dừng,
không cho "đạt". Mã thoát: 0 khớp hết, 1 có lệch, 2 không chạy tới được (repo
chưa tải, archctl hay thước đo hỏng, thiếu công cụ bắt buộc).
"""
import argparse
import ast
import collections
import concurrent.futures
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import warnings
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))

import bench  # noqa: E402
import code_hosts  # noqa: E402

EXAMPLES = 5
# Quá tỉ lệ này số file không parse được thì nghi chính thước đo.
MAX_UNPARSED = 0.5
PY_EXTS = (".py", ".pyi")
NODE_EXTS = (".js", ".mjs", ".ts", ".mts", ".cts")

# Một tiến trình Node cho mỗi repo: đọc "<gốc>\t<file>" từ stdin, in
# "<file>\t<json các specifier>", "ERR" (không parse được) hay "NOTS" (Node
# này không gỡ được kiểu TS).
NODE_SCRIPT = r"""
import * as mod from 'node:module';
import vm from 'node:vm';
import fs from 'node:fs';
import readline from 'node:readline';
const strip = mod.stripTypeScriptTypes;
for await (const line of readline.createInterface({ input: process.stdin })) {
  const [root, file] = line.split('\t');
  const ts = /\.(ts|mts|cts)$/.test(file);
  if (ts && !strip) { console.log(`${file}\tNOTS`); continue; }
  try {
    let code = fs.readFileSync(`${root}/${file}`, 'utf8');
    if (ts) code = strip(code, { mode: 'transform' });
    const m = new vm.SourceTextModule(code, { identifier: file });
    const specs = m.moduleRequests ? m.moduleRequests.map(r => r.specifier) : m.dependencySpecifiers;
    console.log(`${file}\t${JSON.stringify(specs)}`);
  } catch (e) {
    console.log(`${file}\tERR`);
  }
}
"""

# Một tiến trình cho mỗi repo, cùng giao thức với NODE_SCRIPT: in
# "<file>\t<json các [dòng, đường dẫn]>" hay "ERR".
GO_SOURCE = r"""
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"go/parser"
	"go/token"
	"os"
	"strconv"
	"strings"
)

func main() {
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<20)
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	for in.Scan() {
		root, file, _ := strings.Cut(in.Text(), "\t")
		fset := token.NewFileSet()
		f, err := parser.ParseFile(fset, root+"/"+file, nil, parser.ImportsOnly)
		if err != nil {
			fmt.Fprintf(out, "%s\tERR\n", file)
			continue
		}
		specs := [][2]any{}
		for _, imp := range f.Imports {
			path, err := strconv.Unquote(imp.Path.Value)
			if err != nil {
				path = imp.Path.Value
			}
			specs = append(specs, [2]any{fset.Position(imp.Path.Pos()).Line, path})
		}
		js, _ := json.Marshal(specs)
		fmt.Fprintf(out, "%s\t%s\n", file, js)
	}
}
"""

# Không có Cargo.lock: mọi crate trong cây đều ghim `=`, nên cargo không có gì
# để chọn. `[workspace]` rỗng để cargo không coi nó thuộc workspace của repo.
SYN_MANIFEST = """
[package]
name = "syn-oracle"
version = "0.0.0"
edition = "2021"
publish = false

[dependencies]
syn = { version = "=2.0.87", default-features = false, features = ["full", "parsing", "visit"] }
proc-macro2 = { version = "=1.0.89", default-features = false, features = ["span-locations"] }
unicode-ident = "=1.0.13"

[workspace]
"""

# Cùng giao thức với GO_SOURCE, nhưng mỗi dòng trả về là
# {"imports": [[dòng, đường dẫn]], "macros": [[dòng đầu, dòng cuối]]}.
SYN_SOURCE = r"""
use std::io::{BufRead, Write};
use syn::visit::{self, Visit};
use syn::UseTree;

#[derive(Default)]
struct V {
    inline: Vec<String>,
    imports: Vec<(usize, String)>,
    macros: Vec<(usize, usize)>,
    /// Module mở rộng PyO3: `(dòng của tên, tên Python thấy)`.
    pymodules: Vec<(usize, String)>,
}

/// `#[pymodule]` (hay `#[pyo3::pymodule]`) có trong các thuộc tính không, và
/// `#[pyo3(name = "...")]` đổi tên thành gì.
fn pymodule(attrs: &[syn::Attribute]) -> Option<Option<String>> {
    let is_module = attrs.iter().any(|a| a.path().segments.last().is_some_and(|s| s.ident == "pymodule"));
    if !is_module {
        return None;
    }
    let mut rename = None;
    for a in attrs.iter().filter(|a| a.path().is_ident("pyo3")) {
        let _ = a.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let lit: syn::LitStr = meta.value()?.parse()?;
                rename = Some(lit.value());
            } else if meta.input.peek(syn::Token![=]) {
                let _: syn::Expr = meta.value()?.parse()?;
            }
            Ok(())
        });
    }
    Some(rename)
}

fn join(prefix: &str, word: &str) -> String {
    if prefix.is_empty() { word.to_string() } else { format!("{prefix}::{word}") }
}

/// Bản sao của `from_file_module` trong `scan/rustlang.rs`.
fn from_file_module(path: &str, inline: &[String]) -> String {
    if inline.is_empty() {
        return path.to_string();
    }
    let segs: Vec<&str> = path.split("::").collect();
    let (supers, rest) = match segs[0] {
        "super" => {
            let n = segs.iter().take_while(|s| **s == "super").count();
            (n, &segs[n..])
        }
        "self" => (0, &segs[1..]),
        _ => return path.to_string(),
    };
    let out: Vec<&str> = if supers > inline.len() {
        std::iter::repeat_n("super", supers - inline.len()).chain(rest.iter().copied()).collect()
    } else {
        std::iter::once("self")
            .chain(inline[..inline.len() - supers].iter().map(String::as_str))
            .chain(rest.iter().copied())
            .collect()
    };
    out.join("::")
}

impl V {
    fn push(&mut self, line: usize, path: String) {
        if !path.is_empty() {
            let path = from_file_module(&path, &self.inline);
            self.imports.push((line, path));
        }
    }

    /// `line` là dòng của từ đầu tiên trong đoạn đường dẫn đang đọc: archctl
    /// gộp `a::b::c` thành một token và báo dòng của token đó.
    fn tree(&mut self, t: &UseTree, prefix: &str, line: Option<usize>) {
        match t {
            UseTree::Path(p) => {
                let l = line.unwrap_or(p.ident.span().start().line);
                self.tree(&p.tree, &join(prefix, &p.ident.to_string()), Some(l));
            }
            UseTree::Name(n) => self.leaf(prefix, &n.ident, line),
            UseTree::Rename(r) => self.leaf(prefix, &r.ident, line),
            UseTree::Glob(g) => {
                let l = line.unwrap_or(g.star_token.spans[0].start().line);
                self.push(l, prefix.to_string());
            }
            UseTree::Group(g) => {
                for item in &g.items {
                    self.tree(item, prefix, None);
                }
            }
        }
    }

    fn leaf(&mut self, prefix: &str, ident: &syn::Ident, line: Option<usize>) {
        let l = line.unwrap_or(ident.span().start().line);
        let word = ident.to_string();
        let path = if word == "self" && !prefix.is_empty() { prefix.to_string() } else { join(prefix, &word) };
        self.push(l, path);
    }
}

impl<'a> Visit<'a> for V {
    fn visit_item_use(&mut self, u: &'a syn::ItemUse) {
        self.tree(&u.tree, "", None);
    }

    fn visit_item_extern_crate(&mut self, e: &'a syn::ItemExternCrate) {
        self.push(e.extern_token.span.start().line, e.ident.to_string());
    }

    fn visit_item_fn(&mut self, f: &'a syn::ItemFn) {
        if let Some(rename) = pymodule(&f.attrs) {
            let name = rename.unwrap_or_else(|| f.sig.ident.to_string());
            self.pymodules.push((f.sig.ident.span().start().line, name));
        }
        visit::visit_item_fn(self, f);
    }

    fn visit_item_mod(&mut self, m: &'a syn::ItemMod) {
        if let Some(rename) = pymodule(&m.attrs) {
            let name = rename.unwrap_or_else(|| m.ident.to_string());
            self.pymodules.push((m.ident.span().start().line, name));
        }
        let inline = m.content.is_some();
        if inline {
            self.inline.push(m.ident.to_string());
        }
        visit::visit_item_mod(self, m);
        if inline {
            self.inline.pop();
        }
    }

    fn visit_macro(&mut self, m: &'a syn::Macro) {
        let start = m.bang_token.span.start().line;
        let end = m.delimiter.span().close().end().line;
        self.macros.push((start, end));
        visit::visit_macro(self, m);
    }
}

fn main() {
    let stdin = std::io::stdin();
    let mut out = std::io::BufWriter::new(std::io::stdout());
    for row in stdin.lock().lines() {
        let row = row.unwrap();
        let (root, file) = row.split_once('\t').unwrap();
        let parsed = std::fs::read_to_string(format!("{root}/{file}"))
            .ok()
            .and_then(|src| syn::parse_file(&src).ok());
        let Some(ast) = parsed else {
            writeln!(out, "{file}\tERR").unwrap();
            continue;
        };
        let mut v = V::default();
        v.visit_file(&ast);
        let imports: Vec<String> = v
            .imports
            .iter()
            .map(|(l, p)| format!("[{l},\"{}\"]", p.replace('\\', "\\\\").replace('"', "\\\"")))
            .collect();
        let macros: Vec<String> = v.macros.iter().map(|(a, b)| format!("[{a},{b}]")).collect();
        let pymodules: Vec<String> =
            v.pymodules.iter().map(|(l, n)| format!("[{l},\"py:{}\"]", n.replace('\\', "\\\\").replace('"', "\\\""))).collect();
        writeln!(
            out,
            "{file}\t{{\"imports\":[{}],\"macros\":[{}],\"pymodules\":[{}]}}",
            imports.join(","),
            macros.join(","),
            pymodules.join(",")
        )
        .unwrap();
    }
}
"""

CLANG_TOKEN = re.compile(r"^(\w+) '(.*)'\t( \[StartOfLine\])?.*Loc=<.*:(\d+):\d+>$", re.S)


def clang_rows(dump):
    """Mỗi token một bản ghi. Token chứa xuống dòng (raw string, chuỗi có `\`
    nối dòng, khoảng trắng nhiều dòng) bị clang in ra trên nhiều dòng; dòng
    không mở đầu bằng `loại '` là phần tiếp của token trước."""
    rows = []
    for line in dump.split("\n"):
        if rows and not re.match(r"^\w+ '", line):
            rows[-1] += "\n" + line
        else:
            rows.append(line)
    return rows
INCLUDE_SPEC = re.compile(r'#\s*include\s*([<"])([^>"\n]+)[>"]')


def archctl_scan(archctl, root):
    try:
        p = subprocess.run([archctl, "scan", "--json", "--all-hosts", "--root", str(root)],
                           capture_output=True, text=True, timeout=900)
    except subprocess.TimeoutExpired:
        bench.die(f"archctl scan ở {root}: quá 900 giây")
    if p.returncode != 0:
        bench.die(f"archctl scan ở {root} thoát {p.returncode}: {p.stderr.strip()[:300]}")
    try:
        return [f for f in json.loads(p.stdout)["files"] if "skipped" not in f]
    except (ValueError, KeyError) as e:
        bench.die(f"archctl scan ở {root}: JSON hỏng ({e})")


def python_truth(path):
    """`(import, op)` theo `ast`: Counter `(dòng, spec)` của import và
    `(dòng, op:ns.tên)` của mọi `torch.ops.ns.tên`; None nếu không parse được."""
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            tree = ast.parse(path.read_bytes())
        except (SyntaxError, ValueError):
            return None
    imports, ops = collections.Counter(), collections.Counter()
    inner = set()  # Attribute nằm trong một chuỗi chấm dài hơn
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                imports[(node.lineno, alias.name)] += 1
        elif isinstance(node, ast.ImportFrom):
            imports[(node.lineno, "." * node.level + (node.module or ""))] += 1
        elif isinstance(node, ast.Attribute) and isinstance(node.value, ast.Attribute):
            inner.add(id(node.value))
    for node in ast.walk(tree):
        if not isinstance(node, ast.Attribute) or id(node) in inner:
            continue
        parts, cur = [], node
        while isinstance(cur, ast.Attribute):
            parts.append(cur.attr)
            cur = cur.value
        if isinstance(cur, ast.Name):
            parts.append(cur.id)
            parts.reverse()
            if len(parts) >= 4 and parts[:2] == ["torch", "ops"]:
                ops[(node.lineno, f"op:{parts[2]}.{parts[3]}")] += 1
    return imports, ops


# Dấu hiệu một file C/C++ có thể định nghĩa module mở rộng hay op PyTorch.
BINDING_MARKERS = ("PYBIND11_MODULE", "NB_MODULE", "TORCH_LIBRARY", "PyInit_")
EXTENSION_NAME = "@ext"


def clang_tokens(path):
    """Token thô của clang ngoài chỉ thị tiền xử lý, kèm dòng `#include`.

    Trả về `(include, token)`: Counter `(dòng, spec)` và danh sách
    `(loại, chữ, dòng)`. Đếm dòng như clang: chỉ theo `\n`; `splitlines()` tách
    cả ở form feed (`^L` hay gặp trong mã kiểu GNU) và lệch số dòng.
    """
    text = path.read_text(errors="replace")
    if "include" not in text and not any(m in text for m in BINDING_MARKERS):
        return collections.Counter(), []
    lang = ["-x", "c", "-std=c2x"] if path.suffix == ".c" else ["-x", "c++", "-std=c++20"]
    p = subprocess.run(["clang", "-fsyntax-only", *lang, "-Xclang", "-dump-raw-tokens", str(path)],
                       capture_output=True, text=True, errors="replace", timeout=300)
    if p.returncode != 0:
        bench.die(f"clang thoát {p.returncode} ở {path}: {p.stderr.strip()[-300:]}")
    lines = text.split("\n")
    includes, tokens = collections.Counter(), []
    hash_line, in_directive = None, False
    for row in clang_rows(p.stderr):
        m = CLANG_TOKEN.match(row)
        if not m or m.group(1) in ("unknown", "comment"):
            continue
        kind, tok, start, line = m.group(1), m.group(2), m.group(3), int(m.group(4))
        if start:
            in_directive = False
        if kind == "hash" and start:
            hash_line, in_directive = line, True
            continue
        if hash_line == line and kind == "raw_identifier" and tok == "include":
            spec = INCLUDE_SPEC.search(lines[line - 1]) if line <= len(lines) else None
            if spec:
                includes[(line, spec.group(2))] += 1
        hash_line = None
        # Token trong `#define`, `#if`... không phải mã: `#define
        # PYBIND11_MODULE(name, m)` không định nghĩa module nào.
        if not in_directive:
            tokens.append((kind, tok, line))
    return includes, tokens


def c_string(tok):
    """Nội dung một string literal của C/C++: `"a\"b"`, `u8"…"`, `R"(…)"`."""
    m = re.match(r'^(?:u8|u|U|L)?R"([^(]*)\((.*)\)\1"$', tok, re.S)
    if m:
        return m.group(2)
    m = re.match(r'^(?:u8|u|U|L)?"(.*)"$', tok, re.S)
    if not m:
        return None
    # `\` cuối dòng nối dòng, như trình biên dịch; rồi mới giải ký tự thoát.
    return re.sub(r"\\(.)", r"\1", re.sub(r"\\\r?\n", "", m.group(1)))


def is_macro_name(name):
    return name.upper() == name and sum(c.isalpha() for c in name) >= 2


def binding_truth(tokens):
    """Module mở rộng và op PyTorch mà file C/C++ định nghĩa: Counter
    `(dòng, py:tên | op:ns.tên)`, đọc trên token thô của clang."""
    out = collections.Counter()
    close, opener = {}, {}
    stack = []
    for i, (kind, _, _) in enumerate(tokens):
        if kind in ("l_paren", "l_brace", "l_square"):
            stack.append(i)
        elif kind in ("r_paren", "r_brace", "r_square") and stack:
            o = stack.pop()
            close[o], opener[i] = i, o
    word = lambda i: tokens[i][1] if i < len(tokens) and tokens[i][0] == "raw_identifier" else None
    kind = lambda i: tokens[i][0] if i < len(tokens) else None
    depth, extern_blocks = 0, set()
    for i, (k, tok, line) in enumerate(tokens):
        # Độ sâu ngoặc nhọn, trừ khối `extern "C" { }`.
        if k == "l_brace":
            if i >= 2 and word(i - 2) == "extern" and kind(i - 1) == "string_literal":
                extern_blocks.add(i)
            else:
                depth += 1
        elif k == "r_brace":
            if opener.get(i) not in extern_blocks:
                depth = max(depth - 1, 0)
        if k != "raw_identifier":
            continue
        # PYBIND11_MODULE(tên, m) / NB_MODULE(tên, m)
        if tok in ("PYBIND11_MODULE", "NB_MODULE") and kind(i + 1) == "l_paren" and word(i + 2) and kind(i + 3) == "comma":
            name = word(i + 2)
            out[(tokens[i + 2][2], "py:" + (EXTENSION_NAME if name == "TORCH_EXTENSION_NAME" else name))] += 1
        # Định nghĩa `PyInit_tên(...) {` ở mức ngoài cùng, có kiểu trả về đứng trước.
        elif (tok.startswith("PyInit_") and len(tok) > 7 and depth == 0 and kind(i + 1) == "l_paren"
              and i > 0 and (kind(i - 1) == "raw_identifier" or kind(i - 1) == "star")
              and close.get(i + 1) is not None and kind(close[i + 1] + 1) == "l_brace"):
            out[(line, "py:" + tok[len("PyInit_"):])] += 1
        # TORCH_LIBRARY*(ns, m) { m.def("op(...)") }, trừ *_IMPL
        elif "TORCH_LIBRARY" in tok and "IMPL" not in tok and kind(i + 1) == "l_paren" and (i + 1) in close:
            end_args = close[i + 1]
            if kind(end_args + 1) != "l_brace" or (end_args + 1) not in close:
                continue
            if not (word(i + 2) and kind(i + 3) == "comma"):
                continue
            ns = word(i + 2)
            if is_macro_name(ns):
                if ns != "TORCH_EXTENSION_NAME":
                    continue
                ns = EXTENSION_NAME
            # Biến module là đối số thứ hai; chỉ `.def` gọi trên nó (kể cả
            # chuỗi `m.def(…).def(…)`) là op, không phải `m.class_<T>(…).def`.
            if not (word(i + 4) and close[i + 1] == i + 5):
                continue
            module_var, is_op = word(i + 4), {}
            for j in range(end_args + 2, close[end_args + 1]):
                if not (kind(j) == "period" and word(j + 1) == "def" and kind(j + 2) == "l_paren"):
                    continue
                prev = j - 1
                if word(prev) is not None:
                    on_module = word(prev) == module_var
                elif kind(prev) == "r_paren" and prev in opener:
                    o = opener[prev]
                    on_module = o >= 2 and word(o - 1) == "def" and is_op.get(o - 2, False)
                else:
                    on_module = False
                is_op[j] = on_module
                if not on_module:
                    continue
                s = j + 3
                if word(s) and kind(s + 1) == "l_paren":  # TORCH_SELECTIVE_SCHEMA("…")
                    s += 2
                if kind(s) != "string_literal":
                    continue
                # Chuỗi kề nhau được nối, như trình biên dịch.
                parts, e = [], s
                while kind(e) == "string_literal":
                    parts.append(c_string(tokens[e][1]) or "")
                    e += 1
                head = "".join(parts).split("(", 1)[0].strip()
                op_ns, _, name = head.rpartition("::")
                name = name.split(".", 1)[0]
                if re.fullmatch(r"\w+", name) and (not op_ns or re.fullmatch(r"\w+", op_ns)):
                    out[(tokens[s][2], f"op:{op_ns or ns}.{name}")] += 1
    return out


def node_truth(root, files):
    """file → Counter các specifier tĩnh; None nếu không parse được hay không gỡ được kiểu."""
    feed = "".join(f"{root}\t{f}\n" for f in files)
    p = subprocess.run(["node", "--experimental-vm-modules", "--no-warnings", "--input-type=module",
                        "-e", NODE_SCRIPT], input=feed, capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"node thoát {p.returncode} ở {root}: {p.stderr.strip()[-300:]}")
    out = {}
    for row in p.stdout.splitlines():
        file, _, value = row.partition("\t")
        out[file] = None if value in ("ERR", "NOTS") else collections.Counter(json.loads(value))
    unanswered = sorted(set(files) - set(out))
    if unanswered:
        bench.die(f"node không trả lời cho {len(unanswered)} file ở {root}, ví dụ {unanswered[0]}")
    return out


def build_go_helper(workdir):
    src = Path(workdir) / "imports.go"
    src.write_text(GO_SOURCE)
    exe = Path(workdir) / "imports"
    p = subprocess.run(["go", "build", "-o", str(exe), str(src)], capture_output=True, text=True,
                       timeout=600, env={**os.environ, "GO111MODULE": "off", "GOFLAGS": ""})
    if p.returncode != 0:
        bench.die(f"không dựng được trình đọc import Go: {p.stderr.strip()[-300:]}")
    return str(exe)


def go_truth(helper, root, files):
    """file → Counter `(dòng, đường dẫn)`; None nếu go/parser không đọc được."""
    feed = "".join(f"{root}\t{f}\n" for f in files)
    p = subprocess.run([helper], input=feed, capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"trình đọc Go thoát {p.returncode} ở {root}: {p.stderr.strip()[-300:]}")
    out = {}
    for row in p.stdout.splitlines():
        file, _, value = row.partition("\t")
        out[file] = None if value == "ERR" else \
            collections.Counter((line, spec) for line, spec in json.loads(value))
    unanswered = sorted(set(files) - set(out))
    if unanswered:
        bench.die(f"trình đọc Go không trả lời cho {len(unanswered)} file ở {root}, ví dụ {unanswered[0]}")
    return out


def build_syn_helper(cache):
    """Dựng trình đọc `syn` dưới `cache/syn-oracle`; lần sau dùng lại bản đã dựng."""
    proj = Path(cache) / "syn-oracle"
    (proj / "src").mkdir(parents=True, exist_ok=True)
    for path, text in [(proj / "Cargo.toml", SYN_MANIFEST), (proj / "src/main.rs", SYN_SOURCE)]:
        if not path.exists() or path.read_text() != text:
            path.write_text(text)
    env = {**os.environ, "CARGO_NET_OFFLINE": "false", "CARGO_TARGET_DIR": str(proj / "target")}
    p = subprocess.run(["cargo", "build", "--release", "--quiet", "--manifest-path", str(proj / "Cargo.toml")],
                       capture_output=True, text=True, timeout=900, env=env)
    if p.returncode != 0:
        bench.die(f"không dựng được trình đọc `syn`: {p.stderr.strip()[-600:]}")
    return str(proj / "target/release/syn-oracle")


def syn_truth(helper, root, files):
    """file → (Counter `(dòng, đường dẫn)`, các khoảng dòng của macro, Counter `(dòng, py:tên)`
    của `#[pymodule]`); None nếu syn không đọc được."""
    feed = "".join(f"{root}\t{f}\n" for f in files)
    p = subprocess.run([helper], input=feed, capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"trình đọc `syn` thoát {p.returncode} ở {root}: {p.stderr.strip()[-300:]}")
    out = {}
    for row in p.stdout.splitlines():
        file, _, value = row.partition("\t")
        if value == "ERR":
            out[file] = None
            continue
        v = json.loads(value)
        out[file] = (collections.Counter((line, spec) for line, spec in v["imports"]), v["macros"],
                     collections.Counter((line, key) for line, key in v.get("pymodules", [])))
    unanswered = sorted(set(files) - set(out))
    if unanswered:
        bench.die(f"trình đọc `syn` không trả lời cho {len(unanswered)} file ở {root}, ví dụ {unanswered[0]}")
    return out


IPYTHON_REQUIREMENTS = HERE / "ipython-requirements.txt"


def ipython_version():
    m = re.search(r"^ipython==(\S+)", IPYTHON_REQUIREMENTS.read_text(), re.M)
    return m.group(1) if m else "?"


def build_venv(cache, name, requirements, label):
    """Venv dưới `cache/<name>`; lần sau dùng lại nếu bản ghim không đổi.

    Cài đúng file đã ghim kèm hash, chỉ wheel: gói bị chiếm quyền trên PyPI hay
    bị tráo trên đường đi thì pip từ chối, và không `setup.py` nào chạy lúc cài.
    """
    home = Path(cache) / name
    venv, installed = home / "venv", home / "installed.txt"
    python = venv / "bin" / "python"
    wanted = requirements.read_text()
    if not (python.exists() and installed.exists() and installed.read_text() == wanted):
        shutil.rmtree(venv, ignore_errors=True)
        home.mkdir(parents=True, exist_ok=True)
        for cmd in ([sys.executable, "-m", "venv", str(venv)],
                    [str(python), "-m", "pip", "install", "--quiet", "--disable-pip-version-check",
                     "--no-deps", "--only-binary", ":all:", "--require-hashes",
                     "--requirement", str(requirements)]):
            try:
                p = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
            except subprocess.TimeoutExpired:
                bench.die(f"dựng thước đo {label}: `{cmd[2]}` quá 600 giây")
            if p.returncode != 0:
                bench.die(f"dựng thước đo {label} hỏng: {(p.stderr or p.stdout).strip()[-400:]}")
        installed.write_text(wanted)
    return str(python)


def build_ipython_helper(cache):
    return build_venv(cache, "ipython-oracle", IPYTHON_REQUIREMENTS, "IPython")


DEPLOY_REQUIREMENTS = HERE / "deploy-requirements.txt"


def deploy_version():
    text = DEPLOY_REQUIREMENTS.read_text()
    found = [re.search(rf"^{n}==(\S+)", text, re.M) for n in ("pyyaml", "dockerfile-parse")]
    return " + ".join(f"{n} {m.group(1)}" for n, m in zip(("PyYAML", "dockerfile-parse"), found) if m)


def is_deploy_name(name):
    """Tên file compose hay Dockerfile, theo đúng luật của `scan/deploy.rs`."""
    lower = name.lower()
    stem = lower[len("docker-"):] if lower.startswith("docker-") else lower
    compose = (lower.endswith((".yml", ".yaml")) and stem.startswith("compose") and stem[7:8] in (".", "-", "_")
               and not any(p in ("test", "tests", "e2e", "ci") for p in re.split(r"[.\-_]", stem)))
    docker = (name in ("Dockerfile", "Containerfile") or name.startswith(("Dockerfile.", "Containerfile."))
              or name.endswith((".Dockerfile", ".dockerfile")))
    return compose or docker


def deploy_files(root):
    """File compose và Dockerfile mà git theo dõi, kể cả trong thư mục test hay
    ví dụ: càng nhiều file càng thử được nhiều bộ đọc."""
    p = subprocess.run(["git", "-C", str(root), "ls-files", "-z"], capture_output=True, text=True)
    # Repo tải sparse: git vẫn liệt kê file ngoài vùng đã tải.
    return sorted(f for f in p.stdout.split("\0")
                  if f and is_deploy_name(f.rsplit("/", 1)[-1]) and (root / f).is_file() and not (root / f).is_symlink())


def archctl_deploy(archctl, root, files):
    """`{file: Counter (dòng, dữ kiện)}` theo `archctl scan`, hay chuỗi lỗi."""
    # `--verbatim`: để so được đối số và argv. Đây là repo công khai của bộ đo.
    p = subprocess.run([archctl, "scan", "--json", "--verbatim", "--root", str(root), *files],
                       capture_output=True, text=True, timeout=600)
    if p.returncode != 0:
        bench.die(f"archctl scan hỏng ở {root.name}: {p.stderr.strip()[-300:]}")
    out = {}
    for f in json.loads(p.stdout)["deploy"]["files"]:
        if f.get("kind") in ("k8s", "skaffold"):
            continue
        if "error" in f:
            out[f["file"]] = f"ERR {f['error']}"
            continue
        facts = []
        if f["kind"] == "dockerfile":
            for i in f["instructions"]:
                facts.append((i["line"], f"{i['keyword']} {' '.join(i['args'].split())}".rstrip()))
        else:
            for path, line in f["includes"]:
                facts.append((line, f"include {path}"))
            for s in f["services"]:
                name, at = s["name"], s["line"]
                facts.append((at, f"service {name}"))
                if s["image"] is not None:
                    facts.append((at, f"{name}.image={s['image']}"))
                if s["workingDir"] is not None:
                    facts.append((at, f"{name}.working_dir={s['workingDir']}"))
                b = s["build"]
                if b is not None:
                    facts.append((b["line"], f"{name}.build context={b['context']} dockerfile={b['dockerfile']} "
                                             f"target={b['target']} inline={b['inline']}"))
                for key in ("command", "entrypoint"):
                    c = s[key]
                    if c is not None:
                        facts.append((c["line"], f"{name}.{key}={json.dumps(c['argv'], ensure_ascii=False)}"))
                for e in s["env"]:
                    facts.append((e["line"], f"{name}.env {e['key']}"))
                for path, line in s["envFiles"]:
                    facts.append((line, f"{name}.env_file {path}"))
                for dep, line in s["dependsOn"]:
                    facts.append((line, f"{name}.depends_on {dep}"))
                for dep, line in s["links"]:
                    facts.append((line, f"{name}.links {dep}"))
                for host, container, line in s["binds"]:
                    facts.append((line, f"{name}.bind {host}:{container}"))
                if s["extends"] is not None:
                    facts.append((at, f"{name}.extends {s['extends']}"))
                for a in s["aliases"]:
                    facts.append((at, f"{name}.alias {a}"))
                for shown, line, local in s.get("ports", []):
                    facts.append((line, f"{name}.port {shown} local={local}"))
        out[f["file"]] = collections.Counter(facts)
    return out


def deploy_truth(python, root, files):
    """`{file: Counter (dòng, dữ kiện)}` theo PyYAML và dockerfile-parse; chuỗi
    `ERR …` hay `SKIP …` khi không đo được."""
    p = subprocess.run([python, "-I", str(HERE / "deploy_truth.py")],
                       input="".join(f"{root}\t{f}\n" for f in files),
                       capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"thước đo hạ tầng hỏng: {p.stderr.strip()[-400:]}")
    out = {}
    for line in p.stdout.splitlines():
        file, res = line.split("\t", 1)
        if res.startswith(("ERR", "SKIP")):
            out[file] = res
        else:
            out[file] = collections.Counter((l, s) for l, s in json.loads(res)["facts"])
    return out


def yaml_files(root):
    """File YAML mà git theo dõi và có trên đĩa: ứng viên manifest Kubernetes,
    `skaffold.yaml`, `kustomization.yaml`, `Chart.yaml`."""
    p = subprocess.run(["git", "-C", str(root), "ls-files", "-z", "--", "*.yaml", "*.yml", "*.json"],
                       capture_output=True, text=True)
    return sorted(f for f in p.stdout.split("\0")
                  if f and (root / f).is_file() and not (root / f).is_symlink())


def archctl_k8s(archctl, root, files):
    """Dữ kiện từng manifest (`{file: Counter}` hay chuỗi lỗi) và cạnh
    (`{"<khoá>\\t<file>:<dòng>\\t<máy>": {đích}}`) theo `archctl scan`."""
    p = subprocess.run([archctl, "scan", "--json", "--verbatim", "--root", str(root), *files],
                       capture_output=True, text=True, timeout=600)
    if p.returncode != 0:
        bench.die(f"archctl scan hỏng ở {root.name}: {p.stderr.strip()[-300:]}")
    rep = json.loads(p.stdout)["deploy"]
    out = {}
    for f in rep["files"]:
        if f.get("kind") not in ("k8s", "skaffold"):
            continue
        if "error" in f:
            out[f["file"]] = f"ERR {f['error']}"
            continue
        facts = []
        for a in f.get("artifacts", []):
            facts.append((a["line"], f"artifact {a['image']} context={a['context']} dockerfile={a['dockerfile']} "
                                     f"target={a['target']}"))
        for s in f.get("services", []):
            facts.append((s["line"], f"Service {s['name']} ns={s['namespace']} selector={json.dumps(s['selector'])}"))
            if s.get("exposed"):
                facts.append((s["line"], f"Service {s['name']} exposed={s['exposed']}"))
        for r in f.get("routes", []):
            if not r["public"]:
                continue
            facts.append((r["line"], f"{r['kind']} {r['name']} ns={r['namespace']}"))
            facts += [(l, f"{r['name']}.backend {n} ns={ns}") for n, ns, l in r["services"]]
            facts += [(l, f"{r['name']}.host {h}") for h, l in r["hosts"]]
            facts += [(l, f"{r['name']}.delegate {d} ns={ns}") for d, ns, l in r["delegates"]]
        compact = lambda x: json.dumps(x, separators=(",", ":"), ensure_ascii=False)
        for pol in f.get("policies", []):
            name = pol["name"]
            facts.append((pol["line"], f"NetworkPolicy {name} ns={pol['namespace']} pods={compact(pol['pods'])} "
                                       f"ingress={pol['ingress'] is not None} egress={pol['egress'] is not None}"))
            for side in ("ingress", "egress"):
                for rule in pol[side] or []:
                    facts.append((rule["line"], f"{name}.{side}.rule"))
                    for p in rule["peers"]:
                        facts.append((p["line"], f"{name}.{side}.peer pods={compact(p['pods'])} "
                                                 f"namespaces={compact(p['namespaces'])} ip={p['ip']}"))
        for m in f.get("configMaps", []):
            facts.append((m["line"], f"ConfigMap {m['name']} ns={m['namespace']}"))
            facts += [(l, f"{m['name']}.data {k}") for k, l in m["keys"]]
        for kind, name, l in f.get("unknown", []):
            facts.append((l, f"unknown {kind} {name}"))
        for w in f.get("workloads", []):
            name = w["name"]
            facts.append((w["line"], f"{w['kind']} {name} ns={w['namespace']} labels={json.dumps(w['labels'])}"))
            if w.get("host"):
                facts.append((w["line"], f"{name}.host {w['host']}"))
            for c in w["containers"]:
                cn = c["name"]
                facts.append((c["line"], f"{name}.container {cn} init={c['init']} image={c['image']}"))
                for k in ("command", "args"):
                    if c[k] is not None:
                        facts.append((c[k]["line"], f"{name}.{cn}.{k}={json.dumps(c[k]['argv'], ensure_ascii=False)}"))
                if c["workingDir"] is not None:
                    facts.append((c["line"], f"{name}.{cn}.workingDir={c['workingDir']}"))
                facts += [(e["line"], f"{name}.{cn}.env {e['key']}") for e in c["env"]]
                facts += [(r["line"], f"{name}.{cn}.ref {r['key']}={r['configMap']}/{r['item']}") for r in c["configRefs"]]
                facts += [(e["line"], f"{name}.{cn}.envFrom {e['configMap']} prefix={e['prefix']}") for e in c["envFrom"]]
        out[f["file"]] = collections.Counter(facts)
    edges = collections.defaultdict(set)
    guards = {}
    exposed = {s["key"] for s in rep["services"] if s["key"].startswith("k8s:") and s.get("exposed")}
    for s in rep["services"]:
        if not s["key"].startswith("k8s:"):
            continue
        i, e = s["ingress"], s["egress"]
        guards[s["key"]] = [i["isolated"], i["any"], sorted(i["pods"]), e["isolated"], e["any"], sorted(e["pods"])]
        for e in s["edges"]:
            host = e["shown"].split(" → ", 1)[1].rsplit(":", 1)[0] if ":" in e["shown"].split(" → ", 1)[1] \
                else e["shown"].split(" → ", 1)[1]
            edges[f"{s['key']}\t{e['file']}:{e['line']}\t{host}"].add(e["target"])
    return out, edges, guards, exposed


def k8s_truth(python, root, files):
    """Dữ kiện từng manifest và cạnh theo PyYAML và tài liệu Kubernetes."""
    p = subprocess.run([python, "-I", str(HERE / "k8s_truth.py"), "facts"],
                       input="".join(f"{root}\t{f}\n" for f in files),
                       capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"thước đo Kubernetes hỏng: {p.stderr.strip()[-400:]}")
    out = {}
    for line in p.stdout.splitlines():
        file, res = line.split("\t", 1)
        if res.startswith(("ERR", "SKIP")):
            out[file] = res
        else:
            out[file] = collections.Counter((l, s) for l, s in json.loads(res)["facts"])
    p = subprocess.run([python, "-I", str(HERE / "k8s_truth.py"), "edges", str(root)],
                       input="".join(f"{f}\n" for f in files), capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"thước đo Kubernetes hỏng: {p.stderr.strip()[-400:]}")
    res = json.loads(p.stdout)
    return out, {k: set(v) for k, v in res["edges"].items()}, res["guards"], set(res["exposed"])


PROTO_REQUIREMENTS = HERE / "proto-requirements.txt"


def proto_version():
    m = re.search(r"^grpcio-tools==(\S+)", PROTO_REQUIREMENTS.read_text(), re.M)
    return f"protoc (grpcio-tools {m.group(1)})" if m else "?"


def proto_files(root):
    """File `.proto` mà git theo dõi và có trên đĩa (repo tải sparse)."""
    p = subprocess.run(["git", "-C", str(root), "ls-files", "-z", "--", "*.proto"], capture_output=True, text=True)
    return sorted(f for f in p.stdout.split("\0") if f and (root / f).is_file() and not (root / f).is_symlink())


def archctl_protos(archctl, root, files):
    """`{file: Counter (dòng, dữ kiện)}` theo `archctl scan`, hay chuỗi lỗi."""
    p = subprocess.run([archctl, "scan", "--json", "--root", str(root), *files],
                       capture_output=True, text=True, timeout=600)
    if p.returncode != 0:
        bench.die(f"archctl scan hỏng ở {root.name}: {p.stderr.strip()[-300:]}")
    out = {}
    for f in json.loads(p.stdout)["protos"]:
        if "error" in f:
            out[f["file"]] = f"ERR {f['error']}"
            continue
        facts = [(0, f"package {f['package']}")]
        for s in f["services"]:
            facts.append((s["line"], f"service {s['name']}"))
            facts += [(line, f"rpc {s['name']}.{name}") for name, line in s["rpcs"]]
        out[f["file"]] = collections.Counter(facts)
    return out


def proto_truth(python, root, files):
    """`{file: Counter (dòng, dữ kiện)}` theo `protoc`; chuỗi `SKIP …` khi
    `protoc` không dựng được descriptor."""
    p = subprocess.run([python, "-I", str(HERE / "proto_truth.py")],
                       input="".join(f"{root}\t{f}\n" for f in files),
                       capture_output=True, text=True, timeout=1800)
    if p.returncode != 0:
        bench.die(f"thước đo .proto hỏng: {p.stderr.strip()[-400:]}")
    out = {}
    for line in p.stdout.splitlines():
        file, res = line.split("\t", 1)
        out[file] = res if res.startswith("SKIP") else collections.Counter((l, s) for l, s in json.loads(res)["facts"])
    return out


def notebook_truth(python, root, files):
    """`{file: (Counter (dòng, spec), exact)}`, hoặc None nếu file không đọc được."""
    p = subprocess.run([python, str(HERE / "notebook_truth.py")],
                       input="".join(f"{root}\t{f}\n" for f in files),
                       capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"thước đo notebook hỏng: {p.stderr.strip()[-400:]}")
    out = {}
    for line in p.stdout.splitlines():
        file, res = line.split("\t", 1)
        if res == "ERR":
            out[file] = None
            continue
        r = json.loads(res)
        out[file] = (collections.Counter((l, s) for l, s in r["imports"]), r["exact"])
    return out


def tool_version(cmd):
    p = subprocess.run(cmd, capture_output=True, text=True)
    first = (p.stdout or p.stderr).strip().splitlines()
    return first[0] if first else "?"


def sane(tally, repo):
    """Thước đo không parse được quá nửa số file là thước đo hỏng."""
    total = tally.files + tally.unparsed
    if total >= 10 and tally.unparsed > total * MAX_UNPARSED:
        bench.die(f"{repo}: thước đo {tally.label} không parse được {tally.unparsed}/{total} file "
                  "— nghi chính thước đo (phiên bản, cờ), không phải mã")


def node_can_strip_types():
    p = subprocess.run(["node", "--input-type=module", "-e",
                        "import * as m from 'node:module'; process.stdout.write(String(!!m.stripTypeScriptTypes))"],
                       capture_output=True, text=True)
    return p.stdout.strip() == "true"


def diff(want, have, check_extra=True):
    """(thiếu, thừa, lệch dòng). Cùng spec mà khác dòng thì tính là lệch dòng."""
    missing, extra = want - have, (have - want) if check_extra else collections.Counter()
    by_spec_m = collections.Counter(s for (_, s) in missing.elements())
    by_spec_e = collections.Counter(s for (_, s) in extra.elements())
    moved = by_spec_m & by_spec_e
    return missing, extra, moved


class Tally:
    def __init__(self, label):
        self.label, self.files, self.imports, self.unparsed = label, 0, 0, 0
        self.skipped = ""  # phần nào không đo được, và vì sao
        self.bad, self.examples = 0, []
        self.in_macro = 0  # import archctl đọc được trong thân macro, parser không thấy

    def note(self, msg):
        self.bad += 1
        if len(self.examples) < EXAMPLES:
            self.examples.append(msg)

    def line(self, repo):
        unit = {"Binding": "đầu cạnh", "Hạ tầng": "dữ kiện", "Proto": "dữ kiện", "K8s": "dữ kiện",
                "Tên máy": "tên máy"}.get(self.label, "import")
        head = f"  {repo:22} {self.label:8} {self.files:5} file, {self.imports:6} {unit}"
        tail = f", {self.unparsed} không parse được" if self.unparsed else ""
        if self.in_macro:
            tail += f", {self.in_macro} trong macro không đối chiếu"
        if self.skipped:
            tail += f", {self.skipped}"
        verdict = "khớp" if not self.bad else f"{self.bad} LỆCH"
        return f"{head}{tail}: {verdict}"


def check_lines(tally, file, want, have):
    tally.files += 1
    tally.imports += sum(want.values())
    missing, extra, moved = diff(want, have)
    for (line, spec) in sorted(missing.elements()):
        if moved[spec]:
            moved[spec] -= 1
            got = sorted(l for (l, s) in extra if s == spec)
            tally.note(f"{file}:{line} lệch dòng: {spec!r} (archctl báo dòng {got})")
        else:
            tally.note(f"{file}:{line} thiếu: {spec!r}")
    moved_back = collections.Counter(s for (_, s) in missing.elements()) & \
        collections.Counter(s for (_, s) in extra.elements())
    for (line, spec) in sorted(extra.elements()):
        if moved_back[spec]:
            moved_back[spec] -= 1
            continue
        tally.note(f"{file}:{line} thừa: {spec!r}")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--archctl", default=str(ROOT / "target/release/archctl"))
    ap.add_argument("--only", help="tên repo, cách nhau bằng dấu phẩy")
    ap.add_argument("--require", default="",
                    help="công cụ bắt buộc phải có: clang, node, go, syn (tải crate), ipython, deploy, proto (tải gói)")
    ap.add_argument("--cache", default=str(ROOT / "target/bench"))
    args = ap.parse_args()

    archctl = str(Path(args.archctl).resolve())
    repos = [r[0] for r in bench.load_repos()]
    if args.only:
        wanted = args.only.split(",")
        unknown = set(wanted) - set(repos)
        if unknown:
            bench.die(f"không có repo nào tên: {', '.join(sorted(unknown))}")
        repos = [r for r in repos if r in wanted]

    have_clang = shutil.which("clang") is not None
    have_node = shutil.which("node") is not None
    have_go = shutil.which("go") is not None
    strip_ts = have_node and node_can_strip_types()
    required = args.require.split(",")
    unknown = set(filter(None, required)) - {"clang", "node", "go", "syn", "ipython", "deploy", "proto"}
    if unknown:
        bench.die(f"`--require` không biết: {', '.join(sorted(unknown))}")
    for tool, ok in [("clang", have_clang), ("node", have_node), ("go", have_go)]:
        if tool in required and not ok:
            bench.die(f"thiếu `{tool}`, mà `--require` đòi có")
    if "node" in required and not strip_ts:
        bench.die("node không có `module.stripTypeScriptTypes` (cần Node ≥ 22.13), "
                  "mà `--require node` đòi đối chiếu cả TS")
    clang_v = tool_version(["clang", "--version"]) if have_clang else "BỎ QUA: không có"
    node_v = tool_version(["node", "--version"]) if have_node else "BỎ QUA: không có"
    go_v = tool_version(["go", "version"]) if have_go else "BỎ QUA: không có"
    use_syn = "syn" in required
    syn_v = "2.0.87" if use_syn else "BỎ QUA: cần tải crate, bật bằng --require syn"
    use_ipython = "ipython" in required
    ipy_v = ipython_version() if use_ipython else "BỎ QUA: cần tải gói, bật bằng --require ipython"
    use_deploy = "deploy" in required
    dep_v = deploy_version() if use_deploy else "BỎ QUA: cần tải gói, bật bằng --require deploy"
    use_proto = "proto" in required
    proto_v = proto_version() if use_proto else "BỎ QUA: cần tải gói, bật bằng --require proto"
    print(f"python {sys.version.split()[0]} · clang: {clang_v} · node: {node_v} · go: {go_v} · syn: {syn_v}"
          f" · ipython: {ipy_v} · hạ tầng: {dep_v} · proto: {proto_v}"
          + ("" if strip_ts or not have_node else " (không gỡ được kiểu TS: BỎ QUA .ts)"))

    missing_repos = [r for r in repos if not (Path(args.cache) / "repos" / r).is_dir()]
    if missing_repos:
        bench.die(f"chưa tải: {', '.join(missing_repos)} — chạy `python3 bench/bench.py` trước")

    workdir = tempfile.TemporaryDirectory(prefix="archctl-oracle-")
    go_helper = build_go_helper(workdir.name) if have_go else None
    hosts_helper = code_hosts.build_go_helper(workdir.name, bench.die) if have_go else None
    syn_helper = build_syn_helper(args.cache) if use_syn else None
    nb_python = build_ipython_helper(args.cache) if use_ipython else None
    dep_python = build_venv(args.cache, "deploy-oracle", DEPLOY_REQUIREMENTS, "hạ tầng") if use_deploy else None
    proto_python = build_venv(args.cache, "proto-oracle", PROTO_REQUIREMENTS, ".proto") if use_proto else None

    bad = 0
    for repo in repos:
        root = Path(args.cache) / "repos" / repo
        files = archctl_scan(archctl, root)
        got = {f["file"]: collections.Counter((i["line"], i["spec"]) for i in f["imports"])
               for f in files}

        def crossings(file, side):
            """Khoá binding (`py:`, `op:`) archctl đọc được ở file đó; khoá hàm C
            của FFI không thuộc phép đối chiếu này."""
            f = by_file[file]
            return collections.Counter((c["line"], c["key"]) for c in f.get("crossings", {}).get(side, [])
                                       if c["key"].startswith(("py:", "op:")))
        by_file = {f["file"]: f for f in files}
        bind = Tally("Binding")
        by_lang = collections.defaultdict(list)
        for f in files:
            by_lang[f["lang"]].append(f["file"])
        tallies = []

        py = Tally("Python")
        for file in by_lang["Python"]:
            if not file.endswith(PY_EXTS):
                continue  # Cython: ast không đọc được
            truth = python_truth(root / file)
            if truth is None:
                py.unparsed += 1
                continue
            want, ops = truth
            check_lines(py, file, want, got[file])
            have_ops = crossings(file, "calls")
            if ops or have_ops:
                check_lines(bind, file, ops, have_ops)
        tallies.append(py)

        notebooks = [f for f in by_lang["Python"] if f.endswith(".ipynb")]
        if nb_python and notebooks:
            nb = Tally("Notebook")
            for file, truth in notebook_truth(nb_python, root, notebooks).items():
                if truth is None:
                    nb.unparsed += 1
                    continue
                want, exact = truth
                have = got[file]
                if not exact:
                    # Số dòng ở đây archctl chỉ hứa gần đúng: so tên thôi.
                    want = collections.Counter((0, s) for (_, s) in want.elements())
                    have = collections.Counter((0, s) for (_, s) in have.elements())
                check_lines(nb, file, want, have)
            tallies.append(nb)

        if have_clang and by_lang["C/C++"]:
            c = Tally("C/C++")
            with concurrent.futures.ThreadPoolExecutor(os.cpu_count() or 4) as pool:
                truths = pool.map(lambda f: clang_tokens(root / f), by_lang["C/C++"])
                for file, (want, tokens) in zip(by_lang["C/C++"], truths):
                    check_lines(c, file, want, got[file])
                    defined, have = binding_truth(tokens), crossings(file, "provides")
                    if defined or have:
                        check_lines(bind, file, defined, have)
            tallies.append(c)

        node_files = [f for f in by_lang["TypeScript"]
                      if f.endswith(NODE_EXTS) and (strip_ts or not f.endswith((".ts", ".mts", ".cts")))]
        if have_node and node_files:
            js = Tally("JS/TS")
            for file, want in node_truth(root, node_files).items():
                if want is None:
                    js.unparsed += 1
                    continue
                js.files += 1
                js.imports += sum(want.values())
                have = collections.Counter(spec for (_, spec) in got[file].elements())
                for spec in sorted((want - have).elements()):
                    js.note(f"{file} thiếu: {spec!r}")
            tallies.append(js)

        if go_helper and by_lang["Go"]:
            go = Tally("Go")
            for file, want in go_truth(go_helper, root, by_lang["Go"]).items():
                if want is None:
                    go.unparsed += 1
                    continue
                check_lines(go, file, want, got[file])
            tallies.append(go)

        # Tên máy trong chuỗi: Python theo `ast`, Go theo `go/ast`.
        def have_hosts(file):
            found = by_file[file].get("hosts", {}).get("found", [])
            return collections.Counter((line, f"{name}:{port}" if port else name) for line, name, port, _ in found)
        hosts = Tally("Tên máy")
        for file in by_lang["Python"]:
            if file.endswith(PY_EXTS):
                want = code_hosts.python_truth(root / file)
                if want is None:
                    hosts.unparsed += 1
                elif want or have_hosts(file):
                    check_lines(hosts, file, want, have_hosts(file))
        if hosts_helper and by_lang["Go"]:
            for file, want in code_hosts.go_truth(hosts_helper, root, by_lang["Go"], bench.die).items():
                if want is None:
                    hosts.unparsed += 1
                elif want or have_hosts(file):
                    check_lines(hosts, file, want, have_hosts(file))
        if hosts.files:
            tallies.append(hosts)

        if syn_helper and by_lang["Rust"]:
            rs = Tally("Rust")
            for file, truth in syn_truth(syn_helper, root, by_lang["Rust"]).items():
                if truth is None:
                    rs.unparsed += 1
                    continue
                want, macros, modules = truth
                have = collections.Counter(got[file])
                for (line, spec), n in (have - want).items():
                    if any(a <= line <= b for a, b in macros):
                        have[(line, spec)] -= n
                        rs.in_macro += n
                check_lines(rs, file, want, +have)
                # `#[pymodule]` trong thân macro thì `syn` không thấy, như import.
                have_modules = crossings(file, "provides")
                for (line, key), n in (have_modules - modules).items():
                    if any(a <= line <= b for a, b in macros):
                        have_modules[(line, key)] -= n
                if modules or +have_modules:
                    check_lines(bind, file, modules, +have_modules)
            tallies.append(rs)

        if by_lang["C/C++"] and not have_clang:
            bind.skipped = "phía C/C++ BỎ QUA: không có clang"
        tallies.append(bind)

        deploy = deploy_files(root) if dep_python else []
        if deploy:
            dep = Tally("Hạ tầng")
            have_all = archctl_deploy(archctl, root, deploy)
            skipped = 0
            for file, want in deploy_truth(dep_python, root, deploy).items():
                have = have_all.get(file, "ERR archctl không in file này")
                if isinstance(want, str) and want.startswith("SKIP"):
                    skipped += 1
                    continue
                if isinstance(want, str):
                    # Bộ đọc chuẩn cũng không đọc được: không phải chỗ để so.
                    dep.unparsed += 1
                    continue
                if isinstance(have, str):
                    dep.files += 1
                    dep.note(f"{file}: archctl không đọc được mà PyYAML đọc được — {have[4:120]}")
                    continue
                dep.imports += sum(want.values())
                check_lines(dep, file, want, have)
            if skipped:
                dep.skipped = f"{skipped} Dockerfile có heredoc BỎ QUA"
            tallies.append(dep)

        yamls = yaml_files(root) if dep_python else []
        if yamls:
            kt = Tally("K8s")
            have_all, have_edges, have_guards, have_exposed = archctl_k8s(archctl, root, yamls)
            want_all, want_edges, want_guards, want_exposed = k8s_truth(dep_python, root, yamls)
            # Lối vào từ ngoài cụm: tập workload lộ ra.
            kt.imports += len(want_exposed)
            for key in sorted(want_exposed - have_exposed):
                kt.note(f"{key} thiếu lối vào từ Internet")
            for key in sorted(have_exposed - want_exposed):
                kt.note(f"{key} thừa lối vào từ Internet")
            for file, want in want_all.items():
                if isinstance(want, str):
                    if want.startswith("ERR") and file in have_all:
                        kt.unparsed += 1
                    continue
                have = have_all.get(file, "ERR archctl không in file này")
                if isinstance(have, str):
                    kt.files += 1
                    kt.note(f"{file}: archctl không đọc được mà PyYAML đọc được — {have[4:120]}")
                    continue
                check_lines(kt, file, want, have)
            # Cạnh: cùng nguồn, cùng dòng, cùng tên máy thì cùng tập đích.
            for key in sorted(set(want_edges) | set(have_edges)):
                want, have = want_edges.get(key, set()), have_edges.get(key, set())
                kt.imports += len(want)
                src, at, host = key.split("\t")
                for t in sorted(want - have):
                    kt.note(f"{at} thiếu cạnh: {src} → {t} qua `{host}`")
                for t in sorted(have - want):
                    kt.note(f"{at} thừa cạnh: {src} → {t} qua `{host}`")
            # NetworkPolicy: cô lập, cho phép mọi nguồn, cho phép rõ ai, hai chiều.
            for key in sorted(set(want_guards) | set(have_guards)):
                want, have = want_guards.get(key), have_guards.get(key)
                kt.imports += 1
                if want != have:
                    kt.note(f"{key} lệch NetworkPolicy: thước đo {want}, archctl {have}")
            if kt.files:
                tallies.append(kt)

        protos = proto_files(root) if proto_python else []
        if protos:
            pt = Tally("Proto")
            have_all = archctl_protos(archctl, root, protos)
            skipped = 0
            for file, want in proto_truth(proto_python, root, protos).items():
                if isinstance(want, str):
                    skipped += 1
                    continue
                have = have_all.get(file, "ERR archctl không in file này")
                if isinstance(have, str):
                    pt.files += 1
                    pt.note(f"{file}: archctl không đọc được mà protoc đọc được — {have[4:120]}")
                    continue
                pt.imports += sum(want.values())
                check_lines(pt, file, want, have)
            if skipped:
                pt.skipped = f"{skipped} file protoc không dựng được (import ngoài repo) BỎ QUA"
            tallies.append(pt)
        for t in tallies:
            sane(t, repo)
            if t.files or t.unparsed or t.skipped:
                print(t.line(repo))
                for e in t.examples:
                    print(f"      {e}")
            bad += t.bad

    print(f"\n{'khớp hết' if not bad else f'{bad} chỗ lệch so với parser chuẩn'}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
