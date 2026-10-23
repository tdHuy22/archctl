//! Terraform: tài nguyên chạy mã (ECS, Lambda, Cloud Run) thành `service`,
//! tham chiếu trong biến môi trường thành cạnh cần `link`, lối vào Internet cần
//! `link Internet -> …`; phần không đọc được thì báo, không đoán.

use arch_core::check::{Report, check, check_files};
use arch_core::scan::deploy::{self, Key};
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root = std::env::temp_dir().join(format!("archctl-tf-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn write(&self, rel: &str, body: &str) -> &Self {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    fn rules(&self, text: &str) -> &Self {
        self.write(".arch/system.arch", text)
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

    fn load(&self) -> deploy::Deploy {
        let files = arch_core::check::deploy_files(&self.0, &[]);
        deploy::load(&self.0, &files, &deploy::Options { tf: true, k8s: true, ..Default::default() })
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn violations(r: &Report) -> Vec<String> {
    r.violations.iter().map(|v| format!("{:?} {} -> {} {}:{}", v.reason, v.from, v.to, v.file, v.line)).collect()
}

const ECS: &str = r#"variable "db_password" {}
resource "aws_db_instance" "main" {
  engine   = "postgres"
  password = var.db_password
}
resource "aws_ecs_task_definition" "api" {
  family = "api"
  container_definitions = jsonencode([{
    name    = "api"
    image   = "acme/api:1"
    command = ["python", "-m", "api"]
    environment = [
      { name = "DB_HOST", value = aws_db_instance.main.address },
      { name = "DB_PASSWORD", value = "hunter2-tf" },
    ]
  }])
}
resource "aws_ecs_service" "api" {
  task_definition = aws_ecs_task_definition.api.arn
  load_balancer {
    target_group_arn = aws_lb_target_group.api.arn
  }
}
resource "aws_lb" "public" {}
resource "aws_lb_target_group" "api" {}
resource "aws_lb_listener" "http" {
  load_balancer_arn = aws_lb.public.arn
  default_action {
    target_group_arn = aws_lb_target_group.api.arn
  }
}
"#;

const PY_DOCKERFILE: &str = "FROM python:3.12\nWORKDIR /app\nCOPY . .\nCMD [\"python\", \"main.py\"]\n";

#[test]
fn ecs_toi_db_va_lo_ra_internet_can_link() {
    let r = Repo::new("ecs");
    r.write("infra/main.tf", ECS)
        .rules("service Api from tf:aws_ecs_service.api\nservice Db from tf:aws_db_instance.main\n");
    let rep = r.report();
    let v = violations(&rep);
    assert!(v.contains(&"NotLinked Api -> Db infra/main.tf:13".to_string()), "{v:?}");
    assert!(v.iter().any(|x| x.starts_with("NotLinked Internet -> Api infra/main.tf:24")), "{v:?}");
    assert!(rep.deploy_unread.is_empty(), "{:?}", rep.deploy_unread);
    r.rules("service Api from tf:aws_ecs_service.api\nservice Db from tf:aws_db_instance.main\nlink Api -> Db\nlink Internet -> Api\n");
    let rep = r.report();
    assert!(rep.violations.is_empty() && rep.missing.is_empty(), "{:?} {:?}", violations(&rep), rep.missing);
}

#[test]
fn dich_va_workload_chua_khai_la_vi_pham() {
    let r = Repo::new("chua-khai");
    r.write("infra/main.tf", ECS).rules("service Api from tf:aws_ecs_service.api\nlink Internet -> Api\n");
    let rep = r.report();
    // Cơ sở dữ liệu chưa khai: cạnh tới nó không có `link` nào cho được.
    assert!(violations(&rep).contains(&"NotLinked Api -> aws_db_instance.main infra/main.tf:13".to_string()), "{:?}", violations(&rep));
    assert!(arch_core::check::to_undeclared(&rep.violations[0]) || rep.violations.iter().any(arch_core::check::to_undeclared));
    // Luật dùng `tf:` thì mọi tài nguyên chạy mã phải được khai.
    r.write(
        "infra/worker.tf",
        "resource \"aws_lambda_function\" \"worker\" {\n  image_uri = \"acme/worker:1\"\n}\n",
    );
    let rep = r.report();
    assert!(rep.services_undeclared.iter().any(|(f, l, n)| f == "infra/worker.tf" && *l == 1 && n == "aws_lambda_function.worker"), "{:?}", rep.services_undeclared);
}

#[test]
fn hook_doc_lai_khi_sua_file_tf() {
    let r = Repo::new("hook");
    r.write("infra/main.tf", ECS).rules(
        "service Api from tf:aws_ecs_service.api\nservice Db from tf:aws_db_instance.main\nlink Api -> Db\nlink Internet -> Api\n",
    );
    assert!(r.partial(&["infra/main.tf"]).violations.is_empty());
    // Agent thêm một biến môi trường trỏ sang hàng đợi mới.
    r.write(
        "infra/main.tf",
        &ECS.replace(
            "      { name = \"DB_PASSWORD\"",
            "      { name = \"QUEUE_URL\", value = aws_sqs_queue.jobs.url },\n      { name = \"DB_PASSWORD\"",
        )
        .replace("resource \"aws_lb\" \"public\" {}", "resource \"aws_lb\" \"public\" {}\nresource \"aws_sqs_queue\" \"jobs\" {}"),
    );
    let v = violations(&r.partial(&["infra/main.tf"]));
    assert!(v.contains(&"NotLinked Api -> aws_sqs_queue.jobs infra/main.tf:14".to_string()), "{v:?}");
}

#[test]
fn dockerfile_trong_luat_thi_kiem_runs() {
    let r = Repo::new("runs");
    r.write("infra/main.tf", &ECS.replace("[\"python\", \"-m\", \"api\"]", "[\"python\", \"main.py\"]"))
        .write("api/Dockerfile", PY_DOCKERFILE)
        .write("api/main.py", "import os\n")
        .write("worker/main.py", "import os\n")
        .rules(
            "component Api at api/**\ncomponent Worker at worker/**\n\
             service ApiSvc from tf:aws_ecs_service.api, dockerfile:api/Dockerfile runs Worker\n\
             service Db from tf:aws_db_instance.main\nlink ApiSvc -> Db\nlink Internet -> ApiSvc\n",
        );
    let rep = r.report();
    // `command` của ECS thay CMD: `python -m api` chạy component Api, ngoài `runs`.
    assert!(rep.violations.iter().any(|v| format!("{:?}", v.reason) == "NotRun" && v.from == "ApiSvc" && v.to == "Api"), "{:?} {:?}", violations(&rep), rep.entry_unknown);
}

#[test]
fn secret_khong_lot_ra() {
    let r = Repo::new("secret");
    r.write("infra/main.tf", ECS).rules("service Api from tf:aws_ecs_service.api\n");
    let all = format!("{:?}{:?}", r.report(), r.load().services);
    assert!(!all.contains("hunter2-tf"), "lọt mật khẩu");
}

#[test]
fn module_trong_repo_registry_va_ra_ngoai() {
    let r = Repo::new("module");
    r.write(
        "envs/prod/main.tf",
        "module \"app\" {\n  source = \"../../modules/app\"\n  image  = \"acme/app:2\"\n}\nmodule \"vpc\" {\n  source = \"terraform-aws-modules/vpc/aws\"\n}\nmodule \"esc\" {\n  source = \"../../../outside\"\n}\n",
    )
    .write(
        "modules/app/main.tf",
        "variable \"image\" {}\nresource \"aws_lambda_function\" \"worker\" {\n  image_uri = var.image\n  environment {\n    variables = { QUEUE_URL = aws_sqs_queue.jobs.url }\n  }\n}\nresource \"aws_sqs_queue\" \"jobs\" {}\n",
    );
    let d = r.load();
    let keys: Vec<String> = d.services.iter().map(|s| s.key.text()).collect();
    assert!(keys.contains(&"tf:envs/prod/module.app.aws_lambda_function.worker".to_string()), "{keys:?}");
    let w = d.services.iter().find(|s| matches!(&s.key, Key::Tf { addr, .. } if addr == "module.app.aws_lambda_function.worker")).unwrap();
    assert_eq!(w.edges[0].target, Key::Tf { dir: "envs/prod".into(), addr: "module.app.aws_sqs_queue.jobs".into() });
    let unread: Vec<String> = d.unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(unread.iter().any(|u| u.starts_with("envs/prod/main.tf:5") && u.contains("registry")), "{unread:?}");
    assert!(unread.iter().any(|u| u.starts_with("envs/prod/main.tf:8")), "{unread:?}");
    // `tf:envs/prod/<địa chỉ>` chọn đúng module gốc; `tf:<địa chỉ>` khớp mọi gốc.
    r.rules("service W from tf:envs/prod/module.app.aws_lambda_function.worker\nservice Q from tf:module.app.aws_sqs_queue.jobs\nlink W -> Q\n");
    let rep = r.report();
    assert!(rep.missing.is_empty() && rep.violations.is_empty(), "{:?} {:?}", rep.missing, violations(&rep));
    // Phần không đọc được thì `--strict` tính (đi vào `deploy_unread`).
    assert!(!rep.deploy_unread.is_empty());
}

#[cfg(unix)]
#[test]
fn module_la_symlink_khong_doc() {
    let r = Repo::new("symlink");
    let outside = Repo::new("symlink-ngoai");
    outside.write("m/main.tf", "resource \"aws_lambda_function\" \"evil\" {\n  image_uri = \"x\"\n}\n");
    r.write("main.tf", "module \"m\" {\n  source = \"./m\"\n}\n");
    std::os::unix::fs::symlink(outside.0.join("m"), r.0.join("m")).unwrap();
    let d = r.load();
    assert!(!d.services.iter().any(|s| s.key.text().contains("evil")), "{:?}", d.services);
    assert!(d.unread.iter().any(|u| u.file.starts_with("main.tf")), "{:?}", d.unread);
}

#[test]
fn file_tf_doi_khang_khong_lam_treo() {
    let r = Repo::new("doi-khang");
    let mut locals = String::from("locals {\n  a0 = \"x\"\n");
    for i in 1..400 {
        locals.push_str(&format!("  a{i} = \"${{local.a{}}}${{local.a{}}}\"\n", i - 1, i - 1));
    }
    locals.push_str("}\n");
    let deep = format!("{locals}resource \"aws_lambda_function\" \"f\" {{\n  image_uri = \"x\"\n  environment {{\n    variables = {{ A = local.a399, B = {}1{} }}\n  }}\n}}\n", "[".repeat(40), "]".repeat(40));
    r.write("main.tf", &deep).rules("service F from tf:aws_lambda_function.f\n");
    let t = std::time::Instant::now();
    let rep = r.report();
    assert!(t.elapsed().as_secs_f64() < 10.0, "{:?}", t.elapsed());
    assert!(rep.deploy_unread.iter().any(|u| u.why.contains("dài quá")), "{:?}", rep.deploy_unread);
}

#[test]
fn data_source_khai_duoc_trong_luat() {
    let r = Repo::new("data");
    r.write(
        "main.tf",
        "data \"aws_db_instance\" \"shared\" {}\nresource \"aws_lambda_function\" \"f\" {\n  image_uri = \"x\"\n  environment {\n    variables = { DB = data.aws_db_instance.shared.address }\n  }\n}\n",
    )
    .rules("service F from tf:aws_lambda_function.f\nservice Db from tf:data.aws_db_instance.shared\nlink F -> Db\n");
    let rep = r.report();
    assert!(rep.violations.is_empty() && rep.missing.is_empty(), "{:?} {:?}", violations(&rep), rep.missing);
}

#[test]
fn source_co_query_khong_in_khoa() {
    let r = Repo::new("s3");
    r.write("main.tf", "module \"m\" {\n  source = \"s3::https://b.s3.amazonaws.com/m.zip?aws_access_key_id=AKIAXX&aws_access_key_secret=SECRETKEY123\"\n}\n");
    let all = format!("{:?}", r.load().unread);
    assert!(all.contains("registry") && !all.contains("SECRETKEY123"), "{all}");
}

#[test]
fn repo_chi_co_terraform_van_sinh_ban_nhap() {
    // Không có file mã nào: `init --from-code` vẫn sinh `service tf:` và `link`.
    let r = Repo::new("chi-tf");
    r.write("infra/main.tf", ECS);
    let found = arch_core::discover::discover(&r.0, &[]);
    assert_eq!(found.files, 0);
    let law = arch_core::discover::render(&found, "2026-10-20");
    assert!(law.contains("from tf:aws_ecs_service.api"), "{law}");
    assert!(law.lines().any(|l| l.starts_with("link Internet")), "{law}");
}

// `kubernetes_*` khai qua Terraform: dựng thành manifest rồi đi qua bộ đọc
// Kubernetes, khoá là `k8s:<thư mục module gốc>/<tên>`.
const K8S: &str = r#"resource "aws_db_instance" "main" {
  engine = "postgres"
}
resource "kubernetes_deployment_v1" "cart" {
  metadata {
    name = "cart"
  }
  spec {
    selector {
      match_labels = { app = "cart" }
    }
    template {
      metadata {
        labels = { app = "cart" }
      }
      spec {
        container {
          name  = "cart"
          image = "shop/cart:1"
          env {
            name  = "DB_HOST"
            value = aws_db_instance.main.address
          }
          env {
            name  = "DB_PASSWORD"
            value = "hunter2-k8s"
          }
        }
      }
    }
  }
}
resource "kubernetes_service_v1" "cart" {
  metadata {
    name = "cart"
  }
  spec {
    selector = kubernetes_deployment_v1.cart.spec[0].template[0].metadata[0].labels
    port {
      port = 80
    }
  }
}
resource "kubernetes_deployment_v1" "web" {
  metadata {
    name = "web"
  }
  spec {
    selector {
      match_labels = { app = "web" }
    }
    template {
      metadata {
        labels = { app = "web" }
      }
      spec {
        container {
          name  = "web"
          image = "shop/web:1"
          env {
            name  = "CART_URL"
            value = "http://${kubernetes_service_v1.cart.metadata[0].name}:80"
          }
        }
      }
    }
  }
}
resource "kubernetes_service_v1" "web" {
  metadata {
    name = "web"
  }
  spec {
    selector = { app = "web" }
    type     = "LoadBalancer"
    port {
      port = 80
    }
  }
}
"#;

const K8S_RULES: &str = "service Cart from k8s:cart\nservice Web from k8s:web\nservice Db from tf:aws_db_instance.main\n";

#[test]
fn kubernetes_qua_terraform_thanh_canh_va_loi_vao() {
    let r = Repo::new("k8s");
    r.write("infra/main.tf", K8S).rules(K8S_RULES);
    let rep = r.report();
    let v = violations(&rep);
    // Cạnh qua Service mà selector là tham chiếu tới nhãn của Deployment;
    // dòng là dòng của mục `env`, như manifest viết tay.
    assert!(v.contains(&"NotLinked Web -> Cart infra/main.tf:61".to_string()), "{v:?}");
    // Cạnh tới tài nguyên Terraform khác qua tham chiếu.
    assert!(v.contains(&"NotLinked Cart -> Db infra/main.tf:22".to_string()), "{v:?}");
    assert!(v.iter().any(|x| x.starts_with("NotLinked Internet -> Web infra/main.tf:69")), "{v:?}");
    assert!(rep.deploy_unread.is_empty(), "{:?}", rep.deploy_unread);
    r.rules(&format!("{K8S_RULES}link Web -> Cart\nlink Cart -> Db\nlink Internet -> Web\n"));
    let rep = r.report();
    assert!(rep.violations.is_empty() && rep.missing.is_empty(), "{:?} {:?}", violations(&rep), rep.missing);
    let all = format!("{rep:?}{:?}", r.load().services);
    assert!(!all.contains("hunter2-k8s"), "lọt mật khẩu");
}

#[test]
fn hook_doc_lai_kubernetes_khi_sua_file_tf() {
    let r = Repo::new("k8s-hook");
    r.write("infra/main.tf", K8S)
        .rules(&format!("{K8S_RULES}link Web -> Cart\nlink Cart -> Db\nlink Internet -> Web\n"));
    assert!(r.partial(&["infra/main.tf"]).violations.is_empty());
    // Agent cho cart gọi thẳng web.
    r.write(
        "infra/main.tf",
        &K8S.replacen(
            "          env {\n            name  = \"DB_PASSWORD\"",
            "          env {\n            name  = \"WEB\"\n            value = \"http://web\"\n          }\n          env {\n            name  = \"DB_PASSWORD\"",
            1,
        ),
    );
    let v = violations(&r.partial(&["infra/main.tf"]));
    assert!(v.contains(&"NotLinked Cart -> Web infra/main.tf:25".to_string()), "{v:?}");
}

#[test]
fn kubernetes_manifest_ingress_va_config_map() {
    let r = Repo::new("k8s-manifest");
    r.write(
        "main.tf",
        r#"resource "kubernetes_manifest" "api" {
  manifest = {
    apiVersion = "apps/v1"
    kind       = "Deployment"
    metadata   = { name = "api" }
    spec = {
      selector = { matchLabels = { app = "api" } }
      template = {
        metadata = { labels = { app = "api" } }
        spec = {
          containers = [{
            name    = "api"
            image   = "shop/api:1"
            envFrom = [{ configMapRef = { name = "api-env" } }]
          }]
        }
      }
    }
  }
}
resource "kubernetes_config_map_v1" "env" {
  metadata {
    name = "api-env"
  }
  data = {
    WORKER_URL = "http://worker:9000"
  }
}
resource "kubernetes_deployment_v1" "worker" {
  metadata {
    name = "worker"
  }
  spec {
    selector {
      match_labels = { app = "worker" }
    }
    template {
      metadata {
        labels = { app = "worker" }
      }
      spec {
        container {
          name  = "worker"
          image = "shop/worker:1"
        }
      }
    }
  }
}
resource "kubernetes_service_v1" "worker" {
  metadata {
    name = "worker"
  }
  spec {
    selector = { app = "worker" }
    port {
      port = 9000
    }
  }
}
resource "kubernetes_service_v1" "api" {
  metadata {
    name = "api"
  }
  spec {
    selector = { app = "api" }
    port {
      port = 80
    }
  }
}
resource "kubernetes_ingress_v1" "public" {
  metadata {
    name = "public"
  }
  spec {
    rule {
      http {
        path {
          path = "/"
          backend {
            service {
              name = kubernetes_service_v1.api.metadata[0].name
              port {
                number = 80
              }
            }
          }
        }
      }
    }
  }
}
"#,
    )
    .rules("service Api from k8s:api\nservice Worker from k8s:worker\n");
    let v = violations(&r.report());
    assert!(v.contains(&"NotLinked Api -> Worker main.tf:26".to_string()), "{v:?}");
    assert!(v.iter().any(|x| x.starts_with("NotLinked Internet -> Api main.tf:")), "{v:?}");
}

#[test]
fn kubernetes_chua_doc_duoc_thi_bao() {
    let r = Repo::new("k8s-chua-doc");
    r.write(
        "main.tf",
        r#"variable "envs" {}
resource "kubernetes_deployment_v1" "a" {
  metadata {
    name = "a"
  }
  spec {
    template {
      spec {
        container {
          name  = "a"
          image = "x"
          dynamic "env" {
            for_each = var.envs
            content {
              name  = env.key
              value = env.value
            }
          }
        }
      }
    }
  }
}
resource "kubernetes_horizontal_thing" "b" {
  spec {
    template {
      container {
        image = "y"
      }
    }
  }
}
resource "kubernetes_secret_v1" "s" {
  metadata {
    name = "s"
  }
  data = {
    TOKEN = "hunter2-secret"
  }
}
"#,
    )
    .rules("service A from k8s:a\n");
    let rep = r.report();
    let u: Vec<String> = rep.deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(u.iter().any(|x| x.starts_with("main.tf:12 ") && x.contains("dynamic \"env\"")), "{u:?}");
    assert!(u.iter().any(|x| x.starts_with("main.tf:24 ") && x.contains("kubernetes_horizontal_thing")), "{u:?}");
    let all = format!("{rep:?}{:?}", r.load().services);
    assert!(!all.contains("hunter2-secret"), "lọt secret");
}

#[test]
fn kubernetes_tham_chieu_vong_va_no_khong_treo() {
    let r = Repo::new("k8s-vong");
    // Hai ConfigMap trỏ nhau, và nhãn nhân đôi qua nhiều tầng tham chiếu.
    let mut tf = String::from(
        r#"resource "kubernetes_config_map_v1" "a" {
  metadata {
    name   = "a"
    labels = kubernetes_config_map_v1.b.metadata[0].labels
  }
}
resource "kubernetes_config_map_v1" "b" {
  metadata {
    name   = "b"
    labels = kubernetes_config_map_v1.a.metadata[0].labels
  }
}
resource "kubernetes_config_map_v1" "m0" {
  metadata {
    name = "m0"
  }
  data = { k = "v" }
}
"#,
    );
    for i in 1..300 {
        tf.push_str(&format!(
            "resource \"kubernetes_config_map_v1\" \"m{i}\" {{\n  metadata {{\n    name = \"m{i}\"\n  }}\n  data = merge(kubernetes_config_map_v1.m{p}.data, {{ a = kubernetes_config_map_v1.m{p}.data, b = kubernetes_config_map_v1.m{p}.data }})\n}}\n",
            p = i - 1
        ));
    }
    r.write("main.tf", &tf).rules("service A from k8s:a\n");
    let t = std::time::Instant::now();
    let rep = r.report();
    assert!(t.elapsed().as_secs_f64() < 10.0, "{:?}", t.elapsed());
    // Chạm trần thì báo, không lặng lẽ bỏ phần đó.
    let u: Vec<String> = rep.deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(u.iter().any(|x| x.starts_with("main.tf:1 ") && x.contains("vòng")), "{u:?}");
}

