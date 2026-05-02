/// Read the Cortex-M DWT cycle counter on ARM targets.
/// Returns 0 on non-ARM targets (tests, coverage, desktop builds).
#[cfg(target_arch = "arm")]
#[inline(always)]
pub fn cyccnt() -> u32 {
    // DWT CYCCNT register — must be enabled by the runtime before use.
    unsafe { (0xE000_1004u32 as *const u32).read_volatile() }
}

#[cfg(not(target_arch = "arm"))]
#[inline(always)]
pub fn cyccnt() -> u32 {
    0
}
