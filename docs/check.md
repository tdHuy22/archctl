# `archctl check` — ngữ nghĩa và giới hạn

Hiện kiểm **luật phụ thuộc giữa component** cho năm ngôn ngữ.

## Ngôn ngữ đã hỗ trợ

| Ngôn ngữ | Phần mở rộng | Nhận được | Phân giải |
| --- | --- | --- | --- |
| TypeScript, JavaScript | `.ts .tsx .mts .cts .js .jsx .mjs .cjs`, `.vue` (chỉ phần `<script>`) | `import`, `import type`, `import { type A, b }`, `export … from`, `import x = require()`, `require()`, `import()` | tương đối, `index.*`, `.js`→`.ts`, alias `paths` trong tsconfig, **gói anh em trong workspace npm** |
| Python | `.py .pyi`, Cython `.pyx .pxd .pxi`, notebook `.ipynb` (chỉ ô code) | `import a.b`, `import a as b, c`, `from .x import y`, `cimport`, `from x cimport y` | tương đối theo số dấu chấm; tuyệt đối thử gốc repo và **mọi thư mục có `pyproject.toml`, `setup.py`, `setup.cfg`, `manage.py`** (cùng `src/`, `lib/` của chúng), package chứa file gọi trước; **gói không gian tên (PEP 420)**; `from pkg import sub` với `sub` là module con thì đích là **cả `pkg` lẫn `pkg/sub`** |
| C, C++ | `.c .h .cc .cpp .cxx .hpp .hh .hxx .inl`, CUDA `.cu .cuh` | `#include "x"`, `#include <x>` | **thư mục `-I` đọc từ build**; `"…"` ưu tiên thư mục file gọi; không có build thì khớp tên **duy nhất** trong repo |
| Rust | `.rs` | `use`, `pub use`, cây `use a::{b::{c, d as e}, self}`, `use {a, b}`, `extern crate` | theo cây module; **gốc crate của từng file** (thư viện, binary, test, đường dẫn khai trong `Cargo.toml`); nhìn xuyên re-export ở gốc; các crate anh em trong workspace |
| Go | `.go` | `import "x"`, khối `import ( … )` | đọc `module` trong mọi `go.mod` tới độ sâu 4 (trừ `testdata/`, `vendor/`, thư mục ẩn) để tách nội bộ với bên ngoài; module dài nhất thắng |

Mọi ngôn ngữ dùng chung một lexer cấu hình được. Nó biết bỏ qua chú thích,
chuỗi, char literal, raw string, docstring và regex theo từng ngôn ngữ — đó là
chỗ dễ sai nhất, nên chỉ viết một lần.

