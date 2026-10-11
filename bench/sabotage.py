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

Kiểu `crossing` chạy trên `--crossing-repo` (mặc định pydantic-core): xoá cạnh
liên ngôn ngữ khỏi cả luật `init` sinh ra lẫn vi phạm `check` báo, tức là phần
ghép hỏng ở cả hai phía cùng lúc. Phép kiểm đồng nhất không thấy gì; chỉ danh
sách cạnh bắt buộc trong `consistency.CROSSINGS` bắt được.

`--notebook-repo matplotlib` phá thêm hai kiểu ở notebook (mất import, lệch
dòng), đối chiếu bằng IPython cài vào venv nên cần mạng; CI bật nó.

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

# kiểu phá đầu ra `scan --json` → dòng `oracle.py` phải in ra
ORACLE_MODES = {
    "scan-drop": "thiếu:",
    "scan-line": "lệch dòng:",
    "scan-extra": "thừa:",
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


def sabotage_scan(rep: dict, mode: str):
    """Làm hỏng import đầu tiên của file đầu tiên có import: Python, Rust hay notebook tuỳ kiểu."""
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


def crossing_edges() -> set:
    sys.path.insert(0, str(HERE))
    import consistency
    return {edge for edges in consistency.CROSSINGS.values() for edge in edges}


def drop_crossings_from_law(args: list[str]):
    """Luật `init` sinh ra mất mọi cạnh liên ngôn ngữ bắt buộc."""
    root = Path(args[args.index("--root") + 1]) if "--root" in args else Path.cwd()
    law = root / ".arch/system.arch"
    if not law.exists():
        return
    keep = [
        line for line in law.read_text().splitlines(keepends=True)
        if not any(re.match(rf"allow\s+{a}\s*->\s*{b}\b", line) for a, b in crossing_edges())
    ]
    law.write_text("".join(keep))


def act_as_archctl(real: str, mode: str, args: list[str]) -> int:
    p = subprocess.run([real, *args], capture_output=True, text=True)
    out = p.stdout
    if mode == "crossing" and args[:1] == ["init"] and p.returncode == 0:
        drop_crossings_from_law(args)
    if args[:1] == ["check"] and "--json" in args and p.stdout.startswith("{"):
        rep = json.loads(out)
        if mode == "crossing":
            edges = crossing_edges()
            rep["violations"] = [
                v for v in rep["violations"]
                if not (v.get("via") == "ffi" and (v["from"], v["to"]) in edges)
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
        out = json.dumps(rep)
    elif mode == "diff" and args[:1] == ["diff"]:
        out = "kiến trúc a → b\n"
    elif (mode in ORACLE_MODES or mode in RUST_ORACLE_MODES or mode in NOTEBOOK_ORACLE_MODES) \
            and args[:1] == ["scan"] and "--json" in args:
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
    ap.add_argument("--rust-repo", help="phá cả phần Rust của oracle.py trên repo này (tải crate `syn`)")
    ap.add_argument("--notebook-repo",
                    help="phá cả phần notebook của oracle.py trên repo này (cài IPython vào venv)")
    ap.add_argument("--crossing-repo", default="pydantic-core",
                    help="repo có cạnh liên ngôn ngữ bắt buộc, cho kiểu phá `crossing`")
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
        runs += [("oracle.py", args.repo, m, e) for m, e in [("", None), *ORACLE_MODES.items()]]
        if args.rust_repo:
            runs += [("oracle.py", args.rust_repo, m, e)
                     for m, e in [("", None), *RUST_ORACLE_MODES.items()]]
        if args.notebook_repo:
            runs += [("oracle.py", args.notebook_repo, m, e)
                     for m, e in [("", None), *NOTEBOOK_ORACLE_MODES.items()]]
        for script, repo, mode, expect in runs:
            env = {**os.environ, "SABOTAGE": mode,
                   "SABOTAGE_REAL": str(Path(args.archctl).resolve())}
            extra = ["--rounds", "1"] if script == "bench.py" else []
            if mode in RUST_ORACLE_MODES or (script == "oracle.py" and repo == args.rust_repo):
                extra = ["--require", "syn"]
            if mode in NOTEBOOK_ORACLE_MODES or (script == "oracle.py" and repo == args.notebook_repo):
                extra = ["--require", "ipython"]
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
