#!/usr/bin/env python3
"""Đối chiếu import archctl đọc được với parser chuẩn của từng ngôn ngữ.

    python3 bench/oracle.py [--archctl <binary>] [--only flask,curl] [--require clang,node]

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

Go và Rust không có parser chuẩn chạy được ở đây.

Python, C/C++: thiếu, thừa và lệch số dòng đều là lệch. Thiếu clang hay Node
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

    def note(self, msg):
        self.bad += 1
        if len(self.examples) < EXAMPLES:
            self.examples.append(msg)

    def line(self, repo):
        head = f"  {repo:22} {self.label:6} {self.files:5} file, {self.imports:6} import"
        tail = f", {self.unparsed} không parse được" if self.unparsed else ""
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
    ap.add_argument("--require", default="", help="công cụ bắt buộc phải có: clang, node")
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
    strip_ts = have_node and node_can_strip_types()
    required = args.require.split(",")
    for tool, ok in [("clang", have_clang), ("node", have_node)]:
        if tool in required and not ok:
            bench.die(f"thiếu `{tool}`, mà `--require` đòi có")
    if "node" in required and not strip_ts:
        bench.die("node không có `module.stripTypeScriptTypes` (cần Node ≥ 22.13), "
                  "mà `--require node` đòi đối chiếu cả TS")
    clang_v = tool_version(["clang", "--version"]) if have_clang else "BỎ QUA: không có"
    node_v = tool_version(["node", "--version"]) if have_node else "BỎ QUA: không có"
    print(f"python {sys.version.split()[0]} · clang: {clang_v} · node: {node_v}"
          + ("" if strip_ts or not have_node else " (không gỡ được kiểu TS: BỎ QUA .ts)"))

    missing_repos = [r for r in repos if not (Path(args.cache) / "repos" / r).is_dir()]
    if missing_repos:
        bench.die(f"chưa tải: {', '.join(missing_repos)} — chạy `python3 bench/bench.py` trước")

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
