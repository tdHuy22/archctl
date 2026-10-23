"""Thước đo cho chỗ gửi, nhận qua hàng đợi (`scan --json`, mục `queues`).

Lời gọi và đối số lấy từ parser chuẩn, không từ lexer của archctl: `ast` cho
Python, `go/ast` cho Go (dựng một lần thành binary tạm). Luật nhận lời gọi nào
là gửi, nhận viết lại ở đây theo đúng đặc tả trong
`crates/arch-core/src/scan/queue.rs`; chỗ hai bên lệch là chỗ lexer hay bộ tách
đối số của archctl đọc sai.

So `(dòng, "hệ vai loại:tên")`; tên của loại `dynamic` không so (đó là cách
viết rút gọn để báo). JS/TS không có thước đo: Node không có parser AST dựng sẵn.
"""
import ast
import collections
import json
import subprocess
import warnings

PY_LIBS = {
    "kafka": ("kafka", "confluent_kafka", "aiokafka"),
    "rabbit": ("pika",),
    "redis": ("redis", "aioredis", "coredis"),
    "celery": ("celery",),
    "nats": ("nats",),
    "aio": ("aio_pika",),
    "kombu": ("kombu",),
}


def key(line, system, role, kind, name):
    """Khoá so của `oracle.check_lines`: `(dòng, "kafka send fixed:orders")`."""
    return (line, f"{system} {role} {kind}:{name if kind != 'dynamic' else ''}")


def has(specs, prefixes):
    return any(s == p or s.startswith(p + ".") or s.startswith(p + "/") for s in specs for p in prefixes)


def dotted(node):
    """`self.p.produce` → "self.p.produce"; None nếu không phải chuỗi tên."""
    parts = []
    while isinstance(node, ast.Attribute):
        parts.append(node.attr)
        node = node.value
    if isinstance(node, ast.Name):
        parts.append(node.id)
        return ".".join(reversed(parts))
    return None


def target_names(t):
    """Tên trong vế trái của phép gán, vòng lặp: `a`, `a, b`, `self.x`."""
    if isinstance(t, (ast.Tuple, ast.List)):
        return [n for e in t.elts for n in target_names(e)]
    if isinstance(t, ast.Starred):
        return target_names(t.value)
    d = dotted(t)
    return [d] if d else []


