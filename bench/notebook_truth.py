"""Thước đo import trong notebook cho `oracle.py`. Chạy trong venv có IPython.

Đọc "<gốc>\\t<file>" từ stdin, in "<file>\\t<json>" với json là
`{"imports": [[dòng, spec]], "exact": bool}`, hay "<file>\\tERR" khi JSON hỏng,
không phải nbformat 4, hay một ô không parse được.

- Ô code biến đổi bằng `TransformerManager` của IPython, đúng như Jupyter:
  magic thành lời gọi hàm, và chuỗi, ngoặc được tôn trọng. Rồi `ast` đọc import.
- Dòng của từng phần tử `source` lấy khi duyệt JSON; chuỗi giải mã bằng
  `scanstring` của thư viện `json` chuẩn. Khoá trùng thì khoá sau thắng, như
  `json.load`.
- `exact` là mỗi dòng mã có chuỗi JSON của riêng nó, các chuỗi đó nằm ở những
  dòng JSON tăng dần, và IPython không gộp dòng nào. Chỉ khi đó archctl hứa số
  dòng chính xác; còn lại `oracle.py` chỉ so tên.
"""
import ast
import json
import sys
import warnings
from json.decoder import scanstring

from IPython.core.inputtransformer2 import TransformerManager

TM = TransformerManager()
# Magic của cả ô mà thân ô vẫn là Python. Đây là quy ước của archctl
# (`scan/notebook.rs`); IPython chỉ biết gói cả thân ô vào một chuỗi.
PY_CELL_MAGICS = {"time", "timeit", "capture", "prun", "debug"}
WITH_VALUE = {"--out", "--err", "--proc"}


class Walker:
    """JSON kèm dòng (đếm từ 1) nơi mỗi chuỗi mở. Chuỗi thành `(nội dung, dòng)`."""

    def __init__(self, text):
        self.s, self.i, self.line = text, 0, 1

    def ws(self):
        while self.i < len(self.s) and self.s[self.i] in " \t\r\n":
            self.line += self.s[self.i] == "\n"
            self.i += 1

    def expect(self, c):
        self.ws()
        if self.s[self.i] != c:
            raise ValueError(f"cần {c!r} ở {self.i}")
        self.i += 1

    def value(self):
        self.ws()
        c = self.s[self.i]
        if c == "{":
            return self.container("}", True)
        if c == "[":
            return self.container("]", False)
        if c == '"':
            return self.string()
        val, self.i = json.JSONDecoder().raw_decode(self.s, self.i)
        return val

    def container(self, close, is_object):
        self.i += 1
        out = {} if is_object else []
        self.ws()
        if self.s[self.i] == close:
            self.i += 1
            return out
        while True:
            if is_object:
                self.ws()
                key = self.string()[0]
                self.expect(":")
                out[key] = self.value()
            else:
                out.append(self.value())
            self.ws()
            c = self.s[self.i]
            self.i += 1
            if c == close:
                return out
            if c != ",":
                raise ValueError(f"cần , hay {close} ở {self.i}")

    def string(self):
        if self.s[self.i] != '"':
            raise ValueError(f"cần chuỗi ở {self.i}")
        line = self.line
        text, end = scanstring(self.s, self.i + 1)
        self.line += self.s.count("\n", self.i, end)
        self.i = end
        return (text, line)


def text_of(v):
    return v[0] if isinstance(v, tuple) else None


def script_program(words):
    """`%%script --bg --out log python3`, `/usr/bin/python3`, `env python3` ra `python3`."""
    words = iter(words)
    for w in words:
        if w in WITH_VALUE:
            next(words, None)
        elif not w.startswith("-"):
            name = w.rsplit("/", 1)[-1]
            if name != "env":
                return name
            rest = [x for x in words if not x.startswith("-")]
            return rest[0].rsplit("/", 1)[-1] if rest else ""
    return ""


def runs_python(first):
    words = first[2:].split()
    name = words[0] if words else ""
    return (name in PY_CELL_MAGICS or name.startswith("python")
            or (name == "script" and script_program(words[1:]).startswith("python")))


