//! Test cho bộ quét import.
//!
//! Nhóm thứ hai quan trọng hơn nhóm thứ nhất: bộ quét **không được** tính các
//! chuỗi trông giống import nằm trong chú thích, chuỗi hay regex. Báo động giả
//! phá niềm tin vào công cụ nhanh hơn cả bỏ sót.

use arch_core::scan::imports;

fn specs(src: &str) -> Vec<String> {
    imports(src).into_iter().map(|i| i.spec).collect()
}

#[test]
fn moi_dang_import_deu_duoc_thay() {
    let src = r#"
import def from 'a'
import { x, y } from 'b'
import 'c'
import type { T } from 'd'
import * as ns from 'e'
export { z } from 'f'
export * from 'g'
const m = require('h')
const lazy = await import('i')
"#;
    assert_eq!(specs(src), ["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
}

#[test]
fn import_bac_qua_nhieu_dong() {
    let src = "import {\n  alpha,\n  beta,\n} from '../order/create'\n";
    assert_eq!(specs(src), ["../order/create"]);
}

#[test]
fn chu_thich_mot_dong_khong_duoc_tinh() {
    assert!(specs("// import { a } from 'nope'\nimport { b } from 'yes'").contains(&"yes".into()));
    assert_eq!(specs("// import { a } from 'nope'"), Vec::<String>::new());
}

#[test]
fn chu_thich_nhieu_dong_khong_duoc_tinh() {
    let src = "/*\nimport { a } from 'nope'\n*/\nimport { b } from 'yes'\n";
    assert_eq!(specs(src), ["yes"]);
}

#[test]
fn chuoi_chua_chu_import_khong_duoc_tinh() {
    let src = r#"const doc = "import { a } from 'nope'"
import { b } from 'yes'
"#;
    assert_eq!(specs(src), ["yes"]);
}

#[test]
fn template_literal_khong_duoc_tinh() {
    let src = "const doc = `import { a } from 'nope'`\nimport { b } from 'yes'\n";
    assert_eq!(specs(src), ["yes"]);
}

#[test]
fn bieu_thuc_trong_template_khong_lam_lech_bo_quet() {
    let src = "const s = `tổng ${items.map(i => `${i.name}: '${i.v}'`).join(', ')}`\n\
               import { b } from 'yes'\n";
    assert_eq!(specs(src), ["yes"]);
}

#[test]
fn regex_chua_dau_nhay_khong_lam_lech_bo_quet() {
    let src = "const re = /from 'nope'/g\nimport { b } from 'yes'\n";
    assert_eq!(specs(src), ["yes"]);
}

#[test]
fn phep_chia_khong_bi_nham_la_regex() {
    let src = "const r = a / b / c\nimport { b } from 'yes'\n";
    assert_eq!(specs(src), ["yes"]);
}

#[test]
fn so_dong_dung_voi_vi_tri_that() {
    let found = imports("\n\n\nimport { a } from 'x'\n");
    assert_eq!(found[0].line, 4, "số dòng sai còn tệ hơn không báo");
}

/// `export type { T } from 'x'` cũng phải được thấy, giống `import type`.
#[test]
fn export_type_duoc_thay() {
    assert_eq!(specs("export type { T } from 'x'"), ["x"]);
}

#[test]
fn chuoi_rong_bi_bo_qua() {
    assert_eq!(specs("import ''"), Vec::<String>::new());
}
