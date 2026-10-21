"""Thước đo Terraform cho `oracle.py`, độc lập với archctl.

Đầu vào: thư mục repo và một file JSON `{thư mục module gốc: đường dẫn plan
JSON}` do `tofu show -json` sinh ra. OpenTofu là nguồn chính: địa chỉ tài
nguyên sau khi mở module, tham chiếu của từng biểu thức (`expressions.*.
references`), đối số và output của module, giá trị đã biết lúc plan. JSON plan
không giữ `locals`, nên `locals` đọc bằng python-hcl2 (bộ đọc HCL thứ hai, độc
lập với bộ đọc của archctl) rồi lấy tham chiếu bằng biểu thức chính quy.

In JSON: `{gốc: {địa chỉ: {"edges": [...], "exposed": bool, "image": chuỗi hay
null}}}` cho mọi service chạy mã và mọi đích của cạnh.

Luật cạnh, lối vào viết lại từ mô tả trong docs/infra.md, không chép mã Rust.
"""

import json
import re
import sys
from pathlib import Path

import hcl2

COMPUTE = {"aws_ecs_service", "aws_ecs_task_definition", "aws_lambda_function",
           "google_cloud_run_v2_service", "google_cloud_run_service"}
NETWORK_ATTRS = {"address", "endpoint", "reader_endpoint", "dns_name", "uri", "url", "function_url", "invoke_url",
                 "api_endpoint", "primary_endpoint_address", "reader_endpoint_address",
                 "configuration_endpoint_address", "connection_name", "private_ip_address", "public_ip_address",
                 "host", "hostname", "fqdn", "port"}
MESSAGING = {"aws_sqs_queue", "aws_sns_topic", "aws_kinesis_stream", "google_pubsub_topic", "aws_mq_broker"}
REF = re.compile(r"\b((?:module\.[A-Za-z0-9_-]+\.)*(?:local|var|module|data|[a-z][a-z0-9]*_[a-z0-9_]+)\.[A-Za-z0-9_.\-]+)")


def refs_of(node):
    """Mọi tham chiếu trong một giá trị python-hcl2 (chuỗi `${…}` lồng trong
    list, dict)."""
    out = []
    if isinstance(node, str):
        for inner in re.findall(r"\$\{(.*?)\}", node, re.S) or ([node] if node.startswith("${") else []):
            out += REF.findall(inner)
    elif isinstance(node, list):
        for x in node:
            out += refs_of(x)
    elif isinstance(node, dict):
        for x in node.values():
            out += refs_of(x)
    return out


def config_refs(expr):
    """`references` trong một biểu thức của JSON plan, đệ quy qua khối lồng."""
    out = []
    if isinstance(expr, dict):
        out += expr.get("references", [])
        for k, v in expr.items():
            if k != "references":
                out += config_refs(v)
    elif isinstance(expr, list):
        for x in expr:
            out += config_refs(x)
    return out


ALIASES = {"aws_alb": "aws_lb", "aws_alb_listener": "aws_lb_listener", "aws_alb_listener_rule": "aws_lb_listener_rule",
           "aws_alb_target_group": "aws_lb_target_group"}


def type_of(addr):
    parts = addr.split(".")
    while len(parts) > 2 and parts[0] == "module":
        parts = parts[2:]
    t = parts[1] if parts[0] == "data" and len(parts) > 1 else parts[0]
    return ALIASES.get(t, t)


def locals_of(module_dir):
    out = {}
    for f in sorted(Path(module_dir).glob("*.tf")):
        if f.name == "override.tf" or f.name.endswith("_override.tf") or f.name.startswith("zz_archctl"):
            continue
        try:
            doc = hcl2.loads(f.read_text())
        except Exception:  # noqa: BLE001 — file archctl cũng báo không đọc được
            continue
        for block in doc.get("locals", []):
            for k, v in block.items():
                out[k] = refs_of(v)
    return out


class Inst:
    def __init__(self, prefix, config, module_dir, parent=None, call=None):
        self.prefix, self.config, self.dir, self.parent, self.call = prefix, config, module_dir, parent, call
        self.locals = locals_of(module_dir)
        self.children = {}
        for name, mc in config.get("module_calls", {}).items():
            src = mc.get("source", "")
            if not src.startswith(("./", "../")):
                continue
            child_dir = (Path(module_dir) / src).resolve()
            self.children[name] = Inst(f"{prefix}module.{name}.", mc.get("module", {}), child_dir, self, mc)
        self.resources = {f"{prefix}{r['type']}.{r['name']}": r for r in config.get("resources", []) if r.get("mode") == "managed"}