def is_python(root):
    meta = root.get("metadata") if isinstance(root.get("metadata"), dict) else {}
    for outer, inner in (("kernelspec", "language"), ("language_info", "name")):
        o = meta.get(outer)
        if isinstance(o, dict) and text_of(o.get(inner)) is not None:
            if text_of(o[inner]).lower() != "python":
                return False
    return True


def imports(code):
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        tree = ast.parse(code)
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                yield node.lineno, alias.name
        elif isinstance(node, ast.ImportFrom):
            yield node.lineno, "." * node.level + (node.module or "")


def line_starts(parts):
    """Dòng JSON nơi mỗi dòng của `"".join(phần tử)` bắt đầu. Phần tử không kết
    thúc bằng xuống dòng thì phần tử sau viết tiếp vào dòng đó."""
    starts, at_start = [], True
    for text, line in parts:
        newlines = 0
        for ch in text:
            if at_start:
                starts.append(line + newlines)
                at_start = False
            if ch == "\n":
                newlines += 1
                at_start = True
    if at_start:  # dòng rỗng cuối, sau dấu xuống dòng cuối cùng
        starts.append(starts[-1] + 1 if starts else 0)
    return starts


def truth(path):
    with open(path, encoding="utf-8") as f:
        text = f.read()
    text = text[1:] if text.startswith("﻿") else text
    w = Walker(text)
    root = w.value()
    w.ws()
    if w.i != len(text) or not isinstance(root, dict) or "cells" not in root:
        raise ValueError("không phải notebook nbformat 4")
    out, placed, exact = [], [], True
    if not is_python(root) or not isinstance(root["cells"], list):
        return out, exact
    for cell in root["cells"]:
        if not isinstance(cell, dict) or text_of(cell.get("cell_type")) != "code":
            continue
        src = cell.get("source")
        parts = [src] if isinstance(src, tuple) else [x for x in (src or []) if isinstance(x, tuple)]
        # Một chuỗi chứa nhiều dòng mã thì các dòng sau không có dòng JSON của
        # riêng chúng: archctl chỉ hứa gần đúng, nên chỉ so tên.
        if any("\n" in text[:-1] for text, _ in parts):
            exact = False
        code = "".join(t for t, _ in parts)
        lines = code.split("\n")
        first = next((i for i, l in enumerate(lines) if l.strip()), None)
        if first is None:
            continue
        base = 0
        if lines[first].strip().startswith("%%"):
            if not runs_python(lines[first].strip()):
                continue
            base = first + 1
        starts = line_starts(parts)
        # archctl đặt mọi dòng của ô, trừ dòng rỗng sau dấu xuống dòng cuối.
        placed.extend(starts[:len(lines) - code.endswith("\n")])
        body = lines[base:]
        # IPython bỏ các dòng trống đầu ô, nên số dòng của `ast` lệch đúng chừng đó.
        lead = 0
        while lead < len(body) - 1 and not body[lead].strip():
            lead += 1
        before = "\n".join(body[lead:])
        after = TM.transform_cell("\n".join(body))
        # IPython gộp dòng khi magic hay `!` có `\` nối dòng: số dòng của `ast`
        # khi đó không còn trỏ đúng dòng mã. So tên thôi.
        if after.rstrip("\n").count("\n") != before.rstrip("\n").count("\n"):
            exact = False
        for lineno, spec in imports(after):
            idx = min(base + lead + lineno - 1, len(starts) - 1)
            out.append([starts[idx], spec])
    exact = exact and all(a < b for a, b in zip(placed, placed[1:]))
    return out, exact


def main():
    for raw in sys.stdin:
        root, file = raw.rstrip("\n").split("\t")
        try:
            imps, exact = truth(f"{root}/{file}")
            print(f"{file}\t{json.dumps({'imports': imps, 'exact': exact})}", flush=True)
        except (ValueError, IndexError, SyntaxError, UnicodeDecodeError, RecursionError):
            print(f"{file}\tERR", flush=True)


if __name__ == "__main__":
    main()
