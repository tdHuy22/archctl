//! Đọc manifest Kubernetes thành dữ kiện về workload, Service và ConfigMap;
//! đọc `skaffold.yaml` (image dựng từ đâu) và `kustomization.yaml` (patch nào
//! đè lên manifest nào).
//!
//! Như compose (TM-19): giá trị biến môi trường không ra khỏi đây, mỗi giá trị
//! chỉ cho ra tên máy. Giá trị trong ConfigMap được giữ trong bộ nhớ để lấy tên
//! máy theo tên biến môi trường trỏ tới nó, và không bao giờ được in. Tài liệu
//! `kind: Secret` không được đọc. Không render Helm, không áp patch Kustomize.

use super::compose::{self, Budget, Env};
use super::yaml::{self, Node};

/// Trần cho một file, như compose: manifest thật có vài chục tài liệu; file đối
/// kháng thì dùng alias để nhân số mục lên mà không tốn byte nào (TM-18).
pub const MAX_OBJECTS: usize = 2000;
pub const MAX_FACTS: usize = 50_000;
pub const MAX_COPIED: usize = 8 << 20;

/// Loại workload có pod template mà archctl đọc.
pub const WORKLOADS: &[&str] = &[
    "Deployment",
    "StatefulSet",
    "DaemonSet",
    "ReplicaSet",
    "ReplicationController",
    "Job",
    "CronJob",
    "Pod",
];

#[derive(Debug, Clone, Default)]
pub struct File {
    pub workloads: Vec<Workload>,
    pub services: Vec<Service>,
    pub config_maps: Vec<ConfigMap>,
    /// Tài liệu có pod template mà loại không đọc (Argo `Rollout`, CRD): mã
    /// chạy trong đó không được kiểm, nên phải báo ra (TM-21). Loại, tên, dòng.
    pub unknown: Vec<(String, String, usize)>,
    /// Tên trong các tài liệu `kind: Namespace`.
    pub namespaces: Vec<String>,
    pub policies: Vec<NetPolicy>,
    /// Đường từ ngoài cụm vào: Ingress, route của Gateway API, VirtualService
    /// của Istio gắn với một Gateway.
    pub routes: Vec<Route>,
    /// Byte đã chép ra khỏi cây YAML, để bên nạp tính trần chung.
    pub size: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Workload {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub line: usize,
    /// Nhãn của pod template: thứ mà `selector` của Service so với.
    pub labels: Vec<(String, String)>,
    pub containers: Vec<Container>,
    /// `kubectl.kubernetes.io/default-container`: container chính của pod.
    pub default_container: Option<String>,
    /// Pod dùng mạng của máy chủ (`hostNetwork: true`) hay mở `hostPort`: lộ
    /// ra ngoài cụm. Cách viết, để in.
    pub host: Option<String>,
}

impl Workload {
    /// Container chính: theo chú thích `default-container`, không có thì
    /// container đầu tiên (không tính init container), như `kubectl`.
    pub fn main_container(&self) -> Option<usize> {
        let regular = || self.containers.iter().position(|c| !c.init);
        match &self.default_container {
            Some(name) => self
                .containers
                .iter()
                .position(|c| !c.init && &c.name == name)
                .or_else(regular),
            None => regular(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Container {
    pub name: String,
    pub line: usize,
    pub init: bool,
    pub image: Option<String>,
    /// `command` thay `ENTRYPOINT` của image, `args` thay `CMD`.
    pub command: Option<(Vec<String>, usize)>,
    pub args: Option<(Vec<String>, usize)>,
    pub working_dir: Option<String>,
    pub env: Vec<Env>,
    pub env_from: Vec<EnvFrom>,
    pub refs: Vec<ConfigRef>,
}

/// `envFrom: - configMapRef: {name}`: mọi khoá của ConfigMap thành biến môi
/// trường, có thể kèm tiền tố.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnvFrom {
    pub config_map: String,
    pub prefix: String,
    pub line: usize,
}

/// `valueFrom.configMapKeyRef`: biến `key` lấy giá trị khoá `item` của
/// ConfigMap `config_map`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigRef {
    pub key: String,
    pub config_map: String,
    pub item: String,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub namespace: Option<String>,
    pub line: usize,
    pub selector: Vec<(String, String)>,
    /// `type: ExternalName`: tên máy mà Service này là bí danh (viết thường,
    /// bỏ dấu chấm cuối). Có thể trỏ tới một Service khác trong cụm.
    pub external: Option<String>,
    /// Service mở ra ngoài cụm: `type: LoadBalancer`, `NodePort`, hay có
    /// `externalIPs`. Cách viết, để in.
    pub exposed: Option<String>,
}

/// Một đường từ ngoài cụm vào.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Route {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
    pub line: usize,
    /// Service đích (Ingress, Gateway API): tên, namespace nếu backendRef ghi
    /// riêng (route ở namespace hạ tầng trỏ sang namespace ứng dụng), dòng.
    pub services: Vec<(String, Option<String>, usize)>,
    /// Tên máy đích (`destination.host` của VirtualService), kèm dòng.
    pub hosts: Vec<(String, usize)>,
    /// Lối vào từ ngoài (gắn Gateway khác `mesh`). VirtualService chỉ cho
    /// `mesh` vẫn được giữ, vì một VirtualService gắn Gateway có thể
    /// `delegate` sang nó.
    pub public: bool,
    /// `http[].delegate` của VirtualService: tên, namespace, dòng.
    pub delegates: Vec<(String, Option<String>, usize)>,
}

/// Loại route của Gateway API.
const GATEWAY_ROUTES: &[&str] = &["HTTPRoute", "GRPCRoute", "TCPRoute", "TLSRoute", "UDPRoute"];

#[derive(Debug, Clone, Default)]
pub struct ConfigMap {
    pub name: String,
    pub namespace: Option<String>,
    pub line: usize,
    /// Khoá, dòng, giá trị. Giá trị chỉ dùng để tìm tên máy, không bao giờ
    /// được in (TM-19).
    data: Vec<(String, usize, String)>,
}

impl ConfigMap {
    pub fn keys(&self) -> impl Iterator<Item = (&str, usize)> {
        self.data.iter().map(|(k, l, _)| (k.as_str(), *l))
    }

