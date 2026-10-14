//! Cạnh gRPC: chỗ gọi stub ghép với chỗ đăng ký server theo `package.Service`.

use arch_core::check::{Reason, Report, UnresolvedReason, Via, check, check_files};
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir().join(format!("archctl-grpc-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn remove_file(&self, rel: &str) -> &Self {
        std::fs::remove_file(self.0.join(rel)).unwrap();
        self
    }

    fn write(&self, rel: &str, body: &str) -> &Self {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
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

fn rpc(r: &Report) -> Vec<String> {
    r.violations
        .iter()
        .filter(|v| v.via == Via::Rpc)
        .map(|v| format!("{:?} {} -> {} {}:{}", v.reason, v.from, v.to, v.file, v.line))
        .collect()
}

/// Python: client trong `app/client`, server trong `app/server`, mã sinh ra
/// trong `app/gen`, `.proto` trong `proto/`.
fn shop(name: &str) -> Repo {
    let r = Repo::new(name);
    r.write("proto/kv.proto", "syntax = \"proto3\";\npackage shop.v1;\nservice KV { rpc Get(A) returns (B); }\n")
        .write("app/__init__.py", "")
        .write("app/gen/__init__.py", "")
        .write("app/gen/kv_pb2_grpc.py", "class KVStub(object):\n    pass\ndef add_KVServicer_to_server(s, srv):\n    pass\n")
        .write("app/client/__init__.py", "")
        .write("app/client/c.py", "from app.gen import kv_pb2_grpc\n\ndef get(ch):\n    return kv_pb2_grpc.KVStub(ch)\n")
        .write("app/server/__init__.py", "")
        .write(
            "app/server/s.py",
            "from app.gen.kv_pb2_grpc import add_KVServicer_to_server\n\ndef serve(impl, s):\n    add_KVServicer_to_server(impl, s)\n",
        )
        .write(
            ".arch/system.arch",
            "component Client at app/client/**\ncomponent Server at app/server/**\ncomponent Gen at app/gen/**\n\
             allow Client -> Gen\nallow Server -> Gen\n",
        );
    r
}

#[test]
fn python_thieu_allow_la_vi_pham() {
    let r = shop("py");
    assert_eq!(rpc(&r.report()), ["NoAllow Client -> Server app/client/c.py:4"]);
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(".arch/system.arch", &format!("{arch}allow Client -> Server\n"));
    assert!(rpc(&r.report()).is_empty());
}

/// Hook chỉ có file client: phía server nằm ở file khác, vẫn phải tìm ra.
/// Và ngược lại: chỉ có file server mới đăng ký thì vẫn thấy client.
#[test]
fn hook_tim_phia_ben_kia() {
    let r = shop("hook");
    assert_eq!(rpc(&r.partial(&["app/client/c.py"])), ["NoAllow Client -> Server app/client/c.py:4"]);
    assert_eq!(rpc(&r.partial(&["app/server/s.py"])), ["NoAllow Client -> Server app/client/c.py:4"]);
}

#[test]
fn go_client_server() {
    let r = Repo::new("go");
    r.write(
        "api/rpc.proto",
        "package etcdserverpb;\noption go_package = \"example.com/etcd/api/etcdserverpb\";\nservice KV { rpc Range(A) returns (B); }\n",
    )
    .write("api/etcdserverpb/rpc_grpc.pb.go", "package etcdserverpb\nfunc NewKVClient(cc int) int { return 0 }\nfunc RegisterKVServer(s, srv int) {}\n")
    .write(
        "client/kv.go",
        "package client\nimport pb \"example.com/etcd/api/etcdserverpb\"\nfunc f(c int) { _ = pb.NewKVClient(c) }\n",
    )
    .write(
        "server/grpc.go",
        "package server\nimport (\n\tpb \"example.com/etcd/api/etcdserverpb\"\n\t\"example.com/etcd/adapter\"\n)\n\
         func g(s int) { pb.RegisterKVServer(s, 0); pb.RegisterKVHandlerServer(s, 0); _ = adapter.NewKVFromKVClient(0) }\n",
    )
    .write(".arch/system.arch", "component Client at client/**\ncomponent Server at server/**\ncomponent Api at api/**\nallow Client -> Api\nallow Server -> Api\n");
    assert_eq!(
        rpc(&r.report()),
        ["NoAllow Client -> Server client/kv.go:3"],
        "gateway và hàm `NewXClient` không có .proto không phải gRPC"
    );
}

/// TM-15: hai component cùng phục vụ một service (proxy của etcd cũng đăng ký
/// `KV`): cạnh tới cả hai, không cạnh nào biến mất.
#[test]
fn nhieu_server_ra_nhieu_canh() {
    let r = shop("nhieu");
    r.write(
        "app/proxy/p.py",
        "from app.gen import kv_pb2_grpc\n\ndef proxy(impl, s):\n    kv_pb2_grpc.add_KVServicer_to_server(impl, s)\n",
    );
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(".arch/system.arch", &format!("{arch}component Proxy at app/proxy/**\nallow Proxy -> Gen\n"));
    let mut v = rpc(&r.report());
    v.sort();
    assert_eq!(v, ["NoAllow Client -> Proxy app/client/c.py:4", "NoAllow Client -> Server app/client/c.py:4"]);
}

/// TM-16: tên service có ở hai package mà không lần được về file `.proto`:
/// báo nhập nhằng, không đoán.
#[test]
fn ten_trung_bao_nhap_nhang() {
    let r = shop("trung");
    r.write("proto/other.proto", "package other.v1;\nservice KV { rpc Get(A) returns (B); }\n");
    // `from x import KVStub` mà x không phải module sinh ra: không biết .proto.
    r.write("app/client/c.py", "from app.gen import kv_pb2_grpc as g\nfrom app.gen.kv_pb2_grpc import KVStub\n\ndef get(ch):\n    return KVStub(ch)\n");
    let report = r.report();
    assert_eq!(rpc(&report), ["NoAllow Client -> Server app/client/c.py:5"], "lần được về kv.proto qua module");
    // Không rõ module (gọi qua thuộc tính): tên có ở hai package thì nhập nhằng.
    r.write("app/client/c.py", "class C:\n    def get(self, ch):\n        return self.pb.KVStub(ch)\n");
    let report = r.report();
    assert!(rpc(&report).is_empty(), "{:?}", rpc(&report));
    assert!(
        report.unresolved.iter().any(|u| u.reason == UnresolvedReason::Rpc && u.spec.contains("shop.v1.KV") && u.spec.contains("other.v1.KV")),
        "{:?}",
        report.unresolved
    );
}

/// Server giả trong test không phải nơi service chạy: không ra cạnh, nhưng
/// lời gọi tới service chỉ có server giả thì được nói ra (không biết nó chạy
/// thật ở đâu). Server ngoài repo thì như import gói ngoài: im lặng.
#[test]
fn server_gia_va_server_ngoai_repo() {
    let r = shop("gia");
    std::fs::remove_file(r.0.join("app/server/s.py")).unwrap();
    r.write(
        "app/client/test_c.py",
        "from app.gen import kv_pb2_grpc\n\ndef test_x(impl, s):\n    kv_pb2_grpc.add_KVServicer_to_server(impl, s)\n",
    );
    r.write(
        "app/client/otel.py",
        "from opentelemetry.proto.collector.trace.v1 import trace_service_pb2_grpc\n\ndef f(ch):\n    return trace_service_pb2_grpc.TraceServiceStub(ch)\n",
    );
    let report = r.report();
    assert!(rpc(&report).is_empty(), "{:?}", rpc(&report));
    let unresolved: Vec<&str> =
        report.unresolved.iter().filter(|u| u.reason == UnresolvedReason::Rpc).map(|u| u.spec.as_str()).collect();
    assert_eq!(unresolved.len(), 1, "{unresolved:?}");
    assert!(unresolved[0].contains("shop.v1.KV") && unresolved[0].contains("test hay mock"), "{unresolved:?}");
}

/// Server thật nằm trong thư mục tên như `integration`, `mockingbird` vẫn là
/// server thật: danh sách thư mục giả không được nuốt nó.
#[test]
fn thu_muc_ten_giong_test_van_la_server_that() {
    let r = shop("mockingbird");
    let s = std::fs::read_to_string(r.0.join("app/server/s.py")).unwrap();
    std::fs::remove_file(r.0.join("app/server/s.py")).unwrap();
    r.write("app/server/mockingbird/s.py", &s).write("app/server/integration/s2.py", &s);
    assert_eq!(rpc(&r.report()), ["NoAllow Client -> Server app/client/c.py:4"]);
}

/// Hook với file compose: `link` chỉ có lý do là gRPC không bị báo "không
/// thấy trong cấu hình" chỉ vì file gọi gRPC không nằm trong tập đang xét.
#[test]
fn hook_compose_khong_bao_nham_link() {
    let r = shop("hooklink");
    r.write(
        "docker-compose.yml",
        "services:\n  web:\n    build: .\n    command: python -m app.client.c\n  kv:\n    build: .\n    command: python -m app.server.s\n",
    )
    .write("Dockerfile", "FROM python\nWORKDIR /src\nCOPY . .\n");
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(
        ".arch/system.arch",
        &format!("{arch}allow Client -> Server\nservice Web from compose:web runs Client, Gen\nservice Kv from compose:kv runs Server, Gen\nlink Web -> Kv\n"),
    );
    assert!(r.report().links_unseen.is_empty());
    let hook = r.partial(&["docker-compose.yml"]);
    assert!(hook.links_unseen.is_empty(), "{:?}", hook.links_unseen);
    assert!(hook.violations.is_empty(), "{:?}", hook.violations);
}

/// Hook trên file Go gọi `rest.NewRESTClient`: không phải gRPC (không có
/// `.proto`), nên không đi đọc cả repo và không báo "dừng ở trần".
#[test]
fn ham_new_client_thuong_khong_lam_hook_doc_ca_repo() {
    let r = Repo::new("rest");
    r.write("a/a.go", "package a\nimport \"k8s.io/client-go/rest\"\nfunc f() { _ = rest.NewRESTClient(nil) }\n")
        .write(".arch/system.arch", "component A at a/**\ncomponent B at b/**\nallow A -> B\n");
    for i in 0..3 {
        r.write(&format!("b/f{i}.go"), "package b\nfunc RegisterRESTServer() {}\n");
    }
    let report = r.partial(&["a/a.go"]);
    assert!(!report.unresolved.iter().any(|u| u.reason == UnresolvedReason::Rpc), "{:?}", report.unresolved);
}

/// Đã khai `service`: component client và server chạy ở hai service khác nhau
/// thì cần `link`.
#[test]
fn hai_service_can_link() {
    let r = shop("link");
    r.write(
        "docker-compose.yml",
        "services:\n  web:\n    build: .\n    command: python -m app.client.c\n  kv:\n    build: .\n    command: python -m app.server.s\n",
    )
    .write("Dockerfile", "FROM python\nWORKDIR /src\nCOPY . .\n");
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(
        ".arch/system.arch",
        &format!("{arch}allow Client -> Server\nservice Web from compose:web runs Client, Gen\nservice Kv from compose:kv runs Server, Gen\n"),
    );
    let report = r.report();
    let links: Vec<String> =
        report.violations.iter().filter(|v| v.reason == Reason::NotLinked).map(|v| format!("{} -> {} {:?}", v.from, v.to, v.via)).collect();
    assert_eq!(links, ["Web -> Kv Rpc"]);
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(".arch/system.arch", &format!("{arch}link Web -> Kv\n"));
    let report = r.report();
    assert!(report.violations.is_empty(), "{:?}", rpc(&report));
    assert!(report.links_unseen.is_empty(), "link dùng cho gRPC là link được thấy: {:?}", report.links_unseen);
}

/// `init --from-code` sinh `allow` cho cạnh gRPC và `link` giữa service; nâng
/// lên `#accepted` thì không tự vi phạm. Cạnh gRPC không kéo server vào `runs`.
#[test]
fn init_sinh_allow_va_link() {
    let r = shop("init");
    r.write(
        "docker-compose.yml",
        "services:\n  web:\n    build: .\n    command: python -m app.client.c\n  kv:\n    build: .\n    command: python -m app.server.s\n",
    )
    .write("Dockerfile", "FROM python\nWORKDIR /src\nCOPY . .\n");
    let found = arch_core::discover::discover(&r.0, &[]);
    assert!(found.rpc_edges.iter().any(|(a, b)| a == "Client" && b == "Server"), "{:?}", found.rpc_edges);
    let web = found.services.iter().find(|s| s.name == "Web").expect("Web");
    assert!(!web.runs.contains(&"Server".to_string()), "gRPC không phải chạy chung process: {:?}", web.runs);
    assert!(found.links.iter().any(|(a, b)| a == "Web" && b.contains(&"Kv".to_string())), "{:?}", found.links);
    let law = arch_core::discover::render(&found, "2026-10-12").replace("#draft", "#accepted");
    r.write(".arch/system.arch", &law);
    let report = r.report();
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

/// TM-13: `.proto` đối kháng và rất nhiều file không làm treo hook.
#[test]
fn proto_doi_khang_khong_lam_treo() {
    let r = shop("doikhang");
    r.write("proto/deep.proto", &format!("service S {{ {} }}", "{".repeat(500_000)));
    let many: String = (0..5000).map(|i| format!("service S{i} {{ rpc A(B) returns (C); }}\n")).collect();
    r.write("proto/many.proto", &many);
    let start = std::time::Instant::now();
    let report = r.partial(&["app/client/c.py"]);
    assert!(start.elapsed() < std::time::Duration::from_secs(5), "{:?}", start.elapsed());
    assert_eq!(rpc(&report), ["NoAllow Client -> Server app/client/c.py:4"]);
}

/// TM-13: hàng chục nghìn chỗ gọi tới service khác nhau, cộng hàng trăm file
/// `.proto` mỗi file một nghìn service: từng mất 140 giây trong hook và 311
/// giây khi kiểm cả repo (bảng tra quét tuyến tính, tìm chuỗi theo từng tên).
#[test]
fn nhieu_loi_goi_va_nhieu_proto_khong_lam_treo() {
    let r = shop("dosrpc");
    let calls: String = (0..20_000).map(|i| format!("    kv_pb2_grpc.S{i}Stub(ch)\n")).collect();
    r.write("app/client/c.py", &format!("from app.gen import kv_pb2_grpc\n\ndef get(ch):\n    kv_pb2_grpc.KVStub(ch)\n{calls}"));
    for f in 0..100 {
        let services: String = (0..1000).map(|i| format!("service X{f}_{i} {{}}\n")).collect();
        r.write(&format!("proto/many/p{f}.proto"), &format!("package many{f};\n{services}"));
    }
    for f in 0..200 {
        r.write(&format!("app/server/m{f}.py"), &"x = 1\n".repeat(1500));
    }
    for (label, report) in [("cả repo", None), ("hook", Some(["app/client/c.py"]))] {
        let start = std::time::Instant::now();
        let report = match report {
            None => r.report(),
            Some(files) => r.partial(&files),
        };
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "{label}: {:?}", start.elapsed());
        assert_eq!(rpc(&report), ["NoAllow Client -> Server app/client/c.py:4"], "{label}");
    }
}

/// TM-12: phía server được đăng ký theo cách archctl không đọc (qua biến tên
/// lạ, `getattr`) nhưng vẫn có lớp cài đặt `KVServicer`: nói ra, không im
/// lặng để mọi cạnh tới nó biến mất. Gán hàm đăng ký vào biến thì vẫn thấy.
#[test]
fn server_bi_giau_thi_bao() {
    let r = shop("giau");
    r.write(
        "app/server/s.py",
        "from app.gen import kv_pb2_grpc as g\n\nclass Impl(g.KVServicer):\n    pass\n\ndef serve(s):\n    getattr(g, 'add_' + 'KV' + 'Servicer_to_server')(Impl(), s)\n",
    );
    let report = r.report();
    assert!(rpc(&report).is_empty());
    assert!(
        report.unresolved.iter().any(|u| u.reason == UnresolvedReason::Rpc && u.spec.contains("không thấy chỗ đăng ký")),
        "{:?}",
        report.unresolved
    );
    r.write("app/server/s.py", "from app.gen import kv_pb2_grpc as g\n\nreg = g.add_KVServicer_to_server\n\ndef serve(impl, s):\n    reg(impl, s)\n");
    assert_eq!(rpc(&r.report()), ["NoAllow Client -> Server app/client/c.py:4"]);
    // Generic handler: tên đầy đủ trong chuỗi.
    r.write("app/server/s.py", "import grpc\n\ndef serve(s, h):\n    s.add_generic_rpc_handlers((grpc.method_handlers_generic_handler(\"shop.v1.KV\", h),))\n");
    assert_eq!(rpc(&r.report()), ["NoAllow Client -> Server app/client/c.py:4"]);
}

/// TM-16: module sinh ra của thư viện ngoài (`etcd3.etcdrpc.rpc_pb2_grpc`) mà
/// repo không có `rpc.proto`: không ghép vào service `KV` của repo.
#[test]
fn module_ngoai_khong_ghep_nham() {
    let r = shop("ngoai");
    r.write("app/client/c.py", "import etcd3.etcdrpc.rpc_pb2_grpc as etcdrpc\n\ndef get(ch):\n    return etcdrpc.KVStub(ch)\n");
    let report = r.report();
    assert!(rpc(&report).is_empty(), "{:?}", rpc(&report));
    assert!(!report.unresolved.iter().any(|u| u.reason == UnresolvedReason::Rpc), "{:?}", report.unresolved);
}


/// Server Node nạp `.proto` lúc chạy, client Go: cạnh Checkout → Currency của
/// Online Boutique.
#[test]
fn node_server_go_client() {
    let r = Repo::new("node");
    r.write("protos/demo.proto", "syntax = \"proto3\";\npackage hipstershop;\noption go_package = \"example.com/genproto\";\nservice CurrencyService { rpc Convert(A) returns (B); }\nservice PaymentService { rpc Charge(A) returns (B); }\n")
        .write("genproto/demo_grpc.pb.go", "package hipstershop\nfunc NewCurrencyServiceClient(cc int) int { return 0 }\n")
        .write(
            "checkout/main.go",
            "package main\nimport pb \"example.com/genproto\"\nfunc f(c int) { _ = pb.NewCurrencyServiceClient(c) }\n",
        )
        .write(
            "currency/server.js",
            "const grpc = require('@grpc/grpc-js');\nconst shopProto = load('demo.proto').hipstershop;\nconst healthProto = load('health.proto').grpc.health.v1;\nserver.addService(shopProto.CurrencyService.service, {convert});\nserver.addService(healthProto.Health.service, {check});\n",
        )
        .write(
            "payment/index.ts",
            "import * as svc from './gen/demo_grpc_pb';\nserver.addService(svc.PaymentServiceService, impl);\nconst c = new Foo(1);\nconst d = new widgets.Gadget(2);\n",
        )
        .write("web/pay.js", "const client = new proto.hipstershop.PaymentService('pay:50051', creds);\n")
        .write(
            ".arch/system.arch",
            "component Checkout at checkout/**\ncomponent Currency at currency/**\ncomponent Payment at payment/**\ncomponent Web at web/**\ncomponent Gen at genproto/**\nallow Checkout -> Gen\nallow Web -> Gen\n",
        );
    let mut v = rpc(&r.report());
    v.sort();
    assert_eq!(v, ["NoAllow Checkout -> Currency checkout/main.go:3", "NoAllow Web -> Payment web/pay.js:1"]);
    // Hook chỉ có file server Node: vẫn tìm ra client Go.
    assert_eq!(rpc(&r.partial(&["currency/server.js"])), ["NoAllow Checkout -> Currency checkout/main.go:3"]);
}

/// tonic: `KV` trong `.proto` thành `kv_server::KvServer`; client Python gọi
/// server Rust. Có `impl kv_server::Kv for` mà không đăng ký thì báo.
#[test]
fn rust_tonic_server_python_client() {
    let r = shop("rust");
    r.write(
        "srv/src/main.rs",
        "use pb::kv_server::KvServer;\nfn main() { let s = KvServer::new(Impl); }\nimpl pb::kv_server::Kv for Impl {}\n",
    )
    .write("gen/src/lib.rs", "pub mod kv_server { use tonic::codegen::*; pub struct KvServer; }\npub mod kv_client { pub struct KvClient; }\n")
    .remove_file("app/server/s.py");
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(".arch/system.arch", &format!("{arch}component Srv at srv/**\ncomponent RGen at gen/**\n"));
    assert_eq!(rpc(&r.report()), ["NoAllow Client -> Srv app/client/c.py:4"]);
    // Hook chỉ có file client Python: tìm ra server Rust dù tên viết khác hoa thường.
    assert_eq!(rpc(&r.partial(&["app/client/c.py"])), ["NoAllow Client -> Srv app/client/c.py:4"]);
    // Bỏ chỗ đăng ký, giữ chỗ cài đặt: phía server bị giấu thì báo.
    r.write("srv/src/main.rs", "impl pb::kv_server::Kv for Impl {}\n");
    let rep = r.report();
    assert!(rep.unresolved.iter().any(|u| u.file == "srv/src/main.rs"), "{:?}", rep.unresolved);
}

/// Rust: struct `XServer` không đi cùng module `x_server`, hay không có
/// `.proto` khai service đó, thì không phải gRPC.
#[test]
fn rust_khong_phai_grpc_thi_khong_ghep() {
    let r = shop("rust-gia");
    r.write("srv/src/main.rs", "use http::api_server::ApiServer;\nuse a::other_server::KvServer;\nfn f() { ApiServer::new(); }\n")
        .remove_file("app/server/s.py");
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(".arch/system.arch", &format!("{arch}component Srv at srv/**\n"));
    assert!(rpc(&r.report()).is_empty(), "{:?}", rpc(&r.report()));
}

/// Dạng chuẩn của tonic: import có ngoặc nhọn, `impl Kv for` trần; file viết
/// tay dùng `tonic::codegen` vẫn là chỗ dùng; server giả trong `#[cfg(test)]`
/// không tính; `HealthRecords` không phải health check.
#[test]
fn rust_cac_dang_thuong_gap() {
    for (name, src, want) in [
        ("brace", "use pb::kv_server::{Kv, KvServer};\nfn main() { let s = KvServer::new(Impl); }\nimpl Kv for Impl {}\n", true),
        ("codegen", "use tonic::codegen::InterceptedService;\nuse pb::kv_server::KvServer;\nfn main() { KvServer::new(Impl); }\n", true),
        ("cfg-test", "fn real() {}\n#[cfg(test)]\nmod tests {\n    use pb::kv_server::KvServer;\n    fn t() { KvServer::new(Mock); }\n}\n", false),
    ] {
        let r = shop(&format!("rust-{name}"));
        r.write("srv/src/main.rs", src).remove_file("app/server/s.py");
        let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
        r.write(".arch/system.arch", &format!("{arch}component Srv at srv/**\n"));
        let got = rpc(&r.report());
        assert_eq!(!got.is_empty(), want, "{name}: {got:?}");
    }
    let r = shop("rust-health");
    r.write("proto/kv.proto", "syntax = \"proto3\";\npackage shop.v1;\nservice HealthRecords { rpc Get(A) returns (B); }\n")
        .write("app/gen/kv_pb2_grpc.py", "class HealthRecordsStub(object):\n    pass\n")
        .write("app/client/c.py", "from app.gen import kv_pb2_grpc\n\ndef get(ch):\n    return kv_pb2_grpc.HealthRecordsStub(ch)\n")
        .write("srv/src/main.rs", "use pb::health_records_server::HealthRecordsServer;\n")
        .remove_file("app/server/s.py");
    let arch = std::fs::read_to_string(r.0.join(".arch/system.arch")).unwrap();
    r.write(".arch/system.arch", &format!("{arch}component Srv at srv/**\n"));
    assert_eq!(rpc(&r.report()), ["NoAllow Client -> Srv app/client/c.py:4"]);
}

/// Node: gốc chuỗi thuộc tính là biến gán giữa chừng (`const shop =
/// load(def).shop`): chuỗi là đuôi của package, đối chiếu với `.proto`.
#[test]
fn node_package_la_duoi_cua_chuoi() {
    let r = Repo::new("node-duoi");
    r.write("proto/kv.proto", "syntax = \"proto3\";\npackage shop.v1;\nservice KV { rpc Get(A) returns (B); }\n")
        .write("srv/s.js", "server.addService(p.shop.v1.KV.service, impl);\n")
        .write("web/c.js", "const shop = load(def).shop;\nconst c = new shop.v1.KV('kv:1', creds);\nconst d = new a.b.Widget(1);\n")
        .write(".arch/system.arch", "component Srv at srv/**\ncomponent Web at web/**\nallow Web -> Srv\n");
    assert!(rpc(&r.report()).is_empty(), "{:?}", rpc(&r.report()));
    r.write(".arch/system.arch", "component Srv at srv/**\ncomponent Web at web/**\ncomponent X at x/**\nallow Web -> X\n");
    assert_eq!(rpc(&r.report()), ["NoAllow Web -> Srv web/c.js:2"]);
}