#[test]
fn kubernetes_gia_tri_qua_tham_chieu_qua_lon_thi_bao() {
    let r = Repo::new("k8s-lon");
    // Một ConfigMap 2 000 khoá, tài nguyên kia lấy nó 1 000 lần: hai triệu
    // đơn vị, quá trần giá trị chép qua tham chiếu.
    let mut tf = String::from("resource \"kubernetes_config_map_v1\" \"big\" {\n  metadata {\n    name = \"big\"\n  }\n  data = {\n");
    for i in 0..2000 {
        tf.push_str(&format!("    k{i} = \"v\"\n"));
    }
    tf.push_str("  }\n}\nresource \"kubernetes_config_map_v1\" \"fan\" {\n  metadata {\n    name = \"fan\"\n    labels = {\n");
    for i in 0..1000 {
        tf.push_str(&format!("      a{i} = kubernetes_config_map_v1.big.data\n"));
    }
    tf.push_str("    }\n  }\n}\n");
    r.write("main.tf", &tf).rules("service A from k8s:a\n");
    let t = std::time::Instant::now();
    let rep = r.report();
    assert!(t.elapsed().as_secs_f64() < 10.0, "{:?}", t.elapsed());
    let u: Vec<String> = rep.deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(u.iter().any(|x| x.contains("kubernetes_config_map_v1") && x.contains("đơn vị") && !x.contains("vòng")), "{u:?}");
}

