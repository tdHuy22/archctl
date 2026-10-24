"""Thước đo gRPC Java (`scan --json`, mục `rpc` của file `.java`).

Cây cú pháp lấy từ tree-sitter với ngữ pháp tree-sitter-java bản ghim
(`java-requirements.txt`), không từ lexer của archctl. Luật nhận chỗ gọi stub,
lớp cài đặt, chỗ đăng ký viết lại ở đây theo đúng đặc tả trong
`crates/arch-core/src/scan/grpc.rs` (`fn java`); chỗ hai bên lệch là chỗ lexer
hay bộ dò của archctl đọc sai (chuỗi, text block, chú thích, tên viết tách dòng).

Mỗi dòng vào `gốc\\tfile`; mỗi dòng ra `file\\t[[dòng, khoá], ...]` hay `file\\tERR
…`. Khoá: `"call AdService javaPackage:hipstershop"`. Chạy trong venv có
tree-sitter (`oracle.py --require java`).
"""
import collections
import json
import re
import sys

STUBS = ("newStub", "newBlockingStub", "newFutureStub", "newBlockingV2Stub")
INFRA = ("Health", "ServerReflection", "Channelz")


def text(node, src):
    return src[node.start_byte:node.end_byte].decode("utf-8", "replace")


def dotted(node, src):
    """`a.b.C` của identifier, scoped_identifier, field_access chỉ gồm tên;
    None nếu có thứ khác (lời gọi, chỉ số)."""
    if node.type in ("identifier", "type_identifier"):
        return text(node, src)
    if node.type in ("scoped_identifier", "scoped_type_identifier", "field_access"):
        parts = []
        for c in node.named_children:
            d = dotted(c, src)
            if d is None:
                return None
            parts.append(d)
        return ".".join(parts) if parts else None
    return None


def walk(node):
    stack = [node]
    while stack:
        n = stack.pop()
        yield n
        stack.extend(reversed(n.children))


WORD = re.compile(r"[A-Za-z_$][A-Za-z0-9_$]*")
MAX_IMPORTS = 1 << 16
MAX_REFS = 1 << 16
MAX_LOWER = 8


def lower(s):
    return s[:1].isascii() and (s[:1].islower() or s[:1] == "_")


def upper(s):
    return s[:1] == "$" or (s[:1].isascii() and s[:1].isupper())


def unicode_escapes(src):
    """Bước dịch đầu của javac (JLS 3.3): `\\uXXXX` sau số lẻ dấu `\\`, kể cả
    xuống dòng. Tree-sitter không làm bước này, nên làm ở đây như archctl."""
    text = src.decode("utf-8", "replace")
    if "\\u" not in text:
        return src
    out, i, slashes = [], 0, 0
    while i < len(text):
        c = text[i]
        if c == "\\":
            slashes += 1
            out.append(c)
            i += 1
            continue
        if c == "u" and slashes % 2 == 1:
            j = i
            while j < len(text) and text[j] == "u":
                j += 1
            h = text[j:j + 4]
            if len(h) == 4 and all(x in "0123456789abcdefABCDEF" for x in h):
                ch = int(h, 16)
                if not 0xD800 <= ch <= 0xDFFF:
                    out.pop()
                    out.append(chr(ch))
                    i = j + 4
                    slashes = 0
                    continue
        slashes = 0
        out.append(c)
        i += 1
    return "".join(out).encode("utf-8", "surrogatepass")


