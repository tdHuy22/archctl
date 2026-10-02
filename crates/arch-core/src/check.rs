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
    /// Chạm vào kho dữ liệu của component khác.
    NotOwner,
    /// Tên lẽ ra chỉ nằm trong một vùng nhưng lọt ra ngoài.
    Leaked,
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
    /// ghép với định nghĩa cùng tên ở phía C.
    Ffi,
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
    check_files(root, doc, &source_files(root, extra_ignores), false)
}

/// Kiểm một tập file cho trước. `partial` đánh dấu rằng đây không phải toàn
/// repo, để báo cáo không in ra những con số tổng gây hiểu nhầm.
///
/// Chia nhỏ được vì luật phụ thuộc và luật `api` đều xét **từng file một**:
/// import của một file không phụ thuộc vào file nào khác.
pub fn check_files(root: &Path, doc: &Document, files: &[String], partial: bool) -> Report {
    let mut report = Report { partial, ..Report::default() };

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
    let expectations: Vec<_> =
        doc.expectations().into_iter().filter(|e| e.status.is_enforced()).collect();
    let mut needle_set: crate::scan::Needles =
        needles.iter().map(String::as_str).collect();
    needle_set.extend(expectations.iter().map(|e| e.name.as_str()));
    // Tên nào xuất hiện ở component nào, để biết cái gì thiếu và cái gì lọt ra.
    let mut sightings: std::collections::BTreeMap<&str, Vec<Sighting>> =
        std::collections::BTreeMap::new();
    // Mẫu số cho việc xét một cái tên có quá phổ biến để nói lên điều gì không.
    let mut symbol_scanned = 0usize;
    // Hai đầu của một cạnh liên ngôn ngữ, ghép lại sau khi quét xong: một bên
    // khai báo `extern "C" { fn x }`, bên kia định nghĩa `x`.
    let mut ffi_calls: Vec<(String, String, crate::scan::SymbolHit)> = Vec::new();
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
        if src.len() as u64 > MAX_FILE_BYTES {
            report.oversized.push((path.clone(), src.len() as u64));
            continue;
        }
        symbol_scanned += 1;
        let scan = lang.scan_all(&src, &needle_set, &as_refs(&table_needles));
        for import in &scan.imports {
            inspect(&resolver, doc, lang, path, from, import, &mut report);
        }
        inspect_hits(doc, &watched, path, from, &scan.symbols, Via::Symbol, &mut report);
        inspect_hits(doc, &tables, path, from, &scan.tables, Via::Table, &mut report);

        for call in &scan.foreign.calls {
            ffi_calls.push((from.clone(), path.clone(), call.clone()));
        }
        for provided in &scan.foreign.provides {
            // Cùng một tên được hai component cung cấp thì không biết chọn
            // đâu, nên bỏ cả hai: đoán sai sinh ra một cạnh không có thật.
            ffi_provides
                .entry(provided.symbol.clone())
                .and_modify(|owner| {
                    if owner != from {
                        owner.clear();
                    }
                })
                .or_insert_with(|| from.clone());
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

    judge_ffi(doc, &ffi_calls, &ffi_provides, &mut report);
    judge_expectations(&expectations, &sightings, symbol_scanned, &mut report);

    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    report.by_lang = counts;

    report.violations.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report.unresolved.sort_by(|a, b| (&a.file, a.line).cmp(&(&b.file, b.line)));
    report.oversized.sort();
    report
}

/// Ghép hai đầu của một lời gọi qua ranh giới ngôn ngữ.
///
/// Tên nào không ghép được thì im lặng bỏ qua: đó là hàm của thư viện hệ
/// thống, hoặc — khi chỉ quét một phần repo — phía cung cấp nằm ngoài tập
/// file đang xét.
fn judge_ffi(
    doc: &Document,
    calls: &[(String, String, crate::scan::SymbolHit)],
    provides: &std::collections::BTreeMap<String, String>,
    report: &mut Report,
) {
    for (from, file, call) in calls {
        let Some(to) = provides.get(&call.symbol).filter(|owner| !owner.is_empty()) else {
            continue;
        };
        if to == from {
            continue;
        }
        let Some((reason, rule, status)) = judge(doc, from, to) else { continue };
        report.violations.push(Violation {
            from: from.clone(),
            to: to.clone(),
            file: file.clone(),
            line: call.line,
            spec: call.symbol.clone(),
            via: Via::Ffi,
            count: 1,
            reason,
            rule,
            status,
            hint: hint_for(doc, from, to, reason, Via::Ffi),
        });
    }
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
        let seen = sightings.get(want.name.as_str()).map(Vec::as_slice).unwrap_or_default();
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
                let evidence: Vec<&Sighting> =
                    if by_declaration { declared } else { seen.iter().collect() };

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

                let what = if by_declaration { "khai báo" } else { "thấy" };
                let hint = if within.is_empty() {
                    format!("`{}` không có trong mã — sửa mã, hoặc sửa khai báo", want.name)
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
    report.missing.sort_by(|a, b| (&a.source, &a.name).cmp(&(&b.source, &b.name)));
    report.missing.dedup();
    report.vacuous.sort_by(|a, b| (&a.source, &a.name).cmp(&(&b.source, &b.name)));
    report.vacuous.dedup();
}

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
        // Trỏ vào `mod` viết lồng trong file: không có file đích, không có
        // cạnh, và không có gì để báo.
        Resolved::Internal => return,
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
            Via::Symbol | Via::Table | Via::Ffi => format!("chỉ {only} được dùng {to}"),
        };
    }
    if reason == Reason::NotOwner
        && let Some(store) = doc.store(to)
    {
        return format!("gọi {} thay vì chạm thẳng vào dữ liệu của nó", store.owned_by);
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
