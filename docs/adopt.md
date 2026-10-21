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
| Gốc mã nguồn | Thư mục mà chia theo con của nó thì lộ ra **nhiều cấu trúc nhất** — xem dưới |
| Component | Mỗi thư mục con ngay dưới gốc. File nằm thẳng trong gốc gom thành một component riêng |
| Thư mục bên cạnh | Thư mục ngoài gốc mà một cạnh liên ngôn ngữ nối vào bên trong — xem dưới |
| Tên | `create-order` → `CreateOrder`. Trùng thì đánh số |
| Quan hệ | Import **thật** trong mã, chỉ giữa hai component. Thư viện ngoài bị bỏ qua |
| Giới hạn | Tối đa 24 component; phần dư liệt kê trong chú thích cuối file |

### Chọn gốc mã nguồn

Thước đo là **số file không nằm trong nhóm lớn nhất**. Nó phạt cả hai kiểu hỏng
cùng lúc: gốc quá sâu thì phủ được ít file, còn gốc quá nông thì một nhóm nuốt
gần hết và cũng chẳng nói lên gì.

Thử trên tám repo thật:

| Repo | Gốc chọn được | Component |
| --- | --- | --- |
| hugo (943 file Go) | gốc repo | 24 |
| ripgrep | `crates/` | 11 |
| vue core | `packages/` | 13 |
| tokio | gốc repo (10 crate) | 10 |
| flask | `src/flask/` | 3 |
| curl | `lib/` | 7 |
| zstd-rs | gốc repo | 4 |
| rusqlite | `src/` | 6 |

Hai thứ không tính **khi chọn bố cục** — nhưng vẫn được kiểm bình thường sau
đó:

- **Test, ví dụ, tài liệu, `deps/`, `vendor/`.** Nếu bỏ chúng đi mà còn lại
  quá ít thì repo này chủ yếu *là* test, và khi đó bỏ chúng chỉ làm mất
  thông tin.
- **Ngôn ngữ không phải của dự án này**, theo manifest ở gốc repo
  (`Cargo.toml`, `go.mod`, `package.json`, `pyproject.toml`). `zstd-rs` có 45
  file Rust và 279 file C, mà C là một bản sao zstd nhúng sẵn — chọn theo số
  file thì gốc rơi vào `zstd-safe/zstd-sys/zstd/` và toàn bộ mã Rust nằm
  ngoài sơ đồ. Manifest nói thẳng đây là dự án gì, nên không phải đoán. Repo
  không có manifest nào, như redis và curl, thì bỏ qua bước này.

### Mã native bên cạnh gốc

vllm chọn gốc `vllm/`, còn kernel C++ và CUDA nằm ở `csrc/` bên cạnh. Python
gọi chúng bằng `torch.ops._C.x`, không có câu import nào. Chia theo gốc thì
cả tầng kernel nằm ngoài sơ đồ. pydantic-core thì ngược lại: gốc là Rust ở
`src/`, còn gói Python ở `python/pydantic_core/` import module mà Rust định
nghĩa.

Nên một thư mục bên cạnh gốc được đưa vào khi có một
[cạnh liên ngôn ngữ](check.md#cạnh-liên-ngôn-ngữ) nối nó với một component bên
trong. Nó cạnh tranh suất trong 24 component theo số file như mọi thư mục khác.
Thư mục chỉ bọc đúng một thư mục con thì đi xuống một cấp, để
`python/pydantic_core/` thành `PydanticCore` chứ không thành `Python`.

Đổi lại, phần mã mới vào sơ đồ cũng được kiểm. Với vllm, `#include
"cutlass/…"` trong `csrc/` thành 227 cảnh báo chưa phân giải mới: CUTLASS do
CMake tải lúc build, không có trong repo. Cảnh báo đó đúng, và báo cáo gom nó
theo tên header.

Đây không phải cách "gộp anh em của gốc" đã bị loại. Gộp mọi thư mục bên cạnh
sẽ kéo `tests/` vào sơ đồ ở mọi repo. Ở đây chỉ thư mục có cạnh liên ngôn ngữ
được tính, và test, ví dụ, tài liệu, vendor không bao giờ được tính, dù
`tests/` của vllm cũng gọi `torch.ops._C`. Import thường cũng không tính: nếu
hai thư mục cùng ngôn ngữ quan trọng thì gốc đã bao cả hai.

### Không chia nhỏ hơn ranh giới đã có cưỡng chế

`Cargo.toml`, `package.json`, `go.mod` và `pyproject.toml` khai ra đâu là một
package. Ranh giới đó mạnh hơn mọi phỏng đoán theo thư mục, vì nó **có cưỡng
chế thật**: một crate không gọi vào trong crate khác nếu không khai báo phụ
thuộc.

Nên những thư mục nằm trong một package đã khai báo không được xét làm gốc.
`tokio/src/{runtime,io,fs,sync,net}` nhìn rất giống kiến trúc, và thước đo ở
trên chọn đúng chỗ đó — nhưng khi ấy `tokio-util` và `tokio-stream`, hai crate
riêng, rơi hết ra ngoài sơ đồ. Với luật này, tokio ra 10 crate và **0% file
nằm ngoài** thay vì 53%.

Package ở **gốc repo** không tính: nó bao cả repo nên nếu tính thì không bao
giờ chia nhỏ được gì.

Khi không đoán được chắc chắn thì nó **không đoán** — dùng gốc repo và nói ra.

### Nói ra phần chưa phủ

Số component không cho biết bản nháp phủ được bao nhiêu repo:

```
Dựng được 3 component và 6 quan hệ, tất cả ở `#draft`.
⚠ 59 file (71%) chưa thuộc component nào — bản nháp mới phủ một phần repo.
```

Một lần chạy trên `zstd-rs` từng in ra "7 component" trong khi toàn bộ mã
Rust của dự án nằm ngoài sơ đồ, và không có gì nói điều đó ra.

### Luật sinh ra phải tự nhất quán

Luật được dựng **từ chính mã này**, nên nâng mọi thứ lên `#accepted` rồi kiểm
lại phải ra **0 vi phạm**. Mỗi vi phạm còn lại là một chỗ bộ khám phá và bộ
kiểm chứng nhìn khác nhau về cùng một import — tức là một lỗi, không phải một
phát hiện.

Bất biến này được kiểm trên ba mươi hai repo thật, ghim theo commit, bằng
một lệnh chạy lại được — `python3 bench/bench.py`, xem
[bench/README.md](../bench/README.md) — và CI chạy nó ở mỗi pull request. Phần
lõi của nó còn có test giữ lại trong `tests/discover.rs`.

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