def import_facts(root, src):
    """Import và tham chiếu không qua import, theo đặc tả `scan/java.rs`.

    Dãy lá của cây bỏ chú thích; chuỗi, ký tự, số là một lá "khác"; câu
    `package`, `import` là một lá "khác". Tên là chuỗi `từ (. từ)*` dài nhất
    không đứng sau dấu chấm."""
    leaves = []
    package = None
    imports = []
    single = set()
    wildcards = []
    stack = [root]
    while stack:
        n = stack.pop()
        t = n.type
        if t in ("line_comment", "block_comment"):
            continue
        if t in ("package_declaration", "import_declaration"):
            dotted_node = next((c for c in n.named_children if c.type in ("identifier", "scoped_identifier")), None)
            spec = text(dotted_node, src).replace(" ", "") if dotted_node is not None else ""
            spec = re.sub(r"\s+|/\*.*?\*/|//[^\n]*", "", spec, flags=re.S)
            if t == "package_declaration":
                if package is None:
                    package = spec
            else:
                is_static = any(c.type == "static" for c in n.children)
                wild = any(c.type == "asterisk" for c in n.children)
                full = spec + (".*" if wild else "")
                if len(imports) < MAX_IMPORTS:
                    if wild:
                        if len(wildcards) < 256:
                            wildcards.append(spec)
                    elif not is_static and "." in spec:
                        single.add(spec.rsplit(".", 1)[1])
                    imports.append((n.start_point[0] + 1, f"import {full}"))
            leaves.append(("other", "", n.start_point[0] + 1))
            continue
        if t in ("string_literal", "character_literal") or (n.child_count == 0):
            raw = text(n, src)
            if n.child_count == 0 and raw == ".":
                kind = "dot"
            elif n.child_count == 0 and WORD.fullmatch(raw):
                kind = "word"
            elif n.child_count == 0 and raw == "@interface":
                leaves.append(("other", "", n.start_point[0] + 1))
                raw, kind = "interface", "word"
            else:
                kind = "other"
            leaves.append((kind, raw, n.start_point[0] + 1))
            continue
        stack.extend(reversed(n.children))

    # Phạm vi che của mỗi tên kiểu tự khai: cả file ở mức ngoài cùng, không thì
    # khối `{…}` bao quanh chỗ khai.
    close_of, stack, enclosing = {}, [], []
    for k, leaf in enumerate(leaves):
        enclosing.append(stack[-1] if stack else None)
        if leaf[0] == "other" and leaf[1] == "{":
            stack.append(k)
        elif leaf[0] == "other" and leaf[1] == "}" and stack:
            close_of[stack.pop()] = k
    def type_body(o):
        for leaf in reversed(leaves[max(0, o - 512):o]):
            if leaf[0] == "other" and leaf[1] in (";", "{", "}"):
                return False
            if leaf[0] == "word" and leaf[1] in ("class", "interface", "enum", "record", "new"):
                return True
        return False
    shadows = collections.defaultdict(list)
    for k in range(len(leaves) - 1):
        a, b = leaves[k], leaves[k + 1]
        if a[0] == "word" and b[0] == "word" and a[1] in ("class", "interface", "enum", "record"):
            o = enclosing[k]
            if o is None:
                shadows[b[1]].append((0, len(leaves)))
            else:
                # Kiểu cục bộ che từ chỗ khai (JLS 6.3), kiểu thành viên cả thân.
                shadows[b[1]].append((o if type_body(o) else k, close_of.get(o, len(leaves))))

    def shadowed(name, at):
        return any(x <= at <= y for x, y in shadows.get(name, ()))
    refs, seen = [], set()
    i = 0
    while i < len(leaves):
        at = i
        i += 1
        if len(refs) >= MAX_REFS or leaves[at][0] != "word":
            continue
        if at > 0 and leaves[at - 1][0] == "dot":
            continue
        segs = [leaves[at][1]]
        while i + 1 < len(leaves) and leaves[i][0] == "dot" and leaves[i + 1][0] == "word":
            segs.append(leaves[i + 1][1])
            i += 2
        pkg = 0
        while pkg < len(segs) and lower(segs[pkg]):
            pkg += 1
        if pkg:
            if segs[0] in ("this", "super") or len(segs) < 2:
                continue
            cls = 0
            while pkg + cls < len(segs) and upper(segs[pkg + cls]):
                cls += 1
            if cls:
                specs = [".".join(segs[:pkg + cls])]
            else:
                specs = [".".join(segs[:k]) for k in range(min(len(segs), MAX_LOWER), 1, -1)]
        else:
            name = segs[0]
            if not upper(name) or name in single or shadowed(name, at):
                continue
            specs = [f"{p}.{name}" for p in ([package] if package else []) + wildcards]
            if not specs:
                continue
        key = "|".join(specs)
        if key not in seen:
            seen.add(key)
            refs.append((leaves[at][2], f"ref {key}"))
    return imports + refs


