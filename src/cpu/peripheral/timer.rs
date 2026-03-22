use std::cell::RefCell;
use std::rc::Rc;

use crate::cpu::peripheral::interrupt::InterruptController;
use crate::memory::memory::IoDevice;

const DIV_ADDR: u16 = 0xFF04;
const TIMA_ADDR: u16 = 0xFF05;
const TMA_ADDR: u16 = 0xFF06;
const TAC_ADDR: u16 = 0xFF07;

const TIMER_INTERRUPT_BIT: u8 = 2;

/// Divisors (in T-cycles) for each TAC clock-select value.
const CLOCK_DIVISORS: [u16; 4] = [1024, 16, 64, 256];

/// Game Boy Timer peripheral.
///
/// Owns DIV (0xFF04), TIMA (0xFF05), TMA (0xFF06), TAC (0xFF07).
/// `tick(cycles)` must be called after every instruction with the T-cycle cost.
/// Register reads/writes are routed through IoDevice, making this the single
/// source of truth for timer state.
pub struct TimerPeripheral {
    /// 16-bit internal counter. DIV register is its upper byte.
    internal_counter: u16,
    tima: u8,
    tma: u8,
    tac: u8,
    interrupt_controller: Rc<RefCell<InterruptController>>,
}

impl TimerPeripheral {
    pub fn new(interrupt_controller: Rc<RefCell<InterruptController>>) -> Self {
        Self {
            internal_counter: 0,
            tima: 0,
            tma: 0,
            tac: 0,
            interrupt_controller,
        }
    }

    /// Read a timer register.
    pub fn read(&self, address: u16) -> u8 {
        match address {
            DIV_ADDR => (self.internal_counter >> 8) as u8,
            TIMA_ADDR => self.tima,
            TMA_ADDR => self.tma,
            TAC_ADDR => self.tac,
            _ => 0,
        }
    }

    /// Write a timer register.
    pub fn write_register(&mut self, address: u16, value: u8) {
        match address {
            DIV_ADDR => {
                // Any write to DIV resets the internal counter to 0.
                self.internal_counter = 0;
            }
            TIMA_ADDR => {
                self.tima = value;
            }
            TMA_ADDR => {
                self.tma = value;
            }
            TAC_ADDR => {
                self.tac = value;
            }
            _ => {}
        }
    }

    /// Advance the timer by `cycles` T-cycles. Call once per CPU instruction.
    pub fn tick(&mut self, cycles: u16) {
        let timer_enabled = self.tac & 0x04 != 0;
        let divisor = CLOCK_DIVISORS[(self.tac & 0x03) as usize];

        for _ in 0..cycles {
            let prev = self.internal_counter;
            self.internal_counter = self.internal_counter.wrapping_add(1);

            if timer_enabled {
                // TIMA increments on the falling edge of the relevant bit.
                // Divisor/2 is the bit position; it falls 1→0 every `divisor` cycles.
                let bit = divisor / 2;
                if prev & bit != 0 && self.internal_counter & bit == 0 {
                    self.increment_tima();
                }
            }
        }
    }

    fn increment_tima(&mut self) {
        let (new_tima, overflow) = self.tima.overflowing_add(1);
        if overflow {
            self.tima = self.tma;
            self.interrupt_controller
                .borrow_mut()
                .request(TIMER_INTERRUPT_BIT);
        } else {
            self.tima = new_tima;
        }
    }
}

/// IoDevice wrapper so an `Rc<RefCell<TimerPeripheral>>` can be registered
/// on GameBoyMemory for addresses 0xFF04..=0xFF07.
pub struct SharedTimerDevice(pub Rc<RefCell<TimerPeripheral>>);

impl IoDevice for SharedTimerDevice {
    fn read(&self, address: u16) -> u8 {
        self.0.borrow().read(address)
    }

