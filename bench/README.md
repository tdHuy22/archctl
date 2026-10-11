# Bộ đo trên repo thật

```bash
python3 bench/bench.py
```

Một lệnh: dựng archctl bản release, tải hai mươi bốn repo ở đúng commit đã ghim
trong [repos.tsv](repos.tsv), rồi chạy hai phép thử trên từng repo. Chỉ cần
Python 3 và git, không gói Python nào ngoài thư viện chuẩn.

## Có những repo nào, và vì sao

Mỗi repo có mặt vì một cách bố trí mà fixture viết tay không nghĩ ra. Chọn sao
cho mỗi ngôn ngữ có ít nhất hai repo, và có cả thư viện lẫn ứng dụng chia tầng —
chỗ mà luật `allow`/`deny` giữa component có nghĩa nhất.

| Ngôn ngữ | Repo |
| --- | --- |
| TypeScript | vue core (monorepo, alias tsconfig), excalidraw (ứng dụng React, TSX) |
| Python | flask (`src/`), netbox (Django, gốc là thư mục có `manage.py`), scikit-learn (ML), vllm (LLM serving, kernel C++), langchain (LLM, hai mươi hai `pyproject.toml` trong `libs/`) |
| C | curl (CMake), redis (Makefile, `deps/`), nginx (`-I` trong script `auto/`, không file build nào đọc được) |
| C++ | leveldb (storage engine, CMake) |
| Rust | tokio, ripgrep, rusqlite, pydantic-core (với Python), zstd-rs (với C qua submodule) |
| Go | hugo (đồ thị dày), etcd (mười bốn `go.mod` trong một repo) |

Lần thêm tám repo cuối (etcd, excalidraw, langchain, leveldb, netbox, nginx,
scikit-learn, vllm) bắt được bốn chỗ archctl không thấy cạnh nào mà cũng
không báo gì: etcd, langchain, netbox, nginx đều ra 0 component vào diện kiểm.
Cả bốn đã sửa, lý do ở [docs/check.md](../docs/check.md).

Cột submodule trong `repos.tsv` liệt kê submodule cần kéo về. `--depth 1`
không kéo submodule, nên zstd-rs từng vào bộ đo không có dòng C nào dù được
chọn chính vì phần C vendor. Submodule được tải ở đúng commit repo cha ghi lại,
URL đọc từ `.gitmodules` và cũng phải là `https://`.

Cột sparse cho monorepo quá lớn để tải hết: chỉ những thư mục liệt kê (cùng
file nằm thẳng ở gốc) được tải. google-cloud-python đủ là 1,3 GB và 45 nghìn
file Python; mười một package của nó là 83 MB. Phép đếm số file bỏ những file
git đánh dấu nằm ngoài vùng sparse, và một thư mục sparse không có ở commit
đã ghim là mã thoát 2 — không thì một tên gõ sai cho ra một repo rỗng mà vẫn
đạt.

Repo tải về `target/bench/repos/` (khoảng 920 MB), bản làm việc ở
`target/bench/work/`, kết quả chi tiết ở `target/bench/results.json`. Tất cả
nằm trong `target/`: ngoài git, bộ quét của archctl bỏ qua, `cargo clean` dọn.

## Hai phép thử, cả hai đều chặn

**Tự nhất quán** ([consistency.py](consistency.py)). `init --from-code` đọc mã
rồi sinh luật mô tả đúng hiện trạng. Nâng mọi thứ lên `#accepted`, `check` lại
chính mã đó: phải ra **0 vi phạm**. Mỗi vi phạm còn lại là một chỗ bộ khám phá
và bộ kiểm chứng nhìn cùng một đoạn mã mà thấy hai điều khác nhau — lỗi của
archctl, không phải của repo.

Và phải ra **0 cảnh báo trong file luật**: `init` khuyên chạy `check --strict`
trong CI, mà `--strict` coi cảnh báo của file luật là vi phạm. Luật vừa sinh ra
đã có cảnh báo thì lời khuyên đó đổ ngay lần chạy đầu tiên của người dùng mới.

