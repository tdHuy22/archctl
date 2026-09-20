# Ràng buộc kiến trúc

## Trước khi viết mã
1. Đọc `.arch/system.arch` — đây là luật bắt buộc
2. Đọc `.arch/decisions.md` — những gì đã bị từ chối, đừng đề xuất lại

## Trong khi viết mã
- Không tạo module, không thêm phụ thuộc ngoài những gì đã khai báo
- Chỉ chủ sở hữu `store` được truy cập dữ liệu của mình
- Giữ đúng thứ tự bước và hành động bù trừ trong `flow`

## Sau khi sửa mã
Chạy `archctl check` và sửa hết vi phạm trước khi báo hoàn thành.

## TUYỆT ĐỐI KHÔNG
- Không sửa `.arch/system.arch`. Đó là file của người.
- Không sửa `.arch/layout.json` hay `.arch/.state.json`.
- Không tìm đường lách luật, ví dụ gọi vòng qua module khác.

## Khi không thể làm đúng luật
`archctl propose '<luật mới>' --reason "<lý do>"` rồi DỪNG việc đó.
Bị chặn 3 lần cùng một vi phạm: `archctl block --on '<luật>' --reason "..."`.
Không tự ý nới luật, không bỏ qua, không làm tắt.
