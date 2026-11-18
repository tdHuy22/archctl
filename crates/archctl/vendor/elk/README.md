# ELK (Eclipse Layout Kernel) cho `archctl serve`

Bố cục thứ hai của sơ đồ (nút **Bố cục: ELK**). Nhúng vào file chạy bằng
`include_str!` để `serve` không cần mạng; trang chỉ nạp `elk-api.js`, phần tính
toán chạy trong Web Worker (`elk-worker.min.js`) nên không đọc được khoá phiên.

| | |
|---|---|
| Gói | `elkjs` 0.12.1 trên npm (phát hành 2026-10-28) |
| Nguồn | https://registry.npmjs.org/elkjs/-/elkjs-0.12.1.tgz |
| Integrity npm | `sha512-IhMYKHE4S2vtx0QrhBP3lYOIKDeIiJXBrdq9MGYkuB8ZHqhrBnHNfSUndjXFewrX1CPK8chwq+2NA0K0hliYuw==` (đã khớp khi tải) |
| Giấy phép | EPL-2.0 (gói ghi `EPL-2.0 OR GPL-3.0-or-later`; archctl dùng theo EPL-2.0), xem `LICENSE.md` |

sha256 của từng file, có test kiểm (`tests/ui_html.rs` `elk_nhung_dung_ban_da_ghim`):

| File | sha256 |
|---|---|
| `elk-api.js` (`lib/elk-api.js`) | `ccca46175c05bbd280b5de6c9853103711e52c666a101f145b717e345363e24d` |
| `elk-worker.min.js` (`lib/elk-worker.min.js`) | `dca3a19d9ec37f5634bf5876b4ebd2455c3f43f0d32570136c13c13edaba9bfb` |
| `LICENSE.md` | `637e81f4a1b6b4079535c499fca05e238cf7605c1ff5b76d60d7da7ce96700c9` |

Các file chép nguyên, không sửa. Nâng bản: tải gói mới, so integrity với npm,
chép đè ba file, cập nhật bảng trên và hằng trong test.
