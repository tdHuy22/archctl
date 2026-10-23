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
fn ts_type_trong_ngoac_khong_lam_mat_import() {
    // `type` trong ngoặc là bổ ngữ của một tên (TS 4.5), không mở câu lệnh
    // mới. Vue core từng mất 363/1 728 import tĩnh vì coi nó là mốc dừng.
    let src = "\
import { type Foo, bar } from './a'
import { baz, type Qux } from './b'
export { type T } from './c'
import {
  type Ref,
  ref,
} from './d'
import type { X } from './e'
";
    assert_eq!(specs(Lang::TypeScript, src), ["./a", "./b", "./c", "./d", "./e"]);
}

/// Ngược lại, các từ khoá khác trong ngoặc vẫn là mốc dừng: `import()` và
/// `require()` trong `export default {...}` là lời gọi, không phải một phần của
/// câu export. Router và async component của Vue 2 viết đúng như vậy, thường
/// không có dấu chấm phẩy.
#[test]
fn ts_import_dong_trong_khoi_export_khong_bi_nuot() {
    let cases: [(&str, &[&str]); 5] = [
        (
            "export default {\n  components: { Foo: () => import('./Foo.vue') },\n  data() { return { x: require('./bar') } }\n}\n",
            &["./Foo.vue", "./bar"],
        ),
        (
            "export default new Router({ routes: [{ component: () => import('./views/Home.vue') }] })\n",
            &["./views/Home.vue"],
        ),
        ("export interface P { a: import('./types').A }\n", &["./types"]),
        ("export type T = { b: import('./t2').B }\n", &["./t2"]),
        ("export default { x: require('./bar'), type: 'y' }\n", &["./bar"]),
    ];
    for (src, want) in cases {
        assert_eq!(specs(Lang::TypeScript, src), want, "{src}");
    }
}

