"""Chụp kiến trúc mô hình theo layer rồi so với bản đã duyệt.

archctl không đọc được layer: `Net` dựng `Block` trong vòng lặp, và số tầng
lấy từ tham số. Chạy thật thì `repr(model)` in ra cả cây module. Bản đã duyệt
nằm trong `model_arch.txt`. Lần đầu, hay khi đổi kiến trúc có chủ ý, chạy
`UPDATE_ARCH=1 pytest tests/test_model_arch.py` rồi để người duyệt đọc diff
của file đó.
"""
import os
from pathlib import Path

from mlapp.models.net import Net

APPROVED = Path(__file__).with_name("model_arch.txt")


def test_kien_truc_mo_hinh_khop_ban_da_duyet():
    actual = repr(Net()) + "\n"
    if os.environ.get("UPDATE_ARCH") == "1":
        APPROVED.write_text(actual)
    assert APPROVED.exists(), "chưa có bản đã duyệt: chạy với UPDATE_ARCH=1 rồi đọc file sinh ra"
    assert actual == APPROVED.read_text(), "kiến trúc mô hình khác bản đã duyệt"
