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

Python, notebook, C/C++, Go, Rust: thiếu, thừa và lệch số dòng đều là lệch. Thiếu clang, Node hay Go
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

    fn visit_item_mod(&mut self, m: &'a syn::ItemMod) {
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
        writeln!(out, "{file}\t{{\"imports\":[{}],\"macros\":[{}]}}", imports.join(","), macros.join(",")).unwrap();
    }
}
"""

CLANG_TOKEN = re.compile(r"^(\w+) '(.*)'\t( \[StartOfLine\])?.*Loc=<.*:(\d+):\d+>$")
INCLUDE_SPEC = re.compile(r'#\s*include\s*([<"])([^>"\n]+)[>"]')


def archctl_scan(archctl, root):
    try:
        p = subprocess.run([archctl, "scan", "--json", "--root", str(root)],
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
    """`(dòng, spec)` theo `ast`, hoặc None nếu file không parse được."""
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            tree = ast.parse(path.read_bytes())
        except (SyntaxError, ValueError):
            return None
    out = collections.Counter()
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                out[(node.lineno, alias.name)] += 1
        elif isinstance(node, ast.ImportFrom):
            out[(node.lineno, "." * node.level + (node.module or ""))] += 1
    return out


def clang_truth(path):
    """`(dòng, spec)` của mọi `#include` thật theo lexer thô của clang."""
    text = path.read_text(errors="replace")
    if "include" not in text:
        return collections.Counter()
    lang = ["-x", "c", "-std=c2x"] if path.suffix == ".c" else ["-x", "c++", "-std=c++20"]
    p = subprocess.run(["clang", "-fsyntax-only", *lang, "-Xclang", "-dump-raw-tokens", str(path)],
                       capture_output=True, text=True, errors="replace", timeout=300)
    if p.returncode != 0:
        bench.die(f"clang thoát {p.returncode} ở {path}: {p.stderr.strip()[-300:]}")
    # Đếm dòng như clang: chỉ theo `\n`. `splitlines()` tách cả ở form feed
    # (`^L` hay gặp trong mã kiểu GNU) và lệch số dòng.
    lines = text.split("\n")
    out, hash_line = collections.Counter(), None
    for row in p.stderr.splitlines():
        m = CLANG_TOKEN.match(row)
        if not m or m.group(1) == "unknown":
            continue
        kind, tok, start, line = m.group(1), m.group(2), m.group(3), int(m.group(4))
        if kind == "hash" and start:
            hash_line = line
            continue
        if hash_line == line and kind == "raw_identifier" and tok == "include":
            spec = INCLUDE_SPEC.search(lines[line - 1]) if line <= len(lines) else None
            if spec:
                out[(line, spec.group(2))] += 1
        hash_line = None
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
    """file → (Counter `(dòng, đường dẫn)`, các khoảng dòng của macro); None nếu syn không đọc được."""
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
        out[file] = (collections.Counter((line, spec) for line, spec in v["imports"]), v["macros"])
    unanswered = sorted(set(files) - set(out))
    if unanswered:
        bench.die(f"trình đọc `syn` không trả lời cho {len(unanswered)} file ở {root}, ví dụ {unanswered[0]}")
    return out


IPYTHON_REQUIREMENTS = HERE / "ipython-requirements.txt"


def ipython_version():
    m = re.search(r"^ipython==(\S+)", IPYTHON_REQUIREMENTS.read_text(), re.M)
    return m.group(1) if m else "?"


def build_ipython_helper(cache):
    """Venv có IPython dưới `cache/ipython-oracle`; lần sau dùng lại nếu bản ghim không đổi.

    Cài đúng file đã ghim kèm hash, chỉ wheel: gói bị chiếm quyền trên PyPI hay
    bị tráo trên đường đi thì pip từ chối, và không `setup.py` nào chạy lúc cài.
    """
    home = Path(cache) / "ipython-oracle"
    venv, installed = home / "venv", home / "installed.txt"
    python = venv / "bin" / "python"
    wanted = IPYTHON_REQUIREMENTS.read_text()
    if not (python.exists() and installed.exists() and installed.read_text() == wanted):
        shutil.rmtree(venv, ignore_errors=True)
        home.mkdir(parents=True, exist_ok=True)
        for cmd in ([sys.executable, "-m", "venv", str(venv)],
                    [str(python), "-m", "pip", "install", "--quiet", "--disable-pip-version-check",
                     "--no-deps", "--only-binary", ":all:", "--require-hashes",
                     "--requirement", str(IPYTHON_REQUIREMENTS)]):
            try:
                p = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
            except subprocess.TimeoutExpired:
                bench.die(f"dựng thước đo IPython: `{cmd[2]}` quá 600 giây")
            if p.returncode != 0:
                bench.die(f"dựng thước đo IPython hỏng: {(p.stderr or p.stdout).strip()[-400:]}")
        installed.write_text(wanted)
    return str(python)


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
        self.bad, self.examples = 0, []
        self.in_macro = 0  # import archctl đọc được trong thân macro, parser không thấy

    def note(self, msg):
        self.bad += 1
        if len(self.examples) < EXAMPLES:
            self.examples.append(msg)

    def line(self, repo):
        head = f"  {repo:22} {self.label:8} {self.files:5} file, {self.imports:6} import"
        tail = f", {self.unparsed} không parse được" if self.unparsed else ""
        if self.in_macro:
            tail += f", {self.in_macro} trong macro không đối chiếu"
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
                    help="công cụ bắt buộc phải có: clang, node, go, syn (tải crate), ipython (tải gói)")
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
    unknown = set(filter(None, required)) - {"clang", "node", "go", "syn", "ipython"}
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
    print(f"python {sys.version.split()[0]} · clang: {clang_v} · node: {node_v} · go: {go_v} · syn: {syn_v}"
          f" · ipython: {ipy_v}"
          + ("" if strip_ts or not have_node else " (không gỡ được kiểu TS: BỎ QUA .ts)"))

    missing_repos = [r for r in repos if not (Path(args.cache) / "repos" / r).is_dir()]
    if missing_repos:
        bench.die(f"chưa tải: {', '.join(missing_repos)} — chạy `python3 bench/bench.py` trước")

    workdir = tempfile.TemporaryDirectory(prefix="archctl-oracle-")
    go_helper = build_go_helper(workdir.name) if have_go else None
    syn_helper = build_syn_helper(args.cache) if use_syn else None
    nb_python = build_ipython_helper(args.cache) if use_ipython else None

    bad = 0
    for repo in repos:
        root = Path(args.cache) / "repos" / repo
        files = archctl_scan(archctl, root)
        got = {f["file"]: collections.Counter((i["line"], i["spec"]) for i in f["imports"])
               for f in files}
        by_lang = collections.defaultdict(list)
        for f in files:
            by_lang[f["lang"]].append(f["file"])
        tallies = []

        py = Tally("Python")
        for file in by_lang["Python"]:
            if not file.endswith(PY_EXTS):
                continue  # Cython: ast không đọc được
            want = python_truth(root / file)
            if want is None:
                py.unparsed += 1
                continue
            check_lines(py, file, want, got[file])
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
                truths = pool.map(lambda f: clang_truth(root / f), by_lang["C/C++"])
                for file, want in zip(by_lang["C/C++"], truths):
                    check_lines(c, file, want, got[file])
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

        if syn_helper and by_lang["Rust"]:
            rs = Tally("Rust")
            for file, truth in syn_truth(syn_helper, root, by_lang["Rust"]).items():
                if truth is None:
                    rs.unparsed += 1
                    continue
                want, macros = truth
                have = collections.Counter(got[file])
                for (line, spec), n in (have - want).items():
                    if any(a <= line <= b for a, b in macros):
                        have[(line, spec)] -= n
                        rs.in_macro += n
                check_lines(rs, file, want, +have)
            tallies.append(rs)

        for t in tallies:
            sane(t, repo)
            if t.files or t.unparsed:
                print(t.line(repo))
                for e in t.examples:
                    print(f"      {e}")
            bad += t.bad

    print(f"\n{'khớp hết' if not bad else f'{bad} chỗ lệch so với parser chuẩn'}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
