# `archctl check` — ngữ nghĩa và giới hạn

Hiện chỉ kiểm **luật phụ thuộc giữa component** cho TypeScript và JavaScript.

## Cách một component vào diện kiểm

Component bị kiểm khi có ít nhất một `allow` **đang có hiệu lực** nêu đúng tên
nó làm nguồn:

```
allow Stock -> Notify      # từ đây Stock bị kiểm
```

Chưa viết `allow` cho component nào thì component đó tạm để yên, và báo cáo nói
rõ điều đó. Đây là lối vào cho codebase đã có: áp dần từng component, không phải
mô tả cả repo trước khi có được giá trị nào.

`allow * -> X` chỉ **cấp quyền**, không đưa ai vào diện kiểm.

Ngoại lệ: **`deny` luôn có hiệu lực**, kể cả với component chưa vào diện kiểm.
Luật cấm không nên phải chờ ai bật.

## Thứ tự xét

1. Có `deny` đang có hiệu lực khớp quan hệ, và nguồn không nằm trong `except` → **vi phạm**
2. Có `allow` đang có hiệu lực khớp quan hệ → hợp lệ
3. Nguồn đã vào diện kiểm → **vi phạm**, không có allow nào cho phép
4. Còn lại → chưa kiểm

`deny` thắng `allow` khi cả hai cùng khớp. `except` miễn trừ cho **nguồn**:
`deny * -> PaymentDB except Payment` nghĩa là chỉ Payment được chạm PaymentDB.

Luật `#draft` và `#proposed` không có hiệu lực, nên cũng **không cấp quyền**.

## Gói bên ngoài

Gói npm chỉ tham gia luật khi được khai báo `external`, với tên gói trong `at`:

```
external Stripe at "stripe"
deny * -> Stripe except Payment
```

Gói không khai báo thì bị bỏ qua. Bắt khai báo mọi gói npm sẽ khiến file luật
dài vô ích.

## Phân giải module

| Dạng | Xử lý |
| --- | --- |
| `./x`, `../x` | Thử `.ts .tsx .mts .cts .js .jsx .d.ts`, rồi `x/index.*` |
| `./x.js` | Thử cả `./x.ts` — cách viết ESM của TypeScript |
| `stripe`, `@scope/pkg` | Tên gói |
| `@/lib/x`, `~/lib/x`, `/abs` | **Chưa hỗ trợ**, báo ra |
| Tương đối nhưng không có file | **Báo ra** |

Hai dòng cuối là quyết định có chủ ý: thứ nào không phân giải được thì nói
thẳng. Bỏ sót âm thầm làm người dùng tin rằng không có vi phạm, tệ hơn hẳn.

## Giới hạn đã biết

- **tsconfig `paths` chưa hỗ trợ.** Dự án dùng alias `@/...` sẽ thấy nhiều dòng
  "chưa phân giải". Đây là việc tiếp theo.
- **Không có `--changed`.** Quét toàn repo mất 123ms trên 2000 file. Đủ cho CI,
  chưa đủ nhanh cho hook mỗi lần lưu file.
- **`store owned_by` chưa kiểm.** Cần phân tích truy cập database và ORM. Báo
  cáo đếm số luật thuộc loại này và nói rõ là chưa kiểm được.
- **`flow`, `contract`, `boundary`, `invariant` chưa kiểm.** Đọc được, kiểm
  tham chiếu được, nhưng chưa đối chiếu với mã.
- **Bộ quét import không phải parser đầy đủ.** Nhận regex bằng suy đoán theo
  token đứng trước, như các bộ minify. Biểu thức trong template literal bị bỏ
  qua. Nếu độ chính xác không đủ, thay `scan.rs` bằng tree-sitter.
- **Chỉ TypeScript và JavaScript.** Python và JVM chưa có.

## Mẫu `at` chồng nhau

File khớp nhiều component một lúc thì component khai báo trước thắng, và báo
cáo in cảnh báo `MẪU CHỒNG NHAU`. Nên sửa mẫu thay vì dựa vào thứ tự.

## Bỏ qua thư mục

Luôn bỏ qua, không cấu hình được: `.git`, `.arch`, `node_modules`, `target`,
`dist`, `build`, `out`, `.next`, `.nuxt`, `.svelte-kit`, `coverage`, `vendor`,
`__pycache__`, `.venv`. Symlink không được đi theo, để tránh vòng lặp và tránh
ra ngoài gốc dự án.

Thêm mẫu glob vào `.arch/.archignore` để bỏ qua thêm.
