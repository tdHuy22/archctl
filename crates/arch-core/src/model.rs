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

/// Lát cắt để app hiển thị riêng từng phần. Thuần trình bày, không phải luật.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub name: String,
    pub components: Vec<String>,
    pub meta: Meta,
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
}

impl Document {
    pub fn component(&self, name: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.name == name)
    }

    pub fn store(&self, name: &str) -> Option<&Store> {
        self.stores.iter().find(|s| s.name == name)
    }

    pub fn boundary(&self, name: &str) -> Option<&Boundary> {
        self.boundaries.iter().find(|b| b.name == name)
    }

    /// `allow`/`deny` trỏ tới component hoặc store, nên cả hai đều hợp lệ.
    pub fn knows(&self, name: &str) -> bool {
        self.component(name).is_some() || self.store(name).is_some()
    }

    /// Số luật đang có hiệu lực. Dùng cho báo cáo độ phủ.
    pub fn enforced_count(&self) -> usize {
        self.rules.iter().filter(|r| r.meta.status.is_enforced()).count()
            + self.stores.iter().filter(|s| s.meta.status.is_enforced()).count()
    }
}
