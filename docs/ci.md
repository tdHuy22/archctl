# CI

CI quan trọng vì nó chạy ở nơi agent không với tới. Mọi ranh giới ở máy cục bộ
đều dựa vào việc agent không ghi được `.arch/` — `.claude/settings.json` cấm
rồi, nhưng cấm bằng cấu hình thì chỉ chắc bằng chính cấu hình đó, và cấu hình
nằm trong repo.

Repo này tự chạy CI của mình, năm job:
[.github/workflows/ci.yml](../.github/workflows/ci.yml).

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

## Repo thật, ở mỗi pull request

Job `repo thật` chạy [bench/bench.py](../bench/bench.py): tải ba mươi repo ở
đúng commit ghim trong `bench/repos.tsv`, rồi đòi hai điều. Luật do `init
--from-code` sinh ra, nâng lên `#accepted`, kiểm lại chính mã đó thì phải ra 0
vi phạm. Và mọi lệnh phải chạy được trên mã người khác viết, chứ không chỉ trên
fixture viết riêng để test.

Đó là phép thử mạnh nhất mà repo có, nên nó chạy ở mỗi pull request chứ không
chỉ theo lịch: chạy hằng tuần thì một lỗi đã vào `main` rồi mới bị bắt. Tải
khoảng 920 MB rồi chạy chừng hai phút trên máy dev; trên runner chung thì lâu
hơn, và bước đối chiếu với clang ở dưới chạy qua ~4 000 file C/C++ trên hai
lõi, nên job có giới hạn 25 phút.

Nó thoát 2 — không phải 1 — khi không tải được repo, để một lần mạng chập chờn
không trông giống một lỗi của archctl.

Ngay sau đó, `bench/sabotage.py` phá archctl hai mươi tám kiểu và đòi bộ đo bắt đúng
từng kiểu. Bộ đo chỉ có giá trị khi mỗi phép kiểm của nó đổ được; bước này
giữ điều đó đúng qua những lần sửa sau.

Bước cuối, `bench/oracle.py`, lấp chỗ mà phép tự nhất quán không với tới: nó
so archctl với chính nó, nên một lỗi trong bộ trích — chỗ cả bộ khám phá lẫn
bộ kiểm cùng dùng — làm cả hai sai giống nhau mà vẫn ra "0 bất nhất". Vue core
từng mất 21% import tĩnh như vậy suốt nhiều lần chạy xanh. Bước này đối chiếu
với parser chuẩn: `ast` của Python, lexer thô của clang, parser module của
Node, `go/parser` của Go, `syn` cho Rust, IPython cho notebook, PyYAML và
dockerfile-parse cho compose và Dockerfile, PyYAML cho manifest Kubernetes và
cạnh qua Service ([k8s_truth.py](../bench/k8s_truth.py)). Bốn cái đầu có sẵn trên image
`ubuntu-24.04` đã ghim. `syn` thì phải tải từ crates.io, nên riêng bước này
mở lại mạng cho cargo (`CARGO_NET_OFFLINE: "false"`); archctl đã dựng xong ở
bước trước khi còn offline, và crate tải về chỉ vào trình đọc của bộ đo, ghim
cứng bằng `=`. IPython, PyYAML và dockerfile-parse cài bằng pip vào venv riêng,
mọi gói ghim `==` kèm sha256; grpcio-tools (`protoc`) cũng vậy. `helm` bản ghim
tải từ get.helm.sh, kiểm sha256 của tarball. `--require
clang,node,go,syn,ipython,deploy,proto,helm` để ngày thiếu một cái thì bước này
đỏ chứ không lặng lẽ bỏ qua. Hai mươi tám trong bốn mươi chín kiểu phá của
`sabotage.py` nhắm vào bước này: ba ở import Python, hai ở tên máy trong chuỗi
của mã Python, ba ở import Rust, hai ở
notebook, bốn ở cạnh Python ↔ native, bốn ở compose và Dockerfile, bảy ở
manifest Kubernetes, cạnh qua Service, NetworkPolicy và lối vào từ Internet,
một ở manifest dựng từ chart Helm, hai ở `.proto`. Mười hai kiểu khác nhắm vào
phần hạ tầng của chính bộ đo (năm trên compose gồm một ở tên máy viết trong mã,
bảy trên manifest Kubernetes gồm hai ở NetworkPolicy, một ở lối vào, một ở
chart Helm), một kiểu vào cạnh gRPC bắt buộc (chạy
trên etcd, và trên microservices-demo cho cạnh khác ngôn ngữ: Python ↔ Go, Go →
Node). Phần Rust cũng cần `syn`, nên bước
sabotage mở mạng giống bước đối chiếu.

## Kiến trúc mô hình chạy thật

Job `kiến trúc mô hình` chạy công thức trong [ml.md](ml.md) trên
`examples/ml`: dựng mô hình thật, so cây module với
`examples/ml/tests/model_arch.txt`, đỏ khi khác. archctl không đọc được layer từ
mã nguồn, nên phần này cần torch. Torch, pytest và mọi gói kéo theo ghim `==`
kèm sha256 trong `examples/ml/requirements-test.txt`, chỉ wheel. Linux lấy torch
bản CPU (196 MB) từ index của PyTorch, vì bản trên PyPI là bản CUDA. Job tách
riêng để không kéo dài job `repo thật`, và chạy `archctl verify model_arch`
đúng như người dùng chạy ở máy mình, với `python3` của venv.

## Còn một chốt nữa không nằm trong YAML

```
.arch/**  @ten-cua-ban
```

CODEOWNERS bắt buộc một người cụ thể phê duyệt mọi thay đổi trong `.arch/`.
CI kiểm mã có theo luật không; CODEOWNERS kiểm ai được đổi luật.

## CI chỉ chặn được khi nhánh được bảo vệ

Một job đỏ tự nó không ngăn được gì. Nó chặn merge chỉ khi nhánh `main` bật
*required status checks*, và CODEOWNERS chỉ bắt buộc khi bật *require review
from code owners* — cả hai đều là branch protection hoặc ruleset.

Trên GitHub Free, repo **private** không có hai thứ đó. Repo này là một ví dụ:

```
$ gh api repos/<chủ>/<repo>/branches/main/protection
Upgrade to GitHub Pro or make this repository public to enable this feature. (HTTP 403)
```

Khi đó CI vẫn chạy và vẫn báo, nhưng là **báo** chứ không phải **chặn**: một
commit đẩy thẳng lên `main` vẫn vào, đỏ hay xanh. Với dự án mà agent tự đẩy
được lên `main`, đó là khác biệt giữa một chốt chặn và một lời nhắc. Muốn chặn
thật thì repo phải public, hoặc tài khoản phải lên gói có branch protection cho
repo private.

macOS cũng tốn hơn trên repo private: phút dùng runner macOS tính khoảng gấp
mười lần Linux, còn repo public thì không tính phút.
