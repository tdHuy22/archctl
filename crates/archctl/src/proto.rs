//! Lệnh thuộc giao thức agent–người: `propose`, `block`, `accept`, `reject`,
//! `proposals`.
//!
//! Ranh giới quan trọng: `propose` và `block` là lệnh của **agent**; `accept`
//! và `reject` là lệnh của **người**. Việc tách đó nằm ở `.arch/ARCHITECTURE.md`
//! và ở quy trình review, không phải ở đây — xem phần giới hạn trong
//! `docs/protocol.md`.

use arch_core::protocol::{
    self as proto, DECISIONS_FILE, DECISIONS_HEADER, Decision, Intent, PROPOSALS_FILE, Proposal,
    Verdict,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::exit;

/// Phân tích tham số: `--cờ giá trị`, `--cờ` không giá trị, phần còn lại là
/// tham số vị trí.
pub struct Args {
    pub positional: Vec<String>,
    pub flags: BTreeMap<String, String>,
    pub bools: BTreeSet<String>,
}

impl Args {
    pub fn has(&self, name: &str) -> bool {
        self.bools.contains(name)
    }
}

pub fn parse_args(
    args: &[String],
    value_flags: &[&str],
    bool_flags: &[&str],
) -> Result<Args, String> {
    let mut out =
        Args { positional: Vec::new(), flags: BTreeMap::new(), bools: BTreeSet::new() };
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some(name) = arg.strip_prefix("--") {
            if bool_flags.contains(&name) {
                out.bools.insert(name.to_string());
                i += 1;
                continue;
            }
            if !value_flags.contains(&name) {
                return Err(format!("tham số không nhận ra: `--{name}`"));
            }
            let Some(value) = args.get(i + 1) else {
                return Err(format!("`--{name}` thiếu giá trị"));
            };
            out.flags.insert(name.to_string(), value.clone());
            i += 2;
            continue;
        }
        out.positional.push(arg.clone());
        i += 1;
    }
    Ok(out)
}

fn root_from(args: &Args) -> Result<PathBuf, String> {
    let start = args.flags.get("root").map_or_else(|| PathBuf::from("."), PathBuf::from);
    arch_core::find_root(&start).ok_or_else(|| {
        format!("không tìm thấy `{}` ở `{}` hay thư mục cha nào", arch_core::ARCH_FILE, start.display())
    })
}

fn read(root: &Path, rel: &str) -> String {
    std::fs::read_to_string(root.join(rel)).unwrap_or_default()
}

fn write(root: &Path, rel: &str, body: &str) -> Result<(), String> {
    std::fs::write(root.join(rel), body).map_err(|e| format!("không ghi được `{rel}`: {e}"))
}

/// Ai đang quyết định. Lấy từ git để nhật ký ghi được tên thật, nhờ đó một
/// quyết định do agent tự bấm vẫn hiện ra trong review.
fn whoami(root: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["config", "user.name"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "không rõ".into())
}

/// Câu lệnh phải đọc được thành đúng một phần tử, **và** phải hợp lệ khi đặt
/// cạnh luật hiện có — `allow Capture -> Report` chỉ có nghĩa nếu hai
/// component đó đã được khai báo. Kiểm tách rời sẽ báo sai mọi tham chiếu.
fn validate_statement(root: &Path, statement: &str) -> Result<(), String> {
    let first_error = |diags: &[arch_core::Diag]| {
        diags
            .iter()
            .find(|d| d.severity == arch_core::Severity::Error)
            .map(|d| d.msg.clone())
    };

    let (doc, diags) = arch_core::parser::parse(statement);
    if let Some(msg) = first_error(&diags) {
        return Err(format!("câu lệnh sai cú pháp: {msg}"));
    }
    match doc.elements().len() {
        1 => {}
        0 => return Err("câu lệnh rỗng".into()),
        n => return Err(format!("mỗi lần chỉ đề xuất một câu lệnh, nhận được {n}")),
    }

    let law = read(root, arch_core::ARCH_FILE);
    if let Some(msg) = first_error(&arch_core::load(&law).1) {
        return Err(format!("`{}` đang có lỗi, sửa trước đã: {msg}", arch_core::ARCH_FILE));
    }
    match first_error(&arch_core::load(&format!("{law}\n{statement}")).1) {
        Some(msg) => Err(format!("đặt cạnh luật hiện có thì không hợp lệ: {msg}")),
        None => Ok(()),
    }
}

