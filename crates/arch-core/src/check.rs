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
use crate::resolve::{Resolved, Resolver};
use crate::scan::{self, Lang};
use std::path::Path;

/// Thư mục không bao giờ quét. Không cấu hình được, vì quét chúng chỉ tốn thời
/// gian chứ không bao giờ hữu ích.
pub const ALWAYS_IGNORE: &[&str] = &[
    ".git",
    ".arch",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    "coverage",
    "vendor",
    "__pycache__",
    ".venv",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Có `deny` đang có hiệu lực chặn quan hệ này.
    Denied,
    /// Component đã vào diện kiểm nhưng không `allow` nào cho phép quan hệ này.
    NoAllow,
}

/// Phụ thuộc được phát hiện bằng cách nào.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// Câu lệnh import. Không thấy được FFI, liên kết thư viện hay IPC.
    Import,
    /// Tên hàm hoặc từ khoá xuất hiện trong mã. Không có vùng mù như import.
    Symbol,
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
}

impl UnresolvedReason {
    pub fn describe(self, lang: Lang) -> &'static str {
        match (self, lang) {
            (UnresolvedReason::Alias, _) => "alias cần cấu hình build, chưa được hỗ trợ",
            (UnresolvedReason::Missing, Lang::C) => {
                "không tìm thấy header, có thể nằm trong include path của build"
            }
            (UnresolvedReason::Missing, Lang::Rust) => {
                "không tìm thấy module này trong crate"
            }
            (UnresolvedReason::Missing, _) => "không tìm thấy file nào ở đường dẫn này",
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
    /// Luật nhắc tới `store`, cần phân tích truy cập dữ liệu nên chưa kiểm.
    pub rules_deferred: usize,
    /// Số file đã quét theo từng ngôn ngữ, để biết bộ kiểm chứng thật sự nhìn
    /// thấy những phần nào của repo.
    pub by_lang: Vec<(Lang, usize)>,
}

impl Report {
    pub fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
}

/// Mọi file thuộc ngôn ngữ đã hỗ trợ, tương đối so với gốc dự án.
pub fn source_files(root: &Path, extra_ignores: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    walk(root, "", extra_ignores, &mut files);
    files.retain(|f| Lang::for_path(f).is_some());
    files
}

pub fn check(root: &Path, doc: &Document, extra_ignores: &[String]) -> Report {
    let mut report = Report::default();
    let mut files = Vec::new();
    walk(root, "", extra_ignores, &mut files);

    report.rules_deferred = doc.rules.iter().filter(|r| mentions_store(doc, r)).count();

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
    let needles: Vec<&str> = {
        let mut v: Vec<&str> = watched.iter().map(|(_, s)| *s).collect();
        v.sort_unstable();
        v.dedup();
        v
    };

    for path in &files {
        let Some(lang) = Lang::for_path(path) else {
            continue;
        };
        report.files_scanned += 1;
        match counts.iter_mut().find(|(l, _)| *l == lang) {
            Some((_, n)) => *n += 1,
            None => counts.push((lang, 1)),
        }

        let owners = owners_of(doc, path);
        if owners.len() > 1 {
            report.ambiguous.push((path.clone(), owners.clone()));
        }
        let Some(from) = owners.first() else {
            report.files_unmapped += 1;
            continue;
        };

        let Ok(src) = std::fs::read_to_string(root.join(path)) else {
            continue;
        };
        let scan = lang.scan(&src, &needles);
        for import in &scan.imports {
            inspect(&resolver, doc, lang, path, from, import, &mut report);
        }
        inspect_symbols(doc, &watched, path, from, &scan.symbols, &mut report);
    }

    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    report.by_lang = counts;

    report.violations.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report.unresolved.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report
}

#[allow(clippy::too_many_arguments)]
fn inspect(
    resolver: &Resolver,
    doc: &Document,
    lang: Lang,
    path: &str,
    from: &str,
    import: &scan::Import,
    report: &mut Report,
) {
    let mut unresolved = |reason| {
        report.unresolved.push(Unresolved {
            file: path.to_string(),
            line: import.line,
            spec: import.spec.clone(),
            lang,
            reason,
        });
    };

    let to = match resolver.resolve(lang, path, import) {
        Resolved::Path(target) => match owners_of(doc, &target).first() {
            Some(name) => name.clone(),
            None => return,
        },
        // Thư viện không được khai báo `external` thì không tham gia luật. Bắt
        // khai báo mọi gói npm hay mọi header hệ thống sẽ khiến file luật dài
        // vô ích.
        Resolved::Package(pkg) => match external_named(doc, &pkg) {
            Some(name) => name,
            None => return,
        },
        Resolved::Alias => return unresolved(UnresolvedReason::Alias),
        Resolved::Missing => return unresolved(UnresolvedReason::Missing),
    };

    if to == from {
        return;
    }
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

/// Mỗi cặp (file, ký hiệu) báo một lần, ở lần gặp đầu tiên, kèm số lần gặp.
/// In ra từng vị trí một sẽ làm ngập báo cáo khi `strcpy` xuất hiện năm chục
/// lần trong cùng một file.
fn inspect_symbols(
    doc: &Document,
    watched: &[(&str, &str)],
    path: &str,
    from: &str,
    hits: &[crate::scan::SymbolHit],
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
                via: Via::Symbol,
                count,
                reason,
                rule,
                status,
                hint: hint_for(doc, from, group, reason, Via::Symbol),
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
    if !opted_in(doc, from) {
        return None;
    }
    Some((Reason::NoAllow, format!("không có allow nào cho {from} -> {to}"), Status::Accepted))
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
            Via::Symbol => format!("chỉ {only} được dùng nhóm {to}"),
        };
    }
    if via == Via::Symbol {
        return format!("{from} chưa được phép dùng nhóm {to}");
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

fn owners_of(doc: &Document, path: &str) -> Vec<String> {
    doc.components
        .iter()
        .filter(|c| !c.external && glob::matches_any(&c.paths, path))
        .map(|c| c.name.clone())
        .collect()
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

fn mentions_store(doc: &Document, rule: &Rule) -> bool {
    let is_store = |t: &Target| matches!(t, Target::Named(n) if doc.store(n).is_some());
    is_store(&rule.from) || rule.to.iter().any(is_store)
}

fn describe(rule: &Rule) -> String {
    let kind = if rule.kind == RuleKind::Allow { "allow" } else { "deny" };
    let targets: Vec<&str> = rule.to.iter().map(Target::as_str).collect();
    let mut out = format!("{kind} {} -> {}", rule.from.as_str(), targets.join(", "));
    if !rule.except.is_empty() {
        out.push_str(&format!(" except {}", rule.except.join(", ")));
    }
    out
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
        let rel = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };

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
            walk(root, &rel, extra_ignores, out);
        } else if meta.is_file() {
            out.push(rel);
        }
    }
}
