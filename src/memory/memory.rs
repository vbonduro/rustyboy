use std::fmt;

#[derive(Debug)]
pub enum Error {
    OutOfRange(u16),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::OutOfRange(address) => write!(f, "Address 0x{:04X} is out of range", address),
        }
    }
}

pub trait Memory {
    fn read(&self, address: u16) -> Result<u8, Error>;
    fn write(&mut self, address: u16, value: u8) -> Result<(), Error>;
}

/// A flat 65536-byte address space representing the Game Boy memory bus.
/// All addresses 0x0000–0xFFFF are valid for read and write.
pub struct GameBoyMemory {
    data: Vec<u8>,
}

impl GameBoyMemory {
    /// Create a zeroed 64KiB address space.
    pub fn new() -> Self {
        Self {
            data: vec![0u8; 65536],
        }
    }

    /// Create a GameBoyMemory pre-loaded with the given ROM data at address 0.
    /// Panics if `rom` is longer than 65536 bytes.
    pub fn with_rom(rom: Vec<u8>) -> Self {
        assert!(rom.len() <= 65536, "ROM data exceeds 64KiB");
        let mut data = vec![0u8; 65536];
        data[..rom.len()].copy_from_slice(&rom);
        Self { data }
    }
}

impl Memory for GameBoyMemory {
    fn read(&self, address: u16) -> Result<u8, Error> {
        Ok(self.data[address as usize])
    }

    fn write(&mut self, address: u16, value: u8) -> Result<(), Error> {
        self.data[address as usize] = value;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gameboy_memory_read_zero_initialized() {
        let mem = GameBoyMemory::new();
        assert_eq!(mem.read(0x0000).unwrap(), 0x00);
        assert_eq!(mem.read(0xFFFF).unwrap(), 0x00);
    }

    #[test]
    fn test_gameboy_memory_write_then_read() {
        let mut mem = GameBoyMemory::new();
        mem.write(0x1234, 0xAB).unwrap();
        assert_eq!(mem.read(0x1234).unwrap(), 0xAB);
    }

    #[test]
    fn test_gameboy_memory_write_does_not_affect_other_addresses() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xC000, 0x42).unwrap();
        assert_eq!(mem.read(0xC001).unwrap(), 0x00);
        assert_eq!(mem.read(0xBFFF).unwrap(), 0x00);
    }

    #[test]
    fn test_gameboy_memory_with_rom_loads_correctly() {
        let rom = vec![0x11, 0x22, 0x33];
        let mem = GameBoyMemory::with_rom(rom);
        assert_eq!(mem.read(0x0000).unwrap(), 0x11);
        assert_eq!(mem.read(0x0001).unwrap(), 0x22);
        assert_eq!(mem.read(0x0002).unwrap(), 0x33);
        assert_eq!(mem.read(0x0003).unwrap(), 0x00);
    }

    #[test]
    fn test_gameboy_memory_write_all_addresses() {
        let mut mem = GameBoyMemory::new();
        // Write to first, middle and last address to confirm full range works
        mem.write(0x0000, 0x01).unwrap();
        mem.write(0x8000, 0x02).unwrap();
        mem.write(0xFFFF, 0x03).unwrap();
        assert_eq!(mem.read(0x0000).unwrap(), 0x01);
        assert_eq!(mem.read(0x8000).unwrap(), 0x02);
        assert_eq!(mem.read(0xFFFF).unwrap(), 0x03);
    }

    #[test]
    fn test_error_display() {
        let err = Error::OutOfRange(0xABCD);
        let msg = format!("{}", err);
        assert!(msg.contains("0xABCD"));
    }
}
