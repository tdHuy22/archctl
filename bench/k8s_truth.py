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


def selector(node):
    """`{"labels": [[k, v]], "exprs": [[k, op, [v]]]}` như archctl in."""
    labels = pairs(get(node, "matchLabels"))
    exprs = [[scalar(get(e, "key")) or "", scalar(get(e, "operator")) or "",
              [scalar(v) for v in items(get(e, "values")) if scalar(v) is not None]]
             for e in items(get(node, "matchExpressions"))]
    return {"labels": labels, "exprs": exprs}


def policy_of(doc):
    """NetworkPolicy theo tài liệu Kubernetes: không ghi `policyTypes` thì luôn
    cô lập Ingress, cô lập Egress khi có mục `egress`."""
    spec = get(doc, "spec")
    types = [scalar(t) for t in items(get(spec, "policyTypes"))]
    def has(direction, key):
        # `SetDefaults_NetworkPolicy`: Egress chỉ khi danh sách `egress` không rỗng.
        return direction in types if types else (direction == "Ingress" or bool(items(get(spec, key))))
    def rules(key, side):
        out = []
        for rule in items(get(spec, key)):
            peers = []
            for p in items(get(rule, side)):
                pods, nss = get(p, "podSelector"), get(p, "namespaceSelector")
                peers.append({"line": line(p), "pods": selector(pods) if pods is not None else None,
                              "namespaces": selector(nss) if nss is not None else None,
                              "ip": get(p, "ipBlock") is not None})
            out.append({"line": line(rule), "peers": peers})
        return out
    return {"pods": selector(get(spec, "podSelector")),
            "ingress": rules("ingress", "from") if has("Ingress", "ingress") else None,
            "egress": rules("egress", "to") if has("Egress", "egress") else None}


ROUTES = ("HTTPRoute", "GRPCRoute", "TCPRoute", "TLSRoute", "UDPRoute")


def service_exposed(doc):
    """Service mở ra ngoài cụm: `type: LoadBalancer`, `NodePort`, hay có `externalIPs`."""
    kind = scalar(get(doc, "spec", "type"))
    if kind in ("LoadBalancer", "NodePort"):
        return f"type: {kind}"
    return "externalIPs" if items(get(doc, "spec", "externalIPs")) else None


def truthy(v):
    """Giá trị đúng của YAML 1.1, như thư viện YAML của Kubernetes đọc."""
    return v is not None and v.lower() in ("true", "yes", "on", "y")


def workload_host(spec, containers):
    if truthy(scalar(get(spec, "hostNetwork"))):
        return "hostNetwork: true"
    if any(get(p, "hostPort") is not None for c in containers for p in items(get(c, "ports"))):
        return "hostPort"
    return None


def route_of(kind, doc):
    """`{"services": [(tên, ns|None, dòng)], "hosts": [(máy, dòng)], "public": bool,
    "delegates": [(tên, ns|None, dòng)]}` của một lối vào; None khi không phải
    (route của Gateway API chỉ gắn Service). VirtualService chỉ cho `mesh` vẫn
    trả về, `public` sai, vì có thể là đích của `delegate`."""
    spec = get(doc, "spec")
    if kind == "Ingress":
        out = []
        for b in [get(spec, "defaultBackend"), get(spec, "backend")] + \
                 [get(p, "backend") for r in items(get(spec, "rules")) for p in items(get(r, "http", "paths"))]:
            if b is None:
                continue
            nm = scalar(get(b, "service", "name")) or scalar(get(b, "serviceName"))
            if nm is not None:
                out.append((nm, None, line(b)))
        return {"services": out, "hosts": [], "public": True, "delegates": []}
    if kind in ROUTES:
        parents = items(get(spec, "parentRefs"))
        if parents and all(scalar(get(p, "kind")) == "Service" for p in parents):
            return None
        svcs = [(scalar(get(b, "name")), scalar(get(b, "namespace")), line(b))
                for r in items(get(spec, "rules")) for b in items(get(r, "backendRefs"))
                if scalar(get(b, "name")) is not None and (scalar(get(b, "kind")) or "Service") == "Service"]
        return {"services": svcs, "hosts": [], "public": True, "delegates": []}
    if kind == "VirtualService":
        gws = [scalar(g) for g in items(get(spec, "gateways"))]
        hosts, delegates = [], []
        for proto in ("http", "tcp", "tls"):
            for r in items(get(spec, proto)):
                d = get(r, "delegate")
                if scalar(get(d, "name")) is not None:
                    delegates.append((scalar(get(d, "name")), scalar(get(d, "namespace")), line(d)))
                for dest in items(get(r, "route")):
                    h = get(dest, "destination", "host")
                    if scalar(h) is not None:
                        hosts.append((scalar(h).rstrip(".").lower(), line(h)))
        return {"services": [], "hosts": hosts, "public": any(g != "mesh" for g in gws if g is not None),
                "delegates": delegates}
    return None


