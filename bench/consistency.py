"""Phép thử sắc nhất: luật sinh từ mã phải khớp với chính mã đó.

`init --from-code` đọc mã rồi sinh luật mô tả đúng hiện trạng. Nâng mọi thứ
lên `#accepted` rồi `check` lại trên **chính mã đó** thì phải ra 0 vi phạm.
Mỗi vi phạm còn lại là một chỗ bộ khám phá và bộ kiểm chứng nhìn khác nhau —
tức là một lỗi của archctl, không phải của repo.

Cũng phải ra 0 cảnh báo của file luật. `init` khuyên chạy `check --strict`
trong CI, và `--strict` coi cảnh báo của file luật là vi phạm: một luật sinh
ra đã có cảnh báo thì lời khuyên đó đổ ngay ở lần chạy đầu tiên.
"""
import json
import re
import shutil
import statistics
import subprocess
import time
from pathlib import Path

LANG_EXTS = {
    ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".vue",
    ".py", ".pyi", ".pyx", ".pxd", ".pxi", ".ipynb",
    ".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hh", ".hxx", ".inl", ".cu", ".cuh",
    ".rs", ".go",
}
# Thư mục archctl luôn bỏ qua (`ALWAYS_IGNORE` trong `check.rs`) và những tên
# chỉ bỏ khi `.gitignore` bỏ (`OUTPUT_DIRS`). Chép lại ở đây có chủ ý: đây là
# điều archctl *định* làm, còn số file đếm độc lập cho biết nó có làm đúng không.
ALWAYS_IGNORE = {
    ".git", ".arch", "node_modules", "target", ".next", ".nuxt", ".svelte-kit",
    "vendor", "__pycache__", ".venv",
}
OUTPUT_DIRS = {"dist", "build", "out", "coverage"}

# Cạnh liên ngôn ngữ mà luật sinh ra phải có, mỗi họ ghép một cạnh. Đồng nhất
# giữa `init` và `check` không đủ: phần ghép hỏng ở cả hai phía thì vẫn ra 0
# bất nhất, và cạnh biến mất trong im lặng.
CROSSINGS = {
    # cgo: `C.bpf_*` của Go ở gốc sang hàm C trong submodule `libbpf/`.
    "libbpfgo": [("Core", "Libbpf")],
    # PyO3: `from ._pydantic_core import` ↔ `#[pymodule] mod _pydantic_core`.
    "pydantic-core": [("PydanticCore", "Core")],
    # pybind11: `from matplotlib import _path` ↔ `PYBIND11_MODULE(_path, m)`.
    "matplotlib": [("Lib", "Src")],
    # Op PyTorch: `torch.ops._C.x` ↔ `STABLE_TORCH_LIBRARY_FRAGMENT(_C, ops)`.
    "vllm": [("Vllm", "Csrc")],
    # `PYBIND11_MODULE(TORCH_EXTENSION_NAME, m)`, tên lấy từ danh sách nguồn
    # trong `setup.py`: `import fast_bottleneck` ↔ `apex/contrib/csrc/bottleneck/`.
    "apex": [("Bottleneck", "Csrc")],
}

# Lời gọi gRPC: chỗ gọi stub ghép với chỗ đăng ký server theo `package.Service`
# đọc từ `.proto`. Hai phía không import nhau, nên chỉ phép ghép này thấy cạnh.
RPC = {
    # `pb.NewKVClient` của `client/v3` ↔ `pb.RegisterKVServer` của `server/`;
    # gateway trong `api/` (`etcdserverpb.NewKVClient`) cũng gọi server.
    "etcd": [("Client", "Server"), ("Api", "Server")],
    # Online Boutique: client Python `demo_pb2_grpc.ProductCatalogServiceStub`
    # ↔ server Go `pb.RegisterProductCatalogServiceServer`, và ngược lại client
    # Go `pb.NewEmailServiceClient` ↔ server Python `add_EmailServiceServicer_to_server`.
    # Server Node nạp `.proto` lúc chạy: `server.addService(shopProto.CurrencyService.service, …)`
    # ↔ client Go `pb.NewCurrencyServiceClient`.
    "microservices-demo": [("Recommendationservice", "Productcatalogservice"), ("Checkoutservice", "Emailservice"),
                           ("Checkoutservice", "Currencyservice"), ("Checkoutservice", "Paymentservice")],
}


