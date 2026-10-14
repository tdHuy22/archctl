# Tầng hạ tầng: service, mạng, triển khai

Đồ thị import cho biết mã nào gọi mã nào. Nó không cho biết mã nào chạy trong
container nào, và container nào nói chuyện với container nào qua mạng. Hai câu
hỏi đó nằm trong docker-compose, Dockerfile và manifest Kubernetes, và archctl
đọc chúng.

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
service <Tên> [from <nguồn>[, <nguồn>...]] [runs <Component>, ...]   @owner #status
link <Service> -> <Service>[, <Service>...]                          @owner #status
```

Nguồn cấu hình:

| `from` | Nghĩa |
| --- | --- |
| `compose:api` | service `api` trong file compose của repo |
| `compose:deploy/api` | service `api` trong file compose ở thư mục `deploy/`, khi nhiều thư mục cùng có tên đó |
| `compose:./api` | như trên, cho thư mục gốc |
| `dockerfile:svc/Dockerfile` | service dựng từ Dockerfile này, không qua compose; build context là thư mục chứa nó |
| `k8s:api` | workload `api` (Deployment, StatefulSet, DaemonSet, ReplicaSet, Job, CronJob, Pod) trong manifest Kubernetes, ở mọi thư mục: bản phát hành trong `release/`, base của kustomize là bản sao của cùng ứng dụng và được kiểm hết |
| `k8s:shop/k8s/api` | chỉ workload `api` trong thư mục `shop/k8s`, khi hai ứng dụng khác nhau cùng có workload tên đó |

Một service có thể có nhiều nguồn, khi cùng một ứng dụng chạy bằng compose ở
máy dev và bằng Kubernetes ở cụm thật: `service Vote from compose:vote,
k8s:vote runs Vote`. Mỗi nguồn được kiểm riêng theo cùng `runs` và `link`.
`dockerfile:` đi cùng `k8s:` nghĩa là image của container chính trong workload
dựng từ Dockerfile đó (xem [Manifest Kubernetes](#manifest-kubernetes)); đi
cùng `compose:` thì thừa, vì compose đã nói service dựng từ đâu.

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
| Node (JS, TS) | `new proto.pkg.X(addr, creds)` khi nạp `.proto` lúc chạy qua `@grpc/proto-loader`; `new svc.XClient(addr, creds)` của mã sinh grpc-tools | `server.addService(proto.pkg.X.service, impl)`; `server.addService(svc.XService, impl)` |
| Rust (tonic) | `x_client::XClient`, kể cả `use pb::x_client::{XClient}` rồi `XClient::connect` | `x_server::XServer`, kể cả `use pb::x_server::{X, XServer}` rồi `XServer::new`; `impl X for S` với `X` lấy từ `x_server` là chỗ cài đặt |

Chỗ nhắc tới cũng tính, không chỉ chỗ gọi: `S = kv_pb2_grpc.KVStub` rồi `S(ch)`
vẫn là lời gọi. Node nạp `.proto` lúc chạy nên không lần được module: như Go,
chỉ tính khi một `.proto` trong repo khai service đó; chuỗi thuộc tính
(`proto.shop.v1.X`) là đuôi của package và thu hẹp theo nó, vì gốc chuỗi có thể
là biến gán giữa chừng (`const shop = load(def).shop`). Một chữ trần chỉ tính
khi có đuôi `Client` (`new Foo()` là lớp thường). tonic viết tên service theo UpperCamel
(`KV` thành `KvServer`), nên Rust ghép không phân biệt hoa thường, và module
phải cùng tên với struct (`kv_server::KvServer`). File sinh ra
(`*_grpc_pb.js`; Rust khai `mod x_server` và dùng `tonic::codegen`) không phải
chỗ dùng. Server trong `#[cfg(test)] mod` của Rust là server giả. `XStub` không rõ module (re-export qua `__init__.py`,
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
  `test_*.py`, `*_test.py`, `conftest.py`, `*.test.js`, `*.spec.ts`,
  `tests.rs`, `*_test.rs`, và thư mục `test`, `tests`, `testdata`,
  `testutil(s)`, `mock`, `mocks`, `grpctesting`, `__tests__`. Đó là server giả.
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
  Python, Go, TS/JS, Rust (tối đa 64 MB cho cả lượt), chỉ lex file có chứa tên
  cần tìm (so không phân biệt hoa thường).
  Hàm `NewXClient` không phải gRPC (không có `.proto`) thì không đi tìm. Hook
  với file compose không báo `link` là "không thấy" khi giữa hai service có
  `allow` (lời gọi gRPC có thể nằm ở file ngoài tập đang xét); lần kiểm cả
  repo thì báo đủ.
