//! Sơ đồ ở quy mô thật.
//!
//! Fixture chỉ có vài component nên mọi bố cục đều trông ổn. Chế độ hỏng chỉ
//! lộ ra khi đồ thị vừa lớn vừa dày: `hugo` có 24 component và 156 quan hệ,
//! trong đó 58 quan hệ khép thành vòng.

use arch_core::layout::{Saved, place};

fn doc(src: &str) -> arch_core::Document {
    let (d, diags) = arch_core::load(src);
    assert!(!arch_core::diag::has_errors(&diags), "{diags:#?}");
    d
}

/// `(rộng, cao, số cột)` của sơ đồ.
fn shape(src: &str) -> (i64, i64, usize) {
    let points = place(&doc(src), &Saved::default());
    let xs: Vec<i64> = points.values().map(|p| p.x as i64).collect();
    let ys: Vec<i64> = points.values().map(|p| p.y as i64).collect();
    let mut cols: Vec<i64> = xs.clone();
    cols.sort_unstable();
    cols.dedup();
    (
        xs.iter().max().unwrap() - xs.iter().min().unwrap() + 180,
        ys.iter().max().unwrap() - ys.iter().min().unwrap() + 56,
        cols.len(),
    )
}

/// Đồ thị phụ thuộc thật của hugo: 24 component, 155 quan hệ, 58 trong số đó
/// khép thành vòng. Đồ thị sinh ngẫu nhiên không tái tạo được chế độ hỏng này
/// — chỉ hình dạng thật mới làm lộ ra.
const HUGO: &str = include_str!("fixtures/hugo.arch");

/// Mọi component nối với mọi component: trường hợp xấu nhất cho việc xếp lớp.
fn dense(n: usize) -> String {
    let mut src = String::new();
    for i in 0..n {
        src.push_str(&format!("component C{i} at c{i}/** @human #accepted\n"));
    }
    for i in 0..n {
        for j in 0..n {
            if i != j {
                src.push_str(&format!("allow C{i} -> C{j}\n"));
            }
        }
    }
    src
}

#[test]
fn chu_trinh_khong_lam_so_do_no_be_ngang() {
    // Phân lớp bằng nới lỏng lặp mà không bỏ cạnh quay ngược ra thì mỗi vòng
    // đẩy cả vòng tròn lên thêm một bậc. Sơ đồ hugo từng trải ra 63540 điểm
    // ảnh bề ngang — gấp bảy mươi lần khung nhìn.
    let (w, h, cols) = shape(HUGO);
    assert!(cols <= 24, "{cols} cột cho 24 component");
    assert!(w < 4000, "rộng {w} điểm ảnh");
    assert!(w / h < 10, "sơ đồ dẹt {}:1, không nhìn được", w / h);
}

/// Số lớp không bao giờ được vượt số component: với một đồ thị không còn chu
/// trình, đường đi dài nhất tối đa là đi qua mỗi đỉnh một lần.
#[test]
fn so_lop_khong_vuot_so_component() {
    for src in [HUGO, &dense(12), &dense(3)] {
        let n = src.matches("component ").count();
        let (_, _, cols) = shape(src);
        assert!(cols <= n, "{cols} cột cho {n} component");
    }
}

#[test]
fn vong_tron_hai_chieu_van_xep_duoc() {
    let src = "\
component A at a/** @human #accepted
component B at b/** @human #accepted
allow A -> B
allow B -> A
";
    let (_, _, cols) = shape(src);
    assert_eq!(cols, 2, "A và B vẫn phải nằm hai cột khác nhau");
}

#[test]
fn tu_tro_vao_minh_khong_lam_hong_gi() {
    let src = "\
component A at a/** @human #accepted
component B at b/** @human #accepted
allow A -> A
allow A -> B
";
    let (_, _, cols) = shape(src);
    assert_eq!(cols, 2);
}

#[test]
fn chuoi_phu_thuoc_dai_khong_lam_tran_ngan_xep() {
    // Đồ thị do người dùng viết ra, nên một chuỗi rất dài không được làm sập
    // chương trình.
    let mut src = String::new();
    for i in 0..400 {
        src.push_str(&format!("component C{i} at c{i}/** @human #accepted\n"));
    }
    for i in 0..399 {
        src.push_str(&format!("allow C{i} -> C{}\n", i + 1));
    }
    let points = place(&doc(&src), &Saved::default());
    assert_eq!(points.len(), 400);
}

#[test]
fn hai_lan_xep_cho_cung_mot_ket_qua() {
    let src = dense(9);
    let first = place(&doc(&src), &Saved::default());
    for _ in 0..5 {
        assert_eq!(place(&doc(&src), &Saved::default()), first);
    }
}

#[test]
fn so_do_thua_van_giu_duoc_hinh_dang_hop_ly() {
    // Đồ thị hình cây: chiều ngang không được vượt quá chiều sâu thật.
    let src = "\
component Root at r/** @human #accepted
component A at a/** @human #accepted
component B at b/** @human #accepted
component C at c/** @human #accepted
allow Root -> A
allow Root -> B
allow Root -> C
";
    let (_, _, cols) = shape(src);
    assert_eq!(cols, 2, "gốc một cột, ba lá một cột");
}
