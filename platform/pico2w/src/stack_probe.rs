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
        unsafe {
            (
                &__sheap as *const u8 as usize,
                &_stack_start as *const u8 as usize,
            )
        }
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
                label, state.current_sp, state.bottom, current_margin,
            );
        }

        if current_margin <= LOW_HEADROOM_WARN_BYTES
            && !LOW_HEADROOM_WARNED.swap(true, Ordering::Relaxed)
        {
            warn!(
                "stack low headroom {}: sp=0x{:08x} bottom=0x{:08x} margin={}B",
                label, state.current_sp, state.bottom, current_margin,
            );
        }
    }

    /// Paint the bottom `guard_bytes` of an arbitrary stack region with the
    /// sentinel pattern.  Call this before the stack's owning thread starts.
    ///
    /// # Safety
    /// `bottom` must be a valid, writable pointer to at least `guard_bytes`
    /// bytes that are not concurrently accessed by any other thread.
    pub unsafe fn paint_region(bottom: *mut u8, guard_bytes: usize) {
        unsafe { ptr::write_bytes(bottom, STACK_SENTINEL, guard_bytes) };
    }

    /// Verify that the bottom `guard_bytes` of a stack region still contain
    /// the sentinel pattern.  Panics (via defmt) on the first corrupted byte.
    ///
    /// # Safety
    /// `bottom` must be a valid pointer to at least `guard_bytes` bytes.
    pub unsafe fn check_region(bottom: *const u8, guard_bytes: usize, label: &'static str) {
        for i in 0..guard_bytes {
            let byte = unsafe { bottom.add(i).read_volatile() };
            if byte != STACK_SENTINEL {
                panic!(
                    "stack overflow {}: sentinel corrupted at +{}B (addr=0x{:08x})",
                    label,
                    i,
                    bottom as usize + i,
                );
            }
        }
    }

    /// Peak bytes ever used in a painted, downward-growing stack region.
    ///
    /// Scans up from `bottom` (the stack limit) counting intact sentinel bytes;
    /// the first disturbed byte marks the deepest the stack ever reached.  Peak
    /// usage is therefore `size - <leading sentinel bytes>`.
    ///
    /// # Safety
    /// `[bottom, bottom + size)` must have been painted by [`paint_region`] (or
    /// [`paint`]) and be readable.  Pass the *whole* region for a true peak.
    pub unsafe fn region_high_water(bottom: *const u8, size: usize) -> usize {
        let mut free = 0;
        while free < size && unsafe { bottom.add(free).read_volatile() } == STACK_SENTINEL {
            free += 1;
        }
        size - free
    }

    /// Peak bytes ever used by core 0's stack, measured over the region painted
    /// by [`paint`] (bottom = `__sheap`/`_stack_end`, top = pre-paint SP).
    pub fn high_water_core0() -> usize {
        let (bottom, top) = bounds();
        // Safety: paint() sentinels [bottom, sp-margin); scanning up from bottom
        // stays inside that painted span until the deepest real frame.
        unsafe { region_high_water(bottom as *const u8, top - bottom) }
    }
}

#[cfg(not(feature = "stack-probe"))]
mod imp {
    pub fn paint() {}

    pub fn check_current_sp(_label: &'static str) {}

    pub unsafe fn paint_region(_bottom: *mut u8, _guard_bytes: usize) {}

    pub unsafe fn check_region(_bottom: *const u8, _guard_bytes: usize, _label: &'static str) {}

    pub unsafe fn region_high_water(_bottom: *const u8, _size: usize) -> usize {
        0
    }

    pub fn high_water_core0() -> usize {
        0
    }
}

pub use imp::{
    check_current_sp, check_region, high_water_core0, paint, paint_region, region_high_water,
};
