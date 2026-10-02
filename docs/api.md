# `api` — hàm và từ khoá bị theo dõi

```
api Unsafe     at "strcpy", "sprintf", "gets", "strcat", "alloca"
api Danger     at "eval", "pickle"
api RustUnsafe at "unsafe"

deny * -> Unsafe
deny * -> RustUnsafe except Decode
allow Legacy -> Unsafe
```

Một nhóm `api` không ánh xạ sang file nào. Nó là tập tên hàm hoặc từ khoá được
tìm **thẳng trong dòng token** của mã nguồn.

## Vì sao loại luật này quan trọng hơn với mã hệ thống

Đồ thị import có vùng mù lớn: FFI, liên kết thư viện tĩnh và động, `dlopen`,
con trỏ hàm, vtable, tiến trình con, IPC. Với C/C++ thì `#include` là điều kiện
cần nhưng rất xa điều kiện đủ.

Nhóm `api` **không có vùng mù đó**. Một lời gọi `strcpy` phải xuất hiện trong
mã nguồn thì mới chạy được, dù nó được gọi qua con trỏ hay qua lớp bọc nào.

## Vì sao không dùng `grep`

Vì `grep` không biết đâu là mã và đâu không phải. Trong `examples/sensor`:

```c
/* Hai dòng dưới KHÔNG phải lời gọi: strcpy trong chú thích. */
static const char *warn = "strcpy is unsafe";

    strcpy(name, "eth0");
    strcpy(name, "eth1");
```

`grep -c strcpy` trả về **4**. `archctl check` báo **2 lần**, ở đúng dòng 18.
Bộ quét chạy trên dòng token, nên chú thích và chuỗi không bao giờ khớp. Với
một danh sách hàm cấm thì báo động giả là thứ giết chết công cụ nhanh nhất.

## Cách khớp

| Mã | `api X at "eval"` | Vì sao |
| --- | --- | --- |
| `eval(s)` | khớp | đúng tên |
| `builtins.eval(s)` | khớp | tiền tố đủ điều kiện |
| `std::ptr::copy(..)` với `"copy"` | khớp | tiền tố `::` |
| `my_eval(s)` | **không** | là hàm khác |
| `eval_count` | **không** | là tên khác |
| `// eval(s)` | **không** | chú thích |
| `"eval(s)"` | **không** | chuỗi |

Khớp khi từ **đúng bằng** ký hiệu, hoặc ký hiệu đứng sau `.` hay `::`.

## `except` là miễn trừ

```
deny * -> RustUnsafe except Decode
```

nghĩa là **Decode được phép**, không chỉ là "deny không áp dụng cho Decode".
Hai cách hiểu này khác nhau: với cách thứ hai, Decode sẽ rơi xuống nhánh "không
có `allow` nào" và vẫn bị báo vi phạm — trái hẳn ý người viết luật.

## Khai báo `api` là đã cấm

Nhóm `api` tham gia luật giống hệt mọi đích khác, nên component **đã vào diện
kiểm** mà dùng ký hiệu trong nhóm sẽ bị báo vi phạm kể cả khi không có `deny`
nào. Viết `allow Legacy -> Unsafe` để miễn trừ.

Nếu chỉ muốn cấm có chọn lọc, viết `deny` cho đúng component thay vì `deny *`.

## Báo cáo

Mỗi cặp (file, ký hiệu) báo **một lần**, ở lần gặp thật đầu tiên, kèm số lần:

```
✗ VI PHẠM  Native -> Unsafe  (bị cấm, #locked)
  native/parser.c:18  dùng `strcpy` (2 lần trong file)
  luật: deny * -> Unsafe
  → Native chưa được phép dùng nhóm Unsafe
```

In từng vị trí một sẽ làm ngập báo cáo khi một hàm xuất hiện năm chục lần.

## Chi phí

Gần như bằng không. Lex vốn đã chạy để trích import, nên tìm ký hiệu chỉ là lọc
trên dòng token có sẵn: **109ms so với 108ms** trên 2000 file với 6 ký hiệu.

## Giới hạn

- **Không phân biệt lời gọi với định nghĩa.** File tự định nghĩa `strcpy` cũng
  bị tính. Thường là điều mong muốn, nhưng cần biết.
- **Không theo được bí danh.** `const f = eval; f(s)` không bị bắt.
- **Không biết ngữ cảnh.** Một biến tên `unsafe` trong Python cũng khớp nếu
  `unsafe` nằm trong nhóm đang theo dõi. Chọn ký hiệu cho đúng ngôn ngữ.
- **Không thay thế công cụ phân tích bảo mật.** Đây là ràng buộc kiến trúc ở
  mức "ai được phép chạm vào cái gì", không phải bộ dò lỗ hổng.
