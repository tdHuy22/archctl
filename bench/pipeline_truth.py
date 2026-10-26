"""Thước đo cho đồ thị pipeline (`scan --json`, mục `pipeline` của từng file
Python và mục `dvc` ở gốc).

Cây cú pháp lấy từ parser chuẩn, không từ lexer của archctl: `ast` cho Python,
PyYAML (`yaml.compose`, giữ dòng của từng node) cho `dvc.yaml`. Phạm vi của tên
lấy từ chính cây `ast` (thân `def`, `class`), không từ thụt lề như archctl. Luật
nhận lời gọi nào là bước, cạnh, chỗ không đọc được viết lại ở đây theo đúng đặc
tả trong `crates/arch-core/src/scan/pipeline.rs`; chỗ hai bên lệch là chỗ
lexer, bộ tách câu lệnh, bộ dựng phạm vi hay bộ tách đối số của archctl đọc sai.

So `(dòng, "công-cụ edge từ>tới hàm-từ>hàm-tới")` cho cạnh và
`(dòng, "công-cụ unread")` cho chỗ không đọc được (lời giải thích không so).
"""
import ast
import collections
import re
import unicodedata

MAX_EDGES = 10_000
MAX_ITEMS = 1_000
MAX_FOLLOW = 4
MAX_NEST = 64
MAX_ARGS = 16
MAX_STAGES = 2_000
MAX_PATHS = 50_000
MAX_SEGMENTS = 64

HERE = ("here",)
HINTS = ("langgraph", "airflow", "StateGraph", "MessageGraph", "DAG", "Operator", "Sensor")
LG_METHODS = ("add_node", "add_edge", "add_conditional_edges", "add_sequence")
AF_METHODS = ("set_downstream", "set_upstream")


def clean(s):
    return "".join("?" if unicodedata.category(c) == "Cc" else c for c in s)


def step_key(step):
    """Cùng cách viết với `have`: `here`, `module:a.b`, `call(…;…)`."""
    if step[0] == "here":
        return "here"
    if step[0] == "mod":
        return f"{step[1]}:{'.'.join(step[2])}"
    lost = "!" if step[3] else ""
    return f"call({step_key(step[1])};{','.join(step_key(a) for a in step[2])}{lost})"


def edge_key(line, tool, a, b, sa, sb):
    return (line, f"{tool} edge {a}>{b} {step_key(sa)}>{step_key(sb)}")


def unread_key(line, tool):
    return (line, f"{tool} unread")


def json_step(s):
    if s.get("here"):
        return HERE
    if "call" in s:
        return ("call", json_step(s["call"]), tuple(json_step(a) for a in s["args"]), s["lost"])
    return ("mod", s["module"], tuple(s["path"]))


def have(scanned_file):
    """Counter cùng dạng từ một mục `files` của `scan --json`."""
    p = scanned_file.get("pipeline", {})
    out = collections.Counter()
    for e in p.get("edges", []):
        out[edge_key(e["line"], e["tool"], e["from"], e["to"], json_step(e["fromStep"]), json_step(e["toStep"]))] += 1
    for u in p.get("unread", []):
        out[unread_key(u["line"], u["tool"])] += 1
    return out


def dotted(node):
    parts = []
    while isinstance(node, ast.Attribute):
        parts.append(node.attr)
        node = node.value
    if isinstance(node, ast.Name):
        parts.append(node.id)
        return ".".join(reversed(parts))
    return None


def last_seg(w):
    return w.rsplit(".", 1)[-1]


def func_name(call):
    """Đoạn cuối của tên hàm được gọi, như từ đứng trước `(` trong token."""
    f = call.func
    if isinstance(f, ast.Name):
        return f.id
    if isinstance(f, ast.Attribute):
        return f.attr
    return None


class Unreadable(Exception):
    pass


class Expr:
    """Như `enum Expr` của archctl."""

    def __init__(self, kind, **kw):
        self.kind = kind
        self.__dict__.update(kw)


OTHER = Expr("other")


def bind_step(bind, w):
    local, module, name = bind
    segs = w.split(".")
    lsegs = local.split(".")
    if name is None and (len(lsegs) > len(segs) or segs[:len(lsegs)] != lsegs):
        return ("mod", lsegs[0], tuple(segs[1:]))
    path = ([name] if name else []) + segs[min(len(lsegs), len(segs)):]
    return ("mod", module, tuple(path))


def strip_task_suffix(head):
    f = head
    while "." in f and f.rsplit(".", 1)[1] in ("override", "partial", "expand", "expand_kwargs"):
        f = f.rsplit(".", 1)[0]
    return f


def is_airflow_task_class(name):
    return name == "DAG" or name.endswith("Operator") or name.endswith("Sensor")


def runs_script(cmd):
    words = [w for w in re.split(r"[\s;&|()<>]", cmd) if w]
    if any(w.endswith(e) for w in words for e in (".py", ".sh", ".js", ".ts")):
        return True
    return any(a.rsplit("/", 1)[-1].startswith("python") and b == "-m" for a, b in zip(words, words[1:]))