- **`init --from-code`** sinh `allow` cho cạnh gRPC (một mục riêng trong luật)
  và `link` giữa service. **`scan`** in chỗ gọi, chỗ đăng ký, và package,
  service, rpc của từng file `.proto`.

## Manifest Kubernetes

```
service Checkout from k8s:checkoutservice runs Checkout
service Catalog  from k8s:productcatalogservice runs Catalog
link Checkout -> Catalog
```

Workload trong manifest là một `service` như service compose. Cạnh mạng đi qua
tên `Service`:

```
✗ VI PHẠM  Checkout -> Catalog  (cạnh mạng không có link, #accepted)
  kubernetes-manifests/checkoutservice.yaml:57  PRODUCT_CATALOG_SERVICE_ADDR → productcatalogservice:3550
```

**Cạnh mạng.** Giá trị biến môi trường theo đúng luật tên máy của compose (mục
trên), kể cả giá trị lấy từ ConfigMap của repo qua `envFrom.configMapRef` và
`valueFrom.configMapKeyRef`; cạnh khi đó trỏ về dòng trong ConfigMap. `$(TÊN)`
được thay bằng giá trị của biến khai trước trong cùng container, như
Kubernetes; biến không biết (`fieldRef`, Secret) thì giữ nguyên, không đoán.
Tên máy là tên `Service`: `svc`, `svc.ns`, `svc.ns.svc`,
`svc.ns.svc.cluster.local`, và pod của Service headless (`kafka-0.kafka`).
`svc.ns` được hiểu là namespace khi `ns` là `default` hay namespace có trong
repo (`metadata.namespace`, `kind: Namespace`, `namespace:` của kustomization);
`x.y` mà `x` là Service của repo còn `y` không phải namespace đã biết thì vào
mục "chưa đọc được", vì có thể là tên miền ngoài, có thể là namespace chưa
khai. Service `ExternalName` trỏ tới tên trong cụm thì đi tiếp một bước tới
Service đó. Service nối tới workload khi `selector` nằm trong nhãn của pod
template, cùng namespace (thiếu namespace ở một bên thì coi như khớp, vì
namespace thường đặt lúc `kubectl apply -n`); nhãn do controller gắn lúc chạy
(`statefulset.kubernetes.io/pod-name: db-0`, `job-name`) so theo tên và loại
workload. Có Service cùng tên ở nhiều thư mục thì ưu tiên Service và workload
cùng thư mục với bên gọi. `Secret` không bao giờ được đọc, `secretKeyRef` và
`envFrom.secretRef` bị bỏ.

Service không chọn workload nào trong repo (`ExternalName`, không selector,
chart của bên khác) được coi như gói ngoài: không vi phạm, không cảnh báo. Cạnh
tới workload có trong repo mà chưa khai thì là vi phạm, như compose.

**Workload chạy mã nào.** Manifest chỉ ghi tên image. archctl lần ngược:

1. `dockerfile:` khai cùng `k8s:` trong luật: image của container chính
   (theo chú thích `kubectl.kubernetes.io/default-container`, không có thì
   container đầu tiên);
2. artifact trong `skaffold.yaml` (`image` → `context`, `docker.dockerfile`,
   `docker.target`, kể cả trong `profiles`);
3. service compose có cả `image:` lẫn `build:`.