# Tầng hạ tầng phải có ở cả hai phía như cạnh liên ngôn ngữ: `init` sinh ra
# `link` và `runs` cho nó, và gỡ đi thì `check` phải báo. Tên là tên mà `init`
# đặt: service theo tên trong compose, component theo thư mục.
DEPLOY = {
    # `depends_on` của compose, và tên service trong biến môi trường.
    "example-voting-app": {
        # `ports: ["8080:80"]` của vote, result: lộ ra Internet.
        "links": [("Vote", "Redis"), ("Result", "Db"), ("Worker", "Db"), ("Worker", "Redis"),
                  ("Internet", "Vote"), ("Internet", "Result")],
        # `vote/Dockerfile` stage `dev`: `python app.py` trong `vote/`.
        "runs": [("Vote", "Vote")],
    },
    "open-webui": {
        # `OLLAMA_BASE_URL=http://ollama:11434` và `depends_on: ollama`.
        "links": [("OpenWebui", "Ollama")],
        # `CMD ["bash", "start.sh"]` → `uvicorn open_webui.main:app`.
        "runs": [("OpenWebui", "Routers")],
    },
    # Manifest Kubernetes: `PRODUCT_CATALOG_SERVICE_ADDR: "productcatalogservice:3550"`
    # → Service `productcatalogservice` → selector `app: productcatalogservice`.
    "microservices-demo": {
        # `frontend-external` là `LoadBalancer`, cộng VirtualService của Istio: lộ ra Internet.
        "links": [("Checkoutservice", "Productcatalogservice"), ("Frontend", "Cartservice"),
                  ("Recommendationservice", "Productcatalogservice"), ("Cartservice", "RedisCart"),
                  ("Internet", "Frontend")],
        # image `emailservice` → skaffold `context: src/emailservice` →
        # `ENTRYPOINT ["python", "email_server.py"]`; Go dựng bằng `go build`.
        "runs": [("Emailservice", "Emailservice"), ("Checkoutservice", "Checkoutservice")],
        # NetworkPolicy `checkoutservice` chỉ nhận từ `app: frontend`: gỡ link
        # thì vi phạm phải nêu chính policy đó.
        "policy": [("Frontend", "Checkoutservice")],
        # Component `network-policies` chặn mọi đường vào (deny-all) mà không
        # mở đường cho `shoppingassistantservice`, trong khi env của frontend
        # trỏ tới nó: chỗ lệch có thật trong repo.
        "blocked": [("Frontend", "Shoppingassistantservice")],
    },
}


def _run(archctl, args, timeout=300):
    t = time.perf_counter()
    p = subprocess.run([archctl, *args], capture_output=True, text=True, timeout=timeout)
    return p, (time.perf_counter() - t) * 1000


def _median_ms(archctl, args, rounds):
    """Trung vị, bỏ lần đầu vì bộ nhớ đệm của hệ thống file."""
    _run(archctl, args)
    return statistics.median(_run(archctl, args)[1] for _ in range(rounds))


