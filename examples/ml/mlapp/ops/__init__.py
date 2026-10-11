"""Lớp bọc op C++ trong `csrc/`. Mọi nơi khác gọi qua đây."""
import torch


def fused_gelu(x):
    return torch.ops.mlapp_ops.fused_gelu(x)
