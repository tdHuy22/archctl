mod ethernet;

use crate::report::Summary;

pub struct Packet {
    pub name: &'static str,
}

pub fn summarize() -> Summary {
    Summary::default()
}