def analyse(archctl: str, name: str, src: Path, work: Path, expected: int,
            rounds: int = 5) -> dict:
    """Trả về một dict kết quả. Khoá `problems` liệt kê mọi thứ làm phép thử đổ.

    `expected` là số file nguồn archctl phải quét, đếm từ git chứ không từ
    archctl — xem `tracked_sources` trong `bench.py`.
    """
    work = work / name
    if work.exists():
        shutil.rmtree(work)
    shutil.copytree(src, work, symlinks=True, ignore=shutil.ignore_patterns(".git"))
    root = ["--root", str(work)]

    out = {"repo": name, "files": expected, "problems": []}

    p, ms = _run(archctl, ["init", "--from-code", "--no-hooks", *root])
    out["init_ms"] = round(ms)
    law = work / ".arch/system.arch"
    if p.returncode != 0 or not law.exists():
        out["problems"].append(f"init --from-code hỏng: {(p.stderr or p.stdout)[:200]}")
        return out
    text = law.read_text()
    out["components"] = len(re.findall(r"^component ", text, re.M))
    out["rules"] = len(re.findall(r"^allow ", text, re.M))

    # Luật sinh từ chính mã này, nên phải tự nhất quán khi có hiệu lực.
    law.write_text(text.replace("#draft", "#accepted"))

    p, _ = _run(archctl, ["check", "--json", *root])
    try:
        rep = json.loads(p.stdout)
    except json.JSONDecodeError:
        out["problems"].append(f"check --json không ra JSON: {(p.stdout or p.stderr)[:200]}")
        return out

    out["scanned"] = rep["filesScanned"]
    out["unmapped"] = rep["filesUnmapped"]
    out["checked"] = len(rep["checked"])
    out["violations"] = len(rep["violations"])
    out["law_warnings"] = len(rep.get("lawWarnings", []))
    out["unresolved"] = len(rep["unresolved"])
    out["missing"] = len(rep.get("missing", []))
    out["vacuous"] = len(rep.get("vacuous", []))

    # 0 vi phạm trên 0 file thì chẳng chứng minh được gì. Không quét được gì
    # là phép thử vô nghĩa, và nó phải đổ chứ không được trông như đạt.
    if out["scanned"] == 0 or out["files"] == 0:
        out["problems"].append(
            f"không có gì để kiểm: archctl quét {out['scanned']} file, đếm độc lập ra {out['files']}"
        )
    # Lệch là archctl bỏ sót mã được commit, hoặc quét thứ nó định bỏ. etcd
    # từng mất cả package `tests/robustness/coverage/` vì tên thư mục trùng
    # tên output, và không có gì nói ra.
    if out["scanned"] != out["files"]:
        out["problems"].append(
            f"số file lệch: archctl quét {out['scanned']}, git có {out['files']} file nguồn"
        )
    # Repo mà các phần không import lẫn nhau (voting app: ba ứng dụng riêng)
    # vẫn có cái để kiểm ở tầng triển khai: `runs` và `link`.
    deploy_rules = re.search(r"^service\s+\w+\s+from\s+[^\n]*?\s+runs\s", text, re.M)
    if out["checked"] == 0 and not deploy_rules:
        out["problems"].append("không component nào vào diện kiểm, 0 vi phạm là vô nghĩa")
    if "lawWarnings" not in rep:
        out["problems"].append("JSON thiếu khoá `lawWarnings`")
    for v in rep["violations"][:6]:
        out["problems"].append(
            f"bất nhất: {v['from']} -> {v['to']} qua {v.get('via')} ở {v['file']}:{v['line']}"
        )
    if out["violations"] > 6:
        out["problems"].append(f"… và {out['violations'] - 6} bất nhất nữa")
    for w in rep.get("lawWarnings", [])[:6]:
        out["problems"].append(f"luật sinh ra đã có cảnh báo: dòng {w['line']}: {w['msg']}")

    crossings(archctl, name, law, root, out)
    crossings(archctl, name, law, root, out, table=RPC, via="rpc", what="cạnh gRPC")
    deploy(archctl, name, law, root, rep, out)

    out["check_ms"] = round(_median_ms(archctl, ["check", *root], rounds))
    one = pick_source(work)
    if one:
        out["one_file_ms"] = round(_median_ms(archctl, ["check", *root, str(one)], rounds))
    return out


