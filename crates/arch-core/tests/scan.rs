//! Test cho bộ quét import của từng ngôn ngữ.
//!
//! Mỗi ngôn ngữ có hai nhóm: nhận đúng các dạng import, và **không** nhận
//! những chuỗi trông giống import nằm trong chú thích, chuỗi hay regex. Nhóm
//! thứ hai quan trọng hơn — báo động giả phá niềm tin vào công cụ nhanh hơn cả
//! bỏ sót.

use arch_core::scan::Lang;

fn specs(lang: Lang, src: &str) -> Vec<String> {
    lang.imports(src).into_iter().map(|i| i.spec).collect()
}

// ---------- TypeScript ----------

#[test]
fn ts_moi_dang_import_deu_duoc_thay() {
    let src = r#"
import def from 'a'
import { x, y } from 'b'
import 'c'
import type { T } from 'd'
import * as ns from 'e'
export { z } from 'f'
export type { U } from 'g'
export * from 'h'
const m = require('i')
const lazy = await import('j')
"#;
    assert_eq!(specs(Lang::TypeScript, src), ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);
}

#[test]
fn ts_import_bac_qua_nhieu_dong() {
    let src = "import {\n  alpha,\n  beta,\n} from '../order/create'\n";
    assert_eq!(specs(Lang::TypeScript, src), ["../order/create"]);
}

#[test]
fn ts_chu_thich_chuoi_template_khong_duoc_tinh() {
    let src = r#"// import { a } from 'no1'
/* import { a } from 'no2' */
const s = "import { a } from 'no3'"
const t = `import { a } from 'no4'`
import { b } from 'yes'
"#;
    assert_eq!(specs(Lang::TypeScript, src), ["yes"]);
}

#[test]
fn ts_regex_va_phep_chia_khong_lam_lech_bo_quet() {
    let src = "const re = /from 'no'/g\nconst r = a / b / c\nimport { b } from 'yes'\n";
    assert_eq!(specs(Lang::TypeScript, src), ["yes"]);
}

// ---------- Python ----------

#[test]
fn py_moi_dang_import_deu_duoc_thay() {
    let src = "\
import os
import os.path
import numpy as np
import json, csv
from pkg.mod import thing
from . import sibling
from ..other.deep import thing
from .local import a, b
";
    assert_eq!(
        specs(Lang::Python, src),
        ["os", "os.path", "numpy", "json", "csv", "pkg.mod", ".", "..other.deep", ".local"]
    );
}

#[test]
fn py_import_nhieu_dong_trong_ngoac() {
    let src = "from pkg.mod import (\n    alpha,\n    beta,\n)\nimport sau\n";
    assert_eq!(specs(Lang::Python, src), ["pkg.mod", "sau"]);
}

#[test]
fn py_chu_thich_va_docstring_khong_duoc_tinh() {
    let src = "\
# import khong_phai
'''
import cung_khong
from day import kia
'''
s = \"import chuoi\"
import that_su
";
    assert_eq!(specs(Lang::Python, src), ["that_su"]);
}

#[test]
fn py_dong_sau_import_khong_bi_nuot() {
    let src = "import os\nduong_dan = os.path\n";
    assert_eq!(specs(Lang::Python, src), ["os"]);
}

// ---------- C và C++ ----------

#[test]
fn c_include_cuc_bo_va_he_thong() {
    let src = "\
#include <stdio.h>
#include \"parser.h\"
#include <sys/socket.h>
#include \"../net/packet.h\"
";
    let found = Lang::C.imports(src);
    let pairs: Vec<(&str, bool)> =
        found.iter().map(|i| (i.spec.as_str(), i.system)).collect();
    assert_eq!(
        pairs,
        [
            ("stdio.h", true),
            ("parser.h", false),
            ("sys/socket.h", true),
            ("../net/packet.h", false),
        ]
    );
}

#[test]
fn c_include_trong_ifdef_van_duoc_tinh() {
    // Có chủ ý: đoán nhánh nào được biên dịch cần biết toàn bộ cờ build.
    let src = "#ifdef LINUX\n#include <epoll.h>\n#else\n#include <kqueue.h>\n#endif\n";
    assert_eq!(specs(Lang::C, src), ["epoll.h", "kqueue.h"]);
}

