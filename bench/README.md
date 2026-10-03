# Bộ đo trên repo thật

```bash
python3 bench/bench.py
```

Một lệnh: dựng archctl bản release, tải mười repo ở đúng commit đã ghim trong
[repos.tsv](repos.tsv), rồi chạy hai phép thử trên từng repo. Chỉ cần Python 3
và git, không gói Python nào ngoài thư viện chuẩn.

Repo tải về `target/bench/repos/` (khoảng 170 MB), bản làm việc ở
`target/bench/work/`, kết quả chi tiết ở `target/bench/results.json`. Tất cả
nằm trong `target/`: ngoài git, bộ quét của archctl bỏ qua, `cargo clean` dọn.

## Hai phép thử, cả hai đều chặn

**Tự nhất quán** ([consistency.py](consistency.py)). `init --from-code` đọc mã
rồi sinh luật mô tả đúng hiện trạng. Nâng mọi thứ lên `#accepted`, `check` lại
chính mã đó: phải ra **0 vi phạm**. Mỗi vi phạm còn lại là một chỗ bộ khám phá
và bộ kiểm chứng nhìn cùng một đoạn mã mà thấy hai điều khác nhau — lỗi của
archctl, không phải của repo.

Và phải ra **0 cảnh báo trong file luật**: `init` khuyên chạy `check --strict`
trong CI, mà `--strict` coi cảnh báo của file luật là vi phạm. Luật vừa sinh ra
đã có cảnh báo thì lời khuyên đó đổ ngay lần chạy đầu tiên của người dùng mới.

**Luồng lệnh** ([flows.py](flows.py)). Mọi lệnh — `init`, `check` theo bốn
đường, `parse`, `export`, `baseline`, `propose`/`accept`/`block`/`reject`,
`diff`, `serve` — trên mã người khác viết. Test trong `crates/*/tests/` dựng dự
án nhỏ viết riêng để test; ở đây là thư mục vendor, symlink, file sinh tự động
mà fixture không nghĩ ra.

## Mã thoát

| | |
| --- | --- |
| 0 | cả hai phép thử đạt trên mọi repo |
| 1 | archctl sai: có bất nhất, có cảnh báo trong luật sinh ra, hoặc một luồng lệnh hỏng |
| 2 | không chạy tới được: không tải được repo, hoặc build hỏng |

Tách 1 khỏi 2 để một lần mạng chập chờn trên CI không trông giống một lỗi của
archctl.

## Vì sao ghim commit

Không ghim thì con số trôi theo upstream: chạy lại hôm sau cho kết quả khác mà
không biết do archctl đổi hay do repo đổi. Tải theo SHA, `--depth 1`, kiểm lại
`HEAD` sau khi checkout. Nâng một repo lên bản mới là sửa một dòng trong
`repos.tsv` — và là một thay đổi đáng nhìn riêng trong lịch sử git.

Git chạy với `core.hooksPath=/dev/null`, `core.fsmonitor=` và
`protocol.file.allow=never`: repo tải về là mã người khác, không để nó chạy gì
qua cấu hình git. Cùng danh sách mà archctl tự dùng.

## Thời gian chỉ in, không chặn

```bash
python3 bench/bench.py --only hugo,tokio --rounds 9
```

Trung vị của `--rounds` lần chạy, bỏ lần đầu vì bộ nhớ đệm hệ thống file. Trên
máy CI dùng chung con số nhiễu tới mức chặn theo nó chỉ tạo ra đỏ giả, nên CI
chạy với `--rounds 1` và không đọc cột thời gian.

## Mỗi phép kiểm phải đổ được

```bash
python3 bench/sabotage.py
```

Dựng một archctl giả, gọi archctl thật rồi làm hỏng đúng một chỗ: thêm một vi
phạm, thêm một cảnh báo vào luật sinh ra, báo quét 0 file, bỏ khoá
`lawWarnings`, giữ nguyên hash khi luật đổi, nói `diff` có thay đổi khi chỉ
thêm chú thích. Mỗi kiểu phải làm `bench.py` thoát 1, **và phải bị đúng phép
kiểm dành cho nó bắt** — không chỉ đỏ vì một lý do khác. CI chạy bước này sau
bộ đo.

Nó có mặt vì bộ đo này từng nằm ngoài git, và lúc đưa vào, đọc lại thì bốn
phép kiểm không kiểm gì: một cái chỉ xét file có tồn tại vì vế sau của `or`
không bao giờ được chạy tới, một cái chấp nhận `"0 "` ở bất cứ đâu trong đầu
ra, một cái gọi `export --mermaid` — cờ không tồn tại — mà không ai đọc mã
thoát, một cái đúng ra phải ghi thêm một dòng nhưng có thể xoá trắng file
luật. Rồi khi chuyển vào `target/`, bộ đếm file đếm ra 0 ở cả mười repo vì
đường dẫn tuyệt đối nào cũng chứa `target`. Giờ "0 vi phạm trên 0 file" là
một lỗi chứ không phải một lần đạt.

Một phép kiểm không thể đổ thì tệ hơn không có: nó vẫn cộng vào con số đạt.