#[test]
fn ts_import_bang_require() {
    let src = "import fs = require('fs')\nexport import util = require('./util')\n";
    assert_eq!(specs(Lang::TypeScript, src), ["fs", "./util"]);
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

/// Trong chuỗi ba nháy, `\"` vẫn là một ký tự đã thoát: chuỗi không đóng ở
/// `\"""`. Đóng sớm thì phần sau của chuỗi thành mã, sinh import giả, và ba
/// nháy cuối mở một chuỗi mới nuốt mất import thật (idlelib `pyparse.py`).
#[test]
fn py_chuoi_ba_nhay_co_ky_tu_thoat() {
    let src = "x = \"\"\"a\\\"\"\" vẫn ở trong\nimport khong_phai\n\"\"\"\nimport that\n";
    assert_eq!(specs(Lang::Python, src), ["that"]);
    assert_eq!(lines(Lang::Python, src), [4]);
}

/// f-string của Python 3.12 (PEP 701) và t-string của 3.14: biểu thức trong
/// `{}` được chứa chính loại nháy bọc ngoài, được xuống dòng, được có chú
/// thích. Đọc như chuỗi thường thì nháy bên trong đóng chuỗi sớm, và một
/// chuỗi ba nháy lạc nuốt phần sau của file (thư viện chuẩn `test_fstring.py`).
#[test]
fn py_f_string_long_nhay() {
    let cases = [
        "a = f\"{\"\"\"y\"\"\"}\"\n",
        "b = f'''{\n    \"\"\"lồng\n    ''' vẫn lồng\"\"\"\n}'''\n",
        "c = f\"{x[\"k\"]:'>10} {{không phải trường}} {f\"{z!r}\"}\" + '\"\"\"'\n",
        "d = rf\"\\d{n}\" + t\"{'''x'''}\" + T'{\"\"\"}\"\"\"}'\n",
        "e = f\"{w  # chú thích có nháy '''\n}\"\n",
    ];
    for head in cases {
        let src = format!("{head}import that\n");
        assert_eq!(specs(Lang::Python, &src), ["that"], "{head}");
        assert_eq!(lines(Lang::Python, &src), [head.lines().count() + 1], "{head}");
    }
    // Phần chữ của f-string vẫn là chuỗi: tên bảng trong SQL vẫn được thấy.
    let sql = "q = f\"SELECT * FROM payments WHERE id = {pid}\"\n";
    assert_eq!(Lang::Python.scan_all(sql, &Default::default(), &["payments"]).tables.len(), 1);
}

/// `yield from x` và `raise E from e` không phải import. Bản cũ coi chúng là
/// import (1 045 cái giả trên 9 repo của bộ đo), rồi bỏ qua "danh sách tên"
/// kéo sang dòng sau và nuốt import thật ngay sau đó: 15 cái, theo mẫu
/// `except ImportError: raise ... from e` rồi import tiếp (google-api-core
/// `rest_streaming_async.py:30`).
#[test]
fn py_yield_from_va_raise_from_khong_phai_import() {
    let src = "\
def g():
    yield from helpers
try:
    import fast
except ImportError as e:
    raise ImportError(\"cần fast\") from e
import after_raise
x = (yield
     from gen)
";
    assert_eq!(specs(Lang::Python, src), ["fast", "after_raise"]);
}

/// Nối dòng bằng `\` trong câu import. `from a \` xuống dòng `import b` từng
/// sinh thêm import giả `b`; `import a, \` xuống dòng `b` từng mất `b`.
#[test]
fn py_import_noi_dong_bang_gach_cheo() {
    let src = "from pkg.mod \\\n    import thing\nimport a, \\\n    b\nimport c\n";
    assert_eq!(specs(Lang::Python, src), ["pkg.mod", "a", "b", "c"]);
    assert_eq!(lines(Lang::Python, src), [1, 3, 3, 5]);
}

/// `from x import y; import z` là hai câu lệnh. Bỏ qua danh sách tên tới hết
/// dòng thì mất `z` (thư viện chuẩn `test_future_stmt/nested_scope.py`).
#[test]
fn py_hai_cau_import_tren_mot_dong() {
    let src = "from __future__ import nested_scopes; import site\nimport os; from x import (y,\n z); import w\n";
    assert_eq!(specs(Lang::Python, src), ["__future__", "site", "os", "x", "w"]);
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

/// Chú thích khối mở trên dòng chỉ thị là chú thích tới tận `*/`. Bỏ qua cả
/// dòng chỉ thị làm mất dấu `/*`, và phần sau của chú thích bị đọc như mã:
/// `#include` đã bị chú thích hoá lại thành import, dấu nháy trong "it's" làm
/// lệch nhịp (redis `server.h`, libbpf, zstd).
#[test]
fn c_chu_thich_mo_tren_dong_chi_thi() {
    let src = "\
#include \"a.h\"
#define A 1 /* mã cũ:
#include \"old.h\"
   it's gone */
#include \"b.h\" /* đuôi
#include \"no.h\"
*/
#define S \"/*\"
#include \"c.h\"
";
    assert_eq!(specs(Lang::C, src), ["a.h", "b.h", "c.h"]);
    assert_eq!(lines(Lang::C, src), [1, 5, 9]);
}

/// Chỉ thị nối dòng bằng `\` kéo tới hết dòng nối cuối cùng. redis `tls.c`
/// có `#error "... Build against a \` cộng hai dòng chữ; phần chữ bị đọc như
/// mã thì sinh ký hiệu giả, và nháy kép lệch từng lộn ngược cả phần sau của
/// file — archctl thấy 0/31 lời gọi `serverLog`.
#[test]
fn c_chi_thi_noi_dong_bang_gach_cheo() {
    let src = "\
#error \"needs OpenSSL 1.0.2. Build against a \\
newer OpenSSL, never strcat\\
 into it.\"
#define MAX(a, b) \\
    ((a) > (b) ? (a) : (b))
#include \"b.h\"
void f(void) { strcpy(d, s); }
";
    assert_eq!(lines(Lang::C, src), [6]);
    let hits: Vec<String> =
        Lang::C.scan(src, &["strcat", "strcpy"]).symbols.into_iter().map(|h| h.symbol).collect();
    assert_eq!(hits, ["strcpy"]);
}

/// Dấu phân cách chữ số của C++14 và C23 không mở char literal. Với
/// `0xFFFF'FFFF; }` lexer từng nuốt cả dấu `}` đóng hàm, và mọi hàm định nghĩa
/// sau đó rơi vào "bên trong một khối", mất khỏi phần ghép FFI (abseil
/// `hash.h`, `numbers.cc`, `utf8.cc`).
#[test]
fn c_dau_phan_cach_chu_so() {
    let src = "\
int a(int x) { return x * 1'000'000; }
int b(int y) { return y & 0xFFFF'FFFF; }
int c(int z) { return z | 0b0111'1111 | 'q'; }
double d(void) { return 1.5e+3 + 0x1p-3; }
";
    let provides: Vec<String> =
        Lang::C.scan(src, &[]).foreign.provides.into_iter().map(|h| h.symbol).collect();
    assert_eq!(provides, ["a", "b", "c", "d"]);
}

/// Chuỗi thô của C++11 không có ký tự thoát, được chứa nháy kép lẫn xuống
/// dòng, và chỉ đóng ở `)delim"`. abseil `usage_test.cc` dùng `R"(...)"`,
/// matplotlib dùng `R"""(...)"""` cho docstring của pybind11.
#[test]
fn c_chuoi_tho_cpp() {
    let src = "\
const char* a = R\"(he said \"hi)\";
const char* b = R\"delim(
#include \"no.h\"
)\" vẫn ở trong
)delim\";
const char* c = u8R\"(x)\" LR\"(y)\";
const char* d = R\"\"\"(Docstring with \"quotes\" and don't
)\"\"\";
#include \"yes.h\"
";
    assert_eq!(specs(Lang::C, src), ["yes.h"]);
    assert_eq!(lines(Lang::C, src), [9]);
}

/// File lưu bằng UTF-8 có BOM (mặc định của nhiều trình soạn trên Windows).
/// BOM từng thành một token, nên `#` của dòng 1 không còn "ở đầu dòng":
/// `#include <a.h>` ở đó biến mất, và một chỉ thị khác ở đó bị đọc như mã.
#[test]
fn c_bom_khong_an_chi_thi_dong_dau() {
    let found = Lang::C.imports("\u{feff}#include <a.h>\n#include \"b.h\"\n");
    let pairs: Vec<(&str, bool)> = found.iter().map(|i| (i.spec.as_str(), i.system)).collect();
    assert_eq!(pairs, [("a.h", true), ("b.h", false)]);
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

/// `br"..."` và `cr"..."` là chuỗi thô như `r"..."`, không có ký tự thoát.
/// Đọc như chuỗi thường thì `br"\"` không đóng, và vì chuỗi Rust được bắc qua
/// dòng, nó nuốt mọi `use` phía sau.
#[test]
fn rust_chuoi_tho_byte_va_c() {
    let cases = [
        "const A: &[u8] = br\"\\\";\n",
        "const B: &[u8] = br#\"he said \"hi\"#;\n",
        "const C: &CStr = cr\"\\\";\n",
        "const D: &CStr = cr#\"\"\"#;\n",
        "const E: &[u8] = br#\"use crate::no::X;\"#;\n",
    ];
    for head in cases {
        let src = format!("{head}use crate::sau::Thing;\n");
        assert_eq!(specs(Lang::Rust, &src), ["crate::sau::Thing"], "{head}");
        assert_eq!(lines(Lang::Rust, &src), [2], "{head}");
    }
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

/// Tên đặt lại ngay sau `import`, không có ngoặc: `go/parser` tìm ra etcd 4
/// file (`import bolt "go.etcd.io/bbolt"` ở
/// `server/storage/backend/config_default.go:19`) và hugo 1 file mà archctl
/// từng bỏ sót.
#[test]
fn go_import_don_co_ten_dat_lai() {
    let src = "\
package main

import bolt \"go.etcd.io/bbolt\"
import . \"strings\"
import _ \"embed\"
";
    assert_eq!(specs(Lang::Go, src), ["go.etcd.io/bbolt", "strings", "embed"]);
    assert_eq!(lines(Lang::Go, src), [3, 4, 5]);
}

/// Chuỗi thô của Go không có ký tự thoát và không có nội suy: `` `C:\` `` và
/// `` `${HOME` `` đóng ở dấu backtick kế tiếp. Đọc như template JS thì `\` nuốt
/// dấu đóng và `${` nuốt cả phần sau — hugo 15 file, etcd 5, có
/// `common/paths/url.go:129` — và lời gọi cgo phía sau biến mất.
#[test]
fn go_chuoi_tho_khong_co_ky_tu_thoat() {
    let src = "package main\n\nimport \"C\"\n\nvar p = `C:\\`\nvar q = `${HOME`\nvar r = `a\\\nb`\n\nfunc main() { C.native_call() }\n";
    let calls: Vec<(String, usize)> =
        Lang::Go.scan(src, &[]).foreign.calls.into_iter().map(|h| (h.symbol, h.line)).collect();
    assert_eq!(calls, [("native_call".to_string(), 10)]);
    // Ở ngôn ngữ không có backtick, nó chỉ là một ký hiệu lạc.
    assert_eq!(specs(Lang::Python, "x = 1 ` 2\nimport z\n"), ["z"]);
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

/// CUDA là C++ (vllm có 167 file `.cu`, `.cuh`), Cython là Python cộng thêm
/// `cimport` (scikit-learn có 77 file), `.vue` là TypeScript trong `<script>`.
/// Trước đây chúng không được quét, và bộ đo cũng không thấy vì nó chỉ đếm
/// những đuôi đã hỗ trợ.
#[test]
fn duoi_cua_cuda_cython_vue() {
    assert_eq!(Lang::for_path("csrc/attention.cu"), Some(Lang::C));
    assert_eq!(Lang::for_path("csrc/attention.cuh"), Some(Lang::C));
    assert_eq!(Lang::for_path("pkg/_fast.pyx"), Some(Lang::Python));
    assert_eq!(Lang::for_path("pkg/_fast.pxd"), Some(Lang::Python));
    assert_eq!(Lang::for_path("pkg/_fast.pxi"), Some(Lang::Python));
    assert_eq!(Lang::for_path("src/App.vue"), Some(Lang::TypeScript));
    // `.inc` thì không: abseil dùng cho C++, nhưng curl dùng cho Makefile và
    // scikit-learn cho reStructuredText.
    assert_eq!(Lang::for_path("docs/Makefile.inc"), None);
}

/// Component một file của Vue: mã nằm trong `<script>`. `<template>` và
/// `<style>` là HTML và CSS — có chữ như "Don't import './no'", và không có
/// import nào. Số dòng vẫn tính theo file gốc.
#[test]
fn vue_chi_doc_phan_script() {
    let src = "\
<template>
  <p>Don't import './no'</p>
</template>
<script setup lang=\"ts\">
import { type Ref, ref } from 'vue'
import Child from './Child.vue'
</script>
<style>
.a { content: \"import x from 'no2'\" }
</style>
";
    let (lang, got) = arch_core::scan::imports_for("src/App.vue", src).unwrap();
    assert_eq!(lang, Lang::TypeScript);
    let got: Vec<(String, usize)> = got.into_iter().map(|i| (i.spec, i.line)).collect();
    assert_eq!(got, [("vue".to_string(), 5), ("./Child.vue".to_string(), 6)]);
}

/// Cython: `cimport` và `from x cimport y` là import, và `cdef extern from
/// "math.h"` thì không (đó là header C, không phải module Python).
#[test]
fn cython_cimport() {
    let src = "\
from cython cimport floating
from sklearn.utils._typedefs cimport uint8_t, intp_t
cimport numpy as cnp
import numpy as np
from ..utils cimport _typedefs
cdef extern from \"math.h\":
    double sqrt(double x)
";
    assert_eq!(
        specs(Lang::Python, src),
        ["cython", "sklearn.utils._typedefs", "numpy", "numpy", "..utils"]
    );
}

fn lines(lang: Lang, src: &str) -> Vec<usize> {
    lang.imports(src).iter().map(|i| i.line).collect()
}

/// `\` cuối dòng trong chuỗi nối chuỗi sang dòng sau. Bỏ qua `\n` đó mà không
/// đếm thì mọi token phía sau lệch một dòng: vllm `deepseek_v2.py:488` lệch 1,
/// matplotlib `tests/test_axes.py` lệch 3.
#[test]
fn chuoi_noi_dong_khong_lam_lech_so_dong() {
    assert_eq!(lines(Lang::Python, "s = 'a\\\nb'\nimport x\n"), [3]);
    assert_eq!(lines(Lang::C, "const char *s = \"a\\\nb\";\n#include \"x.h\"\n"), [3]);
    assert_eq!(lines(Lang::TypeScript, "const s = `a\\\nb`\nimport x from './x'\n"), [3]);
}

/// Chuỗi thường và char literal không bắc qua dòng, trừ khi nối bằng `\`.
/// Một dấu nháy lạc từng nuốt phần còn lại của file: `excalidraw-app/App.tsx`
/// mất 386 dòng sau `<h1>I'm not a pretzel!</h1>`. Giờ thiệt hại dừng ở cuối
/// dòng chứa nó.
#[test]
fn dau_nhay_lac_chi_hong_mot_dong() {
    let tsx = "import a from 'a'\nconst v = <h1>I'm here</h1>\nconst b = require('./b')\n";
    assert_eq!(specs(Lang::TypeScript, tsx), ["a", "./b"]);
    let c = "#include \"a.h\"\n#if 0\nDon't compile this\n#endif\n#include \"b.h\"\n";
    assert_eq!(specs(Lang::C, c), ["a.h", "b.h"]);
    // Rust thì khác: chuỗi thường được phép trải nhiều dòng.
    let rs = "const S: &str = \"dòng một\ndòng hai\";\nuse crate::sau::X;\n";
    assert_eq!(specs(Lang::Rust, rs), ["crate::sau::X"]);
    // Nối dòng bằng `\` vẫn nối, kể cả khi file xuống dòng kiểu Windows.
    assert_eq!(lines(Lang::C, "const char *s = \"a\\\r\nb\";\r\n#include \"x.h\"\r\n"), [3]);
}

#[test]
fn so_dong_dung_voi_vi_tri_that() {
    assert_eq!(Lang::TypeScript.imports("\n\n\nimport { a } from 'x'\n")[0].line, 4);
    assert_eq!(Lang::Python.imports("\n\nimport os\n")[0].line, 3);
    assert_eq!(Lang::C.imports("\n\n\n\n#include \"x.h\"\n")[0].line, 5);
    assert_eq!(Lang::Rust.imports("\n\nuse crate::x;\n")[0].line, 3);
}

// ---------- Java (chỉ để dò gRPC) ----------

fn words(src: &str) -> Vec<(usize, String)> {
    use arch_core::scan::lexer::Tok;
    Lang::Java.tokens(src).into_iter().filter_map(|t| match t.tok {
        Tok::Word(w) => Some((t.line, w)),
        _ => None,
    }).collect()
}

/// Text block, chuỗi, char literal, chú thích không nuốt mã sau chúng; tên
/// đầy đủ gộp thành một từ; import Java không được đọc (TM-68).
#[test]
fn java_chuoi_text_block_va_chu_thich() {
    let src = "import hipstershop.AdServiceGrpc;\nclass A {\n  String t = \"\"\"\n    \"quoted\" // not a comment /* nor this\n    \\\"\"\" still inside\n    \"\"\";\n  char q = '\"'; char e = '\\''; String s = \"a\\\"b // x\";\n  /* block \"\"\" */ int after1;\n  // line \"\"\" \n  hipstershop.AdServiceGrpc.AdServiceImplBase base;\n}\n";
    let w = words(src);
    let names: Vec<&str> = w.iter().map(|(_, s)| s.as_str()).collect();
    assert!(names.contains(&"after1"), "{names:?}");
    assert!(w.contains(&(10, "hipstershop.AdServiceGrpc.AdServiceImplBase".to_string())), "{w:?}");
    assert!(!names.iter().any(|n| *n == "quoted" || *n == "still" || *n == "block" || *n == "line"), "{names:?}");
    assert!(Lang::Java.imports(src).is_empty());
    assert_eq!(Lang::for_path("src/main/java/A.java"), Some(Lang::Java));
}

/// Text block, chú thích không đóng: dừng ở cuối file, tuyến tính (TM-69).
#[test]
fn java_khoi_khong_dong_khong_treo() {
    let t = std::time::Instant::now();
    let big = format!("class A {{ String s = \"\"\"\n{}", "x \" y\n".repeat(200_000));
    assert_eq!(words(&big).len(), 4, "class A String s, rồi hết file");
    let big = format!("class A {{ /* {}", "x \"\"\" y\n".repeat(200_000));
    assert_eq!(words(&big).len(), 2);
    assert!(t.elapsed().as_secs() < 5);
}