def resolve(inst, ref, seen):
    """Tham chiếu → các cặp (địa chỉ tài nguyên đầy đủ, thuộc tính)."""
    key = (inst.prefix, ref)
    if key in seen:
        return set()
    seen = seen | {key}
    parts = ref.split(".")
    root = parts[0]
    if root == "local" and len(parts) > 1:
        return set().union(*[resolve(inst, r, seen) for r in inst.locals.get(parts[1], [])])
    if root == "var" and len(parts) > 1:
        if inst.call is None:
            return set()
        expr = inst.call.get("expressions", {}).get(parts[1], {})
        return set().union(*[resolve(inst.parent, r, seen) for r in config_refs(expr)])
    if root == "module" and len(parts) > 2:
        child = inst.children.get(parts[1])
        if child is None:
            return set()
        expr = child.config.get("outputs", {}).get(parts[2], {}).get("expression", {})
        return set().union(*[resolve(child, r, seen) for r in config_refs(expr)])
    if root in ("data", "each", "count", "path", "self", "terraform"):
        return set()
    if len(parts) >= 2:
        return {(f"{inst.prefix}{parts[0]}.{parts[1]}", parts[2] if len(parts) > 2 else "")}
    return set()


def all_insts(inst):
    yield inst
    for c in inst.children.values():
        yield from all_insts(c)


def planned(plan):
    out = {}

    def walk(m):
        for r in m.get("resources", []):
            out[re.sub(r"\[[^\]]*\]$", "", r["address"])] = r.get("values", {})
        for c in m.get("child_modules", []):
            walk(c)
    walk(plan.get("planned_values", {}).get("root_module", {}))
    return out


def env_exprs(r):
    e = r.get("expressions", {})
    if r["type"] == "aws_ecs_task_definition":
        return [e.get("container_definitions", {})]
    if r["type"] == "aws_lambda_function":
        return [b.get("variables", {}) for b in e.get("environment", [])]
    if r["type"] == "google_cloud_run_v2_service":
        return [env.get("value", {}) for t in e.get("template", []) for c in t.get("containers", []) for env in c.get("env", [])]
    if r["type"] == "google_cloud_run_service":
        return [env.get("value", {}) for t in e.get("template", []) for s in t.get("spec", []) for c in s.get("containers", [])
                for env in c.get("env", [])]
    return []


def truth(root_dir, plan):
    top = Inst("", plan["configuration"]["root_module"], root_dir)
    insts = list(all_insts(top))
    res = {}
    owner = {}
    for i in insts:
        for a, r in i.resources.items():
            res[a], owner[a] = r, i
    values = planned(plan)

    def points(a, key, ty):
        r = res[a]
        e = r.get("expressions", {})
        found = set()
        for x in (e.get(key, {}),) + tuple(b.get(key, {}) for v in e.values() if isinstance(v, list) for b in v if isinstance(b, dict)):
            for ref in config_refs(x):
                found |= {t for t, _ in resolve(owner[a], ref, frozenset()) if type_of(t) == ty}
        return found

    # Service: ECS service cùng task definition, task definition đứng riêng, Lambda, Cloud Run.
    units, holder = {}, {}
    used_td = set()
    for a in sorted(res):
        if type_of(a) == "aws_ecs_service":
            for td in points(a, "task_definition", "aws_ecs_task_definition"):
                used_td.add(td)
                units[a], holder[a] = True, td
    for a in sorted(res):
        t = type_of(a)
        if t in ("aws_lambda_function", "google_cloud_run_v2_service", "google_cloud_run_service") or (t == "aws_ecs_task_definition" and a not in used_td):
            units[a], holder[a] = True, a
    names = {}
    for u, h in holder.items():
        names.setdefault(h, []).append(u)
        names.setdefault(u, []).append(u)
    # Đứng trước service: bộ cân bằng tải, target group, listener, Cloud Map, Lambda URL.
    fronts = {}
    for u in units:
        if type_of(u) == "aws_ecs_service":
            for tg in points(u, "target_group_arn", "aws_lb_target_group"):
                fronts.setdefault(tg, set()).add(u)
                for l in [a for a in res if type_of(a) in ("aws_lb_listener", "aws_lb_listener_rule") and tg in points(a, "target_group_arn", "aws_lb_target_group")]:
                    lbs = points(l, "load_balancer_arn", "aws_lb")
                    for parent in points(l, "listener_arn", "aws_lb_listener"):
                        lbs |= points(parent, "load_balancer_arn", "aws_lb")
                    for lb in lbs:
                        fronts.setdefault(lb, set()).add(u)
            for sd in points(u, "registry_arn", "aws_service_discovery_service"):
                fronts.setdefault(sd, set()).add(u)
        if type_of(u) == "aws_lambda_function":
            for a in res:
                if type_of(a) in ("aws_lambda_function_url", "aws_lambda_alias") and u in points(a, "function_name", "aws_lambda_function"):
                    fronts.setdefault(a, set()).add(u)
    out = {}
    targets = set()
    for u in sorted(units):
        h = holder[u]
        edges = set()
        for x in env_exprs(res[h]):
            for ref in config_refs(x):
                for t, attr in resolve(owner[h], ref, frozenset()):
                    if t in names:
                        edges |= set(names[t])
                    elif t in fronts:
                        edges |= fronts[t]
                    elif attr in NETWORK_ATTRS or type_of(t) in MESSAGING:
                        edges.add(t)
                        targets.add(t)
        edges.discard(u)
        out[u] = {"edges": sorted(edges), "exposed": exposed(u, res, values, points), "image": image(h, values)}
    for t in sorted(targets - set(units)):
        out[t] = {"edges": [], "exposed": False, "image": None}
    return out


