import torch.nn as nn

from mlapp.ops import fused_gelu


class Block(nn.Module):
    def __init__(self, dim: int):
        super().__init__()
        self.attn = nn.MultiheadAttention(dim, num_heads=4)
        self.up = nn.Linear(dim, 4 * dim)
        self.down = nn.Linear(4 * dim, dim)

    def forward(self, x):
        return self.down(fused_gelu(self.up(x)))


class Net(nn.Module):
    def __init__(self, vocab: int = 1000, dim: int = 64, depth: int = 4):
        super().__init__()
        self.embed = nn.Embedding(vocab, dim)
        self.blocks = nn.ModuleList([Block(dim) for _ in range(depth)])
        self.head = nn.Linear(dim, vocab)


def for_inference(model: Net) -> Net:
    # `model.eval()` là chế độ suy luận của PyTorch, không phải `eval` của Python.
    model.eval()
    return model
