# `archctl check` — ngữ nghĩa và giới hạn

Hiện kiểm **luật phụ thuộc giữa component** cho năm ngôn ngữ.

## Ngôn ngữ đã hỗ trợ

| Ngôn ngữ | Phần mở rộng | Nhận được | Phân giải |
| --- | --- | --- | --- |
| TypeScript, JavaScript | `.ts .tsx .mts .cts .js .jsx .mjs .cjs` | `import`, `import type`, `export … from`, `require()`, `import()` | tương đối, `index.*`, `.js`→`.ts`, alias `paths` trong tsconfig, **gói anh em trong workspace npm** |
| Python | `.py .pyi` | `import a.b`, `import a as b, c`, `from .x import y` | tương đối theo số dấu chấm; tuyệt đối thử gốc, `src/`, `lib/`; **gói không gian tên (PEP 420)** |
| C, C++ | `.c .h .cc .cpp .cxx .hpp .hh .hxx .inl` | `#include "x"`, `#include <x>` | **thư mục `-I` đọc từ build**; `"…"` ưu tiên thư mục file gọi |
| Rust | `.rs` | `use`, `pub use`, `use a::{b, c}`, `extern crate` | theo cây module; đọc `Cargo.toml` để biết gốc crate **và các crate anh em trong workspace** |
| Go | `.go` | `import "x"`, khối `import ( … )` | đọc `module` trong `go.mod` để tách nội bộ với bên ngoài |

Mọi ngôn ngữ dùng chung một lexer cấu hình được. Nó biết bỏ qua chú thích,
chuỗi, char literal, raw string, docstring và regex theo từng ngôn ngữ — đó là
chỗ dễ sai nhất, nên chỉ viết một lần.

## Cạnh liên ngôn ngữ

Phân tích import chỉ thấy phụ thuộc **trong cùng một ngôn ngữ**. Rust gọi C
không qua câu lệnh import nào.

Nhưng một phần lớn FFI viết tay là **khai báo tường minh**:

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

Hai đầu mang cùng một cái tên, và cái tên đó phải khớp thì chương trình mới
liên kết được. Ghép chúng lại là ra cạnh — không cần đọc binary, không cần
bảng ký hiệu:

```
✗ VI PHẠM  Decode -> Native  (không được phép, #accepted)
  src/decode/ethernet.rs:4  gọi `parse_ethernet` qua ranh giới ngôn ngữ
```

| Bên gọi | Nhận bằng |
| --- | --- |
| Rust | `extern "C" { fn x; }` — ABI nào cũng được |
| Go | `import "C"` rồi `C.x` |
| Bên cung cấp | |
| C, C++ | định nghĩa hàm ở mức ngoài cùng |
| Rust | `pub extern "C" fn x` |

Ghép theo tên, nên nó thận trọng ở mấy chỗ:

- **Khai báo trước không phải là cung cấp.** `int f(void);` kết thúc bằng `;`
  chứ không mở thân hàm.
- **Lời gọi lồng trong thân hàm khác không phải là định nghĩa.** Chỉ xét khi
  độ sâu ngoặc nhọn bằng 0.
- **Hai component cùng cung cấp một tên thì bỏ cả hai.** Không biết chọn đâu,
  mà đoán sai sinh ra một cạnh không có thật.
- **Không ghép được thì im lặng.** `memcpy` nằm trong libc, không có định
  nghĩa nào trong repo. FFI tới thư viện hệ thống là chuyện thường.
- **`C.CString`, `C.int` của cgo tự rụng**, vì chúng không khớp định nghĩa nào
  trong repo.

### Vẫn còn vô hình

- **Liên kết động** — `dlopen` cộng tên hàm dựng lúc chạy
- **Con trỏ hàm và vtable**
- **`ctypes` và `cffi` của Python** — tên hàm nằm trong chuỗi, và thư viện
  được chọn lúc chạy
- **Tiến trình con, syscall, IPC, mạng**

`archctl check --changed` chỉ ghép trong tập file đang xét, nên nó có thể bỏ
sót cạnh liên ngôn ngữ khi phía cung cấp không nằm trong tập. Lần quét đầy đủ
— và CI — vẫn thấy.

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
| `@acme/core` khi `packages/core/package.json` khai tên đó | gói anh em trong workspace — phụ thuộc nội bộ, không phải gói ngoài |
| `#include "x.h"` không tìm thấy | **báo ra**, có thể nằm trong include path của build |
| `#include <x.h>` nằm trong một thư mục `-I` của dự án | file của dự án — đúng như trình biên dịch sẽ thấy |
| `use super::Thing` | module cha — `Thing` là item chứ không phải module |
| `use crate::x` không ra file nào | `mod` viết lồng trong file; không có cạnh, và **không báo** — trình biên dịch đã bảo đảm đường dẫn tồn tại |
| `from .x import y` không có file nhưng có thư mục | gói không gian tên (PEP 420) — thư mục chính là gói |
| `from .x import y` không có cả thư mục | **báo ra** |
| `import numpy` không có trong dự án | coi là gói — với import tuyệt đối của Python, không phân biệt được |

