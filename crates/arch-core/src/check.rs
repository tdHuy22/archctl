//! Đối chiếu mã nguồn với luật phụ thuộc.
//!
//! Ngữ nghĩa quan trọng nhất ở đây là **cách một component được đưa vào diện
//! kiểm**: chỉ khi có ít nhất một `allow` đang có hiệu lực mà nguồn là chính
//! nó. Nhờ vậy áp dần được lên codebase đã có — viết danh sách allow cho
//! component nào thì component đó bắt đầu bị kiểm, phần còn lại tạm để yên.
//!
//! Ngoại lệ: `deny` luôn có hiệu lực, kể cả với component chưa vào diện kiểm.
//! Luật cấm là thứ không nên phải chờ ai bật.

use crate::glob;
use crate::model::*;
use crate::resolve::{NativeImport, Resolved, Resolver};
use crate::scan::deploy::{self, Deploy, Key, Svc};
use crate::scan::foreign::Crossing;
use crate::scan::grpc;
use crate::scan::{self, Lang};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

/// Thư mục không bao giờ quét. Không cấu hình được, vì quét chúng chỉ tốn thời
/// gian chứ không bao giờ hữu ích.
pub const ALWAYS_IGNORE: &[&str] = &[
    ".git",
    ".arch",
    "node_modules",
    "target",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "vendor",
    "__pycache__",
    ".venv",
];

/// Tên thường dành cho output, nhưng cũng là tên của mã thật: etcd có package
/// Go `tests/robustness/coverage/`, zstd có mã C trong `build/`. Bỏ theo tên
/// thì mã đó biến mất khỏi báo cáo, không một lời nào.
///
/// Nên hỏi `.gitignore`: output thì bị bỏ qua ở đó, mã thật thì được commit.
/// Không có `.gitignore` nào để hỏi thì vẫn bỏ theo tên — một bundle trong
/// `dist/` bị quét như mã kiến trúc còn tệ hơn.
pub const OUTPUT_DIRS: &[&str] = &["dist", "build", "out", "coverage"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Có `deny` đang có hiệu lực chặn quan hệ này.
    Denied,
    /// Component đã vào diện kiểm nhưng không `allow` nào cho phép quan hệ này.
    NoAllow,
    /// Chạm vào kho dữ liệu của component khác.
    NotOwner,
    /// Tên lẽ ra chỉ nằm trong một vùng nhưng lọt ra ngoài.
    Leaked,
    /// Cạnh mạng giữa hai service mà không có `link` nào cho phép.
    NotLinked,
    /// Mã của một component chạy trong service không khai nó trong `runs`.
    NotRun,
}

/// Phụ thuộc được phát hiện bằng cách nào.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Câu lệnh import. Không thấy được FFI, liên kết thư viện hay IPC.
    Import,
    /// Tên hàm hoặc từ khoá xuất hiện trong mã. Không có vùng mù như import.
    Symbol,
    /// Tên bảng dữ liệu, tìm trong định danh và trong chuỗi.
    Table,
    /// Lời gọi qua ranh giới ngôn ngữ: `extern "C"` của Rust, `C.x` của Go,
    /// ghép với định nghĩa cùng tên ở phía C; import Python của module mở
    /// rộng và `torch.ops.ns.op`, ghép với phía native định nghĩa chúng.
    Ffi,
    /// Cấu hình triển khai: `depends_on`, `links`, tên service trong biến môi
    /// trường.
    Network,
    /// Lệnh khởi chạy của service: `command`, `CMD`, `ENTRYPOINT`.
    Entry,
    /// Lời gọi gRPC: chỗ gọi stub ghép với chỗ đăng ký server cùng
    /// `package.Service`. Hai phía không import nhau.
    Rpc,
    /// Hàng đợi, pub/sub: chỗ gửi ghép với chỗ nhận cùng hệ và tên (topic
    /// Kafka, queue hay exchange RabbitMQ, kênh Redis, task Celery).
    Queue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub from: String,
    pub to: String,
    pub file: String,
    pub line: usize,
    pub spec: String,
    pub via: Via,
    /// Số lần gặp trong file. Luôn 1 với import.
    pub count: usize,
    pub reason: Reason,
    /// Luật bị vi phạm, viết lại thành một dòng.
    pub rule: String,
    pub status: Status,
    pub hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// Cần cấu hình build mà bộ kiểm chứng chưa đọc.
    Alias,
    /// Đường dẫn trông đúng dạng nhưng không có gì ở đó.
    Missing,
    /// Lời gọi hay chỗ đăng ký gRPC không ghép được: tên service có ở nhiều
    /// package `.proto`, hay bước tìm phía bên kia chạm trần.
    Rpc,
    /// Chỗ gửi, nhận qua hàng đợi không ghép được: tên không cố định, tên
    /// lấy từ biến môi trường chưa lần ra, bên nhận chỉ có trong test.
    Queue,
}

impl UnresolvedReason {
    pub fn describe(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (UnresolvedReason::Alias, _) => "alias cần cấu hình build, chưa được hỗ trợ",
            (UnresolvedReason::Missing, Lang::C) => {
                "không tìm thấy header, có thể nằm trong include path của build"
            }
            (UnresolvedReason::Missing, _) => "không tìm thấy file nào ở đường dẫn này",
            (UnresolvedReason::Rpc, _) => "không ghép được lời gọi gRPC với server",
            (UnresolvedReason::Queue, _) => "không ghép được chỗ gửi, nhận qua hàng đợi",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub file: String,
    pub line: usize,
    pub spec: String,
    pub lang: Lang,
    pub reason: UnresolvedReason,
}

/// Một cái tên mà kiến trúc nói là phải có trong mã, nhưng không tìm thấy.
///
/// Thường nghĩa là khai báo đã lạc hậu: bước bị đổi tên, năng lực bị gỡ, loại
/// lỗi không còn. Sơ đồ nói một đằng, mã làm một nẻo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missing {
    pub name: String,
    pub source: String,
    pub line: usize,
    pub hint: String,
}

/// Một kiểm tồn tại đã "pass" nhưng không chứng minh được gì.
///
/// Cái tên có mặt ở khắp repo, nên gặp nó ở đúng component chỉ là trùng hợp.
/// Im lặng ở đây nghĩa là "đã kiểm, không sao" — và đó là nói sai. Bước tên
/// `run` hay `init` là ví dụ hay gặp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vacuous {
    pub name: String,
    pub source: String,
    pub line: usize,
    /// Số file có tên này, trên tổng số file đã dò ký hiệu.
    pub files: usize,
    pub of: usize,
    /// Con số trên đếm chỗ **khai báo**, không phải mọi lần gặp. Khi đúng thì
    /// cảnh báo nặng hơn: cái tên này được đặt ra ở khắp nơi, chứ không chỉ
    /// được nhắc tới ở khắp nơi.
    pub by_declaration: bool,
}

/// Tên có mặt ở từ `1/4` số file trở lên thì coi như không mang thông tin.
const VACUOUS_SHARE: (usize, usize) = (1, 4);
/// Repo nhỏ thì tên nào cũng "có mặt khắp nơi". Dưới mức này không kết luận gì.
const VACUOUS_MIN_FILES: usize = 5;

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub files_scanned: usize,
    /// File không thuộc component nào. Không phải lỗi, nhưng nhiều quá thì sơ
    /// đồ đang không phản ánh repo.
    pub files_unmapped: usize,
    pub checked: Vec<String>,
    pub unchecked: Vec<String>,
    pub violations: Vec<Violation>,
    pub unresolved: Vec<Unresolved>,
    /// File khớp nhiều component một lúc — mẫu `at` chồng nhau.
    pub ambiguous: Vec<(String, Vec<String>)>,
    /// Số file đã quét theo từng ngôn ngữ, để biết bộ kiểm chứng thật sự nhìn
    /// thấy những phần nào của repo.
    pub by_lang: Vec<(Lang, usize)>,
    /// Chỉ quét một phần repo, nên các con số tổng không đầy đủ.
    pub partial: bool,
    /// Tên được khai báo nhưng không có trong mã.
    pub missing: Vec<Missing>,
    /// Kiểm tồn tại đã pass nhưng không chứng minh được gì.
    pub vacuous: Vec<Vacuous>,
    /// File quá lớn để dò, kèm kích thước. Không im lặng bỏ qua: đó là mã
    /// đang không được kiểm.
    pub oversized: Vec<(String, u64)>,
    /// Cách kiểm đã khai báo nhưng chưa có biên nhận còn hiệu lực. Nói cách
    /// khác: lời hứa chưa được chứng minh.
    pub unproven: Vec<(String, crate::verify::Stale)>,
    /// File triển khai không đọc được: hỏng, quá lớn, trỏ ra ngoài dự án.
    pub deploy_unread: Vec<deploy::Unread>,
    /// Service có `runs` mà không biết nó khởi chạy mã nào: `runs` khi đó chỉ
    /// được kiểm theo cạnh import, không theo điểm vào.
    pub entry_unknown: Vec<(String, String)>,
    /// Service trong compose chưa có `service` nào khai: `(file, dòng, tên)`.
    pub services_undeclared: Vec<(String, usize, String)>,
    /// `link` có hiệu lực mà không cấu hình nào cho thấy cạnh đó:
    /// `(nguồn, đích, dòng trong file luật)`.
    pub links_unseen: Vec<(String, String, usize)>,
    /// `link` có hiệu lực mà NetworkPolicy chặn mọi đường giữa hai service:
    /// `(nguồn, đích, dòng trong file luật)`.
    pub links_blocked: Vec<(String, String, usize)>,
    /// Đã đọc cấu hình triển khai của repo trong lần kiểm này.
    pub deploy_checked: bool,
}

