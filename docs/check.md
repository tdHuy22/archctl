# `archctl check` — ngữ nghĩa và giới hạn

Hiện kiểm **luật phụ thuộc giữa component** cho năm ngôn ngữ.

## Ngôn ngữ đã hỗ trợ

| Ngôn ngữ | Phần mở rộng | Nhận được | Phân giải |
| --- | --- | --- | --- |
| TypeScript, JavaScript | `.ts .tsx .mts .cts .js .jsx .mjs .cjs` | `import`, `import type`, `export … from`, `require()`, `import()` | tương đối, `index.*`, `.js`→`.ts`, **alias `paths` trong tsconfig** |
| Python | `.py .pyi` | `import a.b`, `import a as b, c`, `from .x import y` | tương đối theo số dấu chấm; tuyệt đối thử gốc, `src/`, `lib/` |
| C, C++ | `.c .h .cc .cpp .cxx .hpp .hh .hxx .inl` | `#include "x"`, `#include <x>` | `"…"` theo thư mục file gọi rồi `include/`, `src/`, `lib/` |
| Rust | `.rs` | `use`, `pub use`, `use a::{b, c}`, `extern crate` | theo cây module; đọc `Cargo.toml` để biết gốc crate **và các crate anh em trong workspace** |
| Go | `.go` | `import "x"`, khối `import ( … )` | đọc `module` trong `go.mod` để tách nội bộ với bên ngoài |

Mọi ngôn ngữ dùng chung một lexer cấu hình được. Nó biết bỏ qua chú thích,
chuỗi, char literal, raw string, docstring và regex theo từng ngôn ngữ — đó là
chỗ dễ sai nhất, nên chỉ viết một lần.

## Giới hạn lớn nhất: cạnh liên ngôn ngữ vô hình

Phân tích import chỉ thấy được phụ thuộc **trong cùng một ngôn ngữ**. Các loại
sau không xuất hiện ở bất kỳ câu lệnh import nào, nên bộ kiểm chứng không thấy:

- **FFI** — Rust gọi C qua `extern "C"`, Python gọi qua `ctypes` hay `cffi`
- **Liên kết** — thư viện tĩnh và động, kể cả `dlopen`
- **Con trỏ hàm và vtable**
- **Tiến trình con, syscall, IPC, mạng**

Trong `examples/sensor`, cạnh `Decode -> Native` là FFI nên không bị bắt. Có
hẳn một test khoá lại điều đó, để giới hạn này không bị quên.

**Hệ quả cho mã hệ thống và bảo mật:** với C/C++, `#include` là điều kiện cần
nhưng rất xa điều kiện đủ. Loại luật phù hợp hơn cho các mảng đó là **hàm bị
cấm** — xem [api.md](api.md). Nó không có vùng mù này, vì một lời gọi `strcpy`
phải xuất hiện trong mã nguồn thì mới chạy được.

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
`deny * -> LibPcap except Capture` nghĩa là chỉ Capture được chạm libpcap.

Luật `#draft` và `#proposed` không có hiệu lực, nên cũng **không cấp quyền**.

## Thư viện bên ngoài

Chỉ tham gia luật khi được khai báo `external`, với tên định danh trong `at`:

```
external Stripe  at "stripe"                  # gói npm
external NumPy   at "numpy"                   # gói Python
external LibPcap at "pcap.h"                  # header hệ thống C
external GoNet   at "net"                     # gói chuẩn Go
```

Thứ không khai báo thì bị bỏ qua. Bắt khai báo mọi gói npm hay mọi header hệ
thống sẽ khiến file luật dài vô ích.

## Thứ không phân giải được thì báo ra

Quyết định có chủ ý. Bỏ sót âm thầm làm người dùng tin rằng không có vi phạm,
tệ hơn hẳn việc nói thẳng là chưa biết.