    /// Một khoá như biến môi trường tên `env`: dòng của khoá, và dữ kiện lấy
    /// từ giá trị (tên máy, hay giá trị nếu là `PYTHONPATH`, `FLASK_APP`).
    pub fn entry(&self, item: &str, env: &str) -> Option<Env> {
        self.data
            .iter()
            .find(|(k, _, _)| k == item)
            .map(|(_, line, v)| compose::env_entry(env, *line, v))
    }

    /// Mọi khoá, như `envFrom` với tiền tố `prefix`.
    pub fn entries(&self, prefix: &str) -> Vec<Env> {
        self.data
            .iter()
            .map(|(k, line, v)| compose::env_entry(&format!("{prefix}{k}"), *line, v))
            .collect()
    }
}

/// Bộ chọn nhãn: `matchLabels` và `matchExpressions`. Rỗng là mọi pod.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selector {
    pub labels: Vec<(String, String)>,
    /// Khoá, phép (`In`, `NotIn`, `Exists`, `DoesNotExist`), giá trị.
    pub exprs: Vec<(String, String, Vec<String>)>,
}

impl Selector {
    pub fn is_empty(&self) -> bool {
        self.labels.is_empty() && self.exprs.is_empty()
    }

    pub fn matches(&self, labels: &[(String, String)]) -> bool {
        let get = |k: &str| {
            labels
                .iter()
                .find(|(lk, _)| lk == k)
                .map(|(_, v)| v.as_str())
        };
        self.labels.iter().all(|pair| labels.contains(pair))
            && self.exprs.iter().all(|(k, op, values)| match op.as_str() {
                "In" => get(k).is_some_and(|v| values.iter().any(|x| x == v)),
                "NotIn" => get(k).is_none_or(|v| !values.iter().any(|x| x == v)),
                "Exists" => get(k).is_some(),
                "DoesNotExist" => get(k).is_none(),
                // Phép lạ: Kubernetes từ chối manifest, nên không khớp gì.
                _ => false,
            })
    }
}

/// Một nguồn hay đích trong luật của NetworkPolicy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Peer {
    /// `podSelector`; `None` là mọi pod (trong các namespace được chọn).
    pub pods: Option<Selector>,
    /// `namespaceSelector`; `None` là namespace của chính policy.
    pub namespaces: Option<Selector>,
    /// `ipBlock`: địa chỉ, không phải pod.
    pub ip: bool,
    pub line: usize,
}

/// Một luật `ingress` hay `egress`. `peers` rỗng là mọi nguồn (đích).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule {
    pub peers: Vec<Peer>,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetPolicy {
    pub name: String,
    pub namespace: Option<String>,
    pub line: usize,
    pub pods: Selector,
    /// `None`: policy không cô lập chiều này.
    pub ingress: Option<Vec<Rule>>,
    pub egress: Option<Vec<Rule>>,
}

/// Có thể là manifest: có chữ `apiVersion` và `kind`. Lọc thô trước khi phân
/// tích, vì repo có thể có hàng nghìn file YAML không phải manifest (TM-18).
/// Rộng tay: khoá thụt lề, có nháy, flow `{...}`, JSON, BOM đều lọt qua, và
/// thứ thật sự là manifest được quyết sau khi phân tích (TM-21).
impl File {
    /// Đổi mọi số dòng qua `map`: manifest dựng từ template Helm báo dòng của
    /// template, không phải dòng trong văn bản dựng ra.
    pub fn remap_lines(&mut self, map: &dyn Fn(usize) -> usize) {
        for w in &mut self.workloads {
            w.line = map(w.line);
            for c in &mut w.containers {
                c.line = map(c.line);
                if let Some((_, l)) = &mut c.command {
                    *l = map(*l);
                }
                if let Some((_, l)) = &mut c.args {
                    *l = map(*l);
                }
                for e in &mut c.env {
                    e.line = map(e.line);
                }
                for e in &mut c.env_from {
                    e.line = map(e.line);
                }
                for r in &mut c.refs {
                    r.line = map(r.line);
                }
            }
        }
        for s in &mut self.services {
            s.line = map(s.line);
        }
        for m in &mut self.config_maps {
            m.line = map(m.line);
            for (_, l, _) in &mut m.data {
                *l = map(*l);
            }
        }
        for u in &mut self.unknown {
            u.2 = map(u.2);
        }
        for p in &mut self.policies {
            p.line = map(p.line);
            for rule in p.ingress.iter_mut().chain(p.egress.iter_mut()).flatten() {
                rule.line = map(rule.line);
                for peer in &mut rule.peers {
                    peer.line = map(peer.line);
                }
            }
        }
        for r in &mut self.routes {
            r.line = map(r.line);
            for s in &mut r.services {
                s.2 = map(s.2);
            }
            for h in &mut r.hosts {
                h.1 = map(h.1);
            }
            for d in &mut r.delegates {
                d.2 = map(d.2);
            }
        }
    }
}

pub fn may_be(text: &str) -> bool {
    text.contains("apiVersion") && text.contains("kind")
}

/// Chắc là manifest: `apiVersion:` và `kind:` ở đầu dòng. File như vậy mà
/// không phân tích được thì phải báo; file chỉ qua được [`may_be`] thì không.
pub fn looks_like(text: &str) -> bool {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut kind = false;
    let mut api = false;
    for l in text.lines() {
        kind |= l.starts_with("kind:");
        api |= l.starts_with("apiVersion:");
        if kind && api {
            return true;
        }
    }
    false
}

/// Đếm và chép trong một file, có trần.
struct Reader {
    b: Budget,
    facts: usize,
    objects: usize,
}

impl Reader {
    fn fact(&mut self, n: usize) -> bool {
        match self.facts.checked_sub(n) {
            Some(rest) => {
                self.facts = rest;
                true
            }
            None => {
                self.b.over = true;
                false
            }
        }
    }

    fn text(&mut self, n: Option<Node<'_>>) -> Option<String> {
        n.and_then(Node::as_str).map(|s| self.b.own(s))
    }

    fn pairs(&mut self, n: Option<Node<'_>>) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for (k, _, v) in n.into_iter().flat_map(Node::entries) {
            // Cắt bớt nhãn là im lặng bỏ đúng nhãn mà selector dùng: vượt thì
            // cả file thành "chưa đọc được".
            if out.len() >= MAX_LABELS {
                self.b.over = true;
            }
            if self.b.over || !self.fact(1) {
                break;
            }
            let v = v.as_str().unwrap_or("");
            out.push((self.b.own(k), self.b.own(v)));
        }
        out
    }
}

/// Số nhãn của một pod hay selector. Nhãn thật có vài cái.
const MAX_LABELS: usize = 256;

