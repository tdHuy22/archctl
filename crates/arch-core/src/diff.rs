//! So sánh hai phiên bản của file luật.
//!
//! Điểm chính không phải là liệt kê thêm/bớt, mà là **phân loại thay đổi theo
//! bản chất**. Nút cổ chai của cả hệ thống là sự chú ý của con người: người
//! duyệt phải nhận ra ngay một thay đổi chỉ xê dịch chữ trong file, để dành
//! sức cho thay đổi thật sự động tới kiến trúc.
//!
//! Ý tưởng phân loại này lấy từ `delta/architecture-delta.mjs` của
//! tt-a1i/archify (MIT).

use crate::model::{Document, Element, Owner, Status};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Thành phần, kho dữ liệu hoặc quan hệ phụ thuộc thay đổi.
    Topology,
    /// Luồng, hợp đồng, ranh giới lỗi hoặc invariant thay đổi.
    Semantic,
    /// Chỉ chủ sở hữu hoặc trạng thái đổi — ai duyệt, luật có hiệu lực chưa.
    Governance,
}

impl Class {
    pub fn label(self) -> &'static str {
        match self {
            Class::Topology => "cấu trúc",
            Class::Semantic => "ngữ nghĩa",
            Class::Governance => "quản trị",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Added,
    Removed,
    Changed,
}

impl Kind {
    pub fn marker(self) -> char {
        match self {
            Kind::Added => '+',
            Kind::Removed => '-',
            Kind::Changed => '~',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub kind: Kind,
    pub class: Class,
    pub element_kind: &'static str,
    pub key: String,
    pub before: Option<String>,
    pub after: Option<String>,
    /// Trạng thái bị hạ xuống mức lỏng hơn. Nâng lên là việc bình thường; hạ
    /// xuống là nới lỏng luật và là thứ người duyệt không được bỏ sót.
    pub loosening: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Delta {
    pub changes: Vec<Change>,
    /// Phần tử giống hệt nhau, chỉ đổi chỗ trong file. Đếm chứ không liệt kê.
    pub moved: usize,
    pub unchanged: usize,
}

impl Delta {
    /// Thay đổi thật sự động tới kiến trúc, không tính quản trị.
    pub fn architectural(&self) -> usize {
        self.changes.iter().filter(|c| c.class != Class::Governance).count()
    }

    pub fn count(&self, class: Class) -> usize {
        self.changes.iter().filter(|c| c.class == class).count()
    }

    pub fn is_quiet(&self) -> bool {
        self.changes.is_empty()
    }

    /// Luật bị hạ xuống mức lỏng hơn. Đáng chú ý riêng vì nó mở ra những thứ
    /// trước đó bị cấm.
    pub fn loosenings(&self) -> impl Iterator<Item = &Change> {
        self.changes.iter().filter(|c| c.loosening)
    }
}

/// Thêm hoặc bớt một phần tử loại này là thay đổi cấu trúc; loại khác là thay
/// đổi ngữ nghĩa.
fn class_of(element_kind: &str) -> Class {
    match element_kind {
        "component" | "external" | "store" | "rule" => Class::Topology,
        _ => Class::Semantic,
    }
}

pub fn diff(base: &Document, head: &Document) -> Delta {
    let index = |doc: &Document| -> BTreeMap<(&'static str, String), Element> {
        doc.elements().into_iter().map(|e| ((e.kind, e.key.clone()), e)).collect()
    };
    let before = index(base);
    let after = index(head);
    let mut delta = Delta::default();

    for (id, old) in &before {
        let Some(new) = after.get(id) else {
            delta.changes.push(Change {
                kind: Kind::Removed,
                class: class_of(old.kind),
                element_kind: old.kind,
                key: old.key.clone(),
                before: Some(old.body.clone()),
                after: None,
                loosening: false,
            });
            continue;
        };

        let body_changed = old.body != new.body;
        let meta_changed = old.status != new.status || old.owner != new.owner;

        if body_changed {
            delta.changes.push(Change {
                kind: Kind::Changed,
                class: class_of(old.kind),
                element_kind: old.kind,
                key: old.key.clone(),
                before: Some(old.body.clone()),
                after: Some(new.body.clone()),
                loosening: false,
            });
        }
        // Một phần tử đổi cả nội dung lẫn trạng thái thì cả hai đều phải hiện
        // ra. Khoá một luật là hành vi quản trị riêng, không được để thay đổi
        // nội dung che mất.
        if meta_changed {
            delta.changes.push(Change {
                kind: Kind::Changed,
                class: Class::Governance,
                element_kind: old.kind,
                key: old.key.clone(),
                before: Some(meta_text(old)),
                after: Some(meta_text(new)),
                loosening: is_loosening(old.status, new.status),
            });
        }
        if !body_changed && !meta_changed {
            if old.line != new.line {
                delta.moved += 1;
            } else {
                delta.unchanged += 1;
            }
        }
    }

    for (id, new) in &after {
        if before.contains_key(id) {
            continue;
        }
        delta.changes.push(Change {
            kind: Kind::Added,
            class: class_of(new.kind),
            element_kind: new.kind,
            key: new.key.clone(),
            before: None,
            after: Some(new.body.clone()),
            loosening: false,
        });
    }

    delta.changes.sort_by(|a, b| {
        (a.element_kind, &a.key, a.kind.marker()).cmp(&(b.element_kind, &b.key, b.kind.marker()))
    });
    delta
}

fn meta_text(e: &Element) -> String {
    let owner = if e.owner == Owner::Ai { "@ai" } else { "@human" };
    format!("{owner} {}", e.status.as_str())
}

/// Nâng trạng thái lên mức chặt hơn là việc bình thường; hạ xuống là nới lỏng
/// luật và cần người để mắt.
pub fn is_loosening(before: Status, after: Status) -> bool {
    after < before
}
