# archctl

Sơ đồ kiến trúc là luật. AI viết mã. Bộ kiểm chứng đứng giữa.
Mô tả sản phẩm ở [README.md](README.md); file này chỉ ghi trạng thái công việc.

## §0 Đang ở đâu, việc tiếp theo

Cập nhật 2026-11-17.

### Việc mở

- **Ba việc bạn giao (18/11/2026): code xong, chờ kiểm lần cuối.** Nhánh sơ đồ `claude/so-do-ro-rang-f31718` đã gộp nhanh vào `main` (hai bố cục: archctl mặc định, ELK 0.12.1 nhúng sẵn chọn ở ô Bố cục; chi tiết ở mục của phiên đó bên dưới). Trang hướng dẫn chuyển sang `docs/huong-dan.html` (script kiểm trang đạt, 10/10 ví dụ cú pháp `parse` sạch). Đã nâng bản 0.1.2 (`4bcee40`), chưa push, chưa phát hành; README vẫn chỉ tới file của v0.1.1, sửa khi phát hành.
  Trên `main` sau gộp: test, clippy, `+1.88 check`, release đạt; `dogfood`, `verify`, `check --strict` trượt chỉ vì worktree `.claude/worktrees/so-do-ro-rang-f31718` bị quét thêm 179 file (trong worktree: 96 file, 0 ngoài sơ đồ). Gỡ worktree (sạch, đã gộp) rồi chạy lại ba lệnh đó.
- **Đã push** (18/11/2026): `main` = `origin/main` (`ba88b31`), chưa phát hành bản mới; số bản vẫn 0.1.1. `archctl` trong `PATH` đã cài lại từ `main`.
- **Trang hướng dẫn** `.agent/trang/archctl-huong-dan.html` (chưa theo dõi bằng git) đã lỗi thời: nói "serve chưa vẽ được hộp mới". Cần sửa bước 10 và bảng thao tác, hay bỏ.
- Thử lại bằng Aside trên bản sao `shop` với bản cài từ `main` (18/11/2026, bạn chạy `serve`, agent bấm): thêm component thiếu lý do bị chặn, có lý do thì ghi; kéo mũi tên `Web → Review` ghi `allow`; xoá `Review` bị chặn vì còn chỗ nhắc, **Xoá kèm** đưa `system.arch` về y bản gốc; ba mục `SỬA` đủ; `parse`, `check` sạch (3 vi phạm sẵn có của ví dụ).
- Bạn đã thử trong trình duyệt (17/11/2026): khoá `Scan`, gỡ một đích của `deny` (bạn bấm); **+ Thêm**, **Nối mũi tên**, gỡ mũi tên, ô chữ (`boundary`: tham chiếu sai bị chặn, sửa đúng thì ghi), đổi tên, xoá kèm (agent bấm trên bản sao `shop`): đều ghi đúng `system.arch` và mục `SỬA`; `parse` và `check` trên bản sao sạch.
- **Lưu ý khi dùng worktree:** worktree trong `.claude/worktrees/` nằm trong repo nên test `dogfood` (`khong_file_nao_cua_archctl_nam_ngoai_so_do`) trên `main` trượt vì quét cả 357 file của chúng; gỡ worktree thì đạt.
- Test `grpc_java_addservice_doi_khang_khong_treo` (arch-core, đo dưới 5 giây) trượt một lần trong `archctl verify` khi máy vừa build release, chạy lại thì đạt: test đo giờ dễ chập chờn khi máy tải nặng.
- Hook agent-stack đọc nhầm `cargo test -p archctl --test serve` thành `archctl serve` (`pretool_guard.py` `check_arch_argv`); dùng `cargo test --workspace` thay.

### Đã xong, có bằng chứng

