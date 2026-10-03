# CI

CI quan trọng vì nó chạy ở nơi agent không với tới. Mọi ranh giới ở máy cục bộ
đều dựa vào việc agent không ghi được `.arch/` — `.claude/settings.json` cấm
rồi, nhưng cấm bằng cấu hình thì chỉ chắc bằng chính cấu hình đó, và cấu hình
nằm trong repo.

Repo này tự chạy CI của mình: [.github/workflows/ci.yml](../.github/workflows/ci.yml).

## Hai lệnh, đúng thứ tự

```yaml
- run: archctl verify
- run: archctl check --strict
```

Ngược lại là hỏng. `.arch/.verified` không vào git — một biên nhận commit được
là một biên nhận bịa được — nên mọi bản checkout bắt đầu không có biên nhận
nào, và `--strict` coi "chưa có biên nhận" là vi phạm. Đặt `check` trước thì
nó đổ ngay lần chạy đầu tiên, trên một repo không có gì sai.

## CI không chép lại các lệnh kiểm

`archctl verify` đọc `.arch/system.arch` rồi chạy đúng những gì ở đó:

```
verify tests runs "cargo test --quiet"
verify lint  runs "cargo clippy --all-targets --quiet -- -D warnings"
```

File workflow không biết `cargo test` nằm ở đâu, nên không có bản sao nào để
lệch khỏi file luật. Thêm một dòng `verify` là CI chạy thêm cái đó, không phải
sửa YAML. Và cái mà CI chạy đúng là cái mà lập trình viên chạy ở máy mình.

Đổi lại có một vòng tròn: job đó uỷ quyền việc chạy test cho chính công cụ
đang được test. Nếu `cmd_verify` hỏng thành không-làm-gì, nó vẫn ghi biên
nhận, `check` vẫn thấy biên nhận mới, và cả CI thành mù mà không đỏ một ô nào.
Nên có một job thứ hai gọi thẳng `cargo test`, không qua `archctl`. Nó chạy
trên macOS, nên cùng lúc trả lời câu hỏi khác: công cụ viết trên macOS có chạy
ở nơi khác không.

## `--strict` chặn những gì

Vi phạm mới thì `check` nào cũng chặn. `--strict` chặn thêm mọi thứ "chưa kiểm
được": import không phân giải được, file quá lớn, mẫu `at` chồng nhau, biên
nhận hết hạn, **và cảnh báo của chính file luật** — `contract` không có verify,
`invariant` chỉ là ghi chú. Chi tiết ở [check.md](check.md).

Trên nhánh chính thì đó là đúng mức. Một luật không ai kiểm được vẫn nằm trong
file và vẫn đọc lên y như luật đang được áp.

## Không dùng action của bên thứ ba

`arch-core` không có dependency nào ngoài std, vì công cụ chạy trong repo người
dùng. Một action lạ trong CI cũng là mã người khác chạy trên repo này, với cùng
lý do để từ chối. Workflow ở đây chỉ dùng `actions/checkout` của GitHub, ghim
theo SHA chứ không theo nhãn: nhãn `v7` dời sang commit khác được, SHA thì
không.

```yaml
permissions:
  contents: read
```

Không job nào cần ghi gì. Tuỳ cấu hình repo, mặc định của GitHub có thể rộng
hơn thế, nên nói rõ ra.

`CARGO_NET_OFFLINE: "true"` là lớp thứ hai cho `verify no_deps`: thêm một crate
vào `Cargo.toml` thì build đổ, chứ không chỉ sai một con số trong `Cargo.lock`.

## `archctl diff` trong log pull request

```yaml
- run: archctl diff "$BASE"
```

Không chặn gì, chỉ đưa thay đổi của file luật vào log nơi người review nhìn
thấy, và đánh dấu riêng phần **nới lỏng** luật. Lệnh trong `verify ... runs`
nằm trong `arch_hash`, nên một PR đổi lệnh kiểm thành `curl ... | sh` hiện ra
ở đây.

## MSRV đọc từ Cargo.toml

Job `rust tối thiểu` đọc `rust-version` trong `Cargo.toml` rồi biên dịch lại
bằng đúng toolchain đó, thay vì ghi lại con số trong YAML. Hai bản sao của
cùng một con số thì sớm muộn lệch nhau — và con số này từng ghi `1.85` trong
khi mã dùng let-chain của 1.88, suốt nhiều tháng, vì không có gì kiểm nó.

## Còn một chốt nữa không nằm trong YAML

```
.arch/**  @ten-cua-ban
```

CODEOWNERS bắt buộc một người cụ thể phê duyệt mọi thay đổi trong `.arch/`.
CI kiểm mã có theo luật không; CODEOWNERS kiểm ai được đổi luật.
