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
TypeScript/JavaScript.

```bash
cargo test                                  # 55 test
cargo run -p archctl -- check --root examples/shop
```

```
kiến trúc 18614bc9 · 7 file quét · 4/6 component trong diện kiểm

✗ VI PHẠM  Stock -> Payment  (không được phép, #accepted)
  src/stock/reserve.ts:5  import '../payment/charge'
  luật: không có allow nào cho Stock -> Payment
  → Stock chỉ được gọi: Notify

✗ VI PHẠM  Web -> Stripe  (bị cấm, #accepted)
  src/web/checkout-page.ts:5  import 'stripe'
  luật: deny * -> Stripe except Payment
  → gọi Payment thay vì truy cập thẳng Stripe
```

`--json` cho agent đọc. Mã thoát 1 khi có vi phạm, nên dùng được trong CI ngay.

Viết bằng Rust, **không có dependency ngoài std** — kể cả bộ quét import. Binary
400KB. Đọc file luật mất 3ms; quét 2000 file TypeScript mất 123ms.

Ngữ nghĩa và giới hạn đã biết: [docs/check.md](docs/check.md).

Chưa có: tsconfig `paths`, `--changed`, kiểm `store owned_by`, `propose`, `serve`.

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
