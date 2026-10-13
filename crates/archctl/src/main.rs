//! CLI của archctl.
//!
//! Tự phân tích tham số thay vì dùng thư viện, để giữ cây dependency bằng
//! không. Công cụ này chạy trong repo của người dùng và trong vòng lặp của
//! agent, nên mỗi dependency thêm vào là một bề mặt tấn công thêm.

use arch_core::check::{Reason, Report, Via};
use arch_core::diff::{Class, Kind};
use arch_core::protocol::{self as protocol, Intent, Verdict};
use arch_core::diag::{self, Diag, Severity};
use arch_core::model::Status;
use arch_core::{Document, json};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod proto;
mod serve;
mod setup;

/// In ra stdout, bỏ qua lỗi ghi.
///
/// `println!` panic khi stdout đóng sớm, nên `archctl check | head` làm chương
/// trình vỡ. Một công cụ dòng lệnh không được phép như vậy.
#[macro_export]
macro_rules! say {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($arg)*);
    }};
}

/// Như [`say!`] nhưng không xuống dòng.
#[macro_export]
macro_rules! say_inline {
    ($($arg:tt)*) => {{
        use std::io::Write as _;
        let _ = write!(std::io::stdout(), $($arg)*);
    }};
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Số mục in ra cho mỗi loại cảnh báo.
///
/// Trên một cây 5078 file, một lần `check` từng in ra 7247 dòng. Danh sách
/// dài tới mức đó thì người đọc bỏ qua cả những dòng đáng đọc, nên cắt bớt
/// làm nó hữu ích hơn chứ không phải kém đi. `--json` vẫn đưa đủ.
const MAX_LISTED: usize = 12;

/// Mã thoát, theo mục 7 của đặc tả.
pub mod exit {
    pub const OK: u8 = 0;
    pub const VIOLATION: u8 = 1;
    pub const DSL_INVALID: u8 = 2;
    /// Có đề xuất hoặc bế tắc đang chờ người quyết định.
    pub const PENDING: u8 = 3;
    pub const USAGE: u8 = 64;
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");

    match command {
        "parse" => cmd_parse(&args[1..]),
        "check" => cmd_check(&args[1..]),
        "scan" => cmd_scan(&args[1..]),
        "diff" => cmd_diff(&args[1..]),
        "propose" => proto::cmd_propose(&args[1..], Intent::Rule),
        "block" => proto::cmd_propose(&args[1..], Intent::Block),
        "accept" => proto::cmd_decide(&args[1..], Verdict::Accepted),
        "reject" => proto::cmd_decide(&args[1..], Verdict::Rejected),
        "proposals" => proto::cmd_proposals(&args[1..]),
        "init" => setup::cmd_init(&args[1..]),
        "baseline" => setup::cmd_baseline(&args[1..]),
        "export" => setup::cmd_export(&args[1..]),
        "verify" => setup::cmd_verify(&args[1..]),
        "serve" => serve::cmd_serve(&args[1..]),
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
    crate::say!(
        "archctl {VERSION} — sơ đồ kiến trúc là luật

Bắt đầu:
  archctl init [--from-code] [--no-hooks] Dựng .arch/ cho dự án
  archctl baseline                        Tha vi phạm có sẵn, chỉ chặn cái mới

Lệnh chung:
  archctl check [--root <dir>] [--json] [--changed] [--strict] [<file>...]
                                          Đối chiếu mã nguồn với luật.
                                          Cho thẳng đường dẫn thì nhanh nhất.
                                          --strict: mọi cảnh báo, kể cả
                                          của file luật, thành vi phạm.
  archctl scan [--root <dir>] [--json] [--verbatim] [<file>...]
                                          In import archctl đọc được, đích
                                          phân giải của từng cái, cạnh qua
                                          ranh giới ngôn ngữ, và service đọc
                                          từ docker-compose, Dockerfile.
                                          --verbatim: in cả đối số của lệnh
                                          và chỉ thị (có thể chứa secret).
                                          Không cần .arch/.
  archctl diff [<git-ref>] [--root <dir>] So luật hiện tại với một bản trước
  archctl parse <file.arch>               Chỉ đọc và kiểm tra file luật
  archctl verify [<tên>...] [--quiet]     Chạy các cách kiểm đã khai báo và
                                          ghi biên nhận. Lệnh duy nhất chạy
                                          lệnh shell, và chỉ khi người gõ nó.
  archctl proposals                       Liệt kê đề xuất đang chờ
  archctl serve [--port <n>]              Mở sơ đồ tương tác ở 127.0.0.1:4000
  archctl export [--view <tên>] [--violations] [--out <file>]
                                          Xuất sơ đồ Mermaid. Đuôi .md thì
                                          bọc sẵn để GitHub render được.

Lệnh của agent:
  archctl propose '<luật>' --reason \"...\"  Đề xuất thêm một luật
  archctl block --on '<luật>' --reason \"...\" Báo bế tắc, rồi dừng việc đó lại

Lệnh của người:
  archctl accept <id> --reason \"...\"       Duyệt một đề xuất
  archctl reject <id> --reason \"...\"       Từ chối, agent sẽ không đề xuất lại

`diff` mặc định so với HEAD. Nó tách thay đổi kiến trúc khỏi thay đổi chỉ
xê dịch chữ trong file, để người duyệt biết chỗ nào cần đọc kỹ.

Gốc dự án là thư mục gần nhất đi lên có chứa .arch/system.arch.

Mã thoát:
  0   không vi phạm
  1   có vi phạm luật đang có hiệu lực
  2   DSL sai cú pháp hoặc tham chiếu không tồn tại
  3   có đề xuất hoặc bế tắc đang chờ người quyết định
  64  dùng lệnh sai"
    );
}

/// Đọc file luật ở gốc đã cho. In chẩn đoán và trả `None` nếu DSL có lỗi.
///
/// Trả về cả cảnh báo của bộ kiểm tra mô hình, vì người gọi cần đếm chúng:
/// in ra ba dòng ⚠ rồi kết luận "0 cảnh báo" thì dòng cuối — dòng duy nhất
/// nhiều người đọc — nói ngược lại những dòng ngay trên nó.
fn load_arch(root: &Path, as_json: bool) -> Option<(Document, String, Vec<Diag>)> {
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
                crate::say!("✗ {}:{d}", arch_core::ARCH_FILE);
            }
        }
        return None;
    }
    // Cảnh báo của bộ kiểm tra mô hình cũng phải hiện ở đây. Trước đây chỉ
    // `archctl parse` in chúng, mà không ai chạy `parse` thường xuyên — nên
    // "contract này không có verify, đây chỉ là lời hứa" chưa bao giờ tới
    // được người cần đọc nó.
    let warnings: Vec<Diag> =
        diags.into_iter().filter(|d| d.severity == Severity::Warning).collect();
    if !as_json {
        for d in warnings.iter().take(MAX_LISTED) {
            crate::say!("⚠ {}:{d}", arch_core::ARCH_FILE);
            if let Some(hint) = &d.hint {
                crate::say!("  → {hint}");
            }
        }
        if let Some(rest) = warnings.len().checked_sub(MAX_LISTED).filter(|n| *n > 0) {
            crate::say!("⚠ … còn {rest} cảnh báo nữa trong file luật — archctl parse");
        }
        if !warnings.is_empty() {
            crate::say!();
        }
    }
    Some((doc, hash, warnings))
}

