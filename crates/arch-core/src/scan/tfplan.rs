//! Đọc plan JSON của Terraform/OpenTofu (`tofu show -json`), phần lấp chỗ mà
//! bộ đọc HCL ([`super::terraform`]) báo "chưa đọc được": module từ registry,
//! `count`/`for_each` không hằng, hàm lạ.
//!
//! Plan do lệnh `verify` người dùng khai tạo ra; ở đây chỉ đọc file, không
//! chạy gì. Nơi gọi quyết plan có còn khớp file `.tf` không (TM-57). Luật cạnh,
//! lối vào như bộ đọc HCL và như thước đo `bench/tf_truth.py`: tham chiếu lấy
//! từ `configuration` (`expressions.*.references`, qua biến, output của module),
//! giá trị đã biết từ `planned_values`. Plan không giữ `locals`: chúng được đọc
//! từ file HCL của module (module tải về nằm dưới `.terraform/modules`).
//!
//! Giá trị trong plan có thể là secret: chỉ đọc địa chỉ, loại, tham chiếu, vài
//! giá trị quyết lối vào và image; không giá trị nào khác ra khỏi đây (TM-58).

use super::hcl::{self, Seg, E};
use crate::jsonc::Value as J;
use std::collections::{HashMap, HashSet};

/// Cỡ tối đa của một file plan JSON.
pub const MAX_PLAN_BYTES: u64 = 32 << 20;
/// Bước giải tham chiếu cho một plan (TM-59).
pub const MAX_STEPS: usize = 2_000_000;
/// Số module tối đa trong cây của một plan.
pub const MAX_MODULES: usize = 2000;

const COMPUTE: &[&str] = &[
    "aws_ecs_service",
    "aws_ecs_task_definition",
    "aws_lambda_function",
    "google_cloud_run_v2_service",
    "google_cloud_run_service",
];

/// Một service hay đích của cạnh theo plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub addr: String,
    pub ty: String,
    pub compute: bool,
    /// Đích và cách hiển thị (`container_definitions → aws_db_instance.main.address`).
    pub edges: Vec<(String, String)>,
    pub exposed: bool,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Out {
    pub units: Vec<Unit>,
    /// Lý do phần nào của plan không đọc được.
    pub unread: Vec<String>,
}

/// Đọc file của module trong repo.
pub struct Fs<'a> {
    pub read: &'a dyn Fn(&str) -> Result<String, String>,
    /// File `.tf`, `.tf.json` trực tiếp trong một thư mục (đường dẫn trong
    /// repo), và có bị cắt ở trần số file không.
    pub list: &'a dyn Fn(&str) -> (Vec<String>, bool),
}

/// Loại của một địa chỉ, bỏ tiền tố module và chỉ số; `aws_alb*` là `aws_lb*`.
pub fn type_of(addr: &str) -> String {
    let parts: Vec<&str> = addr.split('.').collect();
    let mut i = 0;
    while parts.len() > i + 2 && parts[i] == "module" {
        i += 2;
    }
    let t = if parts.get(i) == Some(&"data") { parts.get(i + 1) } else { parts.get(i) };
    let t = t.copied().unwrap_or("");
    match t {
        "aws_alb" => "aws_lb".into(),
        "aws_alb_listener" => "aws_lb_listener".into(),
        "aws_alb_listener_rule" => "aws_lb_listener_rule".into(),
        "aws_alb_target_group" => "aws_lb_target_group".into(),
        t => t.into(),
    }
}

/// Bỏ chỉ số `[0]`, `["a.b"]` khỏi địa chỉ (cả ở module), đi qua chuỗi trong nháy.
pub fn strip_index(addr: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut esc = false;
    for c in addr.chars() {
        if depth > 0 {
            if quoted {
                if esc {
                    esc = false;
                } else if c == '\\' {
                    esc = true;
                } else if c == '"' {
                    quoted = false;
                }
            } else if c == '"' {
                quoted = true;
            } else if c == '[' {
                depth += 1;
            } else if c == ']' {
                depth -= 1;
            }
            continue;
        }
        if c == '[' {
            depth = 1;
        } else {
            out.push(c);
        }
    }
    out
}

