//! Dựng bản nháp kiến trúc từ mã nguồn có sẵn.
//!
//! Đây là lối vào cho codebase đã có. Không có nó, người dùng đứng trước hai
//! lựa chọn đều tệ: một file luật rỗng không ràng buộc gì, hoặc gõ tay cả
//! trăm dòng trước khi thấy được giá trị nào.
//!
//! Phải nói thẳng một điều, và đầu ra cũng nói thẳng: kết quả ở đây là **mô tả
//! hiện trạng**, không phải thiết kế. Nó chép lại cả những chỗ đang lộn xộn.
//! Vì vậy mọi thứ sinh ra đều mang `#draft` — chưa luật nào có hiệu lực cho
//! tới khi người đọc lại và nâng lên.

use crate::check;
use crate::resolve::{NativeImport, Resolved, Resolver, settle_natives};
use crate::scan::Lang;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

/// Thư mục không nói gì về kiến trúc của sản phẩm. Bỏ khi **chọn bố cục**;
/// chúng vẫn được kiểm bình thường sau đó.
const ASIDE: &[&str] = &[
    "test", "tests", "testing", "selftest", "selftests", "spec", "specs", "__tests__", "e2e", "example", "examples",
    "bench", "benches", "doc", "docs", "deps", "vendor", "third_party", "thirdparty",
    "fixtures", "testdata",
];

/// Tiền tố đào sâu tối đa. `src/flask` và `tokio/src` cần hai cấp.
const MAX_PREFIX_DEPTH: usize = 3;
/// Tiền tố phủ quá ít file thì không đáng xét, và xét hết mọi thư mục của một
/// repo lớn thì tốn.
const MIN_PREFIX_SHARE: usize = 8;
/// Dưới mức này thì lọc theo ngôn ngữ của manifest không còn gì để chia.
const MIN_NATIVE_FILES: usize = 8;

/// Quá nhiều component thì sơ đồ vô dụng và file luật không ai đọc. Phần dư
/// được liệt kê riêng để người tự quyết.
const MAX_COMPONENTS: usize = 24;

#[derive(Debug, Clone, Default)]
pub struct Discovery {
    /// Thư mục gốc mã nguồn, rỗng nghĩa là gốc repo.
    pub source_root: String,
    /// Tên component và mẫu `at` của nó.
    pub components: Vec<(String, String)>,
    /// Quan hệ phụ thuộc quan sát được trong mã.
    pub edges: Vec<(String, String)>,
    /// Lời gọi gRPC giữa hai component. Tách khỏi `edges`: phía server chạy ở
    /// process khác, nên cạnh này không kéo component vào `runs`.
    pub rpc_edges: Vec<(String, String)>,
    /// Cạnh qua hàng đợi (bên gửi → bên nhận cùng topic, queue, task). Như
    /// gRPC, không kéo component vào `runs`.
    pub queue_edges: Vec<(String, String)>,
    /// Cạnh qua bước pipeline (LangGraph, Airflow, `dvc.yaml`): bước trước
    /// đưa dữ liệu cho bước sau.
    pub pipeline_edges: Vec<(String, String)>,
    /// Thư mục không thành component vì đã quá số lượng, kèm số file.
    pub skipped: Vec<(String, usize)>,
    pub files: usize,
    pub by_lang: Vec<(Lang, usize)>,
    /// File không thuộc component nào. Bản nháp phủ được bao nhiêu phần repo
    /// là điều người đọc cần biết ngay, chứ không phải tự suy ra từ số
    /// component.
    pub unmapped: usize,
    /// Service đọc từ docker-compose.
    pub services: Vec<FoundService>,
    /// Cạnh mạng giữa các service, theo tên trong file luật.
    pub links: Vec<(String, Vec<String>)>,
    /// Ghi chú cho bản nháp: workload có ở nhiều thư mục, chart Helm không đọc.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FoundService {
    pub name: String,
    /// `compose:<tên>`, `compose:<thư mục>/<tên>`, `k8s:<tên>`, hay cả hai,
    /// cách nhau bằng dấu phẩy.
    pub source: String,
    pub runs: Vec<String>,
    /// Vì sao không có `runs`: không rõ điểm vào.
    pub note: Option<String>,
}

pub fn discover(root: &Path, ignores: &[String]) -> Discovery {
    let files = check::source_files(root, ignores);
    let mut out = Discovery { files: files.len(), ..Discovery::default() };
    if files.is_empty() {
        // Repo chỉ có hạ tầng (Terraform, manifest): vẫn sinh service và link.
        let deploy_files = check::deploy_files(root, ignores);
        if !deploy_files.is_empty() {
            let owners = Owners { prefix: "", inside: BTreeMap::new(), beside: Vec::new() };
            discover_services(root, &deploy_files, &owners, &[], &mut out);
        }
        return out;
    }

    out.by_lang = tally(files.iter().filter_map(|f| Lang::for_path(f)));
    out.source_root = pick_source_root(root, &files);
    let prefix = if out.source_root.is_empty() {
        String::new()
    } else {
        format!("{}/", out.source_root)
    };

    // Đọc mọi file có thể vào sơ đồ một lần: trong gốc mã nguồn, và ở các thư
    // mục bên cạnh nó.
    let scans = scan_files(root, &files, |f| {
        group_of(f, &prefix).is_some() || beside(f, &out.source_root).is_some()
    });

    // Mỗi thư mục con ngay dưới gốc mã nguồn là một component ứng viên. File
    // nằm thẳng trong gốc gom vào một component riêng. Thư mục bên cạnh gốc
    // cũng vậy, nếu một cạnh liên ngôn ngữ nối nó với bên trong.
    let mut ranked: Vec<(Group, usize)> =
        groups_under(&files, &prefix).into_iter().map(|(dir, n)| (Group::Inside(dir), n)).collect();
    for dir in partners(&scans, &prefix, &out.source_root) {
        let dir = descend(&files, dir);
        let n = files.iter().filter(|f| f.starts_with(&format!("{dir}/"))).count();
        ranked.push((Group::Beside(dir), n));
    }
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let (kept, dropped) = ranked.split_at(ranked.len().min(MAX_COMPONENTS));
    out.skipped = dropped.iter().map(|(g, n)| (g.path(&prefix), *n)).collect();

    let mut names: Vec<String> = Vec::new();
    let mut owners = Owners { prefix: &prefix, inside: BTreeMap::new(), beside: Vec::new() };
    for (group, _) in kept {
        let (base, glob) = match group {
            Group::Inside(dir) if dir == "." => (root_group_name(&prefix), root_glob(&files, &prefix)),
            Group::Inside(dir) => (pascal_case(dir), format!("{prefix}{dir}/**")),
            Group::Beside(dir) => {
                (pascal_case(dir.rsplit('/').next().unwrap_or(dir)), format!("{dir}/**"))
            }
        };
        let name = unique_name(&base, &names);
        names.push(name.clone());
        match group {
            Group::Inside(dir) => {
                owners.inside.insert(dir.clone(), name.clone());
            }
            Group::Beside(dir) => owners.beside.push((format!("{dir}/"), name.clone())),
        }
        out.components.push((name, glob));
    }
    out.components.sort();

    out.edges = observe_edges(root, &scans, &owners);
    out.rpc_edges = observe_rpc(root, ignores, &scans, &owners);
    out.queue_edges = observe_queues(&scans, &owners, &|_, _| Vec::new());
    out.pipeline_edges = observe_pipelines(root, ignores, &scans, &owners);
    out.unmapped = files.iter().filter(|f| owners.of_file(f).is_none()).count();
    let deploy_files = check::deploy_files(root, ignores);
    if !deploy_files.is_empty() {
        discover_services(root, &deploy_files, &owners, &scans, &mut out);
    }
    out
}

