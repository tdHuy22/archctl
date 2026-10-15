"""Thước đo cho tên máy viết thẳng trong mã (`scan --json`, mục `hosts`).

Chuỗi và chỗ nó đứng lấy từ parser chuẩn, không từ lexer của archctl:
`ast` cho Python, `go/ast` cho Go (dựng một lần thành binary tạm). Luật đổi
chuỗi thành tên máy viết lại ở đây theo đúng đặc tả trong
`crates/arch-core/src/scan/hosts.rs` và `compose::hosts_scan`; chỗ hai bên lệch
là chỗ lexer hay bộ tìm khoá của archctl đọc sai.

So `(dòng, máy:cổng)`; khoá chỉ dùng để quyết chuỗi trần có tính không. JS/TS
không có thước đo: Node không có parser AST dựng sẵn.
"""
import ast
import collections
import io
import json
import re
import subprocess
import tokenize
import warnings

MAX_PER_FILE = 1000
MAX_PARTS = 256
MAX_HOSTS = 64

HOSTISH = {"HOST", "HOSTNAME", "HOSTS", "ADDR", "ADDRESS", "SERVER", "SERVERS", "URL", "URLS", "URI", "URIS",
           "ENDPOINT", "ENDPOINTS", "DSN", "BROKER", "BROKERS"}
SECRET_PARTS = {"KEY", "KEYS", "TOKEN", "TOKENS", "SECRET", "SECRETS", "PASSWORD", "PASSWD", "PASS", "PWD", "SALT",
                "CREDENTIAL", "CREDENTIALS", "CREDS", "AUTH", "PRIVATE", "CERT", "SIGNATURE", "JWT", "APIKEY", "OTP"}
CODE_SECRETS = {"PW", "PIN", "PASSPHRASE", "PRIVKEY", "USERINFO", "LOGIN"}
URLISH = {"URL", "URLS", "URI", "URIS", "ENDPOINT", "ENDPOINTS"}
LABELS = {"IMAGE", "IMAGES", "TAG", "TAGS", "VERSION", "REVISION", "DIGEST"}
INBOUND = {"ALLOWED", "TRUSTED", "CORS", "ORIGIN", "ORIGINS", "BIND", "LISTEN"}
GETTERS = {"get", "getenv", "get_env", "getEnv", "Getenv", "GetEnv", "env", "getOrDefault", "getProperty",
           "GetString", "getString", "GetValue", "setdefault", "LookupEnv"}
HOST_FUNCS = {"JoinHostPort", "gethostbyname", "gethostbyname_ex", "getaddrinfo", "LookupHost"}
ENV_HOST_SUFFIXES = {"HOST", "HOSTNAME", "HOSTS", "ADDR", "ADDRESS", "SERVER", "SERVERS", "URL", "URLS", "URI",
                     "URIS", "ENDPOINT", "ENDPOINTS", "DSN", "BROKER", "BROKERS", "PEERS", "NODES", "CLUSTER",
                     "SERVICE"}
ALNUM = set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789")


def words(name):
    out, cur, prev_lower = [], "", False
    for c in name:
        if c not in ALNUM:
            if cur:
                out.append(cur)
                cur = ""
            prev_lower = False
            continue
        if c.isupper() and prev_lower and cur:
            out.append(cur)
            cur = ""
        prev_lower = c.islower() or c.isdigit()
        cur += c.upper()
    if cur:
        out.append(cur)
    return out


def kind(name):
    parts = words(name)
    hostish = bool(parts) and parts[-1] in HOSTISH
    if any(p in INBOUND for p in parts):
        return "INBOUND"
    if any(p in SECRET_PARTS or p in CODE_SECRETS or p in LABELS for p in parts) and not hostish:
        return "TOKEN"
    if (parts and parts[-1] in HOSTISH) or parts == ["DATA", "SOURCE"]:
        return "HOST"
    return ""


def keyish(s):
    return isinstance(s, str) and 0 < len(s) <= 128 and re.fullmatch(r"[A-Za-z0-9_.\-]+", s) is not None


