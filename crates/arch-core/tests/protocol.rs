//! Test cho định dạng của giao thức: đề xuất, nhật ký quyết định, bộ nhớ.
//!
//! `decisions.md` vừa phải đọc được bằng mắt vừa phải đọc được bằng máy, vì
//! agent tự tra nó. Phần lớn test ở đây khoá lại tính chất đó.

use arch_core::Owner;
use arch_core::protocol::*;

#[test]
fn danh_tinh_de_xuat_chi_phu_thuoc_noi_dung() {
    let a = proposal_id(Intent::Rule, "allow Order -> Cache");
    assert_eq!(a, proposal_id(Intent::Rule, "allow   Order   ->   Cache"));
    assert_ne!(a, proposal_id(Intent::Rule, "allow Order -> Queue"));
    assert_ne!(a, proposal_id(Intent::Block, "allow Order -> Cache"));
    assert_eq!(a.len(), 8, "dạng p- cộng sáu ký tự hex");
}

#[test]
fn de_xuat_ghi_ra_roi_doc_lai_duoc_nguyen_ven() {
    let original = vec![
        Proposal {
            id: proposal_id(Intent::Rule, "allow Order -> Cache"),
            intent: Intent::Rule,
            statement: "allow Order -> Cache".into(),
            reason: "truy vấn danh mục gọi 40 lần mỗi request".into(),
            origin: Some("src/order/catalog.ts:88".into()),
            owner: Owner::Ai,
            date: "2026-10-02".into(),
        },
        Proposal {
            id: proposal_id(Intent::Block, "deny * -> PaymentDB except Payment"),
            intent: Intent::Block,
            statement: "deny * -> PaymentDB except Payment".into(),
            reason: "cần đọc lịch sử giao dịch cho trang đơn hàng".into(),
            origin: None,
            owner: Owner::Ai,
            date: "2026-10-02".into(),
        },
    ];
    let (parsed, diags) = parse_proposals(&render_proposals(&original));
    assert!(diags.is_empty(), "{diags:#?}");
    assert_eq!(parsed, original);
}

#[test]
fn de_xuat_thieu_ly_do_la_loi() {
    let src = "proposal p-aaaaaa @ai 2026-10-02\n  rule allow A -> B\n";
    let (_, diags) = parse_proposals(src);
    assert!(diags.iter().any(|d| d.msg.contains("reason")), "{diags:#?}");
}

#[test]
fn de_xuat_khong_co_cau_lenh_la_loi() {
    let src = "proposal p-aaaaaa @ai 2026-10-02\n  reason vì sao đó\n";
    let (_, diags) = parse_proposals(src);
    assert!(diags.iter().any(|d| d.msg.contains("rule")), "{diags:#?}");
}

#[test]
fn file_de_xuat_rong_doc_duoc_binh_thuong() {
    let (parsed, diags) = parse_proposals("");
    assert!(parsed.is_empty());
    assert!(diags.is_empty());
}

#[test]
fn nhat_ky_quyet_dinh_doc_lai_duoc_bang_may() {
    let d = Decision {
        date: "2026-10-02".into(),
        verdict: Verdict::Rejected,
        id: "p-70fc98".into(),
        statement: "allow Capture -> Report".into(),
        by: "tdHuy22".into(),
        reason: "Capture không được biết tới Report.".into(),
    };
    let text = format!("{DECISIONS_HEADER}{}", render_decision(&d));
    let parsed = parse_decisions(&text);
    assert_eq!(parsed, [d]);
}

#[test]
fn nhieu_quyet_dinh_noi_tiep_nhau() {
    let mut log = String::from(DECISIONS_HEADER);
    for (id, verdict) in [("p-000001", Verdict::Rejected), ("p-000002", Verdict::Accepted)] {
        log.push_str(&render_decision(&Decision {
            date: "2026-10-02".into(),
            verdict,
            id: id.into(),
            statement: "allow A -> B".into(),
            by: "ai đó".into(),
            reason: "lý do".into(),
        }));
    }
    let parsed = parse_decisions(&log);
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].verdict, Verdict::Rejected);
    assert_eq!(parsed[1].verdict, Verdict::Accepted);
}

/// Người có thể sửa tay `decisions.md`. Dòng lạ không được làm hỏng việc đọc
/// những mục còn lại.
#[test]
fn dong_la_trong_nhat_ky_khong_lam_hong_viec_doc() {
    let log = "# tiêu đề\n\nghi chú tự do\n\n## 2026-10-02 · TỪ CHỐI · p-abc123\nallow A -> B\n\n**Lý do:** không\n\nthêm vài dòng nữa\n";
    let parsed = parse_decisions(log);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].id, "p-abc123");
    assert_eq!(parsed[0].reason, "không");
}

#[test]
fn bo_nho_giua_cac_lan_chay_ghi_va_doc_lai_duoc() {
    let mut state = State { arch_hash: "b7e21d04".into(), ..State::default() };
    state.attempts.insert("Stock->Payment|src/stock/reserve.ts".into(), 3);
    state.attempts.insert("Web->Stripe|src/web/page.ts".into(), 1);
    assert_eq!(parse_state(&render_state(&state)), state);
}

#[test]
fn bo_nho_hong_khong_lam_do_chuong_trinh() {
    let state = parse_state("rác\narch_hash\nattempt thiếu-số\nattempt ok 2\n");
    assert_eq!(state.attempts.get("ok"), Some(&2));
}

#[test]
fn ngay_doi_tu_epoch_dung() {
    let t = today();
    assert_eq!(t.len(), 10);
    assert_eq!(t.matches('-').count(), 2);
}
