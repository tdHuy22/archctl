import torch

from mlapp.features.build import build
from mlapp.models.net import Net


def train(path: str):
    model = Net()
    opt = torch.optim.AdamW(model.parameters())
    data = build(path)
    # vi phạm: gọi thẳng op C++, bỏ qua lớp bọc `mlapp.ops`
    warm = torch.ops.mlapp_ops.fused_gelu(torch.zeros(1))
    return model, opt, data, warm