/// `references` trong một biểu thức của plan, đệ quy qua khối lồng.
fn config_refs(x: &J, out: &mut Vec<String>) {
    match x {
        J::Obj(fields) => {
            for (k, v) in fields {
                if k == "references" {
                    out.extend(v.as_array().unwrap_or(&[]).iter().filter_map(J::as_str).map(str::to_string));
                } else {
                    config_refs(v, out);
                }
            }
        }
        J::Arr(items) => items.iter().for_each(|v| config_refs(v, out)),
        _ => {}
    }
}

/// Tham chiếu trong một biểu thức HCL, dạng chuỗi như plan ghi
/// (`aws_db_instance.main.address`, `local.x`, `module.db.endpoint`).
/// Gặp biểu thức không đọc được (`for`, `%{}`) thì ghi lý do vào `bad`:
/// tham chiếu bên trong nó mất, không được coi là không có.
fn expr_refs(x: &hcl::Expr, out: &mut Vec<String>, bad: &mut Option<&'static str>) {
    match &x.e {
        E::Ref(root, segs) => {
            let mut s = root.clone();
            for seg in segs {
                match seg {
                    Seg::Attr(a) => {
                        s.push('.');
                        s.push_str(a);
                    }
                    Seg::Index(i) => {
                        expr_refs(i, out, bad);
                        break;
                    }
                    Seg::Splat => break,
                }
            }
            out.push(s);
        }
        E::Template(parts) => {
            for p in parts {
                if let hcl::Part::Interp(e) = p {
                    expr_refs(e, out, bad);
                }
            }
        }
        E::Call(_, args, _) | E::Tuple(args) | E::Op(_, args) => args.iter().for_each(|a| expr_refs(a, out, bad)),
        E::Object(kvs) => kvs.iter().for_each(|(k, v)| {
            expr_refs(k, out, bad);
            expr_refs(v, out, bad);
        }),
        E::Cond(a, b, c) => [a, b, c].iter().for_each(|e| expr_refs(e, out, bad)),
        E::Get(e, segs) => {
            expr_refs(e, out, bad);
            for seg in segs {
                if let Seg::Index(i) = seg {
                    expr_refs(i, out, bad);
                }
            }
        }
        E::Unsupported(why) => *bad = Some(why),
        _ => {}
    }
}

/// `locals` của một module: tên → tham chiếu, hay lý do không đọc được.
type Locals = HashMap<String, Result<Vec<String>, String>>;

/// Một module trong cây của plan.
struct Inst<'p> {
    prefix: String,
    config: &'p J,
    parent: Option<usize>,
    call: Option<&'p J>,
    children: HashMap<String, usize>,
    /// `None`: không có mã của module (registry chưa tải), `locals` không biết.
    /// Mỗi `local`: tham chiếu, hay lý do không đọc được.
    locals: Option<Locals>,
    /// Tên lời gọi, để báo.
    name: String,
}

struct Cx<'p> {
    insts: Vec<Inst<'p>>,
    memo: HashMap<(usize, String), Vec<(String, String)>>,
    busy: HashSet<(usize, String)>,
    steps: usize,
    unread: Vec<String>,
}

