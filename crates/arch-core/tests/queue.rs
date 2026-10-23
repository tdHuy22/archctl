//! Cạnh qua hàng đợi và pub/sub: chỗ gửi ghép với chỗ nhận cùng hệ và tên.

use arch_core::check::{Reason, Report, UnresolvedReason, Via, check, check_files};
use std::path::PathBuf;

// Đồng hồ thời gian CPU của luồng, dùng chung với test trong lib.
#[path = "../src/testclock.rs"]
#[allow(dead_code)]
mod testclock;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir().join(format!("archctl-queue-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn write(&self, rel: &str, body: &str) -> &Self {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    fn arch(&self, extra: &str) -> &Self {
        let old = std::fs::read_to_string(self.0.join(".arch/system.arch")).unwrap_or_default();
        self.write(".arch/system.arch", &format!("{old}{extra}"))
    }

    fn doc(&self) -> arch_core::Document {
        let src = std::fs::read_to_string(self.0.join(arch_core::ARCH_FILE)).unwrap();
        let (doc, diags) = arch_core::load(&src);
        assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
        doc
    }

    fn report(&self) -> Report {
        check(&self.0, &self.doc(), &[])
    }

    fn partial(&self, files: &[&str]) -> Report {
        let files: Vec<String> = files.iter().map(|f| f.to_string()).collect();
        check_files(&self.0, &self.doc(), &files, true)
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn queue(r: &Report) -> Vec<String> {
    r.violations
        .iter()
        .filter(|v| v.via == Via::Queue)
        .map(|v| format!("{:?} {} -> {} {}:{}", v.reason, v.from, v.to, v.file, v.line))
        .collect()
}

fn unmatched(r: &Report) -> Vec<String> {
    r.unresolved
        .iter()
        .filter(|u| u.reason == UnresolvedReason::Queue)
        .map(|u| format!("{}:{} {}", u.file, u.line, u.spec))
        .collect()
}

/// Python gửi qua Kafka, Go nhận.
fn shop(name: &str) -> Repo {
    let r = Repo::new(name);
    r.write("api/orders.py", "from confluent_kafka import Producer\n\nTOPIC = 'orders'\n\ndef place(p, v):\n    p.produce(TOPIC, v)\n")
        .write(
            "worker/main.go",
            "package main\n\nimport \"github.com/segmentio/kafka-go\"\n\nfunc main() {\n\tr := kafka.NewReader(kafka.ReaderConfig{Topic: \"orders\"})\n\t_ = r\n}\n",
        )
        .arch("component Api at api/**\ncomponent Worker at worker/**\ncomponent Lib at lib/**\nallow Api -> Lib\n");
    r
}

#[test]
fn kafka_thieu_allow_la_vi_pham() {
    let r = shop("kafka");
    assert_eq!(queue(&r.report()), ["NoAllow Api -> Worker api/orders.py:6"]);
    let v = &r.report().violations[0];
    assert!(v.spec.contains("kafka:orders"), "{}", v.spec);
    r.arch("allow Api -> Worker\n");
    assert!(queue(&r.report()).is_empty());
}

/// Hook chỉ có một phía: phía bên kia nằm ở file khác, vẫn phải tìm ra.
#[test]
fn hook_tim_phia_ben_kia() {
    let r = shop("hook");
    assert_eq!(queue(&r.partial(&["api/orders.py"])), ["NoAllow Api -> Worker api/orders.py:6"]);
    assert_eq!(queue(&r.partial(&["worker/main.go"])), ["NoAllow Api -> Worker api/orders.py:6"]);
}

/// Hook sửa một bên gửi: bên gửi khác cùng topic ở file khác không bị kéo vào.
#[test]
fn hook_khong_keo_ben_gui_khac() {
    let r = shop("hook-other");
    r.write("audit/a.py", "from confluent_kafka import Producer\n\ndef f(p):\n    p.produce('orders', 1)\n")
        .arch("component Audit at audit/**\nallow Audit -> Lib\n");
    assert_eq!(queue(&r.partial(&["api/orders.py"])), ["NoAllow Api -> Worker api/orders.py:6"]);
}

/// Cùng tên ở hai hệ khác nhau không ghép (TM-41).
#[test]
fn khac_he_khong_ghep() {
    let r = Repo::new("systems");
    r.write("a/a.py", "import redis\n\ndef f(r):\n    r.publish('orders', 1)\n")
        .write("b/b.py", "from kafka import KafkaConsumer\n\nc = KafkaConsumer('orders')\n")
        .arch("component A at a/**\ncomponent B at b/**\ncomponent Lib at lib/**\nallow A -> Lib\n");
    assert!(queue(&r.report()).is_empty());
}

/// RabbitMQ qua exchange mặc định và exchange có tên; Redis theo mẫu; Celery
/// theo tên task mặc định.
#[test]
fn rabbit_redis_celery() {
    let r = Repo::new("rrc");
    r.write("api/__init__.py", "")
        .write(
            "api/send.py",
            "import pika, redis\n\ndef f(ch, r, app):\n    ch.basic_publish('', 'jobs', b'x')\n    ch.basic_publish(exchange='logs', routing_key='', body=b'')\n    r.publish('news.sport', 'x')\n    app.send_task('tasks.charge')\n",
        )
        .write("jobs/j.js", "const amqp = require('amqplib')\nasync function f(ch) { await ch.consume('jobs', cb) }\n")
        .write("logs/l.go", "package l\n\nimport amqp \"github.com/rabbitmq/amqp091-go\"\n\nfunc f(ch *amqp.Channel) { ch.QueueBind(q.Name, \"\", \"logs\", false, nil) }\n")
        .write("news/n.py", "import redis\n\ndef f(p):\n    p.psubscribe('news.*')\n")
        .write("src/tasks.py", "from celery import shared_task\n\n@shared_task\ndef charge(x):\n    pass\n")
        .arch("component Api at api/**\ncomponent Jobs at jobs/**\ncomponent Logs at logs/**\ncomponent News at news/**\ncomponent Tasks at src/**\ncomponent Lib at lib/**\nallow Api -> Lib\n");
    let mut got = queue(&r.report());
    got.sort();
    assert_eq!(
        got,
        [
            "NoAllow Api -> Jobs api/send.py:4",
            "NoAllow Api -> Logs api/send.py:5",
            "NoAllow Api -> News api/send.py:6",
            "NoAllow Api -> Tasks api/send.py:7",
        ]
    );
}

/// Tên không cố định thì báo "chưa ghép", không đoán (TM-40); hằng số cùng
/// file thì lần ra.
#[test]
fn ten_dong_thi_bao() {
    let r = Repo::new("dynamic");
    r.write("a/a.py", "from kafka import KafkaProducer\nimport os\n\ndef f(producer, t):\n    producer.send(t, 1)\n    producer.send(f'orders-{t}', 1)\n    producer.send(os.getenv('TOPIC'), 1)\n")
        .write("b/b.py", "from kafka import KafkaConsumer\n\nc = KafkaConsumer('orders')\n")
        .arch("component A at a/**\ncomponent B at b/**\ncomponent Lib at lib/**\nallow A -> Lib\n");
    let got = unmatched(&r.report());
    assert_eq!(got.len(), 3, "{got:?}");
    assert!(got[0].contains("a/a.py:5") && got[0].contains("tên không cố định `t`"), "{got:?}");
    assert!(got[2].contains("biến môi trường `TOPIC`"), "{got:?}");
    assert!(queue(&r.report()).is_empty());
}

/// Bên nhận chỉ có trong test thì nói ra, không coi như bên nhận thật (TM-41).
#[test]
fn ben_nhan_chi_trong_test() {
    let r = Repo::new("fake");
    r.write("a/a.py", "import redis\n\ndef f(r):\n    r.publish('news', 1)\n")
        .write("b/tests/test_b.py", "import redis\n\ndef test(p):\n    p.subscribe('news')\n")
        .arch("component A at a/**\ncomponent B at b/**\ncomponent Lib at lib/**\nallow A -> Lib\n");
    let report = r.report();
    assert!(queue(&report).is_empty());
    assert!(unmatched(&report).iter().any(|u| u.contains("chỉ thấy bên nhận trong file test")), "{:?}", unmatched(&report));
}

/// Hai service khác nhau cần `link`; `link` dùng cho hàng đợi là link được thấy.
#[test]
fn hai_service_can_link() {
    let r = shop("link");
    r.write(
        "docker-compose.yml",
        "services:\n  api:\n    build: .\n    command: python api/orders.py\n  worker:\n    image: worker\n",
    )
    .write("Dockerfile", "FROM python\nWORKDIR /src\nCOPY . .\n")
    .arch("allow Api -> Worker\nservice Api from compose:api runs Api\nservice Worker from compose:worker runs Worker\n");
    let report = r.report();
    let links: Vec<String> =
        report.violations.iter().filter(|v| v.reason == Reason::NotLinked).map(|v| format!("{} -> {} {:?}", v.from, v.to, v.via)).collect();
    assert_eq!(links, ["Api -> Worker Queue"], "{:?}", report.violations);
    r.arch("link Api -> Worker\n");
    let report = r.report();
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    assert!(report.links_unseen.is_empty(), "{:?}", report.links_unseen);
}

/// TM-42: rất nhiều chỗ gửi không làm hook chậm.
#[test]
fn nhieu_cho_gui_khong_cham() {
    let r = Repo::new("many");
    let mut src = String::from("import redis\n\ndef f(r):\n");
    for i in 0..20_000 {
        src.push_str(&format!("    r.publish('ch{}', 1)\n", i % 5000));
    }
    r.write("a/a.py", &src)
        .write("b/b.py", "import redis\n\ndef f(p):\n    p.psubscribe('ch*')\n")
        .arch("component A at a/**\ncomponent B at b/**\nallow A -> B\n");
    let t = testclock::Clock::start();
    let report = r.partial(&["a/a.py"]);
    assert!(t.elapsed().as_secs_f64() < 10.0, "{:?}", t.elapsed());
    assert!(queue(&report).is_empty());
    assert!(unmatched(&report).iter().any(|u| u.contains("quá")), "trần mỗi file phải báo");
}

/// Tên topic lấy từ env khai trong compose: lần ra qua service chạy component.
/// Khoá mang dấu hiệu bí mật thì không lần, không in giá trị (TM-43).
#[test]
fn ten_tu_env_cua_compose() {
    let r = shop("env");
    r.write(
        "api/orders.py",
        "from confluent_kafka import Producer\nimport os\n\ndef place(p, v):\n    p.produce(os.getenv('ORDERS_TOPIC'), v)\n    p.produce(os.environ['KAFKA_TOKEN'], v)\n",
    )
    .write(
        "docker-compose.yml",
        "services:\n  api:\n    build: .\n    command: python api/orders.py\n    environment:\n      ORDERS_TOPIC: ${ORDERS_TOPIC:-orders}\n      KAFKA_TOKEN: orders\n  worker:\n    image: worker\n",
    )
    .write("Dockerfile", "FROM python\nWORKDIR /src\nCOPY . .\n");
    // Chưa khai service: chỉ một giá trị trong cả cấu hình thì vẫn lần ra.
    let report = r.report();
    assert_eq!(queue(&report), ["NoAllow Api -> Worker api/orders.py:5"]);
    assert!(report.violations[0].spec.contains("ORDERS_TOPIC=orders"), "{}", report.violations[0].spec);
    let un = unmatched(&report);
    assert!(un.iter().any(|u| u.contains("api/orders.py:6") && u.contains("KAFKA_TOKEN")), "{un:?}");
    assert!(un.iter().all(|u| !u.contains("orders)")), "không in giá trị của khoá bí mật: {un:?}");
    // Hook sửa file gửi: lần env trước rồi mới tìm bên nhận.
    assert_eq!(queue(&r.partial(&["api/orders.py"])), ["NoAllow Api -> Worker api/orders.py:5"]);
    // Khai service: lấy env của đúng service chạy component.
    r.arch("service Api from compose:api runs Api\nservice Worker from compose:worker runs Worker\n");
    assert_eq!(queue(&r.report()), ["NoAllow Api -> Worker api/orders.py:5", "NotLinked Api -> Worker api/orders.py:5"]);
}

/// RabbitMQ: tên queue đọc từ env giữ đúng loại (queue hay exchange).
#[test]
fn rabbit_ten_tu_env() {
    let r = Repo::new("rabbit-env");
    r.write("a/a.js", "const amqp = require('amqplib')\nasync function f(ch) { ch.sendToQueue(process.env.JOBS, b) }\n")
        .write("b/b.py", "import pika\n\ndef f(ch):\n    ch.basic_consume(queue='jobs', on_message_callback=cb)\n")
        .write("docker-compose.yml", "services:\n  a:\n    image: a\n    environment:\n      - JOBS=jobs\n")
        .arch("component A at a/**\ncomponent B at b/**\ncomponent Lib at lib/**\nallow A -> Lib\n");
    assert_eq!(queue(&r.report()), ["NoAllow A -> B a/a.js:2"]);
}

/// Env khai trong manifest Kubernetes, kể cả lấy từ ConfigMap.
#[test]
fn ten_tu_env_cua_k8s() {
    let r = shop("k8s-env");
    r.write(
        "api/orders.py",
        "from confluent_kafka import Producer\nimport os\n\ndef place(p, v):\n    p.produce(os.getenv('ORDERS_TOPIC'), v)\n",
    )
    .write(
        "k8s/api.yaml",
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\nspec:\n  template:\n    spec:\n      containers:\n      - name: api\n        image: api\n        env:\n        - name: ORDERS_TOPIC\n          valueFrom:\n            configMapKeyRef: {name: cfg, key: topic}\n---\napiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: cfg\ndata:\n  topic: orders\n",
    )
    .write("k8s/worker.yaml", "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: worker\nspec:\n  template:\n    spec:\n      containers:\n      - name: w\n        image: worker\n")
    .arch("service Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\nlink Api -> Worker\n");
    assert_eq!(queue(&r.report()), ["NoAllow Api -> Worker api/orders.py:5"]);
}

/// `init --from-code` sinh `allow` cho cạnh qua hàng đợi và `link` giữa
/// service; nâng lên `#accepted` thì không tự vi phạm. Bên nhận không bị kéo
/// vào `runs` của bên gửi.
#[test]
fn init_sinh_allow_va_link() {
    let r = Repo::new("init");
    r.write("api/__init__.py", "")
        .write("api/main.py", "import redis\n\ndef f(r):\n    r.publish('news', 1)\n")
        .write("worker/__init__.py", "")
        .write("worker/main.py", "import redis\n\ndef f(p):\n    p.subscribe('news')\n")
        .write(
            "docker-compose.yml",
            "services:\n  api:\n    build: .\n    command: python -m api.main\n  worker:\n    build: .\n    command: python -m worker.main\n",
        )
        .write("Dockerfile", "FROM python\nWORKDIR /src\nCOPY . .\n");
    let found = arch_core::discover::discover(&r.0, &[]);
    let name = |dir: &str| found.components.iter().find(|(_, g)| g.starts_with(dir)).map(|(c, _)| c.clone()).expect(dir);
    let (api, worker) = (name("api"), name("worker"));
    assert_eq!(found.queue_edges, [(api.clone(), worker.clone())]);
    let web = found.services.iter().find(|s| s.runs.contains(&api)).expect("service chạy api");
    assert_eq!(web.runs.len(), 1, "hàng đợi không phải chạy chung process: {:?}", web.runs);
    assert!(found.links.iter().any(|(a, _)| *a == web.name), "{:?}", found.links);
    let law = arch_core::discover::render(&found, "2026-10-19").replace("#draft", "#accepted");
    assert!(law.contains("# Hàng đợi, pub/sub"), "{law}");
    r.write(".arch/system.arch", &law);
    let report = r.report();
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

/// TM-42: hook sửa file có nhiều mẫu `psubscribe`, repo có nhiều mã thường:
/// lọc file theo tiền tố của mẫu, không glob từng từ của repo.
#[test]
fn psubscribe_trong_hook_khong_cham() {
    let r = Repo::new("psub");
    let mut sub = String::from("import redis\n\ndef f(p):\n");
    for i in 0..2000 {
        sub.push_str(&format!("    p.psubscribe('events.{i}.*')\n"));
    }
    r.write("b/sub.py", &sub)
        .write("a/pub.py", "import redis\n\ndef f(r):\n    r.publish('events.7.created', 1)\n")
        .arch("component A at a/**\ncomponent B at b/**\ncomponent Lib at lib/**\nallow A -> Lib\n");
    let filler: String = (0..20_000).map(|i| format!("value_{i} = compute_{i}(alpha, beta, gamma, delta)\n")).collect();
    for k in 0..20 {
        r.write(&format!("a/filler{k}.py"), &filler);
    }
    let t = testclock::Clock::start();
    let report = r.partial(&["b/sub.py"]);
    assert!(t.elapsed().as_secs_f64() < 10.0, "{:?}", t.elapsed());
    assert_eq!(queue(&report), ["NoAllow A -> B a/pub.py:4"]);
}

/// Pub/sub phát tới hai service: thiếu `link` thì báo đủ cả hai.
#[test]
fn pubsub_hai_ben_nhan_bao_du_link() {
    let r = Repo::new("fanout");
    r.write("api/a.py", "import redis\n\ndef f(r):\n    r.publish('news', 1)\n")
        .write("w1/a.py", "import redis\n\ndef f(p):\n    p.subscribe('news')\n")
        .write("w2/a.py", "import redis\n\ndef f(p):\n    p.subscribe('news')\n")
        .write("docker-compose.yml", "services:\n  api:\n    image: a\n  w1:\n    image: b\n  w2:\n    image: c\n")
        .arch(
            "component Api at api/**\ncomponent W1 at w1/**\ncomponent W2 at w2/**\nallow Api -> W1, W2\n\
             service Api from compose:api runs Api\nservice W1 from compose:w1 runs W1\nservice W2 from compose:w2 runs W2\n",
        );
    let report = r.report();
    let mut links: Vec<String> =
        report.violations.iter().filter(|v| v.reason == Reason::NotLinked).map(|v| format!("{} -> {}", v.from, v.to)).collect();
    links.sort();
    assert_eq!(links, ["Api -> W1", "Api -> W2"]);
}

/// `init --from-code` lần tên từ env như `check`: nâng luật lên `#accepted`
/// thì không tự vi phạm.
#[test]
fn init_lan_ten_tu_env() {
    let r = Repo::new("init-env");
    r.write("api/__init__.py", "")
        .write("api/main.py", "from confluent_kafka import Producer\nimport os\n\ndef f(p):\n    p.produce(os.getenv('ORDERS_TOPIC'), 1)\n")
        .write("worker/__init__.py", "")
        .write("worker/main.py", "from kafka import KafkaConsumer\n\nc = KafkaConsumer('orders')\n")
        .write(
            "docker-compose.yml",
            "services:\n  api:\n    build: .\n    command: python -m api.main\n    environment:\n      ORDERS_TOPIC: orders\n  worker:\n    build: .\n    command: python -m worker.main\n",
        )
        .write("Dockerfile", "FROM python\nWORKDIR /src\nCOPY . .\n");
    let found = arch_core::discover::discover(&r.0, &[]);
    assert_eq!(found.queue_edges.len(), 1, "{:?}", found.queue_edges);
    let law = arch_core::discover::render(&found, "2026-10-19").replace("#draft", "#accepted");
    r.write(".arch/system.arch", &law);
    let report = r.report();
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

/// TM-43: giá trị dựng từ `$(DB_PASSWORD)` trong manifest không thành tên topic.
#[test]
fn k8s_gia_tri_tu_secret_khong_in() {
    let r = shop("k8s-secret");
    r.write(
        "api/orders.py",
        "from confluent_kafka import Producer\nimport os\n\ndef place(p, v):\n    p.produce(os.getenv('ORDERS_TOPIC'), v)\n",
    )
    .write(
        "worker/main.go",
        "package main\n\nimport \"github.com/segmentio/kafka-go\"\n\nfunc main() {\n\tr := kafka.NewReader(kafka.ReaderConfig{Topic: \"hunter2xyz\"})\n\t_ = r\n}\n",
    )
    .write(
        "k8s/api.yaml",
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\nspec:\n  template:\n    spec:\n      containers:\n      - name: api\n        image: api\n        env:\n        - name: DB_PASSWORD\n          value: hunter2xyz\n        - name: ORDERS_TOPIC\n          value: $(DB_PASSWORD)\n",
    )
    .arch("service Api from k8s:api runs Api\n");
    let report = r.report();
    let all = format!("{:?} {:?}", report.violations, report.unresolved);
    assert!(!all.contains("hunter2xyz"), "{all}");
    assert!(queue(&report).is_empty());
}

/// Hook sửa file gửi Redis: bên nhận theo mẫu `psubscribe` ở file khác vẫn được
/// tìm ra, dù file đó không chứa đúng tên kênh.
#[test]
fn hook_ben_gui_thay_ben_nhan_theo_mau() {
    let r = Repo::new("psub-hook");
    r.write("api/pub.py", "import redis\n\ndef f(r):\n    r.publish('orders.created', 1)\n")
        .write("worker/sub.py", "import redis\n\ndef f(p):\n    p.psubscribe('orders.*')\n")
        .arch("component Api at api/**\ncomponent Worker at worker/**\ncomponent Lib at lib/**\nallow Api -> Lib\n");
    assert_eq!(queue(&r.report()), ["NoAllow Api -> Worker api/pub.py:4"]);
    assert_eq!(queue(&r.partial(&["api/pub.py"])), ["NoAllow Api -> Worker api/pub.py:4"]);
}

/// NATS: Python gửi `orders.new`, Go nhận theo mẫu `orders.*`; Redis cùng tên
/// không ghép; hook ở bên gửi tìm ra bên nhận theo mẫu ở file khác.
#[test]
fn nats_theo_mau_subject() {
    let r = Repo::new("nats");
    r.write("api/orders.py", "import nats\n\nasync def place(nc):\n    await nc.publish('orders.new', b'x')\n")
        .write(
            "worker/main.go",
            "package main\n\nimport \"github.com/nats-io/nats.go\"\n\nfunc main(nc *nats.Conn) {\n\tnc.QueueSubscribe(\"orders.*\", \"w\", nil)\n}\n",
        )
        .write("audit/a.py", "import redis\n\ndef f(r):\n    r.subscribe('orders.new')\n")
        .arch("component Api at api/**\ncomponent Worker at worker/**\ncomponent Audit at audit/**\ncomponent Lib at lib/**\nallow Api -> Lib\nallow Audit -> Lib\n");
    assert_eq!(queue(&r.report()), ["NoAllow Api -> Worker api/orders.py:4"]);
    assert_eq!(queue(&r.partial(&["api/orders.py"])), ["NoAllow Api -> Worker api/orders.py:4"]);
    assert_eq!(queue(&r.partial(&["worker/main.go"])), ["NoAllow Api -> Worker api/orders.py:4"]);
    // `orders.*` không khớp `orders.new.eu`.
    r.write("api/orders.py", "import nats\n\nasync def place(nc):\n    await nc.publish('orders.new.eu', b'x')\n");
    assert!(queue(&r.report()).is_empty());
}

/// aio-pika gửi qua exchange mặc định, kombu nhận queue đó.
#[test]
fn aio_pika_toi_kombu() {
    let r = Repo::new("aio-kombu");
    r.write(
        "api/send.py",
        "import aio_pika\n\nasync def send(ch):\n    await ch.default_exchange.publish(aio_pika.Message(b'x'), routing_key='jobs')\n",
    )
    .write("worker/recv.py", "from kombu import Consumer, Queue\n\ndef run(conn):\n    with Consumer(conn, queues=[Queue('jobs')]):\n        pass\n")
    .arch("component Api at api/**\ncomponent Worker at worker/**\ncomponent Lib at lib/**\nallow Api -> Lib\n");
    assert_eq!(queue(&r.report()), ["NoAllow Api -> Worker api/send.py:4"]);
    assert_eq!(queue(&r.partial(&["api/send.py"])), ["NoAllow Api -> Worker api/send.py:4"]);
}
