//! Xếp vị trí các node trên sơ đồ.
//!
//! Layout phân tầng rút gọn: gán tầng theo đường dài nhất, xếp thứ tự trong
//! tầng bằng trọng tâm, rồi quy ra toạ độ. Đủ tốt cho đồ thị vài chục node, và
//! quan trọng hơn là **xác định** — cùng đầu vào luôn cho cùng kết quả.
//!
//! Tính chất cốt yếu là **ổn định**: node đã có toạ độ thì giữ nguyên. Sơ đồ
//! nhảy loạn mỗi lần agent thêm một component là lỗi hỏng trải nghiệm nghiêm
//! trọng — người dùng mất phương hướng rồi bỏ không dùng nữa.

use crate::model::{Document, RuleKind, Target};
use std::collections::{BTreeMap, BTreeSet};

pub const NODE_W: f64 = 180.0;
/// Tên node của một service trên sơ đồ. Service có không gian tên riêng, nên
/// `service Api` và `component Api` phải là hai node khác nhau.
pub const SERVICE_PREFIX: &str = "svc:";
pub const NODE_H: f64 = 56.0;
const GAP_X: f64 = 84.0;
const GAP_Y: f64 = 28.0;
const MARGIN: f64 = 48.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Toạ độ đã lưu, đọc từ `.arch/layout.json`.
pub type Saved = BTreeMap<String, Point>;

/// Xếp mọi node. Node có trong `saved` giữ nguyên chỗ cũ; chỉ node mới được
/// tính toạ độ.
pub fn place(doc: &Document, saved: &Saved) -> BTreeMap<String, Point> {
    let nodes = node_names(doc);
    let edges = edge_list(doc, &nodes);
    let mut layers = assign_layers(&nodes, &edges);
    sink_apis(doc, &mut layers);
    let mut ordered = order_within_layers(&nodes, &edges, &layers);
    cluster_boundaries(doc, &mut ordered);
    let mut out = coordinates(&ordered);

    for (name, point) in saved {
        if out.contains_key(name) {
            out.insert(name.clone(), *point);
        }
    }
    out
}

fn node_names(doc: &Document) -> Vec<String> {
    let mut names: Vec<String> = doc
        .components
        .iter()
        .map(|c| c.name.clone())
        .chain(doc.stores.iter().map(|s| s.name.clone()))
        .chain(doc.apis.iter().map(|a| a.name.clone()))
        .chain(doc.services.iter().map(|s| format!("{SERVICE_PREFIX}{}", s.name)))
        .collect();
    names.sort();
    names.dedup();
    names
}

fn edge_list(doc: &Document, nodes: &[String]) -> Vec<(String, String)> {
    let known: BTreeSet<&str> = nodes.iter().map(String::as_str).collect();
    let mut edges = BTreeSet::new();
    for rule in doc.rules.iter().filter(|r| r.kind == RuleKind::Allow) {
        let Target::Named(from) = &rule.from else { continue };
        for target in &rule.to {
            let Target::Named(to) = target else { continue };
            if from != to && known.contains(from.as_str()) && known.contains(to.as_str()) {
                edges.insert((from.clone(), to.clone()));
            }
        }
    }
    // Kho dữ liệu nằm cạnh chủ sở hữu của nó.
    for store in &doc.stores {
        if known.contains(store.owned_by.as_str()) {
            edges.insert((store.owned_by.clone(), store.name.clone()));
        }
    }
    // Service đứng trước mã mà nó chạy, và trước service mà nó gọi.
    for s in &doc.services {
        let from = format!("{SERVICE_PREFIX}{}", s.name);
        for c in s.runs.iter().filter(|c| known.contains(c.as_str())) {
            edges.insert((from.clone(), c.clone()));
        }
    }
    for l in &doc.links {
        for to in l.to.iter().filter(|t| **t != l.from) {
            edges.insert((format!("{SERVICE_PREFIX}{}", l.from), format!("{SERVICE_PREFIX}{to}")));
        }
    }
    edges.into_iter().collect()
}

/// Nới lỏng lặp thay vì sắp xếp tô-pô, để đồ thị có chu trình vẫn dừng được.
/// Cạnh quay ngược chỉ đơn giản là không kéo được tầng lên nữa.
fn assign_layers(nodes: &[String], edges: &[(String, String)]) -> BTreeMap<String, usize> {
    let back = back_edges(nodes, edges);
    let mut layer: BTreeMap<String, usize> = nodes.iter().map(|n| (n.clone(), 0)).collect();
    for _ in 0..nodes.len() {
        let mut moved = false;
        for (i, (from, to)) in edges.iter().enumerate() {
            if back.contains(&i) {
                continue;
            }
            let want = layer[from] + 1;
            if layer[to] < want {
                layer.insert(to.clone(), want);
                moved = true;
            }
        }
        if !moved {
            break;
        }
    }
    layer
}