pub fn parse(text: &str) -> Result<File, yaml::Error> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let doc = yaml::parse(text)?;
    let mut out = File::default();
    let mut r = Reader {
        b: Budget {
            bytes: MAX_COPIED,
            over: false,
        },
        facts: MAX_FACTS,
        objects: 0,
    };
    for root in doc.documents() {
        // `kind: List`, `DeploymentList`… của `kubectl get -o yaml`: một tầng
        // `items`. `kubectl apply` nhận mọi loại `*List` có `items`.
        if root
            .get("kind")
            .and_then(Node::as_str)
            .is_some_and(|k| k.ends_with("List"))
            && root.get("items").is_some_and(Node::is_seq)
        {
            for item in root.get("items").into_iter().flat_map(Node::items) {
                object(item, &mut r, &mut out)?;
            }
        } else {
            object(root, &mut r, &mut out)?;
        }
    }
    out.size = MAX_COPIED - r.b.bytes;
    Ok(out)
}

fn over(line: usize) -> yaml::Error {
    yaml::Error {
        line,
        msg: format!(
            "quá {MAX_OBJECTS} tài liệu, {MAX_FACTS} mục hay {} MB dữ liệu sau khi trải alias trong một file",
            MAX_COPIED >> 20
        ),
    }
}

/// Pod spec và metadata của pod template, theo loại.
fn pod_of<'a>(kind: &str, n: Node<'a>) -> Option<(Node<'a>, Option<Node<'a>>)> {
    let template = match kind {
        "Pod" => return n.get("spec").map(|s| (s, n.get("metadata"))),
        "CronJob" => n
            .get("spec")?
            .get("jobTemplate")?
            .get("spec")?
            .get("template")?,
        _ => n.get("spec")?.get("template")?,
    };
    template.get("spec").map(|s| (s, template.get("metadata")))
}

