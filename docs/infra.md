# Tầng hạ tầng: service, mạng, triển khai

Đồ thị import cho biết mã nào gọi mã nào. Nó không cho biết mã nào chạy trong
container nào, và container nào nói chuyện với container nào qua mạng. Hai câu
hỏi đó nằm trong docker-compose và Dockerfile, và archctl đọc chúng.

```
service Api     from compose:api     runs Serve, Rag, Data
service Ingest  from compose:ingest  runs Rag, Data
service Trainer from compose:trainer runs Train, Data
service Qdrant  from compose:qdrant
service Db      from compose:db

link Api     -> Qdrant, Db
link Ingest  -> Qdrant
link Trainer -> Db
```

Mã và triển khai là hai trục khác nhau: `Rag` chạy trong cả `Api` lẫn
`Ingest`. Vì vậy service có không gian tên riêng — `component Api` và
`service Api` cùng tồn tại được — và `allow`, `deny` không bao giờ trỏ tới
service.

## Khai báo

```
service <Tên> [from <nguồn>] [runs <Component>, ...]   @owner #status
link <Service> -> <Service>[, <Service>...]             @owner #status
```

Nguồn cấu hình:

| `from` | Nghĩa |
| --- | --- |
| `compose:api` | service `api` trong file compose của repo |
| `compose:deploy/api` | service `api` trong file compose ở thư mục `deploy/`, khi nhiều thư mục cùng có tên đó |
| `compose:./api` | như trên, cho thư mục gốc |
| `dockerfile:svc/Dockerfile` | service dựng từ Dockerfile này, không qua compose; build context là thư mục chứa nó |

Không có `from` thì service chỉ là một cái tên trên sơ đồ, ví dụ cơ sở dữ liệu
do bên khác vận hành. Đường dẫn trong `from` phải tương đối và nằm trong dự
án: file luật là thứ agent đề xuất được, và `from dockerfile:../../x` là cách
nhờ archctl đọc hộ một file ngoài repo.

`runs` liệt kê component được phép chạy trong service. Rỗng nghĩa là không có
mã của dự án này: image dựng sẵn như `postgres`.

`link` như `allow`: không có `link` là cấm. Khác `allow`, mọi service có `from`
và đang có hiệu lực đều bị kiểm ngay, vì số service ít và `init --from-code`
sinh sẵn đủ.

## archctl bắt được gì

```
✗ VI PHẠM  Ingest -> Db  (cạnh mạng không có link, #accepted)
  docker-compose.yml:14  DATABASE_URL → db:5432
  luật: không có link nào cho Ingest -> Db
  → thêm `link Ingest -> Db`, hoặc bỏ cạnh này khỏi cấu hình

✗ VI PHẠM  Train chạy trong service Api  (không có trong runs, #accepted)
  mlapp/serve/api.py:2  import 'mlapp.train.loop'
  luật: service Api runs Serve, Rag, Data
  → thêm Train vào `runs` của service Api, hoặc bỏ phụ thuộc này

✗ VI PHẠM  Train chạy trong service Trainer  (không có trong runs, #accepted)
  docker-compose.yml:17  khởi chạy mlapp.train.loop
  luật: service Trainer runs Data
  → thêm Train vào `runs` của service Trainer, hoặc bỏ phụ thuộc này

✗ VI PHẠM  Api -> redis  (cạnh mạng không có link, #accepted)
  docker-compose.yml:9  CACHE_URL → redis:6379
  luật: service compose `redis` chưa khai, nên không có link nào cho Api tới nó
  → khai `service ... from compose:redis` rồi thêm `link Api -> ...`
```

