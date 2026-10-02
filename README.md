# archctl

Sơ đồ kiến trúc là **luật**. AI viết mã. Bộ kiểm chứng đứng giữa.

Con người vẽ kiến trúc, luồng dữ liệu và ràng buộc. Agent lập trình có sẵn
(Claude Code, opencode, Codex) tự do viết mã, miễn không vi phạm. `archctl`
đối chiếu mã với sơ đồ và báo mọi chỗ đi chệch.

```bash
archctl init --from-code    # dựng bản nháp kiến trúc từ mã đã có
archctl check               # đối chiếu mã với luật
archctl serve               # mở sơ đồ tương tác ở 127.0.0.1:4000
```

## Nó bắt được gì

```
kiến trúc c8bd3bd9 · 15 file quét (5 Rust, 4 C/C++, 4 Python, 2 Go) · 7/8 component trong diện kiểm

✗ VI PHẠM  Native -> LibPcap  (bị cấm, #locked)
  native/parser.c:6  import 'pcap.h'
  luật: deny * -> LibPcap except Capture
  → gọi Capture thay vì truy cập thẳng LibPcap

✗ VI PHẠM  Native -> Unsafe  (bị cấm, #locked)
  native/parser.c:18  dùng `strcpy` (2 lần trong file)
  → Native chưa được phép dùng nhóm Unsafe

✗ VI PHẠM  ToolsLib -> Captures  (không phải chủ sở hữu, #locked)
  tools/lib/io.py:8  chạm bảng `packets`
  → gọi Capture thay vì chạm thẳng vào dữ liệu của nó
```

**Năm ngôn ngữ**: TypeScript/JavaScript, Python, C/C++, Rust, Go.

| Luật | Kiểm bằng |
| --- | --- |
| `allow` / `deny` giữa component | đồ thị import, phân giải theo từng ngôn ngữ |
| `api` — hàm và từ khoá bị cấm | dò định danh trên dòng token |
| `store owned_by` — truy cập dữ liệu | dò tên bảng trong chuỗi |
| `boundary ... contain` — lỗi không được lọt ra ngoài vùng | dò định danh ngoài vùng |
| `flow`, `contract`, `invariant state` | mọi tên khai báo phải tồn tại trong mã |
| trạng thái `#draft`…`#locked` | quyết định luật nào đang có hiệu lực |

Bốn khai báo cuối bắt đúng chế độ hỏng hay gặp nhất khi làm việc với agent:
**khai báo lạc hậu**. Agent đổi tên một bước, gỡ một năng lực — và sơ đồ im
lặng nói dối từ đó.

## Sơ đồ để thao tác, không chỉ để nhìn

`archctl serve` mở sơ đồ ở `127.0.0.1:4000`: kéo thả, khoá luật bằng một cú
bấm, duyệt đề xuất tại chỗ. Sơ đồ **tự cập nhật** khi agent sửa mã — nhìn là
biết nó đang làm tới đâu và chỗ nào đang hỏng.

Giao diện là một file HTML tự chứa, tự vẽ SVG, không framework và không gọi
mạng. Người dùng cuối chỉ cần đúng một binary.

`archctl export` xuất sơ đồ Mermaid tĩnh để commit vào repo hoặc dán vào mô tả
pull request; `--violations` phủ vi phạm thật lên trên.

## Agent có lối ra

Gặp luật sai, agent chỉ còn hai lựa chọn nếu không có gì khác: lách luật một
cách sáng tạo, hoặc kẹt vô hạn. Giao thức cho nó lối thứ ba.

```bash
archctl propose 'allow A -> B' --reason "..."   # agent đề xuất rồi DỪNG
archctl block --on 'deny * -> X' --reason "..." # agent báo bế tắc
archctl accept <id> --reason "..."              # người quyết định
archctl reject <id> --reason "..."
```

Agent **không bao giờ ghi** `system.arch` — nó chỉ ghi `proposals.arch`. Một
file một người ghi thì xung đột ghi đè và vượt quyền bị triệt tiêu bằng cấu
trúc, không phải bằng cơ chế phát hiện.

Mọi quyết định vào `decisions.md` kèm lý do bắt buộc. Agent không có trí nhớ
giữa các phiên, nên file đó chặn nó đề xuất lại thứ đã bị từ chối — và nó cũng
chính là ADR tự sinh ra trong lúc làm việc.

