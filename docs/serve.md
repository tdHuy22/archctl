# `archctl serve` — sơ đồ để thao tác và quan sát

```bash
archctl serve            # http://127.0.0.1:4000
archctl serve --port 8080
```

Đặc tả gốc nói sơ đồ là để con người **thao tác và quan sát**. `archctl export`
chỉ làm được vế sau. Đây là vế đầu.

## Làm được gì

| Thao tác | Ghi vào đâu |
| --- | --- |
| Kéo thả hộp | `.arch/layout.json` |
| Bấm `#locked` / `#accepted` / `#draft` | đúng một dòng trong `.arch/system.arch` |
| Tab **Lạc hậu** | chỉ đọc — tên khai báo nhưng không có trong mã |
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
| Khung nét đứt, hoặc nhãn `⌜ Tên` | thành viên của một `boundary` |

Khung boundary **chỉ được vẽ khi nó bao đúng thành viên**. Khi một component
ngoài lọt vào trong khung — thường xảy ra khi boundary trải qua nhiều cột —
khung bị bỏ và thay bằng nhãn trên từng hộp. Một khung bao nhầm làm người xem
đọc sai kiến trúc, mà đó đúng là thứ sơ đồ sinh ra để tránh.

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

## Máy chủ chịu được yêu cầu dị dạng

Chỉ gắn vào `127.0.0.1`, nhưng "chỉ trong máy" không có nghĩa là "chỉ có giao
diện này gửi tới".

| | |
| --- | --- |
| `Content-Length` quá lớn | từ chối. `vec![0u8; usize::MAX]` cấp phát hỏng, mà cấp phát hỏng thì Rust kết thúc **cả tiến trình**: một yêu cầu dị dạng từng đủ giết máy chủ |
| Tiêu đề vô tận | tối đa 64 dòng |
| Kết nối mở rồi im lặng | hết hạn sau 15 giây, không giữ luồng mãi |
| `POST` kèm `Origin` lạ | 403 |

Chỗ cuối đáng nói nhất. Trang web bất kỳ mà người dùng đang mở vẫn `fetch` tới
`127.0.0.1` được: `POST` dạng đơn giản không cần hỏi trước. Kẻ gửi không đọc
được trả lời — nhưng **thao tác ghi vẫn xảy ra**. Nghĩa là một trang lạ có thể
duyệt một đề xuất đang chờ, tức là đúng thứ mà cả hệ thống này dựng lên để bắt
buộc phải có người xem.

Trình duyệt luôn gửi `Origin` cho yêu cầu khác nguồn, nên chỉ cần đối chiếu nó.
Client không phải trình duyệt (`curl`, test) không gửi gì và vẫn dùng như cũ.

## Ghi file

Máy chủ tạo một luồng cho mỗi kết nối, nên hai tab cùng mở có thể cùng
đọc-sửa-ghi `.arch/system.arch`. Hai chỗ chống lại điều đó:

- **Mọi thao tác ghi đi qua một cửa.** Chúng hiếm, nên xếp hàng không tốn gì,
  còn mất một lần sửa thì tốn. Không có khoá này thì bản ghi sau dựa trên bản
  đọc trước và nuốt mất thay đổi kia.
- **Ghi nguyên tử**: ra file tạm cùng thư mục rồi đổi tên. `fs::write` cắt file
  về rỗng trước, nên ai đọc đúng khoảng giữa sẽ thấy file luật rỗng — mà "ai
  đó" ở đây là agent đang chạy `archctl check`, hoặc chính luồng theo dõi.

Mọi lệnh `archctl` ghi file đều theo cách này, không riêng máy chủ.

## Giới hạn

- **Một người dùng.** Hai người cùng sửa thì không mất file, nhưng người sau
  vẫn ghi đè quyết định của người trước — không có hợp nhất.
- **Không sửa được cấu trúc.** Thêm component hay vẽ quan hệ mới vẫn phải sửa
  `.arch/system.arch`. Giao diện chỉ đổi *trạng thái*, vị trí, và quyết định
  trên đề xuất.
- **Dò thay đổi bằng cách quét lại**, chu kỳ 400ms. Trên repo rất lớn thì tốn
  hơn, đổi lại không cần thêm dependency nào.
- **File luật hỏng thì sơ đồ dừng** và hiện lỗi cú pháp kèm số dòng, thay vì vẽ
  một sơ đồ sai.
