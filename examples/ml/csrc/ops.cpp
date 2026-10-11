#include <torch/library.h>

at::Tensor fused_gelu(const at::Tensor& x);

TORCH_LIBRARY(mlapp_ops, m) {
  m.def("fused_gelu(Tensor x) -> Tensor");
  m.impl("fused_gelu", &fused_gelu);
}