impl Report {
    pub fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

/// Trên mức này thì không dò nữa.
///
/// Bộ dò giữ cả dãy token trong bộ nhớ, nên một bundle đã minify 32 MB ngốn
/// hơn một gigabyte. Công cụ này chạy trong vòng lặp của agent và trong CI có
/// hạn mức bộ nhớ, nên con số đó là một lần bị giết chứ không phải một lần
/// chạy chậm.
///
/// Một file nguồn do người viết gần như không bao giờ tới mức này, còn file
/// sinh ra ở cỡ đó thì không chứa thông tin kiến trúc nào.
pub const MAX_FILE_BYTES: u64 = 4 << 20;

/// Mọi file thuộc ngôn ngữ đã hỗ trợ, tương đối so với gốc dự án.
pub fn source_files(root: &Path, extra_ignores: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    walk(root, "", extra_ignores, &mut files);
    files.retain(|f| Lang::for_path(f).is_some());
    files
}

pub fn check(root: &Path, doc: &Document, extra_ignores: &[String]) -> Report {
    let deploy = (!doc.services.is_empty()).then(|| deploy_files(root, extra_ignores));
    check_with(root, doc, &source_files(root, extra_ignores), false, deploy)
}

/// Kiểm một tập file cho trước. `partial` đánh dấu rằng đây không phải toàn
/// repo, để báo cáo không in ra những con số tổng gây hiểu nhầm.
///
/// Chia nhỏ được vì luật phụ thuộc và luật `api` đều xét **từng file một**:
/// import của một file không phụ thuộc vào file nào khác. `runs` của service
/// cũng vậy. Phần còn lại của tầng hạ tầng (điểm vào, cạnh mạng) đọc cấu hình
/// của cả repo, nên chỉ chạy khi một file trong tập có thể là thứ cấu hình trỏ
/// tới ([`may_touch_deploy`]).
pub fn check_files(root: &Path, doc: &Document, files: &[String], partial: bool) -> Report {
    let wanted =
        !doc.services.is_empty() && (!partial || files.iter().any(|f| may_touch_deploy(doc, f)));
    let deploy = wanted.then(|| deploy_files(root, &crate::read_ignores(root)));
    check_with(root, doc, files, partial, deploy)
}

/// Đổi file này thì phần hạ tầng phải đọc lại cấu hình: chính file cấu hình
/// (compose, Dockerfile, `.env`, script); file không phải mã mà cấu hình có
/// thể trỏ tới (`env_file: config/api.env`, script không đuôi,
/// `dockerfile: Dockerfile-api`); file mã không thuộc component nào, vì đó là
/// chỗ của điểm vào (`main.py` ở gốc); và file khai gói Python, vì nó đổi chỗ
/// tìm module. Tài liệu và ảnh thì không.
pub fn may_touch_deploy(doc: &Document, path: &str) -> bool {
    const INERT: &[&str] = &[
        "md", "rst", "adoc", "png", "jpg", "jpeg", "gif", "svg", "ico", "webp", "pdf",
    ];
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    if deploy::touches_deploy(path) || matches!(name, "pyproject.toml" | "setup.cfg" | "setup.py") {
        return true;
    }
    if !scan::is_supported(path) {
        return !ext.is_some_and(|e| INERT.contains(&e.as_str()));
    }
    owners_of(doc, path).is_empty()
}

/// File `.proto` của repo.
pub fn proto_files(root: &Path, extra_ignores: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    walk(root, "", extra_ignores, &mut files);
    files.retain(|f| f.ends_with(".proto"));
    files
}

/// File cấu hình triển khai của sản phẩm trong repo.
pub fn deploy_files(root: &Path, extra_ignores: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    walk(root, "", extra_ignores, &mut files);
    files.retain(|f| deploy::is_deploy_file(f));
    files
}

fn check_with(
    root: &Path,
    doc: &Document,
    files: &[String],
    partial: bool,
    deploy: Option<Vec<String>>,
) -> Report {
    let mut report = Report {
        partial,
        ..Report::default()
    };

    for (name, _) in classify(doc) {
        if opted_in(doc, &name) {
            report.checked.push(name);
        } else {
            report.unchecked.push(name);
        }
    }

    let resolver = Resolver::new(root);
    let mut counts: Vec<(Lang, usize)> = Vec::new();

    // Ký hiệu cần tìm, và nhóm api chứa mỗi ký hiệu. Một ký hiệu có thể thuộc
    // nhiều nhóm.
    let watched = doc.watched_symbols();
    let tables = doc.watched_tables();
    let dedup = |pairs: &[(&str, &str)]| {
        let mut v: Vec<String> = pairs.iter().map(|(_, s)| (*s).to_string()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    let needles = dedup(&watched);
    let table_needles = dedup(&tables);
    // Kỳ vọng từ flow, contract, boundary và invariant cũng là tên cần dò,
    // nên gộp chung một lượt quét với nhóm `api`.
    let expectations: Vec<_> = doc
        .expectations()
        .into_iter()
        .filter(|e| e.status.is_enforced())
        .collect();
    let mut needle_set: crate::scan::Needles = needles.iter().map(String::as_str).collect();
    needle_set.extend(expectations.iter().map(|e| e.name.as_str()));
    // Tên nào xuất hiện ở component nào, để biết cái gì thiếu và cái gì lọt ra.
    let mut sightings: std::collections::BTreeMap<&str, Vec<Sighting>> =
        std::collections::BTreeMap::new();
    // Mẫu số cho việc xét một cái tên có quá phổ biến để nói lên điều gì không.
    let mut symbol_scanned = 0usize;
    // Hai đầu của một cạnh liên ngôn ngữ, ghép lại sau khi quét xong: một bên
    // khai báo `extern "C" { fn x }`, bên kia định nghĩa `x`.
    let mut ffi_calls: Vec<(String, String, Crossing)> = Vec::new();
    // Import có thể đang nạp module mở rộng; chọn cái nào được ghép sau khi
    // thấy hết, vì cùng một tên có thể được nạp từ nhiều package.
    let mut natives: Vec<(String, String, usize, NativeImport)> = Vec::new();
    let mut rpc_calls: Vec<RpcSite> = Vec::new();
    let mut rpc_serves: Vec<RpcSite> = Vec::new();
    let mut rpc_impls: Vec<RpcSite> = Vec::new();
    let mut queue_sites: Vec<QueueSite> = Vec::new();
    // Tên máy viết thẳng trong mã: chỉ cần khi luật có `service` (TM-32).
    let mut code_hosts: Vec<(String, String, scan::hosts::CodeHost)> = Vec::new();
    let mut ffi_provides: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    fn as_refs(v: &[String]) -> Vec<&str> {
        v.iter().map(String::as_str).collect()
    }

    for path in files {
        let Some(lang) = Lang::for_path(path) else {
            continue;
        };
        report.files_scanned += 1;
        match counts.iter_mut().find(|(l, _)| *l == lang) {
            Some((_, n)) => *n += 1,
            None => counts.push((lang, 1)),
        }

        let owners = owners_of(doc, path);
        if ambiguous(doc, path, &owners) {
            report.ambiguous.push((path.clone(), owners.clone()));
        }
        let Some(from) = owners.first() else {
            report.files_unmapped += 1;
            continue;
        };

        // Không phải UTF-8 (`# -*- coding: latin-1 -*-`) vẫn là mã chạy được:
        // đọc thay thế byte lạ, không bỏ cả file (TM-32).
        let Ok(bytes) = std::fs::read(root.join(path)) else {
            continue;
        };
        let src = String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
        if src.len() as u64 > MAX_FILE_BYTES {
            report.oversized.push((path.clone(), src.len() as u64));
            continue;
        }
        symbol_scanned += 1;
        let src = scan::source_view(path, &src);
        let scan = lang.scan_all(&src, &needle_set, &as_refs(&table_needles));
        for import in &scan.imports {
            // Module không có mã Python nào trong repo có thể là module mở
            // rộng viết bằng Rust hay C++: một đầu của cạnh liên ngôn ngữ.
            for native in inspect(&resolver, doc, lang, path, from, import, &mut report) {
                natives.push((from.clone(), path.clone(), import.line, native));
            }
        }
        inspect_hits(
            doc,
            &watched,
            path,
            from,
            &scan.symbols,
            Via::Symbol,
            &mut report,
        );
        inspect_hits(
            doc,
            &tables,
            path,
            from,
            &scan.tables,
            Via::Table,
            &mut report,
        );

        for call in &scan.foreign.calls {
            ffi_calls.push((from.clone(), path.clone(), call.clone()));
        }
        for provided in &scan.foreign.provides {
            // `TORCH_EXTENSION_NAME` thành tên thật, hoặc bỏ nếu không biết chắc.
            let Some(key) = resolver.bind_extension(path, &provided.symbol) else {
                continue;
            };
            // Cùng một tên được hai component cung cấp thì không biết chọn
            // đâu, nên bỏ cả hai: đoán sai sinh ra một cạnh không có thật.
            ffi_provides
                .entry(key)
                .and_modify(|owner| {
                    if owner != from {
                        owner.clear();
                    }
                })
                .or_insert_with(|| from.clone());
        }

        if !grpc::generated_stub_path(path) {
            let site = |s: &grpc::Site| RpcSite {
                from: from.clone(),
                file: path.clone(),
                lang,
                site: s.clone(),
            };
            rpc_calls.extend(scan.rpc.calls.iter().map(site));
            rpc_serves.extend(scan.rpc.serves.iter().map(site));
            rpc_impls.extend(scan.rpc.implements.iter().map(site));
        }
        queue_sites.extend(scan.queues.sites.iter().map(|s| QueueSite {
            from: from.clone(),
            file: path.clone(),
            lang,
            site: s.clone(),
        }));
        if scan.queues.cut {
            report.unresolved.push(Unresolved {
                file: path.clone(),
                line: 0,
                spec: format!(
                    "quá {} chỗ gửi, nhận qua hàng đợi trong file; phần sau không xét",
                    scan::queue::MAX_PER_FILE
                ),
                lang,
                reason: UnresolvedReason::Queue,
            });
        }

        if !doc.services.is_empty() && !scan::hosts::skipped_path(path) {
            code_hosts.extend(
                scan.hosts
                    .hosts
                    .iter()
                    .map(|h| (from.clone(), path.clone(), h.clone())),
            );
            if scan.hosts.cut {
                report.deploy_unread.push(deploy::Unread {
                    file: path.clone(),
                    why: format!(
                        "quá {} tên máy trong chuỗi của file; phần sau không xét",
                        scan::hosts::MAX_PER_FILE
                    ),
                });
            }
        }

        for hit in &scan.symbols {
            if let Some(name) = needle_set.get(hit.symbol.as_str()) {
                sightings.entry(*name).or_default().push(Sighting {
                    component: from.clone(),
                    file: path.clone(),
                    line: hit.line,
                    defines: hit.defines,
                });
            }
        }
    }

    let settled = crate::resolve::settle_natives(natives.iter().map(|(_, _, _, n)| n));
    for (from, path, line, n) in &natives {
        if settled.contains(&(n.module.as_str(), n.at.as_str())) {
            let crossing = Crossing {
                symbol: format!("py:{}", n.module),
                shown: n.shown.clone(),
                line: *line,
            };
            ffi_calls.push((from.clone(), path.clone(), crossing));
        }
    }
    let paired = judge_ffi(doc, &ffi_calls, &ffi_provides, &mut report);
    // Import của module mở rộng không ra file nào, nhưng đã tìm thấy nơi định
    // nghĩa nó ở phía native: nó đã được phân giải.
    report
        .unresolved
        .retain(|u| !paired.contains(&(u.file.as_str(), u.line, u.spec.as_str())));
    judge_expectations(&expectations, &sightings, symbol_scanned, &mut report);
    let rpc_links = judge_grpc(
        root,
        doc,
        files,
        partial,
        rpc_calls,
        rpc_serves,
        rpc_impls,
        &mut report,
    );
    // Tên topic đọc từ biến môi trường: cần cấu hình triển khai để lần ra.
    let env_topics = queue_sites
        .iter()
        .any(|s| matches!(s.site.name, scan::queue::Name::Env(_)));
    // Hook sửa một file mã có chuỗi dạng tên máy: cần cấu hình để biết tên đó
    // trỏ tới service nào, nhưng không xét lại cả cấu hình (TM-34).
    let full = deploy.is_some();
    let deploy = deploy.or_else(|| {
        (!code_hosts.is_empty() || env_topics)
            .then(|| deploy_files(root, &crate::read_ignores(root)))
    });
    // Chỉ giữ giá trị của đúng các biến mà mã đọc làm tên topic (TM-19, TM-43).
    let env_words: Vec<String> = queue_sites
        .iter()
        .filter_map(|s| match &s.site.name {
            scan::queue::Name::Env(v) => Some(env_var(v).1),
            _ => None,
        })
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect();
    let loaded = deploy.map(|deploy_files| {
        let mut opts = deploy_options(doc);
        opts.env_words = env_words;
        let names: BTreeSet<&str> = code_hosts
            .iter()
            .map(|(_, _, h)| h.host.name.as_str())
            .collect();
        opts.hosts = names.into_iter().map(str::to_string).collect();
        deploy::load(root, &deploy_files, &opts)
    });
    let mut queue_sites = resolve_env(doc, queue_sites, loaded.as_ref());
    // Trong hook, phía bên kia ở file khác. Tên phía bên kia lấy từ env thì
    // không tìm được ở đây; lần kiểm cả repo thấy nó.
    if partial && !queue_sites.is_empty() {
        queue_other_side(root, doc, files, &crate::read_ignores(root), &mut queue_sites, &mut report);
    }
    let queue_links = judge_queues(doc, queue_sites, &mut report);
    if let Some(loaded) = loaded {
        let mut seen = rpc_links;
        seen.extend(queue_links);
        seen.extend(judge_code_hosts(doc, &loaded, &code_hosts, &mut report));
        if full {
            report.deploy_checked = true;
            judge_deploy(root, doc, &resolver, &loaded, &seen, &mut report);
        }
    }

    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    report.by_lang = counts;

    report
        .violations
        .sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report
        .unresolved
        .sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report.oversized.sort();
    report.unproven = unproven(root, doc, files);
    report
}

/// Cách kiểm nào chưa có biên nhận còn hiệu lực.
///
/// Chỉ **đọc** biên nhận. `check` chạy trong hook của agent sau mỗi lần sửa
/// file, nên nó không được chạy lệnh shell nào — đó là việc của
/// `archctl verify`, và chỉ khi người gõ lệnh đó.
fn unproven(root: &Path, doc: &Document, files: &[String]) -> Vec<(String, crate::verify::Stale)> {
    if doc.verifies.is_empty() {
        return Vec::new();
    }
    let names: Vec<String> = doc
        .verifies
        .iter()
        .filter(|v| v.meta.status.is_enforced())
        .map(|v| v.name.clone())
        .collect();
    let receipts = crate::verify::parse(
        &std::fs::read_to_string(root.join(crate::verify::RECEIPTS_FILE)).unwrap_or_default(),
    );
    let changed = crate::verify::last_source_change(root, files);
    crate::verify::stale(&names, &receipts, &crate::arch_hash(doc), changed)
}

/// Ghép hai đầu của một lời gọi qua ranh giới ngôn ngữ. Trả về những lời gọi
/// đã ghép được, dạng `(file, dòng, cách viết)`.
///
/// Tên nào không ghép được thì im lặng bỏ qua: đó là hàm của thư viện hệ
/// thống, hoặc — khi chỉ quét một phần repo — phía cung cấp nằm ngoài tập
/// file đang xét.
fn judge_ffi<'a>(
    doc: &Document,
    calls: &'a [(String, String, Crossing)],
    provides: &std::collections::BTreeMap<String, String>,
    report: &mut Report,
) -> std::collections::HashSet<(&'a str, usize, &'a str)> {
    let mut paired = std::collections::HashSet::new();
    for (from, file, call) in calls {
        let Some(to) = scan::foreign::provider(provides, &call.symbol) else {
            continue;
        };
        paired.insert((file.as_str(), call.line, call.shown.as_str()));
        if to == from {
            continue;
        }
        pulled_in(
            doc,
            from,
            to,
            file,
            call.line,
            &call.shown,
            Via::Ffi,
            report,
        );
        let Some((reason, rule, status)) = judge(doc, from, to) else {
            continue;
        };
        report.violations.push(Violation {
            from: from.clone(),
            to: to.clone(),
            file: file.clone(),
            line: call.line,
            spec: call.shown.clone(),
            via: Via::Ffi,
            count: 1,
            reason,
            rule,
            status,
            hint: hint_for(doc, from, to, reason, Via::Ffi),
        });
    }
    paired
}

/// Một chỗ gọi stub hay đăng ký server gRPC, kèm component và file của nó.
#[derive(Debug, Clone)]
struct RpcSite {
    from: String,
    file: String,
    lang: Lang,
    site: grpc::Site,
}

/// Trần byte đọc thêm khi tìm phía bên kia của lời gọi gRPC trong hook (TM-13).
const MAX_RPC_SEARCH_BYTES: usize = 64 << 20;

/// Cạnh gRPC: ghép chỗ gọi với chỗ đăng ký theo `package.Service`, rồi xét
/// `allow` giữa component và, khi file luật đã khai `service`, `link` giữa
/// service. Trả các cặp service có lời gọi gRPC, để `link` của chúng không bị
/// báo là "không thấy trong cấu hình".
///
/// Lời gọi không có server nào trong repo (collector của OpenTelemetry, API
/// của Google) được coi như import gói ngoài: không vi phạm, không cảnh báo.
#[allow(clippy::too_many_arguments)]
fn judge_grpc(
    root: &Path,
    doc: &Document,
    files: &[String],
    partial: bool,
    mut calls: Vec<RpcSite>,
    mut serves: Vec<RpcSite>,
    impls: Vec<RpcSite>,
    report: &mut Report,
) -> std::collections::BTreeSet<(String, String)> {
    if calls.is_empty() && serves.is_empty() && impls.is_empty() {
        return std::collections::BTreeSet::new();
    }
    let ignores = crate::read_ignores(root);
    let mut index = grpc::ProtoIndex::default();
    for p in proto_files(root, &ignores) {
        match deploy::read_in(root, &p, MAX_FILE_BYTES) {
            Ok(text) => index.add(&p, &grpc::read_proto(&text)),
            Err(why) if why.starts_with("quá") => report.oversized.push((p, MAX_FILE_BYTES + 1)),
            Err(_) => {}
        }
    }
    if index.truncated {
        report.unresolved.push(Unresolved {
            file: ".proto".into(),
            line: 0,
            spec: format!(
                "quá {} service trong các file .proto; phần sau không ghép",
                grpc::MAX_INDEX
            ),
            lang: Lang::Python,
            reason: UnresolvedReason::Rpc,
        });
    }
    // Mỗi (service, nguồn gốc) tra một lần: một file có thể có hàng chục
    // nghìn chỗ gọi (TM-13).
    let mut keys: HashMap<(String, grpc::Origin), grpc::Key> = HashMap::new();
    let mut key_of = |s: &grpc::Site| {
        keys.entry((s.service.clone(), s.origin.clone()))
            .or_insert_with(|| index.key(s))
            .clone()
    };
    // Hàm `NewXClient` không phải gRPC thì không đáng đi tìm phía bên kia.
    calls.retain(|s| !matches!(key_of(&s.site), grpc::Key::NotRpc));
    serves.retain(|s| !matches!(key_of(&s.site), grpc::Key::NotRpc));
    let impls: Vec<RpcSite> = impls
        .into_iter()
        .filter(|s| !matches!(key_of(&s.site), grpc::Key::NotRpc))
        .collect();
    if partial && !(calls.is_empty() && serves.is_empty() && impls.is_empty()) {
        other_side(
            root,
            doc,
            files,
            &ignores,
            &impls,
            &mut calls,
            &mut serves,
            report,
        );
    }

    let ambiguous = |s: &RpcSite, candidates: Vec<String>, report: &mut Report| {
        report.unresolved.push(Unresolved {
            file: s.file.clone(),
            line: s.site.line,
            spec: format!("{} (service có ở {})", s.site.shown, candidates.join(", ")),
            lang: s.lang,
            reason: UnresolvedReason::Rpc,
        });
    };
    let mut servers: grpc::Servers = grpc::Servers::new();
    // Khoá chỉ có server trong file test hay mock.
    let mut fake: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for s in &serves {
        match key_of(&s.site) {
            grpc::Key::Found(key) if grpc::fake_server_path(&s.file) => {
                fake.insert(key);
            }
            grpc::Key::Found(key) => {
                servers.entry(key).or_default().insert(s.from.clone());
            }
            grpc::Key::Ambiguous(c) => ambiguous(s, c, report),
            grpc::Key::NotRpc => {}
        }
    }
    let mut edges: Vec<(&RpcSite, String, String)> = Vec::new();
    for c in &calls {
        let key = match key_of(&c.site) {
            grpc::Key::Found(key) => key,
            grpc::Key::Ambiguous(cands) => {
                ambiguous(c, cands, report);
                continue;
            }
            grpc::Key::NotRpc => continue,
        };
        let Some(targets) = servers.get(&key) else {
            // Server chỉ có trong test hay mock: không biết service chạy thật
            // ở đâu, nên nói ra thay vì coi như gói ngoài.
            if fake.contains(&key) && !grpc::fake_server_path(&c.file) {
                report.unresolved.push(Unresolved {
                    file: c.file.clone(),
                    line: c.site.line,
                    spec: format!(
                        "{} ({key}: chỉ thấy server trong file test hay mock)",
                        c.site.shown
                    ),
                    lang: c.lang,
                    reason: UnresolvedReason::Rpc,
                });
            }
            continue;
        };
        for to in targets.iter().filter(|t| **t != c.from) {
            edges.push((c, to.clone(), key.clone()));
        }
    }
    // Có chỗ cài đặt server mà không thấy chỗ đăng ký nào: phía server đang
    // bị giấu (gán hàm đăng ký qua biến, `getattr`…), và mọi cạnh tới nó sẽ
    // biến mất. Nói ra thay vì im lặng (TM-12).
    let mut said: HashSet<String> = HashSet::new();
    for s in impls.iter().filter(|s| !grpc::fake_server_path(&s.file)) {
        let grpc::Key::Found(key) = key_of(&s.site) else {
            continue;
        };
        if !servers.contains_key(&key) && !fake.contains(&key) && said.insert(key.clone()) {
            report.unresolved.push(Unresolved {
                file: s.file.clone(),
                line: s.site.line,
                spec: format!(
                    "{} ({key}: cài đặt server mà không thấy chỗ đăng ký)",
                    s.site.shown
                ),
                lang: s.lang,
                reason: UnresolvedReason::Rpc,
            });
        }
    }
    for (c, to, key) in &edges {
        let shown = format!("{} ({})", c.site.shown, key);
        let Some((reason, rule, status)) = judge(doc, &c.from, to) else {
            continue;
        };
        report.violations.push(Violation {
            from: c.from.clone(),
            to: to.clone(),
            file: c.file.clone(),
            line: c.site.line,
            spec: shown,
            via: Via::Rpc,
            count: 1,
            reason,
            rule,
            status,
            hint: hint_for(doc, &c.from, to, reason, Via::Rpc),
        });
    }
    let hops: Vec<Hop> = edges
        .iter()
        .map(|(c, to, key)| Hop {
            from: c.from.clone(),
            file: c.file.clone(),
            line: c.site.line,
            shown: c.site.shown.clone(),
            to: to.clone(),
            key: key.clone(),
        })
        .collect();
    judge_cross_links(doc, &hops, Via::Rpc, report)
}

/// Một cạnh giữa hai component qua gRPC hay hàng đợi, để xét `link`.
struct Hop {
    from: String,
    file: String,
    line: usize,
    shown: String,
    to: String,
    /// `package.Service` hay `kafka:orders`.
    key: String,
}

/// Một chỗ gửi hay nhận qua hàng đợi, kèm component và file của nó.
#[derive(Debug, Clone)]
struct QueueSite {
    from: String,
    file: String,
    lang: Lang,
    site: scan::queue::Site,
}

/// Trần số chỗ gửi, nhận xét trong một lần kiểm (TM-42).
const MAX_QUEUE_SITES: usize = 200_000;

/// Cạnh qua hàng đợi: ghép chỗ gửi với chỗ nhận cùng hệ và tên, rồi xét
/// `allow` giữa component và, khi file luật đã khai `service`, `link` giữa
/// service. Tên không cố định thì báo "chưa ghép", không đoán (TM-40). Bên
/// gửi không có bên nhận nào trong repo như gói ngoài.
fn judge_queues(
    doc: &Document,
    sites: Vec<QueueSite>,
    report: &mut Report,
) -> std::collections::BTreeSet<(String, String)> {
    use scan::queue::{Name, Role};
    if sites.is_empty() {
        return std::collections::BTreeSet::new();
    }
    let mut sites = sites;
    if sites.len() > MAX_QUEUE_SITES {
        report.unresolved.push(Unresolved {
            file: sites[MAX_QUEUE_SITES].file.clone(),
            line: 0,
            spec: format!("quá {MAX_QUEUE_SITES} chỗ gửi, nhận qua hàng đợi; phần sau không ghép"),
            lang: sites[MAX_QUEUE_SITES].lang,
            reason: UnresolvedReason::Queue,
        });
        sites.truncate(MAX_QUEUE_SITES);
    }
    // Chỗ không ghép được trong file test hay mock: mock producer, consumer
    // giả, không phải đường dữ liệu thật; không báo.
    let unresolved = |s: &QueueSite, why: String, report: &mut Report| {
        if grpc::fake_server_path(&s.file) {
            return;
        }
        report.unresolved.push(Unresolved {
            file: s.file.clone(),
            line: s.site.line,
            spec: format!("{} ({}: {why})", s.site.shown, s.site.system.as_str()),
            lang: s.lang,
            reason: UnresolvedReason::Queue,
        });
    };
    let mut receivers = scan::queue::Index::default();
    // Bên nhận chỉ có trong file test hay mock (TM-41).
    let mut fake = scan::queue::Index::default();
    for s in sites.iter().filter(|s| s.site.role == Role::Receive) {
        let into = if grpc::fake_server_path(&s.file) { &mut fake } else { &mut receivers };
        if into.add(&s.site, &s.file, &s.from) {
            continue;
        }
        match &s.site.name {
            Name::Fixed(_) | Name::Pattern(_) | Name::Task(_) => {}
            Name::Env(var) => unresolved(s, format!("tên lấy từ biến môi trường `{var}` không khai trong cấu hình triển khai"), report),
            Name::Dynamic(e) => unresolved(s, format!("tên không cố định `{e}`"), report),
        }
    }
    if receivers.patterns_cut {
        report.unresolved.push(Unresolved {
            file: ".".into(),
            line: 0,
            spec: format!(
                "quá {} mẫu psubscribe; mẫu sau không ghép",
                scan::queue::MAX_PATTERNS
            ),
            lang: Lang::Python,
            reason: UnresolvedReason::Queue,
        });
    }
    // Mỗi (hệ, tên) tra một lần (TM-42).
    let mut cache: HashMap<(scan::queue::System, String), std::collections::BTreeSet<String>> = HashMap::new();
    let mut hops: Vec<Hop> = Vec::new();
    let mut said: HashSet<(String, usize)> = HashSet::new();
    for s in sites.iter().filter(|s| s.site.role == Role::Send) {
        let name = match &s.site.name {
            Name::Fixed(n) => n,
            Name::Env(var) => {
                unresolved(s, format!("tên lấy từ biến môi trường `{var}` không khai trong cấu hình triển khai"), report);
                continue;
            }
            Name::Dynamic(e) => {
                unresolved(s, format!("tên không cố định `{e}`"), report);
                continue;
            }
            Name::Pattern(_) | Name::Task(_) => continue,
        };
        let key = (s.site.system, name.clone());
        let targets = cache.entry(key).or_insert_with(|| receivers.targets(s.site.system, name));
        if targets.is_empty() {
            if !grpc::fake_server_path(&s.file)
                && !fake.targets(s.site.system, name).is_empty()
                && said.insert((s.file.clone(), s.site.line))
            {
                unresolved(s, format!("`{name}`: chỉ thấy bên nhận trong file test hay mock"), report);
            }
            continue;
        }
        let shown_key = format!("{}:{}", s.site.system.as_str(), name);
        for to in targets.iter().filter(|t| **t != s.from) {
            hops.push(Hop {
                from: s.from.clone(),
                file: s.file.clone(),
                line: s.site.line,
                shown: s.site.shown.clone(),
                to: to.clone(),
                key: shown_key.clone(),
            });
        }
    }
    for h in &hops {
        let Some((reason, rule, status)) = judge(doc, &h.from, &h.to) else {
            continue;
        };
        report.violations.push(Violation {
            from: h.from.clone(),
            to: h.to.clone(),
            file: h.file.clone(),
            line: h.line,
            spec: format!("{} ({})", h.shown, h.key),
            via: Via::Queue,
            count: 1,
            reason,
            rule,
            status,
            hint: hint_for(doc, &h.from, &h.to, reason, Via::Queue),
        });
    }
    judge_cross_links(doc, &hops, Via::Queue, report)
}

/// `q:VAR` của RabbitMQ: tiền tố và tên biến.
pub(crate) fn env_var(v: &str) -> (&'static str, String) {
    match v.split_once(':') {
        Some(("q", r)) => ("q:", r.to_string()),
        Some(("x", r)) => ("x:", r.to_string()),
        _ => ("", v.to_string()),
    }
}

/// Tên lấy từ env (`os.getenv("TOPIC")`): lần qua cấu hình của service chạy
/// component đó; mỗi giá trị là một chỗ gửi, nhận với tên cố định. Không lần
/// ra thì giữ nguyên để báo "chưa ghép".
fn resolve_env(doc: &Document, sites: Vec<QueueSite>, dep: Option<&Deploy>) -> Vec<QueueSite> {
    use scan::queue::Name;
    // Tên lấy từ env: lần qua cấu hình của service chạy component đó.
    let mut env_cache: HashMap<(String, String), Vec<String>> = HashMap::new();
    let mut expanded: Vec<QueueSite> = Vec::with_capacity(sites.len());
    for s in sites {
        let Name::Env(raw) = &s.site.name else {
            expanded.push(s);
            continue;
        };
        let (prefix, var) = env_var(raw);
        let values = env_cache
            .entry((s.from.clone(), var.clone()))
            .or_insert_with(|| env_words(doc, dep, &s.from, &var))
            .clone();
        if values.is_empty() {
            let mut t = s.clone();
            t.site.name = Name::Env(var);
            expanded.push(t);
            continue;
        }
        for v in values {
            let mut t = s.clone();
            t.site.shown = format!("{} [{var}={v}]", s.site.shown);
            t.site.name = Name::Fixed(format!("{prefix}{v}"));
            expanded.push(t);
        }
    }
    expanded
}

/// Giá trị của biến môi trường `var` ở các service chạy component `from`;
/// file luật chưa khai service nào chạy nó thì xét mọi service trong cấu
/// hình, và chỉ nhận khi chúng cùng một giá trị. Chỉ giá trị dạng tên, ở khoá
/// không mang dấu hiệu bí mật (TM-43).
fn env_words(doc: &Document, dep: Option<&Deploy>, from: &str, var: &str) -> Vec<String> {
    let Some(dep) = dep else { return Vec::new() };
    let runners: Vec<&Service> = doc
        .services
        .iter()
        .filter(|s| s.meta.status.is_enforced() && s.runs.iter().any(|r| r == from))
        .collect();
    let pick = |v: &deploy::Svc| v.words.iter().filter(|(k, _)| k == var).map(|(_, w)| w.clone()).collect::<Vec<_>>();
    let mut out: Vec<String> = if runners.is_empty() {
        let all: BTreeSet<String> = dep.services.iter().flat_map(pick).collect();
        if all.len() == 1 { all.into_iter().collect() } else { Vec::new() }
    } else {
        dep.services
            .iter()
            .filter(|v| runners.iter().any(|s| own_sources(s).any(|src| matches_source(src, &v.key))))
            .flat_map(pick)
            .collect()
    };
    out.sort();
    out.dedup();
    out
}

/// Trong hook chỉ có vài file: phía bên kia của chỗ gửi, nhận nằm ở file
/// khác. Tìm nhắm một lượt: lọc file theo các từ trong nó (tên topic, tên
/// hàm task), chỉ lex file khớp, trần byte chung (TM-42).
fn queue_other_side(
    root: &Path,
    doc: &Document,
    files: &[String],
    ignores: &[String],
    sites: &mut Vec<QueueSite>,
    report: &mut Report,
) {
    use scan::queue::{Name, Role};
    // Tên mà bên nhận ở file khác phải mang (của chỗ gửi ở đây), và tên mà
    // bên gửi ở file khác phải mang (của chỗ nhận ở đây). Chỉ lấy đúng phía
    // bên kia: chỗ gửi khác cùng topic không phải việc của lượt này.
    let bare = |n: &str| n.strip_prefix("q:").or_else(|| n.strip_prefix("x:")).unwrap_or(n).to_string();
    let last = |n: &str| n.rsplit_once('.').map_or(n, |(_, l)| l).to_string();
    let mut want_recv: HashSet<String> = HashSet::new();
    let mut want_send: HashSet<String> = HashSet::new();
    let mut patterns: Vec<String> = Vec::new();
    for s in sites.iter() {
        match (&s.site.role, &s.site.name) {
            (Role::Send, Name::Fixed(n)) => {
                want_recv.insert(bare(n));
                // `shop.tasks.charge` gửi tới `def charge`.
                want_recv.insert(last(n));
            }
            (Role::Receive, Name::Fixed(n)) => {
                want_send.insert(bare(n));
            }
            (Role::Receive, Name::Task(f)) => {
                want_send.insert(f.clone());
            }
            (Role::Receive, Name::Pattern(p)) => patterns.push(p.clone()),
            _ => {}
        }
    }
    if want_recv.is_empty() && want_send.is_empty() && patterns.is_empty() {
        return;
    }
    // Mẫu `psubscribe` của file đổi: lọc file theo tiền tố chữ cố định của mẫu,
    // tra bảng băm theo từng độ dài tiền tố, không chạy glob trên từng từ của
    // repo (TM-42). Mẫu không có tiền tố, hay quá nhiều mẫu, thì lọc theo chữ
    // `publish`. Glob chỉ chạy trên tên của chỗ gửi trong file khớp, có trần.
    const MAX_GLOB_PATTERNS: usize = 64;
    let few = patterns.len() <= MAX_GLOB_PATTERNS;
    let prefixes: HashSet<&str> = patterns.iter().map(|p| scan::queue::glob_prefix(p)).collect();
    let loose = !patterns.is_empty() && (!few || prefixes.contains(""));
    let lengths: BTreeSet<usize> = if few { prefixes.iter().map(|p| p.len()).collect() } else { BTreeSet::new() };
    let by_prefix = |w: &str| {
        lengths
            .iter()
            .any(|&l| w.len() >= l && w.is_char_boundary(l) && prefixes.contains(&w[..l]))
    };
    // Tên bên nhận cần tìm ít thì mới khớp từng mẫu của file khác với chúng.
    let few_names = want_recv.len() <= 256;
    // Chỗ gửi Redis ở đây có thể tới bên nhận theo mẫu (`psubscribe("orders.*")`)
    // ở file khác: file đó không chứa đúng tên kênh, nên lọc theo chữ
    // `psubscribe` thay vì bỏ qua nó.
    let redis_send = sites.iter().any(|s| {
        s.site.role == Role::Send && s.site.system == scan::queue::System::Redis && matches!(s.site.name, Name::Fixed(_))
    });
    let scanned: HashSet<&str> = files.iter().map(String::as_str).collect();
    let mut budget = MAX_RPC_SEARCH_BYTES;
    let mut found: Vec<QueueSite> = Vec::new();
    for f in source_files(root, ignores) {
        let Some(lang) = Lang::for_path(&f).filter(|l| matches!(l, Lang::Python | Lang::Go | Lang::TypeScript)) else {
            continue;
        };
        if scanned.contains(f.as_str()) {
            continue;
        }
        let Some(from) = owners_of(doc, &f).into_iter().next() else {
            continue;
        };
        let Ok(text) = deploy::read_in(root, &f, MAX_FILE_BYTES) else {
            continue;
        };
        if text.len() > budget {
            report.unresolved.push(Unresolved {
                file: f.clone(),
                line: 0,
                spec: format!(
                    "tìm phía bên kia của chỗ gửi, nhận qua hàng đợi dừng ở trần {} MB",
                    MAX_RPC_SEARCH_BYTES >> 20
                ),
                lang,
                reason: UnresolvedReason::Queue,
            });
            break;
        }
        budget -= text.len();
        let words = |t: &str| {
            (loose && (t.contains("publish") || t.contains("Publish")))
                || (redis_send && (t.contains("psubscribe") || t.contains("PSubscribe") || t.contains("pSubscribe")))
                || t
                    .split(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | '-' | ':' | '/')))
                    .filter(|w| !w.is_empty())
                    .any(|w| {
                        want_recv.contains(w)
                            || want_send.contains(w)
                            || want_send.contains(&last(w))
                            || (few && by_prefix(w))
                    })
        };
        if !words(&text) {
            continue;
        }
        let view = scan::source_view(&f, &text);
        let toks = lang.tokens(&view);
        let imports = lang.imports(&view);
        for site in scan::queue::scan(&toks, lang, &imports).sites {
            let keep = match (&site.role, &site.name) {
                (Role::Receive, Name::Fixed(n)) => want_recv.contains(&bare(n)),
                (Role::Receive, Name::Task(func)) => want_recv.contains(func),
                (Role::Receive, Name::Pattern(p)) => !few_names || want_recv.iter().any(|n| scan::queue::glob(p, n)),
                (Role::Send, Name::Fixed(n)) => {
                    want_send.contains(&bare(n))
                        || want_send.contains(&last(n))
                        || (!patterns.is_empty() && (!few || patterns.iter().any(|p| scan::queue::glob(p, n))))
                }
                _ => false,
            };
            if keep {
                found.push(QueueSite { from: from.clone(), file: f.clone(), lang, site });
            }
        }
    }
    sites.extend(found);
}