/// Chép `K8S` với một thay thế, kiểm `check`.
fn k8s_variant(name: &str, from: &str, to: &str, rules: &str) -> (Vec<String>, Vec<String>) {
    let r = Repo::new(name);
    assert!(K8S.contains(from), "{from}");
    r.write("infra/main.tf", &K8S.replacen(from, to, 1)).rules(rules);
    let rep = r.report();
    (violations(&rep), rep.deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect())
}

#[test]
fn review_k8s_khong_lang_le_mat_canh() {
    let all = format!("{K8S_RULES}link Web -> Cart\nlink Cart -> Db\nlink Internet -> Web\n");
    // Env tên bí mật tham chiếu cơ sở dữ liệu: không thành cạnh.
    let (v, _) = k8s_variant("k8s-secret-ref", "value = aws_db_instance.main.address", "value = \"x\"", K8S_RULES);
    assert!(!v.iter().any(|x| x.contains("Cart -> Db")), "{v:?}");
    let (v, _) = k8s_variant(
        "k8s-secret-ref2",
        "value = \"hunter2-k8s\"",
        "value = aws_db_instance.main.password",
        K8S_RULES,
    );
    assert!(v.iter().filter(|x| x.contains("Cart -> Db")).count() == 1, "{v:?}");
    // Thuộc tính chỉ biết lúc apply của Service vẫn là Service đó.
    let (v, u) = k8s_variant("k8s-cluster-ip", "kubernetes_service_v1.cart.metadata[0].name", "kubernetes_service_v1.cart.spec[0].cluster_ip", K8S_RULES);
    assert!(v.contains(&"NotLinked Web -> Cart infra/main.tf:61".to_string()), "{v:?} {u:?}");
    // Tham chiếu tới `kubernetes_*` không giải được thì báo.
    let (_, u) = k8s_variant("k8s-uid", "kubernetes_service_v1.cart.metadata[0].name", "kubernetes_deployment_v1.cart.metadata[0].uid", &all);
    assert!(u.iter().any(|x| x.starts_with("infra/main.tf:62 ") && x.contains("chưa giải được")), "{u:?}");
    // `merge` trên nhãn lấy qua tham chiếu: báo, không ra selector rỗng.
    let (_, u) = k8s_variant(
        "k8s-merge",
        "selector = kubernetes_deployment_v1.cart.spec[0].template[0].metadata[0].labels",
        "selector = merge(kubernetes_deployment_v1.cart.spec[0].template[0].metadata[0].labels, {})",
        &all,
    );
    assert!(u.iter().any(|x| x.contains("`merge` trên tham chiếu tài nguyên")), "{u:?}");
    // Tên chưa biết mà có cạnh tới Terraform: báo.
    let (_, u) = k8s_variant("k8s-ten", "    name = \"cart\"\n  }\n  spec {\n    selector {", "    name = var.name\n  }\n  spec {\n    selector {", &all);
    assert!(u.iter().any(|x| x.starts_with("infra/main.tf:4 ") && x.contains("metadata.name")), "{u:?}");
}