Và archctl phải quét **đúng số file nguồn mà git có**. Số này đếm độc lập từ
`git ls-files`, trừ symlink và thư mục archctl luôn bỏ qua; thư mục tên output
(`build`, `dist`, `out`, `coverage`) chỉ bị trừ khi `git check-ignore` nói nó
(hay mọi thứ trong nó) bị ignore, hoặc khi không có `.gitignore` nào để hỏi. Lệch là archctl bỏ sót mã được commit: etcd từng mất cả package Go
`tests/robustness/coverage/` vì tên thư mục trùng tên output, không một lời nào.

Và luật sinh ra phải có **các cạnh liên ngôn ngữ đã biết**, mỗi họ ghép một
cạnh (`CROSSINGS` trong [consistency.py](consistency.py)):

| Repo | Cạnh | Qua |
| --- | --- | --- |
| libbpfgo | `Core -> Libbpf` | cgo |
| pydantic-core | `PydanticCore -> Core` | module PyO3 `_pydantic_core` |
| matplotlib | `Lib -> Src` | module pybind11 `_path` |
| vllm | `Vllm -> Csrc` | op PyTorch `torch.ops._C.*` |

Đổi `allow` đó thành `deny` thì `check` phải báo vi phạm qua ranh giới ngôn
ngữ. Tự nhất quán không bắt được chỗ này: phần ghép hỏng ở cả `init` lẫn
`check` thì hai bên vẫn khớp nhau, và cạnh biến mất trong im lặng.

**Luồng lệnh** ([flows.py](flows.py)). Mọi lệnh — `init`, `check` theo bốn
đường, `parse`, `export`, `baseline`, `propose`/`accept`/`block`/`reject`,
`diff`, `serve` — trên mã người khác viết. Test trong `crates/*/tests/` dựng dự
án nhỏ viết riêng để test; ở đây là thư mục vendor, symlink, file sinh tự động
mà fixture không nghĩ ra.

## Mã thoát

| | |
| --- | --- |
| 0 | cả hai phép thử đạt trên mọi repo |
| 1 | archctl sai: có bất nhất, có cảnh báo trong luật sinh ra, hoặc một luồng lệnh hỏng |
| 2 | không chạy tới được: không tải được repo, hoặc build hỏng |

Tách 1 khỏi 2 để một lần mạng chập chờn trên CI không trông giống một lỗi của
archctl.

## Phép thử thứ ba: đối chiếu với parser chuẩn

```bash
python3 bench/oracle.py
```

Tự nhất quán so archctl với chính nó: bộ khám phá và bộ kiểm dùng chung một
bộ trích, nên một lỗi trong bộ trích làm cả hai sai giống nhau mà vẫn ra
"0 bất nhất". [oracle.py](oracle.py) so import mà `archctl scan --json` đọc
được với parser chuẩn của từng ngôn ngữ, trên các repo `bench.py` đã tải:

- Python: `ast` của chính trình thông dịch chạy script. File không parse được
  (cú pháp mới hơn bản Python đang chạy, Python 2) được đếm riêng.
- C/C++: lexer thô của clang, không tiền xử lý — giống archctl, include trong
  nhánh `#if` nào cũng tính.
- JS/TS: parser module của Node, TS gỡ kiểu trước bằng
  `module.stripTypeScriptTypes`. Chỉ import tĩnh, nên chỉ kiểm phía thiếu.
- Go: `go/parser` ở chế độ `ImportsOnly`, dựng một lần thành binary tạm. Không
  xét build tag, giống archctl.
- Rust: `syn`, ghim cứng phiên bản, dựng dưới `target/bench/syn-oracle/`. Phải
  tải crate nên chỉ chạy khi `--require syn` (CI bật). `syn` không đọc vào thân
  macro, nên import archctl thấy trong `macro_rules!`, `cfg_rt! { }`... được
  đếm riêng, không tính là thừa.

Python, C/C++, Go và Rust đòi khớp cả tên lẫn số dòng: thiếu, thừa hay lệch dòng đều
là lệch. Thiếu clang, Node hay Go thì phần đó in `BỎ QUA`; `--require
clang,node,go` biến việc thiếu thành mã thoát 2, và với Node thì đòi cả khả năng
gỡ kiểu TS. Thước đo tự hỏng cũng là mã 2 chứ không phải "đạt": clang, Node hay
trình đọc Go thoát khác 0, không trả lời đủ số file, hoặc quá nửa số file của một repo không parse được.