/// Trong hook chỉ có vài file: phía bên kia của lời gọi gRPC nằm ở file khác.
/// Tìm nhắm một lượt: đọc file Python, Go (trần byte), quét định danh một lượt
/// mỗi file và tra bảng băm (`add_XServicer_to_server`, `RegisterXServer`,
/// `X_ServiceDesc`, `XStub`, `NewXClient`), chỉ lex file khớp (TM-13).
#[allow(clippy::too_many_arguments)]
fn other_side(
    root: &Path,
    doc: &Document,
    files: &[String],
    ignores: &[String],
    impls: &[RpcSite],
    calls: &mut Vec<RpcSite>,
    serves: &mut Vec<RpcSite>,
    report: &mut Report,
) {
    // Server của service được gọi, hay được cài đặt ở đây mà đăng ký ở file khác.
    // So tên không phân biệt hoa thường: Rust viết service `KV` thành `Kv`.
    let want_servers: HashSet<String> = calls
        .iter()
        .chain(impls)
        .map(|c| grpc::norm(&c.site.service))
        .collect();
    let want_callers: HashSet<String> =
        serves.iter().map(|s| grpc::norm(&s.site.service)).collect();
    let scanned: HashSet<&str> = files.iter().map(String::as_str).collect();
    let mut budget = MAX_RPC_SEARCH_BYTES;
    for f in source_files(root, ignores) {
        let Some(lang) = Lang::for_path(&f).filter(|l| !matches!(l, Lang::C)) else {
            continue;
        };
        if scanned.contains(f.as_str()) {
            continue;
        }
        let Some(from) = owners_of(doc, &f).into_iter().next() else {
            continue;
        };
        let Ok(text) = deploy::read_in(root, &f, MAX_FILE_BYTES) else {
            continue;
        };
        if text.len() > budget {
            report.unresolved.push(Unresolved {
                file: f.clone(),
                line: 0,
                spec: format!(
                    "tìm phía bên kia của lời gọi gRPC dừng ở trần {} MB",
                    MAX_RPC_SEARCH_BYTES >> 20
                ),
                lang,
                reason: UnresolvedReason::Rpc,
            });
            return;
        }
        budget -= text.len();
        if grpc::generated_stub_path(&f) || !mentions(&text, &want_servers, &want_callers) {
            continue;
        }
        let found = grpc::scan(&lang.tokens(&scan::source_view(&f, &text)), lang);
        let site = |s: &grpc::Site| RpcSite {
            from: from.clone(),
            file: f.clone(),
            lang,
            site: s.clone(),
        };
        serves.extend(
            found
                .serves
                .iter()
                .filter(|s| want_servers.contains(&grpc::norm(&s.service)))
                .map(site),
        );
        calls.extend(
            found
                .calls
                .iter()
                .filter(|s| want_callers.contains(&grpc::norm(&s.service)))
                .map(site),
        );
    }
}

