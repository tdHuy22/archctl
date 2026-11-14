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

/// Hook chạy sau mỗi lần agent sửa file hay chạy lệnh shell, cộng ranh giới
/// ghi cho `.arch/`.
///
/// `check --hook` đọc payload của Claude Code, kiểm đúng file vừa sửa (Bash
/// thì file đổi so với HEAD), mất khoảng 10ms — đủ nhanh để agent không có cớ
/// bỏ qua. Nó luôn thoát 0 và đưa vi phạm qua `additionalContext`: hook cũ
/// (`check --changed`, thoát 1) chỉ hiện trong transcript, model không thấy.
/// Bash có trong matcher vì agent sửa file bằng `sed`, `cat >`, `git apply`
/// được.
///
/// Phần `permissions.deny` mới là chỗ biến "agent không được ghi file luật"
/// từ một câu trong tài liệu thành một ràng buộc. Không có nó, cả mô hình
/// quyền chỉ là lời đề nghị: agent ghi thẳng `system.arch` là tự cho mình
/// quyền, và ghi `.verified` là tự cấp cho mình một biên nhận. `proposals.arch`
/// được chừa ra vì đó đúng là chỗ agent phải ghi. `.archignore` và baseline
/// cũng bị chặn: ghi được chúng là tự miễn hay tự tha vi phạm. `accept`,
/// `reject`, `baseline`, `init`, `serve` là lệnh của người (claude-security F13,
/// F16): `serve` in khoá phiên ra stdout, và `/api/decide` gọi `accept` thay cho
/// người. Luật Bash theo tiền tố vẫn lách được (`env archctl …`), nên CI và
/// CODEOWNERS mới là chốt thật.
const CLAUDE_SETTINGS: &str = r#"{
  "permissions": {
    "deny": [
      "Write(./.arch/system.arch)",
      "Edit(./.arch/system.arch)",
      "Write(./.arch/.verified)",
      "Edit(./.arch/.verified)",
      "Write(./.arch/baseline)",
      "Edit(./.arch/baseline)",
      "Write(./.arch/.archignore)",
      "Edit(./.arch/.archignore)",
      "Write(./.arch/decisions.md)",
      "Edit(./.arch/decisions.md)",
      "Write(./.arch/.state)",
      "Edit(./.arch/.state)",
      "Write(./.claude/settings.json)",
      "Edit(./.claude/settings.json)",
      "Write(./.git/config)",
      "Edit(./.git/config)",
      "Bash(archctl accept:*)",
      "Bash(archctl reject:*)",
      "Bash(archctl baseline:*)",
      "Bash(archctl init:*)",
      "Bash(archctl serve:*)"
    ]
  },
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit|Bash",
        "hooks": [
          { "type": "command", "command": "archctl check --hook", "timeout": 30 }
        ]
      }
    ]
  }
}
"#;

const CI_SNIPPET: &str = r#"
Chạy trong CI để vi phạm mới chặn được pull request:

  - run: archctl verify
  - run: archctl check --strict

Thứ tự đó quan trọng: `.arch/.verified` không vào git (một biên nhận commit
được là một biên nhận bịa được), nên bản checkout nào cũng bắt đầu không có
biên nhận nào — và `--strict` coi "chưa có biên nhận" là vi phạm.

