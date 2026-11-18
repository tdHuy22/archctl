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
/// Khe giữa hai cột: đủ chỗ cho vài làn đường nối chạy dọc mà không dính
/// vào nhau (xem [`route`]).
const GAP_X: f64 = 150.0;
/// Khe giữa hai hộp trong một cột: đủ cho vài làn đường nối luồn qua ở giữa.
/// Đồ thị dẹt (repo archctl xếp thành hai hàng) dồn mọi đường vào khe này.
const GAP_Y: f64 = 84.0;
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
        // Lối vào từ ngoài: một nút khi có `link Internet -> …`.
        .chain(
            doc.links
                .iter()
                .any(|l| l.from == crate::model::INTERNET)
                .then(|| format!("{SERVICE_PREFIX}{}", crate::model::INTERNET)),
        )
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
    // Chỉ cạnh giữa hai nút có thật: `link` tới service chưa khai là lỗi của
    // file luật, không được làm sơ đồ đổ.
    let node = |name: &str| format!("{SERVICE_PREFIX}{name}");
    let has = |n: &String| nodes.iter().any(|x| x == n);
    for l in &doc.links {
        for to in l.to.iter().filter(|t| **t != l.from) {
            let (a, b) = (node(&l.from), node(to));
            if has(&a) && has(&b) {
                edges.insert((a, b));
            }
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

// ---------- định tuyến đường nối ----------

/// Đường nối rời mép hộp một đoạn ngắn rồi mới được rẽ.
const STUB: f64 = 14.0;
/// Đường nối đi cách mép mọi hộp ít nhất chừng này.
const CLEAR: f64 = 10.0;
/// Khoảng cách giữa hai làn song song trong một khe.
const TRACK: f64 = 14.0;
/// Số làn tối đa trong một khe: hộp bị kéo ra thật xa không được biến khe
/// trống thành một lưới hàng nghìn làn.
const MAX_TRACKS: usize = 9;
/// Mỗi chỗ rẽ tính như chừng này điểm ảnh đường đi: đường ít gấp khúc dễ dõi.
const BEND_COST: i64 = 40;
/// Đi chung đoạn với một đường đã vẽ thì đắt thêm chừng này mỗi đoạn, nên
/// đường sau tìm làn trống bên cạnh thay vì nằm chồng lên.
const CROWD_COST: i64 = 90;
/// Trần số bước tìm cho một đường; quá thì để giao diện vẽ đường cong.
const MAX_STEPS: usize = 400_000;

/// Đường nối của một cặp hộp: dãy điểm, đoạn giữa hai điểm liền nhau luôn
/// nằm ngang hoặc dọc.
pub type Routes = BTreeMap<(String, String), Vec<Point>>;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Side {
    Left,
    Right,
}

/// Một cặp `(từ, tới)` mượn tên từ bảng toạ độ.
type Pair<'a> = (&'a str, &'a str);
/// Các đầu đường trên một mép hộp: (độ cao đầu bên kia, tên đầu bên kia,
/// cặp, đây có phải đầu nguồn).
type SideEnds<'a> = Vec<(f64, &'a str, Pair<'a>, bool)>;

/// Định tuyến vuông góc cho mọi cặp `(từ, tới)`.
///
/// Đường đi xuôi rời mép phải hộp nguồn và vào mép trái hộp đích; đường đi
/// ngược thì ngược lại; hai hộp cùng cột thì vòng qua khe bên phải. Mỗi đường
/// có điểm rời riêng trên mép hộp, xếp theo độ cao của đầu bên kia để các
/// đường ít cắt nhau. Đường không bao giờ cắt qua một hộp: lưới tìm đường chỉ
/// gồm những đoạn không chạm hộp nào.
///
/// Cặp vắng mặt trong kết quả là cặp không tìm được đường (hai hộp kéo tay đè
/// lên nhau, hay sơ đồ quá lớn); giao diện vẽ đường cong cho cặp đó. Cùng đầu
/// vào luôn cho cùng kết quả.
pub fn route(places: &BTreeMap<String, Point>, pairs: &[(String, String)]) -> Routes {
    // Toạ độ ngoài tầm (không qua `parse_saved`) thì không định tuyến: phép
    // cộng chi phí có thể tràn.
    let sane = |name: &str| {
        places.get(name).is_some_and(|p| p.x.abs() <= MAX_COORD && p.y.abs() <= MAX_COORD)
    };
    let valid: BTreeSet<(&str, &str)> = pairs
        .iter()
        .filter(|(a, b)| a != b && sane(a) && sane(b))
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    if valid.is_empty() {
        return Routes::new();
    }

    // Mép nào của mỗi đầu, rồi chia chỗ trên từng mép.
    let sides = |a: &str, b: &str| {
        let (pa, pb) = (places[a], places[b]);
        if pb.x >= pa.x + NODE_W {
            (Side::Right, Side::Left)
        } else if pb.x + NODE_W <= pa.x {
            (Side::Left, Side::Right)
        } else {
            (Side::Right, Side::Right)
        }
    };
    let mut on_side: BTreeMap<(&str, Side), SideEnds> = BTreeMap::new();
    for &(a, b) in &valid {
        let (sa, sb) = sides(a, b);
        on_side.entry((a, sa)).or_default().push((places[b].y, b, (a, b), true));
        on_side.entry((b, sb)).or_default().push((places[a].y, a, (a, b), false));
    }
    // (cổng, điểm rời mép) của đầu nguồn và đầu đích.
    let mut ends: BTreeMap<(Pair, bool), (Point, Point)> = BTreeMap::new();
    for ((node, side), list) in &mut on_side {
        list.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(y.1)).then(x.3.cmp(&y.3)));
        let p = places[*node];
        let (edge_x, out_x) = match side {
            Side::Right => (p.x + NODE_W, p.x + NODE_W + STUB),
            Side::Left => (p.x, p.x - STUB),
        };
        let k = list.len();
        for (i, (_, _, pair, is_source)) in list.iter().enumerate() {
            let y = p.y + NODE_H * (i + 1) as f64 / (k + 1) as f64;
            ends.insert((*pair, *is_source), (Point { x: edge_x, y }, Point { x: out_x, y }));
        }
    }

    let boxes: Vec<[f64; 4]> = places
        .values()
        .filter(|p| p.x.abs() <= MAX_COORD && p.y.abs() <= MAX_COORD)
        .map(|p| [p.x - CLEAR, p.y - CLEAR, p.x + NODE_W + CLEAR, p.y + NODE_H + CLEAR])
        .collect();
    let stubs: Vec<Point> = ends.values().map(|(_, s)| *s).collect();
    let grid = Grid::new(&boxes, &stubs);

    // Đường ngắn đi trước, để giữ được làn thẳng của nó.
    let mut order: Vec<(&str, &str)> = valid.into_iter().collect();
    let span = |(a, b): &(&str, &str)| {
        let (pa, pb) = (places[*a], places[*b]);
        ((pa.x - pb.x).abs() + (pa.y - pb.y).abs()) as i64
    };
    order.sort_by_key(|pair| (span(pair), *pair));

    let mut used = vec![0u16; grid.xs.len() * grid.ys.len() * 2];
    let mut scratch = Scratch::new(grid.xs.len() * grid.ys.len() * 2);
    let mut out = Routes::new();
    for pair in order {
        let (port_a, stub_a) = ends[&(pair, true)];
        let (port_b, stub_b) = ends[&(pair, false)];
        let Some(cells) = grid.search(stub_a, stub_b, &used, &mut scratch) else { continue };
        for w in cells.windows(2) {
            used[grid.segment(w[0], w[1])] += 1;
        }
        let mut points = vec![port_a];
        points.extend(cells.iter().map(|&c| grid.point(c)));
        points.push(port_b);
        out.insert((pair.0.to_string(), pair.1.to_string()), simplify(points));
    }
    out
}