## Hoa thường phải khớp, trên mọi máy

`import './Util'` khi trên đĩa chỉ có `util.ts` thì **không** phân giải được —
cả trên macOS, nơi ổ mặc định không phân biệt hoa thường và `is_file()` sẽ trả
lời "có". Mọi ngôn ngữ đã hỗ trợ đều phân biệt hoa thường khi import (Python
kể cả trên macOS), nên cách viết có thật trên đĩa là cách viết duy nhất được
nhận: sau khi hệ thống file nói "có", archctl đối chiếu từng đoạn đường dẫn với
tên thật trong thư mục.

Trước đây kết quả phụ thuộc máy. Cùng một commit `rusqlite`, `init
--from-code` sinh 12 luật `allow` trên macOS và 11 trên CI Linux: `use
crate::{Error, Result}` được "tìm thấy" là file `src/Error.rs`, trong khi file
thật là `src/error.rs` và đoạn mã kia trỏ tới một kiểu chứ không phải một
module. Luật sinh ở máy này đem sang máy kia thành một `allow` không bao giờ áp
được — `--strict` chặn — hoặc một vi phạm mà máy kia không thấy.

Giá: 1–6% thời gian trên cả repo (đo xen kẽ, trung vị 21 lần), không đáng kể
ở đường hook. Lần dò trượt — phần lớn — vẫn dừng ngay ở `is_file()`; chỉ lần
dò trúng mới phải đọc thư mục, và mỗi thư mục chỉ đọc một lần.

## Include path của C/C++

`#include "aes.h"` không nói header nằm đâu — đường dẫn thật nằm trong cờ `-I`
của build. Đoán bằng vài tên thư mục thường gặp thì bỏ sót, và mất một cạnh
phụ thuộc là mất một vi phạm thật: `archctl check` sẽ in "0 vi phạm" trong khi
mã đang vi phạm.

Đọc theo thứ tự tin cậy giảm dần:

| Nguồn | Lấy gì |
| --- | --- |
| `compile_commands.json` | `-I`, `-isystem`, `-iquote`, `-idirafter` trong `arguments` hoặc `command`; đường dẫn tính từ `directory` của từng mục |
| `CMakeLists.txt`, `*.cmake` | `include_directories`, `target_include_directories`; mở `${CMAKE_SOURCE_DIR}` và `${CMAKE_CURRENT_SOURCE_DIR}` |
| `Makefile`, `*.mk` | mọi `-I` trên mọi dòng, sau khi mở biến `$(VAR)` |

**Không chạy `cmake` hay `make`.** Đọc file build của một repo lạ thì an toàn,
thực thi nó thì không.

Hai chỗ cố tình thận trọng, vì một đường dẫn đoán sai sinh ra **cạnh sai**, và
báo động giả tốn lòng tin nhiều hơn bỏ sót:

- **Biến chưa biết thì bỏ.** `${SOMETHING_ELSE}/inc` không sinh ra gì.
  `${CMAKE_BINARY_DIR}` cũng vậy: đó là header sinh ra lúc build, không có
  trong cây nguồn.
- **Hậu tố tuyệt đối phải khớp ít nhất hai đoạn.** `compile_commands.json`
  sinh trong container mang tiền tố không khớp gốc hiện tại, nên phải khớp
  theo hậu tố — nhưng nếu chấp nhận một đoạn thì `/usr/include` cũng khớp
  `include/` của repo.

Repo không có file C nào thì không phải quét tìm `Makefile`: phần này đọc muộn.

## Báo cáo không được dài tới mức không ai đọc

Cảnh báo lặp lại được gộp theo tên, và mỗi loại in tối đa mười hai mục:

```
⚠ CHƯA PHÂN GIẢI  'test/jemalloc_test.h'  (132 chỗ)
  deps/jemalloc/test/analyze/prof_bias.c:1  — không tìm thấy header…

⚠ … còn 17 tên nữa chưa phân giải được — xem `--json`
```

Trên một cây 5078 file gộp từ mười repo thật, một lần `check` từng in ra 7247
dòng; giờ là 49. redis đi từ 719 xuống 43, hugo từ 960 xuống 44. Danh sách dài
tới mức đó thì người đọc bỏ qua cả những dòng đáng đọc, nên cắt bớt làm nó hữu
ích hơn chứ không phải kém đi.

