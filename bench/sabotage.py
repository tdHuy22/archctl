#!/usr/bin/env python3
"""Kiểm rằng bộ đo **đổ được**.

    python3 bench/sabotage.py [--archctl <binary>] [--repo flask]

Dựng một archctl giả: gọi archctl thật rồi làm hỏng đúng một chỗ trong đầu ra.
Mỗi kiểu phá phải làm `bench.py` thoát 1; không phá gì thì phải thoát 0. Một
phép kiểm không thể đổ thì tệ hơn không có, vì nó vẫn cộng vào con số "đạt" —
và bộ đo này từng có bốn phép kiểm như vậy.

`oracle.py` cũng vậy: ba kiểu phá đầu ra `scan --json` (mất một import, lệch
một dòng, thêm một import giả) và hai kiểu phá tên máy trong chuỗi của file
Python (mất một, thêm một giả) phải làm nó thoát 1. `--repo` phải có file
Python, vì phần đối chiếu Python không cần công cụ nào ngoài chính Python.

Kiểu `crossing` chạy trên `--crossing-repo` (mặc định pydantic-core): xoá cạnh
liên ngôn ngữ khỏi cả luật `init` sinh ra lẫn vi phạm `check` báo, tức là phần
ghép hỏng ở cả hai phía cùng lúc. Phép kiểm đồng nhất không thấy gì; chỉ danh
sách cạnh bắt buộc trong `consistency.CROSSINGS` bắt được.

Bốn kiểu nữa phá phần cạnh binding, mỗi nhánh của thước đo một kiểu, trên repo
có đúng loại đó (`BINDING_ORACLE_RUNS`): module pybind11 phía C++ của
matplotlib (mất, thêm giả), lời gọi `torch.ops` phía Python của
pytorch-image-models, op `TORCH_LIBRARY` phía C++ của vllm. Cần clang.

`--notebook-repo matplotlib` phá thêm hai kiểu ở notebook (mất import, lệch
dòng), đối chiếu bằng IPython cài vào venv nên cần mạng; CI bật nó.

Năm kiểu phá tầng hạ tầng chạy trên `--deploy-bench-repo` (mặc định
example-voting-app), mỗi kiểu nhắm một phép kiểm trong `consistency.deploy`:
`init` mất mọi `link`; `check` giấu vi phạm cạnh mạng; `check` giấu vi phạm
`runs`; `check` báo thêm một file triển khai không đọc được; `check` giấu vi
phạm do tên máy viết trong mã.

`--deploy-repo example-voting-app` phá thêm bốn kiểu ở dữ kiện compose và
Dockerfile mà `scan --json` in ra (mất một `depends_on`, lệch dòng một biến
môi trường, thêm một tên máy giả, sửa một chỉ thị Dockerfile), đối chiếu bằng
PyYAML và dockerfile-parse cài vào venv nên cần mạng; CI bật nó.

Ba kiểu `deploy-gen`, `deploy-link`, `deploy-runs` chạy lại trên
`--k8s-bench-repo` (mặc định microservices-demo), nơi `link` và `runs` đến từ
manifest Kubernetes chứ không từ compose; thêm hai kiểu ở NetworkPolicy (`check`
giấu vi phạm do policy cho phép, giấu cảnh báo `link` bị chặn). `--k8s-repo microservices-demo` phá
thêm bốn kiểu ở dữ kiện manifest và cạnh qua Service mà `scan --json` in ra
(mất một biến môi trường, lệch dòng một workload, mất một cạnh, đổi đích một
cạnh), cộng hai kiểu ở NetworkPolicy (mất một peer, đổi một workload từ bị cô
lập thành không), đối chiếu bằng `k8s_truth.py` trên PyYAML; cần mạng như
`--deploy-repo`.

`--tf-repo terraform-docs-samples` phá bốn kiểu ở service Terraform của `scan
--json` (mất một cạnh, thêm cạnh giả, đảo lối vào, đổi image), đối chiếu bằng
`tf_truth.py` trên plan của OpenTofu; cần mạng (tải OpenTofu, provider).

`--proto-repo vllm` phá thêm hai kiểu ở dữ kiện `.proto` (mất một `rpc`, lệch
dòng một `service`), đối chiếu bằng `protoc` của grpcio-tools cài vào venv nên
cần mạng; CI bật nó. Kiểu `rpc` chạy trên `--rpc-repo` (mặc định etcd): xoá cạnh
gRPC bắt buộc khỏi cả luật `init` sinh ra lẫn vi phạm `check` báo. Kiểu `queue`
làm vậy với cạnh qua hàng đợi trên `--queue-repo` (mặc định
rabbitmq-tutorials); ba kiểu `queue-*` phá mục `queues` của `scan --json` ở
file Python (mất, lệch dòng, thêm giả), đối chiếu bằng `queue_truth.py`.
`--rust-repo ripgrep` phá thêm ba kiểu như vậy ở file Rust, đối chiếu bằng
`oracle.py --require syn` — tức là tải crate, nên chỉ CI bật. Phần Rust miễn
trừ import nằm trong thân macro; kiểu `rust-extra` đặt import giả ở ngoài macro
và đòi nó vẫn bị báo thừa, để một lần sửa làm phép miễn trừ rộng ra (nuốt cả
file) đỏ ở đây. `--rust-repo` phải có import Rust đầu tiên nằm ngoài macro.
"""
import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent

# kiểu phá → phép kiểm nào phải bắt được nó
MODES = {
    "violation": "bất nhất",
    "lawwarn": "luật sinh ra đã có cảnh báo",
    "empty": "không có gì để kiểm",
    "count": "số file lệch",
    "nokey": "thiếu khoá `lawWarnings`",
    "hash": "hash đổi sau khi luật đổi",
    "diff": "diff: thêm chú thích",
}

# kiểu phá chạy trên `--crossing-repo` → phép kiểm phải bắt được nó
CROSSING_MODES = {"crossing": "thiếu cạnh liên ngôn ngữ"}
# kiểu phá chạy trên `--rpc-repo`: cạnh gRPC bắt buộc mất ở cả hai phía
RPC_MODES = {"rpc": "thiếu cạnh gRPC"}
# kiểu phá chạy trên `--queue-repo`: cạnh qua hàng đợi bắt buộc mất ở cả hai phía
QUEUE_MODES = {"queue": "thiếu cạnh qua hàng đợi"}
# phá mục `queues` của `scan --json` ở file Python đầu tiên có chỗ gửi, nhận
QUEUE_ORACLE_MODES = {
    "queue-drop": "thiếu:",
    "queue-line": "lệch dòng:",
    "queue-extra": "thừa:",
}

# kiểu phá đầu ra `scan --json` → dòng `oracle.py` phải in ra
ORACLE_MODES = {
    "scan-drop": "thiếu:",
    "scan-line": "lệch dòng:",
    "scan-extra": "thừa:",
    # tên máy viết thẳng trong chuỗi của file Python
    "host-drop": "thiếu:",
    "host-extra": "thừa:",
}
RUST_ORACLE_MODES = {
    "rust-drop": "thiếu:",
    "rust-line": "lệch dòng:",
    "rust-extra": "thừa:",
}
NOTEBOOK_ORACLE_MODES = {
    "nb-drop": "thiếu:",
    "nb-line": "lệch dòng:",
}
# phá phần cạnh binding của `scan --json`, mỗi nhánh của thước đo một kiểu, trên
# repo có đúng loại đó: (kiểu, repo, dòng `oracle.py` phải in)
BINDING_ORACLE_RUNS = [
    ("bind-drop", "matplotlib", "thiếu:"),            # module pybind11, phía C++
    ("bind-extra", "matplotlib", "thừa:"),
    ("bind-call", "pytorch-image-models", "thiếu:"),  # `torch.ops` phía Python
    ("bind-op", "vllm", "thiếu:"),                    # op trong `TORCH_LIBRARY`
]
BINDING_ORACLE_MODES = {mode for mode, _, _ in BINDING_ORACLE_RUNS}
# phá tầng hạ tầng ở bộ đo → phép kiểm phải bắt được nó
DEPLOY_MODES = {
    "deploy-gen": "thiếu cạnh mạng",
    "deploy-link": "mà check không báo not_linked",
    "deploy-runs": "mà check không báo not_run",
    "deploy-warn": "không đọc được file triển khai",
    # tên máy viết thẳng trong mã: bỏ vi phạm nằm ở file mã
    "deploy-code": "mà check không báo tên máy",
}
# phá dữ kiện hạ tầng trong `scan --json` → dòng `oracle.py` phải in ra
DEPLOY_ORACLE_MODES = {
    "dep-drop": "thiếu:",
    "dep-line": "lệch dòng:",
    "dep-extra": "thừa:",
    "dep-docker": "thiếu:",
}