**Cạnh mạng.** Lấy từ compose: `depends_on`, `links`, và tên một service khác
xuất hiện trong giá trị biến môi trường — `postgres://…@db:5432/app`,
`http://qdrant:6333`, `kafka1:9092,kafka2:9092`, hay giá trị trần `db` khi
đoạn cuối của tên khoá nói về máy (`DB_HOST`, `REDIS_URL`, `KAFKA_BROKERS`,
`…_ADDR`, `…_DSN`, `…_ENDPOINT`…). Khoá có một đoạn nói về bí mật (`…_KEY`,
`…_TOKEN`, `…_SECRET`, `PASSWORD`, `AUTH`…) thì chỉ đọc URL có `scheme://`
(`AUTH_SERVICE_URL=http://auth:8000` vẫn là cạnh tới `auth`), không đọc giá
trị trần hay `a:b`: `SUPABASE_SERVICE_ROLE_KEY` là một JWT trông y như tên
máy. `${DB_HOST:-db}`
lấy giá trị mặc định; biến không có mặc định thì không biết, và không đoán.
Tên máy so không phân biệt hoa thường và bỏ dấu chấm cuối (`DB.` là `db`), như
DNS. Mỗi giá trị chỉ xét 256 phần tử đầu và 64 tên máy đầu; giá trị dài hơn
thì vào mục "chưa đọc được", vì cạnh tới một service có thể nằm ở phần sau. Tên khác của service (`hostname`, `container_name`, alias trong
`networks`) cũng tính. `env_file` được đọc theo cùng cách.

Cạnh tới một service compose chưa khai trong file luật là vi phạm, không phải
cảnh báo: không có `link` nào cho nó được, nên agent thêm service `redis` vào
compose rồi nối tới nó là đi ngoài luật.

**Mã chạy trong service.** Đơn vị là component, như mọi luật khác: component
chạy trong service thì mọi thứ nó phụ thuộc cũng chạy ở đó. Hai phép kiểm:

- điểm vào: lệnh khởi chạy (`command`, `entrypoint` của compose, hay `CMD`,
  `ENTRYPOINT` của Dockerfile) dẫn tới file nào, file đó thuộc component nào,
  và component đó phải nằm trong `runs`. File điểm vào không thuộc component
  nào (`manage.py` ở gốc) thì xét các component mà nó import trực tiếp;
- cạnh: mỗi import (và cạnh liên ngôn ngữ) từ component nằm trong `runs` của
  một service phải tới component cũng nằm trong `runs` đó.

Phép thứ hai xét từng file một, nên chạy được trong hook ngay khi agent thêm
import: không cần đọc lại compose.

**Khai báo không còn khớp.** Không phải vi phạm, nhưng là cảnh báo, và
`--strict` tính chúng:

```
⚠ KHAI BÁO LẠC HẬU  service Cache `compose:missing`
⚠ LINK KHÔNG THẤY TRONG CẤU HÌNH  link Trainer -> Qdrant
⚠ KHÔNG RÕ ĐIỂM VÀO  service Worker — không tìm thấy trong repo mã mà `dotnet` nạp
⚠ CHƯA ĐỌC ĐƯỢC  docker-compose.yml — YAML dòng 12: …
⚠ SERVICE CHƯA KHAI  redis (docker-compose.yml:24)
```

Dòng cuối chỉ hiện khi file luật đã mô tả triển khai (có ít nhất một
`service ... from compose:`), và không tính vào `--strict` — nó là phần chưa
khai báo, như file nằm ngoài sơ đồ.

## Đọc được gì

**File compose**: `compose.yaml`, `docker-compose.yml`, và các biến thể
`docker-compose.prod.yml`, `compose.override.yaml`. File cùng thư mục là một
project: service cùng tên gộp lại, cạnh và lệnh khởi chạy lấy hợp của mọi
file. Bỏ qua file dành cho test (`docker-compose.test.yml`, `.e2e.`, `.ci.`)
và file trong thư mục test, ví dụ, tài liệu hay thư mục ẩn (`.devcontainer`,
`.github`). `include`, `extends` (cùng file hay file khác), `env_file`, bind
mount `./src:/app` đều được đọc.

