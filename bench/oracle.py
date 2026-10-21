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
import queue_truth  # noqa: E402

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


# `helm` bản ghim, tải từ get.helm.sh, kiểm sha256 của chính tarball (lấy từ
# file `.sha256sum` chính thức lúc ghim). Chỉ khi `--require helm`.
HELM_VERSION = "v3.15.4"
HELM_SHA256 = {
    "darwin-arm64": "88115846a1fb58f8eb8f64fec5c343d95ca394f1be811602fa54a887c98730ac",
    "linux-amd64": "11400fecfc07fd6f034863e4e0c4c4445594673fd2a129e701fe41f31170cfa9",
}


def build_helm(cache):
    import hashlib
    import platform
    import tarfile
    import urllib.request
    osname = {"Darwin": "darwin", "Linux": "linux"}.get(platform.system())
    arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "amd64", "AMD64": "amd64"}.get(platform.machine())
    plat = f"{osname}-{arch}"
    if plat not in HELM_SHA256:
        bench.die(f"không có bản helm ghim cho {plat}")
    home = Path(cache) / "helm-oracle"
    exe = home / f"helm-{HELM_VERSION}-{plat}"
    if exe.exists():
        return str(exe)
    home.mkdir(parents=True, exist_ok=True)
    url = f"https://get.helm.sh/helm-{HELM_VERSION}-{plat}.tar.gz"
    try:
        data = urllib.request.urlopen(url, timeout=120).read()
    except OSError as e:
        bench.die(f"tải helm hỏng: {e}")
    if hashlib.sha256(data).hexdigest() != HELM_SHA256[plat]:
        bench.die(f"helm tải về sai sha256: {url}")
    tgz = home / "helm.tar.gz"
    tgz.write_bytes(data)
    with tarfile.open(tgz) as t:
        member = t.getmember(f"{plat}/helm")
        src = t.extractfile(member)
        exe.write_bytes(src.read())
    exe.chmod(0o755)
    tgz.unlink()
    return str(exe)


# `kustomize` bản ghim, sha256 lấy từ `checksums.txt` chính thức của bản phát
# hành. Chỉ khi `--require kustomize`.
KUSTOMIZE_VERSION = "v5.8.1"
KUSTOMIZE_SHA256 = {
    "darwin_arm64": "8886f8a78474e608cc81234f729fda188a9767da23e28925802f00ece2bab288",
    "linux_amd64": "029a7f0f4e1932c52a0476cf02a0fd855c0bb85694b82c338fc648dcb53a819d",
}


# Terraform: OpenTofu (giấy phép MPL) bản ghim, provider ghim, chỉ cài từ thư
# mục mirror đã kiểm băm (TM-52). Băm của provider lấy từ registry của OpenTofu
# lúc ghim, bản ra ít nhất hai tuần trước.
TOFU_VERSION = "1.12.7"
TOFU_SHA256 = {
    "darwin_arm64": "e3a30269d64b3b9f49937e835817e2c06871fc4cfb7a08622d3b752074f75fb8",
    "linux_amd64": "6807f7ae2a33e254c104da54e628b40aa6fc50d4becd78c43574fe250add33e9",
}
TOFU_PROVIDERS = {
    ("aws", "6.67.0"): {
        "darwin_arm64": "25afdd7ee3235adec8c96ee2b12b9abecc00b56a1b8505acc6f2889798240d04",
        "linux_amd64": "c7955fa64b43262ab05faccecc09c015a388044d8238358371c381ceaab98282",
    },
    ("google", "8.5.0"): {
        "darwin_arm64": "a0c19552b7e4ffd0f003c035832e38359c005dc8e1e524f5d6d3cbad21305ebf",
        "linux_amd64": "c39ec87452a328c1329ccdd6a0c71bebf3ef47a90123835fdee5860dfbb24cc6",
    },
}
TF_REQUIREMENTS = HERE / "tf-requirements.txt"
# Cấu hình provider với khoá giả: plan chỉ tính, không gọi cloud.
TF_PROVIDER_CONFIG = {
    "aws": """provider "aws" {
  region                      = "us-east-1"
  access_key                  = "mock"
  secret_key                  = "mock"
  skip_credentials_validation = true
  skip_requesting_account_id  = true
  skip_metadata_api_check     = true
}
""",
    "google": """provider "google" {
  project      = "mock-project"
  region       = "us-central1"
  access_token = "mock"
}
""",
}


def platform_name():
    import platform
    osname = {"Darwin": "darwin", "Linux": "linux"}.get(platform.system())
    arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "amd64", "AMD64": "amd64"}.get(platform.machine())
    return f"{osname}_{arch}"


def fetch_pinned(url, sha, what):
    import hashlib
    import urllib.request
    try:
        data = urllib.request.urlopen(url, timeout=600).read()
    except OSError as e:
        bench.die(f"tải {what} hỏng: {e}")
    if hashlib.sha256(data).hexdigest() != sha:
        bench.die(f"{what} tải về sai sha256: {url}")
    return data