class Py:
    def __init__(self, tree, src=""):
        specs = []
        for n in ast.walk(tree):
            if isinstance(n, ast.Import):
                specs += [a.name for a in n.names]
            elif isinstance(n, ast.ImportFrom) and n.module and not n.level:
                specs.append(n.module)
        self.libs = {k: has(specs, v) for k, v in PY_LIBS.items()}
        self.consts = {}
        lines = src.splitlines()

        def statement_start(node):
            """Vế trái đứng đầu câu lệnh: trước nó trên dòng chỉ có khoảng trắng
            hay dấu `;`."""
            if node.lineno - 1 >= len(lines):
                return True
            before = lines[node.lineno - 1][:node.col_offset].rstrip()
            return before == "" or before.endswith(";")

        bound = set()
        for n in ast.walk(tree):
            if isinstance(n, ast.Assign) and len(n.targets) == 1 and not isinstance(n.targets[0], (ast.Tuple, ast.List)):
                target, value = n.targets[0], n.value
            elif isinstance(n, ast.AnnAssign) and n.value is not None:
                target, value = n.target, n.value
            else:
                # Ràng buộc không qua phép gán đơn: tên không cố định.
                if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef, ast.Lambda)):
                    a = n.args
                    bound.update(x.arg for x in a.posonlyargs + a.args + a.kwonlyargs)
                    bound.update(x.arg for x in (a.vararg, a.kwarg) if x)
                elif isinstance(n, (ast.For, ast.AsyncFor, ast.comprehension)):
                    bound.update(target_names(n.target))
                elif isinstance(n, ast.withitem) and n.optional_vars is not None:
                    bound.update(target_names(n.optional_vars))
                elif isinstance(n, ast.ExceptHandler) and n.name:
                    bound.add(n.name)
                elif isinstance(n, (ast.Import, ast.ImportFrom)):
                    bound.update(a.asname for a in n.names if a.asname)
                elif isinstance(n, ast.NamedExpr):
                    bound.update(target_names(n.target))
                elif isinstance(n, ast.AugAssign):
                    bound.update(target_names(n.target))
                elif isinstance(n, ast.Assign):
                    for t in n.targets:
                        bound.update(target_names(t))
                continue
            name = dotted(target)
            if name is None or not statement_start(target):
                continue
            # archctl chỉ đọc giá trị nằm trên cùng dòng với dấu `=`.
            if value.lineno != target.lineno or value.end_lineno != target.lineno:
                self.record(name, None)
                continue
            # `r = ch.queue_declare(queue="jobs")` rồi `r.method.queue`; aio-pika
            # `q = await ch.declare_queue("q")`, `x = await ch.declare_exchange("x")`;
            # kombu `q = Queue("q")`, `x = Exchange("x")`.
            call = value.value if isinstance(value, ast.Await) else value
            field = {"queue_declare": "method.queue", "declare_queue": "@aioq", "declare_exchange": "@aiox",
                     "Queue": "@kq", "Exchange": "@kx"}.get((dotted(call.func) or "").split(".")[-1]) \
                if isinstance(call, ast.Call) else None
            if field:
                q = Py.pick(call, "queue", None)
                q = q if q is not None else Py.pick(call, "name", 0)
                qv = self.one(q, consts=False) if q is not None else None
                self.record(f"{name}.{field}", qv if qv and qv[0] == "fixed" and qv[1] else None)
                # kombu: exchange gắn với queue khai qua biến.
                x = Py.pick(call, "exchange", 1) if field == "@kq" else None
                if x is not None:
                    if isinstance(x, ast.Call) and (dotted(x.func) or "").split(".")[-1] == "Exchange":
                        a = Py.pick(x, "name", 0)
                        ex = self.one(a, consts=False) if a is not None else None
                    else:
                        ex = self.one(x, consts=False)
                        if ex[0] == "dynamic":
                            ex = self.consts.get(f"{dotted(x)}.@kx") if dotted(x) else None
                    self.record(f"{name}.@kqx", ex if ex and ex[0] == "fixed" and ex[1] else None)
            v = self.one(value, consts=False)
            self.record(name, v if v[0] in ("fixed", "env") else None)
        for name in bound:
            self.consts[name] = None
        # Biến gán từ hàm dựng producer, consumer Kafka (đoạn cuối của tên).
        self.clients = {}
        makers = {"KafkaProducer": "send", "AIOKafkaProducer": "send", "Producer": "send",
                  "KafkaConsumer": "receive", "AIOKafkaConsumer": "receive", "Consumer": "receive"}
        for n in ast.walk(tree):
            pairs = []
            if isinstance(n, ast.Assign):
                pairs = [(t, n.value) for t in n.targets]
            elif isinstance(n, ast.AnnAssign) and n.value is not None:
                pairs = [(n.target, n.value)]
            elif isinstance(n, ast.keyword) and n.arg:
                pairs = [(ast.Name(id=n.arg), n.value)]
            for t, v in pairs:
                v = v.value if isinstance(v, ast.Await) else v
                name = dotted(t)
                if name and isinstance(v, ast.Call):
                    role = makers.get((dotted(v.func) or "").split(".")[-1])
                    if role:
                        self.clients[name.split(".")[-1]] = role
        # Biến chắc là client NATS: gán từ `nats.connect(…)`, `nc.jetstream()`.
        self.nats = set()
        for n in ast.walk(tree):
            if isinstance(n, ast.Assign):
                v = n.value.value if isinstance(n.value, ast.Await) else n.value
                if not isinstance(v, ast.Call):
                    continue
                d = dotted(v.func) or ""
                last = d.split(".")[-1]
                if (last == "connect" and d in ("connect", "nats.connect")) or last == "jetstream":
                    for t in n.targets:
                        if dotted(t):
                            self.nats.add(dotted(t).split(".")[-1])
        # Chú thích kiểu: `bus: NATS`, `nc: nats.aio.client.Client` (tham số, gán).
        for n in ast.walk(tree):
            target, ann = None, None
            if isinstance(n, ast.arg) and n.annotation is not None:
                target, ann = n.arg, n.annotation
            elif isinstance(n, ast.AnnAssign):
                target, ann = dotted(n.target), n.annotation
            t = dotted(ann) if ann is not None else None
            if target and t and (t == "NATS" or t.endswith(".NATS") or t.startswith("nats.")):
                self.nats.add(target.split(".")[-1])
        # Biến chắc là client Redis (file import cả NATS lẫn Redis).
        self.redis = set()
        for n in ast.walk(tree):
            if isinstance(n, ast.Assign):
                v = n.value.value if isinstance(n.value, ast.Await) else n.value
                if isinstance(v, ast.Call) and (dotted(v.func) or "").split(".")[-1] in ("Redis", "StrictRedis", "from_url", "pubsub"):
                    for t in n.targets:
                        if dotted(t):
                            self.redis.add(dotted(t).split(".")[-1])

    def nats_call(self, f, m):
        """Phương thức của client NATS: biến chắc là client, hay file không import
        thư viện hàng đợi nào khác; `request` thì luôn phải chắc."""
        shared = any(self.libs[k] for k in ("kafka", "rabbit", "redis", "aio", "kombu"))
        return self.nats_sure(f) or (not shared and m != "request")

    def nats_sure(self, f):
        r = (dotted(f.value) or "").split(".")[-1] if isinstance(f, ast.Attribute) else ""
        return r in ("nc", "js") or "nats" in r.lower() or r in self.nats

    def other_sure(self, f, m):
        """Lời gọi chắc là của hệ khác cùng tên phương thức với NATS."""
        r = (dotted(f.value) or "").split(".")[-1] if isinstance(f, ast.Attribute) else ""
        redis = self.libs["redis"] and (r in ("r", "rdb") or r.lower().startswith("redis") or r in self.redis)
        kafka = self.libs["kafka"] and (self.kafka_client(f, "send") or self.kafka_client(f, "receive"))
        head = dotted(f.value) or ""
        aio = ("fixed", "") if head.endswith("default_exchange") else self.consts.get(f"{head}.@aiox")
        rabbit = m == "publish" and (self.libs["aio"] or self.libs["kombu"]) and (
            (self.libs["aio"] and aio) or (self.libs["kombu"] and self.kafka_client(f, "send")) or not self.libs["redis"])
        return bool(redis or kafka or rabbit)

    def kafka_client(self, attr, role):
        """Biến trước phương thức chắc là client Kafka vai `role`."""
        recv = dotted(attr.value) if isinstance(attr, ast.Attribute) else None
        if not recv:
            return False
        last = recv.split(".")[-1]
        named = ("produc" if role == "send" else "consum") in last.lower()
        return named or self.clients.get(last) == role

    def record(self, name, v):
        if name in self.consts and self.consts[name] != v:
            self.consts[name] = None
        else:
            self.consts.setdefault(name, v)

    def env(self, n):
        if isinstance(n, ast.Subscript) and dotted(n.value) in ("os.environ", "environ"):
            s = n.slice
            if isinstance(s, ast.Constant) and isinstance(s.value, str):
                return s.value
        if isinstance(n, ast.Call) and dotted(n.func) in ("os.getenv", "getenv", "os.environ.get", "environ.get"):
            if n.args and isinstance(n.args[0], ast.Constant) and isinstance(n.args[0].value, str):
                return n.args[0].value
        return None

    def one(self, n, consts=True):
        if isinstance(n, ast.Constant) and isinstance(n.value, (str, bytes)):
            v = n.value.decode("latin-1") if isinstance(n.value, bytes) else n.value
            return ("fixed", v)
        if isinstance(n, ast.JoinedStr):
            if all(isinstance(p, ast.Constant) for p in n.values):
                return ("fixed", "".join(p.value for p in n.values))
            return ("dynamic", "")
        name = dotted(n)
        if consts and name is not None and name in self.consts:
            return self.consts[name] or ("dynamic", "")
        e = self.env(n)
        if e is not None:
            return ("env", e)
        return ("dynamic", "")

    def names(self, n):
        if isinstance(n, (ast.List, ast.Tuple)):
            return [self.one(x) for x in n.elts]
        return [self.one(n)]

    @staticmethod
    def pick(call, key, pos):
        for k in call.keywords:
            if k.arg == key:
                return k.value
        if pos is not None and len(call.args) > pos:
            return call.args[pos]
        return None