/// Service, `runs` và `link` từ docker-compose và manifest Kubernetes.
///
/// `runs` là mọi component mà điểm vào chạm tới theo các cạnh đã quan sát, tính
/// từ component chứa file điểm vào (hay, khi file đó không thuộc component nào,
/// từ những component nó import trực tiếp). Đúng điều mà `check` đòi, nên bản
/// nháp nâng lên `#accepted` thì không tự vi phạm.
///
/// Service compose và workload cùng tên, dựng từ cùng một Dockerfile (hay cùng
/// một image dựng sẵn), là một service với hai nguồn. Workload cùng tên ở
/// nhiều thư mục (`kubernetes-manifests/`, `release/`, base của kustomize) mà
/// dựng từ cùng một chỗ là bản sao của một ứng dụng: một nguồn `k8s:<tên>`
/// kiểm hết chúng. Dựng từ nhiều chỗ khác nhau (monorepo, hai ứng dụng cùng có
/// `api`) thì mỗi chỗ dựng một service, nguồn ghi rõ thư mục.
fn discover_services(root: &Path, deploy_files: &[String], owners: &Owners, scans: &[Scanned], out: &mut Discovery) {
    use crate::scan::deploy::{self, Key};
    let hosts: BTreeSet<&str> = scans.iter().flat_map(|s| s.hosts.iter().map(|h| h.host.name.as_str())).collect();
    // Biến môi trường mà mã đọc làm tên topic: giữ giá trị của chúng (TM-19).
    let env_words: Vec<String> = scans
        .iter()
        .flat_map(|s| s.queues.sites.iter())
        .filter_map(|q| match &q.name {
            crate::scan::queue::Name::Env(v) => Some(check::env_var(v).1),
            _ => None,
        })
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let opts = deploy::Options {
        k8s: true,
        tf: true,
        hosts: hosts.into_iter().map(str::to_string).collect(),
        env_words,
        ..Default::default()
    };
    let dep = deploy::load(root, deploy_files, &opts);
    let resolver = Resolver::new(root);

    let mut compose_count: BTreeMap<&str, usize> = BTreeMap::new();
    // Workload theo tên, giữ thứ tự gặp.
    let mut k8s_order: Vec<&str> = Vec::new();
    let mut k8s_by_name: HashMap<&str, Vec<&deploy::Svc>> = HashMap::new();
    for s in &dep.services {
        match &s.key {
            Key::K8s { name, .. } => {
                let list = k8s_by_name.entry(name).or_default();
                if list.is_empty() {
                    k8s_order.push(name);
                }
                list.push(s);
            }
            Key::Compose { name, .. } => *compose_count.entry(name).or_default() += 1,
            Key::Dockerfile(_) | Key::Tf { .. } => {}
        }
    }

    // Mỗi đơn vị là một `service` của bản nháp: một hay nhiều nguồn.
    struct Unit<'a> {
        base: String,
        sources: Vec<String>,
        svcs: Vec<&'a deploy::Svc>,
        k8s: bool,
    }
    let mut units: Vec<Unit<'_>> = Vec::new();
    for s in &dep.services {
        let Key::Compose { dir, name } = &s.key else { continue };
        // Tên trùng ở nhiều thư mục thì ghi rõ thư mục; `./` là gốc repo.
        let source = match (compose_count[name.as_str()] > 1, dir.is_empty()) {
            (false, _) => format!("compose:{name}"),
            (true, true) => format!("compose:./{name}"),
            (true, false) => format!("compose:{dir}/{name}"),
        };
        units.push(Unit { base: pascal_case(name), sources: vec![source], svcs: vec![s], k8s: false });
    }
    let image = |s: &deploy::Svc| s.image.as_deref().map(|i| crate::scan::k8s::image_name(i).0.to_string());
    // Nơi dựng: Dockerfile, không có thì tên image dựng sẵn.
    let identity = |s: &deploy::Svc| s.built.clone().or_else(|| image(s)).unwrap_or_default();
    // Bảng băm thay vì tìm tuần tự: hàng chục nghìn workload thì tìm tuần
    // tự cho từng cái là bậc hai.
    let mut by_compose: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, u) in units.iter().enumerate() {
        if let Key::Compose { name, .. } = &u.svcs[0].key {
            by_compose.entry(name.to_ascii_lowercase()).or_default().push(i);
        }
    }
    let shown = |d: &str| if d.is_empty() { ".".to_string() } else { d.to_string() };
    for name in k8s_order {
        let svcs = &k8s_by_name[name];
        let mut groups: Vec<(String, Vec<&deploy::Svc>)> = Vec::new();
        for s in svcs {
            let id = identity(s);
            match groups.iter_mut().find(|(g, _)| *g == id) {
                Some((_, list)) => list.push(s),
                None => groups.push((id, vec![s])),
            }
        }
        if groups.len() > 1 {
            let dirs: BTreeSet<String> =
                svcs.iter().filter_map(|s| match &s.key { Key::K8s { dir, .. } => Some(shown(dir)), _ => None }).collect();
            out.notes.push(format!(
                "workload `{name}` ở {} dựng từ {} nơi khác nhau: tách thành từng service",
                dirs.into_iter().collect::<Vec<_>>().join(", "),
                groups.len()
            ));
        }
        let split = groups.len() > 1;
        for (_, group) in groups {
            let sources: Vec<String> = if split {
                let dirs: BTreeSet<&str> =
                    group.iter().filter_map(|s| match &s.key { Key::K8s { dir, .. } => Some(dir.as_str()), _ => None }).collect();
                dirs.into_iter().map(|d| if d.is_empty() { format!("k8s:./{name}") } else { format!("k8s:{d}/{name}") }).collect()
            } else {
                vec![format!("k8s:{name}")]
            };
            let first = group[0];
            let twin = by_compose.get(&name.to_ascii_lowercase()).and_then(|list| {
                list.iter().copied().find(|&i| {
                    let u = &units[i];
                    !u.k8s
                        && u.svcs.len() == 1
                        && match (&u.svcs[0].built, &first.built) {
                            (Some(a), Some(b)) => a == b,
                            (None, None) => {
                                !u.svcs[0].code && !first.code && image(u.svcs[0]).is_some() && image(u.svcs[0]) == image(first)
                            }
                            _ => false,
                        }
                })
            });
            match twin {
                Some(i) => {
                    units[i].sources.extend(sources);
                    units[i].svcs.extend(group);
                    units[i].k8s = true;
                }
                None => units.push(Unit { base: pascal_case(name), sources, svcs: group, k8s: true }),
            }
        }
    }
    // Tài nguyên Terraform: cùng địa chỉ ở nhiều module gốc (`envs/prod`,
    // `envs/staging`) là bản sao của cùng một service, như workload cùng tên.
    let mut tf_order: Vec<&str> = Vec::new();
    let mut tf_by_addr: HashMap<&str, Vec<&deploy::Svc>> = HashMap::new();
    for s in &dep.services {
        if let Key::Tf { addr, .. } = &s.key {
            let list = tf_by_addr.entry(addr).or_default();
            if list.is_empty() {
                tf_order.push(addr);
            }
            list.push(s);
        }
    }
    let tf_start = units.len();
    for addr in tf_order {
        let name = addr.rsplit('.').next().unwrap_or(addr);
        units.push(Unit { base: pascal_case(name), sources: vec![format!("tf:{addr}")], svcs: tf_by_addr[addr].clone(), k8s: false });
    }
    let mut taken: HashSet<String> = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    for (ui, u) in units.iter().enumerate() {
        // Tài nguyên trùng tên ngắn với service khác: `ApiTf`.
        if ui >= tf_start {
            let base = if taken.contains(&u.base) { format!("{}Tf", u.base) } else { u.base.clone() };
            let dsl = if taken.contains(&base) {
                (2..).map(|n| format!("{base}{n}")).find(|c| !taken.contains(c)).unwrap_or_default()
            } else {
                base
            };
            taken.insert(dsl.clone());
            names.push(dsl);
            continue;
        }
        // Workload không gộp được với service compose cùng tên: `VoteK8s`.
        let only_k8s = u.k8s && u.svcs.iter().all(|v| matches!(v.key, Key::K8s { .. }));
        let base = if only_k8s && taken.contains(&u.base) { format!("{}K8s", u.base) } else { u.base.clone() };
        let dsl = if taken.contains(&base) {
            (2..).map(|n| format!("{base}{n}")).find(|c| !taken.contains(c)).unwrap_or_default()
        } else {
            base
        };
        taken.insert(dsl.clone());
        names.push(dsl);
    }
    let by_key: HashMap<&Key, &str> =
        units.iter().zip(&names).flat_map(|(u, n)| u.svcs.iter().map(move |v| (&v.key, n.as_str()))).collect();

    let mut links: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (u, dsl) in units.iter().zip(&names) {
        let mut seeds: BTreeSet<String> = BTreeSet::new();
        let mut lost: Option<String> = None;
        for svc in &u.svcs {
            match entry_seeds(root, &resolver, owners, svc) {
                Ok(found) => seeds.extend(found),
                Err(why) => {
                    lost.get_or_insert(why);
                }
            }
        }
        // Lệnh nào không lần ra mã thì `check` báo không rõ điểm vào; sinh
        // `runs` cho service đó là sinh ra một luật tự cảnh báo.
        let runs = if lost.is_some() { Vec::new() } else { closure(&seeds, &out.edges) };
        let code = u.svcs.iter().any(|v| v.code);
        let note = (runs.is_empty() && code).then(|| {
            lost.clone()
                .or_else(|| u.svcs.iter().find_map(|v| v.unknown.clone()))
                .unwrap_or_else(|| "điểm vào không thuộc component nào".into())
        });
        out.services.push(FoundService { name: dsl.clone(), source: u.sources.join(", "), runs, note });
        for svc in &u.svcs {
            for e in &svc.edges {
                if let Some(to) = by_key.get(&e.target)
                    && *to != dsl
                {
                    links.entry(dsl.clone()).or_default().insert((*to).to_string());
                }
            }
            // Lộ ra ngoài: `link Internet -> …`.
            if !svc.exposed.is_empty() {
                links.entry(crate::model::INTERNET.to_string()).or_default().insert(dsl.clone());
            }
            // Đường NetworkPolicy cho phép rõ: `check` đòi `link` cho chúng.
            for (guard, inbound) in [(&svc.ingress, true), (&svc.egress, false)] {
                for g in guard.grants.iter().flat_map(|g| g.iter()) {
                    let deploy::Grant::Pods { keys, .. } = g else { continue };
                    for k in keys.iter() {
                        let Some(other) = by_key.get(k) else { continue };
                        if *other == dsl {
                            continue;
                        }
                        let (from, to) = if inbound { ((*other).to_string(), dsl.clone()) } else { (dsl.clone(), (*other).to_string()) };
                        links.entry(from).or_default().insert(to);
                    }
                }
            }
        }
    }
    // Cạnh qua hàng đợi, giờ lần được cả tên lấy từ env, như `check`: giá trị
    // ở các service chạy component đó; không service nào chạy nó thì chỉ khi
    // cả cấu hình có đúng một giá trị.
    let svcs_of: HashMap<&str, &Vec<&deploy::Svc>> = units.iter().zip(&names).map(|(u, n)| (n.as_str(), &u.svcs)).collect();
    let words = |svc: &deploy::Svc, var: &str| svc.words.iter().filter(|(k, _)| k == var).map(|(_, w)| w.clone()).collect::<Vec<_>>();
    let resolve = |from: &str, var: &str| -> Vec<String> {
        let runners: Vec<&FoundService> = out.services.iter().filter(|s| s.runs.iter().any(|r| r == from)).collect();
        let mut vals: Vec<String> = if runners.is_empty() {
            let all: BTreeSet<String> = dep.services.iter().flat_map(|v| words(v, var)).collect();
            if all.len() == 1 { all.into_iter().collect() } else { Vec::new() }
        } else {
            runners
                .iter()
                .filter_map(|r| svcs_of.get(r.name.as_str()))
                .flat_map(|list| list.iter().flat_map(|v| words(v, var)))
                .collect()
        };
        vals.sort();
        vals.dedup();
        vals
    };
    out.queue_edges = observe_queues(scans, owners, &resolve);
    // Lời gọi gRPC giữa hai service, theo đúng điều `check` đòi: với mỗi
    // service chạy phía client, nếu không service nào chạy phía server là
    // chính nó thì cần `link` tới một service chạy phía server.
    for (client, server) in out.rpc_edges.iter().chain(&out.queue_edges) {
        let running = |c: &str| out.services.iter().filter(|s| s.runs.iter().any(|r| r == c)).map(|s| s.name.clone()).collect::<Vec<_>>();
        let targets = running(server);
        let Some(first) = targets.first() else { continue };
        for s in running(client) {
            if !targets.contains(&s) && !links.get(&s).is_some_and(|t| targets.iter().any(|x| t.contains(x))) {
                links.entry(s).or_default().insert(first.clone());
            }
        }
    }
    // Tên máy viết thẳng trong mã: cạnh từ mọi service chạy component chứa
    // file, như `check` đòi.
    let mut placed: HashMap<&str, Vec<&Key>> = HashMap::new();
    for (k, name) in &by_key {
        placed.entry(name).or_default().push(k);
    }
    for s in scans.iter().filter(|s| !s.hosts.is_empty()) {
        let Some(c) = owners.of_file(&s.file) else { continue };
        let runners: Vec<String> =
            out.services.iter().filter(|x| x.runs.iter().any(|r| r == c)).map(|x| x.name.clone()).collect();
        for h in &s.hosts {
            let Some(keys) = dep.hosts.get(&h.host.name) else { continue };
            for r in &runners {
                for k in deploy::reachable(placed.get(r.as_str()).map_or(&[][..], Vec::as_slice), keys) {
                    if let Some(to) = by_key.get(k)
                        && *to != r.as_str()
                    {
                        links.entry(r.clone()).or_default().insert((*to).to_string());
                    }
                }
            }
        }
    }
    out.links = links.into_iter().map(|(from, to)| (from, to.into_iter().collect())).collect();
}

