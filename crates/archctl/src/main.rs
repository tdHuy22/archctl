//! CLI của archctl.
//!
//! Tự phân tích tham số thay vì dùng thư viện, để giữ cây dependency bằng
//! không. Công cụ này chạy trong repo của người dùng và trong vòng lặp của
//! agent, nên mỗi dependency thêm vào là một bề mặt tấn công thêm.

use arch_core::check::{Reason, Report};
use arch_core::diff::{Class, Kind};
use arch_core::diag::{self, Severity};
use arch_core::model::Status;
use arch_core::{Document, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Mã thoát, theo mục 7 của đặc tả.
mod exit {
    pub const OK: u8 = 0;
    pub const VIOLATION: u8 = 1;
    pub const DSL_INVALID: u8 = 2;
    pub const USAGE: u8 = 64;
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");

    match command {
        "parse" => cmd_parse(&args[1..]),
        "check" => cmd_check(&args[1..]),
        "diff" => cmd_diff(&args[1..]),
        "help" | "-h" | "--help" => {
            print_usage();
            ExitCode::from(exit::OK)
        }
        "version" | "-V" | "--version" => {
            println!("archctl {VERSION}");
            ExitCode::from(exit::OK)
        }
        other => {
            eprintln!("archctl: lệnh không nhận ra: `{other}`\n");
            print_usage();
            ExitCode::from(exit::USAGE)
        }
    }
}

fn print_usage() {
    println!(
        "archctl {VERSION} — sơ đồ kiến trúc là luật

Cách dùng:
  archctl check [--root <dir>] [--json]   Đối chiếu mã nguồn với luật
  archctl diff [<git-ref>] [--root <dir>] So luật hiện tại với một bản trước
  archctl parse <file.arch>               Chỉ đọc và kiểm tra file luật
  archctl help
  archctl version

`diff` mặc định so với HEAD. Nó tách thay đổi kiến trúc khỏi thay đổi chỉ
xê dịch chữ trong file, để người duyệt biết chỗ nào cần đọc kỹ.

Gốc dự án là thư mục gần nhất đi lên có chứa .arch/system.arch.

Mã thoát:
  0   không vi phạm
  1   có vi phạm luật đang có hiệu lực
  2   DSL sai cú pháp hoặc tham chiếu không tồn tại
  64  dùng lệnh sai"
    );
}

/// Đọc file luật ở gốc đã cho. In chẩn đoán và trả `None` nếu DSL có lỗi.
fn load_arch(root: &Path, as_json: bool) -> Option<(Document, String)> {
    let path = root.join(arch_core::ARCH_FILE);
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("archctl: không đọc được `{}`: {e}", path.display());
            return None;
        }
    };
    let (doc, diags) = arch_core::load(&src);
    let hash = arch_core::arch_hash(&doc);

    if diag::has_errors(&diags) {
        if !as_json {
            for d in diags.iter().filter(|d| d.severity == Severity::Error) {
                println!("✗ {}:{d}", arch_core::ARCH_FILE);
            }
        }
        return None;
    }
    Some((doc, hash))
}

fn cmd_check(args: &[String]) -> ExitCode {
    let mut as_json = false;
    let mut root_arg: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => as_json = true,
            "--root" => match args.get(i + 1) {
                Some(p) => {
                    root_arg = Some(PathBuf::from(p));
                    i += 1;
                }
                None => {
                    eprintln!("archctl check: `--root` thiếu đường dẫn");
                    return ExitCode::from(exit::USAGE);
                }
            },
            other => {
                eprintln!("archctl check: tham số không nhận ra: `{other}`");
                return ExitCode::from(exit::USAGE);
            }
        }
        i += 1;
    }

    let start = root_arg.unwrap_or_else(|| PathBuf::from("."));
    let Some(root) = arch_core::find_root(&start) else {
        eprintln!(
            "archctl: không tìm thấy `{}` ở `{}` hay thư mục cha nào.\n\
             → chạy trong dự án đã có .arch/, hoặc dùng `--root <dir>`",
            arch_core::ARCH_FILE,
            start.display()
        );
        return ExitCode::from(exit::USAGE);
    };

    let Some((doc, hash)) = load_arch(&root, as_json) else {
        return ExitCode::from(exit::DSL_INVALID);
    };

    let ignores = arch_core::read_ignores(&root);
    let report = arch_core::check::check(&root, &doc, &ignores);

    if as_json {
        print_json(&hash, &report);
    } else {
        print_human(&hash, &report);
    }

    ExitCode::from(if report.has_violations() { exit::VIOLATION } else { exit::OK })
}

