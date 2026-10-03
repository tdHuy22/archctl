#!/usr/bin/env python3
"""Đo archctl trên các repo thật trong `repos.tsv`, ghim theo commit.

    python3 bench/bench.py                      # dựng archctl, tải repo còn thiếu, chạy hết
    python3 bench/bench.py --only hugo,tokio    # chỉ vài repo
    python3 bench/bench.py --archctl <binary>   # dùng binary có sẵn

Repo tải về `target/bench/repos/`, bản làm việc ở `target/bench/work/`. Cả hai
nằm trong `target/`: ngoài git, bộ quét của archctl bỏ qua, `cargo clean` dọn.

Mã thoát 1 khi archctl sai: luật sinh từ mã có vi phạm hay cảnh báo trên chính
mã đó, hoặc một kiểm tra luồng lệnh nào hỏng. Mã thoát 2 khi không chạy được
tới đó — mạng, build. Hai thứ tách nhau để một lần mạng chập chờn trên CI
không trông giống một lỗi của archctl. Thời gian chỉ in ra, không chặn: trên
máy CI dùng chung thì con số nhiễu.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))

import consistency  # noqa: E402
import flows  # noqa: E402

# Repo tải về là mã người khác. Không để nó chạy gì qua cấu hình git: hook,
# fsmonitor, hay giao thức file. Cùng danh sách mà archctl tự dùng.
GIT_HARDENING = [
    "-c", "core.hooksPath=/dev/null",
    "-c", "core.fsmonitor=",
    "-c", "protocol.file.allow=never",
    "-c", "advice.detachedHead=false",
]
GIT_ENV = {**os.environ, "GIT_LFS_SKIP_SMUDGE": "1", "GIT_TERMINAL_PROMPT": "0"}
# Không có giới hạn này thì một lần mạng treo giữ job CI tới sáu tiếng.
GIT_TIMEOUT_S = 300


def git(*args, cwd=None):
    try:
        return subprocess.run(
            ["git", *GIT_HARDENING, *args], cwd=cwd, env=GIT_ENV,
            capture_output=True, text=True, timeout=GIT_TIMEOUT_S,
        )
    except subprocess.TimeoutExpired:
        return subprocess.CompletedProcess(args, 124, "", f"quá {GIT_TIMEOUT_S} giây")


# `repos.tsv` nằm trong repo, nên một pull request sửa được nó. Tên đi vào
# `rmtree`, SHA và URL đi vào dòng lệnh git: `../..` làm bộ đo xoá ngoài thư
# mục cache, còn một giá trị mở đầu bằng `-` thì git đọc thành tuỳ chọn.
NAME = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
SHA = re.compile(r"[0-9a-f]{40}")
SUBPATH = re.compile(r"[A-Za-z0-9][A-Za-z0-9._/-]*")


def die(msg):
    """Mã thoát 2: không chạy tới được phép thử. Mã 1 dành cho archctl sai."""
    print(msg, file=sys.stderr)
    sys.exit(2)


def load_repos():
    repos = []
    for n, line in enumerate((HERE / "repos.tsv").read_text().splitlines(), 1):
        if not line.strip() or line.startswith("#"):
            continue
        cols = line.split("\t")
        if len(cols) < 4:
            die(f"repos.tsv:{n}: cần ít nhất bốn cột (tên, url, commit, submodule), có {len(cols)}")
        name, url, sha, subs, *_ = cols
        if not NAME.fullmatch(name) or ".." in name:
            die(f"repos.tsv:{n}: tên không hợp lệ: {name!r}")
        if not url.startswith("https://"):
            die(f"repos.tsv:{n}: chỉ nhận URL https://, nhận {url!r}")
        if not SHA.fullmatch(sha):
            die(f"repos.tsv:{n}: commit phải là SHA đủ 40 ký tự, nhận {sha!r}")
        paths = [] if subs == "-" else subs.split(",")
        for path in paths:
            if not SUBPATH.fullmatch(path) or ".." in path.split("/"):
                die(f"repos.tsv:{n}: đường dẫn submodule không hợp lệ: {path!r}")
        repos.append((name, url, sha, paths))
    return repos


def fetch(name, url, sha, dest: Path):
    """Đúng commit đã ghim, không hơn. Trả về lỗi, hoặc None."""
    if dest.exists() and git("rev-parse", "HEAD", cwd=dest).stdout.strip() == sha:
        return None
    shutil.rmtree(dest, ignore_errors=True)
    dest.parent.mkdir(parents=True, exist_ok=True)
    steps = [
        (["init", "-q", str(dest)], None),
        (["remote", "add", "origin", url], dest),
        (["fetch", "-q", "--depth", "1", "--no-tags", "origin", sha], dest),
        (["checkout", "-q", "--detach", "FETCH_HEAD"], dest),
    ]
    for args, cwd in steps:
        p = git(*args, cwd=cwd)
        if p.returncode != 0:
            return f"git {args[0]}: {p.stderr.strip()[:200]}"
    head = git("rev-parse", "HEAD", cwd=dest).stdout.strip()
    return None if head == sha else f"HEAD là {head}, không phải {sha}"


def fetch_submodule(dest: Path, path: str):
    """Submodule ở đúng commit mà repo cha ghi lại. Trả về lỗi, hoặc None.

    `--depth 1` không kéo submodule, nên trước đây zstd-rs vào bộ đo không có
    dòng C nào dù được chọn chính vì phần C vendor. URL đọc từ `.gitmodules`
    của repo cha ở commit đã ghim, và cũng phải là https.
    """
    tree = git("ls-tree", "HEAD", "--", path, cwd=dest).stdout.split()
    if len(tree) < 3 or tree[1] != "commit":
        return f"{path} không phải submodule ở commit đã ghim"
    sha = tree[2]
    url = None
    keys = git("config", "-f", ".gitmodules", "--get-regexp", r"\.path$", cwd=dest).stdout
    for line in keys.splitlines():
        key, _, value = line.partition(" ")
        if value == path:
            url = git("config", "-f", ".gitmodules", "--get",
                      key[: -len(".path")] + ".url", cwd=dest).stdout.strip()
    if not url or not url.startswith("https://"):
        return f"{path}: chỉ nhận URL https:// trong .gitmodules, nhận {url!r}"
    err = fetch(path, url, sha, dest / path)
    return f"{path}: {err}" if err else None


def tracked_sources(dest: Path, subs) -> int:
    """Số file nguồn archctl phải quét, đếm độc lập với archctl.

    Lấy từ `git ls-files`: thứ được commit là mã thật. Bỏ symlink (archctl
    không đi theo, có chủ ý), bỏ thư mục archctl luôn bỏ qua, và bỏ thư mục
    tên output (`build`, `dist`…) khi chính git nói nó bị ignore — hoặc khi
    không có `.gitignore` nào để hỏi. Phần `.gitignore` do `git check-ignore`
    trả lời, nên nó kiểm luôn bộ đọc `.gitignore` của archctl.
    """
    files = []
    for prefix in ["", *subs]:
        here = dest / prefix if prefix else dest
        p = git("ls-files", "-s", "-z", cwd=here)
        if p.returncode != 0:
            die(f"git ls-files ở {here}: {p.stderr.strip()[:200]}")
        for entry in p.stdout.split("\0"):
            if not entry:
                continue
            meta, _, path = entry.partition("\t")
            if meta.split()[0] in ("120000", "160000"):  # symlink, submodule
                continue
            files.append(f"{prefix}/{path}" if prefix else path)

    files = [f for f in files if Path(f).suffix in consistency.LANG_EXTS]
    files = [f for f in files if not set(f.split("/")[:-1]) & consistency.ALWAYS_IGNORE]

    # Mọi thư mục tên output trên đường dẫn của từng file.
    output_dirs = {
        "/".join(f.split("/")[: i + 1])
        for f in files
        for i, part in enumerate(f.split("/")[:-1])
        if part in consistency.OUTPUT_DIRS
    }
    skipped = set()
    for d in output_dirs:
        parts = d.split("/")
        has_gitignore = any(
            (dest / "/".join(parts[:i]) / ".gitignore").is_file() for i in range(len(parts))
        )
        if not has_gitignore:
            skipped.add(d)
    asked = sorted(output_dirs - skipped)
    if asked:
        # Hỏi cả thư mục lẫn một file giả bên trong: `dist/*` không bỏ chính
        # `dist` nhưng bỏ mọi thứ trong đó, và archctl coi như bỏ thư mục.
        probe = "__archctl_probe__"
        try:
            p = subprocess.run(
                ["git", *GIT_HARDENING, "-c", "core.excludesFile=/dev/null",
                 "check-ignore", "--no-index", "--stdin", "-z"],
                cwd=dest, env=GIT_ENV, capture_output=True, text=True, timeout=GIT_TIMEOUT_S,
                input="".join(f"{d}/\0{d}/{probe}\0" for d in asked),
            )
        except subprocess.TimeoutExpired:
            die(f"git check-ignore ở {dest}: quá {GIT_TIMEOUT_S} giây")
        # 0: có thứ bị ignore, 1: không có gì. Mã khác là git hỏng, không
        # phải archctl sai — đếm tiếp thì ra "số file lệch" giả.
        if p.returncode not in (0, 1):
            die(f"git check-ignore ở {dest}: {p.stderr.strip()[:200]}")
        for d in p.stdout.split("\0"):
            if d:
                skipped.add(d.rstrip("/").removesuffix(f"/{probe}"))
    return sum(
        1 for f in files
        if not any(f == d or f.startswith(d + "/") for d in skipped)
    )


def build():
    p = subprocess.run(["cargo", "build", "--release", "--locked", "--quiet"], cwd=ROOT)
    if p.returncode != 0:
        sys.exit(2)
    return str(ROOT / "target/release/archctl")


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--archctl", help="binary có sẵn; bỏ trống thì tự dựng bản release")
    ap.add_argument("--only", help="tên repo, cách nhau bằng dấu phẩy")
    ap.add_argument("--rounds", type=int, default=5, help="số lần đo để lấy trung vị")
    ap.add_argument("--cache", default=str(ROOT / "target/bench"))
    args = ap.parse_args()

    repos = load_repos()
    if args.only:
        wanted = set(args.only.split(","))
        unknown = wanted - {r[0] for r in repos}
        if unknown:
            die(f"không có repo nào tên: {', '.join(sorted(unknown))}")
        repos = [r for r in repos if r[0] in wanted]

    # Đường dẫn tuyệt đối: bộ đo đổi thư mục làm việc giữa chừng, và CI truyền
    # vào `target/release/archctl` theo thư mục gốc repo.
    archctl = str(Path(args.archctl).resolve()) if args.archctl else build()
    cache = Path(args.cache)
    work = cache / "work"

    broken = []
    for name, url, sha, subs in repos:
        dest = cache / "repos" / name
        err = fetch(name, url, sha, dest)
        for path in [] if err else subs:
            err = err or fetch_submodule(dest, path)
        if err:
            broken.append(f"{name}: {err}")
    if broken:
        die("không tải được:\n  " + "\n  ".join(broken))

    results = []
    for name, _, sha, subs in repos:
        print(f"· {name}", flush=True)
        src = cache / "repos" / name
        expected = tracked_sources(src, subs)
        r = consistency.analyse(archctl, name, src, work, expected, args.rounds)
        fl = flows.run_flows(archctl, name, src, work)
        r["commit"] = sha
        r["flows_passed"] = len(fl.passed)
        r["flows_failed"] = fl.failed
        r["flows_notes"] = fl.notes
        results.append(r)

    report(results)
    (cache / "results.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
    bad = any(r["problems"] or r["flows_failed"] for r in results)
    sys.exit(1 if bad else 0)


def report(results):
    cols = [
        ("repo", "repo", "<"), ("file", "files", ">"), ("quét", "scanned", ">"),
        ("comp", "components", ">"), ("kiểm", "checked", ">"),
        ("allow", "rules", ">"), ("vi phạm", "violations", ">"),
        ("cảnh báo luật", "law_warnings", ">"), ("luồng", None, ">"),
        ("init", "init_ms", ">"), ("check", "check_ms", ">"), ("1 file", "one_file_ms", ">"),
    ]

    def cell(r, key):
        if key is None:
            total = r["flows_passed"] + len(r["flows_failed"])
            return f"{r['flows_passed']}/{total}"
        v = r.get(key, "—")
        return f"{v}ms" if key.endswith("_ms") and v != "—" else str(v)

    rows = [[h for h, _, _ in cols]] + [[cell(r, k) for _, k, _ in cols] for r in results]
    widths = [max(len(row[i]) for row in rows) for i in range(len(cols))]
    print()
    for row in rows:
        print("  ".join(
            v.ljust(w) if a == "<" else v.rjust(w)
            for v, w, (_, _, a) in zip(row, widths, cols)
        ))

    files = sum(r.get("scanned", 0) for r in results)
    violations = sum(r.get("violations", 0) for r in results)
    law = sum(r.get("law_warnings", 0) for r in results)
    passed = sum(r["flows_passed"] for r in results)
    total = passed + sum(len(r["flows_failed"]) for r in results)
    print(f"\n{len(results)} repo · {files} file · {violations} bất nhất · "
          f"{law} cảnh báo trong luật sinh ra · luồng lệnh {passed}/{total}")

    for r in results:
        lines = r["problems"] + [f"luồng: {x}" for x in r["flows_failed"]]
        notes = r["flows_notes"]
        if lines or notes:
            print(f"\n{r['repo']}:")
            for l in lines:
                print(f"  ✗ {l}")
            for n in notes:
                print(f"  · {n}")


if __name__ == "__main__":
    main()
