# archctl

Sơ đồ kiến trúc là luật. AI viết mã. Bộ kiểm chứng đứng giữa.
Mô tả sản phẩm ở [README.md](README.md); file này chỉ ghi trạng thái công việc.

## §0 Đang ở đâu, việc tiếp theo

Cập nhật 2026-10-04.

### Đã xong, có bằng chứng

- **Go có thước đo chuẩn** (2026-10-04, chưa push): `oracle.py` dựng một trình
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

1. **Push rồi xem job `repo thật`**: lần đầu CI chạy oracle với
   `--require go`. Nếu image không có `go` trong PATH thì mã thoát là 2, sửa ở CI.
2. Rust chưa có thước đo chuẩn (không có crate `syn` khi offline).

Các repo `googleapis/python-*` đơn lẻ đã archive và rỗng, nên ca lồng hai cấp
dùng monorepo dạng sparse.

**Đang cân nhắc (2026-10-04): viết lại archctl bằng Python.** Đã đánh giá,
khuyên chưa nên; chờ bạn quyết. Số đo trên máy này: lex cả repo bằng Python
(tokenizer `re` rút gọn) chậm hơn Rust 3,6–7 lần, `ast` cho file `.py` chậm
hơn khoảng 11–14 lần; riêng khởi động `python3.14` kèm vài module chuẩn đã 82 ms, trong khi
`check <file>` của Rust trên netbox hết 39 ms.

Không có việc nào đang dở. **Đã quyết (2026-10-03): giữ repo private, CI là
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
