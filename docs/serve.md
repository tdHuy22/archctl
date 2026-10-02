# `archctl serve` — sơ đồ để thao tác và quan sát

```bash
archctl serve            # http://127.0.0.1:4000
archctl serve --port 8080
```

Đặc tả gốc nói sơ đồ là để con người **thao tác và quan sát**. `export --mermaid`
chỉ làm được vế sau. Đây là vế đầu.

## Làm được gì

| Thao tác | Ghi vào đâu |
| --- | --- |
| Kéo thả hộp | `.arch/layout.json` |
| Bấm `#locked` / `#accepted` / `#draft` | đúng một dòng trong `.arch/system.arch` |
| Duyệt hoặc từ chối đề xuất | `.arch/system.arch` và `.arch/decisions.md` |
| Sắp xếp lại | xoá `.arch/layout.json` |

Và quan sát: sơ đồ **tự cập nhật** khi mã hoặc luật đổi. Agent sửa file, sơ đồ
đổi theo trong vòng nửa giây — không cần tải lại trang. Đây là "chiều 3" trong
đặc tả: nhìn sơ đồ là biết agent đang làm tới đâu và chỗ nào đang hỏng.

## Đọc sơ đồ

| Hình | Nghĩa |
| --- | --- |
| Hộp bo góc | component |
| Hình trụ | kho dữ liệu, kèm tên bảng |
| Hình bình hành | nhóm `api` bị theo dõi |
| Viền nét đứt | `#draft` hoặc `#proposed` — chưa có hiệu lực |
| Viền đậm, có chấm tròn | `#locked` |
| Viền và chữ đỏ | đang có vi phạm xuất phát từ đây |
| Cạnh đỏ đậm | vi phạm thật trong mã, **không có** trong luật |
| Cạnh nét đứt vàng | `deny` |

Nhóm `api` luôn nằm ở cột cuối: không gì phụ thuộc vào chúng, còn mọi nơi đều
có thể gọi vào.

## Vì sao không dùng React Flow

Giao diện là **một file HTML tự chứa**, tự vẽ SVG, không framework, không gói
ngoài, không gọi mạng. Hệ quả:

- Mở được khi không có Internet, và trong mạng nội bộ kín
- Không có bước build, không `node_modules`, không bundler
- Người dùng cuối chỉ cần đúng một binary
- Không có bề mặt tấn công nào ngoài máy: chỉ gắn vào `127.0.0.1`

Layout tính ở Rust (phân tầng, xác định) rồi gửi kèm toạ độ, nên giao diện
không phải tự tính và kết quả không đổi giữa các lần chạy.

## Sơ đồ không được nhảy

Node đã có toạ độ trong `layout.json` thì **giữ nguyên vị trí**, kể cả khi agent
thêm component mới. Khung nhìn chỉ tự canh **một lần** lúc mở, không canh lại ở
mỗi lần cập nhật — màn hình tự nhảy khi agent vừa sửa một chỗ thì rất khó theo
dõi.

Muốn xếp lại từ đầu thì bấm **Sắp xếp lại**.

## Giới hạn

- **Một người dùng.** Không có khoá ghi; hai tab cùng sửa thì tab sau thắng.
- **Không sửa được cấu trúc.** Thêm component hay vẽ quan hệ mới vẫn phải sửa
  `.arch/system.arch`. Giao diện chỉ đổi *trạng thái*, vị trí, và quyết định
  trên đề xuất.
- **Dò thay đổi bằng cách quét lại**, chu kỳ 400ms. Trên repo rất lớn thì tốn
  hơn, đổi lại không cần thêm dependency nào.
- **File luật hỏng thì sơ đồ dừng** và hiện lỗi cú pháp kèm số dòng, thay vì vẽ
  một sơ đồ sai.