HOST_FUNCS = {"JoinHostPort", "gethostbyname", "gethostbyname_ex", "getaddrinfo", "LookupHost"}
GETTERS = {"get", "getenv", "get_env", "getEnv", "Getenv", "GetEnv", "env", "getOrDefault", "getProperty",
           "GetString", "getString", "GetValue", "setdefault", "LookupEnv"}
JAVA_HOST_CTORS = {"Socket", "InetSocketAddress", "createUnresolved", "Jedis", "JedisPool", "JedisPooled",
                   "HostAndPort", "fromParts", "ServerAddress"}
JAVA_SETTERS = {"put", "putIfAbsent", "setProperty", "set", "property", "withProperty"}
SPRING = re.compile(r"\$\{([A-Za-z0-9_.\-]{1,128}):([^{}$]*)\}")
ENVISH = re.compile(r"[A-Z0-9_]*[A-Z][A-Z0-9_]*")
KEYISH = re.compile(r"[A-Za-z0-9_.\-]{1,128}")
ESCAPES = {"n": "\n", "t": "\t", "b": "\b", "r": "\r", "f": "\f", "s": " ", "0": "\0",
           '"': '"', "'": "'", "\\": "\\"}


def unquote(raw):
    """Giá trị của một `string_literal`: chuỗi thường thì giải thoát; text block
    giữ nguyên phần giữa hai dấu `\"\"\"` như lexer của archctl (`Tok::Triple`)."""
    if raw.startswith('"""'):
        return raw[3:-3], True
    body, out, i = raw[1:-1], [], 0
    while i < len(body):
        c = body[i]
        if c == "\\" and i + 1 < len(body):
            n = body[i + 1]
            if n == "u":
                j = i + 1
                while j < len(body) and body[j] == "u":
                    j += 1
                try:
                    out.append(chr(int(body[j:j + 4], 16)))
                    i = j + 4
                    continue
                except ValueError:
                    pass
            if n in "01234567":
                j = i + 1
                while j < len(body) and j < i + 4 and body[j] in "01234567":
                    j += 1
                out.append(chr(int(body[i + 1:j], 8)))
                i = j
                continue
            out.append(ESCAPES.get(n, "\\" + n))
            i += 2
            continue
        out.append(c)
        i += 1
    return "".join(out), False


