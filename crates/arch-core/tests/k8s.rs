//! Manifest Kubernetes: workload thành `service`, env trỏ tên Service thành
//! cạnh cần `link`, image lần về mã qua skaffold, compose hay `dockerfile:`.

use arch_core::check::{Reason, Report, Via, check, check_files};
use arch_core::scan::deploy::{self, Entry, Key};
use std::path::PathBuf;

struct Repo(PathBuf);

impl Repo {
    fn new(name: &str) -> Repo {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let root =
            std::env::temp_dir().join(format!("archctl-k8s-{name}-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        Repo(root)
    }

    fn write(&self, rel: &str, body: &str) -> &Self {
        let path = self.0.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
        self
    }

    fn remove(&self, rel: &str) -> &Self {
        std::fs::remove_file(self.0.join(rel)).unwrap();
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
        deploy::load(
            &self.0,
            &files,
            &deploy::Options {
                k8s: true,
                ..Default::default()
            },
        )
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn violations(r: &Report) -> Vec<String> {
    r.violations
        .iter()
        .map(|v| {
            format!(
                "{:?} {} -> {} {}:{}",
                v.reason, v.from, v.to, v.file, v.line
            )
        })
        .collect()
}

const PY_DOCKERFILE: &str =
    "FROM python:3.12\nWORKDIR /app\nCOPY . .\nCMD [\"python\", \"main.py\"]\n";

const API: &str = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
spec:
  selector:
    matchLabels: {app: api}
  template:
    metadata:
      labels: {app: api}
    spec:
      containers:
      - name: server
        image: api
        env:
        - name: DB_HOST
          value: db
        - name: WORKER_URL
          value: "http://worker:8080/jobs"
        - name: DATABASE_PASSWORD
          valueFrom:
            secretKeyRef: {name: db, key: password}
        - name: CACHE_ADDR
          valueFrom:
            configMapKeyRef: {name: cfg, key: cache}
---
apiVersion: v1
kind: Service
metadata:
  name: api
spec:
  selector: {app: api}
"#;

/// Hai service Python dựng qua skaffold, một cơ sở dữ liệu dựng sẵn.
fn shop(name: &str) -> Repo {
    let r = Repo::new(name);
    r.write("api/Dockerfile", PY_DOCKERFILE)
        .write("api/main.py", "import os\n")
        .write("worker/Dockerfile", PY_DOCKERFILE)
        .write("worker/main.py", "import os\n")
        .write(
            "skaffold.yaml",
            "apiVersion: skaffold/v4beta6\nkind: Config\nbuild:\n  artifacts:\n  - image: api\n    context: api\n  - image: worker\n    context: worker\nmanifests:\n  rawYaml: [k8s/*.yaml]\n",
        )
        .write("k8s/api.yaml", API)
        .write(
            "k8s/worker.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: worker}\nspec:\n  template:\n    metadata:\n      labels: {app: worker, tier: back}\n    spec:\n      containers:\n      - {name: w, image: gcr.io/acme/worker:v3}\n---\napiVersion: v1\nkind: Service\nmetadata: {name: worker}\nspec:\n  selector: {app: worker}\n",
        )
        .write(
            "k8s/db.yaml",
            "apiVersion: apps/v1\nkind: StatefulSet\nmetadata: {name: db}\nspec:\n  template:\n    metadata:\n      labels: {app: db}\n    spec:\n      containers:\n      - {name: pg, image: postgres:16}\n---\napiVersion: v1\nkind: Service\nmetadata: {name: db}\nspec:\n  selector: {app: db}\n",
        )
        .write(
            "k8s/cfg.yaml",
            "apiVersion: v1\nkind: ConfigMap\nmetadata: {name: cfg}\ndata:\n  cache: \"redis-cache:6379\"\n---\napiVersion: v1\nkind: Secret\nmetadata: {name: db}\nstringData:\n  password: s3cr3t-hunter2\n",
        )
        .rules(
            "component Api at api/**\ncomponent Worker at worker/**\n\
             service Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\n\
             link Api -> Db, Worker\n",
        );
    r
}

#[test]
fn khop_thi_khong_bao_gi() {
    let r = shop("khop");
    let rep = r.report();
    assert!(rep.violations.is_empty(), "{:?}", violations(&rep));
    assert!(rep.entry_unknown.is_empty(), "{:?}", rep.entry_unknown);
    assert!(
        rep.missing.is_empty() && rep.links_unseen.is_empty(),
        "{:?} {:?}",
        rep.missing,
        rep.links_unseen
    );
    assert!(rep.deploy_unread.is_empty(), "{:?}", rep.deploy_unread);
    assert!(
        rep.services_undeclared.is_empty(),
        "{:?}",
        rep.services_undeclared
    );
}

#[test]
fn canh_mang_thieu_link() {
    let r = shop("thieu-link");
    r.rules(
        "component Api at api/**\ncomponent Worker at worker/**\n\
         service Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\n\
         link Api -> Db\n",
    );
    let rep = r.report();
    assert_eq!(
        violations(&rep),
        ["NotLinked Api -> Worker k8s/api.yaml:18"]
    );
    assert_eq!(rep.violations[0].via, Via::Network);
    assert_eq!(rep.violations[0].spec, "WORKER_URL → worker:8080");
    // Hook với chính file manifest vừa đổi.
    let rep = r.partial(&["k8s/api.yaml"]);
    assert_eq!(
        violations(&rep),
        ["NotLinked Api -> Worker k8s/api.yaml:18"]
    );
}

#[test]
fn diem_vao_nam_ngoai_runs() {
    let r = shop("runs");
    r.rules(
        "component Api at api/**\ncomponent Worker at worker/**\n\
         service Api from k8s:api runs Worker\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\n\
         link Api -> Db, Worker\n",
    );
    let rep = r.report();
    assert_eq!(violations(&rep), ["NotRun Api -> Api api/Dockerfile:4"]);
}

#[test]
fn secret_va_gia_tri_khong_lot_ra() {
    let r = shop("secret");
    r.write(
        "k8s/cfg.yaml",
        "apiVersion: v1\nkind: ConfigMap\nmetadata: {name: cfg}\ndata:\n  cache: \"redis://admin:p4ss-w0rd@worker:6379/0\"\n---\napiVersion: v1\nkind: Secret\nmetadata: {name: db}\nstringData:\n  password: s3cr3t-hunter2\n",
    )
    .rules("component Api at api/**\ncomponent Worker at worker/**\nservice Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\n");
    let rep = r.report();
    let all = format!("{rep:?}{:?}", r.load().services);
    for secret in [
        "s3cr3t-hunter2",
        "p4ss-w0rd",
        "admin:",
        "http://worker:8080/jobs",
    ] {
        assert!(!all.contains(secret), "lọt `{secret}`");
    }
    // Cạnh lấy từ ConfigMap trỏ về chính dòng trong ConfigMap.
    assert!(
        violations(&rep).contains(&"NotLinked Api -> Worker k8s/cfg.yaml:5".to_string()),
        "{:?}",
        violations(&rep)
    );
}

#[test]
fn canh_toi_workload_chua_khai_la_vi_pham() {
    let r = shop("chua-khai");
    r.rules("component Api at api/**\ncomponent Worker at worker/**\nservice Api from k8s:api runs Api\nservice Db from k8s:db\nlink Api -> Db\n");
    let rep = r.report();
    assert_eq!(
        violations(&rep),
        ["NotLinked Api -> worker k8s/api.yaml:18"]
    );
    assert!(arch_core::check::to_undeclared(&rep.violations[0]));
    assert!(
        rep.violations[0].hint.contains("from k8s:worker"),
        "{}",
        rep.violations[0].hint
    );
    assert_eq!(
        rep.services_undeclared
            .iter()
            .map(|u| u.2.as_str())
            .collect::<Vec<_>>(),
        ["worker"]
    );
}

#[test]
fn khai_bao_khong_con_khop() {
    let r = shop("lac-hau");
    r.remove("k8s/db.yaml");
    let rep = r.report();
    assert_eq!(
        rep.missing
            .iter()
            .map(|m| m.name.as_str())
            .collect::<Vec<_>>(),
        ["k8s:db"]
    );
}

#[test]
fn image_khong_lan_duoc_thi_bao_va_dockerfile_trong_luat_thi_dung() {
    let r = shop("image");
    r.remove("skaffold.yaml");
    let rep = r.report();
    let unknown: Vec<&str> = rep.entry_unknown.iter().map(|(s, _)| s.as_str()).collect();
    assert_eq!(unknown, ["Api", "Worker"]);
    assert!(
        rep.entry_unknown[0].1.contains("không lần được"),
        "{}",
        rep.entry_unknown[0].1
    );
    // `dockerfile:` cùng `k8s:`: image của container chính.
    r.rules(
        "component Api at api/**\ncomponent Worker at worker/**\n\
         service Api from k8s:api, dockerfile:api/Dockerfile runs Worker\n\
         service Worker from k8s:worker, dockerfile:worker/Dockerfile runs Worker\nservice Db from k8s:db\n\
         link Api -> Db, Worker\n",
    );
    let rep = r.report();
    assert!(rep.entry_unknown.is_empty(), "{:?}", rep.entry_unknown);
    assert_eq!(violations(&rep), ["NotRun Api -> Api api/Dockerfile:4"]);
}

#[test]
fn image_khop_nhieu_noi_dung_thi_khong_chon() {
    let r = shop("nhap-nhang");
    r.write(
        "skaffold.yaml",
        "apiVersion: skaffold/v4beta6\nkind: Config\nbuild:\n  artifacts:\n  - image: api\n    context: api\n  - image: api\n    context: worker\n  - image: worker\n    context: worker\n",
    );
    let rep = r.report();
    assert_eq!(rep.entry_unknown.len(), 1, "{:?}", rep.entry_unknown);
    assert!(
        rep.entry_unknown[0].1.contains("khớp nhiều nơi dựng"),
        "{}",
        rep.entry_unknown[0].1
    );
}

#[test]
fn command_args_theo_kubernetes() {
    let r = shop("args");
    r.write(
        "api/Dockerfile",
        "FROM python:3.12\nWORKDIR /app\nCOPY . .\nENTRYPOINT [\"python\"]\nCMD [\"main.py\"]\n",
    )
    .write("api/other.py", "import os\n")
    .write("api/third.py", "import os\n")
    .write(
        "k8s/api.yaml",
        &API.replace(
            "        image: api\n",
            "        image: api\n        args: [other.py]\n",
        ),
    );
    let entries = |r: &Repo| -> Vec<Entry> {
        let d = r.load();
        let api = d
            .services
            .iter()
            .find(|s| {
                s.key
                    == Key::K8s {
                        dir: "k8s".into(),
                        name: "api".into(),
                    }
            })
            .unwrap();
        api.starts.iter().flat_map(|s| s.entries.clone()).collect()
    };
    // `args` thay `CMD`, giữ `ENTRYPOINT`.
    assert_eq!(entries(&r), [Entry::File("api/other.py".into())]);
    // `command` không có `args`: `CMD` của image bị bỏ.
    r.write(
        "k8s/api.yaml",
        &API.replace(
            "        image: api\n",
            "        image: api\n        command: [python, third.py]\n",
        ),
    );
    assert_eq!(entries(&r), [Entry::File("api/third.py".into())]);
}

#[test]
fn ten_mien_ngoai_khong_ghep_va_namespace_thi_ghep() {
    let r = shop("dns");
    let api = API
        .replace("value: db\n", "value: \"https://worker.example.com/x\"\n")
        .replace(
            "\"http://worker:8080/jobs\"",
            "\"db.prod.svc.cluster.local:5432\"",
        );
    r.write("k8s/api.yaml", &api).write(
        "k8s/db.yaml",
        "apiVersion: apps/v1\nkind: StatefulSet\nmetadata: {name: db, namespace: prod}\nspec:\n  template:\n    metadata:\n      labels: {app: db}\n    spec:\n      containers:\n      - {name: pg, image: postgres:16}\n---\napiVersion: v1\nkind: Service\nmetadata: {name: db, namespace: prod}\nspec:\n  selector: {app: db}\n",
    );
    r.rules("component Api at api/**\ncomponent Worker at worker/**\nservice Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\n");
    let rep = r.report();
    assert_eq!(violations(&rep), ["NotLinked Api -> Db k8s/api.yaml:18"]);
}

#[test]
fn service_khong_chon_workload_nao_la_ben_ngoai() {
    let r = shop("ngoai");
    r.write(
        "k8s/ext.yaml",
        "apiVersion: v1\nkind: Service\nmetadata: {name: payments}\nspec:\n  type: ExternalName\n  externalName: pay.example.com\n",
    )
    .write("k8s/api.yaml", &API.replace("value: db\n", "value: payments\n"));
    let rep = r.report();
    assert!(rep.violations.is_empty(), "{:?}", violations(&rep));
}

#[test]
fn helm_khong_doc_va_loai_la_duoc_bao() {
    let r = shop("helm");
    r.write("chart/Chart.yaml", "apiVersion: v2\nname: shop\nversion: 0.1.0\n")
        .write(
            "chart/templates/api.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {{ .Release.Name }}-api\nspec:\n  template:\n    spec:\n      containers:\n      - name: a\n        image: {{ .Values.image }}\n",
        )
        .write(
            "k8s/rollout.yaml",
            "apiVersion: argoproj.io/v1alpha1\nkind: Rollout\nmetadata: {name: canary}\nspec:\n  template:\n    spec:\n      containers:\n      - {name: c, image: api}\n",
        );
    let d = r.load();
    assert_eq!(d.charts, ["chart"]);
    let unread: Vec<String> = d
        .unread
        .iter()
        .map(|u| format!("{} {}", u.file, u.why))
        .collect();
    assert_eq!(unread.len(), 1, "{unread:?}");
    assert!(
        unread[0].starts_with("k8s/rollout.yaml:1 `Rollout`"),
        "{unread:?}"
    );
    assert_eq!(r.report().deploy_unread.len(), 1);
}

#[test]
fn patch_kustomize_doi_env_thi_bao() {
    let r = shop("kustomize");
    r.write("k8s/kustomization.yaml", "resources: [api.yaml, worker.yaml, db.yaml, cfg.yaml]\npatches:\n- path: more-env.yaml\n- path: replicas.yaml\n")
        .write(
            "k8s/more-env.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: api}\nspec:\n  template:\n    spec:\n      containers:\n      - name: server\n        env: [{name: EXTRA_URL, value: \"http://evil:1\"}]\n",
        )
        .write("k8s/replicas.yaml", "apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: worker}\nspec:\n  replicas: 3\n");
    let rep = r.report();
    let unread: Vec<String> = rep
        .deploy_unread
        .iter()
        .map(|u| format!("{} {}", u.file, u.why))
        .collect();
    assert_eq!(unread.len(), 1, "{unread:?}");
    assert!(
        unread[0].starts_with("k8s/more-env.yaml patch Kustomize"),
        "{unread:?}"
    );
    // File patch không phải workload thứ hai tên `api`.
    assert!(rep.missing.is_empty(), "{:?}", rep.missing);
}

#[test]
fn duong_dan_skaffold_ra_ngoai_bi_chan() {
    let r = shop("ra-ngoai");
    r.write(
        "skaffold.yaml",
        "apiVersion: skaffold/v4beta6\nkind: Config\nbuild:\n  artifacts:\n  - image: api\n    context: ../../etc\n  - image: worker\n    context: worker\n    docker: {dockerfile: ../../../etc/passwd}\n",
    );
    let d = r.load();
    let unread: Vec<String> = d
        .unread
        .iter()
        .map(|u| format!("{} {}", u.file, u.why))
        .collect();
    assert_eq!(unread.len(), 2, "{unread:?}");
    assert!(
        unread.iter().all(|u| u.contains("nằm ngoài dự án")),
        "{unread:?}"
    );
}

#[test]
fn khong_khai_k8s_thi_khong_doc_manifest() {
    let r = shop("khong-doc");
    r.write("compose.yaml", "services:\n  db:\n    image: postgres\n")
        .write("k8s/rollout.yaml", "apiVersion: argoproj.io/v1alpha1\nkind: Rollout\nmetadata: {name: c}\nspec:\n  template:\n    spec:\n      containers: [{name: c, image: x}]\n")
        .rules("service Db from compose:db\n");
    let rep = r.report();
    assert!(rep.deploy_unread.is_empty(), "{:?}", rep.deploy_unread);
}

#[test]
fn init_gop_compose_voi_workload_cung_noi_dung() {
    let r = shop("init");
    r.write(
        "compose.yaml",
        "services:\n  api:\n    image: api\n    build: ./api\n    environment:\n      DB_HOST: db\n  db:\n    image: postgres:16\n",
    )
    .write("release/api.yaml", &API.replace("image: api", "image: gcr.io/acme/api:v1"));
    let found = arch_core::discover::discover(&r.0, &[]);
    let sources: Vec<(&str, &str)> = found
        .services
        .iter()
        .map(|s| (s.name.as_str(), s.source.as_str()))
        .collect();
    assert_eq!(
        sources,
        [
            ("Api", "compose:api, k8s:api"),
            ("Db", "compose:db, k8s:db"),
            ("Worker", "k8s:worker")
        ],
        "{:#?}",
        found.services
    );
    // `release/api.yaml` dựng từ cùng chỗ: là bản sao, không phải service mới.
    assert!(found.notes.is_empty(), "{:?}", found.notes);
    // Bản nháp nâng lên `#accepted` thì không tự vi phạm.
    let text = arch_core::discover::render(&found, "2026-10-13").replace("#draft", "#accepted");
    r.rules(&text);
    let rep = r.report();
    assert!(rep.violations.is_empty(), "{:?}\n{text}", violations(&rep));
    assert!(
        rep.entry_unknown.is_empty() && rep.missing.is_empty(),
        "{:?} {:?}",
        rep.entry_unknown,
        rep.missing
    );
}

#[test]
fn manifest_doi_khang_khong_lam_treo() {
    let r = Repo::new("doi-khang");
    // Hàng nghìn Service cùng chọn một nhãn mà hàng nghìn workload cùng mang,
    // mỗi workload trỏ tới mọi Service.
    for f in 0..10 {
        let mut text = String::new();
        for i in 0..900 {
            let n = f * 900 + i;
            let peers: Vec<String> = (n..n + 8).map(|p| format!("s{p}")).collect();
            text.push_str(&format!(
                "---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: w{n}}}\nspec:\n  template:\n    metadata:\n      labels: {{a: b}}\n    spec:\n      containers:\n      - name: c\n        image: x\n        env: [{{name: PEERS, value: \"{}\"}}]\n---\napiVersion: v1\nkind: Service\nmetadata: {{name: s{n}}}\nspec:\n  selector: {{a: b}}\n",
                peers.join(",")
            ));
        }
        r.write(&format!("k8s/m{f}.yaml"), &text);
    }
    for i in 0..3000 {
        r.write(&format!("other/f{i}.yaml"), "a: 1\n");
    }
    r.rules("service W from k8s:w0\n");
    let t = std::time::Instant::now();
    let rep = r.report();
    assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    assert!(!rep.deploy_unread.is_empty(), "vượt trần thì phải báo");
}

#[test]
fn luat_nhieu_nguon_sai_bi_bao() {
    for (text, want) in [
        (
            "service A from compose:a, dockerfile:x/Dockerfile\n",
            "`dockerfile:` thừa",
        ),
        (
            "service A from k8s:a, dockerfile:x/Dockerfile, dockerfile:y/Dockerfile\n",
            "`dockerfile:` thừa",
        ),
        (
            "service A from k8s:a\nservice B from k8s:a\n",
            "đã dùng cho service",
        ),
        ("service A from k8s:../a\n", "phải là đường dẫn tương đối"),
        (
            "service A from k8s:a,,compose:b\n",
            "viết sai thứ tự hay thừa chữ",
        ),
    ] {
        let (_, diags) = arch_core::load(text);
        assert!(
            diags.iter().any(|d| d.msg.contains(want)),
            "{text}: {diags:#?}"
        );
    }
    let (doc, diags) = arch_core::load(
        "service A from compose:a, k8s:deploy/a, dockerfile:a/Dockerfile runs X\ncomponent X at x/**\n",
    );
    assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
    assert_eq!(
        doc.services[0].sources_text(),
        "compose:a, k8s:deploy/a, dockerfile:a/Dockerfile"
    );
}

#[test]
fn bao_cao_vi_pham_mang_qua_k8s_dung_reason() {
    let r = shop("reason");
    r.rules("component Api at api/**\ncomponent Worker at worker/**\nservice Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\nlink Api -> Worker\n");
    let rep = r.report();
    assert!(
        rep.violations
            .iter()
            .all(|v| v.reason == Reason::NotLinked && v.to == "Db"),
        "{:?}",
        violations(&rep)
    );
}

/// Luật chỉ có `link Api -> Worker`: mọi cách trỏ tới `db` phải ra vi phạm.
fn only_worker_link(r: &Repo) {
    r.rules(
        "component Api at api/**\ncomponent Worker at worker/**\n\
         service Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\n\
         link Api -> Worker\n",
    );
}

#[test]
fn cac_cach_goi_ten_may_cua_kubernetes() {
    for (name, value, extra) in [
        // Namespace `default` luôn có; namespace khai bằng `kind: Namespace`.
        ("default", "db.default", ""),
        (
            "namespace",
            "db.prod",
            "---\napiVersion: v1\nkind: Namespace\nmetadata: {name: prod}\n",
        ),
        // `$(TÊN)` lấy giá trị của biến khai trước.
        ("dollar", "postgres://$(DB_NAME):5432/app", ""),
        // Service `ExternalName` là bí danh của Service trong cụm.
        (
            "alias",
            "dbalias",
            "---\napiVersion: v1\nkind: Service\nmetadata: {name: dbalias}\nspec: {type: ExternalName, externalName: db.default.svc.cluster.local.}\n",
        ),
    ] {
        let r = shop(name);
        let api = API.replace(
            "        - name: DB_HOST\n          value: db\n",
            &format!("        - name: DB_NAME\n          value: db\n        - name: DB_HOST\n          value: \"{value}\"\n"),
        );
        r.write("k8s/api.yaml", &api).write(
            "k8s/more.yaml",
            &format!("apiVersion: v1\nkind: ConfigMap\nmetadata: {{name: x}}\n{extra}"),
        );
        only_worker_link(&r);
        let found = violations(&r.report());
        assert!(
            found
                .iter()
                .any(|v| v.starts_with("NotLinked Api -> Db k8s/api.yaml")),
            "{name}: {found:?}"
        );
    }
}

#[test]
fn nhan_controller_va_loai_list() {
    let r = shop("controller");
    r.write(
        "k8s/db.yaml",
        "apiVersion: apps/v1\nkind: StatefulSetList\nitems:\n- apiVersion: apps/v1\n  kind: StatefulSet\n  metadata: {name: db}\n  spec:\n    template:\n      metadata:\n        labels: {app: db}\n      spec:\n        containers:\n        - {name: pg, image: postgres:16}\n---\napiVersion: v1\nkind: Service\nmetadata: {name: db}\nspec:\n  selector: {statefulset.kubernetes.io/pod-name: db-0}\n",
    );
    only_worker_link(&r);
    assert!(
        violations(&r.report())
            .iter()
            .any(|v| v.starts_with("NotLinked Api -> Db")),
        "{:?}",
        violations(&r.report())
    );
}

#[test]
fn manifest_khong_dung_dang_quen_van_duoc_doc() {
    let flow = "{\"apiVersion\": \"v1\", \"kind\": \"Service\", \"metadata\": {\"name\": \"db\"}, \"spec\": {\"selector\": {\"app\": \"db\"}}}\n";
    for (name, file, body) in [
        ("json", "k8s/db-svc.json", flow.to_string()),
        ("bom", "k8s/db-svc.yaml", format!("\u{feff}{}", "apiVersion: v1\nkind: Service\nmetadata: {name: db}\nspec:\n  selector: {app: db}\n")),
        ("thut-le", "k8s/db-svc.yaml", "  apiVersion: v1\n  kind: Service\n  metadata: {name: db}\n  spec:\n    selector: {app: db}\n".into()),
    ] {
        let r = shop(name);
        let db = std::fs::read_to_string(r.0.join("k8s/db.yaml")).unwrap();
        r.write("k8s/db.yaml", db.split("---").next().unwrap()).write(file, &body);
        only_worker_link(&r);
        assert!(violations(&r.report()).iter().any(|v| v.starts_with("NotLinked Api -> Db")), "{name}: {:?}", violations(&r.report()));
    }
}

#[test]
fn nhan_qua_tran_thi_bao_chu_khong_cat() {
    let r = shop("nhan");
    let labels: String = (0..300).map(|i| format!("        l{i}: x\n")).collect();
    r.write(
        "k8s/db.yaml",
        &std::fs::read_to_string(r.0.join("k8s/db.yaml"))
            .unwrap()
            .replace(
                "      labels: {app: db}\n",
                &format!("      labels:\n{labels}        app: db\n"),
            ),
    );
    only_worker_link(&r);
    let rep = r.report();
    assert!(
        rep.deploy_unread.iter().any(|u| u.file == "k8s/db.yaml"),
        "{:?}",
        rep.deploy_unread
    );
}

#[test]
fn bien_doi_cua_kustomize_va_json6902_thi_bao() {
    let r = shop("transform");
    r.write(
        "k8s/kustomization.yaml",
        "namePrefix: prod-\nresources: [api.yaml, worker.yaml, db.yaml, cfg.yaml]\n",
    );
    let rep = r.report();
    assert!(
        rep.deploy_unread
            .iter()
            .any(|u| u.why.starts_with("`namePrefix` của Kustomize")),
        "{:?}",
        rep.deploy_unread
    );
    r.write(
        "k8s/kustomization.yaml",
        "resources: [api.yaml, worker.yaml, db.yaml, cfg.yaml]\npatches:\n- target: {kind: Deployment, name: api}\n  patch: |-\n    - op: replace\n      path: /spec/template/spec\n      value: {}\n",
    );
    let rep = r.report();
    assert!(
        rep.deploy_unread
            .iter()
            .any(|u| u.why.starts_with("patch Kustomize")),
        "{:?}",
        rep.deploy_unread
    );
}

#[test]
fn file_o_thu_muc_vi_du_duoc_kustomization_keo_vao_thi_doc() {
    let r = shop("keo-vao");
    r.write("k8s/kustomization.yaml", "resources: [api.yaml, worker.yaml, ../examples/cache.yaml]\n").write(
        "examples/cache.yaml",
        "apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: cache}\nspec:\n  template:\n    metadata:\n      labels: {app: cache}\n    spec:\n      containers: [{name: c, image: redis}]\n---\napiVersion: v1\nkind: Service\nmetadata: {name: cache}\nspec:\n  selector: {app: cache}\n",
    );
    r.write(
        "k8s/api.yaml",
        &API.replace("value: db\n", "value: cache\n"),
    );
    let rep = r.report();
    assert!(
        violations(&rep)
            .iter()
            .any(|v| v.starts_with("NotLinked Api -> cache")),
        "{:?}",
        violations(&rep)
    );
}

#[test]
fn service_khong_runs_ma_chay_ma_cua_repo_la_vi_pham() {
    let r = shop("khong-runs");
    // `db` dựng từ mã của Worker mà luật nói nó không chạy mã nào của dự án.
    r.write(
        "k8s/db.yaml",
        "apiVersion: apps/v1\nkind: StatefulSet\nmetadata: {name: db}\nspec:\n  template:\n    metadata:\n      labels: {app: db}\n    spec:\n      containers:\n      - {name: pg, image: worker}\n---\napiVersion: v1\nkind: Service\nmetadata: {name: db}\nspec:\n  selector: {app: db}\n",
    );
    let rep = r.report();
    assert!(
        violations(&rep).contains(&"NotRun Db -> Worker worker/Dockerfile:4".to_string()),
        "{:?}",
        violations(&rep)
    );
}

#[test]
fn ban_sao_o_thu_muc_khac_cung_bi_kiem() {
    let r = shop("ban-sao");
    // Bản phát hành cùng image, đường mạng mới chỉ có ở đây.
    r.write(
        "release/api.yaml",
        &API.replace("image: api", "image: gcr.io/acme/api:v1")
            .replace("value: db\n", "value: \"worker:9\"\n"),
    );
    only_worker_link(&r);
    let found = violations(&r.report());
    assert!(
        found
            .iter()
            .any(|v| v.starts_with("NotLinked Api -> Db k8s/api.yaml")),
        "{found:?}"
    );
    // Đổi thêm cạnh ở bản sao: vẫn bị kiểm.
    r.write(
        "release/api.yaml",
        &API.replace("image: api", "image: gcr.io/acme/api:v1")
            .replace(
                "        - name: WORKER_URL",
                "        - name: CACHE_HOST\n          value: db\n        - name: WORKER_URL",
            ),
    );
    let found = violations(&r.report());
    assert!(
        found
            .iter()
            .any(|v| v.starts_with("NotLinked Api -> Db release/api.yaml")),
        "{found:?}"
    );
}

#[test]
fn hai_ung_dung_cung_ten_thi_tach() {
    let r = shop("monorepo");
    r.write("blog/Dockerfile", PY_DOCKERFILE).write("blog/main.py", "import os\n").write(
        "blog/k8s/api.yaml",
        "apiVersion: apps/v1\nkind: Deployment\nmetadata: {name: api}\nspec:\n  template:\n    metadata:\n      labels: {app: api}\n    spec:\n      containers: [{name: c, image: blog}]\n",
    )
    .write(
        "skaffold.yaml",
        "apiVersion: skaffold/v4beta6\nkind: Config\nbuild:\n  artifacts:\n  - image: api\n    context: api\n  - image: worker\n    context: worker\n  - image: blog\n    context: blog\n",
    );
    let found = arch_core::discover::discover(&r.0, &[]);
    let sources: Vec<&str> = found.services.iter().map(|s| s.source.as_str()).collect();
    assert!(
        sources.contains(&"k8s:k8s/api") && sources.contains(&"k8s:blog/k8s/api"),
        "{sources:?}"
    );
    assert!(
        found.notes.iter().any(|n| n.contains("tách")),
        "{:?}",
        found.notes
    );
}

#[test]
fn sidecar_lan_duoc_khong_che_container_chinh() {
    let r = shop("sidecar");
    r.write(
        "k8s/api.yaml",
        &API.replace("        image: api\n", "        image: ghcr.io/acme/frontend:1\n").replace(
            "        - name: CACHE_ADDR",
            "        - name: X\n          value: y\n        - name: CACHE_ADDR",
        )
        .replace("      containers:\n      - name: server", "      containers:\n      - name: log\n        image: worker\n      - name: server")
        .replace("    metadata:\n      labels: {app: api}", "    metadata:\n      labels: {app: api}\n      annotations: {kubectl.kubernetes.io/default-container: server}"),
    );
    r.rules("component Api at api/**\ncomponent Worker at worker/**\nservice Api from k8s:api runs Api, Worker\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\nlink Api -> Db, Worker\n");
    let rep = r.report();
    assert!(
        rep.entry_unknown
            .iter()
            .any(|(s, why)| s == "Api" && why.contains("ghcr.io/acme/frontend")),
        "{:?}",
        rep.entry_unknown
    );
}

#[test]
fn ten_may_namespace_la_thi_bao() {
    let r = shop("ns-la");
    r.write(
        "k8s/api.yaml",
        &API.replace("value: db\n", "value: \"db.staging:5432\"\n"),
    );
    only_worker_link(&r);
    let rep = r.report();
    assert!(
        rep.deploy_unread
            .iter()
            .any(|u| u.why.contains("namespace `staging`")),
        "{:?}",
        rep.deploy_unread
    );
}

#[test]
fn cung_ten_may_lap_lai_va_patch_nhan_workload_khong_lam_treo() {
    let r = Repo::new("lap-lai");
    let mut svc = String::new();
    for i in 0..1000 {
        svc.push_str(&format!("---\napiVersion: v1\nkind: Service\nmetadata: {{name: db, namespace: n{i}}}\nspec: {{selector: {{app: db}}}}\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: d{i}}}\nspec:\n  template:\n    metadata: {{labels: {{app: db}}}}\n    spec: {{containers: [{{name: c, image: x}}]}}\n"));
    }
    r.write("k8s/db.yaml", &svc);
    let env: String = (0..3000)
        .map(|i| format!("        - {{name: A{i}_ADDR, value: \"db:5432\"}}\n"))
        .collect();
    r.write("k8s/client.yaml", &format!("apiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: client}}\nspec:\n  template:\n    spec:\n      containers:\n      - name: c\n        image: x\n        env:\n{env}"));
    let patches: String = (0..7500).map(|_| "- target: {kind: Deployment}\n  patch: '[{\"op\": \"add\", \"path\": \"/spec/template/spec/containers/0/env/-\", \"value\": {}}]'\n".to_string()).collect();
    r.write(
        "shared/kustomization.yaml",
        &format!("kind: Component\npatches:\n{patches}"),
    );
    for i in 0..20 {
        r.write(
            &format!("r{i}/kustomization.yaml"),
            "resources: [../k8s/db.yaml]\ncomponents: [../shared]\n",
        );
    }
    r.rules("service Client from k8s:client\nservice D from k8s:d0\n");
    let t = std::time::Instant::now();
    let rep = r.partial(&["k8s/client.yaml"]);
    assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    assert!(!rep.violations.is_empty() || !rep.deploy_unread.is_empty());
}

fn policy(name: &str, target: &str, body: &str) -> String {
    format!(
        "apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata: {{name: {name}}}\nspec:\n  podSelector: {target}\n{body}"
    )
}

#[test]
fn networkpolicy_cho_phep_ma_thieu_link_la_vi_pham() {
    let r = shop("np-grant");
    // Worker chỉ nhận từ Api; Db nhận từ Worker — đường mà env không cho thấy.
    r.write(
        "k8s/np.yaml",
        &format!(
            "{}---\n{}",
            policy("worker", "{matchLabels: {app: worker}}", "  ingress:\n  - from:\n    - podSelector: {matchLabels: {app: api}}\n"),
            policy("db", "{matchLabels: {app: db}}", "  ingress:\n  - from:\n    - podSelector:\n        matchExpressions: [{key: app, operator: In, values: [worker, api]}]\n")
        ),
    );
    let rep = r.report();
    let found = violations(&rep);
    assert_eq!(
        found,
        ["NotLinked Worker -> Db k8s/np.yaml:17"],
        "{found:?}"
    );
    assert!(rep.violations[0].spec.starts_with("NetworkPolicy `db`"));
    assert!(
        rep.links_blocked.is_empty() && rep.links_unseen.is_empty(),
        "{:?} {:?}",
        rep.links_blocked,
        rep.links_unseen
    );
    // `init` sinh `link` cho đường policy cho phép.
    let found = arch_core::discover::discover(&r.0, &[]);
    assert!(
        found
            .links
            .iter()
            .any(|(f, t)| f == "Worker" && t.contains(&"Db".to_string())),
        "{:?}",
        found.links
    );
}

#[test]
fn link_bi_networkpolicy_chan_thi_canh_bao() {
    let r = shop("np-block");
    // Chặn mọi đường vào mọi pod, rồi chỉ mở Api -> Db.
    r.write(
        "k8s/np.yaml",
        &format!(
            "{}---\n{}",
            policy("deny-all", "{}", "  policyTypes: [Ingress]\n"),
            policy(
                "db",
                "{matchLabels: {app: db}}",
                "  ingress:\n  - from:\n    - podSelector: {matchLabels: {app: api}}\n"
            )
        ),
    );
    let rep = r.report();
    assert_eq!(
        rep.links_blocked
            .iter()
            .map(|(f, t, _)| format!("{f}->{t}"))
            .collect::<Vec<_>>(),
        ["Api->Worker"]
    );
    assert!(rep.violations.is_empty(), "{:?}", violations(&rep));
}

#[test]
fn networkpolicy_khong_ket_luan_thi_khong_bao() {
    let r = shop("np-mo");
    r.write(
        "k8s/np.yaml",
        &format!(
            "{}---\n{}---\n{}",
            // Namespace theo nhãn mà repo không cho biết: không kết luận.
            policy(
                "worker",
                "{matchLabels: {app: worker}}",
                "  ingress:\n  - from:\n    - namespaceSelector: {matchLabels: {team: x}}\n"
            ),
            // `podSelector: {}` và ipBlock: không nêu workload nào.
            policy(
                "db",
                "{matchLabels: {app: db}}",
                "  ingress:\n  - from:\n    - podSelector: {}\n    - ipBlock: {cidr: 10.0.0.0/8}\n"
            ),
            // Chỉ chặn chiều ra của pod không ai nhắc tới.
            policy(
                "egress",
                "{matchLabels: {app: nobody}}",
                "  policyTypes: [Egress]\n"
            )
        ),
    );
    let rep = r.report();
    assert!(
        rep.violations.is_empty() && rep.links_blocked.is_empty(),
        "{:?} {:?}",
        violations(&rep),
        rep.links_blocked
    );
}

#[test]
fn networkpolicy_chieu_ra_va_namespace_theo_ten() {
    let r = shop("np-egress");
    r.write(
        "k8s/np.yaml",
        &policy(
            "api-out",
            "{matchLabels: {app: api}}",
            "  policyTypes: [Egress]\n  egress:\n  - to:\n    - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: default}}\n      podSelector: {matchLabels: {app: db}}\n",
        ),
    );
    let rep = r.report();
    // Api chỉ được gọi ra Db: link Api -> Worker bị chặn, Api -> Db có link nên không vi phạm.
    assert_eq!(
        rep.links_blocked
            .iter()
            .map(|(f, t, _)| format!("{f}->{t}"))
            .collect::<Vec<_>>(),
        ["Api->Worker"]
    );
    assert!(rep.violations.is_empty(), "{:?}", violations(&rep));
}

#[test]
fn nhieu_networkpolicy_khong_lam_treo() {
    let r = Repo::new("np-dos");
    let mut text = String::new();
    for i in 0..1500 {
        text.push_str(&format!("---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: w{i}}}\nspec:\n  template:\n    metadata: {{labels: {{a: b}}}}\n    spec: {{containers: [{{name: c, image: x}}]}}\n"));
    }
    r.write("k8s/w.yaml", &text);
    let mut pol = String::new();
    for i in 0..1500 {
        pol.push_str(&format!("---\n{}", policy(&format!("p{i}"), "{matchLabels: {a: b}}", "  ingress:\n  - from:\n    - podSelector: {matchLabels: {a: b}}\n    - podSelector: {matchLabels: {a: b}}\n")));
    }
    r.write("k8s/p.yaml", &pol);
    r.rules("service W from k8s:w0\nservice V from k8s:w1\nlink W -> V\n");
    let t = std::time::Instant::now();
    let rep = r.report();
    assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    assert!(!rep.deploy_unread.is_empty() || !rep.violations.is_empty());
}

/// Ca vàng, kết quả viết tay theo ví dụ trong tài liệu NetworkPolicy của
/// Kubernetes, không suy từ mã: để thước đo không chỉ chép lại cách hiểu của
/// chính archctl.
#[test]
fn networkpolicy_ca_vang_theo_tai_lieu_kubernetes() {
    let blocked = |body: &str| -> Vec<String> {
        let r = shop("np-vang");
        r.write("k8s/np.yaml", body);
        let rep = r.report();
        assert!(rep.violations.is_empty(), "{:?}", violations(&rep));
        let mut b: Vec<String> = rep
            .links_blocked
            .iter()
            .map(|(f, t, _)| format!("{f}->{t}"))
            .collect();
        b.sort();
        b
    };
    // "Default deny all egress traffic": mọi pod mất chiều ra.
    assert_eq!(
        blocked(&policy(
            "default-deny-egress",
            "{}",
            "  policyTypes: [Egress]\n"
        )),
        ["Api->Db", "Api->Worker"]
    );
    // "Default deny all ingress traffic".
    assert_eq!(
        blocked(&policy(
            "default-deny-ingress",
            "{}",
            "  policyTypes: [Ingress]\n"
        )),
        ["Api->Db", "Api->Worker"]
    );
    // "Allow all ingress traffic": `ingress: [{}]`.
    assert!(
        blocked(&format!(
            "{}---\n{}",
            policy("deny", "{}", "  policyTypes: [Ingress]\n"),
            policy("allow-all", "{}", "  ingress:\n  - {}\n")
        ))
        .is_empty()
    );
    // Không ghi policyTypes, `egress: []`: chỉ là policy Ingress, chiều ra vẫn mở.
    assert!(
        blocked(&policy(
            "api",
            "{matchLabels: {app: api}}",
            "  egress: []\n"
        ))
        .is_empty()
    );
    // Policy ở namespace `other` không cô lập pod mà namespace không biết.
    assert!(blocked("apiVersion: networking.k8s.io/v1\nkind: NetworkPolicy\nmetadata: {name: d, namespace: other}\nspec:\n  podSelector: {}\n  policyTypes: [Ingress]\n").is_empty());
}

#[test]
fn mot_policy_nhieu_peer_va_nhieu_service_khong_lam_treo() {
    let r = Repo::new("np-tich");
    let mut wl = String::new();
    let mut rules = String::new();
    for i in 0..30 {
        wl.push_str(&format!("---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: w{i}}}\nspec:\n  template:\n    metadata: {{labels: {{a: b}}}}\n    spec: {{containers: [{{name: c, image: x}}]}}\n"));
        rules.push_str(&format!("service S{i} from k8s:w{i}\n"));
    }
    let peers: String = (0..5000)
        .map(|_| "    - podSelector: {matchLabels: {a: b}}\n")
        .collect();
    r.write("k8s/w.yaml", &wl).write(
        "k8s/p.yaml",
        &policy(
            "p",
            "{matchLabels: {a: b}}",
            &format!("  ingress:\n  - from:\n{peers}"),
        ),
    );
    r.rules(&rules);
    let t = std::time::Instant::now();
    let rep = r.report();
    assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    // Mỗi cặp service một vi phạm: 30 × 29.
    assert_eq!(rep.violations.len(), 30 * 29);
}

/// Lối vào từ Internet: mỗi dạng cấu hình lộ Db ra ngoài phải đòi `link Internet -> Db`.
#[test]
fn moi_dang_lo_ra_ngoai_deu_doi_link_internet() {
    let svc = |extra: &str| {
        format!(
            "---\napiVersion: v1\nkind: Service\nmetadata: {{name: db-ext}}\nspec:\n  selector: {{app: db}}\n{extra}"
        )
    };
    let ingress_v1 = "---\napiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata: {name: web}\nspec:\n  rules:\n  - http:\n      paths:\n      - path: /\n        backend: {service: {name: db, port: {number: 5432}}}\n";
    let ingress_beta = "---\napiVersion: extensions/v1beta1\nkind: Ingress\nmetadata: {name: web}\nspec:\n  backend: {serviceName: db, servicePort: 5432}\n";
    let ingress_default = "---\napiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata: {name: web}\nspec:\n  defaultBackend: {service: {name: db}}\n";
    let httproute = "---\napiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata: {name: r}\nspec:\n  parentRefs: [{name: public-gw}]\n  rules:\n  - backendRefs: [{name: db, port: 5432}]\n";
    let vs = "---\napiVersion: networking.istio.io/v1beta1\nkind: VirtualService\nmetadata: {name: v}\nspec:\n  gateways: [public-gw]\n  hosts: ['*']\n  tcp:\n  - route: [{destination: {host: db.default.svc.cluster.local}}]\n";
    for (name, extra) in [
        ("lb", svc("  type: LoadBalancer\n")),
        ("nodeport", svc("  type: NodePort\n")),
        ("externalips", svc("  externalIPs: [203.0.113.7]\n")),
        ("ingress", ingress_v1.to_string()),
        ("ingress-beta", ingress_beta.to_string()),
        ("ingress-default", ingress_default.to_string()),
        ("httproute", httproute.to_string()),
        ("virtualservice", vs.to_string()),
    ] {
        let r = shop(&format!("lo-{name}"));
        r.write("k8s/expose.yaml", &extra);
        let rep = r.report();
        assert_eq!(
            rep.violations
                .iter()
                .map(|v| format!("{} -> {}", v.from, v.to))
                .collect::<Vec<_>>(),
            ["Internet -> Db"],
            "{name}: {:?}",
            violations(&rep)
        );
        // Có `link Internet -> Db` thì không còn vi phạm, và link được thấy.
        r.rules(
            "component Api at api/**\ncomponent Worker at worker/**\n\
             service Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\n\
             link Api -> Db, Worker\nlink Internet -> Db\n",
        );
        let rep = r.report();
        assert!(
            rep.violations.is_empty() && rep.links_unseen.is_empty(),
            "{name}: {:?} {:?}",
            violations(&rep),
            rep.links_unseen
        );
    }
}

#[test]
fn duong_trong_cum_khong_phai_loi_vao() {
    let r = shop("trong-cum");
    r.write(
        "k8s/mesh.yaml",
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata: {name: r}\nspec:\n  parentRefs: [{kind: Service, name: db}]\n  rules:\n  - backendRefs: [{name: db}]\n---\napiVersion: networking.istio.io/v1beta1\nkind: VirtualService\nmetadata: {name: v}\nspec:\n  gateways: [mesh]\n  http:\n  - route: [{destination: {host: db}}]\n---\napiVersion: v1\nkind: Service\nmetadata: {name: db-internal}\nspec:\n  type: ClusterIP\n  selector: {app: db}\n",
    );
    let rep = r.report();
    assert!(rep.violations.is_empty(), "{:?}", violations(&rep));
    // `link Internet -> Db` mà không gì lộ Db: link không thấy trong cấu hình.
    r.rules(
        "component Api at api/**\ncomponent Worker at worker/**\n\
         service Api from k8s:api runs Api\nservice Worker from k8s:worker runs Worker\nservice Db from k8s:db\n\
         link Api -> Db, Worker\nlink Internet -> Db\n",
    );
    let rep = r.report();
    assert_eq!(
        rep.links_unseen
            .iter()
            .map(|(f, t, _)| format!("{f}->{t}"))
            .collect::<Vec<_>>(),
        ["Internet->Db"]
    );
}

#[test]
fn pod_dung_mang_may_chu_la_lo_ra_ngoai() {
    for (name, spec) in [
        ("hostnetwork", "      hostNetwork: true\n"),
        ("hostport", ""),
    ] {
        let r = shop(&format!("host-{name}"));
        let ports = if name == "hostport" {
            ", ports: [{containerPort: 5432, hostPort: 5432}]"
        } else {
            ""
        };
        r.write(
            "k8s/db.yaml",
            &format!("apiVersion: apps/v1\nkind: StatefulSet\nmetadata: {{name: db}}\nspec:\n  template:\n    metadata:\n      labels: {{app: db}}\n    spec:\n{spec}      containers:\n      - {{name: pg, image: postgres:16{ports}}}\n---\napiVersion: v1\nkind: Service\nmetadata: {{name: db}}\nspec:\n  selector: {{app: db}}\n"),
        );
        let found: Vec<String> = r
            .report()
            .violations
            .iter()
            .map(|v| format!("{} -> {}", v.from, v.to))
            .collect();
        assert_eq!(found, ["Internet -> Db"], "{name}");
    }
}

#[test]
fn luat_internet_sai_bi_bao() {
    for (text, want) in [
        ("service Internet\n", "tên dành riêng"),
        ("service A\nlink A -> Internet\n", "chỉ đứng ở đầu"),
    ] {
        let (_, diags) = arch_core::load(text);
        assert!(
            diags.iter().any(|d| d.msg.contains(want)),
            "{text}: {diags:#?}"
        );
    }
    let (_, diags) = arch_core::load("service A\nlink Internet -> A\n");
    assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
}

/// Các ca reviewer tìm: route khác namespace, `hostNetwork: True`, host ngoài
/// cụm của VirtualService, `delegate`, route lặp một Service hàng nghìn lần.
#[test]
fn loi_vao_cac_ca_kho() {
    let internet = |r: &Repo| -> Vec<String> {
        r.report()
            .violations
            .iter()
            .filter(|v| v.from == "Internet")
            .map(|v| v.to.clone())
            .collect()
    };
    let ns_db = "apiVersion: apps/v1\nkind: StatefulSet\nmetadata: {name: db, namespace: data}\nspec:\n  template:\n    metadata:\n      labels: {app: db}\n    spec:\n      containers:\n      - {name: pg, image: postgres:16}\n---\napiVersion: v1\nkind: Service\nmetadata: {name: db, namespace: data}\nspec:\n  selector: {app: db}\n";
    // HTTPRoute ở namespace hạ tầng trỏ sang Service ở `data`.
    let r = shop("route-ns");
    r.write("k8s/db.yaml", ns_db).write(
        "k8s/route.yaml",
        "apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata: {name: r, namespace: edge}\nspec:\n  parentRefs: [{name: public-gw}]\n  rules:\n  - backendRefs: [{name: db, namespace: data}]\n",
    );
    assert_eq!(internet(&r), ["Db"]);
    // `hostNetwork: True` cũng là đúng với Kubernetes.
    let r = shop("host-hoa");
    r.write(
        "k8s/db.yaml",
        &std::fs::read_to_string(r.0.join("k8s/db.yaml"))
            .unwrap()
            .replace(
                "    spec:\n      containers:",
                "    spec:\n      hostNetwork: True\n      containers:",
            ),
    );
    assert_eq!(internet(&r), ["Db"]);
    // Host ngoài cụm không phải Service `db`.
    let r = shop("vs-ngoai");
    r.write("k8s/vs.yaml", "apiVersion: networking.istio.io/v1beta1\nkind: VirtualService\nmetadata: {name: v}\nspec:\n  gateways: [public-gw]\n  http:\n  - route: [{destination: {host: db.example.com}}]\n");
    assert!(internet(&r).is_empty(), "{:?}", internet(&r));
    // `delegate` sang VirtualService con không gắn gateway.
    let r = shop("vs-delegate");
    r.write("k8s/vs.yaml", "apiVersion: networking.istio.io/v1beta1\nkind: VirtualService\nmetadata: {name: root}\nspec:\n  gateways: [public-gw]\n  http:\n  - delegate: {name: child}\n---\napiVersion: networking.istio.io/v1beta1\nkind: VirtualService\nmetadata: {name: child}\nspec:\n  http:\n  - route: [{destination: {host: db}}]\n");
    assert_eq!(internet(&r), ["Db"]);
    // Một route lặp một Service hàng chục nghìn lần, trước 3 000 workload.
    let r = Repo::new("route-lap");
    let mut wl = String::new();
    for i in 0..3000 {
        wl.push_str(&format!("---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: w{i}}}\nspec:\n  template:\n    metadata: {{labels: {{app: x}}}}\n    spec: {{containers: [{{name: c, image: x}}]}}\n"));
    }
    let refs: String = (0..40_000).map(|_| "  - {name: web}\n").collect();
    r.write("k8s/w.yaml", &wl)
        .write("k8s/s.yaml", "apiVersion: v1\nkind: Service\nmetadata: {name: web}\nspec: {selector: {app: x}}\n")
        .write("k8s/r.yaml", &format!("apiVersion: gateway.networking.k8s.io/v1\nkind: HTTPRoute\nmetadata: {{name: r}}\nspec:\n  parentRefs: [{{name: gw}}]\n  rules:\n  - backendRefs:\n{refs}"))
        .rules("service W from k8s:w0\n");
    let t = std::time::Instant::now();
    let rep = r.partial(&["k8s/r.yaml"]);
    assert!(t.elapsed().as_secs_f64() < 5.0, "{:?}", t.elapsed());
    assert!(!rep.violations.is_empty() || !rep.deploy_unread.is_empty());
}

#[test]
fn ten_may_trong_ma_qua_service_kubernetes() {
    let r = shop("ma-k8s");
    // Worker không có `link` tới Db; Api có `link` tới Worker.
    r.write(
        "worker/main.py",
        "import os\nDB = os.getenv('DB_HOST', 'db.default.svc.cluster.local')\n",
    )
    .write("api/main.py", "import os\nW = 'http://worker:8080/jobs'\n");
    let got: Vec<String> = r
        .report()
        .violations
        .iter()
        .map(|v| format!("{} -> {} {}:{} {}", v.from, v.to, v.file, v.line, v.spec))
        .collect();
    assert_eq!(
        got,
        ["Worker -> Db worker/main.py:2 DB_HOST → db.default.svc.cluster.local"]
    );
}

#[test]
fn ten_may_trong_ma_uu_tien_cung_ung_dung() {
    // Monorepo: `app1/k8s` và `app2/k8s` cùng có Service `api`.
    let r = Repo::new("ma-monorepo");
    for app in ["app1", "app2"] {
        r.write(&format!("{app}/web/Dockerfile"), PY_DOCKERFILE)
            .write(
                &format!("{app}/web/main.py"),
                "import requests\nrequests.get('http://api:8080/x')\n",
            );
        r.write(
            &format!("{app}/k8s/all.yaml"),
            &format!(
                "apiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: web}}\nspec:\n  template:\n    metadata: {{labels: {{app: web}}}}\n    spec: {{containers: [{{name: w, image: {app}-web}}]}}\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata: {{name: api}}\nspec:\n  template:\n    metadata: {{labels: {{app: api}}}}\n    spec: {{containers: [{{name: a, image: {app}-api}}]}}\n---\napiVersion: v1\nkind: Service\nmetadata: {{name: api}}\nspec: {{selector: {{app: api}}}}\n"
            ),
        );
    }
    r.write("skaffold.yaml", "apiVersion: skaffold/v4beta6\nkind: Config\nbuild:\n  artifacts:\n  - image: app1-web\n    context: app1/web\n  - image: app2-web\n    context: app2/web\nmanifests:\n  rawYaml: [app1/k8s/*.yaml, app2/k8s/*.yaml]\n")
        .rules(
            "component Web1 at app1/web/**\ncomponent Web2 at app2/web/**\n\
             service Web1s from k8s:app1/k8s/web runs Web1\nservice Api1s from k8s:app1/k8s/api\n\
             service Web2s from k8s:app2/k8s/web runs Web2\nservice Api2s from k8s:app2/k8s/api\n\
             link Web1s -> Api1s\nlink Web2s -> Api2s\n",
        );
    let got: Vec<String> = r
        .report()
        .violations
        .iter()
        .map(|v| format!("{} -> {}", v.from, v.to))
        .collect();
    assert!(got.is_empty(), "{got:?}");
    // Gỡ link của app1: chỉ đích cùng ứng dụng bị báo.
    let law = std::fs::read_to_string(r.0.join(".arch/system.arch"))
        .unwrap()
        .replace("link Web1s -> Api1s\n", "");
    r.rules(&law);
    let got: Vec<String> = r
        .report()
        .violations
        .iter()
        .map(|v| format!("{} -> {} {}", v.from, v.to, v.file))
        .collect();
    assert_eq!(got, ["Web1s -> Api1s app1/web/main.py"]);
}

/// Chart Helm nhỏ: `web` gọi `db` qua env, địa chỉ lấy từ values.
fn helm_repo(name: &str, values: &str) -> Repo {
    let r = Repo::new(name);
    r.write("chart/Chart.yaml", "apiVersion: v2\nname: shop\nversion: 0.1.0\n")
        .write("chart/values.yaml", values)
        .write("chart/templates/_helpers.tpl", "{{- define \"shop.labels\" -}}\napp: {{ .name }}\n{{- end }}\n")
        .write(
            "chart/templates/web.yaml",
            "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: web\nspec:\n  template:\n    metadata:\n      labels:\n        {{- include \"shop.labels\" (dict \"name\" \"web\") | nindent 8 }}\n    spec:\n      containers:\n      - name: web\n        image: web:1\n        env:\n        - name: DB_ADDR\n          value: {{ printf \"%s:5432\" .Values.db.host | quote }}\n",
        )
        .write(
            "chart/templates/db.yaml",
            "{{- if .Values.db.create }}\napiVersion: apps/v1\nkind: StatefulSet\nmetadata:\n  name: db\nspec:\n  template:\n    metadata:\n      labels: {app: db}\n    spec:\n      containers: [{name: pg, image: postgres:16}]\n---\napiVersion: v1\nkind: Service\nmetadata:\n  name: db\nspec:\n  selector: {app: db}\n{{- end }}\n",
        )
        .rules("service Web from k8s:web\nservice Db from k8s:db\n");
    r
}

#[test]
fn chart_helm_doi_link_nhu_manifest() {
    let r = helm_repo("helm-link", "db:\n  create: true\n  host: db\n");
    let got: Vec<String> = r
        .report()
        .violations
        .iter()
        .map(|v| format!("{} -> {} {}:{}", v.from, v.to, v.file, v.line))
        .collect();
    // Dòng là dòng của template, không phải của bản dựng ra.
    assert_eq!(got, ["Web -> Db chart/templates/web.yaml:15"]);
    // Hook: sửa `values.yaml` thì kiểm lại.
    let hook = r.partial(&["chart/values.yaml"]);
    assert_eq!(hook.violations.len(), 1, "{:?}", hook.violations);
    // `create: false`: không còn workload `db`.
    let r = helm_repo("helm-off", "db:\n  create: false\n  host: db\n");
    assert!(
        r.report().violations.is_empty(),
        "{:?}",
        r.report().violations
    );
}

#[test]
fn chart_helm_values_khai_trong_luat() {
    let r = helm_repo("helm-values", "db:\n  create: true\n  host: nowhere\n");
    assert!(r.report().violations.is_empty());
    r.write("deploy/prod.yaml", "db:\n  host: db\n")
        .rules("service Web from k8s:chart/web, values:deploy/prod.yaml\nservice Db from k8s:db\n");
    let got: Vec<String> = r
        .report()
        .violations
        .iter()
        .map(|v| format!("{} -> {}", v.from, v.to))
        .collect();
    assert_eq!(got, ["Web -> Db"]);
    // File values khai mà không có: báo, không lặng lẽ dùng mặc định.
    r.rules("service Web from k8s:chart/web, values:deploy/none.yaml\nservice Db from k8s:db\n");
    assert!(
        r.report()
            .deploy_unread
            .iter()
            .any(|u| u.file == "deploy/none.yaml"),
        "{:?}",
        r.report().deploy_unread
    );
    // Nguồn `k8s:` không nêu thư mục mà repo có hai chart: báo, không áp
    // `values:` cho cả hai.
    r.write(
        "other/Chart.yaml",
        "apiVersion: v2\nname: other\nversion: 0.1.0\n",
    )
    .rules("service Web from k8s:web, values:deploy/prod.yaml\nservice Db from k8s:db\n");
    let rep = r.report();
    assert!(
        rep.deploy_unread
            .iter()
            .any(|u| u.file == "deploy/prod.yaml" && u.why.contains("nhiều chart")),
        "{:?}",
        rep.deploy_unread
    );
    assert!(rep.violations.is_empty(), "{:?}", rep.violations);
}

#[test]
fn chart_helm_ngoai_tap_con_thi_bao() {
    let r = helm_repo("helm-tpl", "db:\n  create: true\n  host: db\n");
    r.write("chart/templates/x.yaml", "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: x\ndata:\n  a: {{ tpl .Values.db.host . }}\n");
    let rep = r.report();
    assert!(
        rep.deploy_unread
            .iter()
            .any(|u| u.file == "chart/templates/x.yaml" && u.why.contains("tpl")),
        "{:?}",
        rep.deploy_unread
    );
    // Lý do không trích giá trị.
    assert!(rep.deploy_unread.iter().all(|u| !u.why.contains("nowhere")));
}