def queue_name(v):
    return ("fixed", f"q:{v[1]}") if v[0] == "fixed" else (("env", f"q:{v[1]}") if v[0] == "env" else v)


def exchange_name(v):
    return ("fixed", f"x:{v[1]}") if v[0] == "fixed" else (("env", f"x:{v[1]}") if v[0] == "env" else v)


def subject_like(py, call):
    """Đối số đầu là chuỗi cố định dạng subject NATS (không phải URL, động từ HTTP)."""
    a = Py.pick(call, "subject", 0)
    v = py.one(a) if a is not None else ("dynamic", "")
    return v[0] == "fixed" and "." in v[1] and not any(c in v[1] for c in "/: ") \
        and v[1] not in ("GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS")


def nats_name(v):
    """Subject của bên nhận NATS có đoạn `*`, `>` là mẫu."""
    if v[0] == "fixed" and any(t in ("*", ">") for t in v[1].split(".")):
        return ("pattern", v[1])
    return v


def kombu_ref(py, x, ctor, field):
    """`Queue("q")`/`Exchange("x")` viết tại chỗ, biến gán từ nó, hay chuỗi."""
    if isinstance(x, ast.Call) and (dotted(x.func) or "").split(".")[-1] == ctor:
        a = Py.pick(x, "name", 0)
        return py.one(a) if a is not None else ("dynamic", "")
    v = py.one(x)
    if v[0] == "dynamic" and dotted(x):
        return py.consts.get(f"{dotted(x)}.{field}") or v
    return v


def rabbit_target(ex, key):
    if ex == ("fixed", ""):
        return queue_name(key) if key is not None else ("dynamic", "")
    return exchange_name(ex)


def queue_method(libs, m):
    """Phương thức hàng đợi theo tên khi thư viện được import (tham chiếu không
    gọi, `getattr`)."""
    if m in ("publish", "request") and libs["nats"]:
        return ("nats", "send")
    if m in ("subscribe", "pull_subscribe") and libs["nats"]:
        return ("nats", "receive")
    if m in ("produce", "send", "send_and_wait") and libs["kafka"]:
        return ("kafka", "send")
    if m == "subscribe" and libs["kafka"]:
        return ("kafka", "receive")
    if m in ("subscribe", "psubscribe") and libs["redis"]:
        return ("redis", "receive")
    if m == "publish" and libs["redis"]:
        return ("redis", "send")
    if m == "basic_publish" and libs["rabbit"]:
        return ("rabbitmq", "send")
    if m in ("basic_consume", "queue_bind") and libs["rabbit"]:
        return ("rabbitmq", "receive")
    if m == "send_task" and libs["celery"]:
        return ("celery", "send")
    return None


