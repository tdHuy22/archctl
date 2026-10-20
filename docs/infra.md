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

## Cạnh qua hàng đợi

Bên gửi ghi vào topic `orders`, bên nhận đọc topic đó. Hai bên không import
nhau và không gọi tên máy của nhau, nên cả đồ thị import lẫn cạnh mạng đều không
thấy dữ liệu đi từ đâu tới đâu. archctl dò chỗ gửi và chỗ nhận, rồi ghép theo
hệ và tên:

```
✗ VI PHẠM  Api -> Worker  (không được phép, #accepted)
  api/orders.py:5  gửi qua hàng đợi `p.produce("orders", v) (kafka:orders)`
  luật: không có allow nào cho Api -> Worker
```

| Hệ | Bên gửi | Bên nhận | Ghép theo |
| --- | --- | --- | --- |
| Kafka | Python: `produce(topic)` (confluent-kafka), `send`, `send_and_wait` (kafka-python, aiokafka); Go: `sarama.ProducerMessage{Topic:}`, `kafka.Message{Topic:}`, `kafka.Writer{Topic:}` (kafka-go, confluent); Node: `producer.send({topic})`, `sendBatch` (kafkajs) | Python: `subscribe([…])`, `KafkaConsumer("t", …)`; Go: `ConsumePartition`, `group.Consume(ctx, []string{…}, h)`, `kafka.ReaderConfig{Topic:, GroupTopics:}`, `SubscribeTopics`, `Subscribe`; Node: `consumer.subscribe({topic \| topics})` | topic |
| RabbitMQ | Python: `basic_publish(exchange, routing_key)` (pika); Go: `Publish`, `PublishWithContext` (amqp091-go, streadway); Node: `sendToQueue(q)`, `publish(exchange, key)` (amqplib) | `basic_consume(queue)`, `Consume(queue, …)`, `consume(q)`; `queue_bind`, `QueueBind`, `bindQueue` nhận từ exchange | queue (qua exchange mặc định `""`), hay exchange |
| Redis pub/sub | `publish(kênh, tin)` (redis-py, go-redis, node-redis, ioredis) | `subscribe(kênh, …)`; `psubscribe("news.*")` khớp theo mẫu | kênh |
| Celery | `send_task("shop.tasks.charge")` | `@app.task`, `@shared_task` (tên mặc định `<module>.<hàm>` theo đường dẫn file; `name=` thì theo tên đó) | tên task |

`app.task.delay()` và `apply_async()` gọi qua hàm đã import, nên cạnh đã có từ
đồ thị import; chỉ `send_task` theo tên chuỗi là cạnh mới.

- **Luật.** Như gRPC: cạnh bên gửi → bên nhận cần `allow`; đã khai `service`
  và hai component chạy ở hai service khác nhau thì cần thêm `link`. Cạnh mạng
  tới broker (Kafka, RabbitMQ, Redis) vẫn đi qua phép kiểm tên máy ở trên.
