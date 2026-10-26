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
    // 64 bit đầu của SHA-256: `accept` tính lại id từ nội dung, nên id phải
    // không dò được câu khác cùng id (FNV 24 bit thì vài giây, review opus).
    let digest = sha256::hex(format!("{}|{}", intent.keyword(), normalize_space(statement)).as_bytes());
    format!("p-{}", &digest[..16])
}

fn normalize_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------- proposals.arch ----------

pub fn parse_proposals(src: &str) -> (Vec<Proposal>, Vec<Diag>) {
    let src = crate::strip_bom(src);
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
    let src = crate::strip_bom(src);
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
    let src = crate::strip_bom(src);
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


/// SHA-256 (FIPS 180-4), viết bằng std vì crate không có dependency nào. Id
/// đề xuất do agent ghi phải gắn chặt với nội dung của nó: FNV 24 bit thì agent
/// dò được câu khác cùng id trong vài giây (review opus 2026-10-25). Dấu vân tay
/// của luật (`arch_hash` trong `lib.rs`) cũng dùng nó.
pub(crate) mod sha256 {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
        0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
        0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
        0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
        0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
        0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
        0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
    ];

    /// Digest 32 byte của `data`.
    fn digest(data: &[u8]) -> [u8; 32] {
        let mut h: [u32; 8] =
            [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19];
        let mut msg = data.to_vec();
        let bits = (data.len() as u64).wrapping_mul(8);
        msg.push(0x80);
        while msg.len() % 64 != 56 {
            msg.push(0);
        }
        msg.extend_from_slice(&bits.to_be_bytes());
        for block in msg.as_chunks::<64>().0 {
            let mut w = [0u32; 64];
            for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
                w[i] = u32::from_be_bytes(*word);
            }
            for i in 16..64 {
                let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
                let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
                w[i] = w[i - 16].wrapping_add(s0).wrapping_add(w[i - 7]).wrapping_add(s1);
            }
            let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
            for i in 0..64 {
                let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
                let ch = (e & f) ^ (!e & g);
                let t1 = hh.wrapping_add(s1).wrapping_add(ch).wrapping_add(K[i]).wrapping_add(w[i]);
                let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
                let maj = (a & b) ^ (a & c) ^ (b & c);
                let t2 = s0.wrapping_add(maj);
                hh = g;
                g = f;
                f = e;
                e = d.wrapping_add(t1);
                d = c;
                c = b;
                b = a;
                a = t1.wrapping_add(t2);
            }
            for (slot, v) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
                *slot = slot.wrapping_add(v);
            }
        }
        let mut out = [0u8; 32];
        for (i, v) in h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        out
    }

    /// Chữ hex thường của digest.
    pub(crate) fn hex(data: &[u8]) -> String {
        digest(data).iter().map(|b| format!("{b:02x}")).collect()
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn vector_chuan() {
            assert_eq!(super::hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
            assert_eq!(super::hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
            assert_eq!(
                super::hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
                "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
            );
            assert_eq!(
                super::hex(&[b'a'; 1_000_000]),
                "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
            );
        }
    }
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
