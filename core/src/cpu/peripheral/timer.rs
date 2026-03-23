pub(crate) const DIV_ADDR: u16 = 0xFF04;
pub(crate) const TIMA_ADDR: u16 = 0xFF05;
pub(crate) const TMA_ADDR: u16 = 0xFF06;
pub(crate) const TAC_ADDR: u16 = 0xFF07;

pub(crate) const TIMER_INTERRUPT_BIT: u8 = 2;

/// Divisors (in T-cycles) for each TAC clock-select value.
const CLOCK_DIVISORS: [u16; 4] = [1024, 16, 64, 256];

/// Input snapshot passed to the timer each tick.
pub struct TimerInput {
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
}

/// Result of a timer tick — CPU writes these back to IO registers.
pub struct TimerOutput {
    pub tima: u8,
    pub div: u8,
    pub interrupt: bool,
}

/// Game Boy Timer peripheral.
///
/// Only owns the 16-bit internal counter (hidden hardware state).
/// TIMA, TMA, TAC, and DIV live in the IO register array owned by memory;
/// the CPU passes them in via `TimerInput` and writes back `TimerOutput`.
pub struct TimerPeripheral {
    internal_counter: u16,
}

impl TimerPeripheral {
    pub fn new() -> Self {
        Self {
            internal_counter: 0,
        }
    }

    /// Upper byte of the internal counter (the DIV register value).
    pub fn div(&self) -> u8 {
        (self.internal_counter >> 8) as u8
    }

    /// The full 16-bit internal counter. Used by the APU to synchronize
    /// its frame sequencer with bit 12 (DIV bit 4) falling edge.
    pub fn internal_counter(&self) -> u16 {
        self.internal_counter
    }

    /// Any write to DIV resets the internal counter to 0.
    pub fn reset_div(&mut self) {
        self.internal_counter = 0;
    }

    /// Advance the timer by `cycles` T-cycles.
    ///
    /// Pure transform: reads register state from `input`, returns new state
    /// in `TimerOutput`. The caller (CPU) writes the results back to memory.
    pub fn tick(&mut self, cycles: u16, input: TimerInput) -> TimerOutput {
        let timer_enabled = input.tac & 0x04 != 0;
        let divisor = CLOCK_DIVISORS[(input.tac & 0x03) as usize];
        let mut tima = input.tima;
        let mut interrupt = false;

        for _ in 0..cycles {
            let prev = self.internal_counter;
            self.internal_counter = self.internal_counter.wrapping_add(1);

            if timer_enabled {
                // TIMA increments on the falling edge of the relevant bit.
                let bit = divisor / 2;
                if prev & bit != 0 && self.internal_counter & bit == 0 {
                    let (new_tima, overflow) = tima.overflowing_add(1);
                    if overflow {
                        tima = input.tma;
                        interrupt = true;
                    } else {
                        tima = new_tima;
                    }
                }
            }
        }

        TimerOutput {
            tima,
            div: self.div(),
            interrupt,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tick_with(timer: &mut TimerPeripheral, cycles: u16, tac: u8, tima: u8, tma: u8) -> TimerOutput {
        timer.tick(cycles, TimerInput { tima, tma, tac })
    }

    // ── Initial state ──────────────────────────────────────────────────────────

    #[test]
    fn timer_new_has_zero_div() {
        let timer = TimerPeripheral::new();
        assert_eq!(timer.div(), 0);
    }

    // ── Timer disabled ─────────────────────────────────────────────────────────

    #[test]
    fn timer_disabled_tima_does_not_increment() {
        let mut timer = TimerPeripheral::new();
        let mut tima = 0u8;
        for _ in 0..2048 {
            let out = tick_with(&mut timer, 1, 0x00, tima, 0);
            tima = out.tima;
        }
        assert_eq!(tima, 0);
    }

    // ── TIMA increment rates ───────────────────────────────────────────────────

    #[test]
    fn timer_increments_tima_every_1024_cycles_at_clock_select_00() {
        let mut timer = TimerPeripheral::new();
        let out = tick_with(&mut timer, 1023, 0x04, 0, 0);
        assert_eq!(out.tima, 0, "not yet");
        let out = tick_with(&mut timer, 1, 0x04, out.tima, 0);
        assert_eq!(out.tima, 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_16_cycles_at_clock_select_01() {
        let mut timer = TimerPeripheral::new();
        let out = tick_with(&mut timer, 15, 0x05, 0, 0);
        assert_eq!(out.tima, 0, "not yet");
        let out = tick_with(&mut timer, 1, 0x05, out.tima, 0);
        assert_eq!(out.tima, 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_64_cycles_at_clock_select_10() {
        let mut timer = TimerPeripheral::new();
        let out = tick_with(&mut timer, 63, 0x06, 0, 0);
        assert_eq!(out.tima, 0, "not yet");
        let out = tick_with(&mut timer, 1, 0x06, out.tima, 0);
        assert_eq!(out.tima, 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_256_cycles_at_clock_select_11() {
        let mut timer = TimerPeripheral::new();
        let out = tick_with(&mut timer, 255, 0x07, 0, 0);
        assert_eq!(out.tima, 0, "not yet");
        let out = tick_with(&mut timer, 1, 0x07, out.tima, 0);
        assert_eq!(out.tima, 1, "should have incremented");
    }

    // ── Overflow and reload ────────────────────────────────────────────────────

    #[test]
    fn timer_overflow_reloads_tma() {
        let mut timer = TimerPeripheral::new();
        let out = tick_with(&mut timer, 16, 0x05, 0xFF, 0x42);
        assert_eq!(out.tima, 0x42);
    }

    #[test]
    fn timer_overflow_sets_interrupt_flag() {
        let mut timer = TimerPeripheral::new();
        let out = tick_with(&mut timer, 16, 0x05, 0xFF, 0x00);
        assert!(out.interrupt, "timer interrupt should be signalled");
    }

    #[test]
    fn timer_no_interrupt_without_overflow() {
        let mut timer = TimerPeripheral::new();
        let out = tick_with(&mut timer, 16, 0x05, 0x00, 0x00);
        assert!(!out.interrupt);
    }

    // ── DIV register ──────────────────────────────────────────────────────────

    #[test]
    fn div_reflects_upper_byte_of_internal_counter() {
        let mut timer = TimerPeripheral::new();
        let out = tick_with(&mut timer, 255, 0x00, 0, 0);
        assert_eq!(out.div, 0, "not yet");
        let out = tick_with(&mut timer, 1, 0x00, 0, 0);
        assert_eq!(out.div, 1);
    }

    #[test]
    fn div_reset_clears_internal_counter() {
        let mut timer = TimerPeripheral::new();
        tick_with(&mut timer, 256, 0x00, 0, 0);
        assert_eq!(timer.div(), 1);
        timer.reset_div();
        assert_eq!(timer.div(), 0);
    }

    #[test]
    fn div_reset_regardless_of_counter_value() {
        let mut timer = TimerPeripheral::new();
        tick_with(&mut timer, 512, 0x00, 0, 0);
        timer.reset_div();
        assert_eq!(timer.div(), 0);
    }
}
