# `archctl diff` — tách thay đổi thật khỏi thay đổi hình thức

```bash
archctl diff            # so với HEAD
archctl diff main       # so với một ref bất kỳ
```

Nút cổ chai của cả hệ thống là **sự chú ý của con người**. Luật chỉ có giá trị
khi người duyệt thật sự đọc đề xuất trước khi bấm chấp nhận. Nếu một lần sắp
xếp lại file cũng hiện ra như thay đổi kiến trúc, người duyệt sẽ ngừng đọc, và
cả cơ chế mất tác dụng.

Vì vậy `diff` không so sánh văn bản. Nó so sánh **mô hình đã chuẩn hoá** và xếp
mọi thay đổi vào bốn loại.

| Loại | Nội dung | Có tính là đổi kiến trúc |
| --- | --- | --- |
| **cấu trúc** | component, external, store, quan hệ: thêm, bớt, đổi phạm vi `at`, đổi `owned_by` | có |
| **ngữ nghĩa** | flow, contract, boundary, invariant, view | có |
| **quản trị** | chỉ đổi `@human`/`@ai` hoặc `#draft`…`#locked` | không |
| **xê dịch** | nội dung y hệt, chỉ đổi chỗ trong file | không, chỉ đếm |

```
kiến trúc 5bbbb540 → 67424c5b  (so với HEAD)

+ component  Cache
    at src/cache/**  (cấu trúc)
~ component  Decode
    at src/decode/**
  → at src/decode/**, src/parse/**  (cấu trúc)
! component  Tools
    @human #accepted
  → @human #draft  (quản trị)
- rule       deny * -> LibPcap except Capture  (cấu trúc)

3 cấu trúc · 0 ngữ nghĩa · 1 quản trị · 0 chỉ xê dịch trong file

! Luật bị nới lỏng: Tools  — đọc kỹ chỗ này
```

Khi không có gì đổi về ngữ nghĩa:

```
kiến trúc 5bbbb540 — không đổi so với HEAD
19 phần tử chỉ xê dịch trong file, không cần đọc kỹ
```

## Dấu `!` — luật bị nới lỏng

Nâng trạng thái lên mức chặt hơn là việc bình thường. **Hạ xuống** thì khác:
`#locked` → `#accepted` → `#proposed` → `#draft` mở ra những thứ trước đó bị
cấm. Mọi lần hạ đều được đánh dấu `!` và nhắc lại ở cuối báo cáo.

Đây là chỗ agent dễ lách nhất nếu nó được quyền sửa file luật. Nó không được
quyền đó, nhưng `diff` vẫn phải bắt được nếu bằng cách nào đó việc này xảy ra.

## Danh tính của phần tử

Ghép được hai phiên bản với nhau hay không phụ thuộc vào việc phần tử có tên
riêng không.

- **Có tên** — component, store, boundary, flow, view, contract: ghép theo tên,
  nên đổi nội dung hiện ra là `~ đã đổi` kèm trước và sau.
- **Không tên** — quan hệ và invariant: danh tính chính là nội dung, nên đổi
  nội dung hiện thành một cặp `+` và `-`. Ghép hai luật với nhau chỉ là đoán,
  và đoán sai còn tệ hơn.

## Hash của kiến trúc

`archctl check` và `archctl diff` đều in hash ở đầu. Nó băm **mô hình đã chuẩn
hoá**, không phải văn bản file:

- thêm chú thích, đổi khoảng trắng, đổi thứ tự khai báo → **hash giữ nguyên**
- đổi `at`, đổi quan hệ, đổi trạng thái → **hash đổi**

Lý do: hash là tín hiệu báo agent đọc lại file luật. Báo "kiến trúc đã thay
đổi" khi không có gì đổi là kêu sói, và agent sẽ học cách phớt lờ cảnh báo.

## Nguồn ý tưởng

Cách phân loại thay đổi theo bản chất — thay vì chỉ thêm/bớt — lấy từ
`delta/architecture-delta.mjs` của [tt-a1i/archify](https://github.com/tt-a1i/archify)
(MIT). Họ chia thành `topology / semantic / scope / evidence / geometry` cho
mục đích hiển thị sơ đồ; ở đây rút gọn còn bốn loại hợp với việc duyệt luật.

Không có dòng code nào được sao chép.