Lần đầu chạy (2026-10-04) nó bắt những lỗi mà cả hai phép thử kia đều cho qua:
Vue core mất 363/1 728 import tĩnh vì `import { type A, b }`, Python có 1 045
import giả từ `raise E from e` và `yield from x` cộng 15 import thật bị nuốt
ngay sau chúng, 11 import Python báo sai dòng. Chi tiết ở
[docs/check.md](../docs/check.md#đối-chiếu-với-parser-chuẩn).

## Vì sao ghim commit

Không ghim thì con số trôi theo upstream: chạy lại hôm sau cho kết quả khác mà
không biết do archctl đổi hay do repo đổi. Tải theo SHA, `--depth 1`, kiểm lại
`HEAD` sau khi checkout. Nâng một repo lên bản mới là sửa một dòng trong
`repos.tsv` — và là một thay đổi đáng nhìn riêng trong lịch sử git.

Git chạy với `core.hooksPath=/dev/null`, `core.fsmonitor=` và
`protocol.file.allow=never`: repo tải về là mã người khác, không để nó chạy gì
qua cấu hình git. Cùng danh sách mà archctl tự dùng.

## Thời gian chỉ in, không chặn

```bash
python3 bench/bench.py --only hugo,tokio --rounds 9
```

Trung vị của `--rounds` lần chạy, bỏ lần đầu vì bộ nhớ đệm hệ thống file. Trên
máy CI dùng chung con số nhiễu tới mức chặn theo nó chỉ tạo ra đỏ giả, nên CI
chạy với `--rounds 1` và không đọc cột thời gian.

## Mỗi phép kiểm phải đổ được

```bash
python3 bench/sabotage.py
```

Dựng một archctl giả, gọi archctl thật rồi làm hỏng đúng một chỗ: thêm một vi
phạm, thêm một cảnh báo vào luật sinh ra, báo quét 0 file, báo quét thiếu một
file, bỏ khoá
`lawWarnings`, giữ nguyên hash khi luật đổi, nói `diff` có thay đổi khi chỉ
thêm chú thích. Mỗi kiểu phải làm `bench.py` thoát 1, **và phải bị đúng phép
kiểm dành cho nó bắt** — không chỉ đỏ vì một lý do khác. Ba kiểu nữa làm hỏng
đầu ra `scan --json` — mất một import, lệch một dòng, thêm một import giả — và
phải làm `oracle.py` thoát 1. Kiểu `crossing` xoá cạnh liên ngôn ngữ khỏi
cả luật `init` sinh ra lẫn vi phạm `check` báo, trên pydantic-core, và chỉ
danh sách cạnh bắt buộc ở trên bắt được nó. `--rust-repo ripgrep` thêm ba kiểu y vậy ở file
Rust, đối chiếu bằng `syn` nên cần mạng; kiểu thêm import giả nằm ngoài macro,
nên nó cũng giữ cho phép miễn trừ import trong thân macro không rộng ra. CI
chạy bước này sau bộ đo, với `--rust-repo ripgrep`.

Nó có mặt vì bộ đo này từng nằm ngoài git, và lúc đưa vào, đọc lại thì bốn
phép kiểm không kiểm gì: một cái chỉ xét file có tồn tại vì vế sau của `or`
không bao giờ được chạy tới, một cái chấp nhận `"0 "` ở bất cứ đâu trong đầu
ra, một cái gọi `export --mermaid` — cờ không tồn tại — mà không ai đọc mã
thoát, một cái đúng ra phải ghi thêm một dòng nhưng có thể xoá trắng file
luật. Rồi khi chuyển vào `target/`, bộ đếm file đếm ra 0 ở cả mười repo vì
đường dẫn tuyệt đối nào cũng chứa `target`. Giờ "0 vi phạm trên 0 file" là
một lỗi chứ không phải một lần đạt.

Một phép kiểm không thể đổ thì tệ hơn không có: nó vẫn cộng vào con số đạt.