# kiểu phá tầng hạ tầng chạy lại trên repo có manifest Kubernetes
K8S_BENCH_MODES = {m: DEPLOY_MODES[m] for m in ("deploy-gen", "deploy-link", "deploy-runs")}
# NetworkPolicy, chỉ có ở repo manifest Kubernetes
K8S_BENCH_MODES |= {
    "deploy-policy": "mà check không báo policy",
    "deploy-blocked": "thiếu cảnh báo NetworkPolicy",
    # cạnh gRPC khác ngôn ngữ: Python ↔ Go, Go → Node
    "rpc": RPC_MODES["rpc"],
    # lối vào từ Internet
    "deploy-internet": "gỡ link Internet",
    # cạnh từ manifest dựng từ chart Helm
    "deploy-helm": "mà check không báo ở chart",
}
# phá dữ kiện manifest và cạnh qua Service trong `scan --json` → dòng `oracle.py` phải in ra
K8S_ORACLE_MODES = {
    "k8s-env": "thiếu:",
    "k8s-line": "lệch dòng:",
    "k8s-edge": "thiếu cạnh:",
    "k8s-target": "thừa cạnh:",
    "k8s-policy": "thiếu:",
    "k8s-guard": "lệch NetworkPolicy",
    "k8s-exposed": "thiếu lối vào từ Internet",
    # manifest dựng từ chart Helm: mất một biến môi trường
    "helm-drop": "thiếu:",
    # bản dựng Kustomize: mất một biến môi trường, thêm một image giả
    "kustomize-drop": "thiếu:",
    "kustomize-extra": "thừa:",
    # bản dựng Kustomize dựng được mà archctl báo "chưa đọc được"
    "kustomize-unread": "archctl không dựng mà kustomize dựng được",
}

# phá service Terraform trong `scan --json` → dòng `oracle.py` phải in ra
TF_ORACLE_MODES = {
    "tf-edge": "thiếu cạnh",
    "tf-extra": "thừa cạnh",
    "tf-exposed": "lệch lối vào từ Internet",
    "tf-image": "lệch image",
}

# phá dữ kiện `.proto` trong `scan --json` → dòng `oracle.py` phải in ra
PROTO_ORACLE_MODES = {
    "proto-drop": "thiếu:",
    "proto-line": "lệch dòng:",
}


def sabotage_tf(rep: dict, mode: str):
    """Làm hỏng **mọi** service Terraform: mất một cạnh, thêm cạnh giả, đảo
    lối vào, đổi image. Module gốc nào OpenTofu không plan được thì không được
    đo, nên phá một service có thể rơi đúng vào chỗ đó."""
    tf = [s for s in rep.get("deploy", {}).get("services", []) if s["key"].startswith("tf:")]
    for s in tf:
        if mode == "tf-edge" and s["edges"]:
            s["edges"].pop(0)
        if mode == "tf-extra" and s.get("image"):
            other = next((x for x in tf if x is not s and x["key"] not in {e["target"] for e in s["edges"]}), None)
            if other:
                s["edges"].append({**(s["edges"][0] if s["edges"] else {"file": s["file"], "line": s["line"], "shown": "X"}),
                                   "target": other["key"]})
        if mode == "tf-exposed" and s.get("image"):
            s["exposed"] = [] if s.get("exposed") else [{"file": s["file"], "line": s["line"], "shown": "phá thử"}]
        if mode == "tf-image" and s.get("image"):
            s["image"] = "khong-co-that"


