"""Phép thử sắc nhất: luật sinh từ mã phải khớp với chính mã đó.

`init --from-code` đọc mã rồi sinh luật mô tả đúng hiện trạng. Nâng mọi thứ
lên `#accepted` rồi `check` lại trên **chính mã đó** thì phải ra 0 vi phạm.
Mỗi vi phạm còn lại là một chỗ bộ khám phá và bộ kiểm chứng nhìn khác nhau —
tức là một lỗi của archctl, không phải của repo.

Cũng phải ra 0 cảnh báo của file luật. `init` khuyên chạy `check --strict`
trong CI, và `--strict` coi cảnh báo của file luật là vi phạm: một luật sinh
ra đã có cảnh báo thì lời khuyên đó đổ ngay ở lần chạy đầu tiên.
"""
import json
import re
import shutil
import statistics
import subprocess
import time
from pathlib import Path

LANG_EXTS = {
    ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs", ".py", ".pyi",
    ".c", ".h", ".cc", ".cpp", ".cxx", ".hpp", ".hh", ".hxx", ".inl", ".rs", ".go",
}
# Thư mục archctl luôn bỏ qua (`ALWAYS_IGNORE` trong `check.rs`) và những tên
# chỉ bỏ khi `.gitignore` bỏ (`OUTPUT_DIRS`). Chép lại ở đây có chủ ý: đây là
# điều archctl *định* làm, còn số file đếm độc lập cho biết nó có làm đúng không.
ALWAYS_IGNORE = {
    ".git", ".arch", "node_modules", "target", ".next", ".nuxt", ".svelte-kit",
    "vendor", "__pycache__", ".venv",
}
OUTPUT_DIRS = {"dist", "build", "out", "coverage"}


def _run(archctl, args, timeout=300):
    t = time.perf_counter()
    p = subprocess.run([archctl, *args], capture_output=True, text=True, timeout=timeout)
    return p, (time.perf_counter() - t) * 1000


def _median_ms(archctl, args, rounds):
    """Trung vị, bỏ lần đầu vì bộ nhớ đệm của hệ thống file."""
    _run(archctl, args)
    return statistics.median(_run(archctl, args)[1] for _ in range(rounds))


def analyse(archctl: str, name: str, src: Path, work: Path, expected: int,
            rounds: int = 5) -> dict:
    """Trả về một dict kết quả. Khoá `problems` liệt kê mọi thứ làm phép thử đổ.

    `expected` là số file nguồn archctl phải quét, đếm từ git chứ không từ
    archctl — xem `tracked_sources` trong `bench.py`.
    """
    work = work / name
    if work.exists():
        shutil.rmtree(work)
    shutil.copytree(src, work, symlinks=True, ignore=shutil.ignore_patterns(".git"))
    root = ["--root", str(work)]

    out = {"repo": name, "files": expected, "problems": []}

    p, ms = _run(archctl, ["init", "--from-code", "--no-hooks", *root])
    out["init_ms"] = round(ms)
    law = work / ".arch/system.arch"
    if p.returncode != 0 or not law.exists():
        out["problems"].append(f"init --from-code hỏng: {(p.stderr or p.stdout)[:200]}")
        return out
    text = law.read_text()
    out["components"] = len(re.findall(r"^component ", text, re.M))
    out["rules"] = len(re.findall(r"^allow ", text, re.M))

    # Luật sinh từ chính mã này, nên phải tự nhất quán khi có hiệu lực.
    law.write_text(text.replace("#draft", "#accepted"))

    p, _ = _run(archctl, ["check", "--json", *root])
    try:
        rep = json.loads(p.stdout)
    except json.JSONDecodeError:
        out["problems"].append(f"check --json không ra JSON: {(p.stdout or p.stderr)[:200]}")
        return out

    out["scanned"] = rep["filesScanned"]
    out["unmapped"] = rep["filesUnmapped"]
    out["checked"] = len(rep["checked"])
    out["violations"] = len(rep["violations"])
    out["law_warnings"] = len(rep.get("lawWarnings", []))
    out["unresolved"] = len(rep["unresolved"])
    out["missing"] = len(rep.get("missing", []))
    out["vacuous"] = len(rep.get("vacuous", []))

    # 0 vi phạm trên 0 file thì chẳng chứng minh được gì. Không quét được gì
    # là phép thử vô nghĩa, và nó phải đổ chứ không được trông như đạt.
    if out["scanned"] == 0 or out["files"] == 0:
        out["problems"].append(
            f"không có gì để kiểm: archctl quét {out['scanned']} file, đếm độc lập ra {out['files']}"
        )
    # Lệch là archctl bỏ sót mã được commit, hoặc quét thứ nó định bỏ. etcd
    # từng mất cả package `tests/robustness/coverage/` vì tên thư mục trùng
    # tên output, và không có gì nói ra.
    if out["scanned"] != out["files"]:
        out["problems"].append(
            f"số file lệch: archctl quét {out['scanned']}, git có {out['files']} file nguồn"
        )
    if out["checked"] == 0:
        out["problems"].append("không component nào vào diện kiểm, 0 vi phạm là vô nghĩa")
    if "lawWarnings" not in rep:
        out["problems"].append("JSON thiếu khoá `lawWarnings`")
    for v in rep["violations"][:6]:
        out["problems"].append(
            f"bất nhất: {v['from']} -> {v['to']} qua {v.get('via')} ở {v['file']}:{v['line']}"
        )
    if out["violations"] > 6:
        out["problems"].append(f"… và {out['violations'] - 6} bất nhất nữa")
    for w in rep.get("lawWarnings", [])[:6]:
        out["problems"].append(f"luật sinh ra đã có cảnh báo: dòng {w['line']}: {w['msg']}")

    out["check_ms"] = round(_median_ms(archctl, ["check", *root], rounds))
    one = pick_source(work)
    if one:
        out["one_file_ms"] = round(_median_ms(archctl, ["check", *root, str(one)], rounds))
    return out


def pick_source(work: Path):
    """Một file nguồn thật nằm trong `work`, để đo hoặc để sửa.

    Không nhận symlink: repo tải về là mã người khác, và `flows` ghi thêm vào
    file này — một `src/x.rs` trỏ tới `~/.zshrc` thì bộ đo sẽ sửa file ngoài
    thư mục làm việc.
    """
    base = work.resolve()
    for ext in (".rs", ".py", ".ts", ".go", ".c"):
        for p in work.rglob(f"*{ext}"):
            if p.is_symlink() or not p.is_file():
                continue
            if {".git", "target", "node_modules"} & set(p.relative_to(work).parts):
                continue
            if base in p.resolve().parents:
                return p
    return None