#[test]
fn review_k8s_config_map_ingress_cu_va_manifest_env() {
    let r = Repo::new("k8s-review2");
    r.write(
        "main.tf",
        r#"resource "aws_db_instance" "main" {
  engine = "postgres"
}
resource "kubernetes_config_map_v1" "cfg" {
  metadata {
    name = "cfg"
  }
  data = {
    DB_HOST = aws_db_instance.main.address
  }
}
resource "kubernetes_manifest" "api" {
  manifest = {
    apiVersion = "v1"
    kind       = "Pod"
    metadata   = { name = "api" }
    spec = {
      containers = [{
        name  = "api"
        image = "shop/api:1"
        env = [
          { name = "DB_URL", value = "postgres://${kubernetes_config_map_v1.cfg.data.DB_HOST}:5432" },
          { name = "DB_HOST2", value = aws_db_instance.main.address },
        ]
      }]
    }
  }
}
resource "kubernetes_ingress" "old" {
  metadata {
    name = "old"
  }
  spec {
    backend {
      service_name = "api"
      service_port = 80
    }
  }
}
resource "kubernetes_manifest" "file" {
  manifest = yamldecode(file("${path.module}/x.yaml"))
}
"#,
    )
    // Luật chỉ có `k8s:`: phần chưa đọc vẫn phải báo.
    .rules("service Api from k8s:api\n");
    let rep = r.report();
    let v = violations(&rep);
    assert!(v.contains(&"NotLinked Api -> aws_db_instance.main main.tf:22".to_string()), "{v:?}");
    assert!(v.contains(&"NotLinked Api -> aws_db_instance.main main.tf:23".to_string()), "{v:?}");
    let u: Vec<String> = rep.deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(u.iter().any(|x| x.starts_with("main.tf:29 ") && x.contains("kubernetes_ingress")), "{u:?}");
    assert!(u.iter().any(|x| x.starts_with("main.tf:41 ") && x.contains("yamldecode")), "{u:?}");
}

