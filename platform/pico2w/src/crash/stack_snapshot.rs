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

/// `.uninit` backing store: 8 words used, 8 words of PADDING.
///
/// The padding is slack, NOT a safety mechanism — this array is not the last
/// `.uninit` object (`DMA_CRASH_SNAPSHOT` is, as of this writing), so padding
/// here protects nothing. An earlier version of this comment claimed it fenced
/// the "last `.uninit` object lands inside core 1's MPU region" hazard; it did
/// not, and could not, because which object is last is a link-order accident.
///
/// That hazard is now fixed at its source: `setup_core1_mpu` rounds core 1's
/// region base UP to the MPU granule, so the region can never start below
/// `__euninit` and no `.uninit` object is reachable by it. See `mpu.rs`.
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

/// The array must actually be big enough for the header plus the window.
const _: () = assert!(IDX_WORDS + CRASH_SNAP_WORDS <= 16);
const CRASH_SNAP_MAGIC: usize = 0x5CAF_0001;

/// Number of stack words carried in a crash record. Must match
/// `CrashRecord::stack_snapshot`.
pub const CRASH_SNAP_WORDS: usize = 6;

/// Capture `CRASH_SNAP_WORDS` stack words starting at `base` (the PRE-FAULT
/// stack pointer) into `.uninit`, for the next boot to fold into the crash
/// record. On the FAULT path the window therefore begins at the return-address
/// slot, which is what makes a wild-PC fault out of a `pop {rlist, pc}`
/// diagnosable. On the PANIC path `base` is the MSP inside the panic handler,
/// so the window starts on handler locals instead — panics carry file and line,
/// so this costs little, but the window means something different there.
///
/// # Safety
/// Called from a fault/panic handler with interrupts effectively quiesced.
/// `base` is clamped to physical SRAM before any dereference, so a wild SP
/// cannot fault here. That is a "will not fault" bound, NOT an assertion that
/// `base` points at a stack — see the inline note in the body.
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
