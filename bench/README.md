# Bộ đo trên repo thật

```bash
python3 bench/bench.py
```

Một lệnh: dựng archctl bản release, tải ba mươi mốt repo ở đúng commit đã ghim
trong [repos.tsv](repos.tsv), rồi chạy hai phép thử trên từng repo. Chỉ cần
Python 3 và git, không gói Python nào ngoài thư viện chuẩn.

## Có những repo nào, và vì sao

Mỗi repo có mặt vì một cách bố trí mà fixture viết tay không nghĩ ra. Chọn sao
cho mỗi ngôn ngữ có ít nhất hai repo, và có cả thư viện lẫn ứng dụng chia tầng —
chỗ mà luật `allow`/`deny` giữa component có nghĩa nhất.

| Ngôn ngữ | Repo |
| --- | --- |
| TypeScript | vue core (monorepo, alias tsconfig), excalidraw (ứng dụng React, TSX) |
| Python | flask (`src/`), netbox (Django, gốc là thư mục có `manage.py`), scikit-learn (ML), vllm (LLM serving, kernel C++), langchain (LLM, hai mươi hai `pyproject.toml` trong `libs/`), pytorch-image-models (huấn luyện DL), apex (DL cộng C++/CUDA, extension đặt tên trong `setup.py`) |
| C | curl (CMake), redis (Makefile, `deps/`), nginx (`-I` trong script `auto/`, không file build nào đọc được) |
| C++ | leveldb (storage engine, CMake) |
| Rust | tokio, ripgrep, rusqlite, pydantic-core (với Python), zstd-rs (với C qua submodule) |
| Go | hugo (đồ thị dày), etcd (mười bốn `go.mod` trong một repo), kubernetes (phần scheduler, tải sparse; module staging có `go.mod` riêng) |
| docker-compose | example-voting-app (năm service, stage Dockerfile thừa hưởng `WORKDIR`, bind mount, file compose ghi đè; cả manifest Kubernetes dùng image dựng sẵn), open-webui (ứng dụng LLM tự host: `build: .`, `COPY --from=build`, điểm vào qua `start.sh`, mã Python trong `backend/` khai bằng `sources` của hatch) |
| Hàng đợi | rabbitmq-tutorials (cùng bài RabbitMQ viết bằng Python, Go, Node; tải sparse ba thư mục đó: queue, exchange ghép chéo ngôn ngữ, Go khai queue rồi dùng qua `q.Name`) |
| Kubernetes | microservices-demo (Online Boutique: mười một microservice Go, Python, Node, C#, Java gọi nhau bằng gRPC; env `…_ADDR` trỏ tên Service, `skaffold.yaml` nối image với thư mục mã, bản phát hành trong `release/`, chart Helm, component Kustomize) |

Lần thêm tám repo cuối (etcd, excalidraw, langchain, leveldb, netbox, nginx,
scikit-learn, vllm) bắt được bốn chỗ archctl không thấy cạnh nào mà cũng
không báo gì: etcd, langchain, netbox, nginx đều ra 0 component vào diện kiểm.
Cả bốn đã sửa, lý do ở [docs/check.md](../docs/check.md).

Cột submodule trong `repos.tsv` liệt kê submodule cần kéo về. `--depth 1`
không kéo submodule, nên zstd-rs từng vào bộ đo không có dòng C nào dù được
chọn chính vì phần C vendor. Submodule được tải ở đúng commit repo cha ghi lại,
URL đọc từ `.gitmodules` và cũng phải là `https://`.

Cột sparse cho monorepo quá lớn để tải hết: chỉ những thư mục liệt kê (cùng
file nằm thẳng ở gốc) được tải. google-cloud-python đủ là 1,3 GB và 45 nghìn
file Python; mười một package của nó là 83 MB. Phép đếm số file bỏ những file
git đánh dấu nằm ngoài vùng sparse, và một thư mục sparse không có ở commit
đã ghim là mã thoát 2 — không thì một tên gõ sai cho ra một repo rỗng mà vẫn
đạt.

kubernetes chỉ tải phần scheduler (`pkg/scheduler`, `cmd/kube-scheduler`, module
staging `k8s.io/kube-scheduler`), 277 file. Import sang package khác của chính
module `k8s.io/kubernetes`, như `pkg/features`, thành 67 cảnh báo chưa phân giải:
archctl nhận đúng đó là mã nội bộ, chỉ là nó không được tải về.

Repo tải về `target/bench/repos/` (khoảng 920 MB), bản làm việc ở
`target/bench/work/`, kết quả chi tiết ở `target/bench/results.json`. Tất cả
nằm trong `target/`: ngoài git, bộ quét của archctl bỏ qua, `cargo clean` dọn.

## Hai phép thử, cả hai đều chặn

**Tự nhất quán** ([consistency.py](consistency.py)). `init --from-code` đọc mã
rồi sinh luật mô tả đúng hiện trạng. Nâng mọi thứ lên `#accepted`, `check` lại
chính mã đó: phải ra **0 vi phạm**. Mỗi vi phạm còn lại là một chỗ bộ khám phá
và bộ kiểm chứng nhìn cùng một đoạn mã mà thấy hai điều khác nhau — lỗi của
archctl, không phải của repo.

Và phải ra **0 cảnh báo trong file luật**: `init` khuyên chạy `check --strict`
trong CI, mà `--strict` coi cảnh báo của file luật là vi phạm. Luật vừa sinh ra
đã có cảnh báo thì lời khuyên đó đổ ngay lần chạy đầu tiên của người dùng mới.

Và archctl phải quét **đúng số file nguồn mà git có**. Số này đếm độc lập từ
`git ls-files`, trừ symlink và thư mục archctl luôn bỏ qua; thư mục tên output
(`build`, `dist`, `out`, `coverage`) chỉ bị trừ khi `git check-ignore` nói nó
(hay mọi thứ trong nó) bị ignore, hoặc khi không có `.gitignore` nào để hỏi. Lệch là archctl bỏ sót mã được commit: etcd từng mất cả package Go
`tests/robustness/coverage/` vì tên thư mục trùng tên output, không một lời nào.

Và luật sinh ra phải có **các cạnh liên ngôn ngữ đã biết**, mỗi họ ghép một
cạnh (`CROSSINGS` trong [consistency.py](consistency.py)):

| Repo | Cạnh | Qua |
| --- | --- | --- |
| libbpfgo | `Core -> Libbpf` | cgo |
| pydantic-core | `PydanticCore -> Core` | module PyO3 `_pydantic_core` |
| matplotlib | `Lib -> Src` | module pybind11 `_path` |
| vllm | `Vllm -> Csrc` | op PyTorch `torch.ops._C.*` |
| apex | `Bottleneck -> Csrc` | `PYBIND11_MODULE(TORCH_EXTENSION_NAME, m)`, tên đọc từ `setup.py` |

Đổi `allow` đó thành `deny` thì `check` phải báo vi phạm qua ranh giới ngôn
ngữ. Tự nhất quán không bắt được chỗ này: phần ghép hỏng ở cả `init` lẫn
`check` thì hai bên vẫn khớp nhau, và cạnh biến mất trong im lặng.

Tầng hạ tầng ([docs/infra.md](../docs/infra.md)) theo cùng cách. Luật sinh ra
không được có file triển khai nào không đọc được, service có `runs` mà không rõ
điểm vào, `link` không cấu hình nào cho thấy, hay service compose nào bị bỏ
sót. Và phải có các cạnh đã biết (`DEPLOY` trong [consistency.py](consistency.py)):

| Repo | Phải có | Gỡ đi thì `check` phải báo |
| --- | --- | --- |
| example-voting-app | `link Internet -> Vote, Result` (cổng compose); `link Vote -> Redis`, `Result -> Db`, `Worker -> Db, Redis`; `Vote` trong `runs` của service Vote | `not_linked`, `not_run` |
| open-webui | `link OpenWebui -> Ollama`; `Routers` trong `runs` của OpenWebui | `not_linked`, `not_run` |
| microservices-demo | `link Internet -> Frontend` (Service `LoadBalancer`, Istio); `link Checkoutservice -> Productcatalogservice`, `Frontend -> Cartservice`, `Recommendationservice -> Productcatalogservice`, `Cartservice -> RedisCart`; `Emailservice`, `Checkoutservice` trong `runs` của service cùng tên; `link Frontend -> Checkoutservice` mà NetworkPolicy `checkoutservice` cho phép; đúng một `link` bị NetworkPolicy chặn: `Frontend -> Shoppingassistantservice` | `not_linked` (có vi phạm nêu tên policy), `not_run`, cảnh báo `linksBlocked` |

Repo mà các phần không import lẫn nhau (ba ứng dụng riêng của voting app) không
có component nào vào diện kiểm; nó vẫn đạt vì tầng hạ tầng có cái để kiểm.

**Luồng lệnh** ([flows.py](flows.py)). Mọi lệnh — `init`, `check` theo bốn
đường, `parse`, `export`, `baseline`, `propose`/`accept`/`block`/`reject`,
`diff`, `serve` — trên mã người khác viết. Test trong `crates/*/tests/` dựng dự
án nhỏ viết riêng để test; ở đây là thư mục vendor, symlink, file sinh tự động
mà fixture không nghĩ ra.

## Mã thoát

| | |
| --- | --- |
| 0 | cả hai phép thử đạt trên mọi repo |
| 1 | archctl sai: có bất nhất, có cảnh báo trong luật sinh ra, hoặc một luồng lệnh hỏng |
| 2 | không chạy tới được: không tải được repo, hoặc build hỏng |

Tách 1 khỏi 2 để một lần mạng chập chờn trên CI không trông giống một lỗi của
archctl.

## Phép thử thứ ba: đối chiếu với parser chuẩn

```bash
python3 bench/oracle.py
```

Tự nhất quán so archctl với chính nó: bộ khám phá và bộ kiểm dùng chung một
bộ trích, nên một lỗi trong bộ trích làm cả hai sai giống nhau mà vẫn ra
"0 bất nhất". [oracle.py](oracle.py) so import mà `archctl scan --json` đọc
được với parser chuẩn của từng ngôn ngữ, trên các repo `bench.py` đã tải:

- Python: `ast` của chính trình thông dịch chạy script. File không parse được
  (cú pháp mới hơn bản Python đang chạy, Python 2) được đếm riêng.
- C/C++: lexer thô của clang, không tiền xử lý — giống archctl, include trong
  nhánh `#if` nào cũng tính.
- JS/TS: parser module của Node, TS gỡ kiểu trước bằng
  `module.stripTypeScriptTypes`. Chỉ import tĩnh, nên chỉ kiểm phía thiếu.
- Go: `go/parser` ở chế độ `ImportsOnly`, dựng một lần thành binary tạm. Không
  xét build tag, giống archctl.
- Rust: `syn`, ghim cứng phiên bản, dựng dưới `target/bench/syn-oracle/`. Phải
  tải crate nên chỉ chạy khi `--require syn` (CI bật). `syn` không đọc vào thân
  macro, nên import archctl thấy trong `macro_rules!`, `cfg_rt! { }`... được
  đếm riêng, không tính là thừa.
- Notebook: `TransformerManager` của IPython biến đổi từng ô như Jupyter, rồi
  `ast` đọc import; dòng của từng phần tử `source` lấy khi duyệt JSON bằng
  `scanstring` chuẩn ([notebook_truth.py](notebook_truth.py)). IPython và các
  gói nó kéo theo ghim `==` kèm sha256
  ([ipython-requirements.txt](ipython-requirements.txt)), chỉ cài wheel, vào
  venv dưới `target/bench/ipython-oracle/`,
  nên chỉ chạy khi `--require ipython` (CI bật). Số dòng chỉ so khi mỗi dòng mã
  là một chuỗi JSON riêng, ở những dòng JSON tăng dần (như mọi notebook Jupyter
  ghi ra), và IPython không gộp dòng.

- Binding (cạnh Python ↔ native): khoá thô mà `scan --json` in cho từng file
  được đọc lại độc lập: phía C/C++ trên token thô của clang (module pybind11,
  nanobind, `PyInit_`, `m.def` trong `TORCH_LIBRARY*`), phía Python là
  `torch.ops.ns.op` theo `ast`, PyO3 theo `syn` khi `--require syn`.
- Hạ tầng: dữ kiện mà `scan --json` đọc từ mọi file compose và Dockerfile git
  theo dõi (kể cả trong thư mục test, ví dụ) được đọc lại bằng PyYAML (`<<` trải
  bằng `flatten_mapping`, lệnh dạng chuỗi tách bằng `shlex`) và dockerfile-parse
  ở mức chỉ thị ([deploy_truth.py](deploy_truth.py)), ghim kèm hash trong
  [deploy-requirements.txt](deploy-requirements.txt), chỉ khi `--require deploy`
  (CI bật). dockerfile-parse không hiểu heredoc của BuildKit, nên file có heredoc
  in `BỎ QUA`; nó cắt lệnh ở dòng trống giữa dòng nối còn BuildKit thì không, nên
  thước đo thay dòng trống đó bằng chú thích trước khi đưa vào.
- Kubernetes: dữ kiện mà `scan --json` đọc từ mọi file YAML có dáng manifest và
  `skaffold.yaml` (workload, nhãn, container, image, `command`, `args`, khoá env,
  tham chiếu ConfigMap, Service và selector, khoá ConfigMap) được đọc lại bằng
  PyYAML; mọi cạnh qua Service (nguồn, dòng, tên máy → tập workload đích), kết
  quả NetworkPolicy của từng workload (cô lập chiều nào, cho phép rõ ai) và tập
  workload lộ ra Internet (Service, Ingress, route, `hostNetwork`)
  được tính lại độc lập theo tài liệu Kubernetes ([k8s_truth.py](k8s_truth.py)).
  Cùng venv và cùng `--require deploy`.
- Helm: chart (không nằm trong thư mục ví dụ) được `helm template` bản ghim dựng
  (tải từ get.helm.sh, kiểm sha256, chỉ khi `--require helm`; CI bật). Bản dựng
  tách theo `# Source:` thành từng template; dữ kiện từng template (như mục
  Kubernetes, không kèm dòng vì helm chỉ có dòng của văn bản dựng ra) so với bản
  archctl tự dựng, và bản dựng của helm đi vào phần tính cạnh, lối vào,
  NetworkPolicy của `k8s_truth.py`. Không có helm thì mọi thứ archctl dựng từ
  chart bị bỏ khỏi phép so.
- Tên máy trong chuỗi: `(dòng, máy:cổng)` mà `scan --json` đọc từ chuỗi của mọi
  file Python và Go được tính lại bằng [code_hosts.py](code_hosts.py). Chuỗi và
  chỗ nó đứng (tham số, gán có kiểu hay không, khoá dict, giá trị mặc định của
  `getenv`, vế sau `or`) lấy từ `ast` và `go/ast`; chuỗi ghép ngầm `'a' 'b'`
  tách theo `tokenize`, vì `ast` gộp chúng còn mọi lexer thì không; luật đổi
  chuỗi thành tên máy viết lại theo đặc tả trong `scan/hosts.rs`. Go cần
  `--require go` như phần import. JS/TS chưa có thước đo.
- Hàng đợi: `(dòng, hệ, vai, loại, tên)` của chỗ gửi, nhận mà `scan --json` in
  (mục `queues`) được tính lại bằng [queue_truth.py](queue_truth.py) từ `ast`
  và `go/ast`, theo đặc tả trong `scan/queue.rs` (lời gọi nào là gửi, nhận;
  thư viện nào phải import; hằng số, env, queue khai rồi dùng qua biến). Tên
  của chỗ không cố định không so. Ngoài các repo, bộ mẫu `fixtures/queues` phủ
  những dạng repo thật không có; chỉ so khi không có `--only`. JS/TS chưa có
  thước đo.

Python, notebook, binding, hạ tầng, tên máy, hàng đợi, C/C++, Go và Rust đòi khớp cả tên lẫn số dòng: thiếu, thừa hay lệch dòng đều
là lệch. Thiếu clang, Node hay Go thì phần đó in `BỎ QUA`; `--require
clang,node,go` biến việc thiếu thành mã thoát 2, và với Node thì đòi cả khả năng
gỡ kiểu TS. Thước đo tự hỏng cũng là mã 2 chứ không phải "đạt": clang, Node hay
trình đọc Go thoát khác 0, không trả lời đủ số file, hoặc quá nửa số file của một repo không parse được.

Lần đầu chạy (2026-10-04) nó bắt những lỗi mà cả hai phép thử kia đều cho qua:
Vue core mất 363/1 728 import tĩnh vì `import { type A, b }`, Python có 1 045
import giả từ `raise E from e` và `yield from x` cộng 15 import thật bị nuốt
ngay sau chúng, 11 import Python báo sai dòng. Chi tiết ở
[docs/check.md](../docs/check.md#đối-chiếu-với-parser-chuẩn).

## Vì sao ghim commit

Không ghim thì con số trôi theo upstream: chạy lại hôm sau cho kết quả khác mà
không biết do archctl đổi hay do repo đổi. Tải theo SHA, `--depth 1`, kiểm lại
`HEAD` sau khi checkout. Nâng một repo lên bản mới là sửa một dòng trong
`repos.tsv` — và là một thay đổi đáng nhìn riêng trong lịch sử git.

Git chạy với `core.hooksPath=/dev/null`, `core.fsmonitor=` và
`protocol.file.allow=never`: repo tải về là mã người khác, không để nó chạy gì
qua cấu hình git. Cùng danh sách mà archctl tự dùng.

## Thời gian chỉ in, không chặn

```bash
python3 bench/bench.py --only hugo,tokio --rounds 9
```

Trung vị của `--rounds` lần chạy, bỏ lần đầu vì bộ nhớ đệm hệ thống file. Trên
máy CI dùng chung con số nhiễu tới mức chặn theo nó chỉ tạo ra đỏ giả, nên CI
chạy với `--rounds 1` và không đọc cột thời gian.

## Mỗi phép kiểm phải đổ được

```bash
python3 bench/sabotage.py
```

Dựng một archctl giả, gọi archctl thật rồi làm hỏng đúng một chỗ: thêm một vi
phạm, thêm một cảnh báo vào luật sinh ra, báo quét 0 file, báo quét thiếu một
file, bỏ khoá
`lawWarnings`, giữ nguyên hash khi luật đổi, nói `diff` có thay đổi khi chỉ
thêm chú thích. Mỗi kiểu phải làm `bench.py` thoát 1, **và phải bị đúng phép
kiểm dành cho nó bắt** — không chỉ đỏ vì một lý do khác. Ba kiểu nữa làm hỏng
đầu ra `scan --json` — mất một import, lệch một dòng, thêm một import giả — và
phải làm `oracle.py` thoát 1; hai kiểu nữa làm vậy với tên máy trong chuỗi của
file Python (mất một, thêm một giả). Bốn kiểu nữa phá phần cạnh binding, mỗi nhánh
của thước đo một kiểu: module pybind11 của matplotlib (mất, thêm giả), lời gọi
`torch.ops` của pytorch-image-models, op `TORCH_LIBRARY` của vllm. `--notebook-repo
matplotlib` thêm hai kiểu y vậy ở notebook, đối chiếu bằng IPython nên cần
mạng. Kiểu `crossing` xoá cạnh liên ngôn ngữ khỏi
cả luật `init` sinh ra lẫn vi phạm `check` báo, trên pydantic-core, và chỉ
danh sách cạnh bắt buộc ở trên bắt được nó. `--rust-repo ripgrep` thêm ba kiểu y vậy ở file
Rust, đối chiếu bằng `syn` nên cần mạng; kiểu thêm import giả nằm ngoài macro,
nên nó cũng giữ cho phép miễn trừ import trong thân macro không rộng ra.

Năm kiểu phá tầng hạ tầng của bộ đo, trên example-voting-app: `init` mất mọi
`link`, `check` giấu vi phạm cạnh mạng, `check` giấu vi phạm `runs`, `check`
báo thêm một file triển khai không đọc được, `check` giấu vi phạm do tên máy
viết trong mã (`consistency.DEPLOY` đòi `Redis(host="redis")` của
`vote/app.py` và chuỗi kết nối của `result/server.js` phải bị báo ở chính dòng
mã đó khi gỡ `link`). `--deploy-repo example-voting-app`
thêm bốn kiểu ở dữ kiện compose và Dockerfile của `scan --json` (mất một
`depends_on`, lệch dòng một biến môi trường, thêm một tên máy giả, sửa một chỉ
thị Dockerfile), đối chiếu bằng PyYAML nên cần mạng. Ba kiểu đầu chạy lại trên
microservices-demo, nơi `link` và `runs` đến từ manifest Kubernetes, cùng kiểu
`rpc` cho cạnh gRPC khác ngôn ngữ, cộng hai kiểu ở NetworkPolicy (`check` giấu vi phạm do policy cho phép, giấu cảnh báo
`link` bị chặn), một kiểu ở lối vào từ Internet (`check` giấu vi phạm từ
`Internet`), một kiểu ở chart Helm (`check` giấu vi phạm nằm ở template;
`consistency.DEPLOY` đòi gỡ `link Frontend -> Cartservice` thì phải báo ở
`helm-chart/templates/frontend.yaml`). `--k8s-repo microservices-demo` thêm tám
kiểu ở dữ kiện manifest, cạnh qua Service và NetworkPolicy (mất một biến môi
trường, lệch dòng một workload, mất một cạnh, đổi đích một cạnh, mất một peer
của policy, đổi một workload bị cô lập thành không, bỏ lối vào của một workload,
mất một biến môi trường của manifest dựng từ chart).

Kiểu `rpc` xoá cạnh gRPC bắt buộc (`consistency.RPC`: etcd `Client -> Server`,
`Api -> Server`) khỏi cả luật `init` sinh ra lẫn vi phạm `check` báo.
`consistency.RPC` còn đòi bốn cạnh gRPC khác ngôn ngữ của microservices-demo:
Python → Go (`Recommendationservice -> Productcatalogservice`), Go → Python
(`Checkoutservice -> Emailservice`), Go → Node (`Checkoutservice ->
Currencyservice`, `Checkoutservice -> Paymentservice`).
`--proto-repo vllm` thêm hai kiểu ở dữ kiện `.proto` của `scan --json` (mất một
`rpc`, lệch dòng một `service`), đối chiếu bằng `protoc` của grpcio-tools nên
cần mạng. Kiểu `queue` xoá cạnh qua hàng đợi bắt buộc (`consistency.QUEUE`:
rabbitmq-tutorials `Python -> Go`, `Go -> JavascriptNodejs`, `JavascriptNodejs
-> Python`); ba kiểu `queue-*` phá mục `queues` của `scan --json` (mất, lệch
dòng, thêm giả), đối chiếu bằng `queue_truth.py` (`ast` cho Python, `go/ast`
cho Go). CI chạy bước này sau bộ đo, với `--rust-repo ripgrep
--notebook-repo matplotlib --deploy-repo example-voting-app --k8s-repo
microservices-demo --proto-repo vllm`: năm mươi ba kiểu phá.

Nó có mặt vì bộ đo này từng nằm ngoài git, và lúc đưa vào, đọc lại thì bốn
phép kiểm không kiểm gì: một cái chỉ xét file có tồn tại vì vế sau của `or`
không bao giờ được chạy tới, một cái chấp nhận `"0 "` ở bất cứ đâu trong đầu
ra, một cái gọi `export --mermaid` — cờ không tồn tại — mà không ai đọc mã
thoát, một cái đúng ra phải ghi thêm một dòng nhưng có thể xoá trắng file
luật. Rồi khi chuyển vào `target/`, bộ đếm file đếm ra 0 ở cả mười repo vì
đường dẫn tuyệt đối nào cũng chứa `target`. Giờ "0 vi phạm trên 0 file" là
một lỗi chứ không phải một lần đạt.

Một phép kiểm không thể đổ thì tệ hơn không có: nó vẫn cộng vào con số đạt.