So tên image bỏ tag và digest. Tên trần ở phía dựng (`checkoutservice` trong
skaffold, registry đặt lúc chạy) khớp đoạn cuối của tên đầy đủ
(`gcr.io/x/checkoutservice:v1`); tên có registry thì phải khớp đủ. Khớp nhiều
nơi dựng khác nhau thì không chọn hộ, mà báo "không rõ điểm vào". Không lần
được, như `postgres:16`, thì là image dựng sẵn: service có `runs` thì báo
"không rõ điểm vào", như compose, kể cả khi một sidecar của pod lần được.
Dựng không qua Dockerfile (`jib`, `ko`, `buildpacks`) cũng vậy. Service không
có `runs` mà cấu hình cho thấy rõ nó chạy mã của một component thì vi phạm
`runs` (với compose cũng vậy): `runs` rỗng nghĩa là không chạy mã nào của dự
án.

Lệnh khởi chạy theo đúng Kubernetes: `command` thay `ENTRYPOINT` của image,
`args` thay `CMD`; có `command` mà không có `args` thì `CMD` bị bỏ.
`workingDir` thay `WORKDIR`. Init container cũng là mã chạy trong pod.

**Đọc file nào.** Mọi file YAML và JSON của repo có chữ `apiVersion` và `kind`
(kể cả khoá thụt lề, có nháy, flow, BOM), trừ thư mục test, ví dụ, tài liệu, ẩn
(như compose) — nhưng file trong các thư mục đó mà một kustomization kéo vào thì
vẫn đọc, vì nó được triển khai. File có `apiVersion:` và `kind:` ở đầu dòng mà
không phân tích được thì báo. `kind: List`, `DeploymentList`… được trải một
tầng. Workload không container nào có image là mảnh patch, không phải
workload. Manifest chỉ được đọc khi luật có nguồn `k8s:` (hay khi `init`,
`scan`): repo chỉ dùng compose không tốn gì thêm.

- **Helm:** template trong thư mục chart (`templates/`, `charts/` cạnh
  `Chart.yaml`) không được đọc, vì nó chưa phải YAML cho tới khi render.
  `scan` in thư mục chart ra. Không tính vào `--strict`. Muốn kiểm thì khai
  `verify` gọi `helm template`.
- **Kustomize:** file trong `resources` đọc như manifest thường; file patch
  (`patches`, `patchesStrategicMerge`, `patchesJson6902`) không phải manifest.
  Patch không được áp. Patch đổi lệnh, biến môi trường hay image của một
  workload đã khai (kể cả JSON 6902 thay cả nhánh `/spec/template/spec`), trong
  một kustomization kéo workload đó vào (qua `resources`, `components`), thì
  được báo "chưa đọc được": workload thật chạy khác thứ archctl thấy. Patch chỉ
  đổi `replicas`, `resources` thì không. `namePrefix`, `nameSuffix`, `images`,
  `configMapGenerator`, `replacements`, `vars` cũng được báo như vậy, vì chúng
  đổi tên Service, image hay giá trị env.
- **Loại lạ có pod template** (Argo `Rollout`, CRD): báo "chưa đọc được", vì
  mã chạy trong đó không được kiểm.
- **Bản sao:** `k8s:<tên>` khớp workload tên đó ở mọi thư mục, và mỗi bản
  được kiểm `link`, `runs` như nhau: bản phát hành trong `release/` thường
  mới là thứ được triển khai thật.

**`init --from-code`** sinh `service ... from k8s:<tên>` cho từng workload.
Workload cùng tên ở nhiều thư mục mà dựng từ cùng một chỗ (cùng Dockerfile,
hay cùng image dựng sẵn) là bản sao: một service. Dựng từ nhiều chỗ khác nhau
(monorepo, hai ứng dụng cùng có `api`) thì mỗi chỗ một service, nguồn ghi rõ
thư mục, và ghi chú lại. Service compose và workload cùng tên dựng từ cùng một
chỗ thành một service hai nguồn; không cùng thì là hai service (`Vote` và
`VoteK8s`).

## NetworkPolicy