/// Component mà các lệnh khởi chạy của một service chạm tới trực tiếp. Lệnh
/// nào không lần ra mã thì trả lý do.
fn entry_seeds(
    root: &Path,
    resolver: &Resolver,
    owners: &Owners,
    svc: &crate::scan::deploy::Svc,
) -> Result<BTreeSet<String>, String> {
    let lost = svc.starts.iter().find_map(|st| {
        if let Some(why) = st.cut.first() {
            return Some(format!("không đọc hết lệnh khởi chạy: {why}"));
        }
        let missing = st.unresolved.first().cloned().or_else(|| {
            st.entries
                .iter()
                .find(|e| {
                    let files = check::entry_files(root, resolver, std::slice::from_ref(e));
                    // File không biết ngôn ngữ và không thuộc component nào:
                    // `check` báo không rõ điểm vào.
                    files.is_empty()
                        || files.iter().any(|f| {
                            Lang::for_path(f).is_none()
                                || (owners.of_file(f).is_none() && check::direct_targets(root, resolver, f).is_err())
                        })
                })
                .map(|_| st.shown.clone())
        });
        missing.map(|m| format!("không tìm thấy trong repo mã mà `{m}` nạp"))
    });
    if let Some(why) = lost {
        return Err(why);
    }
    let mut seeds: BTreeSet<String> = BTreeSet::new();
    for st in &svc.starts {
        for file in check::entry_files(root, resolver, &st.entries) {
            match owners.of_file(&file) {
                Some(c) => {
                    seeds.insert(c.clone());
                }
                None => {
                    for (target, _, _) in check::direct_targets(root, resolver, &file).unwrap_or_default() {
                        if let Some(c) = owners.of_target(root, &target) {
                            seeds.insert(c.clone());
                        }
                    }
                }
            }
        }
    }
    Ok(seeds)
}