/// Mã chạy trong service `a` có thể gọi mã chạy trong `b`: có `allow` giữa
/// hai component trong `runs` của chúng. Trong hook chỉ có vài file, nên lời
/// gọi gRPC biện minh cho `link a -> b` có thể nằm ở file khác; khi đó không
/// báo `link` là "không thấy". Lần kiểm cả repo thì thấy hết và báo đúng.
fn may_call(doc: &Document, a: &str, b: &str) -> bool {
    let runs = |name: &str| {
        doc.services
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.runs.clone())
            .unwrap_or_default()
    };
    let (from, to) = (runs(a), runs(b));
    from.iter().any(|x| {
        to.iter()
            .any(|y| x != y && matching(doc, RuleKind::Allow, x, y).is_some())
    })
}

/// File có nhắc tới phía bên kia không: một lượt qua các định danh, tra bảng
/// băm, nên tốn theo cỡ file chứ không theo số service cần tìm (TM-13). Dạng
/// generic (`"/pkg.KV/Get"`, `method_handlers_generic_handler("pkg.KV", …)`)
/// thì tên service đứng một mình.
fn mentions(text: &str, servers: &HashSet<String>, callers: &HashSet<String>) -> bool {
    let generic = [
        "generic_handler",
        "unary_",
        "stream_",
        "Invoke",
        "NewStream",
    ]
    .iter()
    .any(|g| text.contains(g));
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .any(|w| {
            let cut = |p: &str, s: &str| {
                w.strip_prefix(p)
                    .and_then(|r| r.strip_suffix(s))
                    .map(grpc::norm)
            };
            // Python, Go, mã sinh của Node (`XService`), tonic (`XServer`); Node
            // nạp `.proto` lúc chạy thì tên service đứng một mình.
            [
                cut("add_", "Servicer_to_server"),
                cut("Register", "Server"),
                cut("", "_ServiceDesc"),
                cut("", "Server"),
                cut("", "Service"),
                Some(grpc::norm(w)),
            ]
            .into_iter()
            .flatten()
            .any(|x| servers.contains(&x))
                || [
                    cut("", "Stub"),
                    cut("New", "Client"),
                    cut("", "Client"),
                    Some(grpc::norm(w)),
                ]
                .into_iter()
                .flatten()
                .any(|x| callers.contains(&x))
                || (generic
                    && (servers.contains(&grpc::norm(w)) || callers.contains(&grpc::norm(w))))
        })
}

