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

Giai đoạn 0 — mới dựng khung. Xem mục 13 của đặc tả để biết lộ trình.

## Cấu trúc repo

```
archctl/
├── docs/         # đặc tả, ghi chú thiết kế
├── templates/    # khung .arch/ mà `archctl init` sinh ra
├── examples/     # ví dụ DSL để thử parser
└── packages/     # mã nguồn (chưa có)
```