/// Mọi component đi tới được từ `seeds` theo `edges`, kể cả chính chúng.
fn closure(seeds: &BTreeSet<String>, edges: &[(String, String)]) -> Vec<String> {
    let mut seen: BTreeSet<String> = seeds.clone();
    let mut stack: Vec<String> = seeds.iter().cloned().collect();
    while let Some(c) = stack.pop() {
        for (from, to) in edges {
            if *from == c && seen.insert(to.clone()) {
                stack.push(to.clone());
            }
        }
    }
    seen.into_iter().collect()
}

/// Một component ứng viên: thư mục con của gốc mã nguồn (tương đối với gốc,
/// `.` là file nằm thẳng trong gốc), hoặc thư mục bên cạnh gốc (tương đối với
/// gốc repo).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Group {
    Inside(String),
    Beside(String),
}

impl Group {
    /// Đường dẫn tính từ gốc repo, để in ra: nhóm trong gốc và thư mục bên
    /// cạnh lẫn trong cùng một danh sách thì phải cùng một hệ quy chiếu.
    fn path(&self, prefix: &str) -> String {
        match self {
            Group::Inside(d) if d == "." => format!("{prefix}*"),
            Group::Inside(d) => format!("{prefix}{d}"),
            Group::Beside(d) => d.clone(),
        }
    }
}

/// File nào thuộc component nào trong bản nháp.
struct Owners<'a> {
    prefix: &'a str,
    /// Thư mục con của gốc mã nguồn → tên component.
    inside: BTreeMap<String, String>,
    /// Thư mục bên cạnh gốc, có `/` cuối → tên component.
    beside: Vec<(String, String)>,
}

impl Owners<'_> {
    fn of_file(&self, path: &str) -> Option<&String> {
        group_of(path, self.prefix)
            .and_then(|g| self.inside.get(&g))
            .or_else(|| self.beside.iter().find(|(d, _)| path.starts_with(d.as_str())).map(|(_, n)| n))
    }

    /// Như [`Self::of_file`], nhưng đích có thể là cả một thư mục — xem
    /// [`group_of_target`].
    fn of_target(&self, root: &Path, target: &str) -> Option<&String> {
        group_of_target(root, target, self.prefix).and_then(|g| self.inside.get(&g)).or_else(|| {
            self.beside
                .iter()
                .find(|(d, _)| target.starts_with(d.as_str()) || target == d.trim_end_matches('/'))
                .map(|(_, n)| n)
        })
    }
}

/// Những gì một file nói về phụ thuộc của nó, đã phân giải: đọc một lần, dùng
/// cho cả việc chọn component lẫn việc dựng cạnh.
struct Scanned {
    file: String,
    /// File hay thư mục mà import trỏ tới: module, và những tên trong `from
    /// pkg import a` mà chính chúng là module con.
    targets: Vec<String>,
    /// Khoá ghép qua ranh giới ngôn ngữ — xem `scan/foreign.rs`.
    calls: Vec<String>,
    provides: Vec<String>,
    /// Import có thể đang nạp module mở rộng; cái nào thành lời gọi thì tuỳ
    /// những file khác — xem [`crate::resolve::settle_natives`].
    natives: Vec<NativeImport>,
    /// Chỗ gọi stub và chỗ đăng ký server gRPC.
    rpc: crate::scan::grpc::Rpc,
    /// Chỗ gửi, nhận qua hàng đợi.
    queues: crate::scan::queue::Queues,
    pipeline: crate::scan::pipeline::Graph,
    /// Tên máy viết thẳng trong chuỗi; rỗng với file test, ví dụ.
    hosts: Vec<crate::scan::hosts::CodeHost>,
}

impl Scanned {
    /// Lời gọi qua ranh giới ngôn ngữ, gồm cả module mở rộng được ghép theo
    /// `settled`.
    fn crossings<'a>(&'a self, settled: &'a HashSet<(&str, &str)>) -> impl Iterator<Item = String> + 'a {
        let natives = self
            .natives
            .iter()
            .filter(|n| settled.contains(&(n.module.as_str(), n.at.as_str())))
            .map(|n| format!("py:{}", n.module));
        self.calls.iter().cloned().chain(natives)
    }
}

fn scan_files(root: &Path, files: &[String], wanted: impl Fn(&str) -> bool) -> Vec<Scanned> {
    let resolver = Resolver::new(root);
    let mut out = Vec::new();
    for file in files {
        let Some(lang) = Lang::for_path(file) else { continue };
        if !wanted(file) {
            continue;
        }
        let Ok(bytes) = std::fs::read(root.join(file)) else { continue };
        let src = String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        if src.len() as u64 > crate::check::MAX_FILE_BYTES {
            continue;
        }
        let src = crate::scan::source_view(file, &src);
        let scan = lang.scan_all(&src, &Default::default(), &[]);
        let mut s = Scanned {
            file: file.clone(),
            targets: Vec::new(),
            calls: scan.foreign.calls.into_iter().map(|c| c.symbol).collect(),
            provides: scan
                .foreign
                .provides
                .into_iter()
                .filter_map(|c| resolver.bind_extension(file, &c.symbol))
                .collect(),
            natives: Vec::new(),
            rpc: scan.rpc,
            queues: scan.queues,
            pipeline: scan.pipeline,
            hosts: if crate::scan::hosts::skipped_path(file) { Vec::new() } else { scan.hosts.hosts },
        };
        for import in &scan.imports {
            // Cùng các đích như `check` thấy.
            let base = resolver.resolve(lang, file, import);
            let (targets, natives) = resolver.import_targets(lang, file, import, &base);
            s.targets.extend(targets);
            if let Resolved::Path(target) = base {
                s.targets.push(target);
            }
            s.natives.extend(natives);
        }
        // Java: lớp dùng không qua `import`, như `check` (TM-71).
        for r in &scan.refs {
            if let Some(spec) = resolver.java_ref(file, r)
                && let Resolved::Path(target) = resolver.resolve(lang, file, &r.import(&spec))
            {
                s.targets.push(target);
            }
        }
        out.push(s);
    }
    out
}

/// Thư mục bên cạnh gốc mã nguồn chứa `file`: phần đường dẫn chung với gốc,
/// cộng một cấp. `src/` của pydantic-core thì `python/`; `vllm/` thì `csrc/`.
///
/// `None` khi file nằm trong gốc, nằm thẳng ở chỗ rẽ nhánh, hoặc thư mục đó là
/// test, ví dụ, tài liệu hay vendor. Đó là lý do việc này không lặp lại cách
/// "gộp anh em của gốc" đã bị loại: nó kéo `tests/` vào sơ đồ ở mọi repo.
fn beside(file: &str, source_root: &str) -> Option<String> {
    if source_root.is_empty() {
        return None;
    }
    let root: Vec<&str> = source_root.split('/').collect();
    let parts: Vec<&str> = file.split('/').collect();
    let common = root.iter().zip(&parts).take_while(|(a, b)| a == b).count();
    if common == root.len() || parts.len() <= common + 1 || ASIDE.contains(&parts[common]) {
        return None;
    }
    Some(parts[..=common].join("/"))
}

