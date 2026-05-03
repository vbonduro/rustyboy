/// Enable the DWT cycle counter. Call once at startup before using `cyccnt()`.
/// On Cortex-M33 the counter is gated behind TRCENA in CoreDebug DEMCR and
/// the CYCCNTENA bit in DWT CTRL — neither is set by the Embassy runtime.
#[cfg(target_arch = "arm")]
#[inline(always)]
pub fn init_cyccnt() {
    unsafe {
        // Set TRCENA (bit 24) in CoreDebug DEMCR to enable the DWT block.
        let demcr = 0xE000_EDFCu32 as *mut u32;
        demcr.write_volatile(demcr.read_volatile() | (1 << 24));
        // Reset counter then set CYCCNTENA (bit 0) in DWT CTRL.
        (0xE000_1004u32 as *mut u32).write_volatile(0);
        let ctrl = 0xE000_1000u32 as *mut u32;
        ctrl.write_volatile(ctrl.read_volatile() | 1);
    }
}

#[cfg(not(target_arch = "arm"))]
#[inline(always)]
pub fn init_cyccnt() {}

/// Read the Cortex-M DWT cycle counter on ARM targets.
/// Returns 0 on non-ARM targets (tests, coverage, desktop builds).
#[cfg(target_arch = "arm")]
#[inline(always)]
pub fn cyccnt() -> u32 {
    unsafe { (0xE000_1004u32 as *const u32).read_volatile() }
}

#[cfg(not(target_arch = "arm"))]
#[inline(always)]
pub fn cyccnt() -> u32 {
    0
}
