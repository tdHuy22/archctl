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
        deploy::load(&self.0, &files, &deploy::Options { tf: true, ..Default::default() })
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