/// Bỏ điểm nằm giữa hai đoạn thẳng hàng.
fn simplify(points: Vec<Point>) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(points.len());
    for p in points {
        if out.last() == Some(&p) {
            continue;
        }
        if out.len() >= 2 {
            let (a, b) = (out[out.len() - 2], out[out.len() - 1]);
            let same_row = (a.y - b.y).abs() < 1e-6 && (b.y - p.y).abs() < 1e-6;
            let same_column = (a.x - b.x).abs() < 1e-6 && (b.x - p.x).abs() < 1e-6;
            if same_row || same_column {
                out.pop();
            }
        }
        out.push(p);
    }
    out
}

/// Mảng chi phí của A*, dùng lại giữa các lần tìm: cấp phát và xoá cả lưới cho
/// mỗi đường làm sơ đồ lớn (hugo) chậm đi vài lần.
struct Scratch {
    cost: Vec<i64>,
    parent: Vec<usize>,
    touched: Vec<usize>,
}

impl Scratch {
    fn new(size: usize) -> Scratch {
        Scratch { cost: vec![i64::MAX; size], parent: vec![usize::MAX; size], touched: Vec::new() }
    }

    fn reset(&mut self) {
        for &s in &self.touched {
            self.cost[s] = i64::MAX;
            self.parent[s] = usize::MAX;
        }
        self.touched.clear();
    }
}