Luật mạng trong cụm thường đã có sẵn ở dạng NetworkPolicy. archctl so nó với
`link` theo hai chiều:

```
✗ VI PHẠM  Worker -> Db  (cạnh mạng không có link, #accepted)
  k8s/np.yaml:17  NetworkPolicy `db` cho phép

⚠ LINK BỊ NETWORKPOLICY CHẶN  link Frontend -> Shoppingassistantservice
  .arch/system.arch:42  — NetworkPolicy chặn mọi đường từ workload của Frontend tới workload của Shoppingassistantservice
```

- **Cụm cho phép mà không có `link`: vi phạm.** Một luật `ingress.from` (hay
  `egress.to`) có `podSelector` chọn workload của một service đã khai là một
  đường mạng được mở rõ; nó cần `link` như cạnh lấy từ biến môi trường. Agent
  nới NetworkPolicy để mở đường mới thì bị bắt. `init --from-code` sinh `link`
  cho các đường này.
- **Có `link` mà cụm chặn: cảnh báo, `--strict` tính.** Workload bị cô lập
  theo một chiều khi có policy cùng chiều chọn nó; khi đó chỉ đường mà luật
  của các policy đó cho phép mới đi được. `link A -> B` bị báo khi mọi cặp
  workload của A và B đều bị chặn, ở chiều vào của B hay chiều ra của A.

Đọc theo tài liệu Kubernetes: không ghi `policyTypes` thì luôn cô lập chiều
vào, cô lập chiều ra khi mục `egress` là danh sách không rỗng (`egress: []` thì
không); luật không ghi nguồn (đích) là mọi pod; `podSelector: {}` là mọi pod; `matchLabels` và `matchExpressions` (`In`,
`NotIn`, `Exists`, `DoesNotExist`); `podSelector` không kèm `namespaceSelector`
là namespace của chính policy. `namespaceSelector` theo
`kubernetes.io/metadata.name` (nhãn Kubernetes tự gắn) thì biết là namespace
nào; theo nhãn khác thì repo không cho biết, nên khi xét "bị chặn" coi như cho
phép, còn khi xét "cho phép rõ" vẫn so `podSelector` với mọi workload. `ipBlock`
là địa chỉ, không phải pod của repo. Cổng (`ports`) không được xét.

Một pod chỉ bị coi là cô lập khi biết chắc nó cùng namespace với policy (cả
hai cùng ghi một namespace, hay cùng không ghi): manifest ứng dụng thường để
namespace cho `kubectl apply -n`, và default-deny của namespace `monitoring`
không được làm mọi `link` thành "bị chặn". Còn khi xét đường policy cho phép
rõ, phép khớp vẫn rộng (namespace thiếu ở một bên là khớp), để không lách được
bằng cách ghi thêm namespace.

Không xét: pod không bị policy nào chọn (mặc định của Kubernetes là mở, và
`link` vẫn được kiểm theo cấu hình như cũ); đường cho mọi pod (`from: []`,
`podSelector: {}`), vì nó không nêu service nào. Mỗi cặp service một vi phạm,
ở luật đầu tiên cho phép; peer cho ra cùng tập workload chỉ tính một lần, và
việc xét có trần (2 000 000 bước, vượt thì báo "chưa đọc được").

Trên Online Boutique, component `network-policies` chặn mọi đường vào
(deny-all) mà không mở đường cho `shoppingassistantservice`, trong khi biến môi
trường của frontend trỏ tới nó: `check` báo đúng chỗ lệch đó.

## Không đọc

- Helm template, overlay của Kustomize, Terraform: không render. Ai cần thì
  khai `verify` gọi `helm template`, `terraform graph`, kube-linter, checkov.
- Ingress, Istio: đợt sau. gRPC của Java, C#, C++: đợt sau. Node qua
  `makeGenericClientConstructor`, nice-grpc, ts-proto; client Node gán vào biến
  rồi mới `new`.
