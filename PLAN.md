# archctl

Sơ đồ kiến trúc là luật. AI viết mã. Bộ kiểm chứng đứng giữa.
Mô tả sản phẩm ở [README.md](README.md); file này chỉ ghi trạng thái công việc.

## §0 Đang ở đâu, việc tiếp theo

Cập nhật 2026-10-16.

### Đã xong, có bằng chứng

- **Cạnh mạng viết trong mã, đợt 4 việc 8.1** (2026-10-15, commit `d2ecab8`; bạn chốt "làm
  theo đề xuất, miễn hiệu quả", trang https://claude.ai/artifact/EjLmvkEkkAFPDkyNyTnTwW).
  `scan/hosts.rs` đọc tên máy trong chuỗi của mã Python, JS/TS, Go, Rust, C/C++ theo
  luật tên máy của biến môi trường, với khoá là chỗ chuỗi đứng; tên khớp service của
  repo là cạnh từ mọi service chạy component đó, thiếu `link` là vi phạm (`check`,
  hook), `init` sinh `link`, `scan` in (lọc theo tên service, `--all-hosts`). Lexer:
  template JS, ký tự thoát, chuỗi thô Python, f-string `{…}`→`\0`, chuỗi ba nháy,
  `#define`, trần độ sâu f-string (lỗi tràn stack có từ trước); file không phải UTF-8
  được đọc ở mọi luật (trước bị bỏ cả file). Review: opus vòng 1 (10 mục), agent đối
  kháng (khoảng 150 biến thể), opus vòng 2 (4 mục) — sửa hết; oracle mới
  `bench/code_hosts.py` (ast, tokenize, go/ast) bắt thêm khoảng 15 lỗi. Threat model
  TM-32..35 đã có. Biên nhận trên mã cuối: `cargo test` (629), clippy, `cargo +1.88
  check`, bộ đo 30 repo 0 bất nhất 1050/1050, oracle khớp hết (7 903 tên máy),
  sabotage 62 ca 0 trượt (3 kiểu mới, 47), `verify` 3/3, `check --strict` 0; `init` trên
  4 repo có service sinh `link` y như trước, bộ đo chỉ ra 2 cạnh từ mã đều đúng.

- **Lối vào từ Internet, việc C của đợt 3** (2026-10-14, commit ngay sau `781b0e9`; bạn chốt "làm
  theo đề xuất", trang https://claude.ai/artifact/CwipwvTdGjT9rjhrg8Q8Hf). `Internet` là
  tên dành riêng: `link Internet -> X` cho X lộ ra ngoài; khai `service Internet` hay
  `link X -> Internet` là lỗi. Lộ ra ngoài gồm Ingress (rules, `defaultBackend`), route
  Gateway API (backendRef có `namespace`), VirtualService gắn gateway khác `mesh` (cả
  `delegate`), Service `LoadBalancer`/`NodePort`/`externalIPs`, `hostNetwork` (mọi cách
  viết true của YAML), `hostPort`, cổng compose (chuỗi, dạng dài, `network_mode: host`;
  trừ loopback cả IPv6). Thiếu `link` là vi phạm, mỗi service một mục; có `link` mà
  không gì lộ ra là cảnh báo `--strict` tính. `init` sinh link, `export`/`serve` vẽ nút
  Internet. Bộ đo: link Internet cho voting app, Online Boutique trong
  `consistency.DEPLOY`; oracle `k8s_truth.py` tính lại tập lộ ra ngoài; 2 kiểu phá mới
  (44). Threat model TM-29..31 đã có. Review opus: 6 mục (route lặp một Service làm
  hook chạy 78 s, backendRef khác namespace, `hostNetwork: True`, `::1:6000:6000`, host
  ngoài cụm và `delegate` của VirtualService, cạnh vi phạm trên `serve`), sửa hết, có
  test. Biên nhận trên mã cuối: `cargo test` (608), clippy, `cargo +1.88 check`, bộ đo
  30 repo 0 bất nhất 1050/1050, oracle khớp hết, sabotage 59 ca 0 trượt, `verify` 3/3,
  `check --strict` 0.

- **gRPC của Node và Rust, việc B của đợt 3** (2026-10-14, commit ngay sau `c5ae95d`). Node:
  `addService(proto.X.service)`, `new proto.X(addr)` (nạp `.proto` lúc chạy) và mã
  sinh `XService`/`XClient`; chuỗi thuộc tính là đuôi package (`Origin::Suffix`).
  Rust/tonic: `x_server::XServer`, `x_client::XClient`, cả `use` có ngoặc nhọn rồi
  tên trần, `impl X for`; ghép không phân biệt hoa thường (`KV` ↔ `KvServer`);
  `#[cfg(test)] mod` là server giả. Hook tìm phía bên kia cả TS/JS, Rust. Online
  Boutique thêm cạnh Go → Node (Checkout → Currency, Payment) vào
  `consistency.RPC`; kiểu phá `rpc` chạy cả trên microservices-demo. Threat model
  TM-26..28 đã có. Review opus: 5 mục (import ngoặc nhọn của tonic không dò, package
  Node từ gốc tuỳ ý, `tonic::codegen` bỏ cả file, `health*` quá rộng, server giả
  trong `#[cfg(test)]`), sửa hết, có test. Biên nhận trên mã cuối (gồm cả việc A):
  `cargo test` (601), clippy, `cargo +1.88 check`, bộ đo 30 repo 0 bất nhất 1050/1050,
  oracle khớp hết, sabotage 58/58, `verify` 3/3, `check --strict` 0.

- **NetworkPolicy đối chiếu `link`, việc A của đợt 3** (2026-10-14, commit `c5ae95d`;
  bạn chốt "theo đề xuất hết"). `scan/k8s.rs` đọc NetworkPolicy (podSelector,
  namespaceSelector, `matchExpressions`, ingress, egress, `policyTypes` mặc định
  theo `SetDefaults_NetworkPolicy`); `deploy.rs` tính từng workload bị cô lập
  chiều nào và được cho phép rõ với ai; `check`: cụm cho phép mà thiếu `link` là
  vi phạm (nêu tên policy), `link` bị chặn là cảnh báo `--strict` tính; `init`
  sinh `link` cho đường policy cho phép. Trên Online Boutique bắt một chỗ lệch có
  thật: deny-all của component `network-policies` chặn frontend →
  shoppingassistantservice mà env trỏ tới. Bộ đo: `policy`, `blocked` trong
  `consistency.DEPLOY`; oracle so dữ kiện policy và kết quả cô lập, cho phép của
  từng workload; ca vàng viết tay theo tài liệu Kubernetes; 4 kiểu phá mới (42).
  Threat model TM-23..25 đã có. Review opus: 4 mục (vòng xét đường cho phép bung
  bậc hai làm hook treo 25–91 s và 3,6 GB, `egress: []`, namespace một bên, thước
  đo không độc lập), sửa hết; ca DoS còn dưới 1,7 s, 163 MB. Biên nhận trên mã
  cuối: `cargo test` (596), clippy, `cargo +1.88 check`, bộ đo 30 repo 0 bất nhất
  1050/1050, oracle khớp hết, sabotage 57/57, `verify` 3/3, `check --strict` 0.
  Cùng lượt: `e31a772` đã lên `main` sau CI xanh 5/5 trên `thu/k8s` (run
  38540029655), kèm `4271fa7` sửa bộ đo (git tự bảo trì làm sabotage đổ ngẫu nhiên).

- **Manifest Kubernetes, việc cuối của tầng hạ tầng đợt 2** (2026-10-13, bạn cho commit 2026-10-14;
  bạn chốt "theo khuyên hết", trang https://claude.ai/artifact/Tzgo8YiLFnA9BhWjwtfE29).
  `scan/k8s.rs`: workload, Service (kể cả `ExternalName`), ConfigMap, `Namespace`,
  `*List`, `skaffold.yaml`, `kustomization.yaml` (patch, phép biến đổi). `deploy.rs`:
  image → skaffold, compose, hay `dockerfile:` trong luật; `command`/`args` theo
  Kubernetes; env và ConfigMap (`$(TÊN)`) → tên máy theo DNS → Service → selector
  (cả nhãn controller) → workload. DSL `from a, b` nhiều nguồn, `k8s:<tên>` kiểm
  mọi bản sao. `check`: `link`, `runs` (cả service không `runs` mà rõ ràng chạy
  mã), khai báo lạc hậu, workload chưa khai, patch hay phép biến đổi Kustomize đè
  lên workload đã khai, loại lạ có pod template, `x.y` không rõ namespace. `init`
  gộp compose với workload cùng nơi dựng, gom bản sao, tách ứng dụng khác nơi
  dựng. Bộ đo thêm microservices-demo (30 repo): `link`, `runs`, cạnh gRPC Python ↔
  Go bắt buộc; oracle `k8s_truth.py` (PyYAML) khớp 357 dữ kiện và 54 cạnh; 7
  kiểu phá mới. Threat model TM-18..22 đã có. Review: reviewer opus 6 mục, agent
  đối kháng 36 ca (22 lỗ), tự đo 4 ca DoS; sửa hết trừ chỗ đúng thiết kế (Secret,
  sidecar image dựng sẵn, workload chưa khai không tính `--strict`). Đổi so với
  trang chốt: `k8s:<tên>` không chọn một thư mục mà kiểm mọi bản sao, vì reviewer
  chỉ ra bản `release/` (bản triển khai thật) không được kiểm.

- **Cạnh gRPC, việc 6.1** (2026-10-12, commit `da89744`, đã push lên `main`
  2026-10-13 sau khi CI trên nhánh thử `thu/grpc` xanh 5/5, run 38341136312;
  bạn chốt "theo khuyên hết").
  `scan/grpc.rs`: bộ đọc `.proto`, dò client/server Python và Go (chỗ gọi và
  chỗ nhắc tới, generic handler, `Invoke`, `X_ServiceDesc`, dot-import), dấu cài
  đặt server; ghép `package.Service`. `check`: cạnh `allow` (via `rpc`), `link`
  khi đã khai service, hook tìm phía bên kia; `init` sinh `allow`/`link`; `scan`
  in chỗ gọi, đăng ký, cài đặt và `.proto`. Bộ đo: etcd `Client -> Server`,
  `Api -> Server` (`consistency.RPC`); oracle `.proto` bằng `protoc`
  (grpcio-tools ghim hash): vllm 54 dữ kiện, etcd, google-cloud, otel khớp;
  sabotage `rpc`, `proto-drop`, `proto-line`. Threat model TM-13..17 đã có.
  Review: reviewer opus 6 mục, agent đối kháng 13 mục (72 thư mục tấn công),
  đã sửa hết; DoS hook 140 s → 0,56 s, cả repo 311 s → 0,39 s. Biên nhận trên mã
  cuối: `cargo test` (548), clippy, `cargo +1.88 check`, bộ đo 29 repo 0 bất
  nhất 1015/1015, oracle `--require …,deploy,proto` khớp hết, sabotage 43/43,
  `verify` 3/3, `check --strict` 0.

- **Tầng hạ tầng, đợt 1** (2026-10-12, commit "Tầng hạ tầng đợt 1…"; bạn chốt "theo khuyên hết").
  DSL `service <Tên> [from compose:…|dockerfile:…] [runs …]` và `link A -> B`
  (không gian tên riêng); bộ đọc YAML tập con (khớp PyYAML 793/804 file YAML của
  bộ đo), Dockerfile, lệnh khởi chạy, compose; ba phép kiểm trong `check` (cạnh
  mạng thiếu `link` — kể cả tới service chưa khai —, mã chạy ngoài `runs`, khai
  báo lệch), hook đọc lại cấu hình khi file đổi có thể là thứ cấu hình trỏ tới;
  `init --from-code` sinh `service`/`runs`/`link`; `export`, `serve`, `scan`
  (mặc định che đối số và `user:pass@`, `--verbatim` để xem đủ); `docs/infra.md`.
  Bộ đo thêm example-voting-app, open-webui (29 repo), oracle `--require deploy`
  (PyYAML, dockerfile-parse ghim hash), 8 kiểu phá mới. Threat model TM-8 tới
  TM-12 đã có, kèm `file:dòng` và test. Review ba vòng (differential-review, agent
  đối kháng, reviewer opus hai lượt): tìm 6 + 11 + 14 + 4 (cộng F12 của phiên
  chính), sửa hết, mỗi mục tái hiện trên thư mục tấn công trước và sau; mọi ca
  DoS dưới 0,4 s (trước tới >60 s, 4,6 GB); báo cáo ở scratchpad
  `review/DIFFERENTIAL_REVIEW_REPORT.md`. Biên nhận trên mã cuối: `cargo test`
  (528), clippy, `cargo +1.88 check`, bộ đo 29 repo 24 129 file 0 bất nhất
  1015/1015, oracle `--require clang,node,go,syn,ipython,deploy` khớp hết (hạ tầng
  12 repo, 49 file, 4 792 dữ kiện), sabotage 38/38, `verify` 3/3, `check --strict`
  0. Chưa kiểm: nhánh hằng `O_NOFOLLOW` của Linux chỉ biên dịch trên CI.

- **Oracle đối chiếu cạnh binding** (2026-10-11, commit `3167cc9`; bạn chọn raw token
  của clang). `archctl scan` in thêm cạnh liên ngôn ngữ của từng file (khoá
  thô). `oracle.py` bảng "Binding": C/C++ trên token thô của clang, `torch.ops`
  theo `ast`, PyO3 theo `syn`. Lần đầu chạy khớp hết: vllm 287 file 1 470 đầu
  cạnh, apex 34, matplotlib 14, pydantic-core 1. Nó bắt được một lỗi thật:
  lexer giữ ký tự xuống dòng của `\` nối dòng trong chuỗi, nên op có schema
  viết qua hai dòng bị mất; đã sửa ở lexer. Review (sonnet) tìm 2 vừa, 5 nhẹ,
  đã sửa: `.def` của `class_` trong `TORCH_LIBRARY` bị tính là op (cả archctl
  lẫn oracle; giờ chỉ `.def` trên biến module), oracle không bỏ token
  `comment`, token nhiều dòng; sabotage thêm `bind-drop`, `bind-extra`,
  `bind-call`, `bind-op`: 24/24. Biên nhận trên mã cuối: test, clippy, `cargo
  +1.88 check`, `verify` 3/3, `check --strict` 0; bộ đo 27 repo 0 bất nhất
  945/945; oracle `--require clang,node,go,syn,ipython` khớp hết (Binding: vllm
  288 file 1 471, gồm một `#[pymodule]` Rust chỉ thấy khi có `syn`). Đã commit
  trước đó: `c0de687` (mốc ML/LLM), `7fd016b` (oracle notebook,
  `TORCH_EXTENSION_NAME`).

- **Mốc "Còn mở sau ML/LLM"** (2026-10-11, commit `7fd016b`). `oracle.py --require
  ipython` đối chiếu import notebook với IPython (`bench/notebook_truth.py`,
  gói ghim kèm sha256 trong `bench/ipython-requirements.txt`, chỉ wheel):
  matplotlib 3 notebook 16 import khớp cả dòng; các notebook khó của test cũng
  khớp. Sabotage thêm `nb-drop`, `nb-line`: 17/17. `TORCH_EXTENSION_NAME` đọc từ
  `setup.py` (danh sách nguồn viết thẳng, hoặc đúng một extension): bộ đo thêm
  `apex` (313 file), 42 lời gọi tới 19 module, cả 19 khớp `setup.py` theo `ast`;
  cạnh bắt buộc `Bottleneck -> Csrc`. Threat model TM-5 (gói PyPI trong CI),
  TM-6 (`setup.py` lồng sâu, nhiều tên, symlink tới `/dev/zero`; có test cho
  cả ba). Review (sonnet) tìm 2 vừa, 3 nhẹ, đã sửa cả 5: đọc `setup.py` từng
  bậc hai theo số tên (78 000 tên 5,4 s) và đọc không giới hạn qua symlink.
  Biên nhận trên mã cuối: test, clippy, `cargo +1.88 check`, `verify` 3/3,
  `check --strict` 0; bộ đo 27 repo 23 832 file 0 bất nhất 945/945; sabotage
  17/17; oracle `--require clang,node,go,ipython` khớp hết. Đường hook một file
  không chậm hơn bản gốc ở repo nào.

- **Mốc ML/LLM** (2026-10-11, commit `c0de687`). `docs/ml.md` với sáu công thức luật
  và `examples/ml` (mỗi công thức một vi phạm cài sẵn, cộng một vi phạm trong
  notebook; test `ml.rs` đòi đủ bảy). Đọc `.ipynb` (`scan/notebook.rs`): chỉ ô
  code, số dòng là dòng trong file. Threat model 4 mục ở `.agent/threat-model.md`,
  cả 4 chốt chặn có test. Bộ đo thêm `kubernetes` (sparse scheduler, 277 file)
  và `pytorch-image-models` (346 file): 0 bất nhất, 70/70; oracle khớp
  (Go 3 320 import, Python 3 129). Bộ đo 26 repo 23 519 file 0 bất nhất 910/910,
  sabotage 14/14 (trước review). Review (opus) tìm 1 vừa, 5 nhẹ, đã sửa cả 6:
  công thức `api` bỏ `eval` (bắt nhầm mọi `model.eval()`), magic trong chuỗi
  `"""`/ngoặc không bị bỏ, khoá JSON trùng lấy khoá cuối, `%%script --bg
  python3`, số repo trong tài liệu. Biên nhận trên mã cuối: test, clippy,
  `cargo +1.88 check`, `verify` 3/3, `check --strict` 0; bộ đo 26 repo 23 519
  file 0 bất nhất 910/910; sabotage 14/14.

- **Cạnh Python ↔ native** (2026-10-11, commit `0d81811`). Ghép import Python với
  module mở rộng (PyO3 `#[pymodule]`, `PYBIND11_MODULE`, `NB_MODULE`,
  `PyInit_`), và `torch.ops.ns.op` với `m.def` trong `TORCH_LIBRARY*`. Chỉ
  namespace viết thẳng: đoán namespace macro từng ghép nhầm
  `_vllm_fa3_C` trên vllm. Cùng tên module mà nạp từ nhiều chỗ thì stub `.pyi`
  quyết. `init --from-code` đưa thư mục native bên cạnh gốc vào sơ đồ (vllm
  `Csrc`, pydantic-core `PydanticCore`). Bộ đo có `CROSSINGS` (4 cạnh bắt
  buộc) và kiểu phá `crossing`. Review (sonnet) tìm 5, sửa 3 (cạnh giả khi
  hai package cùng nạp `_core`, `descend` tính cả `tests/`, đường dẫn trong
  danh sách bỏ sót), 2 ghi vào tài liệu. Biên nhận sau sửa: 441 test, clippy
  `-D warnings`, `cargo +1.88 check`, `verify` 3/3, `check --strict` 0; bộ đo
  24 repo 22 893 file 0 bất nhất 840/840; sabotage 14/14. vllm: 265 lời gọi
  op ghép được, cả 161 tên có schema trong `csrc/`. matplotlib: `.ft2font` ở
  `_backend_pdf_ps.py:26` (lỗi của matplotlib) từng ghép nhầm vào module native,
  giờ lại báo chưa phân giải. Đo A/B: `check` +0–3%, hook không đổi, `init`
  vllm 592 → 812 ms.

- **Phần Rust của oracle đổ được** (2026-10-04): `sabotage.py --rust-repo
  ripgrep` phá thêm ba kiểu ở file Rust (mất import, lệch dòng, import giả
  ngoài macro — kiểu cuối giữ phép miễn trừ trong macro không rộng ra). CI
  bật nó và mở mạng cho bước sabotage. Run 37216343770 trên nhánh thử: 16/16
  lượt đạt (13 kiểu phá + 3 lượt không phá).

- **Rust có thước đo chuẩn, chỉ trên CI** (2026-10-04): `oracle.py --require
  syn` dựng trình đọc `syn` 2.0.87 (ghim `=`, không Cargo.lock) dưới
  `target/bench/syn-oracle/`; CI mở mạng cho riêng bước đó, archctl vẫn dựng
  offline. Máy dev không bật, không tải gì. Run 37214317001 trên nhánh thử:
  6 repo, 1 509 file, 18 303 import khớp cả tên lẫn dòng ngay lần đầu, 0 file
  syn không đọc được; 548 import trong thân macro (tokio 534) không đối chiếu
  được và đếm riêng.

- **Go có thước đo chuẩn** (2026-10-04, `261799d`, CI xanh ở run 37213550539
  với Go 1.24 có sẵn trên image `ubuntu-24.04`, khớp đúng các số trên máy dev): `oracle.py` dựng một trình
  đọc `go/parser` (`ImportsOnly`) thành binary tạm, đối chiếu cả tên lẫn dòng;
  CI đòi `--require clang,node,go`. Lần chạy đầu bắt được lỗi thật: `import
  bolt "go.etcd.io/bbolt"` (tên đặt lại, không ngoặc) bị bỏ qua, etcd mất 4
  import, hugo 1. Đã sửa ở `scan/golang.rs` theo test-trước. Biên nhận: 420
  test, clippy `-D warnings`, `cargo +1.88 check`; bộ đo 24 repo 22 893 file,
  0 bất nhất, 840/840; `sabotage.py` 12/12; `oracle.py` khớp hết, riêng Go
  2 168 file, 13 921 import. Go cài trên máy dev bằng Homebrew (1.27.1).

- **CI xanh với bước đối chiếu parser chuẩn** (run 37211617194, commit
  `4fa0ff4`, 2026-10-04): job `repo thật` 7m52s trên Ubuntu 24.04, Python 3.12;
  24 repo 22 893 file, 0 bất nhất, 840/840; `sabotage.py` bắt được cả 12/12 kiểu phá;
  `oracle.py --require clang,node` khớp hết ở cả Python, C/C++ và JS/TS (bước
  này mất khoảng 4 phút).

- **Bộ trích import sửa theo parser chuẩn, đã push lên `main`.** Rà 2026-10-04 bằng
  cách đối chiếu với `ast` của CPython, lexer thô của clang và parser module
  của Node trên 24 repo của bộ đo; mọi lỗi tìm thấy đã sửa theo test-trước,
  26 test mới:
  - TypeScript: `type` trong ngoặc import/export không còn làm mất cả câu
    (Vue core từng mất 363/1 728 import tĩnh, excalidraw 59/1 433, sơ đồ tự
    dựng của Vue core thiếu `RuntimeCore -> CompilerDom` và hai cạnh khác);
    `import x = require()`.
  - Lexer dùng chung: chuỗi một dòng và char literal dừng ở cuối dòng (trừ
    chuỗi Rust), `\` nối dòng được đếm dòng; chỉ thị C bỏ qua tới hết chú thích
    mở trên dòng đó và hết các dòng nối (`redis/src/tls.c` từng bị đọc ngược:
    0/31 `serverLog`); dấu phân cách chữ số C++14; chuỗi thô C++11, Go,
    Rust `br`/`cr`; `\"` trong chuỗi ba nháy; f-string PEP 701 và t-string;
    BOM.
  - Python: `raise E from e`/`yield from x` không còn là import (1 045 import
    giả, 15 import thật bị nuốt); nối dòng `\`; `;`; `from pkg import sub` trỏ
    cả tới module con (171 lần đích ở component khác, 6 cạnh mới trong sơ đồ
    tự dựng); Cython `.pyx .pxd .pxi` với `cimport`.
  - FFI: hàm trong `extern "C" { }` là bên cung cấp; `extern { }` của Rust
    không ghi ABI là bên gọi. Hàm trong namespace C++ cố ý vẫn không (tên đã
    mangle).
  - File mới được quét: CUDA `.cu .cuh`, `.vue` (chỉ `<script>`). `.inc` cố ý
    không (curl dùng cho Makefile, scikit-learn cho reStructuredText).
  - Lệnh mới `archctl scan [--json]`: in import đọc được và đích phân giải.
  - Bộ đo có phép thử thứ ba `bench/oracle.py` (đối chiếu với parser chuẩn),
    `sabotage.py` thêm ba kiểu phá nhắm vào nó, CI chạy nó với `--require
    clang,node` (image `ubuntu-24.04` có Python 3.12, Clang 18, Node 22).
  - Review độc lập của diff bắt thêm hai lỗi do chính loạt sửa này gây ra,
    đã sửa kèm test: `import()`/`require()` trong `export default {...}` bị
    nuốt (đếm độ sâu ngoặc từng miễn mọi từ khoá, giờ chỉ miễn `type`); dò
    module con từng nhảy sang project bên cạnh có gói cùng tên (giờ gói thường
    chỉ tìm trong thư mục của nó, module file không có module con). Cùng đợt:
    `.cuh` khớp tên duy nhất, `oracle.py` dừng khi chính thước đo hỏng.
  - Biên nhận: 419 test, clippy `-D warnings`, `cargo +1.88 check`, `archctl
    verify` 3/3 và `check --strict` 0 vi phạm trên chính repo; bộ đo 24 repo
    22 893 file (thêm 255 file mới quét được), 0 bất nhất, 0 cảnh báo, 840/840;
    `sabotage.py` 12/12; `oracle.py` khớp hết: Python 103 539 import, C/C++
    23 044 `#include`, JS/TS 3 310 import tĩnh, cộng thư viện chuẩn của
    Python 3.14 (13 599 import). Thời gian `check` sau khi thêm phần dò module
    con: opentelemetry-python 233 → 193 ms, các repo khác +0–15%; hook netbox
    29 → 28 ms.

- **Bộ đo tải được một phần monorepo (sparse), và có ca PEP 420 lồng hai
  cấp.** `repos.tsv` thêm cột `sparse`; `bench.py` tải bằng
  `--filter=blob:none` + `sparse-checkout --cone`, phép đếm bỏ file git đánh
  dấu `S`, thư mục sparse gõ sai là mã thoát 2 (kể cả khi dùng lại cache),
  sparse cùng submodule bị từ chối. google-cloud-python, 11 package (83 MB
  thay vì 1,3 GB): `google/` và `google/cloud/` đều không có `__init__.py`,
  1554 file, 11 component, 18 cạnh giữa distribution. 24 repo, 22638 file, 0
  bất nhất, 840/840, `sabotage.py` 7/7; 22 repo cũ ra đúng số cũ.

- **PEP 420 có repo thứ hai: matplotlib.** Hình dạng khác otel:
  một distribution, gốc import là `lib/` chứ không `src/`, `mpl_toolkits/`
  không có `__init__.py` nằm cạnh gói thường `matplotlib`. 76 cạnh từ
  `galleries/`, `doc/` sang `Lib` đi qua `mpl_toolkits.*`. Bộ đo: 1219 file, 0
  bất nhất, 35/35. Hai cảnh báo `.font_manager`/`.ft2font` ở
  `lib/matplotlib/backends/_backend_pdf_ps.py:25-26` là lỗi của chính
  matplotlib (phải là `..`), nằm trong `if TYPE_CHECKING:` nên không bao giờ
  chạy; archctl phân giải đúng. jaraco.text đã xét rồi bỏ: 9 file.

- **Cạnh cgo giữa hai component giờ được đo.** Thêm libbpfgo
  (Go ở gốc, submodule C `libbpf/`) vào bộ đo; nó lộ ba chỗ hỏng, cả ba đã
  sửa, có test:
  - `selftest/` (24 thư mục, mỗi cái một `main.go`) thắng gốc repo khi chọn
    bố cục; 145/205 file — cả thư viện — nằm ngoài sơ đồ. `selftest`,
    `selftests` giờ nằm trong `ASIDE`.
  - Bộ khám phá chỉ đọc import, không ghép cạnh ffi như `check`, nên luật
    sinh ra tự vi phạm (`Core -> Selftest` qua ffi). Giờ nó ghép theo đúng
    cách của `check`.
  - Cạnh đó còn là cạnh sai: `s32 BPF_STRUCT_OPS(name, …) {` bị đọc thành
    định nghĩa hàm `BPF_STRUCT_OPS`. Giờ tên viết hoa toàn bộ mà đối số đầu là
    một tên trần thì là macro; `SHA256(const unsigned char *d, …)` vẫn là hàm
    (review bắt bản đầu làm mất cạnh tới hàm viết hoa của OpenSSL).
  Kết quả: `Core -> Libbpf` có 113 lời gọi cgo, 111 tên. 22 repo, 19865 file,
  0 bất nhất, 0 cảnh báo luật, 770/770, `sabotage.py` 7/7; 21 repo cũ ra đúng
  số cũ. 390 test, clippy sạch.

- **Bộ đo lên hai mươi mốt repo:** thêm abseil-cpp (C++ Bazel,
  include từ gốc `absl/...`), opentelemetry-python (sáu distribution chung
  namespace PEP 420 `opentelemetry`; cạnh `Sdk -> Api`, `Exporter -> Proto`
  ra đúng), go-sqlite3 (cgo). Lần chạy 2026-10-03: 19660 file, 0 bất nhất, 0
  cảnh báo luật, 735/735, `sabotage.py` 7/7, 120 giây.
- **Header vendor chỉ thuộc về cây chứa nó.** glfw (đã thử rồi bỏ) có năm
  cạnh sai giữa các thư mục phiên bản: `<vulkan/vulkan.h>` của v3.3 khớp tên
  duy nhất vào `v3.2/.../deps/`. Giờ file dưới `deps/`, `third_party/`,
  `external/`… chỉ được đoán cho file cùng cây; `deps/` ở gốc (redis) thì cả
  repo thấy. Review trước commit siết thêm hai chỗ: có bản vendor bị ẩn thì
  chỉ nhận bản vendor của chính cây mình (stub trong `tests/` không thành
  "duy nhất"), và tên vendor ghi trong chính include (`<external/api.h>`) là
  chủ ý, không bị ẩn. 11 repo C/C++/Go cũ ra đúng số trước khi sửa. 385 test,
  clippy sạch. Sau khi sửa glfw không còn cạnh nào để kiểm nên bị loại khỏi bộ đo;
  test `c_header_vendor_chi_thay_duoc_tu_cay_chua_no` giữ lại ca này.

- **Đã push lên `main`, CI xanh trên Linux.** Run 37134780674: 4/4 job, bộ
  đo 18 repo ra đúng số của macOS (17918 file, 0 bất nhất, 630/630,
  sabotage 7/7), job `repo thật` 2 phút 6 giây. Runner Linux ghim
  `ubuntu-24.04` (`ubuntu-latest` chuyển sang 26 từ 19/10/2026); run
  37135164321 đạt 4/4.

- **Bộ đo lên mười tám repo, phủ đủ năm ngôn ngữ, mỗi ngôn ngữ ít nhất hai
  repo** (C++ một: leveldb). Thêm etcd, excalidraw, langchain, leveldb, netbox,
  nginx, scikit-learn, vllm; zstd-rs giờ kéo submodule C (cột submodule mới
  trong `repos.tsv`). Tám repo mới bắt được bốn chỗ archctl ra **0 cạnh, 0
  cảnh báo**, cả bốn đã sửa, có test: Go nhiều `go.mod` (etcd), gốc Python là
  thư mục có `pyproject.toml`/`manage.py` (langchain, netbox), C không có file
  build nào khai `-I` thì khớp tên duy nhất (nginx — bạn đã duyệt đổi nguyên
  tắc). Lần chạy 2026-10-03: 17896 file, 0 bất nhất, 0 cảnh báo luật,
  630/630 luồng, `sabotage.py` 6/6; tải 685 MB, 120 giây.
- **`dist`, `build`, `out`, `coverage` không còn bị bỏ theo tên.** Có
  `.gitignore` thì hỏi nó; không có thì bỏ như cũ. etcd từng mất package Go
  `tests/robustness/coverage/` trong im lặng. Bộ đo giờ đòi số file archctl
  quét bằng số file nguồn git có (`git ls-files`, phần ignore do `git
  check-ignore` trả lời): 18/18 repo khớp, 17918 file. `sabotage.py` thêm
  kiểu phá thứ bảy (quét thiếu một file), bắt đúng.
- **Review trước commit sửa thêm bốn chỗ:** hook `check <file>` từng chậm gấp
  ba trên repo Python (netbox 28→78ms; giờ 33ms) vì quét cây tìm `go.mod` và
  bốn dấu hiệu Python mỗi lần; `<stdio.h>` từng khớp vào `wasm-shim/stdio.h`
  của zstd (363 cạnh sai) — header chuẩn không bao giờ khớp tên; `.gitignore`
  phía trên gốc archctl trong monorepo giờ được đọc; `dist/*` tính như bỏ
  `dist`.
- **381 test, clippy sạch với `-D warnings`, 0 dependency ngoài std.** Biên
  nhận ghi bằng `receipt.py` ngày 2026-10-03.
- **Rust: `use crate::X` với `X` ở gốc crate giờ có đích.** Trước đó khoảng
  470 câu `use` như vậy ở năm repo Rust của bộ đo không sinh cạnh nào, không
  cảnh báo nào. Đích là nơi định nghĩa: viết thẳng trong file gốc thì là file
  gốc, re-export thì nhìn xuyên tới module thật. Gốc crate tính theo từng file
  (thư viện, binary, test tích hợp, ví dụ, đường dẫn trong `Cargo.toml`). Bộ
  quét đọc cây `use` đúng nghĩa và theo dõi `mod` lồng. Bộ đo thêm 5 cạnh, cả 5
  đã lần ra tới dòng mã: rusqlite `Blob -> Core`, `Blob -> Util`, `Util ->
  Core`; ripgrep `Ignore -> Globset`, `Regex -> Matcher` — hai cái sau đến từ
  kiểu `use { a::B, c::D };` mà bộ quét cũ trả về rỗng. Vẫn 0 bất nhất, 0 cảnh
  báo luật, 350/350; thời gian `check` trong ngưỡng nhiễu.
- **Repo ở GitHub: [tdHuy22/archctl](https://github.com/tdHuy22/archctl),
  private.** Lần chạy CI đầu tiên trên runner thật (run 37108650598): 4/4 job
  đạt, 70 giây, và số liệu bộ đo trên Linux khớp macOS — trừ một cột, xem dòng
  dưới.
- **Kết quả giờ giống nhau trên mọi máy.** CI Linux cho `rusqlite` 11 luật
  `allow`, macOS cho 12: ổ mặc định của macOS không phân biệt hoa thường, nên
  `use crate::Error` được "tìm thấy" là `src/Error.rs`. Lỗi có ở cả năm ngôn
  ngữ (`./Util`, `Config.h`, `pkg.Helpers`, package Go `Internal`). Giờ mọi
  lần dò đường dẫn đối chiếu tên thật trong thư mục; macOS ra đúng bảng của CI
  Linux ở mọi cột không phải thời gian, cả mười repo. Giá: 1–6% thời gian
  `check` cả repo, đường hook không đổi (11,1ms).
- **Mười repo thật, chạy lại được bằng một lệnh**: `python3 bench/bench.py`
  (hugo, tokio, ripgrep, vue core, flask, curl, redis, pydantic-core, zstd-rs,
  rusqlite — ghim theo commit trong `bench/repos.tsv`). Lần chạy 2026-10-03:
  4798 file (khớp đếm độc lập ở từng repo), **0 bất nhất**, **0 cảnh báo trong
  luật sinh ra**, luồng lệnh **350/350** (35 mỗi repo), 36 giây khi repo đã tải.
  Bộ đo đổ được, và điều đó cũng được kiểm bằng một lệnh:
  `python3 bench/sabotage.py` phá archctl bảy kiểu, mỗi kiểu bị đúng phép kiểm
  của nó bắt. Mất mạng, tên repo gõ sai, `repos.tsv` có tên `../..` hay SHA mở
  đầu bằng `-` đều ra mã thoát 2.
- **Bất biến tự nhất quán**: luật do `init --from-code` sinh ra, nâng lên
  `#accepted` rồi kiểm lại chính mã đó → 0 vi phạm trên cả 10 repo. Mỗi vi
  phạm còn lại sẽ là một chỗ bộ khám phá và bộ kiểm chứng nhìn khác nhau.
- **`verify` chạy thật.** `verify <tên> runs "<lệnh>"` khai báo cách kiểm,
  `archctl verify` chạy và ghi biên nhận, `archctl check` nhắc khi biên nhận
  đã cũ hơn mã hoặc luật. Chỉ `verify` thực thi lệnh; `check` không bao giờ,
  và `propose`/`accept` đều từ chối đề xuất mang theo lệnh shell.
- **Tự áp luật lên chính nó.** `.arch/system.arch` ở repo này viết tay từ
  thiết kế; `check` ở gốc ra 0 vi phạm, 0 cảnh báo. Test `dogfood.rs` chạy
  `arch_core::check` trên repo nên vi phạm kiến trúc làm `cargo test` đổ.
- **CI chạy ở `.github/workflows/ci.yml`**, bốn job: `luật` (dựng → `diff`
  vào log → `verify` → `check --strict`), `nền tảng khác` (macOS, gọi thẳng
  `cargo test`), `rust tối thiểu` (đọc `rust-version` từ `Cargo.toml` rồi biên
  dịch lại bằng đúng toolchain đó), `mười repo thật` (bộ đo ở trên, mỗi pull
  request). Không action bên thứ ba; `actions/checkout` ghim theo SHA. Lý do
  từng dòng ở [docs/ci.md](docs/ci.md).
- Hiệu năng: 2000 file TypeScript 57ms; đường hook (`check <file>`) 11ms;
  cây 5078 file: `init` 1.57s, `check` 1.08s, 35 MB.

Bộ đo ở [bench/](bench/README.md). Repo tải về `target/bench/repos/`, ngoài
git, `cargo clean` dọn.

### Việc tiếp theo

1. `c5ae95d`..`82b069a` đã lên `main` (2026-10-15) sau CI xanh 5/5 trên `thu/dot4`
   (run 38760676354); `82b069a` nâng trần job "repo thật" lên 40 phút vì run trước
   bị huỷ ở bước đối chiếu. Nhánh thử đã xoá.
2. Đọc chart Helm (đợt 5) đang dở, CHƯA COMMIT (cây làm việc có thay đổi, file mới
   `crates/arch-core/src/scan/helm.rs`, `bench/fixtures/`). Đã có: tự dựng tập con
   template, nguồn `values:`, `scan` in bản dựng, oracle so với `helm template`
   v3.15.4 (sha256 ghim), 2 kiểu phá mới, CI `--require ...,helm`, tài liệu.
   Review opus vòng 1 (10 mục) và vòng 2 (11 mục, 2026-10-16) đã sửa hết. Vòng 2 gồm:
   `(x) .Field` đọc nhầm thành `(x).Field` (token `Chain`), alias của chart con
   (`.Chart.Name`, một chart hai alias), gộp values mặc định của chart con trước khi
   xét `condition` (`build`/`flatten`), tràn stack do `list $d` lồng (trần độ lồng
   khi dựng), thời gian bậc ba theo số chart con (800 chart con: 213 s xuống 0,4 s),
   dependency thiếu trong `charts/` báo, define của chart tắt không tham gia,
   `values:` mơ hồ khi nhiều chart, số trong toYaml/toJson, `gt` với số thực. So với
   helm thật trên các chart thử của reviewer đều khớp. TM-36..39 đã có.
   Kiểm chứng cuối có biên nhận (2026-10-16, máy rảnh) ALL-OK: test, clippy, MSRV,
   release, bộ đo, oracle `--require ...,helm`, sabotage, verify, `check --strict`.
   Đã commit `becab75`, lên main sau CI xanh 5/5 (run 39041527752).
3. Test thời gian đo bằng thời gian CPU: commit `70af652`, `24b091c` (chưa push).
   `.arch/system.arch` chưa có `testclock.rs` nên test dogfood đỏ từ `70af652`;
   đã sửa trong cây làm việc (thêm vào component Util), commit cùng đợt 6.
4. Đợt 6, hàng đợi (2026-10-19), CHƯA COMMIT: bạn chốt "làm theo đề xuất"
   (trang https://claude.ai/artifact/9QrPEQYLevs4b2LWki6J2K). Đã có:
   `scan/queue.rs` (Kafka, RabbitMQ, Redis pub/sub, Celery; Python, Go, Node;
   hằng số cùng file, env, queue khai rồi dùng qua biến), `check` ghép theo hệ
   và tên, `allow`, `link`, "chưa ghép" cho tên động, hook tìm phía bên kia;
   tên từ env lần qua compose và manifest (chỉ giữ giá trị của biến mà mã đọc
   làm tên topic, giữ TM-19); `init`, `scan`, `serve`; tài liệu; bộ đo:
   repo `rabbitmq-tutorials`, thước đo `bench/queue_truth.py` (`ast`, `go/ast`)
   khớp 37 + 13 + 54 (bộ mẫu `fixtures/queues`), bảng `consistency.QUEUE`, 4
   kiểu phá mới (53). Threat model TM-40..43 còn "cần làm" tới khi reviewer
   xác nhận. Reviewer opus vòng 1 (8 mục: hook chậm vì `psubscribe`, hằng số
   không theo phạm vi, lời gọi nhận ra mà không báo tên, `init` chưa lần env,
   bậc hai trên file lồng sâu, thiếu `link` chỉ báo đích đầu, `$(SECRET)` của
   k8s, test dùng thời gian thực) đã sửa hết. Vòng 2 (7 mục: hook bên gửi Redis
   không thấy `psubscribe`, hằng số trong callback, tham số TS có kiểu trả về,
   nhiễu `res.send`/`ws.send`, bậc hai ở tham số lồng, vai của `cfg.Topic` Go,
   oracle `QueueDeclare`) đã sửa. Vòng 3 (literal nhiều dòng bị coi là gán
   nhiều vế, tên env bị bỏ khi biến không rõ là producer, client có chú thích
   kiểu, kiểu trả về generic có phẩy, dòng dài bậc hai) đã sửa; thước đo khớp
   bộ mẫu 72 và 4 repo; test lib 14 + tích hợp 19 đạt. TM-40..43 "đã có".
   Kiểm chứng cuối có biên nhận (2026-10-19): `cargo test`, clippy, MSRV 1.88,
   release, bộ đo 31 repo, oracle `--require …,helm`, sabotage 53 kiểu, `verify`
   3/3, `check --strict` 0 (thêm `bench/fixtures/**` vào `.arch/.archignore`).
   Commit `bb2ac35` (chưa push; 70af652, 24b091c cũng chưa push).
   Còn lại trong mốc: Overlay Kustomize (hướng B).

**Đã quyết (2026-10-11): cạnh binding Python ↔ native tự viết.** Đã tìm theo
skill reuse-best. Không công cụ tĩnh nào phủ cả PyO3, pybind11, nanobind
và `torch.ops`. [metacall/meta-ast](https://github.com/metacall/meta-ast)
chỉ nhận lời gọi `metacall_load_from_*`.
[pybind11-stubgen](https://github.com/pybind/pybind11-stubgen) phải import
module đã build. Oracle sau này: `ast` cho phía Python, `syn` cho PyO3,
raw token của clang cho C++.

Các repo `googleapis/python-*` đơn lẻ đã archive và rỗng, nên ca lồng hai cấp
dùng monorepo dạng sparse.

**Đang cân nhắc (2026-10-04): viết lại archctl bằng Python.** Đã đánh giá,
khuyên chưa nên; chờ bạn quyết. Số đo trên máy này: lex cả repo bằng Python
(tokenizer `re` rút gọn) chậm hơn Rust 3,6–7 lần, `ast` cho file `.py` chậm
hơn khoảng 11–14 lần; riêng khởi động `python3.14` kèm vài module chuẩn đã 82 ms, trong khi
`check <file>` của Rust trên netbox hết 39 ms.

**Đang cân nhắc (2026-10-06): mở rộng sang hạ tầng và ML/DL/LLM.** Đã rà, chờ
bạn chọn hướng. Mã thuộc mảng system, network, hạ tầng đã có trong bộ đo
(redis, leveldb, libbpf, nginx, curl, tokio, etcd, netbox, opentelemetry).
Kiến trúc ở tầng service, mạng, triển khai thì chưa: DSL không có khái niệm
đó, và archctl không đọc `.proto`, Dockerfile, YAML k8s, `.tf`, `.ipynb`.
Với ML/LLM, kiến trúc module vẽ và kiểm được: một project mẫu có luật `deny
* -> OpenAI except LLM` và `deny Serve -> Train` bắt đúng 3/3 vi phạm. Ba
thứ chưa thấy:
- cạnh Python sang mã native: `torch.ops._C.rotary_embedding` của vllm ứng
  với `csrc/libtorch_stable/torch_bindings.cpp:400`; `#[pymodule]` của
  pydantic-core;
- kiến trúc mô hình theo layer;
- topology lúc chạy.
Sơ đồ tự dựng của vllm có 24 component và 216 cạnh. Có 3 086 trên 5 624 file
nằm ngoài sơ đồ, gồm cả 313 file C++/CUDA trong `csrc/`. Ứng viên theo thứ
tự đáng làm: cạnh PyO3, pybind11 và `TORCH_LIBRARY` trong `scan/foreign.rs`
(phải tính cả cách đưa `csrc/` vào sơ đồ, vì việc gộp "anh em của gốc" từng
bị loại ở curl); công thức luật ML/LLM kèm `verify` gọi công cụ ngoài; đọc
`.ipynb`. Kiến trúc mô hình và tầng hạ tầng thì trỏ ra công cụ ngoài, đừng
đưa vào lõi.
Các ứng viên đó giờ là việc ở §1. Còn treo: có đưa tầng hạ tầng (service,
triển khai) vào DSL không.

Không có việc nào đang dở, mọi mốc đã commit (`3167cc9` là cuối).

**Kiểm lại yêu cầu ban đầu (2026-10-11): chưa khép hết.** Yêu cầu: phủ rộng
chưa, phủ system, network, hạ tầng chưa, và có vẽ được kiến trúc ML/DL/LLM
không.
- Mã system, network, hạ tầng: có, trong bộ đo 27 repo (thêm kubernetes).
- Kiến trúc tầng service, mạng, triển khai: chưa. DSL không có khái niệm đó,
  archctl không đọc `.proto`, Dockerfile, YAML, `.tf`; chờ bạn quyết.
- Kiến trúc module của ML/LLM: có, giờ gồm cả mã native. Sơ đồ tự dựng của
  vllm có `Csrc` với 8 cạnh vào (`archctl export` ra 24 component, 216 cạnh);
  file ngoài sơ đồ 3 086 → 2 775, trong đó 2 029 là `tests/`, 357 là `rust/`
  (chỉ test nạp, bằng `pytest.importorskip`), còn lại `examples/`,
  `benchmarks/`, `tools/` và vài thư mục nhỏ.
- Kiến trúc mô hình theo layer: lúc kiểm mới là công thức, chưa chạy thật
  (không có torch, chưa có `model_arch.txt`). Đã khép ở việc 4.1, xem dưới.
- Topology lúc chạy (GPU worker, hàng đợi, LangGraph, DAG): chưa, ghi ở mục
  "Chưa làm" của `docs/ml.md`.
Việc còn lại ở mốc "Khép yêu cầu ban đầu" trong §1.

**CI trên nhánh thử xanh, đã đưa lên `main` (2026-10-12).** Nhánh
`thu/ha-tang-dot-1`. Lần 1 (run 38221130465, `2ac0fb8`): 4/5 job, `luật` trượt vì
clippy 1.99 bắt `needless_borrows_for_generic_args` mà clippy 1.98 trên máy dev
không bắt; sửa ở `dc61339`. Lần 2 (run 38222284663, `dc61339`): 5/5 — `luật`,
`rust tối thiểu`, `nền tảng khác`, `repo thật`, `kiến trúc mô hình` (lần đầu
trên Linux). Test chạy trên Linux x86_64 nên nhánh hằng `O_NOFOLLOW` của Linux
đã biên dịch. Bạn bảo "CI xanh thì merge vào main rồi push": fast-forward.

**4.1 và 4.3 xong, chưa commit** (2026-10-11). `verify model_arch` chạy thật:
`examples/ml/requirements-test.txt` ghim torch 2.14.0 (Linux lấy bản `+cpu` từ
index của PyTorch), pytest và 13 gói kéo theo, 174 hash, chỉ bản ra trước
2026-09-27; `examples/ml/tests/model_arch.txt` là `repr(Net())` thật; CI thêm
job `kiến trúc mô hình`. Biên nhận: `verify model_arch` 1/1 trong venv
`target/ml-venv`; đổi `depth` 4 → 5 hay xoá bản duyệt thì thoát 1; sửa một hash
thì pip dừng, thoát 1; khoá Linux cài được bằng `uv pip install --python-platform
x86_64-manylinux_2_28`. Bước pip thật trên runner Linux chưa chạy (cần push).
`.agent/threat-model.md` vào git, thêm TM-7. Review (opus): 0 cao, 0 vừa, 3 nhẹ,
đã sửa cả 3 (`.venv/` vào `.gitignore`, số dòng trong threat model, §0 này).

**4.2: bạn chốt "theo khuyên hết, làm đợt 1" (2026-10-12).** Đợt 1 xong, đã
commit (xem "Đã xong" ở trên). Thiết kế đã chốt: tầng riêng `service` và
`link` (không gian tên riêng, nên `component Api` và `service Api` cùng tồn tại
được); đợt đầu đọc Dockerfile và compose; mã chạy trong service tính từ lệnh
khởi chạy theo đồ thị import, đơn vị là component như mọi luật khác (`runs`
phải khép kín theo cạnh import: component chạy trong service thì mọi thứ nó
import cũng phải chạy trong đó), nên kiểm được từng file trong hook; Helm,
Kustomize, Terraform không render; gRPC đợt hai. Mã mới đặt vào component có
sẵn (bộ đọc thuần vào `scan/`, đối chiếu vào `check.rs`, dựng nháp vào
`discover.rs`), nên không phải đổi `.arch/system.arch`. Trước đó: bạn chọn đưa
tầng service, mạng, triển khai vào DSL. Trang so sánh hai phương án và năm câu cần chốt:
https://claude.ai/artifact/VVd2dz5WrcsyoKJBkvetPm. Khuyên: tầng riêng `service`
và `link`; đợt đầu đọc Dockerfile và compose; mã chạy trong service xác định từ
điểm vào theo đồ thị import; Helm, Kustomize, Terraform không render trong lõi;
gRPC đợt hai. reuse-best: không công cụ nào vừa mô hình service, vừa đọc file
triển khai, vừa đối chiếu với mã mà giữ 0 dependency; mượn mô hình deployment
của Structurizr và `dependsOn` của Backstage, lint sâu gọi ngoài qua `verify`.
**Đã quyết (2026-10-03): giữ repo private, CI là
lời nhắc chứ không chặn** — gói GitHub Free không có branch protection cho repo
private, nên job đỏ không ngăn được commit vào `main`. Muốn đổi thì để repo
public hoặc lên Pro; lý do ở [docs/ci.md](docs/ci.md).

Hai việc từng nằm trong danh sách đã cân
nhắc rồi **loại**, ghi lại để khỏi nghĩ lại:

1. **Component theo `[workspace] members`.** Đã dùng manifest để *không chia
   nhỏ hơn* ranh giới package, và đó là phần có giá trị. Lấy thẳng members
   làm component thì `rusqlite` tụt từ 6 component xuống 2, vì mã của nó nằm
   gọn trong một crate duy nhất.
2. **curl 65% file ngoài sơ đồ.** Gốc `lib/` thắng `src/` (tool curl) rất
   sát. Cách duy nhất nghĩ ra là gộp thêm "anh em của gốc", nhưng điều đó kéo
   `tests/` vào sơ đồ ở mọi repo khác — đánh đổi sai.

### Cố ý không làm bằng phân tích tĩnh

Thứ tự bước trong `flow`, `requires`/`ensures` của `contract`, `invariant
data`/`money`/`access`, `retry`/`timeout` của `boundary`. Tất cả cần trace
runtime, contract test, property test hoặc tiêm lỗi — lý do và bằng chứng từ
bài báo ở [docs/khao-sat.md](docs/khao-sat.md).

Chúng **không bị bỏ ngỏ**: `verify <tên> runs "<lệnh>"` trỏ ra công cụ làm
được việc đó, `archctl verify` chạy thật, và biên nhận hết hạn khi mã hoặc
luật đổi. Xem [docs/verify.md](docs/verify.md).

Cạnh liên ngôn ngữ: `extern "C"` và `C.x` đã ghép được; `dlopen`, con trỏ
hàm, `ctypes` của Python và IPC thì không — xem [docs/check.md](docs/check.md).

## §1 Các mốc

### Cạnh Python ↔ native

- [x] Ghép module mở rộng với import Python: `#[pymodule]` của PyO3,
  `PYBIND11_MODULE`, `NB_MODULE`. Chỉ tính import không trỏ tới file `.py`
  (pydantic-core: `from ._pydantic_core import` hiện dừng ở file `.pyi`).
- [x] Ghép op của PyTorch: `m.def("op(…)")` trong khối `TORCH_LIBRARY*` với
  `torch.ops.<ns>.<op>` (vllm: `csrc/` ↔ `vllm/_custom_ops.py`).
- [x] Bộ khám phá đưa vào sơ đồ thư mục ở đầu kia của cạnh liên ngôn ngữ
  (`csrc/` của vllm, `python/` của pydantic-core), không kéo theo `tests/`.
- [x] Bộ đo: số mới cho vllm và pydantic-core, thêm kiểu phá vào `sabotage.py`.
- [x] Chọn mức oracle cho C++ (bạn chọn raw token của clang) rồi để `oracle.py` đối chiếu cạnh binding
  (raw token của clang, hay tree-sitter chỉ chạy ở CI).
- [x] Tài liệu: mục cạnh liên ngôn ngữ trong `docs/check.md`, README.

### ML/LLM dùng được ngay

- [x] Tài liệu công thức luật cho ML/LLM: `external` cho torch và SDK của LLM,
  `deny Serve -> Train`, `api` cấm `pickle`, `eval`; `verify` gọi công cụ
  ngoài để chụp kiến trúc mô hình rồi so với bản đã duyệt.
- [x] Đọc `.ipynb`: ô code đưa qua bộ quét Python.
- [x] Bộ đo: thêm một repo huấn luyện DL.
- [x] Bộ đo: thêm một repo hạ tầng viết bằng Go (Kubernetes, tải sparse).

### Còn mở sau mốc ML/LLM

- [x] `oracle.py` đối chiếu import trong notebook: ô code qua `json` cộng `ast`,
  bỏ magic như IPython; hiện oracle bỏ qua `.ipynb`.
- [x] Module mở rộng đặt tên bằng `PYBIND11_MODULE(TORCH_EXTENSION_NAME, m)`
  (kiểu `torch.utils.cpp_extension`, gặp ở detectron2, mmcv): đọc tên thật từ
  `CppExtension`/`CUDAExtension("pkg._C", …)` trong `setup.py`. Hiện không ghép.

### Khép yêu cầu ban đầu

- [x] Cài torch bản CPU để chạy thật `verify model_arch`
  (sinh `examples/ml/tests/model_arch.txt`, CI chạy nó ở job `kiến trúc mô hình`).
- [x] Chốt năm câu thiết kế tầng service, mạng, triển khai trong DSL
  (bạn chọn theo khuyên hết; trang phương án: https://claude.ai/artifact/VVd2dz5WrcsyoKJBkvetPm).
- [x] `.agent/threat-model.md` vào git (bạn đồng ý), thêm TM-7.
- [x] @ban Push lên nhánh thử rồi chạy CI bằng `workflow_dispatch`
  (job `kiến trúc mô hình` chưa chạy trên runner Linux lần nào).

### Tầng hạ tầng, đợt 1

- [x] Threat model cho bộ đọc compose, Dockerfile (TM-8 trở đi).
- [x] DSL: `service <Tên> [from compose:<tên>|dockerfile:<đường dẫn>] [runs <Component>, ...]`
  và `link <Service> -> <Service>, ...`; parser, validate, diff, hash.
- [x] Bộ đọc YAML tập con (anchor, alias, `<<`, flow, block scalar), giới hạn độ sâu,
  ngân sách gộp, trần dung lượng.
- [x] Bộ đọc Dockerfile (stage, WORKDIR, COPY, CMD, ENTRYPOINT, `go build`) và
  bộ đọc lệnh khởi chạy (python, `-m`, uvicorn, gunicorn, celery, script `.py`, node, `sh -c`).
- [x] Đọc compose: build, image, command, entrypoint, depends_on, links,
  environment (chỉ giữ host, không in giá trị), env_file, volumes, extends, include.
- [x] Ba phép kiểm trong `check`: mã chạy ngoài `runs`, cạnh mạng thiếu `link`,
  khai báo không còn khớp; JSON, `--strict`, hook.
- [x] `init --from-code` sinh `service`, `runs`, `link`.
- [x] `export` và `serve` vẽ tầng service; `scan` in thông tin triển khai.
- [x] Bộ đo: repo thật có compose, oracle so với PyYAML và dockerfile-parse ghim hash,
  sabotage cho từng phép kiểm; tài liệu.

### Tầng hạ tầng, đợt 2

- [x] Cạnh gRPC: đọc `.proto` (service, rpc) và stub sinh ra, ghép client với server.
  Bạn chốt "theo khuyên hết" (2026-10-12, trang https://claude.ai/artifact/DBcz8fdtBZ8DLbwdHjYmUU):
  cạnh component cần `allow`, đã khai `service` thì cần thêm `link`; ghép theo
  `package.Service`, lùi về tên khi duy nhất; server ngoài repo như gói ngoài;
  Python và Go. reuse-best: không có bộ trích dùng lại được, oracle `.proto` là
  `grpcio-tools`. Threat model TM-13 tới TM-17.
- [x] Bộ đọc `.proto` và bộ dò client/server Python, Go; ghép, `allow`, `link`; hook tìm phía server.
- [x] `init --from-code`, `scan`, `export`/`serve` có cạnh gRPC; tài liệu.
- [x] Bộ đo: etcd cạnh bắt buộc qua `etcdserverpb.KV`; oracle `.proto` bằng `grpcio-tools`; sabotage.
- [x] Manifest Kubernetes (Deployment, Service) vào `service`/`link`; không render Helm.
  Bạn chốt "theo khuyên hết" (2026-10-13, trang https://claude.ai/artifact/Tzgo8YiLFnA9BhWjwtfE29):
  `from k8s:<tên>` (thêm `<thư mục>/` khi trùng); image tự lần qua skaffold và
  compose, không ra thì cho khai thêm `dockerfile:`, vẫn không ra thì "không rõ
  điểm vào"; một service nhiều nguồn (`from compose:vote, k8s:vote`); cạnh từ env
  và ConfigMap qua Service selector, không đọc Secret, Service không chọn workload
  nào trong repo như gói ngoài; Helm không đọc, patch Kustomize không áp. reuse-best:
  tự làm trên bộ đọc YAML sẵn có, oracle trên PyYAML ghim hash. Threat model TM-18 tới TM-22.
- [x] Bộ đọc manifest (workload, Service, ConfigMap, patch Kustomize), `skaffold.yaml`; trần.
- [x] `from k8s:` và nhiều nguồn trong một service; `check`: `link`, `runs`, khai báo lạc hậu, workload chưa khai; hook.
- [x] `init --from-code` gộp compose với k8s, gom bản sao theo nơi dựng; `scan`, `export`/`serve`.
- [x] Bộ đo: Online Boutique, oracle `k8s_truth.py`, sabotage; tài liệu.

### Tầng hạ tầng, đợt 3

Bạn chốt "theo đề xuất hết" (2026-10-14, trang https://claude.ai/artifact/Xy5ZNKRt6p7JdGwcco9zCa):
đẩy `e31a772` qua nhánh thử rồi lên `main`; làm A rồi B; với A, cụm cho phép mà
không có `link` là vi phạm, có `link` mà cụm chặn là cảnh báo (`--strict` tính).

- [x] Đẩy `e31a772` qua nhánh thử `thu/k8s`, CI xanh thì lên `main`, xoá nhánh.
- [x] NetworkPolicy đối chiếu `link`: đọc policy (podSelector, namespaceSelector,
  ingress, egress), cho phép rõ mà thiếu `link` là vi phạm, `link` bị chặn là cảnh
  báo; `init` sinh `link` theo policy; `scan`; bộ đo (Online Boutique), oracle, sabotage; tài liệu.
- [x] gRPC của Node (`@grpc/proto-loader`, `addService`) và Rust (`tonic`); bộ đo
  Online Boutique (currency, payment), vllm; oracle, sabotage; tài liệu.
- [x] Lối vào từ Internet (việc C): `link Internet -> X`; Ingress, Gateway API, Istio,
  Service `LoadBalancer`/`NodePort`/`externalIPs`, `hostNetwork`/`hostPort`, cổng
  compose; bộ đo (Online Boutique, voting app), oracle, sabotage; tài liệu.


### Tầng hạ tầng, đợt 4

- [x] Chốt 5 câu về cạnh mạng từ mã @ban: bạn chốt "làm theo đề xuất, miễn hiệu quả"; trang
  https://claude.ai/artifact/EjLmvkEkkAFPDkyNyTnTwW (khuyên A: tên máy viết thẳng
  trong mã, thiếu `link` là vi phạm)
- [x] Cạnh mạng từ mã: chuỗi tên máy (URL, `máy:cổng`, `host=`) trong mã → `link`;
  bộ đo (voting app), oracle, sabotage; tài liệu.
- [x] Đẩy `c5ae95d`, `781b0e9`, `bc82d75` qua nhánh thử, CI xanh rồi lên `main` @ban

### Tầng hạ tầng, đợt 5: Helm

- [x] Chốt 4 câu về đọc chart Helm @ban: bạn chốt "làm theo đề xuất"; trang https://claude.ai/artifact/EhNC8dx9sjp8jmMgYqoL16 (khuyên tự render tập con template với values.yaml)
- [x] Đọc chart Helm: render tập con template, manifest dựng ra đi qua bộ đọc Kubernetes; bộ đo so với `helm template`; tài liệu.

### Tầng hạ tầng, đợt 6: hàng đợi và pub/sub

- [x] Chốt 4 câu @ban: bạn chốt "làm theo đề xuất" (2026-10-19, trang https://claude.ai/artifact/9QrPEQYLevs4b2LWki6J2K): làm hàng đợi trước, Kustomize sau, Terraform để sau nữa; cạnh qua topic như gRPC (`allow` giữa component, `link` khi đã khai `service`); tên topic không cố định thì "chưa ghép", `--strict` tính, trừ hằng số cùng file và env khai trong compose, manifest; đợt đầu Celery, Kafka, RabbitMQ, Redis pub/sub cho Python, Go, Node. reuse-best: glia (giấy phép không thương mại, tree-sitter), GitNexus (TypeScript), grafel (daemon) đều không nhúng được vào CLI không dependency chạy trong hook; tự làm trên lexer sẵn có, oracle bằng `ast` của Python và go/ast.
- [x] Bộ dò bên gửi, bên nhận (Python, Go, Node) theo tên topic, queue, task; hằng số cùng file; ghép, `allow`, `link`; hook tìm phía bên kia.
- [x] Tên topic lấy từ env khai trong compose, manifest.
- [x] `init --from-code`, `scan`, `export`/`serve` có cạnh qua hàng đợi; tài liệu.
- [x] Bộ đo: repo thật có hàng đợi, oracle, sabotage; CI.
- [ ] Overlay Kustomize (hướng B).