#[test]
fn c_include_trong_chu_thich_va_chuoi_khong_duoc_tinh() {
    let src = "\
// #include \"no1.h\"
/* #include \"no2.h\" */
const char *s = \"#include \\\"no3.h\\\"\";
#include \"yes.h\"
";
    assert_eq!(specs(Lang::C, src), ["yes.h"]);
}

#[test]
fn c_chi_thi_khac_khong_sinh_rac() {
    let src = "#define INCLUDE_ME 1\n#pragma once\n#include \"yes.h\"\n";
    assert_eq!(specs(Lang::C, src), ["yes.h"]);
}

// ---------- Rust ----------

#[test]
fn rust_moi_dang_use_deu_duoc_thay() {
    let src = "\
use crate::decode::Packet;
use super::helper;
use std::collections::HashMap;
pub use crate::report::Summary;
extern crate libc;
";
    assert_eq!(
        specs(Lang::Rust, src),
        [
            "crate::decode::Packet",
            "super::helper",
            "std::collections::HashMap",
            "crate::report::Summary",
            "libc",
        ]
    );
}

#[test]
fn rust_use_voi_ngoac_nhon_duoc_tach_ra() {
    let src = "use crate::decode::{Packet, Header};\n";
    assert_eq!(specs(Lang::Rust, src), ["crate::decode::Packet", "crate::decode::Header"]);
}

/// Tên sau `as` là tên mới, không phải một thành viên. Bản cũ coi nó là
/// thành viên: `use crate::{error as err}` sinh thêm `crate::err` — một module
/// không tồn tại, mà một khi `crate::X` biết trỏ về gốc crate thì nó thành một
/// cạnh giả tới `lib.rs`.
#[test]
fn rust_ten_sau_as_khong_phai_thanh_vien() {
    let src = "use crate::{error as err, Foo as _};\nuse crate::x::Y as Z;\n";
    assert_eq!(specs(Lang::Rust, src), ["crate::error", "crate::Foo", "crate::x::Y"]);
}

/// Ngoặc lồng giữ tiền tố của mình. Bản cũ cho `crate::a::C` thay vì
/// `crate::a::b::C`: một đường dẫn sai, phân giải ra một file sai.
#[test]
fn rust_ngoac_long_giu_tien_to() {
    let src = "use crate::a::{b::{C, D as Dd}, self};\n";
    assert_eq!(specs(Lang::Rust, src), ["crate::a::b::C", "crate::a::b::D", "crate::a"]);
}

#[test]
fn rust_ngoac_o_dau_va_hai_cham_dan_dau() {
    assert_eq!(specs(Lang::Rust, "use {crate::A, crate::b::C};\n"), ["crate::A", "crate::b::C"]);
    assert_eq!(specs(Lang::Rust, "use ::std::io;\n"), ["std::io"]);
}

/// Tên mà mỗi câu `use` đưa vào phạm vi. Bộ phân giải đọc chúng ở file gốc
/// crate để biết `crate::Error` thật ra là `crate::error::Error`.
#[test]
fn rust_ten_duoc_dua_vao_pham_vi() {
    let src = "\
use crate::Error;
pub use self::error::{Error as Failure, Code};
pub use libsqlite3_sys as ffi;
use crate::a::{b::{C, D as Dd}, self};
pub use crate::model::*;
use crate::x as _;
extern crate serde;
";
    let got = arch_core::scan::rust_bindings(src);
    assert_eq!(
        got,
        [
            ("Error".to_string(), "crate::Error".to_string()),
            ("Failure".into(), "self::error::Error".into()),
            ("Code".into(), "self::error::Code".into()),
            ("ffi".into(), "libsqlite3_sys".into()),
            ("C".into(), "crate::a::b::C".into()),
            ("Dd".into(), "crate::a::b::D".into()),
            ("a".into(), "crate::a".into()),
            ("serde".into(), "serde".into()),
        ]
    );
}