/// Thư mục bên cạnh gốc mã nguồn mà một cạnh liên ngôn ngữ nối với bên trong:
/// Python trong `vllm/` gọi op của `csrc/`, gói Python trong `python/` import
/// module mà Rust trong `src/` định nghĩa.
///
/// Chỉ cạnh liên ngôn ngữ, không phải import thường. Import thường nối hai
/// thư mục cùng ngôn ngữ, và nếu chúng quan trọng thì gốc mã nguồn đã bao
/// cả hai. Cạnh liên ngôn ngữ thì không có cách nào khác để thấy: không câu
/// import nào nói rằng `torch.ops._C.x` chạy mã trong `csrc/`.
fn partners(scans: &[Scanned], prefix: &str, source_root: &str) -> BTreeSet<String> {
    // `true` là nhóm trong gốc, `false` là thư mục bên cạnh.
    let unit = |file: &str| match group_of(file, prefix) {
        Some(group) => Some((true, group)),
        None => beside(file, source_root).map(|dir| (false, dir)),
    };
    // Hai đơn vị cùng cung cấp một tên thì không biết chọn bên nào, như `check`.
    let mut provides: BTreeMap<&str, Option<(bool, String)>> = BTreeMap::new();
    for s in scans {
        let Some(here) = unit(&s.file) else { continue };
        for key in &s.provides {
            provides
                .entry(key)
                .and_modify(|owner| {
                    if owner.as_ref() != Some(&here) {
                        *owner = None;
                    }
                })
                .or_insert_with(|| Some(here.clone()));
        }
    }
    let settled = settle_natives(scans.iter().flat_map(|s| &s.natives));
    let mut out = BTreeSet::new();
    for s in scans {
        let Some(caller) = unit(&s.file) else { continue };
        for key in s.crossings(&settled) {
            let Some(Some(provider)) = provides.get(key.as_str()) else { continue };
            match (&caller, provider) {
                ((true, _), (false, dir)) | ((false, dir), (true, _)) => {
                    out.insert(dir.clone());
                }
                _ => {}
            }
        }
    }
    out
}

/// Đi xuống khi thư mục chỉ bọc đúng một thư mục con chứa mã của nó:
/// `python/` của pydantic-core chỉ có `pydantic_core/`, và tên đó mới nói
/// lên component là gì. Thư mục test đứng cạnh không tính — `python/tests/`
/// không làm `python/` thành một component tên `Python`.
fn descend(files: &[String], mut dir: String) -> String {
    loop {
        let inner = format!("{dir}/");
        let mut children = BTreeSet::new();
        for f in files.iter().filter_map(|f| f.strip_prefix(inner.as_str())) {
            match f.split_once('/') {
                Some((child, _)) if ASIDE.contains(&child) => {}
                Some((child, _)) => {
                    children.insert(child);
                }
                None => return dir, // có file nằm thẳng trong đó
            }
        }
        match children.into_iter().collect::<Vec<_>>().as_slice() {
            [only] => dir = format!("{dir}/{only}"),
            _ => return dir,
        }
    }
}

/// Quan hệ phụ thuộc thật trong mã. Chỉ quan hệ giữa hai component, bỏ qua
/// thư viện bên ngoài — liệt kê mọi gói npm hay header hệ thống sẽ làm bản
/// nháp dài tới mức không ai đọc.
fn observe_edges(root: &Path, scans: &[Scanned], owners: &Owners) -> Vec<(String, String)> {
    let mut edges: Vec<(String, String)> = Vec::new();
    // Cạnh liên ngôn ngữ, ghép theo tên đúng như `check` làm. Thiếu phần này
    // thì lời gọi cgo của libbpfgo sang thư mục C khác là cạnh mà bộ kiểm thấy
    // còn luật sinh ra không có, và luật tự vi phạm chính nó.
    let mut ffi_calls: Vec<(&String, String)> = Vec::new();
    let mut ffi_provides: BTreeMap<String, String> = BTreeMap::new();
    // Chọn module mở rộng trên đúng tập file mà `check` thấy — file thuộc một
    // component — không thì hai bên ghép khác nhau và luật tự vi phạm.
    let owned: Vec<&Scanned> = scans.iter().filter(|s| owners.of_file(&s.file).is_some()).collect();
    let settled = settle_natives(owned.iter().flat_map(|s| &s.natives));

    for s in owned {
        let Some(from) = owners.of_file(&s.file) else { continue };
        ffi_calls.extend(s.crossings(&settled).map(|key| (from, key)));
        for key in &s.provides {
            // Hai component cùng cung cấp một tên thì bỏ cả hai, như `check`.
            ffi_provides
                .entry(key.clone())
                .and_modify(|owner| {
                    if owner != from {
                        owner.clear();
                    }
                })
                .or_insert_with(|| from.clone());
        }
        for target in &s.targets {
            if let Some(to) = owners.of_target(root, target)
                && to != from
            {
                edges.push((from.clone(), to.clone()));
            }
        }
    }
    // Thư viện dùng chung mà build dựng, cho `ctypes`/`cffi`, như `check`.
    if ffi_calls.iter().any(|(_, k)| k.starts_with("lib:")) {
        for (key, at) in crate::cbuild::shared_libraries(root) {
            let Some(owner) = owners.of_file(&at) else { continue };
            ffi_provides
                .entry(key)
                .and_modify(|o| {
                    if o != owner {
                        o.clear();
                    }
                })
                .or_insert_with(|| owner.clone());
        }
    }
    for (from, key) in ffi_calls {
        if let Some(to) = crate::scan::foreign::provider(&ffi_provides, &key).filter(|o| *o != from) {
            edges.push((from.clone(), to.clone()));
        }
    }
    edges.sort();
    edges.dedup();
    edges
}

/// Lời gọi gRPC giữa hai component, ghép như `check`: theo `package.Service`
/// đọc từ `.proto`, mọi component đăng ký server cho service đó, bỏ tên nhập
/// nhằng, bỏ lời gọi tới service không có server trong repo.
fn observe_rpc(root: &Path, ignores: &[String], scans: &[Scanned], owners: &Owners) -> Vec<(String, String)> {
    use crate::scan::grpc::{Key, ProtoIndex, read_proto};
    if scans.iter().all(|s| s.rpc.is_empty()) {
        return Vec::new();
    }
    let mut index = ProtoIndex::default();
    for p in check::proto_files(root, ignores) {
        if let Ok(text) = crate::scan::deploy::read_in(root, &p, check::MAX_FILE_BYTES) {
            index.add(&p, &read_proto(&text));
        }
    }
    let mut servers: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let real = |s: &&Scanned| !crate::scan::grpc::generated_stub_path(&s.file);
    for s in scans.iter().filter(real).filter(|s| !crate::scan::grpc::fake_server_path(&s.file)) {
        let Some(from) = owners.of_file(&s.file) else { continue };
        for site in &s.rpc.serves {
            if let Key::Found(key) = index.key(site) {
                servers.entry(key).or_default().insert(from.clone());
            }
        }
    }
    let mut edges = Vec::new();
    for s in scans.iter().filter(real) {
        let Some(from) = owners.of_file(&s.file) else { continue };
        for site in &s.rpc.calls {
            let Key::Found(key) = index.key(site) else { continue };
            for to in servers.get(&key).into_iter().flatten().filter(|t| *t != from) {
                edges.push((from.clone(), to.clone()));
            }
        }
    }
    edges.sort();
    edges.dedup();
    edges
}

