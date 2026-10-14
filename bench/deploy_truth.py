"""Thước đo tầng hạ tầng cho `oracle.py`. Chạy trong venv có PyYAML và
dockerfile-parse (`bench/deploy-requirements.txt`).

Đọc "<gốc>\\t<file>" từ stdin, in "<file>\\t<json>" với json là
`{"facts": [[dòng, dữ kiện]]}`, hay "<file>\\tERR <lý do>" khi bộ đọc chuẩn không
đọc được, hay "<file>\\tSKIP <lý do>" khi thước đo không đo được file đó.

- Compose: PyYAML `compose` (giữ số dòng của từng node), khoá gộp `<<` trải ra
  bằng `SafeConstructor.flatten_mapping`; lệnh viết dạng chuỗi tách bằng
  `shlex.split`, như compose làm. Biến môi trường chỉ so khoá và dòng: archctl
  không in giá trị hay tên máy rút từ giá trị (TM-10), và luật rút tên máy có
  test riêng trong `scan/compose.rs`.
- Dockerfile: dockerfile-parse, ở mức chỉ thị (dòng, từ khoá, đối số sau khi
  nối dòng). Nó không hiểu heredoc của BuildKit, nên file có heredoc là SKIP.
"""
import io
import json
import re
import shlex
import sys

import yaml
from dockerfile_parse import DockerfileParser
from yaml.constructor import SafeConstructor

NULL = "tag:yaml.org,2002:null"
CTOR = SafeConstructor()
# Heredoc có thể mở ở một dòng nối, không chỉ ở dòng `RUN`.
HEREDOC = re.compile(r"<<-?[\"']?[A-Za-z_]\w*")


def is_compose(name):
    lower = name.lower()
    stem = lower[len("docker-"):] if lower.startswith("docker-") else lower
    ok = lower.endswith((".yml", ".yaml")) and stem.startswith("compose") and stem[7:8] in (".", "-", "_")
    return ok and not any(p in ("test", "tests", "e2e", "ci") for p in re.split(r"[.\-_]", stem))


def scalar(node):
    """Chuỗi như đã viết; null là None; không phải scalar cũng là None."""
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
            out[seen[key]] = (key, k, v)  # khoá trùng: khoá sau thắng
        else:
            seen[key] = len(out)
            out.append((key, k, v))
    return out


def get(node, key):
    for k, _, v in entries(node):
        if k == key:
            return v
    return None


def items(node):
    return node.value if isinstance(node, yaml.SequenceNode) else []


def line(node):
    return node.start_mark.line + 1


def command(node):
    s = scalar(node)
    if s is not None:
        return shlex.split(s)
    if isinstance(node, yaml.SequenceNode):
        return [scalar(i) or "" for i in node.value]
    return None


