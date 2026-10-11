//! Notebook Jupyter (`.ipynb`): mã Python nằm trong ô code của một file JSON.
//!
//! Chỉ ô code là mã. Ô markdown, output, metadata thì không — một câu `import`
//! trong ô markdown là văn bản. Số dòng báo ra là dòng trong chính file
//! `.ipynb`, nên mở file ra là thấy đúng chỗ.

use arch_core::scan::{Lang, source_view};

/// Import đọc được: `(spec, dòng)`.
fn imports(path: &str, src: &str) -> Vec<(String, usize)> {
    let lang = Lang::for_path(path).expect("phải nhận ra đuôi file");
    let view = source_view(path, src);
    lang.imports(&view).into_iter().map(|i| (i.spec, i.line)).collect()
}

/// Notebook như Jupyter ghi ra: thụt một khoảng, mỗi dòng mã một chuỗi JSON.
const NB: &str = r##"{
 "cells": [
  {
   "cell_type": "markdown",
   "metadata": {},
   "source": [
    "# Phân tích\n",
    "import fake_from_markdown\n"
   ]
  },
  {
   "cell_type": "code",
   "execution_count": 1,
   "metadata": {},
   "outputs": [
    {
     "name": "stdout",
     "output_type": "stream",
     "text": ["import fake_from_output\n"]
    }
   ],
   "source": [
    "%matplotlib inline\n",
    "!pip install requests\n",
    "import numpy as np\n",
    "from mlapp.models.net import Net"
   ]
  },
  {
   "cell_type": "code",
   "execution_count": null,
   "metadata": {},
   "outputs": [],
   "source": [
    "%%bash\n",
    "import not_python\n"
   ]
  },
  {
   "cell_type": "code",
   "execution_count": null,
   "metadata": {},
   "outputs": [],
   "source": [
    "%%time\n",
    "s = \"import trong_chuoi\"\n",
    "import pandas as pd\n"
   ]
  }
 ],
 "metadata": {
  "kernelspec": {"display_name": "Python 3", "language": "python", "name": "python3"}
 },
 "nbformat": 4,
 "nbformat_minor": 5
}
"##;

#[test]
fn ipynb_la_python() {
    assert_eq!(Lang::for_path("notebooks/eda.ipynb"), Some(Lang::Python));
}

#[test]
fn chi_doc_o_code_va_giu_dung_dong_trong_file() {
    assert_eq!(
        imports("eda.ipynb", NB),
        [
            ("numpy".to_string(), 25),
            ("mlapp.models.net".to_string(), 26),
            ("pandas".to_string(), 47),
        ]
    );
}

#[test]
fn nguon_la_mot_chuoi_hoac_file_viet_tren_mot_dong() {
    // nbformat cho phép `source` là một chuỗi; file sinh bằng công cụ khác có
    // thể nằm trọn trên một dòng. Số dòng khi đó chỉ gần đúng, nhưng không
    // import nào bị mất và các ô không dính vào nhau.
    let src = r#"{"cells": [{"cell_type": "code", "source": "import a\nimport b\n", "outputs": []}, {"cell_type": "code", "source": ["x = 1\n", "import c"], "outputs": []}], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#;
    let got: Vec<String> = imports("x.ipynb", src).into_iter().map(|(s, _)| s).collect();
    assert_eq!(got, ["a", "b", "c"]);
}

#[test]
fn notebook_khong_phai_python_thi_khong_doc() {
    let src = r#"{"cells": [{"cell_type": "code", "source": ["library(dplyr)\n", "import x\n"], "outputs": []}], "metadata": {"kernelspec": {"language": "R", "name": "ir"}}, "nbformat": 4, "nbformat_minor": 5}"#;
    assert!(imports("r.ipynb", src).is_empty());
}

#[test]
fn json_hong_thi_khong_doc_gi_va_khong_panic() {
    let bad_escape = r#"{"cells": [{"cell_type": "code", "source": ["\u00"]}]}"#;
    for bad in ["", "{", "{\"cells\": [", "[1, 2]", "{\"cells\": \"x\"}", "\u{feff}{\"cells\": []", bad_escape] {
        assert!(imports("bad.ipynb", bad).is_empty(), "{bad:?}");
    }
}

#[test]
fn ky_tu_thoat_trong_chuoi_json_duoc_giai() {
    let src = r#"{"cells": [{"cell_type": "code", "source": ["from pkg import \"x\" as y  # é\n", "import tab\tsep\n"], "outputs": []}], "metadata": {}, "nbformat": 4, "nbformat_minor": 5}"#;
    let got: Vec<String> = imports("e.ipynb", src).into_iter().map(|(s, _)| s).collect();
    assert_eq!(got, ["pkg", "tab"]);
}

// ---- Notebook là input không tin được ----
//
// archctl chạy trong hook của agent sau mỗi lần sửa file. Một notebook do
// người khác hay agent viết không được làm nó chết, treo, hay ngốn bộ nhớ —
// xem `.agent/threat-model.md`.

#[test]
fn json_long_nhau_rat_sau_khong_tran_stack() {
    let deep = format!("{{\"cells\": {}{}}}", "[".repeat(100_000), "]".repeat(100_000));
    assert!(source_view("deep.ipynb", &deep).is_empty());
}