fn object(n: Node<'_>, r: &mut Reader, out: &mut File) -> Result<(), yaml::Error> {
    if !n.is_map() {
        return Ok(());
    }
    let line = n.line();
    r.objects += 1;
    if r.objects > MAX_OBJECTS {
        return Err(over(line));
    }
    let Some(kind) = n.get("kind").and_then(Node::as_str) else {
        return Ok(());
    };
    let meta = n.get("metadata");
    let name_of = |r: &mut Reader| r.text(meta.and_then(|m| m.get("name"))).unwrap_or_default();
    let ns_of = |r: &mut Reader| r.text(meta.and_then(|m| m.get("namespace")));
    match kind {
        // Secret không bao giờ được đọc, kể cả tên khoá (TM-19).
        "Secret" => {}
        "Namespace" => {
            let name = name_of(r);
            out.namespaces.push(name);
        }
        "Ingress" => {
            let spec = n.get("spec");
            let mut services = Vec::new();
            // `networking.k8s.io/v1`: `backend.service.name`; `v1beta1`:
            // `backend.serviceName`. `defaultBackend` (v1), `backend` (v1beta1).
            let mut backend = |b: Option<Node<'_>>, r: &mut Reader| {
                let Some(b) = b else { return };
                let name = b
                    .get("service")
                    .and_then(|s| s.get("name"))
                    .or_else(|| b.get("serviceName"));
                if let Some(nm) = name.and_then(Node::as_str)
                    && r.fact(1)
                {
                    services.push((r.b.own(nm), None, b.line()));
                }
            };
            backend(spec.and_then(|s| s.get("defaultBackend")), r);
            backend(spec.and_then(|s| s.get("backend")), r);
            for rule in spec
                .and_then(|s| s.get("rules"))
                .into_iter()
                .flat_map(Node::items)
            {
                for p in rule
                    .get("http")
                    .and_then(|h| h.get("paths"))
                    .into_iter()
                    .flat_map(Node::items)
                {
                    backend(p.get("backend"), r);
                }
            }
            let route = Route {
                kind: r.b.own(kind),
                name: name_of(r),
                namespace: ns_of(r),
                line,
                services,
                public: true,
                ..Route::default()
            };
            out.routes.push(route);
        }
        k if GATEWAY_ROUTES.contains(&k) => {
            let spec = n.get("spec");
            // Route gắn vào Service (GAMMA, mesh) là đường trong cụm, không phải lối vào.
            let parents: Vec<Node<'_>> = spec
                .and_then(|s| s.get("parentRefs"))
                .into_iter()
                .flat_map(Node::items)
                .collect();
            let mesh = !parents.is_empty()
                && parents
                    .iter()
                    .all(|p| p.get("kind").and_then(Node::as_str) == Some("Service"));
            if !mesh {
                let mut services = Vec::new();
                for rule in spec
                    .and_then(|s| s.get("rules"))
                    .into_iter()
                    .flat_map(Node::items)
                {
                    for b in rule.get("backendRefs").into_iter().flat_map(Node::items) {
                        let kind_ok = b
                            .get("kind")
                            .and_then(Node::as_str)
                            .is_none_or(|k| k == "Service");
                        if kind_ok
                            && let Some(nm) = b.get("name").and_then(Node::as_str)
                            && r.fact(1)
                        {
                            let ns = r.text(b.get("namespace"));
                            services.push((r.b.own(nm), ns, b.line()));
                        }
                    }
                }
                let route = Route {
                    kind: r.b.own(k),
                    name: name_of(r),
                    namespace: ns_of(r),
                    line,
                    services,
                    public: true,
                    ..Route::default()
                };
                out.routes.push(route);
            }
        }
        "VirtualService" => {
            let spec = n.get("spec");
            // Không ghi `gateways` hay chỉ `mesh`: đường trong cụm.
            let gateways: Vec<&str> = spec
                .and_then(|s| s.get("gateways"))
                .into_iter()
                .flat_map(Node::items)
                .filter_map(Node::as_str)
                .collect();
            let public = gateways.iter().any(|g| *g != "mesh");
            let mut hosts = Vec::new();
            let mut delegates = Vec::new();
            for proto in ["http", "tcp", "tls"] {
                for rule in spec
                    .and_then(|s| s.get(proto))
                    .into_iter()
                    .flat_map(Node::items)
                {
                    if !r.fact(1) {
                        break;
                    }
                    if let Some(d) = rule.get("delegate")
                        && let Some(nm) = d.get("name").and_then(Node::as_str)
                    {
                        let ns = r.text(d.get("namespace"));
                        delegates.push((r.b.own(nm), ns, d.line()));
                    }
                    for d in rule.get("route").into_iter().flat_map(Node::items) {
                        if let Some(h) = d.get("destination").and_then(|x| x.get("host"))
                            && let Some(host) = h.as_str()
                            && r.fact(1)
                        {
                            hosts.push((
                                r.b.own(&host.trim_end_matches('.').to_ascii_lowercase()),
                                h.line(),
                            ));
                        }
                    }
                }
            }
            let route = Route {
                kind: r.b.own(kind),
                name: name_of(r),
                namespace: ns_of(r),
                line,
                hosts,
                public,
                delegates,
                ..Route::default()
            };
            out.routes.push(route);
        }
        "NetworkPolicy" => {
            let spec = n.get("spec");
            let types: Vec<&str> = spec
                .and_then(|s| s.get("policyTypes"))
                .map(|t| t.items().filter_map(Node::as_str).collect())
                .unwrap_or_default();
            // Không ghi `policyTypes`: luôn cô lập ingress, cô lập egress khi mục
            // `egress` là danh sách không rỗng (`SetDefaults_NetworkPolicy` của
            // Kubernetes); `egress: []` không cô lập gì.
            let has = |dir: &str, key: &str| {
                if types.is_empty() {
                    dir == "Ingress"
                        || spec
                            .and_then(|s| s.get(key))
                            .is_some_and(|e| e.items().next().is_some())
                } else {
                    types.contains(&dir)
                }
            };
            let (ingress, egress) = (has("Ingress", "ingress"), has("Egress", "egress"));
            let pods = selector(spec.and_then(|s| s.get("podSelector")), r);
            let rules = |key: &str, side: &str, r: &mut Reader| -> Vec<Rule> {
                let mut out = Vec::new();
                for rule in spec
                    .and_then(|s| s.get(key))
                    .into_iter()
                    .flat_map(Node::items)
                {
                    if !r.fact(1) {
                        break;
                    }
                    let mut peers = Vec::new();
                    for p in rule.get(side).into_iter().flat_map(Node::items) {
                        if !r.fact(1) {
                            break;
                        }
                        let pods = p.get("podSelector").map(|s| selector(Some(s), r));
                        let namespaces = p.get("namespaceSelector").map(|s| selector(Some(s), r));
                        peers.push(Peer {
                            pods,
                            namespaces,
                            ip: p.get("ipBlock").is_some(),
                            line: p.line(),
                        });
                    }
                    out.push(Rule {
                        peers,
                        line: rule.line(),
                    });
                }
                out
            };
            let ingress = ingress.then(|| rules("ingress", "from", r));
            let egress = egress.then(|| rules("egress", "to", r));
            let p = NetPolicy {
                name: name_of(r),
                namespace: ns_of(r),
                line,
                pods,
                ingress,
                egress,
            };
            out.policies.push(p);
        }
        "Service" => {
            let spec = n.get("spec");
            // `ExternalName` không chọn pod nào: nó là bí danh của một tên máy.
            let kind = spec.and_then(|s| s.get("type")).and_then(Node::as_str);
            let (selector, external) = match kind {
                Some("ExternalName") => {
                    let host = r.text(spec.and_then(|s| s.get("externalName")));
                    (
                        Vec::new(),
                        host.map(|h| h.trim_end_matches('.').to_ascii_lowercase()),
                    )
                }
                _ => (r.pairs(spec.and_then(|s| s.get("selector"))), None),
            };
            let exposed = match kind {
                Some(k @ ("LoadBalancer" | "NodePort")) => Some(format!("type: {k}")),
                _ if spec
                    .and_then(|s| s.get("externalIPs"))
                    .is_some_and(|e| e.items().next().is_some()) =>
                {
                    Some("externalIPs".to_string())
                }
                _ => None,
            };
            let s = Service {
                name: name_of(r),
                namespace: ns_of(r),
                line,
                selector,
                external,
                exposed,
            };
            out.services.push(s);
        }
        "ConfigMap" => {
            let mut data = Vec::new();
            for (k, l, v) in n.get("data").into_iter().flat_map(Node::entries) {
                if !r.fact(1) {
                    break;
                }
                let v = v.as_str().unwrap_or("");
                data.push((r.b.own(k), l, r.b.own(v)));
            }
            let c = ConfigMap {
                name: name_of(r),
                namespace: ns_of(r),
                line,
                data,
            };
            out.config_maps.push(c);
        }
        k if WORKLOADS.contains(&k) => {
            let Some((spec, pod_meta)) = pod_of(k, n) else {
                return Ok(());
            };
            let mut w = Workload {
                kind: r.b.own(k),
                name: name_of(r),
                namespace: ns_of(r),
                line,
                labels: r.pairs(pod_meta.and_then(|m| m.get("labels"))),
                containers: Vec::new(),
                default_container: r.text(
                    pod_meta
                        .and_then(|m| m.get("annotations"))
                        .and_then(|a| a.get("kubectl.kubernetes.io/default-container")),
                ),
                host: None,
            };
            for (key, init) in [("initContainers", true), ("containers", false)] {
                for c in spec.get(key).into_iter().flat_map(Node::items) {
                    if !r.fact(1) {
                        break;
                    }
                    if w.host.is_none()
                        && c.get("ports")
                            .into_iter()
                            .flat_map(Node::items)
                            .any(|p| p.get("hostPort").is_some())
                    {
                        w.host = Some("hostPort".to_string());
                    }
                    w.containers.push(container(c, init, r));
                }
            }
            // YAML 1.1 như Kubernetes đọc: `true`, `True`, `yes`, `on`, `y`.
            if spec
                .get("hostNetwork")
                .and_then(Node::as_str)
                .is_some_and(truthy)
            {
                w.host = Some("hostNetwork: true".to_string());
            }
            out.workloads.push(w);
        }
        k if !k.ends_with("List") && generic_pod(n) => {
            let name = name_of(r);
            out.unknown.push((r.b.own(k), name, line));
        }
        _ => {}
    }
    if r.b.over {
        return Err(over(line));
    }
    Ok(())
}

/// Tài liệu loại lạ mà có pod template: `spec.template.spec.containers`, hay
/// `spec.jobTemplate.spec.template.spec.containers`.
fn generic_pod(n: Node<'_>) -> bool {
    let has = |t: Option<Node<'_>>| {
        t.and_then(|t| t.get("spec"))
            .and_then(|s| s.get("containers"))
            .is_some_and(Node::is_seq)
    };
    let spec = n.get("spec");
    has(spec.and_then(|s| s.get("template")))
        || has(spec
            .and_then(|s| s.get("jobTemplate"))
            .and_then(|j| j.get("spec"))
            .and_then(|s| s.get("template")))
}

