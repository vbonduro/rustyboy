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

/// `.uninit` backing store. Padded well clear of `_stack_end` so its tail cannot
/// land inside core 1's read-only MPU region (see `heartbeat` for the same
/// hazard, which silently killed core 1 when it was first hit).
#[no_mangle]
#[link_section = ".uninit.CRASH_STACK_SNAP"]
pub static mut CRASH_STACK_SNAP: [usize; 16] = [0; 16];

const CRASH_SNAP_BASE: usize = 0;
const CRASH_SNAP_MAGIC: usize = 0x5CAF_0001;

/// Number of stack words carried in a crash record. Must match
/// `CrashRecord::stack_snapshot`.
pub const CRASH_SNAP_WORDS: usize = 6;

/// Capture `CRASH_SNAP_WORDS` stack words starting at `base` into `.uninit`,
/// for the next boot to fold into the crash record.
///
/// # Safety
/// Called from a fault/panic handler with interrupts effectively quiesced.
/// `base` is range-checked against the core-0 stack before any dereference, so
/// a wild SP cannot turn this into an out-of-bounds read.
pub unsafe fn capture_crash_stack(base: usize) {
    let f = &raw mut CRASH_STACK_SNAP;
    (*f)[CRASH_SNAP_BASE] = CRASH_SNAP_MAGIC;
    (*f)[CRASH_SNAP_BASE + 1] = base;
    // Only read what is provably inside the core-0 stack. A drifted or wild SP
    // is exactly the condition this instrument exists to record, so it must not
    // fault while recording it.
    let lo = 0x2000_0000usize;
    let hi = 0x2008_2000usize;
    for i in 0..CRASH_SNAP_WORDS {
        let a = base.wrapping_add(i * 4);
        (*f)[CRASH_SNAP_BASE + 2 + i] = if a >= lo && a + 4 <= hi && a % 4 == 0 {
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
    if (*f)[CRASH_SNAP_BASE] != CRASH_SNAP_MAGIC {
        return None;
    }
    let base = (*f)[CRASH_SNAP_BASE + 1];
    let mut w = [0u32; CRASH_SNAP_WORDS];
    for (i, out) in w.iter_mut().enumerate() {
        *out = (*f)[CRASH_SNAP_BASE + 2 + i] as u32;
    }
    Some((base, w))
}

/// Clear the snapshot so a stale one cannot be folded into a later record.
///
/// # Safety
/// Call once at boot, after `last_crash_stack()` has been consumed.
pub unsafe fn clear_crash_stack() {
    let f = &raw mut CRASH_STACK_SNAP;
    (*f)[CRASH_SNAP_BASE] = 0;
}
