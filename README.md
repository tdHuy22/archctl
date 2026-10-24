# archctl

Sơ đồ kiến trúc là **luật**. AI viết mã. Bộ kiểm chứng đứng giữa.

Con người vẽ kiến trúc, luồng dữ liệu và ràng buộc. Agent lập trình có sẵn
(Claude Code, opencode, Codex) tự do viết mã, miễn không vi phạm. `archctl`
đối chiếu mã với sơ đồ và báo mọi chỗ đi chệch.

```bash
archctl init --from-code    # dựng bản nháp kiến trúc từ mã đã có
archctl check               # đối chiếu mã với luật
archctl scan <file>         # archctl đọc được import nào, trỏ tới đâu
archctl verify              # chạy các cách kiểm đã khai báo
archctl serve               # mở sơ đồ tương tác ở 127.0.0.1:4000
```

## Nó bắt được gì

```
kiến trúc c8bd3bd9 · 15 file quét (5 Rust, 4 C/C++, 4 Python, 2 Go) · 7/8 component trong diện kiểm

✗ VI PHẠM  Native -> LibPcap  (bị cấm, #locked)
  native/parser.c:6  import 'pcap.h'
  luật: deny * -> LibPcap except Capture
  → gọi Capture thay vì truy cập thẳng LibPcap

✗ VI PHẠM  Native -> Unsafe  (bị cấm, #locked)
  native/parser.c:18  dùng `strcpy` (2 lần trong file)
  → Native chưa được phép dùng nhóm Unsafe

✗ VI PHẠM  ToolsLib -> Captures  (không phải chủ sở hữu, #locked)
  tools/lib/io.py:8  chạm bảng `packets`
  → gọi Capture thay vì chạm thẳng vào dữ liệu của nó
```

**Sáu ngôn ngữ**: TypeScript/JavaScript, Python, C/C++, Rust, Go, Java.

| Luật | Kiểm bằng |
| --- | --- |
| `allow` / `deny` giữa component | đồ thị import, phân giải theo từng ngôn ngữ (C/C++ đọc `-I` từ build) |
| `api` — hàm và từ khoá bị cấm | dò định danh trên dòng token |
| cạnh liên ngôn ngữ | `extern "C"` và `C.x` ghép với định nghĩa hàm C; import Python ghép với module mở rộng (PyO3, pybind11, nanobind); `torch.ops.ns.op` ghép với `TORCH_LIBRARY` |
| `store owned_by` — truy cập dữ liệu | dò tên bảng trong chuỗi |
| `boundary ... contain` — lỗi không được lọt ra ngoài vùng | dò định danh ngoài vùng |
| `flow`, `contract`, `invariant state` | mọi tên khai báo phải tồn tại trong mã |
| `verify ... runs "..."` | lệnh phải chạy xong và đạt, trên đúng bản mã này |
| `service ... runs`, `link` — triển khai và mạng | đọc docker-compose, Dockerfile, manifest Kubernetes và Terraform: lệnh khởi chạy dẫn tới mã nào, `depends_on`, tên service hay Service Kubernetes trong biến môi trường hay viết thẳng trong mã (`"http://payment:80"`, `host="redis"`), tham chiếu Terraform trong biến môi trường (`aws_db_instance.main.address`) |
| cạnh gRPC (`allow`, và `link` khi đã khai service) | chỗ gọi stub (`XStub`, `NewXClient`) ghép với chỗ đăng ký server theo `package.Service` của `.proto`; Python và Go |
| cạnh qua hàng đợi (`allow`, và `link` khi đã khai service) | chỗ gửi ghép với chỗ nhận cùng topic Kafka, queue hay exchange RabbitMQ, kênh Redis, task Celery; tên từ env lần qua compose, manifest; Python, Go, Node |
| cạnh qua bước pipeline (`allow`) | bước trước đưa dữ liệu cho bước sau: `add_edge` của LangGraph, `>>` và `python_callable` của Airflow, stage `dvc.yaml` ghi file mà stage sau đọc; hàm của bước lần về file qua import của file đồ thị |
| trạng thái `#draft`…`#locked` | quyết định luật nào đang có hiệu lực |