def defaults(value):
    out, rest = [], value
    while "${" in rest:
        at = rest.index("${")
        out.append(rest[:at])
        body = rest[at + 2:]
        if "}" not in body:
            out.append(rest[at:])
            return "".join(out)
        end = body.index("}")
        inner = body[:end]
        if ":-" in inner:
            out.append(inner.split(":-", 1)[1])
        elif "-" in inner:
            out.append(inner.split("-", 1)[1])
        else:
            out.append(rest[at:at + 2 + end + 1])
        rest = body[end + 1:]
    out.append(rest)
    return "".join(out)


def digits(s):
    return bool(re.fullmatch(r"[0-9]+", s))


def hosts_in(key, value, max_parts=MAX_PARTS, max_hosts=MAX_HOSTS):
    """`compose::hosts_scan_within`: [(tên, cổng)] theo thứ tự."""
    parts = [p for p in re.split(r"[_\-.]", key.upper()) if p]
    secret = any(p in SECRET_PARTS for p in parts)
    hostish = not secret and bool(parts) and parts[-1] in ENV_HOST_SUFFIXES
    if "${" in value:
        value = defaults(value)
    out, seen = [], set()

    def push(name, port):
        name = name.rstrip(".")
        ok = name and len(name) <= 253 and all(c in ALNUM or c in ".-_" for c in name) \
            and any(c.isascii() and c.isalpha() for c in name)
        if not ok or (name.lower(), port) in seen:
            return
        if len(out) >= max_hosts:
            return
        seen.add((name.lower(), port))
        out.append((name.lower(), port))

    def host_port(text, with_port):
        if ":" in text:
            h, p = text.rsplit(":", 1)
            if p and digits(p):
                push(h, p if with_port else None)
                return
        push(text, None)

    for part in [p for p in re.split(r"[, ;\t\n]", value) if p][:max_parts]:
        if "://" in part:
            after = part.split("://", 1)[1]
            end = min([i for i in (after.find("/"), after.find("?"), after.find("#")) if i >= 0], default=len(after))
            authority = after[:end]
            if "@" in authority:
                host_port(authority[authority.rfind("@") + 1:], True)
            elif "@" in after and (":" in authority or "." not in authority):
                rest = after[after.rfind("@") + 1:]
                cut = min([i for i in (rest.find("/"), rest.find("?"), rest.find("#")) if i >= 0], default=len(rest))
                host_port(rest[:cut], True)
                host_port(authority, False)
            else:
                host_port(authority, True)
        elif secret:
            continue
        elif ":" in part:
            h, p = part.rsplit(":", 1)
            if ":" not in h and "@" not in h and p and digits(p):
                push(h, p)
        elif hostish:
            push(part, None)
    return out


PLACEHOLDER = re.compile(r":(\x00|%[dsviu]|%\([^)]*\)[A-Za-z]|\{[A-Za-z0-9_]*\}|$|/(?!/)|/$)")


def normalize(text):
    """`hosts::normalize`: bỏ cổng ghép lúc chạy, tách URL nhiều máy, lấy máy
    trong `tcp(...)`. Trả (chuỗi, có bỏ cổng không)."""
    stripped = [None]

    def drop(m):
        # `"API:"`, `"http://db:" + port`: dấu `:` ở cuối không phải chỗ giữ chỗ.
        held = m.group(1)
        if held and not held.startswith("/"):
            if held.lower() in ("%d", "%i", "%u") or "port" in held.lower():
                stripped[0] = "port"
            elif stripped[0] is None:
                stripped[0] = "any"
        # `:/x` giữ lại `/`: chỉ dấu `:` và chỗ giữ chỗ bị bỏ.
        return "/" if m.group(1).startswith("/") else ""
    s = PLACEHOLDER.sub(drop, text)
    if "://" not in s:
        s = re.sub(r":([0-9]+)/\S*", r":\1", s)
    extra = "".join(" " + m for m in re.findall(r"tcp\(([^)]*)\)", s))
    if "://" in s:
        out = []
        for part in re.split(r"(?<=\s)", s):
            if "://" not in part:
                out.append(part)
                continue
            scheme, after = part.split("://", 1)
            end = min([i for i in (after.find("/"), after.find("?"), after.find("#")) if i >= 0], default=len(after))
            authority, path = after[:end], after[end:]
            at = authority.rfind("@")
            user, hosts = (authority[:at + 1], authority[at + 1:]) if at >= 0 else ("", authority)
            if "," not in hosts:
                out.append(part)
                continue
            for h in hosts.split(","):
                out.append(f"{scheme}://{user}{h}{path.rstrip()} ")
        s = "".join(out)
    return s + extra, stripped[0]


