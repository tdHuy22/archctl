//! `archctl init` và `archctl baseline` — lối vào cho dự án đã có mã.

use crate::exit;
use crate::proto::parse_args;
use arch_core::baseline::{self, BASELINE_FILE};
use arch_core::protocol::{DECISIONS_FILE, DECISIONS_HEADER, PROPOSALS_FILE};
use arch_core::{ARCH_FILE, discover};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const ARCHITECTURE_MD: &str = include_str!("../../../templates/ARCHITECTURE.md");

/// Hook chạy sau mỗi lần agent sửa file. Cho thẳng đường dẫn vào `check` nên
/// chỉ mất khoảng 10ms — đủ nhanh để agent không có cớ bỏ qua.
const CLAUDE_SETTINGS: &str = r#"{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write",
        "hooks": [
          { "type": "command", "command": "archctl check --changed" }
        ]
      }
    ]
  }
}
"#;

const CI_SNIPPET: &str = r#"
Chạy trong CI để vi phạm mới chặn được pull request:

  - run: archctl check --strict

Và khoá file luật lại bằng CODEOWNERS, để agent không tự duyệt được:

  .arch/**  @ten-cua-ban
"#;
const POINTER: &str = "Kiến trúc dự án: đọc `.arch/ARCHITECTURE.md` trước khi viết mã.";

const EMPTY_LAW: &str = "\
# Luật kiến trúc. Chỉ người sửa file này.
# Agent KHÔNG được ghi vào đây — nó dùng `archctl propose`.
#
# component <Tên> at <glob>   @human|@ai  #draft|#proposed|#accepted|#locked
# store     <Tên> owned_by <Component> at \"bảng\", ...
# api       <Tên> at \"strcpy\", \"gets\"
# allow     <Nguồn> -> <Đích>
# deny      <Nguồn> -> <Đích> [except <Component>]
#
# Component nào có `allow` đang có hiệu lực thì component đó bắt đầu bị kiểm.
# Chưa khai báo thì tạm để yên — áp dần được, không cần mô tả cả repo trước.
";

pub fn cmd_init(args: &[String]) -> ExitCode {
    let parsed = match parse_args(args, &["root"], &["from-code", "force", "no-hooks"]) {
        Ok(a) => a,
        Err(e) => return fail(&e, exit::USAGE),
    };
    if let Some(bad) = parsed.positional.first() {
        return fail(&format!("tham số không nhận ra: `{bad}`"), exit::USAGE);
    }
    let from_code = parsed.has("from-code");
    let force = parsed.has("force");

    let root = parsed.flags.get("root").map_or_else(|| PathBuf::from("."), PathBuf::from);
    if !root.is_dir() {
        return fail(&format!("`{}` không phải thư mục", root.display()), exit::USAGE);
    }
    if root.join(ARCH_FILE).exists() && !force {
        return fail(
            &format!("`{ARCH_FILE}` đã tồn tại. Dùng `--force` nếu thật sự muốn ghi đè."),
            exit::USAGE,
        );
    }
    if let Err(e) = std::fs::create_dir_all(root.join(".arch")) {
        return fail(&format!("không tạo được `.arch/`: {e}"), exit::USAGE);
    }

    let law = if from_code {
        let ignores = arch_core::read_ignores(&root);
        let found = discover::discover(&root, &ignores);
        if found.files == 0 {
            return fail(
                "không tìm thấy file mã nguồn nào thuộc ngôn ngữ đã hỗ trợ.\n\
                 → chạy trong thư mục dự án, hoặc dùng `--root <dir>`",
                exit::USAGE,
            );
        }
        report_discovery(&found);
        discover::render(&found, &arch_core::protocol::today())
    } else {
        EMPTY_LAW.to_string()
    };

    let files: [(&str, &str); 4] = [
        (ARCH_FILE, &law),
        (PROPOSALS_FILE, "# Đề xuất của agent, chờ người duyệt.\n"),
        (DECISIONS_FILE, DECISIONS_HEADER),
        (".arch/ARCHITECTURE.md", ARCHITECTURE_MD),
    ];
    for (rel, body) in files {
        // Không đè `decisions.md` đang có: nhật ký quyết định là thứ không
        // được mất, kể cả khi init lại.
        if rel == DECISIONS_FILE && root.join(rel).exists() {
            continue;
        }
        if let Err(e) = arch_core::write_atomic(&root.join(rel), body) {
            return fail(&format!("không ghi được `{rel}`: {e}"), exit::USAGE);
        }
        crate::say!("  tạo  {rel}");
    }

    if !parsed.has("no-hooks") {
        match write_hook(&root) {
            Ok(true) => crate::say!("  tạo  .claude/settings.json  (hook tự kiểm sau mỗi lần sửa)"),
            Ok(false) => {}
            Err(e) => eprintln!("  ⚠ .claude/settings.json: {e}"),
        }
    }

    for pointer in ["AGENTS.md", "CLAUDE.md"] {
        match append_pointer(&root, pointer) {
            Ok(true) => crate::say!("  sửa  {pointer}"),
            Ok(false) => {}
            Err(e) => eprintln!("  ⚠ {pointer}: {e}"),
        }
    }
    if let Ok(true) = append_gitignore(&root) {
        crate::say!("  sửa  .gitignore");
    }

    crate::say!("\nTiếp theo:");
    if from_code {
        crate::say!("  1. Đọc `{ARCH_FILE}` — nó đang mô tả hiện trạng, chưa phải thiết kế");
        crate::say!("  2. Sửa cho đúng ý, rồi đổi `#draft` thành `#accepted` ở phần đã chắc");
        crate::say!("  3. archctl check");
        crate::say!("  4. archctl baseline  — nếu muốn tha vi phạm cũ và chỉ chặn cái mới");
    } else {
        crate::say!("  1. Khai báo component trong `{ARCH_FILE}`");
        crate::say!("  2. archctl check");
    }
    crate::say!("  ·  archctl serve  — xem và sửa trên sơ đồ");
    crate::say!("{CI_SNIPPET}");
    ExitCode::from(exit::OK)
}

/// Chỉ tạo khi chưa có file cấu hình. Gộp vào cấu hình sẵn có là việc rủi ro —
/// đó là file của người dùng, hỏng nó thì hỏng cả phiên làm việc của họ.
fn write_hook(root: &Path) -> Result<bool, String> {
    let path = root.join(".claude/settings.json");
    if path.exists() {
        return Ok(false);
    }
    std::fs::create_dir_all(root.join(".claude")).map_err(|e| e.to_string())?;
    arch_core::write_atomic(&path, CLAUDE_SETTINGS).map_err(|e| e.to_string())?;
    Ok(true)
}

fn report_discovery(found: &discover::Discovery) {
    let langs: Vec<String> =
        found.by_lang.iter().map(|(l, n)| format!("{n} {}", l.name())).collect();
    let root = if found.source_root.is_empty() { "gốc repo" } else { &found.source_root };
    crate::say!(
        "Đã đọc {} file ({}) · gốc mã nguồn: {root}",
        found.files,
        langs.join(", ")
    );
    crate::say!(
        "Dựng được {} component và {} quan hệ, tất cả ở `#draft`.",
        found.components.len(),
        found.edges.len()
    );
    // Số component không nói bản nháp phủ được bao nhiêu repo. Một lần chạy
    // từng ra "7 component" trong khi toàn bộ mã của dự án nằm ngoài sơ đồ.
    if let Some(share) = (found.unmapped * 100).checked_div(found.files)
        && share >= 25
    {
        crate::say!(
            "⚠ {} file ({share}%) chưa thuộc component nào — bản nháp mới phủ một phần repo.",
            found.unmapped
        );
    }
    if !found.skipped.is_empty() {
        crate::say!("Còn {} thư mục nữa, liệt kê trong phần chú thích cuối file.", found.skipped.len());
    }
    crate::say!();
}

/// Thêm một dòng trỏ về `.arch/ARCHITECTURE.md`. Trả `false` nếu đã có rồi.
fn append_pointer(root: &Path, name: &str) -> Result<bool, String> {
    let path = root.join(name);
    let mut body = std::fs::read_to_string(&path).unwrap_or_default();
    if body.contains(".arch/ARCHITECTURE.md") {
        return Ok(false);
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    if body.is_empty() {
        body.push_str("# Hướng dẫn\n\n");
    } else {
        body.push('\n');
    }
    body.push_str(POINTER);
    body.push('\n');
    arch_core::write_atomic(&path, &body).map_err(|e| e.to_string())?;
    Ok(true)
}

fn append_gitignore(root: &Path) -> Result<bool, String> {
    let path = root.join(".gitignore");
    let mut body = std::fs::read_to_string(&path).unwrap_or_default();
    if body.contains("/.arch/.state") {
        return Ok(false);
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    // `**/` chứ không neo vào gốc: monorepo có thể có nhiều `.arch/` lồng
    // trong các dự án con, và mẫu neo gốc bỏ sót hết chúng.
    body.push_str("**/.arch/.state\n");
    arch_core::write_atomic(&path, &body).map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn cmd_export(args: &[String]) -> ExitCode {
    let parsed = match parse_args(args, &["root", "view", "out"], &["violations"]) {
        Ok(a) => a,
        Err(e) => return fail(&e, exit::USAGE),
    };
    let start = parsed.flags.get("root").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let Some(root) = arch_core::find_root(&start) else {
        return fail(&format!("không tìm thấy `{ARCH_FILE}`"), exit::USAGE);
    };

    let src = std::fs::read_to_string(root.join(ARCH_FILE)).unwrap_or_default();
    let (doc, diags) = arch_core::load(&src);
    if let Some(d) = diags.iter().find(|d| d.severity == arch_core::Severity::Error) {
        return fail(&format!("`{ARCH_FILE}` có lỗi: {}", d.msg), exit::DSL_INVALID);
    }
    if let Some(name) = parsed.flags.get("view")
        && !doc.views.iter().any(|v| &v.name == name)
    {
        return fail(&format!("không có view nào tên `{name}`"), exit::USAGE);
    }

    let report = parsed.has("violations").then(|| {
        let ignores = arch_core::read_ignores(&root);
        arch_core::check::check(&root, &doc, &ignores)
    });
    let options = arch_core::export::Options {
        view: parsed.flags.get("view").map(String::as_str),
        violations: report.as_ref().map_or(&[][..], |r| &r.violations),
    };

    let hash = arch_core::arch_hash(&doc);
    let today = arch_core::protocol::today();
    match parsed.flags.get("out") {
        Some(out) => {
            // Đuôi `.md` thì bọc trong khối mã để GitHub render được.
            let body = if out.ends_with(".md") {
                arch_core::export::markdown(&doc, &options, &hash, &today)
            } else {
                arch_core::export::mermaid(&doc, &options)
            };
            let path = root.join(out);
            if let Err(e) = arch_core::write_atomic(&path, &body) {
                return fail(&format!("không ghi được `{out}`: {e}"), exit::USAGE);
            }
            crate::say!("Đã ghi {out}");
            if let Some(r) = &report {
                crate::say!("{} vi phạm được phủ lên sơ đồ.", r.violations.len());
            }
        }
        None => crate::say_inline!("{}", arch_core::export::mermaid(&doc, &options)),
    }
    ExitCode::from(exit::OK)
}

pub fn cmd_baseline(args: &[String]) -> ExitCode {
    let parsed = match parse_args(args, &["root"], &[]) {
        Ok(a) => a,
        Err(e) => return fail(&e, exit::USAGE),
    };
    let start = parsed.flags.get("root").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let Some(root) = arch_core::find_root(&start) else {
        return fail(&format!("không tìm thấy `{ARCH_FILE}`"), exit::USAGE);
    };

    let src = std::fs::read_to_string(root.join(ARCH_FILE)).unwrap_or_default();
    let (doc, diags) = arch_core::load(&src);
    if let Some(d) = diags.iter().find(|d| d.severity == arch_core::Severity::Error) {
        return fail(&format!("`{ARCH_FILE}` có lỗi: {}", d.msg), exit::DSL_INVALID);
    }

    let ignores = arch_core::read_ignores(&root);
    let report = arch_core::check::check(&root, &doc, &ignores);
    let now: BTreeSet<String> = report.violations.iter().map(baseline::key).collect();
    let existing = std::fs::read_to_string(root.join(BASELINE_FILE)).ok();
    let first_run = existing.is_none();
    let before = baseline::parse(existing.as_deref().unwrap_or_default());

    let added = now.difference(&before).count();
    let removed = before.difference(&now).count();
    let body = baseline::render(&now, &arch_core::protocol::today());
    if let Err(e) = arch_core::write_atomic(&root.join(BASELINE_FILE), &body) {
        return fail(&format!("không ghi được `{BASELINE_FILE}`: {e}"), exit::USAGE);
    }

    crate::say!("Đã ghi {BASELINE_FILE}: {} vi phạm được tha.", now.len());
    if removed > 0 {
        crate::say!("  {removed} mục cũ đã sửa xong và bị bỏ khỏi danh sách — không quay lại được.");
    }
    // Lần đầu thì mọi thứ đều "mới", nói ra chỉ gây hiểu nhầm.
    if added > 0 && !first_run {
        crate::say!("  ! {added} vi phạm mới vừa được tha thêm. Chỉ nên làm vậy khi vừa mở rộng luật.");
    }
    if now.is_empty() {
        crate::say!("  Không còn vi phạm nào. Xoá file này đi là xong.");
    }
    ExitCode::from(exit::OK)
}

fn fail(message: &str, code: u8) -> ExitCode {
    eprintln!("archctl: {message}");
    ExitCode::from(code)
}
