"""Thước đo `.proto` cho `oracle.py`. Chạy trong venv có grpcio-tools
(`bench/proto-requirements.txt`).

Đọc "<gốc>\\t<file>" từ stdin, in "<file>\\t<json>" với json là
`{"facts": [[dòng, dữ kiện]]}`, hay "<file>\\tSKIP <lý do>" khi `protoc` không
dựng được descriptor (thường là import không có trong repo, như
`gogoproto/gogo.proto`).

Dữ kiện: `package X` (dòng 0, vì archctl không in dòng của package), `service
S` ở dòng của từ khoá `service`, `rpc S.M` ở dòng của từ khoá `rpc`. Dòng lấy
từ `source_code_info` của descriptor: path `[6, i]` là service thứ i, `[6, i,
2, j]` là method thứ j của nó.
"""
import json
import os
import subprocess
import sys
import tempfile

import grpc_tools
from google.protobuf import descriptor_pb2

WKT = os.path.join(os.path.dirname(grpc_tools.__file__), "_proto")


def googleapis_root():
    import google.api
    # Gói không gian tên: không có `__file__`. `.../site-packages/google/api`
    # → `.../site-packages`.
    return os.path.dirname(os.path.dirname(list(google.api.__path__)[0]))


def facts(root, file):
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "set.bin")
        cmd = [sys.executable, "-I", "-m", "grpc_tools.protoc", f"-I{root}", f"-I{WKT}", f"-I{googleapis_root()}",
               f"--descriptor_set_out={out}", "--include_source_info", file]
        p = subprocess.run(cmd, cwd=root, capture_output=True, text=True, timeout=120)
        if p.returncode != 0:
            first = (p.stderr.strip().splitlines() or ["?"])[0]
            return f"SKIP protoc: {first[:160]}"
        fds = descriptor_pb2.FileDescriptorSet()
        with open(out, "rb") as f:
            fds.ParseFromString(f.read())
    fd = fds.file[0]
    lines = {tuple(loc.path): loc.span[0] + 1 for loc in fd.source_code_info.location}
    found = [(0, f"package {fd.package}")]
    for i, s in enumerate(fd.service):
        found.append((lines.get((6, i), 0), f"service {s.name}"))
        for j, m in enumerate(s.method):
            found.append((lines.get((6, i, 2, j), 0), f"rpc {s.name}.{m.name}"))
    return found


def main():
    for raw in sys.stdin:
        root, file = raw.rstrip("\n").split("\t")
        res = facts(root, file)
        if isinstance(res, str):
            print(f"{file}\t{res}", flush=True)
        else:
            print(f"{file}\t{json.dumps({'facts': res}, ensure_ascii=False)}", flush=True)


if __name__ == "__main__":
    main()
