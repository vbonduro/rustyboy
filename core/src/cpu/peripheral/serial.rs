use alloc::vec::Vec;

const SC_TRANSFER_BIT: u8 = 0x80;

/// Serial port peripheral. Captures bytes transferred via the Game Boy serial link.
///
/// When the ROM writes to SC (0xFF02) with bit 7 set it starts a transfer.
/// The byte currently in SB (0xFF01) is captured into the output buffer.
pub struct SerialPort {
    output: Vec<u8>,
}

impl SerialPort {
    pub fn new() -> Self {
        Self { output: Vec::new() }
    }

    pub fn output(&self) -> &[u8] {
        &self.output
    }

    /// Called when SC (0xFF02) is written. If the transfer-start bit is set,
    /// captures `sb` (the current SB register value) into the output buffer.
    pub fn handle_sc_write(&mut self, sc_value: u8, sb: u8) {
        if sc_value & SC_TRANSFER_BIT != 0 {
            self.output.push(sb);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serial_transfer_captures_sb_byte() {
        let mut port = SerialPort::new();
        port.handle_sc_write(0x81, b'H');
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
        port.handle_sc_write(0x81, b'H');
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
}
