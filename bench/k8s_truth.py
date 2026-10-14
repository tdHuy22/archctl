"""Thước đo manifest Kubernetes cho `oracle.py`. Chạy trong venv của thước đo
hạ tầng (PyYAML, `bench/deploy-requirements.txt`).

Hai việc, chọn bằng đối số đầu:

- `facts`: đọc "<gốc>\\t<file>" từ stdin, in "<file>\\t<json>" với json là
  `{"facts": [[dòng, dữ kiện]]}` cho từng manifest hay `skaffold.yaml`, hay
  "<file>\\tERR <lý do>" khi PyYAML không đọc được, "<file>\\tSKIP" khi file
  không phải manifest. Như `deploy_truth.py`: biến môi trường chỉ so khoá và
  dòng; ConfigMap chỉ so khoá; Secret không có dữ kiện nào.
- `edges <gốc>`: đọc danh sách file YAML từ stdin, in JSON
  `{"<khoá nguồn>\\t<file>:<dòng>\\t<máy>": [khoá đích, ...]}`. Cài lại độc lập
  theo tài liệu Kubernetes: tên máy là tên Service (`svc`, `svc.ns`,
  `svc.ns.svc…`, pod của Service headless `pod.svc…`); Service chọn workload
  khi `selector` nằm trong nhãn của pod template, cùng namespace. Thứ archctl
  quy ước thêm (ưu tiên cùng thư mục; bỏ chart Helm, file patch Kustomize,
  workload không có image) ghi trong `docs/infra.md` và được cài lại ở đây.
  Tên máy rút từ giá trị theo dạng ghi trong `docs/infra.md`: URL có
  `scheme://`, `máy:cổng` khi khoá không nói về bí mật, giá trị trần khi đoạn
  cuối khoá nói về máy.
"""
import json
import re
import sys
from pathlib import Path

import yaml
from yaml.constructor import SafeConstructor

NULL = "tag:yaml.org,2002:null"
CTOR = SafeConstructor()
WORKLOADS = {"Deployment", "StatefulSet", "DaemonSet", "ReplicaSet", "ReplicationController", "Job", "CronJob", "Pod"}
HOST_SUFFIXES = {"HOST", "HOSTNAME", "HOSTS", "ADDR", "ADDRESS", "SERVER", "SERVERS", "URL", "URLS", "URI", "URIS",
                 "ENDPOINT", "ENDPOINTS", "DSN", "BROKER", "BROKERS", "PEERS", "NODES", "CLUSTER", "SERVICE"}
SECRET_PARTS = {"KEY", "KEYS", "TOKEN", "TOKENS", "SECRET", "SECRETS", "PASSWORD", "PASSWD", "PASS", "PWD", "SALT",
                "CREDENTIAL", "CREDENTIALS", "CREDS", "AUTH", "PRIVATE", "CERT", "SIGNATURE", "JWT", "APIKEY", "OTP"}
ASIDE = {"test", "tests", "testing", "selftest", "selftests", "spec", "specs", "__tests__", "e2e", "example",
         "examples", "bench", "benches", "doc", "docs", "deps", "vendor", "third_party", "thirdparty", "fixtures",
         "testdata", "node_modules"}


def scalar(node):
    if isinstance(node, yaml.ScalarNode):
        return None if node.tag == NULL else node.value
    return None


def entries(node):
    if not isinstance(node, yaml.MappingNode):
        return []
    CTOR.flatten_mapping(node)
    seen, out = {}, []
    for k, v in node.value:
        key = k.value if isinstance(k, yaml.ScalarNode) else None
        if key is None:
            continue
        if key in seen:
            out[seen[key]] = (key, k, v)
        else:
            seen[key] = len(out)
            out.append((key, k, v))
    return out


def get(node, *path):
    for key in path:
        if node is None:
            return None
        node = next((v for k, _, v in entries(node) if k == key), None)
    return node


def items(node):
    return node.value if isinstance(node, yaml.SequenceNode) else []


def line(node):
    return node.start_mark.line + 1


CONTROLLER_LABELS = {"statefulset.kubernetes.io/pod-name", "apps.kubernetes.io/pod-index", "controller-revision-hash",
                     "pod-template-hash", "job-name", "batch.kubernetes.io/job-name", "controller-uid",
                     "batch.kubernetes.io/controller-uid", "batch.kubernetes.io/job-completion-index"}


