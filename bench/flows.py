"""Chạy mọi lệnh của archctl trên một repo thật, không phải fixture.

Test trong `crates/*/tests/` dựng dự án nhỏ, sạch, viết riêng để test. Ở đây
là mã người khác viết, với đủ thứ lộn xộn mà fixture không nghĩ ra: thư mục
vendor, symlink, file sinh tự động, tên file có dấu cách.
"""
import json
import re
import shutil
import sys
import socket
import subprocess
import time
import urllib.request
from pathlib import Path

from consistency import pick_source


class Flows:
    def __init__(self, archctl: str):
        self.archctl = archctl
        self.passed: list[str] = []
        self.failed: list[str] = []
        self.notes: list[str] = []

    def run(self, *args, timeout=300):
        return subprocess.run(
            [self.archctl, *args], capture_output=True, text=True, timeout=timeout
        )

    def check(self, label, cond, detail=""):
        if cond:
            self.passed.append(label)
        else:
            self.failed.append(f"{label}  {str(detail)[:160]}".rstrip())


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _git(work, *args):
    # Không để git tự `gc`/`maintenance` tách nền: tiến trình đó còn ghi vào
    # `.git` khi lượt sau xoá thư mục này, và `rmtree` đổ vì file biến mất giữa
    # chừng (CI, sabotage chạy liền hai lượt trên cùng repo).
    subprocess.run(["git", "-c", "gc.auto=0", "-c", "maintenance.auto=false", *args],
                   cwd=work, capture_output=True, check=False)


def _remove(path):
    """Xoá thư mục; file đã biến mất giữa chừng thì bỏ qua."""
    def gone(func, name, exc):
        exc = exc[1] if isinstance(exc, tuple) else exc
        if not isinstance(exc, FileNotFoundError):
            raise exc
    if sys.version_info >= (3, 12):
        shutil.rmtree(path, onexc=gone)
    else:
        shutil.rmtree(path, onerror=gone)