def compact(x):
    return json.dumps(x, separators=(",", ":"), ensure_ascii=False)


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
            if service_exposed(doc):
                out.append((at, f"Service {name} exposed={service_exposed(doc)}"))
        elif (route := route_of(kind, doc)) is not None and route["public"]:
            out.append((at, f"{kind} {name} ns={ns}"))
            out += [(l, f"{name}.backend {s} ns={rns}") for s, rns, l in route["services"]]
            out += [(l, f"{name}.host {h}") for h, l in route["hosts"]]
            out += [(l, f"{name}.delegate {d} ns={dns}") for d, dns, l in route["delegates"]]
        elif kind == "NetworkPolicy":
            pol = policy_of(doc)
            out.append((at, f"NetworkPolicy {name} ns={ns} pods={compact(pol['pods'])} "
                            f"ingress={pol['ingress'] is not None} egress={pol['egress'] is not None}"))
            for side in ("ingress", "egress"):
                for rule in pol[side] or []:
                    out.append((rule["line"], f"{name}.{side}.rule"))
                    for p in rule["peers"]:
                        out.append((p["line"], f"{name}.{side}.peer pods={compact(p['pods'])} "
                                               f"namespaces={compact(p['namespaces'])} ip={p['ip']}"))
        elif kind == "ConfigMap":
            out.append((at, f"ConfigMap {name} ns={ns}"))
            for k, kn, _ in entries(get(doc, "data")):
                out.append((line(kn), f"{name}.data {k}"))
        elif kind in WORKLOADS:
            spec, meta = pod_of(kind, doc)
            if spec is None:
                continue
            out.append((at, f"{kind} {name} ns={ns} labels={json.dumps(pairs(get(meta, 'labels')))}"))
            how = workload_host(spec, [c for key in ("initContainers", "containers") for c in items(get(spec, key))])
            if how:
                out.append((at, f"{name}.host {how}"))
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