def sabotage_protos(rep: dict, mode: str):
    """Mất `rpc` đầu tiên, hay lệch dòng `service` đầu tiên, của **mọi** file
    `.proto`: file nào `protoc` không dựng được thì oracle bỏ qua, nên phá một
    file có thể rơi đúng vào chỗ không ai đo."""
    for f in rep.get("protos", []):
        for svc in f.get("services", []):
            if mode == "proto-line":
                svc["line"] += 1
                break
            if svc["rpcs"]:
                svc["rpcs"].pop(0)
                break


def sabotage_deploy(rep: dict, mode: str):
    """Làm hỏng dữ kiện hạ tầng đầu tiên đúng loại trong `scan --json`."""
    files = rep.get("deploy", {}).get("files", [])
    if mode == "dep-docker":
        for f in files:
            ins = f.get("instructions") if f.get("kind") == "dockerfile" else None
            # `scan` chỉ in đối số với `--verbatim` (TM-10), và đó là lần gọi mà
            # oracle đối chiếu; lần gọi khác không có gì để phá.
            if ins and "args" in ins[0]:
                ins[0]["args"] += " khong_co_that"
                return
        return
    for f in files:
        for s in f.get("services", []) if f.get("kind") == "compose" else []:
            if mode == "dep-drop" and s["dependsOn"]:
                s["dependsOn"].pop(0)
                return
            if mode in ("dep-line", "dep-extra") and s["env"]:
                if mode == "dep-line":
                    s["env"][0]["line"] += 1
                else:
                    s["env"].append({**s["env"][0], "key": "KHONG_CO_THAT_HOST"})
                return


def sabotage_k8s(rep: dict, mode: str):
    """Làm hỏng dữ kiện manifest đầu tiên đúng loại, hay cạnh qua Service đầu
    tiên, trong `scan --json`."""
    dep = rep.get("deploy", {})
    if mode == "kustomize-unread":
        if dep.get("kustomize"):
            b = dep["kustomize"].pop(0)
            dep.setdefault("unread", []).append(
                {"file": f"{b['root'] or '.'}/kustomization.yaml",
                 "why": f"Kustomize không dựng được bản dựng `{b['root'] or '.'}`: phá thử"})
        return
    if mode in ("kustomize-drop", "kustomize-extra"):
        for b in dep.get("kustomize", []):
            for d in b["docs"]:
                for w in d["facts"].get("workloads", []):
                    for c in w["containers"]:
                        if mode == "kustomize-extra":
                            c["image"] = "khong-co-that"
                            return
                        if c["env"]:
                            c["env"].pop(0)
                            return
        return
    if mode == "helm-drop":
        for f in dep.get("files", []):
            for w in f.get("workloads", []) if f.get("rendered") else []:
                for c in w["containers"]:
                    if c["env"]:
                        c["env"].pop(0)
                        return
        return
    if mode == "k8s-exposed":
        for s in dep.get("services", []):
            if s["key"].startswith("k8s:") and s.get("exposed"):
                s["exposed"] = []
                return
        return
    if mode == "k8s-guard":
        for s in dep.get("services", []):
            if s["key"].startswith("k8s:") and s.get("ingress", {}).get("isolated"):
                s["ingress"]["isolated"] = False
                return
        return
    if mode == "k8s-policy":
        for f in dep.get("files", []):
            for pol in f.get("policies", []) if f.get("kind") == "k8s" else []:
                for rule in pol.get("ingress") or []:
                    if rule["peers"]:
                        rule["peers"].pop(0)
                        return
        return
    if mode in ("k8s-edge", "k8s-target"):
        keys = [s["key"] for s in dep.get("services", []) if s["key"].startswith("k8s:")]
        for s in dep.get("services", []):
            if s["key"].startswith("k8s:") and s["edges"]:
                if mode == "k8s-edge":
                    s["edges"].pop(0)
                else:
                    e = s["edges"][0]
                    e["target"] = next(k for k in keys if k not in (e["target"], s["key"]))
                return
        return
    for f in dep.get("files", []):
        for w in f.get("workloads", []) if f.get("kind") == "k8s" else []:
            if mode == "k8s-line":
                w["line"] += 1
                return
            for c in w["containers"]:
                if c["env"]:
                    c["env"].pop(0)
                    return


