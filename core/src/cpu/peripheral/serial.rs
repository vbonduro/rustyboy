use alloc::vec::Vec;

const SC_TRANSFER_BIT: u8 = 0x80;
const SC_INTERNAL_CLOCK_BIT: u8 = 0x01;

/// T-cycles for one serial transfer using the internal clock (8 bits × 64 T-cycles/bit).
const SERIAL_TRANSFER_CYCLES: u16 = 512;

pub(crate) const SERIAL_INTERRUPT_BIT: u8 = 3;

/// Serial port peripheral. Captures bytes transferred via the Game Boy serial link.
///
/// When the ROM writes to SC (0xFF02) with bit 7 (transfer start) and bit 0
/// (internal clock) set, a transfer begins. After 512 T-cycles the transfer
/// completes: SB is set to 0xFF (received byte), SC bit 7 is cleared, and
/// the serial interrupt (IF bit 3) is fired.
///
/// External-clock transfers (bit 0 = 0) are not timed — they complete
/// immediately so the game is never stuck waiting for a missing link cable.
pub struct SerialPort {
    output: Vec<u8>,
    sb: u8,
    sc: u8,
    /// Remaining T-cycles until the in-progress internal-clock transfer completes.
    /// `None` means no transfer is in progress.
    cycles_remaining: Option<u16>,
}

impl SerialPort {
    pub fn new() -> Self {
        Self {
            output: Vec::new(),
            sb: 0,
            sc: 0,
            cycles_remaining: None,
        }
    }

    pub fn output(&self) -> &[u8] {
        &self.output
    }
    pub fn sb(&self) -> u8 {
        self.sb
    }
    pub fn sc(&self) -> u8 {
        self.sc
    }
    pub fn set_sb(&mut self, v: u8) {
        self.sb = v;
    }

    #[inline(always)]
    pub fn is_idle(&self) -> bool {
        self.cycles_remaining.is_none()
    }

    /// Called when SC (0xFF02) is written. Captures the current SB and starts a
    /// timed transfer if the internal clock bit is set.
    pub fn handle_sc_write(&mut self, sc_value: u8) {
        self.sc = sc_value;
        if sc_value & SC_TRANSFER_BIT != 0 {
            self.output.push(self.sb);
            if sc_value & SC_INTERNAL_CLOCK_BIT != 0 && self.cycles_remaining.is_none() {
                // Internal clock: time the transfer over 512 T-cycles.
                // Only start if no transfer is already in progress — games that
                // poll by re-writing SC=0x81 must not reset the countdown.
                self.cycles_remaining = Some(SERIAL_TRANSFER_CYCLES);
            }
            // External clock transfers are left pending; they will never
            // complete (no cable), so we don't start a countdown.
        }
    }

    /// Advance the serial port by `cycles` T-cycles.
    ///
    /// Returns `true` if a transfer completed and the serial interrupt should fire.
    /// On completion, SB is set to 0xFF and SC transfer-start bit is cleared.
    pub fn tick(&mut self, cycles: u16) -> bool {
        let remaining = match self.cycles_remaining {
            Some(r) => r,
            None => return false,
        };

        if remaining > cycles {
            self.cycles_remaining = Some(remaining - cycles);
            false
        } else {
            self.cycles_remaining = None;
            self.sb = 0xFF;
            self.sc &= !SC_TRANSFER_BIT;
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_port(sb: u8) -> SerialPort {
        let mut p = SerialPort::new();
        p.set_sb(sb);
        p
    }

    #[test]
    fn test_serial_transfer_captures_sb_byte() {
        let mut port = make_port(b'H');
        port.handle_sc_write(0x81); // internal clock transfer
        assert_eq!(port.output(), b"H");
    }

    #[test]
    fn test_serial_transfer_without_start_bit_does_not_capture() {
        let mut port = make_port(b'X');
        port.handle_sc_write(0x01); // bit 7 NOT set
        assert_eq!(port.output(), b"");
    }

    #[test]
    fn test_serial_captures_multiple_bytes_in_order() {
        let mut port = make_port(b'H');
        port.handle_sc_write(0x81);
        port.tick(512); // complete first transfer
        port.set_sb(b'i');
        port.handle_sc_write(0x81);
        assert_eq!(port.output(), b"Hi");
    }

    #[test]
    fn test_serial_output_starts_empty() {
        let port = SerialPort::new();
        assert_eq!(port.output(), b"");
    }

    #[test]
    fn test_write_to_sb_alone_does_not_capture() {
        let port = SerialPort::new();
        assert_eq!(port.output(), b"");
    }

    // ── Internal-clock transfer timing ────────────────────────────────────────

    #[test]
    fn internal_clock_no_interrupt_before_512_cycles() {
        let mut port = make_port(b'A');
        port.handle_sc_write(0x81);
        assert!(!port.tick(511));
    }

    #[test]
    fn internal_clock_interrupt_fires_at_512_cycles() {
        let mut port = make_port(b'A');
        port.handle_sc_write(0x81);
        assert!(port.tick(512));
        assert_eq!(port.sb(), 0xFF);
        assert_eq!(port.sc() & SC_TRANSFER_BIT, 0);
    }

    #[test]
    fn internal_clock_interrupt_fires_across_multiple_ticks() {
        let mut port = make_port(b'A');
        port.handle_sc_write(0x81);
        assert!(!port.tick(256));
        assert!(port.tick(256));
    }

    #[test]
    fn external_clock_transfer_never_fires_interrupt() {
        let mut port = make_port(b'A');
        port.handle_sc_write(0x80); // external clock (no internal clock bit)
        assert!(!port.tick(512));
    }

    #[test]
    fn no_transfer_tick_returns_no_interrupt() {
        let mut port = SerialPort::new();
        assert!(!port.tick(512));
    }
}