/// Lưới tìm đường: các đường dọc và ngang đi qua mép (đã nới) của mọi hộp,
/// qua điểm rời mép của mọi đường nối, cộng vài làn chia đều trong mỗi khe.
struct Grid {
    xs: Vec<f64>,
    ys: Vec<f64>,
    /// Đoạn ngang từ ô `(i, j)` sang `(i + 1, j)` chạm hộp.
    blocked_h: Vec<bool>,
    /// Đoạn dọc từ ô `(i, j)` sang `(i, j + 1)` chạm hộp.
    blocked_v: Vec<bool>,
    /// Đường dọc thứ `i` men theo mép hộp (hay điểm rời mép): đi dọc trên đó
    /// thì đường nối dính sát hộp, nên tính đắt gấp đôi.
    hug_x: Vec<bool>,
    /// Đường ngang thứ `j` men theo mép trên hay dưới của hộp.
    hug_y: Vec<bool>,
}

impl Grid {
    fn new(boxes: &[[f64; 4]], stubs: &[Point]) -> Grid {
        let lo_x = boxes.iter().map(|b| b[0]).fold(f64::INFINITY, f64::min) - 8.0 * TRACK;
        let hi_x = boxes.iter().map(|b| b[2]).fold(f64::NEG_INFINITY, f64::max) + 8.0 * TRACK;
        let lo_y = boxes.iter().map(|b| b[1]).fold(f64::INFINITY, f64::min) - 8.0 * TRACK;
        let hi_y = boxes.iter().map(|b| b[3]).fold(f64::NEG_INFINITY, f64::max) + 8.0 * TRACK;
        let xs = lines(
            boxes.iter().flat_map(|b| [b[0], b[2]]).chain(stubs.iter().map(|p| p.x)).chain([lo_x, hi_x]),
        );
        let ys = lines(
            boxes.iter().flat_map(|b| [b[1], b[3]]).chain(stubs.iter().map(|p| p.y)).chain([lo_y, hi_y]),
        );
        let (nx, ny) = (xs.len(), ys.len());
        let mut blocked_h = vec![false; nx * ny];
        let mut blocked_v = vec![false; nx * ny];
        let at = |v: &[f64], x: f64| v.partition_point(|p| *p < x - 1e-6);
        for b in boxes {
            let (i0, i1) = (at(&xs, b[0]), at(&xs, b[2]));
            let (j0, j1) = (at(&ys, b[1]), at(&ys, b[3]));
            // Đoạn ngang nằm hẳn trong hộp: hàng ở giữa mép trên và mép dưới.
            for j in (j0 + 1)..j1 {
                for i in i0..i1 {
                    blocked_h[j * nx + i] = true;
                }
            }
            for i in (i0 + 1)..i1 {
                for j in j0..j1 {
                    blocked_v[j * nx + i] = true;
                }
            }
        }
        let near = |v: f64, edges: &[f64]| edges.iter().any(|e| (v - e).abs() < 1e-6);
        let edge_x: Vec<f64> = boxes.iter().flat_map(|b| [b[0], b[2]]).chain(stubs.iter().map(|p| p.x)).collect();
        let edge_y: Vec<f64> = boxes.iter().flat_map(|b| [b[1], b[3]]).collect();
        let hug_x = xs.iter().map(|x| near(*x, &edge_x)).collect();
        let hug_y = ys.iter().map(|y| near(*y, &edge_y)).collect();
        Grid { xs, ys, blocked_h, blocked_v, hug_x, hug_y }
    }

