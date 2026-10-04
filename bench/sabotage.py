#!/usr/bin/env python3
"""Kiểm rằng bộ đo **đổ được**.

    python3 bench/sabotage.py [--archctl <binary>] [--repo flask]

Dựng một archctl giả: gọi archctl thật rồi làm hỏng đúng một chỗ trong đầu ra.
Mỗi kiểu phá phải làm `bench.py` thoát 1; không phá gì thì phải thoát 0. Một
phép kiểm không thể đổ thì tệ hơn không có, vì nó vẫn cộng vào con số "đạt" —
và bộ đo này từng có bốn phép kiểm như vậy.

`oracle.py` cũng vậy: ba kiểu phá đầu ra `scan --json` (mất một import, lệch
một dòng, thêm một import giả) phải làm nó thoát 1. `--repo` phải có file
Python, vì phần đối chiếu Python không cần công cụ nào ngoài chính Python.
"""
import argparse
import json
import os
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

# kiểu phá đầu ra `scan --json` → dòng `oracle.py` phải in ra
ORACLE_MODES = {
    "scan-drop": "thiếu:",
    "scan-line": "lệch dòng:",
    "scan-extra": "thừa:",
}


def sabotage_scan(rep: dict, mode: str):
    """Làm hỏng import đầu tiên của file Python đầu tiên có import."""
    for f in rep["files"]:
        if f["file"].endswith(".py") and f["imports"]:
            imports = f["imports"]
            if mode == "scan-drop":
                imports.pop(0)
            elif mode == "scan-line":
                imports[0]["line"] += 1
            elif mode == "scan-extra":
                imports.append({**imports[0], "spec": "khong_co_that"})
            return


def act_as_archctl(real: str, mode: str, args: list[str]) -> int:
    p = subprocess.run([real, *args], capture_output=True, text=True)
    out = p.stdout
    if args[:1] == ["check"] and "--json" in args and p.stdout.startswith("{"):
        rep = json.loads(out)
        if mode == "violation":
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
        out = json.dumps(rep)
    elif mode == "diff" and args[:1] == ["diff"]:
        out = "kiến trúc a → b\n"
    elif mode in ORACLE_MODES and args[:1] == ["scan"] and "--json" in args:
        rep = json.loads(out)
        sabotage_scan(rep, mode)
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
    args = ap.parse_args()

    with tempfile.TemporaryDirectory() as tmp:
        # Gọi qua một file shell thay vì đặt bit chạy được cho file này.
        fake = Path(tmp) / "archctl"
        fake.write_text(
            f'#!/bin/sh\nexec "{sys.executable}" "{Path(__file__).resolve()}" --as-archctl "$@"\n'
        )
        fake.chmod(0o755)

        bad = 0
        runs = [("bench.py", m, e) for m, e in [("", None), *MODES.items()]]
        runs += [("oracle.py", m, e) for m, e in [("", None), *ORACLE_MODES.items()]]
        for script, mode, expect in runs:
            env = {**os.environ, "SABOTAGE": mode,
                   "SABOTAGE_REAL": str(Path(args.archctl).resolve())}
            extra = ["--rounds", "1"] if script == "bench.py" else []
            p = subprocess.run(
                [sys.executable, str(HERE / script), "--archctl", str(fake),
                 "--only", args.repo, *extra],
                capture_output=True, text=True, env=env,
            )
            label = f"{script[:-3]} {mode or 'không phá'}"
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