`--json` vẫn đưa đủ, không cắt gì.

## Ba nhóm cảnh báo, và `--strict` chặn hai nhóm

Mọi cảnh báo đều được in, và đều vào con số ở dòng tổng kết. Nhưng chúng không
cùng một loại, nên `--strict` không đối xử như nhau.

**Đã thử kiểm và không kiểm được** — `--strict` chặn. Tên không phân giải
được, file quá 4 MB, khai báo lạc hậu, `allow` không bao giờ áp được vào đâu,
mẫu `at` chồng nhau, biên nhận hết hạn. Mỗi cái là một chỗ báo cáo đang im
lặng mà không phải vì sạch.

**Luật không ai kiểm được** — `--strict` chặn. `contract` không có verify,
`invariant` chỉ là ghi chú, component không có `at`. Nhóm này đáng chặn hơn
nhóm trên: một import không phân giải được chỉ là một chỗ tối, còn một
`contract` không có verify là một luật **có mặt trong file**, đọc lên y như
luật đang được áp — ai xem sơ đồ cũng tưởng nó đang được giữ.

**Chưa khai báo** — `--strict` **không** chặn. Component chưa có `allow` nào,
file không thuộc component nào. Áp dần là cách dùng được thiết kế: `init
--from-code` sinh ra toàn `#draft`, và một repo đang chạy thì luôn có phần
chưa vào sơ đồ. Chặn nhóm này nghĩa là không ai áp được `--strict` cho tới khi
đã mô tả xong cả repo, và chẳng ai làm thế. Xem [adopt.md](adopt.md).

Trước đây nhóm thứ hai chỉ được in chứ không được đếm: dòng cuối nói "0 cảnh
báo" trong khi ba dòng ⚠ nằm ngay phía trên, và `--strict` trong CI cho qua
một file luật đầy luật không ai kiểm.

## Giới hạn tài nguyên

File trên **4 MB** không được dò, và được báo ra:

```
⚠ QUÁ LỚN  src/web/bundle.min.js (30 MB) — không dò
  → file sinh ra hay đã minify thì thêm vào .arch/.archignore
```

Bộ dò giữ cả dãy token trong bộ nhớ, nên một bundle đã minify 32 MB ngốn hơn
một gigabyte. Trong CI có hạn mức bộ nhớ thì đó là một lần bị giết, và triệu
chứng không chỉ ra được nguyên nhân. Một file nguồn do người viết gần như
không bao giờ tới mức đó; file sinh ra ở cỡ đó thì không chứa thông tin kiến
trúc nào.

Mẫu glob cũng bị chặn: bộ khớp chỉ giữ một điểm quay lui nên không có mẫu nào
làm nó chạy theo hàm mũ. Điều này quan trọng vì mẫu có thể do agent đề xuất
rồi được `archctl accept` ghi vào file luật.

## Giới hạn khác đã biết

- **`flow`, `contract`, `boundary`, `invariant` kiểm được một phần** — xem
  [declarations.md](declarations.md). Thứ tự bước và nội dung ngữ nghĩa thì
  chưa.
- **Chỉ thị tiền xử lý không được diễn giải.** Include trong `#ifdef` vẫn được
  tính. Có chủ ý: đoán nhánh nào được biên dịch cần biết toàn bộ cờ build, và
  phân tích phụ thuộc nên thận trọng.
- **Bộ quét không phải parser đầy đủ.** Nhận regex bằng suy đoán theo token
  đứng trước, như các bộ minify. Nếu độ chính xác không đủ, thay `scan/` bằng
  tree-sitter — ranh giới module đã được đặt sẵn cho việc đó.
- **Rust: item ở gốc crate không sinh cạnh, và không được báo.** `use
  crate::{Connection, Result}` trỏ vào `lib.rs`, nhưng bộ phân giải chỉ thử
  `src.rs` và `src/mod.rs` cho gốc crate, rồi coi là `mod` viết lồng trong file.
  Ở `rusqlite`, `src/blob/` dùng `Connection` của Core mà không có cạnh `Blob ->
  Core` nào — một `deny Blob -> Core` sẽ không bắt được nó. Sửa đúng phải biết
  gốc crate của từng file (`src/lib.rs`, `src/main.rs`, mỗi file trong
  `src/bin/`, `tests/`, `examples/` là một crate riêng), và sẽ sinh thêm cạnh
  tới component chứa `lib.rs` ở hầu hết repo Rust — kể cả luật của chính
  archctl. Chưa làm.

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