def literal_hosts(text, key):
    """Tên máy của một chuỗi, theo `hosts::literal_hosts`."""
    if not text:
        return []
    k = kind(key) if key else ""
    if k == "INBOUND":
        return []
    urlish = bool(key) and bool(words(key)) and words(key)[-1] in URLISH
    url = "://" in text
    libpq = not url and "=" in text and all("=" in p for p in text.split())
    pairs = libpq or (not url and "=" in text and ";" in text)
    if not pairs and ":" not in text and k != "HOST":
        return []
    trimmed = text.strip().rstrip("/")
    whole = not any(c in trimmed for c in ":/") and not any(c.isspace() for c in trimmed)
    if whole and k == "HOST":
        text = trimmed
    if urlish and k == "HOST" and whole and "." not in trimmed:
        k = ""
    text, stripped = normalize(text)
    single = bool(text) and re.fullmatch(r"[A-Za-z0-9._\-]+", text) is not None
    port_only = stripped is not None and not url and single and (k == "HOST" or stripped == "port")
    if port_only:
        k = "HOST"
    whole = whole or port_only
    found = []

    def add(key, value, strict):
        found.extend((h, strict) for h in hosts_in(key, value, 10**9, MAX_PER_FILE))
    if pairs:
        for part in re.split(r"[;\s]" if libpq else ";", text):
            if "=" in part:
                pk, v = part.split("=", 1)
                v = v.strip()
                if v[:4].lower() == "tcp:":
                    v = v[4:]
                v = v.split("\\")[0]
                pk = kind(pk.strip())
                if pk != "INBOUND":
                    add(pk, v, pk != "HOST")
            else:
                add("", part.strip(), True)
    else:
        if k == "HOST" and not whole:
            k = ""
        add(k, text, not url and k != "HOST")
    out = []
    for (name, port), strict in found:
        if port is not None:
            p = int(port)
            if not (1 <= p <= 65535) or (strict and p < 80):
                continue
        out.append((name, port))
    return out


def tally(literals):
    """[(dòng, chuỗi, khoá, là phần tử danh sách)] theo thứ tự trong file →
    Counter `(dòng, máy:cổng)`. Phần tử danh sách chỉ nhận khoá kiểu máy."""
    seen, out = set(), collections.Counter()
    for line, text, key, listy in literals:
        if listy and (key is None or kind(key) not in ("HOST", "INBOUND")):
            key = None
        for name, port in literal_hosts(text, key):
            if (line, name, port) in seen:
                continue
            if len(seen) >= MAX_PER_FILE:
                return out
            seen.add((line, name, port))
            out[(line, f"{name}:{port}" if port else name)] += 1
    return out


# ---- Python ----

def envish(s):
    return isinstance(s, str) and 0 < len(s) <= 128 and re.fullmatch(r"[A-Z0-9_]+", s) is not None \
        and re.search(r"[A-Z]", s) is not None


def _name(node):
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    return None


