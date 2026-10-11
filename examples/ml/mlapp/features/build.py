from mlapp.data.loader import load


def build(path: str):
    frame = load(path)
    return frame.dropna()