    fn cell(&self, p: Point) -> usize {
        let i = self.xs.partition_point(|x| *x < p.x - 1e-6);
        let j = self.ys.partition_point(|y| *y < p.y - 1e-6);
        j * self.xs.len() + i
    }

    fn point(&self, cell: usize) -> Point {
        let nx = self.xs.len();
        Point { x: self.xs[cell % nx], y: self.ys[cell / nx] }
    }

    /// Chỉ số của đoạn giữa hai ô kề nhau, dùng để đếm số đường đi qua.
    fn segment(&self, a: usize, b: usize) -> usize {
        let low = a.min(b);
        let vertical = a.abs_diff(b) != 1;
        low * 2 + usize::from(vertical)
    }

    /// A* trên lưới, trạng thái gồm ô và hướng đang đi (để tính chỗ rẽ).
    fn search(&self, from: Point, to: Point, used: &[u16], scratch: &mut Scratch) -> Option<Vec<usize>> {
        let found = self.search_in(from, to, used, scratch);
        scratch.reset();
        found
    }

    fn search_in(&self, from: Point, to: Point, used: &[u16], scratch: &mut Scratch) -> Option<Vec<usize>> {
        use std::cmp::Reverse;
        use std::collections::BinaryHeap;

        let (nx, ny) = (self.xs.len(), self.ys.len());
        let (start, goal) = (self.cell(from), self.cell(to));
        let goal_point = self.point(goal);
        let guess = |cell: usize| {
            let p = self.point(cell);
            ((p.x - goal_point.x).abs() + (p.y - goal_point.y).abs()) as i64
        };
        // Trạng thái = ô * 2 + hướng (0: ngang, 1: dọc).
        let Scratch { cost, parent, touched } = scratch;
        let mut heap = BinaryHeap::new();
        let mut seq = 0u64;
        cost[start * 2] = 0;
        touched.push(start * 2);
        heap.push(Reverse((guess(start), seq, start * 2, 0)));

        let mut steps = 0;
        while let Some(Reverse((_, _, state, reached))) = heap.pop() {
            // Mục cũ: trạng thái này đã tới được với giá rẻ hơn.
            if reached > cost[state] {
                continue;
            }
            let (cell, dir) = (state / 2, state % 2);
            if cell == goal {
                let mut path = vec![cell];
                let mut s = state;
                // Chặn chu trình phòng xa: đường không bao giờ dài hơn số ô.
                while parent[s] != usize::MAX && path.len() <= nx * ny {
                    s = parent[s];
                    path.push(s / 2);
                }
                path.reverse();
                path.dedup();
                return Some(path);
            }
            steps += 1;
            if steps > MAX_STEPS {
                return None;
            }
            let here = cost[state];
            let (i, j) = (cell % nx, cell / nx);
            let mut moves = [None; 4];
            if i + 1 < nx && !self.blocked_h[cell] {
                moves[0] = Some((cell + 1, 0));
            }
            if i > 0 && !self.blocked_h[cell - 1] {
                moves[1] = Some((cell - 1, 0));
            }
            if j + 1 < ny && !self.blocked_v[cell] {
                moves[2] = Some((cell + nx, 1));
            }
            if j > 0 && !self.blocked_v[cell - nx] {
                moves[3] = Some((cell - nx, 1));
            }
            for (next, next_dir) in moves.into_iter().flatten() {
                let (a, b) = (self.point(cell), self.point(next));
                let length = ((a.x - b.x).abs() + (a.y - b.y).abs()).round() as i64;
                let mut step = length + CROWD_COST * i64::from(used[self.segment(cell, next)]);
                let hugging = if next_dir == 0 { self.hug_y[j] } else { self.hug_x[i] };
                if hugging {
                    step += length;
                }
                if next_dir != dir {
                    step += BEND_COST;
                }
                // Đoạn cuối vào cổng là đoạn ngang: tới đích theo chiều dọc
                // là thêm một chỗ rẽ.
                if next == goal && next_dir == 1 {
                    step += BEND_COST;
                }
                let total = here.saturating_add(step);
                let key = next * 2 + next_dir;
                if total < cost[key] {
                    if cost[key] == i64::MAX {
                        touched.push(key);
                    }
                    cost[key] = total;
                    parent[key] = state;
                    seq += 1;
                    heap.push(Reverse((total + guess(next), seq, key, total)));
                }
            }
        }
        None
    }
}