fn print_human(hash: &str, report: &Report) {
    let total = report.checked.len() + report.unchecked.len();
    let langs: Vec<String> =
        report.by_lang.iter().map(|(l, n)| format!("{n} {}", l.name())).collect();
    println!(
        "kiến trúc {hash} · {} file quét ({}) · {}/{total} component trong diện kiểm\n",
        report.files_scanned,
        if langs.is_empty() { "không có file nào hỗ trợ".to_string() } else { langs.join(", ") },
        report.checked.len(),
    );

    for v in &report.violations {
        let what = match v.reason {
            Reason::Denied => "bị cấm",
            Reason::NoAllow => "không được phép",
        };
        println!("✗ VI PHẠM  {} -> {}  ({what}, {})", v.from, v.to, v.status.as_str());
        println!("  {}:{}  import '{}'", v.file, v.line, v.spec);
        println!("  luật: {}", v.rule);
        println!("  → {}\n", v.hint);
    }

    for u in &report.unresolved {
        println!("⚠ CHƯA PHÂN GIẢI  '{}'", u.spec);
        println!("  {}:{}  — {}\n", u.file, u.line, u.reason.describe(u.lang));
    }

    for (file, owners) in &report.ambiguous {
        println!("⚠ MẪU CHỒNG NHAU  {file} khớp cả {}\n", owners.join(" và "));
    }

    if !report.unchecked.is_empty() {
        println!(
            "⚠ CHƯA KIỂM  {} — chưa có allow nào nên tạm để yên",
            report.unchecked.join(", ")
        );
    }
    if report.files_unmapped > 0 {
        println!("⚠ {} file không thuộc component nào trong sơ đồ", report.files_unmapped);
    }
    if report.rules_deferred > 0 {
        println!(
            "⚠ {} luật về store chưa kiểm được, cần phân tích truy cập dữ liệu",
            report.rules_deferred
        );
    }

    let warnings = report.unresolved.len()
        + report.ambiguous.len()
        + usize::from(!report.unchecked.is_empty())
        + usize::from(report.files_unmapped > 0)
        + usize::from(report.rules_deferred > 0);
    println!("\n{} vi phạm, {warnings} cảnh báo", report.violations.len());
}

fn print_json(hash: &str, report: &Report) {
    let violations: Vec<String> = report
        .violations
        .iter()
        .map(|v| {
            format!(
                r#"{{"from":{},"to":{},"file":{},"line":{},"spec":{},"reason":{},"rule":{},"status":{},"hint":{}}}"#,
                json::string(&v.from),
                json::string(&v.to),
                json::string(&v.file),
                v.line,
                json::string(&v.spec),
                json::string(match v.reason {
                    Reason::Denied => "denied",
                    Reason::NoAllow => "no_allow",
                }),
                json::string(&v.rule),
                json::string(v.status.as_str()),
                json::string(&v.hint),
            )
        })
        .collect();

    let unresolved: Vec<String> = report
        .unresolved
        .iter()
        .map(|u| {
            format!(
                r#"{{"file":{},"line":{},"spec":{},"reason":{},"lang":{}}}"#,
                json::string(&u.file),
                u.line,
                json::string(&u.spec),
                json::string(u.reason.describe(u.lang)),
                json::string(u.lang.name()),
            )
        })
        .collect();

    println!(
        r#"{{"archHash":{},"filesScanned":{},"filesUnmapped":{},"checked":{},"unchecked":{},"rulesDeferred":{},"violations":[{}],"unresolved":[{}]}}"#,
        json::string(hash),
        report.files_scanned,
        report.files_unmapped,
        json::strings(report.checked.iter().map(String::as_str)),
        json::strings(report.unchecked.iter().map(String::as_str)),
        report.rules_deferred,
        violations.join(","),
        unresolved.join(","),
    );
}