def _last_env(node):
    """`os.getenv("X")`, `environ["X"]`: tên biến môi trường ở cuối, như archctl
    đọc `) or`, `] ||`."""
    if isinstance(node, ast.Call):
        items = sorted([*node.args, *(k.value for k in node.keywords)], key=lambda n: (n.lineno, n.col_offset))
        last = items[-1] if items else None
    elif isinstance(node, ast.Subscript):
        last = node.slice
    else:
        name = _name(node) if isinstance(node, (ast.Name, ast.Attribute)) else None
        return name if envish(name) else None
    if isinstance(last, ast.Constant) and envish(last.value):
        return last.value
    return None


def string_starts(source):
    """Từng mảnh chuỗi theo `tokenize`: `(đầu, cuối, nguyên văn)`, vị trí dạng
    `(dòng, cột ký tự)`. `ast` gộp `'a' f'b'` thành một chuỗi; archctl, như mọi
    lexer, thấy hai. Từ Python 3.12, f-string là một dãy token
    `FSTRING_START … FSTRING_END`."""
    fs_start, fs_end = getattr(tokenize, "FSTRING_START", -1), getattr(tokenize, "FSTRING_END", -1)
    try:
        text = source.decode("utf-8")
    except UnicodeDecodeError:
        return None
    rows = text.split("\n")

    def cut(a, b):
        if a[0] == b[0]:
            return rows[a[0] - 1][a[1]:b[1]]
        return "\n".join([rows[a[0] - 1][a[1]:], *rows[a[0]:b[0] - 1], rows[b[0] - 1][:b[1]]])

    pieces, open_at, depth = [], None, 0
    try:
        for tok in tokenize.tokenize(io.BytesIO(source).readline):
            if tok.type == tokenize.STRING and depth == 0:
                pieces.append((tok.start, tok.end, tok.string))
            elif tok.type == fs_start:
                if depth == 0:
                    open_at = tok.start
                depth += 1
            elif tok.type == fs_end:
                depth -= 1
                if depth == 0:
                    pieces.append((open_at, tok.end, cut(open_at, tok.end)))
    except (tokenize.TokenError, SyntaxError, IndentationError):
        return None
    return pieces


def piece_text(raw):
    """Chữ của một mảnh chuỗi như archctl giữ: f-string bỏ phần `{...}`."""
    try:
        node = ast.parse(raw.strip(), mode="eval").body
    except (SyntaxError, ValueError):
        return None
    if isinstance(node, ast.Constant):
        return node.value.decode("latin-1") if isinstance(node.value, bytes) else node.value
    if isinstance(node, ast.JoinedStr):
        return joined(node)
    return None


def joined(node):
    """Chữ của f-string như archctl giữ: mỗi `{...}` thành một `\\0`."""
    return "".join(v.value if isinstance(v, ast.Constant) else "\x00" for v in node.values)