    fn write(&mut self, address: u16, value: u8) {
        self.0.borrow_mut().write_register(address, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_timer() -> (TimerPeripheral, Rc<RefCell<InterruptController>>) {
        let ic = Rc::new(RefCell::new(InterruptController::new()));
        let timer = TimerPeripheral::new(ic.clone());
        (timer, ic)
    }

    // ── Initial state ──────────────────────────────────────────────────────────

    #[test]
    fn timer_new_has_zeroed_registers() {
        let (timer, _ic) = make_timer();
        assert_eq!(timer.read(DIV_ADDR), 0);
        assert_eq!(timer.read(TIMA_ADDR), 0);
        assert_eq!(timer.read(TMA_ADDR), 0);
        assert_eq!(timer.read(TAC_ADDR), 0);
    }

    // ── Timer disabled ─────────────────────────────────────────────────────────

    #[test]
    fn timer_disabled_tima_does_not_increment() {
        let (mut timer, _ic) = make_timer();
        timer.write_register(TAC_ADDR, 0x00);
        for _ in 0..2048 {
            timer.tick(1);
        }
        assert_eq!(timer.read(TIMA_ADDR), 0);
    }

    // ── TIMA increment rates ───────────────────────────────────────────────────

    #[test]
    fn timer_increments_tima_every_1024_cycles_at_clock_select_00() {
        let (mut timer, _ic) = make_timer();
        timer.write_register(TAC_ADDR, 0x04); // enabled, /1024
        timer.tick(1023);
        assert_eq!(timer.read(TIMA_ADDR), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.read(TIMA_ADDR), 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_16_cycles_at_clock_select_01() {
        let (mut timer, _ic) = make_timer();
        timer.write_register(TAC_ADDR, 0x05); // enabled, /16
        timer.tick(15);
        assert_eq!(timer.read(TIMA_ADDR), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.read(TIMA_ADDR), 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_64_cycles_at_clock_select_10() {
        let (mut timer, _ic) = make_timer();
        timer.write_register(TAC_ADDR, 0x06); // enabled, /64
        timer.tick(63);
        assert_eq!(timer.read(TIMA_ADDR), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.read(TIMA_ADDR), 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_256_cycles_at_clock_select_11() {
        let (mut timer, _ic) = make_timer();
        timer.write_register(TAC_ADDR, 0x07); // enabled, /256
        timer.tick(255);
        assert_eq!(timer.read(TIMA_ADDR), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.read(TIMA_ADDR), 1, "should have incremented");
    }

    // ── Overflow and reload ────────────────────────────────────────────────────

    #[test]
    fn timer_overflow_reloads_tma() {
        let (mut timer, _ic) = make_timer();
        timer.write_register(TMA_ADDR, 0x42);
        timer.write_register(TIMA_ADDR, 0xFF);
        timer.write_register(TAC_ADDR, 0x05); // enabled, /16
        timer.tick(16); // one increment → overflow
        assert_eq!(timer.read(TIMA_ADDR), 0x42);
    }

    #[test]
    fn timer_overflow_requests_interrupt_bit_2() {
        let (mut timer, ic) = make_timer();
        ic.borrow_mut().set_ie(0xFF); // all interrupts enabled
        timer.write_register(TIMA_ADDR, 0xFF);
        timer.write_register(TAC_ADDR, 0x05); // enabled, /16
        timer.tick(16);
        assert!(ic.borrow().has_pending(), "timer interrupt should be pending");
        assert_eq!(
            ic.borrow_mut().take_pending(),
            Some(TIMER_INTERRUPT_BIT),
            "interrupt bit should be 2"
        );
    }

    #[test]
    fn timer_no_interrupt_without_overflow() {
        let (mut timer, ic) = make_timer();
        ic.borrow_mut().set_ie(0xFF);
        timer.write_register(TIMA_ADDR, 0x00);
        timer.write_register(TAC_ADDR, 0x05); // enabled, /16
        timer.tick(16); // TIMA goes 0 → 1, no overflow
        assert!(!ic.borrow().has_pending());
    }

    // ── DIV register ──────────────────────────────────────────────────────────

    #[test]
    fn div_reflects_upper_byte_of_internal_counter() {
        let (mut timer, _ic) = make_timer();
        timer.tick(255);
        assert_eq!(timer.read(DIV_ADDR), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.read(DIV_ADDR), 1);
    }

    #[test]
    fn div_write_resets_internal_counter_to_zero() {
        let (mut timer, _ic) = make_timer();
        timer.tick(256);
        assert_eq!(timer.read(DIV_ADDR), 1);
        timer.write_register(DIV_ADDR, 0x00); // any write resets
        assert_eq!(timer.read(DIV_ADDR), 0);
    }

    #[test]
    fn div_write_resets_regardless_of_written_value() {
        let (mut timer, _ic) = make_timer();
        timer.tick(512);
        timer.write_register(DIV_ADDR, 0xFF); // value is irrelevant
        assert_eq!(timer.read(DIV_ADDR), 0);
    }

    // ── TMA/TIMA writes ────────────────────────────────────────────────────────

    #[test]
    fn tma_write_updates_tma() {
        let (mut timer, _ic) = make_timer();
        timer.write_register(TMA_ADDR, 0xAB);
        assert_eq!(timer.read(TMA_ADDR), 0xAB);
    }

    #[test]
    fn tima_write_updates_tima() {
        let (mut timer, _ic) = make_timer();
        timer.write_register(TIMA_ADDR, 0x77);
        assert_eq!(timer.read(TIMA_ADDR), 0x77);
    }
}
