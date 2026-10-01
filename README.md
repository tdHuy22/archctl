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

Giai đoạn 1 đang làm. Đã có: parser DSL đầy đủ và bộ kiểm tra tham chiếu.

```bash
cargo test                                    # 22 test
cargo run -p archctl -- parse examples/shop/system.arch
```

Viết bằng Rust, **không có dependency ngoài std**. Binary 360KB, chạy 3.2ms
mỗi lần — để `archctl check` gọi được trong hook sau mỗi lần agent sửa file
mà không làm agent bỏ qua. Node khởi động rỗng mất 28.7ms.

Chưa có: kiểm phụ thuộc trên mã thật, `propose`, `serve`.

## Cấu trúc repo

```
archctl/
├── docs/         # đặc tả, ghi chú thiết kế
├── templates/    # khung .arch/ mà `archctl init` sinh ra
├── examples/     # ví dụ DSL để thử parser
└── crates/
    ├── arch-core/   # parser, mô hình, kiểm tra tham chiếu
    └── archctl/     # CLI
```
