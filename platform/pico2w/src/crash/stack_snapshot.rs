//! Stack window captured at fault time and carried across the reset.
//!
//! `commit_crash_and_reset` does not build a `CrashRecord` — it stashes state in
//! the watchdog/POWMAN scratch registers and resets, and the record is assembled
//! on the NEXT boot. There is no room in scratch for a stack window, so it lives
//! here in `.uninit`, which survives the reset the same way the scratch
//! registers do.
//!
//! For a wild-PC fault out of a `pop {rlist, pc}` this window spans the return
//! slot, which is what makes such a record diagnosable at all.
//!
//! LIMITATION: populated only for faults that reach a handler. A hardware
//! WATCHDOG reset has no capture point, so `WatchdogTimeout` records carry
//! `schema_ver = 2` with an all-zero window. The decoder cannot currently
//! distinguish "captured, all zeros" from "never captured".

/// `.uninit` backing store: 8 words used, 8 words of deliberate PADDING.
///
/// Core 1's MPU region 0 base is `_stack_end & !0x1F`, so up to 31 bytes of the
/// LAST `.uninit` object are privileged read-only for core 1. If that object is
/// written by core 1 it takes a MemManage fault that escalates to HardFault and
/// kills the core silently. The trailing 8 words absorb that if this array
/// happens to be placed last. The static assert below keeps the padding from
/// being "optimised" away by a future edit.
#[no_mangle]
#[link_section = ".uninit.CRASH_STACK_SNAP"]
pub static mut CRASH_STACK_SNAP: [usize; 16] = [0; 16];

/// Physical SRAM bounds on RP2350; reads are clamped to this range.
const SRAM_LO: usize = 0x2000_0000;
const SRAM_HI: usize = 0x2008_2000;

/// Index layout within the array.
const IDX_MAGIC: usize = 0;
const IDX_BASE: usize = 1;
const IDX_WORDS: usize = 2;

/// The padding described above must always exceed the 32-byte MPU granule.
const _: () = assert!((16 - (IDX_WORDS + CRASH_SNAP_WORDS)) * core::mem::size_of::<usize>() >= 32);
const CRASH_SNAP_MAGIC: usize = 0x5CAF_0001;

/// Number of stack words carried in a crash record. Must match
/// `CrashRecord::stack_snapshot`.
pub const CRASH_SNAP_WORDS: usize = 6;

/// Capture `CRASH_SNAP_WORDS` stack words starting at `base` (the PRE-FAULT
/// stack pointer) into `.uninit`, for the next boot to fold into the crash
/// record. The window therefore begins at the return-address slot, which is
/// what makes a wild-PC fault out of a `pop {rlist, pc}` diagnosable.
///
/// # Safety
/// Called from a fault/panic handler with interrupts effectively quiesced.
/// `base` is range-checked against the core-0 stack before any dereference, so
/// a wild SP cannot turn this into an out-of-bounds read.
pub unsafe fn capture_stack_window(base: usize) {
    let f = &raw mut CRASH_STACK_SNAP;
    (*f)[IDX_MAGIC] = CRASH_SNAP_MAGIC;
    (*f)[IDX_BASE] = base;
    // Clamp to physical SRAM. This is a "will not fault" check, NOT an
    // assertion that `base` is a valid stack address — a drifted or wild SP is
    // exactly the condition this instrument exists to record, so out-of-range
    // words are reported as zero rather than being allowed to fault a second
    // time inside the handler.
    let lo = SRAM_LO;
    let hi = SRAM_HI;
    for i in 0..CRASH_SNAP_WORDS {
        let a = base.wrapping_add(i * 4);
        (*f)[IDX_WORDS + i] = if a >= lo && a + 4 <= hi && a % 4 == 0 {
            core::ptr::read_volatile(a as *const usize)
        } else {
            0
        };
    }
}

/// Read back the snapshot captured before the last reset.
/// Returns `None` if no snapshot was recorded.
pub unsafe fn last_crash_stack() -> Option<(usize, [u32; CRASH_SNAP_WORDS])> {
    let f = &raw const CRASH_STACK_SNAP;
    if (*f)[IDX_MAGIC] != CRASH_SNAP_MAGIC {
        return None;
    }
    let base = (*f)[IDX_BASE];
    let mut w = [0u32; CRASH_SNAP_WORDS];
    for (i, out) in w.iter_mut().enumerate() {
        *out = (*f)[IDX_WORDS + i] as u32;
    }
    Some((base, w))
}

/// Clear the snapshot so a stale one cannot be folded into a later record.
///
/// # Safety
/// Call once at boot, after `last_crash_stack()` has been consumed.
pub unsafe fn clear_crash_stack() {
    let f = &raw mut CRASH_STACK_SNAP;
    (*f)[IDX_MAGIC] = 0;
}
