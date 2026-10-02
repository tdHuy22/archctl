import numpy as np

# Vi phạm: thư viện import ngược lên tầng ứng dụng, tạo phụ thuộc vòng.
from ..analyze import note


def read_capture(path: str):
    return np.zeros(len(path) + len(note))
