//! CLI của archctl.
//!
//! Tự phân tích tham số thay vì dùng thư viện, để giữ cây dependency bằng
//! không. Công cụ này chạy trong repo của người dùng và trong vòng lặp của
//! agent, nên mỗi dependency thêm vào là một bề mặt tấn công thêm.

use arch_core::diag::{self, Severity};
use arch_core::model::Status;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Mã thoát, theo mục 7 của đặc tả.
mod exit {
    pub const OK: u8 = 0;
    pub const DSL_INVALID: u8 = 2;
    pub const USAGE: u8 = 64;
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = args.first().map(String::as_str).unwrap_or("help");

    match command {
        "parse" => cmd_parse(&args[1..]),
        "help" | "-h" | "--help" => {
            print_usage();
            ExitCode::from(exit::OK)
        }
        "version" | "-V" | "--version" => {
            println!("archctl {VERSION}");
            ExitCode::from(exit::OK)
        }
        other => {
            eprintln!("archctl: lệnh không nhận ra: `{other}`\n");
            print_usage();
            ExitCode::from(exit::USAGE)
        }
    }
}

fn print_usage() {
    println!(
        "archctl {VERSION} — sơ đồ kiến trúc là luật

Cách dùng:
  archctl parse <file.arch>   Đọc và kiểm tra file luật
  archctl help                In hướng dẫn này
  archctl version             In phiên bản

Mã thoát:
  0   không có lỗi
  2   DSL sai cú pháp hoặc tham chiếu không tồn tại
  64  dùng lệnh sai"
    );
}

fn cmd_parse(args: &[String]) -> ExitCode {
    let Some(path) = args.first() else {
        eprintln!("archctl parse: thiếu đường dẫn file");
        return ExitCode::from(exit::USAGE);
    };

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("archctl: không đọc được `{path}`: {e}");
            return ExitCode::from(exit::USAGE);
        }
    };

    let (doc, diags) = arch_core::load(&src);

    println!("{path}\n");
    println!(
        "{} component · {} store · {} quan hệ · {} flow · {} contract · {} boundary · {} invariant",
        doc.components.len(),
        doc.stores.len(),
        doc.rules.len(),
        doc.flows.len(),
        doc.contracts.len(),
        doc.boundaries.len(),
        doc.invariants.len(),
    );

    let locked = doc.components.iter().filter(|c| c.meta.status == Status::Locked).count()
        + doc.rules.iter().filter(|r| r.meta.status == Status::Locked).count();
    println!("{} luật có hiệu lực · {} trong đó đã khoá\n", doc.enforced_count(), locked);

    for d in &diags {
        let mark = match d.severity {
            Severity::Error => "✗",
            Severity::Warning => "⚠",
        };
        println!("{mark} {d}");
    }

    let errors = diag::count(&diags, Severity::Error);
    let warnings = diag::count(&diags, Severity::Warning);
    if errors == 0 && warnings == 0 {
        println!("không có lỗi, không có cảnh báo");
    } else {
        println!("\n{errors} lỗi, {warnings} cảnh báo");
    }

    ExitCode::from(if errors > 0 { exit::DSL_INVALID } else { exit::OK })
}