**YAML**: tập con đủ cho compose và manifest Kubernetes — mapping, sequence,
flow `[…]` `{…}`, chuỗi nháy đơn, nháy kép, block scalar `|` `>`, anchor,
alias, khoá gộp `<<`, nhiều tài liệu. Cách gập dòng chép theo PyYAML, và bộ đo
so từng giá trị với PyYAML. Không đọc khoá phức `? `; gặp thì báo lỗi kèm
dòng.

**Dockerfile**: stage (`FROM … AS`, kể cả `FROM <stage>` thừa hưởng
`WORKDIR`), `WORKDIR`, `COPY`/`ADD` (kể cả `--from=<stage>`), `CMD`,
`ENTRYPOINT` dạng JSON lẫn dạng shell, `ENV`, `ARG`, `RUN`, dòng nối, chú thích
giữa dòng nối, `# escape=`, heredoc của BuildKit. Đường dẫn trong container
được ánh xạ ngược về file trong repo qua `COPY` và bind mount.

**Lệnh khởi chạy**:

| Lệnh | Điểm vào |
| --- | --- |
| `python app.py`, `python -m pkg.mod` | file, hay module tìm từ thư mục làm việc và `PYTHONPATH` rồi theo các gói của dự án |
| `uvicorn`, `gunicorn`, `hypercorn`, `daphne`, `granian` `pkg.mod:app` | module (`--app-dir`, `--chdir` được tính) |
| `celery -A pkg` | module |
| `flask run` | `FLASK_APP`, không có thì `wsgi.py` rồi `app.py` |
| `streamlit run`, `torchrun`, `accelerate launch`, `deepspeed`, `node`, `nodemon`… `x.py`/`x.js` | file |
| `python -m uvicorn …`, `python -m torch.distributed.run …` | như gọi thẳng trình chạy đó |
| `npm start`, `yarn dev`, `pnpm run x` | script trong `package.json`, đọc tiếp |
| `react-scripts`, `vite`, `vue-cli-service`, `ng serve` | `src/index.*`, `src/main.*` theo quy ước |
| `go run ./cmd/x`; binary dựng bằng `go build -o …` trong Dockerfile | package Go |
| `cargo run` | `src/main.rs` |
| `sh -c "a && b"`, `bash -o pipefail -c "…"`, script `.sh` hay script không đuôi có dòng `#!…sh`, `exec "$@"` | đọc tiếp từng lệnh; `cd dir`, `pushd dir` đổi thư mục làm việc cho các lệnh sau |
| `./wait-for-it.sh db:5432 -- python app.py` | script, rồi lệnh sau `--` |

Mỗi thứ lệnh khởi chạy nạp phải lần ra được mã trong repo. Một cái không ra
(`python -m khong_co`, file không có) thì service đó vào mục "không rõ điểm
vào", dù các lệnh khác của nó lần ra được: lệnh không lần ra có thể đang chạy
mã ngoài `runs` mà không ai thấy. Lệnh nằm trong script (hay script npm) cũng
vậy, không chỉ lệnh viết thẳng trong `command`. Chỗ bộ đọc thôi đọc giữa
chừng cũng vậy: `sh -c` lồng quá 4 tầng, script gọi script quá 4 tầng, quá
256 bước lần theo, script là symlink hay quá 1 MB, hay lệnh không chạy chương
trình nào archctl nhận ra (`sh -c "java -jar app.jar"`). File điểm vào không
biết ngôn ngữ (`./entrypoint` viết bằng Python, không đuôi), hay không đọc
được (quá 4 MB, không phải UTF-8, có symlink trên đường dẫn), cũng vào mục
này, kể cả khi nó thuộc một component: import của nó không được quét, nên
không biết nó nạp mã nào.

Bọc ngoài được bỏ qua: `exec`, `env A=1`, `tini --`, `gosu user`, `uv run`,
`poetry run`, `opentelemetry-instrument`, `"$PYTHON_CMD"`… Ngôn ngữ archctl
không đọc (Java, .NET, PHP) và image dựng sẵn (nginx, logstash) thì báo là
không rõ điểm vào — chỉ khi service đó có `runs`.