- **Tên.** Chuỗi cố định, hay hằng số khai trong cùng file (`TOPIC = "orders"`,
  `const Topic = "orders"`, `self.topic = "orders"`). Hằng số không theo phạm
  vi, nên tên gán hai giá trị khác nhau, hay còn được ràng buộc ở chỗ khác
  (tham số hàm, biến vòng lặp, `as`, gán nhiều vế, `+=`), là không cố định:
  `def send(p, topic)` không lấy `topic = "audit"` của hàm khác. Phép gán nằm
  trong ngoặc (keyword argument xuống dòng) không phải hằng số. Queue khai rồi
  dùng qua biến cũng lần ra: `q, err := ch.QueueDeclare("hello", …)` rồi
  `q.Name`, `const q = await ch.assertQueue("hello")` rồi `q.queue`,
  `r = ch.queue_declare(queue="hello")` rồi `r.method.queue`. Thuộc tính viết
  tắt của JS (`producer.send({ topic, messages })`) lấy theo hằng số cùng tên;
  Go `msg.Topic = "orders"` lấy vai theo struct gán cho `msg`
  (`ProducerMessage{}` là gửi, `ReaderConfig{}` là nhận; struct khác thì bỏ). Tên đọc từ biến môi trường (`os.getenv("TOPIC")`,
  `os.environ["TOPIC"]`, `os.Getenv("TOPIC")`, `process.env.TOPIC`) được lần
  qua `environment` của compose và `env` của manifest (kể cả ConfigMap) của
  service chạy component đó; luật chưa khai service nào chạy nó thì chỉ nhận
  khi cả cấu hình có đúng một giá trị. Chỉ giá trị dạng tên (chữ, số, `.`,
  `_`, `-`, `:`), và không bao giờ ở khoá mang dấu hiệu bí mật (`…_KEY`,
  `…_TOKEN`, `…_PASSWORD`). Còn lại (biến, f-string, nối chuỗi, tên từ module
  khác) là "không ghép được" (`--strict` tính): không đoán. Lời gọi đã nhận ra
  mà không thấy tên (`producer.send(rec)`, `subscribe({ fromBeginning: true })`),
  `getattr(p, "produce")`, tham chiếu phương thức không gọi (`send =
  p.produce`) cũng vậy.
- **Chỉ khi chắc là hàng đợi.** `send`, `publish`, `subscribe`, `Consume` chỉ
  tính khi file import thư viện của hệ đó; riêng `send`, `subscribe` của Kafka
  (trùng tên với `res.send`, `ws.send`, `obs.subscribe`) chỉ báo tên không cố
  định khi biến chắc là producer, consumer (tên có `produc`, `consum`, hay gán
  từ `KafkaProducer(…)`, `kafka.producer()`); tên cố định thì vẫn ghép; lời gọi đặc trưng (`basic_publish`,
  `send_task`, `sendToQueue`, `ConsumePartition`, `produce` với tên cố định)
  thì luôn tính. Cùng tên ở hai hệ khác nhau (Kafka `orders`, Redis `orders`)
  không ghép.
- **Không tính.** Bên nhận trong file test hay mock (như gRPC); bên gửi tới
  tên mà mọi bên nhận đều là giả thì báo "không ghép được". Chỗ tên không cố
  định trong file test, mock thì không báo. Bên gửi không có
  bên nhận nào trong repo (topic do hệ khác đọc) như gói ngoài.
- **Hook.** Khi file đổi có chỗ gửi hay nhận, archctl lần tên từ env trước,
  rồi tìm phía bên kia trong repo một lượt (lọc file theo tên topic, tên hàm
  task, tiền tố chữ cố định của mẫu `psubscribe`, và file có `psubscribe` khi
  file đổi gửi qua Redis; trần 64 MB như gRPC). Phía bên kia dùng tên lấy từ env thì hook không
  tìm ra; lần kiểm cả repo thì thấy.
- **Trần.** 10 000 chỗ gửi, nhận mỗi file, 200 000 cả lần kiểm, 1 000 mẫu
  `psubscribe`; quá thì báo. Ngoặc khớp tính một lần cho mỗi file, nên file lồng
  sâu không làm chậm theo bậc hai.
- **`init --from-code`** sinh `allow` cho cạnh qua hàng đợi (một mục riêng
  trong luật, chỉ tên cố định) và `link` giữa service. **`scan`** in từng chỗ
  gửi, nhận với hệ và tên (JSON: `queues`).

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