def run_flows(archctl: str, name: str, src: Path, work: Path) -> Flows:
    f = Flows(archctl)
    work = work / name
    if work.exists():
        _remove(work)
    shutil.copytree(src, work, symlinks=True, ignore=shutil.ignore_patterns(".git"))
    R = ["--root", str(work)]
    law = work / ".arch/system.arch"

    # --- init ---
    p = f.run("init", "--from-code", *R)
    f.check("init --from-code", p.returncode == 0, p.stderr)
    f.check("sinh ra file luật", law.exists())
    if not law.exists():
        return f
    settings = work / ".claude/settings.json"
    body = settings.read_text() if settings.exists() else ""
    # Bản cũ của phép kiểm này là `exists() or "..." in read_text()` — vế sau
    # không bao giờ được xét, nên nó chỉ kiểm file có tồn tại.
    f.check("hook cho agent", "archctl check --changed" in body, body[:120])
    f.check("cấm agent ghi file luật", "Write(./.arch/system.arch)" in body, body[:120])

    text = law.read_text()
    decls = [
        l for l in text.splitlines()
        if l.startswith(("component ", "allow ", "deny ", "store ", "external ", "service ", "link "))
    ]
    f.check(
        "mọi thứ bắt đầu ở #draft",
        bool(decls) and all("#draft" in l for l in decls),
        [l for l in decls if "#draft" not in l][:2],
    )
    law.write_text(text.replace("#draft", "#accepted"))

    # --- check ---
    p = f.run("check", *R)
    f.check("check chạy được", p.returncode in (0, 1), f"mã {p.returncode}: {p.stderr}")
    p = f.run("check", "--json", *R)
    try:
        rep = json.loads(p.stdout)
    except json.JSONDecodeError:
        rep = {}
    f.check("--json ra JSON hợp lệ", "archHash" in rep, p.stdout[:120])
    first_hash = rep.get("archHash")

    # --- parse, export, baseline ---
    p = f.run("parse", str(law))
    f.check("parse", p.returncode == 0, p.stderr)
    p = f.run("export", *R)
    f.check("export ra Mermaid", p.returncode == 0 and "flowchart" in p.stdout, p.stderr)
    p = f.run("export", "--violations", *R)
    f.check("export --violations", p.returncode == 0, p.stderr)
    p = f.run("baseline", *R)
    f.check("baseline", p.returncode == 0, p.stderr)
    after = f.run("check", *R)
    f.check(
        "baseline tha hết vi phạm cũ",
        "0 vi phạm mới" in after.stdout,
        [l for l in after.stdout.splitlines() if "vi phạm mới" in l],
    )

    # --- giao thức agent–người ---
    names = re.findall(r"^component (\w+)", text, re.M)
    if len(names) >= 2:
        stmt = f"allow {names[0]} -> {names[1]}"
        p = f.run("propose", stmt, "--reason", "thử giao thức", *R)
        f.check("propose", p.returncode in (0, 3), f"mã {p.returncode}: {p.stderr}")
        listing = f.run("proposals", *R).stdout
        ids = re.findall(r"\b(p-[0-9a-f]{6,16})\b", listing)
        f.check("proposals liệt kê được", bool(ids), listing)
        if ids:
            p = f.run("accept", ids[0], "--reason", "ok", *R)
            f.check("accept ghi vào luật", p.returncode == 0 and stmt in law.read_text(), p.stderr)
            log = (work / ".arch/decisions.md").read_text()
            f.check("quyết định vào nhật ký", "DUYỆT" in log and "ok" in log)
        p = f.run("block", "--on", f"deny * -> {names[0]}", "--reason", "bế tắc thử", *R)
        f.check("block", p.returncode in (0, 3), f"mã {p.returncode}: {p.stderr}")
        ids = re.findall(r"\b(p-[0-9a-f]{6,16})\b", f.run("proposals", *R).stdout)
        f.check("block hiện trong proposals", bool(ids))
        if ids:
            p = f.run("reject", ids[-1], "--reason", "không cần", *R)
            f.check("reject", p.returncode == 0, p.stderr)
    else:
        f.notes.append(f"bỏ qua giao thức agent: chỉ có {len(names)} component")

    # Luật chỉ đổi khi giao thức agent chạy (`accept` ghi vào luật).
    if len(names) >= 2:
        rep2 = json.loads(f.run("check", "--json", *R).stdout)
        f.check("hash đổi sau khi luật đổi", rep2["archHash"] != first_hash)

    # --- diff ---
    _git(work, "init", "-q", ".")
    _git(work, "add", "-A")
    _git(work, "-c", "user.email=bench@archctl", "-c", "user.name=bench", "commit", "-qm", "x")
    law.write_text(law.read_text() + "\n# ghi chú thêm\n")
    p = f.run("diff", *R)
    # Bản cũ chấp nhận cả `"0 " in stdout` — gần như mọi đầu ra đều khớp.
    f.check("diff: thêm chú thích không phải thay đổi thật", "không đổi so với" in p.stdout, p.stdout)
    if len(names) >= 2:
        # Bản cũ viết `law.read_text() + "..." if ... else ""`: Python đọc là
        # `(a + b) if ... else ""`, nên repo dưới hai component bị xoá trắng
        # file luật. Chưa repo nào chạm tới, nhưng nó nằm đó.
        law.write_text(law.read_text() + f"deny {names[0]} -> {names[1]}\n")
        p = f.run("diff", *R)
        hit = [l for l in p.stdout.splitlines() if "deny" in l and names[0] in l and names[1] in l]
        f.check("diff: thấy luật mới", p.returncode == 0 and bool(hit), p.stdout)

    # --- đường hook: --changed và đường dẫn cho thẳng ---
    one = pick_source(work)
    if one:
        one.write_text(one.read_text(errors="replace") + "\n")
        p = f.run("check", "--changed", *R)
        f.check("check --changed", p.returncode in (0, 1), p.stderr)
        p = f.run("check", str(one), *R)
        f.check("check <file>", p.returncode in (0, 1), p.stderr)

    p = f.run("check", "--strict", *R)
    f.check("--strict chạy được", p.returncode in (0, 1), f"mã {p.returncode}")

    # --- serve ---
    port = _free_port()
    proc = subprocess.Popen(
        [archctl, "serve", "--port", str(port), *R],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        model = None
        for _ in range(100):
            try:
                with urllib.request.urlopen(f"http://127.0.0.1:{port}/api/model", timeout=3) as r:
                    model = json.loads(r.read())
                break
            except Exception:
                time.sleep(0.1)
        f.check("serve trả về mô hình", model is not None)
        if model:
            for field in ("nodes", "edges", "violations", "missing", "vacuous",
                          "unproven", "proposals", "stats"):
                f.check(f"serve: có `{field}`", field in model)
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=3) as r:
                html = r.read().decode()
            f.check("serve: giao diện tự chứa",
                    "<title>archctl</title>" in html and "https://" not in html)
    finally:
        proc.kill()
        proc.wait()
    return f