#[test]
fn chuoi_doi_khang_khong_chen_truong_vao_yaml() {
    let r = Repo::new("k8s-chen");
    // Nếu chuỗi không được thoát, nhãn này thêm `hostNetwork: true` (lộ ra
    // ngoài cụm) và đổi tên workload.
    r.write(
        "main.tf",
        r#"resource "kubernetes_pod_v1" "a" {
  metadata {
    name   = "a"
    labels = { "x\"\n    hostNetwork: true\n#" = "y\"\nkind: Secret\n" }
  }
  spec {
    container {
      name  = "a\"\n      hostPort: 80"
      image = "shop/a:1"
    }
  }
}
"#,
    )
    .rules("service A from k8s:a\n");
    let rep = r.report();
    assert!(rep.deploy_unread.is_empty(), "{:?}", rep.deploy_unread);
    assert!(!violations(&rep).iter().any(|v| v.contains("Internet")), "{:?}", violations(&rep));
    assert!(r.load().services.iter().any(|s| s.key == Key::K8s { dir: String::new(), name: "a".into() }));
}

#[test]
fn review2_config_map_override_va_element() {
    // B: ConfigMap giữ tham chiếu tới cơ sở dữ liệu, workload lấy qua
    // `env_from` và `config_map_key_ref`.
    let cm = "resource \"kubernetes_config_map_v1\" \"cfg\" {\n  metadata {\n    name = \"cfg\"\n  }\n  data = {\n    DB_HOST = aws_db_instance.main.address\n  }\n}\n";
    let r = Repo::new("k8s-cm");
    r.write(
        "infra/main.tf",
        &format!(
            "{cm}{}",
            K8S.replacen(
                "          env {\n            name  = \"DB_HOST\"\n            value = aws_db_instance.main.address\n          }\n",
                "          env_from {\n            config_map_ref {\n              name = \"cfg\"\n            }\n          }\n          env {\n            name = \"DB2\"\n            value_from {\n              config_map_key_ref {\n                name = \"cfg\"\n                key  = \"DB_HOST\"\n              }\n            }\n          }\n",
                1,
            )
        ),
    )
    .rules(K8S_RULES);
    let v = violations(&r.report());
    // Dòng của `env_from` và của `env` lấy giá trị (sau 8 dòng ConfigMap).
    assert!(v.contains(&"NotLinked Cart -> Db infra/main.tf:28".to_string()), "{v:?}");
    assert!(v.contains(&"NotLinked Cart -> Db infra/main.tf:33".to_string()), "{v:?}");

    // A: file override đổi `kubernetes_*`, luật chỉ có `k8s:`: báo.
    let r = Repo::new("k8s-override");
    r.write("main.tf", "resource \"kubernetes_namespace_v1\" \"a\" {\n  metadata {\n    name = \"a\"\n  }\n}\n")
        .write("main_override.tf", "resource \"kubernetes_deployment_v1\" \"cart\" {}\n")
        .rules("service A from k8s:a\n");
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.file.clone()).collect();
    assert!(u.contains(&"main_override.tf".to_string()), "{u:?}");

    // C: Terraform thuần AWS có module registry, luật chỉ có `k8s:`: không nhiễu.
    let r = Repo::new("k8s-khong-nhieu");
    r.write("infra/main.tf", "module \"vpc\" {\n  source = \"terraform-aws-modules/vpc/aws\"\n}\n")
        .write("k8s/web.yaml", "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  template:\n    spec:\n      containers:\n        - name: web\n          image: shop/web:1\n")
        .rules("service Web from k8s:web\n");
    let rep = r.report();
    assert!(rep.deploy_unread.is_empty(), "{:?}", rep.deploy_unread);

    // D: `element(x.*.address, 0)` với `count` vẫn là cạnh.
    let r = Repo::new("element");
    r.write(
        "main.tf",
        "resource \"aws_db_instance\" \"main\" {\n  count = 1\n}\nresource \"aws_lambda_function\" \"fn\" {\n  image_uri = \"x\"\n  environment {\n    variables = {\n      DB_HOST = element(aws_db_instance.main.*.address, 0)\n    }\n  }\n}\n",
    )
    .rules("service Fn from tf:aws_lambda_function.fn\nservice Db from tf:aws_db_instance.main\n");
    let rep = r.report();
    assert!(violations(&rep).contains(&"NotLinked Fn -> Db main.tf:8".to_string()), "{:?} {:?}", violations(&rep), rep.deploy_unread);
}

#[test]
fn review3_file_hong_provider_va_config_map_cho_yaml() {
    // E: file `.tf` hỏng có `kubernetes_*`, luật chỉ có `k8s:`: báo.
    let r = Repo::new("k8s-hong");
    let deep = format!("resource \"kubernetes_deployment_v1\" \"a\" {{}}\nlocals {{\n  x = {}1{}\n}}\n", "[".repeat(70), "]".repeat(70));
    r.write("main.tf", &deep).rules("service A from k8s:a\n");
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.starts_with("HCL:")), "{u:?}");

    // F: chỉ có `required_providers.kubernetes` và module registry: báo module.
    let r = Repo::new("k8s-required");
    r.write(
        "main.tf",
        "terraform {\n  required_providers {\n    kubernetes = { source = \"hashicorp/kubernetes\" }\n  }\n}\nmodule \"app\" {\n  source = \"example/app/aws\"\n}\n",
    )
    .rules("service A from k8s:a\n");
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("registry")), "{u:?}");

    // G, H: workload YAML lấy ConfigMap do Terraform dựng; ConfigMap cùng tên ở
    // namespace khác không thành cạnh; ARN của bucket không phải cạnh mạng.
    let r = Repo::new("k8s-yaml-cm");
    r.write(
        "infra/main.tf",
        r#"resource "aws_db_instance" "a" {}
resource "aws_db_instance" "b" {}
resource "aws_s3_bucket" "x" {}
resource "kubernetes_config_map_v1" "cfg" {
  metadata {
    name      = "cfg"
    namespace = "shop"
  }
  data = {
    DB_HOST    = aws_db_instance.a.address
    BUCKET_ARN = aws_s3_bucket.x.arn
  }
}
resource "kubernetes_config_map_v1" "other" {
  metadata {
    name      = "cfg"
    namespace = "other"
  }
  data = {
    DB_HOST = aws_db_instance.b.address
  }
}
"#,
    )
    .write(
        "k8s/api.yaml",
        "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\n  namespace: shop\nspec:\n  template:\n    spec:\n      containers:\n        - name: api\n          image: shop/api:1\n          envFrom:\n            - configMapRef:\n                name: cfg\n          env:\n            - name: DB2\n              valueFrom:\n                configMapKeyRef:\n                  name: cfg\n                  key: DB_HOST\n",
    )
    .rules("service Api from k8s:k8s/api\nservice A from tf:infra/aws_db_instance.a\nservice B from tf:infra/aws_db_instance.b\n");
    let rep = r.report();
    let v = violations(&rep);
    assert!(v.contains(&"NotLinked Api -> A k8s/api.yaml:13".to_string()), "{v:?}");
    assert!(v.contains(&"NotLinked Api -> A k8s/api.yaml:16".to_string()), "{v:?}");
    assert!(!v.iter().any(|x| x.contains("-> B") || x.contains("aws_s3_bucket")), "{v:?}");
    // Workload YAML không ghi namespace (quyết lúc `helm --namespace`): khớp
    // ConfigMap ở namespace nào cũng được.
    r.write("k8s/api.yaml", "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: api\nspec:\n  template:\n    spec:\n      containers:\n        - name: api\n          image: shop/api:1\n          envFrom:\n            - configMapRef:\n                name: cfg\n");
    let v = violations(&r.report());
    assert!(v.contains(&"NotLinked Api -> A k8s/api.yaml:12".to_string()), "{v:?}");
}

