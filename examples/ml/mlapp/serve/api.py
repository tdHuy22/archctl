import pickle

from mlapp.models.net import Net
from mlapp.train.loop import train  # vi phạm: phục vụ không được kéo mã huấn luyện vào


def load_model(blob: bytes) -> Net:
    return pickle.loads(blob)  # vi phạm: pickle chạy được mã tuỳ ý khi nạp


def retrain(path: str):
    return train(path)