fn cmd_diff(args: &[String]) -> ExitCode {
    let mut git_ref = "HEAD".to_string();
    let mut root_arg: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--root" => match args.get(i + 1) {
                Some(p) => {
                    root_arg = Some(PathBuf::from(p));
                    i += 1;
                }
                None => {
                    eprintln!("archctl diff: `--root` thiếu đường dẫn");
                    return ExitCode::from(exit::USAGE);
                }
            },
            other if other.starts_with('-') => {
                eprintln!("archctl diff: tham số không nhận ra: `{other}`");
                return ExitCode::from(exit::USAGE);
            }
            other => git_ref = other.to_string(),
        }
        i += 1;
    }

    let start = root_arg.unwrap_or_else(|| PathBuf::from("."));
    let Some(root) = arch_core::find_root(&start) else {
        eprintln!("archctl: không tìm thấy `{}`", arch_core::ARCH_FILE);
        return ExitCode::from(exit::USAGE);
    };

    let head_src = match std::fs::read_to_string(root.join(arch_core::ARCH_FILE)) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("archctl: không đọc được file luật: {e}");
            return ExitCode::from(exit::USAGE);
        }
    };
    let Some(base_src) = git_show(&root, &git_ref) else {
        eprintln!(
            "archctl: không lấy được `{}:{}` từ git.\n\
             → kiểm tra ref, hoặc dự án chưa commit file luật lần nào",
            git_ref,
            arch_core::ARCH_FILE
        );
        return ExitCode::from(exit::USAGE);
    };

    let (base, base_diags) = arch_core::load(&base_src);
    let (head, head_diags) = arch_core::load(&head_src);
    for (label, diags) in [("bản trước", &base_diags), ("bản hiện tại", &head_diags)] {
        if diag::has_errors(diags) {
            eprintln!("archctl: {label} có lỗi cú pháp, không so sánh được");
            return ExitCode::from(exit::DSL_INVALID);
        }
    }

    let base_hash = arch_core::arch_hash(&base);
    let head_hash = arch_core::arch_hash(&head);
    let delta = arch_core::diff::diff(&base, &head);

    if base_hash == head_hash {
        println!("kiến trúc {head_hash} — không đổi so với {git_ref}");
        if delta.moved > 0 {
            println!("{} phần tử chỉ xê dịch trong file, không cần đọc kỹ", delta.moved);
        }
        return ExitCode::from(exit::OK);
    }

    println!("kiến trúc {base_hash} → {head_hash}  (so với {git_ref})\n");
    for c in &delta.changes {
        let marker = if c.loosening { '!' } else { c.kind.marker() };
        print!("{marker} {:<10} {}", c.element_kind, c.key);
        match (c.kind, &c.before, &c.after) {
            (Kind::Changed, Some(before), Some(after)) => {
                println!("\n    {before}\n  → {after}  ({})", c.class.label());
            }
            // Luật và invariant lấy chính nội dung làm danh tính, nên in lại
            // body chỉ là lặp.
            (_, _, Some(body)) | (_, Some(body), _) if !body.is_empty() && *body != c.key => {
                println!("\n    {body}  ({})", c.class.label());
            }
            _ => println!("  ({})", c.class.label()),
        }
    }

    println!(
        "\n{} cấu trúc · {} ngữ nghĩa · {} quản trị · {} chỉ xê dịch trong file",
        delta.count(Class::Topology),
        delta.count(Class::Semantic),
        delta.count(Class::Governance),
        delta.moved,
    );
    if delta.architectural() == 0 {
        println!("Không có thay đổi cấu trúc hay ngữ nghĩa nào.");
    }
    let loosened: Vec<&str> = delta.loosenings().map(|c| c.key.as_str()).collect();
    if !loosened.is_empty() {
        println!("\n! Luật bị nới lỏng: {}  — đọc kỹ chỗ này", loosened.join(", "));
    }
    ExitCode::from(exit::OK)
}

/// Lấy nội dung file luật ở một ref của git. Dùng `git` có sẵn thay vì thư
/// viện, để giữ cây dependency bằng không.
fn git_show(root: &Path, git_ref: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("show")
        // `./` bắt git tính đường dẫn từ thư mục hiện tại, không phải từ gốc
        // repo — cần thiết khi dự án nằm trong thư mục con của repo.
        .arg(format!("{git_ref}:./{}", arch_core::ARCH_FILE))
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn cmd_parse(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("archctl parse: thiếu đường dẫn file");
        return ExitCode::from(exit::USAGE);
    };

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("archctl: không đọc được `{path}`: {e}");
            return ExitCode::from(exit::USAGE);
        }
    };

    let (doc, diags) = arch_core::load(&src);

    println!("{path} · kiến trúc {}\n", arch_core::arch_hash(&doc));
    println!(
        "{} component · {} store · {} quan hệ · {} flow · {} contract · {} boundary · {} invariant",
        doc.components.len(),
        doc.stores.len(),
        doc.rules.len(),
        doc.flows.len(),
        doc.contracts.len(),
        doc.boundaries.len(),
        doc.invariants.len(),
    );

    let locked = doc.components.iter().filter(|c| c.meta.status == Status::Locked).count()
        + doc.rules.iter().filter(|r| r.meta.status == Status::Locked).count();
    println!("{} luật có hiệu lực · {} trong đó đã khoá\n", doc.enforced_count(), locked);

    for d in &diags {
        let mark = if d.severity == Severity::Error { "✗" } else { "⚠" };
        println!("{mark} {d}");
    }

    let errors = diag::count(&diags, Severity::Error);
    let warnings = diag::count(&diags, Severity::Warning);
    if errors == 0 && warnings == 0 {
        println!("không có lỗi, không có cảnh báo");
    } else {
        println!("\n{errors} lỗi, {warnings} cảnh báo");
    }

    ExitCode::from(if errors > 0 { exit::DSL_INVALID } else { exit::OK })
}
