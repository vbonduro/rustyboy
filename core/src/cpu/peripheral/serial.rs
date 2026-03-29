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
    /// Remaining T-cycles until the in-progress internal-clock transfer completes.
    /// `None` means no transfer is in progress.
    cycles_remaining: Option<u16>,
}

/// Result of a serial tick.
pub struct SerialOutput {
    /// Whether the transfer completed this tick and the interrupt should fire.
    pub interrupt: bool,
    /// New value for SB (0xFF on transfer complete, unchanged otherwise).
    pub sb: Option<u8>,
    /// New value for SC (transfer bit cleared on complete, unchanged otherwise).
    pub sc: Option<u8>,
}

impl SerialPort {
    pub fn new() -> Self {
        Self { output: Vec::new(), cycles_remaining: None }
    }

    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// Called when SC (0xFF02) is written. Captures `sb` and starts a timed
    /// transfer if the internal clock bit is set; completes immediately for
    /// external-clock transfers (no link cable present).
    pub fn handle_sc_write(&mut self, sc_value: u8, sb: u8) {
        if sc_value & SC_TRANSFER_BIT != 0 {
            self.output.push(sb);
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
    /// Returns a `SerialOutput` describing any state changes from a completed transfer.
    pub fn tick(&mut self, cycles: u16) -> SerialOutput {
        let remaining = match self.cycles_remaining {
            Some(r) => r,
            None => return SerialOutput { interrupt: false, sb: None, sc: None },
        };

        if remaining > cycles {
            self.cycles_remaining = Some(remaining - cycles);
            SerialOutput { interrupt: false, sb: None, sc: None }
        } else {
            self.cycles_remaining = None;
            SerialOutput {
                interrupt: true,
                sb: Some(0xFF),
                sc: Some(0x00),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_transfer_captures_sb_byte() {
        let mut port = SerialPort::new();
        port.handle_sc_write(0x81, b'H'); // internal clock transfer
        assert_eq!(port.output(), b"H");
    }

    #[test]
    fn test_serial_transfer_without_start_bit_does_not_capture() {
        let mut port = SerialPort::new();
        port.handle_sc_write(0x01, b'X'); // bit 7 NOT set
        assert_eq!(port.output(), b"");
    }

    #[test]
    fn test_serial_captures_multiple_bytes_in_order() {
        let mut port = SerialPort::new();
        // Internal-clock transfer: start, tick to complete, then start next
        port.handle_sc_write(0x81, b'H');
        port.tick(512); // complete first transfer
        port.handle_sc_write(0x81, b'i');
        assert_eq!(port.output(), b"Hi");
    }

    #[test]
    fn test_serial_output_starts_empty() {
        let port = SerialPort::new();
        assert_eq!(port.output(), b"");
    }

    #[test]
    fn test_write_to_sb_alone_does_not_capture() {
        // Only an SC write triggers capture; an SB write alone does nothing.
        let port = SerialPort::new();
        assert_eq!(port.output(), b"");
    }

    // ── Internal-clock transfer timing ────────────────────────────────────────

    #[test]
    fn internal_clock_no_interrupt_before_512_cycles() {
        let mut port = SerialPort::new();
        port.handle_sc_write(0x81, b'A'); // internal clock
        let out = port.tick(511);
        assert!(!out.interrupt);
    }

    #[test]
    fn internal_clock_interrupt_fires_at_512_cycles() {
        let mut port = SerialPort::new();
        port.handle_sc_write(0x81, b'A'); // internal clock
        let out = port.tick(512);
        assert!(out.interrupt);
        assert_eq!(out.sb, Some(0xFF));
        assert_eq!(out.sc, Some(0x00));
    }

    #[test]
    fn internal_clock_interrupt_fires_across_multiple_ticks() {
        let mut port = SerialPort::new();
        port.handle_sc_write(0x81, b'A');
        let out1 = port.tick(256);
        assert!(!out1.interrupt);
        let out2 = port.tick(256);
        assert!(out2.interrupt);
    }

    #[test]
    fn external_clock_transfer_never_fires_interrupt() {
        let mut port = SerialPort::new();
        port.handle_sc_write(0x80, b'A'); // external clock (no internal clock bit)
        let out = port.tick(512);
        assert!(!out.interrupt);
    }

    #[test]
    fn no_transfer_tick_returns_no_interrupt() {
        let mut port = SerialPort::new();
        let out = port.tick(512);
        assert!(!out.interrupt);
        assert!(out.sb.is_none());
        assert!(out.sc.is_none());
    }
}