/// Cạnh qua hàng đợi: bên gửi và bên nhận cùng hệ và tên cố định, hay tên
/// lấy từ env mà `env` (component, biến) lần ra như `check`. Tên không cố
/// định thì `init` không đoán; `check` báo chúng.
fn observe_queues(scans: &[Scanned], owners: &Owners, env: &dyn Fn(&str, &str) -> Vec<String>) -> Vec<(String, String)> {
    use crate::scan::queue::{Index, Name, Role, Site};
    if scans.iter().all(|s| s.queues.is_empty()) {
        return Vec::new();
    }
    // Chỗ gửi, nhận với tên env đã thay bằng từng giá trị lần ra.
    let expand = |site: &Site, from: &str| -> Vec<Site> {
        let Name::Env(raw) = &site.name else { return vec![site.clone()] };
        let (prefix, var) = check::env_var(raw);
        env(from, &var)
            .into_iter()
            .map(|v| Site { name: Name::Fixed(format!("{prefix}{v}")), ..site.clone() })
            .collect()
    };
    let mut index = Index::default();
    for s in scans.iter().filter(|s| !crate::scan::grpc::fake_server_path(&s.file)) {
        let Some(from) = owners.of_file(&s.file) else { continue };
        for site in s.queues.sites.iter().filter(|x| x.role == Role::Receive) {
            for site in expand(site, from) {
                index.add(&site, &s.file, from);
            }
        }
    }
    let mut edges = Vec::new();
    for s in scans {
        let Some(from) = owners.of_file(&s.file) else { continue };
        for site in s.queues.sites.iter().filter(|x| x.role == Role::Send).flat_map(|x| expand(x, from)) {
            let Name::Fixed(name) = &site.name else { continue };
            for to in index.targets(site.system, name).into_iter().filter(|t| *t != *from) {
                edges.push((from.clone(), to));
            }
        }
    }
    edges.sort();
    edges.dedup();
    edges
}

/// Cạnh qua bước pipeline, như `check` thấy.
fn observe_pipelines(root: &Path, ignores: &[String], scans: &[Scanned], owners: &Owners) -> Vec<(String, String)> {
    let graphs: Vec<(String, crate::scan::pipeline::Graph)> =
        scans.iter().filter(|s| !s.pipeline.is_empty()).map(|s| (s.file.clone(), s.pipeline.clone())).collect();
    let dvc = check::pipeline_files(root, ignores);
    if graphs.is_empty() && dvc.is_empty() {
        return Vec::new();
    }
    let resolver = Resolver::new(root);
    let comp = |f: &str| owners.of_file(f).cloned();
    let mut scratch = check::Report::default();
    let mut edges: Vec<(String, String)> = check::pipeline_hops(root, &resolver, graphs, &dvc, &comp, &mut scratch)
        .into_iter()
        .map(|h| (h.from, h.to))
        .collect();
    edges.sort();
    edges.dedup();
    edges
}

/// Thư mục con chứa file này, tính từ gốc mã nguồn. `"."` nghĩa là nằm thẳng
/// trong gốc.
fn group_of(path: &str, prefix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    Some(rest.split_once('/').map_or_else(|| ".".to_string(), |(dir, _)| dir.to_string()))
}

/// Nhóm mà một **đích phân giải** thuộc về.
///
/// Khác `group_of` ở chỗ đích có thể là cả một thư mục: Go phân giải import
/// thành package, và gói không gian tên của Python cũng vậy. Coi `helpers` là
/// "file nằm thẳng ở gốc" sẽ gom mọi package cấp một của Go vào chung một
/// component, và khi đó mọi cạnh giữa chúng biến mất.
fn group_of_target(root: &Path, path: &str, prefix: &str) -> Option<String> {
    let rest = path.strip_prefix(prefix)?;
    match rest.split_once('/') {
        Some((dir, _)) => Some(dir.to_string()),
        None if !rest.is_empty() && root.join(path).is_dir() => Some(rest.to_string()),
        None => Some(".".to_string()),
    }
}

/// Mẫu `at` cho nhóm file nằm thẳng trong gốc.
///
/// `*` thì hỏng: nó khớp một đoạn bất kỳ, nên nó nhận luôn mọi **thư mục** con
/// chứ không riêng file. Go và Python phân giải import thành cả thư mục, nên
/// một package không thành component sẽ bị mẫu này nhận bừa — và khi đó bộ
/// khám phá với bộ kiểm chứng nói hai chuyện khác nhau về cùng một import.
///
/// Liệt kê đúng những phần mở rộng thật sự có ở đó thì mẫu chỉ còn khớp file.
fn root_glob(files: &[String], prefix: &str) -> String {
    let mut exts: Vec<String> = files
        .iter()
        .filter_map(|f| f.strip_prefix(prefix))
        .filter(|rest| !rest.contains('/'))
        .filter_map(|rest| rest.rsplit_once('.').map(|(_, ext)| ext.to_string()))
        .collect();
    exts.sort();
    exts.dedup();
    if exts.is_empty() {
        return format!("{prefix}*");
    }
    exts.iter().map(|e| format!("{prefix}*.{e}")).collect::<Vec<_>>().join(", ")
}

/// Đoạn đường dẫn chỉ nói "đây là mã nguồn", không nói component nào.
const GENERIC_DIRS: &[&str] = &["src", "lib", "source", "crates", "packages", "internal", "pkg"];

/// Tên cho nhóm file nằm thẳng trong gốc.
///
/// `pascal_case(".")` ra chuỗi rỗng rồi rơi vào nhánh dự phòng thành `M` —
/// một cái tên không nói gì với người đọc sơ đồ. Lấy tên từ chính gốc mã
/// nguồn thì có nghĩa hơn: `src/flask` ra `Flask`, `tokio/src` ra `Tokio`.
fn root_group_name(prefix: &str) -> String {
    let name = prefix
        .trim_end_matches('/')
        .split('/')
        .rev()
        .find(|part| !part.is_empty() && !GENERIC_DIRS.contains(part))
        .unwrap_or("");
    if name.is_empty() { "Core".to_string() } else { pascal_case(name) }
}

/// Gốc mã nguồn là thư mục đầu chứa phần lớn file. Không có thư mục nào chiếm
/// ưu thế rõ rệt thì dùng luôn gốc repo — đoán sai còn tệ hơn không đoán.
/// Gốc mã nguồn: thư mục mà chia theo con của nó thì lộ ra nhiều cấu trúc
/// nhất.
///
/// Trước đây chỗ này chọn theo tên quy ước (`src`, `lib`, `internal`…) và tên
/// quy ước thắng vô điều kiện. Hugo có `internal/` với 28 file trên tổng 943,
/// nên nó thắng và 915 file còn lại nằm ngoài sơ đồ. Flask có đúng một thư mục
/// dưới `src/`, nên bản nháp ra **một** component và không luật nào cả.
///
/// Thước đo thay thế là **số file không nằm trong nhóm lớn nhất**. Nó phạt cả
/// hai kiểu hỏng cùng lúc: gốc quá sâu thì phủ ít, còn gốc quá nông thì một
/// nhóm nuốt gần hết và cũng chẳng nói lên gì. `crates/` của ripgrep thắng
/// gốc repo vì chia được thành mười một; gốc repo của hugo thắng `tpl/` vì
/// `tpl` chỉ là một phần.
fn pick_source_root(root: &Path, files: &[String]) -> String {
    let main = architectural(root, files);
    let floor = main.len() / MIN_PREFIX_SHARE;

    let mut best: Option<(usize, usize, String)> = None;
    for prefix in candidates(root, &main) {
        let groups = groups_under(&main, &prefix);
        if groups.len() < 2 {
            continue;
        }
        let covered: usize = groups.values().sum();
        if covered < floor {
            continue;
        }
        let biggest = groups.values().copied().max().unwrap_or(0);
        // Hoà thì gốc nông thắng, rồi tới tên nhỏ nhất: kết quả không được
        // phụ thuộc thứ tự duyệt thư mục.
        let score = (covered - biggest, covered, std::cmp::Reverse(prefix.clone()));
        if best.as_ref().is_none_or(|b| {
            (score.0, score.1, &score.2) > (b.0, b.1, &std::cmp::Reverse(b.2.clone()))
        }) {
            best = Some((score.0, score.1, prefix));
        }
    }
    best.map(|(_, _, p)| p.trim_end_matches('/').to_string()).unwrap_or_default()
}