/// Các đường của lưới theo một chiều: giá trị gốc đã sắp và bỏ trùng, cộng
/// làn chia đều trong mỗi khe đủ rộng.
fn lines(values: impl Iterator<Item = f64>) -> Vec<f64> {
    let mut base: Vec<f64> = values.collect();
    base.sort_by(f64::total_cmp);
    base.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let mut out = Vec::with_capacity(base.len() * 3);
    for w in base.windows(2) {
        out.push(w[0]);
        let gap = w[1] - w[0];
        let count = ((gap / TRACK) as usize).saturating_sub(1).min(MAX_TRACKS);
        for t in 1..=count {
            out.push(w[0] + gap * t as f64 / (count + 1) as f64);
        }
    }
    out.extend(base.last());
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
            let x = coordinate(point.get("x")?.as_str()?)?;
            let y = coordinate(point.get("y")?.as_str()?)?;
            Some((name.clone(), Point { x, y }))
        })
        .collect()
}

/// Toạ độ xa nhất được nhận, theo mỗi chiều. Xa hơn nhiều bậc so với mọi sơ
/// đồ thật, nhưng đủ nhỏ để phép cộng chi phí đường đi không tràn.
pub const MAX_COORD: f64 = 1.0e6;

/// Đọc một toạ độ: phải là số hữu hạn, và bị kẹp trong `±MAX_COORD`. `inf`,
/// `NaN` hay `1e300` trong `layout.json` (agent ghi được file này) từng làm
/// bộ định tuyến tràn số rồi treo máy chủ.
pub fn coordinate(text: &str) -> Option<f64> {
    let v: f64 = text.trim().parse().ok()?;
    v.is_finite().then(|| v.clamp(-MAX_COORD, MAX_COORD))
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
    fn loi_vao_internet_co_nut_rieng() {
        let d = doc("service A\nlink Internet -> A");
        let p = place(&d, &Saved::new());
        assert!(p["svc:Internet"].x < p["svc:A"].x, "lối vào đứng trước service nó mở");
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

    // ---------- sơ đồ rành mạch: hộp thưa, đường không xuyên hộp ----------

    const SHOP: &str = include_str!("../../../examples/shop/.arch/system.arch");
    const OWN: &str = include_str!("../../../.arch/system.arch");

    /// Mọi cặp được vẽ thành đường: luật allow và deny, kho, service, link.
    fn drawn_pairs(d: &Document) -> Vec<(String, String)> {
        let nodes = node_names(d);
        let mut pairs = edge_list(d, &nodes);
        for rule in &d.rules {
            let Target::Named(from) = &rule.from else { continue };
            for t in &rule.to {
                if let Target::Named(to) = t {
                    pairs.push((from.clone(), to.clone()));
                }
            }
        }
        pairs.sort();
        pairs.dedup();
        pairs
    }

    /// Đoạn thẳng `p`–`q` (ngang hoặc dọc) có lọt vào lòng hộp đặt ở `b`.
    fn cuts(p: Point, q: Point, b: Point) -> bool {
        let (x0, x1) = (p.x.min(q.x), p.x.max(q.x));
        let (y0, y1) = (p.y.min(q.y), p.y.max(q.y));
        let inside_x = |x: f64| x > b.x && x < b.x + NODE_W;
        let inside_y = |y: f64| y > b.y && y < b.y + NODE_H;
        if y0 == y1 {
            inside_y(y0) && x0 < b.x + NODE_W && x1 > b.x
        } else {
            inside_x(x0) && y0 < b.y + NODE_H && y1 > b.y
        }
    }

    fn assert_clean(name: &str, places: &BTreeMap<String, Point>, pairs: &[(String, String)]) {
        let routes = route(places, pairs);
        let wanted = pairs.iter().filter(|(a, b)| a != b && places.contains_key(a) && places.contains_key(b));
        for (from, to) in wanted {
            let points = routes
                .get(&(from.clone(), to.clone()))
                .unwrap_or_else(|| panic!("{name}: không có đường {from} -> {to}"));
            for w in points.windows(2) {
                assert!(w[0].x == w[1].x || w[0].y == w[1].y, "{name}: {from} -> {to} có đoạn xiên {w:?}");
                for (other, at) in places {
                    assert!(!cuts(w[0], w[1], *at), "{name}: {from} -> {to} xuyên qua hộp {other} ở đoạn {w:?}");
                }
            }
        }
    }

    /// Tiêu chí (a): hai hộp bất kỳ cách nhau ít nhất 100 điểm ảnh theo chiều
    /// ngang, hay 40 điểm ảnh theo chiều dọc.
    #[test]
    fn sap_xep_lai_thi_hop_cach_rong_va_khong_chong() {
        for (name, src) in [("shop", SHOP), ("archctl", OWN)] {
            let p = place(&doc(src), &Saved::new());
            let all: Vec<_> = p.iter().collect();
            for (i, (a, pa)) in all.iter().enumerate() {
                for (b, pb) in &all[i + 1..] {
                    let apart_x = (pa.x - pb.x).abs() >= NODE_W + 100.0;
                    let apart_y = (pa.y - pb.y).abs() >= NODE_H + 40.0;
                    assert!(apart_x || apart_y, "{name}: {a} {pa:?} và {b} {pb:?} sát nhau");
                }
            }
        }
    }

    /// Tiêu chí (b) trên ba sơ đồ thật; hugo là đồ thị dày nhất (24 component,
    /// 155 quan hệ, 58 khép thành vòng).
    #[test]
    fn duong_noi_khong_xuyen_qua_hop_nao() {
        let hugo = include_str!("../tests/fixtures/hugo.arch");
        for (name, src) in [("shop", SHOP), ("archctl", OWN), ("hugo", hugo)] {
            let d = doc(src);
            assert_clean(name, &place(&d, &Saved::new()), &drawn_pairs(&d));
        }
    }

    /// Ca dễ hỏng nhất: cạnh dài A -> C mà B nằm ngay giữa, cùng hàng.
    #[test]
    fn canh_dai_vong_qua_hop_nam_giua() {
        let d = doc(
            "component A at a/**\ncomponent B at b/**\ncomponent C at c/**\n\
             allow A -> B\nallow B -> C\nallow A -> C\nallow C -> A",
        );
        let p = place(&d, &Saved::new());
        assert_eq!(p["A"].y, p["B"].y, "dựng đúng ca: ba hộp cùng hàng {p:?}");
        assert_eq!(p["B"].y, p["C"].y, "dựng đúng ca: ba hộp cùng hàng {p:?}");
        assert_clean("chuỗi", &p, &drawn_pairs(&d));
    }

    /// Tiêu chí (c): các đường ra từ một hộp rời ở những điểm khác nhau, các
    /// đường vào một hộp cũng vậy.
    #[test]
    fn duong_ra_tu_mot_hop_roi_o_diem_khac_nhau() {
        for (name, src) in [("shop", SHOP), ("archctl", OWN)] {
            let d = doc(src);
            let routes = route(&place(&d, &Saved::new()), &drawn_pairs(&d));
            assert!(!routes.is_empty());
            let mut seen = BTreeSet::new();
            for ((from, to), points) in &routes {
                let (first, last) = (points[0], points[points.len() - 1]);
                let a = (from.clone(), first.x.to_bits(), first.y.to_bits());
                let b = (to.clone(), last.x.to_bits(), last.y.to_bits());
                assert!(seen.insert(a), "{name}: {from} có hai đường rời cùng một điểm");
                assert!(seen.insert(b), "{name}: {to} có hai đường vào cùng một điểm");
            }
        }
    }

    /// Tiêu chí (d): hộp kéo tay giữ chỗ, kể cả khi bị thả chắn ngang đường
    /// của hộp khác — đường đó phải đi vòng.
    #[test]
    fn hop_keo_tay_giu_cho_va_duong_di_vong() {
        let d = doc(SHOP);
        let auto = place(&d, &Saved::new());
        let (web, order) = (auto["Web"], auto["Order"]);
        // Thả Stripe vào giữa Web và Order, cùng hàng với Web.
        let middle = Point { x: (web.x + order.x) / 2.0, y: web.y };
        let mut saved = Saved::new();
        saved.insert("Stripe".into(), middle);
        let p = place(&d, &saved);
        assert_eq!(p["Stripe"], middle);
        for (name, at) in &auto {
            if name != "Stripe" {
                assert_eq!(p[name], *at, "{name} bị xê dịch theo");
            }
        }
        assert_clean("kéo tay", &p, &drawn_pairs(&d));
    }

    /// Tiêu chí (d): cùng đầu vào luôn ra cùng sơ đồ, cả vị trí lẫn đường.
    #[test]
    fn cung_dau_vao_ra_cung_so_do() {
        let d = doc(OWN);
        let p = place(&d, &Saved::new());
        let r = route(&p, &drawn_pairs(&d));
        for _ in 0..3 {
            let d2 = doc(OWN);
            let p2 = place(&d2, &Saved::new());
            assert_eq!(p2, p);
            assert_eq!(route(&p2, &drawn_pairs(&d2)), r);
        }
    }

    /// Review 18/11/2026: `1e300` hay `inf` trong `layout.json` làm chi phí
    /// tràn số, cây cha thành vòng, máy chủ treo và ăn dần bộ nhớ.
    #[test]
    fn toa_do_cuc_lon_khong_lam_treo() {
        let saved = parse_saved(
            r#"{"nodes": {"A": {"x": "1e300", "y": "-1e300"}, "B": {"x": "inf", "y": "0"},
                          "C": {"x": "NaN", "y": "0"}, "D": {"x": "5", "y": "-inf"}}}"#,
        );
        assert_eq!(saved["A"], Point { x: MAX_COORD, y: -MAX_COORD });
        assert!(!saved.contains_key("B") && !saved.contains_key("C") && !saved.contains_key("D"), "{saved:?}");

        let d = doc("component A at a/**\ncomponent B at b/**\ncomponent C at c/**\nallow A -> B\nallow C -> A");
        let p = place(&d, &saved);
        assert_clean("toạ độ lớn", &p, &drawn_pairs(&d));
        // Toạ độ đưa thẳng vào, không qua `parse_saved`: bỏ qua, không treo.
        let mut wild = p.clone();
        wild.insert("B".into(), Point { x: 1e300, y: f64::INFINITY });
        let r = route(&wild, &drawn_pairs(&d));
        assert!(!r.contains_key(&("A".to_string(), "B".to_string())));
        assert!(r.contains_key(&("C".to_string(), "A".to_string())));
    }

    #[test]
    fn hai_hop_de_len_nhau_thi_khong_ve_duong_thay_vi_xuyen() {
        let mut p = BTreeMap::new();
        p.insert("A".to_string(), Point { x: 0.0, y: 0.0 });
        p.insert("B".to_string(), Point { x: 10.0, y: 5.0 });
        p.insert("C".to_string(), Point { x: 600.0, y: 0.0 });
        let r = route(&p, &[("A".into(), "C".into())]);
        assert!(r.is_empty(), "cổng của A nằm trong lòng B: {r:?}");
    }
}
