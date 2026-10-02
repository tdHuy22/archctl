// Phụ thuộc vào native/parser.c đi qua FFI, không qua `use`, nên bộ kiểm
// chứng KHÔNG nhìn thấy cạnh Decode -> Native. Xem docs/check.md.
unsafe extern "C" {
    fn parse_ethernet(buf: *const u8, len: usize) -> i32;
}

pub fn parse(buf: &[u8]) -> i32 {
    unsafe { parse_ethernet(buf.as_ptr(), buf.len()) }
}