| Trường hợp | Xử lý |
| --- | --- |
| `@/lib/x` khi có `tsconfig.json` khai báo | phân giải theo `paths`; monorepo thì lấy cấu hình gần file gọi nhất |
| `@/lib/x` khi không có khai báo nào | **báo ra** |
| `#include "x.h"` không tìm thấy | **báo ra**, có thể nằm trong include path của build |
| `use crate::x` không có module | **báo ra** |
| `from .x import y` không có file | **báo ra** |
| `import numpy` không có trong dự án | coi là gói — với import tuyệt đối của Python, không phân biệt được |

## Giới hạn khác đã biết

- **Include path của C/C++ chưa đọc từ build.** CMake, Makefile và
  `compile_commands.json` đều chưa được đọc, nên chỉ xấp xỉ bằng vài thư mục
  thường gặp.
- **`flow`, `contract`, `boundary`, `invariant` kiểm được một phần** — xem
  [declarations.md](declarations.md). Thứ tự bước và nội dung ngữ nghĩa thì
  chưa.
- **Chỉ thị tiền xử lý không được diễn giải.** Include trong `#ifdef` vẫn được
  tính. Có chủ ý: đoán nhánh nào được biên dịch cần biết toàn bộ cờ build, và
  phân tích phụ thuộc nên thận trọng.
- **Bộ quét không phải parser đầy đủ.** Nhận regex bằng suy đoán theo token
  đứng trước, như các bộ minify. Nếu độ chính xác không đủ, thay `scan/` bằng
  tree-sitter — ranh giới module đã được đặt sẵn cho việc đó.

## Phạm vi quét

| Lệnh | Quét gì | Thời gian trên 2000 file |
| --- | --- | --- |
| `archctl check` | toàn repo | ~120ms |
| `archctl check --changed` | file đổi so với HEAD, hỏi git | ~24ms |
| `archctl check <file>...` | đúng những file đó | ~11ms |

Chia nhỏ được vì luật phụ thuộc, `api` và `store` đều xét **từng file một**.
Dạng thứ ba dành cho hook: nó đã biết file nào vừa đổi nên không cần hỏi git.

Nếu chính `.arch/system.arch` vừa đổi thì `--changed` tự chuyển sang quét toàn
bộ — luật đổi thì mọi file đều phải xét lại.

## Truy cập dữ liệu: `store owned_by`

```
store Captures owned_by Capture at "packets", "sessions"
```

Chỉ `Capture` được chạm hai bảng đó. Đây là một `deny` **ngầm**: nó không chờ
component vào diện kiểm, nhưng một `allow` viết tay vẫn thắng được nó.

Tên bảng **chỉ được dò trong chuỗi** — SQL thô, query builder, tên collection:

```
"SELECT ts FROM packets"      → bắt
knex('packets')               → bắt
db.collection('packets')      → bắt
struct S { packets: u64 }     → KHÔNG, đó là trường struct
"FROM packets_archive"        → KHÔNG, bảng khác
```

Dò cả trong định danh sẽ bắt được `prisma.packets`, nhưng cũng bắt nhầm mọi
biến và trường tên `packets`. Phân biệt hai thứ đó cần thông tin kiểu. Muốn
bắt cách truy cập theo thuộc tính của ORM thì khai báo nó thành nhóm `api` —
cỗ máy đó vốn để dò định danh.

## Mẫu `at` chồng nhau

File khớp nhiều component một lúc thì component khai báo trước thắng, và báo
cáo in cảnh báo `MẪU CHỒNG NHAU`. Nên sửa mẫu thay vì dựa vào thứ tự.

## Bỏ qua thư mục

Luôn bỏ qua, không cấu hình được: `.git`, `.arch`, `node_modules`, `target`,
`dist`, `build`, `out`, `.next`, `.nuxt`, `.svelte-kit`, `coverage`, `vendor`,
`__pycache__`, `.venv`. Symlink không được đi theo, để tránh vòng lặp và tránh
ra ngoài gốc dự án.

Thêm mẫu glob vào `.arch/.archignore` để bỏ qua thêm.
