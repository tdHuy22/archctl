# Toolchain

## Rust

Cài ngày 2026-09-20 bằng trình cài chính thức, có cờ `--no-modify-path` nên
rustup **không** tự sửa file shell nào.

- Phiên bản: rustc 1.98.1, cargo 1.98.1 (stable, aarch64-apple-darwin)
- Vị trí: `~/.rustup` (toolchain) và `~/.cargo` (binary, cache)
- PATH: một khối đánh dấu `# RUST:START` ... `# RUST:END` trong `~/.zshrc`
- Bản lưu trước khi sửa: `~/.zshrc.bak-archctl`

### Gỡ sạch

```bash
rustup self uninstall          # xoá ~/.rustup và ~/.cargo
```

Rồi xoá khối `# RUST:START` ... `# RUST:END` trong `~/.zshrc`.
Sau hai bước đó máy không còn dấu vết Rust nào.

### Dọn cache mà vẫn giữ Rust

```bash
rm -rf ~/.cargo/registry        # cache crate tải về, cargo sẽ tải lại khi cần
```

## Node

v26.2.0, quản lý bằng nvm. Chỉ dùng để build giao diện web; người dùng cuối
không cần Node vì bundle được nhúng vào binary Rust.