/// Giá trị đúng của YAML 1.1 (thư viện YAML của Kubernetes).
fn truthy(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "true" | "yes" | "on" | "y")
}

fn selector(n: Option<Node<'_>>, r: &mut Reader) -> Selector {
    let Some(n) = n else {
        return Selector::default();
    };
    let mut s = Selector {
        labels: r.pairs(n.get("matchLabels")),
        exprs: Vec::new(),
    };
    for e in n.get("matchExpressions").into_iter().flat_map(Node::items) {
        if s.exprs.len() >= MAX_LABELS {
            r.b.over = true;
        }
        if r.b.over || !r.fact(1) {
            break;
        }
        let key = r.text(e.get("key")).unwrap_or_default();
        let op = r.text(e.get("operator")).unwrap_or_default();
        let values = e
            .get("values")
            .into_iter()
            .flat_map(Node::items)
            .filter_map(Node::as_str)
            .map(|v| r.b.own(v))
            .collect();
        s.exprs.push((key, op, values));
    }
    s
}

fn words(n: Option<Node<'_>>, r: &mut Reader) -> Option<(Vec<String>, usize)> {
    let n = n?;
    if !n.is_seq() {
        // Kubernetes chỉ nhận danh sách; chuỗi là manifest sai, coi như rỗng
        // để không đoán.
        return n.as_str().map(|_| (Vec::new(), n.line()));
    }
    let items: Vec<String> = n
        .items()
        .map(|i| r.b.own(i.as_str().unwrap_or("")))
        .collect();
    Some((items, n.line()))
}

fn container(n: Node<'_>, init: bool, r: &mut Reader) -> Container {
    let mut c = Container {
        name: r.text(n.get("name")).unwrap_or_default(),
        line: n.line(),
        init,
        image: r.text(n.get("image")),
        command: words(n.get("command"), r),
        args: words(n.get("args"), r),
        working_dir: r.text(n.get("workingDir")),
        ..Container::default()
    };
    // Giá trị của các biến đứng trước, cho `$(TÊN)`: Kubernetes thay bằng giá
    // trị của biến khai trước đó trong cùng container. Chỉ trong bộ nhớ.
    let mut earlier: Vec<(&str, String)> = Vec::new();
    for e in n.get("env").into_iter().flat_map(Node::items) {
        if !r.fact(1) {
            break;
        }
        let Some(key) = e.get("name").and_then(Node::as_str) else {
            continue;
        };
        if let Some(v) = e.get("value") {
            let raw = v.as_str().unwrap_or("");
            let value = if raw.contains("$(") {
                expand(raw, &earlier)
            } else {
                raw.to_string()
            };
            if r.b.take(value.len()) {
                c.env.push(compose::env_entry(key, e.line(), &value));
                earlier.push((key, value));
            }
        } else if let Some(cm) = e.get("valueFrom").and_then(|v| v.get("configMapKeyRef")) {
            // `secretKeyRef`, `fieldRef` thì bỏ: không đọc Secret (TM-19).
            if let (Some(name), Some(item)) = (
                cm.get("name").and_then(Node::as_str),
                cm.get("key").and_then(Node::as_str),
            ) {
                let key = r.b.own(key);
                let (config_map, item) = (r.b.own(name), r.b.own(item));
                c.refs.push(ConfigRef {
                    key,
                    config_map,
                    item,
                    line: e.line(),
                });
            }
        }
    }
    for f in n.get("envFrom").into_iter().flat_map(Node::items) {
        if !r.fact(1) {
            break;
        }
        if let Some(name) = f
            .get("configMapRef")
            .and_then(|m| m.get("name"))
            .and_then(Node::as_str)
        {
            let config_map = r.b.own(name);
            let prefix = r.text(f.get("prefix")).unwrap_or_default();
            c.env_from.push(EnvFrom {
                config_map,
                prefix,
                line: f.line(),
            });
        }
    }
    c
}