/// Chỉ số của những cạnh đi ngược chiều phân tầng.
///
/// Xếp các component thành một hàng theo **bậc ra trừ bậc vào**: gọi nhiều mà
/// bị gọi ít thì đứng trước. Mọi cạnh đi ngược hàng đó là cạnh quay ngược, và
/// bỏ chúng ra thì phần còn lại chắc chắn không còn chu trình.
///
/// Thứ tự ấy đúng với trực giác phân tầng kiến trúc, và nó cũng cho sơ đồ gọn
/// nhất trong ba cách đã thử. Trên mười repo thật:
///
/// | cách chọn | cạnh ngược | sơ đồ dẹt nhất |
/// | --- | --- | --- |
/// | duyệt sâu | 112 | 26.8:1 |
/// | bậc ra trừ bậc vào | 84 | **12.6:1** |
/// | Eades–Lin–Smyth | 74 | 16.4:1 |
///
/// Không bỏ cạnh quay ngược ra thì phân lớp không dừng đúng chỗ — mỗi vòng
/// lặp đẩy cả vòng lên thêm một bậc, và sơ đồ hugo trải ra 63540 điểm ảnh bề
/// ngang.
///
/// Cạnh bị bỏ ở đây chỉ bỏ khỏi việc **xếp lớp**; nó vẫn được vẽ như mọi cạnh
/// khác, chỉ là đi ngược chiều chung.
fn back_edges(nodes: &[String], edges: &[(String, String)]) -> BTreeSet<usize> {
    let mut out_degree: BTreeMap<&str, isize> = nodes.iter().map(|n| (n.as_str(), 0)).collect();
    let mut in_degree: BTreeMap<&str, isize> = out_degree.clone();
    for (from, to) in edges {
        if from == to {
            continue;
        }
        *out_degree.entry(from.as_str()).or_default() += 1;
        *in_degree.entry(to.as_str()).or_default() += 1;
    }

    let mut order: Vec<&str> = nodes.iter().map(String::as_str).collect();
    // Hoà thì theo tên, để hai lần chạy cho cùng một sơ đồ.
    order.sort_by_key(|n| {
        (std::cmp::Reverse(out_degree[n] - in_degree[n]), *n)
    });
    let rank: BTreeMap<&str, usize> = order.iter().enumerate().map(|(i, n)| (*n, i)).collect();

    edges
        .iter()
        .enumerate()
        .filter(|(_, (from, to))| rank.get(from.as_str()) >= rank.get(to.as_str()))
        .map(|(i, _)| i)
        .collect()
}

/// Nhóm `api` luôn là điểm cuối: không gì phụ thuộc vào chúng, còn mọi nơi
/// đều có thể gọi vào. Không ép thì chúng nằm ở cột đầu — vì trong luật không
/// ai trỏ tới chúng — và mọi cạnh vi phạm phải vòng ngược lại.
fn sink_apis(doc: &Document, layers: &mut BTreeMap<String, usize>) {
    if doc.apis.is_empty() {
        return;
    }
    let deepest = layers
        .iter()
        .filter(|(name, _)| doc.api(name).is_none())
        .map(|(_, level)| *level)
        .max()
        .unwrap_or(0);
    for api in &doc.apis {
        layers.insert(api.name.clone(), deepest + 1);
    }
}

