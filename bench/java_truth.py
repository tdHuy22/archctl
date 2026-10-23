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
import json
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
    return out


def main():
    import tree_sitter
    import tree_sitter_java

    parser = tree_sitter.Parser(tree_sitter.Language(tree_sitter_java.language()))
    for raw in sys.stdin:
        root, file = raw.rstrip("\n").split("\t")
        try:
            with open(f"{root}/{file}", "rb") as f:
                src = f.read()
            print(f"{file}\t{json.dumps(sorted(facts(src, parser)), ensure_ascii=False)}", flush=True)
        except (OSError, ValueError, RecursionError) as e:
            print(f"{file}\tERR {type(e).__name__}: {' '.join(str(e).split())[:160]}", flush=True)


def have(scanned_file, services):
    """Counter cùng dạng từ một mục `files` của `scan --json`; chỉ service có
    trong `.proto` của repo (`scan` đã lọc theo đó)."""
    import collections
    out = collections.Counter()
    rpc = scanned_file.get("rpc", {})
    for kind, field in (("call", "calls"), ("serve", "serves"), ("impl", "implements")):
        for s in rpc.get(field, []):
            out[(s["line"], f"{kind} {s['service']} {s['origin']}:{s['from']}")] += 1
    return out


if __name__ == "__main__":
    main()