/// `$(TÊN)` thành giá trị của biến khai trước; `$$(TÊN)` là chữ `$(TÊN)`.
/// Biến không biết (`fieldRef`, Secret) giữ nguyên: không đoán.
fn expand(value: &str, earlier: &[(&str, String)]) -> String {
    let mut out = String::new();
    let mut rest = value;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        if let Some(t) = tail.strip_prefix("$$(") {
            out.push_str("$(");
            rest = t;
        } else if let Some(t) = tail.strip_prefix("$(")
            && let Some(end) = t.find(')')
            && let Some((_, v)) = earlier.iter().rev().find(|(k, _)| *k == &t[..end])
        {
            out.push_str(v);
            rest = &t[end + 1..];
        } else {
            out.push('$');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Một artifact trong `skaffold.yaml`: image dựng từ thư mục nào, Dockerfile nào.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub image: String,
    /// Như file viết, tương đối với thư mục của `skaffold.yaml`.
    pub context: String,
    /// `None`: dựng không qua Dockerfile (`ko`, `jib`, `buildpacks`, `bazel`,
    /// `custom`), nên không biết lệnh khởi chạy.
    pub dockerfile: Option<String>,
    pub target: Option<String>,
    pub line: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Skaffold {
    pub artifacts: Vec<Artifact>,
    /// Đường dẫn manifest mà skaffold triển khai (`rawYaml`, `kustomize.paths`,
    /// `kubectl.manifests`), như file viết, kèm dòng.
    pub manifests: Vec<(String, usize)>,
}

pub fn skaffold(text: &str) -> Result<Skaffold, yaml::Error> {
    let doc = yaml::parse(text)?;
    let mut out = Skaffold::default();
    let mut r = Reader {
        b: Budget {
            bytes: MAX_COPIED,
            over: false,
        },
        facts: MAX_FACTS,
        objects: 0,
    };
    for root in doc.documents() {
        let profiles: Vec<Node<'_>> = root
            .get("profiles")
            .into_iter()
            .flat_map(Node::items)
            .collect();
        for base in std::iter::once(root).chain(profiles) {
            for a in base
                .get("build")
                .and_then(|b| b.get("artifacts"))
                .into_iter()
                .flat_map(Node::items)
            {
                if !r.fact(1) {
                    return Err(over(a.line()));
                }
                let Some(image) = r.text(a.get("image")) else {
                    continue;
                };
                const OTHER: &[&str] = &["ko", "jib", "buildpacks", "bazel", "custom"];
                let (dockerfile, target) =
                    if let Some(d) = a.get("docker").or_else(|| a.get("kaniko")) {
                        (
                            Some(
                                r.text(d.get("dockerfile"))
                                    .unwrap_or_else(|| "Dockerfile".into()),
                            ),
                            r.text(d.get("target")),
                        )
                    } else if OTHER.iter().any(|k| a.get(k).is_some()) {
                        (None, None)
                    } else {
                        (Some("Dockerfile".into()), None)
                    };
                let context = r.text(a.get("context")).unwrap_or_else(|| ".".into());
                out.artifacts.push(Artifact {
                    image,
                    context,
                    dockerfile,
                    target,
                    line: a.line(),
                });
            }
            let lists = [
                base.get("manifests").and_then(|m| m.get("rawYaml")),
                base.get("manifests")
                    .and_then(|m| m.get("kustomize"))
                    .and_then(|k| k.get("paths")),
                base.get("deploy")
                    .and_then(|d| d.get("kubectl"))
                    .and_then(|k| k.get("manifests")),
                base.get("deploy")
                    .and_then(|d| d.get("kustomize"))
                    .and_then(|k| k.get("paths")),
            ];
            for p in lists.into_iter().flatten().flat_map(Node::items) {
                if !r.fact(1) {
                    return Err(over(p.line()));
                }
                if let Some(s) = p.as_str() {
                    out.manifests.push((r.b.own(s), p.line()));
                }
            }
        }
        if r.b.over {
            return Err(over(root.line()));
        }
    }
    Ok(out)
}

/// Một patch trong `kustomization.yaml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Patch {
    /// File patch, như file viết; `None` khi patch viết thẳng.
    pub path: Option<String>,
    /// Đích ghi trong `target`: loại và tên, mỗi thứ có thể bỏ trống.
    pub target: Option<(Option<String>, Option<String>)>,
    /// Patch viết thẳng: đích lấy từ nội dung, và có đổi thứ archctl đọc không.
    pub inline: Option<Vec<Touch>>,
    pub line: usize,
}

/// Một workload mà patch nhắm tới, và patch có đổi lệnh, env, image không.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Touch {
    pub kind: Option<String>,
    pub name: Option<String>,
    pub touches: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kustomization {
    pub resources: Vec<(String, usize)>,
    pub components: Vec<(String, usize)>,
    pub patches: Vec<Patch>,
    /// `namespace:` đặt cho mọi tài nguyên.
    pub namespace: Option<String>,
    /// Phép biến đổi archctl không áp mà đổi thứ nó kiểm: tên (`namePrefix`,
    /// `nameSuffix`), image (`images`), ConfigMap sinh ra
    /// (`configMapGenerator`), giá trị chép từ chỗ khác (`replacements`,
    /// `vars`). Khoá và dòng.
    pub transforms: Vec<(String, usize)>,
}

/// Khoá của kustomization đổi tên Service, image hay giá trị env.
pub const TRANSFORMS: &[&str] = &[
    "namePrefix",
    "nameSuffix",
    "images",
    "configMapGenerator",
    "replacements",
    "vars",
];

pub fn is_kustomization(name: &str) -> bool {
    matches!(
        name,
        "kustomization.yaml" | "kustomization.yml" | "Kustomization"
    )
}

pub fn is_skaffold(name: &str) -> bool {
    matches!(name, "skaffold.yaml" | "skaffold.yml")
}

pub fn kustomization(text: &str) -> Result<Kustomization, yaml::Error> {
    let doc = yaml::parse(text)?;
    let mut out = Kustomization::default();
    let mut r = Reader {
        b: Budget {
            bytes: MAX_COPIED,
            over: false,
        },
        facts: MAX_FACTS,
        objects: 0,
    };
    let Some(root) = doc.documents().next() else {
        return Ok(out);
    };
    out.namespace = r.text(root.get("namespace"));
    for (k, l, _) in root.entries() {
        if TRANSFORMS.contains(&k) {
            out.transforms.push((k.to_string(), l));
        }
    }
    // `bases` là tên cũ của `resources`.
    for key in ["resources", "bases", "components"] {
        for p in root.get(key).into_iter().flat_map(Node::items) {
            if !r.fact(1) {
                return Err(over(p.line()));
            }
            if let Some(s) = p.as_str() {
                let entry = (r.b.own(s), p.line());
                if key == "components" {
                    out.components.push(entry)
                } else {
                    out.resources.push(entry)
                }
            }
        }
    }
    // Patch viết thẳng được đọc một lần cho mỗi nội dung, và tính vào trần
    // byte: alias nhân một chuỗi 1 MB lên hàng nghìn patch (TM-18).
    // Khoá theo vị trí của chuỗi trong cây YAML: alias trỏ về cùng một chuỗi,
    // và băm lại nội dung 1 MB cho mỗi alias cũng là bậc hai.
    let mut seen: std::collections::HashMap<(usize, usize), Vec<Touch>> =
        std::collections::HashMap::new();
    let mut inline = |s: &str, r: &mut Reader| -> Option<Vec<Touch>> {
        let key = (s.as_ptr() as usize, s.len());
        if let Some(t) = seen.get(&key) {
            return Some(t.clone());
        }
        if !r.b.take(s.len()) {
            return None;
        }
        let t = touches(s);
        seen.insert(key, t.clone());
        Some(t)
    };
    for key in ["patches", "patchesStrategicMerge", "patchesJson6902"] {
        for p in root.get(key).into_iter().flat_map(Node::items) {
            if !r.fact(1) {
                return Err(over(p.line()));
            }
            let target = p
                .get("target")
                .map(|t| (r.text(t.get("kind")), r.text(t.get("name"))));
            let (path, inline) = match p.as_str() {
                // `patchesStrategicMerge` nhận cả đường dẫn lẫn patch viết thẳng.
                Some(s) if s.contains('\n') || s.trim_start().starts_with('{') => {
                    (None, inline(s, &mut r))
                }
                Some(s) => (Some(r.b.own(s)), None),
                None => (
                    r.text(p.get("path")),
                    p.get("patch")
                        .and_then(Node::as_str)
                        .and_then(|s| inline(s, &mut r)),
                ),
            };
            out.patches.push(Patch {
                path,
                target,
                inline,
                line: p.line(),
            });
        }
    }
    if r.b.over {
        return Err(over(1));
    }
    Ok(out)
}

/// Patch đổi lệnh khởi chạy, biến môi trường hay image thì đổi thứ archctl
/// kiểm; patch đổi `replicas`, `resources` thì không.
const TOUCHED: &[&str] = &[
    "command",
    "args",
    "env",
    "envFrom",
    "image",
    "workingDir",
    "containers",
    "initContainers",
];

/// Đích và tác động của một patch, đọc từ nội dung: strategic merge (tài liệu
/// có `kind`, `metadata.name`) hay JSON 6902 (danh sách thao tác có `path`).
/// Không đọc được thì coi như patch đổi mọi thứ, không rõ đích.
pub fn touches(text: &str) -> Vec<Touch> {
    let Ok(doc) = yaml::parse(text) else {
        return vec![Touch {
            kind: None,
            name: None,
            touches: true,
        }];
    };
    let mut out = Vec::new();
    for root in doc.documents().take(MAX_OBJECTS) {
        if root.is_seq() {
            let hit = root.items().take(MAX_FACTS).any(|op| {
                ["path", "from"].iter().any(|k| {
                    op.get(k)
                        .and_then(Node::as_str)
                        .is_some_and(json_pointer_touches)
                })
            });
            out.push(Touch {
                kind: None,
                name: None,
                touches: hit,
            });
            continue;
        }
        let kind = root.get("kind").and_then(Node::as_str).map(str::to_string);
        let name = root
            .get("metadata")
            .and_then(|m| m.get("name"))
            .and_then(Node::as_str)
            .map(str::to_string);
        let pod = kind
            .as_deref()
            .and_then(|k| pod_of(k, root))
            .map(|(s, _)| s);
        let hit = pod.is_some_and(|spec| {
            ["containers", "initContainers"].iter().any(|key| {
                spec.get(key)
                    .into_iter()
                    .flat_map(Node::items)
                    .take(MAX_FACTS)
                    .any(|c| {
                        c.entries().any(|(k, _, _)| TOUCHED.contains(&k))
                            && c.entries().any(|(k, _, _)| k != "name")
                    })
            })
        });
        out.push(Touch {
            kind,
            name,
            touches: hit,
        });
    }
    out
}

/// Một đường dẫn JSON 6902 có đụng tới container không: chứa một đoạn trong
/// [`TOUCHED`], hay là tổ tiên của danh sách container (`/spec/template/spec`
/// thay cả pod spec). `~1` là `/`, `~0` là `~`.
fn json_pointer_touches(path: &str) -> bool {
    let segs: Vec<String> = path
        .split('/')
        .skip(1)
        .map(|s| s.replace("~1", "/").replace("~0", "~"))
        .collect();
    if segs.iter().any(|s| TOUCHED.contains(&s.as_str())) {
        return true;
    }
    const PODS: &[&[&str]] = &[
        &["spec", "template", "spec"],
        &["spec", "jobTemplate", "spec", "template", "spec"],
    ];
    PODS.iter()
        .any(|pod| segs.len() <= pod.len() && segs.iter().zip(pod.iter()).all(|(a, b)| a == b))
}

/// Tên image để so: bỏ digest và tag. Trả tên đầy đủ (gồm registry) và đoạn
/// cuối.
pub fn image_name(image: &str) -> (&str, &str) {
    let image = image.split('@').next().unwrap_or(image);
    let (head, last) = image.rsplit_once('/').map_or(("", image), |(h, l)| (h, l));
    let last = last.split(':').next().unwrap_or(last);
    let full = &image[..head.len() + if head.is_empty() { 0 } else { 1 } + last.len()];
    (full, last)
}

/// Image trong manifest có phải image này dựng ra không. Tên trần ở phía dựng
/// (`checkoutservice` trong skaffold, registry đặt lúc chạy) khớp đoạn cuối
/// của tên đầy đủ; tên có registry thì phải khớp đủ (TM-22).
pub fn same_image(manifest: &str, built: &str) -> bool {
    let (mf, ml) = image_name(manifest);
    let (bf, bl) = image_name(built);
    if mf.is_empty() || bf.is_empty() {
        return false;
    }
    mf == bf || (!bf.contains('/') && ml == bl)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUTIQUE: &str = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: checkoutservice
spec:
  selector:
    matchLabels:
      app: checkoutservice
  template:
    metadata:
      labels:
        app: checkoutservice
    spec:
      containers:
        - name: server
          image: checkoutservice
          env:
          - name: PRODUCT_CATALOG_SERVICE_ADDR
            value: "productcatalogservice:3550"
          - name: DB_PASSWORD
            valueFrom:
              secretKeyRef: {name: db, key: pw}
          - name: CACHE_ADDR
            valueFrom:
              configMapKeyRef: {name: cfg, key: cache}
          envFrom:
          - configMapRef: {name: cfg}
            prefix: APP_
---
apiVersion: v1
kind: Service
metadata:
  name: checkoutservice
spec:
  selector:
    app: checkoutservice
---
apiVersion: v1
kind: Secret
metadata:
  name: db
stringData:
  pw: hunter2
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: cfg
data:
  cache: redis-cart:6379
"#;

    #[test]
    fn doc_workload_service_configmap() {
        assert!(looks_like(BOUTIQUE));
        let f = parse(BOUTIQUE).unwrap();
        assert_eq!(f.workloads.len(), 1);
        let w = &f.workloads[0];
        assert_eq!(
            (w.kind.as_str(), w.name.as_str(), w.line),
            ("Deployment", "checkoutservice", 1)
        );
        assert_eq!(
            w.labels,
            [("app".to_string(), "checkoutservice".to_string())]
        );
        let c = &w.containers[0];
        assert_eq!(c.image.as_deref(), Some("checkoutservice"));
        assert_eq!(c.env.len(), 1, "secretKeyRef bị bỏ");
        assert_eq!(c.env[0].hosts[0].shown(), "productcatalogservice:3550");
        assert_eq!(
            c.refs,
            [ConfigRef {
                key: "CACHE_ADDR".into(),
                config_map: "cfg".into(),
                item: "cache".into(),
                line: 23
            }]
        );
        assert_eq!(c.env_from[0].prefix, "APP_");
        assert_eq!(
            f.services[0].selector,
            [("app".to_string(), "checkoutservice".to_string())]
        );
        let cm = &f.config_maps[0];
        assert_eq!(
            cm.entry("cache", "CACHE_ADDR").unwrap().hosts[0].name,
            "redis-cart"
        );
        assert_eq!(cm.entries("APP_")[0].key, "APP_cache");
        // Secret không vào đâu cả, và giá trị env không được giữ.
        let debug = format!("{f:?}");
        assert!(!debug.contains("hunter2"));
        assert!(!format!("{:?}", f.workloads).contains("productcatalogservice:3550"));
    }

    #[test]
    fn cronjob_pod_list_va_loai_la() {
        let text = "apiVersion: v1\nkind: List\nitems:\n- apiVersion: batch/v1\n  kind: CronJob\n  metadata: {name: nightly}\n  spec:\n    jobTemplate:\n      spec:\n        template:\n          spec:\n            containers: [{name: j, image: job, command: [python, -m, app.jobs]}]\n- apiVersion: v1\n  kind: Pod\n  metadata: {name: p, labels: {a: b}}\n  spec:\n    containers: [{name: c, image: x}]\n---\napiVersion: argoproj.io/v1alpha1\nkind: Rollout\nmetadata: {name: api}\nspec:\n  template:\n    spec:\n      containers: [{name: c, image: api}]\n";
        let f = parse(text).unwrap();
        assert_eq!(
            f.workloads
                .iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>(),
            ["nightly", "p"]
        );
        assert_eq!(
            f.workloads[0].containers[0].command.as_ref().unwrap().0,
            ["python", "-m", "app.jobs"]
        );
        assert_eq!(f.workloads[1].labels, [("a".to_string(), "b".to_string())]);
        assert_eq!(f.unknown, [("Rollout".to_string(), "api".to_string(), 19)]);
    }

    #[test]
    fn container_chinh() {
        let mut w = Workload {
            containers: vec![
                Container {
                    name: "init".into(),
                    init: true,
                    ..Container::default()
                },
                Container {
                    name: "app".into(),
                    ..Container::default()
                },
                Container {
                    name: "proxy".into(),
                    ..Container::default()
                },
            ],
            ..Workload::default()
        };
        assert_eq!(w.main_container(), Some(1));
        w.default_container = Some("proxy".into());
        assert_eq!(w.main_container(), Some(2));
    }

    #[test]
    fn so_ten_image() {
        assert_eq!(
            image_name("gcr.io/p/emailservice:v1@sha256:ab"),
            ("gcr.io/p/emailservice", "emailservice")
        );
        assert_eq!(
            image_name("localhost:5000/api"),
            ("localhost:5000/api", "api")
        );
        assert!(same_image(
            "us-docker.pkg.dev/x/emailservice:v0.10.3",
            "emailservice"
        ));
        assert!(same_image("emailservice", "emailservice"));
        assert!(!same_image(
            "evil.io/x/emailservice",
            "good.io/x/emailservice"
        ));
        assert!(!same_image("emailservice2", "emailservice"));
        assert!(same_image("myorg/api:1", "myorg/api"));
    }

    #[test]
    fn doc_skaffold_va_kustomization() {
        let s = skaffold("apiVersion: skaffold/v3\nkind: Config\nbuild:\n  artifacts:\n  - image: emailservice\n    context: src/emailservice\n  - image: cart\n    context: src/cart\n    docker: {dockerfile: Dockerfile.prod, target: run}\n  - image: ad\n    jib: {}\nmanifests:\n  kustomize:\n    paths: [kubernetes-manifests]\nprofiles:\n- name: x\n  build:\n    artifacts:\n    - image: other\n").unwrap();
        let a: Vec<_> = s
            .artifacts
            .iter()
            .map(|a| {
                (
                    a.image.as_str(),
                    a.context.as_str(),
                    a.dockerfile.as_deref(),
                    a.target.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            a,
            [
                ("emailservice", "src/emailservice", Some("Dockerfile"), None),
                ("cart", "src/cart", Some("Dockerfile.prod"), Some("run")),
                ("ad", ".", None, None),
                ("other", ".", Some("Dockerfile"), None)
            ]
        );
        assert_eq!(s.manifests, [("kubernetes-manifests".to_string(), 14)]);

        let k = kustomization("resources:\n- ../base\n- svc.yaml\ncomponents: [../../components/x]\npatches:\n- path: env.yaml\n- target: {kind: Deployment, name: api}\n  patch: |-\n    - op: add\n      path: /spec/template/spec/containers/0/env/-\n      value: {name: A, value: b}\npatchesStrategicMerge:\n- |-\n  kind: Deployment\n  metadata: {name: web}\n  spec: {replicas: 3}\n").unwrap();
        assert_eq!(
            k.resources,
            [("../base".to_string(), 2), ("svc.yaml".to_string(), 3)]
        );
        assert_eq!(k.components.len(), 1);
        assert_eq!(k.patches[0].path.as_deref(), Some("env.yaml"));
        assert_eq!(
            k.patches[1].target,
            Some((Some("Deployment".into()), Some("api".into())))
        );
        assert!(k.patches[1].inline.as_ref().unwrap()[0].touches);
        let web = &k.patches[2].inline.as_ref().unwrap()[0];
        assert_eq!((web.name.as_deref(), web.touches), (Some("web"), false));
    }

    #[test]
    fn patch_doi_env_la_cham() {
        let t = touches(
            "kind: Deployment\nmetadata: {name: api}\nspec:\n  template:\n    spec:\n      containers:\n      - name: app\n        env: [{name: X, value: y}]\n",
        );
        assert_eq!(
            t,
            [Touch {
                kind: Some("Deployment".into()),
                name: Some("api".into()),
                touches: true
            }]
        );
        let t = touches(
            "kind: Deployment\nmetadata: {name: api}\nspec:\n  template:\n    spec:\n      containers:\n      - name: app\n",
        );
        assert!(!t[0].touches);
    }

    #[test]
    fn alias_nhan_env_bi_chan() {
        let mut text = String::from("apiVersion: v1\nkind: List\nshared: &e\n");
        for i in 0..200 {
            text.push_str(&format!(
                "- {{name: K{i}, value: \"{}\"}}\n",
                "a".repeat(100)
            ));
        }
        text.push_str("items:\n");
        for i in 0..3000 {
            text.push_str(&format!("- {{kind: Pod, metadata: {{name: p{i}}}, spec: {{containers: [{{name: c, image: x, env: *e}}]}}}}\n"));
        }
        let t = std::time::Instant::now();
        assert!(parse(&text).is_err());
        assert!(t.elapsed().as_secs_f64() < 2.0);
    }
}