impl Cx<'_> {
    /// Tham chiếu → các cặp (địa chỉ tài nguyên đầy đủ, thuộc tính).
    fn resolve(&mut self, inst: usize, r: &str) -> Vec<(String, String)> {
        // Plan ghi cả `module.db[0].endpoint`, `aws_x.y["k"].attr` khi có
        // `count`/`for_each`: bỏ chỉ số như địa chỉ của tài nguyên.
        let r = strip_index(r);
        let key = (inst, r.clone());
        // Mọi lần gọi, cả lần trùng nhớ, đều tốn bước (TM-59).
        self.steps += 1;
        if let Some(v) = self.memo.get(&key) {
            return v.clone();
        }
        if self.busy.contains(&key) || self.steps >= MAX_STEPS {
            return Vec::new();
        }
        self.busy.insert(key.clone());
        let parts: Vec<&str> = r.split('.').collect();
        let mut out: Vec<(String, String)> = Vec::new();
        match (parts.first().copied(), parts.get(1).copied(), parts.get(2).copied()) {
            (Some("local"), Some(name), _) => match &self.insts[inst].locals {
                Some(locals) => match locals.get(name).cloned() {
                    Some(Ok(refs)) => {
                        for x in refs {
                            out.extend(self.resolve(inst, &x));
                        }
                    }
                    Some(Err(why)) => self.note(format!("`local.{name}` của module `{}`: {why}", self.insts[inst].name)),
                    None => self.note(format!("`local.{name}` không thấy trong mã của module `{}`", self.insts[inst].name)),
                },
                None => self.note(format!(
                    "module `{}` không có mã trong repo hay `.terraform/modules` (chưa `init`): `local.{name}` không lần được",
                    self.insts[inst].name
                )),
            },
            (Some("var"), Some(name), _) => {
                if let (Some(call), Some(parent)) = (self.insts[inst].call, self.insts[inst].parent) {
                    let mut refs = Vec::new();
                    if let Some(x) = call.get_path(&["expressions", name]) {
                        config_refs(x, &mut refs);
                    }
                    for x in refs {
                        out.extend(self.resolve(parent, &x));
                    }
                }
            }
            (Some("module"), Some(name), Some(output)) => {
                if let Some(&child) = self.insts[inst].children.get(name) {
                    let mut refs = Vec::new();
                    if let Some(x) = self.insts[child].config.get_path(&["outputs", output, "expression"]) {
                        config_refs(x, &mut refs);
                    }
                    for x in refs {
                        out.extend(self.resolve(child, &x));
                    }
                }
            }
            (Some("data" | "each" | "count" | "path" | "self" | "terraform" | "module"), _, _) => {}
            (Some(ty), Some(name), attr) => {
                out.push((format!("{}{ty}.{name}", self.insts[inst].prefix), attr.unwrap_or("").to_string()));
            }
            _ => {}
        }
        out.sort();
        out.dedup();
        self.busy.remove(&key);
        self.memo.insert(key, out.clone());
        out
    }

    fn note(&mut self, why: String) {
        if !self.unread.contains(&why) {
            self.unread.push(why);
        }
    }
}

/// `locals` của một module: tên → tham chiếu trong biểu thức (hay lý do không
/// đọc được), và các file không lấy được `locals`.
fn locals_of(dir: &str, fs: &Fs<'_>) -> (Locals, Vec<String>) {
    let mut out = HashMap::new();
    let mut lost = Vec::new();
    let (files, more) = (fs.list)(dir);
    if more {
        lost.push(format!("thư mục `{dir}` có quá nhiều file `.tf`; phần sau không đọc `locals`"));
    }
    for f in files {
        let name = f.rsplit('/').next().unwrap_or(&f);
        if name.ends_with(".tf.json") || name == "override.tf" || name.ends_with("_override.tf") {
            lost.push(format!("`locals` trong `{f}` (JSON hay override) chưa được đọc"));
            continue;
        }
        let body = match (fs.read)(&f).map_err(|e| e.to_string()).and_then(|t| hcl::parse(&t).map_err(|e| e.msg)) {
            Ok(b) => b,
            Err(why) => {
                lost.push(format!("không lấy được `locals` trong `{f}`: {why}"));
                continue;
            }
        };
        for it in &body.items {
            let hcl::Item::Block(b) = it else { continue };
            if b.kind != "locals" {
                continue;
            }
            for it in &b.body.items {
                if let hcl::Item::Attr(a) = it {
                    let mut refs = Vec::new();
                    let mut bad = None;
                    expr_refs(&a.expr, &mut refs, &mut bad);
                    out.insert(a.name.clone(), bad.map_or(Ok(refs), |why| Err(format!("{why} chưa được đọc"))));
                }
            }
        }
    }
    (out, lost)
}

/// Module chờ dựng: cấu hình, cha, lời gọi, tiền tố địa chỉ, khoá trong
/// `modules.json`, thư mục mã (nếu có).
type Pending<'p> = (&'p J, Option<usize>, Option<&'p J>, String, String, Option<String>);