fn cmd_check(args: &[String]) -> ExitCode {
    let mut as_json = false;
    let mut changed_only = false;
    let mut strict = false;
    let mut only: Vec<String> = Vec::new();
    let mut root_arg: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => as_json = true,
            "--changed" => changed_only = true,
            "--strict" => strict = true,
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
            other if other.starts_with('-') => {
                eprintln!("archctl check: tham số không nhận ra: `{other}`");
                return ExitCode::from(exit::USAGE);
            }
            // Đường dẫn cho thẳng: hook đã biết file nào vừa đổi, nên không
            // cần hỏi git — bỏ được phần tốn thời gian nhất.
            other => only.push(other.to_string()),
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

    let Some((doc, hash, law)) = load_arch(&root, as_json) else {
        return ExitCode::from(exit::DSL_INVALID);
    };

    let ignores = arch_core::read_ignores(&root);
    let subset = if only.is_empty() {
        changed_only.then(|| changed_files(&root)).flatten()
    } else {
        Some(relative_to(&root, &only))
    };
    let report = match subset {
        Some(files) => arch_core::check::check_files(&root, &doc, &files, true),
        None => arch_core::check::check(&root, &doc, &ignores),
    };
    let memory = remember(&root, &hash, &report);

    if as_json {
        print_json(&hash, &report, &law);
    } else {
        print_human(&hash, &report, &memory, law.len());
    }

    let fresh = arch_core::baseline::split(&report.violations, &memory.baseline).fresh.len();
    // `--strict` coi cảnh báo là vi phạm. Dành cho CI: một import không phân
    // giải được nghĩa là có một phần mã đang không được kiểm, và trên nhánh
    // chính thì đó không nên là chuyện bỏ qua được.
    //
    // Cảnh báo của chính file luật tính vào đây. Chúng là loại tệ nhất trong
    // các loại "chưa kiểm được": `contract` không có verify hay `invariant`
    // chỉ là ghi chú nghĩa là luật *có* mà không ai kiểm, và trên nhánh chính
    // một luật như vậy đọc lên y như một luật đang được áp.
    //
    // `report.unchecked` (component chưa có `allow`) và `files_unmapped` thì
    // **không** tính, dù chúng vẫn được in và vẫn vào dòng tổng kết. Chúng
    // không phải chỗ hỏng mà là phần chưa khai báo, và áp dần là cách dùng
    // được thiết kế. Tính chúng vào đây nghĩa là không ai bật được `--strict`
    // cho tới khi đã mô tả xong cả repo.
    let unchecked = report.unresolved.len()
        + report.ambiguous.len()
        + report.missing.len()
        + report.vacuous.len()
        + report.oversized.len()
        + report.unproven.len()
        + report.deploy_unread.len()
        + report.entry_unknown.len()
        + report.links_unseen.len()
        + law.len();
    let failed = fresh > 0 || (strict && unchecked > 0);
    if strict && fresh == 0 && unchecked > 0 {
        crate::say!("\n--strict: {unchecked} chỗ chưa kiểm được, coi như vi phạm");
    }
    ExitCode::from(if failed { exit::VIOLATION } else { exit::OK })
}

/// Những gì `check` nhớ giữa các lần chạy: kiến trúc có đổi không, và mỗi vi
/// phạm đã gặp lại bao nhiêu lần.
struct Memory {
    previous_hash: String,
    attempts: std::collections::BTreeMap<String, usize>,
    pending: Vec<(Intent, String, String)>,
    /// Vi phạm đã có từ trước khi áp luật. Được tha, nhưng vẫn đếm.
    baseline: std::collections::BTreeSet<String>,
    baseline_stale: usize,
}

fn violation_key(v: &arch_core::check::Violation) -> String {
    format!("{}->{}|{}", v.from, v.to, v.file)
}

fn remember(root: &Path, hash: &str, report: &Report) -> Memory {
    let path = root.join(protocol::STATE_FILE);
    let previous = protocol::parse_state(&std::fs::read_to_string(&path).unwrap_or_default());

    // Chỉ đếm những vi phạm còn tồn tại: sửa xong thì bộ đếm về không.
    let mut attempts = std::collections::BTreeMap::new();
    for v in &report.violations {
        let key = violation_key(v);
        let n = previous.attempts.get(&key).copied().unwrap_or(0) + 1;
        attempts.insert(key, n);
    }
    let state = protocol::State { arch_hash: hash.to_string(), attempts: attempts.clone() };
    let _ = arch_core::write_atomic(&path, &protocol::render_state(&state));

    let (proposals, _) = protocol::parse_proposals(
        &std::fs::read_to_string(root.join(protocol::PROPOSALS_FILE)).unwrap_or_default(),
    );
    let baseline = arch_core::baseline::parse(
        &std::fs::read_to_string(root.join(arch_core::baseline::BASELINE_FILE))
            .unwrap_or_default(),
    );
    Memory {
        previous_hash: previous.arch_hash,
        attempts,
        pending: proposals.into_iter().map(|p| (p.intent, p.id, p.statement)).collect(),
        baseline_stale: arch_core::baseline::stale(&report.violations, &baseline),
        baseline,
    }
}