- Cổng trong NetworkPolicy, policy của CNI riêng (Cilium, Calico).
- `$(VAR)` trong lệnh (`command`, `args`), và trong env khi biến lấy từ
  `fieldRef` hay Secret: không thay, không đoán.
- Cạnh chỉ có lúc chạy: URL ghép trong mã, service discovery, biến lấy từ
  secret store, `network_mode`, ConfigMap không có trong repo.

## Trong hook, `init`, sơ đồ

**Hook.** `archctl check <file>` với file mã thuộc một component chỉ kiểm
`runs` theo import của file đó. Nó đọc lại cấu hình của cả repo khi file vừa
đổi có thể là thứ cấu hình trỏ tới: file compose, Dockerfile, manifest và mọi
file YAML, `.env`, script, `package.json`; mọi file không phải mã (trừ tài liệu và ảnh), vì `env_file`,
`extends.file`, `dockerfile:` đặt tên tuỳ ý; file mã không thuộc component
nào, vì đó là chỗ của điểm vào (`main.py` ở gốc); và `pyproject.toml`,
`setup.py`, `setup.cfg`, vì chúng đổi chỗ tìm module. Repo chưa khai `service`
nào thì không đọc compose, không tốn gì.

**`init --from-code`** sinh `service`, `runs` và `link` (từ compose và manifest
Kubernetes). `runs` là mọi
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

File compose, Dockerfile và manifest là của người khác hay do agent viết, và bộ
đọc chạy trong hook. Chốt chặn (threat model TM-8 tới TM-12, TM-18 tới TM-22):

- không chạy lệnh nào — lệnh khởi chạy chỉ được tách thành chữ;
- **không bao giờ in giá trị biến môi trường** (compose, manifest, ConfigMap;
  `Secret` không được đọc): chỉ `KHOÁ → máy:cổng`, và tên
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
  đọc. Manifest: đọc tối đa 32 MB và 10 000 file YAML cho cả repo (lọc thô theo
  chữ `apiVersion`, `kind` trước khi phân tích), mỗi file 2 000 tài liệu,
  50 000 mục, 8 MB sau khi trải alias, 256 nhãn mỗi pod; cả repo 20 000
  workload, Service, ConfigMap, 20 000 cạnh mạng, 2 000 kustomization, 500 000
  bước so selector, lần `resources` và áp patch; selector tra bằng bảng băm
  theo cặp nhãn, đích của một tên máy tính một lần cho mỗi workload, patch viết
  thẳng đọc một lần dù alias nhân nó lên. Đường dẫn trong `skaffold.yaml` và
  `kustomization.yaml` đi qua cùng chốt chặn như trên. Vượt trần ở file compose
  hay Dockerfile thì file đó vào mục "chưa đọc được"; ở lệnh khởi chạy thì
  service vào mục "không rõ điểm vào"; ở manifest thì phần còn lại vào mục
  "chưa đọc được".

Thứ không đọc được thì báo ra, không im lặng bỏ qua.

## Bộ đo

`bench/repos.tsv` có `example-voting-app` (compose năm service, stage
Dockerfile thừa hưởng `WORKDIR`, bind mount) và `open-webui` (ứng dụng LLM tự
host: `build: .`, `COPY --from=build`, điểm vào qua `start.sh` rồi `uvicorn`).
`microservices-demo` (Online Boutique) có manifest Kubernetes, `skaffold.yaml`,
bản phát hành trong `release/`, chart Helm và component Kustomize; mười một
microservice gọi nhau bằng gRPC qua địa chỉ trong env. Luật `init` sinh ra phải
tự nhất quán; cạnh mạng và `runs` bắt buộc phải có, và gỡ chúng đi thì `check`
phải báo (`consistency.DEPLOY`). `oracle.py --require deploy` so dữ kiện từng
file compose và Dockerfile với PyYAML và dockerfile-parse, và dữ kiện từng
manifest cùng mọi cạnh qua Service với `bench/k8s_truth.py` (PyYAML, cài lại
luật DNS và selector theo tài liệu Kubernetes); `sabotage.py` phá từng phép
kiểm để chắc nó đổ được.