/// Trong `mod tests { ... }` viết lồng, `super` là module của chính file, không
/// phải module cha của nó. Không biết điều đó thì `#[cfg(test)] mod tests { use
/// super::*; }` — có ở gần như mọi file Rust — thành một cạnh tới module cha,
/// và ở module cấp một là một cạnh giả tới `lib.rs`.
#[test]
fn rust_mod_long_doi_super_va_self_ve_module_cua_file() {
    let cases = [
        ("mod tests { use super::*; }", vec!["self"]),
        ("mod tests { use super::super::X; }", vec!["super::X"]),
        ("mod a { mod b { use super::X; } }", vec!["self::a::X"]),
        ("mod tests { use self::helper::Y; }", vec!["self::tests::helper::Y"]),
        ("mod tests { use crate::x::Y; }", vec!["crate::x::Y"]),
        // Thân hàm không phải module.
        ("fn f() { use super::X; }", vec!["super::X"]),
        // `mod x;` khai một module file, không mở khối nào.
        ("mod decl;\nuse super::Q;", vec!["super::Q"]),
        // Khối đã đóng thì trở lại module của file.
        ("mod t { fn g() {} }\nuse super::Z;", vec!["super::Z"]),
    ];
    for (src, want) in cases {
        assert_eq!(specs(Lang::Rust, src), want, "{src}");
    }
}

/// Chỉ `use` ở tầng ngoài cùng của file mới đưa tên vào phạm vi của file — tức
/// vào `crate::` khi đó là file gốc crate.
#[test]
fn rust_ten_trong_ham_hay_mod_long_khong_vao_pham_vi_file() {
    let src = "mod inner { pub use crate::x::Y; }\nfn f() { use a::B; }\npub use c::D;\n";
    assert_eq!(arch_core::scan::rust_bindings(src), [("D".to_string(), "c::D".to_string())]);
}

#[test]
fn rust_lifetime_khong_bi_nham_la_chuoi() {
    // `'a` không đóng. Nếu lexer coi nó là chuỗi thì sẽ nuốt hết phần sau và
    // bỏ sót dòng `use` phía dưới.
    let src = "fn f<'a>(x: &'a str) -> &'a str { x }\nuse crate::sau::Thing;\n";
    assert_eq!(specs(Lang::Rust, src), ["crate::sau::Thing"]);
}

#[test]
fn rust_char_literal_van_duoc_hieu_dung() {
    let src = "const C: char = '\\n';\nconst D: char = 'x';\nuse crate::sau::Thing;\n";
    assert_eq!(specs(Lang::Rust, src), ["crate::sau::Thing"]);
}

#[test]
fn rust_raw_string_va_chu_thich_long_nhau() {
    let src = "\
let s = r#\"use crate::khong::Co;\"#;
/* một /* chú thích lồng */ use crate::cung_khong::Co; */
use crate::that_su::Thing;
";
    assert_eq!(specs(Lang::Rust, src), ["crate::that_su::Thing"]);
}

// ---------- Go ----------

#[test]
fn go_import_don_va_khoi() {
    let src = "\
package main

import \"fmt\"

import (
    \"os\"
    alias \"net/http\"

    \"github.com/acme/app/internal/auth\"
)
";
    assert_eq!(
        specs(Lang::Go, src),
        ["fmt", "os", "net/http", "github.com/acme/app/internal/auth"]
    );
}

#[test]
fn go_chu_thich_va_chuoi_khong_duoc_tinh() {
    let src = "// import \"no1\"\nconst s = \"import no2\"\nimport \"yes\"\n";
    assert_eq!(specs(Lang::Go, src), ["yes"]);
}

// ---------- Chung ----------

#[test]
fn phan_mo_rong_quyet_dinh_ngon_ngu() {
    assert_eq!(Lang::for_path("a/b.ts"), Some(Lang::TypeScript));
    assert_eq!(Lang::for_path("a/b.py"), Some(Lang::Python));
    assert_eq!(Lang::for_path("a/b.hpp"), Some(Lang::C));
    assert_eq!(Lang::for_path("a/b.rs"), Some(Lang::Rust));
    assert_eq!(Lang::for_path("a/b.go"), Some(Lang::Go));
    assert_eq!(Lang::for_path("a/b.md"), None);
    assert_eq!(Lang::for_path("Makefile"), None);
}

#[test]
fn so_dong_dung_voi_vi_tri_that() {
    assert_eq!(Lang::TypeScript.imports("\n\n\nimport { a } from 'x'\n")[0].line, 4);
    assert_eq!(Lang::Python.imports("\n\nimport os\n")[0].line, 3);
    assert_eq!(Lang::C.imports("\n\n\n\n#include \"x.h\"\n")[0].line, 5);
    assert_eq!(Lang::Rust.imports("\n\nuse crate::x;\n")[0].line, 3);
}