fn load_proposals(root: &Path) -> Result<Vec<Proposal>, String> {
    let (proposals, diags) = proto::parse_proposals(&read(root, PROPOSALS_FILE));
    match diags.first() {
        Some(d) => Err(format!("`{PROPOSALS_FILE}` hỏng: {}", d.msg)),
        None => Ok(proposals),
    }
}

pub fn cmd_propose(args: &[String], intent: Intent) -> ExitCode {
    let parsed = match parse_args(args, &["reason", "from", "root", "on"], &[]) {
        Ok(a) => a,
        Err(e) => return fail(&e, exit::USAGE),
    };

    // `propose '<luật>'` nhận tham số vị trí; `block --on '<luật>'` nhận cờ.
    let statement = match intent {
        Intent::Rule => parsed.positional.first().cloned(),
        Intent::Block => parsed.flags.get("on").cloned(),
    };
    let Some(statement) = statement else {
        return fail(
            match intent {
                Intent::Rule => "archctl propose: thiếu câu lệnh\n→ archctl propose 'allow A -> B' --reason \"...\"",
                Intent::Block => "archctl block: thiếu `--on`\n→ archctl block --on 'deny * -> X' --reason \"...\"",
            },
            exit::USAGE,
        );
    };
    let Some(reason) = parsed.flags.get("reason") else {
        return fail("thiếu `--reason`. Lý do là thứ người duyệt đọc để quyết định.", exit::USAGE);
    };
    let root = match root_from(&parsed) {
        Ok(r) => r,
        Err(e) => return fail(&e, exit::USAGE),
    };
    if let Err(e) = validate_statement(&root, &statement) {
        return fail(&e, exit::USAGE);
    }
    let id = proto::proposal_id(intent, &statement);

    // Đã bị từ chối trước đây thì không đề xuất lại. Đây là lý do
    // `decisions.md` tồn tại: agent không có trí nhớ giữa các phiên.
    let decisions = proto::parse_decisions(&read(&root, DECISIONS_FILE));
    if let Some(d) = decisions.iter().rev().find(|d| d.id == id && d.verdict == Verdict::Rejected) {
        crate::say!("Đề xuất này đã bị từ chối ngày {} bởi {}.", d.date, d.by);
        crate::say!("Lý do: {}", d.reason);
        crate::say!("\n→ Đừng đề xuất lại. Tìm hướng khác, hoặc hỏi người dùng.");
        return ExitCode::from(exit::PENDING);
    }

    let mut proposals = match load_proposals(&root) {
        Ok(p) => p,
        Err(e) => return fail(&e, exit::DSL_INVALID),
    };
    if proposals.iter().any(|p| p.id == id) {
        crate::say!("Đề xuất `{id}` đã có sẵn, đang chờ người duyệt. Không ghi thêm.");
        return ExitCode::from(exit::PENDING);
    }

    proposals.push(Proposal {
        id: id.clone(),
        intent,
        statement: statement.clone(),
        reason: reason.clone(),
        origin: parsed.flags.get("from").cloned(),
        owner: arch_core::Owner::Ai,
        date: proto::today(),
    });
    if let Err(e) = write(&root, PROPOSALS_FILE, &proto::render_proposals(&proposals)) {
        return fail(&e, exit::USAGE);
    }

    match intent {
        Intent::Rule => crate::say!("Đã ghi đề xuất `{id}`: {statement}"),
        Intent::Block => crate::say!("Đã báo bế tắc `{id}` tại: {statement}"),
    }
    crate::say!("→ DỪNG phần việc này lại và làm việc khác. Chờ người quyết định.");
    ExitCode::from(exit::PENDING)
}

