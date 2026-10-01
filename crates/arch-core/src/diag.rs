//! Thông báo lỗi và cảnh báo. Mọi chẩn đoán đều mang số dòng, vì agent cần
//! biết sửa ở đâu và người cần biết luật nào đang chỉ là lời hứa.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Sai cú pháp hoặc tham chiếu không tồn tại. Mã thoát 2.
    Error,
    /// Luật không kiểm được, hoặc component không ánh xạ sang thư mục nào.
    Warning,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "lỗi",
            Severity::Warning => "cảnh báo",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diag {
    pub severity: Severity,
    pub line: usize,
    pub msg: String,
    /// Hướng sửa. Thiếu trường này thì agent hay chọn cách sửa sai hướng.
    pub hint: Option<String>,
}

impl Diag {
    pub fn error(line: usize, msg: impl Into<String>) -> Self {
        Diag { severity: Severity::Error, line, msg: msg.into(), hint: None }
    }

    pub fn warning(line: usize, msg: impl Into<String>) -> Self {
        Diag { severity: Severity::Warning, line, msg: msg.into(), hint: None }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.severity.label(), self.line, self.msg)?;
        if let Some(h) = &self.hint {
            write!(f, "\n  → {h}")?;
        }
        Ok(())
    }
}

pub fn has_errors(diags: &[Diag]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}

pub fn count(diags: &[Diag], severity: Severity) -> usize {
    diags.iter().filter(|d| d.severity == severity).count()
}