def may_be(text):
    return "apiVersion" in text and "kind" in text


def looks_like(text):
    lines = text.lstrip("\ufeff").split("\n")
    return any(l.startswith("kind:") for l in lines) and any(l.startswith("apiVersion:") for l in lines)


def objects(text):
    for doc in yaml.compose_all(text.lstrip("\ufeff"), Loader=yaml.SafeLoader):
        if not isinstance(doc, yaml.MappingNode):
            continue
        kind = scalar(get(doc, "kind")) or ""
        if kind.endswith("List") and isinstance(get(doc, "items"), yaml.SequenceNode):
            yield from (i for i in items(get(doc, "items")) if isinstance(i, yaml.MappingNode))
        else:
            yield doc


def pod_of(kind, doc):
    if kind == "Pod":
        return get(doc, "spec"), get(doc, "metadata")
    tpl = get(doc, "spec", "jobTemplate", "spec", "template") if kind == "CronJob" else get(doc, "spec", "template")
    return get(tpl, "spec"), get(tpl, "metadata")


def pairs(node):
    return [[k, scalar(v) or ""] for k, _, v in entries(node)]


def generic_pod(doc):
    for tpl in (get(doc, "spec", "template"), get(doc, "spec", "jobTemplate", "spec", "template")):
        if isinstance(get(tpl, "spec", "containers"), yaml.SequenceNode):
            return True
    return False


def manifest_facts(text):
    out = []
    for doc in objects(text):
        kind = scalar(get(doc, "kind"))
        if kind is None or kind == "Secret":
            continue
        name = scalar(get(doc, "metadata", "name")) or ""
        ns = scalar(get(doc, "metadata", "namespace"))
        at = line(doc)
        if kind == "Service":
            sel = [] if scalar(get(doc, "spec", "type")) == "ExternalName" else pairs(get(doc, "spec", "selector"))
            out.append((at, f"Service {name} ns={ns} selector={json.dumps(sel)}"))
        elif kind == "ConfigMap":
            out.append((at, f"ConfigMap {name} ns={ns}"))
            for k, kn, _ in entries(get(doc, "data")):
                out.append((line(kn), f"{name}.data {k}"))
        elif kind in WORKLOADS:
            spec, meta = pod_of(kind, doc)
            if spec is None:
                continue
            out.append((at, f"{kind} {name} ns={ns} labels={json.dumps(pairs(get(meta, 'labels')))}"))
            for key, init in (("initContainers", True), ("containers", False)):
                for c in items(get(spec, key)):
                    cname = scalar(get(c, "name")) or ""
                    out.append((line(c), f"{name}.container {cname} init={init} image={scalar(get(c, 'image'))}"))
                    for k in ("command", "args"):
                        node = get(c, k)
                        if isinstance(node, yaml.SequenceNode):
                            out.append((line(node), f"{name}.{cname}.{k}={json.dumps([scalar(i) or '' for i in node.value], ensure_ascii=False)}"))
                    wd = scalar(get(c, "workingDir"))
                    if wd is not None:
                        out.append((line(c), f"{name}.{cname}.workingDir={wd}"))
                    for e in items(get(c, "env")):
                        key = scalar(get(e, "name"))
                        if key is None:
                            continue
                        if get(e, "value") is not None:
                            out.append((line(e), f"{name}.{cname}.env {key}"))
                        else:
                            ref = get(e, "valueFrom", "configMapKeyRef")
                            cm, item = scalar(get(ref, "name")), scalar(get(ref, "key"))
                            if cm is not None and item is not None:
                                out.append((line(e), f"{name}.{cname}.ref {key}={cm}/{item}"))
                    for f in items(get(c, "envFrom")):
                        cm = scalar(get(f, "configMapRef", "name"))
                        if cm is not None:
                            out.append((line(f), f"{name}.{cname}.envFrom {cm} prefix={scalar(get(f, 'prefix')) or ''}"))
        elif not kind.endswith("List") and generic_pod(doc):
            out.append((at, f"unknown {kind} {name}"))
    return out