def sabotage_crossings(rep: dict, mode: str):
    """Làm hỏng đầu cạnh binding đầu tiên đúng loại: module (`py:`) hay op
    (`op:`) phía C/C++, lời gọi `op:` phía Python."""
    lang, side, prefix = {
        "bind-drop": ("C/C++", "provides", ("py:", "op:")),
        "bind-extra": ("C/C++", "provides", ("py:", "op:")),
        "bind-call": ("Python", "calls", ("op:",)),
        "bind-op": ("C/C++", "provides", ("op:",)),
    }[mode]
    for f in rep["files"]:
        entries = f.get("crossings", {}).get(side, [])
        first = next((c for c in entries if c["key"].startswith(prefix)), None)
        if f["lang"] == lang and first:
            if mode == "bind-extra":
                entries.append({**first, "key": "py:khong_co_that"})
            else:
                entries.remove(first)
            return


def sabotage_scan(rep: dict, mode: str):
    """Làm hỏng import đầu tiên của file đầu tiên có import: Python, Rust hay notebook tuỳ kiểu."""
    if mode.startswith("host-"):
        for f in rep["files"]:
            found = f.get("hosts", {}).get("found", [])
            if f["file"].endswith(".py") and found:
                if mode == "host-drop":
                    found.pop(0)
                else:
                    found.append([found[0][0], "khong-co-that", None, None])
                return
        return
    prefix, ext = next((p, e) for p, e in [("rust-", ".rs"), ("nb-", ".ipynb"), ("scan-", ".py")]
                       if mode.startswith(p))
    kind = mode[len(prefix):]
    for f in rep["files"]:
        if f["file"].endswith(ext) and f["imports"]:
            imports = f["imports"]
            if kind == "drop":
                imports.pop(0)
            elif kind == "line":
                imports[0]["line"] += 1
            elif kind == "extra":
                imports.append({**imports[0], "spec": "khong_co_that"})
            return


def sabotage_queues(rep: dict, mode: str):
    """Làm hỏng chỗ gửi, nhận đầu tiên của file Python đầu tiên có nó."""
    for f in rep["files"]:
        sites = f.get("queues", {}).get("sites", [])
        if f["file"].endswith(".py") and sites:
            if mode == "queue-drop":
                sites.pop(0)
            elif mode == "queue-line":
                sites[0]["line"] += 1
            else:
                sites.append({**sites[0], "name": "khong-co-that", "kind": "fixed"})
            return


def crossing_edges(table: str = "CROSSINGS") -> set:
    sys.path.insert(0, str(HERE))
    import consistency
    return {edge for edges in getattr(consistency, table).values() for edge in edges}


def drop_crossings_from_law(args: list[str], table: str = "CROSSINGS"):
    """Luật `init` sinh ra mất mọi cạnh bắt buộc của bảng `table`."""
    root = Path(args[args.index("--root") + 1]) if "--root" in args else Path.cwd()
    law = root / ".arch/system.arch"
    if not law.exists():
        return
    keep = [
        line for line in law.read_text().splitlines(keepends=True)
        if not any(re.match(rf"allow\s+{a}\s*->\s*{b}\b", line) for a, b in crossing_edges(table))
    ]
    law.write_text("".join(keep))


