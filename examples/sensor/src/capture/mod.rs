use crate::decode::Packet;

// Vi phạm: không có `allow Capture -> Report`. Capture phải đi qua Decode.
use crate::report::Summary;

// Lifetime và char literal: nếu lexer coi `'a` là mở chuỗi thì nó sẽ nuốt
// phần còn lại của file và bỏ sót dòng `use` phía trên.
pub fn label<'a>(p: &'a Packet, sep: char) -> &'a str {
    let _ = (sep, '\n');
    p.name
}

pub fn run() -> Summary {
    // Vi phạm: deny * -> RustUnsafe except Decode
    let n = unsafe { COUNT };
    let _ = n;
    Summary::default()
}

static mut COUNT: u64 = 0;
