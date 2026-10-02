//! Giao thức giữa agent và người: đề xuất, bế tắc, và nhật ký quyết định.
//!
//! Ba file, mỗi file **một người ghi duy nhất**. Đây là quyết định thiết kế
//! quan trọng nhất của cả hệ thống: nó triệt tiêu xung đột ghi đè và việc agent
//! vượt quyền ngay từ cấu trúc, thay vì phải phát hiện rồi xử lý sau.
//!
//! | File | Ai ghi |
//! | --- | --- |
//! | `system.arch` | người, qua `accept` |
//! | `proposals.arch` | agent, qua `propose` và `block` |
//! | `decisions.md` | `accept` và `reject` |
//!
//! `decisions.md` là phần dễ bị xem nhẹ nhất nhưng hữu dụng nhất: agent không
//! có trí nhớ giữa các phiên làm việc, nên nếu không ghi lại, hôm nay người từ
//! chối một đề xuất thì ngày mai agent đề xuất lại y hệt.

use crate::diag::Diag;
use crate::model::Owner;
use std::collections::BTreeMap;
use std::fmt::Write as _;

pub const PROPOSALS_FILE: &str = ".arch/proposals.arch";
pub const DECISIONS_FILE: &str = ".arch/decisions.md";
pub const STATE_FILE: &str = ".arch/.state";

/// Số lần gặp lại cùng một vi phạm trước khi nhắc agent dừng và báo bế tắc.
pub const MAX_TRIES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Intent {
    /// Agent muốn thêm một luật mới.
    Rule,
    /// Agent không làm đúng luật hiện có được và đang chờ quyết định.
    Block,
}

impl Intent {
    pub fn keyword(self) -> &'static str {
        match self {
            Intent::Rule => "rule",
            Intent::Block => "block",
        }
    }

    fn from_keyword(word: &str) -> Option<Intent> {
        match word {
            "rule" => Some(Intent::Rule),
            "block" => Some(Intent::Block),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    pub id: String,
    pub intent: Intent,
    /// Luật được đề xuất, hoặc luật đang chặn.
    pub statement: String,
    pub reason: String,
    /// Chỗ trong mã làm nảy sinh đề xuất, dạng `file:dòng`.
    pub origin: Option<String>,
    pub owner: Owner,
    pub date: String,
}

/// Danh tính của đề xuất chỉ phụ thuộc nội dung, nên đề xuất lại đúng thứ đã
/// đề xuất là không-thao-tác, và đối chiếu được với nhật ký quyết định.
pub fn proposal_id(intent: Intent, statement: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in format!("{}|{}", intent.keyword(), normalize_space(statement)).bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("p-{:06x}", ((h as u32) ^ ((h >> 32) as u32)) & 0x00ff_ffff)
}

fn normalize_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------- proposals.arch ----------

pub fn parse_proposals(src: &str) -> (Vec<Proposal>, Vec<Diag>) {
    let mut out: Vec<Proposal> = Vec::new();
    let mut diags = Vec::new();

    for (idx, raw) in src.lines().enumerate() {
        let no = idx + 1;
        let text = raw.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }

        if let Some(rest) = text.strip_prefix("proposal ") {
            let mut parts = rest.split_whitespace();
            let id = parts.next().unwrap_or_default().to_string();
            let owner = if parts.next() == Some("@human") { Owner::Human } else { Owner::Ai };
            let date = parts.next().unwrap_or("").to_string();
            out.push(Proposal {
                id,
                intent: Intent::Rule,
                statement: String::new(),
                reason: String::new(),
                origin: None,
                owner,
                date,
            });
            continue;
        }

        let Some(current) = out.last_mut() else {
            diags.push(
                Diag::error(no, "dòng này không thuộc đề xuất nào")
                    .with_hint("mỗi khối bắt đầu bằng `proposal <id> @ai <ngày>`"),
            );
            continue;
        };
        let (key, value) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let value = value.trim();
        match key {
            "rule" | "block" => {
                current.intent = Intent::from_keyword(key).unwrap_or(Intent::Rule);
                current.statement = value.to_string();
            }
            "reason" => current.reason = value.to_string(),
            "from" => current.origin = Some(value.to_string()),
            other => diags
                .push(Diag::error(no, format!("khoá không nhận ra trong đề xuất: `{other}`"))),
        }
    }

    for p in &out {
        if p.statement.is_empty() {
            diags.push(Diag::error(0, format!("đề xuất `{}` không có `rule` hay `block`", p.id)));
        }
        if p.reason.is_empty() {
            diags.push(
                Diag::error(0, format!("đề xuất `{}` không có `reason`", p.id))
                    .with_hint("lý do là thứ người duyệt đọc để quyết định"),
            );
        }
    }
    (out, diags)
}