def crossings(archctl: str, name: str, law: Path, root: list, out: dict,
              table: dict = CROSSINGS, via: str = "ffi", what: str = "cạnh liên ngôn ngữ"):
    """Cạnh liên ngôn ngữ phải có ở cả hai phía: `init` sinh ra `allow` cho
    nó, và đổi `allow` đó thành `deny` thì `check` phải báo vi phạm qua ranh
    giới ngôn ngữ. `deny` chứ không xoá: component chỉ có đúng một `allow` mà
    mất nó thì ra khỏi diện kiểm, và `check` im lặng vì lý do khác. Cạnh gRPC
    (`RPC`, `via: rpc`) kiểm cùng một cách.
    """
    want = table.get(name, [])
    if not want:
        return
    text = law.read_text()
    edges = set(re.findall(r"^allow\s+(\w+)\s*->\s*(\w+)", text, re.M))
    for a, b in want:
        if (a, b) not in edges:
            out["problems"].append(f"thiếu {what}: luật sinh ra không có {a} -> {b}")
    pattern = "|".join(rf"{a}\s*->\s*{b}\b" for a, b in want)
    law.write_text(re.sub(rf"^allow(\s+(?:{pattern}))", r"deny\1", text, flags=re.M))
    try:
        p, _ = _run(archctl, ["check", "--json", *root])
        seen = {(v["from"], v["to"]) for v in json.loads(p.stdout)["violations"] if v.get("via") == via}
    except (json.JSONDecodeError, KeyError):
        seen = set()
    finally:
        law.write_text(text)
    for a, b in want:
        if (a, b) not in seen:
            out["problems"].append(f"thiếu {what}: đổi allow {a} -> {b} thành deny mà check không báo")