## Áp lên dự án đã có mã

```bash
archctl init --from-code   # đọc repo, dựng bản nháp, mọi thứ ở #draft
archctl baseline           # tha vi phạm có sẵn, chỉ chặn cái mới
```

Component nào có `allow` đang có hiệu lực thì component đó bắt đầu bị kiểm;
phần còn lại tạm để yên. Không cần mô tả cả repo trước khi có được giá trị nào.

## Chạy trong vòng lặp của agent

`archctl init` sinh sẵn hook chạy sau mỗi lần agent sửa file:

```json
{ "type": "command", "command": "archctl check --changed" }
```

| Phạm vi | 2000 file TypeScript |
| --- | --- |
| `archctl check` | ~120ms |
| `archctl check --changed` | ~24ms |
| `archctl check <file>...` | **~11ms** |

Và trong CI, nơi agent không với tới:

```yaml
- run: archctl check --strict
```

## Tính chất

| | |
| --- | --- |
| Dependency ngoài std | **0** |
| Test | 197 |
| Binary | 832 KB |
| Khởi động | 3ms |

Không gói ngoài nào cả — kể cả bộ quét import, bộ đọc JSON, máy chủ HTTP và
engine layout. Công cụ này chạy trong repo của người dùng và trong vòng lặp của
agent, nên mỗi dependency thêm vào là một bề mặt tấn công thêm.

## Tài liệu

| | |
| --- | --- |
| [docs/check.md](docs/check.md) | Ngữ nghĩa kiểm chứng, phân giải từng ngôn ngữ, giới hạn |
| [docs/declarations.md](docs/declarations.md) | `flow`, `contract`, `boundary`, `invariant` |
| [docs/api.md](docs/api.md) | Hàm và từ khoá bị cấm |
| [docs/protocol.md](docs/protocol.md) | Giao thức agent–người |
| [docs/adopt.md](docs/adopt.md) | `init --from-code` và `baseline` |
| [docs/serve.md](docs/serve.md) | Sơ đồ tương tác |
| [docs/diff.md](docs/diff.md) | Tách thay đổi thật khỏi thay đổi hình thức |
| [docs/toolchain.md](docs/toolchain.md) | Cài và gỡ Rust |

Bản đặc tả đầy đủ:
https://claude.ai/code/artifact/2b26df5d-d6b0-40d0-a398-7e574c7d1387

## Giới hạn đã biết

- **Cạnh liên ngôn ngữ vô hình.** FFI, liên kết thư viện, con trỏ hàm, IPC đều
  không xuất hiện trong câu lệnh import nào. Với C/C++ thì `#include` là điều
  kiện cần nhưng rất xa điều kiện đủ — đó là lý do có `api`.
- **Thứ tự bước trong `flow` chưa kiểm.** Bộ quét không phân biệt được lời gọi
  với định nghĩa hàm, nên kiểm thứ tự sẽ báo nhầm. Cần trace runtime.
  `requires`/`ensures` của `contract` và `invariant data`/`money` cũng vậy — và
  chúng vẫn bị cảnh báo là "chỉ là lời hứa" nếu thiếu `verify`.
- **Đây là lan can, không phải hàng rào an ninh.** Agent chạy được shell nên vẫn
  ghi đè được file luật. Cưỡng chế thật nằm ở CI, CODEOWNERS trên `.arch/**`, và
  việc tên người duyệt bị ghi vào nhật ký.

## Cấu trúc repo

```
archctl/
├── docs/         # tài liệu
├── templates/    # khung .arch/ mà `archctl init` sinh ra
├── examples/
│   ├── shop/     # dự án TypeScript, hai vi phạm cài sẵn
│   └── sensor/   # Rust + C + Python + Go, mỗi ngôn ngữ một vi phạm
└── crates/
    ├── arch-core/   # parser, mô hình, kiểm chứng, layout, xuất sơ đồ
    └── archctl/     # CLI và máy chủ sơ đồ
```

## Ghi công

Cách phân loại thay đổi trong `archctl diff` lấy ý tưởng từ
[tt-a1i/archify](https://github.com/tt-a1i/archify) (MIT). Hai dự án bổ sung
cho nhau: archify biến một spec thành sơ đồ đẹp, archctl đối chiếu spec với mã.
Không có dòng code nào được sao chép.
