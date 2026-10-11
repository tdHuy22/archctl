# Threat model

## 2026-10-11 · Đọc notebook Jupyter (`.ipynb`)
Luồng: file `.ipynb` bất kỳ trong repo đang quét (của người khác, hoặc agent vừa
viết) → `check.rs` / `discover.rs` / `scan` đọc file, bỏ nếu > 4 MB →
`scan::source_view` → `notebook::code_view` (bộ đọc JSON tự viết, đệ quy) →
chuỗi mã → lexer Python có sẵn → import, ký hiệu, cạnh. Không chạy mã, không
gọi mạng, không ghi file. Tài sản: archctl chạy được trong hook của agent và
trong CI (không crash, không treo, không ăn hết bộ nhớ), và luật kiến trúc
không bị lách bằng notebook.

| ID | Mối đe dọa | Actor | Surface | Asset | Chốt chặn phải có | Trạng thái |
|---|---|---|---|---|---|---|
| TM-1 | JSON lồng rất sâu (`[[[[…`) làm tràn stack của bộ đọc đệ quy, archctl chết trong hook và CI | mã bên thứ ba, agent | `scan/notebook.rs` `Reader::value` | khả năng chạy của `check` | giới hạn độ sâu `MAX_DEPTH`, quá thì trả rỗng; test 100 000 tầng `[` không panic | đã có: `scan/notebook.rs:174`; test `json_long_nhau_rat_sau_khong_tran_stack` |
| TM-2 | File nhỏ mà khuếch đại bộ nhớ: mã ở cuối sau hàng triệu dòng trống làm `Vec<String>` cỡ số dòng | mã bên thứ ba, agent | `notebook::code_view` | bộ nhớ, thời gian của hook | dựng đầu ra bằng một chuỗi nối dần, kích thước O(kích thước file); test 200 000 dòng trống | đã có: `scan/notebook.rs:29`, `:44`; test `hang_tram_nghin_dong_trong_khong_khuech_dai_bo_nho`. Test chỉ kiểm kích thước đầu ra, không phân biệt được với cách cũ; reviewer đo bộ nhớ đỉnh 72–85 MB với file 4 MB đối kháng (file `.py` cùng cỡ: 248 MB) |
| TM-3 | JSON hỏng hay cắt cụt làm panic (chỉ số vượt mảng, surrogate lẻ, UTF-8 sai) | mã bên thứ ba, agent | `Reader::string`, `unicode`, `hex4` | khả năng chạy của `check` | mọi truy cập qua `.get`, lỗi thì `None`; test mọi tiền tố của một notebook thật và vài kiểu hỏng không panic | đã có: `scan/notebook.rs:269`, `:285`; test `moi_tien_to_cua_notebook_va_cac_kieu_hong_khong_panic` |
| TM-4 | Lách luật bằng cách đặt import trong ô bị bỏ qua mà vẫn chạy Python (`%%python`, `%%script python`) | agent | `notebook::runs_python` | hiệu lực của luật kiến trúc | ô `%%python`, `%%python3`, `%%script python…` được đọc như Python; test. Phần còn lại (`!python -c`, `exec`) như với `.py`: archctl là lan can, không phải hàng rào an ninh (README, "Giới hạn đã biết") | đã có: `scan/notebook.rs:61`; test `o_python_viet_bang_magic_van_duoc_doc`; review 2026-10-11 thêm: khoá JSON trùng lấy khoá cuối (`get`), `%%script --bg python3`, `/usr/bin/python3`, `env python3`, dòng trống đầu ô; test `khoa_trung_lay_khoa_cuoi_nhu_python`, `script_python_viet_kieu_khac_van_la_python` |

## 2026-10-11 · Thước đo notebook bằng IPython trong CI, và đọc `setup.py`
Luồng: (1) CI, bước `oracle.py --require ipython` và `sabotage.py --notebook-repo`
→ pip tải IPython cùng 16 gói từ PyPI vào venv dưới `target/bench/ipython-oracle/`
→ `notebook_truth.py` chạy trong venv đó, cạnh mã nguồn private của repo. Job chỉ
có quyền `contents: read`, không secret, `persist-credentials: false`.
(2) archctl đọc mọi `setup.py` ở các thư mục project Python (≤ 1 MB mỗi file)
→ lexer Python → `parse_setup` → tên extension và danh sách nguồn → ghép cạnh.
Đường này chạy cả trong hook `check <file>`. Tài sản: mã nguồn private không lọt
ra ngoài, kết luận của oracle đáng tin, hook không treo.