- **Helm:** chart (`Chart.yaml`, `values.yaml`, `templates/`, chart con trong
  `charts/`) được archctl tự dựng, không chạy `helm`, rồi đọc như manifest
  thường ở thư mục chart: `k8s:helm-chart/cartservice`. Dòng trong vi phạm là
  dòng của template. Giá trị lấy từ `values.yaml` của chart; luật khai thêm
  file values bằng nguồn `values:` cùng service:
  `service Web from k8s:chart/web, values:deploy/prod.yaml` (gộp sâu như Helm,
  file sau thắng, áp cho chart ở thư mục các nguồn `k8s:` của service đó; nguồn
  `k8s:` không nêu thư mục chỉ dùng được khi repo có đúng một chart; file
  khai mà không có là "chưa đọc được"). `.Release.Name` là `release-name`,
  namespace `default`, như `helm template`.

  Dựng được: `.Values`, `.Release`, `.Chart`, `.Template`, biến (`$x :=`,
  `$`), `if`/`else if`/`else`, `with`, `range` (list, map, số), `define`,
  `include`, `template`, `block`, chú thích, `{{-`/`-}}`, và các hàm `toYaml`,
  `toJson`, `nindent`, `indent`, `quote`, `squote`, `default`, `required`,
  `empty`, `not`, `and`, `or`, `coalesce`, `ternary`, `eq`, `ne`, `lt`…,
  `print`, `printf` (`%s %d %v %q`), `toString`, `int`, `lower`, `upper`,
  `title`, `trim`, `trimSuffix`, `trimPrefix`, `trunc`, `replace`, `contains`,
  `hasPrefix`, `hasSuffix`, `len`, `list`, `dict`, `get`, `hasKey`, `keys`,
  `join`, `first`, `index`, `kindIs`, `kindOf`, `b64enc`. Chart con dựng như
  Helm: mỗi mục `dependencies` là một chart con (cùng chart dưới hai `alias`
  là hai chart, `.Chart.Name` là alias); `values.yaml` của chart con gộp lên
  chart cha dưới khoá alias (chart cha thắng) cùng `.Values.global`, rồi mới
  xét `condition` và `tags`; chart con tắt không góp values hay `define`.
  Chart con khai mà không có trong `charts/` (chưa `helm dependency build`)
  là "chưa đọc được".

  Thứ ngoài tập con (`tpl`, `lookup`, `.Files`, `.Capabilities`,
  `semverCompare`, hàm sinh ngẫu nhiên, hàm lạ), chart con nén `.tgz`, hay
  template hỏng làm cả file vào mục "chưa đọc được" (`--strict` tính): không
  đoán nhánh nào. Template là input đối kháng chạy trong hook: mỗi chart tối
  đa 2 000 000 bước dựng, 8 MB dựng ra, `include`, khối và giá trị dựng bằng
  `list`/`dict` lồng tối đa 64 tầng, chart con lồng tối đa 8 tầng,
  200 000 giá trị đọc từ values (TM-36 tới TM-39). Không in manifest dựng ra
  hay giá trị values; lỗi chỉ có file, dòng, tên hàm.