def python_literals(source):
    """[(dòng, chuỗi, khoá)] theo thứ tự; None nếu không parse được."""
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        try:
            tree = ast.parse(source)
        except (SyntaxError, ValueError):
            return None
        pieces = string_starts(source)
    lines = source.split(b"\n")

    def char_pos(lineno, byte_col):
        row = lines[lineno - 1] if lineno - 1 < len(lines) else b""
        return (lineno, len(row[:byte_col].decode("utf-8", errors="replace")))
    keys = {}

    def set_key(node, key, listy=False):
        if node is not None and key is not None:
            keys.setdefault(id(node), (key, listy))
            # `host="a" if x else "b"`: chuỗi đứng ngay sau `=` vẫn là giá trị
            # của khoá, như archctl đọc; vế `else` thì không.
            if isinstance(node, ast.IfExp):
                set_key(node.body, key)
            # Danh sách toàn chuỗi ở khoá: `HOSTS = ["a", "db"]`.
            if isinstance(node, (ast.List, ast.Tuple)) and node.elts and all(
                    isinstance(e, ast.Constant) and isinstance(e.value, str) for e in node.elts):
                for e in node.elts:
                    set_key(e, key, True)

    def callee(call):
        return _name(call.func) if isinstance(call, ast.Call) else None

    def set_value(node, key):
        """Giá trị của một phép gán, tham số hay đối số có tên: archctl lần ngược
        tới dấu `=` qua `or`, `else` và lời gọi."""
        if node is None or key is None:
            return
        set_key(node, key)
        # Chuỗi đứng ngay sau `=` nhận khoá dù sau nó là gì: `"x/" + y`,
        # `"…".format(p)`, `"db" or x`.
        if isinstance(node, ast.BinOp):
            set_value(node.left, key)
        elif isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) \
                and isinstance(node.func.value, (ast.Constant, ast.JoinedStr)):
            set_key(node.func.value, key)
        if isinstance(node, ast.IfExp):
            set_value(node.orelse, key)
        elif isinstance(node, ast.BoolOp) and isinstance(node.op, ast.Or):
            set_value(node.values[0], key)
            for a, b in zip(node.values, node.values[1:]):
                env = _last_env(a)
                if env:
                    set_key(b, env)
                else:
                    set_value(b, key)
        elif isinstance(node, ast.Call) and callee(node) not in HOST_FUNCS:
            getter = callee(node) in GETTERS and node.args and isinstance(node.args[0], ast.Constant) \
                and keyish(node.args[0].value)
            if not getter:
                for kw in node.keywords:
                    if kw.arg in ("default", "fallback", "defaultValue", "default_value"):
                        set_key(kw.value, key)

    skip = set()  # chuỗi con của f-string, và phần định dạng `{x:>10}`
    for node in ast.walk(tree):
        if isinstance(node, ast.JoinedStr):
            for v in node.values:
                skip.add(id(v))
                # archctl không tách chuỗi trong mã của `{...}`.
                if isinstance(v, ast.FormattedValue):
                    for sub in ast.walk(v.value):
                        skip.add(id(sub))
                if isinstance(v, ast.FormattedValue) and v.format_spec is not None:
                    for sub in ast.walk(v.format_spec):
                        skip.add(id(sub))
        if isinstance(node, ast.keyword):
            set_value(node.value, node.arg)
        elif isinstance(node, ast.Assign):
            set_value(node.value, _name(node.targets[-1]) if isinstance(node.targets[-1], (ast.Name, ast.Attribute)) else None)
        elif isinstance(node, ast.AnnAssign):
            set_value(node.value, _name(node.target) if isinstance(node.target, (ast.Name, ast.Attribute)) else None)
        elif isinstance(node, ast.Dict):
            # Khoá là chuỗi: `{host: "db"}` của Python thì `host` là một biến.
            for k, v in zip(node.keys, node.values):
                if isinstance(k, ast.Constant) and isinstance(k.value, str):
                    set_key(v, k.value)
        elif isinstance(node, (ast.Call, ast.List, ast.Tuple, ast.Set)):
            # `net.JoinHostPort("db", …)`: đối số đầu là một máy.
            if isinstance(node, ast.Call) and callee(node) in HOST_FUNCS and node.args:
                set_key(node.args[0], "host")
            # `cfg.get("host", "db")`, `getenv("DB_HOST", default="db")`.
            if isinstance(node, ast.Call) and _name(node.func) in GETTERS and node.args \
                    and isinstance(node.args[0], ast.Constant) and keyish(node.args[0].value):
                if len(node.args) > 1:
                    set_key(node.args[1], node.args[0].value)
                for kw in node.keywords:
                    if kw.arg in ("default", "fallback", "defaultValue", "default_value"):
                        set_key(kw.value, node.args[0].value)
            # `getenv("DB_HOST", "db")`: chỉ đối số đầu của lời gọi.
            if isinstance(node, ast.Call) and len(node.args) > 1 and isinstance(node.args[0], ast.Constant) \
                    and envish(node.args[0].value):
                set_key(node.args[1], node.args[0].value)
        elif isinstance(node, ast.BoolOp) and isinstance(node.op, ast.Or):
            for a, b in zip(node.values, node.values[1:]):
                set_key(b, _last_env(a))
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
            a = node.args
            pos = [*a.posonlyargs, *a.args]
            for arg, d in zip(pos[len(pos) - len(a.defaults):], a.defaults):
                set_value(d, arg.arg)
            for arg, d in zip(a.kwonlyargs, a.kw_defaults):
                set_value(d, arg.arg)

    def triple(node):
        row = lines[node.lineno - 1] if node.lineno - 1 < len(lines) else b""
        rest = row[node.col_offset:].lstrip(b"rRbBuUfFtT")
        return rest.startswith((b'"""', b"'''"))

    def value_pos(lineno, byte_col):
        """Thứ đứng trước chuỗi (bỏ khoảng trắng) là `(`, `,` hay `=`: chuỗi ba
        nháy ở đó là một giá trị, không phải docstring."""
        ln, col = lineno - 1, byte_col
        while ln >= 0:
            row = lines[ln][:col] if col is not None else lines[ln]
            if row.lstrip().startswith(b"#"):
                ln, col = ln - 1, None
                continue
            stripped = row.rstrip()
            if stripped:
                return stripped[-1:] in (b"(", b",", b"=")
            ln, col = ln - 1, None
        return False

    def before_char(lineno, byte_col):
        ln, col = lineno - 1, byte_col
        while ln >= 0:
            row = lines[ln][:col] if col is not None else lines[ln]
            if row.lstrip().startswith(b"#"):
                ln, col = ln - 1, None
                continue
            stripped = row.rstrip()
            if stripped:
                return stripped[-1:]
            ln, col = ln - 1, None
        return b""

    # Tên đứng ngay trước `=` của một giá trị: đích của phép gán, kiểu của phép
    # gán có kiểu, tên đối số.
    assigned = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.Assign) and isinstance(node.targets[-1], (ast.Name, ast.Attribute)):
            assigned[id(node.value)] = _name(node.targets[-1])
        elif isinstance(node, ast.AnnAssign) and isinstance(node.annotation, (ast.Name, ast.Attribute)):
            assigned[id(node.value)] = _name(node.annotation)
        elif isinstance(node, ast.keyword) and node.arg:
            assigned[id(node.value)] = node.arg

    # `("db", 5432)` trong ngoặc, là đối số hay giá trị của một tên kiểu máy.
    address = {}
    for node in ast.walk(tree):
        if isinstance(node, ast.Tuple) and len(node.elts) == 2:
            h, p = node.elts
            row = lines[node.lineno - 1] if node.lineno - 1 < len(lines) else b""
            if isinstance(h, ast.Constant) and isinstance(h.value, str) and isinstance(p, ast.Constant) \
                    and type(p.value) is int and p.end_lineno == p.lineno \
                    and re.fullmatch(rb"[0-9]{1,5}", lines[p.lineno - 1][p.col_offset:p.end_col_offset]) \
                    and row[node.col_offset:node.col_offset + 1] == b"(" \
                    and (before_char(node.lineno, node.col_offset) == b"("
                         or (before_char(node.lineno, node.col_offset) == b"="
                             and kind(assigned.get(id(node)) or "") == "HOST")):
                address[id(h)] = lines[p.lineno - 1][p.col_offset:p.end_col_offset].decode()

    out = []
    for node in ast.walk(tree):
        if id(node) in skip:
            continue
        if isinstance(node, ast.Constant) and isinstance(node.value, (str, bytes)):
            text = node.value if isinstance(node.value, str) else node.value.decode("latin-1")
        elif isinstance(node, ast.JoinedStr):
            text = joined(node)
        else:
            continue
        key, listy = keys.get(id(node), (None, False))
        if id(node) in address:
            out.append((node.lineno, node.col_offset, f"{text}:{address[id(node)]}", None, False))
            continue
        if triple(node):
            if value_pos(node.lineno, node.col_offset):
                out.append((node.lineno, node.col_offset, text, None, False))
            continue
        # Chuỗi ghép ngầm: mỗi mảnh một chuỗi, khoá chỉ thuộc mảnh đầu.
        if pieces is not None and node.end_lineno is not None:
            lo, hi = char_pos(node.lineno, node.col_offset), char_pos(node.end_lineno, node.end_col_offset)
            inside = [p for p in pieces if lo <= p[0] and p[1] <= hi]
            # archctl ghép chuỗi liền nhau như trình dịch Python; chuỗi ba nháy
            # thì là token riêng.
            is_triple = [p[2].lstrip("rRbBuUfFtT").startswith(('"""', "\'\'\'")) for p in inside]
            if len(inside) > 1 and not any(is_triple):
                out.append((node.lineno, node.col_offset, text, key, listy))
                continue
            if len(inside) > 1:
                for n, (start, _, raw) in enumerate(inside):
                    part = piece_text(raw)
                    if part is None:
                        continue
                    if raw.lstrip("rRbBuUfFtT").startswith(('"""', "\'\'\'")):
                        if n == 0 and value_pos(node.lineno, node.col_offset):
                            out.append((start[0], start[1], part, None, False))
                        continue
                    out.append((start[0], start[1], part, key if n == 0 else None, listy))
                continue
        out.append((node.lineno, node.col_offset, text, key, listy))
    out.sort(key=lambda r: (r[0], r[1]))
    return [(line, text, key, listy) for line, _, text, key, listy in out]