- **Trả "không" ở hộp `#locked` thì báo đã huỷ** (18/11/2026, bạn bảo "sửa ở đây"; tiêu chí ghi trước khi code, đạt 4/4). Tái hiện trong Aside trên bản 0.1.1-main: trả "không" thì form im lặng. Sửa `ui.html` (`send`): hiện "Đã huỷ: chưa ghi gì vào system.arch…" kèm dòng khoá, không gửi gì. Test `ui_html.rs` `tra_khong_o_hop_xac_nhan_thi_bao_da_huy` (`fails-before` có biên nhận: trượt trên code cũ, đạt sau sửa; lần đầu chạy với `--link target` cho kết quả sai vì `target` chung giữ bản build cũ của `ui.html`, đã xoá biên nhận đó). Thử lại trong Aside bằng bản release mới (bạn chạy `serve`, agent bấm, `window.confirm` bị thay để trả lời): "không" hiện thông báo, file không đổi; "có" đổi tên `Frontend`→`WebApp`, `parse` sạch. Biên nhận trên mã cuối: `cargo test --workspace`, clippy, release, `verify` 3/3, `check --strict` 0, `checks.py` 0 lỗi chặn. Review: mức thấp, bỏ reviewer vì hạn mức tuần 90%, tự rà diff.

- **Khoá lạ trong thân `contract`/`boundary` là lỗi** (17/11/2026, bạn bảo "sửa ở đây", bạn xác nhận tiêu chí; đạt 3/3; commit `74cbd2e`, đã gộp vào `main` cùng việc sửa sơ đồ; trên `main` sau gộp: `cargo test --workspace`, clippy, `verify` 3/3, `check --strict` 0 đều có biên nhận). Tái hiện trong trình duyệt: dòng `chaosmonkey bat len moi dem` thêm vào `boundary Checkout` qua ô chữ được ghi, `archctl parse` (cả 0.1.1) im lặng. Sửa `parse_fields` (`parser.rs`): danh sách khoá đóng cho từng khối, khoá lạ là lỗi có số dòng và gợi ý khoá gần nhất (`requiers` → `requires`). Test `parse.rs` (3 test, 2 test trượt trên code cũ ở assert số lỗi) và `edit.rs` `o_chu_tu_choi_khoa_la_trong_than_khoi`; `docs/declarations.md` có bảng khoá. Biên nhận trên mã cuối: `cargo test --workspace`, clippy, `+1.88 check`, `archctl verify` 3/3, `check --strict` 0, `checks.py` 0 lỗi chặn; mọi file `.arch` theo dõi trong repo đọc sạch. Review: mức thấp, bỏ reviewer vì hạn mức tuần 88%, tự rà diff.

- **Nút trạng thái giữ nguyên `\r\n`, BOM, dòng cuối không xuống dòng** (17/11/2026, bạn bảo "sửa ở đây"). `set_status` trước viết lại cả file bằng `\n`; giờ chỉ thay đúng đoạn byte của dòng đó. Tái hiện bằng test `khoa_mot_luat_giu_nguyen_crlf_va_bom` (`fails-before` có biên nhận: trượt trên code cũ, đạt sau sửa). Biên nhận trên mã cuối: `cargo test --workspace` (serve 39/39), clippy, `+1.88 check`, release, `verify` 3/3, `check --strict` 0, `checks.py` 0.

- **Lý do cho nút trạng thái + test tự động TM-92** (17/11/2026, commit tại chỗ, chưa push). Bạn chọn "làm cả 1 và 2"; tiêu chí ghi trước khi code, đạt 4/4: (a) `/api/status` thiếu lý do → 400, không ghi; có lý do → mục `SỬA` (lý do gộp một dòng, dòng `-`/`+`), cùng hàm `record` với `/api/edit`; đổi sang đúng trạng thái hiện có thì không ghi gì; (b) `ui.html` hỏi lý do bằng `prompt`, Huỷ thì thôi; (c) `tests/ui_html.rs` quét `<script>` của `ui.html` (73 chỗ chèn trong HTML), 14 lỗi gài đều bị bắt; bọc thêm `esc()` cho 3 chỗ tra bảng hằng; (d) biên nhận trên mã cuối: `cargo test --workspace` (serve 38/38, ui_html 2/2), clippy, `+1.88 check`, release, `verify` 3/3, `check --strict` 0, `checks.py` 0. Docs `serve.md`, TM-92/95. Review: mức thấp, hạn mức tuần 83% nên done-gate bỏ reviewer, tự rà diff.