- **Kustomize:** mỗi kustomization gốc (không bị kustomization khác trỏ tới qua
  `resources`, `bases`, `components`, và không phải `kind: Component`) là một
  bản dựng: archctl tự dựng nó như `kustomize build`, không chạy lệnh, rồi đọc
  như manifest ở thư mục kustomization gốc (`k8s:overlays/prod/cart`). Base
  vẫn được đọc như manifest thường; bản dựng là một bản sao nữa của workload,
  kiểm `link`, `runs` như mọi bản. Kustomization gốc nằm cùng thư mục với
  manifest của nó (`kubernetes-manifests/`) thì bản dựng thay bản đọc thô. Bản
  dựng tra tên máy, ConfigMap, selector của Service trong chính nó trước;
  không thấy thì tra manifest thô mà không bản dựng nào kéo vào (DB, hàng
  đợi khai riêng, chart Helm), không bao giờ tra bản thô của base hay bản dựng
  khác. Manifest thô không tra vào bản dựng, trừ file nằm ở thư mục
  gốc của bản dựng mà kustomization không gồm (như `loadgenerator.yaml` triển
  khai riêng qua skaffold): nó tra tên như bản dựng đó, mà với mọi chỗ khác
  vẫn là manifest thô. NetworkPolicy áp theo
  namespace: policy thô không thuộc bản dựng áp lên cả workload của bản dựng.
  Áp: `resources`, `components`, `patches` (strategic merge theo tên container,
  env, volume, cổng; JSON 6902 `add`, `replace`, `remove`), `patchesStrategicMerge`,
  `patchesJson6902`, `namespace`, `namePrefix`/`nameSuffix` (cập nhật cả tham
  chiếu ConfigMap, Secret, ServiceAccount, backend Ingress như Kustomize; tên
  máy trong env thì không, như Kustomize), `commonLabels`, `labels`
  (`includeSelectors`, `includeTemplates`), `images`, `configMapGenerator`
  (`literals`, `envs`, `files`, `behavior`; sinh trước component, như
  Kustomize), `replicas`. `target` của patch khớp tên, namespace gốc hay hiện
  tại (trống là `default`). Patch không phải map, patch không `target` mà
  thiếu `kind` hay `metadata.name`, mục patch thiếu nội dung: cả bản dựng
  "chưa đọc được", như Kustomize báo lỗi. `k8s:cart` khớp
  `prod-cart` do `namePrefix: prod-` sinh ra; dòng trong vi phạm là dòng của
  file gây ra giá trị: patch, file env, kustomization, hay manifest gốc.
  Không đọc `secretGenerator`. Ngoài tập con (`replacements`, `vars`,
  `helmCharts`, generator, transformer, `configurations`, directive `$patch`,
  `target` theo regex hay `annotationSelector`, resource từ xa, đường dẫn ra
  ngoài dự án; resource từ xa chỉ in tên máy chủ, không in token trong URL)
  thì cả bản dựng "chưa đọc được" (`--strict` tính), và patch của
  các kustomization nó với tới được báo như trước; không áp một phần rồi im.
  Trần: kustomization lồng 16 tầng, phát hiện vòng, 5 000 tài nguyên một bản
  dựng, 200 bản dựng và 20 000 tài nguyên cả repo, 2 000 000 bước dựng chung,
  1 000 000 đơn vị cây chung (một node, cộng một cho mỗi 16 byte khoá và
  chuỗi; kể cả base dùng lại, bản chép patch, ConfigMap sinh ra), cây sâu 128
  cấp kể cả qua alias (TM-44 tới TM-47).
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

## Lối vào từ Internet

```
link Internet -> Frontend
```

`Internet` là tên dành riêng: lộ một service ra ngoài là một cạnh mạng từ
`Internet`, và như mọi `link`, không có `link` là cấm. Không khai `service
Internet` được, và `Internet` chỉ đứng ở đầu `link` (gọi ra dịch vụ ngoài thì
khai `service` cho dịch vụ đó).

```
✗ VI PHẠM  Internet -> Db  (cạnh mạng không có link, #accepted)
  k8s/db.yaml:12  Service `db-ext` type: LoadBalancer
  → thêm `link Internet -> Db`, hoặc bỏ lối vào này
```

Thứ tính là lộ ra ngoài:

| Nguồn | Dạng |
| --- | --- |
| compose | `ports:` publish ra mọi giao diện mạng (`"8080:80"`, `"80"`, dạng dài không có `host_ip`), `network_mode: host`. Cổng gắn vào `127.0.0.1`, `localhost`, `::1` (kể cả `${BIND:-127.0.0.1}`) và `expose:` thì không |
| Service | `type: LoadBalancer`, `NodePort`, hay có `externalIPs` |
| Ingress | `rules[].http.paths[].backend`, `defaultBackend` (v1), `backend.serviceName` (v1beta1) |
| Gateway API | `HTTPRoute`, `GRPCRoute`, `TCPRoute`, `TLSRoute`, `UDPRoute` qua `backendRefs`; route chỉ gắn vào Service (mesh, GAMMA) thì không |
| Istio | `VirtualService` gắn với một `Gateway` khác `mesh`, qua `destination.host` |
| Pod | `hostNetwork: true`, `hostPort` |

Từ Service, route tới workload đi qua selector như cạnh mạng. Có `link Internet
-> X` mà không cấu hình nào lộ X thì là "link không thấy trong cấu hình",
`--strict` tính. Route trỏ tới Service không có trong repo thì như gói ngoài.
`init --from-code` sinh `link Internet -> …` cho mọi service lộ ra; `export`,
`serve` vẽ một nút `Internet`.

