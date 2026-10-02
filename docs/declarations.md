# `flow`, `contract`, `boundary`, `invariant`

Bốn khai báo này mô tả **hành vi**, không phải cấu trúc. Phần lớn nội dung của
chúng cần trace runtime, contract test hoặc property test mới kiểm được — và
những phần đó vẫn chưa kiểm.

Nhưng có một phần kiểm được bằng phân tích tĩnh, và nó bắt đúng chế độ hỏng hay
gặp nhất khi làm việc với agent: **khai báo lạc hậu**. Agent đổi tên một bước,
gỡ một năng lực, bỏ một loại lỗi — và sơ đồ im lặng nói dối từ đó.

Sơ đồ sai còn tệ hơn không có sơ đồ.

## Kiểm được gì

| Khai báo | Kiểm được | Chưa kiểm |
| --- | --- | --- |
| `flow` | mọi bước phải **tồn tại** trong mã | thứ tự thực thi |
| `contract` | năng lực và `errors` phải tồn tại trong component | `requires`, `ensures`, `latency` |
| `boundary` | `contain` — tên **không được lọt** ra ngoài vùng; `compensate` phải tồn tại | `isolation`, `retry`, `timeout` |
| `invariant state` | mọi trạng thái phải tồn tại | chuyển trạng thái có hợp lệ không |
| `invariant data`/`money`/`access`/`algo` | — | tất cả |

Chỉ những trường **có cấu trúc** mới sinh ra kỳ vọng: bước của `flow`, `errors`
của `contract`, `contain` và `compensate` của `boundary`, các trạng thái trong
`invariant state`. Những trường còn lại là văn xuôi; moi tên ra từ đó chỉ sinh
kỳ vọng rác.

## Một bước của flow trỏ vào đâu

Một bước có thể là ba thứ khác nhau, và chỉ một trong ba nói gì về mã:

```
flow PlaceOrder:
  Web -> Order.reserveStock -> charge
```

| Dạng | Nghĩa | Kiểm gì |
| --- | --- | --- |
| `Web` — tên component | người thực hiện | không gì cả: không có ký hiệu nào tên `Web` trong mã |
| `Order.reserveStock` | năng lực của một component | `reserveStock` phải có, **và phải nằm trong `Order`** |
| `charge` — tên trần | một hàm ở đâu đó | `charge` phải có ở đâu đó |
| `order.charge` — biến chấm phương thức | `order` không phải component | `charge` phải có ở đâu đó |

Dạng thứ hai kiểm được nhiều nhất, nên nên dùng nó:

```
⚠ KHAI BÁO LẠC HẬU  bước của flow PlaceOrder `reserveStock`
  — `reserveStock` chỉ thấy trong Order, lẽ ra phải có trong Web
```

## Khi "pass" không chứng minh được gì

Kiểm tồn tại dựa trên việc **gặp một cái tên**. Tên càng chung thì gặp nó càng
không nói lên điều gì — một bước tên `run` sẽ khớp ở khắp nơi, và kiểm tồn tại
coi như vô hiệu.

Chỗ nguy hiểm không phải là nó vô hiệu, mà là **im lặng**: không có cảnh báo
nào thì người đọc hiểu rằng chỗ đó đã được kiểm và không sao.

```
⚠ KIỂM KHÔNG KẾT LUẬN  bước của flow PlaceOrder `run`
  .arch/system.arch:5  — `run` có trong 12/13 file nên tìm thấy nó không nói lên điều gì
  → đổi sang một tên chỉ thuộc về bước này
```

Ngưỡng: có mặt ở **từ một phần tư số file trở lên**, và ít nhất năm file. Mức
sàn đó để repo nhỏ không bị cảnh báo rác — trong hai file thì tên nào cũng "có
mặt ở một nửa repo".

Lưu ý câu chữ: *không kết luận được*, không phải *tên xấu*. `log` có thể là một
hàm hoàn toàn có thật được gọi khắp nơi; gặp nó trong `Order` vẫn không chứng
minh được rằng bước `log` của luồng này tồn tại.

## Khai báo lạc hậu

```
⚠ KHAI BÁO LẠC HẬU  bước của flow PlaceOrder `updateAnalytics`
  .arch/system.arch:22  — `updateAnalytics` không có trong mã — sửa mã, hoặc sửa khai báo
```

