# archctl

Sơ đồ kiến trúc là luật. AI viết mã. Bộ kiểm chứng đứng giữa.
Mô tả sản phẩm ở [README.md](README.md); file này chỉ ghi trạng thái công việc.

## §0 Đang ở đâu, việc tiếp theo

Cập nhật 2026-10-26.

### Đã xong, có bằng chứng

- **Sửa phát hiện audit trước phát hành** (2026-10-26; bạn chốt "sửa 6 MEDIUM và LOW rẻ
  rồi phát hành"). Audit claude-security cả repo mức medium (báo cáo ngoài git trong
  `CLAUDE-SECURITY-20261025-043313/`): sửa 6 MEDIUM và LOW rẻ — lọc tên nháp `init`
  (F1), trần placeholder và số tên máy (F2), trần tên và ngân sách glob (F3), độ sâu
  `use` Rust (F4), trần CMake mở rộng và đọc có trần (F5), độ sâu giá trị (F6), dòng
  header `serve` 8 KiB (F7), id đề xuất SHA-256 tính lại khi `accept` (F12), danh sách
  cấm đúng đường dẫn và lệnh (F13/F16), không ghi đè file luật là symlink (F14), tên
  gốc Terraform của oracle (F17). Reviewer opus kiểm lại: 7/7 hết, mục mới đã sửa.
  Biên nhận logs-r3 trên mã cuối, ALL-OK: `cargo test`, clippy, `cargo +1.88 check`,
  build release, bộ đo (287 s), oracle `--require …,java` (594 s), sabotage 74 kiểu 0
  trượt, `verify`, `check --strict`. Còn lại: LOW ở việc 17.1.

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


- **Xong trước đó:** ở `.agent/NHAT-KY.md` (mục "Từ §0 của PLAN.md: lịch sử cũ").

### Việc tiếp theo

0. (2026-10-26, mới nhất) Đã phát hành **v0.1.0** (`f64db05`) và **v0.1.1**
   (`187bcaa`), cả hai công khai, v0.1.1 là Latest:
   https://github.com/tdHuy22/archctl/releases/tag/v0.1.1. Mỗi bản: CI main xanh
   5/5, workflow phát hành đạt, 8 file, bản aarch64-apple-darwin khớp sha256.
   Audit claude-security: 18 phát hiện đều đã sửa (MEDIUM và LOW rẻ ở 0.1.0,
   LOW còn lại ở 0.1.1, TM-71..85), reviewer opus kiểm từng bản sửa. Biên nhận
   cuối (logs-r6, mã 0.1.1): test, clippy, MSRV, release, bộ đo, oracle,
   sabotage 0 trượt, `verify`, `check --strict` đạt. Không còn việc mở trong
   kế hoạch; cây làm việc sạch. Lưu ý cho người dùng 0.1.0: hash luật đổi dạng
   (32 hex), lần đầu chạy 0.1.1 báo "kiến trúc đã thay đổi", cần `verify` lại.
   Việc tiếp: chờ bạn đặt mốc mới. Các mục dưới là lịch sử cũ.


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
