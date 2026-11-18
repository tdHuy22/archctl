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
| **+ Thêm** component, store, luật, service, flow… | `.arch/system.arch`, mục `SỬA` trong `.arch/decisions.md` |
| **Nối mũi tên**: kéo từ hộp này sang hộp kia | thêm `allow`/`deny` (hay `link` giữa hai service), như trên |
| Bấm một mũi tên → Gỡ, Sửa luật, Xoá cả dòng | như trên |
| Bấm một hộp, hay tab **Khai báo** → Sửa, Đổi tên, Xoá | như trên |
| Kéo thả hộp | `.arch/layout.json` |
| Bấm `#locked` / `#accepted` / `#draft` | đúng một dòng trong `.arch/system.arch`, mục `SỬA` trong `.arch/decisions.md` |
| Tab **Lạc hậu** | chỉ đọc — tên khai báo nhưng không có trong mã |
| Duyệt hoặc từ chối đề xuất | `.arch/system.arch` và `.arch/decisions.md` |
| Sắp xếp lại | xoá `.arch/layout.json` |
| Chọn **Bố cục: archctl** / **Bố cục: ELK** | chỉ trình duyệt này nhớ (không ghi gì vào repo) |

Và quan sát: sơ đồ **tự cập nhật** khi mã hoặc luật đổi. Agent sửa file, sơ đồ
đổi theo trong vòng nửa giây — không cần tải lại trang. Đây là "chiều 3" trong
đặc tả: nhìn sơ đồ là biết agent đang làm tới đâu và chỗ nào đang hỏng.

## Vẽ sơ đồ

Ý gốc của công cụ: **người vẽ kiến trúc, agent viết mã theo**. Nên mọi thứ
trong `system.arch` thêm, sửa, xoá được ngay trên sơ đồ, không phải mở file.

- **Khai báo một dòng** (`component`, `external`, `store`, `allow`, `deny`,
  `api`, `service`, `link`, `view`) có form; dòng sinh ra hiện ngay bên dưới.
  Bấm **Sửa dạng chữ** để gõ thẳng.
- **Khai báo nhiều dòng hay có biểu thức** (`flow`, `contract`, `boundary`,
  `invariant`, `verify`) sửa bằng ô chữ. Gõ sai thì máy chủ nói dòng nào sai.
- **Đổi tên** đổi cả mọi chỗ nhắc tới, đúng không gian tên: đổi component
  `Web` thì `allow`, `store … owned_by`, `service … runs`, `view`, `boundary`,
  `contract Web.x`, bước `Web` của `flow`, `invariant [Web]` đổi theo, còn
  `service Web` (không gian tên khác) giữ nguyên. Hộp giữ chỗ cũ trên sơ đồ.
- **Xoá một tên còn được nhắc** thì bị từ chối kèm danh sách chỗ nhắc. Bấm
  **Xoá kèm** thì tên được gỡ khỏi danh sách đích, luật chỉ còn trỏ tới nó bị
  xoá. Chỗ không gỡ tự động được — `store` thuộc component đó, bước của
  `flow`, `contract`, phạm vi `invariant` — vẫn bị từ chối: sửa chúng trước.
- **Bấm vào một vi phạm** (cạnh đỏ) → **Cho phép**: thêm `allow` cho đúng cặp
  đó, khi đó là quyết định kiến trúc chứ không phải lỗi của mã.

Mỗi thao tác đều phải ghi **lý do**, như `archctl accept --reason`, kể cả bấm
nút trạng thái (`#draft`, `#accepted`, `#locked`) — khoá hay mở khoá cũng là
quyết định kiến trúc: máy chủ ghi
một mục `## <ngày> · SỬA · e-…` vào `decisions.md`, gồm dòng mô tả, các dòng cũ
(`-`) và mới (`+`), người sửa, lý do. Agent đọc file đó để biết vì sao luật đổi.

Luật `#locked` sửa được, nhưng giao diện hỏi lại một lần: khoá là để không ai lỡ
tay đổi.

### Không làm hỏng file luật

| | |
| --- | --- |
| Bản sửa làm file không đọc được, trùng tên, hay để `allow` trỏ vào tên không còn | từ chối, nói dòng nào; không ghi gì |
| File đã đổi từ lúc tải sơ đồ (agent hay người sửa tay) | từ chối (409): mỗi thao tác gửi kèm số dòng **và** nguyên văn cả khai báo, kể cả thân khối |
| Xoá kèm | chỉ viết lại dòng thật sự nhắc tên bị xoá; dòng khác giữ nguyên từng byte |
| Xoá tên mà chỉ bước `flow`, `retry` của `boundary` hay `verify` của `invariant`/`contract` còn nhắc | từ chối và liệt kê, kể cả khi bấm Xoá kèm: bộ đọc luật không báo lỗi cho những chỗ này, xoá xong chúng thành mồ côi lặng lẽ |
| Dòng không đụng tới (mọi thao tác, kể cả nút trạng thái) | giữ nguyên từng byte, kể cả chú thích, `\r\n`, BOM |
| `system.arch` là symlink | không ghi đè |
| Lý do nhiều dòng | gộp thành một dòng: không chèn được mục giả vào nhật ký |

### Chữ do agent viết không chạy được trong trang

Tên, đường dẫn, đề xuất, lý do đều có thể do agent viết, mà script lọt vào trang
chạy được với khoá phiên — tức là tự sửa được luật. Giao diện dựng HTML bằng
chuỗi template, nên mọi `${...}` rơi vào HTML phải qua `esc()` hoặc chắc chắn ra
số hay chữ viết sẵn trong mã. Test `tests/ui_html.rs` đọc `ui.html` và bắt mọi
chỗ khác, cả `innerHTML = <biến>` khi biến từng được gán chữ thô, giá trị thuộc
tính không có nháy, `href`/`on…` lấy từ dữ liệu, `insertAdjacentHTML`, `eval`.
Bộ quét bảo thủ: báo nhầm thì bọc `esc()`, đừng nới luật.

