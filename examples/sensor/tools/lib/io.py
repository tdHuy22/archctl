import numpy as np

# Vi phạm: thư viện import ngược lên tầng ứng dụng, tạo phụ thuộc vòng.
from ..analyze import note


# Vi phạm: ToolsLib chạm thẳng bảng của Capture thay vì đi qua nó.
QUERY = "SELECT ts, len FROM packets WHERE len > 0"
SAFE = "bảng packets_archive không tính, vì khác tên"


def read_capture(path: str):
    return np.zeros(len(path) + len(note))
