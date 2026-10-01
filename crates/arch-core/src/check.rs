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
use crate::resolve::{self, Resolved};
use crate::scan;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    pub from: String,
    pub to: String,
    pub file: String,
    pub line: usize,
    pub spec: String,
    pub reason: Reason,
    /// Luật bị vi phạm, viết lại thành một dòng.
    pub rule: String,
    pub status: Status,
    pub hint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedReason {
    /// Trông như alias của tsconfig `paths` — chưa hỗ trợ.
    Alias,
    /// Đường dẫn tương đối nhưng không có file nào ở đó.
    Missing,
}

impl UnresolvedReason {
    pub fn describe(self) -> &'static str {
        match self {
            UnresolvedReason::Alias => "alias tsconfig paths chưa được hỗ trợ",
            UnresolvedReason::Missing => "không tìm thấy file nào ở đường dẫn này",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unresolved {
    pub file: String,
    pub line: usize,
    pub spec: String,
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
}

impl Report {
    pub fn has_violations(&self) -> bool {
        !self.violations.is_empty()
    }
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

    for path in &files {
        if !scan::is_supported(path) {
            continue;
        }
        report.files_scanned += 1;

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
        for import in scan::imports(&src) {
            inspect(root, doc, path, from, &import, &mut report);
        }
    }

    report.violations.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report.unresolved.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report
}

fn inspect(
    root: &Path,
    doc: &Document,
    path: &str,
    from: &str,
    import: &scan::Import,
    report: &mut Report,
) {
    let to = match resolve::resolve(root, path, &import.spec) {
        Resolved::File(target) => match owners_of(doc, &target).first() {
            Some(name) => name.clone(),
            None => return,
        },
        // Gói không được khai báo `external` thì không tham gia luật. Bắt khai
        // báo mọi gói npm sẽ khiến file luật dài vô ích.
        Resolved::Package(pkg) => match external_named(doc, &pkg) {
            Some(name) => name,
            None => return,
        },
        Resolved::Alias => {
            report.unresolved.push(Unresolved {
                file: path.to_string(),
                line: import.line,
                spec: import.spec.clone(),
                reason: UnresolvedReason::Alias,
            });
            return;
        }
        Resolved::Missing => {
            report.unresolved.push(Unresolved {
                file: path.to_string(),
                line: import.line,
                spec: import.spec.clone(),
                reason: UnresolvedReason::Missing,
            });
            return;
        }
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
            reason,
            rule,
            status,
            hint: hint_for(doc, from, &to, reason),
        });
    }
}

/// `None` nghĩa là quan hệ hợp lệ, hoặc component chưa vào diện kiểm.
fn judge(doc: &Document, from: &str, to: &str) -> Option<(Reason, String, Status)> {
    if let Some(rule) = matching(doc, RuleKind::Deny, from, to) {
        return Some((Reason::Denied, describe(rule), rule.meta.status));
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

fn hint_for(doc: &Document, from: &str, to: &str, reason: Reason) -> String {
    if reason == Reason::Denied
        && let Some(rule) = matching_ignoring_except(doc, RuleKind::Deny, from, to)
        && let [only] = rule.except.as_slice()
    {
        return format!("gọi {only} thay vì truy cập thẳng {to}");
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