def act_as_archctl(real: str, mode: str, args: list[str]) -> int:
    p = subprocess.run([real, *args], capture_output=True, text=True)
    out = p.stdout
    if mode == "crossing" and args[:1] == ["init"] and p.returncode == 0:
        drop_crossings_from_law(args)
    if mode == "rpc" and args[:1] == ["init"] and p.returncode == 0:
        drop_crossings_from_law(args, "RPC")
    if mode == "queue" and args[:1] == ["init"] and p.returncode == 0:
        drop_crossings_from_law(args, "QUEUE")
    if mode == "deploy-gen" and args[:1] == ["init"] and p.returncode == 0:
        root = Path(args[args.index("--root") + 1]) if "--root" in args else Path.cwd()
        law = root / ".arch/system.arch"
        if law.exists():
            law.write_text("".join(l for l in law.read_text().splitlines(keepends=True)
                                   if not l.startswith("link ")))
    if args[:1] == ["check"] and "--json" in args and p.stdout.startswith("{"):
        rep = json.loads(out)
        if mode in ("crossing", "rpc", "queue"):
            via, edges = {
                "crossing": ("ffi", crossing_edges()),
                "rpc": ("rpc", crossing_edges("RPC")),
                "queue": ("queue", crossing_edges("QUEUE")),
            }[mode]
            rep["violations"] = [
                v for v in rep["violations"]
                if not (v.get("via") == via and (v["from"], v["to"]) in edges)
            ]
        elif mode == "violation":
            rep["violations"].append(
                {"from": "A", "to": "B", "file": "x.rs", "line": 1, "via": "import"}
            )
        elif mode == "lawwarn":
            rep["lawWarnings"].append({"line": 3, "msg": "giả", "hint": None})
        elif mode == "empty":
            rep["filesScanned"] = 0
        elif mode == "count":
            rep["filesScanned"] -= 1
        elif mode == "nokey":
            rep.pop("lawWarnings", None)
        elif mode == "hash":
            rep["archHash"] = "deadbeef"
        elif mode == "deploy-link":
            rep["violations"] = [v for v in rep["violations"] if v.get("reason") != "not_linked"]
        elif mode == "deploy-runs":
            rep["violations"] = [v for v in rep["violations"] if v.get("reason") != "not_run"]
        elif mode == "deploy-warn":
            rep.setdefault("deployUnread", []).append({"file": "docker-compose.yml", "why": "giả"})
        elif mode == "deploy-policy":
            rep["violations"] = [v for v in rep["violations"] if not v.get("spec", "").startswith("NetworkPolicy")]
        elif mode == "deploy-blocked":
            rep["linksBlocked"] = []
        elif mode == "deploy-internet":
            rep["violations"] = [v for v in rep["violations"] if v["from"] != "Internet"]
        elif mode == "deploy-helm":
            rep["violations"] = [v for v in rep["violations"] if "/templates/" not in v["file"]]
        elif mode == "deploy-code":
            rep["violations"] = [v for v in rep["violations"]
                                 if not (v.get("reason") == "not_linked" and v["file"].endswith((".py", ".js")))]
        out = json.dumps(rep)
    elif mode == "diff" and args[:1] == ["diff"]:
        out = "kiến trúc a → b\n"
    elif (mode in ORACLE_MODES or mode in RUST_ORACLE_MODES or mode in NOTEBOOK_ORACLE_MODES) \
            and args[:1] == ["scan"] and "--json" in args:
        rep = json.loads(out)
        sabotage_scan(rep, mode)
        out = json.dumps(rep)
    elif mode in BINDING_ORACLE_MODES and args[:1] == ["scan"] and "--json" in args:
        rep = json.loads(out)
        sabotage_crossings(rep, mode)
        out = json.dumps(rep)
    elif mode in DEPLOY_ORACLE_MODES and args[:1] == ["scan"] and "--json" in args:
        rep = json.loads(out)
        sabotage_deploy(rep, mode)
        out = json.dumps(rep)
    elif mode in K8S_ORACLE_MODES and args[:1] == ["scan"] and "--json" in args:
        rep = json.loads(out)
        sabotage_k8s(rep, mode)
        out = json.dumps(rep)
    elif mode in QUEUE_ORACLE_MODES and args[:1] == ["scan"] and "--json" in args:
        rep = json.loads(out)
        sabotage_queues(rep, mode)
        out = json.dumps(rep)
    elif mode in TF_ORACLE_MODES and args[:1] == ["scan"] and "--json" in args:
        rep = json.loads(out)
        sabotage_tf(rep, mode)
        out = json.dumps(rep)
    elif mode in PROTO_ORACLE_MODES and args[:1] == ["scan"] and "--json" in args:
        rep = json.loads(out)
        sabotage_protos(rep, mode)
        out = json.dumps(rep)
    sys.stdout.write(out)
    sys.stderr.write(p.stderr)
    return p.returncode


