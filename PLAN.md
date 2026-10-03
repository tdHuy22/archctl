# archctl

Sơ đồ kiến trúc là luật. AI viết mã. Bộ kiểm chứng đứng giữa.
Mô tả sản phẩm ở [README.md](README.md); file này chỉ ghi trạng thái công việc.

## §0 Đang ở đâu, việc tiếp theo

Cập nhật 2026-10-03.

### Đã xong, có bằng chứng

- **343 test, clippy sạch với `-D warnings`, 0 dependency ngoài std.** Biên
  nhận ghi bằng `receipt.py` ngày 2026-10-03.
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
  `python3 bench/sabotage.py` phá archctl sáu kiểu, mỗi kiểu bị đúng phép kiểm
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

1. **Rust: item ở gốc crate không sinh cạnh.** `use crate::{Connection,
   Result}` không ra cạnh nào và không được báo, nên một `deny` sẽ bỏ sót nó
   trong im lặng — đã kiểm trên `rusqlite`. Sửa đúng cần biết gốc crate của
   từng file, và sẽ đổi luật của chính archctl cùng số liệu các repo Rust. Chi
   tiết ở [docs/check.md](docs/check.md), mục "Giới hạn khác đã biết".
2. **CI chỉ báo, không chặn.** Repo private trên GitHub Free không có branch
   protection hay ruleset (API trả 403 "Upgrade to GitHub Pro or make this
   repository public"), nên job đỏ không ngăn được commit vào `main`. Cần người
   quyết: để public, lên Pro, hoặc chấp nhận CI là lời nhắc. Xem
   [docs/ci.md](docs/ci.md).

Ngoài ra không có việc nào đang dở. Hai việc từng nằm trong danh sách đã cân
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