def skaffold_facts(text):
    out = []
    for doc in yaml.compose_all(text, Loader=yaml.SafeLoader):
        for base in [doc, *items(get(doc, "profiles"))]:
            for a in items(get(base, "build", "artifacts")):
                image = scalar(get(a, "image"))
                if image is None:
                    continue
                d = get(a, "docker") or get(a, "kaniko")
                if d is not None:
                    df, target = scalar(get(d, "dockerfile")) or "Dockerfile", scalar(get(d, "target"))
                elif any(get(a, k) is not None for k in ("ko", "jib", "buildpacks", "bazel", "custom")):
                    df, target = None, None
                else:
                    df, target = "Dockerfile", None
                ctx = scalar(get(a, "context")) or "."
                out.append((line(a), f"artifact {image} context={ctx} dockerfile={df} target={target}"))
    return out


def facts_main():
    entries = [raw.rstrip("\n").split("\t") for raw in sys.stdin if raw.strip()]
    charts = [dir_of(f) for _, f in entries if f.rsplit("/", 1)[-1] == "Chart.yaml"]
    for root, file in entries:
        name = file.rsplit("/", 1)[-1]
        # Template Helm không phải YAML cho tới khi render: archctl không đọc.
        if in_chart(charts, file):
            print(f"{file}\tSKIP", flush=True)
            continue
        text = None
        try:
            text = Path(root, file).read_text(encoding="utf-8")
            if name in ("skaffold.yaml", "skaffold.yml"):
                facts = skaffold_facts(text)
            elif may_be(text):
                facts = manifest_facts(text)
                if not facts and not looks_like(text):
                    print(f"{file}\tSKIP", flush=True)
                    continue
            else:
                print(f"{file}\tSKIP", flush=True)
                continue
            print(f"{file}\t{json.dumps({'facts': facts}, ensure_ascii=False)}", flush=True)
        except (yaml.YAMLError, UnicodeDecodeError, ValueError, RecursionError) as e:
            if text is None or name.startswith("skaffold.") or looks_like(text):
                print(f"{file}\tERR {type(e).__name__}: {' '.join(str(e).split())[:160]}", flush=True)
            else:
                print(f"{file}\tSKIP", flush=True)


# ---- cạnh ----

def defaults(value):
    return re.sub(r"\$\{[^}:-]*:?-([^}]*)\}", r"\1", value)


def hosts(key, value):
    parts = [p for p in re.split(r"[_.\-]", key.upper()) if p]
    secret = any(p in SECRET_PARTS for p in parts)
    hostish = not secret and parts and parts[-1] in HOST_SUFFIXES
    out = []
    for part in [p for p in re.split(r"[,\s;]+", defaults(value)) if p][:256]:
        m = re.match(r"^[A-Za-z][A-Za-z0-9+.\-]*://(?:[^@/?#]*@)?([^/:?#@]+)", part)
        if m:
            out.append(m.group(1))
        elif secret:
            continue
        elif re.fullmatch(r"[A-Za-z0-9._\-]+:\d+", part):
            out.append(part.rsplit(":", 1)[0])
        elif hostish and re.fullmatch(r"[A-Za-z0-9._\-]+", part):
            out.append(part)
    return {h.rstrip(".").lower() for h in out if re.search(r"[A-Za-z]", h)}


def in_chart(charts, f):
    return any(f.startswith(f"{c + '/' if c else ''}templates/") or f.startswith(f"{c + '/' if c else ''}charts/")
               for c in charts)


def expand(value, earlier):
    """`$(TÊN)` thành giá trị biến khai trước trong cùng container; `$$(` là chữ."""
    out, i = [], 0
    while i < len(value):
        if value.startswith("$$(", i):
            out.append("$(")
            i += 3
        elif value.startswith("$(", i) and value.find(")", i) != -1 and value[i + 2:value.find(")", i)] in earlier:
            end = value.find(")", i)
            out.append(earlier[value[i + 2:end]])
            i = end + 1
        else:
            out.append(value[i])
            i += 1
    return "".join(out)


def aside(path):
    return any(d in ASIDE or d.startswith(".") for d in path.split("/")[:-1])


def dir_of(path):
    return path.rsplit("/", 1)[0] if "/" in path else ""


def within(base, rel):
    if rel.startswith(("/", "~")) or "://" in rel or "\\" in rel:
        return None
    parts = [p for p in base.split("/") if p]
    for seg in rel.split("/"):
        if seg in ("", "."):
            continue
        if seg == "..":
            if not parts:
                return None
            parts.pop()
        else:
            parts.append(seg)
    return "/".join(parts)


def key_of(dir_, name):
    return f"k8s:{dir_ + '/' if dir_ else ''}{name}"


