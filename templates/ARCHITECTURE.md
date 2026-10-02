# Ràng buộc kiến trúc

## Trước khi viết mã
1. Đọc `.arch/system.arch` — đây là luật bắt buộc
2. Đọc `.arch/decisions.md` — những gì đã bị từ chối, đừng đề xuất lại

## Trong khi viết mã
- Không tạo module, không thêm phụ thuộc ngoài những gì đã khai báo
- Chỉ chủ sở hữu `store` được truy cập dữ liệu của mình
- Không dùng hàm nằm trong nhóm `api` bị cấm
- Giữ đúng thứ tự bước và hành động bù trừ trong `flow`

## Sau khi sửa mã
Chạy `archctl check` và sửa hết vi phạm trước khi báo hoàn thành.
Nếu đầu ra nói kiến trúc đã thay đổi, đọc lại `.arch/system.arch` ngay.

## TUYỆT ĐỐI KHÔNG
- Không sửa `.arch/system.arch`. Đó là file của người.
- Không chạy `archctl accept` hay `archctl reject`. Đó là lệnh của người.
- Không sửa `.arch/decisions.md` hay `.arch/.state`.
- Không tìm đường lách luật, ví dụ gọi vòng qua module khác.

## Khi không thể làm đúng luật

Có hai lối ra, và phải dùng một trong hai thay vì lách:

Thiếu một `allow`:

    archctl propose 'allow A -> B' --reason "<vì sao cần>" --from <file:dòng>

Có một luật đang chặn và không có cách nào khác:

    archctl block --on 'deny * -> X' --reason "<đã thử gì, vì sao không được>"

Cả hai lệnh đều trả mã thoát 3. Sau khi chạy, **DỪNG phần việc đó lại** và
làm việc khác. Đừng thử tiếp, đừng tìm đường vòng.

`archctl check` sẽ nhắc khi cùng một vi phạm lặp lại ba lần — đó là lúc dùng
hai lệnh trên, không phải lúc thử cách thứ tư.
