"""Xuất log câu hỏi của người dùng ra file, cho stage `export_feedback` của dvc."""
import json


def export(rows, path: str = "data/feedback.jsonl") -> None:
    with open(path, "w") as f:
        for row in rows:
            f.write(json.dumps(row) + "\n")