def python_sites(src):
    """Danh sách `(dòng, hệ, vai, loại, tên)`; None nếu `ast` không đọc được."""
    try:
        with warnings.catch_warnings():
            warnings.simplefilter("ignore")
            tree = ast.parse(src)
    except (SyntaxError, ValueError):
        return None
    py = Py(tree, src)
    out = []

    def push(line, system, role, names, strong):
        for kind, name in names:
            if not strong and kind not in ("fixed", "env"):
                continue
            out.append((line, system, role, kind, name if kind != "dynamic" else ""))

    # Thuộc tính là `func` của lời gọi, vế trái, hay nằm trong chuỗi thuộc tính
    # dài hơn: không phải tham chiếu phương thức đứng một mình.
    not_ref = set()
    for n in ast.walk(tree):
        if isinstance(n, ast.Call):
            not_ref.add(id(n.func))
        if isinstance(n, ast.Attribute):
            not_ref.add(id(n.value))
        if isinstance(n, ast.NamedExpr):
            not_ref.add(id(n.target))
    for n in ast.walk(tree):
        if isinstance(n, ast.Attribute) and id(n) not in not_ref and isinstance(n.ctx, ast.Load):
            d = dotted(n)
            hit = queue_method(py.libs, n.attr) if d else None
            if hit and hit[0] == "kafka" and n.attr in ("send", "send_and_wait", "subscribe") \
                    and not py.kafka_client(n, hit[1]):
                hit = None
            if hit:
                push(n.end_lineno, hit[0], hit[1], [("dynamic", "")], True)
            continue
        if isinstance(n, ast.Call) and (dotted(n.func) or "").split(".")[-1] == "getattr":
            if len(n.args) > 1:
                m = py.one(n.args[1])
                hit = queue_method(py.libs, m[1]) if m[0] == "fixed" else None
                if hit:
                    push(n.func.lineno, hit[0], hit[1], [("dynamic", "")], True)
            continue
        if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
            for d in n.decorator_list:
                call = d if isinstance(d, ast.Call) else None
                name = dotted(call.func if call else d)
                if name is None:
                    continue
                if name == "shared_task" or name.endswith(".shared_task") or name.endswith(".task") \
                        or (name == "task" and py.libs["celery"]):
                    explicit = Py.pick(call, "name", None) if call else None
                    v = py.one(explicit) if explicit is not None else ("task", n.name)
                    push(d.lineno, "celery", "receive", [v], True)
            continue
        if not isinstance(n, ast.Call):
            continue
        f = n.func
        if isinstance(f, ast.Attribute):
            m, method, line = f.attr, True, f.end_lineno
        elif isinstance(f, ast.Name):
            m, method, line = f.id, False, f.lineno
        else:
            continue
        libs = py.libs
        args_n = len(n.args) + len(n.keywords)
        if m in ("publish", "request") and method and libs["nats"] and py.nats_call(f, m):
            a = Py.pick(n, "subject", 0)
            if a is not None:
                push(line, "nats", "send", [py.one(a)], True)
        elif m in ("subscribe", "pull_subscribe") and method and libs["nats"] and py.nats_call(f, m):
            a = Py.pick(n, "subject", 0)
            if a is not None:
                push(line, "nats", "receive", [nats_name(py.one(a))], True)
        elif m in ("publish", "subscribe") and method and libs["nats"] and not py.other_sure(f, m):
            push(line, "nats", "send" if m == "publish" else "receive", [("dynamic", "")], True)
        elif m == "request" and method and libs["nats"] and subject_like(py, n):
            push(line, "nats", "send", [("dynamic", "")], True)
        elif m == "publish" and method and (libs["aio"] or libs["kombu"]) and (
                (libs["aio"] and ((dotted(f.value) or "").endswith("default_exchange")
                                  or py.consts.get(f"{dotted(f.value) or ''}.@aiox")))
                or (libs["kombu"] and py.kafka_client(f, "send")) or not libs["redis"]):
            key = Py.pick(n, "routing_key", 1)
            keyv = py.one(key) if key is not None else None
            head = dotted(f.value) or ""
            aio = ("fixed", "") if head.endswith("default_exchange") else py.consts.get(f"{head}.@aiox")
            if aio and libs["aio"]:
                ex = aio
            elif libs["kombu"]:
                x = Py.pick(n, "exchange", None)
                ex = ("fixed", "") if x is None else kombu_ref(py, x, "Exchange", "@kx")
            else:
                ex = ("dynamic", "")
            push(line, "rabbitmq", "send", [rabbit_target(ex, keyv)], True)
        elif m in ("consume", "iterator", "get") and method and libs["aio"]:
            head = dotted(f.value) or ""
            q = py.consts.get(f"{head}.@aioq")
            if q:
                push(line, "rabbitmq", "receive", [queue_name(q)], True)
            elif m == "consume":
                push(line, "rabbitmq", "receive", [("dynamic", "")], True)
        elif m == "bind" and method and libs["aio"]:
            x = Py.pick(n, "exchange", 0)
            if x is not None:
                v = py.one(x)
                if v[0] == "dynamic":
                    v = py.consts.get(f"{dotted(x)}.@aiox") or v
                push(line, "rabbitmq", "receive", [exchange_name(v)], True)
        elif m == "Consumer" and libs["kombu"]:
            a = Py.pick(n, "queues", 0 if method else 1)
            if a is not None:
                for q in (a.elts if isinstance(a, (ast.List, ast.Tuple)) else [a]):
                    push(line, "rabbitmq", "receive", [queue_name(kombu_ref(py, q, "Queue", "@kq"))], True)
                    if not isinstance(q, ast.Call) and dotted(q) and py.consts.get(f"{dotted(q)}.@kqx"):
                        push(line, "rabbitmq", "receive", [exchange_name(py.consts[f"{dotted(q)}.@kqx"])], True)
                    if isinstance(q, ast.Call) and (dotted(q.func) or "").split(".")[-1] == "Queue":
                        x = Py.pick(q, "exchange", 1)
                        if x is not None:
                            ex = kombu_ref(py, x, "Exchange", "@kx")
                            if ex != ("fixed", ""):
                                push(line, "rabbitmq", "receive", [exchange_name(ex)], True)
        elif m == "send_task" and method:
            a = Py.pick(n, "name", 0)
            if a is not None:
                push(line, "celery", "send", [py.one(a)], libs["celery"])
        elif m == "produce" and method:
            a = Py.pick(n, "topic", 0)
            if a is not None:
                push(line, "kafka", "send", [py.one(a)], libs["kafka"])
        elif m in ("send", "send_and_wait") and method and libs["kafka"]:
            a = Py.pick(n, "topic", 0)
            if a is not None:
                push(line, "kafka", "send", [py.one(a)], py.kafka_client(f, "send"))
        elif m in ("KafkaConsumer", "AIOKafkaConsumer"):
            push(line, "kafka", "receive", [py.one(a) for a in n.args], libs["kafka"])
        elif m == "subscribe" and method:
            first = Py.pick(n, "topics", 0)
            keyed = any(k.arg == "topics" for k in n.keywords)
            pattern = Py.pick(n, "pattern", None)
            if first is not None and (isinstance(first, (ast.List, ast.Tuple)) or keyed):
                if libs["kafka"]:
                    push(line, "kafka", "receive", py.names(first), True)
            elif pattern is not None:
                if libs["kafka"]:
                    push(line, "kafka", "receive", [("dynamic", "")], True)
            elif libs["redis"]:
                names = [py.one(a) for a in n.args] \
                    + [("fixed", k.arg) if k.arg else ("dynamic", "") for k in n.keywords]
                push(line, "redis", "receive", names, True)
        elif m == "psubscribe" and method and libs["redis"]:
            names = [("pattern", v) if k == "fixed" else (k, v) for k, v in (py.one(a) for a in n.args)] \
                + [("pattern", k.arg) if k.arg else ("dynamic", "") for k in n.keywords]
            push(line, "redis", "receive", names, True)
        elif m == "publish" and method and libs["redis"]:
            a = Py.pick(n, "channel", 0)
            if args_n >= 2 and a is not None:
                push(line, "redis", "send", [py.one(a)], True)
        elif m == "basic_publish":
            ex, key = Py.pick(n, "exchange", 0), Py.pick(n, "routing_key", 1)
            if ex is not None:
                push(line, "rabbitmq", "send", [rabbit_target(py.one(ex), py.one(key) if key is not None else None)], True)
        elif m == "basic_consume":
            q = Py.pick(n, "queue", 0)
            if q is not None:
                push(line, "rabbitmq", "receive", [queue_name(py.one(q))], True)
        elif m == "queue_bind":
            x = Py.pick(n, "exchange", 1)
            if x is not None:
                push(line, "rabbitmq", "receive", [exchange_name(py.one(x))], True)
    return out


