# Khảo sát: có thứ gì dùng được cho bốn giới hạn còn lại không

Ngày 2026-10-02. Bốn giới hạn còn lại đều cần "hiểu hành vi" mà bộ dò token
không có: thứ tự bước trong `flow`, `requires`/`ensures` của `contract`,
`invariant data`/`money`/`access`, `retry`/`timeout` của `boundary`. Cộng một
giới hạn thứ năm: cạnh phụ thuộc liên ngôn ngữ.

Kết luận ngắn: **không có gì dùng thẳng được**, hai việc làm được mà không cần
thêm dependency nào, và một việc vẫn nên từ chối làm.

## Không có công cụ nào kiểm sẵn thứ này

Không tìm thấy công cụ nào kiểm `flow`, `contract`, `invariant` hay `boundary`
theo kiểu khai báo của `.arch`.

Bài gần nhất là **DCL** (Dependency Constraint Language, Terra & Valente, SPE
2009, [PDF](https://homepages.dcc.ufmg.br/~mtov/pub/2009_spe.pdf)) — ràng buộc
dạng *chỉ được* và *không được* giữa module, rất giống `allow`/`deny`. Nhưng
không có repo nào còn sống: `gh search` không ra `dcl-suite` hay `dclcheck`,
và tìm trên web không ra cú pháp. Nghiên cứu không kèm công cụ dùng được.

## ast-grep: đo rồi, không đáng

`ast-grep` phân biệt được lời gọi với định nghĩa hàm bằng AST, đúng chỗ bộ dò
token kẹt. Giá thật:

| | archctl hiện tại | nếu thêm ast-grep |
| --- | --- | --- |
| crate phải kéo về | **0** | **80** |
| binary | 0.9 MB | ~7 MB |

80 crate gồm toàn bộ grammar tree-sitter cho hơn hai mươi ngôn ngữ (bash,
c-sharp, dart, elixir, haskell, hcl...), cộng serde, regex, ignore. Công cụ
này chạy trong repo của người dùng và trong vòng lặp của agent, nên đó là
tám mươi bề mặt tấn công mới để đổi lấy một việc mà phần dưới nói là không
nên làm.

## Nghiên cứu nói đừng kiểm thứ tự bằng heuristic

Ba con số, đều từ phần tóm tắt chứ không phải toàn văn (các PDF này dùng font
nhúng nên không trích được text, và máy chưa có `poppler`):

| Nguồn | Con số | Nghĩa là |
| --- | --- | --- |
| Pruijt et al., SPE 2016 | 10 công cụ kiểm tuân thủ kiến trúc, **đều có AST hoặc bytecode**, chỉ báo cáo 77% phụ thuộc trên benchmark và 72% trên mã thật | bỏ sót là chuẩn mực của cả lĩnh vực, không phải khuyết điểm riêng của archctl |
| PyCG, ICSE 2021 | call graph Python một ngôn ngữ: precision 99.2%, recall **69.9%** | ngay cả phân tích tĩnh chuyên sâu một ngôn ngữ cũng chỉ thấy khoảng bảy phần mười |
| Amann et al., TSE 2018 ([PDF](https://arxiv.org/pdf/1712.00242)) | các bộ phát hiện dùng-sai-API tĩnh có "precision và recall cực thấp" | đây đúng là thể loại mà kiểm thứ tự bước sẽ rơi vào |

Giữ nguyên quyết định cũ: **không kiểm thứ tự bước**. Thứ tự thật chỉ kiểm
được bằng trace runtime.

> **Cả hai việc ở mục dưới đã làm xong** (commit `2f93092` và commit kế tiếp).
> Mục này giữ lại phần đo đạc đã dẫn tới quyết định.

## Hai việc làm được, không cần dependency nào

### 1. Phân biệt định nghĩa với lời gọi

Đây là chỗ đã đánh giá sai. Tài liệu viết rằng bộ quét "không phân biệt được
lời gọi với định nghĩa", rồi gộp luôn kết luận sang việc kiểm thứ tự. Nhưng
**riêng việc phân biệt** chỉ cần nhìn token đứng ngay trước tên: `fn`, `def`,
`function`, `func`, `const … = (`, hay dạng `kiểu tên(` của C.

Đo trên `examples/` cả năm ngôn ngữ: nhận đúng mọi định nghĩa, không nhận
nhầm cái nào. Đo trên 43 file Rust của chính repo này, so tín hiệu "được định
nghĩa ở đâu" với "có xuất hiện ở đâu":

| tên | xuất hiện | được định nghĩa | sắc hơn |
| --- | --- | --- | --- |
| `new` | 34/43 file | 7/43 | 5× |
| `check` | 20/43 | 1/43 | 20× |
| `join` | 26/43 | 1/43 | 26× |
| `c` | 28/43 | 1/43 | 28× |

Theo ngưỡng "không kết luận được" hiện tại, **32 tên** bị gắn cảnh báo khi
đếm xuất hiện; cả **32** đều hết khi đếm định nghĩa.

Việc này đổi `contract Payment.charge` từ "`charge` có xuất hiện trong
Payment" thành "`charge` được **định nghĩa** trong Payment" — chặt hơn hẳn,
và đúng với điều khai báo muốn nói.

### 2. Cạnh FFI Rust↔C và Go↔C

`examples/sensor` cố ý chứa cạnh `Decode -> Native` qua FFI, và tài liệu ghi
là không bắt được. Nhưng bằng chứng nằm ngay trong mã:

```rust
// src/decode/ethernet.rs
unsafe extern "C" {
    fn parse_ethernet(buf: *const u8, len: usize) -> i32;
}
```
```c
// native/parser.c
int parse_ethernet(const unsigned char *buf, unsigned long len) {
```

Khai báo `extern "C"` ở một bên, định nghĩa cùng tên ở bên kia. Đây là **một
vi phạm thật đang bị bỏ sót**: `.arch` không có `allow Decode -> Native`.

Go còn dễ hơn: `import "C"` rồi `C.parse_ethernet()` — tên sau `C.` chính là
tên hàm C.

Hai trường hợp này chính xác cao vì chúng là **khai báo tường minh**, không
phải suy đoán. Chúng không giải quyết cả bài toán liên ngôn ngữ — `dlopen`,
con trỏ hàm, IPC vẫn vô hình — nhưng chúng là phần lớn FFI viết tay.

Hướng đọc thêm nếu cần chính xác hơn: [cbindgen](https://github.com/mozilla/cbindgen)
và [bindgen](https://github.com/rust-lang/rust-bindgen) sinh ra đúng cặp
header ↔ `extern` này, nên chúng là nơi xem các dạng khai báo còn lại.

## Ba thứ còn lại: trỏ ra ngoài, đừng tự kiểm

> **Đã làm.** Cơ chế là `verify <tên> runs "<lệnh>"` cộng lệnh `archctl
> verify`, với biên nhận gắn vào bản mã lúc chạy — xem [verify.md](verify.md).

`contract` `requires`/`ensures`, `invariant data`/`money`, `boundary`
`retry`/`timeout` đều cần công cụ chuyên dụng, và đều đã có công cụ tốt:

| Việc | Công cụ | Giấy phép |
| --- | --- | --- |
| tiền/hậu điều kiện Rust | [Kani](https://github.com/model-checking/kani) 3.5k★, [Verus](https://github.com/verus-lang/verus) 3.3k★ | Apache-2.0, MIT |
| hợp đồng Python | [deal](https://github.com/life4/deal) 916★, [icontract](https://github.com/Parquery/icontract) 417★ | MIT |
| property test | [Hypothesis](https://github.com/HypothesisWorks/hypothesis) 9k★, [fast-check](https://github.com/dubzzz/fast-check) 5.2k★, [quickcheck](https://github.com/BurntSushi/quickcheck) 2.8k★ | đọc LICENSE, MIT, Unlicense |
| tiêm lỗi mạng | [Toxiproxy](https://github.com/Shopify/toxiproxy) 12.4k★ | MIT |

Cơ chế đã có sẵn trong `.arch`: trường `verify`. archctl không tự kiểm ngữ
nghĩa — nó kiểm rằng `verify` **tồn tại và đã chạy xong trên bản mã này**.

## Điều chưa chắc

- Mọi con số từ bài báo ở trên đến từ phần tóm tắt, không phải toàn văn. Các
  PDF dùng font nhúng nên không trích được text bằng thư viện chuẩn, và máy
  chưa có `poppler`. Muốn chắc thì cài rồi đọc lại.
- Chưa chạy thử ast-grep hay Semgrep trên mã thật; con số 80 crate là đếm từ
  `Cargo.lock` sinh ra, không phải từ việc dùng thật.
- Danh sách loại phụ thuộc của DCL (access, declare, create, extend,
  implement, throw) chưa xác minh được trong bài gốc.