def compose_facts(text):
    docs = list(yaml.compose_all(text, Loader=yaml.SafeLoader))
    out = []
    if not docs or docs[0] is None:
        return out
    root = docs[0]
    for item in items(get(root, "include")):
        if scalar(item) is not None:
            out.append((line(item), f"include {scalar(item)}"))
        else:
            path = get(item, "path")
            for p in ([path] if scalar(path) is not None else items(path)):
                out.append((line(p), f"include {scalar(p)}"))
    services = get(root, "services")
    if services is None:
        if any(get(v, "image") is not None or get(v, "build") is not None for _, _, v in entries(root)):
            services = root
        else:
            return out
    for name, k, svc in entries(services):
        if name.startswith("x-") or not isinstance(svc, yaml.MappingNode):
            continue
        at = line(k)
        out.append((at, f"service {name}"))
        image = scalar(get(svc, "image"))
        if image is not None:
            out.append((at, f"{name}.image={image}"))
        wd = scalar(get(svc, "working_dir"))
        if wd is not None:
            out.append((at, f"{name}.working_dir={wd}"))
        build = get(svc, "build")
        if build is not None:
            if scalar(build) is not None:
                ctx, df, target, inline = scalar(build), None, None, False
            else:
                ctx = scalar(get(build, "context")) or "."
                df, target = scalar(get(build, "dockerfile")), scalar(get(build, "target"))
                inline = scalar(get(build, "dockerfile_inline")) is not None
            out.append((line(build), f"{name}.build context={ctx} dockerfile={df} target={target} inline={inline}"))
        for key in ("command", "entrypoint"):
            node = get(svc, key)
            argv = command(node) if node is not None else None
            if argv is not None:
                out.append((line(node), f"{name}.{key}={json.dumps(argv, ensure_ascii=False)}"))
        env = get(svc, "environment")
        for key, kn, _ in entries(env):
            out.append((line(kn), f"{name}.env {key.strip()}"))
        for item in items(env):
            text_ = scalar(item)
            if text_ is not None:
                out.append((line(item), f"{name}.env {text_.partition('=')[0].strip()}"))
        files = get(svc, "env_file")
        for f in ([files] if scalar(files) is not None else items(files)):
            path = scalar(f) if scalar(f) is not None else scalar(get(f, "path"))
            if path is not None:
                out.append((line(f), f"{name}.env_file {path}"))
        deps = get(svc, "depends_on")
        for item in items(deps):
            if scalar(item) is not None:
                out.append((line(item), f"{name}.depends_on {scalar(item)}"))
        for dep, kn, _ in entries(deps):
            out.append((line(kn), f"{name}.depends_on {dep}"))
        for item in items(get(svc, "links")):
            if scalar(item) is not None:
                out.append((line(item), f"{name}.links {scalar(item).split(':')[0]}"))
        for item in items(get(svc, "volumes")):
            text_ = scalar(item)
            if text_ is not None:
                parts = text_.split(":", 2)
                if len(parts) >= 2 and (parts[0] == "." or parts[0].startswith(("./", "../"))):
                    out.append((line(item), f"{name}.bind {parts[0]}:{parts[1]}"))
            elif scalar(get(item, "type")) == "bind":
                src, dst = scalar(get(item, "source")), scalar(get(item, "target"))
                if src is not None and dst is not None and (src == "." or src.startswith(("./", "../"))):
                    out.append((line(item), f"{name}.bind {src}:{dst}"))
        ext = get(svc, "extends")
        ext_name = scalar(ext) if scalar(ext) is not None else scalar(get(ext, "service"))
        if ext_name is not None:
            out.append((at, f"{name}.extends {ext_name}"))
        aliases = [scalar(get(svc, k)) for k in ("hostname", "container_name")]
        for _, _, net in entries(get(svc, "networks")):
            aliases += [scalar(a) for a in items(get(net, "aliases"))]
        for a in aliases:
            if a is not None:
                out.append((at, f"{name}.alias {a}"))
        # Cổng publish: cục bộ khi gắn vào loopback (theo tài liệu Docker).
        for item in items(get(svc, "ports")):
            text_ = scalar(item)
            if text_ is not None:
                out.append((line(item), f"{name}.port {text_} local={port_local(text_)}"))
            else:
                host = scalar(get(item, "host_ip")) or ""
                shown = f"{host + ':' if host else ''}{scalar(get(item, 'published')) or ''}:{scalar(get(item, 'target')) or ''}"
                out.append((line(item), f"{name}.port {shown} local={bool(host) and loopback(expand(host))}"))
        mode = get(svc, "network_mode")
        if scalar(mode) == "host":
            out.append((line(mode), f"{name}.port network_mode: host local=False"))
    return out


def expand(value):
    return re.sub(r"\$\{[^}:-]*:?-([^}]*)\}", r"\1", value)


def loopback(host):
    h = host.strip("[]")
    return h in ("localhost", "::1") or h.startswith("127.")


def port_local(spec):
    spec = expand(spec).split("/")[0]
    if spec.startswith("["):
        return loopback(spec[1:].split("]")[0])
    parts = spec.split(":")
    return len(parts) >= 3 and loopback(":".join(parts[:-2]))


def blank_continuations(text):
    """Dòng trống nằm giữa một lệnh nối dòng: BuildKit bỏ nó và nối tiếp (chỉ
    cảnh báo), dockerfile-parse thì cắt lệnh ở đó. Thay bằng dòng chú thích —
    thứ dockerfile-parse đã xử lý đúng — để số dòng giữ nguyên."""
    escape = "\\"
    m = re.match(r"(?:\s*#\s*\w+=.*\n)*?\s*#\s*escape=(\S)", text)
    if m:
        escape = m.group(1)
    out, cont = [], False
    for line in text.split("\n"):
        stripped = line.strip()
        if cont and stripped == "":
            out.append("#")
            continue
        out.append(line)
        if not stripped.startswith("#"):
            cont = line.rstrip().endswith(escape)
    return "\n".join(out)


def dockerfile_facts(text):
    if HEREDOC.search(text):
        return None
    text = blank_continuations(text)
    p = DockerfileParser(fileobj=io.BytesIO(text.encode()), env_replace=False)
    return [(s["startline"] + 1, f"{s['instruction']} {' '.join(s['value'].split())}".rstrip())
            for s in p.structure if s["instruction"] != "COMMENT"]


def main():
    for raw in sys.stdin:
        root, file = raw.rstrip("\n").split("\t")
        name = file.rsplit("/", 1)[-1]
        try:
            with open(f"{root}/{file}", encoding="utf-8") as f:
                text = f.read()
            if text.startswith("﻿"):
                text = text[1:]
            if is_compose(name):
                facts = compose_facts(text)
            else:
                facts = dockerfile_facts(text)
                if facts is None:
                    print(f"{file}\tSKIP heredoc của BuildKit: dockerfile-parse không đọc được", flush=True)
                    continue
            print(f"{file}\t{json.dumps({'facts': facts}, ensure_ascii=False)}", flush=True)
        except (yaml.YAMLError, UnicodeDecodeError, ValueError, RecursionError) as e:
            print(f"{file}\tERR {type(e).__name__}: {' '.join(str(e).split())[:160]}", flush=True)


if __name__ == "__main__":
    main()