def python_truth(path):
    try:
        src = path.read_bytes().decode("utf-8", errors="replace")
    except OSError:
        return None
    sites = python_sites(src)
    return None if sites is None else collections.Counter(key(*x) for x in sites)


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
	"strconv"
	"strings"
)

type site struct {
	Line   int    `json:"line"`
	System string `json:"system"`
	Role   string `json:"role"`
	Kind   string `json:"kind"`
	Name   string `json:"name"`
}

type val struct{ kind, name string }

func has(specs []string, prefixes ...string) bool {
	for _, s := range specs {
		for _, p := range prefixes {
			if s == p || strings.HasPrefix(s, p+".") || strings.HasPrefix(s, p+"/") {
				return true
			}
		}
	}
	return false
}

type file struct {
	fs                         *token.FileSet
	kafka, rabbit, redis, nats bool
	natsVars, redisVars        map[string]bool
	consts                     map[string]*val
	out                        []site
}

func (f *file) env(e ast.Expr) (string, bool) {
	c, ok := e.(*ast.CallExpr)
	if !ok || len(c.Args) < 1 {
		return "", false
	}
	sel, ok := c.Fun.(*ast.SelectorExpr)
	if !ok || sel.Sel.Name != "Getenv" {
		return "", false
	}
	if x, ok := sel.X.(*ast.Ident); !ok || x.Name != "os" {
		return "", false
	}
	if b, ok := c.Args[0].(*ast.BasicLit); ok && b.Kind == token.STRING {
		if s, err := strconv.Unquote(b.Value); err == nil {
			return s, true
		}
	}
	return "", false
}

func (f *file) one(e ast.Expr, consts bool) val {
	if u, ok := e.(*ast.UnaryExpr); ok && u.Op == token.AND {
		e = u.X
	}
	switch v := e.(type) {
	case *ast.BasicLit:
		if v.Kind == token.STRING {
			if s, err := strconv.Unquote(v.Value); err == nil {
				return val{"fixed", s}
			}
		}
	case *ast.Ident, *ast.SelectorExpr:
		if consts {
			if c, ok := f.consts[path(e)]; ok {
				if c == nil {
					return val{"dynamic", ""}
				}
				return *c
			}
		}
	}
	if s, ok := f.env(e); ok {
		return val{"env", s}
	}
	return val{"dynamic", ""}
}

func (f *file) names(e ast.Expr) []val {
	if cl, ok := e.(*ast.CompositeLit); ok {
		if _, ok := cl.Type.(*ast.ArrayType); ok {
			out := []val{}
			for _, x := range cl.Elts {
				out = append(out, f.one(x, true))
			}
			return out
		}
	}
	return []val{f.one(e, true)}
}

func (f *file) push(pos token.Pos, system, role string, vals []val, strong bool) {
	line := f.fs.Position(pos).Line
	for _, v := range vals {
		if !strong && v.kind != "fixed" && v.kind != "env" {
			continue
		}
		name := v.name
		if v.kind == "dynamic" {
			name = ""
		}
		f.out = append(f.out, site{line, system, role, v.kind, name})
	}
}

func path(e ast.Expr) string {
	switch v := e.(type) {
	case *ast.Ident:
		return v.Name
	case *ast.SelectorExpr:
		if p := path(v.X); p != "" {
			return p + "." + v.Sel.Name
		}
	}
	return ""
}

func queue(v val) val {
	if v.kind == "fixed" || v.kind == "env" {
		return val{v.kind, "q:" + v.name}
	}
	return v
}

func exchange(v val) val {
	if v.kind == "fixed" || v.kind == "env" {
		return val{v.kind, "x:" + v.name}
	}
	return v
}

func target(ex, key val) val {
	if ex.kind == "fixed" && ex.name == "" {
		return queue(key)
	}
	return exchange(ex)
}

func field(cl *ast.CompositeLit, key string, deep bool) ast.Expr {
	for _, e := range cl.Elts {
		kv, ok := e.(*ast.KeyValueExpr)
		if !ok {
			continue
		}
		if id, ok := kv.Key.(*ast.Ident); ok && id.Name == key {
			return kv.Value
		}
		if deep {
			if inner, ok := kv.Value.(*ast.CompositeLit); ok {
				if v := field(inner, key, true); v != nil {
					return v
				}
			}
			if u, ok := kv.Value.(*ast.UnaryExpr); ok {
				if inner, ok := u.X.(*ast.CompositeLit); ok {
					if v := field(inner, key, true); v != nil {
						return v
					}
				}
			}
		}
	}
	return nil
}

