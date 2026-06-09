//! Stack-smashing-protector runtime support for `-Z stack-protector=all`
//! (CRASH_DEBUG_NOTES #5 hunt).
//!
//! `-Z stack-protector` makes the compiler place a canary (the value of
//! `__stack_chk_guard`) between a function's locals and its saved return
//! address, and check it on return; a contiguous buffer overrun that writes
//! *up* past the canary is caught at that function's epilogue, which calls
//! `__stack_chk_fail`. In `no_std` we must supply both symbols.
//!
//! Crucially this is **victim-independent**: it traps at the overrunning
//! function regardless of where the overflow lands, so it survives #5's
//! layout-sensitivity (which defeats every in-situ probe). It only catches
//! *contiguous array* overruns, though — a wild-pointer write would slip past.
//!
//! `__stack_chk_fail` reads `LR` (the return address into the function whose
//! canary was clobbered) before any `bl` clobbers it, logs it over RTT, then
//! panics so the crash handler records it. Map the logged LR to a symbol.

use core::sync::atomic::{AtomicUsize, Ordering};

/// Canary value the compiler stamps into each guarded frame. Fixed (not
/// randomized) — we only need detection, not security.
#[no_mangle]
pub static __stack_chk_guard: usize = 0x2B7E_1516;

static LAST_FAIL_LR: AtomicUsize = AtomicUsize::new(0);

pub fn last_fail_lr() -> usize {
    LAST_FAIL_LR.load(Ordering::Relaxed)
}

/// Called from a guarded function's epilogue when its canary was overwritten.
#[no_mangle]
#[inline(never)]
pub extern "C" fn __stack_chk_fail() -> ! {
    // Read LR first thing: on entry it holds the return address into the
    // function that detected the smash. `mov` does not clobber LR; we read it
    // before the `defmt::error!` call (a `bl`) would.
    let lr: usize;
    unsafe {
        core::arch::asm!("mov {}, lr", out(reg) lr, options(nomem, nostack, preserves_flags));
    }
    LAST_FAIL_LR.store(lr, Ordering::Relaxed);
    defmt::error!(
        "STACK-PROTECTOR: canary smashed; overrunning fn return addr LR={=usize:#010x}",
        lr
    );
    panic!("stack smashing detected (-Z stack-protector)");
}
