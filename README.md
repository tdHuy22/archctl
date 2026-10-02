# archctl

Sơ đồ kiến trúc là **luật**. AI viết mã. Bộ kiểm chứng đứng giữa.

Con người vẽ kiến trúc, luồng dữ liệu, thuật toán và ràng buộc. Agent lập trình
có sẵn (Claude Code, opencode, Codex) tự do viết mã, miễn không vi phạm. `archctl`
đối chiếu mã với sơ đồ và báo mọi chỗ đi chệch.

## Ba thành phần

| Thành phần | Dạng | Vai trò |
| --- | --- | --- |
| `.arch/` | Thư mục file text trong repo người dùng | Nguồn sự thật duy nhất về kiến trúc |
| `archctl` | CLI | Phân tích mã, đối chiếu luật, báo vi phạm |
| `archctl serve` | Web app cục bộ | Vẽ sơ đồ, cho người sửa và duyệt |

Không có backend, không API key, không dữ liệu rời khỏi máy.

## Đặc tả

Bản đặc tả đầy đủ (14 mục):
https://claude.ai/code/artifact/2b26df5d-d6b0-40d0-a398-7e574c7d1387

## Trạng thái

Giai đoạn 1 đang làm. Đã có **parser DSL** và **kiểm luật phụ thuộc** cho
**TypeScript, Python, C/C++, Rust và Go**.

```bash
cargo test                                   # 136 test
cargo run -p archctl -- check --root examples/sensor
```

**Áp lên dự án đã có mã:** `archctl init --from-code` đọc repo và dựng bản nháp
kiến trúc từ import thật — 2000 file mất 190ms. Mọi thứ ở `#draft` nên chưa gì
bị cưỡng chế; nâng dần từng component lên `#accepted`. `archctl baseline` tha
vi phạm có sẵn và chỉ chặn vi phạm mới, như cách áp `strict` của TypeScript lên
dự án đang chạy. Xem [docs/adopt.md](docs/adopt.md).

```
kiến trúc d832ca07 · 15 file quét (5 Rust, 4 C/C++, 4 Python, 2 Go) · 7/8 component trong diện kiểm

✗ VI PHẠM  Native -> LibPcap  (bị cấm, #accepted)
  native/parser.c:6  import 'pcap.h'
  luật: deny * -> LibPcap except Capture
  → gọi Capture thay vì truy cập thẳng LibPcap

✗ VI PHẠM  Capture -> Report  (không được phép, #accepted)
  src/capture/mod.rs:4  import 'crate::report::Summary'
  luật: không có allow nào cho Capture -> Report
  → Capture chỉ được gọi: Decode, LibPcap
```

`--json` cho agent đọc. Mã thoát 1 khi có vi phạm, nên dùng được trong CI ngay.

`api` khai báo nhóm hàm và từ khoá bị cấm — `strcpy`, `eval`, `unsafe`. Loại
luật này **không có vùng mù FFI** như đồ thị import, và vì chạy trên dòng token
nên chú thích và chuỗi không bao giờ gây báo động giả: trong `examples/sensor`,
`grep -c strcpy` ra 4, `archctl` báo đúng 2. Xem [docs/api.md](docs/api.md).

**Giao thức agent–người** cho agent một lối ra khi gặp luật sai, thay vì chỉ
có lách hoặc kẹt: `propose` đề xuất thêm luật, `block` báo bế tắc, rồi người
`accept` hoặc `reject`. Mọi quyết định vào `decisions.md` — và vì agent không
có trí nhớ giữa các phiên, file đó chặn nó đề xuất lại thứ đã bị từ chối. Xem
[docs/protocol.md](docs/protocol.md).

`archctl diff` so luật hiện tại với một bản trước, và **tách thay đổi kiến trúc
khỏi thay đổi chỉ xê dịch chữ trong file** — người duyệt biết ngay chỗ nào cần
đọc kỹ. Luật bị hạ xuống mức lỏng hơn được đánh dấu riêng. Xem
[docs/diff.md](docs/diff.md).

Năm ngôn ngữ dùng chung **một lexer cấu hình được**, biết bỏ qua chú thích,
chuỗi, char literal, raw string, docstring và regex theo từng ngôn ngữ. Vẫn
**không có dependency ngoài std**. Binary 457KB. Quét 2000 file mất ~112ms.

**Giới hạn lớn nhất:** cạnh liên ngôn ngữ — FFI, liên kết thư viện, con trỏ
hàm, IPC — không xuất hiện trong bất kỳ câu lệnh import nào nên không bị bắt.
Ngữ nghĩa và giới hạn đầy đủ: [docs/check.md](docs/check.md).

Chưa có: tsconfig `paths`, `--changed`, kiểm `store owned_by`, `serve`.

## Ghi công

Cách phân loại thay đổi trong `archctl diff` lấy ý tưởng từ
[tt-a1i/archify](https://github.com/tt-a1i/archify) (MIT). Hai dự án bổ sung
cho nhau: archify biến một spec thành sơ đồ đẹp, archctl đối chiếu spec với mã.
Không có dòng code nào được sao chép.

## Cấu trúc repo

```
archctl/
├── docs/         # đặc tả, ghi chú thiết kế
├── templates/    # khung .arch/ mà `archctl init` sinh ra
├── examples/
│   ├── shop/     # dự án TypeScript, hai vi phạm cài sẵn
│   └── sensor/   # Rust + C + Python + Go, mỗi ngôn ngữ một vi phạm
└── crates/
    ├── arch-core/   # parser, mô hình, kiểm tra tham chiếu
    └── archctl/     # CLI
```