def exposed(u, res, values, points):
    t = type_of(u)
    if t == "aws_ecs_service":
        for tg in points(u, "target_group_arn", "aws_lb_target_group"):
            for l in [a for a in res if type_of(a) in ("aws_lb_listener", "aws_lb_listener_rule") and tg in points(a, "target_group_arn", "aws_lb_target_group")]:
                lbs = points(l, "load_balancer_arn", "aws_lb")
                for parent in points(l, "listener_arn", "aws_lb_listener"):
                    lbs |= points(parent, "load_balancer_arn", "aws_lb")
                if any(values.get(lb, {}).get("internal") is False for lb in lbs):
                    return True
        for nc in values.get(u, {}).get("network_configuration") or []:
            if nc.get("assign_public_ip") is True:
                for sg in points(u, "security_groups", "aws_security_group"):
                    rules = values.get(sg, {}).get("ingress") or []
                    if any("0.0.0.0/0" in (r.get("cidr_blocks") or []) or "::/0" in (r.get("ipv6_cidr_blocks") or []) for r in rules):
                        return True
        return False
    if t == "aws_lambda_function":
        aliases = {a for a in res if type_of(a) == "aws_lambda_alias" and u in points(a, "function_name", "aws_lambda_function")}
        for a in res:
            if type_of(a) == "aws_lambda_function_url" and values.get(a, {}).get("authorization_type") == "NONE" \
                    and (u in points(a, "function_name", "aws_lambda_function") or points(a, "function_name", "aws_lambda_alias") & aliases):
                return True
            if type_of(a) in ("aws_apigatewayv2_integration", "aws_api_gateway_integration") \
                    and u in points(a, "integration_uri" if type_of(a) == "aws_apigatewayv2_integration" else "uri", "aws_lambda_function"):
                return True
        return False
    if t in ("google_cloud_run_v2_service", "google_cloud_run_service"):
        if values.get(u, {}).get("invoker_iam_disabled") is True:
            return True
        for a in res:
            if type_of(a).startswith(t + "_iam_") and u in points(a, "name", t) | points(a, "service", t):
                v = values.get(a, {})
                members = ([v.get("member")] if v.get("member") else []) + list(v.get("members") or [])
                if v.get("role") in (None, "roles/run.invoker") and any(m in ("allUsers", "allAuthenticatedUsers") for m in members):
                    return True
        return False
    return False


def image(h, values):
    v = values.get(h, {})
    t = type_of(h)
    if t == "aws_ecs_task_definition":
        try:
            cs = json.loads(v.get("container_definitions") or "null")
        except (TypeError, ValueError):
            return None
        return cs[0].get("image") if cs else None
    if t == "aws_lambda_function":
        return v.get("image_uri")
    if t == "google_cloud_run_v2_service":
        for tpl in v.get("template") or []:
            for c in tpl.get("containers") or []:
                return c.get("image")
    if t == "google_cloud_run_service":
        for tpl in v.get("template") or []:
            for s in tpl.get("spec") or []:
                for c in s.get("containers") or []:
                    return c.get("image")
    return None


def main():
    repo, plans = Path(sys.argv[2]), json.loads(Path(sys.argv[3]).read_text())
    out = {}
    for root, plan_path in sorted(plans.items()):
        plan = json.loads(Path(plan_path).read_text())
        out[root] = truth(repo / root if root else repo, plan)
    print(json.dumps(out, sort_keys=True))


def roots_main():
    """Module gốc, tính độc lập với archctl: thư mục có `.tf` mà không module
    nào trong repo gọi tới qua `source = "./…"`."""
    repo = Path(sys.argv[2])
    listed = [l.strip() for l in sys.stdin if l.strip().endswith(".tf")]
    dirs = sorted({str(Path(f).parent) if "/" in f else "" for f in listed})
    dirs = ["" if d == "." else d for d in dirs]
    called = set()
    for d in dirs:
        for f in sorted((repo / d).glob("*.tf")):
            try:
                doc = hcl2.loads(f.read_text())
            except Exception:  # noqa: BLE001
                continue
            for block in doc.get("module", []):
                for _, body in block.items():
                    # python-hcl2 8 giữ dấu nháy của chuỗi: `"\"./x\""`.
                    src = body.get("source", "")
                    src = src.strip('"') if isinstance(src, str) else ""
                    if src.startswith(("./", "../")):
                        target = (repo / d / src).resolve()
                        try:
                            called.add(str(target.relative_to(repo.resolve())))
                        except ValueError:
                            pass
    print(json.dumps([d for d in dirs if d not in called]))


if __name__ == "__main__":
    {"truth": main, "roots": roots_main}[sys.argv[1]]()
