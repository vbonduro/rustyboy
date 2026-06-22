use core::sync::atomic::{AtomicU8, Ordering};

struct RpSpinlockCriticalSection;
critical_section::set_impl!(RpSpinlockCriticalSection);

const LOCK_UNOWNED: u8 = 0;
const LOCK_ALREADY_OWNED: u8 = 2;

// 0 = unowned, 1 = core 0, 2 = core 1.
static LOCK_OWNER: AtomicU8 = AtomicU8::new(LOCK_UNOWNED);

unsafe impl critical_section::Impl for RpSpinlockCriticalSection {
    unsafe fn acquire() -> u8 {
        let interrupts_active = cortex_m::register::primask::read().is_active();
        let core = rp_pac::SIO.cpuid().read() as u8 + 1;

        if LOCK_OWNER.load(Ordering::Acquire) == core {
            return LOCK_ALREADY_OWNED;
        }

        loop {
            cortex_m::interrupt::disable();
            core::sync::atomic::compiler_fence(Ordering::SeqCst);

            if rp_pac::SIO.spinlock(31).read() != 0 {
                LOCK_OWNER.store(core, Ordering::Relaxed);
                break;
            }

            if interrupts_active {
                cortex_m::interrupt::enable();
            }
        }

        interrupts_active as u8
    }

    unsafe fn release(token: u8) {
        if token == LOCK_ALREADY_OWNED {
            return;
        }

        LOCK_OWNER.store(LOCK_UNOWNED, Ordering::Relaxed);
        core::sync::atomic::compiler_fence(Ordering::SeqCst);
        rp_pac::SIO.spinlock(31).write_value(1);

        if token != 0 {
            cortex_m::interrupt::enable();
        }
    }
}
