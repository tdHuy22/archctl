//! Đồng hồ cho test đo độ phức tạp: thời gian CPU của chính luồng test, không
//! phải thời gian thực. Test chạy song song, và máy bận làm thời gian thực
//! dài ra dù code không chậm đi; thời gian CPU của luồng thì không.

use std::time::{Duration, Instant};

#[cfg(all(any(target_os = "linux", target_os = "macos"), target_pointer_width = "64"))]
fn thread_cpu() -> Option<Duration> {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, out: *mut Timespec) -> i32;
    }
    #[cfg(target_os = "linux")]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    #[cfg(target_os = "macos")]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 16;
    let mut t = Timespec { sec: 0, nsec: 0 };
    // SAFETY: `clock_gettime` chỉ ghi vào `t`, một `timespec` 64 bit hợp lệ.
    let ok = unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut t) } == 0;
    ok.then(|| Duration::new(t.sec as u64, t.nsec as u32))
}

#[cfg(not(all(any(target_os = "linux", target_os = "macos"), target_pointer_width = "64")))]
fn thread_cpu() -> Option<Duration> {
    None
}

/// Bắt đầu đo; `elapsed()` trả thời gian CPU của luồng, hay thời gian thực
/// khi nền tảng không có đồng hồ CPU theo luồng.
pub enum Clock {
    Cpu(Duration),
    Wall(Instant),
}

impl Clock {
    pub fn start() -> Clock {
        match thread_cpu() {
            Some(d) => Clock::Cpu(d),
            None => Clock::Wall(Instant::now()),
        }
    }

    pub fn elapsed(&self) -> Duration {
        match self {
            Clock::Cpu(d) => thread_cpu().unwrap_or(*d).saturating_sub(*d),
            Clock::Wall(t) => t.elapsed(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ngu_khong_tinh_lam_tinh() {
        let c = Clock::start();
        let mut x = 0u64;
        for i in 0..20_000_000u64 {
            x = std::hint::black_box(x.wrapping_add(i));
        }
        let busy = c.elapsed();
        let c = Clock::start();
        std::thread::sleep(Duration::from_millis(300));
        let idle = c.elapsed();
        assert!(busy > Duration::ZERO, "{busy:?}");
        if matches!(c, Clock::Cpu(_)) {
            assert!(idle < Duration::from_millis(100), "{idle:?}");
        }
    }
}