def python_truth(path):
    lits = python_literals(path.read_bytes())
    return None if lits is None else tally(lits)


# ---- Go ----

GO_SOURCE = r"""
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"go/ast"
	"go/parser"
	"go/token"
	"os"
	"regexp"
	"sort"
	"strconv"
	"strings"
)

var envish = regexp.MustCompile(`^[A-Z0-9_]*[A-Z][A-Z0-9_]*$`)

func name(e ast.Expr) *string {
	switch v := e.(type) {
	case *ast.Ident:
		return &v.Name
	case *ast.SelectorExpr:
		return &v.Sel.Name
	case *ast.BasicLit:
		if v.Kind == token.STRING {
			if s, err := strconv.Unquote(v.Value); err == nil {
				return &s
			}
		}
	}
	return nil
}

func envName(e ast.Expr) *string {
	if l, ok := e.(*ast.BasicLit); ok && l.Kind == token.STRING {
		if s, err := strconv.Unquote(l.Value); err == nil && len(s) <= 128 && envish.MatchString(s) {
			return &s
		}
	}
	return nil
}

var keyish = regexp.MustCompile(`^[A-Za-z0-9_.\-]{1,128}$`)

var hostFuncs = map[string]bool{"JoinHostPort": true, "gethostbyname": true, "gethostbyname_ex": true,
	"getaddrinfo": true, "LookupHost": true}

var getters = map[string]bool{"get": true, "getenv": true, "get_env": true, "getEnv": true, "Getenv": true,
	"GetEnv": true, "env": true, "getOrDefault": true, "getProperty": true, "GetString": true, "getString": true,
	"GetValue": true, "setdefault": true, "LookupEnv": true}

type row struct {
	Line, Col int
	Text      string
	Key       *string
	List      bool
}

func main() {
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<20)
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	for in.Scan() {
		root, file, _ := strings.Cut(in.Text(), "\t")
		fset := token.NewFileSet()
		f, err := parser.ParseFile(fset, root+"/"+file, nil, parser.SkipObjectResolution)
		if err != nil {
			fmt.Fprintf(out, "%s\tERR\n", file)
			continue
		}
		keys := map[ast.Expr]*string{}
		listy := map[ast.Expr]bool{}
		var set func(e ast.Expr, k *string)
		setValue := func(e ast.Expr, k *string) { set(e, k) }
		set = func(e ast.Expr, k *string) {
			if k == nil {
				return
			}
			if _, ok := keys[e]; !ok {
				keys[e] = k
			}
			// Danh sách toàn chuỗi ở khoá: `hosts := []string{"a", "db"}`.
			if c, ok := e.(*ast.CompositeLit); ok && len(c.Elts) > 0 {
				for _, el := range c.Elts {
					if l, ok := el.(*ast.BasicLit); !ok || l.Kind != token.STRING {
						return
					}
				}
				for _, el := range c.Elts {
					if _, ok := keys[el]; !ok {
						keys[el] = k
						listy[el] = true
					}
				}
			}
		}
		imports := map[ast.Expr]bool{}
		for _, imp := range f.Imports {
			imports[imp.Path] = true
		}
		ast.Inspect(f, func(n ast.Node) bool {
			switch v := n.(type) {
			case *ast.KeyValueExpr:
				set(v.Value, name(v.Key))
			case *ast.AssignStmt:
				if (v.Tok == token.ASSIGN || v.Tok == token.DEFINE) && len(v.Lhs) == 1 && len(v.Rhs) == 1 {
					setValue(v.Rhs[0], name(v.Lhs[0]))
				}
			case *ast.ValueSpec:
				if len(v.Names) == 1 && len(v.Values) == 1 {
					setValue(v.Values[0], &v.Names[0].Name)
				}
			case *ast.CallExpr:
				if fn := name(v.Fun); fn != nil && hostFuncs[*fn] && len(v.Args) > 0 {
					h := "host"
					set(v.Args[0], &h)
				}
				// `cfg.Get("host", "db")`, `getenv("db_host", "db")`.
				if fn := name(v.Fun); fn != nil && getters[*fn] && len(v.Args) > 1 {
					if k := name(v.Args[0]); k != nil && keyish.MatchString(*k) {
						if _, ok := v.Args[0].(*ast.BasicLit); ok {
							set(v.Args[1], k)
						}
					}
				}
				// `getEnv("REDIS_ADDR", "redis:6379")`: chỉ đối số đầu.
				if len(v.Args) > 1 {
					set(v.Args[1], envName(v.Args[0]))
				}
			}
			return true
		})
		rows := []row{}
		ast.Inspect(f, func(n ast.Node) bool {
			l, ok := n.(*ast.BasicLit)
			if !ok || l.Kind != token.STRING || imports[l] {
				return true
			}
			s, err := strconv.Unquote(l.Value)
			if err != nil {
				return true
			}
			p := fset.Position(l.Pos())
			rows = append(rows, row{p.Line, p.Column, s, keys[l], listy[l]})
			return true
		})
		sort.SliceStable(rows, func(i, j int) bool {
			return rows[i].Line < rows[j].Line || (rows[i].Line == rows[j].Line && rows[i].Col < rows[j].Col)
		})
		list := [][4]any{}
		for _, r := range rows {
			list = append(list, [4]any{r.Line, r.Text, r.Key, r.List})
		}
		js, _ := json.Marshal(list)
		fmt.Fprintf(out, "%s\t%s\n", file, js)
	}
}
"""


def build_go_helper(workdir, die):
    from pathlib import Path
    import os
    src = Path(workdir) / "hosts.go"
    src.write_text(GO_SOURCE)
    exe = Path(workdir) / "hosts"
    p = subprocess.run(["go", "build", "-o", str(exe), str(src)], capture_output=True, text=True,
                       timeout=600, env={**os.environ, "GO111MODULE": "off", "GOFLAGS": ""})
    if p.returncode != 0:
        die(f"không dựng được trình đọc chuỗi Go: {p.stderr.strip()[-300:]}")
    return str(exe)


def go_truth(helper, root, files, die):
    """file → Counter `(dòng, máy:cổng)`; None nếu go/parser không đọc được."""
    feed = "".join(f"{root}\t{f}\n" for f in files)
    p = subprocess.run([helper], input=feed, capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        die(f"trình đọc chuỗi Go thoát {p.returncode} ở {root}: {p.stderr.strip()[-300:]}")
    out = {}
    for row in p.stdout.splitlines():
        file, _, value = row.partition("\t")
        out[file] = None if value == "ERR" else tally([(l, t, k, li) for l, t, k, li in json.loads(value)])
    return out