fn print_human(hash: &str, report: &Report, memory: &Memory, law_warnings: usize) {
    // Báo trước mọi thứ khác: agent đang làm theo một bản luật đã cũ.
    if !memory.previous_hash.is_empty() && memory.previous_hash != hash {
        crate::say!(
            "⚠ Kiến trúc đã thay đổi kể từ lần kiểm trước ({} → {hash})",
            memory.previous_hash
        );
        crate::say!("  Đọc lại {} trước khi tiếp tục.\n", arch_core::ARCH_FILE);
    }

    let total = report.checked.len() + report.unchecked.len();
    let langs: Vec<String> =
        report.by_lang.iter().map(|(l, n)| format!("{n} {}", l.name())).collect();
    // Quét một phần thì con số tổng vô nghĩa, nói rõ phạm vi thay vì in chúng.
    if report.partial {
        let deploy = if report.deploy_checked { " · và cấu hình triển khai" } else { "" };
        crate::say!("kiến trúc {hash} · chỉ quét {} file vừa đổi{deploy}\n", report.files_scanned);
    } else {
        crate::say!(
            "kiến trúc {hash} · {} file quét ({}) · {}/{total} component trong diện kiểm\n",
            report.files_scanned,
            if langs.is_empty() {
                "không có file nào hỗ trợ".to_string()
            } else {
                langs.join(", ")
            },
            report.checked.len(),
        );
    }

    let split = arch_core::baseline::split(&report.violations, &memory.baseline);
    for v in &split.fresh {
        let what = match v.reason {
            Reason::Denied => "bị cấm",
            Reason::NoAllow => "không được phép",
            Reason::NotOwner => "không phải chủ sở hữu",
            Reason::Leaked => "lọt ra ngoài vùng",
            Reason::NotLinked => "cạnh mạng không có link",
            Reason::NotRun => "không có trong runs",
        };
        match v.reason {
            Reason::NotRun => crate::say!(
                "✗ VI PHẠM  {} chạy trong service {}  ({what}, {})",
                v.to, v.from, v.status.as_str()
            ),
            _ => crate::say!("✗ VI PHẠM  {} -> {}  ({what}, {})", v.from, v.to, v.status.as_str()),
        }
        match v.via {
            Via::Import => crate::say!("  {}:{}  import '{}'", v.file, v.line, v.spec),
            Via::Table if v.count > 1 => crate::say!(
                "  {}:{}  chạm bảng `{}` ({} lần trong file)",
                v.file, v.line, v.spec, v.count
            ),
            Via::Table => crate::say!("  {}:{}  chạm bảng `{}`", v.file, v.line, v.spec),
            Via::Symbol if v.count > 1 => {
                crate::say!("  {}:{}  dùng `{}` ({} lần trong file)", v.file, v.line, v.spec, v.count)
            }
            Via::Symbol => crate::say!("  {}:{}  dùng `{}`", v.file, v.line, v.spec),
            Via::Ffi => crate::say!(
                "  {}:{}  gọi `{}` qua ranh giới ngôn ngữ",
                v.file, v.line, v.spec
            ),
            Via::Network | Via::Entry => crate::say!("  {}:{}  {}", v.file, v.line, v.spec),
            Via::Rpc => crate::say!("  {}:{}  gọi gRPC `{}`", v.file, v.line, v.spec),
        }
        crate::say!("  luật: {}", v.rule);
        crate::say!("  → {}", v.hint);

        // Thử đi thử lại cùng một chỗ nghĩa là agent không sửa được. Nó cần
        // một lối ra, nếu không nó sẽ đi tìm đường lách.
        match memory.attempts.get(&violation_key(v)).copied().unwrap_or(1) {
            n if n >= protocol::MAX_TRIES => {
                crate::say!("  ! đã gặp lại {n} lần — không sửa được thì dừng lại và:");
                // `deny` có một luật cụ thể để báo bế tắc. `NoAllow` thì không
                // có luật nào chặn cả — thứ còn thiếu là một `allow`, nên lối
                // ra đúng là đề xuất thêm nó.
                match v.reason {
                    Reason::Denied => crate::say!(
                        "    archctl block --on '{}' --reason \"...\"",
                        v.rule
                    ),
                    Reason::Leaked => crate::say!(
                        "    archctl block --on '{}' --reason \"...\"",
                        v.rule
                    ),
                    Reason::NoAllow | Reason::NotOwner => crate::say!(
                        "    archctl propose 'allow {} -> {}' --reason \"...\"",
                        v.from, v.to
                    ),
                    // Đích chưa khai: `link` tới nó không hợp lệ cho tới khi có
                    // dòng `service`.
                    Reason::NotLinked if arch_core::check::to_undeclared(v) => crate::say!(
                        "    archctl propose 'service {} from compose:{}' --reason \"...\"   (rồi `link {} -> {}`)",
                        arch_core::discover::pascal_case(&v.to),
                        v.to,
                        v.from,
                        arch_core::discover::pascal_case(&v.to)
                    ),
                    Reason::NotLinked => crate::say!(
                        "    archctl propose 'link {} -> {}' --reason \"...\"",
                        v.from, v.to
                    ),
                    // `runs` nằm trong dòng `service`, mà đề xuất chỉ thêm
                    // dòng mới: sửa dòng đó là việc của người.
                    Reason::NotRun => crate::say!(
                        "    báo người sửa `runs` của service {} trong {}",
                        v.from,
                        arch_core::ARCH_FILE
                    ),
                }
            }
            n if n > 1 => crate::say!("  (lần thứ {n})"),
            _ => {}
        }
        crate::say!();
    }

    // Khai báo nói một đằng, mã làm một nẻo. Không phải vi phạm luật, nhưng
    // là dấu hiệu sơ đồ đã lạc hậu — và sơ đồ sai còn tệ hơn không có sơ đồ.
    for m in report.missing.iter().take(MAX_LISTED) {
        crate::say!("⚠ KHAI BÁO LẠC HẬU  {} `{}`", m.source, m.name);
        crate::say!("  {}:{}  — {}\n", arch_core::ARCH_FILE, m.line, m.hint);
    }

    // Pass nhưng pass vô nghĩa. Nguy hiểm hơn một cảnh báo thường: người đọc
    // tin rằng chỗ đó đã được kiểm.
    for v in report.vacuous.iter().take(MAX_LISTED) {
        crate::say!("⚠ KIỂM KHÔNG KẾT LUẬN  {} `{}`", v.source, v.name);
        crate::say!(
            "  {}:{}  — `{}` được {} trong {}/{} file nên tìm thấy nó không nói lên điều gì",
            arch_core::ARCH_FILE,
            v.line,
            v.name,
            if v.by_declaration { "khai báo" } else { "nhắc tới" },
            v.files,
            v.of
        );
        crate::say!("  → đổi sang một tên chỉ thuộc về bước này\n");
    }

    // Gộp theo tên: một header sinh lúc build có thể bị include ở hàng trăm
    // chỗ, mà in cả trăm lần thì không nói thêm được gì.
    let mut grouped: Vec<(&str, &arch_core::check::Unresolved, usize)> = Vec::new();
    for u in &report.unresolved {
        match grouped.iter_mut().find(|(spec, _, _)| *spec == u.spec) {
            Some((_, _, n)) => *n += 1,
            None => grouped.push((&u.spec, u, 1)),
        }
    }
    for (spec, first, n) in grouped.iter().take(MAX_LISTED) {
        let more = if *n > 1 { format!("  ({n} chỗ)") } else { String::new() };
        crate::say!("⚠ CHƯA PHÂN GIẢI  '{spec}'{more}");
        crate::say!("  {}:{}  — {}\n", first.file, first.line, first.reason.describe(first.lang));
    }
    if let Some(rest) = grouped.len().checked_sub(MAX_LISTED).filter(|n| *n > 0) {
        crate::say!("⚠ … còn {rest} tên nữa chưa phân giải được — xem `--json`\n");
    }

    // Một lời hứa có `verify` trỏ tới đâu đó vẫn chỉ là lời hứa, chừng nào
    // chưa ai chạy nó trên đúng bản mã này.
    for (name, why) in report.unproven.iter().take(MAX_LISTED) {
        crate::say!("⚠ CHƯA CHỨNG MINH  cách kiểm `{name}` — {}", why.describe());
        crate::say!("  → archctl verify {name}\n");
    }

    // Bỏ qua thì bỏ qua, nhưng không được im lặng: đó là mã đang không kiểm.
    for (file, bytes) in report.oversized.iter().take(MAX_LISTED) {
        crate::say!("⚠ QUÁ LỚN  {file} ({} MB) — không dò", bytes / (1 << 20));
        crate::say!(
            "  → file sinh ra hay đã minify thì thêm vào {}\n",
            arch_core::IGNORE_FILE
        );
    }

    for (file, owners) in report.ambiguous.iter().take(MAX_LISTED) {
        crate::say!("⚠ MẪU CHỒNG NHAU  {file} khớp cả {}\n", owners.join(" và "));
    }

    // Tầng hạ tầng: thứ không đọc được thì nói ra, không im lặng (TM-12).
    for u in report.deploy_unread.iter().take(MAX_LISTED) {
        crate::say!("⚠ CHƯA ĐỌC ĐƯỢC  {} — {}\n", u.file, u.why);
    }
    for (service, why) in report.entry_unknown.iter().take(MAX_LISTED) {
        crate::say!("⚠ KHÔNG RÕ ĐIỂM VÀO  service {service} — {why}");
        crate::say!("  → `runs` của nó chỉ được kiểm theo cạnh import\n");
    }
    for (from, to, line) in report.links_unseen.iter().take(MAX_LISTED) {
        crate::say!("⚠ LINK KHÔNG THẤY TRONG CẤU HÌNH  link {from} -> {to}");
        crate::say!("  {}:{line}  — không compose nào cho thấy {from} gọi {to}\n", arch_core::ARCH_FILE);
    }

    for (label, shown, total) in [
        ("khai báo lạc hậu", MAX_LISTED, report.missing.len()),
        ("kiểm không kết luận", MAX_LISTED, report.vacuous.len()),
        ("file quá lớn", MAX_LISTED, report.oversized.len()),
        ("mẫu chồng nhau", MAX_LISTED, report.ambiguous.len()),
        ("cách kiểm chưa chứng minh", MAX_LISTED, report.unproven.len()),
        ("file triển khai chưa đọc được", MAX_LISTED, report.deploy_unread.len()),
        ("service không rõ điểm vào", MAX_LISTED, report.entry_unknown.len()),
        ("link không thấy trong cấu hình", MAX_LISTED, report.links_unseen.len()),
    ] {
        if let Some(rest) = total.checked_sub(shown).filter(|n| *n > 0) {
            crate::say!("⚠ … còn {rest} {label} nữa — xem `--json`\n");
        }
    }

    if !report.unchecked.is_empty() && !report.partial {
        crate::say!(
            "⚠ CHƯA KIỂM  {} — chưa có allow nào nên tạm để yên",
            report.unchecked.join(", ")
        );
    }
    if report.files_unmapped > 0 && !report.partial {
        crate::say!("⚠ {} file không thuộc component nào trong sơ đồ", report.files_unmapped);
    }
    if !report.services_undeclared.is_empty() {
        let names: Vec<String> = report
            .services_undeclared
            .iter()
            .map(|(file, line, name)| format!("{name} ({file}:{line})"))
            .collect();
        crate::say!("⚠ SERVICE CHƯA KHAI  {}", names.join(", "));
    }

    if !split.grandfathered.is_empty() {
        crate::say!(
            "⊖ {} vi phạm có sẵn từ trước, đang được tha theo `{}`",
            split.grandfathered.len(),
            arch_core::baseline::BASELINE_FILE
        );
    }
    // Phải báo kể cả khi không còn mục nào được tha — đó chính là lúc sửa
    // xong hết, và là lúc đáng siết bánh răng nhất.
    if memory.baseline_stale > 0 {
        crate::say!(
            "⊖ {} mục trong baseline đã sửa xong — chạy `archctl baseline` để siết lại",
            memory.baseline_stale
        );
    }
    if !split.grandfathered.is_empty() || memory.baseline_stale > 0 {
        crate::say!();
    }

    for (intent, id, statement) in &memory.pending {
        let what = match intent {
            Intent::Rule => "ĐANG CHỜ DUYỆT",
            Intent::Block => "ĐANG CHỜ GỠ BẾ TẮC",
        };
        crate::say!("⏸ {what}  {id}  {statement}");
    }

    let warnings = law_warnings
        + report.missing.len()
        + report.vacuous.len()
        + report.unproven.len()
        + report.oversized.len()
        + report.unresolved.len()
        + report.ambiguous.len()
        + report.deploy_unread.len()
        + report.entry_unknown.len()
        + report.links_unseen.len()
        + usize::from(!report.unchecked.is_empty())
        + usize::from(report.files_unmapped > 0)
        + usize::from(!report.services_undeclared.is_empty());
    crate::say!("\n{} vi phạm mới, {warnings} cảnh báo", split.fresh.len());
    if !memory.pending.is_empty() {
        crate::say!("{} đề xuất đang chờ người quyết định — archctl proposals", memory.pending.len());
    }
}

