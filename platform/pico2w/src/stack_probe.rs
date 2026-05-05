#[cfg(feature = "stack-probe")]
mod imp {
    use core::ptr;
    use core::sync::atomic::{AtomicBool, Ordering};

    use cortex_m::register::{control, msp, psp};
    use defmt::{info, panic, warn};

    const STACK_SENTINEL: u8 = 0xA5;
    const PAINT_SAFETY_MARGIN_BYTES: usize = 512;
    const LOW_HEADROOM_WARN_BYTES: usize = 16 * 1024;
    const COLLISION_GUARD_BYTES: usize = 4 * 1024;

    static READ_BANK_STACK_LOGGING: AtomicBool = AtomicBool::new(false);
    static LOW_HEADROOM_WARNED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" {
        static __sheap: u8;
        static _stack_start: u8;
    }

    #[derive(Clone, Copy)]
    pub struct StackUsage {
        pub current_sp: usize,
        pub current_msp: usize,
        pub current_psp: usize,
        pub thread_uses_psp: bool,
        pub bottom: usize,
        pub top: usize,
        pub untouched_bytes: usize,
        pub peak_used_bytes: usize,
    }

    impl StackUsage {
        pub fn budget_bytes(&self) -> usize {
            self.top - self.bottom
        }

        pub fn headroom_bytes(&self) -> usize {
            self.untouched_bytes
        }
    }

    #[derive(Clone, Copy)]
    struct CurrentStackState {
        current_sp: usize,
        bottom: usize,
    }

    #[inline]
    fn bounds() -> (usize, usize) {
        unsafe { (&__sheap as *const u8 as usize, &_stack_start as *const u8 as usize) }
    }

    fn active_thread_sp() -> (usize, usize, usize, bool) {
        let msp = msp::read() as usize;
        let psp = psp::read() as usize;
        let thread_uses_psp = control::read().spsel().is_psp();
        let current_sp = if thread_uses_psp { psp } else { msp };
        (current_sp, msp, psp, thread_uses_psp)
    }

    fn current_state() -> CurrentStackState {
        let (bottom, _) = bounds();
        let (current_sp, _, _, _) = active_thread_sp();
        CurrentStackState { current_sp, bottom }
    }

    pub fn paint() {
        let (bottom, top) = bounds();
        let (current_sp, _, _, _) = active_thread_sp();
        let paint_end = current_sp
            .saturating_sub(PAINT_SAFETY_MARGIN_BYTES)
            .clamp(bottom, top);
        let len = paint_end.saturating_sub(bottom);

        if len == 0 {
            return;
        }

        unsafe {
            ptr::write_bytes(bottom as *mut u8, STACK_SENTINEL, len);
        }
    }

    pub fn snapshot() -> StackUsage {
        let (bottom, top) = bounds();
        let (current_sp, current_msp, current_psp, thread_uses_psp) = active_thread_sp();
        let mut probe = bottom;
        while probe < top {
            let byte = unsafe { ptr::read_volatile(probe as *const u8) };
            if byte != STACK_SENTINEL {
                break;
            }
            probe += 1;
        }

        let untouched_bytes = probe - bottom;
        StackUsage {
            current_sp,
            current_msp,
            current_psp,
            thread_uses_psp,
            bottom,
            top,
            untouched_bytes,
            peak_used_bytes: (top - bottom).saturating_sub(untouched_bytes),
        }
    }

    pub fn log(label: &'static str) {
        let usage = snapshot();
        info!(
            "stack {}: sp=0x{:08x} msp=0x{:08x} psp=0x{:08x} using_psp={} budget={}B peak={}B headroom={}B bottom=0x{:08x}",
            label,
            usage.current_sp,
            usage.current_msp,
            usage.current_psp,
            usage.thread_uses_psp,
            usage.budget_bytes(),
            usage.peak_used_bytes,
            usage.headroom_bytes(),
            usage.bottom,
        );
    }

    pub fn check_current_sp(label: &'static str) {
        let state = current_state();
        let current_margin = state.current_sp.saturating_sub(state.bottom);

        if current_margin <= COLLISION_GUARD_BYTES {
            panic!(
                "stack collision risk {}: sp=0x{:08x} bottom=0x{:08x} margin={}B",
                label,
                state.current_sp,
                state.bottom,
                current_margin,
            );
        }

        if current_margin <= LOW_HEADROOM_WARN_BYTES
            && !LOW_HEADROOM_WARNED.swap(true, Ordering::Relaxed)
        {
            warn!(
                "stack low headroom {}: sp=0x{:08x} bottom=0x{:08x} margin={}B",
                label,
                state.current_sp,
                state.bottom,
                current_margin,
            );
        }
    }

    pub fn log_read_bank_stack(bank: usize) {
        let usage = snapshot();
        info!(
            "read_bank {} stack: sp=0x{:08x} peak={}B headroom={}B",
            bank,
            usage.current_sp,
            usage.peak_used_bytes,
            usage.headroom_bytes(),
        );
    }

    pub fn set_read_bank_logging(enabled: bool) {
        READ_BANK_STACK_LOGGING.store(enabled, Ordering::Relaxed);
    }

    pub fn read_bank_logging_enabled() -> bool {
        READ_BANK_STACK_LOGGING.load(Ordering::Relaxed)
    }
}

#[cfg(not(feature = "stack-probe"))]
mod imp {
    pub fn paint() {}

    pub fn log(_label: &'static str) {}

    pub fn check_current_sp(_label: &'static str) {}

    pub fn log_read_bank_stack(_bank: usize) {}

    pub fn set_read_bank_logging(_enabled: bool) {}

    pub fn read_bank_logging_enabled() -> bool {
        false
    }
}

pub use imp::{
    check_current_sp,
    log,
    log_read_bank_stack,
    paint,
    read_bank_logging_enabled,
    set_read_bank_logging,
};