/// Cạnh qua gRPC hay hàng đợi giữa hai component chạy ở hai service khác
/// nhau cần `link`.
/// Với mỗi service chạy phía client, phải có một service chạy phía server là
/// chính nó, hoặc có `link` tới. Không service nào chạy phía server thì không
/// biết lời gọi đi đâu, và không đòi gì.
fn judge_cross_links(
    doc: &Document,
    edges: &[Hop],
    via: Via,
    report: &mut Report,
) -> std::collections::BTreeSet<(String, String)> {
    let mut used = std::collections::BTreeSet::new();
    if doc.services.is_empty() {
        return used;
    }
    let running = |c: &str| -> Vec<&Service> {
        doc.services
            .iter()
            .filter(|s| s.meta.status.is_enforced() && s.runs.iter().any(|r| r == c))
            .collect()
    };
    let linked = |a: &str, b: &str| {
        doc.links
            .iter()
            .any(|l| l.meta.status.is_enforced() && l.from == a && l.to.iter().any(|t| t == b))
    };
    let mut reported: HashSet<(String, String, String, usize)> = HashSet::new();
    for c in edges {
        let (to, key) = (&c.to, &c.key);
        let targets = running(to);
        if targets.is_empty() {
            continue;
        }
        for s in running(&c.from) {
            if targets.iter().any(|t| t.name == s.name) {
                continue;
            }
            let mut ok = false;
            for t in targets.iter().filter(|t| linked(&s.name, &t.name)) {
                used.insert((s.name.clone(), t.name.clone()));
                ok = true;
            }
            if ok {
                continue;
            }
            let t = targets[0];
            if !reported.insert((s.name.clone(), t.name.clone(), c.file.clone(), c.line)) {
                continue;
            }
            report.violations.push(Violation {
                from: s.name.clone(),
                to: t.name.clone(),
                file: c.file.clone(),
                line: c.line,
                spec: format!("{} ({key}, {} chạy trong {})", c.shown, to, t.name),
                via,
                count: 1,
                reason: Reason::NotLinked,
                rule: format!("không có link nào cho {} -> {}", s.name, t.name),
                status: Status::Accepted,
                hint: hint_for(doc, &s.name, &t.name, Reason::NotLinked, via),
            });
        }
    }
    used
}

/// Một lần gặp một cái tên trong mã.
struct Sighting {
    component: String,
    file: String,
    line: usize,
    /// Đây là chỗ đặt ra cái tên, không phải chỗ dùng nó.
    defines: bool,
}

#[allow(clippy::too_many_arguments)]
/// Đối chiếu kỳ vọng với những gì thật sự thấy trong mã.
fn judge_expectations(
    expectations: &[crate::model::Expectation],
    sightings: &std::collections::BTreeMap<&str, Vec<Sighting>>,
    scanned: usize,
    report: &mut Report,
) {
    use crate::model::Expect;

    for want in expectations {
        let seen = sightings
            .get(want.name.as_str())
            .map(Vec::as_slice)
            .unwrap_or_default();
        match &want.expect {
            Expect::Exists { within } => {
                // Chỗ **khai báo** là bằng chứng mạnh hơn hẳn một lần gặp bất
                // kỳ: `charge` được định nghĩa trong Payment nói rằng Payment
                // có năng lực đó, còn `charge` xuất hiện trong Payment có thể
                // chỉ là một lời gọi sang chỗ khác.
                //
                // Nhưng chỉ dùng được khi tìm thấy khai báo. Bộ quét nhận
                // dạng khai báo theo từ khoá, mà không phải dạng nào cũng có
                // từ khoá — `errors Timeout` có thể là một hằng chuỗi. Không
                // thấy khai báo nào thì quay về cách đếm cũ, vì đòi hỏi một
                // bằng chứng mình không biết cách tìm là tự sinh báo động giả.
                let declared: Vec<&Sighting> = seen.iter().filter(|s| s.defines).collect();
                let by_declaration = !declared.is_empty();
                let evidence: Vec<&Sighting> = if by_declaration {
                    declared
                } else {
                    seen.iter().collect()
                };

                let ok = evidence
                    .iter()
                    .any(|s| within.is_empty() || within.contains(&s.component));
                if ok {
                    // Pass rồi, nhưng pass vì cái tên có mặt khắp nơi thì
                    // không chứng minh được gì. Phải nói ra, vì im lặng ở đây
                    // bị đọc là "đã kiểm, không sao".
                    let mut files: Vec<&str> = evidence.iter().map(|s| s.file.as_str()).collect();
                    files.sort_unstable();
                    files.dedup();
                    let (num, den) = VACUOUS_SHARE;
                    if files.len() >= VACUOUS_MIN_FILES && files.len() * den >= scanned * num {
                        report.vacuous.push(Vacuous {
                            name: want.name.clone(),
                            source: want.source.clone(),
                            line: want.line,
                            files: files.len(),
                            of: scanned,
                            by_declaration,
                        });
                    }
                    continue;
                }

                let what = if by_declaration {
                    "khai báo"
                } else {
                    "thấy"
                };
                let hint = if within.is_empty() {
                    format!(
                        "`{}` không có trong mã — sửa mã, hoặc sửa khai báo",
                        want.name
                    )
                } else if evidence.is_empty() {
                    format!("`{}` không có trong mã", want.name)
                } else {
                    // Có thật nhưng nằm sai chỗ: thông tin này cụ thể hơn hẳn
                    // "không tìm thấy", nên nói ra.
                    let mut elsewhere: Vec<&str> =
                        evidence.iter().map(|s| s.component.as_str()).collect();
                    elsewhere.sort_unstable();
                    elsewhere.dedup();
                    format!(
                        "`{}` chỉ {what} trong {}, lẽ ra phải có trong {}",
                        want.name,
                        elsewhere.join(", "),
                        within.join(", ")
                    )
                };
                report.missing.push(Missing {
                    name: want.name.clone(),
                    source: want.source.clone(),
                    line: want.line,
                    hint,
                });
            }
            Expect::Confined { to } => {
                // Gộp theo file như mọi vi phạm theo ký hiệu khác: in từng vị
                // trí một sẽ làm ngập báo cáo.
                let mut per_file: std::collections::BTreeMap<(&str, &str), (usize, usize)> =
                    std::collections::BTreeMap::new();
                // Ở đây dùng **mọi** lần gặp chứ không riêng chỗ khai báo:
                // `contain` nói rằng tên này không được *dùng* ngoài vùng.
                for s in seen {
                    if to.contains(&s.component) {
                        continue;
                    }
                    per_file
                        .entry((s.component.as_str(), s.file.as_str()))
                        .and_modify(|(_, n)| *n += 1)
                        .or_insert((s.line, 1));
                }
                for ((owner, file), (line, count)) in per_file {
                    report.violations.push(Violation {
                        from: owner.to_string(),
                        to: want.name.clone(),
                        file: file.to_string(),
                        line,
                        spec: want.name.clone(),
                        via: Via::Symbol,
                        count,
                        reason: Reason::Leaked,
                        rule: want.source.clone(),
                        status: want.status,
                        hint: format!(
                            "`{}` chỉ được nằm trong {} — ở ngoài phải chuyển thành lỗi khác",
                            want.name,
                            to.join(", ")
                        ),
                    });
                }
            }
        }
    }
    report
        .missing
        .sort_by(|a, b| (&a.source, &a.name).cmp(&(&b.source, &b.name)));
    report.missing.dedup();
    report
        .vacuous
        .sort_by(|a, b| (&a.source, &a.name).cmp(&(&b.source, &b.name)));
    report.vacuous.dedup();
}

///
/// Trả về những module mà import này nạp nhưng repo không có mã Python nào
/// cho chúng — ứng viên module mở rộng, xem
/// [`Resolver::import_targets`].
fn inspect(
    resolver: &Resolver,
    doc: &Document,
    lang: Lang,
    path: &str,
    from: &str,
    import: &scan::Import,
    report: &mut Report,
) -> Vec<NativeImport> {
    let base = resolver.resolve(lang, path, import);
    let (submodules, natives) = resolver.import_targets(lang, path, import, &base);
    let mut unresolved = |reason| {
        report.unresolved.push(Unresolved {
            file: path.to_string(),
            line: import.line,
            spec: import.spec.clone(),
            lang,
            reason,
        });
    };

    // Mỗi component đích báo một lần cho một câu import, kể cả khi module và
    // module con của nó cùng nằm ở đó.
    let mut targets: Vec<String> = Vec::new();
    match &base {
        Resolved::Path(target) => targets.extend(owners_of(doc, target).first().cloned()),
        // Thư viện không được khai báo `external` thì không tham gia luật. Bắt
        // khai báo mọi gói npm hay mọi header hệ thống sẽ khiến file luật dài
        // vô ích.
        Resolved::Package(pkg) => targets.extend(external_named(doc, pkg)),
        Resolved::Alias => {
            unresolved(UnresolvedReason::Alias);
            return natives;
        }
        Resolved::Missing => {
            unresolved(UnresolvedReason::Missing);
            return natives;
        }
        // Trỏ vào `mod` viết lồng trong file: không có file đích, không có
        // cạnh, và không có gì để báo.
        Resolved::Internal => return natives,
    }
    for sub in submodules {
        if let Some(name) = owners_of(doc, &sub).first()
            && !targets.contains(name)
        {
            targets.push(name.clone());
        }
    }

    for to in targets {
        if to == from {
            continue;
        }
        pulled_in(
            doc,
            from,
            &to,
            path,
            import.line,
            &import.spec,
            Via::Import,
            report,
        );
        if let Some((reason, rule, status)) = judge(doc, from, &to) {
            report.violations.push(Violation {
                from: from.to_string(),
                to: to.clone(),
                file: path.to_string(),
                line: import.line,
                spec: import.spec.clone(),
                via: Via::Import,
                count: 1,
                reason,
                rule,
                status,
                hint: hint_for(doc, from, &to, reason, Via::Import),
            });
        }
    }
    natives
}

/// Mỗi cặp (file, ký hiệu) báo một lần, ở lần gặp đầu tiên, kèm số lần gặp.
/// In ra từng vị trí một sẽ làm ngập báo cáo khi `strcpy` xuất hiện năm chục
/// lần trong cùng một file.
#[allow(clippy::too_many_arguments)]
fn inspect_hits(
    doc: &Document,
    watched: &[(&str, &str)],
    path: &str,
    from: &str,
    hits: &[crate::scan::SymbolHit],
    via: Via,
    report: &mut Report,
) {
    let mut seen: std::collections::BTreeMap<(&str, &str), (usize, usize)> =
        std::collections::BTreeMap::new();
    for hit in hits {
        for (group, symbol) in watched {
            if *symbol != hit.symbol {
                continue;
            }
            seen.entry((group, symbol))
                .and_modify(|(_, n)| *n += 1)
                .or_insert((hit.line, 1));
        }
    }

    for ((group, symbol), (line, count)) in seen {
        if group == from {
            continue;
        }
        if let Some((reason, rule, status)) = judge(doc, from, group) {
            report.violations.push(Violation {
                from: from.to_string(),
                to: group.to_string(),
                file: path.to_string(),
                line,
                spec: symbol.to_string(),
                via,
                count,
                reason,
                rule,
                status,
                hint: hint_for(doc, from, group, reason, via),
            });
        }
    }
}

/// `None` nghĩa là quan hệ hợp lệ, hoặc component chưa vào diện kiểm.
fn judge(doc: &Document, from: &str, to: &str) -> Option<(Reason, String, Status)> {
    if let Some(rule) = matching(doc, RuleKind::Deny, from, to) {
        return Some((Reason::Denied, describe(rule), rule.meta.status));
    }
    // `deny * -> X except Y` là miễn trừ cho Y, không chỉ là "deny không áp
    // dụng". Thiếu bước này thì Y rơi xuống nhánh "không có allow nào" và bị
    // báo vi phạm, trái hẳn ý người viết.
    if let Some(rule) = matching_ignoring_except(doc, RuleKind::Deny, from, to)
        && rule.except.iter().any(|e| e == from)
    {
        return None;
    }
    if matching(doc, RuleKind::Allow, from, to).is_some() {
        return None;
    }
    // Quyền sở hữu kho dữ liệu là một `deny` ngầm, nhưng yếu hơn luật viết
    // tay: `allow` ở trên đã kịp cho qua. Nó cũng không chờ component vào
    // diện kiểm — chạm dữ liệu của người khác luôn là vi phạm.
    if let Some(store) = doc.store(to) {
        if store.owned_by == from {
            return None;
        }
        return Some((
            Reason::NotOwner,
            format!("store {to} owned_by {}", store.owned_by),
            store.meta.status,
        ));
    }
    if !opted_in(doc, from) {
        return None;
    }
    Some((
        Reason::NoAllow,
        format!("không có allow nào cho {from} -> {to}"),
        Status::Accepted,
    ))
}

fn matching<'a>(doc: &'a Document, kind: RuleKind, from: &str, to: &str) -> Option<&'a Rule> {
    doc.rules.iter().find(|r| {
        r.kind == kind
            && r.meta.status.is_enforced()
            && hits(&r.from, from)
            && r.to.iter().any(|t| hits(t, to))
            // `except` miễn trừ cho nguồn: `deny * -> X except Y` nghĩa là Y
            // vẫn được gọi X.
            && !r.except.iter().any(|e| e == from)
    })
}

fn hits(target: &Target, name: &str) -> bool {
    match target {
        Target::Any => true,
        Target::Named(n) => n == name,
    }
}

/// Component vào diện kiểm khi có `allow` đang có hiệu lực nêu đúng tên nó làm
/// nguồn. `allow * -> X` chỉ cấp quyền, không đưa ai vào diện kiểm.
fn opted_in(doc: &Document, component: &str) -> bool {
    doc.rules.iter().any(|r| {
        r.kind == RuleKind::Allow
            && r.meta.status.is_enforced()
            && matches!(&r.from, Target::Named(n) if n == component)
    })
}