`.inc` cố ý không được quét: abseil dùng nó cho C++, nhưng curl dùng cho
Makefile và scikit-learn cho reStructuredText. `archctl scan <file>` in ra
đúng những import archctl đọc được và đích của từng cái — xem
[bên dưới](#archctl-scan-archctl-đọc-được-gì).

### Một dấu nháy lạc chỉ hỏng một dòng

Lexer không hiểu ngữ pháp, nên chỗ nó hiểu sai một dấu nháy là chỗ nó lệch
nhịp: chuỗi thành mã, mã thành chuỗi. Trước đây một lần lệch kéo tới hết file.
Một dòng `#error "... \` nối dòng làm cả `redis/src/tls.c` bị đọc ngược — clang
thấy 31 lời gọi `serverLog`, archctl thấy 0, và một luật `api` cấm hàm nào đó
không thấy gì trong cả file TLS.

Giờ chuỗi thường và char literal **dừng ở cuối dòng**, trừ khi nối bằng `\`
(kể cả `\` + CRLF), ở mọi ngôn ngữ mà chuỗi không được bắc qua dòng. Chỉ chuỗi
của Rust được bắc qua dòng. Một dấu nháy lạc — chữ trong JSX như
`<h1>I'm not a pretzel!</h1>` (từng nuốt 386 dòng của `excalidraw-app/App.tsx`),
regex sau `)` bị đọc thành phép chia — chỉ còn hỏng phần còn lại của dòng đó.

Rồi từng chỗ châm ngòi tìm được trên repo thật được sửa tận gốc:

| Chỗ châm ngòi | Ở đâu | Giờ |
| --- | --- | --- |
| `#define X 1 /* ... it's ...` — chú thích mở trên dòng chỉ thị | redis `server.h`, libbpf, zstd | chỉ thị được bỏ qua tới hết chú thích và hết các dòng nối bằng `\`; `#define S "/*"` không mở chú thích nào |
| `#error "... \` nối sang dòng chữ | redis `tls.c`, matplotlib | như trên |
| `0xFFFF'FFFF`, `1'000'000` (C++14, C23) | abseil `hash.h`, `numbers.cc` | dấu phân cách chữ số, không phải char literal |
| `R"(...)"`, `R"delim(...)delim"`, `R"""(...)"""` | abseil, matplotlib (pybind11) | chuỗi thô C++11, không ký tự thoát |
| `` `C:\` ``, `` `${HOME` `` trong Go | hugo 15 file, etcd 5 | chuỗi thô Go: không thoát, không nội suy; backtick ở Python, C, Rust chỉ là một ký hiệu |
| `br"\"`, `cr#"..."#` | Rust | chuỗi thô như `r"..."` |
| `\"""` trong chuỗi ba nháy | idlelib `pyparse.py` | `\"` không đóng chuỗi |
| `f"{x["k"]}"`, `f"{(` xuống dòng, `t"{...}"` | Python 3.12 (PEP 701), 3.14 (PEP 750) | `{...}` trong f-string và t-string là mã: chuỗi lồng, chú thích, xuống dòng |
| BOM đầu file | file lưu từ trình soạn trên Windows | không còn là token, nên `#include <x>` ở dòng 1 vẫn là chỉ thị |

`\` cuối dòng trong chuỗi cũng làm lệch **số dòng** của mọi token phía sau:
vllm `deepseek_v2.py:488` lệch 1, matplotlib `test_axes.py` lệch 3. Giờ dòng đó
được đếm.

### Đối chiếu với parser chuẩn

Phép tự nhất quán của bộ đo so archctl với chính nó — bộ khám phá và bộ kiểm
dùng chung bộ trích — nên một lỗi trong bộ trích làm cả hai sai giống nhau mà
vẫn ra "0 bất nhất". [bench/oracle.py](../bench/oracle.py) đối chiếu với parser
chuẩn của từng ngôn ngữ trên mọi repo của bộ đo, và CI chạy nó ở mỗi pull
request. Bảng dưới là lần đo khi bộ đo có 24 repo:

| Ngôn ngữ | Thước đo | Kết quả 2026-10-04 |
| --- | --- | --- |
| Python | `ast` của CPython 3.14 | 13 545 file, 103 539 import: khớp cả tên lẫn dòng. Thư viện chuẩn của chính 3.14 (1 865 file, 13 599 import) cũng khớp. |
| C, C++ | lexer thô của clang 21 | 4 239 file, 23 044 `#include`: khớp cả tên lẫn dòng |
| JS, TS | parser module của Node 24 | 1 007 file, 3 310 import tĩnh: không thiếu cái nào |
| Go | `go/parser` của Go 1.27, chế độ `ImportsOnly` | 2 168 file, 13 921 import: khớp cả tên lẫn dòng |
| Rust | `syn` 2.0.87, chỉ chạy trên CI | 1 509 file, 18 303 `use`/`extern crate`: khớp cả tên lẫn dòng; 548 import nằm trong thân macro (`syn` không đọc vào) không đối chiếu, 534 trong số đó ở tokio (`cfg_rt! { ... }`) |
| Notebook (2026-10-11) | `TransformerManager` của IPython 9.17.1 cộng `ast`, chỉ chạy trên CI | 3 notebook của matplotlib, 16 import: khớp cả tên lẫn dòng. Các notebook khó trong `tests/notebook.rs` (magic trong chuỗi `"""`, `%%time`, `%%script python3`, khoá trùng, dòng trống đầu ô) cũng khớp |
| Binding (2026-10-11) | token thô của clang cho C/C++, `ast` cho `torch.ops`, `syn` cho PyO3 | vllm 287 file, 1 470 đầu cạnh; apex 34; matplotlib 14; pydantic-core 1 (PyO3): khớp cả khoá lẫn dòng ngay lần đầu |
| Proto (2026-10-12) | `protoc` của grpcio-tools 1.84.0, đọc thành FileDescriptorSet; googleapis-common-protos cho import `google/api`; cài vào venv nên cần mạng; CI bật | package, service, rpc khớp cả tên lẫn dòng: vllm 2 file 54 dữ kiện, etcd 5 file, google-cloud-python 2, opentelemetry-python 2. File mà `protoc` không dựng được vì import nằm ngoài repo (`gogoproto` của etcd) bị bỏ qua |
| Hạ tầng (2026-10-12) | PyYAML 6.0.3 (`compose` giữ số dòng, `<<` trải bằng `flatten_mapping`) cho compose, dockerfile-parse 2.0.1 cho Dockerfile, cài vào venv nên cần mạng; CI bật | 12 repo, 49 file compose và Dockerfile, 4 792 dữ kiện (service, build, lệnh, khoá env, `depends_on`, bind mount, alias, chỉ thị Dockerfile): khớp cả giá trị lẫn dòng; vllm chiếm 3 830. Hai Dockerfile có heredoc của BuildKit (hugo, vllm) bị bỏ qua vì dockerfile-parse không đọc heredoc. Bộ đọc YAML còn được so riêng với PyYAML trên 804 file YAML của bộ đo: 793 khớp, phần lệch là template Helm và anchor trùng tên, không phải compose |
| Kubernetes (2026-10-13) | PyYAML 6.0.3 cùng venv hạ tầng, [k8s_truth.py](../bench/k8s_truth.py): dữ kiện từng manifest và `skaffold.yaml`, và mọi cạnh qua Service tính lại độc lập theo luật DNS, selector của tài liệu Kubernetes | 3 repo (microservices-demo, example-voting-app, etcd), 87 file, 455 dữ kiện (workload, nhãn, container, image, lệnh, khoá env, tham chiếu ConfigMap, Service, selector, NetworkPolicy) cộng 54 cạnh và kết quả NetworkPolicy (cô lập, cho phép rõ) của từng workload: khớp hết. Template Helm bỏ qua ở cả hai phía. Ngữ nghĩa NetworkPolicy còn có ca vàng viết tay theo tài liệu Kubernetes trong test |

Trước khi sửa, cũng phép đối chiếu đó cho:

- **TypeScript**: `import { type A, b } from './x'` mất cả câu, vì `type` bị coi
  là mở câu lệnh mới — Vue core mất 363/1 728 import tĩnh (21%), excalidraw
  59/1 433, sơ đồ tự dựng của Vue core thiếu `RuntimeCore -> CompilerDom`.
- **Python**: `raise E from e` và `yield from x` thành 1 045 import giả, và
  import thật ngay sau chúng bị nuốt 15 lần (mẫu `except ImportError: raise
  ... from e` rồi import tiếp). `from a \` xuống dòng `import b` sinh import
  giả `b`; `import a, \` xuống dòng `b` mất `b`; `from x import y; import z` mất
  `z`.
- **C/C++**: `#include` mất 2/21 830 — ít, vì include nằm đầu file, trước chỗ
  lệch nhịp. Thiệt hại thật nằm ở phần sau: ký hiệu, ghép FFI.
- **Go**: `import bolt "go.etcd.io/bbolt"`, tức tên đặt lại mà không có ngoặc,
  bị bỏ qua cả câu: etcd mất 4 import, hugo 1. Trong khối `import ( ... )` thì
  vẫn đọc đúng.

Rust đối chiếu với `syn` cần tải crate, nên chỉ chạy khi có `--require syn`;
CI bật cờ đó, máy dev mặc định bỏ qua. Lần đầu chạy, Rust khớp hết.

## Cạnh liên ngôn ngữ

Phân tích import chỉ thấy phụ thuộc **trong cùng một ngôn ngữ**. Rust gọi C
không qua câu lệnh import nào.

Nhưng một phần lớn FFI viết tay là **khai báo tường minh**:

```rust
// src/decode/ethernet.rs
unsafe extern "C" {
    fn parse_ethernet(buf: *const u8, len: usize) -> i32;
}
```
```c
// native/parser.c
int parse_ethernet(const unsigned char *buf, unsigned long len) {
```

Hai đầu mang cùng một cái tên, và cái tên đó phải khớp thì chương trình mới
liên kết được. Ghép chúng lại là ra cạnh — không cần đọc binary, không cần
bảng ký hiệu:

```
✗ VI PHẠM  Decode -> Native  (không được phép, #accepted)
  src/decode/ethernet.rs:4  gọi `parse_ethernet` qua ranh giới ngôn ngữ
```

| Bên gọi | Nhận bằng |
| --- | --- |
| Rust | `extern "C" { fn x; }` — ABI nào cũng được; không ghi ABI (`extern { … }`) là "C" |
| Go | `import "C"` rồi `C.x` |
| Bên cung cấp | |
| C, C++ | định nghĩa hàm ở mức ngoài cùng, kể cả bên trong `extern "C" { … }` |
| Rust | `pub extern "C" fn x`, `extern fn x` |

Ghép theo tên, nên nó thận trọng ở mấy chỗ:

- **Khai báo trước không phải là cung cấp.** `int f(void);` kết thúc bằng `;`
  chứ không mở thân hàm.
- **Lời gọi lồng trong thân hàm khác không phải là định nghĩa.** Chỉ xét khi
  độ sâu ngoặc nhọn bằng 0. Khối `extern "C" { … }` không tính là một tầng —
  mã C++ đưa hàm ra cho C gọi thường bọc chúng trong đó — còn `namespace` thì
  có: hàm C++ trong namespace mang tên đã mangle, Rust hay Go không liên kết
  tới được.
- **Hai component cùng cung cấp một tên thì bỏ cả hai.** Không biết chọn đâu,
  mà đoán sai sinh ra một cạnh không có thật.
- **Không ghép được thì im lặng.** `memcpy` nằm trong libc, không có định
  nghĩa nào trong repo. FFI tới thư viện hệ thống là chuyện thường.
- **Macro bọc hàm không phải là định nghĩa hàm C.** `s32 BPF_STRUCT_OPS(name, …) {`
  là macro bọc hàm `name`; ở phía Go, `C.BPF_STRUCT_OPS` lại là hằng enum.
  Nhận ra bằng hai dấu hiệu: tên viết hoa toàn bộ, và đối số đầu là một tên
  trần. `SHA256(const unsigned char *d, …)` vẫn là hàm.
- **`C.CString`, `C.int` của cgo tự rụng**, vì chúng không khớp định nghĩa nào
  trong repo.

### Python gọi mã native

Python không khai báo tên hàm native như `extern "C"`. Hai đầu có chung một
thứ khác: **tên module mở rộng**, và với PyTorch là **tên op**.

```python
# python/pkg/__init__.py                 // src/lib.rs
from ._core import Validator             #[pymodule] mod _core { … }

# vllm/_custom_ops.py                    // csrc/torch_bindings.cpp
torch.ops._C.rotary_embedding(…)         STABLE_TORCH_LIBRARY_FRAGMENT(_C, ops) {
                                           ops.def("rotary_embedding(Tensor positions, …)");
```

| Bên cung cấp | Nhận bằng |
| --- | --- |
| Rust (PyO3) | `#[pymodule]` trên `fn x` hoặc `mod x`; `#[pyo3(name = "...")]` đổi tên |
| C, C++ | `PYBIND11_MODULE(x, m)`, `NB_MODULE(x, m)` (nanobind), định nghĩa `PyInit_x` của C API; `x` là `TORCH_EXTENSION_NAME` thì tên đọc từ `setup.py` — xem dưới |
| C++ (PyTorch) | `m.def("op(…)")` trong khối `TORCH_LIBRARY*(ns, m) { }`, kể cả `FRAGMENT`, `STABLE_`, macro bọc lại như `TORCH_LIBRARY_EXPAND` |
| Bên gọi | |
| Python | import một module mà repo không có mã Python nào cho nó; `torch.ops.ns.op` |

Chỗ dễ sai là phía gọi, và luật ở đó chặt:

- **Chỉ import không ra mã Python.** Module không có file nào, hoặc chỉ có stub
  `.pyi` — pydantic-core có `_pydantic_core.pyi` nằm cạnh, và stub chỉ mô tả
  kiểu. Có `_core.py` thật thì đó là cạnh Python thường.
- **Chỉ module trong package của chính repo.** `torch._C` là module mở rộng
  của PyTorch. Repo có `PYBIND11_MODULE(_C, m)` cũng không làm `import
  torch._C` thành cạnh.
- **Cùng một tên, nhiều chỗ nạp thì phải chọn.** Phía native chỉ đặt tên
  module, còn nó nằm trong package nào là do cấu hình build. Có stub `.pyi`
  thì stub nói module nằm đâu: `pkgb` import `._core` mà stub ở `pkga` thì
  `pkgb._core` là thứ khác và vẫn bị báo chưa phân giải. Không có stub thì
  mọi import phải trỏ cùng một chỗ; hai package cùng nạp `_core` của riêng
  mình thì không ghép bên nào.
- **Op ghép theo cả namespace lẫn tên.** `torch.ops.aten.add` không bao giờ
  ghép với `mylib::add`. `TORCH_LIBRARY_IMPL` không tính là cung cấp: nó chỉ
  gắn kernel cho op đã định nghĩa ở chỗ khác.
- **Namespace viết bằng macro thì không ghép.** `TORCH_EXTENSION_NAME` do hệ
  thống build định nghĩa. Đã thử coi nó là một module mở rộng mà Python
  import, nhưng vllm import cả chục module như vậy, và
  `torch.ops._vllm_fa3_C.get_scheduler_metadata` (op của flash-attention, tải
  lúc build) ghép nhầm vào op CPU cùng tên. Phần này bỏ sót chứ không đoán:
  trên vllm, 265 lời gọi ở 54 file ghép được, và cả 161 tên op đều có chuỗi
  schema tương ứng trong `csrc/`.
- **`TORCH_EXTENSION_NAME` đọc từ `setup.py`.** Extension dựng bằng
  `torch.utils.cpp_extension` viết `PYBIND11_MODULE(TORCH_EXTENSION_NAME, m)`
  hay `TORCH_LIBRARY(TORCH_EXTENSION_NAME, m)`, và tên thật là đoạn cuối của
  tên trong `CppExtension`/`CUDAExtension("pkg._C", sources)`, kể cả gọi qua
  biến (`extension = CUDAExtension`). File C++ có trong danh sách nguồn viết
  thẳng của extension nào thì mang tên extension đó. Danh sách nguồn tính lúc
  chạy (`glob`) thì chỉ ghép khi `setup.py` khai đúng một extension; nhiều
  extension như vllm thì không đoán. Tên đã khai trong `setup.py` cũng làm
  import ở cấp cao nhất (`import amp_C` của apex) thành module của dự án. Trên
  apex: 42 lời gọi tới 19 module, cả 19 đối chiếu được với `setup.py` bằng
  `ast`. Tên đặt qua biến (`name=ext_name` của mmcv) thì không đọc.
- **Bộ đo đọc lại cả hai phía bằng thước đo khác.** `bench/oracle.py` so
  khoá binding archctl đọc được ở từng file với token thô của clang (C/C++),
  `ast` (`torch.ops` phía Python) và `syn` (PyO3) — xem bảng ở
  [Đối chiếu với parser chuẩn](#đối-chiếu-với-parser-chuẩn). Lần đầu chạy nó
  bắt được chuỗi schema có `\` nối dòng (`"eig\` xuống dòng `ht(…)"`): lexer
  từng giữ ký tự xuống dòng trong chuỗi, nên op `eight` mất. "Khớp" không có
  nghĩa là phủ hết: cả hai phía cùng bỏ `PyInit_` nằm trong namespace hay có
  `noexcept`, cùng tính cả hai nhánh `#if`/`#else`, cùng bỏ schema viết `u8"…"`,
  và đuôi `.ipp`, `.tcc`, `.mm` không được quét. Ngược lại, `torch.ops` viết
  xuống dòng hay nằm trong f-string thì `ast` thấy còn archctl không, và bộ đo
  sẽ báo thiếu ở đó.
- **Import của module đã ghép được không còn bị báo "chưa phân giải".** Nó
  không ra file Python nào, nhưng nơi định nghĩa nó đã tìm thấy.

### Vẫn còn vô hình

- **Liên kết động** — `dlopen` cộng tên hàm dựng lúc chạy
- **Con trỏ hàm và vtable**
- **`ctypes` và `cffi` của Python** — tên hàm nằm trong chuỗi, và thư viện
  được chọn lúc chạy
- **Op PyTorch qua biến trung gian** (`ops = torch.ops._C; ops.x()`), qua
  `getattr`, hay có namespace là macro mà `setup.py` không cho biết giá trị;
  op đăng ký bằng Python (`torch.library`)
- **Module mở rộng nằm thẳng ở gốc** (`import _core` không qua package nào):
  không phân biệt được với gói bên ngoài
- **Tiến trình con, syscall, IPC, mạng**

`archctl check --changed` chỉ ghép trong tập file đang xét, nên nó có thể bỏ
sót cạnh liên ngôn ngữ khi phía cung cấp không nằm trong tập. Lần quét đầy đủ
— và CI — vẫn thấy. Cũng vì thế, ở đường hook một import module mở rộng không
có stub vẫn bị báo chưa phân giải, dù lần quét đầy đủ thì không.

**Hệ quả cho mã hệ thống và bảo mật:** với C/C++, `#include` là điều kiện cần
nhưng rất xa điều kiện đủ. Loại luật phù hợp hơn cho các mảng đó là **hàm bị
cấm** — xem [api.md](api.md). Nó không có vùng mù này, vì một lời gọi `strcpy`
phải xuất hiện trong mã nguồn thì mới chạy được.

## Cách một component vào diện kiểm

Component bị kiểm khi có ít nhất một `allow` **đang có hiệu lực** nêu đúng tên
nó làm nguồn:

```
allow Stock -> Notify      # từ đây Stock bị kiểm
```

Chưa viết `allow` cho component nào thì component đó tạm để yên, và báo cáo nói
rõ điều đó. Đây là lối vào cho codebase đã có: áp dần từng component, không phải
mô tả cả repo trước khi có được giá trị nào.

`allow * -> X` chỉ **cấp quyền**, không đưa ai vào diện kiểm.

Ngoại lệ: **`deny` luôn có hiệu lực**, kể cả với component chưa vào diện kiểm.
Luật cấm không nên phải chờ ai bật.

## Thứ tự xét

1. Có `deny` đang có hiệu lực khớp quan hệ, và nguồn không nằm trong `except` → **vi phạm**
2. Có `allow` đang có hiệu lực khớp quan hệ → hợp lệ
3. Nguồn đã vào diện kiểm → **vi phạm**, không có allow nào cho phép
4. Còn lại → chưa kiểm

`deny` thắng `allow` khi cả hai cùng khớp. `except` miễn trừ cho **nguồn**:
`deny * -> LibPcap except Capture` nghĩa là chỉ Capture được chạm libpcap.

Luật `#draft` và `#proposed` không có hiệu lực, nên cũng **không cấp quyền**.

## Thư viện bên ngoài

Chỉ tham gia luật khi được khai báo `external`, với tên định danh trong `at`:

```
external Stripe  at "stripe"                  # gói npm
external NumPy   at "numpy"                   # gói Python
external LibPcap at "pcap.h"                  # header hệ thống C
external GoNet   at "net"                     # gói chuẩn Go
```

Thứ không khai báo thì bị bỏ qua. Bắt khai báo mọi gói npm hay mọi header hệ
thống sẽ khiến file luật dài vô ích.

## Thứ không phân giải được thì báo ra

Quyết định có chủ ý. Bỏ sót âm thầm làm người dùng tin rằng không có vi phạm,
tệ hơn hẳn việc nói thẳng là chưa biết.

| Trường hợp | Xử lý |
| --- | --- |
| `@/lib/x` khi có `tsconfig.json` khai báo | phân giải theo `paths`; monorepo thì lấy cấu hình gần file gọi nhất |
| `@/lib/x` khi không có khai báo nào | **báo ra** |
| `@acme/core` khi `packages/core/package.json` khai tên đó | gói anh em trong workspace — phụ thuộc nội bộ, không phải gói ngoài |
| `#include "x.h"` không tìm thấy | **báo ra**, có thể nằm trong include path của build |
| `#include <x.h>` nằm trong một thư mục `-I` của dự án | file của dự án — đúng như trình biên dịch sẽ thấy |
| `use super::Thing` | module cha — `Thing` là item chứ không phải module |
| `use crate::x` không ra file nào | `mod` viết lồng trong file; không có cạnh, và **không báo** — trình biên dịch đã bảo đảm đường dẫn tồn tại |
| `from .x import y` không có file nhưng có thư mục | gói không gian tên (PEP 420) — thư mục chính là gói |
| `from .x import y` không có cả thư mục | **báo ra** |
| `from pkg import name`, `name` là module con | đích là cả `pkg` lẫn `pkg/name` — Python nạp cả hai, và module con có thể ở component khác. Gói thường (có `__init__.py`) chỉ tìm trong chính thư mục của nó, không sang project bên cạnh có gói cùng tên; module file (`utils.py`) không có module con; chỉ gói không gian tên PEP 420 mới trải qua nhiều distribution |
| `from pkg import name`, `name` là hàm hay lớp | chỉ `pkg`; **không báo** — không có file nào để tìm |
| `import numpy` không có trong dự án | coi là gói — với import tuyệt đối của Python, không phân biệt được |

## Rust: `crate::` là crate nào, và `X` ở gốc là gì

`use crate::{Connection, Result}` không trỏ vào file module nào: `Connection`
là một item ở **gốc crate**. Đích của nó là nơi item đó được định nghĩa —
viết thẳng trong `lib.rs` thì là `lib.rs`; do `lib.rs` nhập vào bằng `use`
(`pub use error::Error`) thì là `error.rs`. Không nhìn xuyên thì `lib.rs`
thành chỗ rửa phụ thuộc: `deny Scan -> Model` lách được bằng `use
crate::Document`, vì `Document` đi qua `lib.rs`.

Gốc crate thì tuỳ file. Mỗi binary trong `src/bin/`, mỗi test tích hợp, mỗi ví
dụ là một crate riêng, và `crate::` trong đó trỏ về gốc của chính nó, không
phải thư viện cùng gói. Đường dẫn khai trong `Cargo.toml` (`[lib]`, `[[bin]]`,
`[[test]]`…) cũng là gốc: ripgrep đặt binary chính ở `crates/core/main.rs`,
và gói đó không có `src/` nào. File dùng chung cho nhiều crate — `tests/common/`
— thì không biết là crate nào, nên không đoán.

Trước đây cả ba đều trượt im lặng: `crate::X` với `X` ở gốc ra "không có
cạnh", không cảnh báo. Ở năm repo Rust của bộ đo có khoảng 470 câu `use` như
vậy; ở `rusqlite`, `deny Blob -> Core` cho 0 vi phạm trong khi `src/blob/`
dùng `Connection` của Core.

Hai chỗ đọc mã đi kèm, vì thiếu chúng thì bản sửa trên sinh cạnh giả:

- **`mod` viết lồng.** Trong `#[cfg(test)] mod tests { use super::*; }`,
  `super` là module của chính file, không phải module cha của nó. Bộ quét
  theo dõi khối `mod tên { … }` và đổi đường dẫn về module của file. Không có
  bước này, chính archctl có tám cạnh giả tới `lib.rs`, mỗi `mod tests` một cái.
- **Cây `use`.** Tên sau `as` là tên mới, không phải thành viên; ngoặc lồng
  giữ tiền tố của mình; `use { a::B, c::D };` — kiểu viết chuẩn của ripgrep —
  trước đây không ra import nào cả.

## Hoa thường phải khớp, trên mọi máy

`import './Util'` khi trên đĩa chỉ có `util.ts` thì **không** phân giải được —
cả trên macOS, nơi ổ mặc định không phân biệt hoa thường và `is_file()` sẽ trả
lời "có". Mọi ngôn ngữ đã hỗ trợ đều phân biệt hoa thường khi import (Python
kể cả trên macOS), nên cách viết có thật trên đĩa là cách viết duy nhất được
nhận: sau khi hệ thống file nói "có", archctl đối chiếu từng đoạn đường dẫn với
tên thật trong thư mục.

Trước đây kết quả phụ thuộc máy. Cùng một commit `rusqlite`, `init
--from-code` sinh 12 luật `allow` trên macOS và 11 trên CI Linux: `use
crate::{Error, Result}` được "tìm thấy" là file `src/Error.rs`, trong khi file
thật là `src/error.rs` và đoạn mã kia trỏ tới một kiểu chứ không phải một
module. Luật sinh ở máy này đem sang máy kia thành một `allow` không bao giờ áp
được — `--strict` chặn — hoặc một vi phạm mà máy kia không thấy.

Giá: 1–6% thời gian trên cả repo (đo xen kẽ, trung vị 21 lần), không đáng kể
ở đường hook. Lần dò trượt — phần lớn — vẫn dừng ngay ở `is_file()`; chỉ lần
dò trúng mới phải đọc thư mục, và mỗi thư mục chỉ đọc một lần.

## Include path của C/C++

`#include "aes.h"` không nói header nằm đâu — đường dẫn thật nằm trong cờ `-I`
của build. Đoán bằng vài tên thư mục thường gặp thì bỏ sót, và mất một cạnh
phụ thuộc là mất một vi phạm thật: `archctl check` sẽ in "0 vi phạm" trong khi
mã đang vi phạm.

Đọc theo thứ tự tin cậy giảm dần:

| Nguồn | Lấy gì |
| --- | --- |
| `compile_commands.json` | `-I`, `-isystem`, `-iquote`, `-idirafter` trong `arguments` hoặc `command`; đường dẫn tính từ `directory` của từng mục |
| `CMakeLists.txt`, `*.cmake` | `include_directories`, `target_include_directories`; mở `${CMAKE_SOURCE_DIR}` và `${CMAKE_CURRENT_SOURCE_DIR}` |
| `Makefile`, `*.mk` | mọi `-I` trên mọi dòng, sau khi mở biến `$(VAR)` |

**Không chạy `cmake` hay `make`.** Đọc file build của một repo lạ thì an toàn,
thực thi nó thì không.

Hai chỗ cố tình thận trọng, vì một đường dẫn đoán sai sinh ra **cạnh sai**, và
báo động giả tốn lòng tin nhiều hơn bỏ sót:

- **Biến chưa biết thì bỏ.** `${SOMETHING_ELSE}/inc` không sinh ra gì.
  `${CMAKE_BINARY_DIR}` cũng vậy: đó là header sinh ra lúc build, không có
  trong cây nguồn.
- **Hậu tố tuyệt đối phải khớp ít nhất hai đoạn.** `compile_commands.json`
  sinh trong container mang tiền tố không khớp gốc hiện tại, nên phải khớp
  theo hậu tố — nhưng nếu chấp nhận một đoạn thì `/usr/include` cũng khớp
  `include/` của repo.

Repo không có file C nào thì không phải quét tìm `Makefile`: phần này đọc muộn.

### Không có file build nào khai `-I`

nginx đặt thư mục include trong script `auto/` (`CORE_INCS="src/core"`), và
`auto/configure` mới sinh `Makefile` lúc build. Đọc cây nguồn thì không thấy
`-I` nào, nên `#include <ngx_core.h>` từng bị coi là header hệ thống: 415 file,
0 cạnh, 0 cảnh báo.

Khi **không file build nào** khai thư mục include, archctl dựa vào tên: header
có đuôi đường dẫn trùng với **đúng một** file trong repo thì là file đó, cho cả
`"…"` lẫn `<…>`. Header chuẩn của C, POSIX và Windows (`<stdio.h>`,
`<sys/types.h>`…) thì không bao giờ: zstd có `wasm-shim/stdio.h` chỉ dùng khi
build wasm, và khớp tên vào đó từng cho 363 cạnh sai. Hai
file cùng khớp thì không đoán và báo là chưa phân giải được — nguyên tắc vẫn là
cạnh sai tốn lòng tin hơn cạnh bị bỏ sót, chỉ có điều 0 cạnh trên cả repo là
bỏ sót lớn nhất.

Có file build thì không đoán: `<x.h>` ngoài các thư mục nó khai là của hệ
thống, dù repo có một file trùng tên. Bảng tên dừng ở 200 000 file; quá giới
hạn thì không đoán gì, vì dừng giữa chừng thì "duy nhất" tuỳ thứ tự đọc thư mục
và hai máy sẽ ra hai kết quả.

## Báo cáo không được dài tới mức không ai đọc

Cảnh báo lặp lại được gộp theo tên, và mỗi loại in tối đa mười hai mục:

```
⚠ CHƯA PHÂN GIẢI  'test/jemalloc_test.h'  (132 chỗ)
  deps/jemalloc/test/analyze/prof_bias.c:1  — không tìm thấy header…

⚠ … còn 17 tên nữa chưa phân giải được — xem `--json`
```

Trên một cây 5078 file gộp từ mười repo thật, một lần `check` từng in ra 7247
dòng; giờ là 49. redis đi từ 719 xuống 43, hugo từ 960 xuống 44. Danh sách dài
tới mức đó thì người đọc bỏ qua cả những dòng đáng đọc, nên cắt bớt làm nó hữu
ích hơn chứ không phải kém đi.

`--json` vẫn đưa đủ, không cắt gì.

## Ba nhóm cảnh báo, và `--strict` chặn hai nhóm

Mọi cảnh báo đều được in, và đều vào con số ở dòng tổng kết. Nhưng chúng không
cùng một loại, nên `--strict` không đối xử như nhau.

**Đã thử kiểm và không kiểm được** — `--strict` chặn. Tên không phân giải
được, file quá 4 MB, khai báo lạc hậu, `allow` không bao giờ áp được vào đâu,
mẫu `at` chồng nhau, biên nhận hết hạn; ở tầng hạ tầng: file compose,
Dockerfile hay manifest không đọc được (kể cả loại workload lạ và patch
Kustomize đổi lệnh hay env của workload đã khai), service có `runs` mà không rõ điểm vào (kể cả khi
bộ đọc thôi đọc lệnh khởi chạy giữa chừng), `link` không cấu hình nào cho thấy, `link` bị NetworkPolicy chặn
([infra.md](infra.md)). Mỗi cái là một chỗ báo cáo đang
im lặng mà không phải vì sạch.

**Luật không ai kiểm được** — `--strict` chặn. `contract` không có verify,
`invariant` chỉ là ghi chú, component không có `at`. Nhóm này đáng chặn hơn
nhóm trên: một import không phân giải được chỉ là một chỗ tối, còn một
`contract` không có verify là một luật **có mặt trong file**, đọc lên y như
luật đang được áp — ai xem sơ đồ cũng tưởng nó đang được giữ.

**Chưa khai báo** — `--strict` **không** chặn. Component chưa có `allow` nào,
file không thuộc component nào, service trong compose hay workload trong
manifest chưa có `service` nào khai. Áp dần là cách dùng được thiết kế: `init
--from-code` sinh ra toàn `#draft`, và một repo đang chạy thì luôn có phần
chưa vào sơ đồ. Chặn nhóm này nghĩa là không ai áp được `--strict` cho tới khi
đã mô tả xong cả repo, và chẳng ai làm thế. Xem [adopt.md](adopt.md).

Trước đây nhóm thứ hai chỉ được in chứ không được đếm: dòng cuối nói "0 cảnh
báo" trong khi ba dòng ⚠ nằm ngay phía trên, và `--strict` trong CI cho qua
một file luật đầy luật không ai kiểm.

## Giới hạn tài nguyên

File trên **4 MB** không được dò, và được báo ra:

```
⚠ QUÁ LỚN  src/web/bundle.min.js (30 MB) — không dò
  → file sinh ra hay đã minify thì thêm vào .arch/.archignore
```

Bộ dò giữ cả dãy token trong bộ nhớ, nên một bundle đã minify 32 MB ngốn hơn
một gigabyte. Trong CI có hạn mức bộ nhớ thì đó là một lần bị giết, và triệu
chứng không chỉ ra được nguyên nhân. Một file nguồn do người viết gần như
không bao giờ tới mức đó; file sinh ra ở cỡ đó thì không chứa thông tin kiến
trúc nào.

Mẫu glob cũng bị chặn: bộ khớp chỉ giữ một điểm quay lui nên không có mẫu nào
làm nó chạy theo hàm mũ. Điều này quan trọng vì mẫu có thể do agent đề xuất
rồi được `archctl accept` ghi vào file luật.

## Gốc import của Python

`from dcim.models import Device` tính từ đâu là tuỳ chương trình chạy từ đâu,
và mã nguồn không nói ra. archctl coi những thư mục sau là gốc: gốc repo, và
mọi thư mục có `pyproject.toml`, `setup.py`, `setup.cfg` (một package) hay
`manage.py` (một dự án Django), tới độ sâu 4, trừ `vendor/`, `dist/`,
`node_modules/` và thư mục ẩn. Mỗi gốc thử chính nó, `src/` và `lib/`.

Thứ tự: các gốc **chứa file gọi** trước, sâu nhất trước — `tests` của
`libs/core` là `tests` của nó, không phải của package bên cạnh. Rồi tới các gốc
còn lại, và ở đó tên phải duy nhất: hai package cùng có `shared` thì không đoán.

Thư mục mà `pyproject.toml` khai cũng là gốc: `[tool.hatch.build.targets.wheel]`
`sources` hay `packages`, `[tool.setuptools.packages.find]` `where`,
`package-dir`, và `from` trong `packages` của poetry. open-webui để mã trong
`backend/` với `sources = ["backend"]`: thiếu dòng đó thì 11 component của nó ra
0 cạnh. Đường dẫn khai ra ngoài project hay tuyệt đối thì bỏ.

Trước đây chỉ gốc repo, `src/` và `lib/` được thử. netbox (mã trong `netbox/`,
chạy từ đó) và langchain (mỗi package một thư mục trong `libs/`) ra 0 cạnh, 0
cảnh báo: mọi import tuyệt đối thành gói bên thứ ba.

## Giới hạn khác đã biết

- **`flow`, `contract`, `boundary`, `invariant` kiểm được một phần** — xem
  [declarations.md](declarations.md). Thứ tự bước và nội dung ngữ nghĩa thì
  chưa.
- **Chỉ thị tiền xử lý không được diễn giải.** Include trong `#ifdef` vẫn được
  tính. Có chủ ý: đoán nhánh nào được biên dịch cần biết toàn bộ cờ build, và
  phân tích phụ thuộc nên thận trọng.
- **Bộ quét không phải parser đầy đủ.** Nhận regex bằng suy đoán theo token
  đứng trước, như các bộ minify. Nếu độ chính xác không đủ, thay `scan/` bằng
  tree-sitter — ranh giới module đã được đặt sẵn cho việc đó.
- **Rust: vài đường đi qua gốc crate vẫn dừng ở file gốc.** Re-export kiểu
  `pub use self::types::*` không cho biết tên nào đi qua, và macro
  `#[macro_export]` luôn nằm ở gốc dù được định nghĩa ở đâu: cả hai trỏ về
  `lib.rs` thay vì nơi định nghĩa. `#[path = "…"]` không được đọc.
- **Rust: `use con::X` từ một module không phải gốc bị coi là crate ngoài.**
  Edition 2018 cho phép gọi thẳng module con đang trong phạm vi — `mod pos_io;
  use pos_io::X;` trong `src/blob/mod.rs` — nhưng bộ phân giải chỉ tìm tên đó
  từ gốc crate, không thấy, nên xếp là gói ngoài. `self::pos_io::X` thì đúng.
  Module con gần như luôn nằm cùng thư mục, tức cùng component, nên hiếm khi
  mất một cạnh có nghĩa.

## Phạm vi quét

| Lệnh | Quét gì | Thời gian trên 2000 file |
| --- | --- | --- |
| `archctl check` | toàn repo | ~120ms |
| `archctl check --changed` | file đổi so với HEAD, hỏi git | ~24ms |
| `archctl check <file>...` | đúng những file đó | ~11ms |

Chia nhỏ được vì luật phụ thuộc, `api` và `store` đều xét **từng file một**.
Dạng thứ ba dành cho hook: nó đã biết file nào vừa đổi nên không cần hỏi git.

Nếu chính `.arch/system.arch` vừa đổi thì `--changed` tự chuyển sang quét toàn
bộ — luật đổi thì mọi file đều phải xét lại.

## Truy cập dữ liệu: `store owned_by`

```
store Captures owned_by Capture at "packets", "sessions"
```

Chỉ `Capture` được chạm hai bảng đó. Đây là một `deny` **ngầm**: nó không chờ
component vào diện kiểm, nhưng một `allow` viết tay vẫn thắng được nó.

Tên bảng **chỉ được dò trong chuỗi** — SQL thô, query builder, tên collection:

```
"SELECT ts FROM packets"      → bắt
knex('packets')               → bắt
db.collection('packets')      → bắt
struct S { packets: u64 }     → KHÔNG, đó là trường struct
"FROM packets_archive"        → KHÔNG, bảng khác
```

Dò cả trong định danh sẽ bắt được `prisma.packets`, nhưng cũng bắt nhầm mọi
biến và trường tên `packets`. Phân biệt hai thứ đó cần thông tin kiểu. Muốn
bắt cách truy cập theo thuộc tính của ORM thì khai báo nó thành nhóm `api` —
cỗ máy đó vốn để dò định danh.

## Mẫu `at` chồng nhau

File khớp nhiều component một lúc thì component khai báo trước thắng, và báo
cáo in cảnh báo `MẪU CHỒNG NHAU`. Nên sửa mẫu thay vì dựa vào thứ tự.

## Bỏ qua thư mục

Luôn bỏ qua, không cấu hình được: `.git`, `.arch`, `node_modules`, `target`,
`.next`, `.nuxt`, `.svelte-kit`, `vendor`, `__pycache__`, `.venv`. Symlink
không được đi theo, để tránh vòng lặp và tránh ra ngoài gốc dự án.

`dist`, `build`, `out`, `coverage` thì **hỏi `.gitignore`**. Đó thường là
output, nhưng cũng là tên của mã thật: etcd có package Go
`tests/robustness/coverage/`, zstd có mã C trong `build/`. Trước đây chúng bị
bỏ theo tên, và mọi vi phạm trong đó biến mất khỏi báo cáo mà không có gì nói
ra. Giờ:

- `.gitignore` bỏ qua thư mục đó, hay mọi thứ trong nó (`dist/*`) → bỏ qua.
  Output thì nằm ở đây. `.gitignore` được đọc từ gốc repo git, kể cả khi
  `.arch/` nằm ở một thư mục con của monorepo — trừ khi chính thư mục đó bị
  repo bên ngoài ignore (bộ đo chép repo vào `target/`), khi ấy `.gitignore`
  bên ngoài không phải luật của nó. `.git/info/exclude` cũng được đọc.
- có `.gitignore` mà nó không bỏ → quét. Được commit tức là mã của repo,
  kể cả bundle mà netbox commit trong `project-static/dist/`; không muốn thì
  thêm vào `.arch/.archignore`.
- không có `.gitignore` nào để hỏi → bỏ theo tên như cũ. Một bundle sinh ra mà
  bị quét như mã kiến trúc còn tệ hơn. `.git/info/exclude` chỉ có trên một
  máy, nên nó bỏ thêm được nhưng không biến trường hợp này thành "quét".

Bộ đọc `.gitignore` chỉ đủ cho câu hỏi này: mẫu sau thắng mẫu trước, `!` đưa
trở lại, mẫu có `/` thì neo vào thư mục chứa file, thư mục cha bị bỏ thì con
cũng bị. Chưa hiểu `[abc]`. Bộ đo đối
chiếu nó với chính `git check-ignore` trên mọi repo của bộ đo.

Thêm mẫu glob vào `.arch/.archignore` để bỏ qua thêm.

## `archctl scan`: archctl đọc được gì

```
archctl scan [--root <dir>] [--json] [--verbatim] [--all-hosts] [<file>...]
```

In từng import archctl đọc được trong file, ở dòng nào, và phân giải ra đâu —
đúng những gì `check` và `init --from-code` thấy: cùng danh sách file, cùng
phần mã của file (`.vue` chỉ phần `<script>`), cùng bộ phân giải. Dùng khi một
cạnh đáng có mà không có: hỏi thẳng archctl thay vì đoán.

```
src/flask/wrappers.py (Python)
      7  werkzeug.wrappers  → gói ngoài werkzeug
     10  .  → src/flask/__init__.py
         .  → src/flask/json/__init__.py (module con)
     11  .globals  → src/flask/globals.py
```

Cạnh qua ranh giới ngôn ngữ cũng được in, với khoá thô như bộ quét thấy
(`op:mylib.fused`, `py:_core`; `TORCH_EXTENSION_NAME` còn là `py:@ext`, chưa
thay bằng tên trong `setup.py`):

```
app/k.py (Python)
      3  torch.ops.mylib.fused  → gọi qua ranh giới ngôn ngữ (op:mylib.fused)
native/ops.cpp (C/C++)
      2  cung cấp op:mylib.fused cho ngôn ngữ khác
```

File compose, Dockerfile và manifest Kubernetes cũng được in: mỗi service hay
workload một khối, lệnh khởi chạy dẫn tới file nào, cạnh mạng thấy ở đâu, thư
mục chart Helm không đọc. Giá trị biến môi trường của compose, manifest và
ConfigMap không bao giờ được in, chỉ tên máy trong đó. Đối số của lệnh (`command`,
`RUN`, `ENV`, `ARG`) mặc định cũng không, vì hay chở token: chỉ tên chương
trình, số đối số, từ khoá và dòng; đường dẫn và URL bỏ phần `user:mật khẩu@`.
`--verbatim` in đủ, cho lúc bạn tự xem ([infra.md](infra.md)). Tên máy viết
thẳng trong chuỗi của mã chỉ được in khi là tên một service của repo, vì chuỗi
dạng `a:b` khác có thể là `user:mật khẩu`; `--all-hosts` in mọi ứng viên:

```
triển khai compose:api (docker-compose.yml:2)
  khởi chạy docker-compose.yml:4 mlapp.serve.api → mlapp/serve/api.py
  mạng → qdrant  (docker-compose.yml:6 QDRANT_URL → qdrant:6333)
  mạng → db  (docker-compose.yml:7 DATABASE_URL → db:5432)
```

Không cần `.arch/`: `--root` là gốc, không thì thư mục gần nhất có
`.arch/system.arch`, không nữa thì thư mục hiện tại. `--json` là dạng
[bench/oracle.py](../bench/oracle.py) đọc để đối chiếu với parser chuẩn; phần
triển khai nằm ở khoá `deploy`. Cho thẳng tên file compose hay Dockerfile thì
nó được đọc kể cả trong thư mục test hay ví dụ.