def deploy(archctl: str, name: str, law: Path, root: list, rep: dict, out: dict):
    """Tầng hạ tầng của luật sinh ra phải tự nhất quán, và cạnh bắt buộc của
    repo phải có ở cả hai phía."""
    text = law.read_text()
    out["services"] = len(re.findall(r"^service ", text, re.M))
    out["links"] = sum(len(t.split(",")) for t in re.findall(r"^link\s+\w+\s*->\s*([^@\n]+)", text, re.M))
    for key, what, shown in [
        ("deployUnread", "không đọc được file triển khai", lambda x: f"{x['file']}: {x['why']}"),
        ("entryUnknown", "service có runs mà không rõ điểm vào", lambda x: f"{x['service']}: {x['why']}"),
        ("linksUnseen", "link sinh ra mà không thấy trong cấu hình", lambda x: f"{x['from']} -> {x['to']}"),
        ("linksBlocked", "link sinh ra mà NetworkPolicy chặn", lambda x: f"{x['from']} -> {x['to']}"),
        ("servicesUndeclared", "service hay workload không được sinh", lambda x: f"{x['name']} ({x['file']}:{x['line']})"),
    ]:
        for item in rep.get(key, [])[:4]:
            out["problems"].append(f"{what}: {shown(item)}")
    for m in rep.get("missing", []):
        if m["source"].startswith("service "):
            out["problems"].append(f"{m['source']} `{m['name']}`: {m['hint']}")

    want = DEPLOY.get(name)
    if want and want.get("blocked"):
        # Chỗ lệch đã biết của repo: phải báo đúng nó, không thừa không thiếu.
        expected = {f"link sinh ra mà NetworkPolicy chặn: {a} -> {b}" for a, b in want["blocked"]}
        got = {p for p in out["problems"] if p.startswith("link sinh ra mà NetworkPolicy chặn")}
        out["problems"] = [p for p in out["problems"] if p not in expected]
        for miss in sorted(expected - got):
            out["problems"].append(f"thiếu cảnh báo NetworkPolicy: {miss.split(': ', 1)[1]}")
    if not want:
        return
    links = {(a, b.strip()) for a, bs in re.findall(r"^link\s+(\w+)\s*->\s*([^@\n]+)", text, re.M)
             for b in bs.split(",")}
    # `from` có thể có nhiều nguồn: `from compose:vote, k8s:vote runs Vote`.
    runs = {(s, c.strip()) for s, cs in re.findall(r"^service\s+(\w+)\s+from\s+[^\n]*?\s+runs\s+([^@\n]+)", text, re.M)
            for c in cs.split(",")}
    checks = []
    for a, b in want.get("links", []):
        if (a, b) not in links:
            out["problems"].append(f"thiếu cạnh mạng: luật sinh ra không có link {a} -> {b}")
        else:
            checks.append(("network", a, b))
    for s, c in want.get("runs", []):
        if (s, c) not in runs:
            out["problems"].append(f"thiếu runs: luật sinh ra không có {c} trong runs của service {s}")
        else:
            checks.append(("runs", s, c))
    for a, b in want.get("policy", []):
        if (a, b) not in links:
            out["problems"].append(f"thiếu cạnh mạng: luật sinh ra không có link {a} -> {b}")
        else:
            checks.append(("policy", a, b))
    # Gỡ từng cái đi thì `check` phải báo đúng loại vi phạm đó.
    for kind, a, b in checks:
        if kind in ("network", "policy"):
            pattern = rf"^(link\s+{a}\s*->\s*)([^@\n]+)"
            broken = re.sub(pattern, lambda m: m.group(1) + ", ".join(
                x.strip() for x in m.group(2).split(",") if x.strip() != b) + " ", text, flags=re.M)
            broken = re.sub(rf"^link\s+{a}\s*->\s*@", "#", broken, flags=re.M)
        else:
            pattern = rf"^(service\s+{a}\s+from\s+[^\n]*?\s+runs\s+)([^@\n]+)"
            # Gỡ hết thì service mất `runs` và không còn gì để kiểm: khi đó thay
            # bằng một component khác để `runs` vẫn có hiệu lực.
            others = [c for c in re.findall(r"^component\s+(\w+)", text, re.M) if c != b]
            def drop(m):
                rest = [x.strip() for x in m.group(2).split(",") if x.strip() != b] or others[:1]
                return m.group(1) + ", ".join(rest) + " "
            broken = re.sub(pattern, drop, text, flags=re.M)
        law.write_text(broken)
        try:
            p, _ = _run(archctl, ["check", "--json", *root])
            vs = json.loads(p.stdout)["violations"]
            seen = {(v["from"], v["to"], v.get("reason")) for v in vs}
            # Vi phạm do NetworkPolicy cho phép: nêu tên policy trong `spec`.
            seen |= {(v["from"], v["to"], "policy") for v in vs if v.get("spec", "").startswith("NetworkPolicy")}
        except (json.JSONDecodeError, KeyError):
            seen = set()
        finally:
            law.write_text(text)
        reason = {"network": "not_linked", "runs": "not_run", "policy": "policy"}[kind]
        if (a, b, reason) not in seen:
            what = f"{b} khỏi runs của {a}" if kind == "runs" else f"link {a} -> {b}"
            out["problems"].append(f"gỡ {what} mà check không báo {reason}")


def pick_source(work: Path):
    """Một file nguồn thật nằm trong `work`, để đo hoặc để sửa.

    Không nhận symlink: repo tải về là mã người khác, và `flows` ghi thêm vào
    file này — một `src/x.rs` trỏ tới `~/.zshrc` thì bộ đo sẽ sửa file ngoài
    thư mục làm việc.
    """
    base = work.resolve()
    for ext in (".rs", ".py", ".ts", ".go", ".c"):
        for p in work.rglob(f"*{ext}"):
            if p.is_symlink() or not p.is_file():
                continue
            if {".git", "target", "node_modules"} & set(p.relative_to(work).parts):
                continue
            if base in p.resolve().parents:
                return p
    return None