/// Thứ tự trong mỗi tầng, giảm số cạnh cắt nhau bằng trọng tâm các node liền
/// trước. Khởi đầu theo thứ tự chữ cái để kết quả không đổi giữa các lần chạy.
#[allow(clippy::type_complexity)]
fn order_within_layers(
    nodes: &[String],
    edges: &[(String, String)],
    layers: &BTreeMap<String, usize>,
) -> Vec<Vec<String>> {
    let depth = layers.values().copied().max().unwrap_or(0) + 1;
    let mut grid: Vec<Vec<String>> = vec![Vec::new(); depth];
    for name in nodes {
        grid[layers[name]].push(name.clone());
    }

    let mut incoming: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let mut outgoing: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in edges {
        incoming.entry(to).or_default().push(from);
        outgoing.entry(from).or_default().push(to);
    }

    // Quét xuôi rồi quét ngược: chỉ nhìn node liền trước thì những node không
    // có ai trỏ tới sẽ bị dồn xuống đáy, kéo cạnh của chúng cắt ngang cả cột.
    const PASSES: usize = 6;
    for pass in 0..PASSES {
        let forward = pass % 2 == 0;
        let levels: Vec<usize> =
            if forward { (1..depth).collect() } else { (0..depth.saturating_sub(1)).rev().collect() };
        for level in levels {
            let neighbour = if forward { level - 1 } else { level + 1 };
            let positions: BTreeMap<&str, usize> =
                grid[neighbour].iter().enumerate().map(|(i, n)| (n.as_str(), i)).collect();
            let links = if forward { &incoming } else { &outgoing };

            let mut keyed: Vec<(f64, usize, String)> = grid[level]
                .iter()
                .enumerate()
                .map(|(current, name)| {
                    let near: Vec<usize> = links
                        .get(name.as_str())
                        .map(|ns| ns.iter().filter_map(|n| positions.get(n).copied()).collect())
                        .unwrap_or_default();
                    // Không có láng giềng thì giữ nguyên chỗ cũ, thay vì bị
                    // đẩy về một đầu.
                    let center = if near.is_empty() {
                        current as f64
                    } else {
                        near.iter().sum::<usize>() as f64 / near.len() as f64
                    };
                    (center, current, name.clone())
                })
                .collect();
            keyed.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            grid[level] = keyed.into_iter().map(|(_, _, n)| n).collect();
        }
    }
    grid
}

/// Gom thành viên của cùng một `boundary` lại cạnh nhau trong mỗi cột.
///
/// Khung boundary được vẽ bằng hình chữ nhật bao quanh các thành viên. Nếu
/// chúng nằm rải rác thì một component **không thuộc** boundary sẽ tình cờ lọt
/// vào trong khung, và người xem đọc sai kiến trúc.
fn cluster_boundaries(doc: &Document, grid: &mut [Vec<String>]) {
    if doc.boundaries.is_empty() {
        return;
    }
    let group_of = |name: &str| {
        doc.boundaries.iter().position(|b| b.contains.iter().any(|m| m == name))
    };

    for column in grid.iter_mut() {
        // Tâm của mỗi nhóm trong cột này. Node không thuộc nhóm nào giữ nguyên
        // chỗ, nên phần còn lại của sơ đồ không bị xáo theo.
        let mut sums: BTreeMap<usize, (f64, f64)> = BTreeMap::new();
        for (at, name) in column.iter().enumerate() {
            if let Some(group) = group_of(name) {
                let entry = sums.entry(group).or_insert((0.0, 0.0));
                entry.0 += at as f64;
                entry.1 += 1.0;
            }
        }

        let mut keyed: Vec<(f64, usize, usize, String)> = column
            .iter()
            .enumerate()
            .map(|(at, name)| {
                let group = group_of(name);
                let center = group
                    .and_then(|g| sums.get(&g))
                    .map_or(at as f64, |(total, count)| total / count);
                // Khoá phụ là chỉ số nhóm: cùng tâm thì thành viên của một
                // nhóm vẫn phải dính vào nhau, và node ngoài nhóm bị đẩy ra.
                (center, group.unwrap_or(usize::MAX), at, name.clone())
            })
            .collect();
        keyed.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        *column = keyed.into_iter().map(|(_, _, _, n)| n).collect();
    }
}

fn coordinates(grid: &[Vec<String>]) -> BTreeMap<String, Point> {
    let tallest = grid.iter().map(Vec::len).max().unwrap_or(0) as f64;
    let full = tallest * NODE_H + (tallest - 1.0).max(0.0) * GAP_Y;

    let mut out = BTreeMap::new();
    for (level, column) in grid.iter().enumerate() {
        let count = column.len() as f64;
        let height = count * NODE_H + (count - 1.0).max(0.0) * GAP_Y;
        let top = MARGIN + (full - height) / 2.0;
        for (row, name) in column.iter().enumerate() {
            out.insert(
                name.clone(),
                Point {
                    x: MARGIN + level as f64 * (NODE_W + GAP_X),
                    y: top + row as f64 * (NODE_H + GAP_Y),
                },
            );
        }
    }
    out
}