/// File nói được điều gì đó về kiến trúc sản phẩm. Dùng để **chọn gốc**; sau
/// khi có gốc thì nhóm vẫn tính mọi file.
///
/// Hai thứ bị loại. Thứ nhất là test, ví dụ và tài liệu — nhưng nếu bỏ chúng
/// đi mà còn lại quá ít thì repo này chủ yếu **là** test, và khi đó bỏ chúng
/// chỉ làm mất thông tin.
///
/// Thứ hai là ngôn ngữ không phải của dự án này. `zstd-rs` có 45 file Rust và
/// 279 file C, mà C là một bản sao zstd nhúng sẵn: chọn theo số file thì gốc
/// rơi vào `zstd-safe/zstd-sys/zstd/` và toàn bộ mã Rust nằm ngoài sơ đồ.
/// Manifest ở gốc repo nói thẳng đây là dự án gì, nên không phải đoán.
fn architectural(root: &Path, files: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = files
        .iter()
        .filter(|f| {
            let dirs = f.rsplit_once('/').map_or("", |(d, _)| d);
            !dirs.split('/').any(|d| ASIDE.contains(&d))
        })
        .cloned()
        .collect();
    if kept.len() * 4 < files.len() {
        kept = files.to_vec();
    }

    let langs = manifest_langs(root);
    if langs.is_empty() {
        return kept;
    }
    let native: Vec<String> = kept
        .iter()
        .filter(|f| Lang::for_path(f).is_some_and(|l| langs.contains(&l)))
        .cloned()
        .collect();
    // Manifest là bằng chứng chứ không phải phỏng đoán, nên không cần ngưỡng
    // tỉ lệ — chỉ cần còn đủ file để chia thành component.
    if native.len() >= MIN_NATIVE_FILES { native } else { kept }
}

/// Ngôn ngữ mà manifest ở gốc repo khai đây là dự án của nó. Một repo có thể
/// khai nhiều cái cùng lúc: `pydantic-core` vừa là crate Rust vừa là gói
/// Python.
fn manifest_langs(root: &Path) -> Vec<Lang> {
    const MANIFESTS: &[(&str, Lang)] = &[
        ("Cargo.toml", Lang::Rust),
        ("go.mod", Lang::Go),
        ("package.json", Lang::TypeScript),
        ("tsconfig.json", Lang::TypeScript),
        ("pyproject.toml", Lang::Python),
        ("setup.py", Lang::Python),
    ];
    let mut out: Vec<Lang> = MANIFESTS
        .iter()
        .filter(|(name, _)| root.join(name).is_file())
        .map(|(_, lang)| *lang)
        .collect();
    out.dedup();
    out
}

/// Thư mục của những package mà chính hệ sinh thái đã khai báo. Không tính
/// package ở gốc repo: nó bao cả repo nên không nói gì về ranh giới bên trong.
///
/// Ranh giới này mạnh hơn mọi phỏng đoán theo thư mục, vì nó **có cưỡng chế
/// thật**: một crate không gọi vào trong crate khác nếu không khai báo phụ
/// thuộc. Chia nhỏ hơn nó là mô tả một thứ không tồn tại.
fn declared_packages(root: &Path) -> Vec<String> {
    const MANIFESTS: &[&str] = &["Cargo.toml", "package.json", "go.mod", "pyproject.toml"];
    let mut out = Vec::new();
    collect_packages(root, "", 0, MANIFESTS, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_packages(
    root: &Path,
    prefix: &str,
    depth: usize,
    manifests: &[&str],
    out: &mut Vec<String>,
) {
    const SKIP: &[&str] = &["node_modules", "target", ".git", ".arch", "dist", "build"];
    if depth > MAX_PREFIX_DEPTH + 1 {
        return;
    }
    let dir = if prefix.is_empty() { root.to_path_buf() } else { root.join(prefix) };
    if !prefix.is_empty() && manifests.iter().any(|m| dir.join(m).is_file()) {
        out.push(format!("{prefix}/"));
    }
    let Ok(entries) = std::fs::read_dir(&dir) else { return };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        if SKIP.contains(&name.as_str()) || name.starts_with('.') || !entry.path().is_dir() {
            continue;
        }
        let next = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
        collect_packages(root, &next, depth + 1, manifests, out);
    }
}

/// Tiền tố đáng thử: gốc repo, và mọi thư mục tới độ sâu `MAX_PREFIX_DEPTH`
/// chứa đủ nhiều file — trừ những chỗ nằm trong một package đã khai báo.
fn candidates(root: &Path, files: &[String]) -> Vec<String> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for f in files {
        let dirs = f.rsplit_once('/').map_or("", |(d, _)| d);
        if dirs.is_empty() {
            continue;
        }
        let parts: Vec<&str> = dirs.split('/').collect();
        for depth in 1..=parts.len().min(MAX_PREFIX_DEPTH) {
            *counts.entry(format!("{}/", parts[..depth].join("/"))).or_default() += 1;
        }
    }
    let floor = (files.len() / MIN_PREFIX_SHARE).max(2);
    let packages = declared_packages(root);
    let mut out = vec![String::new()];
    out.extend(
        counts
            .into_iter()
            .filter(|(_, n)| *n >= floor)
            // Đi vào trong một package là chia nhỏ hơn ranh giới thật. `tokio`
            // có `tokio/src/{runtime,io,fs,…}` nhìn rất giống kiến trúc, nhưng
            // chọn chỗ đó thì `tokio-util` và `tokio-stream` — hai crate riêng
            // — rơi hết ra ngoài sơ đồ.
            .filter(|(p, _)| !packages.iter().any(|pkg| p.starts_with(pkg.as_str())))
            .map(|(p, _)| p),
    );
    out
}

/// Nhóm file theo thư mục con ngay dưới `prefix`. File nằm thẳng trong đó gom
/// vào nhóm `.`.
fn groups_under(files: &[String], prefix: &str) -> BTreeMap<String, usize> {
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    for file in files {
        if let Some(group) = group_of(file, prefix) {
            *groups.entry(group).or_default() += 1;
        }
    }
    groups
}

fn tally(langs: impl Iterator<Item = Lang>) -> Vec<(Lang, usize)> {
    let mut counts: Vec<(Lang, usize)> = Vec::new();
    for lang in langs {
        match counts.iter_mut().find(|(l, _)| *l == lang) {
            Some((_, n)) => *n += 1,
            None => counts.push((lang, 1)),
        }
    }
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    counts
}

/// `create-order` → `CreateOrder`. Ký tự không hợp lệ bị bỏ, vì tên component
/// phải là định danh.
pub fn pascal_case(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c == '-' || c == '_' || c == '.' || c == ' ' {
            upper = true;
        } else if c.is_alphanumeric() {
            if upper {
                out.extend(c.to_uppercase());
            } else {
                out.push(c);
            }
            upper = false;
        }
    }
    if out.is_empty() || out.starts_with(|c: char| c.is_numeric()) {
        out.insert(0, 'M');
    }
    out
}

