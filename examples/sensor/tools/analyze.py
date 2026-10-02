"""Phân tích kết quả bắt gói.

import khong_phai_import
from day import kia
"""

import numpy as np

from .lib.io import read_capture

note = "import cung_khong_phai"


def main():
    data = read_capture("out.pcap")
    return np.mean(data)