// Đợt 8: plan JSON do lệnh `verify` tạo lấp chỗ bộ đọc HCL không đọc được,
// chỉ khi biên nhận còn khớp file `.tf` (TM-57).
const REGISTRY_TF: &str = "resource \"aws_db_instance\" \"main\" {}\nmodule \"ecs\" {\n  source  = \"terraform-aws-modules/ecs/aws\"\n  db_host = aws_db_instance.main.address\n}\n";

fn registry_plan(with_edge: bool) -> String {
    let td_refs = if with_edge { r#"["var.db_host"]"# } else { "[]" };
    format!(
        r#"{{"format_version":"1.2","configuration":{{"root_module":{{
  "resources":[{{"address":"aws_db_instance.main","mode":"managed","type":"aws_db_instance","name":"main","expressions":{{}}}}],
  "module_calls":{{"ecs":{{"source":"terraform-aws-modules/ecs/aws",
    "expressions":{{"db_host":{{"references":["aws_db_instance.main.address","aws_db_instance.main"]}}}},
    "module":{{"resources":[
      {{"address":"aws_ecs_task_definition.api","mode":"managed","type":"aws_ecs_task_definition","name":"api",
        "expressions":{{"container_definitions":{{"references":{td_refs}}}}}}},
      {{"address":"aws_ecs_service.svc","mode":"managed","type":"aws_ecs_service","name":"svc",
        "expressions":{{"task_definition":{{"references":["aws_ecs_task_definition.api.arn","aws_ecs_task_definition.api"]}}}}}}
    ]}}}}}}}}}},
 "planned_values":{{"root_module":{{
  "resources":[{{"address":"aws_db_instance.main","values":{{"password":"hunter2-plan"}}}}],
  "child_modules":[{{"address":"module.ecs","resources":[
    {{"address":"module.ecs.aws_ecs_service.svc","values":{{}}}},
    {{"address":"module.ecs.aws_ecs_task_definition.api","values":{{"container_definitions":"[{{\"image\":\"acme/api:1\"}}]"}}}}]}}]}}}}}}"#
    )
}

impl Repo {
    /// Biên nhận `verify tf_plan` đạt, mang băm `.tf`, chứng nhận mọi file plan
    /// hiện có và băm mã module, như `archctl verify` ghi sau khi lệnh tạo plan
    /// chạy xong (lệnh đã ghi mọi file đó).
    fn receipt(&self, at: u64) -> &Self {
        let files = arch_core::check::deploy_files(&self.0, &[]);
        let r = arch_core::verify::Receipt {
            name: "tf_plan".into(),
            passed: true,
            at,
            arch_hash: "x".into(),
            tf: deploy::tf_hash(&self.0, &files),
            plans: deploy::plan_files(&self.0).into_iter().map(|p| { let h = deploy::file_hash(&self.0, &p); (p, h) }).collect(),
            mods: Some(deploy::modules_hash(&self.0, &files)),
        };
        self.write(arch_core::verify::RECEIPTS_FILE, &arch_core::verify::render(&[r]))
    }

    /// Ghi plan rồi biên nhận của lệnh tạo nó.
    fn plan(&self, json: &str) -> &Self {
        self.write(".arch/tf-plan.json", json).receipt(1)
    }
}

const PLAN_VERIFY: &str = "verify tf_plan runs \"tofu show -json p > .arch/tf-plan.json\"\n";
const REGISTRY_RULES: &str = "service Api from tf:module.ecs.aws_ecs_service.svc\nservice Db from tf:aws_db_instance.main\nverify tf_plan runs \"tofu show -json p > .arch/tf-plan.json\"\n";

#[test]
fn plan_lap_module_registry_khi_khop() {
    let r = Repo::new("plan-registry");
    r.write("main.tf", REGISTRY_TF).rules(REGISTRY_RULES);
    // Chưa có plan: module registry chưa đọc được.
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("registry")), "{u:?}");
    r.plan(&registry_plan(true));
    let rep = r.report();
    assert!(rep.deploy_unread.is_empty(), "{:?}", rep.deploy_unread);
    let v = violations(&rep);
    // Service trong module registry, cạnh tới cơ sở dữ liệu qua biến của module;
    // dòng là dòng gọi module.
    assert!(v.iter().any(|x| x.starts_with("NotLinked Api -> Db main.tf:2")), "{v:?}");
    let all = format!("{rep:?}{:?}", r.load().services);
    assert!(!all.contains("hunter2-plan"), "lọt mật khẩu trong plan");
    r.rules(&format!("{REGISTRY_RULES}link Api -> Db\n"));
    assert!(violations(&r.report()).is_empty());
}

#[test]
fn plan_cu_hay_ghi_truoc_bien_nhan_thi_khong_dung() {
    let r = Repo::new("plan-cu");
    r.write("main.tf", REGISTRY_TF).rules(REGISTRY_RULES);
    r.plan(&registry_plan(true));
    assert!(r.report().deploy_unread.is_empty());
    // Agent sửa `.tf` sau lần chạy verify: plan cũ, không dùng, báo.
    r.write("main.tf", &format!("{REGISTRY_TF}resource \"aws_sqs_queue\" \"q\" {{}}\n"));
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(u.iter().any(|x| x.starts_with(".arch/tf-plan.json ") && x.contains("plan Terraform cũ")), "{u:?}");
    assert!(u.iter().any(|x| x.contains("registry")), "{u:?}");
    // Một lệnh khác (bộ test agent viết được) ghi đè plan sau lệnh tạo plan:
    // băm plan không còn trùng biên nhận, không dùng.
    r.write("main.tf", REGISTRY_TF).plan(&registry_plan(true));
    assert!(r.report().deploy_unread.is_empty());
    r.write(".arch/tf-plan.json", &registry_plan(false));
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("plan Terraform cũ")), "{u:?}");
    // Mã module tải về đổi sau lần chạy: cũng là cũ.
    r.plan(&registry_plan(true)).write(".terraform/modules/ecs/main.tf", "locals {}\n");
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("plan Terraform cũ")), "{u:?}");
    // Biên nhận của lệnh không khai là ghi plan: không dùng.
    r.plan(&registry_plan(true)).rules("service Api from tf:module.ecs.aws_ecs_service.svc\nservice Db from tf:aws_db_instance.main\nverify tf_plan runs \"sh test.sh\"\n");
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("registry")), "{u:?}");
    // Biên nhận không đạt: không dùng.
    r.rules(REGISTRY_RULES);
    let files = arch_core::check::deploy_files(&r.0, &[]);
    let failed = arch_core::verify::Receipt {
        name: "tf_plan".into(),
        passed: false,
        at: 1,
        arch_hash: "x".into(),
        tf: deploy::tf_hash(&r.0, &files),
        plans: deploy::plan_files(&r.0).into_iter().map(|p| { let h = deploy::file_hash(&r.0, &p); (p, h) }).collect(),
        mods: Some(deploy::modules_hash(&r.0, &files)),
    };
    r.write(arch_core::verify::RECEIPTS_FILE, &arch_core::verify::render(&[failed]));
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("plan Terraform cũ")), "{u:?}");
}