fn hint_for(doc: &Document, from: &str, to: &str, reason: Reason, via: Via) -> String {
    if reason == Reason::Denied
        && let Some(rule) = matching_ignoring_except(doc, RuleKind::Deny, from, to)
        && let [only] = rule.except.as_slice()
    {
        return match via {
            Via::Import => format!("gọi {only} thay vì truy cập thẳng {to}"),
            Via::Symbol | Via::Table | Via::Ffi | Via::Network | Via::Entry | Via::Rpc | Via::Queue => {
                format!("chỉ {only} được dùng {to}")
            }
        };
    }
    if reason == Reason::NotOwner
        && let Some(store) = doc.store(to)
    {
        return format!(
            "gọi {} thay vì chạm thẳng vào dữ liệu của nó",
            store.owned_by
        );
    }
    if reason == Reason::NotLinked {
        return format!("thêm `link {from} -> {to}`, hoặc bỏ cạnh này khỏi cấu hình");
    }
    if reason == Reason::NotRun {
        return format!("thêm {to} vào `runs` của service {from}, hoặc bỏ phụ thuộc này");
    }
    if via != Via::Import {
        return format!("{from} chưa được phép dùng {to}");
    }
    let allowed = allowed_targets(doc, from);
    if allowed.is_empty() {
        format!("{from} chưa có allow nào tới đâu cả")
    } else {
        format!("{from} chỉ được gọi: {}", allowed.join(", "))
    }
}

fn matching_ignoring_except<'a>(
    doc: &'a Document,
    kind: RuleKind,
    from: &str,
    to: &str,
) -> Option<&'a Rule> {
    doc.rules.iter().find(|r| {
        r.kind == kind
            && r.meta.status.is_enforced()
            && hits(&r.from, from)
            && r.to.iter().any(|t| hits(t, to))
    })
}

