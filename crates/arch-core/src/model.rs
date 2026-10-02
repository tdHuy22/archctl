//! Mô hình kiến trúc — kết quả của việc đọc `system.arch`.

/// Ai tạo ra phần tử. Ghi lại nguồn gốc, không quyết định quyền hạn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    Human,
    Ai,
}

/// Trạng thái quyết định luật có hiệu lực hay chưa.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Status {
    Draft,
    Proposed,
    Accepted,
    Locked,
}

impl Status {
    /// Bộ kiểm chứng chỉ áp dụng từ `#accepted` trở lên.
    pub fn is_enforced(self) -> bool {
        matches!(self, Status::Accepted | Status::Locked)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Status::Draft => "#draft",
            Status::Proposed => "#proposed",
            Status::Accepted => "#accepted",
            Status::Locked => "#locked",
        }
    }
}

/// Chú thích chung của mọi phần tử, cộng vị trí trong file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub owner: Owner,
    pub status: Status,
    pub line: usize,
}

impl Default for Meta {
    /// Thiếu chú thích thì mặc định là ý định của người và có hiệu lực ngay.
    /// Mặc định theo hướng chặt chẽ hơn là lỏng lẻo.
    fn default() -> Self {
        Meta { owner: Owner::Human, status: Status::Accepted, line: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component {
    pub name: String,
    /// Glob ánh xạ sang thư mục thật. Rỗng nghĩa là bộ kiểm chứng bỏ qua.
    pub paths: Vec<String>,
    /// `external` = ngoài tầm kiểm soát, không soi bên trong.
    pub external: bool,
    pub meta: Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Store {
    pub name: String,
    /// Chỉ component này được truy cập trực tiếp.
    pub owned_by: String,
    pub tables: Vec<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Any,
    Named(String),
}

impl Target {
    pub fn as_str(&self) -> &str {
        match self {
            Target::Any => "*",
            Target::Named(n) => n,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleKind {
    Allow,
    Deny,
}

/// Quan hệ phụ thuộc. Mặc định của hệ thống là cấm: không có `allow` thì
/// không được gọi. `deny` dùng để nhấn mạnh và cho thông báo lỗi rõ hơn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub kind: RuleKind,
    pub from: Target,
    pub to: Vec<Target>,
    pub except: Vec<String>,
    pub meta: Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowLine {
    Sequence(Vec<String>),
    Parallel(Vec<String>),
    Cond { cond: String, steps: Vec<String> },
    OnFail { step: String, steps: Vec<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flow {
    pub name: String,
    pub lines: Vec<FlowLine>,
    pub meta: Meta,
}

/// Một mục trong thân `contract` hoặc `boundary`. Giai đoạn này chỉ giữ dạng
/// khoá và các dòng giá trị; phần hiểu ngữ nghĩa để sau.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub key: String,
    pub values: Vec<String>,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub component: String,
    pub capability: String,
    pub fields: Vec<Field>,
    pub meta: Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Boundary {
    pub name: String,
    pub contains: Vec<String>,
    pub fields: Vec<Field>,
    pub meta: Meta,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invariant {
    /// data | state | money | time | access | algo
    pub kind: String,
    /// Phạm vi trong ngoặc vuông: tên component, tên boundary, hoặc `system`.
    pub scope: Option<String>,
    pub expr: String,
    pub verify: Vec<String>,
    pub meta: Meta,
}

/// Nhóm hàm hoặc từ khoá bị theo dõi: `strcpy`, `eval`, `unsafe`.
///
/// Khác với `component` và `store`, nhóm này không ánh xạ sang file nào. Nó
/// được phát hiện ngay trong dòng token, nên **không có vùng mù** như đồ thị
/// import: FFI, con trỏ hàm hay liên kết thư viện đều không giấu được một lời
/// gọi `strcpy` nằm trong mã nguồn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Api {
    pub name: String,
    /// Tên hàm hoặc từ khoá cần tìm.
    pub symbols: Vec<String>,
    pub meta: Meta,
}

/// Lát cắt để app hiển thị riêng từng phần. Thuần trình bày, không phải luật.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub name: String,
    pub components: Vec<String>,
    pub meta: Meta,
}

/// Một phần tử bất kỳ của mô hình, nhìn dưới dạng chung để so sánh được.
///
/// `key` là **danh tính**: hai phần tử cùng `key` ở hai phiên bản là cùng một
/// thứ. `body` là **nội dung ngữ nghĩa**, không gồm chú thích quản trị và số
/// dòng. Tách hai thứ này ra là điều kiện để phân biệt được "kiến trúc đổi"
/// với "chỉ xê dịch trong file".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub kind: &'static str,
    pub key: String,
    pub body: String,
    pub owner: Owner,
    pub status: Status,
    pub line: usize,
}

impl Element {
    fn new(kind: &'static str, key: String, body: String, meta: &Meta) -> Self {
        Element { kind, key, body, owner: meta.owner, status: meta.status, line: meta.line }
    }

    /// Phần tử có tên riêng thì ghép được giữa hai phiên bản để báo "đã đổi".
    /// Luật và invariant không có tên, nên thay đổi của chúng hiện thành một
    /// cặp thêm/bớt — rõ ràng hơn là đoán xem luật nào tương ứng luật nào.
    pub fn is_named(&self) -> bool {
        !matches!(self.kind, "rule" | "invariant")
    }
}

fn list(items: &[String]) -> String {
    let mut sorted = items.to_vec();
    sorted.sort();
    sorted.join(", ")
}

/// Điều mà một khai báo kiến trúc nói về mã nguồn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Tên này phải xuất hiện trong mã. `within` rỗng nghĩa là ở đâu cũng được.
    Exists { within: Vec<String> },
    /// Tên này **không được** xuất hiện ngoài các component đã nêu.
    Confined { to: Vec<String> },
}

/// Một cái tên mà `flow`, `contract`, `boundary` hay `invariant` nhắc tới, kèm
/// điều kiện đi với nó.
///
/// Đây là phần kiểm được của bốn khai báo đó. Phần ngữ nghĩa — thứ tự thực
/// thi, `requires`/`ensures`, bất biến về dữ liệu — cần trace runtime, contract
/// test hoặc property test, và vẫn chưa kiểm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expectation {
    pub name: String,
    /// Khai báo nào sinh ra kỳ vọng này, để thông báo nói rõ nguồn gốc.
    pub source: String,
    pub expect: Expect,
    pub line: usize,
    pub status: Status,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Document {
    pub components: Vec<Component>,
    pub stores: Vec<Store>,
    pub rules: Vec<Rule>,
    pub flows: Vec<Flow>,
    pub contracts: Vec<Contract>,
    pub boundaries: Vec<Boundary>,
    pub invariants: Vec<Invariant>,
    pub views: Vec<View>,
    pub apis: Vec<Api>,
}

impl Document {
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name == name)
    }

    pub fn store(&self, name: &str) -> Option<&Store> {
        self.stores.iter().find(|s| s.name == name)
    }

    pub fn api(&self, name: &str) -> Option<&Api> {
        self.apis.iter().find(|a| a.name == name)
    }

    /// Mọi tên bảng cần tìm trong mã, kèm kho dữ liệu chứa nó.
    pub fn watched_tables(&self) -> Vec<(&str, &str)> {
        self.stores
            .iter()
            .flat_map(|s| s.tables.iter().map(move |t| (s.name.as_str(), t.as_str())))
            .collect()
    }

    /// Mọi ký hiệu cần tìm trong mã, kèm nhóm chứa nó.
    pub fn watched_symbols(&self) -> Vec<(&str, &str)> {
        self.apis
            .iter()
            .flat_map(|a| a.symbols.iter().map(move |s| (a.name.as_str(), s.as_str())))
            .collect()
    }

    pub fn boundary(&self, name: &str) -> Option<&Boundary> {
        self.boundaries.iter().find(|b| b.name == name)
    }

    /// `allow`/`deny` trỏ tới component, store hoặc nhóm api.
    pub fn knows(&self, name: &str) -> bool {
        self.component(name).is_some() || self.store(name).is_some() || self.api(name).is_some()
    }

    /// Số luật đang có hiệu lực. Dùng cho báo cáo độ phủ.
    pub fn enforced_count(&self) -> usize {
        self.rules.iter().filter(|r| r.meta.status.is_enforced()).count()
            + self.stores.iter().filter(|s| s.meta.status.is_enforced()).count()
    }

    /// Mọi tên mà khai báo kiến trúc nói là phải có trong mã.
    ///
    /// Chỉ rút từ những trường **có cấu trúc**: bước của `flow`, năng lực và
    /// `errors` của `contract`, `contain` và `compensate` của `boundary`, các
    /// trạng thái của `invariant state`. Những trường còn lại là văn xuôi —
    /// moi tên ra từ đó chỉ sinh kỳ vọng rác.
    /// Bước của flow → `(tên cần tìm trong mã, component phải chứa nó)`.
    ///
    /// Một bước có thể là ba thứ khác nhau, và chỉ một trong ba nói gì về mã:
    ///
    /// - tên component: người thực hiện, như `Web` trong `Web -> charge`.
    ///   Không có ký hiệu nào tên `Web` trong mã, nên đòi phải có là báo nhầm.
    /// - `Component.năng lực`: dò `năng lực`, và dò đúng trong component đó.
    ///   Dò cả chuỗi `Component.năng lực` thì không bao giờ khớp, vì bộ quét
    ///   làm việc trên từng định danh một.
    /// - tên hàm trần: dò ở bất cứ đâu.
    fn step_expectation(&self, step: &str) -> Option<(String, Vec<String>)> {
        if self.knows(step) {
            return None;
        }
        match step.rsplit_once('.') {
            // `order.charge` là biến chứ không phải component, nên chỉ dò tên
            // sau dấu chấm mà không ràng buộc nơi chứa.
            Some((head, tail)) if is_identifier(tail) => {
                let within = self.component(head).map(|c| vec![c.name.clone()]).unwrap_or_default();
                Some((tail.to_string(), within))
            }
            _ => is_identifier(step).then(|| (step.to_string(), Vec::new())),
        }
    }

    pub fn expectations(&self) -> Vec<Expectation> {
        let mut out = Vec::new();

        for flow in &self.flows {
            for line in &flow.lines {
                for step in flow_steps(line) {
                    let Some((name, within)) = self.step_expectation(&step) else { continue };
                    out.push(Expectation {
                        name,
                        source: format!("bước của flow {}", flow.name),
                        expect: Expect::Exists { within },
                        line: flow.meta.line,
                        status: flow.meta.status,
                    });
                }
            }
        }

        for contract in &self.contracts {
            let within = vec![contract.component.clone()];
            let label = format!("{}.{}", contract.component, contract.capability);
            out.push(Expectation {
                name: contract.capability.clone(),
                source: format!("năng lực trong contract {label}"),
                expect: Expect::Exists { within: within.clone() },
                line: contract.meta.line,
                status: contract.meta.status,
            });
            for field in &contract.fields {
                if field.key != "errors" {
                    continue;
                }
                for name in split_names(&field.values.join(" "), &['|', ',']) {
                    out.push(Expectation {
                        name,
                        source: format!("loại lỗi trong contract {label}"),
                        expect: Expect::Exists { within: within.clone() },
                        line: field.line,
                        status: contract.meta.status,
                    });
                }
            }
        }

        for boundary in &self.boundaries {
            for field in &boundary.fields {
                let text = field.values.join(" ");
                match field.key.as_str() {
                    // Lỗi hạ tầng không được lọt ra ngoài vùng.
                    "contain" => {
                        for name in split_names(&text, &[',']) {
                            out.push(Expectation {
                                name,
                                source: format!("`contain` của boundary {}", boundary.name),
                                expect: Expect::Confined { to: boundary.contains.clone() },
                                line: field.line,
                                status: boundary.meta.status,
                            });
                        }
                    }
                    // `compensate charge.fail => releaseStock`
                    "compensate" => {
                        for name in compensate_names(&text) {
                            out.push(Expectation {
                                name,
                                source: format!("`compensate` của boundary {}", boundary.name),
                                expect: Expect::Exists { within: boundary.contains.clone() },
                                line: field.line,
                                status: boundary.meta.status,
                            });
                        }
                    }
                    _ => {}
                }
            }
        }

        for inv in &self.invariants {
            if inv.kind != "state" {
                continue;
            }
            let within = inv
                .scope
                .as_deref()
                .filter(|s| *s != "system")
                .map(|s| vec![s.to_string()])
                .unwrap_or_default();
            for name in split_names(&inv.expr.replace("->", ","), &[',']) {
                out.push(Expectation {
                    name,
                    source: "trạng thái trong invariant state".into(),
                    expect: Expect::Exists { within: within.clone() },
                    line: inv.meta.line,
                    status: inv.meta.status,
                });
            }
        }

        out.sort_by(|a, b| (&a.name, &a.source).cmp(&(&b.name, &b.source)));
        out.dedup_by(|a, b| a.name == b.name && a.source == b.source);
        out
    }

    /// Mọi phần tử dưới dạng chung, để so sánh hai phiên bản và để băm.
    pub fn elements(&self) -> Vec<Element> {
        let mut out = Vec::new();

        for c in &self.components {
            let kind = if c.external { "external" } else { "component" };
            out.push(Element::new(kind, c.name.clone(), format!("at {}", list(&c.paths)), &c.meta));
        }
        for s in &self.stores {
            out.push(Element::new(
                "store",
                s.name.clone(),
                format!("owned_by {} at {}", s.owned_by, list(&s.tables)),
                &s.meta,
            ));
        }
        for r in &self.rules {
            let text = r.one_line();
            out.push(Element::new("rule", text.clone(), text, &r.meta));
        }
        for f in &self.flows {
            let body = f.lines.iter().map(flow_line_text).collect::<Vec<_>>().join("; ");
            out.push(Element::new("flow", f.name.clone(), body, &f.meta));
        }
        for c in &self.contracts {
            out.push(Element::new(
                "contract",
                format!("{}.{}", c.component, c.capability),
                fields_text(&c.fields),
                &c.meta,
            ));
        }
        for b in &self.boundaries {
            out.push(Element::new(
                "boundary",
                b.name.clone(),
                format!("contains {} · {}", list(&b.contains), fields_text(&b.fields)),
                &b.meta,
            ));
        }
        for i in &self.invariants {
            let scope = i.scope.as_deref().unwrap_or("-");
            let key = format!("invariant {} [{}] {}", i.kind, scope, i.expr);
            let body = format!("{key} verify {}", list(&i.verify));
            out.push(Element::new("invariant", key, body, &i.meta));
        }
        for v in &self.views {
            out.push(Element::new("view", v.name.clone(), list(&v.components), &v.meta));
        }
        for a in &self.apis {
            out.push(Element::new("api", a.name.clone(), list(&a.symbols), &a.meta));
        }

        out.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
        out
    }

    /// Dạng chuẩn hoá của toàn bộ luật: đã sắp xếp, không có chú thích, không
    /// có số dòng. Đây là thứ được băm — đổi thứ tự khai báo hay thêm một dòng
    /// ghi chú thì hash phải giữ nguyên, nếu không agent sẽ bị báo "kiến trúc
    /// đã đổi" liên tục rồi học cách phớt lờ.
    pub fn canonical(&self) -> String {
        self.elements()
            .iter()
            .map(|e| {
                let owner = if e.owner == Owner::Ai { "@ai" } else { "@human" };
                format!("{}|{}|{}|{owner}|{}", e.kind, e.key, e.body, e.status.as_str())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl Rule {
    /// Luật viết lại thành một dòng, dùng làm cả danh tính lẫn thông báo.
    pub fn one_line(&self) -> String {
        let kind = if self.kind == RuleKind::Allow { "allow" } else { "deny" };
        let targets: Vec<&str> = self.to.iter().map(Target::as_str).collect();
        let mut out = format!("{kind} {} -> {}", self.from.as_str(), targets.join(", "));
        if !self.except.is_empty() {
            out.push_str(&format!(" except {}", list(&self.except)));
        }
        out
    }
}

pub(crate) fn flow_steps(line: &FlowLine) -> Vec<String> {
    match line {
        FlowLine::Sequence(steps) | FlowLine::Parallel(steps) => steps.clone(),
        FlowLine::Cond { steps, .. } => steps.clone(),
        FlowLine::OnFail { step, steps } => {
            let mut all = vec![step.clone()];
            all.extend(steps.clone());
            all
        }
    }
}

/// Tách danh sách tên, chỉ giữ những gì trông như định danh. Văn xuôi lẫn vào
/// thì bị loại, nên một trường viết tự do không sinh ra kỳ vọng rác.
fn split_names(text: &str, separators: &[char]) -> Vec<String> {
    text.split(|c| separators.contains(&c))
        .map(str::trim)
        .filter(|s| is_identifier(s))
        .map(str::to_string)
        .collect()
}

fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_alphanumeric() || c == '_')
}

/// `charge.fail => releaseStock` → `charge`, `releaseStock`.
fn compensate_names(text: &str) -> Vec<String> {
    text.split("=>")
        .flat_map(|side| side.split(','))
        .map(|side| side.trim().split('.').next().unwrap_or("").trim())
        .filter(|s| is_identifier(s))
        .map(str::to_string)
        .collect()
}

fn flow_line_text(line: &FlowLine) -> String {
    match line {
        FlowLine::Sequence(steps) => steps.join(" -> "),
        FlowLine::Parallel(steps) => format!("parallel: {}", steps.join(", ")),
        FlowLine::Cond { cond, steps } => format!("if {cond}: {}", steps.join(", ")),
        FlowLine::OnFail { step, steps } => format!("on_fail({step}): {}", steps.join(", ")),
    }
}

fn fields_text(fields: &[Field]) -> String {
    fields.iter().map(|f| format!("{} {}", f.key, f.values.join(" / "))).collect::<Vec<_>>().join("; ")
}