// Biểu thức là context: `ctx`, `context.Background()`, `r.Context()`.
func isCtx(e ast.Expr) bool {
	switch x := e.(type) {
	case *ast.Ident:
		return x.Name == "ctx"
	case *ast.CallExpr:
		if sel, ok := x.Fun.(*ast.SelectorExpr); ok {
			if id, ok := sel.X.(*ast.Ident); ok && id.Name == "context" {
				return true
			}
			return sel.Sel.Name == "Context"
		}
	}
	return false
}

func scan(root, rel string) ([]site, error) {
	fs := token.NewFileSet()
	af, err := parser.ParseFile(fs, root+"/"+rel, nil, 0)
	if err != nil {
		return nil, err
	}
	specs := []string{}
	for _, im := range af.Imports {
		if s, err := strconv.Unquote(im.Path.Value); err == nil {
			specs = append(specs, s)
		}
	}
	f := &file{fs: fs, consts: map[string]*val{}}
	f.kafka = has(specs, "github.com/IBM/sarama", "github.com/Shopify/sarama", "github.com/segmentio/kafka-go", "github.com/confluentinc/confluent-kafka-go")
	f.rabbit = has(specs, "github.com/rabbitmq/amqp091-go", "github.com/streadway/amqp")
	f.redis = has(specs, "github.com/redis/go-redis", "github.com/go-redis/redis")
	f.nats = has(specs, "github.com/nats-io/nats.go")
	f.natsVars = map[string]bool{}
	f.redisVars = map[string]bool{}
	redisType := func(e ast.Expr) bool {
		if st, ok := e.(*ast.StarExpr); ok {
			e = st.X
		}
		se, ok := e.(*ast.SelectorExpr)
		if !ok {
			return false
		}
		id, ok := se.X.(*ast.Ident)
		return ok && id.Name == "redis" && (se.Sel.Name == "Client" || se.Sel.Name == "ClusterClient" || se.Sel.Name == "UniversalClient" || se.Sel.Name == "Ring")
	}
	natsType := func(e ast.Expr) bool {
		if st, ok := e.(*ast.StarExpr); ok {
			e = st.X
		}
		se, ok := e.(*ast.SelectorExpr)
		if !ok {
			return false
		}
		id, ok := se.X.(*ast.Ident)
		if !ok {
			return false
		}
		return (id.Name == "nats" && (se.Sel.Name == "Conn" || se.Sel.Name == "EncodedConn" || se.Sel.Name == "JetStreamContext")) ||
			(id.Name == "jetstream" && se.Sel.Name == "JetStream")
	}
	ast.Inspect(af, func(n ast.Node) bool {
		switch x := n.(type) {
		case *ast.Field:
			if natsType(x.Type) {
				for _, nm := range x.Names {
					f.natsVars[nm.Name] = true
				}
			}
			if redisType(x.Type) && len(x.Names) > 0 {
				f.redisVars[x.Names[len(x.Names)-1].Name] = true
			}
		case *ast.ValueSpec:
			if x.Type != nil && natsType(x.Type) {
				for _, nm := range x.Names {
					f.natsVars[nm.Name] = true
				}
			}
		case *ast.AssignStmt:
			if len(x.Lhs) > 0 && len(x.Rhs) == 1 {
				if c, ok := x.Rhs[0].(*ast.CallExpr); ok {
					if se, ok := c.Fun.(*ast.SelectorExpr); ok {
						q, _ := se.X.(*ast.Ident)
						qn := ""
						if q != nil {
							qn = q.Name
						}
						if (qn == "nats" && se.Sel.Name == "Connect") || se.Sel.Name == "JetStream" || (qn == "jetstream" && se.Sel.Name == "New") {
							if id, ok := x.Lhs[0].(*ast.Ident); ok {
								f.natsVars[id.Name] = true
							}
						}
						if se.Sel.Name == "NewClient" || se.Sel.Name == "NewClusterClient" || se.Sel.Name == "NewUniversalClient" {
							if id, ok := x.Lhs[len(x.Lhs)-1].(*ast.Ident); ok && len(x.Lhs) == 1 {
								f.redisVars[id.Name] = true
							}
						}
					}
				}
			}
		}
		return true
	})
	record := func(name string, e ast.Expr) {
		v := f.one(e, false)
		var p *val
		if v.kind == "fixed" || v.kind == "env" {
			p = &v
		}
		old, seen := f.consts[name]
		if !seen {
			f.consts[name] = p
		} else if (old == nil) != (p == nil) || (old != nil && *old != *p) {
			f.consts[name] = nil
		}
	}
	ast.Inspect(af, func(n ast.Node) bool {
		switch v := n.(type) {
		case *ast.ValueSpec:
			if len(v.Names) == 1 && len(v.Values) == 1 {
				record(v.Names[0].Name, v.Values[0])
			}
		case *ast.AssignStmt:
			if len(v.Lhs) == 1 && len(v.Rhs) == 1 && (v.Tok == token.ASSIGN || v.Tok == token.DEFINE) {
				if id, ok := v.Lhs[0].(*ast.Ident); ok {
					record(id.Name, v.Rhs[0])
				}
			}
			// q, err := ch.QueueDeclare("hello", …) rồi q.Name.
			if len(v.Lhs) == 2 && len(v.Rhs) == 1 {
				id, ok := v.Lhs[0].(*ast.Ident)
				c, ok2 := v.Rhs[0].(*ast.CallExpr)
				if ok && ok2 {
					if sel, ok := c.Fun.(*ast.SelectorExpr); ok && sel.Sel.Name == "QueueDeclare" && len(c.Args) > 0 {
						q := f.one(c.Args[0], false)
						var p *val
						if q.kind == "fixed" && q.name != "" {
							p = &q
						}
						old, seen := f.consts[id.Name+".Name"]
						if !seen {
							f.consts[id.Name+".Name"] = p
						} else if (old == nil) != (p == nil) || (old != nil && *old != *p) {
							f.consts[id.Name+".Name"] = nil
						}
					}
				}
			}
		}
		return true
	})
	// Tên ràng buộc không qua phép gán đơn: tham số, receiver, biến vòng
	// lặp, gán nhiều vế, gán kèm phép toán. Không cố định.
	bound := map[string]bool{}
	fields := func(fl *ast.FieldList) {
		if fl == nil {
			return
		}
		for _, fd := range fl.List {
			for _, nm := range fd.Names {
				bound[nm.Name] = true
			}
		}
	}
	ast.Inspect(af, func(n ast.Node) bool {
		switch v := n.(type) {
		case *ast.FuncDecl:
			fields(v.Recv)
			fields(v.Type.Params)
		case *ast.FuncLit:
			fields(v.Type.Params)
		case *ast.RangeStmt:
			for _, e := range []ast.Expr{v.Key, v.Value} {
				if id, ok := e.(*ast.Ident); ok {
					bound[id.Name] = true
				}
			}
		case *ast.AssignStmt:
			if len(v.Lhs) > 1 || (v.Tok != token.ASSIGN && v.Tok != token.DEFINE) {
				for _, e := range v.Lhs {
					if id, ok := e.(*ast.Ident); ok {
						bound[id.Name] = true
					}
				}
			}
		}
		return true
	})
	for nm := range bound {
		f.consts[nm] = nil
	}
	// Biến gán từ struct Kafka: vai của nó.
	roles := map[string]string{}
	structRole := func(e ast.Expr) string {
		if u, ok := e.(*ast.UnaryExpr); ok && u.Op == token.AND {
			e = u.X
		}
		cl, ok := e.(*ast.CompositeLit)
		if !ok {
			return ""
		}
		sel, ok := cl.Type.(*ast.SelectorExpr)
		if !ok {
			return ""
		}
		switch sel.Sel.Name {
		case "ProducerMessage", "Message", "Writer", "WriterConfig":
			return "send"
		case "ReaderConfig":
			return "receive"
		}
		return ""
	}
	ast.Inspect(af, func(n ast.Node) bool {
		switch v := n.(type) {
		case *ast.AssignStmt:
			if len(v.Lhs) == 1 && len(v.Rhs) == 1 {
				if id, ok := v.Lhs[0].(*ast.Ident); ok {
					if r := structRole(v.Rhs[0]); r != "" {
						roles[id.Name] = r
					}
				}
			}
		case *ast.ValueSpec:
			if len(v.Names) == 1 && len(v.Values) == 1 {
				if r := structRole(v.Values[0]); r != "" {
					roles[v.Names[0].Name] = r
				}
			}
		}
		return true
	})
	ast.Inspect(af, func(n ast.Node) bool {
		switch v := n.(type) {
		case *ast.AssignStmt:
			// msg := &sarama.ProducerMessage{} rồi msg.Topic = "orders"; vai
			// theo struct gán cho biến.
			if f.kafka && v.Tok == token.ASSIGN && len(v.Lhs) == 1 && len(v.Rhs) == 1 {
				if sel, ok := v.Lhs[0].(*ast.SelectorExpr); ok && sel.Sel.Name == "Topic" {
					if x, ok := sel.X.(*ast.Ident); ok {
						if role, ok := roles[x.Name]; ok {
							f.push(sel.Sel.Pos(), "kafka", role, []val{f.one(v.Rhs[0], true)}, true)
						}
					}
				}
			}
		case *ast.CompositeLit:
			sel, ok := v.Type.(*ast.SelectorExpr)
			if !ok || !f.kafka {
				return true
			}
			switch sel.Sel.Name {
			case "ProducerMessage", "Message", "Writer", "WriterConfig":
				if e := field(v, "Topic", sel.Sel.Name == "Message"); e != nil {
					f.push(sel.Sel.Pos(), "kafka", "send", f.names(e), true)
				}
			case "ReaderConfig":
				for _, k := range []string{"Topic", "GroupTopics"} {
					if e := field(v, k, false); e != nil {
						f.push(sel.Sel.Pos(), "kafka", "receive", f.names(e), true)
					}
				}
			}
		case *ast.CallExpr:
			sel, ok := v.Fun.(*ast.SelectorExpr)
			if !ok {
				return true
			}
			a := v.Args
			at := sel.Sel.Pos()
			natsRecv := ""
			if id, ok := sel.X.(*ast.Ident); ok {
				natsRecv = id.Name
			} else if s2, ok := sel.X.(*ast.SelectorExpr); ok {
				natsRecv = s2.Sel.Name
			}
			natsSure := natsRecv == "nc" || natsRecv == "js" || strings.Contains(strings.ToLower(natsRecv), "nats") || f.natsVars[natsRecv]
			natsOK := natsSure || !(f.kafka || f.rabbit || f.redis)
			otherSure := (f.redis && (natsRecv == "r" || natsRecv == "rdb" || strings.HasPrefix(strings.ToLower(natsRecv), "redis") || f.redisVars[natsRecv])) ||
				(f.kafka && (strings.Contains(strings.ToLower(natsRecv), "produc") || strings.Contains(strings.ToLower(natsRecv), "consum"))) ||
				(f.rabbit && sel.Sel.Name == "Publish" && len(a) == 5)
			if f.nats && !natsOK && !otherSure && (sel.Sel.Name == "Publish" || sel.Sel.Name == "Subscribe" || sel.Sel.Name == "Request" || sel.Sel.Name == "RequestWithContext") {
				role := "send"
				if sel.Sel.Name == "Subscribe" {
					role = "receive"
				}
				f.push(at, "nats", role, []val{{kind: "dynamic"}}, true)
				return true
			}
			if f.nats && natsOK {
				// Subject: đối số đầu, hay đối số thứ hai khi đầu là ctx (JetStream mới).
				subj := func() (val, bool) {
					if len(a) == 0 {
						return val{}, false
					}
					if len(a) >= 3 && isCtx(a[0]) {
						return f.one(a[1], true), true
					}
					return f.one(a[0], true), true
				}
				switch sel.Sel.Name {
				case "Publish", "Request", "PublishMsg", "RequestWithContext", "PublishAsync":
					if s, ok := subj(); ok {
						f.push(at, "nats", "send", []val{s}, true)
					}
					return true
				case "Subscribe", "QueueSubscribe", "ChanSubscribe", "ChanQueueSubscribe", "SubscribeSync", "QueueSubscribeSync", "PullSubscribe":
					if s, ok := subj(); ok {
						if s.kind == "fixed" {
							for _, t := range strings.Split(s.name, ".") {
								if t == "*" || t == ">" {
									s.kind = "pattern"
								}
							}
						}
						f.push(at, "nats", "receive", []val{s}, true)
					}
					return true
				}
			}
			switch sel.Sel.Name {
			case "ConsumePartition":
				if len(a) > 0 {
					f.push(at, "kafka", "receive", []val{f.one(a[0], true)}, f.kafka)
				}
			case "SubscribeTopics":
				if f.kafka && len(a) > 0 {
					f.push(at, "kafka", "receive", f.names(a[0]), true)
				}
			case "Consume":
				arr := false
				if len(a) == 3 {
					if cl, ok := a[1].(*ast.CompositeLit); ok {
						_, arr = cl.Type.(*ast.ArrayType)
					}
				}
				if f.kafka && arr {
					f.push(at, "kafka", "receive", f.names(a[1]), true)
				} else if f.rabbit && len(a) >= 6 {
					f.push(at, "rabbitmq", "receive", []val{queue(f.one(a[0], true))}, true)
				}
			case "Subscribe", "PSubscribe":
				if f.redis && len(a) >= 2 {
					vals := []val{}
					for _, x := range a[1:] {
						o := f.one(x, true)
						if o.kind == "fixed" && sel.Sel.Name == "PSubscribe" {
							o.kind = "pattern"
						}
						vals = append(vals, o)
					}
					f.push(at, "redis", "receive", vals, true)
				} else if f.kafka && sel.Sel.Name == "Subscribe" && len(a) == 2 {
					f.push(at, "kafka", "receive", []val{f.one(a[0], true)}, true)
				}
			case "Publish":
				if f.rabbit && len(a) == 5 {
					f.push(at, "rabbitmq", "send", []val{target(f.one(a[0], true), f.one(a[1], true))}, true)
				} else if f.redis && len(a) == 3 {
					f.push(at, "redis", "send", []val{f.one(a[1], true)}, true)
				}
			case "PublishWithContext", "PublishWithDeferredConfirmWithContext":
				if f.rabbit && len(a) >= 3 {
					f.push(at, "rabbitmq", "send", []val{target(f.one(a[1], true), f.one(a[2], true))}, true)
				}
			case "QueueBind":
				if f.rabbit && len(a) >= 3 {
					f.push(at, "rabbitmq", "receive", []val{exchange(f.one(a[2], true))}, true)
				}
			}
		}
		return true
	})
	return f.out, nil
}

