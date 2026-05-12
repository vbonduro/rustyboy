pub(crate) const DIV_ADDR: u16 = 0xFF04;
pub(crate) const TIMA_ADDR: u16 = 0xFF05;
pub(crate) const TMA_ADDR: u16 = 0xFF06;
pub(crate) const TAC_ADDR: u16 = 0xFF07;

pub(crate) const TIMER_INTERRUPT_BIT: u8 = 2;

/// Divisors (in T-cycles) for each TAC clock-select value.
const CLOCK_DIVISORS: [u16; 4] = [1024, 16, 64, 256];

/// Game Boy Timer peripheral.
///
/// Owns the 16-bit internal counter and all timer IO registers
/// (TIMA, TMA, TAC). DIV is the upper byte of `internal_counter`.
pub struct TimerPeripheral {
    internal_counter: u16,
    tima: u8,
    tma: u8,
    tac: u8,
}

impl TimerPeripheral {
    pub fn new() -> Self {
        Self {
            internal_counter: 0,
            tima: 0,
            tma: 0,
            tac: 0,
        }
    }

    /// Upper byte of the internal counter (the DIV register value).
    pub fn div(&self) -> u8 {
        (self.internal_counter >> 8) as u8
    }

    pub fn tima(&self) -> u8 {
        self.tima
    }
    pub fn tma(&self) -> u8 {
        self.tma
    }
    pub fn tac(&self) -> u8 {
        self.tac
    }

    pub fn set_tima(&mut self, v: u8) {
        self.tima = v;
    }
    pub fn set_tma(&mut self, v: u8) {
        self.tma = v;
    }
    pub fn set_tac(&mut self, v: u8) {
        self.tac = v;
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

    /// Extract timer state into a [`TimerState`] for serialization.
    pub fn to_save_state(&self) -> crate::cpu::save_state::TimerState {
        crate::cpu::save_state::TimerState {
            internal_counter: self.internal_counter,
            tima: self.tima,
            tma: self.tma,
            tac: self.tac,
        }
    }

    /// Apply timer state from a parsed [`TimerState`].
    pub fn load_state(&mut self, state: crate::cpu::save_state::TimerState) {
        self.internal_counter = state.internal_counter;
        self.tima = state.tima;
        self.tma = state.tma;
        self.tac = state.tac;
    }

    /// Advance the timer by `cycles` T-cycles.
    ///
    /// Returns `true` if a timer interrupt was triggered.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn tick(&mut self, cycles: u16) -> bool {
        let prev = self.internal_counter;
        self.internal_counter = self.internal_counter.wrapping_add(cycles);

        let mut interrupt = false;

        if self.tac & 0x04 != 0 {
            let divisor = CLOCK_DIVISORS[(self.tac & 0x03) as usize];
            let n_edges = if self.internal_counter < prev {
                (65536u32 / divisor as u32 - prev as u32 / divisor as u32)
                    + self.internal_counter as u32 / divisor as u32
            } else {
                self.internal_counter as u32 / divisor as u32 - prev as u32 / divisor as u32
            };

            for _ in 0..n_edges {
                let (new_tima, overflow) = self.tima.overflowing_add(1);
                if overflow {
                    self.tima = self.tma;
                    interrupt = true;
                } else {
                    self.tima = new_tima;
                }
            }
        }

        interrupt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_timer(tac: u8, tima: u8, tma: u8) -> TimerPeripheral {
        let mut t = TimerPeripheral::new();
        t.set_tac(tac);
        t.set_tima(tima);
        t.set_tma(tma);
        t
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
        let mut timer = make_timer(0x00, 0, 0);
        for _ in 0..2048 {
            timer.tick(1);
        }
        assert_eq!(timer.tima(), 0);
    }

    // ── TIMA increment rates ───────────────────────────────────────────────────

    #[test]
    fn timer_increments_tima_every_1024_cycles_at_clock_select_00() {
        let mut timer = make_timer(0x04, 0, 0);
        timer.tick(1023);
        assert_eq!(timer.tima(), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.tima(), 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_16_cycles_at_clock_select_01() {
        let mut timer = make_timer(0x05, 0, 0);
        timer.tick(15);
        assert_eq!(timer.tima(), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.tima(), 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_64_cycles_at_clock_select_10() {
        let mut timer = make_timer(0x06, 0, 0);
        timer.tick(63);
        assert_eq!(timer.tima(), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.tima(), 1, "should have incremented");
    }

    #[test]
    fn timer_increments_tima_every_256_cycles_at_clock_select_11() {
        let mut timer = make_timer(0x07, 0, 0);
        timer.tick(255);
        assert_eq!(timer.tima(), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.tima(), 1, "should have incremented");
    }

    // ── Overflow and reload ────────────────────────────────────────────────────

    #[test]
    fn timer_overflow_reloads_tma() {
        let mut timer = make_timer(0x05, 0xFF, 0x42);
        timer.tick(16);
        assert_eq!(timer.tima(), 0x42);
    }

    #[test]
    fn timer_overflow_sets_interrupt_flag() {
        let mut timer = make_timer(0x05, 0xFF, 0x00);
        assert!(timer.tick(16), "timer interrupt should be signalled");
    }

    #[test]
    fn timer_no_interrupt_without_overflow() {
        let mut timer = make_timer(0x05, 0x00, 0x00);
        assert!(!timer.tick(16));
    }

    // ── DIV register ──────────────────────────────────────────────────────────

    #[test]
    fn div_reflects_upper_byte_of_internal_counter() {
        let mut timer = make_timer(0x00, 0, 0);
        timer.tick(255);
        assert_eq!(timer.div(), 0, "not yet");
        timer.tick(1);
        assert_eq!(timer.div(), 1);
    }

    #[test]
    fn div_reset_clears_internal_counter() {
        let mut timer = make_timer(0x00, 0, 0);
        timer.tick(256);
        assert_eq!(timer.div(), 1);
        timer.reset_div();
        assert_eq!(timer.div(), 0);
    }

    #[test]
    fn div_reset_regardless_of_counter_value() {
        let mut timer = make_timer(0x00, 0, 0);
        timer.tick(512);
        timer.reset_div();
        assert_eq!(timer.div(), 0);
    }
}
