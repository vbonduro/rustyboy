//! Crate-wide access to the single watchdog peripheral.
//!
//! The watchdog is a hardware singleton with exactly one owner and one
//! configured window, and every feed site wants the same thing: "I am still
//! making progress, give me the standard window again." Threading a
//! `&mut Watchdog` down to each of those sites made the window an argument
//! that every intermediate signature had to carry and every caller had to get
//! right — several already passed bare literals that had drifted from the
//! configured value.
//!
//! Binding it here instead means a feed site needs no access path and cannot
//! choose the wrong window.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, Ordering};

use critical_section::Mutex;
use embassy_rp::watchdog::Watchdog;
use embassy_time::Duration;

static WATCHDOG: Mutex<RefCell<Option<Watchdog>>> = Mutex::new(RefCell::new(None));

/// The window `feed()` re-arms with, in milliseconds. Set once by `init`.
static WINDOW_MS: AtomicU32 = AtomicU32::new(0);

/// Take ownership of the watchdog, arm it, and publish it to the crate.
///
/// Call once, early in `main`. Feeds before this point are silently ignored,
/// which is correct: nothing is armed yet, so there is nothing to starve.
pub fn init(mut watchdog: Watchdog, window: Duration) {
    watchdog.start(window);
    // Pause the watchdog while a debugger has the cores halted. embassy's
    // `start()` clears the pause-on-debug bits, so without this the watchdog
    // fires during a probe-rs flash or GDB halt — resetting the chip
    // mid-operation (mid-flash page-write timeouts; GDB "core is running"
    // fatal errors). No effect in production (no debugger => never paused).
    watchdog.pause_on_debug(true);

    WINDOW_MS.store(window.as_millis() as u32, Ordering::Relaxed);
    critical_section::with(|cs| {
        WATCHDOG.borrow(cs).replace(Some(watchdog));
    });
}

/// Re-arm with the configured window. The call every progress point wants.
pub fn feed() {
    feed_for(Duration::from_millis(u64::from(
        WINDOW_MS.load(Ordering::Relaxed),
    )));
}

/// Re-arm with a one-off window that differs from the configured one.
///
/// For a single synchronous operation whose timing genuinely differs from the
/// main loop's — a flash erase needs a longer window, a bank write a tighter
/// one. Prefer `feed()` everywhere else: a window chosen per call site is a
/// window that drifts.
pub fn feed_for(window: Duration) {
    critical_section::with(|cs| {
        if let Some(w) = WATCHDOG.borrow(cs).borrow_mut().as_mut() {
            w.feed(window);
        }
    });
}

/// Arm a short window and spin, so the watchdog resets the chip.
///
/// Used to reboot into a newly staged ROM. Diverges: the reset lands before
/// the loop can make progress.
pub fn force_reset() -> ! {
    critical_section::with(|cs| {
        if let Some(w) = WATCHDOG.borrow(cs).borrow_mut().as_mut() {
            w.start(Duration::from_millis(100));
        }
    });
    loop {
        cortex_m::asm::nop();
    }
}
