# archctl cho dự án ML và LLM

Với mã ML, archctl kiểm được tầng module: dữ liệu, đặc trưng, mô hình, huấn
luyện, phục vụ, cổng gọi LLM, và lời gọi từ Python sang kernel C++ hay CUDA.
Nó không đọc được kiến trúc mô hình theo layer, cũng không thấy topology lúc
chạy. Phần đó phải chạy thật mới biết, nên nó đi qua `verify`.

Mọi công thức dưới đây có trong [`examples/ml`](../examples/ml), mỗi công thức
cài sẵn đúng một vi phạm, cộng một vi phạm trong notebook. Test
`crates/arch-core/tests/ml.rs` đòi đủ tám vi phạm đó, nên tài liệu này không
lặng lẽ sai đi được.

| Tầng | archctl | Bằng gì |
| --- | --- | --- |
| Module: dữ liệu, mô hình, huấn luyện, phục vụ, cổng LLM | kiểm được | `allow`, `deny`, `external` |
| Python gọi C++, CUDA, Rust | kiểm được | [cạnh liên ngôn ngữ](check.md#python-gọi-mã-native) |
| Hàm nguy hiểm như `pickle` | kiểm được | [`api`](api.md) |
| Lỗi của nhà cung cấp LLM lọt ra tầng ngoài | kiểm được | `boundary … contain` |
| Bước của đồ thị LangGraph, DAG Airflow, stage `dvc.yaml` đưa dữ liệu cho nhau | kiểm được | [cạnh pipeline](#đồ-thị-pipeline-langgraph-airflow-dvc) cần `allow` |
| Hình dạng đồ thị (thứ tự bước, nhánh rẽ) | chạy thật rồi so | `verify` |
| Kiến trúc mô hình theo layer | chạy thật rồi so | `verify` |
| Process, GPU, đồ thị dựng lúc chạy | không | trace lúc chạy |

## Bảy công thức

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

### 7. Log người dùng không vào thẳng huấn luyện

```
deny Web -> Train
```

Log câu hỏi phải qua `Data` để lọc dữ liệu cá nhân trước khi thành dữ liệu huấn
luyện. Trong `examples/ml`, `dvc.yaml` có stage `export_feedback` chạy
`python -m mlapp.web.export` ghi `data/feedback.jsonl`, và stage `train` đọc
đúng file đó rồi chạy `python -m mlapp.train.loop`. Không file nào import file
nào; cạnh nằm ở chỗ stage sau đọc file stage trước ghi:

```
✗ VI PHẠM  Web -> Train  (bị cấm, #accepted)
  dvc.yaml:9  bước pipeline `stage export_feedback → train qua data/feedback.jsonl (dvc)`
```

Đồ thị LangGraph của RAG trong cùng ví dụ (`mlapp/rag/graph.py`: bước
`retrieve` của Rag rồi bước `generate` là cổng LLM) là đường hợp lệ: có `allow
Rag -> LLM` nên không báo.

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
như đọc diff của luật. Bản đã duyệt của ví dụ:

```
Net(
  (embed): Embedding(1000, 64)
  (blocks): ModuleList(
    (0-3): 4 x Block(
      (attn): MultiheadAttention(
        (out_proj): NonDynamicallyQuantizableLinear(in_features=64, out_features=64, bias=True)
      )
      (up): Linear(in_features=64, out_features=256, bias=True)
      (down): Linear(in_features=256, out_features=64, bias=True)
    )
  )
  (head): Linear(in_features=64, out_features=1000, bias=True)
)
```

Chạy ví dụ ở máy mình: cài torch và pytest vào một venv bằng
[`requirements-test.txt`](../examples/ml/requirements-test.txt) (ghim `==` kèm
sha256, torch bản CPU), rồi đặt venv lên đầu `PATH`:

```
python3 -m venv .venv
.venv/bin/python -m pip install --require-hashes --only-binary :all: --no-deps \
    -r requirements-test.txt
PATH="$PWD/.venv/bin:$PATH" archctl verify model_arch
```

CI của repo này chạy đúng lệnh đó ở job `kiến trúc mô hình` ([ci.md](ci.md)).
Đổi `depth` từ 4 thành 5 thì nó đỏ: "kiến trúc mô hình khác bản đã duyệt".

`archctl verify` chạy lệnh và ghi biên nhận gắn với bản mã lúc chạy;
`archctl check` nhắc khi biên nhận đã cũ hơn mã — xem [verify.md](verify.md).
Lệnh chụp là gì cũng được (`repr`, đồ thị của `torch.export`, bảng tóm tắt của
torchinfo), miễn nó thoát khác 0 khi kiến trúc khác bản đã duyệt.

## Đồ thị pipeline: LangGraph, Airflow, dvc

Các bước của một pipeline nối với nhau bằng lời gọi hàm hay file cấu hình,
không bằng import. `rag/graph.py` import cả `retrieve` lẫn `generate` rồi
`add_edge("retrieve", "generate")`; hai file của hai bước thì không import
nhau. archctl đọc các lời gọi đó (không chạy gì), lần hàm của mỗi bước về file
qua import của chính file đồ thị, và coi "bước A đưa dữ liệu cho bước B" là một
cạnh từ component chứa A tới component chứa B: cần `allow`, bị `deny` chặn, như
một import. Không cần `link`: các bước thường chạy chung một process (LangGraph)
hay do bộ lập lịch điều phối. File đồ thị không cần thuộc component nào
(`dags/` của Airflow, `main.py` ở gốc). `init --from-code` sinh `allow` cho cạnh
này.

Tên được tra như Python: theo phạm vi (`def`, `class`, biết nhờ thụt lề) và lần
gán hay import gần nhất trước dòng dùng. Hai hàm cùng dựng đồ thị vào biến
`workflow` là hai đồ thị, gán lại `g = StateGraph()` là đồ thị mới, tham số `g`
của một hàm khác là builder khác; phương thức `generate` của một lớp không che
`from rag.llm import generate`; `generate = traceable(generate)` bọc
`generate` của import. Trong Airflow, chỉ thân hàm `@dag`, `@task_group` và khối
`with DAG` là chỗ nối đồ thị: thân một `@task`, hay hàm làm `python_callable`,
là mã chạy lúc thực thi. Hàm nhập qua `__init__.py` chỉ nhập lại tên thì được
lần tới file định nghĩa nó. Lời gọi bọc (`traceable(generate)`, `partial(f)`,
`ToolNode([search, fetch])`) là cạnh tới hàm được bọc khi hàm bọc là của gói
ngoài; `lambda s: generate(s)` là cạnh tới `generate`.

| Công cụ | Đọc được | Báo "chưa đọc được" |
| --- | --- | --- |
| LangGraph (file import `langgraph`, hay gọi `StateGraph(`) | `add_node("tên", hàm)`, `add_node(hàm)`, `add_sequence([...])`, `add_edge` (cả danh sách nguồn), `add_conditional_edges` với `path_map` (dict, list, biến gán dict), với hàm rẽ có kiểu trả về `Literal[...]`, với `tools_condition`; hàm là tên nhập, `module.hàm`, lời gọi bọc, `lambda`, phương thức `self.x`, biến gán từ những thứ đó | tên bước từ biến hay f-string, `add_node` trong vòng lặp, `add_edge` gọi qua biến (`edge = g.add_edge`), cạnh tới bước mà builder đó không `add_node`, rẽ nhánh không `path_map` và không `Literal`, `Command(goto=…)` (cả khi nhập với tên khác), `Send(…)`, hàm của bước là tham số hay biến không gán trong file |
| Airflow (file import `airflow`, hay gọi `DAG(`, `…Operator(`) | operator, sensor có `python_callable` (cả `.partial(…).expand(…)`), task TaskFlow (`extract()`, `f.override(…)()`), `a >> b`, `a << b`, danh sách, nhóm `with TaskGroup(…) as tg` (cạnh tới mọi task gán trong khối), `set_downstream`, `set_upstream`, `chain`, `chain_linear`, `cross_downstream` của Airflow (cả khi nhập với tên khác); trong `@dag` hay `with DAG(…)`, TaskFlow truyền qua đối số (`load(transform(extract()))`) | task lấy qua chỉ số (`tasks[0]`), task không gán trong file, `dag.get_task(…)`, `set_dependency`, `>>` nằm trong biểu thức khác (`log(a >> b)`), `BashOperator` chạy script của repo, `chain` hai danh sách khác độ dài |
| dvc (mọi `dvc.yaml` của repo, ghép chung một đồ thị như dvc) | stage ghi file (`outs`, `metrics`, `plots`) mà stage sau đọc (`deps`), so theo đoạn đường dẫn (`data/a` khớp `data/a/x.csv`, không khớp `data/ab`), `wdir`; script trong `cmd` (`python x.py`, `python -m m`, `cd d && …`) | `foreach`, `matrix`, `${…}`, `wdir` ra ngoài repo, script shell, script không có trong repo, `cmd` không trỏ tới file mã nào (`python -c`, `make`, `papermill`) |

Không kiểm thứ tự bước hay điều kiện rẽ. Operator không chạy mã của repo
(`BashOperator` chạy `echo`) không có đầu cạnh nào. Đồ thị trong file test, mock
vẫn cho cạnh (nó nối bước thật của các component), nhưng chỗ không đọc được của
nó không báo. Builder lấy từ hàm của module khác (`g = new_graph()` trong file
không import LangGraph, không gọi `StateGraph(`) thì không thấy; code trong file
không thuộc component nào (hàm `def` ngay trong `dags/x.py` ngoài mọi
component) không có component để gắn cạnh. `cmd` của dvc, nội dung chuỗi và đối
số của operator không bao giờ được in: lệnh có thể mang token. Mỗi file có trần
(10 000 cạnh, 2 000 000 bước xét); chạm trần thì báo chưa đọc.

Thứ tự bước, nhánh rẽ và đồ thị dựng lúc chạy thì chụp khi chạy thật rồi so với
bản đã duyệt, như kiến trúc mô hình ở trên. Công thức cho LangGraph (cần cài
`langgraph` trong môi trường chạy `verify`; repo này không chạy nó trong CI):

```
verify rag_graph runs "python3 -m pytest -q tests/test_graph_shape.py"
```

```python
# tests/test_graph_shape.py
import os
from pathlib import Path

from rag.graph import graph  # đồ thị đã `compile()`

APPROVED = Path(__file__).with_name("graph_shape.mmd")


def test_hinh_dang_do_thi_khop_ban_da_duyet():
    shape = graph.get_graph().draw_mermaid()
    if os.environ.get("UPDATE_GRAPH"):
        APPROVED.write_text(shape)
    assert shape == APPROVED.read_text(), "đồ thị khác bản đã duyệt: UPDATE_GRAPH=1 rồi duyệt diff"
```

Với Airflow, so `{t.task_id: sorted(t.downstream_task_ids) for t in dag.tasks}`
của DAG nạp bằng `DagBag(include_examples=False)`; với dvc, so đầu ra của `dvc
dag --md`. Lệnh nào cũng được, miễn nó thoát khác 0 khi đồ thị khác bản đã
duyệt. Bộ đo đối chiếu phần đọc tĩnh với `ast` và PyYAML trên langgraph,
airflow, example-get-started ([bench/README.md](../bench/README.md)).

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

Cách đọc này được đối chiếu với chính IPython trong bộ đo: `bench/oracle.py
--require ipython` biến đổi từng ô bằng `TransformerManager` như Jupyter rồi so
import và số dòng với archctl.

Notebook có output lớn (ảnh, bảng) có thể vượt ngưỡng 4 MB mỗi file và được báo
là quá lớn. Xoá output trước khi commit (`jupyter nbconvert --clear-output
--inplace`, hay nbstripout) thì vừa qua được ngưỡng, vừa giữ diff sạch.

## Chưa làm

- **Đồ thị lúc chạy:** process và GPU worker, đồ thị dựng trong vòng lặp hay
  từ file cấu hình, nhánh rẽ bằng `Command`/`Send`. Phần khai tĩnh của
  LangGraph, Airflow, dvc đã được đọc (mục trên); phần còn lại được báo
  "chưa đọc được" và chụp bằng `verify`. Prefect, Kubeflow, Dagster chưa đọc.