Chưa xét: NetworkPolicy chặn đường từ ingress controller (phải biết pod của nó
nằm đâu), `Gateway` của Gateway API không có listener nào, cổng của
`LoadBalancer` bị `loadBalancerSourceRanges` thu hẹp.

## Cạnh mạng viết trong mã

Cạnh mạng lấy từ cấu hình có một lối lách: viết thẳng địa chỉ của service khác
vào mã thì không dòng cấu hình nào đổi. Vì vậy `check` đọc cả chuỗi trong mã
Python, JS/TS, Go, Rust, C/C++. Chuỗi trỏ tới một service của repo là cạnh mạng
từ mọi service chạy component chứa file đó (theo `runs`), và cần `link` như cạnh
lấy từ biến môi trường:

```
✗ VI PHẠM  Vote -> Result  (cạnh mạng không có link, #accepted)
  vote/app.py:54  chuỗi → result:80
  luật: không có link nào cho Vote -> Result (tên máy viết trong mã)
  → thêm `link Vote -> Result`, hoặc bỏ địa chỉ này khỏi mã
```

Chuỗi được đọc theo đúng luật tên máy của biến môi trường (mục "Cạnh mạng" ở
trên). Chỗ biến môi trường có tên khoá, ở đây tên khoá là chỗ chuỗi đứng:

| Chuỗi | Ví dụ | Tính? |
|---|---|---|
| URL có `scheme://` | `requests.get("http://result:80/api")`, `'postgres://u:p@db/x'`, `mongodb://a:1,db:2/x` | có, phần `user:mật khẩu@` bị bỏ |
| `máy:cổng` | `Addr: "cart:7070"`, `"db:5432/x"`, `("db", 5432)` | có; không có khoá kiểu máy thì cổng phải từ 80 trở lên |
| cổng ghép lúc chạy | `f"http://db:{port}"`, `` `http://db:${port}` ``, `"http://db:" + port`; không có URL thì `"db:%d" % p`, `"db:{port}"`, hay chuỗi ở khoá kiểu máy (`db_host = f"db:{p}"`) | có, tên máy không kèm cổng; `f"cache:{key}"`, `"login:%s"` thì không: đó là khoá Redis, log |
| chuỗi trần ở khoá kiểu máy | `Redis(host="redis")`, `{"hostname": "db"}`, `redisHost := "cache"`, `host: str = "db"`, `char host[] = "db"`, `HOSTS = ["a", "db"]` | có, khi cả chuỗi là một tên máy |
| giá trị mặc định | `os.getenv("DB_HOST", "db")`, `cfg.get("host", "db")`, `getenv("X_HOST", default="db")`, `process.env.DB_HOST \|\| "db"`, `host = x or "db"`, `host: str = field(default="db")` | có |
| chuỗi kết nối | `"Server=tcp:db,1433;…"`, `"host=db port=5432 …"` (libpq), `"root:pw@tcp(db:3306)/x"` | có, từng cặp `khoá=giá trị` |
| hàm nhận tên máy | `net.JoinHostPort("db", p)`, `socket.gethostbyname("db")` | có |
| chuỗi ba nháy làm giá trị, `#define DB_URL "…"` của C | `connect("""http://db:5432""")` | có; docstring thì không |
| chuỗi trần ở chỗ khác | `"db"`, `name = "cache"`, `if x == "db"`, `print("API:")` | không: chữ thường trùng tên service quá nhiều |

