// Read the cycle counter. Implementation is provided by the platform crate.
// On pico2w this reads the ARM DWT CYCCNT register.
// With LTO the call inlines to a single register read at zero extra cost.
extern "C" {
    fn perf_cycle_read() -> u32;
}

#[inline(always)]
pub fn cyccnt() -> u32 {
    unsafe { perf_cycle_read() }
}