def literals(root, src):
    """Chuỗi của file kèm khoá theo đặc tả `hosts::context` cho Java:
    `[(dòng, chuỗi, khoá, False)]`. Khoá là tên được khai hay được gán
    (`String host = "db"`, `this.host = "db"`, `hosts[0] = "db"`), đối số sau
    tên biến môi trường hay khoá cấu hình (`getOrDefault("REDIS_ADDR", "redis")`),
    đối số đầu của hàm tên máy, nhánh sau của `? :` (khoá là nhánh trước), giá
    trị của phần tử annotation (`@A(host = "db")`). Text block chỉ khi đứng ở chỗ
    một giá trị."""
    keys = {}

    def last(node):
        if node is None:
            return None
        t = node.type
        if t == "identifier":
            return text(node, src)
        if t == "field_access":
            return text(node.child_by_field_name("field"), src)
        if t == "array_access":
            return last(node.child_by_field_name("array"))
        if t == "string_literal":
            v, block = unquote(text(node, src))
            return None if block else v
        return None

    def first_string(node):
        while node is not None:
            if node.type == "string_literal":
                return node
            # `("db")`: ngoặc chỉ chứa một chuỗi, như `grouped` của archctl.
            if node.type == "parenthesized_expression" and node.named_child_count == 1 \
                    and node.named_children[0].type == "string_literal":
                node = node.named_children[0]
            elif node.type == "binary_expression" and text(node.child_by_field_name("operator"), src) == "+":
                node = node.child_by_field_name("left")
            else:
                return None
        return None

    def put(value, key):
        lit = first_string(value)
        if lit is not None and key is not None and lit.start_byte not in keys:
            keys[lit.start_byte] = key

    for n in walk(root):
        t = n.type
        if t == "variable_declarator":
            put(n.child_by_field_name("value"), last(n.child_by_field_name("name")))
        elif t == "assignment_expression" and text(n.child_by_field_name("operator"), src) == "=":
            put(n.child_by_field_name("right"), last(n.child_by_field_name("left")))
        elif t == "element_value_pair":
            put(n.child_by_field_name("value"), last(n.child_by_field_name("key")))
        elif t == "enhanced_for_statement":
            # `for (String h : "a b".split(" "))`: chuỗi đứng sau `:`.
            put(n.child_by_field_name("value"), last(n.child_by_field_name("name")))
        elif t == "assert_statement" and n.named_child_count == 2:
            put(n.named_children[1], last(n.named_children[0]))
        elif t == "ternary_expression":
            put(n.child_by_field_name("alternative"), last(n.child_by_field_name("consequence")))
        elif t in ("method_invocation", "object_creation_expression"):
            name = n.child_by_field_name("name") if t == "method_invocation" else n.child_by_field_name("type")
            fn = text(name, src).rsplit(".", 1)[-1] if name is not None else ""
            args_node = n.child_by_field_name("arguments")
            args = list(args_node.named_children) if args_node is not None else []
            if fn in HOST_FUNCS and args:
                put(args[0], "host")
            # Java: hàm dựng địa chỉ, và builder, setter có tên là khoá máy;
            # khoá máy hay không do `code_hosts.kind` quyết (`?fn:` ở oracle).
            if fn in JAVA_HOST_CTORS and args:
                put(args[0], "host")
            elif args:
                put(args[0], f"?fn:{fn}")
            if len(args) > 1 and args[0].type == "string_literal":
                k, block = unquote(text(args[0], src))
                if not block and ENVISH.fullmatch(k) and len(k) <= 128:
                    put(args[1], k)
                elif not block and (fn in GETTERS or fn in JAVA_SETTERS) and KEYISH.fullmatch(k):
                    put(args[1], k)
    rows = []
    for n in walk(root):
        if n.type != "string_literal":
            continue
        value, block = unquote(text(n, src))
        if block:
            prev = n.prev_sibling
            if prev is None or text(prev, src) not in ("(", ",", "="):
                continue
        key = keys.get(n.start_byte)
        m = SPRING.fullmatch(value) if not block else None
        if m:
            value, key = m.group(2), m.group(1)
        rows.append((n.start_point[0] + 1, n.start_point[1], value, key))
    rows.sort()
    return [(line, value, key, False) for line, _, value, key in rows if value]


