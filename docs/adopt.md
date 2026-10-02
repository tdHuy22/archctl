# Áp luật lên dự án đã có mã

Hai rào cản khiến không ai thử một công cụ như thế này:

1. **Phải gõ tay cả file luật** trước khi thấy được giá trị nào
2. **Ngày đầu bật lên có hàng trăm vi phạm**, bắt sửa hết là không thực tế

`archctl init --from-code` và `archctl baseline` giải đúng hai chỗ đó.

## `archctl init --from-code`

```bash
archctl init --from-code
```

```
Đã đọc 2000 file (2000 TypeScript) · gốc mã nguồn: src
Dựng được 20 component và 372 quan hệ, tất cả ở `#draft`.

  tạo  .arch/system.arch
  tạo  .arch/proposals.arch
  tạo  .arch/decisions.md
  tạo  .arch/ARCHITECTURE.md
  sửa  AGENTS.md
  sửa  CLAUDE.md
  sửa  .gitignore
```

Mất khoảng 190ms trên 2000 file.

### Nó đoán thế nào

| Bước | Cách làm |
| --- | --- |
| Gốc mã nguồn | Thư mục tên quy ước (`src`, `lib`, `app`, `internal`, `pkg`), nhiều file nhất trong số đó. Không có cái nào thì lấy thư mục chiếm từ 60% số file. Vẫn không có thì dùng gốc repo |
| Component | Mỗi thư mục con ngay dưới gốc mã nguồn. File nằm thẳng trong gốc gom thành một component riêng |
| Tên | `create-order` → `CreateOrder`. Trùng thì đánh số |
| Quan hệ | Import **thật** trong mã, chỉ giữa hai component. Thư viện ngoài bị bỏ qua |
| Giới hạn | Tối đa 24 component; phần dư liệt kê trong chú thích cuối file |

Khi không đoán được chắc chắn thì nó **không đoán** — dùng gốc repo và nói ra.

### Kết quả là mô tả, không phải thiết kế

Đây là điều quan trọng nhất, và chính file sinh ra cũng nói thẳng ở đầu:

```
# ĐÂY LÀ MÔ TẢ HIỆN TRẠNG, KHÔNG PHẢI THIẾT KẾ. Nó chép lại cả những chỗ
# đang lộn xộn. Mọi thứ đang `#draft` nên chưa luật nào có hiệu lực.
```

Mọi thứ mang `@ai #draft`, nên ngay sau `init` thì `archctl check` trả về **0 vi
phạm** — chưa có gì bị cưỡng chế. Việc của người là đọc lại, xoá component không
có thật, gộp những cái nên gộp, bỏ quan hệ không nên tồn tại, rồi nâng dần lên
`#accepted`.

Không cần sửa xong cả file mới chạy được: **component nào có `allow` đang có
hiệu lực thì component đó bắt đầu bị kiểm**, phần còn lại tạm để yên.

## `archctl baseline`

Sau khi nâng vài component lên `#accepted`, sẽ có vi phạm có sẵn. Bắt sửa hết
trước khi bật CI là không thực tế; bỏ qua hết thì công cụ vô dụng.

```bash
archctl check       # 7 vi phạm, mã thoát 1
archctl baseline    # ghi lại hiện trạng
archctl check       # 0 vi phạm mới, mã thoát 0
```

Từ đó: **vi phạm cũ được tha, vi phạm mới làm hỏng build**. Đây là cách người ta
áp `strict` của TypeScript hay một luật lint mới lên dự án đang chạy.

```
⊖ 7 vi phạm có sẵn từ trước, đang được tha theo `.arch/baseline`

✗ VI PHẠM  Native -> Unsafe  (bị cấm, #accepted)
  native/checksum.c:12  dùng `gets`

1 vi phạm mới, 1 cảnh báo
```

### Bánh răng chỉ siết, không nới

Baseline ghi **danh tính từng vi phạm**, không phải tổng số. Sửa một cái rồi gây
ra một cái khác thì không bù trừ cho nhau — cái mới vẫn bị chặn.

Khi một mục đã sửa xong, `check` nhắc:

```
⊖ 1 mục trong baseline đã sửa xong — chạy `archctl baseline` để siết lại
```

Chạy lại thì mục đó bị bỏ khỏi danh sách và **không quay lại được**:

```
Đã ghi .arch/baseline: 6 vi phạm được tha.
  1 mục cũ đã sửa xong và bị bỏ khỏi danh sách — không quay lại được.
```

Nếu `baseline` phải tha thêm vi phạm mới, nó nói ra — việc đó chỉ hợp lý khi vừa
mở rộng luật, không phải khi vừa viết mã sai.

### Danh tính một vi phạm

`<Nguồn>-><Đích>|<file>|<ký hiệu>`. Đổi tên file làm mục cũ hết hiệu lực, và đó
là hành vi đúng: file được viết lại thì vi phạm trong đó không còn được tha.

`.arch/baseline` **phải vào git** — nó là trạng thái chung của cả đội. Khác với
`.arch/.state`, vốn là bộ nhớ cục bộ và được `init` thêm vào `.gitignore`.

## `archctl init` không kèm `--from-code`

Dựng khung rỗng kèm chú thích cú pháp, cho dự án mới hoặc khi muốn tự viết từ
đầu. Nó không bao giờ đè lên `.arch/system.arch` đang có — phải thêm `--force`.
`decisions.md` thì **không bao giờ bị đè**, kể cả với `--force`: nhật ký quyết
định là thứ không được mất.