#[test]
fn hang_tram_nghin_dong_trong_khong_khuech_dai_bo_nho() {
    let src = format!(
        "{{\"cells\": [{}{{\"cell_type\": \"code\", \"source\": [\"import far_away\\n\"]}}], \"metadata\": {{}}}}",
        "\n".repeat(200_000)
    );
    let view = source_view("far.ipynb", &src);
    assert!(view.len() <= src.len(), "đầu ra {} byte, file {} byte", view.len(), src.len());
    assert_eq!(imports("far.ipynb", &src), [("far_away".to_string(), 200_001)]);
}

#[test]
fn moi_tien_to_cua_notebook_va_cac_kieu_hong_khong_panic() {
    for end in 0..=NB.len() {
        if NB.is_char_boundary(end) {
            let _ = source_view("cut.ipynb", &NB[..end]);
        }
    }
    let broken = [
        r#"{"cells": [{"cell_type": "code", "source": ["\ud800"]}]}"#,
        r#"{"cells": [{"cell_type": "code", "source": ["\ud800A"]}]}"#,
        r#"{"cells": [{"cell_type": "code", "source": ["\udc00"]}]}"#,
        r#"{"cells": [{"cell_type": "code", "source": ["\u+041"]}]}"#,
        r#"{"cells": [{"cell_type": "code", "source": ["\x"]}]}"#,
        r#"{"cells": [{"cell_type": "code", "source": [1, null, {"a": []}, "import ok\n"]}]}"#,
        r#"{"cells": [{"cell_type": "code", "source": "import a\n"} {"x": 1}]}"#,
    ];
    for b in broken {
        let _ = source_view("broken.ipynb", b);
    }
    // Phần tử không phải chuỗi trong `source` thì bỏ, phần còn lại vẫn đọc.
    let mixed = broken[5];
    assert_eq!(imports("mixed.ipynb", mixed), [("ok".to_string(), 1)]);
}

#[test]
fn o_python_viet_bang_magic_van_duoc_doc() {
    // `%%python`, `%%script python3` chạy Python trong một tiến trình con; bỏ
    // qua chúng là chừa một lối đi tắt khỏi luật.
    let src = r#"{"cells": [
  {"cell_type": "code", "source": ["%%python3\n", "import via_python_magic\n"]},
  {"cell_type": "code", "source": ["%%script python\n", "import via_script\n"]},
  {"cell_type": "code", "source": ["%%script bash\n", "import not_python\n"]}
 ], "metadata": {}}"#;
    let got: Vec<String> = imports("magic.ipynb", src).into_iter().map(|(s, _)| s).collect();
    assert_eq!(got, ["via_python_magic", "via_script"]);
}

#[test]
fn dong_bat_dau_bang_phan_tram_trong_chuoi_ba_nhay_hay_ngoac_khong_phai_magic() {
    // Trong chuỗi `"""` dòng `%(id)s` là dữ liệu; trong ngoặc, `% b` là phép
    // chia lấy dư. Bỏ chúng đi thì trạng thái chuỗi lệch, và docstring ở ô sau
    // thành mã thật.
    let src = r#"{"cells": [
  {"cell_type": "code", "source": ["sql = \"\"\"\n", "SELECT 1\n", "%(id)s\"\"\"\n", "x = (a\n", "% b)\n"]},
  {"cell_type": "code", "source": ["def helper():\n", "    \"\"\"\n", "    import openai\n", "    \"\"\"\n", "import real\n"]}
 ], "metadata": {}}"#;
    let got: Vec<String> = imports("fp.ipynb", src).into_iter().map(|(s, _)| s).collect();
    assert_eq!(got, ["real"]);
}

#[test]
fn khoa_trung_lay_khoa_cuoi_nhu_python() {
    // `json.load` của Python, tức Jupyter, giữ khoá cuối. Lấy khoá đầu là chừa
    // một lối giấu ô code khỏi archctl.
    let src = r#"{"cells": [
  {"cell_type": "markdown", "cell_type": "code", "source": ["import hidden_by_dup_type\n"]},
  {"cell_type": "code", "source": ["x = 1\n"], "source": ["import hidden_by_dup_source\n"]}
 ], "metadata": {}}"#;
    let got: Vec<String> = imports("dup.ipynb", src).into_iter().map(|(s, _)| s).collect();
    assert_eq!(got, ["hidden_by_dup_type", "hidden_by_dup_source"]);
}

#[test]
fn script_python_viet_kieu_khac_van_la_python() {
    let src = r#"{"cells": [
  {"cell_type": "code", "source": ["%%script --bg --out log python3\n", "import a\n"]},
  {"cell_type": "code", "source": ["%%script /usr/bin/python3\n", "import b\n"]},
  {"cell_type": "code", "source": ["%%script env python3 -u\n", "import c\n"]},
  {"cell_type": "code", "source": ["\n", "%%bash\n", "import not_python\n"]}
 ], "metadata": {}}"#;
    let got: Vec<String> = imports("script.ipynb", src).into_iter().map(|(s, _)| s).collect();
    assert_eq!(got, ["a", "b", "c"]);
}
