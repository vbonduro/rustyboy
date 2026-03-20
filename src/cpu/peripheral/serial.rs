use crate::memory::memory::{BusEvent, Memory};

use super::bus::Peripheral;

const SB: u16 = 0xFF01;
const SC: u16 = 0xFF02;
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
}

impl Peripheral for SerialPort {
    fn handle(&mut self, event: &BusEvent, mem: &mut dyn Memory) {
        if event.address == SC && event.value & SC_TRANSFER_BIT != 0 {
            if let Ok(byte) = mem.read(SB) {
                self.output.push(byte);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::peripheral::bus::PeripheralBus;
    use crate::memory::memory::GameBoyMemory;

    fn make_bus_with_serial() -> (PeripheralBus, std::rc::Rc<std::cell::RefCell<SerialPort>>) {
        use std::cell::RefCell;
        use std::rc::Rc;

        // We need shared access to SerialPort to inspect output after flush.
        // Use a wrapper that holds an Rc<RefCell<SerialPort>>.
        struct SharedSerial(Rc<RefCell<SerialPort>>);
        impl Peripheral for SharedSerial {
            fn handle(&mut self, event: &BusEvent, mem: &mut dyn Memory) {
                self.0.borrow_mut().handle(event, mem);
            }
        }

        let port = Rc::new(RefCell::new(SerialPort::new()));
        let mut bus = PeripheralBus::new();
        bus.subscribe(SC..=SC, Box::new(SharedSerial(port.clone())));
        (bus, port)
    }

    #[test]
    fn test_serial_transfer_captures_sb_byte() {
        let mut mem = GameBoyMemory::new();
        let (mut bus, port) = make_bus_with_serial();

        mem.write(SB, b'H').unwrap();
        mem.write(SC, 0x81).unwrap(); // start transfer
        bus.flush(&mut mem);

        assert_eq!(port.borrow().output(), b"H");
    }

    #[test]
    fn test_serial_transfer_without_start_bit_does_not_capture() {
        let mut mem = GameBoyMemory::new();
        let (mut bus, port) = make_bus_with_serial();

        mem.write(SB, b'X').unwrap();
        mem.write(SC, 0x01).unwrap(); // bit 7 NOT set
        bus.flush(&mut mem);

        assert_eq!(port.borrow().output(), b"");
    }

    #[test]
    fn test_serial_captures_multiple_bytes_in_order() {
        let mut mem = GameBoyMemory::new();
        let (mut bus, port) = make_bus_with_serial();

        for &byte in b"Hi" {
            mem.write(SB, byte).unwrap();
            mem.write(SC, 0x81).unwrap();
            bus.flush(&mut mem);
        }

        assert_eq!(port.borrow().output(), b"Hi");
    }

    #[test]
    fn test_serial_output_starts_empty() {
        let port = SerialPort::new();
        assert_eq!(port.output(), b"");
    }

    #[test]
    fn test_write_to_sb_alone_does_not_capture() {
        let mut mem = GameBoyMemory::new();
        let (mut bus, port) = make_bus_with_serial();

        mem.write(SB, b'Z').unwrap();
        bus.flush(&mut mem); // only SB written, no SC event

        assert_eq!(port.borrow().output(), b"");
    }
}
