import pandas as pd
import torch  # vi phạm: khâu dữ liệu không được biết gì về torch


def load(path: str) -> pd.DataFrame:
    return pd.read_csv(path)


def as_tensor(frame: pd.DataFrame):
    return torch.tensor(frame.values)