/// Đọc plan của module gốc `root` (thư mục trong repo, `""` là gốc repo).
/// `modules` là nội dung `<root>/.terraform/modules/modules.json` nếu có.
pub fn read(root: &str, plan: &J, modules: Option<&J>, fs: &Fs<'_>) -> Out {
    let mut out = Out::default();
    let Some(top) = plan.get_path(&["configuration", "root_module"]) else {
        out.unread.push("plan không có `configuration.root_module`".into());
        return out;
    };
    // Thư mục của module tải về, theo khoá `a.b` của `modules.json`.
    let mut dirs: HashMap<String, String> = HashMap::new();
    for m in modules.and_then(|m| m.get("Modules")).and_then(J::as_array).unwrap_or(&[]) {
        if let (Some(k), Some(d)) = (m.get("Key").and_then(J::as_str), m.get("Dir").and_then(J::as_str)) {
            // Chỉ thư mục trong repo (TM-60).
            if let Some(d) = super::deploy::within(root, d) {
                dirs.insert(k.to_string(), d);
            }
        }
    }
    let mut cx = Cx { insts: Vec::new(), memo: HashMap::new(), busy: HashSet::new(), steps: 0, unread: Vec::new() };
    // Dựng cây module: (cấu hình, cha, lời gọi, tiền tố, khoá, thư mục).
    let mut todo: Vec<Pending<'_>> =
        vec![(top, None, None, String::new(), String::new(), Some(root.to_string()))];
    while let Some((config, parent, call, prefix, key, dir)) = todo.pop() {
        if cx.insts.len() >= MAX_MODULES {
            out.unread.push(format!("plan có quá {MAX_MODULES} module; phần sau không đọc"));
            break;
        }
        let i = cx.insts.len();
        let locals = dir.as_deref().map(|d| {
            let (l, lost) = locals_of(d, fs);
            out.unread.extend(lost);
            l
        });
        let name = if key.is_empty() { "gốc".to_string() } else { key.clone() };
        cx.insts.push(Inst { prefix: prefix.clone(), config, parent, call, children: HashMap::new(), locals, name });
        if let Some(p) = parent {
            let short = key.rsplit('.').next().unwrap_or(&key).to_string();
            cx.insts[p].children.insert(short, i);
        }
        for (name, mc) in config.get("module_calls").and_then(J::as_object).unwrap_or(&[]) {
            let Some(child) = mc.get("module") else { continue };
            let src = mc.get("source").and_then(J::as_str).unwrap_or("");
            let ckey = if key.is_empty() { name.clone() } else { format!("{key}.{name}") };
            let cdir = if src.starts_with("./") || src.starts_with("../") {
                dir.as_deref().and_then(|d| super::deploy::within(d, src))
            } else {
                dirs.get(&ckey).cloned()
            };
            todo.push((child, Some(i), Some(mc), format!("{prefix}module.{name}."), ckey, cdir));
        }
    }
    // Tài nguyên theo địa chỉ, chủ sở hữu, chỉ mục theo loại.
    let mut res: HashMap<String, (&J, usize)> = HashMap::new();
    let mut by_type: HashMap<String, Vec<String>> = HashMap::new();
    for (i, inst) in cx.insts.iter().enumerate() {
        for r in inst.config.get("resources").and_then(J::as_array).unwrap_or(&[]) {
            if r.get("mode").and_then(J::as_str) != Some("managed") {
                continue;
            }
            let (Some(t), Some(n)) = (r.get("type").and_then(J::as_str), r.get("name").and_then(J::as_str)) else { continue };
            let a = format!("{}{t}.{n}", inst.prefix);
            by_type.entry(type_of(&a)).or_default().push(a.clone());
            res.insert(a, (r, i));
        }
    }
    for v in by_type.values_mut() {
        v.sort();
    }
    let values = planned(plan);
    // Tài nguyên `count = 0` (không có bản nào trong plan) không được tạo.
    let made = |a: &str| values.contains_key(a);

    // `points(a, key, ty)`: đích loại `ty` mà thuộc tính `key` của `a` (hay của
    // khối lồng một tầng) tham chiếu tới.
    let points = |cx: &mut Cx<'_>, a: &str, key: &str, ty: &str| -> HashSet<String> {
        let Some(&(r, owner)) = res.get(a) else { return HashSet::new() };
        let mut refs = Vec::new();
        if let Some(e) = r.get("expressions") {
            if let Some(x) = e.get(key) {
                config_refs(x, &mut refs);
            }
            for (_, v) in e.as_object().unwrap_or(&[]) {
                for b in v.as_array().unwrap_or(&[]) {
                    if let Some(x) = b.get(key) {
                        config_refs(x, &mut refs);
                    }
                }
            }
        }
        let mut found = HashSet::new();
        for x in refs {
            for (t, _) in cx.resolve(owner, &x) {
                if type_of(&t) == ty {
                    found.insert(t);
                }
            }
        }
        found
    };
    let of = |ty: &str| by_type.get(ty).cloned().unwrap_or_default();

    // Service: ECS service cùng task definition, task definition đứng riêng,
    // Lambda, Cloud Run.
    let mut holder: Vec<(String, String)> = Vec::new();
    let mut used_td: HashSet<String> = HashSet::new();
    for a in of("aws_ecs_service").into_iter().filter(|a| made(a)) {
        for td in points(&mut cx, &a, "task_definition", "aws_ecs_task_definition") {
            used_td.insert(td.clone());
            holder.push((a.clone(), td));
        }
    }
    for ty in ["aws_lambda_function", "google_cloud_run_v2_service", "google_cloud_run_service", "aws_ecs_task_definition"] {
        for a in of(ty).into_iter().filter(|a| made(a)) {
            if ty != "aws_ecs_task_definition" || !used_td.contains(&a) {
                holder.push((a.clone(), a));
            }
        }
    }
    holder.sort();
    holder.dedup();
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    for (u, h) in &holder {
        names.entry(h.clone()).or_default().push(u.clone());
        names.entry(u.clone()).or_default().push(u.clone());
    }
    // Đứng trước service: bộ cân bằng tải, target group, listener, Cloud Map,
    // Lambda URL và alias.
    let listeners: Vec<String> = of("aws_lb_listener").into_iter().chain(of("aws_lb_listener_rule")).collect();
    let mut lbs_of_tg: HashMap<String, HashSet<String>> = HashMap::new();
    for l in &listeners {
        let tgs = points(&mut cx, l, "target_group_arn", "aws_lb_target_group");
        let mut lbs = points(&mut cx, l, "load_balancer_arn", "aws_lb");
        for parent in points(&mut cx, l, "listener_arn", "aws_lb_listener") {
            lbs.extend(points(&mut cx, &parent, "load_balancer_arn", "aws_lb"));
        }
        for tg in tgs {
            lbs_of_tg.entry(tg).or_default().extend(lbs.iter().cloned());
        }
    }
    let mut fronts: HashMap<String, HashSet<String>> = HashMap::new();
    let mut lambda_fronts: HashMap<String, Vec<String>> = HashMap::new();
    for ty in ["aws_lambda_function_url", "aws_lambda_alias"] {
        for a in of(ty) {
            for f in points(&mut cx, &a, "function_name", "aws_lambda_function") {
                lambda_fronts.entry(f).or_default().push(a.clone());
            }
        }
    }
    let units: Vec<String> = {
        let mut u: Vec<String> = holder.iter().map(|(u, _)| u.clone()).collect();
        u.dedup();
        u
    };
    for u in &units {
        match type_of(u).as_str() {
            "aws_ecs_service" => {
                for tg in points(&mut cx, u, "target_group_arn", "aws_lb_target_group") {
                    fronts.entry(tg.clone()).or_default().insert(u.clone());
                    for lb in lbs_of_tg.get(&tg).into_iter().flatten() {
                        fronts.entry(lb.clone()).or_default().insert(u.clone());
                    }
                }
                for sd in points(&mut cx, u, "registry_arn", "aws_service_discovery_service") {
                    fronts.entry(sd).or_default().insert(u.clone());
                }
            }
            "aws_lambda_function" => {
                for a in lambda_fronts.get(u).into_iter().flatten() {
                    fronts.entry(a.clone()).or_default().insert(u.clone());
                }
            }
            _ => {}
        }
    }
    // Lối vào Internet: tính một lần cho mọi service, đi qua mỗi tài nguyên
    // một lần (TM-59), như bộ đọc HCL.
    let val = |a: &str, k: &str| values.get(a).and_then(|v| v.get(k));
    let world = |r: &J, keys: &[&str]| {
        keys.iter().any(|k| match r.get(k) {
            Some(J::Str(c)) => c == "0.0.0.0/0" || c == "::/0",
            v => strs(v).iter().any(|c| *c == "0.0.0.0/0" || *c == "::/0"),
        })
    };
    let mut open_sg: HashSet<String> = HashSet::new();
    for sg in of("aws_security_group") {
        if val(&sg, "ingress").and_then(J::as_array).unwrap_or(&[]).iter().any(|r| world(r, &["cidr_blocks", "ipv6_cidr_blocks"])) {
            open_sg.insert(sg);
        }
    }
    for ty in ["aws_security_group_rule", "aws_vpc_security_group_ingress_rule"] {
        for rule in of(ty) {
            let Some(v) = values.get(&rule) else { continue };
            if ty == "aws_security_group_rule" && v.get("type").and_then(J::as_str).is_some_and(|t| t != "ingress") {
                continue;
            }
            if world(v, &["cidr_blocks", "ipv6_cidr_blocks", "cidr_ipv4", "cidr_ipv6"]) {
                open_sg.extend(points(&mut cx, &rule, "security_group_id", "aws_security_group"));
            }
        }
    }
    let mut open: HashSet<String> = HashSet::new();
    for u in units.iter().filter(|u| type_of(u) == "aws_ecs_service") {
        for tg in points(&mut cx, u, "target_group_arn", "aws_lb_target_group") {
            if lbs_of_tg.get(&tg).into_iter().flatten().any(|lb| matches!(val(lb, "internal"), Some(J::Bool(false)))) {
                open.insert(u.clone());
            }
        }
        let public = val(u, "network_configuration").and_then(J::as_array).unwrap_or(&[]).iter().any(|nc| is_true(nc.get("assign_public_ip")));
        if public && points(&mut cx, u, "security_groups", "aws_security_group").iter().any(|sg| open_sg.contains(sg)) {
            open.insert(u.clone());
        }
    }
    let mut alias_of: HashMap<String, HashSet<String>> = HashMap::new();
    for a in of("aws_lambda_alias") {
        alias_of.insert(a.clone(), points(&mut cx, &a, "function_name", "aws_lambda_function"));
    }
    for a in of("aws_lambda_function_url") {
        if val(&a, "authorization_type").and_then(J::as_str) != Some("NONE") {
            continue;
        }
        open.extend(points(&mut cx, &a, "function_name", "aws_lambda_function"));
        for al in points(&mut cx, &a, "function_name", "aws_lambda_alias") {
            open.extend(alias_of.get(&al).into_iter().flatten().cloned());
        }
    }
    for (ty, key) in [("aws_apigatewayv2_integration", "integration_uri"), ("aws_api_gateway_integration", "uri")] {
        for a in of(ty) {
            open.extend(points(&mut cx, &a, key, "aws_lambda_function"));
        }
    }
    for t in ["google_cloud_run_v2_service", "google_cloud_run_service"] {
        for u in of(t) {
            if is_true(val(&u, "invoker_iam_disabled")) {
                open.insert(u);
            }
        }
        for suffix in ["_iam_member", "_iam_binding", "_iam_policy"] {
            for a in of(&format!("{t}{suffix}")) {
                let mut svcs = points(&mut cx, &a, "name", t);
                svcs.extend(points(&mut cx, &a, "service", t));
                if suffix == "_iam_policy" {
                    // `policy_data` là JSON do `data "google_iam_policy"` dựng:
                    // không đọc, báo như bộ đọc HCL.
                    for s in &svcs {
                        cx.note(format!("`{a}` (IAM policy) chưa được đọc: không biết có cho `allUsers` gọi `{s}` không"));
                    }
                    continue;
                }
                let role = val(&a, "role").and_then(J::as_str);
                let mut members: Vec<&str> = val(&a, "member").and_then(J::as_str).into_iter().collect();
                members.extend(strs(val(&a, "members")));
                if role.is_none_or(|r| r == "roles/run.invoker") && members.iter().any(|m| *m == "allUsers" || *m == "allAuthenticatedUsers") {
                    open.extend(svcs);
                }
            }
        }
    }
    let mut targets: Vec<String> = Vec::new();
    for u in &units {
        let hs: Vec<&String> = holder.iter().filter(|(x, _)| x == u).map(|(_, h)| h).collect();
        let mut edges: Vec<(String, String)> = Vec::new();
        let mut image = None;
        for h in hs {
            let Some(&(r, owner)) = res.get(h) else { continue };
            for (attr, x) in env_exprs(r) {
                let mut refs = Vec::new();
                config_refs(x, &mut refs);
                for x in refs {
                    for (t, a) in cx.resolve(owner, &x) {
                        let shown = if a.is_empty() { format!("{attr} → {t}") } else { format!("{attr} → {t}.{a}") };
                        if let Some(svcs) = names.get(&t) {
                            edges.extend(svcs.iter().map(|s| (s.clone(), shown.clone())));
                        } else if let Some(svcs) = fronts.get(&t) {
                            edges.extend(svcs.iter().map(|s| (s.clone(), shown.clone())));
                        } else if super::terraform::is_network_attr(&a) || super::terraform::is_messaging(&type_of(&t)) {
                            edges.push((t.clone(), shown));
                            targets.push(t);
                        }
                    }
                }
            }
            image = image.or_else(|| image_of(h, &values));
        }
        edges.retain(|(t, _)| t != u);
        edges.sort();
        edges.dedup_by(|a, b| a.0 == b.0);
        let exposed = open.contains(u);
        out.units.push(Unit { addr: u.clone(), ty: type_of(u), compute: true, edges, exposed, image });
    }
    // Bên nhận của hàng đợi, topic: như bộ đọc HCL (event source mapping, đăng
    // ký SNS, push subscription của Pub/Sub).
    let mut from_queue: Vec<(String, String, String)> = Vec::new();
    for (ty, src_key, dst_key) in [
        ("aws_lambda_event_source_mapping", "event_source_arn", "function_name"),
        ("aws_sns_topic_subscription", "topic_arn", "endpoint"),
        ("google_pubsub_subscription", "topic", "push_endpoint"),
    ] {
        for a in of(ty).into_iter().filter(|a| made(a)) {
            let Some(&(r, owner)) = res.get(&a) else { continue };
            let Some(e) = r.get("expressions") else { continue };
            let (mut src_refs, mut dst_refs) = (Vec::new(), Vec::new());
            if let Some(x) = e.get(src_key) {
                config_refs(x, &mut src_refs);
            }
            if let Some(x) = e.get(dst_key) {
                config_refs(x, &mut dst_refs);
            }
            for b in e.get("push_config").and_then(J::as_array).unwrap_or(&[]) {
                if let Some(x) = b.get(dst_key) {
                    config_refs(x, &mut dst_refs);
                }
            }
            let srcs: Vec<String> = src_refs
                .iter()
                .flat_map(|x| cx.resolve(owner, x))
                .map(|(t, _)| t)
                .filter(|t| super::terraform::is_messaging(&type_of(t)))
                .collect();
            let dsts: Vec<String> = dst_refs.iter().flat_map(|x| cx.resolve(owner, x)).map(|(t, _)| t).collect();
            for src in &srcs {
                for t in &dsts {
                    let to: Vec<String> = if let Some(v) = names.get(t) {
                        v.clone()
                    } else if let Some(v) = fronts.get(t) {
                        v.iter().cloned().collect()
                    } else if super::terraform::is_messaging(&type_of(t)) {
                        vec![t.clone()]
                    } else {
                        Vec::new()
                    };
                    for x in to.into_iter().filter(|x| x != src) {
                        targets.push(src.clone());
                        if super::terraform::is_messaging(&type_of(&x)) {
                            targets.push(x.clone());
                        }
                        from_queue.push((src.clone(), x, format!("{ty}: {src} → {t}")));
                    }
                }
            }
        }
    }
    targets.sort();
    targets.dedup();
    for t in targets {
        if !units.contains(&t) {
            out.units.push(Unit { ty: type_of(&t), addr: t, compute: false, edges: Vec::new(), exposed: false, image: None });
        }
    }
    for (src, to, shown) in from_queue {
        if let Some(u) = out.units.iter_mut().find(|u| u.addr == src)
            && !u.edges.iter().any(|(t, _)| *t == to)
        {
            u.edges.push((to, shown));
        }
    }
    if cx.steps >= MAX_STEPS {
        out.unread.push(format!("plan cần quá {MAX_STEPS} bước giải tham chiếu; phần sau không đọc"));
    }
    out.unread.extend(cx.unread);
    out
}