fn unique_name(base: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}{n}")).find(|c| !taken.contains(c)).unwrap_or_default()
}

/// Viết bản nháp ra dạng DSL. Mọi thứ `@ai #draft`: chưa có hiệu lực, và ghi
/// rõ nguồn gốc là máy sinh chứ không phải người thiết kế.
pub fn render(d: &Discovery, date: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();

    let langs: Vec<String> =
        d.by_lang.iter().map(|(l, n)| format!("{n} {}", l.name())).collect();
    let _ = writeln!(out, "# Sinh bằng `archctl init --from-code` ngày {date}.");
    let _ = writeln!(out, "# Đã đọc {} file ({}).", d.files, langs.join(", "));
    out.push_str(
        "#\n\
         # ĐÂY LÀ MÔ TẢ HIỆN TRẠNG, KHÔNG PHẢI THIẾT KẾ. Nó chép lại cả những chỗ\n\
         # đang lộn xộn. Mọi thứ đang `#draft` nên chưa luật nào có hiệu lực.\n\
         #\n\
         # Cách dùng: xoá những component không có thật, gộp những cái nên gộp,\n\
         # bỏ những quan hệ không nên tồn tại, rồi nâng dần lên `#accepted`.\n\
         # Component nào có `allow` đang có hiệu lực thì component đó bắt đầu bị\n\
         # kiểm — không cần sửa xong cả file mới chạy được.\n",
    );

    if !d.components.is_empty() {
        out.push('\n');
        let width = d.components.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        for (name, glob) in &d.components {
            let _ = writeln!(out, "component {name:<width$} at {glob}   @ai #draft");
        }
    }
    if !d.edges.is_empty() {
        out.push_str("\n# Quan hệ quan sát được trong mã. Cái nào không nên tồn tại thì xoá.\n");
        let width = d.edges.iter().map(|(f, _)| f.len()).max().unwrap_or(0);
        for (from, to) in &d.edges {
            let _ = writeln!(out, "allow {from:<width$} -> {to}   @ai #draft");
        }
    }
    let rpc: Vec<&(String, String)> = d.rpc_edges.iter().filter(|e| !d.edges.contains(e)).collect();
    if !rpc.is_empty() {
        out.push_str("\n# Lời gọi gRPC: client gọi stub, server đăng ký service cùng `.proto`.\n");
        let width = rpc.iter().map(|(f, _)| f.len()).max().unwrap_or(0);
        for (from, to) in rpc {
            let _ = writeln!(out, "allow {from:<width$} -> {to}   @ai #draft");
        }
    }
    let queue: Vec<&(String, String)> =
        d.queue_edges.iter().filter(|e| !d.edges.contains(e) && !d.rpc_edges.contains(e)).collect();
    if !queue.is_empty() {
        out.push_str("\n# Hàng đợi, pub/sub: bên gửi và bên nhận cùng topic, queue hay task.\n");
        let width = queue.iter().map(|(f, _)| f.len()).max().unwrap_or(0);
        for (from, to) in queue {
            let _ = writeln!(out, "allow {from:<width$} -> {to}   @ai #draft");
        }
    }
    let pipeline: Vec<&(String, String)> = d
        .pipeline_edges
        .iter()
        .filter(|e| !d.edges.contains(e) && !d.rpc_edges.contains(e) && !d.queue_edges.contains(e))
        .collect();
    if !pipeline.is_empty() {
        out.push_str("\n# Bước pipeline (LangGraph, Airflow, dvc): bước trước đưa dữ liệu cho bước sau.\n");
        let width = pipeline.iter().map(|(f, _)| f.len()).max().unwrap_or(0);
        for (from, to) in pipeline {
            let _ = writeln!(out, "allow {from:<width$} -> {to}   @ai #draft");
        }
    }
    if !d.services.is_empty() {
        out.push_str(
            "\n# Triển khai, đọc từ docker-compose và manifest Kubernetes. `runs` là\n\
             # component mà lệnh khởi chạy chạm tới theo các quan hệ ở trên. Service\n\
             # không phải của sản phẩm (công cụ dev, bản thử) thì xoá.\n",
        );
        for note in &d.notes {
            let _ = writeln!(out, "# {note}");
        }
        let width = d.services.iter().map(|s| s.name.len()).max().unwrap_or(0);
        let swidth = d.services.iter().map(|s| s.source.len()).max().unwrap_or(0);
        for s in &d.services {
            if let Some(note) = &s.note {
                let _ = writeln!(out, "# {}: không có `runs` — {note}", s.name);
            }
            let runs = if s.runs.is_empty() { String::new() } else { format!(" runs {}", s.runs.join(", ")) };
            let _ = writeln!(out, "service {:<width$} from {:<swidth$}{runs}   @ai #draft", s.name, s.source);
        }
    }
    if !d.links.is_empty() {
        out.push_str(
            "\n# Cạnh mạng thấy trong cấu hình: depends_on, links, tên service hay Service\n\
             # Kubernetes trong biến môi trường, lời gọi gRPC.\n",
        );
        let width = d.links.iter().map(|(f, _)| f.len()).max().unwrap_or(0);
        for (from, to) in &d.links {
            let _ = writeln!(out, "link {from:<width$} -> {}   @ai #draft", to.join(", "));
        }
    }
    if !d.skipped.is_empty() {
        let _ = write!(
            out,
            "\n# Còn {} thư mục nữa chưa thành component, xếp theo số file:\n",
            d.skipped.len()
        );
        for (dir, n) in &d.skipped {
            let _ = writeln!(out, "#   {dir} ({n} file)");
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doi_ten_thu_muc_thanh_dinh_danh() {
        assert_eq!(pascal_case("create-order"), "CreateOrder");
        assert_eq!(pascal_case("web_ui"), "WebUi");
        assert_eq!(pascal_case("api"), "Api");
        assert_eq!(pascal_case("2fa"), "M2fa", "tên không được bắt đầu bằng số");
        assert_eq!(pascal_case("@scope"), "Scope");
    }

    #[test]
    fn ten_trung_duoc_danh_so() {
        let taken = vec!["Api".to_string()];
        assert_eq!(unique_name("Api", &taken), "Api2");
        assert_eq!(unique_name("Web", &taken), "Web");
    }

    /// Không có manifest ở đó nên lọc theo ngôn ngữ không tham gia; chỉ còn
    /// thước đo bố cục.
    fn root_for(files: &[&str]) -> String {
        let owned: Vec<String> = files.iter().map(|s| s.to_string()).collect();
        pick_source_root(Path::new("/khong-ton-tai-o-dau-ca"), &owned)
    }

    #[test]
    fn di_vao_thu_muc_chia_ra_duoc_nhieu_nhat() {
        assert_eq!(root_for(&["src/a/x.ts", "src/b/y.ts", "src/c/z.ts", "docs/w.ts"]), "src");
    }

    #[test]
    fn khong_co_thu_muc_nao_chia_ra_duoc_thi_dung_goc_repo() {
        assert_eq!(
            root_for(&["aa/x.ts", "bb/y.ts", "cc/z.ts"]),
            "",
            "đoán sâu hơn khi không có gì để chia là đoán bừa"
        );
    }

    #[test]
    fn nhom_file_theo_thu_muc_con() {
        assert_eq!(group_of("src/web/page.ts", "src/").as_deref(), Some("web"));
        assert_eq!(group_of("src/main.ts", "src/").as_deref(), Some("."));
        assert_eq!(group_of("docs/x.md", "src/"), None);
    }
}