## Cạnh gRPC

Client gọi `pb.NewKVClient(conn)`, server đăng ký `pb.RegisterKVServer(s,
impl)`. Hai bên chỉ cùng import mã sinh ra từ `rpc.proto`, không import nhau,
nên đồ thị import không thấy lời gọi. archctl dò chỗ gọi stub và chỗ đăng ký
server, rồi ghép theo tên đầy đủ `package.Service` đọc từ `.proto`:

```
✗ VI PHẠM  Client -> Server  (không được phép, #accepted)
  client/v3/retry.go:103  gọi gRPC `pb.NewKVClient (grpc:etcdserverpb.KV)`
  luật: không có allow nào cho Client -> Server
```

| Ngôn ngữ | Phía client | Phía server |
| --- | --- | --- |
| Python | `XStub` từ module `*_pb2_grpc` (import tuyệt đối hay tương đối, `as`, `import *`, kể cả gán vào biến rồi gọi); `ch.unary_unary("/pkg.X/M")` | `add_XServicer_to_server` (kể cả gán vào biến); `method_handlers_generic_handler("pkg.X", …)` |
| Go | `pb.NewXClient(conn)` (`pb` là alias import, tên package trong `go_package`, hay gọi không tiền tố trong package sinh ra hoặc qua dot-import); `conn.Invoke(ctx, "/pkg.X/M", …)` | `pb.RegisterXServer(s, impl)`; `s.RegisterService(&pb.X_ServiceDesc, impl)` |

Chỗ nhắc tới cũng tính, không chỉ chỗ gọi: `S = kv_pb2_grpc.KVStub` rồi `S(ch)`
vẫn là lời gọi. `XStub` không rõ module (re-export qua `__init__.py`,
`self.pb.KVStub`) chỉ tính khi một `.proto` trong repo khai service `X`: không
thì `QuantStub()` của PyTorch cũng thành gRPC.

- **Luật.** Cạnh giữa hai component cần `allow`, như import. Khi file luật đã
  khai `service` và hai component chạy ở hai service khác nhau (theo `runs`),
  cần thêm `link` giữa hai service: với mỗi service chạy phía client, phải có
  một service chạy phía server là chính nó, hoặc có `link` tới. Cạnh gRPC
  không kéo component server vào `runs` của client: server chạy ở process khác.
- **Ghép.** Lần từ module sinh ra về file `.proto`: `rpc_pb2_grpc` từ
  `rpc.proto` (cùng tên file ở nhiều thư mục, như API có phiên bản
  `shop/v1/kv.proto` và `shop/v2/kv.proto`, thì phân biệt theo đường dẫn
  module `shop.v2.kv_pb2_grpc`), import path của Go khớp `option go_package`.
  Không lần được thì theo tên service, khi tên đó chỉ có ở một package; có ở
  nhiều package thì báo "không ghép được" (`--strict` tính). Module sinh ra
  mà repo không có file `.proto` cùng tên (thư viện ngoài, hay `.proto` để ở
  repo khác) thì ghép theo tên trần: client và server cùng dùng nó vẫn ghép
  với nhau (server `VllmEngine` của vllm), nhưng không bị ghép nhầm vào service
  cùng tên của repo. Go thì phải có `.proto`, vì `NewXClient` cũng là tên hàm
  thường.
- **Nhiều server.** Một service có thể có nhiều component phục vụ (proxy của
  etcd cũng đăng ký `KV`): cạnh tới mọi component đó.