Bốn khai báo cuối bắt đúng chế độ hỏng hay gặp nhất khi làm việc với agent:
**khai báo lạc hậu**. Agent đổi tên một bước, gỡ một năng lực — và sơ đồ im
lặng nói dối từ đó.

## Sơ đồ để thao tác, không chỉ để nhìn

`archctl serve` mở sơ đồ ở `127.0.0.1:4000`: kéo thả, khoá luật bằng một cú
bấm, duyệt đề xuất tại chỗ. Sơ đồ **tự cập nhật** khi agent sửa mã — nhìn là
biết nó đang làm tới đâu và chỗ nào đang hỏng.

Giao diện là một file HTML tự chứa, tự vẽ SVG, không framework và không gọi
mạng. Người dùng cuối chỉ cần đúng một binary.

`archctl export` xuất sơ đồ Mermaid tĩnh để commit vào repo hoặc dán vào mô tả
pull request; `--violations` phủ vi phạm thật lên trên.

## Agent có lối ra

Gặp luật sai, agent chỉ còn hai lựa chọn nếu không có gì khác: lách luật một
cách sáng tạo, hoặc kẹt vô hạn. Giao thức cho nó lối thứ ba.

```bash
archctl propose 'allow A -> B' --reason "..."   # agent đề xuất rồi DỪNG
archctl block --on 'deny * -> X' --reason "..." # agent báo bế tắc
archctl accept <id> --reason "..."              # người quyết định
archctl reject <id> --reason "..."
```

Agent **không bao giờ ghi** `system.arch` — nó chỉ ghi `proposals.arch`. Một
file một người ghi thì xung đột ghi đè và vượt quyền bị triệt tiêu bằng cấu
trúc, không phải bằng cơ chế phát hiện.

Mọi quyết định vào `decisions.md` kèm lý do bắt buộc. Agent không có trí nhớ
giữa các phiên, nên file đó chặn nó đề xuất lại thứ đã bị từ chối — và nó cũng
chính là ADR tự sinh ra trong lúc làm việc.

## Cài đặt

Một binary, không cần gì khác. Tải bản cho máy mình ở trang Releases
(`archctl-vX.Y.Z-<target>.tar.gz`), kiểm SHA-256, giải nén rồi chép vào một
thư mục trong `PATH`. Trên macOS:

```bash
shasum -a 256 -c archctl-v0.1.0-aarch64-apple-darwin.tar.gz.sha256
```

Trên Linux:

```bash
sha256sum -c archctl-v0.1.0-x86_64-unknown-linux-gnu.tar.gz.sha256
```

Rồi:

```bash
tar -xzf archctl-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
```

```bash
install archctl-v0.1.0-x86_64-unknown-linux-gnu/archctl ~/.local/bin/
```