Và khoá file luật lại bằng CODEOWNERS, để agent không tự duyệt được:

  .arch/**  @ten-cua-ban

CI quan trọng vì nó chạy ngoài tầm với của agent. Mọi ranh giới ở máy cục bộ
đều dựa vào việc agent không ghi được `.arch/` — `.claude/settings.json` đã
cấm, nhưng cấm bằng cấu hình thì chỉ chắc bằng chính cấu hình đó.
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
        if found.files == 0 && found.services.is_empty() {
            return fail(
                "không tìm thấy file mã nguồn nào thuộc ngôn ngữ đã hỗ trợ, cũng không có cấu hình triển khai nào.\n\
                 → chạy trong thư mục dự án, hoặc dùng `--root <dir>`",
                exit::USAGE,
            );
        }
        report_discovery(&found);
        let draft = discover::render(&found, &arch_core::protocol::today());
        // Lưới cuối (TM-76): bản nháp chỉ được có phần tử `@ai #draft`, không
        // `verify` nào. Khác đi nghĩa là một tên trong repo đã chèn được câu.
        let (doc, _) = arch_core::load(&draft);
        let elements = doc.elements();
        if !doc.verifies.is_empty()
            || elements.iter().any(|e| e.owner != arch_core::Owner::Ai || e.status != arch_core::Status::Draft)
        {
            return fail(
                "bản nháp sinh ra có câu không phải `@ai #draft`: một tên thư mục hay service trong repo chứa ký tự lạ. Không ghi gì.",
                exit::USAGE,
            );
        }
        draft
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
            Err(e) => crate::say_err!("  ⚠ .claude/settings.json: {e}"),
        }
    }

    for pointer in ["AGENTS.md", "CLAUDE.md"] {
        match append_pointer(&root, pointer) {
            Ok(true) => crate::say!("  sửa  {pointer}"),
            Ok(false) => {}
            Err(e) => crate::say_err!("  ⚠ {pointer}: {e}"),
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
    for name in &found.dropped {
        crate::say!("⚠ BỎ QUA  `{name}`: tên có ký tự làm vỡ câu của file luật (xuống dòng, `\"`, `,`, hay đoạn mở đầu bằng `@`, `#`)");
    }
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

/// File mà `init` sửa ở gốc repo (`CLAUDE.md`, `AGENTS.md`, `.gitignore`):
/// symlink trỏ tới file thường trong repo (`CLAUDE.md -> AGENTS.md`, quy ước phổ
/// biến) thì sửa chính file đích, không thay symlink bằng một file mới. Trỏ ra
/// ngoài repo thì không đụng.
fn update_target(root: &Path, name: &str) -> Result<std::path::PathBuf, String> {
    let path = root.join(name);
    let is_link = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link {
        return Ok(path);
    }
    let target = std::fs::canonicalize(&path).map_err(|e| format!("`{name}` là symlink hỏng: {e}"))?;
    let inside = std::fs::canonicalize(root).is_ok_and(|r| target.starts_with(r));
    if inside && target.is_file() {
        Ok(target)
    } else {
        Err(format!("`{name}` là symlink trỏ ra ngoài repo hay không tới file thường: không sửa"))
    }
}

/// Thêm một dòng trỏ về `.arch/ARCHITECTURE.md`. Trả `false` nếu đã có rồi.
fn append_pointer(root: &Path, name: &str) -> Result<bool, String> {
    let path = update_target(root, name)?;
    let mut body = arch_core::read_for_update(&path)?;
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
    let path = update_target(root, ".gitignore")?;
    let mut body = arch_core::read_for_update(&path)?;
    // `**/` chứ không neo vào gốc: monorepo có thể có nhiều `.arch/` lồng
    // trong các dự án con, và mẫu neo gốc bỏ sót hết chúng. Plan Terraform
    // chứa giá trị đã tính, có thể là mật khẩu (TM-58). Từng dòng thêm riêng:
    // repo init từ bản cũ đã có hai dòng đầu mà chưa có dòng plan.
    let missing: Vec<&str> = ["**/.arch/.state", "**/.arch/.verified", "**/.arch/tf-plan*"]
        .into_iter()
        .filter(|line| !body.lines().any(|l| l.trim() == *line))
        .collect();
    if missing.is_empty() {
        return Ok(false);
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    for line in missing {
        body.push_str(line);
        body.push('\n');
    }
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

    let src = arch_core::read_config(&root.join(ARCH_FILE)).unwrap_or_default();
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

    let src = arch_core::read_config(&root.join(ARCH_FILE)).unwrap_or_default();
    let (doc, diags) = arch_core::load(&src);
    if let Some(d) = diags.iter().find(|d| d.severity == arch_core::Severity::Error) {
        return fail(&format!("`{ARCH_FILE}` có lỗi: {}", d.msg), exit::DSL_INVALID);
    }

    let ignores = arch_core::read_ignores(&root);
    let report = arch_core::check::check(&root, &doc, &ignores);
    let now: BTreeSet<String> = report.violations.iter().map(baseline::key).collect();
    let existing = match arch_core::read_for_update(&root.join(BASELINE_FILE)) {
        Ok(text) => text,
        Err(e) => return fail(&e, exit::USAGE),
    };
    let first_run = !root.join(BASELINE_FILE).exists();
    let before = baseline::parse(&existing);

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
    crate::say_err!("archctl: {message}");
    ExitCode::from(code)
}

/// `archctl verify [<tên>...]` — chạy các cách kiểm và ghi biên nhận.
///
/// Chỉ lệnh này chạy lệnh shell, và chỉ khi người gõ nó. `check` không bao
/// giờ chạy gì, kể cả trong hook của agent: nó chỉ đọc biên nhận.
pub fn cmd_verify(args: &[String]) -> ExitCode {
    let parsed = match crate::proto::parse_args(args, &["root", "timeout"], &["quiet"]) {
        Ok(a) => a,
        Err(e) => {
            crate::say_err!("archctl: {e}");
            return ExitCode::from(exit::USAGE);
        }
    };
    let limit = match parsed.flags.get("timeout") {
        None => VERIFY_TIMEOUT_DEFAULT,
        Some(t) => match t.parse::<u64>() {
            Ok(n) if (1..=VERIFY_TIMEOUT_MAX).contains(&n) => n,
            _ => {
                crate::say_err!(
                    "archctl: `--timeout` cần số giây từ 1 tới {VERIFY_TIMEOUT_MAX}, không phải `{t}`"
                );
                return ExitCode::from(exit::USAGE);
            }
        },
    };
    let limit = std::time::Duration::from_secs(limit);
    let start = parsed.flags.get("root").map_or_else(|| PathBuf::from("."), PathBuf::from);
    let Some(root) = arch_core::find_root(&start) else {
        crate::say_err!("archctl: không tìm thấy `{}`", arch_core::ARCH_FILE);
        return ExitCode::from(exit::USAGE);
    };
    let Some(src) = arch_core::read_config(&root.join(arch_core::ARCH_FILE)) else {
        crate::say_err!("archctl: không đọc được `{}`", arch_core::ARCH_FILE);
        return ExitCode::from(exit::USAGE);
    };
    let (doc, diags) = arch_core::load(&src);
    if arch_core::diag::has_errors(&diags) {
        for d in diags.iter().filter(|d| d.severity == arch_core::Severity::Error) {
            crate::say_err!("✗ {}:{d}", arch_core::ARCH_FILE);
        }
        return ExitCode::from(exit::DSL_INVALID);
    }

    let wanted: Vec<&arch_core::model::Verify> = if parsed.positional.is_empty() {
        doc.verifies.iter().collect()
    } else {
        // Một cái tên gõ sai mà vẫn thoát 0 thì trong CI nó là "đạt".
        let unknown: Vec<&String> = parsed
            .positional
            .iter()
            .filter(|name| doc.verify(name).is_none())
            .collect();
        if !unknown.is_empty() {
            crate::say_err!(
                "archctl: không có cách kiểm nào tên: {}",
                unknown.iter().map(|n| n.as_str()).collect::<Vec<_>>().join(", ")
            );
            return ExitCode::from(exit::USAGE);
        }
        doc.verifies.iter().filter(|v| parsed.positional.contains(&v.name)).collect()
    };
    // Cách kiểm chưa có hiệu lực thì `check` không đòi, nên chạy nó ở đây là
    // thực thi một lệnh mà không ai yêu cầu.
    let wanted: Vec<&arch_core::model::Verify> =
        wanted.into_iter().filter(|v| v.meta.status.is_enforced()).collect();

    if wanted.is_empty() {
        if doc.verifies.is_empty() {
            crate::say!("Chưa khai báo cách kiểm nào.");
            crate::say!("  → verify <tên> runs \"<lệnh>\"  trong {}", arch_core::ARCH_FILE);
        } else {
            crate::say!("Không có cách kiểm nào tên như vậy.");
        }
        return ExitCode::from(exit::OK);
    }

    let hash = arch_core::arch_hash(&doc);
    let path = root.join(arch_core::verify::RECEIPTS_FILE);
    let mut receipts = match arch_core::read_for_update(&path) {
        Ok(text) => arch_core::verify::parse(&text),
        Err(e) => return fail(&e, exit::USAGE),
    };
    let quiet = parsed.has("quiet");
    let mut failed = 0usize;

    for v in &wanted {
        crate::say!("▸ {}  {}", v.name, v.command);
        // Lấy mốc **trước** khi chạy. Một bộ test chạy hai phút mà agent sửa
        // file ở phút thứ nhất thì biên nhận lấy mốc lúc xong sẽ mới hơn file
        // vừa sửa, và lần chạy đó im lặng được coi là chứng minh cho mã nó
        // chưa từng thấy.
        let at = arch_core::verify::now();
        // Băm file Terraform cũng lấy trước khi chạy: plan mà lệnh này ghi chỉ
        // chứng minh cho đúng các file đó (TM-57).
        let tf = arch_core::scan::deploy::tf_hash(&root, &arch_core::check::deploy_files(&root, &arch_core::read_ignores(&root)));
        // Lệnh ghi plan: xoá trước các file plan mà lệnh nêu, nên file nào có
        // mặt sau khi chạy là do chính lệnh ghi (TM-57). Plan là thứ sinh ra,
        // ghi lại được; lệnh hỏng thì không còn plan cũ nào bị tưởng là mới.
        let writes_plan = v.command.contains(".arch/tf-plan");
        let named = |p: &String| arch_core::scan::deploy::names_plan(&v.command, p) && !p.contains(char::is_whitespace) && !p.contains('#');
        if writes_plan {
            for p in arch_core::scan::deploy::plan_files(&root).into_iter().filter(named) {
                let _ = std::fs::remove_file(root.join(&p));
            }
        }
        let began = std::time::Instant::now();
        let outcome = run_bounded(&v.command, &root, quiet, limit);
        let passed = outcome == Outcome::Passed;
        if !passed {
            failed += 1;
        }
        let verdict = match outcome {
            Outcome::Passed => "đạt".to_string(),
            Outcome::Failed => "HỎNG".to_string(),
            Outcome::TimedOut(true) => format!("HỎNG (quá hạn {} giây, đã dừng lệnh)", limit.as_secs()),
            Outcome::TimedOut(false) => format!(
                "HỎNG (quá hạn {} giây; chỉ dừng được `sh`, tiến trình con của lệnh có thể còn chạy)",
                limit.as_secs()
            ),
        };
        crate::say!("  {verdict} · {:.1} giây", began.elapsed().as_secs_f64());
        // Lệnh khai là ghi plan Terraform: băm plan và mã module tải về lấy
        // sau khi chạy xong, gắn plan với đúng lệnh này (TM-57).
        let (plans, mods) = if passed && writes_plan {
            let plans: Vec<(String, String)> = arch_core::scan::deploy::plan_files(&root)
                .into_iter()
                .filter(named)
                .map(|p| {
                    let h = arch_core::scan::deploy::file_hash(&root, &p);
                    (p, h)
                })
                .collect();
            let files = arch_core::check::deploy_files(&root, &arch_core::read_ignores(&root));
            (plans, Some(arch_core::scan::deploy::modules_hash(&root, &files)))
        } else {
            (Vec::new(), None)
        };
        receipts.retain(|r| r.name != v.name);
        receipts.push(arch_core::verify::Receipt {
            name: v.name.clone(),
            passed,
            at,
            arch_hash: hash.clone(),
            tf,
            plans,
            mods,
        });
    }

    if let Err(e) = arch_core::write_atomic(&path, &arch_core::verify::render(&receipts)) {
        crate::say_err!("archctl: không ghi được biên nhận: {e}");
        return ExitCode::from(exit::USAGE);
    }
    crate::say!("\n{}/{} đạt · biên nhận ghi vào {}", wanted.len() - failed, wanted.len(),
        arch_core::verify::RECEIPTS_FILE);
    ExitCode::from(if failed > 0 { exit::VIOLATION } else { exit::OK })
}

/// Giới hạn mặc định cho mỗi lệnh `verify` (bạn chốt 14/11/2026: 30 phút).
/// Không có giới hạn thì một lệnh treo làm `verify` — và job CI — treo theo.
const VERIFY_TIMEOUT_DEFAULT: u64 = 30 * 60;
const VERIFY_TIMEOUT_MAX: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed,
    /// Quá hạn. `true`: đã dừng cả cây tiến trình con; `false`: chỉ dừng được
    /// `sh` (máy thiếu `ps`/`kill`, hay không phải unix).
    TimedOut(bool),
}

/// Chạy `sh -c <lệnh>` ở `root`, tối đa `limit`. Quá hạn thì dừng cả cây tiến
/// trình con rồi coi là hỏng.
///
/// Lệnh **không** chạy trong nhóm tiến trình riêng: người gọi archctl (CI,
/// agent-stack) giết nhóm của archctl khi chính họ quá hạn, và một nhóm riêng
/// sẽ sống sót qua lần giết đó. Ctrl-C ở terminal cũng tới được lệnh như cũ.
fn run_bounded(command: &str, root: &Path, quiet: bool, limit: std::time::Duration) -> Outcome {
    use std::process::{Command, Stdio};
    let spawned = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(root)
        .stdout(if quiet { Stdio::null() } else { Stdio::inherit() })
        .stderr(if quiet { Stdio::null() } else { Stdio::inherit() })
        .spawn();
    let Ok(mut child) = spawned else { return Outcome::Failed };
    let deadline = std::time::Instant::now() + limit;
    let mut pause = std::time::Duration::from_millis(5);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return if status.success() { Outcome::Passed } else { Outcome::Failed },
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(std::time::Duration::from_millis(100));
            }
            waited => {
                // `sh` chưa được thu (`try_wait` vừa trả `None` hay lỗi), nên
                // pid của nó và cây con dưới nó vẫn là của lần chạy này (TM-90).
                let whole = kill_tree(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return if matches!(waited, Ok(None)) { Outcome::TimedOut(whole) } else { Outcome::Failed };
            }
        }
    }
}

/// Dừng `pid` và mọi tiến trình con cháu của nó. Đọc cây từ `ps`, gửi STOP
/// cho cả cây trước (không còn ai sinh thêm con hay mồ côi hoá con mình), đọc
/// lại để bắt con vừa sinh, rồi mới KILL. Gọi `ps`, `kill` bằng đường dẫn
/// tuyệt đối: `PATH` là thứ repo và lệnh verify đặt được (TM-90). Con cháu tự
/// tách khỏi cây (`setsid` rồi fork hai lần) thì không thấy — giới hạn đã nhận.
/// `false` khi không dừng được cây (thiếu `ps`/`kill`, không phải unix).
fn kill_tree(pid: u32) -> bool {
    #[cfg(unix)]
    {
        let tool = |names: &[&'static str]| names.iter().copied().find(|p| Path::new(p).is_file());
        let (Some(ps), Some(kill)) = (tool(&["/bin/ps", "/usr/bin/ps"]), tool(&["/bin/kill", "/usr/bin/kill"]))
        else {
            return false;
        };
        let send = |signal: &str, pids: &[u32]| {
            if pids.is_empty() {
                return;
            }
            let _ = std::process::Command::new(kill)
                .arg(signal)
                .args(pids.iter().map(u32::to_string))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        };
        let mut tree = vec![pid];
        for _ in 0..3 {
            let Ok(out) = std::process::Command::new(ps)
                .args(["-A", "-o", "pid=,ppid="])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
            else {
                break;
            };
            let pairs: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|l| {
                    let mut it = l.split_whitespace();
                    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
                })
                .collect();
            let before = tree.len();
            let mut i = 0;
            while i < tree.len() {
                let parent = tree[i];
                for &(child, ppid) in &pairs {
                    if ppid == parent && child != pid && !tree.contains(&child) {
                        tree.push(child);
                    }
                }
                i += 1;
            }
            send("-STOP", &tree);
            if tree.len() == before && before > 1 {
                break;
            }
        }
        // Chính `sh` để cho `Child::kill` (std biết nó chưa được thu).
        let rest: Vec<u32> = tree.into_iter().filter(|p| *p != pid).collect();
        send("-KILL", &rest);
        true
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn verify_mac_dinh_ba_muoi_phut() {
        assert_eq!(super::VERIFY_TIMEOUT_DEFAULT, 1800);
    }

    /// Mọi file cho agent quyền tự cho mình qua đều nằm trong danh sách cấm,
    /// theo đúng hằng đường dẫn (claude-security F13: từng lệch `baseline.arch`).
    #[test]
    fn danh_sach_cam_khop_hang_duong_dan() {
        for file in [
            arch_core::ARCH_FILE,
            arch_core::IGNORE_FILE,
            arch_core::baseline::BASELINE_FILE,
            arch_core::verify::RECEIPTS_FILE,
            arch_core::protocol::DECISIONS_FILE,
            arch_core::protocol::STATE_FILE,
        ] {
            for tool in ["Write", "Edit"] {
                let rule = format!("{tool}(./{file})");
                assert!(super::CLAUDE_SETTINGS.contains(&rule), "thiếu {rule}");
            }
        }
        assert!(!super::CLAUDE_SETTINGS.contains(arch_core::protocol::PROPOSALS_FILE), "agent phải ghi được đề xuất");
    }
}