Đây là **cảnh báo**, không phải vi phạm: mã có thể đúng mà khai báo mới là thứ
sai. `--strict` biến nó thành vi phạm, dành cho CI.

Khi tên có thật nhưng nằm sai chỗ, thông báo nói rõ hơn:

```
⚠ KHAI BÁO LẠC HẬU  năng lực trong contract Payment.charge `charge`
  — `charge` chỉ thấy trong Order, lẽ ra phải có trong Payment
```

## `contain` — lỗi không được lọt ra ngoài vùng

Đây là phần có giá trị nhất trong bốn thứ, và là **ràng buộc thật** chứ không
chỉ là đối chiếu tài liệu.

```
boundary Checkout contains Order, Stock, Payment:
  contain  Timeout, Unavailable
```

Nghĩa là: `Timeout` và `Unavailable` là lỗi hạ tầng, chúng được xử lý **bên
trong** vùng. Tầng ngoài chỉ được thấy lỗi nghiệp vụ.

```
✗ VI PHẠM  Web -> Timeout  (lọt ra ngoài vùng, #accepted)
  src/web/checkout-page.ts:12  dùng `Timeout` (2 lần trong file)
  luật: `contain` của boundary Checkout
  → `Timeout` chỉ được nằm trong Order, Stock, Payment — ở ngoài phải chuyển thành lỗi khác
```

Rò rỉ kiểu này là lỗi kiến trúc kinh điển: một lỗi timeout của database bò lên
tới tầng giao diện, và từ đó mọi tầng ở giữa phải biết về nó.

## Vì sao không kiểm thứ tự bước

Nhìn thì tưởng dễ: `charge` phải xuất hiện sau `reserveStock`. Nhưng bộ quét
không phân biệt được **lời gọi** với **định nghĩa**. Một module định nghĩa
`charge()` ở dòng 10 và `reserveStock()` ở dòng 20 sẽ bị báo sai thứ tự, trong
khi thứ tự định nghĩa hàm thì hoàn toàn tuỳ ý.

Phân biệt hai thứ đó cần thông tin kiểu và phân tích luồng điều khiển qua
async, nhánh điều kiện, return sớm và gọi gián tiếp. Đó là vấn đề nghiên cứu,
không phải việc vài trăm dòng.

Thứ tự thật chỉ kiểm được bằng **trace runtime**. Chưa làm.

## Nhất quán bên trong khai báo

Những lỗi này bắt được ngay khi đọc file, không cần nhìn mã:

```
✗ lỗi:22: `on_fail(c)` nhưng `c` không phải bước của flow `PlaceOrder`
  → bù trừ phải gắn vào một bước có thật trong luồng

⚠ cảnh báo:40: `retry Payment.charge` nhưng không có contract nào tên đó
  → thử lại chỉ an toàn khi năng lực đó được khai báo là idempotent
```

Cái thứ hai đáng chú ý: `retry` mà không có `idempotent_by` là cách tạo ra lỗi
trừ tiền hai lần.

## Cách tìm tên

Giống hệt `api`: tra trên **dòng token**, nên tên nằm trong chú thích hay trong
chuỗi không được tính. Khớp khi từ đúng bằng tên, hoặc tên đứng sau `.` hay
`::` (`order.charge` khớp `charge`).

Tra bằng bảng băm nên số lượng tên không ảnh hưởng tốc độ: thêm cả bốn loại
kiểm vào chỉ làm 2000 file chậm thêm từ ~110ms lên ~120ms.

## Chế độ hỏng còn lại

- **Kiểm tồn tại hỏng về phía an toàn.** Nhầm thì nó nói "có" trong khi không
  có — bỏ sót, chứ không báo động giả.
- **Ngưỡng "không kết luận được" là một con số chọn tay.** Một cái tên có mặt ở
  24% số file vẫn gần như không nói lên điều gì, mà không bị báo. Ngưỡng dịch
  chuyển đường im lặng chứ không xoá nó.
- **`invariant data`, `money`, `access`, `algo` vẫn chỉ là lời hứa**, và vẫn bị
  cảnh báo như vậy nếu thiếu `verify`. Muốn chặn dữ liệu nhạy cảm rò ra ngoài
  thì dùng `store owned_by` hoặc `api` cộng `deny` — hai thứ đó kiểm thật.