def edges_main(root, helm_json=None, skip_json=None):
    """`helm_json`: file JSON `{template: [thư mục chart, văn bản dựng ra]}` của
    `helm template` (và `kustomize build`), đọc như manifest ở thư mục chart
    (thư mục kustomization gốc). `skip_json`: file thô mà bản dựng Kustomize ở
    chính thư mục của nó đã gồm, không đọc thô."""
    root = Path(root)
    rendered = json.loads(Path(helm_json).read_text()) if helm_json and helm_json != "-" else {}
    skip = set(json.loads(Path(skip_json).read_text())) if skip_json else set()
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

    def kfiles(d):
        """File manifest mà kustomization ở `d` kéo vào, đệ quy qua base và
        component: bản thô của chúng là bản sao base của bản dựng `d`."""
        out, stack, seen = set(), [(d, 0)], {d}
        while stack:
            cur, depth = stack.pop()
            kk = kust_at(cur)
            if not isinstance(kk, dict):
                continue
            for r in (kk.get("resources") or []) + (kk.get("bases") or []) + (kk.get("components") or []):
                full = within(cur, r) if isinstance(r, str) else None
                if full is None:
                    continue
                p = Path(root, full)
                if p.is_file():
                    out.add(full)
                elif p.is_dir() and depth < 16 and full not in seen:
                    seen.add(full)
                    stack.append((full, depth + 1))
        return out

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
    workloads, services, maps, policies, routes = [], [], [], [], []
    sources = []
    for f in dict.fromkeys(files):
        name = f.rsplit("/", 1)[-1]
        if not f.endswith((".yaml", ".yml", ".json")):
            continue
        if (in_chart(charts, f) or f in patch_files or f in skip
                or name in ("skaffold.yaml", "skaffold.yml", "Chart.yaml", "kustomization.yaml", "kustomization.yml")
                or (name.startswith(("compose", "docker-compose")) and name.endswith((".yml", ".yaml")))):
            continue
        try:
            text = Path(root, f).read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        if len(text.encode()) > (1 << 20):
            continue
        sources.append((f, dir_of(f), text))
    # Bản dựng Kustomize là một phạm vi riêng (xem `rank`); manifest thô ở thư
    # mục gốc của bản dựng mà bản dựng không gồm chạy cùng bản dựng đó.
    kroot_dirs = {d for f, (d, _) in rendered.items() if "<kustomize>" in f}
    sources = [(f, d, t, None, d if d in kroot_dirs else None) for f, d, t in sources]
    sources += [(f, d, t, d if "<kustomize>" in f else None, None) for f, (d, t) in sorted(rendered.items())]
    for f, d, text, scope, attached in sources:
        if not may_be(text):
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
            obj = {"file": f, "dir": d, "scope": scope, "attached": attached, "name": scalar(get(doc, "metadata", "name")) or "", "ns": meta_ns}
            if kind == "NetworkPolicy":
                policies.append({**obj, **policy_of(doc)})
            if kind is not None and (route := route_of(kind, doc)) is not None:
                routes.append({**obj, "kind": kind, **route})
            if kind == "Service":
                external = None
                if scalar(get(doc, "spec", "type")) == "ExternalName":
                    sel = []
                    external = (scalar(get(doc, "spec", "externalName")) or "").rstrip(".").lower() or None
                else:
                    sel = pairs(get(doc, "spec", "selector"))
                services.append({**obj, "selector": [tuple(p) for p in sel], "external": external,
                                 "exposed": service_exposed(doc)})
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
                obj["host"] = workload_host(spec, containers)
                obj["containers"] = containers
                workloads.append(obj)

    build_files = set().union(*(kfiles(sc) for sc in {o["scope"] for o in workloads + services + maps + policies + routes}
                                if sc is not None))

    def view(o):
        """Phạm vi một đối tượng nhìn ra: bản dựng của nó, hay bản dựng mà
        manifest thô ở thư mục gốc gắn kèm."""
        return o["scope"] if o["scope"] is not None else o["attached"]

    def rank(frm, obj):
        """0: cùng phạm vi; 1: bản dựng nhìn ra manifest thô không bản dựng nào
        kéo vào; None: không thấy. Manifest thô (kể cả gắn kèm) thấy mọi
        manifest thô, không nhìn vào bản dựng; bản dựng không nhìn sang bản
        dựng khác hay bản thô của base."""
        if obj["scope"] is not None:
            return 0 if frm == obj["scope"] else None
        if frm is None or obj["attached"] == frm:
            return 0
        return 1 if obj["file"] not in build_files else None

    def nearest(frm, objs):
        ranked = [(r, o) for o in objs if (r := rank(frm, o)) is not None]
        best = min((r for r, _ in ranked), default=None)
        return [o for r, o in ranked if r == best]

    def policy_sees(pol, w):
        return rank(view(pol), w) is not None or rank(view(w), pol) is not None

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
        hits = nearest(view(svc), [w for w in workloads if same_ns(svc["ns"], w["ns"]) and plain <= w["labels"]
                                      and all(controller_ok(k, v, w) for k, v in ctl)])
        same = [w for w in hits if w["dir"] == svc["dir"]]
        return same or hits

    def find(name, ns, d, scope):
        found = nearest(scope, [s for s in services if s["name"].lower() == name and same_ns(s["ns"], ns)])
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
        ns, d, sc = w["ns"], w["dir"], view(w)
        if len(l) == 1:
            return find(l[0], ns, d, sc)
        if len(l) >= 3 and l[2] == "svc":
            return find(l[0], l[1], d, sc)
        if len(l) >= 4 and l[3] == "svc":
            return find(l[1], l[2], d, sc)
        if len(l) == 2:
            return find(l[0], l[1], d, sc) if l[1] in namespaces else find(l[1], ns, d, sc)
        if len(l) == 3 and l[2] in namespaces:
            return find(l[1], l[2], d, sc)
        return []

    def config(name, w):
        ok = nearest(view(w), [m for m in maps if m["name"] == name and same_ns(m["ns"], w["ns"])])
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
    # NetworkPolicy: mỗi workload bị cô lập chiều nào, cho phép rõ ai.
    NAME = "kubernetes.io/metadata.name"

    def matches(sel, labels):
        lab = dict(labels)
        if not set(map(tuple, sel["labels"])) <= labels:
            return False
        for k, op, values in sel["exprs"]:
            v = lab.get(k)
            ok = {"In": v is not None and v in values, "NotIn": v is None or v not in values,
                  "Exists": v is not None, "DoesNotExist": v is None}.get(op, False)
            if not ok:
                return False
        return True

    def empty(sel):
        return not sel["labels"] and not sel["exprs"]

    guards = {key_of(w["dir"], w["name"]): [False, False, set(), False, False, set()] for w in workloads}
    for pol in policies:
        targets = [w for w in workloads if policy_sees(pol, w) and same_ns(pol["ns"], w["ns"]) and matches(pol["pods"], w["labels"])]
        for side, base in (("ingress", 0), ("egress", 3)):
            if pol[side] is None:
                continue
            any_, pods = False, set()
            for rule in pol[side]:
                if not rule["peers"]:
                    any_ = True
                for p in rule["peers"]:
                    if p["pods"] is None and p["namespaces"] is None:
                        continue
                    nss = p["namespaces"]
                    if nss is None:
                        scope = ("same", pol["ns"])
                    elif empty(nss):
                        scope = ("any", None)
                    elif not nss["exprs"] and len(nss["labels"]) == 1 and nss["labels"][0][0] == NAME:
                        scope = ("names", [nss["labels"][0][1]])
                    elif not nss["labels"] and len(nss["exprs"]) == 1 and nss["exprs"][0][0] == NAME and nss["exprs"][0][1] == "In":
                        scope = ("names", nss["exprs"][0][2])
                    else:
                        scope = ("unknown", None)
                    sel = p["pods"] or {"labels": [], "exprs": []}
                    if empty(sel):
                        any_ = True
                        continue
                    for w in workloads:
                        if not policy_sees(pol, w):
                            continue
                        kind_, val = scope
                        ok = (same_ns(val, w["ns"]) if kind_ == "same" else
                              (w["ns"] is None or w["ns"] in val) if kind_ == "names" else True)
                        if ok and matches(sel, w["labels"]):
                            pods.add(key_of(w["dir"], w["name"]))
            for t in targets:
                g = guards[key_of(t["dir"], t["name"])]
                # Cô lập chỉ khi biết chắc cùng namespace (cả hai cùng ghi hay cùng không).
                g[base] = g[base] or t["ns"] == pol["ns"]
                g[base + 1] = g[base + 1] or any_
                g[base + 2] |= pods
    # Lối vào từ ngoài cụm: workload nào lộ ra.
    exposed = {key_of(w["dir"], w["name"]) for w in workloads if w["host"]}
    for s in services:
        if s["exposed"]:
            exposed |= {key_of(w["dir"], w["name"]) for w in select(s)}
    def in_cluster(host, ns):
        l = host.split(".")
        if len(l) == 1:
            return l[0], ns
        if len(l) == 2 or l[2] == "svc":
            return l[0], l[1]
        return None

    for r in routes:
        if not r["public"]:
            continue
        targets = [s for name, rns, _ in r["services"] for s in find(name.lower(), rns or r["ns"], r["dir"], view(r))]
        entry_hosts = [(h, r["ns"]) for h, _ in r["hosts"]]
        for name, dns, _ in r["delegates"]:
            dns = dns or r["ns"]
            for child in routes:
                if (child["kind"] == "VirtualService" and rank(view(r), child) is not None and child["name"] == name
                        and same_ns(child["ns"], dns)):
                    entry_hosts += [(h, child["ns"] or dns) for h, _ in child["hosts"]]
        for host, hns in entry_hosts:
            hit = in_cluster(host, hns)
            if hit:
                targets += find(hit[0], hit[1], r["dir"], view(r))
        for s in targets:
            exposed |= {key_of(w["dir"], w["name"]) for w in select(s)}
    print(json.dumps({"exposed": sorted(exposed), "edges": {k: sorted(v) for k, v in out.items()},
                      "guards": {k: [g[0], g[1], sorted(g[2]), g[3], g[4], sorted(g[5])] for k, g in guards.items()}},
                     ensure_ascii=False))