def push_flat(step, into):
    if step[0] == "call":
        into.append(step[1])
        into.extend(step[2])
    else:
        into.append(step)


class Py:
    def __init__(self, src, tree):
        self.src = src
        self.tree = tree
        # Dòng dạng byte: `col_offset` của `ast` đếm theo byte UTF-8.
        self.lines = src.encode("utf-8").split(b"\n")
        self._calls = None
        # Phạm vi: 0 là module.
        self.scopes = [{"parent": None, "class": False, "dag": False}]
        self.names = [collections.defaultdict(list)]  # tên → [(dòng, cột, def)]
        self.stars = []
        self.groups = {}  # id của nút `with TaskGroup` → [(tên, dòng, phạm vi)]
        self.stmts = []  # (nút câu lệnh, phạm vi, trong DAG)
        self.walk_body(tree.body, 0, False, [])
        for d in self.names:
            for k in d:
                d[k].sort(key=lambda x: (x[0], x[1]))

    # ---- phạm vi ----

    def bind(self, scope, name, line, col, d):
        self.names[scope][name].append((line, col, d))

    def walk_body(self, body, scope, dag, groups):
        for st in body:
            self.walk_stmt(st, scope, dag, groups)

    def walk_stmt(self, st, scope, dag, groups):
        self.stmts.append((st, scope, dag))
        if isinstance(st, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            lit = None if isinstance(st, ast.ClassDef) else self.literal_return(st)
            self.bind(scope, st.name, st.lineno, st.col_offset, ("fn", lit))
            decos = [dotted(d) or (dotted(d.func) if isinstance(d, ast.Call) else None) for d in st.decorator_list]
            # Thân `def` lồng (hàm của `@task`) không nối đồ thị, trừ `@dag`, `@task_group`.
            is_dag = any(d and last_seg(d) in ("dag", "task_group") for d in decos)
            sid = len(self.scopes)
            self.scopes.append({"parent": scope, "class": isinstance(st, ast.ClassDef), "dag": is_dag})
            self.names.append(collections.defaultdict(list))
            if not isinstance(st, ast.ClassDef):
                a = st.args
                for arg in [*a.posonlyargs, *a.args, *a.kwonlyargs, *([a.vararg] if a.vararg else []), *([a.kwarg] if a.kwarg else [])]:
                    self.bind(sid, arg.arg, st.lineno, -1, ("param",))
            self.walk_body(st.body, sid, is_dag, groups)
            return
        if isinstance(st, ast.ImportFrom):
            module = "." * st.level + (st.module or "")
            if module:
                for a in st.names:
                    if a.name == "*":
                        if scope == 0:
                            self.stars.append(module)
                    else:
                        local = a.asname or a.name
                        self.bind(scope, local, st.lineno, st.col_offset, ("bind", (local, module, a.name)))
            return
        if isinstance(st, ast.Import):
            for a in st.names:
                local = a.asname or a.name
                self.bind(scope, local.split(".")[0], st.lineno, st.col_offset, ("bind", (local, a.name, None)))
            return
        if isinstance(st, (ast.With, ast.AsyncWith)):
            is_dag, group = False, False
            for item in st.items:
                c = item.context_expr
                h = self.chain(c) if isinstance(c, ast.Call) else None
                kind = last_seg(h[0]) if h else None
                if kind == "DAG":
                    is_dag = True
                if kind == "TaskGroup" and isinstance(item.optional_vars, ast.Name):
                    group = True
                if isinstance(item.optional_vars, ast.Name):
                    self.bind(scope, item.optional_vars.id, st.lineno, c.col_offset, ("with", c, st))
            inner_groups, inner_dag = groups, dag
            if group:
                self.groups.setdefault(id(st), [])
                inner_groups = groups + [id(st)]
            elif is_dag:
                inner_dag = True
            self.walk_body(st.body, scope, inner_dag, inner_groups)
            return
        targets, value = [], None
        if isinstance(st, ast.Assign):
            targets = [t.id for t in st.targets if isinstance(t, ast.Name)]
            value = st.value
        elif isinstance(st, ast.AnnAssign) and isinstance(st.target, ast.Name) and st.value is not None:
            targets = [st.target.id]
            value = st.value
        for t in targets:
            for g in groups:
                self.groups[g].append((t, st.lineno, scope))
            self.bind(scope, t, st.lineno, st.col_offset, ("assign", value))
        for field in ("body", "orelse", "finalbody"):
            sub = getattr(st, field, None)
            if isinstance(sub, list) and sub and isinstance(sub[0], ast.stmt):
                self.walk_body(sub, scope, dag, groups)
        for h in getattr(st, "handlers", None) or []:
            self.walk_body(h.body, scope, dag, groups)
        for c in getattr(st, "cases", None) or []:
            self.walk_body(c.body, scope, dag, groups)

    def literal_return(self, fn):
        r = fn.returns
        if isinstance(r, ast.Subscript) and dotted(r.value) and last_seg(dotted(r.value)) == "Literal":
            elts = r.slice.elts if isinstance(r.slice, ast.Tuple) else [r.slice]
            if all(isinstance(e, ast.Constant) and isinstance(e.value, str) for e in elts):
                return [e.value for e in elts]
        return None

    def lookup(self, first, scope, line):
        s, own = scope, True
        while s is not None:
            if own or not self.scopes[s]["class"]:
                lst = self.names[s].get(first)
                if lst:
                    pick = next((x for x in reversed(lst) if x[0] <= line), lst[0]) if own else lst[-1]
                    return s, pick[2], pick[0]
            own = False
            s = self.scopes[s]["parent"]
        return None

    def bound(self, w, scope, line):
        r = self.lookup(w.split(".")[0], scope, line)
        if r and r[1][0] == "bind":
            return bind_step(r[1][1], w)
        return None

    def lib_name(self, w, scope, line, lib):
        b = self.bound(w, scope, line)
        if b and (b[1] == lib or b[1].startswith(lib + ".")) and b[2]:
            return b[2][-1]
        return None

    # ---- biểu thức ----

    def head(self, node):
        """Hai byte đầu của biểu thức, như token đầu mà archctl thấy. Không dùng
        `ast.get_source_segment`: nó tách lại cả file mỗi lần gọi."""
        line = self.lines[node.lineno - 1] if 0 < node.lineno <= len(self.lines) else b""
        return line[node.col_offset:node.col_offset + 2]

    def expr(self, node):
        if node is None or node is OTHER:
            return OTHER
        h = self.head(node)
        if h[:1].isdigit() or (h[:1] == b"-" and h[1:2].isdigit()):
            return Expr("num")
        if isinstance(node, ast.Constant):
            return Expr("str", s=node.value) if isinstance(node.value, str) else OTHER
        if isinstance(node, ast.JoinedStr):
            return Expr("fstr")
        if isinstance(node, (ast.List, ast.Tuple)):
            return Expr("seq", items=list(node.elts))
        if isinstance(node, ast.Dict):
            if any(k is None for k in node.keys):
                return OTHER
            return Expr("dict", pairs=list(zip(node.keys, node.values)))
        if isinstance(node, ast.Lambda):
            return Expr("lambda", body=node.body)
        d = dotted(node)
        if d is not None:
            return Expr("name", w=d)
        c = self.chain(node)
        if c:
            return Expr("call", head=c[0], args=c[1], rest=c[2])
        return OTHER

    def chain(self, node):
        """`head(args)` của lời gọi đầu trong chuỗi `a(x).b(y)(z)`, cùng đối số
        của các lời gọi sau, như archctl đọc chuỗi token."""
        outer = []
        while True:
            if isinstance(node, ast.Call):
                d = dotted(node.func)
                if d is not None:
                    return d, self.args(node), [a for args in reversed(outer) for a in args]
                outer.append(self.args(node))
                node = node.func
                # `a(x).b(...)`: thuộc tính ngay sau một lời gọi.
                while isinstance(node, ast.Attribute):
                    node = node.value
                if not isinstance(node, ast.Call):
                    return None
            elif isinstance(node, ast.Attribute) and isinstance(node.value, ast.Call):
                node = node.value
            else:
                return None

    def args(self, call):
        items = []
        for a in call.args:
            items.append(((a.lineno, a.col_offset), None, OTHER if isinstance(a, ast.Starred) else a))
        for k in call.keywords:
            items.append(((k.lineno, k.col_offset), k.arg, k.value if k.arg else OTHER))
        return [(kw, v) for _, kw, v in sorted(items, key=lambda x: x[0])]

    @staticmethod
    def arg(args, pos, kw):
        for k, v in args:
            if k == kw:
                return v
        positional = [v for k, v in args if k is None]
        return positional[pos] if pos < len(positional) else None

    def literal(self, node):
        e = self.expr(node)
        return e.s if e.kind == "str" and "\0" not in e.s else None

    def resolve_name(self, w, scope, line, depth):
        first = w.split(".")[0]
        if first in ("self", "cls"):
            return HERE
        r = self.lookup(first, scope, line)
        if r is None:
            if len(self.stars) == 1:
                return ("mod", self.stars[0], tuple(w.split(".")))
            raise Unreadable()
        sid, d, l = r
        if d[0] == "fn":
            return HERE
        if d[0] == "bind":
            return bind_step(d[1], w)
        if d[0] == "param":
            raise Unreadable()
        if depth >= MAX_FOLLOW:
            raise Unreadable()
        return self.resolve_expr(d[1], sid, max(l - 1, 0), depth + 1)

    def resolve_expr(self, node, scope, line, depth):
        if depth > MAX_NEST:
            raise Unreadable()
        e = self.expr(node)
        if e.kind == "lambda":
            be = self.expr(e.body)
            if be.kind == "call":
                return self.resolve_name(be.head, scope, line, depth + 1)
            return HERE
        if e.kind == "name":
            return self.resolve_name(e.w, scope, line, depth)
        if e.kind == "call":
            h = self.resolve_name(e.head, scope, line, depth + 1)
            inner = []
            tried = found = 0
            for k, v in e.args[:MAX_ARGS]:
                ve = self.expr(v)
                if ve.kind == "seq":
                    items = [(x, scope, line) for x in ve.items[:MAX_ARGS]]
                elif ve.kind == "name":
                    r = self.lookup(ve.w, scope, line)
                    if r and r[1][0] == "assign" and self.expr(r[1][1]).kind == "seq":
                        items = [(x, r[0], max(r[2] - 1, 0)) for x in self.expr(r[1][1]).items[:MAX_ARGS]]
                    else:
                        items = [(v, scope, line)]
                elif ve.kind in ("call", "lambda"):
                    items = [(v, scope, line)]
                else:
                    items = []
                for it, sc, ln in items:
                    if k is None:
                        tried += 1
                    try:
                        st = self.resolve_expr(it, sc, ln, depth + 1)
                    except Unreadable:
                        continue
                    if k is None:
                        found += 1
                    push_flat(st, inner)
                if len(inner) >= MAX_ARGS:
                    break
            inner = inner[:MAX_ARGS]
            lost = tried > 0 and found == 0 and h != HERE
            return ("call", h, tuple(inner), lost) if inner or lost else h
        raise Unreadable()

    # ---- lời gọi theo câu lệnh ----

    def own_nodes(self, st):
        """Nút biểu thức của riêng câu lệnh, không vào câu lệnh con."""
        stack = [c for c in ast.iter_child_nodes(st) if not isinstance(c, ast.stmt)]
        while stack:
            n = stack.pop()
            yield n
            stack.extend(c for c in ast.iter_child_nodes(n) if not isinstance(c, ast.stmt))

    def calls_in(self):
        if self._calls is None:
            self._calls = [(node, scope, dag) for st, scope, dag in self.stmts
                           for node in self.own_nodes(st) if isinstance(node, ast.Call)]
        return self._calls

    def method_calls(self, methods):
        out = []
        for node, scope, _ in self.calls_in():
            if not isinstance(node.func, ast.Attribute) or node.func.attr not in methods:
                continue
            recv = dotted(node.func.value) or "?"
            pos = (node.func.end_lineno, node.func.end_col_offset)
            out.append((pos, node.func.end_lineno, scope, recv, node.func.attr, self.args(node)))
        out.sort(key=lambda x: x[0])
        return [x[1:] for x in out]

    def aliases(self, out, methods, tool):
        for st, _, _ in self.stmts:
            nodes = list(self.own_nodes(st))
            funcs = {id(n.func) for n in nodes if isinstance(n, ast.Call)}
            for n in nodes:
                if isinstance(n, ast.Attribute) and n.attr in methods and dotted(n) and id(n) not in funcs:
                    out[unread_key(n.end_lineno, tool)] += 1

    # ---- LangGraph ----

    def langgraph(self, out):
        def unread(line):
            out[unread_key(line, "langgraph")] += 1

        def builder(recv, scope, line):
            if recv == "?":
                return (scope, recv, line)
            r = self.lookup(recv.split(".")[0], scope, line)
            return (r[0], recv, r[2]) if r else (scope, recv, 0)

        nodes, dynamic, pending = {}, set(), []
        edges = [0]

        def resolved(fn, line):
            try:
                return fn()
            except Unreadable:
                unread(line)
                return None

        for line, scope, recv, m, args in self.method_calls(("add_node", "add_sequence")):
            key = builder(recv, scope, line)
            if m == "add_sequence":
                a = self.arg(args, 0, "nodes")
                if a is None:
                    continue
                e = self.expr(a)
                if e.kind != "seq" or len(e.items) > MAX_ITEMS:
                    unread(line)
                    dynamic.add(key)
                    continue
                prev = None
                for item in e.items:
                    ie = self.expr(item)
                    if ie.kind == "name":
                        name, fn = last_seg(ie.w), (lambda w=ie.w: self.resolve_name(w, scope, line, 0))
                    elif ie.kind == "seq" and len(ie.items) == 2:
                        n = self.literal(ie.items[0])
                        if n is None:
                            unread(line)
                            dynamic.add(key)
                            prev = None
                            continue
                        name, fn = n, (lambda x=ie.items[1]: self.resolve_expr(x, scope, line, 0))
                    else:
                        unread(line)
                        dynamic.add(key)
                        prev = None
                        continue
                    nodes[(key, name)] = resolved(fn, line)
                    if prev is not None:
                        pending.append((line, key, prev, name))
                    prev = name
                continue
            node = self.arg(args, 0, "node")
            action = self.arg(args, 1, "action")
            if node is not None and action is not None:
                name = self.literal(node)
                if name is None:
                    unread(line)
                    dynamic.add(key)
                    continue
                fn = lambda a=action: self.resolve_expr(a, scope, line, 0)  # noqa: E731
            elif node is not None:
                e = self.expr(node)
                if e.kind != "name":
                    unread(line)
                    dynamic.add(key)
                    continue
                name, fn = last_seg(e.w), (lambda w=e.w: self.resolve_name(w, scope, line, 0))
            else:
                continue
            nodes[(key, name)] = resolved(fn, line)

        def edge(line, key, a, b):
            if a == b or edges[0] >= MAX_EDGES:
                return
            steps = []
            for name in (a, b):
                if (key, name) in nodes:
                    steps.append(nodes[(key, name)])
                else:
                    if key not in dynamic:
                        unread(line)
                    steps.append(None)
            if steps[0] is None or steps[1] is None:
                return
            edges[0] += 1
            out[edge_key(line, "langgraph", clean(a), clean(b), steps[0], steps[1])] += 1

        for p in pending:
            edge(*p)

        def is_start(e):
            return (e.kind == "name" and last_seg(e.w) == "START") or (e.kind == "str" and e.s == "__start__")

        def is_end(e):
            return (e.kind == "name" and last_seg(e.w) == "END") or (e.kind == "str" and e.s == "__end__")

        for line, scope, recv, m, args in self.method_calls(("add_edge", "add_conditional_edges")):
            key = builder(recv, scope, line)
            src = self.arg(args, 0, "start_key" if m == "add_edge" else "source")
            if src is None:
                continue
            se = self.expr(src)
            if is_start(se):
                continue
            if se.kind == "str" and "\0" not in se.s:
                sources = [se.s]
            elif se.kind == "seq" and m == "add_edge" and len(se.items) <= MAX_ITEMS:
                names = [self.literal(x) for x in se.items]
                if any(n is None for n in names):
                    unread(line)
                    continue
                sources = names
            else:
                unread(line)
                continue
            if m == "add_edge":
                dst = self.arg(args, 1, "end_key")
                if dst is None or is_end(self.expr(dst)):
                    continue
                n = self.literal(dst)
                if n is None:
                    unread(line)
                    continue
                targets = [n]
            else:
                targets = self.branch_targets(args, scope, line)
                if targets is None:
                    unread(line)
                    continue
            for a in sources:
                for b in targets:
                    edge(line, key, a, b)

        self.aliases(out, LG_METHODS, "langgraph")
        for node, scope, _ in self.calls_in():
            w = dotted(node.func)
            if w is None:
                continue
            line = node.func.end_lineno
            name = self.lib_name(w, scope, line, "langgraph")
            if name == "Command":
                args = self.args(node)
                spread = any(k.arg is None for k in node.keywords)
                positional = sum(1 for k, _ in args if k is None)
                if spread or positional >= 4 or any(k == "goto" for k, _ in args):
                    unread(line)
            elif name == "Send":
                unread(line)

    def branch_targets(self, args, scope, line):
        def names(nodes):
            if len(nodes) > MAX_ITEMS:
                return None
            out = []
            for n in nodes:
                e = self.expr(n)
                if (e.kind == "name" and last_seg(e.w) == "END") or (e.kind == "str" and e.s == "__end__"):
                    continue
                x = self.literal(n)
                if x is None:
                    return None
                out.append(x)
            return out

        pm = self.arg(args, 2, "path_map")
        if pm is not None:
            e = self.expr(pm)
            if e.kind == "name":
                r = self.lookup(e.w, scope, line)
                e = self.expr(r[1][1]) if r and r[1][0] == "assign" else OTHER
            if e.kind == "dict":
                return names([v for _, v in e.pairs])
            if e.kind == "seq":
                return names(e.items)
            return None
        path = self.arg(args, 1, "path")
        if path is None:
            return None
        e = self.expr(path)
        if e.kind == "name":
            if last_seg(e.w) == "tools_condition":
                return ["tools"]
            r = self.lookup(e.w, scope, line)
            if r and r[1][0] == "fn" and r[1][1] is not None:
                return [n for n in r[1][1] if n != "__end__"]
        return None

    # ---- Airflow ----

    def cross(self, line, l, r):
        for x in l:
            for y in r:
                if x[1] is None or y[1] is None:
                    continue
                if self.n_edges >= MAX_EDGES:
                    return
                self.n_edges += 1
                self.out[edge_key(line, "airflow", clean(x[0]), clean(y[0]), x[1], y[1])] += 1

    def airflow(self, out):
        self.out, self.n_edges = out, 0

        def unread(line):
            out[unread_key(line, "airflow")] += 1

        for st, scope, dag in self.stmts:
            if not isinstance(st, (ast.Expr, ast.Return, ast.Assign, ast.AugAssign, ast.AnnAssign)):
                continue
            line = st.lineno
            value = st.value
            if isinstance(value, ast.Yield):
                value = value.value
            spine = set()
            if isinstance(value, ast.BinOp) and isinstance(value.op, (ast.RShift, ast.LShift)):
                operands, ops = [], []
                v = value
                while isinstance(v, ast.BinOp) and isinstance(v.op, (ast.RShift, ast.LShift)):
                    spine.add(id(v))
                    operands.append(v.right)
                    ops.append(isinstance(v.op, ast.RShift))
                    v = v.left
                operands.append(v)
                operands.reverse()
                ops.reverse()
                self.chain_statement(operands, ops, scope, line, unread)
            elif dag and value is not None:
                label = None
                if isinstance(st, ast.Assign) and len(st.targets) == 1 and isinstance(st.targets[0], ast.Name):
                    label = st.targets[0].id
                elif isinstance(st, ast.AnnAssign) and isinstance(st.target, ast.Name):
                    label = st.target.id
                self.taskflow(value, scope, line, label, 0)
            for n in self.own_nodes(st):
                if isinstance(n, ast.BinOp) and isinstance(n.op, (ast.RShift, ast.LShift)) and id(n) not in spine \
                        and isinstance(n.left, ast.Name):
                    try:
                        t = self.task_name(n.left.id, scope, line, 0)
                    except Unreadable:
                        t = None
                    if t:
                        unread(line)
                        break

        for line, scope, recv, m, args in self.method_calls(("set_downstream", "set_upstream", "set_dependency")):
            if m == "set_dependency":
                unread(line)
                continue
            a = self.arg(args, 0, "task_or_task_list")
            if a is None:
                continue
            l = r = None
            err = False
            try:
                l = self.task_name(recv, scope, line, 0)
            except Unreadable:
                err = True
            try:
                r = self.tasks(a, scope, line, 0)
            except Unreadable:
                err = True
            if err:
                unread(line)
            elif l is not None and r is not None:
                if m == "set_downstream":
                    self.cross(line, l, r)
                else:
                    self.cross(line, r, l)

        self.aliases(out, AF_METHODS, "airflow")
        found = []
        for node, scope, _ in self.calls_in():
            w = dotted(node.func)
            if not w:
                continue
            line = node.func.end_lineno
            kind = self.lib_name(w, scope, line, "airflow")
            if kind in ("chain", "chain_linear", "cross_downstream"):
                found.append(((node.func.end_lineno, node.func.end_col_offset), node, scope, line, kind))
        for _, node, scope, line, kind in sorted(found, key=lambda x: x[0]):
            args = self.args(node)
            if kind == "cross_downstream":
                f, t = self.arg(args, 0, "from_tasks"), self.arg(args, 1, "to_tasks")
                if f is None or t is None:
                    continue
                l = r = None
                err = False
                try:
                    l = self.tasks(f, scope, line, 0)
                except Unreadable:
                    err = True
                try:
                    r = self.tasks(t, scope, line, 0)
                except Unreadable:
                    err = True
                if err:
                    unread(line)
                elif l is not None and r is not None:
                    self.cross(line, l, r)
                continue
            sides = []
            bad = False
            for k, v in args:
                if k is not None:
                    continue
                is_list = self.expr(v).kind == "seq"
                try:
                    t = self.tasks(v, scope, line, 0)
                except Unreadable:
                    unread(line)
                    bad = True
                    break
                if t is None:
                    bad = True
                    break
                sides.append((t, is_list))
            if bad:
                continue
            for (l, ll), (r, rl) in zip(sides, sides[1:]):
                if kind == "chain" and ll and rl:
                    if len(l) != len(r):
                        unread(line)
                        continue
                    for x, y in zip(l, r):
                        self.cross(line, [x], [y])
                else:
                    self.cross(line, l, r)

    def chain_statement(self, operands, ops, scope, line, unread):
        sides = []
        for o in operands:
            try:
                t = self.tasks(o, scope, line, 0)
            except Unreadable:
                unread(line)
                return
            if t is None:
                return
            sides.append(t)
        for n, down in enumerate(ops):
            l, r = (sides[n], sides[n + 1]) if down else (sides[n + 1], sides[n])
            self.cross(line, l, r)

    def taskflow(self, node, scope, line, label, depth):
        if depth > MAX_NEST:
            return None
        e = self.expr(node)
        if e.kind != "call":
            return None
        f = strip_task_suffix(e.head)
        if is_airflow_task_class(last_seg(f)) or last_seg(f) == "TaskGroup":
            return None
        r = self.lookup(f.split(".")[0], scope, line)
        if not r or r[1][0] not in ("fn", "bind"):
            return None
        try:
            callee = self.task_call(e.head, e.args, scope, line)
        except Unreadable:
            return None
        if label is not None:
            callee = (label, callee[1])
        for _, v in (e.args + e.rest)[:MAX_ARGS]:
            ve = self.expr(v)
            items = ve.items[:MAX_ITEMS] if ve.kind == "seq" else [v]
            for it in items:
                ie = self.expr(it)
                if ie.kind == "call":
                    t = self.taskflow(it, scope, line, None, depth + 1)
                    if t is not None:
                        self.cross(line, [t], [callee])
                elif ie.kind == "name":
                    try:
                        ts = self.task_name(ie.w, scope, line, 0)
                    except Unreadable:
                        ts = None
                    if ts:
                        self.cross(line, ts, [callee])
        return callee

    def tasks(self, node, scope, line, depth):
        """Danh sách `(nhãn, hàm|None)`, hay None khi vế này không phải task."""
        if depth > MAX_NEST:
            raise Unreadable()
        e = self.expr(node)
        if e.kind in ("num", "str", "fstr", "lambda"):
            return None
        if e.kind == "seq":
            if len(e.items) > MAX_ITEMS:
                raise Unreadable()
            out = []
            for it in e.items:
                t = self.tasks(it, scope, line, depth + 1)
                if t is None:
                    return None
                out.extend(t)
                if len(out) > MAX_ITEMS:
                    raise Unreadable()
            return out
        if e.kind == "name":
            return self.task_name(e.w, scope, line, depth + 1)
        if e.kind == "call":
            return [self.task_call(e.head, e.args, scope, line)]
        raise Unreadable()

    def task_name(self, w, scope, line, depth):
        if "." in w or depth > MAX_NEST:
            raise Unreadable()
        r = self.lookup(w, scope, line)
        if r is None:
            raise Unreadable()
        sid, d, l = r
        if d[0] == "fn":
            return None
        if d[0] in ("bind", "param"):
            raise Unreadable()
        value = d[1]
        with_stmt = d[2] if d[0] == "with" else None
        e = self.expr(value)
        if e.kind == "call" and last_seg(e.head) == "TaskGroup":
            members = self.groups.get(id(with_stmt)) if with_stmt is not None else None
            if not members:
                raise Unreadable()
            out = []
            for m, ml, ms in members[:MAX_ITEMS]:
                t = self.task_name(m, ms, ml, depth + 1)
                if t:
                    out.extend(t)
                if len(out) > MAX_ITEMS:
                    raise Unreadable()
            return out
        if e.kind == "call":
            t = self.task_call(e.head, e.args, sid, l)
            return [(w, t[1])]
        if e.kind in ("name", "seq") and depth < MAX_FOLLOW * 4:
            return self.tasks(value, sid, l, depth + 1)
        if e.kind in ("num", "str", "fstr", "dict", "lambda"):
            return None
        if with_stmt is not None:
            return None
        raise Unreadable()

    def task_call(self, head, args, scope, line):
        label = f"{head}()"
        f = strip_task_suffix(head)
        last = last_seg(f)
        if last.endswith("Operator") or last.endswith("Sensor"):
            for k, v in args:
                if k == "python_callable":
                    return (label, self.resolve_expr(v, scope, line, 0))
            for k, v in args:
                if k == "bash_command":
                    cmd = self.literal(v)
                    if cmd is not None and runs_script(cmd):
                        raise Unreadable()
                    break
            return (label, None)
        if last == "TaskGroup":
            raise Unreadable()
        step = self.resolve_name(f, scope, line, 0)
        if step[0] == "mod" and (step[1] == "airflow" or step[1].startswith("airflow.")):
            raise Unreadable()
        return (label, step)


def uses(tree, lib):
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            if any(a.name == lib or a.name.startswith(lib + ".") for a in node.names):
                return True
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            if node.module == lib or node.module.startswith(lib + "."):
                return True
    return False


def calls(tree, pred):
    return any(isinstance(n, ast.Call) and func_name(n) and pred(func_name(n)) for n in ast.walk(tree))


def python_truth(path):
    """Counter `(dòng, khoá)`, hay None nếu `ast` không đọc được file."""
    try:
        src = path.read_text(encoding="utf-8")
    except (ValueError, UnicodeDecodeError):
        return None
    out = collections.Counter()
    # Cổng của archctl đòi tên thư viện hay lời gọi `StateGraph(`, `DAG(`,
    # `…Operator(` có trong file: không có chữ nào thì không cần parse.
    if not any(h in src for h in HINTS):
        return out
    try:
        tree = ast.parse(src)
    except (SyntaxError, ValueError, RecursionError):
        return None
    lg = uses(tree, "langgraph") or calls(tree, lambda n: n in ("StateGraph", "MessageGraph"))
    af = uses(tree, "airflow") or calls(tree, is_airflow_task_class)
    if not lg and not af:
        return out
    py = Py(src, tree)
    if lg:
        py.langgraph(out)
    if af:
        py.airflow(out)
    return out


# ---- dvc ----

def within(base, path):
    if path.startswith(("/", "~")) or "\\" in path or "://" in path:
        return None
    parts = [s for s in base.split("/") if s]
    for seg in path.split("/"):
        if seg in ("", "."):
            continue
        if seg == "..":
            if not parts:
                return None
            parts.pop()
        elif len(seg) >= 2 and seg[0].isascii() and seg[0].isalpha() and seg[1] == ":":
            return None  # tiền tố ổ đĩa, như `plain_segment` của deploy.rs
        else:
            parts.append(seg)
    return "/".join(parts)


def dvc_truth(text, file):
    """Counter `(dòng, khoá)` của cạnh và chỗ không đọc được, như `dvc` và
    `dvc_edges` của archctl (trong một file)."""
    import yaml

    out = collections.Counter()
    unread = lambda line: out.__setitem__(unread_key(line, "dvc"), out[unread_key(line, "dvc")] + 1)  # noqa: E731
    try:
        doc = yaml.compose(text, Loader=yaml.SafeLoader)
    except yaml.YAMLError as e:
        mark = getattr(e, "problem_mark", None) or getattr(e, "context_mark", None)
        unread(mark.line + 1 if mark else 0)
        return out
    if not isinstance(doc, yaml.MappingNode):
        return out
    get = lambda node, key: next((v for k, v in node.value if isinstance(k, yaml.ScalarNode) and k.value == key), None)  # noqa: E731
    stages_node = get(doc, "stages")
    if stages_node is None:
        return out
    if not isinstance(stages_node, yaml.MappingNode):
        unread(stages_node.start_mark.line + 1)
        return out
    base = file.rsplit("/", 1)[0] if "/" in file else ""
    stages = []
    paths = [0]
    for k, node in stages_node.value:
        name, line = k.value, k.start_mark.line + 1
        if len(stages) >= MAX_STAGES or paths[0] >= MAX_PATHS:
            unread(line)
            break
        if isinstance(node, yaml.MappingNode) and (get(node, "foreach") or get(node, "matrix")):
            unread(line)
            continue
        wdir = get(node, "wdir") if isinstance(node, yaml.MappingNode) else None
        if isinstance(wdir, yaml.ScalarNode):
            if "${" in wdir.value:
                unread(line)
                continue
            d = within(base, wdir.value.rstrip("/"))
            if d is None:
                unread(line)
        else:
            d = base
        cmd = get(node, "cmd") if isinstance(node, yaml.MappingNode) else None
        cmds = []
        if isinstance(cmd, yaml.SequenceNode):
            cmds = [i.value for i in cmd.value if isinstance(i, yaml.ScalarNode)]
        elif isinstance(cmd, yaml.ScalarNode):
            cmds = [cmd.value]
        if any("${" in c for c in cmds):
            unread(line)

        def lst(keys):
            v, seen = [], set()
            for key in keys:
                n = get(node, key) if isinstance(node, yaml.MappingNode) else None
                if not isinstance(n, yaml.SequenceNode):
                    continue
                for item in n.value:
                    il = item.start_mark.line + 1
                    if paths[0] >= MAX_PATHS:
                        unread(il)
                        return v
                    if isinstance(item, yaml.ScalarNode):
                        raw = item.value
                    elif isinstance(item, yaml.MappingNode) and item.value and isinstance(item.value[0][0], yaml.ScalarNode):
                        raw = item.value[0][0].value
                    else:
                        continue
                    if "${" in raw:
                        unread(il)
                        continue
                    if "://" in raw:
                        continue
                    p = raw.rstrip("/")
                    r = within(d, p) if d is not None else None
                    p = r if r is not None else p
                    if len(p.split("/")) > MAX_SEGMENTS:
                        unread(il)
                        continue
                    if p not in seen:
                        seen.add(p)
                        paths[0] += 1
                        v.append((p, il))
            return v

        deps = lst(["deps"])
        outs = lst(["outs", "metrics", "plots"])
        stages.append((clean(name), deps, outs))

    exact = collections.defaultdict(list)
    under = collections.defaultdict(list)
    for s, (_, _, outs) in enumerate(stages):
        for p, _ in outs:
            exact[p].append(s)
            d = p
            while "/" in d:
                d = d.rsplit("/", 1)[0]
                under[d].append(s)
    seen = set()
    n = 0
    for t, (_, deps, _) in enumerate(stages):
        for p, line in deps:
            frm = list(exact.get(p, []))
            d = p
            while "/" in d:
                d = d.rsplit("/", 1)[0]
                frm.extend(exact.get(d, []))
            frm.extend(under.get(p, []))
            for f in frm:
                if f == t or (f, t, p) in seen:
                    continue
                seen.add((f, t, p))
                if n >= MAX_EDGES:
                    return out
                n += 1
                out[(line, f"dvc edge {stages[f][0]}>{stages[t][0]} {p}")] += 1
    return out


def dvc_have(entry):
    out = collections.Counter()
    for e in entry.get("edges", []):
        out[(e["line"], f"dvc edge {e['from']}>{e['to']} {e['path']}")] += 1
    for u in entry.get("unread", []):
        out[unread_key(u["line"], "dvc")] += 1
    return out


def main():
    """Mỗi dòng vào `gốc\\tfile` của một `dvc.yaml`; mỗi dòng ra
    `file\\t[[dòng, khoá], ...]` hay `file\\tERR …`. Chạy trong venv có PyYAML."""
    import json
    import sys

    for raw in sys.stdin:
        root, file = raw.rstrip("\n").split("\t")
        try:
            with open(f"{root}/{file}", encoding="utf-8") as f:
                text = f.read()
            if text.startswith("﻿"):
                text = text[1:]
            facts = sorted(k for k, n in dvc_truth(text, file).items() for _ in range(n))
            print(f"{file}\t{json.dumps(facts, ensure_ascii=False)}", flush=True)
        except (UnicodeDecodeError, ValueError, RecursionError) as e:
            print(f"{file}\tERR {type(e).__name__}: {' '.join(str(e).split())[:160]}", flush=True)


if __name__ == "__main__":
    main()
