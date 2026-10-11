# archctl cho dự án ML và LLM

Với mã ML, archctl kiểm được tầng module: dữ liệu, đặc trưng, mô hình, huấn
luyện, phục vụ, cổng gọi LLM, và lời gọi từ Python sang kernel C++ hay CUDA.
Nó không đọc được kiến trúc mô hình theo layer, cũng không thấy topology lúc
chạy. Phần đó phải chạy thật mới biết, nên nó đi qua `verify`.

Mọi công thức dưới đây có trong [`examples/ml`](../examples/ml), mỗi công thức
cài sẵn đúng một vi phạm, cộng một vi phạm trong notebook. Test
`crates/arch-core/tests/ml.rs` đòi đủ bảy vi phạm đó, nên tài liệu này không
lặng lẽ sai đi được.

| Tầng | archctl | Bằng gì |
| --- | --- | --- |
| Module: dữ liệu, mô hình, huấn luyện, phục vụ, cổng LLM | kiểm được | `allow`, `deny`, `external` |
| Python gọi C++, CUDA, Rust | kiểm được | [cạnh liên ngôn ngữ](check.md#python-gọi-mã-native) |
| Hàm nguy hiểm như `pickle` | kiểm được | [`api`](api.md) |
| Lỗi của nhà cung cấp LLM lọt ra tầng ngoài | kiểm được | `boundary … contain` |
| Kiến trúc mô hình theo layer | chạy thật rồi so | `verify` |
| Process, GPU, hàng đợi, đồ thị agent lúc chạy | không | trace lúc chạy |

## Sáu công thức

Các ví dụ dùng bộ component của `examples/ml`: `Data`, `Features`, `Models`,
`Ops` (lớp bọc op C++), `Kernels` (`csrc/`), `Train`, `Serve`, `LLM` (cổng gọi
nhà cung cấp), `Rag`, `Web`. Thư viện ngoài chỉ tham gia luật khi được khai
báo `external`. `external Torch at "torch"` nhận cả `import torch`, `import
torch.nn as nn` lẫn `from torch import …`.

### 1. Phục vụ không kéo mã huấn luyện vào

```
deny Serve -> Train
```

Image phục vụ phình ra theo mọi dependency của vòng lặp huấn luyện. Một thay
đổi ở đó (logger mới, cờ CLI) làm hỏng đường phục vụ mà không ai ngờ.

```
✗ VI PHẠM  Serve -> Train  (bị cấm, #accepted)
  mlapp/serve/api.py:4  import 'mlapp.train.loop'
```

### 2. Một cổng duy nhất gọi nhà cung cấp LLM

```
external OpenAI    at "openai"
external Anthropic at "anthropic"
allow LLM -> OpenAI, Anthropic
deny * -> OpenAI, Anthropic except LLM
```

Thử lại, đổi nhà cung cấp, đếm chi phí, che khoá API: tất cả nằm ở một chỗ.
Agent thêm tính năng RAG rất hay gọi thẳng SDK cho nhanh, và luật này bắt
đúng chỗ đó.

```
✗ VI PHẠM  Rag -> OpenAI  (bị cấm, #accepted)
  mlapp/rag/retriever.py:1  import 'openai'
  → gọi LLM thay vì truy cập thẳng OpenAI
```

### 3. Khâu dữ liệu không biết framework học sâu

```
deny Data     -> Torch
deny Features -> Torch
```

Khâu dữ liệu chạy được ở nơi không có GPU, và đổi framework không phải viết
lại nó. Mỗi nguồn một dòng: `deny` nhận nhiều đích nhưng chỉ một nguồn.

### 4. Không nạp mô hình bằng `pickle`

```
api Unsafe at "pickle"
deny * -> Unsafe
```

`pickle.loads` chạy được mã tuỳ ý trong file nó nạp, và checkpoint tải từ mạng
là file không tin được. Nạp trọng số bằng safetensors. Nhóm `api` tìm thẳng
trong dòng token, nên tên trong chú thích hay trong chuỗi không bị tính.

Đừng thêm `eval` vào nhóm này cho dự án PyTorch. Ký hiệu khớp cả khi đứng sau
dấu chấm, nên mọi `model.eval()` (chế độ suy luận) và `df.eval(...)` của pandas
đều thành vi phạm. `torch.load` cũng nạp bằng pickle khi `weights_only=False`,
nhưng `api` không đọc đối số; bắt chỗ đó cần một linter hiểu lời gọi, nối vào
qua `verify`.

### 5. Lỗi của nhà cung cấp không lọt khỏi cổng

```
boundary LLMCalls contains LLM:
  contain  RateLimitError, APITimeoutError
```

Tầng ngoài chỉ được thấy `LLMUnavailable`, thứ do cổng định nghĩa. Một
`except RateLimitError` ở tầng web nghĩa là tầng web đã biết mình đang gọi
OpenAI.

```
✗ VI PHẠM  Web -> RateLimitError  (lọt ra ngoài vùng, #accepted)
  mlapp/web/app.py:10  dùng `RateLimitError`
```

archctl còn cảnh báo `boundary LLMCalls không có verify`. Phần `contain` kiểm
được bằng phân tích tĩnh; thử lại và timeout của cổng thì cần test chạy thật.

### 6. Op C++ chỉ được gọi qua lớp bọc

```
allow Ops -> Torch, Kernels
```

Không có `allow Train -> Kernels`, nên `torch.ops.mlapp_ops.fused_gelu` gọi
thẳng từ vòng lặp huấn luyện là vi phạm, dù không câu import nào nói tới
`csrc/`:

```
✗ VI PHẠM  Train -> Kernels  (không được phép, #accepted)
  mlapp/train/loop.py:12  gọi `torch.ops.mlapp_ops.fused_gelu` qua ranh giới ngôn ngữ
```

Ghép được khi namespace viết thẳng trong `TORCH_LIBRARY(mlapp_ops, m)`. Module
mở rộng PyO3, pybind11, nanobind cũng vậy. Giới hạn nằm ở
[check.md](check.md#python-gọi-mã-native).

## Kiến trúc mô hình: chụp khi chạy thật

`Net` dựng `Block` trong một vòng lặp, và số tầng lấy từ tham số hay file cấu
hình. Đọc mã nguồn không ra được "Embedding → 4 × Block → Linear"; chạy thật
thì ra ngay. Nên kiến trúc mô hình được chụp lúc chạy rồi so với bản đã duyệt:

```
verify model_arch runs "python3 -m pytest -q tests/test_model_arch.py"
```

[`tests/test_model_arch.py`](../examples/ml/tests/test_model_arch.py) so
`repr(Net())`, tức cây module đầy đủ, với `tests/model_arch.txt`. Lần đầu,
hay khi đổi kiến trúc có chủ ý, chạy `UPDATE_ARCH=1 pytest
tests/test_model_arch.py` để sinh lại file đó, và người duyệt đọc diff của nó
như đọc diff của luật.

`archctl verify` chạy lệnh và ghi biên nhận gắn với bản mã lúc chạy;
`archctl check` nhắc khi biên nhận đã cũ hơn mã — xem [verify.md](verify.md).
Lệnh chụp là gì cũng được (`repr`, đồ thị của `torch.export`, bảng tóm tắt của
torchinfo), miễn nó thoát khác 0 khi kiến trúc khác bản đã duyệt.

## Notebook

`.ipynb` được quét như Python, và chỉ ô code được tính: ô markdown, output và
metadata thì không. Notebook là nơi agent hay đi tắt nhất, và luật áp lên nó y
như lên file `.py`:

```
✗ VI PHẠM  Notebooks -> OpenAI  (bị cấm, #accepted)
  notebooks/eda.ipynb:30  import 'openai'
```

Số dòng là dòng trong file `.ipynb`, nên mở file bằng trình soạn thảo là thấy
đúng chuỗi JSON đó. Jupyter ghi mỗi dòng mã thành một chuỗi riêng nên số dòng
chính xác. Khi một chuỗi chứa nhiều dòng mã (`source` là một chuỗi, hay file
sinh bằng công cụ khác) thì số dòng từ đó trở đi chỉ gần đúng, nhưng không
import nào bị mất. Notebook nbformat 3 (rất cũ, dùng `worksheets`) không được
đọc.

Magic của IPython (`%matplotlib`, `!pip install`) không phải Python và bị bỏ
qua, khi dòng đó mở một câu lệnh mới: trong chuỗi `"""` hay trong ngoặc, dòng
bắt đầu bằng `%` vẫn là Python. Ô mở đầu bằng magic của cả ô cũng vậy (`%%bash`, `%%html`,
`%%writefile`), trừ `%%time`, `%%timeit`, `%%capture`, `%%prun`, `%%debug`,
vì những ô đó vẫn là Python. Ô `%%python3` hay `%%script python` chạy Python
trong tiến trình con và cũng được đọc: bỏ qua chúng là chừa một lối đi tắt khỏi
luật. Notebook khai kernel khác Python (R, Julia) thì
không được đọc.

Notebook có output lớn (ảnh, bảng) có thể vượt ngưỡng 4 MB mỗi file và được báo
là quá lớn. Xoá output trước khi commit (`jupyter nbconvert --clear-output
--inplace`, hay nbstripout) thì vừa qua được ngưỡng, vừa giữ diff sạch.

## Chưa làm

- **Đồ thị lúc chạy:** process và GPU worker, hàng đợi, đồ thị của LangGraph,
  DAG của `dvc.yaml` hay Airflow. Những thứ đó được khai báo bằng lời gọi hàm
  hay file cấu hình, không bằng import.