def main():
    if len(sys.argv) > 1 and sys.argv[1] == "--as-archctl":
        sys.exit(act_as_archctl(os.environ["SABOTAGE_REAL"], os.environ.get("SABOTAGE", ""),
                                sys.argv[2:]))

    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--archctl", default=str(ROOT / "target/release/archctl"))
    ap.add_argument("--repo", default="flask")
    ap.add_argument("--rust-repo", help="phá cả phần Rust của oracle.py trên repo này (tải crate `syn`)")
    ap.add_argument("--notebook-repo",
                    help="phá cả phần notebook của oracle.py trên repo này (cài IPython vào venv)")
    ap.add_argument("--crossing-repo", default="pydantic-core",
                    help="repo có cạnh liên ngôn ngữ bắt buộc, cho kiểu phá `crossing`")
    ap.add_argument("--rpc-repo", default="etcd",
                    help="repo có cạnh gRPC bắt buộc, cho kiểu phá `rpc`")
    ap.add_argument("--queue-repo", default="rabbitmq-tutorials",
                    help="repo có cạnh qua hàng đợi bắt buộc, cho kiểu phá `queue` và `queue-*`")
    ap.add_argument("--deploy-bench-repo", default="example-voting-app",
                    help="repo có compose và `runs`, `link` bắt buộc, cho các kiểu phá `deploy-*`")
    ap.add_argument("--k8s-bench-repo", default="microservices-demo",
                    help="repo có manifest Kubernetes và `runs`, `link` bắt buộc, cho kiểu phá `deploy-*` lần hai")
    ap.add_argument("--k8s-repo",
                    help="phá cả phần manifest Kubernetes của oracle.py trên repo này (cài PyYAML vào venv)")
    ap.add_argument("--tf-repo",
                    help="phá cả phần Terraform của oracle.py trên repo này (tải OpenTofu và provider ghim)")
    ap.add_argument("--proto-repo",
                    help="phá cả phần `.proto` của oracle.py trên repo này (cài grpcio-tools vào venv)")
    ap.add_argument("--deploy-repo",
                    help="phá cả phần hạ tầng của oracle.py trên repo này (cài PyYAML vào venv)")
    args = ap.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        # Gọi qua một file shell thay vì đặt bit chạy được cho file này.
        fake = Path(tmp) / "archctl"
        fake.write_text(
            f'#!/bin/sh\nexec "{sys.executable}" "{Path(__file__).resolve()}" --as-archctl "$@"\n'
        )
        fake.chmod(0o755)

        bad = 0
        runs = [("bench.py", args.repo, m, e) for m, e in [("", None), *MODES.items()]]
        runs += [("bench.py", args.crossing_repo, m, e)
                 for m, e in [("", None), *CROSSING_MODES.items()]]
        runs += [("bench.py", args.rpc_repo, m, e)
                 for m, e in [("", None), *RPC_MODES.items()]]
        runs += [("bench.py", args.queue_repo, m, e)
                 for m, e in [("", None), *QUEUE_MODES.items()]]
        runs += [("oracle.py", args.queue_repo, m, e)
                 for m, e in [("", None), *QUEUE_ORACLE_MODES.items()]]
        runs += [("bench.py", args.deploy_bench_repo, m, e)
                 for m, e in [("", None), *DEPLOY_MODES.items()]]
        runs += [("bench.py", args.k8s_bench_repo, m, e)
                 for m, e in [("", None), *K8S_BENCH_MODES.items()]]
        runs += [("oracle.py", args.repo, m, e) for m, e in [("", None), *ORACLE_MODES.items()]]
        if args.k8s_repo:
            runs += [("oracle.py", args.k8s_repo, m, e)
                     for m, e in [("", None), *K8S_ORACLE_MODES.items()]]
        if args.deploy_repo:
            runs += [("oracle.py", args.deploy_repo, m, e)
                     for m, e in [("", None), *DEPLOY_ORACLE_MODES.items()]]
        if args.proto_repo:
            runs += [("oracle.py", args.proto_repo, m, e)
                     for m, e in [("", None), *PROTO_ORACLE_MODES.items()]]
        if args.tf_repo:
            runs += [("oracle.py", args.tf_repo, m, e)
                     for m, e in [("", None), *TF_ORACLE_MODES.items()]]
        if args.rust_repo:
            runs += [("oracle.py", args.rust_repo, m, e)
                     for m, e in [("", None), *RUST_ORACLE_MODES.items()]]
        for repo in dict.fromkeys(r for _, r, _ in BINDING_ORACLE_RUNS):
            runs.append(("oracle.py", repo, "", None))
            runs += [("oracle.py", repo, m, e) for m, r, e in BINDING_ORACLE_RUNS if r == repo]
        if args.notebook_repo:
            runs += [("oracle.py", args.notebook_repo, m, e)
                     for m, e in [("", None), *NOTEBOOK_ORACLE_MODES.items()]]
        for script, repo, mode, expect in runs:
            env = {**os.environ, "SABOTAGE": mode,
                   "SABOTAGE_REAL": str(Path(args.archctl).resolve())}
            extra = ["--rounds", "1"] if script == "bench.py" else []
            need = []
            if mode in RUST_ORACLE_MODES or (script == "oracle.py" and repo == args.rust_repo):
                need = ["syn"]
            if mode in NOTEBOOK_ORACLE_MODES or (script == "oracle.py" and repo == args.notebook_repo):
                need = ["ipython"]
            if mode in DEPLOY_ORACLE_MODES or mode in K8S_ORACLE_MODES \
                    or (script == "oracle.py" and repo in (args.deploy_repo, args.k8s_repo) and not mode):
                need = ["deploy"]
            # Repo manifest Kubernetes có chart Helm: đối chiếu với `helm template`.
            if script == "oracle.py" and repo == args.k8s_repo and (mode in K8S_ORACLE_MODES or not mode):
                need = [*need, "helm", "kustomize"]
            binding_repos = {r for _, r, _ in BINDING_ORACLE_RUNS}
            if mode in BINDING_ORACLE_MODES or (script == "oracle.py" and not mode and repo in binding_repos
                                                and repo not in (args.notebook_repo, args.rust_repo, args.repo)):
                need = ["clang"]
            # Phần `.proto` cộng thêm, không thay: repo của nó có thể cũng là
            # repo của phần Binding.
            if mode in PROTO_ORACLE_MODES or (script == "oracle.py" and repo == args.proto_repo and not mode):
                need = [*need, "proto"]
            if mode in TF_ORACLE_MODES or (script == "oracle.py" and repo == args.tf_repo and not mode):
                need = [*need, "terraform"]
            if need:
                extra = ["--require", ",".join(need)]
            p = subprocess.run(
                [sys.executable, str(HERE / script), "--archctl", str(fake),
                 "--only", repo, *extra],
                capture_output=True, text=True, env=env,
            )
            label = f"{script[:-3]} {mode or 'không phá'}" + (f" ({repo})" if repo != args.repo else "")
            caught = expect is None or expect in p.stdout
            want = 0 if expect is None else 1
            if p.returncode == want and caught:
                print(f"  ✓ {label:<22} mã {p.returncode}" + (f"  bắt bởi: {expect}" if expect else ""))
            else:
                bad += 1
                print(f"  ✗ {label:<22} mã {p.returncode}, mong {want}"
                      + (f"; không thấy `{expect}`" if not caught else ""))
                print("    " + (p.stdout + p.stderr).strip().replace("\n", "\n    ")[-600:])
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
