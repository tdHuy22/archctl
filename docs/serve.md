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

## Tab "Lạc hậu" gom ba thứ

Ba chuyện khác nhau, cùng một câu hỏi: **khai báo này có còn đúng không?**

| | |
| --- | --- |
| lạc hậu | tên trong khai báo không có trong mã |
| không kết luận | tên có, nhưng phổ biến tới mức gặp nó chẳng nói lên gì |
| chưa chứng minh | cách kiểm chưa chạy, hỏng, hoặc chạy trước khi mã đổi |

Cái thứ ba đến từ `archctl verify` — xem [verify.md](verify.md).

## Sơ đồ ở quy mô thật

Fixture chỉ có vài component nên mọi bố cục đều trông ổn. Chế độ hỏng chỉ lộ
ra khi đồ thị vừa lớn vừa dày — `hugo` có 24 component và 155 quan hệ, 58
trong số đó khép thành vòng:

| | bề ngang |
| --- | --- |
| ban đầu | **63540** điểm ảnh |
| sau khi bỏ cạnh quay ngược khỏi việc xếp lớp | 4140 |
| sau khi đổi cách chọn cạnh quay ngược | **3084** |

Phân lớp bằng nới lỏng lặp cần một đồ thị không còn chu trình. Không bỏ cạnh
quay ngược ra thì mỗi vòng lặp đẩy cả vòng tròn lên thêm một bậc, và kết quả
phụ thuộc vào việc vòng lặp dừng ở đâu — tức là rác.

Cạnh nào bị coi là quay ngược thì chọn bằng cách xếp component thành một hàng
theo **bậc ra trừ bậc vào**: gọi nhiều mà bị gọi ít thì đứng trước. Thứ tự ấy
đúng với trực giác phân tầng, và trên mười repo thật nó cho sơ đồ gọn nhất
trong ba cách đã thử:

| cách chọn | cạnh ngược | sơ đồ dẹt nhất |
| --- | --- | --- |
| duyệt sâu | 112 | 26.8:1 |
| bậc ra trừ bậc vào | 84 | **12.6:1** |
| Eades–Lin–Smyth | 74 | 16.4:1 |

Cạnh bị bỏ chỉ bỏ khỏi việc **xếp lớp**; nó vẫn được vẽ như mọi cạnh khác.

Đồ thị hugo nằm trong `tests/fixtures/hugo.arch`: đồ thị sinh ngẫu nhiên không
tái tạo được chế độ hỏng này, chỉ hình dạng thật mới làm lộ ra.

### Thấy toàn cảnh quan trọng hơn đọc được chữ

Sàn thu nhỏ cũ là 0.6, nên một sơ đồ rộng gấp sáu lần khung nhìn mở ra chỉ
thấy một góc — và không có gì cho biết còn gì ở ngoài. Sàn giờ là 0.2, và chữ
tự ẩn khi nhỏ quá: đường dẫn dưới 0.45, tên dưới 0.25. Ở mức đó hình dạng tổng
thể là thứ duy nhất còn đọc được, nên để nó bị chữ che là mất nốt.

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

## Luồng theo dõi không được ngốn CPU khi ngồi yên

Dò ở nhịp 400ms suốt thì tốn thật. Đo trên một cây 5078 file, không ai đụng
vào gì:

| | trước | sau |
| --- | --- | --- |
| không ai mở sơ đồ | ~19% một lõi | **0** |
| tab đang mở, không có gì đổi | ~19% | **5%** |
| hugo, 943 file, tab đang mở | ~9% | **2%** |

Hai việc:

- **Không ai mở sơ đồ thì không dò.** Không có client nào thì không có gì để
  báo. Đây là trường hợp hay gặp nhất — người dùng đóng tab mà quên tắt máy
  chủ.
- **Giãn nhịp dần khi im lặng**, từ 400ms lên tối đa 2s, rồi về lại 400ms ngay
  khi thấy thay đổi đầu tiên.

Giãn nhịp không làm mất tính "nhìn là biết agent đang làm tới đâu": đo từ lúc
sửa file tới lúc sơ đồ được báo, sau tám giây im lặng là 548ms, những lần ngay
sau đó là 345ms.

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
- **Dò thay đổi bằng cách quét lại** thay vì gọi API theo dõi của hệ điều
  hành, để không thêm dependency nào.
- **File luật hỏng thì sơ đồ dừng** và hiện lỗi cú pháp kèm số dòng, thay vì vẽ
  một sơ đồ sai.