/// Biểu thức chứa biến môi trường của tài nguyên chạy mã, kèm tên để hiển thị.
fn env_exprs(r: &J) -> Vec<(&'static str, &J)> {
    let Some(e) = r.get("expressions") else { return Vec::new() };
    let mut out: Vec<(&'static str, &J)> = Vec::new();
    match r.get("type").and_then(J::as_str).unwrap_or("") {
        "aws_ecs_task_definition" => out.extend(e.get("container_definitions").map(|x| ("container_definitions", x))),
        "aws_lambda_function" => {
            for b in e.get("environment").and_then(J::as_array).unwrap_or(&[]) {
                out.extend(b.get("variables").map(|x| ("environment.variables", x)));
            }
        }
        t @ ("google_cloud_run_v2_service" | "google_cloud_run_service") => {
            for tpl in e.get("template").and_then(J::as_array).unwrap_or(&[]) {
                let specs: Vec<&J> = if t == "google_cloud_run_service" {
                    tpl.get("spec").and_then(J::as_array).unwrap_or(&[]).iter().collect()
                } else {
                    vec![tpl]
                };
                for s in specs {
                    for c in s.get("containers").and_then(J::as_array).unwrap_or(&[]) {
                        for env in c.get("env").and_then(J::as_array).unwrap_or(&[]) {
                            out.extend(env.get("value").map(|x| ("env.value", x)));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

/// Giá trị đã biết của mỗi tài nguyên (bản đầu tiên), theo địa chỉ không chỉ số.
fn planned(plan: &J) -> HashMap<String, &J> {
    let mut out = HashMap::new();
    let mut todo: Vec<&J> = plan.get_path(&["planned_values", "root_module"]).into_iter().collect();
    while let Some(m) = todo.pop() {
        for r in m.get("resources").and_then(J::as_array).unwrap_or(&[]) {
            if let Some(a) = r.get("address").and_then(J::as_str) {
                out.entry(strip_index(a)).or_insert_with(|| r.get("values").unwrap_or(&J::Null));
            }
        }
        todo.extend(m.get("child_modules").and_then(J::as_array).unwrap_or(&[]));
    }
    out
}

fn is_true(v: Option<&J>) -> bool {
    matches!(v, Some(J::Bool(true)))
}

fn strs(v: Option<&J>) -> Vec<&str> {
    v.and_then(J::as_array).unwrap_or(&[]).iter().filter_map(J::as_str).collect()
}

fn image_of(h: &str, values: &HashMap<String, &J>) -> Option<String> {
    let v = values.get(h)?;
    fn first(x: Option<&J>) -> Option<&J> {
        x.and_then(J::as_array).and_then(|a| a.first())
    }
    match type_of(h).as_str() {
        "aws_ecs_task_definition" => {
            let cs = crate::jsonc::parse(v.get("container_definitions")?.as_str()?)?;
            first(Some(&cs))?.get("image")?.as_str().map(str::to_string)
        }
        "aws_lambda_function" => v.get("image_uri")?.as_str().map(str::to_string),
        "google_cloud_run_v2_service" => first(first(v.get("template"))?.get("containers"))?.get("image")?.as_str().map(str::to_string),
        "google_cloud_run_service" => {
            first(first(first(v.get("template"))?.get("spec"))?.get("containers"))?.get("image")?.as_str().map(str::to_string)
        }
        _ => None,
    }
}

/// Loại chạy mã.
pub fn is_compute(ty: &str) -> bool {
    COMPUTE.contains(&ty)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bo_chi_so_ca_trong_nhay() {
        assert_eq!(strip_index("module.a[0].aws_x.b[\"k]\"]"), "module.a.aws_x.b");
        assert_eq!(type_of("module.a.module.b.aws_alb.x"), "aws_lb");
        assert_eq!(type_of("data.aws_db_instance.x"), "aws_db_instance");
    }
}