#[test]
fn plan_khong_bo_canh_hcl_thay_va_bao_lech() {
    let r = Repo::new("plan-lech");
    // Bộ đọc HCL thấy cạnh Api -> Db; plan (giả) không có cạnh nào.
    r.write("main.tf", ECS).rules(&format!("service Api from tf:aws_ecs_service.api\nservice Db from tf:aws_db_instance.main\nlink Internet -> Api\n{PLAN_VERIFY}"));
    let plan = r#"{"configuration":{"root_module":{"resources":[
      {"address":"aws_ecs_task_definition.api","mode":"managed","type":"aws_ecs_task_definition","name":"api","expressions":{"container_definitions":{}}},
      {"address":"aws_ecs_service.api","mode":"managed","type":"aws_ecs_service","name":"api","expressions":{"task_definition":{"references":["aws_ecs_task_definition.api"]}}}]}},
     "planned_values":{"root_module":{"resources":[{"address":"aws_ecs_service.api","values":{}},{"address":"aws_ecs_task_definition.api","values":{}}]}}}"#;
    r.plan(plan);
    let rep = r.report();
    assert!(violations(&rep).contains(&"NotLinked Api -> Db main.tf:13".to_string()), "{:?}", violations(&rep));
    let u: Vec<String> = rep.deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("bộ đọc thấy cạnh") && x.contains("aws_db_instance.main")), "{u:?}");
    assert!(u.iter().any(|x| x.contains("lộ ra Internet")), "{u:?}");
}

#[test]
fn plan_doi_khang_khong_lam_treo_va_khong_ra_ngoai_repo() {
    let r = Repo::new("plan-doi-khang");
    r.write("main.tf", REGISTRY_TF).rules(REGISTRY_RULES);
    r.plan(&"[".repeat(300_000));
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("không phải JSON đọc được")), "{u:?}");
    // `modules.json` trỏ ra ngoài repo: không đọc mã ở đó, `local` báo không lần được.
    let plan = registry_plan(true).replace(r#""references":["var.db_host"]"#, r#""references":["local.h"]"#);
    r.write(".terraform/modules/modules.json", r#"{"Modules":[{"Key":"ecs","Dir":"../../../../etc"}]}"#).plan(&plan);
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("`local.h` không lần được")), "{u:?}");
    // Mã module tải về nằm trong repo: `locals` đọc được, cạnh lần qua nó.
    r.write(".terraform/modules/modules.json", r#"{"Modules":[{"Key":"ecs","Dir":".terraform/modules/ecs"}]}"#)
        .write(".terraform/modules/ecs/main.tf", "locals {\n  h = \"${var.db_host}:5432\"\n}\n")
        .plan(&plan);
    let rep = r.report();
    assert!(rep.deploy_unread.is_empty(), "{:?}", rep.deploy_unread);
    assert!(violations(&rep).iter().any(|x| x.starts_with("NotLinked Api -> Db main.tf:2")), "{:?}", violations(&rep));
}

#[test]
fn review_plan_chi_so_for_iam_va_module_dung_chung() {
    // Tham chiếu có chỉ số (`count` trên module): plan ghi `module.db[0].endpoint`.
    let r = Repo::new("plan-chi-so");
    r.write(
        "main.tf",
        "module \"db\" {\n  source = \"./db\"\n  count  = var.n\n}\nvariable \"n\" {}\nresource \"aws_lambda_function\" \"fn\" {\n  image_uri = \"x\"\n  environment {\n    variables = { DB = module.db[0].endpoint }\n  }\n}\n",
    )
    .write("db/main.tf", "resource \"aws_db_instance\" \"this\" {}\noutput \"endpoint\" {\n  value = aws_db_instance.this.address\n}\n")
    .rules(&format!("service Fn from tf:aws_lambda_function.fn\nservice Db from tf:module.db.aws_db_instance.this\n{PLAN_VERIFY}"));
    r.plan(
        r#"{"configuration":{"root_module":{
      "resources":[{"address":"aws_lambda_function.fn","mode":"managed","type":"aws_lambda_function","name":"fn",
        "expressions":{"environment":[{"variables":{"references":["module.db[0].endpoint","module.db[0]","module.db"]}}]}}],
      "module_calls":{"db":{"source":"./db","module":{
        "resources":[{"address":"aws_db_instance.this","mode":"managed","type":"aws_db_instance","name":"this","expressions":{}}],
        "outputs":{"endpoint":{"expression":{"references":["aws_db_instance.this.address","aws_db_instance.this"]}}}}}}}},
     "planned_values":{"root_module":{"resources":[{"address":"aws_lambda_function.fn","values":{}}],
       "child_modules":[{"resources":[{"address":"module.db[0].aws_db_instance.this","values":{}}]}]}}}"#,
    );
    let rep = r.report();
    assert!(violations(&rep).iter().any(|x| x.starts_with("NotLinked Fn -> Db main.tf:")), "{:?} {:?}", violations(&rep), rep.deploy_unread);

    // `for` trong `locals`: plan không giữ tham chiếu bên trong, mục vẫn báo.
    let r = Repo::new("plan-for");
    r.write(
        "main.tf",
        "resource \"aws_db_instance\" \"main\" {}\nlocals {\n  hosts = { for k, v in { main = aws_db_instance.main } : k => v.address }\n}\nresource \"aws_lambda_function\" \"fn\" {\n  image_uri = \"x\"\n  environment {\n    variables = { DB = local.hosts[\"main\"] }\n  }\n}\n",
    )
    .rules(&format!("service Fn from tf:aws_lambda_function.fn\n{PLAN_VERIFY}"));
    r.plan(
        r#"{"configuration":{"root_module":{"resources":[{"address":"aws_lambda_function.fn","mode":"managed","type":"aws_lambda_function","name":"fn",
        "expressions":{"environment":[{"variables":{"references":["local.hosts"]}}]}}]}},
      "planned_values":{"root_module":{"resources":[{"address":"aws_lambda_function.fn","values":{}}]}}}"#,
    );
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("`for`") && !x.starts_with("plan:")), "{u:?}");
    assert!(u.iter().any(|x| x.starts_with("plan:") && x.contains("local.hosts")), "{u:?}");

    // IAM policy của Cloud Run: plan không đọc `policy_data`, mục vẫn báo.
    let r = Repo::new("plan-iam");
    r.write(
        "main.tf",
        "resource \"google_cloud_run_v2_service\" \"s\" {\n  template {\n    containers {\n      image = \"x\"\n    }\n  }\n}\nresource \"google_cloud_run_v2_service_iam_policy\" \"p\" {\n  name        = google_cloud_run_v2_service.s.name\n  policy_data = data.google_iam_policy.all.policy_data\n}\ndata \"google_iam_policy\" \"all\" {}\n",
    )
    .rules(&format!("service S from tf:google_cloud_run_v2_service.s\n{PLAN_VERIFY}"));
    r.plan(
        r#"{"configuration":{"root_module":{"resources":[
        {"address":"google_cloud_run_v2_service.s","mode":"managed","type":"google_cloud_run_v2_service","name":"s","expressions":{}},
        {"address":"google_cloud_run_v2_service_iam_policy.p","mode":"managed","type":"google_cloud_run_v2_service_iam_policy","name":"p",
         "expressions":{"name":{"references":["google_cloud_run_v2_service.s.name","google_cloud_run_v2_service.s"]}}}]}},
      "planned_values":{"root_module":{"resources":[{"address":"google_cloud_run_v2_service.s","values":{}},{"address":"google_cloud_run_v2_service_iam_policy.p","values":{}}]}}}"#,
    );
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("IAM policy")), "{u:?}");

    // Module dùng chung giữa hai gốc, chỉ một gốc có plan: mục của module vẫn báo.
    let r = Repo::new("plan-dung-chung");
    let call = "module \"svc\" {\n  source = \"../../modules/svc\"\n}\n";
    r.write("envs/prod/main.tf", call)
        .write("envs/staging/main.tf", call)
        .write("modules/svc/main.tf", "resource \"aws_lambda_function\" \"f\" {\n  image_uri = \"x\"\n  environment {\n    variables = { A = trimsuffix(\"a\", \"b\") }\n  }\n}\n")
        .rules("service F from tf:envs/prod/module.svc.aws_lambda_function.f\nverify tf_plan runs \"x > .arch/tf-plan/envs/prod.json\"\n");
    r.write(".arch/tf-plan/envs/prod.json", r#"{"configuration":{"root_module":{}}}"#).receipt(1);
    let u: Vec<String> = r.report().deploy_unread.iter().map(|u| format!("{} {}", u.file, u.why)).collect();
    assert!(u.iter().any(|x| x.starts_with("modules/svc/main.tf:4 ") && x.contains("trimsuffix")), "{u:?}");
}