| ID | Mối đe dọa | Actor | Surface | Asset | Chốt chặn phải có | Trạng thái |
|---|---|---|---|---|---|---|
| TM-5 | Gói PyPI bị chiếm quyền hay bị tráo trên đường đi chạy trong job CI: gửi mã nguồn private ra ngoài, hoặc làm oracle "khớp" giả | mã bên thứ ba | `bench/oracle.py` `build_ipython_helper`, `bench/ipython-requirements.txt` | mã nguồn private; độ tin của CI | ghim `==` kèm sha256 mọi file, `--require-hashes --only-binary :all: --no-deps` (không `setup.py` nào chạy lúc cài); job chỉ đọc, không secret | đã có: `bench/oracle.py:664-666` (`build_ipython_helper`); `bench/ipython-requirements.txt` (17 gói, 63 hash); ci.yml `permissions: contents: read`. Review 2026-10-11 cài thử đúng lệnh đó: đạt. Chưa có test tự động cho nhánh pip từ chối hash sai |
| TM-6 | `setup.py` cố tình lồng lời gọi rất sâu làm bộ đọc chạy bậc hai, treo hook | mã bên thứ ba, agent | `resolve.rs` `parse_setup`, `call_args` | khả năng chạy của `check` | ghép cặp ngoặc một lượt bằng stack, `call_args` nhảy qua nhóm lồng; ngưỡng 1 MB mỗi file; test 60 000 tầng dưới 5 giây | đã có: `resolve.rs:1418` (ghép ngoặc), `:1438` (`call_args`), tra tên và nguồn bằng bảng băm (`:1275`), `read_bounded` `:1352` (chỉ file thường, đọc tối đa ngưỡng), trần tổng 8 MB `:1283`; test `setup_py_long_nhau_sau_khong_lam_treo`, `setup_py_nhieu_ten_extension_khong_lam_cham_hook` (50 000 tên), `setup_py_tro_toi_thiet_bi_khong_bi_doc_mai` (`/dev/zero`) |

## 2026-10-11 · torch và pytest trong CI cho `verify model_arch`
Luồng: CI, job `kiến trúc mô hình` → pip tải torch (Linux: bản `+cpu` từ
download.pytorch.org; macOS: PyPI) cùng 14 gói từ PyPI vào `target/ml-venv` →
`archctl verify model_arch` trong `examples/ml` chạy `python3 -m pytest` của venv,
import torch, cạnh mã nguồn private của repo. Job chỉ có quyền `contents: read`,
không secret, `persist-credentials: false`. Tài sản: mã nguồn private không lọt ra
ngoài, kết luận "kiến trúc khớp bản đã duyệt" đáng tin.

| ID | Mối đe dọa | Actor | Surface | Asset | Chốt chặn phải có | Trạng thái |
|---|---|---|---|---|---|---|
| TM-7 | Gói bị chiếm quyền, bị tráo trên đường đi, hay trùng tên trên index thứ hai (download.pytorch.org) chạy trong job: gửi mã nguồn ra ngoài, hoặc làm `verify` đạt giả | mã bên thứ ba | `examples/ml/requirements-test.txt`, ci.yml job `mo-hinh` | mã nguồn private; độ tin của CI | ghim `==` kèm sha256 mọi file, `--require-hashes --only-binary :all: --no-deps` (không `setup.py` nào chạy, index nào cũng chỉ trả được đúng file đã ghim); chỉ nhận bản đã ra ít nhất hai tuần (`--exclude-newer 2026-09-27`); job chỉ đọc, không secret | đã có: `examples/ml/requirements-test.txt:16-208` (17 mục, 174 hash, option duy nhất là `--extra-index-url` ở `:16`); `.github/workflows/ci.yml:233-235` (cờ pip), `:22-23` (`contents: read`, job không ghi đè), `:219` (`persist-credentials: false`). Review 2026-10-11 (opus): đối chiếu từng hash với PyPI JSON và index PyTorch, khớp đủ; file mới nhất upload 2026-09-26; pip bỏ file cùng tên khác hash từ index thứ hai. Kiểm tay 2026-10-11: sửa hash của iniconfig thì pip dừng với "DO NOT MATCH THE HASHES", mã 1; cài đủ trên macOS arm64 Python 3.11 bằng pip, và cho Linux x86_64 Python 3.12 bằng `uv pip install --python-platform` (bước pip thật trên runner Linux chỉ CI kiểm được) |