pub fn render_proposals(proposals: &[Proposal]) -> String {
    let mut out = String::from(
        "# Đề xuất của agent, chờ người duyệt.\n\
         # Agent ghi bằng `archctl propose` và `archctl block`. Đừng sửa tay.\n\
         # Người duyệt bằng `archctl accept <id>` hoặc `archctl reject <id> --reason \"...\"`.\n",
    );
    for p in proposals {
        let owner = if p.owner == Owner::Ai { "@ai" } else { "@human" };
        let _ = write!(out, "\nproposal {} {owner} {}\n", p.id, p.date);
        let _ = writeln!(out, "  {} {}", p.intent.keyword(), p.statement);
        let _ = writeln!(out, "  reason {}", p.reason);
        if let Some(origin) = &p.origin {
            let _ = writeln!(out, "  from {origin}");
        }
    }
    out
}

// ---------- decisions.md ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Accepted,
    Rejected,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Accepted => "DUYỆT",
            Verdict::Rejected => "TỪ CHỐI",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub date: String,
    pub verdict: Verdict,
    pub id: String,
    pub statement: String,
    pub by: String,
    pub reason: String,
}

/// Mỗi quyết định là một mục `## <ngày> · <phán quyết> · <id>`. Định dạng này
/// vừa đọc được bằng mắt vừa đọc được bằng máy, vì agent phải tự tra được.
pub fn parse_decisions(src: &str) -> Vec<Decision> {
    let mut out: Vec<Decision> = Vec::new();
    for raw in src.lines() {
        let text = raw.trim();
        if let Some(head) = text.strip_prefix("## ") {
            let parts: Vec<&str> = head.split('·').map(str::trim).collect();
            let [date, verdict, id] = parts.as_slice() else { continue };
            let verdict = match *verdict {
                v if v == Verdict::Accepted.label() => Verdict::Accepted,
                v if v == Verdict::Rejected.label() => Verdict::Rejected,
                _ => continue,
            };
            out.push(Decision {
                date: (*date).to_string(),
                verdict,
                id: (*id).to_string(),
                statement: String::new(),
                by: String::new(),
                reason: String::new(),
            });
            continue;
        }
        let Some(current) = out.last_mut() else { continue };
        if let Some(v) = text.strip_prefix("**Người quyết định:**") {
            current.by = v.trim().to_string();
        } else if let Some(v) = text.strip_prefix("**Lý do:**") {
            current.reason = v.trim().to_string();
        } else if !text.is_empty() && current.statement.is_empty() && !text.starts_with("**") {
            current.statement = text.to_string();
        }
    }
    out
}

pub fn render_decision(d: &Decision) -> String {
    format!(
        "\n## {} · {} · {}\n{}\n\n**Người quyết định:** {}\n**Lý do:** {}\n",
        d.date,
        d.verdict.label(),
        d.id,
        d.statement,
        d.by,
        d.reason
    )
}

pub const DECISIONS_HEADER: &str = "# Nhật ký quyết định kiến trúc\n\n\
     archctl ghi tự động mỗi lần duyệt hoặc từ chối một đề xuất.\n\
     Agent đọc file này để không đề xuất lại thứ đã bị từ chối. Đừng xoá mục nào.\n";

// ---------- .arch/.state ----------

/// Bộ nhớ giữa các lần chạy `check`. Định dạng dòng đơn giản thay vì JSON, để
/// không cần thêm dependency chỉ để đọc vài con số.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    /// Hash kiến trúc ở lần kiểm trước, để biết luật đã đổi mà báo agent đọc lại.
    pub arch_hash: String,
    /// Số lần gặp lại cùng một vi phạm. Khoá là danh tính của vi phạm.
    pub attempts: BTreeMap<String, usize>,
}

pub fn parse_state(src: &str) -> State {
    let mut state = State::default();
    for line in src.lines() {
        let line = line.trim();
        if let Some(hash) = line.strip_prefix("arch_hash ") {
            state.arch_hash = hash.trim().to_string();
        } else if let Some(rest) = line.strip_prefix("attempt ")
            && let Some((key, count)) = rest.rsplit_once(' ')
            && let Ok(n) = count.trim().parse()
        {
            state.attempts.insert(key.trim().to_string(), n);
        }
    }
    state
}

pub fn render_state(state: &State) -> String {
    let mut out = String::from("# archctl sinh tự động. Không cần đưa vào git.\n");
    let _ = writeln!(out, "arch_hash {}", state.arch_hash);
    for (key, n) in &state.attempts {
        let _ = writeln!(out, "attempt {key} {n}");
    }
    out
}

// ---------- ngày tháng ----------

/// Ngày hôm nay dạng `YYYY-MM-DD` theo UTC.
///
/// Tự tính từ epoch thay vì dùng thư viện, để giữ cây dependency bằng không.
/// Thuật toán civil-from-days của Howard Hinnant.
pub fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_chi_phu_thuoc_noi_dung() {
        let a = proposal_id(Intent::Rule, "allow Order -> Cache");
        let b = proposal_id(Intent::Rule, "allow   Order  ->  Cache");
        assert_eq!(a, b, "khoảng trắng không đổi danh tính");
        assert_ne!(a, proposal_id(Intent::Block, "allow Order -> Cache"));
    }

    #[test]
    fn doi_ngay_tu_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_728), (2026, 10, 2));
    }

    #[test]
    fn hom_nay_dung_dinh_dang() {
        let t = today();
        assert_eq!(t.len(), 10);
        assert!(t.starts_with("20"), "nhận: {t}");
    }
}