#[test]
fn plan_nhieu_lambda_va_url_khong_bac_hai() {
    let r = Repo::new("plan-lon");
    r.write("main.tf", "resource \"aws_lambda_function\" \"f0\" {\n  image_uri = \"x\"\n}\n")
        .rules(&format!("service F from tf:aws_lambda_function.f0\n{PLAN_VERIFY}"));
    let mut res = Vec::new();
    let mut vals = Vec::new();
    for i in 0..5000 {
        res.push(format!(r#"{{"address":"aws_lambda_function.f{i}","mode":"managed","type":"aws_lambda_function","name":"f{i}","expressions":{{}}}}"#));
        res.push(format!(r#"{{"address":"aws_lambda_function_url.u{i}","mode":"managed","type":"aws_lambda_function_url","name":"u{i}","expressions":{{"function_name":{{"references":["aws_lambda_function.f{i}.function_name","aws_lambda_function.f{i}"]}}}}}}"#));
        vals.push(format!(r#"{{"address":"aws_lambda_function.f{i}","values":{{}}}}"#));
        vals.push(format!(r#"{{"address":"aws_lambda_function_url.u{i}","values":{{"authorization_type":"NONE"}}}}"#));
    }
    let plan = format!(
        r#"{{"configuration":{{"root_module":{{"resources":[{}]}}}},"planned_values":{{"root_module":{{"resources":[{}]}}}}}}"#,
        res.join(","),
        vals.join(",")
    );
    r.plan(&plan);
    let t = std::time::Instant::now();
    let rep = r.report();
    assert!(t.elapsed().as_secs_f64() < 10.0, "{:?}", t.elapsed());
    assert!(violations(&rep).iter().any(|x| x.starts_with("NotLinked Internet -> F")), "{:?}", violations(&rep));
}

#[test]
fn hang_doi_toi_ben_nhan_qua_terraform() {
    let r = Repo::new("hang-doi");
    r.write(
        "main.tf",
        r#"resource "aws_sqs_queue" "jobs" {}
resource "aws_sns_topic" "events" {}
resource "aws_lambda_function" "worker" {
  image_uri = "acme/worker:1"
}
resource "aws_lambda_function" "api" {
  image_uri = "acme/api:1"
  environment {
    variables = { QUEUE_URL = aws_sqs_queue.jobs.url }
  }
}
resource "aws_lambda_event_source_mapping" "jobs" {
  event_source_arn = aws_sqs_queue.jobs.arn
  function_name    = aws_lambda_function.worker.arn
}
resource "aws_sns_topic_subscription" "to_jobs" {
  topic_arn = aws_sns_topic.events.arn
  protocol  = "sqs"
  endpoint  = aws_sqs_queue.jobs.arn
}
resource "google_pubsub_topic" "orders" {}
resource "google_cloud_run_v2_service" "billing" {
  template {
    containers {
      image = "acme/billing:1"
    }
  }
}
resource "google_pubsub_subscription" "billing" {
  topic = google_pubsub_topic.orders.id
  push_config {
    push_endpoint = google_cloud_run_v2_service.billing.uri
  }
}
variable "fn" {}
resource "aws_lambda_event_source_mapping" "x" {
  event_source_arn = aws_sqs_queue.jobs.arn
  function_name    = lookup({}, "a")
}
"#,
    )
    .rules(
        "service Api from tf:aws_lambda_function.api\nservice Worker from tf:aws_lambda_function.worker\nservice Jobs from tf:aws_sqs_queue.jobs\nservice Events from tf:aws_sns_topic.events\nservice Orders from tf:google_pubsub_topic.orders\nservice Billing from tf:google_cloud_run_v2_service.billing\n",
    );
    let rep = r.report();
    let v = violations(&rep);
    for want in [
        "NotLinked Api -> Jobs main.tf:9",
        "NotLinked Jobs -> Worker main.tf:12",
        "NotLinked Events -> Jobs main.tf:16",
        "NotLinked Orders -> Billing main.tf:29",
    ] {
        assert!(v.contains(&want.to_string()), "{want}: {v:?}");
    }
    // Đích của event source mapping không đọc được: báo, không bỏ lặng lẽ.
    let u: Vec<String> = rep.deploy_unread.iter().map(|u| u.why.clone()).collect();
    assert!(u.iter().any(|x| x.contains("function_name") && x.contains("aws_lambda_event_source_mapping.x")), "{u:?}");
}