Khai báo mới đứng ngay sau khai báo cuối cùng cùng loại (component cạnh
component, luật cạnh luật), không thì ở cuối file.

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

### Đường nối không xuyên hộp, không dính chùm

Hộp cách nhau rộng (khe 150 điểm ảnh giữa hai cột, 84 giữa hai hộp trong một
cột), và mọi đường nối được **định tuyến vuông góc** trên máy chủ
(`layout::route`), trả về trong trường `routes` của `/api/model`:

- đường đi xuôi rời mép phải và vào mép trái; đi ngược thì ngược lại; hai hộp
  cùng cột thì vòng qua khe bên phải;
- mỗi đường có **điểm rời riêng** trên mép hộp, xếp theo độ cao của đầu bên
  kia nên ít cắt nhau; cạnh vi phạm và luật cùng cặp dùng chung một đường, còn
  `A → B` và `B → A` là hai đường khác nhau;
- tìm đường bằng A* trên lưới chỉ gồm những đoạn **không chạm hộp nào**, kể cả
  hộp kéo tay; mỗi chỗ rẽ và mỗi đoạn đi chung với đường đã vẽ đều bị tính
  đắt hơn, nên đường sau tìm làn trống bên cạnh;
- không tìm được (hai hộp kéo đè lên nhau, sơ đồ quá lớn so với trần số bước)
  thì giao diện vẽ đường cong như trước. Lúc đang kéo một hộp, đường của nó
  tạm vẽ cong; thả ra thì máy chủ định tuyến lại.

Khung `boundary` được tô nền nhạt và ghi `boundary <tên>` để thấy được ngay.

### Hai bố cục để so sánh

Ô chọn cạnh nút **Sắp xếp lại**:

| | **archctl** (mặc định) | **ELK** |
| --- | --- | --- |
| Ai xếp | máy chủ (`layout.rs`) | [ELK](https://eclipse.dev/elk/) layered, chạy trong Web Worker của trình duyệt |
| Hộp kéo tay | giữ chỗ, đường nối tới nó vẫn vòng tránh hộp | giữ chỗ, nhưng đường nối tới nó vẽ cong (ELK không xếp quanh hộp cố định) |
| Kích thước file chạy | khoảng 75 KB (bộ định tuyến và phần vẽ) | khoảng 1,6 MB (`elk-worker.min.js`); hiện file chạy nhúng cả hai |
| Ca lỗi | không tìm được đường thì vẽ cong | ELK lỗi hay quá 10 giây thì báo và quay về archctl |
| Mạng | không cần | không cần: ELK nhúng sẵn trong file chạy (`crates/archctl/vendor/elk/`) |

Cả hai dùng chung `.arch/layout.json` cho hộp kéo tay. Nguồn, bản ghim, sha256
và giấy phép EPL-2.0 của ELK: `crates/archctl/vendor/elk/README.md`.

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
| `/api/` không có khoá phiên, kể cả `GET` | 403 |
| `Host` không phải `127.0.0.1`, `localhost` hay `[::1]` (kèm hay không kèm cổng) | 421 |

Chỗ cuối đáng nói nhất. Trang web bất kỳ mà người dùng đang mở vẫn `fetch` tới
`127.0.0.1` được: `POST` dạng đơn giản không cần hỏi trước. Kẻ gửi không đọc
được trả lời — nhưng **thao tác ghi vẫn xảy ra**. Nghĩa là một trang lạ có thể
duyệt một đề xuất đang chờ, tức là đúng thứ mà cả hệ thống này dựng lên để bắt
buộc phải có người xem.

Trình duyệt luôn gửi `Origin` cho yêu cầu khác nguồn, nên chỉ cần đối chiếu nó.

Nhưng `curl` **không** gửi `Origin`, nên bước đó một mình không chặn được nó —
và "chỉ gắn vào 127.0.0.1" cũng không, vì agent chạy ngay trên máy này. Một
dòng lệnh là nó tự duyệt được đề xuất của chính mình:

```
curl -X POST http://127.0.0.1:4000/api/decide \
     -d '{"id":"p-aaa111","verdict":"accept","reason":"x"}'
```

Nên `serve` sinh một khoá phiên lúc khởi động và in nó trong địa chỉ:

```
archctl serve · http://127.0.0.1:4000/?k=f93986017075681d53133f31f37c8b95
```

Khoá chỉ đi ra màn hình của người dùng. Nó không vào file, không vào `argv`
nên `ps` không thấy, và không sống qua lần chạy sau. Giao diện đọc nó từ địa
chỉ rồi gửi kèm mọi yêu cầu `/api/`: header `X-Archctl-Key`, hay `?k=` cho
luồng sự kiện vì `EventSource` không đặt được header.

Đọc cũng cần khoá: mô hình có glob đường dẫn, chỗ vi phạm và đề xuất đang chờ.
Một trang web DNS-rebind tên máy của nó về `127.0.0.1` thì trình duyệt coi là
cùng nguồn và đọc được trả lời, nhưng header `Host` vẫn là tên của trang đó,
nên máy chủ từ chối mọi `Host` không phải chính máy này.

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
- **Dò thay đổi bằng cách quét lại** thay vì gọi API theo dõi của hệ điều
  hành, để không thêm dependency nào.
- **File luật hỏng thì sơ đồ dừng** và hiện lỗi cú pháp kèm số dòng, thay vì vẽ
  một sơ đồ sai.