fn print_json(hash: &str, report: &Report, law: &[Diag]) {
    let violations: Vec<String> = report
        .violations
        .iter()
        .map(|v| {
            format!(
                r#"{{"from":{},"to":{},"file":{},"line":{},"spec":{},"via":{},"count":{},"reason":{},"rule":{},"status":{},"hint":{}}}"#,
                json::string(&v.from),
                json::string(&v.to),
                json::string(&v.file),
                v.line,
                json::string(&v.spec),
                json::string(match v.via {
                    Via::Import => "import",
                    Via::Symbol => "symbol",
                    Via::Table => "table",
                    Via::Ffi => "ffi",
                    Via::Network => "network",
                    Via::Entry => "entry",
                    Via::Rpc => "rpc",
                }),
                v.count,
                json::string(match v.reason {
                    Reason::Denied => "denied",
                    Reason::NoAllow => "no_allow",
                    Reason::NotOwner => "not_owner",
                    Reason::Leaked => "leaked",
                    Reason::NotLinked => "not_linked",
                    Reason::NotRun => "not_run",
                }),
                json::string(&v.rule),
                json::string(v.status.as_str()),
                json::string(&v.hint),
            )
        })
        .collect();

    let missing: Vec<String> = report
        .missing
        .iter()
        .map(|m| {
            format!(
                r#"{{"name":{},"source":{},"line":{},"hint":{}}}"#,
                json::string(&m.name),
                json::string(&m.source),
                m.line,
                json::string(&m.hint),
            )
        })
        .collect();

    let vacuous: Vec<String> = report
        .vacuous
        .iter()
        .map(|v| {
            format!(
                r#"{{"name":{},"source":{},"line":{},"files":{},"of":{},"byDeclaration":{}}}"#,
                json::string(&v.name),
                json::string(&v.source),
                v.line,
                v.files,
                v.of,
                v.by_declaration,
            )
        })
        .collect();

    // Bản người đọc bị cắt ở mười hai mục và bảo "xem `--json`", nên bản JSON
    // phải có đủ mọi loại — nếu không thì CI và hook không bao giờ thấy chúng.
    let unproven: Vec<String> = report
        .unproven
        .iter()
        .map(|(name, why)| {
            format!(r#"{{"name":{},"why":{}}}"#, json::string(name), json::string(why.describe()))
        })
        .collect();

    let oversized: Vec<String> = report
        .oversized
        .iter()
        .map(|(file, bytes)| format!(r#"{{"file":{},"bytes":{bytes}}}"#, json::string(file)))
        .collect();

    let ambiguous: Vec<String> = report
        .ambiguous
        .iter()
        .map(|(file, owners)| {
            format!(
                r#"{{"file":{},"owners":{}}}"#,
                json::string(file),
                json::strings(owners.iter().map(String::as_str))
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

    // Cảnh báo của chính file luật cũng vào đây, vì `--strict` tính chúng là
    // vi phạm. Thiếu chúng thì bản JSON và mã thoát nói hai chuyện khác nhau.
    let law_warnings: Vec<String> = law
        .iter()
        .map(|d| {
            format!(
                r#"{{"line":{},"msg":{},"hint":{}}}"#,
                d.line,
                json::string(&d.msg),
                match &d.hint {
                    Some(h) => json::string(h),
                    None => "null".to_string(),
                }
            )
        })
        .collect();

    let deploy_unread: Vec<String> = report
        .deploy_unread
        .iter()
        .map(|u| format!(r#"{{"file":{},"why":{}}}"#, json::string(&u.file), json::string(&u.why)))
        .collect();
    let entry_unknown: Vec<String> = report
        .entry_unknown
        .iter()
        .map(|(s, why)| format!(r#"{{"service":{},"why":{}}}"#, json::string(s), json::string(why)))
        .collect();
    let undeclared: Vec<String> = report
        .services_undeclared
        .iter()
        .map(|(file, line, name)| {
            format!(r#"{{"file":{},"line":{line},"name":{}}}"#, json::string(file), json::string(name))
        })
        .collect();
    let unseen: Vec<String> = report
        .links_unseen
        .iter()
        .map(|(from, to, line)| {
            format!(r#"{{"from":{},"to":{},"line":{line}}}"#, json::string(from), json::string(to))
        })
        .collect();

    crate::say!(
        r#"{{"archHash":{},"filesScanned":{},"filesUnmapped":{},"checked":{},"unchecked":{},"violations":[{}],"unresolved":[{}],"missing":[{}],"vacuous":[{}],"unproven":[{}],"oversized":[{}],"ambiguous":[{}],"deployUnread":[{}],"entryUnknown":[{}],"servicesUndeclared":[{}],"linksUnseen":[{}],"lawWarnings":[{}]}}"#,
        json::string(hash),
        report.files_scanned,
        report.files_unmapped,
        json::strings(report.checked.iter().map(String::as_str)),
        json::strings(report.unchecked.iter().map(String::as_str)),
        violations.join(","),
        unresolved.join(","),
        missing.join(","),
        vacuous.join(","),
        unproven.join(","),
        oversized.join(","),
        ambiguous.join(","),
        deploy_unread.join(","),
        entry_unknown.join(","),
        undeclared.join(","),
        unseen.join(","),
        law_warnings.join(","),
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
        crate::say!("kiến trúc {head_hash} — không đổi so với {git_ref}");
        if delta.moved > 0 {
            crate::say!("{} phần tử chỉ xê dịch trong file, không cần đọc kỹ", delta.moved);
        }
        return ExitCode::from(exit::OK);
    }

    crate::say!("kiến trúc {base_hash} → {head_hash}  (so với {git_ref})\n");
    for c in &delta.changes {
        let marker = if c.loosening { '!' } else { c.kind.marker() };
        crate::say_inline!("{marker} {:<10} {}", c.element_kind, c.key);
        match (c.kind, &c.before, &c.after) {
            (Kind::Changed, Some(before), Some(after)) => {
                crate::say!("\n    {before}\n  → {after}  ({})", c.class.label());
            }
            // Luật và invariant lấy chính nội dung làm danh tính, nên in lại
            // body chỉ là lặp.
            (_, _, Some(body)) | (_, Some(body), _) if !body.is_empty() && *body != c.key => {
                crate::say!("\n    {body}  ({})", c.class.label());
            }
            _ => crate::say!("  ({})", c.class.label()),
        }
    }

    crate::say!(
        "\n{} cấu trúc · {} ngữ nghĩa · {} quản trị · {} chỉ xê dịch trong file",
        delta.count(Class::Topology),
        delta.count(Class::Semantic),
        delta.count(Class::Governance),
        delta.moved,
    );
    if delta.architectural() == 0 {
        crate::say!("Không có thay đổi cấu trúc hay ngữ nghĩa nào.");
    }
    let loosened: Vec<&str> = delta.loosenings().map(|c| c.key.as_str()).collect();
    if !loosened.is_empty() {
        crate::say!("\n! Luật bị nới lỏng: {}  — đọc kỹ chỗ này", loosened.join(", "));
    }
    ExitCode::from(exit::OK)
}

/// `archctl scan`: archctl đọc được gì từ mã, và mỗi import trỏ tới đâu.
///
/// Để người dùng hỏi thẳng "vì sao cạnh này không có", và để bộ đo đối chiếu
/// với parser chuẩn của từng ngôn ngữ: phép nhất quán của bộ đo so archctl với
/// chính nó, nên không bắt được lỗi nằm trong bộ trích. Đọc đúng như `check`
/// đọc — cùng danh sách file, cùng phần mã của file, cùng bộ phân giải.
///
/// Không cần `.arch/`: `--root` là gốc, không thì thư mục gần nhất có
/// `.arch/system.arch`, không nữa thì thư mục hiện tại.
fn cmd_scan(args: &[String]) -> ExitCode {
    use arch_core::resolve::{Resolved, Resolver};
    use arch_core::scan::{self, Lang};

    let mut as_json = false;
    let mut verbatim = false;
    let mut only: Vec<String> = Vec::new();
    let mut root_arg: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => as_json = true,
            "--verbatim" => verbatim = true,
            "--root" => match args.get(i + 1) {
                Some(p) => {
                    root_arg = Some(PathBuf::from(p));
                    i += 1;
                }
                None => {
                    eprintln!("archctl scan: `--root` thiếu đường dẫn");
                    return ExitCode::from(exit::USAGE);
                }
            },
            other if other.starts_with('-') => {
                eprintln!("archctl scan: tham số không nhận ra: `{other}`");
                return ExitCode::from(exit::USAGE);
            }
            other => only.push(other.to_string()),
        }
        i += 1;
    }
    let root = root_arg
        .or_else(|| arch_core::find_root(Path::new(".")))
        .unwrap_or_else(|| PathBuf::from("."));
    let files = if only.is_empty() {
        arch_core::check::source_files(&root, &arch_core::read_ignores(&root))
    } else {
        relative_to(&root, &only)
            .into_iter()
            .filter(|f| scan::is_supported(f) || scan::deploy::touches_deploy(f) || f.ends_with(".proto"))
            .collect()
    };
    // Cho thẳng tên file thì đọc cả file trong thư mục test hay ví dụ: người
    // gõ đã chọn nó.
    let deploy_files: Vec<String> = if only.is_empty() {
        arch_core::check::deploy_files(&root, &arch_core::read_ignores(&root))
    } else {
        files
            .iter()
            .filter(|f| {
                let name = f.rsplit('/').next().unwrap_or(f);
                scan::deploy::is_compose(name) || scan::deploy::is_dockerfile(name)
            })
            .cloned()
            .collect()
    };

    let resolver = Resolver::new(&root);
    // Bảng tra `.proto`: chỗ dò ra mà không phải gRPC (`QuantStub()` không có
    // `.proto` nào khai `Quant`) thì không in.
    let mut proto_index = scan::grpc::ProtoIndex::default();
    for p in arch_core::check::proto_files(&root, &arch_core::read_ignores(&root)) {
        if let Ok(text) = scan::deploy::read_in(&root, &p, arch_core::check::MAX_FILE_BYTES) {
            proto_index.add(&p, &scan::grpc::read_proto(&text));
        }
    }
    let rpc_of = |file: &str, list: &[scan::grpc::Site]| -> Vec<scan::grpc::Site> {
        if scan::grpc::generated_stub_path(file) {
            return Vec::new();
        }
        list.iter().filter(|s| !matches!(proto_index.key(s), scan::grpc::Key::NotRpc)).cloned().collect()
    };
    let mut json_files: Vec<String> = Vec::new();
    for file in &files {
        let Some(lang) = Lang::for_path(file) else { continue };
        let src = match std::fs::read_to_string(root.join(file)) {
            Ok(src) if src.len() as u64 <= arch_core::check::MAX_FILE_BYTES => src,
            Ok(_) => {
                json_files.push(format!(
                    r#"{{"file":{},"lang":{},"skipped":"oversized","imports":[],"crossings":{{"calls":[],"provides":[]}}}}"#,
                    json::string(file),
                    json::string(lang.name())
                ));
                if !as_json {
                    say!("{file} ({}): bỏ qua, quá lớn", lang.name());
                }
                continue;
            }
            Err(_) => {
                json_files.push(format!(
                    r#"{{"file":{},"lang":{},"skipped":"unreadable","imports":[],"crossings":{{"calls":[],"provides":[]}}}}"#,
                    json::string(file),
                    json::string(lang.name())
                ));
                if !as_json {
                    say!("{file} ({}): bỏ qua, không đọc được dạng UTF-8", lang.name());
                }
                continue;
            }
        };
        let view = scan::source_view(file, arch_core::strip_bom(&src));
        let scanned = lang.scan(&view, &[]);
        if !as_json {
            say!("{file} ({})", lang.name());
        }
        let mut items: Vec<String> = Vec::new();
        for import in &scanned.imports {
            let base = resolver.resolve(lang, file, import);
            let subs = resolver.submodules(lang, file, import, &base);
            let (kind, target) = match &base {
                Resolved::Path(p) => ("path", Some(p.as_str())),
                Resolved::Package(p) => ("package", Some(p.as_str())),
                Resolved::Alias => ("alias", None),
                Resolved::Internal => ("internal", None),
                Resolved::Missing => ("missing", None),
            };
            if as_json {
                items.push(format!(
                    r#"{{"line":{},"spec":{},"system":{},"names":{},"resolved":{},"target":{},"submodules":{}}}"#,
                    import.line,
                    json::string(&import.spec),
                    import.system,
                    json::strings(import.names.iter().map(String::as_str)),
                    json::string(kind),
                    target.map_or_else(|| "null".to_string(), json::string),
                    json::strings(subs.iter().map(String::as_str)),
                ));
            } else {
                let shown = match &base {
                    Resolved::Path(p) => p.clone(),
                    Resolved::Package(p) => format!("gói ngoài {p}"),
                    Resolved::Alias => "alias chưa đọc được cấu hình".to_string(),
                    Resolved::Internal => "bên trong chính crate".to_string(),
                    Resolved::Missing => "không tìm thấy".to_string(),
                };
                say!("  {:>5}  {}  → {shown}", import.line, import.spec);
                for sub in &subs {
                    say!("  {:>5}  {}  → {sub} (module con)", "", import.spec);
                }
            }
        }
        // Cạnh liên ngôn ngữ, khoá thô như bộ quét thấy: `TORCH_EXTENSION_NAME`
        // còn là `@ext`, chưa thay bằng tên trong `setup.py`.
        let foreign = &scanned.foreign;
        let (rpc_calls, rpc_serves, rpc_impls) =
            (rpc_of(file, &scanned.rpc.calls), rpc_of(file, &scanned.rpc.serves), rpc_of(file, &scanned.rpc.implements));
        if !as_json {
            for c in &foreign.calls {
                say!("  {:>5}  {}  → gọi qua ranh giới ngôn ngữ ({})", c.line, c.shown, c.symbol);
            }
            for c in &foreign.provides {
                say!("  {:>5}  cung cấp {} cho ngôn ngữ khác", c.line, c.symbol);
            }
            for c in &rpc_calls {
                say!("  {:>5}  {}  → gọi gRPC service {}{}", c.line, c.shown, c.service, origin_text(&c.origin));
            }
            for c in &rpc_serves {
                say!("  {:>5}  {}  → phục vụ gRPC service {}{}", c.line, c.shown, c.service, origin_text(&c.origin));
            }
            for c in &rpc_impls {
                say!("  {:>5}  {}  → cài đặt server gRPC {}{}", c.line, c.shown, c.service, origin_text(&c.origin));
            }
        }
        if as_json {
            let crossings = |list: &[scan::foreign::Crossing]| {
                list.iter()
                    .map(|c| {
                        format!(
                            r#"{{"line":{},"key":{},"shown":{}}}"#,
                            c.line,
                            json::string(&c.symbol),
                            json::string(&c.shown)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let sites = |list: &[scan::grpc::Site]| {
                list.iter()
                    .map(|c| {
                        let (kind, value) = match &c.origin {
                            scan::grpc::Origin::Unknown => ("unknown", ""),
                            scan::grpc::Origin::Loose => ("loose", ""),
                            scan::grpc::Origin::Package(v) => ("package", v.as_str()),
                            scan::grpc::Origin::Stem(v) => ("proto", v.as_str()),
                            scan::grpc::Origin::GoPackage(v) => ("goPackage", v.as_str()),
                            scan::grpc::Origin::GoName(v) => ("goName", v.as_str()),
                        };
                        format!(
                            r#"{{"line":{},"service":{},"origin":{},"from":{},"shown":{}}}"#,
                            c.line,
                            json::string(&c.service),
                            json::string(kind),
                            json::string(value),
                            json::string(&c.shown)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",")
            };
            json_files.push(format!(
                r#"{{"file":{},"lang":{},"imports":[{}],"crossings":{{"calls":[{}],"provides":[{}]}},"rpc":{{"calls":[{}],"serves":[{}],"implements":[{}]}}}}"#,
                json::string(file),
                json::string(lang.name()),
                items.join(","),
                crossings(&foreign.calls),
                crossings(&foreign.provides),
                sites(&rpc_calls),
                sites(&rpc_serves),
                sites(&rpc_impls)
            ));
        }
    }
    let deploy = scan_deploy(&root, &resolver, &deploy_files, as_json, verbatim);
    let proto_files: Vec<String> = if only.is_empty() {
        arch_core::check::proto_files(&root, &arch_core::read_ignores(&root))
    } else {
        files.iter().filter(|f| f.ends_with(".proto")).cloned().collect()
    };
    let protos = scan_protos(&root, &proto_files, as_json);
    if as_json {
        say!(
            r#"{{"root":{},"files":[{}],"deploy":{deploy},"protos":[{protos}]}}"#,
            json::string(&root.to_string_lossy()),
            json_files.join(",")
        );
    }
    ExitCode::from(exit::OK)
}

/// `(từ rpc.proto)`, `(từ go.etcd.io/…/etcdserverpb)`: thứ dùng để ghép với `.proto`.
fn origin_text(origin: &arch_core::scan::grpc::Origin) -> String {
    match origin {
        arch_core::scan::grpc::Origin::Unknown | arch_core::scan::grpc::Origin::Loose => String::new(),
        arch_core::scan::grpc::Origin::Package(p) => format!(" (package {p})"),
        arch_core::scan::grpc::Origin::Stem(s) => format!(" (từ {s}.proto)"),
        arch_core::scan::grpc::Origin::GoPackage(p) => format!(" (từ {p})"),
        arch_core::scan::grpc::Origin::GoName(p) => format!(" (từ package {p}?)"),
    }
}

/// Phần `.proto` của `scan`: package, `go_package`, service và rpc của từng file.
fn scan_protos(root: &Path, files: &[String], as_json: bool) -> String {
    let mut out: Vec<String> = Vec::new();
    for file in files {
        let text = match arch_core::scan::deploy::read_in(root, file, arch_core::check::MAX_FILE_BYTES) {
            Ok(t) => t,
            Err(why) => {
                if !as_json {
                    say!("{file} (proto): bỏ qua, {why}");
                }
                out.push(format!(r#"{{"file":{},"error":{}}}"#, json::string(file), json::string(&why)));
                continue;
            }
        };
        let p = arch_core::scan::grpc::read_proto(&text);
        if !as_json {
            say!("{file} (proto) package {}", if p.package.is_empty() { "(không có)" } else { p.package.as_str() });
            for s in &p.services {
                say!("  {:>5}  service {}  {} rpc", s.line, s.name, s.rpcs.len());
            }
            if p.truncated {
                say!("  ? quá trần service hay rpc; phần sau không đọc");
            }
        }
        let services: Vec<String> = p
            .services
            .iter()
            .map(|s| {
                let rpcs: Vec<String> = s.rpcs.iter().map(|(n, l)| format!("[{},{l}]", json::string(n))).collect();
                format!(r#"{{"name":{},"line":{},"rpcs":[{}]}}"#, json::string(&s.name), s.line, rpcs.join(","))
            })
            .collect();
        out.push(format!(
            r#"{{"file":{},"package":{},"goPackage":{},"services":[{}],"truncated":{}}}"#,
            json::string(file),
            json::string(&p.package),
            p.go_package.as_deref().map_or_else(|| "null".to_string(), json::string),
            services.join(","),
            p.truncated
        ));
    }
    out.join(",")
}

/// Phần triển khai của `scan`: dữ kiện từng file compose và Dockerfile như bộ
/// đọc thấy, rồi từng service đã ghép.
///
/// Mặc định không in thứ gì có thể mang secret (TM-10): không đối số của chỉ
/// thị Dockerfile (`ENV OPENAI_API_KEY=…`), không argv của lệnh (`--requirepass
/// …`), không phần `user:pass@` của đường dẫn, image hay URL (`ADD https://…`,
/// `include:`). `--verbatim` in nguyên những thứ đó, cho bộ đo đối chiếu với bộ
/// đọc khác. Giá trị biến môi trường của compose thì không bao giờ được in, kể
/// cả với `--verbatim`.
fn scan_deploy(
    root: &Path,
    resolver: &arch_core::resolve::Resolver,
    files: &[String],
    as_json: bool,
    verbatim: bool,
) -> String {
    use arch_core::scan::{compose, deploy, dockerfile};
    let opt = |v: Option<&str>| v.map_or_else(|| "null".to_string(), json::string);
    let command = |c: &Option<(dockerfile::Command, usize)>| match c {
        None => "null".to_string(),
        Some((cmd, line)) => {
            let (exec, argv) = match cmd {
                dockerfile::Command::Exec(a) => (true, a.clone()),
                dockerfile::Command::Shell(s) => (false, vec![s.clone()]),
            };
            let argv = if verbatim {
                format!(r#","argv":{}"#, json::strings(argv.iter().map(String::as_str)))
            } else {
                String::new()
            };
            format!(r#"{{"exec":{exec},"argc":{}{argv},"line":{line}}}"#, match cmd {
                dockerfile::Command::Exec(a) => a.len(),
                dockerfile::Command::Shell(_) => 1,
            })
        }
    };
    // Chuỗi lấy từ cấu hình (đường dẫn, image, URL của `ADD`, `include`): mặc
    // định qua `shown_path`, bỏ `user:pass@` và cắt bớt (TM-10).
    let shown = |s: &str| if verbatim { s.to_string() } else { deploy::shown_path(s) };
    let quoted = |s: &str| json::string(&shown(s));
    let maybe = |s: Option<&str>| s.map_or_else(|| "null".to_string(), quoted);
    let list = |v: &[String]| format!("[{}]", v.iter().map(|s| quoted(s)).collect::<Vec<_>>().join(","));
    let pairs = |v: &[(String, usize)]| {
        format!("[{}]", v.iter().map(|(n, l)| format!("[{},{l}]", quoted(n))).collect::<Vec<_>>().join(","))
    };

    let mut file_json: Vec<String> = Vec::new();
    for file in files {
        let name = file.rsplit('/').next().unwrap_or(file);
        let text = match deploy::read_in(root, file, deploy::MAX_DEPLOY_BYTES) {
            Ok(t) => t,
            Err(why) => {
                file_json.push(format!(r#"{{"file":{},"error":{}}}"#, json::string(file), json::string(&why)));
                continue;
            }
        };
        if deploy::is_compose(name) {
            match compose::parse(&text) {
                Err(e) => file_json.push(format!(
                    r#"{{"file":{},"kind":"compose","error":{}}}"#,
                    json::string(file),
                    json::string(&e.to_string())
                )),
                Ok(f) => {
                    let services: Vec<String> = f
                        .services
                        .iter()
                        .map(|s| {
                            let build = s.build.as_ref().map_or_else(
                                || "null".to_string(),
                                |b| {
                                    format!(
                                        r#"{{"context":{},"dockerfile":{},"inline":{},"target":{},"line":{}}}"#,
                                        quoted(&b.context),
                                        maybe(b.dockerfile.as_deref()),
                                        b.inline.is_some(),
                                        maybe(b.target.as_deref()),
                                        b.line
                                    )
                                },
                            );
                            // Chỉ khoá và dòng: tên máy rút từ giá trị có thể là
                            // một mảnh của secret khi nó không phải tên service.
                            let env: Vec<String> = s
                                .env
                                .iter()
                                .map(|e| format!(r#"{{"key":{},"line":{}}}"#, json::string(&e.key), e.line))
                                .collect();
                            let binds: Vec<String> = s
                                .binds
                                .iter()
                                .map(|b| format!("[{},{},{}]", quoted(&b.host), quoted(&b.container), b.line))
                                .collect();
                            format!(
                                r#"{{"name":{},"line":{},"image":{},"build":{build},"command":{},"entrypoint":{},"workingDir":{},"dependsOn":{},"links":{},"env":[{}],"envFiles":{},"binds":[{}],"extends":{},"aliases":{}}}"#,
                                quoted(&s.name),
                                s.line,
                                maybe(s.image.as_deref()),
                                command(&s.command),
                                command(&s.entrypoint),
                                maybe(s.working_dir.as_deref()),
                                pairs(&s.depends_on),
                                pairs(&s.links),
                                env.join(","),
                                pairs(&s.env_files),
                                binds.join(","),
                                s.extends.as_ref().map_or_else(|| "null".to_string(), |e| quoted(&e.service)),
                                list(&s.aliases),
                            )
                        })
                        .collect();
                    file_json.push(format!(
                        r#"{{"file":{},"kind":"compose","services":[{}],"includes":{}}}"#,
                        json::string(file),
                        services.join(","),
                        pairs(&f.includes)
                    ));
                }
            }
        } else {
            let d = dockerfile::parse(&text);
            let stages: Vec<String> = d
                .stages
                .iter()
                .map(|st| {
                    let copies: Vec<String> = st
                        .copies
                        .iter()
                        .map(|c| {
                            format!(
                                r#"{{"from":{},"sources":{},"dest":{},"line":{}}}"#,
                                maybe(c.from.as_deref()),
                                list(&c.sources),
                                quoted(&c.dest),
                                c.line
                            )
                        })
                        .collect();
                    format!(
                        r#"{{"from":{},"name":{},"line":{},"workdir":{},"copies":[{}],"cmd":{},"entrypoint":{}}}"#,
                        quoted(&st.from),
                        maybe(st.name.as_deref()),
                        st.line,
                        maybe(st.workdir.as_deref()),
                        copies.join(","),
                        command(&st.cmd),
                        command(&st.entrypoint),
                    )
                })
                .collect();
            let instructions: Vec<String> = dockerfile::instructions_of(&text)
                .iter()
                .map(|(line, keyword, args)| {
                    let args = if verbatim { format!(r#","args":{}"#, json::string(args)) } else { String::new() };
                    format!(r#"{{"line":{line},"keyword":{}{args}}}"#, json::string(keyword))
                })
                .collect();
            file_json.push(format!(
                r#"{{"file":{},"kind":"dockerfile","instructions":[{}],"stages":[{}]}}"#,
                json::string(file),
                instructions.join(","),
                stages.join(",")
            ));
        }
    }

    let loaded = deploy::load(root, files);
    let mut services: Vec<String> = Vec::new();
    for svc in &loaded.services {
        let key = match &svc.key {
            deploy::Key::Compose { dir, name } => format!("compose:{}{name}", if dir.is_empty() { String::new() } else { format!("{dir}/") }),
            deploy::Key::Dockerfile(p) => format!("dockerfile:{p}"),
        };
        let starts: Vec<(String, Vec<String>)> = svc
            .starts
            .iter()
            .map(|st| (format!("{}:{} {}", st.file, st.line, st.shown), arch_core::check::entry_files(root, resolver, &st.entries)))
            .collect();
        if !as_json {
            say!("triển khai {key} ({}:{})", svc.file, svc.line);
            for ((shown, entry), st) in starts.iter().zip(&svc.starts) {
                say!("  khởi chạy {shown} → {}", if entry.is_empty() { "không tìm thấy mã trong repo".to_string() } else { entry.join(", ") });
                if !st.unresolved.is_empty() && !entry.is_empty() {
                    say!("  ? không lần ra mã của `{}`", st.unresolved.join("`, `"));
                }
                if !st.cut.is_empty() {
                    say!("  ? không đọc hết: {}", st.cut.join("; "));
                }
            }
            for e in &svc.edges {
                say!("  mạng → {}  ({}:{} {})", e.to, e.file, e.line, e.shown);
            }
            if let Some(why) = &svc.unknown {
                say!("  ? {why}");
            }
        }
        let edges: Vec<String> = svc
            .edges
            .iter()
            .map(|e| {
                format!(
                    r#"{{"to":{},"file":{},"line":{},"shown":{}}}"#,
                    json::string(&e.to),
                    json::string(&e.file),
                    e.line,
                    json::string(&e.shown)
                )
            })
            .collect();
        let starts_json: Vec<String> = starts
            .iter()
            .zip(&svc.starts)
            .map(|((shown, entry), st)| {
                format!(
                    r#"{{"shown":{},"entries":{},"unresolved":{},"cut":{}}}"#,
                    json::string(shown),
                    json::strings(entry.iter().map(String::as_str)),
                    json::strings(st.unresolved.iter().map(String::as_str)),
                    json::strings(st.cut.iter().map(String::as_str))
                )
            })
            .collect();
        services.push(format!(
            r#"{{"key":{},"file":{},"line":{},"code":{},"edges":[{}],"starts":[{}],"unknown":{}}}"#,
            json::string(&key),
            json::string(&svc.file),
            svc.line,
            svc.code,
            edges.join(","),
            starts_json.join(","),
            opt(svc.unknown.as_deref()),
        ));
    }
    if !as_json {
        for u in &loaded.unread {
            say!("chưa đọc được {} — {}", u.file, u.why);
        }
    }
    let unread: Vec<String> = loaded
        .unread
        .iter()
        .map(|u| format!(r#"{{"file":{},"why":{}}}"#, json::string(&u.file), json::string(&u.why)))
        .collect();
    format!(r#"{{"files":[{}],"services":[{}],"unread":[{}]}}"#, file_json.join(","), services.join(","), unread.join(","))
}

/// Đưa đường dẫn người dùng gõ về dạng tương đối so với gốc dự án, và bỏ
/// những file không thuộc ngôn ngữ nào đã hỗ trợ.
fn relative_to(root: &Path, paths: &[String]) -> Vec<String> {
    let base = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    paths
        .iter()
        .filter_map(|p| {
            // Người gõ tay thường đứng ở gốc dự án; hook thì đưa đường dẫn
            // tuyệt đối. Thử theo thư mục hiện tại trước, rồi theo gốc.
            let full = Path::new(p)
                .canonicalize()
                .or_else(|_| base.join(p).canonicalize())
                .ok()?;
            let rel = full.strip_prefix(&base).ok()?.to_string_lossy().replace('\\', "/");
            // Giữ mọi file, không chỉ file mã: `env_file`, script không đuôi
            // là thứ cấu hình triển khai trỏ tới, và `check` tự chọn có đọc lại
            // cấu hình hay không (`may_touch_deploy`).
            full.is_file().then_some(rel)
        })
        .collect()
}

/// File mã nguồn đã đổi so với HEAD, cộng file chưa theo dõi.
///
/// `None` nghĩa là không dùng được chế độ này — không phải repo git, hoặc
/// chính file luật vừa đổi nên phải quét lại toàn bộ.
/// Gọi `git` mà không để cấu hình của repo chọn hộ mình một lệnh để chạy.
///
/// `core.fsmonitor` là một lệnh shell, và git chạy nó trong `git status`. Một
/// dòng trong `.git/config` là đủ để `archctl check --changed` — chính lệnh
/// mà `init` cài làm hook của agent — thực thi thứ người viết dòng đó chọn.
/// `core.pager` và alias cũng vậy với các lệnh khác.
///
/// Repo là thứ agent ghi được, nên cấu hình trong đó không đáng tin hơn mã
/// nguồn trong đó.
fn git(root: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("-C").arg(root);
    for (key, value) in [
        ("core.fsmonitor", ""),
        ("core.hooksPath", "/dev/null"),
        ("core.pager", "cat"),
        ("core.editor", "true"),
        ("core.sshCommand", "false"),
        ("protocol.ext.allow", "never"),
    ] {
        cmd.arg("-c").arg(format!("{key}={value}"));
    }
    cmd.arg("--no-optional-locks");
    cmd
}

/// File đã đổi theo git. `None` nghĩa là "không rút gọn được, kiểm hết" — và
/// mọi chỗ không chắc đều trả về `None`, vì kiểm thừa chỉ tốn thời gian còn
/// kiểm thiếu thì để lọt vi phạm.
fn changed_files(root: &Path) -> Option<Vec<String>> {
    // `-z` thay vì đầu ra mặc định: mặc định git bọc nháy và đổi tên có dấu
    // thành chuỗi thoát bát phân (`"src/trang-ch\341\273\247.ts"`). Đường dẫn
    // đó không có thật trên đĩa nên file bị bỏ qua **trong im lặng** — tức là
    // mọi file tên không thuần ASCII đều không được hook kiểm.
    let out = git(root)
        .args(["status", "--porcelain", "-z", "--untracked-files=all", "."])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // Tên file không phải UTF-8 thì đã bị thay ký tự, và đường dẫn sai còn tệ
    // hơn không có đường dẫn nào.
    let text = std::str::from_utf8(&out.stdout).ok()?;

    let mut files = Vec::new();
    let mut records = text.split('\0');
    while let Some(record) = records.next() {
        // `XY <đường dẫn>`, X là trạng thái trong index, Y là trong thư mục.
        if record.len() < 4 {
            continue;
        }
        let (code, path) = record.split_at(3);
        let code = code.as_bytes();
        // Đổi tên và sao chép kèm một bản ghi nữa là đường dẫn cũ.
        if code[0] == b'R' || code[0] == b'C' {
            records.next();
        }
        // Luật đổi thì mọi file đều phải xét lại, không chỉ file vừa sửa.
        if path.ends_with(".arch/system.arch") {
            return None;
        }
        // Mọi file, không chỉ file mã: xem `relative_to`.
        if root.join(path).is_file() {
            files.push(path.to_string());
        } else if code[0] != b'D' && code[1] != b'D' {
            // Không phải xoá mà vẫn không thấy file: không hiểu đầu ra của
            // git nữa, nên quay về kiểm cả repo.
            return None;
        }
    }
    Some(files)
}

/// Lấy nội dung file luật ở một ref của git. Dùng `git` có sẵn thay vì thư
/// viện, để giữ cây dependency bằng không.
fn git_show(root: &Path, git_ref: &str) -> Option<String> {
    let out = git(root)
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

    crate::say!("{path} · kiến trúc {}\n", arch_core::arch_hash(&doc));
    crate::say!(
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
    crate::say!("{} luật có hiệu lực · {} trong đó đã khoá\n", doc.enforced_count(), locked);

    for d in &diags {
        let mark = if d.severity == Severity::Error { "✗" } else { "⚠" };
        crate::say!("{mark} {d}");
    }

    let errors = diag::count(&diags, Severity::Error);
    let warnings = diag::count(&diags, Severity::Warning);
    if errors == 0 && warnings == 0 {
        crate::say!("không có lỗi, không có cảnh báo");
    } else {
        crate::say!("\n{errors} lỗi, {warnings} cảnh báo");
    }

    ExitCode::from(if errors > 0 { exit::DSL_INVALID } else { exit::OK })
}