func main() {
	in := bufio.NewScanner(os.Stdin)
	in.Buffer(make([]byte, 1<<20), 1<<20)
	w := bufio.NewWriter(os.Stdout)
	defer w.Flush()
	for in.Scan() {
		root, rel, _ := strings.Cut(in.Text(), "\t")
		sites, err := scan(root, rel)
		if err != nil {
			fmt.Fprintf(w, "%s\tERR\n", rel)
			continue
		}
		b, _ := json.Marshal(sites)
		fmt.Fprintf(w, "%s\t%s\n", rel, b)
	}
}
"""


def build_go_helper(workdir, die):
    from pathlib import Path
    import os
    src = Path(workdir) / "queues.go"
    src.write_text(GO_SOURCE)
    exe = Path(workdir) / "queues"
    p = subprocess.run(["go", "build", "-o", str(exe), str(src)], capture_output=True, text=True,
                       timeout=600, env={**os.environ, "GO111MODULE": "off", "GOFLAGS": ""})
    if p.returncode != 0:
        die(f"không dựng được trình đọc hàng đợi Go: {p.stderr.strip()[-300:]}")
    return str(exe)


def go_truth(helper, root, files, die):
    """file → Counter `(dòng, "hệ vai loại:tên")`; None nếu go/parser không đọc được."""
    feed = "".join(f"{root}\t{f}\n" for f in files)
    p = subprocess.run([helper], input=feed, capture_output=True, text=True, timeout=900)
    if p.returncode != 0:
        die(f"trình đọc hàng đợi Go thoát {p.returncode} ở {root}: {p.stderr.strip()[-300:]}")
    out = {}
    for row in p.stdout.splitlines():
        file, _, value = row.partition("\t")
        out[file] = None if value == "ERR" else collections.Counter(
            key(s["line"], s["system"], s["role"], s["kind"], s["name"]) for s in (json.loads(value) or []))
    return out


def have(scanned_file):
    """Counter cùng dạng từ một mục `files` của `scan --json`."""
    return collections.Counter(
        key(q["line"], q["system"], q["role"], q["kind"], q["name"])
        for q in scanned_file.get("queues", {}).get("sites", []))