def build_tofu(cache):
    """`(đường dẫn tofu, thư mục mirror provider)`, tải một lần vào cache."""
    import io
    import zipfile
    plat = platform_name()
    if plat not in TOFU_SHA256:
        bench.die(f"không có bản OpenTofu ghim cho {plat}")
    import hashlib
    home = Path(cache) / "tf-oracle"
    mirror = home / "mirror"
    home.mkdir(parents=True, exist_ok=True)
    # Giữ file zip đã tải và kiểm lại băm mỗi lần dùng: cache bị sửa thì dừng.
    def pinned(path, url, sha, what):
        if not path.exists() or hashlib.sha256(path.read_bytes()).hexdigest() != sha:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(fetch_pinned(url, sha, what))
        return path.read_bytes()
    zdata = pinned(home / f"tofu_{TOFU_VERSION}_{plat}.zip",
                   f"https://github.com/opentofu/opentofu/releases/download/v{TOFU_VERSION}/tofu_{TOFU_VERSION}_{plat}.zip",
                   TOFU_SHA256[plat], "OpenTofu")
    exe = home / f"tofu-{TOFU_VERSION}-{plat}"
    exe.write_bytes(zipfile.ZipFile(io.BytesIO(zdata)).read("tofu"))
    exe.chmod(0o755)
    for (name, version), shas in TOFU_PROVIDERS.items():
        if plat not in shas:
            bench.die(f"không có provider {name} ghim cho {plat}")
        zname = f"terraform-provider-{name}_{version}_{plat}.zip"
        pinned(mirror / "registry.opentofu.org" / "hashicorp" / name / zname,
               f"https://github.com/opentofu/terraform-provider-{name}/releases/download/v{version}/{zname}",
               shas[plat], f"provider {name}")
    return str(exe), str(mirror)