Target có sẵn: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` (dựng
trên Ubuntu 24.04, nên cần glibc mới như của máy đó), `aarch64-apple-darwin`,
`x86_64-apple-darwin`. Windows chưa có bản dựng sẵn. Dựng từ mã cần Rust 1.88
trở lên, không tải crate nào:

```bash
cargo install --locked --path crates/archctl
```

Bản phát hành do `.github/workflows/release.yml` dựng khi đẩy tag `vX.Y.Z`
trùng `version` trong `Cargo.toml`; nó chỉ tạo bản nháp, kiểm đủ 8 file, người
duyệt xem rồi mới phát hành.

## Áp lên dự án đã có mã

```bash
archctl init --from-code   # đọc repo, dựng bản nháp, mọi thứ ở #draft
archctl baseline           # tha vi phạm có sẵn, chỉ chặn cái mới
```

Component nào có `allow` đang có hiệu lực thì component đó bắt đầu bị kiểm;
phần còn lại tạm để yên. Không cần mô tả cả repo trước khi có được giá trị nào.

## Chạy trong vòng lặp của agent

`archctl init` sinh sẵn hook chạy sau mỗi lần agent sửa file:

```json
{ "type": "command", "command": "archctl check --changed" }
```

| Phạm vi | 2000 file TypeScript, 7.8 MB |
| --- | --- |
| `archctl check` | 58ms |
| `archctl check --changed` | 25ms |
| `archctl check <file>...` | **11ms** |

Đo bằng trung vị của 21 lần chạy, tính cả thời gian khởi động tiến trình — hai
hàng dưới gần như chỉ còn phần đó.

Và trong CI, nơi agent không với tới:

```yaml
- run: archctl verify
- run: archctl check --strict
```

Thứ tự đó quan trọng, và CI không chép lại các lệnh kiểm — nó chạy những gì
file luật khai báo. Repo này tự chạy CI của mình:
[.github/workflows/ci.yml](.github/workflows/ci.yml), lý do từng dòng ở
[docs/ci.md](docs/ci.md).

## Tính chất

| | |
| --- | --- |
| Dependency ngoài std | **0** |
| Test | 360 |
| Binary | 1057 KB |
| Khởi động | 3ms |
| Mười repo thật, ghim theo commit | 4798 file · 0 bất nhất · 350/350 luồng lệnh |

Không gói ngoài nào cả — kể cả bộ quét import, bộ đọc JSON, máy chủ HTTP và
engine layout. Công cụ này chạy trong repo của người dùng và trong vòng lặp của
agent, nên mỗi dependency thêm vào là một bề mặt tấn công thêm.

## Tài liệu

| | |
| --- | --- |
| [docs/check.md](docs/check.md) | Ngữ nghĩa kiểm chứng, phân giải từng ngôn ngữ, giới hạn |
| [docs/declarations.md](docs/declarations.md) | `flow`, `contract`, `boundary`, `invariant` |
| [docs/verify.md](docs/verify.md) | Chạy thật các cách kiểm, và biên nhận tự hết hạn |
| [docs/api.md](docs/api.md) | Hàm và từ khoá bị cấm |
| [docs/ml.md](docs/ml.md) | Công thức luật cho dự án ML và LLM |
| [docs/infra.md](docs/infra.md) | Service, mạng, triển khai: đối chiếu với docker-compose, Dockerfile và manifest Kubernetes |
| [docs/protocol.md](docs/protocol.md) | Giao thức agent–người |
| [docs/adopt.md](docs/adopt.md) | `init --from-code` và `baseline` |
| [docs/serve.md](docs/serve.md) | Sơ đồ tương tác |
| [docs/diff.md](docs/diff.md) | Tách thay đổi thật khỏi thay đổi hình thức |
| [docs/ci.md](docs/ci.md) | Chạy ở nơi agent không với tới |
| [bench/README.md](bench/README.md) | Mười repo thật, ghim theo commit, một lệnh chạy lại |
| [docs/toolchain.md](docs/toolchain.md) | Cài và gỡ Rust |
| [docs/khao-sat.md](docs/khao-sat.md) | Có thứ gì dùng được cho các giới hạn còn lại không |

Bản đặc tả đầy đủ:
https://claude.ai/code/artifact/2b26df5d-d6b0-40d0-a398-7e574c7d1387

## Giới hạn đã biết

- **Một phần cạnh liên ngôn ngữ vẫn vô hình.** `extern "C"` của Rust và `C.x`
  của Go ghép được với định nghĩa hàm C. Python ghép được với module mở rộng
  (PyO3, pybind11, nanobind), op PyTorch có namespace viết thẳng, và thư viện
  dùng chung mà repo dựng khi nạp bằng `ctypes`/`cffi` với tên viết thẳng.
  Nhưng `dlopen` của C, con trỏ hàm, op có namespace là macro và IPC thì
  không. Với C/C++ thì `#include` là điều kiện cần nhưng rất xa điều kiện
  đủ, và đó là lý do có `api`.
