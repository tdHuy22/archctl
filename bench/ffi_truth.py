"""Thước đo cho Python nạp thư viện native lúc chạy (`scan --json`, mục
`crossings`: khoá `lib:` của `calls`, và `unread`).

Lời gọi và chuỗi lấy từ `ast`, không từ lexer của archctl; luật chọn tên viết
lại theo đúng đặc tả trong `crates/arch-core/src/scan/foreign.rs`
(`library_loads`, `library_key`). Chỗ hai bên lệch là chỗ lexer hay bộ dò của
archctl đọc sai.

So `(dòng, "lib:tên")` và `(dòng, "?")` cho lời nạp không đọc được tên. Chỉ
thư viện chuẩn của Python.
"""
import ast
import collections
import re
import warnings

LOADERS = {"CDLL", "PyDLL", "WinDLL", "OleDLL", "LoadLibrary", "load_library"}
ATTRS = {"cdll", "pydll", "windll", "oledll"}
KEY = re.compile(r"[A-Za-z0-9_+\-]{1,128}")


def library_key(path):
    base = re.split(r"[/\\]", path)[-1]
    stem = base.split(".")[0]
    if stem.startswith("lib") and len(stem) > 3:
        stem = stem[3:]
    return f"lib:{stem}" if KEY.fullmatch(stem) else None


def chain(node):
    """Các đoạn của `a.b.c` tính từ cuối, dừng ở chỗ không phải tên (`f().x`)."""
    parts = []
    while isinstance(node, ast.Attribute):
        parts.append(node.attr)
        node = node.value
    if isinstance(node, ast.Name):
        parts.append(node.id)
    return parts[::-1]


def strings(node):
    """Chuỗi trong các đối số theo thứ tự trong mã, kèm `id` của nút; f-string có
    `\\0` ở chỗ nội suy."""
    inner = {id(v) for j in ast.walk(node) if isinstance(j, ast.JoinedStr) for v in j.values}
    out = []
    for n in sorted((x for x in ast.walk(node) if isinstance(x, (ast.Constant, ast.JoinedStr))),
                    key=lambda x: (x.lineno, x.col_offset)):
        if isinstance(n, ast.JoinedStr):
            out.append(("".join(v.value if isinstance(v, ast.Constant) else "\0" for v in n.values), id(n)))
        elif isinstance(n.value, str) and id(n) not in inner:
            out.append((n.value, id(n)))
    return out


def truth(path):
    """Counter `(dòng, khoá)`; None nếu `ast` không đọc được file."""
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            tree = ast.parse(path.read_bytes())
    except (SyntaxError, ValueError, RecursionError, MemoryError):
        return None
    names = set()
    for n in ast.walk(tree):
        if isinstance(n, ast.Name):
            names.add(n.id)
        elif isinstance(n, ast.Attribute):
            names.add(n.attr)
        elif isinstance(n, (ast.Import, ast.ImportFrom)):
            for a in n.names:
                names.update(a.name.split("."))
            if isinstance(n, ast.ImportFrom) and n.module:
                names.update(n.module.split("."))
    ctypes = bool(names & {"ctypes", "ctypeslib"})
    cffi = bool(names & {"cffi", "FFI"})
    out = collections.Counter()
    if not ctypes and not cffi:
        return out
    inner = {id(n.value) for n in ast.walk(tree) if isinstance(n, ast.Attribute)}
    for n in ast.walk(tree):
        # `ctypes.cdll.foo`: chuỗi thuộc tính dài nhất, không phải `.LoadLibrary`.
        if isinstance(n, ast.Attribute) and id(n) not in inner and ctypes:
            segs = chain(n)
            if len(segs) >= 2 and segs[-2] in ATTRS and segs[-1] not in LOADERS:
                key = library_key(segs[-1])
                if key:
                    out[(n.end_lineno, key)] += 1
        if not isinstance(n, ast.Call):
            continue
        segs = chain(n.func) or ([n.func.attr] if isinstance(n.func, ast.Attribute) else [])
        if not segs:
            continue
        last = segs[-1]
        if not ((ctypes and last in LOADERS) or (cffi and last == "dlopen" and len(segs) >= 2)):
            continue
        args = ast.Call(func=ast.Name(id="_"), args=n.args, keywords=n.keywords)
        first = n.args[0] if n.args else None
        if isinstance(first, ast.Constant) and first.value is None:
            continue
        # Chuỗi trọn một đối số (vị trí hay từ khoá).
        whole = {id(a) for a in list(n.args) + [k.value for k in n.keywords]
                 if isinstance(a, ast.JoinedStr) or (isinstance(a, ast.Constant) and isinstance(a.value, str))}
        found = strings(args)

        def libish(s):
            base = re.split(r"[/\\]", s)[-1]
            return base.startswith("lib") or any(e in base for e in (".so", ".dylib", ".dll", ".pyd"))
        if last == "load_library":
            a0 = n.args[0] if n.args else None
            name = a0.value if isinstance(a0, ast.Constant) and isinstance(a0.value, str) else None
        else:
            name = next((v for v, i in reversed(found) if libish(v) or i in whole), None)
        base = re.split(r"[/\\]", name)[-1] if name is not None else None
        key = library_key(base) if base is not None and "\0" not in base else None
        out[(n.func.end_lineno, key or "?")] += 1
    return out


def have(scanned_file):
    c = scanned_file.get("crossings", {})
    out = collections.Counter((x["line"], x["key"]) for x in c.get("calls", []) if x["key"].startswith("lib:"))
    out.update((x["line"], "?") for x in c.get("unread", []))
    return out