def tf_files(root):
    """File `.tf` của repo (cả file chưa commit, cho bộ mẫu), ngoài thư mục
    test, ví dụ, ẩn (như archctl)."""
    p = subprocess.run(["git", "-C", str(root), "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
                       capture_output=True, text=True)
    listed = p.stdout.split("\0") if p.returncode == 0 else [str(x.relative_to(root)) for x in Path(root).rglob("*.tf")]
    aside = {"test", "tests", "testdata", "example", "examples", "docs", "doc", "fixtures", "e2e", "samples", "sample"}
    out = []
    for f in listed:
        if not f.endswith(".tf") or not (Path(root) / f).is_file() or (Path(root) / f).is_symlink():
            continue
        dirs = f.split("/")[:-1]
        if any(d in aside or d.startswith(".") for d in dirs):
            continue
        out.append(f)
    return sorted(out)


def tf_plan(tofu, mirror, root, module_root, files, workdir):
    """Plan JSON của một module gốc, hay lý do không plan được. Chạy trên bản
    chép các file `.tf` trong thư mục tạm, môi trường sạch, khoá giả."""
    import hashlib
    import shutil
    # Plan chỉ phụ thuộc file `.tf`, module gốc, OpenTofu và provider ghim,
    # không phụ thuộc archctl: nhớ theo băm để sabotage không plan lại.
    key = hashlib.sha256(json.dumps([TOFU_VERSION, sorted(TOFU_PROVIDERS), module_root,
                                     [(f, hashlib.sha256((Path(root) / f).read_bytes()).hexdigest()) for f in files]],
                                    default=str).encode()).hexdigest()
    cached = Path(mirror).parent / "plans" / f"{key}.json"
    if cached.exists():
        text = cached.read_text()
        return (str(cached), None) if not text.startswith("SKIP ") else (None, text[5:])
    # Mỗi lần `tofu init` chép provider (vài trăm MB) vào `.terraform` của thư
    # mục làm việc: xoá ngay sau khi chép plan vào cache, không thì hàng chục
    # module gốc nhân số vòng sabotage làm đầy đĩa runner CI.
    work = Path(tempfile.mkdtemp(prefix="tf-", dir=workdir))
    try:
        plan, why = tf_plan_fresh(tofu, mirror, root, module_root, files, work)
        cached.parent.mkdir(parents=True, exist_ok=True)
        cached.write_text(Path(plan).read_text() if plan else f"SKIP {why}")
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return (str(cached), None) if plan else (None, why)


def tf_plan_fresh(tofu, mirror, root, module_root, files, work):
    import shutil
    repo = work / "repo"
    for f in files:
        dst = repo / f
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(Path(root) / f, dst)
    mdir = repo / module_root if module_root else repo
    text = "".join((mdir / f).read_text() for f in os.listdir(mdir) if f.endswith(".tf"))
    # Mọi module tới được từ gốc: chỉ module trong repo; `data "external"`,
    # provisioner ở bất kỳ module nào thì không chạy (TM-52).
    seen, todo = set(), [mdir.resolve()]
    while todo:
        d = todo.pop()
        if d in seen:
            continue
        seen.add(d)
        if not d.is_dir() or not str(d).startswith(str(repo.resolve())):
            return None, "module trỏ ra ngoài repo: không chạy"
        body = "".join((d / f).read_text() for f in os.listdir(d) if f.endswith(".tf"))
        if re.search(r'\bdata\s+"?external"?\s|\bprovisioner\s', body):
            return None, "có `data \"external\"` hay provisioner: không chạy"
        # `source` trong khối `module` (cả khối một dòng); `source` của
        # `required_providers` thì không phải module.
        for src in re.findall(r'\bmodule\s+"[^"]*"\s*\{[^}]*?\bsource\s*=\s*"([^"]*)"', body, re.S):
            if not src.startswith(("./", "../")):
                return None, f"module không cục bộ (`{src.split('?')[0][:60]}`): không tải"
            todo.append((d / src).resolve())
    for name, conf in TF_PROVIDER_CONFIG.items():
        # Có sẵn khối provider thì ghi đè cấu hình của nó; không thì thêm mới.
        has = re.search(rf'^\s*provider\s+"{name}"', text, re.M)
        (mdir / (f"zz_archctl_{name}_override.tf" if has else f"zz_archctl_{name}.tf")).write_text(conf)
    (work / "cli.tfrc").write_text(f'provider_installation {{\n  filesystem_mirror {{\n    path = "{mirror}"\n  }}\n}}\n')
    env = {"PATH": "/usr/bin:/bin", "HOME": str(work), "TF_CLI_CONFIG_FILE": str(work / "cli.tfrc"),
           "TF_IN_AUTOMATION": "1"}
    for args in (["init", "-backend=false", "-input=false", "-no-color"],
                 ["plan", "-refresh=false", "-input=false", "-no-color", "-lock=false", "-out=archctl.plan"]):
        p = subprocess.run([tofu, *args], cwd=mdir, env=env, capture_output=True, text=True, timeout=600)
        if p.returncode != 0:
            lines = [l.strip() for l in (p.stderr + "\n" + p.stdout).splitlines() if l.strip()]
            err = next((l for l in lines if l.startswith("Error:")), lines[-1] if lines else "")
            return None, f"tofu {args[0]} hỏng: {err[:160]}"
    p = subprocess.run([tofu, "show", "-json", "archctl.plan"], cwd=mdir, env=env, capture_output=True, text=True, timeout=600)
    if p.returncode != 0:
        return None, "tofu show hỏng"
    out = work / "plan.json"
    out.write_text(p.stdout)
    return str(out), None


def archctl_tf(archctl, root):
    """`{gốc: {địa chỉ: {"edges": [...], "exposed": bool, "image": ...}}}` theo `archctl scan`."""
    p = subprocess.run([archctl, "scan", "--json", "--verbatim", "--root", str(root)],
                       capture_output=True, text=True, timeout=600)
    if p.returncode != 0:
        bench.die(f"archctl scan hỏng ở {root}: {p.stderr.strip()[-300:]}")
    split = lambda key: tuple(key[3:].rsplit("/", 1)) if "/" in key[3:] else ("", key[3:])
    out = collections.defaultdict(dict)
    for s in json.loads(p.stdout)["deploy"]["services"]:
        if not s["key"].startswith("tf:"):
            continue
        r, addr = split(s["key"])
        out[r][addr] = {"edges": sorted({split(e["target"])[1] for e in s["edges"]}),
                        "exposed": bool(s.get("exposed")), "image": s.get("image")}
    return out


def compare_terraform(t, archctl, tofu, mirror, tf_python, root, workdir, refused=False):
    """`refused`: bộ mẫu mà mọi module gốc phải bị từ chối plan (TM-52)."""
    files = tf_files(root)
    if not files:
        return
    p = subprocess.run([tf_python, "-I", str(HERE / "tf_truth.py"), "roots", str(root)],
                       input="".join(f"{f}\n" for f in files), capture_output=True, text=True, timeout=300)
    if p.returncode != 0:
        bench.die(f"thước đo Terraform hỏng: {p.stderr.strip()[-400:]}")
    plans, skipped = {}, []
    roots = json.loads(p.stdout)
    # Mỗi module gốc plan trong thư mục tạm riêng: chạy song song.
    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
        results = list(pool.map(lambda r: (r, *tf_plan(tofu, mirror, root, r, files, workdir)), roots))
    for r, plan, why in results:
        if plan:
            plans[r] = plan
        else:
            skipped.append(f"{r or '.'}: {why}")
    if refused:
        t.files += 1
        for r in plans:
            t.note(f"{r or '.'}: phải bị từ chối plan mà OpenTofu đã chạy")
        # Đúng vì chốt chặn `data "external"`, không phải vì init hỏng.
        for why in skipped:
            if "external" not in why:
                t.note(f"bị từ chối vì lý do khác: {why[:120]}")
        return
    if skipped:
        t.skipped = f"{len(skipped)} module gốc OpenTofu không plan được BỎ QUA ({skipped[0][:120]})"
        # Plan hỏng quá nửa thì phần đo được không còn đại diện: tính là lệch.
        # Gốc bị từ chối có chủ ý (`data "external"`, module không cục bộ) thì
        # không đếm: đó là chốt chặn TM-52.
        failed = [x for x in skipped if "không chạy" not in x and "không tải" not in x]
        if len(failed) * 2 > len(roots):
            t.note(f"{len(failed)}/{len(roots)} module gốc plan hỏng")
    if not plans:
        return
    pj = Path(workdir) / f"tf-plans-{root.name}.json"
    pj.write_text(json.dumps(plans))
    p = subprocess.run([tf_python, "-I", str(HERE / "tf_truth.py"), "truth", str(root), str(pj)],
                       capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"thước đo Terraform hỏng: {p.stderr.strip()[-400:]}")
    want_all, have_all = json.loads(p.stdout), archctl_tf(archctl, root)
    for r in sorted(plans):
        want, have = want_all.get(r, {}), have_all.get(r, {})
        t.files += 1
        for a in sorted(set(want) - set(have)):
            t.note(f"{r or '.'}: thiếu tài nguyên `{a}`")
        for a in sorted(set(have) - set(want)):
            t.note(f"{r or '.'}: thừa tài nguyên `{a}`")
        for a in sorted(set(want) & set(have)):
            w, h = want[a], have[a]
            t.imports += 1 + len(w["edges"])
            for x in sorted(set(w["edges"]) - set(h["edges"])):
                t.note(f"{r or '.'}: thiếu cạnh `{a}` → `{x}`")
            for x in sorted(set(h["edges"]) - set(w["edges"])):
                t.note(f"{r or '.'}: thừa cạnh `{a}` → `{x}`")
            if w["exposed"] != h["exposed"]:
                t.note(f"{r or '.'}: `{a}` lệch lối vào từ Internet: OpenTofu {w['exposed']}, archctl {h['exposed']}")
            if w["image"] is not None and w["image"] != h["image"]:
                t.note(f"{r or '.'}: `{a}` lệch image")


def build_kustomize(cache):
    import hashlib
    import platform
    import tarfile
    import urllib.request
    osname = {"Darwin": "darwin", "Linux": "linux"}.get(platform.system())
    arch = {"arm64": "arm64", "aarch64": "arm64", "x86_64": "amd64", "AMD64": "amd64"}.get(platform.machine())
    plat = f"{osname}_{arch}"
    if plat not in KUSTOMIZE_SHA256:
        bench.die(f"không có bản kustomize ghim cho {plat}")
    home = Path(cache) / "kustomize-oracle"
    exe = home / f"kustomize-{KUSTOMIZE_VERSION}-{plat}"
    if exe.exists():
        return str(exe)
    home.mkdir(parents=True, exist_ok=True)
    url = (f"https://github.com/kubernetes-sigs/kustomize/releases/download/kustomize%2F{KUSTOMIZE_VERSION}/"
           f"kustomize_{KUSTOMIZE_VERSION}_{plat}.tar.gz")
    try:
        data = urllib.request.urlopen(url, timeout=120).read()
    except OSError as e:
        bench.die(f"tải kustomize hỏng: {e}")
    if hashlib.sha256(data).hexdigest() != KUSTOMIZE_SHA256[plat]:
        bench.die(f"kustomize tải về sai sha256: {url}")
    tgz = home / "kustomize.tar.gz"
    tgz.write_bytes(data)
    with tarfile.open(tgz) as t:
        exe.write_bytes(t.extractfile(t.getmember("kustomize")).read())
    exe.chmod(0o755)
    tgz.unlink()
    return str(exe)


# Đuôi băm Kustomize thêm vào tên ConfigMap, Secret sinh ra (archctl không thêm).
KUSTOMIZE_HASH = re.compile(r"^(.+)-[bcdfghjklmnpqrstvwxz2456789]{10}$")


def kustomize_build(kustomize, root, built_roots):
    """`{gốc: [văn bản từng tài nguyên]}` theo `kustomize build`, và gốc mà
    kustomize không dựng được. Đuôi băm của ConfigMap sinh ra được bỏ."""
    out, failed = {}, []
    for d in built_roots:
        p = subprocess.run([kustomize, "build", d or "."], cwd=root, capture_output=True, text=True, timeout=300,
                           env={"PATH": os.environ.get("PATH", ""), "HOME": str(root)})
        if p.returncode != 0:
            failed.append(d)
            continue
        text = p.stdout
        for name in re.findall(r"^  name: (\S+)$", text, re.M):
            m = KUSTOMIZE_HASH.match(name)
            if m and re.search(rf"^kind: (ConfigMap|Secret)\n(?:.*\n)*?  name: {re.escape(name)}$", text, re.M):
                text = re.sub(rf"\b{re.escape(name)}\b", m.group(1), text)
        out[d] = [part for part in re.split(r"^---\s*$", text, flags=re.M) if part.strip()]
    return out, failed


def helm_render(helm, root, charts, workdir):
    """`{template: [thư mục chart, văn bản]}` theo `helm template`, và chart
    helm không dựng được."""
    out, failed = {}, []
    env = {"PATH": os.environ.get("PATH", ""), "HOME": workdir,
           "HELM_CACHE_HOME": workdir, "HELM_CONFIG_HOME": workdir, "HELM_DATA_HOME": workdir}
    for d in charts:
        p = subprocess.run([helm, "template", "release-name", d or ".", "--namespace", "default"],
                           cwd=root, capture_output=True, text=True, timeout=300, env=env)
        if p.returncode != 0:
            failed.append(d)
            continue
        # `alias` của chart con: helm ghi `charts/<alias>/…`, thư mục thật là
        # `charts/<name>/`.
        aliases = {}
        meta = Path(root, d, "Chart.yaml").read_text() if Path(root, d, "Chart.yaml").is_file() else ""
        for block in re.split(r"^\s*-\s+(?=name:)", meta, flags=re.M)[1:]:
            name = re.search(r"^name:\s*(\S+)", block, re.M)
            alias = re.search(r"^\s*alias:\s*(\S+)", block, re.M)
            if name and alias:
                aliases[alias.group(1)] = name.group(1)
        current = None
        for part in re.split(r"^---\s*$", p.stdout, flags=re.M):
            m = re.search(r"^# Source: ([^/\n]+)/(.+)$", part, re.M)
            if not m:
                continue
            rel = re.sub(r"^charts/([^/]+)/", lambda x: f"charts/{aliases.get(x.group(1), x.group(1))}/", m.group(2))
            path = f"{d + '/' if d else ''}{rel}"
            current = out.setdefault(path, [d, ""])
            current[1] += "---\n" + part.lstrip("\n")
    return out, failed


def compare_helm(ht, have_all, have_rendered, want_all, helm_files, failed):
    """So dữ kiện từng template archctl dựng với bản của helm."""
    for c in failed:
        ht.note(f"{c or '.'}: helm template không dựng được")
    for file, want in want_all.items():
        ht.files += 1
        if isinstance(want, str):
            ht.unparsed += 1
            continue
        ht.imports += sum(want.values())
        have = have_all.get(file)
        if file not in have_rendered or not isinstance(have, collections.Counter):
            if want:
                ht.note(f"{file}: archctl không dựng được mà helm dựng được")
            continue
        have = collections.Counter(s for (_, s) in have.elements())
        for f in sorted((want - have).elements()):
            ht.note(f"{file} thiếu: {f}")
        for f in sorted((have - want).elements()):
            ht.note(f"{file} thừa: {f}")
    for file in sorted(have_rendered - helm_files):
        if have_all.get(file):
            ht.note(f"{file}: archctl dựng ra mà helm không")


def built_key_of(built):
    """Khoá `k8s:` thuộc một bản dựng Kustomize: `k8s:<gốc>/<tên>`; gốc ở thư
    mục gốc repo thì `k8s:<tên>` (không có `/`)."""
    roots = [b["root"] for b in built]
    return lambda k: any(k.startswith(f"k8s:{r}/") if r else (k.startswith("k8s:") and "/" not in k[4:]) for r in roots)


def kustomize_keys(built, kustomize_on, have, want):
    """Đưa cạnh, NetworkPolicy, lối vào của bản dựng về cùng khoá ở hai phía:
    tên workload của thước đo (đã thêm tiền tố) đổi về tên archctl giữ (trước
    `namePrefix`); cạnh bản dựng so không kèm file, dòng (kustomize không giữ
    nguồn); không có kustomize thì bỏ mọi khoá bản dựng ở cả hai phía."""
    renamed = {}
    for b in built:
        prefix = f"k8s:{b['root'] + '/' if b['root'] else ''}"
        for d in b["docs"]:
            for w in d["facts"].get("workloads", []):
                renamed[prefix + w["name"]] = prefix + d["original"]
    ren = lambda k: renamed.get(k, k)
    w_edges, w_guards, w_exposed = want
    w_edges = {"\t".join([ren(k.split("\t")[0])] + k.split("\t")[1:]): {ren(t) for t in v} for k, v in w_edges.items()}
    w_guards = {ren(k): v for k, v in w_guards.items()}
    w_exposed = {ren(k) for k in w_exposed}
    built_key = built_key_of(built)
    out = []
    for edges, guards, exposed in (have, (w_edges, w_guards, w_exposed)):
        if not kustomize_on:
            edges = {k: collections.Counter({t: n for t, n in collections.Counter(v).items() if not built_key(t)})
                     for k, v in edges.items() if not built_key(k)}
            guards = {k: v for k, v in guards.items() if not built_key(k)}
            exposed = {k for k in exposed if not built_key(k)}
        # Bỏ dòng thì nhiều dòng gộp một khoá: đếm số lần mỗi đích (Counter),
        # để mất một cạnh trong số nhiều dòng cùng tên máy vẫn lộ ra.
        dropped = collections.defaultdict(collections.Counter)
        for k, v in edges.items():
            src, at, host = k.split("\t")
            dropped[f"{src}\t{'?' if built_key(src) else at}\t{host}"].update(v)
        out.append((dict(dropped), guards, exposed))
    return out[0], out[1]


def kustomize_roots(python, root, files):
    p = subprocess.run([python, "-I", str(HERE / "k8s_truth.py"), "kroots", str(root)],
                       input="".join(f"{f}\n" for f in files), capture_output=True, text=True, timeout=300)
    if p.returncode != 0:
        bench.die(f"thước đo Kustomize hỏng: {p.stderr.strip()[-400:]}")
    return json.loads(p.stdout)


# Gốc Kustomize mà archctl được phép báo "chưa đọc được" dù `kustomize build`
# dựng được (dùng phép ngoài tập con), theo repo. Gốc khác mà archctl không
# dựng là lệch: "chưa đọc được" không được che một bản dựng hỏng.
KUSTOMIZE_UNREAD_OK = {}


def check_roots(t, want_roots, built, unread, kustomize=None, root=None, unread_ok=()):
    """Gốc thước đo tự tính (PyYAML) so với gốc archctl dựng hay báo không dựng
    được: gốc bị bỏ sót mà không báo gì là cạnh mất im lặng. Có `kustomize` thì
    gốc archctl báo không dựng được mà kustomize dựng được cũng là lệch (trừ
    `unread_ok`)."""
    why = {}
    for u in unread:
        for r in re.findall(r"bản dựng `([^`]*)`", u["why"]):
            why.setdefault("" if r == "." else r, u["why"])
    have = {b["root"] for b in built} | set(why)
    for r in sorted(set(want_roots) - have):
        t.note(f"{r or '.'}: kustomization gốc mà archctl không dựng, không báo")
    for r in sorted(have - set(want_roots)):
        t.note(f"{r or '.'}: archctl dựng mà thước đo không coi là gốc")
    if why and kustomize is None:
        print(f"      ({len(why)} bản dựng Kustomize archctl báo chưa đọc được; không có kustomize để so)")
    if kustomize is not None:
        # Gốc có resource từ xa: kustomize sẽ `git clone` URL do repo viết,
        # tốn mạng và không ổn định; archctl cố ý không tải, không so.
        ok, _ = kustomize_build(kustomize, root, sorted(r for r in why if r not in unread_ok and "từ xa" not in why[r]))
        for r in sorted(ok):
            t.note(f"{r or '.'}: archctl không dựng mà kustomize dựng được: {why[r][:160]}")


def compare_kustomize(t, built, kbuilt, kfailed, truth):
    """So dữ kiện từng bản dựng archctl với `kustomize build`, theo gốc (không
    kèm dòng: kustomize không giữ nguồn)."""
    for d in kfailed:
        t.note(f"{d or '.'}: kustomize build không dựng được mà archctl dựng được")
    for b in built:
        r = b["root"]
        if r in kfailed:
            continue
        have = collections.Counter(s for d in b["docs"] for (_, s) in file_facts(d["facts"]))
        want = collections.Counter()
        for f, v in truth.items():
            if f.startswith(f"{r + '/' if r else ''}<kustomize>#") and isinstance(v, collections.Counter):
                want.update(v)
        t.files += 1
        t.imports += sum(want.values())
        for x in sorted((want - have).elements()):
            t.note(f"{r or '.'} thiếu: {x}")
        for x in sorted((have - want).elements()):
            t.note(f"{r or '.'} thừa: {x}")


def helm_truth(python, helm_json):
    p = subprocess.run([python, "-I", str(HERE / "k8s_truth.py"), "helmfacts", helm_json],
                       capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        bench.die(f"thước đo Helm hỏng: {p.stderr.strip()[-400:]}")
    out = {}
    for line in p.stdout.splitlines():
        file, res = line.split("\t", 1)
        out[file] = res if res.startswith("ERR") else collections.Counter(s for _, s in json.loads(res)["facts"])
    return out


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


def file_facts(f):
    """Dữ kiện `(dòng, chuỗi)` của một file manifest trong `scan --json`."""
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
    return facts


def archctl_k8s(archctl, root, files):
    """Dữ kiện từng manifest (`{file: Counter}` hay chuỗi lỗi) và cạnh
    (`{"<khoá>\\t<file>:<dòng>\\t<máy>": {đích}}`) theo `archctl scan`."""
    p = subprocess.run([archctl, "scan", "--json", "--verbatim", "--root", str(root), *files],
                       capture_output=True, text=True, timeout=600)
    if p.returncode != 0:
        bench.die(f"archctl scan hỏng ở {root.name}: {p.stderr.strip()[-300:]}")
    rep = json.loads(p.stdout)["deploy"]
    out = {}
    rendered = set()
    for f in rep["files"]:
        if f.get("kind") not in ("k8s", "skaffold"):
            continue
        if f.get("rendered"):
            rendered.add(f["file"])
        if "error" in f:
            out[f["file"]] = f"ERR {f['error']}"
            continue
        out[f["file"]] = collections.Counter(file_facts(f))
    # Đếm từng lần xuất hiện: hai bản sao cùng khoá (gốc kéo hai overlay của
    # một base) có cạnh ở cùng file, dòng; thước đo đếm từng tài liệu dựng ra.
    edges = collections.defaultdict(collections.Counter)
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
            edges[f"{s['key']}\t{e['file']}:{e['line']}\t{host}"][e["target"]] += 1
    return out, edges, guards, exposed, rendered, rep.get("charts", []), rep.get("kustomize", []), rep.get("unread", [])


def k8s_truth(python, root, files, helm_json=None, skip_edges=()):
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
    skip = Path(tempfile.mkdtemp()) / "skip.json"
    skip.write_text(json.dumps(sorted(skip_edges)))
    p = subprocess.run([python, "-I", str(HERE / "k8s_truth.py"), "edges", str(root), helm_json or "-", str(skip)],
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
        unit = {"Binding": "đầu cạnh", "Hạ tầng": "dữ kiện", "Proto": "dữ kiện", "K8s": "dữ kiện", "Helm": "dữ kiện",
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
                    help="công cụ bắt buộc phải có: clang, node, go, syn (tải crate), ipython, deploy, proto (tải gói), helm (tải binary)")
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
    unknown = set(filter(None, required)) - {"clang", "node", "go", "syn", "ipython", "deploy", "proto", "helm", "kustomize", "terraform"}
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
    use_helm = "helm" in required
    use_kustomize = "kustomize" in required
    use_tf = "terraform" in required
    use_deploy = "deploy" in required or use_helm or use_kustomize
    dep_v = deploy_version() if use_deploy else "BỎ QUA: cần tải gói, bật bằng --require deploy"
    use_proto = "proto" in required
    proto_v = proto_version() if use_proto else "BỎ QUA: cần tải gói, bật bằng --require proto"
    helm_v = HELM_VERSION if use_helm else "BỎ QUA: cần tải helm, bật bằng --require helm"
    kust_v = KUSTOMIZE_VERSION if use_kustomize else "BỎ QUA: cần tải kustomize, bật bằng --require kustomize"
    tf_v = f"OpenTofu {TOFU_VERSION}" if use_tf else "BỎ QUA: cần tải OpenTofu, bật bằng --require terraform"
    print(f"python {sys.version.split()[0]} · clang: {clang_v} · node: {node_v} · go: {go_v} · syn: {syn_v}"
          f" · ipython: {ipy_v} · hạ tầng: {dep_v} · proto: {proto_v} · helm: {helm_v} · kustomize: {kust_v}"
          f" · terraform: {tf_v}"
          + ("" if strip_ts or not have_node else " (không gỡ được kiểu TS: BỎ QUA .ts)"))

    missing_repos = [r for r in repos if not (Path(args.cache) / "repos" / r).is_dir()]
    if missing_repos:
        bench.die(f"chưa tải: {', '.join(missing_repos)} — chạy `python3 bench/bench.py` trước")

    workdir = tempfile.TemporaryDirectory(prefix="archctl-oracle-")
    go_helper = build_go_helper(workdir.name) if have_go else None
    hosts_helper = code_hosts.build_go_helper(workdir.name, bench.die) if have_go else None
    queue_helper = queue_truth.build_go_helper(workdir.name, bench.die) if have_go else None
    syn_helper = build_syn_helper(args.cache) if use_syn else None
    nb_python = build_ipython_helper(args.cache) if use_ipython else None
    dep_python = build_venv(args.cache, "deploy-oracle", DEPLOY_REQUIREMENTS, "hạ tầng") if use_deploy else None
    proto_python = build_venv(args.cache, "proto-oracle", PROTO_REQUIREMENTS, ".proto") if use_proto else None
    helm = build_helm(args.cache) if use_helm else None
    kustomize = build_kustomize(args.cache) if use_kustomize else None
    tofu, tf_mirror = build_tofu(args.cache) if use_tf else (None, None)
    tf_python = build_venv(args.cache, "tf-oracle", TF_REQUIREMENTS, "Terraform") if use_tf else None

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

        # Chỗ gửi, nhận qua hàng đợi: Python theo `ast`, Go theo `go/ast`.
        queues = Tally("Hàng đợi")
        for file in by_lang["Python"]:
            if file.endswith(PY_EXTS):
                want = queue_truth.python_truth(root / file)
                have_q = queue_truth.have(by_file[file])
                if want is None:
                    queues.unparsed += 1
                elif want or have_q:
                    check_lines(queues, file, want, have_q)
        if queue_helper and by_lang["Go"]:
            for file, want in queue_truth.go_truth(queue_helper, root, by_lang["Go"], bench.die).items():
                have_q = queue_truth.have(by_file[file])
                if want is None:
                    queues.unparsed += 1
                elif want or have_q:
                    check_lines(queues, file, want, have_q)
        if queues.files:
            tallies.append(queues)

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
            have_all, have_edges, have_guards, have_exposed, have_rendered, charts, built, dunread = archctl_k8s(archctl, root, yamls)
            # Chart Helm: `helm template` thật dựng ra, đọc như manifest ở thư
            # mục chart. Không có helm thì bỏ mọi thứ archctl dựng từ chart.
            roots = [c for c in charts if not any(c != o and c.startswith(f"{o}/charts/" if o else "charts/") for o in charts)]
            helm_json, helm_files, failed = None, set(), []
            rendered = {}
            if helm and roots:
                rendered, failed = helm_render(helm, root, roots, workdir.name)
                helm_files = set(rendered)
            # Bản dựng Kustomize: `kustomize build` thật, đọc như manifest ở thư
            # mục kustomization gốc. File mà bản dựng ở chính thư mục của nó đã
            # gồm thì archctl không đọc thô nữa; thước đo cũng vậy.
            built_roots = [b["root"] for b in built]
            same_dir = {d["facts"]["file"] for b in built for d in b["docs"]
                        if (d["facts"]["file"].rsplit("/", 1)[0] if "/" in d["facts"]["file"] else "") == b["root"]}
            kbuilt, kfailed = ({}, [])
            if kustomize and built_roots:
                kbuilt, kfailed = kustomize_build(kustomize, root, built_roots)
                for d, docs in kbuilt.items():
                    for i, t in enumerate(docs):
                        rendered[f"{d + '/' if d else ''}<kustomize>#{i}"] = [d, t]
            if rendered:
                helm_json = str(Path(workdir.name) / f"helm-{repo}.json")
                Path(helm_json).write_text(json.dumps(rendered))
            want_all, want_edges, want_guards, want_exposed = k8s_truth(dep_python, root, yamls, helm_json, same_dir)
            (have_edges, have_guards, have_exposed), (want_edges, want_guards, want_exposed) = kustomize_keys(
                built, bool(kustomize), (have_edges, have_guards, have_exposed), (want_edges, want_guards, want_exposed))
            chart_key = lambda k: any(k.startswith(f"k8s:{c}/") for c in roots)
            if not helm:
                have_edges = {k: v for k, v in have_edges.items() if not chart_key(k)}
                have_edges = {k: collections.Counter({t: n for t, n in v.items() if not chart_key(t)})
                              for k, v in have_edges.items()}
                have_guards = {k: v for k, v in have_guards.items() if not chart_key(k)}
                have_exposed = {k for k in have_exposed if not chart_key(k)}
            # Dòng của cạnh từ chart: archctl báo dòng template, helm chỉ có
            # dòng của văn bản dựng ra; so không kèm dòng.
            # Cạnh từ template Helm: một dòng template dựng ra nhiều dòng (vòng
            # `range`), archctl báo một lần: so theo tập (mỗi đích một lần).
            def no_line(edges):
                out = {}
                for k, v in edges.items():
                    nk = re.sub(r"\t(" + "|".join(map(re.escape, helm_files or ["\0"])) + r"):\d+\t", r"\t\1:?\t", k)
                    if nk == k:
                        out[k] = collections.Counter(v)
                    else:
                        out[nk] = collections.Counter(dict.fromkeys(set(out.get(nk, {})) | set(v), 1))
                return out
            have_edges, want_edges = no_line(have_edges), no_line(want_edges)
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
                want, have = want_edges.get(key, collections.Counter()), have_edges.get(key, collections.Counter())
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
            if helm and roots:
                ht = Tally("Helm")
                compare_helm(ht, have_all, have_rendered,
                             {f: v for f, v in helm_truth(dep_python, helm_json).items() if "<kustomize>" not in f},
                             helm_files, failed)
                tallies.append(ht)
            kroots = kustomize_roots(dep_python, root, yamls)
            if kroots or built_roots:
                kt2 = Tally("Kustomize")
                check_roots(kt2, kroots, built, dunread, kustomize or None, root, KUSTOMIZE_UNREAD_OK.get(repo, ()))
                if kustomize:
                    compare_kustomize(kt2, built, kbuilt, kfailed, helm_truth(dep_python, helm_json))
                tallies.append(kt2)

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
        if tofu:
            tt = Tally("Terraform")
            compare_terraform(tt, archctl, tofu, tf_mirror, tf_python, root, workdir.name)
            tallies.append(tt)
        for t in tallies:
            sane(t, repo)
            if t.files or t.unparsed or t.skipped:
                print(t.line(repo))
                for e in t.examples:
                    print(f"      {e}")
            bad += t.bad

    # Chart mẫu trong `bench/fixtures/helm-*`: các ca chart thật của bộ đo
    # không có (chart con không override, `alias`, `condition` dạng danh sách,
    # `tags`, `define` trùng tên, template `.tpl`/`.json`/`tests/`, chart ở gốc).
    if helm and not args.only:
        for fx in sorted(HERE.glob("fixtures/helm-*")):
            # Cả thư mục: chart mẫu có thể chưa commit.
            have_all, _, _, _, have_rendered, charts, _, _ = archctl_k8s(archctl, fx, [])
            roots = [c for c in charts if not any(c != o and c.startswith(f"{o}/charts/" if o else "charts/") for o in charts)]
            rendered, failed = helm_render(helm, fx, roots, workdir.name)
            helm_json = str(Path(workdir.name) / f"helm-{fx.name}.json")
            Path(helm_json).write_text(json.dumps(rendered))
            ht = Tally("Helm")
            compare_helm(ht, have_all, have_rendered, helm_truth(dep_python, helm_json), set(rendered), failed)
            print(ht.line(fx.name))
            for e in ht.examples:
                print(f"      {e}")
            bad += ht.bad

    # Overlay mẫu trong `bench/fixtures/kustomize-*`: các phép biến đổi mà
    # repo thật của bộ đo không dùng (patch strategic merge và JSON 6902,
    # `namePrefix`, nhãn, `images`, `configMapGenerator`, component).
    if kustomize and not args.only:
        for fx in sorted(HERE.glob("fixtures/kustomize-*")):
            yamls_fx = sorted(str(p.relative_to(fx)) for p in fx.rglob("*.yaml"))
            _, h_edges, h_guards, h_exposed, _, _, built_fx, unread_fx = archctl_k8s(archctl, fx, yamls_fx)
            kb, kf = kustomize_build(kustomize, fx, [b["root"] for b in built_fx])
            rendered_fx = {f"{d + '/' if d else ''}<kustomize>#{i}": [d, t] for d, docs in kb.items() for i, t in enumerate(docs)}
            kjson = str(Path(workdir.name) / f"kustomize-{fx.name}.json")
            Path(kjson).write_text(json.dumps(rendered_fx))
            same_fx = {d["facts"]["file"] for b in built_fx for d in b["docs"]
                       if (d["facts"]["file"].rsplit("/", 1)[0] if "/" in d["facts"]["file"] else "") == b["root"]}
            _, w_edges, w_guards, w_exposed = k8s_truth(dep_python, fx, yamls_fx, kjson, same_fx)
            (h_edges, h_guards, h_exposed), (w_edges, w_guards, w_exposed) = kustomize_keys(
                built_fx, True, (h_edges, h_guards, h_exposed), (w_edges, w_guards, w_exposed))
            kt3 = Tally("Kustomize")
            check_roots(kt3, kustomize_roots(dep_python, fx, yamls_fx), built_fx, unread_fx, kustomize, fx)
            compare_kustomize(kt3, built_fx, kb, kf, helm_truth(dep_python, kjson))
            # Cạnh, NetworkPolicy, lối vào của bản dựng: tính lại trên bản kustomize.
            bk = built_key_of(built_fx)
            for key in sorted(k for k in set(w_edges) | set(h_edges) if bk(k.split("\t")[0])):
                want, have = w_edges.get(key, collections.Counter()), h_edges.get(key, collections.Counter())
                kt3.imports += len(want)
                for x in sorted(want - have):
                    kt3.note(f"thiếu cạnh: {key} → {x}")
                for x in sorted(have - want):
                    kt3.note(f"thừa cạnh: {key} → {x}")
            for key in sorted(k for k in set(w_guards) | set(h_guards) if bk(k)):
                if w_guards.get(key) != h_guards.get(key):
                    kt3.note(f"{key} lệch NetworkPolicy: thước đo {w_guards.get(key)}, archctl {h_guards.get(key)}")
            for key in sorted({k for k in w_exposed ^ h_exposed if bk(k)}):
                kt3.note(f"{key} lệch lối vào từ Internet")
            print(kt3.line(fx.name))
            for e in kt3.examples:
                print(f"      {e}")
            bad += kt3.bad

    # Mẫu Terraform trong `bench/fixtures/terraform-*`: module trong repo có
    # đối số, output; ECS qua `jsonencode` và heredoc; cạnh qua `locals`, bộ cân
    # bằng tải, cơ sở dữ liệu, hàng đợi; các dạng lối vào.
    if tofu and not args.only:
        for fx in sorted(HERE.glob("fixtures/terraform-*")):
            tt = Tally("Terraform")
            compare_terraform(tt, archctl, tofu, tf_mirror, tf_python, fx, workdir.name, fx.name.endswith("-refused"))
            print(tt.line(fx.name))
            for e in tt.examples:
                print(f"      {e}")
            bad += tt.bad

    # Mẫu hàng đợi trong `bench/fixtures/queues`: các dạng mà repo thật của bộ
    # đo không có (env, hằng số, khai queue rồi dùng qua biến, Celery, Redis).
    if not args.only:
        fx = HERE / "fixtures" / "queues"
        files = archctl_scan(archctl, fx)
        by_file = {f["file"]: f for f in files}
        qt = Tally("Hàng đợi")
        for file in sorted(by_file):
            if file.endswith(".py"):
                want = queue_truth.python_truth(fx / file)
            elif file.endswith(".go") and queue_helper:
                want = queue_truth.go_truth(queue_helper, fx, [file], bench.die)[file]
            else:
                continue
            check_lines(qt, file, want, queue_truth.have(by_file[file]))
        print(qt.line(fx.name))
        for e in qt.examples:
            print(f"      {e}")
        bad += qt.bad

    print(f"\n{'khớp hết' if not bad else f'{bad} chỗ lệch so với parser chuẩn'}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