- **Sửa sơ đồ đầy đủ trong `archctl serve`** (17/11/2026, chưa push). Tiêu chí ghi từ đầu việc (nguồn: phiên gọi giao; bạn chốt (a)–(d) qua câu hỏi). Đạt 3/4, tiêu chí 1 đạt một phần: (1) trọn vòng thêm component, nối mũi tên, đổi đường dẫn, xoá luật chạy qua API thật (`tests/serve.rs` `ve_so_do_tron_vong_qua_giao_dien`, `check` phản ánh ngay); trong trình duyệt mới kiểm form và chặn thiếu lý do; (2) có test cho từng ca từ chối; (3) lý do bắt buộc, mục `SỬA` trong `decisions.md`, `#locked` hỏi xác nhận, xoá kèm liệt kê chỗ nhắc; (4) biên nhận trên mã cuối: `cargo test --workspace` (serve 35/35), clippy, `+1.88 check`, `verify` 3/3, `check --strict` 0, `checks.py` 0. Mã: `crates/archctl/src/edit.rs` (mới), `serve.rs` (`POST /api/edit`, `decls`), `ui.html` (**+ Thêm**, **Nối mũi tên**, bấm mũi tên để gỡ, tab **Khai báo**), `protocol.rs` (`Verdict::Edited`). Review sonnet · high tìm 5 lỗi (4 vừa, 1 thấp), đã sửa cả 5 kèm 4 test. TM-91..95. Docs: `serve.md`, `protocol.md`, README.

- **Sửa sáu chỗ GĐ3 của agent-stack** (14–15/11/2026; kế hoạch phiên bạn duyệt; đạt 4/4 tiêu chí). (1) `archctl check --hook`: đọc payload PostToolUse trên stdin, đưa vi phạm vào `hookSpecificOutput.additionalContext`, luôn mã 0; Haiku qua `claude -p`: bản 0.1.1 "NONE", bản mới trích `✗ VI PHẠM  Web -> Db` (`.tmp/e2e/`). (2) `init` sinh matcher `Edit|Write|MultiEdit|Bash`, `"timeout": 30`; sau Bash chỉ kiểm file đổi dấu (`seen` trong `.state`), không đếm khống. (3) `sys.path.insert/append` tính tĩnh từ `__file__`; trên bản chép agent-stack thấy 8/9 chỗ `ARCH_BASELINE` (bản cũ 0; chỗ thứ 9 là hook→hook). (4) kiểm một file giữ bộ đếm file khác; hoàn tác bằng git thì về 0. (5) JSON thêm `baselined`, `attempts`, `fresh`, `baselineStale`, `pending`, `previousHash`. (6) `verify --timeout` mặc định 1800 giây, giết cả cây con. Test mới trượt trên code cũ (`fails-before` có biên nhận; `hook.rs`, `verify.rs` khóa mức vừa). Review sonnet·high 7 lỗi, sửa cả 7; review lại phần sửa 1 lỗi (symlink tới thư mục làm hook sau Bash im lặng), đã sửa kèm test. Biên nhận trên mã cuối: `cargo test`, clippy, `+1.88 check`, release, bench 36 repo 0 bất nhất 1344/1344 luồng, oracle khớp hết, sabotage 96/96 bắt, `verify` 3/3, `check --strict` 0, `checks.py` 0 lỗi chặn. TM-86..90. Định dạng chữ của `check` và mã thoát không đổi; agent-stack không cần sửa.

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
   (15/11/2026) Phiên chính của agent-stack cập nhật commit ghim của archctl trong `install.sh`/`vetted.lock`, duyệt lại rồi mới cài; phiên này không cài.


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

### Mốc kiểm tra

Chép từ `.agent/NHAT-KY.md` (đợt 7 Terraform; bản gốc vẫn ở đó). Chạy lần lượt, mỗi lệnh qua `receipt.py run --`, dừng ở lệnh trượt:
`cargo test --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo +1.88 check --workspace --tests`; `cargo build --release`; `python3 bench/bench.py --rounds 1`; `python3 bench/oracle.py --require clang,node,go,syn,ipython,deploy,proto,helm,kustomize,terraform,java`; `python3 bench/sabotage.py --archctl target/release/archctl --rust-repo ripgrep --notebook-repo matplotlib --deploy-repo example-voting-app --k8s-repo microservices-demo --proto-repo vllm --tf-repo terraform-docs-samples`; `target/release/archctl verify`; `target/release/archctl check --strict`.
MỐC KIỂM TRA: `python3 bench/oracle.py --require clang,node,go,syn,ipython,deploy,proto,helm,kustomize,terraform,java` khớp hết; sabotage 0 trượt (69 kiểu ở biên nhận 23/10).

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