- **Không tính.** Server đăng ký trong file test hay mock: `*_test.go`,
  `test_*.py`, `*_test.py`, `conftest.py`, và thư mục `test`, `tests`,
  `testdata`, `testutil(s)`, `mock`, `mocks`, `grpctesting`. Đó là server giả.
  Lời gọi tới service mà mọi server đều là server giả thì được báo "không ghép
  được" (không biết service chạy thật ở đâu). Gateway
  (`RegisterXHandlerServer`), health, reflection. Lời gọi tới service không có
  server nào trong repo (collector của OpenTelemetry, API của Google) được
  coi như import gói ngoài: không vi phạm, không cảnh báo.
- **Server bị giấu.** Có chỗ cài đặt server (lớp kế thừa `KVServicer`, struct
  nhúng `UnimplementedKVServer`) mà không thấy chỗ đăng ký nào thì báo "không
  ghép được": phía server đang được đăng ký theo cách archctl không đọc, và
  mọi cạnh tới nó sẽ biến mất.
- **Không thấy.** `getattr(module, "KVStub")`, tên dựng lúc chạy, reflection,
  mã trong f-string, gọi wrapper nằm ngoài package sinh ra mà không nhắc tên
  stub.
- **Hook.** `archctl check client.py` vẫn thấy cạnh: khi file đổi có lời gọi
  hay chỗ đăng ký gRPC, archctl tìm phía bên kia trong repo một lượt: đọc file
  Python, Go (tối đa 64 MB cho cả lượt), chỉ lex file có chứa tên cần tìm.
  Hàm `NewXClient` không phải gRPC (không có `.proto`) thì không đi tìm. Hook
  với file compose không báo `link` là "không thấy" khi giữa hai service có
  `allow` (lời gọi gRPC có thể nằm ở file ngoài tập đang xét); lần kiểm cả
  repo thì báo đủ.
- **`init --from-code`** sinh `allow` cho cạnh gRPC (một mục riêng trong luật)
  và `link` giữa service. **`scan`** in chỗ gọi, chỗ đăng ký, và package,
  service, rpc của từng file `.proto`.

## Không đọc

- Helm template, overlay của Kustomize, Terraform: không render. Ai cần thì
  khai `verify` gọi `helm template`, `terraform graph`, kube-linter, checkov.
- Manifest Kubernetes: đợt sau. gRPC từ TypeScript, Rust, Java: đợt sau.
- Cạnh chỉ có lúc chạy: URL ghép trong mã, service discovery, biến lấy từ
  secret store, `network_mode`.

## Trong hook, `init`, sơ đồ

**Hook.** `archctl check <file>` với file mã thuộc một component chỉ kiểm
`runs` theo import của file đó. Nó đọc lại cấu hình của cả repo khi file vừa
đổi có thể là thứ cấu hình trỏ tới: file compose, Dockerfile, `.env`, script,
`package.json`; mọi file không phải mã (trừ tài liệu và ảnh), vì `env_file`,
`extends.file`, `dockerfile:` đặt tên tuỳ ý; file mã không thuộc component
nào, vì đó là chỗ của điểm vào (`main.py` ở gốc); và `pyproject.toml`,
`setup.py`, `setup.cfg`, vì chúng đổi chỗ tìm module. Repo chưa khai `service`
nào thì không đọc compose, không tốn gì.

**`init --from-code`** sinh `service`, `runs` và `link`. `runs` là mọi
component mà điểm vào chạm tới theo các cạnh đã quan sát — đúng điều `check`
đòi, nên bản nháp nâng lên `#accepted` thì không tự vi phạm.

**`export`** vẽ service trong một subgraph "Triển khai", `link` thành cạnh
"mạng". **`serve`** vẽ service thành hộp bo tròn, cạnh "chạy trong" nét chấm
tới các component trong `runs`. **`scan`** in mọi thứ bộ đọc thấy: dữ kiện
từng file, lệnh khởi chạy dẫn tới file nào, cạnh mạng. Mặc định nó chỉ in tên
chương trình, số đối số và dòng của mỗi lệnh, từ khoá và dòng của mỗi chỉ thị
Dockerfile, tên khoá của mỗi biến môi trường: đối số của `command`, `RUN`,
`ENV`, `ARG` hay chở token. Đường dẫn, image, URL (`ADD https://…`,
`include:`) thì bỏ phần `user:mật khẩu@` và cắt bớt khi quá dài. `scan
--verbatim` in nguyên mọi thứ đó, kể cả đối số `ENV`, `ARG` của Dockerfile,
khi bạn cần xem và chắc output không đi vào log chung. Giá trị biến môi
trường trong compose thì không bao giờ được in, kể cả với `--verbatim`.