def same_ns(a, b):
    return a is None or b is None or a == b


def edges_main(root):
    root = Path(root)
    listed = [l.strip() for l in sys.stdin if l.strip()]
    charts = [dir_of(f) for f in listed if f.rsplit("/", 1)[-1] == "Chart.yaml" and not aside(f)]
    patch_files, namespaces = set(), {"default"}

    # Kustomization: file nào được kéo vào (kể cả ở thư mục ví dụ, ẩn), file
    # nào là patch.
    kusts = {}

    def kust_at(d):
        if d in kusts:
            return kusts[d]
        kusts[d] = None
        for n in ("kustomization.yaml", "kustomization.yml", "Kustomization"):
            p = Path(root, d, n) if d else Path(root, n)
            if p.is_file() and not p.is_symlink():
                try:
                    kusts[d] = yaml.safe_load(p.read_text()) or {}
                except (yaml.YAMLError, UnicodeDecodeError, OSError):
                    pass
                break
        return kusts[d]

    reached = set()
    roots = [dir_of(f) for f in listed if f.rsplit("/", 1)[-1] in ("kustomization.yaml", "kustomization.yml") and not aside(f)]
    queue = list(dict.fromkeys(roots))
    done = set()
    while queue:
        d = queue.pop(0)
        if d in done:
            continue
        done.add(d)
        k = kust_at(d)
        if not isinstance(k, dict):
            continue
        stack = [(d, 0)]
        seen = {d}
        while stack:
            cur, depth = stack.pop()
            kk = kust_at(cur)
            if not isinstance(kk, dict):
                continue
            for r in (kk.get("resources") or []) + (kk.get("bases") or []) + (kk.get("components") or []):
                if not isinstance(r, str):
                    continue
                full = within(cur, r)
                if full is None:
                    continue
                p = Path(root, full)
                if p.is_file():
                    reached.add(full)
                elif p.is_dir() and depth < 4 and full not in seen and isinstance(kust_at(full), dict):
                    seen.add(full)
                    stack.append((full, depth + 1))
                    queue.append(full)
    for d, k in kusts.items():
        if not isinstance(k, dict):
            continue
        if isinstance(k.get("namespace"), str):
            namespaces.add(k["namespace"])
        for key in ("patches", "patchesStrategicMerge", "patchesJson6902"):
            for p in k.get(key) or []:
                path = p.get("path") if isinstance(p, dict) else (p if isinstance(p, str) and "\n" not in p and not p.lstrip().startswith("{") else None)
                if path:
                    full = within(d, path)
                    if full:
                        patch_files.add(full)

    files = [f for f in listed if not aside(f)] + sorted(f for f in reached if f not in listed or aside(f))
    workloads, services, maps = [], [], []
    for f in dict.fromkeys(files):
        name = f.rsplit("/", 1)[-1]
        if not f.endswith((".yaml", ".yml", ".json")):
            continue
        if (in_chart(charts, f) or f in patch_files
                or name in ("skaffold.yaml", "skaffold.yml", "Chart.yaml", "kustomization.yaml", "kustomization.yml")
                or (name.startswith(("compose", "docker-compose")) and name.endswith((".yml", ".yaml")))):
            continue
        try:
            text = Path(root, f).read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        if not may_be(text) or len(text.encode()) > (1 << 20):
            continue
        try:
            docs = list(objects(text))
        except yaml.YAMLError:
            continue
        for doc in docs:
            kind = scalar(get(doc, "kind"))
            meta_ns = scalar(get(doc, "metadata", "namespace"))
            if meta_ns is not None and kind in WORKLOADS | {"Service", "ConfigMap"}:
                namespaces.add(meta_ns)
            if kind == "Namespace":
                namespaces.add(scalar(get(doc, "metadata", "name")) or "")
            obj = {"file": f, "dir": dir_of(f), "name": scalar(get(doc, "metadata", "name")) or "", "ns": meta_ns}
            if kind == "Service":
                external = None
                if scalar(get(doc, "spec", "type")) == "ExternalName":
                    sel = []
                    external = (scalar(get(doc, "spec", "externalName")) or "").rstrip(".").lower() or None
                else:
                    sel = pairs(get(doc, "spec", "selector"))
                services.append({**obj, "selector": [tuple(p) for p in sel], "external": external})
            elif kind == "ConfigMap":
                maps.append({**obj, "data": [(k, line(kn), scalar(v) or "") for k, kn, v in entries(get(doc, "data"))]})
            elif kind in WORKLOADS:
                spec, meta = pod_of(kind, doc)
                if spec is None:
                    continue
                containers = [c for key in ("initContainers", "containers") for c in items(get(spec, key))]
                if not any(scalar(get(c, "image")) for c in containers):
                    continue
                obj["labels"] = {tuple(p) for p in pairs(get(meta, "labels"))}
                obj["kind"] = kind
                obj["containers"] = containers
                workloads.append(obj)

    def controller_ok(k, v, w):
        if k == "statefulset.kubernetes.io/pod-name":
            return w["kind"] == "StatefulSet" and v.startswith(w["name"] + "-") and v[len(w["name"]) + 1:].isdigit()
        if k in ("job-name", "batch.kubernetes.io/job-name"):
            return (w["kind"] == "Job" and v == w["name"]) or (w["kind"] == "CronJob" and v.startswith(w["name"] + "-"))
        return True

    def select(svc):
        if not svc["selector"]:
            return []
        plain = {p for p in svc["selector"] if p[0] not in CONTROLLER_LABELS}
        ctl = [p for p in svc["selector"] if p[0] in CONTROLLER_LABELS]
        hits = [w for w in workloads if same_ns(svc["ns"], w["ns"]) and plain <= w["labels"]
                and all(controller_ok(k, v, w) for k, v in ctl)]
        same = [w for w in hits if w["dir"] == svc["dir"]]
        return same or hits

    def find(name, ns, d):
        found = [s for s in services if s["name"].lower() == name and same_ns(s["ns"], ns)]
        same = [s for s in found if s["dir"] == d]
        return same or found

    def resolve(host, w):
        found = resolve_once(host, w)
        for s in list(found):
            if s.get("external"):
                found += resolve_once(s["external"], w)
        return found

    def resolve_once(host, w):
        l = host.split(".")
        ns, d = w["ns"], w["dir"]
        if len(l) == 1:
            return find(l[0], ns, d)
        if len(l) >= 3 and l[2] == "svc":
            return find(l[0], l[1], d)
        if len(l) >= 4 and l[3] == "svc":
            return find(l[1], l[2], d)
        if len(l) == 2:
            return find(l[0], l[1], d) if l[1] in namespaces else find(l[1], ns, d)
        if len(l) == 3 and l[2] in namespaces:
            return find(l[1], l[2], d)
        return []

    def config(name, w):
        ok = [m for m in maps if m["name"] == name and same_ns(m["ns"], w["ns"])]
        return next((m for m in ok if m["dir"] == w["dir"]), ok[0] if ok else None)

    out = {}
    for w in workloads:
        src = key_of(w["dir"], w["name"])
        for c in w["containers"]:
            envs, earlier = [], {}
            for e in items(get(c, "env")):
                key = scalar(get(e, "name"))
                if key is None:
                    continue
                if get(e, "value") is not None:
                    value = expand(scalar(get(e, "value")) or "", earlier)
                    earlier[key] = value
                    envs.append((w["file"], line(e), key, value))
                else:
                    ref = get(e, "valueFrom", "configMapKeyRef")
                    m = config(scalar(get(ref, "name")), w) if ref is not None else None
                    item = scalar(get(ref, "key"))
                    for k, l, v in (m["data"] if m else []):
                        if k == item:
                            envs.append((m["file"], l, key, v))
            for f in items(get(c, "envFrom")):
                m = config(scalar(get(f, "configMapRef", "name")), w)
                prefix = scalar(get(f, "prefix")) or ""
                for k, l, v in (m["data"] if m else []):
                    envs.append((m["file"], l, prefix + k, v))
            for file, l, key, value in envs:
                for host in hosts(key, value):
                    targets = {key_of(t["dir"], t["name"]) for s in resolve(host, w) for t in select(s)}
                    targets.discard(src)
                    if targets:
                        out.setdefault(f"{src}\t{file}:{l}\t{host}", set()).update(targets)
    print(json.dumps({k: sorted(v) for k, v in out.items()}, ensure_ascii=False))


if __name__ == "__main__":
    if sys.argv[1:2] == ["facts"]:
        facts_main()
    elif sys.argv[1:2] == ["edges"]:
        edges_main(sys.argv[2])
    else:
        sys.exit("dùng: k8s_truth.py facts | edges <gốc>")