def facts(src, parser):
    tree = parser.parse(src)
    file_pkg = None
    imported = {}
    static = {}
    wild = []
    nodes = list(walk(tree.root_node))
    for n in nodes:
        if n.type == "package_declaration" and file_pkg is None:
            for c in n.named_children:
                d = dotted(c, src)
                if d:
                    file_pkg = d
                    break
        elif n.type == "import_declaration":
            is_static = any(c.type == "static" for c in n.children)
            path = next((dotted(c, src) for c in n.named_children if dotted(c, src)), None)
            if not path:
                continue
            segs = path.split(".")
            if any(c.type == "asterisk" for c in n.children):
                # `import static a.b.XGrpc.*;`
                if is_static and segs[-1].endswith("Grpc"):
                    wild.append((segs[-1], ".".join(segs[:-1])))
                continue
            if is_static and len(segs) >= 2 and segs[-2].endswith("Grpc"):
                static[segs[-1]] = (segs[-2], ".".join(segs[:-2]))
            elif not is_static and segs[-1].endswith("Grpc"):
                imported[segs[-1]] = ".".join(segs[:-1])

    def service_of(path):
        if not path:
            return None
        *pkg, cls = path
        if not cls.endswith("Grpc"):
            return None
        service = cls[:-4]
        if not service[:1].isupper() or service in INFRA or any(p in ("health", "reflection", "channelz") for p in pkg):
            return None
        p = ".".join(pkg) if pkg else (imported.get(cls) or file_pkg)
        origin = f"javaPackage:{p}" if p else "unknown:"
        return service, origin

    out = []
    implemented = set()
    register = None
    for n in nodes:
        if n.type == "method_invocation":
            name = n.child_by_field_name("name")
            obj = n.child_by_field_name("object")
            if name is None:
                continue
            m = text(name, src)
            line = name.start_point[0] + 1
            if m in STUBS:
                if obj is None:
                    found = None
                    if m in static:
                        cls, pkg = static[m]
                        found = service_of((pkg.split(".") if pkg else []) + [cls])
                    elif len(wild) == 1:
                        cls, pkg = wild[0]
                        found = service_of((pkg.split(".") if pkg else []) + [cls])
                else:
                    d = dotted(obj, src)
                    found = service_of(d.split(".")) if d else None
                if found:
                    out.append((line, f"call {found[0]} {found[1]}"))
            elif m == "addService" and obj is not None:
                # Lần đầu theo vị trí trong file, không theo thứ tự duyệt cây:
                # `b.addService(x).addService(y)` lồng lời gọi sau ra ngoài.
                if register is None or name.start_byte < register[0]:
                    register = (name.start_byte, line)
        elif n.type == "object_creation_expression":
            # Lớp vô danh `new XGrpc.XImplBase() { … }`.
            t = n.child_by_field_name("type")
            d = dotted(t, src) if t is not None else None
            if d:
                segs = d.split(".")
                found = service_of(segs[:-1])
                if found and (segs[-1] == "AsyncService" or segs[-1] == found[0] + "ImplBase"):
                    out.append((t.start_point[0] + 1, f"impl {found[0]} {found[1]}"))
                    implemented.add(found)
        elif n.type in ("superclass", "super_interfaces"):
            types = []
            for c in n.named_children:
                if c.type == "type_list":
                    types.extend(c.named_children)
                else:
                    types.append(c)
            for t in types:
                d = dotted(t, src)
                if not d:
                    continue
                segs = d.split(".")
                found = service_of(segs[:-1])
                if found and (segs[-1] == "AsyncService" or segs[-1] == found[0] + "ImplBase"):
                    out.append((t.start_point[0] + 1, f"impl {found[0]} {found[1]}"))
                    implemented.add(found)
    if register is not None:
        for service, origin in sorted(implemented):
            out.append((register[1], f"serve {service} {origin}"))
    return out + import_facts(tree.root_node, src)


def main():
    import tree_sitter
    import tree_sitter_java

    parser = tree_sitter.Parser(tree_sitter.Language(tree_sitter_java.language()))
    for raw in sys.stdin:
        root, file = raw.rstrip("\n").split("\t")
        try:
            with open(f"{root}/{file}", "rb") as f:
                src = unicode_escapes(f.read())
            tree = parser.parse(src)
            out = {"facts": sorted(facts(src, parser)), "literals": literals(tree.root_node, src)}
            print(f"{file}\t{json.dumps(out, ensure_ascii=False)}", flush=True)
        except (OSError, ValueError, RecursionError) as e:
            print(f"{file}\tERR {type(e).__name__}: {' '.join(str(e).split())[:160]}", flush=True)


def have(scanned_file, services):
    """Counter cùng dạng từ một mục `files` của `scan --json`: import, tham
    chiếu, và gRPC chỉ service có trong `.proto` của repo (`scan` đã lọc theo đó)."""
    import collections
    out = collections.Counter()
    for i in scanned_file.get("imports", []):
        out[(i["line"], f"import {i['spec']}")] += 1
    for r in scanned_file.get("refs", []):
        out[(r["line"], "ref " + "|".join(r["specs"]))] += 1
    for line, name, port, _ in scanned_file.get("hosts", {}).get("found", []):
        out[(line, f"host {name}:{port}" if port else f"host {name}")] += 1
    rpc = scanned_file.get("rpc", {})
    for kind, field in (("call", "calls"), ("serve", "serves"), ("impl", "implements")):
        for s in rpc.get(field, []):
            out[(s["line"], f"{kind} {s['service']} {s['origin']}:{s['from']}")] += 1
    return out


if __name__ == "__main__":
    main()
