# `verify` — từ lời hứa thành bằng chứng

`contract`, `boundary` và `invariant` mô tả hành vi. Phần lớn nội dung của
chúng cần contract test, property test hay tiêm lỗi mới kiểm được —
[declarations.md](declarations.md) nói rõ phần nào kiểm được bằng phân tích
tĩnh và phần nào không.

Phần không kiểm được thì trỏ ra ngoài. Đó là việc của `verify`.

## Vấn đề nó sửa

Hệ thống vẫn cảnh báo khi một khai báo thiếu `verify`:

```
⚠ contract `Payment.charge` không có verify, đây chỉ là lời hứa
```

Câu đó ngụ ý rằng **có** `verify` thì không phải lời hứa nữa. Nhưng `verify`
từng chỉ là một cái tên không dẫn tới đâu cả — chưa có gì chạy. Thêm nó vào
không làm điều gì được chứng minh; nó chỉ làm cảnh báo im đi.

Đúng chế độ hỏng mà cả dự án này dựng lên để tránh: **im lặng bị đọc thành
"đã kiểm, không sao"**.

## Khai báo

```
verify property_test  runs "npm run test:props"        @human #accepted
verify chaos_payment  runs "npm run chaos -- payment"  @human #accepted

invariant data [Order] order.total == sum(items.price) verify property_test

boundary Checkout contains Order, Stock, Payment:
  verify  chaos_payment
```

Tên tách khỏi lệnh vì hai thứ đó phục vụ hai người đọc khác nhau:
`verify property_test` đọc được trên sơ đồ, `npm run test:props` thì không.

Một cái tên chưa khai báo, hay một khai báo không ai dùng, đều được báo ra.

## Chạy

```bash
archctl verify                  # chạy hết
archctl verify property_test    # chạy một cái
archctl verify --quiet          # nuốt đầu ra của lệnh, chỉ giữ kết quả
```

```
▸ property_test  npm run test:props
  đạt · 2.4 giây
▸ chaos_payment  npm run chaos -- payment
  HỎNG · 11.8 giây

1/2 đạt · biên nhận ghi vào .arch/.verified
```

Mã thoát khác 0 khi có cách kiểm nào hỏng, nên dùng được trong CI.

## Biên nhận tự hết hạn

Một lần chạy đạt từ **trước khi sửa mã** không chứng minh được gì cho mã hiện
tại. Biên nhận ghi lại thời điểm chạy và dấu vân tay của luật lúc đó, rồi
`archctl check` so lại:

```
⚠ CHƯA CHỨNG MINH  cách kiểm `property_test` — mã đã đổi sau lần chạy gần nhất
  → archctl verify property_test
```

| Lý do | Nghĩa là |
| --- | --- |
| chưa chạy lần nào | khai báo xong nhưng chưa ai chạy |
| lần chạy gần nhất không đạt | lệnh trả về mã khác 0 |
| luật đã đổi sau lần chạy gần nhất | `.arch/system.arch` đổi về **nội dung**; thêm chú thích thì không tính |
| mã đã đổi sau lần chạy gần nhất | có file nguồn sửa sau lúc chạy |

Thời điểm ghi theo **mili giây**. Giây thì không đủ: sửa mã rồi chạy verify
ngay là chuyện thường, và khi cả hai rơi vào cùng một giây thì biên nhận vừa
ghi xong đã bị coi là cũ.

Biên nhận nằm ở `.arch/.verified` và không vào git.

## Ai được chạy lệnh shell

Đây là chỗ duy nhất trong archctl thực thi một lệnh, nên ranh giới phải rõ.

| | |
| --- | --- |
| `archctl verify` | chạy lệnh — và chỉ khi người gõ lệnh đó |
| `archctl check` | **không bao giờ** chạy gì, kể cả trong hook của agent. Nó chỉ đọc biên nhận |
| `archctl propose` | **từ chối** đề xuất bắt đầu bằng `verify` |

Chỗ cuối quan trọng. `archctl accept` ghi thẳng đề xuất của agent vào file
luật, nên nếu agent đề xuất được một cách kiểm thì một lần duyệt vội là đủ để
nó tự chọn thứ chạy trên máy người dùng. Người muốn thêm cách kiểm thì tự sửa
`.arch/system.arch` — đó cũng là nguyên tắc một-người-ghi-một-file đã có ở
[protocol.md](protocol.md).

Cả hai ranh giới đều có test, và test cho `check` dựng một cách kiểm
`touch <file>` rồi khẳng định file đó không bao giờ xuất hiện.

## Nó không kiểm nội dung lệnh

archctl không biết `npm run test:props` có thật sự kiểm invariant đó không.
Nó chỉ biết lệnh đã chạy, đã đạt, và chạy trên đúng bản mã này. Phần còn lại
là việc của người viết lệnh — cũng như mọi công cụ CI khác.
