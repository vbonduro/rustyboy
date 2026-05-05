#[cfg(feature = "stack-probe")]
mod imp {
    use core::ptr;
    use core::sync::atomic::{AtomicBool, Ordering};

    use cortex_m::register::{control, msp, psp};
    use defmt::{panic, warn};

    const STACK_SENTINEL: u8 = 0xA5;
    const PAINT_SAFETY_MARGIN_BYTES: usize = 512;
    const LOW_HEADROOM_WARN_BYTES: usize = 16 * 1024;
    const COLLISION_GUARD_BYTES: usize = 4 * 1024;

    static LOW_HEADROOM_WARNED: AtomicBool = AtomicBool::new(false);

    unsafe extern "C" {
        static __sheap: u8;
        static _stack_start: u8;
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

    fn active_thread_sp() -> usize {
        let msp = msp::read() as usize;
        let psp = psp::read() as usize;
        if control::read().spsel().is_psp() {
            psp
        } else {
            msp
        }
    }

    fn current_state() -> CurrentStackState {
        let (bottom, _) = bounds();
        CurrentStackState {
            current_sp: active_thread_sp(),
            bottom,
        }
    }

    pub fn paint() {
        let (bottom, top) = bounds();
        let paint_end = active_thread_sp()
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
}

#[cfg(not(feature = "stack-probe"))]
mod imp {
    pub fn paint() {}

    pub fn check_current_sp(_label: &'static str) {}
}

pub use imp::{check_current_sp, paint};
