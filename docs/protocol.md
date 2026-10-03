# Giao thức agent–người

Ba thứ đã làm trước đó — `check`, `diff`, `api` — đều ở phía **phát hiện**.
Giao thức này là phía **phản hồi**: nó cho agent một lối ra khi gặp luật sai.
Không có nó, agent gặp luật chặn chỉ còn hai lựa chọn, và cả hai đều tệ: lách
luật một cách sáng tạo, hoặc kẹt vô hạn.

## Một file, một người ghi

| File | Ai ghi | Qua lệnh |
| --- | --- | --- |
| `system.arch` | người | `accept` |
| `proposals.arch` | agent | `propose`, `block` |
| `decisions.md` | cả hai | `accept`, `reject` |
| `.state` | archctl | `check` |

Agent **không bao giờ ghi `system.arch`**. Một file một người ghi thì không còn
xung đột ghi đè và không còn vượt quyền — triệt tiêu bằng cấu trúc, không phải
bằng cơ chế phát hiện.

## Thang leo thang

```
1. archctl check báo đỏ
2. Agent SỬA MÃ cho đúng luật              ← mặc định
3. Không sửa được sau 3 lần?
   thiếu allow  → archctl propose 'allow A -> B' --reason "..."
   có luật chặn → archctl block --on 'deny ...' --reason "..."
   rồi DỪNG phần việc đó lại
4. Người: archctl accept <id> --reason "..."
          archctl reject <id> --reason "..."
```

Không có nhánh "agent tự nới luật cho vừa mã mình viết".

## Bộ đếm lần thử

`.arch/.state` nhớ mỗi vi phạm đã gặp lại bao nhiêu lần. Sửa xong thì bộ đếm
về không. Tới lần thứ ba, `check` in ra đúng lệnh cần chạy:

```
✗ VI PHẠM  Native -> Unsafe  (bị cấm, #accepted)
  native/parser.c:18  dùng `strcpy` (2 lần trong file)
  → Native chưa được phép dùng nhóm Unsafe
  ! đã gặp lại 3 lần — không sửa được thì dừng lại và:
    archctl block --on 'deny * -> Unsafe' --reason "..."
```

Gợi ý khác nhau theo loại vi phạm. `deny` có một luật cụ thể để báo bế tắc;
`NoAllow` thì **không có luật nào chặn cả** — thứ còn thiếu là một `allow`, nên
lối ra đúng là `propose`.

## Nhật ký quyết định: bộ nhớ dài hạn của agent

Agent không có trí nhớ giữa các phiên làm việc. Người từ chối một đề xuất hôm
nay thì ngày mai nó đề xuất lại y hệt, rồi ngày kia nữa. Đây là thứ gây bực
nhất khi dùng thực tế, và `decisions.md` tồn tại để chặn đúng việc đó.

```
$ archctl propose 'allow Capture -> Report' --reason "vẫn cần số liệu ngay"

Đề xuất này đã bị từ chối ngày 2026-10-02 bởi tdHuy22.
Lý do: Capture không được biết tới Report. Đẩy số liệu qua Decode.

→ Đừng đề xuất lại. Tìm hướng khác, hoặc hỏi người dùng.
```

Danh tính đề xuất là hash của nội dung sau khi chuẩn hoá khoảng trắng, nên viết
lại cùng một luật theo cách khác vẫn bị nhận ra.

Phụ phẩm: `decisions.md` chính là ADR (Architecture Decision Record) **tự sinh
ra trong lúc làm việc**, thứ bình thường không ai chịu ngồi viết.

## Mọi quyết định đều phải có lý do

`--reason` bắt buộc cả khi **duyệt**, không chỉ khi từ chối. Ma sát phải tương
xứng với mức độ của hành vi: duyệt một thay đổi luật là hành vi quản trị, không
phải một cú bấm.

Người quyết định được lấy từ `git config user.name` và ghi vào nhật ký, nên một
quyết định do agent tự bấm vẫn hiện ra trong review.

## Đề xuất được kiểm cùng với luật hiện có

`allow Capture -> Report` chỉ có nghĩa nếu cả hai component đã được khai báo.
`propose` vì vậy đọc `system.arch` rồi ghép câu lệnh vào và kiểm cả cụm — kiểm
tách rời sẽ báo sai mọi tham chiếu.

Nhờ đó `proposals.arch` không bao giờ chứa thứ không parse được, và `accept`
không bao giờ làm hỏng file luật. `accept` vẫn kiểm lại lần nữa trước khi ghi,
và không ghi gì nếu kết quả hỏng.

## Mã thoát

| Mã | Nghĩa |
| --- | --- |
| 0 | xong, không vi phạm |
| 1 | có vi phạm luật đang có hiệu lực |
| 2 | DSL sai cú pháp hoặc tham chiếu không tồn tại |
| 3 | có đề xuất hoặc bế tắc đang chờ người quyết định |
| 64 | dùng lệnh sai |

Mã 3 quan trọng với agent: nó phân biệt "tôi đã báo và đang chờ" với "tôi thất
bại".

## Giới hạn: đây là lan can, không phải hàng rào an ninh

Agent chạy được lệnh shell, nên về lý thuyết nó vẫn ghi đè được `system.arch`,
hoặc tự chạy `archctl accept`. Không cơ chế nào trong repo ngăn được điều đó
một cách tuyệt đối.

Chốt chặn thật sự nằm ở nơi agent không với tới:

- **CI** — `archctl check --strict` chạy trên server, so với `system.arch` ở
  nhánh `main`
- **Pull request** — mọi thay đổi trong `.arch/` hiện trong diff; `archctl diff`
  tách riêng phần nới lỏng luật
- **CODEOWNERS** — `.arch/**` bắt buộc người cụ thể phê duyệt
- **Nhật ký** — tên người quyết định được ghi lại, nên duyệt khống vẫn lộ

Trong máy cá nhân, đây là sự **hợp tác**. Việc **cưỡng chế** thuộc về CI và quy
trình review, giống hệt cách một đội kiểm soát lẫn nhau.

## Ranh giới này dựa vào đâu

"Agent không ghi `.arch/system.arch`" là nền của cả giao thức, nên nó phải là
ràng buộc chứ không phải lời đề nghị. `archctl init` ghi vào
`.claude/settings.json`:

```json
"permissions": { "deny": [
  "Write(./.arch/system.arch)", "Edit(./.arch/system.arch)",
  "Write(./.arch/.verified)",   "Edit(./.arch/.verified)",
  "Write(./.git/config)",       "Edit(./.git/config)"
] }
```

`.git/config` nằm trong danh sách vì git thực thi `core.fsmonitor` và
`core.pager` như lệnh shell — `archctl` tự gọi git với những khoá đó đã tắt,
nhưng mọi công cụ khác trong vòng lặp thì không.

Cấm bằng cấu hình thì chỉ chắc bằng chính cấu hình đó. Lớp cưỡng chế thật vẫn
là CI và CODEOWNERS trên `.arch/**`, vì chúng chạy ngoài tầm với của agent.