/// Đọc `.arch/layout.json`. Hỏng thì coi như chưa có gì — vị trí là thứ xếp
/// lại được, không đáng để làm hỏng cả phiên làm việc.
pub fn parse_saved(src: &str) -> Saved {
    let src = crate::strip_bom(src);
    let Some(value) = crate::jsonc::parse(src) else { return Saved::new() };
    let Some(nodes) = value.get("nodes").and_then(|n| n.as_object()) else {
        return Saved::new();
    };
    nodes
        .iter()
        .filter_map(|(name, point)| {
            let x = point.get("x")?.as_str()?.parse().ok()?;
            let y = point.get("y")?.as_str()?.parse().ok()?;
            Some((name.clone(), Point { x, y }))
        })
        .collect()
}

pub fn render_saved(saved: &Saved) -> String {
    let body: Vec<String> = saved
        .iter()
        .map(|(name, p)| {
            format!("    {}: {{ \"x\": {:.0}, \"y\": {:.0} }}", crate::json::string(name), p.x, p.y)
        })
        .collect();
    format!("{{\n  \"nodes\": {{\n{}\n  }}\n}}\n", body.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(src: &str) -> Document {
        let (d, diags) = crate::load(src);
        assert!(!crate::diag::has_errors(&diags), "{diags:#?}");
        d
    }

    #[test]
    fn chuoi_phu_thuoc_xep_thanh_cac_cot_tu_trai_sang_phai() {
        let d = doc(
            "component A at a/**\ncomponent B at b/**\ncomponent C at c/**\n\
             allow A -> B\nallow B -> C",
        );
        let p = place(&d, &Saved::new());
        assert!(p["A"].x < p["B"].x, "{p:?}");
        assert!(p["B"].x < p["C"].x, "{p:?}");
    }

    #[test]
    fn node_khong_lien_quan_nam_cung_cot_dau() {
        let d = doc("component A at a/**\ncomponent B at b/**");
        let p = place(&d, &Saved::new());
        assert_eq!(p["A"].x, p["B"].x);
        assert_ne!(p["A"].y, p["B"].y, "không được chồng lên nhau");
    }

    #[test]
    fn chu_trinh_khong_lam_treo() {
        let d = doc(
            "component A at a/**\ncomponent B at b/**\n\
             allow A -> B\nallow B -> A",
        );
        let p = place(&d, &Saved::new());
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn kho_du_lieu_nam_canh_chu_so_huu() {
        let d = doc("component A at a/**\nstore Db owned_by A at \"t\"");
        let p = place(&d, &Saved::new());
        assert!(p["A"].x < p["Db"].x, "{p:?}");
    }

    /// Tính chất quan trọng nhất: thêm node mới không được làm xê dịch node cũ.
    #[test]
    fn vi_tri_da_luu_duoc_giu_nguyen() {
        let d = doc("component A at a/**\ncomponent B at b/**\nallow A -> B");
        let mut saved = Saved::new();
        saved.insert("A".into(), Point { x: 999.0, y: 42.0 });
        let p = place(&d, &saved);
        assert_eq!(p["A"], Point { x: 999.0, y: 42.0 });
        assert!(p.contains_key("B"), "node mới vẫn được xếp");
    }

    #[test]
    fn vi_tri_cua_node_da_bi_xoa_thi_bo_qua() {
        let d = doc("component A at a/**");
        let mut saved = Saved::new();
        saved.insert("DaXoa".into(), Point { x: 1.0, y: 2.0 });
        let p = place(&d, &saved);
        assert!(!p.contains_key("DaXoa"));
    }

    #[test]
    fn ket_qua_khong_doi_giua_cac_lan_chay() {
        let src = "component A at a/**\ncomponent B at b/**\ncomponent C at c/**\n\
                   allow A -> C\nallow B -> C";
        let first = place(&doc(src), &Saved::new());
        for _ in 0..5 {
            assert_eq!(place(&doc(src), &Saved::new()), first);
        }
    }

    #[test]
    fn luu_ra_roi_doc_lai_duoc() {
        let mut saved = Saved::new();
        saved.insert("A".into(), Point { x: 12.0, y: 34.0 });
        saved.insert("web-ui".into(), Point { x: 56.0, y: 78.0 });
        assert_eq!(parse_saved(&render_saved(&saved)), saved);
    }

    #[test]
    fn layout_hong_thi_coi_nhu_chua_co() {
        assert!(parse_saved("{ rác").is_empty());
        assert!(parse_saved("").is_empty());
        assert!(parse_saved(r#"{"nodes": {"A": {"x": "không phải số"}}}"#).is_empty());
    }
}