fn allowed_targets(doc: &Document, from: &str) -> Vec<String> {
    let mut out: Vec<String> = doc
        .rules
        .iter()
        .filter(|r| r.kind == RuleKind::Allow && r.meta.status.is_enforced() && hits(&r.from, from))
        .flat_map(|r| r.to.iter().map(|t| t.as_str().to_string()))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Component không external, kèm mẫu đường dẫn của nó.
fn classify(doc: &Document) -> Vec<(String, Vec<String>)> {
    doc.components
        .iter()
        .filter(|c| !c.external && !c.paths.is_empty())
        .map(|c| (c.name.clone(), c.paths.clone()))
        .collect()
}

/// Hai component cùng nhận một file ở **cùng độ cụ thể**: không có cách nào
/// chọn, nên phải hỏi người viết.
fn ambiguous(doc: &Document, path: &str, owners: &[String]) -> bool {
    let top = |name: &str| {
        doc.component(name).map(|c| {
            c.paths
                .iter()
                .filter(|p| glob::matches(p, path))
                .map(|p| specificity(p))
                .max()
        })
    };
    matches!(owners, [a, b, ..] if top(a) == top(b))
}

/// Component sở hữu một đường dẫn, **cụ thể nhất trước**.
///
/// Mẫu chồng nhau là chuyện bình thường chứ không phải lỗi: `src/**` và
/// `src/web/**` cùng khớp một file, và người viết rõ ràng muốn cái thứ hai.
/// Cùng quy tắc với mọi bộ định tuyến — mẫu nào nói nhiều hơn thì thắng.
///
/// Chỉ khi hai mẫu **cùng độ cụ thể** mới là nhập nhằng thật, và chỗ gọi báo
/// ra chuyện đó.
fn owners_of(doc: &Document, path: &str) -> Vec<String> {
    let mut matched: Vec<(usize, &str)> = doc
        .components
        .iter()
        .filter(|c| !c.external)
        .filter_map(|c| {
            c.paths
                .iter()
                .filter(|p| glob::matches(p, path))
                .map(|p| specificity(p))
                .max()
                .map(|s| (s, c.name.as_str()))
        })
        .collect();
    matched.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    matched
        .into_iter()
        .map(|(_, name)| name.to_string())
        .collect()
}

/// Số đoạn nói rõ một cái tên. `src/web/**` là 2, `src/**` là 1, `*` là 0.
fn specificity(pattern: &str) -> usize {
    pattern
        .trim_matches('/')
        .split('/')
        .filter(|seg| !seg.is_empty() && !seg.contains(['*', '?']))
        .count()
}

/// Tên gói trong `at` của một `external`. So khớp chính xác trước, rồi không
/// phân biệt hoa thường — `external Stripe at "stripe"` là cách viết tự nhiên.
fn external_named(doc: &Document, package: &str) -> Option<String> {
    doc.components
        .iter()
        .filter(|c| c.external)
        .find(|c| {
            c.paths.iter().any(|p| p == package)
                || c.paths.iter().any(|p| p.eq_ignore_ascii_case(package))
                || c.name.eq_ignore_ascii_case(package)
        })
        .map(|c| c.name.clone())
}

fn describe(rule: &Rule) -> String {
    let kind = if rule.kind == RuleKind::Allow {
        "allow"
    } else {
        "deny"
    };
    let targets: Vec<&str> = rule.to.iter().map(Target::as_str).collect();
    let mut out = format!("{kind} {} -> {}", rule.from.as_str(), targets.join(", "));
    if !rule.except.is_empty() {
        out.push_str(&format!(" except {}", rule.except.join(", ")));
    }
    out
}

/// Service có hiệu lực mà component `from` chạy trong đó, còn `to` thì không:
/// cạnh `from -> to` kéo mã của `to` vào những service đó.
///
/// Đơn vị là component, như mọi luật khác: component chạy trong service thì
/// mọi thứ nó phụ thuộc cũng chạy ở đó, kể cả khi chỉ một file của nó dùng tới.
/// Nhờ vậy `runs` kiểm được từng file một, trong hook.
#[allow(clippy::too_many_arguments)]
fn pulled_in(
    doc: &Document,
    from: &str,
    to: &str,
    file: &str,
    line: usize,
    spec: &str,
    via: Via,
    report: &mut Report,
) {
    if doc.services.is_empty() || !doc.component(to).is_some_and(|c| !c.external) {
        return;
    }
    for s in doc.services.iter().filter(|s| s.meta.status.is_enforced()) {
        if s.runs.iter().any(|r| r == from) && !s.runs.iter().any(|r| r == to) {
            report.violations.push(Violation {
                from: s.name.clone(),
                to: to.to_string(),
                file: file.to_string(),
                line,
                spec: spec.to_string(),
                via,
                count: 1,
                reason: Reason::NotRun,
                rule: format!("service {} runs {}", s.name, s.runs.join(", ")),
                status: s.meta.status,
                hint: hint_for(doc, &s.name, to, Reason::NotRun, via),
            });
        }
    }
}

/// Cấu hình nào ứng với `service`: service compose hay workload Kubernetes
/// cùng tên (cùng thư mục nếu có ghi), hay Dockerfile đã nạp.
fn matches_source(source: &Source, key: &Key) -> bool {
    match (source, key) {
        (Source::Compose { dir, name }, Key::Compose { dir: d, name: n })
        | (Source::K8s { dir, name }, Key::K8s { dir: d, name: n }) => {
            n == name && dir.as_ref().is_none_or(|x| x == d)
        }
        (Source::Dockerfile(p), Key::Dockerfile(q)) => p == q,
        _ => false,
    }
}

/// Nguồn mà một `service` được đối chiếu: `dockerfile:` đi cùng `k8s:` là
/// image của workload, không phải một cấu hình riêng.
fn own_sources(s: &Service) -> impl Iterator<Item = &Source> {
    let k8s = s.on_k8s();
    s.sources.iter().filter(move |x| {
        !(k8s && matches!(x, Source::Dockerfile(_))) && !matches!(x, Source::Values(_))
    })
}

/// Cách nạp cấu hình mà file luật cần: manifest Kubernetes chỉ được đọc khi
/// luật có nguồn `k8s:` (TM-18); `dockerfile:` đi cùng `k8s:` là image của
/// container chính.
pub fn deploy_options(doc: &Document) -> deploy::Options {
    let mut opts = deploy::Options {
        k8s: doc.services.iter().any(Service::on_k8s),
        ..Default::default()
    };
    // `values:` của Helm: gộp vào chart ở thư mục các nguồn `k8s:` cùng service.
    for s in &doc.services {
        for v in s.sources.iter().filter_map(|x| {
            if let Source::Values(p) = x {
                Some(p)
            } else {
                None
            }
        }) {
            for x in &s.sources {
                if let Source::K8s { dir, .. } = x {
                    opts.helm_values
                        .push((dir.clone().unwrap_or_else(|| "*".into()), v.clone()));
                }
            }
        }
    }
    for s in doc.services.iter().filter(|s| s.on_k8s()) {
        let Some(dockerfile) = s.sources.iter().find_map(|x| match x {
            Source::Dockerfile(p) => Some(p),
            _ => None,
        }) else {
            continue;
        };
        for x in &s.sources {
            if let Source::K8s { dir, name } = x {
                opts.pins.push(deploy::Pin {
                    dir: dir.clone(),
                    name: name.clone(),
                    dockerfile: dockerfile.clone(),
                });
            }
        }
    }
    opts
}

/// Bước xét đường NetworkPolicy cho phép (service × workload được cho phép).
const MAX_GRANT_STEPS: usize = 2_000_000;

/// Đối chiếu cấu hình triển khai với `service` và `link`.
/// Đầu câu `rule` của vi phạm cạnh mạng tới service compose chưa khai.
const UNDECLARED: &str = "service compose ";
/// Như [`UNDECLARED`], cho workload Kubernetes.
const UNDECLARED_K8S: &str = "workload Kubernetes ";

/// Cạnh mạng tới một service compose hay workload chưa khai trong file luật:
/// `to` là tên trong cấu hình, và phải khai `service` cho nó trước khi có
/// `link` được.
pub fn to_undeclared(v: &Violation) -> bool {
    v.reason == Reason::NotLinked
        && (v.rule.starts_with(UNDECLARED) || v.rule.starts_with(UNDECLARED_K8S))
}

/// Tên máy viết thẳng trong mã của một component là cạnh mạng từ mọi service
/// chạy component đó, như tên máy trong biến môi trường: cần `link`, và đích
/// chưa khai thì không `link` nào cho được (TM-32). Trả các cặp đã có `link`,
/// để chúng không bị báo là "không thấy trong cấu hình".
fn judge_code_hosts(
    doc: &Document,
    dep: &Deploy,
    sites: &[(String, String, scan::hosts::CodeHost)],
    report: &mut Report,
) -> BTreeSet<(String, String)> {
    let mut used = BTreeSet::new();
    let declared = |key: &Key| {
        doc.services
            .iter()
            .find(|s| own_sources(s).any(|src| matches_source(src, key)))
    };
    let mut running: HashMap<&str, Vec<&Service>> = HashMap::new();
    // Cấu hình của từng service, để biết tên máy trong mã của nó trỏ tới đâu.
    let mut placed: HashMap<&str, Vec<&Key>> = HashMap::new();
    for v in &dep.services {
        if let Some(s) = doc
            .services
            .iter()
            .find(|s| own_sources(s).any(|src| matches_source(src, &v.key)))
        {
            placed.entry(s.name.as_str()).or_default().push(&v.key);
        }
    }
    // Một vi phạm cho mỗi cặp service trong một file, đếm số chỗ: 1 000 chuỗi
    // `"db:5432"` không thành 1 000 vi phạm trong context của agent (TM-34).
    let mut reported: HashMap<(String, String, String), usize> = HashMap::new();
    for (from, file, h) in sites {
        let Some(keys) = dep.hosts.get(&h.host.name) else {
            continue;
        };
        let runners = running.entry(from.as_str()).or_insert_with(|| {
            doc.services
                .iter()
                .filter(|s| s.meta.status.is_enforced() && s.runs.iter().any(|r| r == from))
                .collect()
        });
        for s in runners.iter() {
            let here = placed.get(s.name.as_str()).map_or(&[][..], Vec::as_slice);
            for key in deploy::reachable(here, keys) {
                let t = declared(key);
                let to = match t {
                    Some(t) if t.name == s.name => continue,
                    Some(t) => t.name.clone(),
                    None => key.text(),
                };
                if t.is_some()
                    && doc.links.iter().any(|l| {
                        l.meta.status.is_enforced() && l.from == s.name && l.to.contains(&to)
                    })
                {
                    used.insert((s.name.clone(), to));
                    continue;
                }
                if let Some(&at) = reported.get(&(s.name.clone(), to.clone(), file.clone())) {
                    report.violations[at].count += 1;
                    continue;
                }
                reported.insert(
                    (s.name.clone(), to.clone(), file.clone()),
                    report.violations.len(),
                );
                let (rule, hint) = match (t, key) {
                    (Some(_), _) => (
                        format!(
                            "không có link nào cho {} -> {to} (tên máy viết trong mã)",
                            s.name
                        ),
                        format!(
                            "thêm `link {} -> {to}`, hoặc bỏ địa chỉ này khỏi mã",
                            s.name
                        ),
                    ),
                    (None, Key::K8s { name, .. }) => (
                        format!(
                            "{UNDECLARED_K8S}`{name}` chưa khai, nên không có link nào cho {} tới nó",
                            s.name
                        ),
                        format!(
                            "khai `service ... from k8s:{name}` rồi thêm `link {} -> ...`",
                            s.name
                        ),
                    ),
                    (None, Key::Compose { name, .. } | Key::Dockerfile(name)) => (
                        format!(
                            "{UNDECLARED}`{name}` chưa khai, nên không có link nào cho {} tới nó",
                            s.name
                        ),
                        format!(
                            "khai `service ... from compose:{name}` rồi thêm `link {} -> ...`",
                            s.name
                        ),
                    ),
                };
                report.violations.push(Violation {
                    from: s.name.clone(),
                    to,
                    file: file.clone(),
                    line: h.line,
                    spec: h.shown(),
                    via: Via::Network,
                    count: 1,
                    reason: Reason::NotLinked,
                    rule,
                    status: Status::Accepted,
                    hint,
                });
            }
        }
    }
    used
}

fn judge_deploy(
    root: &Path,
    doc: &Document,
    resolver: &Resolver,
    dep: &Deploy,
    rpc_links: &std::collections::BTreeSet<(String, String)>,
    report: &mut Report,
) {
    let mut svcs: Vec<Svc> = dep.services.clone();
    report.deploy_unread.extend(dep.unread.iter().cloned());
    for s in doc.services.iter().filter(|s| !s.on_k8s()) {
        for source in &s.sources {
            if let Source::Dockerfile(path) = source
                && !svcs.iter().any(|v| v.key == Key::Dockerfile(path.clone()))
            {
                match deploy::load_dockerfile(root, path) {
                    Ok(v) => svcs.push(v),
                    Err(u) if u.why == "không có" => {}
                    Err(u) => report.deploy_unread.push(u),
                }
            }
        }
    }

    // `service` của file luật → các cấu hình ứng với nó.
    let mut matched: Vec<(&Service, Vec<&Svc>)> = Vec::new();
    for s in doc.services.iter().filter(|s| !s.sources.is_empty()) {
        let mut all: Vec<&Svc> = Vec::new();
        for source in own_sources(s) {
            let found: Vec<&Svc> = svcs
                .iter()
                .filter(|v| matches_source(source, &v.key))
                .collect();
            let dirs: std::collections::BTreeSet<&str> = found
                .iter()
                .filter_map(|v| match &v.key {
                    Key::Compose { dir, .. } | Key::K8s { dir, .. } => Some(dir.as_str()),
                    Key::Dockerfile(_) => None,
                })
                .collect();
            if s.meta.status.is_enforced() {
                if found.is_empty() {
                    report.missing.push(Missing {
                        name: source.text(),
                        source: format!("service {}", s.name),
                        line: s.meta.line,
                        hint: match source {
                            Source::Compose { name, .. } => {
                                format!("không có service `{name}` trong file compose nào")
                            }
                            Source::K8s { name, .. } => format!(
                                "không có workload `{name}` trong manifest Kubernetes nào đọc được"
                            ),
                            Source::Dockerfile(p) => format!("không có Dockerfile `{p}`"),
                            Source::Values(p) => format!("không có file values `{p}`"),
                        },
                    });
                } else if dirs.len() > 1 && matches!(source, Source::Compose { .. }) {
                    // Workload cùng tên ở nhiều thư mục là bản sao của cùng một
                    // ứng dụng (`release/`, base của kustomize): `k8s:<tên>`
                    // kiểm hết chúng, không chọn hộ một bản.
                    let (what, form) = ("service compose", "compose");
                    report.missing.push(Missing {
                        name: source.text(),
                        source: format!("service {}", s.name),
                        line: s.meta.line,
                        hint: format!(
                            "khớp {what} ở nhiều thư mục ({}) — viết `{form}:<thư mục>/<tên>`",
                            dirs.iter()
                                .map(|d| if d.is_empty() { "." } else { d })
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    });
                }
            }
            all.extend(found);
        }
        matched.push((s, all));
    }

    // Cấu hình → `service` của file luật, để gọi tên đích của cạnh mạng.
    let declared = |key: &Key| {
        doc.services
            .iter()
            .find(|s| own_sources(s).any(|src| matches_source(src, key)))
    };

    // Cặp service có lời gọi gRPC cũng là `link` được thấy.
    let mut seen: std::collections::BTreeSet<(String, String)> = rpc_links.clone();
    // Đường NetworkPolicy cho phép mà thiếu `link`: báo một lần cho mỗi cặp
    // service, ở chỗ đầu tiên thấy.
    let mut granted: HashSet<(String, String)> = HashSet::new();
    let mut exposed_reported: HashSet<String> = HashSet::new();
    let mut walked: HashSet<(String, bool, usize)> = HashSet::new();
    let mut grant_steps = 0usize;
    let linked_pairs: HashSet<(&str, &str)> = doc
        .links
        .iter()
        .filter(|l| l.meta.status.is_enforced())
        .flat_map(|l| l.to.iter().map(move |t| (l.from.as_str(), t.as_str())))
        .collect();
    // Import của từng file điểm vào, đọc một lần dù hàng nghìn service cùng
    // chạy nó (TM-8).
    let mut targets: HashMap<String, Targets> = HashMap::new();
    for (s, found) in &matched {
        if !s.meta.status.is_enforced() {
            continue;
        }
        for v in found {
            // Patch Kustomize đè lên workload này mà archctl không áp (TM-21).
            report.deploy_unread.extend(v.patched.iter().cloned());
            // Lộ ra ngoài: cạnh từ `Internet`, cần `link Internet -> …` (TM-29).
            if let Some(e) = v.exposed.first() {
                let internet = crate::model::INTERNET;
                seen.insert((internet.to_string(), s.name.clone()));
                if !linked_pairs.contains(&(internet, s.name.as_str()))
                    && exposed_reported.insert(s.name.clone())
                {
                    report.violations.push(Violation {
                        from: internet.to_string(),
                        to: s.name.clone(),
                        file: e.file.clone(),
                        line: e.line,
                        spec: e.shown.clone(),
                        via: Via::Network,
                        count: v.exposed.len(),
                        reason: Reason::NotLinked,
                        rule: format!(
                            "service {} lộ ra Internet mà không có `link {internet} -> {}`",
                            s.name, s.name
                        ),
                        status: Status::Accepted,
                        hint: format!("thêm `link {internet} -> {}`, hoặc bỏ lối vào này", s.name),
                    });
                }
            }
            // NetworkPolicy cho phép rõ một đường tới service khác: cũng là cạnh
            // mạng, cần `link` (TM-24). Mỗi bộ luật của một policy duyệt một lần
            // cho mỗi service, mỗi cặp service một vi phạm, có ngân sách: một
            // policy 5 000 peer chọn hàng chục service là bậc hai (TM-23).
            for (guard, inbound) in [(&v.ingress, true), (&v.egress, false)] {
                for rules in &guard.grants {
                    if !walked.insert((
                        s.name.clone(),
                        inbound,
                        std::rc::Rc::as_ptr(rules) as usize,
                    )) {
                        continue;
                    }
                    for g in rules.iter() {
                        let deploy::Grant::Pods {
                            keys,
                            policy,
                            file,
                            line,
                        } = g
                        else {
                            continue;
                        };
                        for k in keys.iter() {
                            grant_steps += 1;
                            if grant_steps > MAX_GRANT_STEPS {
                                break;
                            }
                            let Some(o) = declared(k) else { continue };
                            if o.name == s.name {
                                continue;
                            }
                            let (from, to) = if inbound {
                                (&o.name, &s.name)
                            } else {
                                (&s.name, &o.name)
                            };
                            seen.insert((from.clone(), to.clone()));
                            if !linked_pairs.contains(&(from.as_str(), to.as_str()))
                                && granted.insert((from.clone(), to.clone()))
                            {
                                report.violations.push(Violation {
                                    from: from.clone(),
                                    to: to.clone(),
                                    file: file.clone(),
                                    line: *line,
                                    spec: format!("NetworkPolicy `{policy}` cho phép"),
                                    via: Via::Network,
                                    count: 1,
                                    reason: Reason::NotLinked,
                                    rule: format!(
                                        "NetworkPolicy cho phép {from} -> {to} mà không có link nào"
                                    ),
                                    status: Status::Accepted,
                                    hint: hint_for(doc, from, to, Reason::NotLinked, Via::Network),
                                });
                            }
                        }
                    }
                }
            }
            // Cạnh mạng.
            for e in &v.edges {
                // Cạnh tới service compose hay workload chưa khai: không có
                // `link` nào cho nó được, nên đó là cạnh mạng không ai cho phép
                // — không bỏ qua.
                let Some(t) = declared(&e.target) else {
                    let (what, form) = match &e.target {
                        Key::K8s { .. } => (UNDECLARED_K8S, "k8s"),
                        _ => (UNDECLARED, "compose"),
                    };
                    report.violations.push(Violation {
                        from: s.name.clone(),
                        to: e.to.clone(),
                        file: e.file.clone(),
                        line: e.line,
                        spec: e.shown.clone(),
                        via: Via::Network,
                        count: 1,
                        reason: Reason::NotLinked,
                        rule: format!(
                            "{what}`{}` chưa khai, nên không có link nào cho {} tới nó",
                            e.to, s.name
                        ),
                        status: Status::Accepted,
                        hint: format!(
                            "khai `service ... from {form}:{}` rồi thêm `link {} -> ...`",
                            e.to, s.name
                        ),
                    });
                    continue;
                };
                if t.name == s.name {
                    continue;
                }
                seen.insert((s.name.clone(), t.name.clone()));
                let linked = doc.links.iter().any(|l| {
                    l.meta.status.is_enforced() && l.from == s.name && l.to.contains(&t.name)
                });
                if !linked {
                    report.violations.push(Violation {
                        from: s.name.clone(),
                        to: t.name.clone(),
                        file: e.file.clone(),
                        line: e.line,
                        spec: e.shown.clone(),
                        via: Via::Network,
                        count: 1,
                        reason: Reason::NotLinked,
                        rule: format!("không có link nào cho {} -> {}", s.name, t.name),
                        status: Status::Accepted,
                        hint: hint_for(doc, &s.name, &t.name, Reason::NotLinked, Via::Network),
                    });
                }
            }
            // Điểm vào.
            if s.runs.is_empty() {
                // `runs` rỗng là "không chạy mã của dự án". Cấu hình cho thấy
                // rõ nó chạy mã của một component thì đó là vi phạm `runs`,
                // không phải cách lách (TM-21). Chỗ không lần ra hết thì để
                // yên, như image dựng sẵn: `init` cũng sinh `runs` rỗng ở đó.
                let clear = v.code
                    && v.unknown.is_none()
                    && v.starts.iter().all(|st| {
                        st.unresolved.is_empty()
                            && st.cut.is_empty()
                            && st.entries.iter().all(|e| {
                                let files = entry_files(root, resolver, std::slice::from_ref(e));
                                !files.is_empty()
                                    && files.iter().all(|f| Lang::for_path(f).is_some())
                            })
                    });
                if clear {
                    let mut quiet = Report::default();
                    let mut found: HashSet<(String, String, usize)> = HashSet::new();
                    for st in &v.starts {
                        for file in entry_files(root, resolver, &st.entries) {
                            entry_runs(
                                root,
                                doc,
                                resolver,
                                s,
                                st,
                                &file,
                                &mut targets,
                                &mut found,
                                &mut quiet,
                            );
                        }
                    }
                    report.violations.extend(quiet.violations);
                }
                continue;
            }
            if v.starts.is_empty() {
                let why = v
                    .unknown
                    .clone()
                    .unwrap_or_else(|| "không xác định được lệnh khởi chạy".into());
                report.entry_unknown.push((s.name.clone(), why));
                continue;
            }
            // Workload có container lần được (sidecar) mà container chính thì
            // không: vẫn là chỗ không biết mã nào chạy.
            if let (Key::K8s { .. }, Some(why)) = (&v.key, &v.unknown) {
                report.entry_unknown.push((s.name.clone(), why.clone()));
            }
            // Từng thứ lệnh chạy phải lần ra được mã: một cái không ra thì có
            // thể nó đang chạy mã ngoài `runs` mà không ai thấy (TM-12).
            let mut found: HashSet<(String, String, usize)> = HashSet::new();
            for st in &v.starts {
                let mut lost: Vec<String> = st.unresolved.clone();
                let mut unread: Vec<String> = Vec::new();
                for e in &st.entries {
                    let files = entry_files(root, resolver, std::slice::from_ref(e));
                    if files.is_empty() {
                        lost.push(match e {
                            deploy::Entry::Module { name, .. } => name.clone(),
                            deploy::Entry::File(f) | deploy::Entry::Dir(f) => f.clone(),
                        });
                    }
                    for file in files {
                        // Không biết ngôn ngữ (`./entrypoint` viết bằng Python,
                        // không đuôi): import của nó không được quét, kể cả khi
                        // nó thuộc một component, nên không biết nó nạp gì.
                        if Lang::for_path(&file).is_none() {
                            unread.push(file.clone());
                        }
                        entry_runs(
                            root,
                            doc,
                            resolver,
                            s,
                            st,
                            &file,
                            &mut targets,
                            &mut found,
                            report,
                        );
                    }
                }
                if !lost.is_empty() {
                    lost.dedup();
                    report.entry_unknown.push((
                        s.name.clone(),
                        format!(
                            "{}:{} không tìm thấy trong repo mã mà `{}` nạp",
                            st.file,
                            st.line,
                            lost.join("`, `")
                        ),
                    ));
                }
                if !unread.is_empty() {
                    report.entry_unknown.push((
                        s.name.clone(),
                        format!(
                            "{}:{} không biết ngôn ngữ của `{}`, nên không biết nó nạp mã nào",
                            st.file,
                            st.line,
                            unread.join("`, `")
                        ),
                    ));
                }
                if !st.cut.is_empty() {
                    report.entry_unknown.push((
                        s.name.clone(),
                        format!(
                            "{}:{} không đọc hết lệnh khởi chạy: {}",
                            st.file,
                            st.line,
                            st.cut.join("; ")
                        ),
                    ));
                }
            }
        }
    }

    for l in doc.links.iter().filter(|l| l.meta.status.is_enforced()) {
        // `Internet` không có cấu hình riêng: lối vào nằm ở cấu hình của đích.
        let has_config = |name: &str| {
            name == crate::model::INTERNET
                || matched.iter().any(|(s, f)| s.name == name && !f.is_empty())
        };
        if !has_config(&l.from) {
            continue;
        }
        for t in &l.to {
            if has_config(t)
                && !seen.contains(&(l.from.clone(), t.clone()))
                && !(report.partial && may_call(doc, &l.from, t))
            {
                report
                    .links_unseen
                    .push((l.from.clone(), t.clone(), l.meta.line));
            }
        }
    }

    if grant_steps > MAX_GRANT_STEPS {
        report.deploy_unread.push(deploy::Unread {
            file: "NetworkPolicy".into(),
            why: format!(
                "quá {MAX_GRANT_STEPS} bước xét đường policy cho phép; phần còn lại không xét"
            ),
        });
    }

    // `link` mà NetworkPolicy chặn mọi đường giữa các workload của hai bên:
    // luật nói một đằng, cụm làm một nẻo. Chỉ xét workload Kubernetes.
    let workloads = |name: &str| -> Vec<&Svc> {
        matched
            .iter()
            .filter(|(s, _)| s.name == name)
            .flat_map(|(_, f)| f.iter().copied())
            .filter(|v| matches!(v.key, Key::K8s { .. }))
            .collect()
    };
    for l in doc.links.iter().filter(|l| l.meta.status.is_enforced()) {
        let from = workloads(&l.from);
        if from.is_empty() {
            continue;
        }
        for t in &l.to {
            let to = workloads(t);
            if to.is_empty() {
                continue;
            }
            let open = from.iter().any(|a| {
                to.iter()
                    .any(|b| b.ingress.allows(&a.key) && a.egress.allows(&b.key))
            });
            if !open {
                report
                    .links_blocked
                    .push((l.from.clone(), t.clone(), l.meta.line));
            }
        }
    }

    // Đã mô tả triển khai thì mọi service trong compose, mọi workload trong
    // manifest phải có mặt.
    let uses = |f: fn(&Source) -> bool| doc.services.iter().any(|s| s.sources.iter().any(f));
    let compose = uses(|x| matches!(x, Source::Compose { .. }));
    let k8s = uses(|x| matches!(x, Source::K8s { .. }));
    for v in &svcs {
        match &v.key {
            Key::Compose { name, .. } if compose && declared(&v.key).is_none() => {
                report
                    .services_undeclared
                    .push((v.file.clone(), v.line, name.clone()));
            }
            Key::K8s { name, .. } if k8s && declared(&v.key).is_none() => {
                report
                    .services_undeclared
                    .push((v.file.clone(), v.line, name.clone()));
            }
            _ => {}
        }
    }
    report.services_undeclared.sort();
    report.services_undeclared.dedup();
    report.entry_unknown.sort();
    report.entry_unknown.dedup();
    // Cùng một patch đè lên workload mà hai `service` cùng trỏ tới: báo một lần.
    let mut seen_unread: HashSet<(String, String)> = HashSet::new();
    report
        .deploy_unread
        .retain(|u| seen_unread.insert((u.file.clone(), u.why.clone())));
}

/// File mà các điểm vào nạp. Module không có ở thư mục làm việc trong
/// container thì tra theo các gói của dự án, như một gói đã `pip install`.
pub fn entry_files(root: &Path, resolver: &Resolver, entries: &[deploy::Entry]) -> Vec<String> {
    let mut out = Vec::new();
    for e in entries {
        match deploy::entry_files(root, e) {
            Ok(files) => out.extend(files),
            Err(module) => {
                let import = scan::Import {
                    spec: module,
                    line: 0,
                    system: false,
                    names: Vec::new(),
                };
                if let Resolved::Path(p) = resolver.resolve(Lang::Python, "", &import) {
                    let main = matches!(e, deploy::Entry::Module { main: true, .. });
                    if main && let Some(dir) = p.strip_suffix("/__init__.py") {
                        let m = format!("{dir}/__main__.py");
                        if root.join(&m).is_file() {
                            out.push(m);
                        }
                    }
                    if root.join(&p).is_file() {
                        out.push(p);
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Component mà một điểm vào chạy phải nằm trong `runs`. File điểm vào không
/// thuộc component nào (`manage.py` ở gốc repo) thì xét các component mà nó
/// import trực tiếp.
#[allow(clippy::too_many_arguments)]
fn entry_runs(
    root: &Path,
    doc: &Document,
    resolver: &Resolver,
    s: &Service,
    st: &deploy::Start,
    file: &str,
    targets: &mut HashMap<String, Targets>,
    found: &mut HashSet<(String, String, usize)>,
    report: &mut Report,
) {
    let outside =
        |c: &str| doc.component(c).is_some_and(|c| !c.external) && !s.runs.iter().any(|r| r == c);
    let violation = |to: &str, file: &str, line: usize, spec: String, via: Via| Violation {
        from: s.name.clone(),
        to: to.to_string(),
        file: file.to_string(),
        line,
        spec,
        via,
        count: 1,
        reason: Reason::NotRun,
        rule: format!("service {} runs {}", s.name, s.runs.join(", ")),
        status: s.meta.status,
        hint: hint_for(doc, &s.name, to, Reason::NotRun, via),
    };
    // Một vi phạm cho mỗi (component, chỗ): `from pkg import x` phân giải ra
    // cả `pkg/__init__.py` lẫn `pkg/x.py`, cùng một component.
    if let Some(owner) = owners_of(doc, file).first() {
        if outside(owner) && found.insert((owner.clone(), st.file.clone(), st.line)) {
            report.violations.push(violation(
                owner,
                &st.file,
                st.line,
                format!("khởi chạy {}", st.shown),
                Via::Entry,
            ));
        }
        return;
    }
    let direct = targets
        .entry(file.to_string())
        .or_insert_with(|| direct_targets(root, resolver, file));
    let direct = match direct {
        Ok(d) => d,
        // Đọc không được thì không biết nó nạp gì (TM-12).
        Err(why) => {
            report.entry_unknown.push((
                s.name.clone(),
                format!("{}:{} điểm vào `{file}`: {why}", st.file, st.line),
            ));
            return;
        }
    };
    for (target, line, spec) in direct.iter() {
        if let Some(owner) = owners_of(doc, target).first()
            && outside(owner)
            && found.insert((owner.clone(), file.to_string(), *line))
        {
            report
                .violations
                .push(violation(owner, file, *line, spec.clone(), Via::Import));
        }
    }
}

/// Đích trực tiếp của các import trong một file: `(đích, dòng, import)`, hay
/// lý do không đọc được.
pub(crate) type Targets = Result<Vec<(String, usize, String)>, String>;

/// Đích trực tiếp của các import trong một file.
///
/// File do cấu hình triển khai chỉ tới, nên đọc như [`deploy::read_in`]:
/// không qua symlink, chỉ file thường, có trần ([`MAX_FILE_BYTES`]). Không
/// biết ngôn ngữ thì không có import nào; đọc không được thì trả lý do.
pub(crate) fn direct_targets(root: &Path, resolver: &Resolver, file: &str) -> Targets {
    let Some(lang) = Lang::for_path(file) else {
        return Ok(Vec::new());
    };
    let src = deploy::read_in(root, file, MAX_FILE_BYTES)?;
    let src = scan::source_view(file, &src);
    let scanned = lang.scan_all(&src, &Default::default(), &[]);
    let mut out = Vec::new();
    for import in &scanned.imports {
        let base = resolver.resolve(lang, file, import);
        let (subs, _) = resolver.import_targets(lang, file, import, &base);
        if let Resolved::Path(p) = base {
            out.push((p, import.line, import.spec.clone()));
        }
        out.extend(
            subs.into_iter()
                .map(|p| (p, import.line, import.spec.clone())),
        );
    }
    Ok(out)
}

fn walk(root: &Path, prefix: &str, extra_ignores: &[String], out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(root.join(prefix)) else {
        return;
    };
    let mut names: Vec<_> = entries.flatten().collect();
    names.sort_by_key(|e| e.file_name());

    for entry in names {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let rel = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };

        // Không đi theo symlink: tránh vòng lặp và tránh ra ngoài gốc dự án.
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        if meta.file_type().is_symlink() || glob::matches_any(extra_ignores, &rel) {
            continue;
        }
        if meta.is_dir() {
            if ALWAYS_IGNORE.contains(&name.as_str()) {
                continue;
            }
            if OUTPUT_DIRS.contains(&name.as_str()) && gitignored_dir(root, &rel) != Some(false) {
                continue;
            }
            walk(root, &rel, extra_ignores, out);
        } else if meta.is_file() {
            out.push(rel);
        }
    }
}

/// `.gitignore` có bỏ qua thư mục `rel` không. `None`: không có `.gitignore`
/// nào áp lên nó, tức là không biết.
///
/// Đủ cho một câu hỏi về một thư mục, không phải bộ đọc `.gitignore` đầy đủ:
/// đọc file ở mọi thư mục từ gốc repo git (có thể nằm trên gốc archctl, trong
/// monorepo) xuống tới cha của `rel`, và `.git/info/exclude`; mẫu sau thắng
/// mẫu trước, `!` đưa trở lại, có `/` ở giữa hay đầu thì neo vào thư mục chứa
/// file, không có thì khớp tên ở mọi độ sâu. Thư mục cha bị bỏ thì con cũng
/// bị. `dist/*` bỏ mọi thứ trong `dist`, nên tính như bỏ chính `dist`.
fn gitignored_dir(root: &Path, rel: &str) -> Option<bool> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let (top, prefix) = git_top(&root);
    let parts: Vec<&str> = prefix
        .iter()
        .map(String::as_str)
        .chain(rel.split('/'))
        .collect();
    let (sources, known) = ignore_sources(&top, &parts);
    if ignored_within(&sources, &parts, prefix.len() + 1) {
        return Some(true);
    }
    known.then_some(false)
}

/// Thư mục gốc của repo git chứa `root`, và đường đi từ đó xuống `root`.
///
/// Gốc archctl là thư mục gần nhất có `.arch/`, có thể là `web/` trong một
/// repo git lớn hơn, và `.gitignore` ở trên vẫn áp lên `web/dist`. Nhưng nếu
/// `root` nằm trong thư mục mà repo đó bỏ qua — bộ đo chép netbox vào
/// `archctl/target/` — thì git không bao giờ nhìn vào đây, và `.gitignore` của
/// repo ngoài không phải luật của nó. Không thấy `.git` thì cũng không đọc gì
/// phía trên: `~/.gitignore` của ai đó không phải luật của repo này.
fn git_top(root: &Path) -> (std::path::PathBuf, Vec<String>) {
    let mut top = root.to_path_buf();
    let mut prefix: Vec<String> = Vec::new();
    while !top.join(".git").exists() {
        let name = top.file_name().and_then(|n| n.to_str()).map(str::to_string);
        match (name, top.parent()) {
            (Some(name), Some(parent)) => {
                prefix.insert(0, name);
                top = parent.to_path_buf();
            }
            _ => return (root.to_path_buf(), Vec::new()),
        }
    }
    if !prefix.is_empty() {
        let parts: Vec<&str> = prefix.iter().map(String::as_str).collect();
        let (sources, _) = ignore_sources(&top, &parts);
        if ignored_within(&sources, &parts, 1) {
            return (root.to_path_buf(), Vec::new());
        }
    }
    (top, prefix)
}

/// Mọi `.gitignore` từ `top` xuống tới cha của `parts`, kèm độ sâu của thư
/// mục chứa nó, và `.git/info/exclude` ở độ sâu 0. Phần tử thứ hai: có
/// `.gitignore` nào không.
fn ignore_sources(top: &Path, parts: &[&str]) -> (Vec<(usize, String)>, bool) {
    let mut sources: Vec<(usize, String)> = Vec::new();
    for depth in 0..parts.len() {
        let dir = parts[..depth].join("/");
        if let Ok(text) = std::fs::read_to_string(top.join(&dir).join(".gitignore")) {
            sources.push((depth, crate::strip_bom(&text).to_string()));
        }
    }
    // Chỉ `.gitignore` mới nói gì đó về repo: được commit, máy nào cũng như
    // nhau. `.git/info/exclude` là của riêng một máy, nên nó được bỏ thêm thư
    // mục nhưng không được biến "không biết" thành "quét".
    let known = !sources.is_empty();
    if let Ok(text) = std::fs::read_to_string(top.join(".git/info/exclude")) {
        sources.insert(0, (0, crate::strip_bom(&text).to_string()));
    }
    (sources, known)
}

/// Có thư mục nào trong `parts[..first]`, `parts[..first + 1]`, … tới hết bị
/// bỏ qua không. Từ ngoài vào: cha đã bị bỏ thì git không bao giờ nhìn vào
/// trong, nên `!` ở sâu hơn không cứu được.
fn ignored_within(sources: &[(usize, String)], parts: &[&str], first: usize) -> bool {
    for end in first..=parts.len() {
        let mut ignored = false;
        for (depth, text) in sources {
            if *depth >= end {
                continue;
            }
            let sub = parts[*depth..end].join("/");
            for line in text.lines() {
                let line = line.trim_end();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let (negate, pattern) = match line.strip_prefix('!') {
                    Some(rest) => (true, rest),
                    None => (false, line.strip_prefix('\\').unwrap_or(line)),
                };
                let pattern = pattern.trim_end_matches('/');
                if pattern.is_empty() {
                    continue;
                }
                let whole = pattern
                    .strip_suffix("/**")
                    .or_else(|| pattern.strip_suffix("/*"));
                let hit = |pattern: &str| {
                    if pattern.contains('/') || whole.is_some() {
                        glob::matches(pattern.trim_start_matches('/'), &sub)
                    } else {
                        glob::matches(pattern, parts[end - 1])
                    }
                };
                if hit(pattern) || whole.is_some_and(|w| !w.is_empty() && hit(w)) {
                    ignored = !negate;
                }
            }
        }
        if ignored {
            return true;
        }
    }
    false
}