pub fn cmd_decide(args: &[String], verdict: Verdict) -> ExitCode {
    let parsed = match parse_args(args, &["reason", "root"], &[]) {
        Ok(a) => a,
        Err(e) => return fail(&e, exit::USAGE),
    };
    let Some(id) = parsed.positional.first().cloned() else {
        return fail("thiếu id của đề xuất. Xem `archctl proposals`.", exit::USAGE);
    };
    // Bắt ghi lý do cả khi duyệt, không chỉ khi từ chối. Ma sát phải tương
    // xứng với mức độ: duyệt một thay đổi luật là hành vi quản trị.
    let Some(reason) = parsed.flags.get("reason") else {
        return fail("thiếu `--reason`. Quyết định nào cũng phải ghi lý do.", exit::USAGE);
    };

    let root = match root_from(&parsed) {
        Ok(r) => r,
        Err(e) => return fail(&e, exit::USAGE),
    };
    let mut proposals = match load_proposals(&root) {
        Ok(p) => p,
        Err(e) => return fail(&e, exit::DSL_INVALID),
    };
    let Some(pos) = proposals.iter().position(|p| p.id == id) else {
        return fail(&format!("không có đề xuất nào tên `{id}`"), exit::USAGE);
    };
    let proposal = proposals.remove(pos);

    if verdict == Verdict::Accepted && proposal.intent == Intent::Rule {
        let mut law = read(&root, arch_core::ARCH_FILE);
        if !law.ends_with('\n') {
            law.push('\n');
        }
        law.push_str(&format!("{} @ai #accepted\n", proposal.statement));
        let (_, diags) = arch_core::load(&law);
        if let Some(d) = diags.iter().find(|d| d.severity == arch_core::Severity::Error) {
            return fail(
                &format!("thêm luật này làm file luật hỏng: {}\nKhông ghi gì cả.", d.msg),
                exit::DSL_INVALID,
            );
        }
        if let Err(e) = write(&root, arch_core::ARCH_FILE, &law) {
            return fail(&e, exit::USAGE);
        }
    }

    let decision = Decision {
        date: proto::today(),
        verdict,
        id: id.clone(),
        statement: proposal.statement.clone(),
        by: whoami(&root),
        reason: reason.clone(),
    };
    let mut log = read(&root, DECISIONS_FILE);
    if log.is_empty() {
        log.push_str(DECISIONS_HEADER);
    }
    log.push_str(&proto::render_decision(&decision));
    if let Err(e) = write(&root, DECISIONS_FILE, &log) {
        return fail(&e, exit::USAGE);
    }
    if let Err(e) = write(&root, PROPOSALS_FILE, &proto::render_proposals(&proposals)) {
        return fail(&e, exit::USAGE);
    }

    crate::say!("{} `{id}`: {}", verdict.label(), proposal.statement);
    match (verdict, proposal.intent) {
        (Verdict::Accepted, Intent::Rule) => {
            crate::say!("Đã thêm vào `{}` với `@ai #accepted`.", arch_core::ARCH_FILE)
        }
        (Verdict::Accepted, Intent::Block) => crate::say!(
            "Đã ghi nhận. Luật chặn vẫn còn — sửa `{}` nếu muốn gỡ.",
            arch_core::ARCH_FILE
        ),
        (Verdict::Rejected, _) => {
            crate::say!("Đã ghi vào `{DECISIONS_FILE}`. Agent sẽ không đề xuất lại.")
        }
    }
    ExitCode::from(exit::OK)
}

pub fn cmd_proposals(args: &[String]) -> ExitCode {
    let parsed = match parse_args(args, &["root"], &[]) {
        Ok(a) => a,
        Err(e) => return fail(&e, exit::USAGE),
    };
    let root = match root_from(&parsed) {
        Ok(r) => r,
        Err(e) => return fail(&e, exit::USAGE),
    };
    let proposals = match load_proposals(&root) {
        Ok(p) => p,
        Err(e) => return fail(&e, exit::DSL_INVALID),
    };

    if proposals.is_empty() {
        crate::say!("Không có đề xuất nào đang chờ.");
        return ExitCode::from(exit::OK);
    }
    for p in &proposals {
        let what = match p.intent {
            Intent::Rule => "ĐỀ XUẤT",
            Intent::Block => "BẾ TẮC ",
        };
        crate::say!("{what} {}  {}", p.id, p.statement);
        crate::say!("  lý do: {}", p.reason);
        if let Some(origin) = &p.origin {
            crate::say!("  từ:    {origin}");
        }
        crate::say!("  ngày:  {}\n", p.date);
    }
    crate::say!(
        "{} đang chờ. Duyệt: archctl accept <id> --reason \"...\"  ·  Từ chối: archctl reject <id> --reason \"...\"",
        proposals.len()
    );
    ExitCode::from(exit::PENDING)
}

fn fail(message: &str, code: u8) -> ExitCode {
    eprintln!("archctl: {message}");
    ExitCode::from(code)
}