def helm_facts_main(helm_json):
    """Dữ kiện từng template theo bản dựng của `helm template`."""
    for f, (_, text) in sorted(json.loads(Path(helm_json).read_text()).items()):
        try:
            facts = manifest_facts(text) if may_be(text) else []
        except (yaml.YAMLError, ValueError, RecursionError) as e:
            print(f"{f}\tERR {type(e).__name__}", flush=True)
            continue
        print(f"{f}\t{json.dumps({'facts': facts}, ensure_ascii=False)}", flush=True)


def kroots_main(root):
    """Kustomization gốc, tính độc lập với archctl: kustomization (không ở thư
    mục ví dụ, test) không bị kustomization nào khác trỏ tới qua `resources`,
    `bases`, `components`, và không phải `kind: Component`."""
    root = Path(root)
    listed = [l.strip() for l in sys.stdin if l.strip()]
    dirs = [dir_of(f) for f in listed
            if f.rsplit("/", 1)[-1] in ("kustomization.yaml", "kustomization.yml") and not aside(f)]
    referenced, components = set(), set()
    for d in dirs:
        for n in ("kustomization.yaml", "kustomization.yml"):
            p = Path(root, d, n) if d else Path(root, n)
            if not p.is_file():
                continue
            try:
                k = yaml.safe_load(p.read_text()) or {}
            except (yaml.YAMLError, UnicodeDecodeError, OSError):
                k = {}
            if not isinstance(k, dict):
                continue
            if k.get("kind") == "Component":
                components.add(d)
            for r in (k.get("resources") or []) + (k.get("bases") or []) + (k.get("components") or []):
                if isinstance(r, str) and (full := within(d, r)) is not None and Path(root, full).is_dir():
                    referenced.add(full)
            break
    print(json.dumps(sorted(d for d in set(dirs) if d not in referenced and d not in components)))


if __name__ == "__main__":
    if sys.argv[1:2] == ["facts"]:
        facts_main()
    elif sys.argv[1:2] == ["kroots"]:
        kroots_main(sys.argv[2])
    elif sys.argv[1:2] == ["helmfacts"]:
        helm_facts_main(sys.argv[2])
    elif sys.argv[1:2] == ["edges"]:
        edges_main(sys.argv[2], sys.argv[3] if len(sys.argv) > 3 else None, sys.argv[4] if len(sys.argv) > 4 else None)
    else:
        sys.exit("dùng: k8s_truth.py facts | edges <gốc>")
