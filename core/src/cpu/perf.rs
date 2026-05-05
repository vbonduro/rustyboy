// Read the cycle counter. Bare-metal platforms provide the implementation,
// while host builds use a cheap monotonic fallback so tests and coverage can
// link with `perf` enabled.
#[cfg(target_os = "none")]
extern "C" {
    fn perf_cycle_read() -> u32;
}

#[cfg(target_os = "none")]
#[inline(always)]
pub fn cyccnt() -> u32 {
    unsafe { perf_cycle_read() }
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn cyccnt() -> u32 {
    use core::sync::atomic::{AtomicU32, Ordering};

    static HOST_CYCLE_COUNTER: AtomicU32 = AtomicU32::new(0);
    HOST_CYCLE_COUNTER.fetch_add(1, Ordering::Relaxed)
}