## An toàn

File compose và Dockerfile là của người khác hay do agent viết, và bộ đọc chạy
trong hook. Chốt chặn (threat model TM-8 tới TM-12):

- không chạy lệnh nào — lệnh khởi chạy chỉ được tách thành chữ;
- **không bao giờ in giá trị biến môi trường**: chỉ `KHOÁ → máy:cổng`, và tên
  máy chỉ là tên một service; phần `user:mật khẩu@` của URL bị bỏ, kể cả trong
  đường dẫn hiện trong thông báo lỗi. Giá trị thường mang mật khẩu, và output
  của archctl đi vào log CI và context của agent. Riêng `PYTHONPATH` và
  `FLASK_APP` được giữ để tìm điểm vào. Thông báo lỗi YAML chỉ có dòng, cột,
  không trích nội dung, kể cả tên alias;
- đường dẫn lấy từ cấu hình không được ra khỏi dự án: không tuyệt đối, không
  `..` vượt gốc, không đi theo symlink ở bất kỳ đoạn nào, chỉ file thường, tối
  đa 1 MB. File được mở không chờ trên FIFO (và với `O_NOFOLLOW` ở nơi biết
  chắc hằng số), rồi kiểm lại trên file đã mở: phải là file thường, cùng
  `(thiết bị, inode)` với thứ vừa kiểm;
- input đối kháng không treo được hook. Alias YAML là tham chiếu, `<<` có ngân
  sách tính theo byte, độ lồng tối đa 64. Mỗi file compose tối đa 2 000
  service, 50 000 mục (env, `depends_on`, volume…) và 8 MB dữ liệu sau khi trải
  alias (argv, giá trị env đem đi tìm tên máy); cả repo tối đa 200 000 mục,
  32 MB như vậy (kể cả bản sao do `extends`) và 16 MB đọc từ đĩa. Mỗi file
  (Dockerfile, script, `env_file`, `package.json`) được đọc và phân tích một
  lần dù hàng nghìn service cùng dùng nó. Dockerfile tối đa 64 stage, 4 096
  `COPY`, mỗi giá trị sau khi thay biến tối đa 8 KB, tổng giá trị `ARG`/`ENV`
  được nhớ tối đa 1 MB. Mỗi lệnh khởi chạy lần qua tối đa 256 bước, cả repo
  20 000; lồng `sh -c` tối đa 4 tầng; mỗi lệnh ghi chi tiết tối đa 5 chỗ thôi
  đọc. Vượt trần ở file compose hay Dockerfile thì file đó vào mục "chưa đọc
  được"; ở lệnh khởi chạy thì service vào mục "không rõ điểm vào".

Thứ không đọc được thì báo ra, không im lặng bỏ qua.

## Bộ đo

`bench/repos.tsv` có `example-voting-app` (compose năm service, stage
Dockerfile thừa hưởng `WORKDIR`, bind mount) và `open-webui` (ứng dụng LLM tự
host: `build: .`, `COPY --from=build`, điểm vào qua `start.sh` rồi `uvicorn`).
Luật `init` sinh ra phải tự nhất quán; cạnh mạng và `runs` bắt buộc phải có, và
gỡ chúng đi thì `check` phải báo (`consistency.DEPLOY`). `oracle.py --require
deploy` so dữ kiện từng file compose và Dockerfile với PyYAML và
dockerfile-parse; `sabotage.py` phá từng phép kiểm để chắc nó đổ được.