Khoá kiểu máy là tên tham số, thuộc tính, biến được gán (kể cả có kiểu) hay khoá
chuỗi của dict có đoạn cuối `host`, `addr`, `server`, `url`, `uri`, `endpoint`,
`dsn`, `broker` (cả số nhiều, cả camelCase `redisHost`); `service`, `cluster`
thì không, vì trong mã chúng thường là nhãn. Chuỗi trần ở khoá `url`,
`endpoint` phải có dấu chấm: `endpoint="login"`, `LOGIN_URL = "login"` là tên
route. Khoá nói về bí mật (`password`, `apiKey`, `token`, và riêng trong mã
`pw`, `login`) và khoá nhãn (`image`, `tag`, `version`) chỉ đọc URL, trừ khi
đoạn cuối của nó là kiểu máy (`auth_server`). Khoá nói về phía gọi vào
(`ALLOWED_HOSTS`, `CORS_ORIGINS`, `bind`, `listen`) không đọc gì: đó không phải
cạnh đi ra. Cặp `("db", 5432)` tính khi là đối số (`connect((…))`) hay giá trị
của một tên kiểu máy (`addr = (…)`), không tính `CACHE = ("cache", 3600)`. Chuỗi trần chỉ tính khi cả chuỗi là một tên máy: `url =
"data:image/png;base64,"` không có máy nào tên `base64`, và câu `"Hostname of
the redis server"` cũng không. Ký tự thoát được giải mã (`"d\x62"` là `db`),
chuỗi liền nhau của Python và C được ghép như trình dịch (`"http://" "db:5432"`).

Tên máy được ghép với cấu hình như biến môi trường: tên service compose và
alias, tên Service Kubernetes (`svc`, `svc.ns` khi `ns` là namespace của repo,
`svc.ns.svc.cluster.local`, qua selector tới workload). Service compose chỉ thấy
tên trong cùng project, workload chỉ thấy Service của cụm, và ưu tiên Service
cùng thư mục với nó: mã của `vote` chạy bằng compose gọi `result` là service
compose `result`, không phải workload `result` trong manifest; `app1/` và
`app2/` của một monorepo có Service `api` riêng. Tên không có trong repo
(`api.openai.com`, `localhost`) thì như gói ngoài. Đích chưa khai trong luật là
vi phạm, như cạnh từ cấu hình. Có `link` thì cạnh từ mã cũng làm `link` đó
"thấy trong cấu hình". Nhiều chỗ trong một file tới cùng một service là một vi
phạm, kèm số chỗ.

Không xét file test và mock (`tests/`, `test_*.py`, `*_test.go`, `*.spec.ts`,
`conftest.py`, `mocks/`…): chúng không chạy trong service nào mà hay có địa chỉ
để thử. Thư mục ví dụ, tài liệu, fixture thì vẫn xét, vì mã chạy thật có thể
import chúng. File không phải UTF-8 vẫn được đọc. Chuỗi gốc không bao giờ được
in: `check` chỉ in `khoá → máy:cổng` với máy là tên một service; `scan` cũng chỉ
in tên máy khớp một service của repo, `scan --all-hosts` in mọi ứng viên.