- **Tầng hạ tầng mới đọc docker-compose, Dockerfile, manifest Kubernetes, và
  gRPC của Python, Go, Node, Rust, hàng đợi (Kafka, RabbitMQ, Redis pub/sub,
  Celery); NetworkPolicy được đối chiếu với `link`;
  lối vào từ Internet (Ingress, Gateway API, Istio, `LoadBalancer`, cổng
  compose) cần `link Internet -> …`; tên máy viết thẳng trong mã cũng cần
  `link`.**
  Chart Helm được tự dựng từ `values.yaml`, overlay Kustomize được tự dựng
  như `kustomize build` (tập con; ngoài tập con thì báo chưa đọc); Terraform
  được đọc tĩnh (ECS, Lambda, Cloud Run; module registry, `for_each` không
  hằng thì báo chưa đọc). gRPC của Java đọc được, C# thì không; cạnh mạng chỉ có lúc chạy (tên máy ghép trong mã,
  service discovery) thì không thấy — xem [docs/infra.md](docs/infra.md).
- **Đồ thị pipeline chỉ đọc phần khai tĩnh** của LangGraph, Airflow, dvc: tên
  bước từ biến, đồ thị dựng trong vòng lặp, nhánh rẽ bằng `Command`/`Send` thì
  báo chưa đọc; thứ tự bước và nhánh rẽ chụp bằng `verify` — xem
  [docs/ml.md](docs/ml.md#đồ-thị-pipeline-langgraph-airflow-dvc).
- **File trên 4 MB không được dò**, và được báo ra. Bộ dò giữ cả dãy token
  trong bộ nhớ, nên một bundle đã minify 32 MB ngốn hơn một gigabyte.
- **Thứ tự bước trong `flow` chưa kiểm.** Bộ quét không phân biệt được lời gọi
  với định nghĩa hàm, nên kiểm thứ tự sẽ báo nhầm. Cần trace runtime.
  `requires`/`ensures` của `contract` và `invariant data`/`money` cũng vậy — và
  chúng trỏ ra công cụ ngoài qua `verify`, và `archctl verify` chạy thật rồi
  ghi biên nhận — xem [docs/verify.md](docs/verify.md).
- **Đây là lan can, không phải hàng rào an ninh.** Agent chạy được shell nên vẫn
  ghi đè được file luật. Cưỡng chế thật nằm ở CI, CODEOWNERS trên `.arch/**`, và
  việc tên người duyệt bị ghi vào nhật ký.

## Nó tự áp luật lên chính nó

`.arch/system.arch` ở repo này mô tả kiến trúc của chính archctl, viết tay từ
thiết kế chứ không phải do `init --from-code` đoán. Một test chạy
`arch_core::check` trên repo, nên thêm một `use` sai hướng là `cargo test` báo
ngay:

```
vi phạm kiến trúc:
  Scan -> Model (crates/arch-core/src/scan/mod.rs:9 crate::model::Document)
```

Luật đáng chú ý nhất: **bộ quét không được biết gì về luật.** Nó chỉ nhận một
danh sách tên cần tìm. Nếu nó đọc được mô hình thì mỗi thay đổi trong DSL sẽ
kéo theo thay đổi trong lexer — chỗ dễ sai nhất của cả hệ thống.

Và `verify no_deps` đếm `[[package]]` trong `Cargo.lock`: phải đúng bằng hai,
là hai crate của chính workspace.

## Cấu trúc repo

```
archctl/
├── docs/         # tài liệu
├── templates/    # khung .arch/ mà `archctl init` sinh ra
├── examples/
│   ├── shop/     # dự án TypeScript, hai vi phạm cài sẵn
│   ├── sensor/   # Rust + C + Python + Go, mỗi ngôn ngữ một vi phạm
│   └── ml/       # dự án ML/LLM, mỗi công thức trong docs/ml.md một vi phạm
└── crates/
    ├── arch-core/   # parser, mô hình, kiểm chứng, layout, xuất sơ đồ
    └── archctl/     # CLI và máy chủ sơ đồ
```

## Ghi công

Cách phân loại thay đổi trong `archctl diff` lấy ý tưởng từ
[tt-a1i/archify](https://github.com/tt-a1i/archify) (MIT). Hai dự án bổ sung
cho nhau: archify biến một spec thành sơ đồ đẹp, archctl đối chiếu spec với mã.
Không có dòng code nào được sao chép.