Chưa đọc: `f"db:{port}"` không có URL hay khoá kiểu máy (không phân biệt được
với khoá Redis), tên máy ghép lúc chạy (`"http://%s:5432" % host`, `Sprintf("%s",
"db")`, nối hai nửa tên, biến), chuỗi trong mã của `{...}` của f-string, chuỗi
trong các ngôn ngữ archctl chưa đọc (Java, C#, Ruby); đối số đầu của một lời gọi
được gán (`db_host = Field("db")`, `Redis("db", 6379)`), `return "db"`,
`case "db":`, khoá tính toán của JS (`{["host"]: "db"}`), `new Map([["host",
"db"]])`, alias của `links:` trong compose (`db:database`), `\N{…}`.

## Không đọc

- Terraform: không render; Kustomize và template Helm ngoài tập con ở trên. Ai
  cần thì khai `verify` gọi `kustomize build`, `helm template`, `terraform
  graph`, kube-linter, checkov.
- Hàng đợi: hàm bọc tự viết quanh producer (`publish_event("orders")`), tên
  topic từ module khác, kombu,
  aio-pika, Faust, NATS, SQS, Pub/Sub của Google; Kafka subscribe theo regex
  (báo "không ghép được"); routing key của exchange `topic`, `direct` (mọi
  queue gắn vào exchange đều coi là bên nhận).
- gRPC của Java, C#, C++: đợt sau. Node qua
  `makeGenericClientConstructor`, nice-grpc, ts-proto; client Node gán vào biến
  rồi mới `new`.
- Cổng trong NetworkPolicy, policy của CNI riêng (Cilium, Calico).
- `$(VAR)` trong lệnh (`command`, `args`), và trong env khi biến lấy từ
  `fieldRef` hay Secret: không thay, không đoán.
- Cạnh chỉ có lúc chạy: tên máy ghép trong mã (f-string, nối chuỗi, biến),
  service discovery, biến lấy từ secret store, `network_mode`, ConfigMap không
  có trong repo.
- Chuỗi trong mã Java, C#, Kotlin, Ruby: archctl chưa đọc các ngôn ngữ đó
  (`worker/Program.cs` của voting app). Các dạng chuỗi chưa đọc khác: mục "Cạnh
  mạng viết trong mã".

## Trong hook, `init`, sơ đồ

**Hook.** `archctl check <file>` với file mã thuộc một component chỉ kiểm
`runs` theo import của file đó. Nó đọc lại cấu hình của cả repo khi file vừa
đổi có thể là thứ cấu hình trỏ tới: file compose, Dockerfile, manifest và mọi
file YAML, `.env`, script, `package.json`; mọi file không phải mã (trừ tài liệu và ảnh), vì `env_file`,
`extends.file`, `dockerfile:` đặt tên tuỳ ý; file mã không thuộc component
nào, vì đó là chỗ của điểm vào (`main.py` ở gốc); và `pyproject.toml`,
`setup.py`, `setup.cfg`, vì chúng đổi chỗ tìm module. File mã có chuỗi dạng tên
máy (URL, `máy:cổng`, chuỗi ở khoá kiểu máy) thì hook đọc cấu hình chỉ để biết
tên đó trỏ tới service nào, không xét lại cả cấu hình; đo trên open-webui và
Online Boutique, hook vẫn dưới 10 ms, repo cỡ 30 000 file thì thêm khoảng
0,1 s để tìm file cấu hình. Repo chưa khai `service` nào thì không
đọc compose, không tốn gì.

**`init --from-code`** sinh `service`, `runs` và `link` (từ compose, manifest
Kubernetes và tên máy viết trong mã). `runs` là mọi
component mà điểm vào chạm tới theo các cạnh đã quan sát — đúng điều `check`
đòi, nên bản nháp nâng lên `#accepted` thì không tự vi phạm.

**`export`** vẽ service trong một subgraph "Triển khai", `link` thành cạnh
"mạng". **`serve`** vẽ service thành hộp bo tròn, cạnh "chạy trong" nét chấm
tới các component trong `runs`. **`scan`** in mọi thứ bộ đọc thấy: dữ kiện
từng file, lệnh khởi chạy dẫn tới file nào, cạnh mạng, tên máy trong chuỗi của
từng file mã (`khoá → máy:cổng`, đánh dấu file test, ví dụ không xét). Mặc định nó chỉ in tên
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
  "chưa đọc được". Chuỗi trong mã (TM-32 tới TM-35) được đọc hết, dài bao nhiêu
  cũng vậy (cắt ở một độ dài là chỗ để giấu một URL), tuyến tính theo cỡ file;
  mỗi file tối đa 1 000 tên máy (quá thì file vào mục "chưa đọc được"); khử
  trùng và tra đích bằng bảng băm, tính vào ngân sách bước chung; f-string lồng
  quá 32 tầng thì đọc phẳng, không đệ quy.

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
luật DNS và selector theo tài liệu Kubernetes). Tên máy trong chuỗi của mã
Python và Go được so với `bench/code_hosts.py`: chuỗi và chỗ nó đứng lấy từ
`ast`, `tokenize` và `go/ast`, luật đổi chuỗi thành tên máy viết lại theo đặc
tả; JS/TS chưa có thước đo. Voting app phải có cạnh `Vote -> Redis` từ
`Redis(host="redis")` và `Result -> Db` từ chuỗi kết nối của `result/server.js`,
và gỡ `link` thì `check` phải báo ở chính dòng mã đó. `sabotage.py` phá từng
phép kiểm để chắc nó đổ được.
