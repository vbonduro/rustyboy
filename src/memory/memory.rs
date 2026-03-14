use std::fmt;

use super::rom::{Ram, ROMVec, ReadOnlyMemory};

#[derive(Debug)]
pub enum Error {
    OutOfRange(u16),
    ReadOnly(u16),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::OutOfRange(address) => write!(f, "Address 0x{:04X} is out of range", address),
            Error::ReadOnly(address) => write!(f, "Address 0x{:04X} is read-only", address),
        }
    }
}

pub trait Memory {
    fn read(&self, address: u16) -> Result<u8, Error>;
    fn write(&mut self, address: u16, value: u8) -> Result<(), Error>;
}

/// Game Boy memory map dispatching reads/writes to the appropriate region.
///
/// Address map:
///   0x0000–0x7FFF  ROM (read-only cartridge)
///   0x8000–0x9FFF  VRAM
///   0xA000–0xBFFF  External RAM
///   0xC000–0xDFFF  Work RAM (WRAM)
///   0xE000–0xFDFF  Echo RAM (mirrors WRAM 0xC000–0xDDFF)
///   0xFE00–0xFE9F  OAM
///   0xFF80–0xFFFE  High RAM (HRAM)
///   0xFFFF         Interrupt Enable register (stub)
///   Everything else: unmapped (returns 0xFF on read, error on write)
pub struct GameBoyMemory {
    rom: ROMVec,
    vram: Ram,
    external_ram: Ram,
    wram: Ram,
    oam: Ram,
    hram: Ram,
}

impl GameBoyMemory {
    pub fn new() -> Self {
        Self {
            rom: ROMVec::new(vec![0u8; 0x8000]),
            vram: Ram::new(0x2000),
            external_ram: Ram::new(0x2000),
            wram: Ram::new(0x2000),
            oam: Ram::new(0xA0),
            hram: Ram::new(0x7F),
        }
    }

    pub fn with_rom(data: Vec<u8>) -> Self {
        assert!(data.len() <= 0x8000, "ROM data exceeds 32KiB");
        let mut rom_data = vec![0u8; 0x8000];
        rom_data[..data.len()].copy_from_slice(&data);
        Self {
            rom: ROMVec::new(rom_data),
            vram: Ram::new(0x2000),
            external_ram: Ram::new(0x2000),
            wram: Ram::new(0x2000),
            oam: Ram::new(0xA0),
            hram: Ram::new(0x7F),
        }
    }
}

impl Memory for GameBoyMemory {
    fn read(&self, address: u16) -> Result<u8, Error> {
        match address {
            0x0000..=0x7FFF => self.rom.read(address).map_err(|_| Error::OutOfRange(address)),
            0x8000..=0x9FFF => self.vram.read(address - 0x8000).map_err(|_| Error::OutOfRange(address)),
            0xA000..=0xBFFF => self.external_ram.read(address - 0xA000).map_err(|_| Error::OutOfRange(address)),
            0xC000..=0xDFFF => self.wram.read(address - 0xC000).map_err(|_| Error::OutOfRange(address)),
            0xE000..=0xFDFF => self.wram.read(address - 0xE000).map_err(|_| Error::OutOfRange(address)),
            0xFE00..=0xFE9F => self.oam.read(address - 0xFE00).map_err(|_| Error::OutOfRange(address)),
            0xFF80..=0xFFFE => self.hram.read(address - 0xFF80).map_err(|_| Error::OutOfRange(address)),
            _ => Ok(0xFF), // Unmapped regions return 0xFF (open bus)
        }
    }

    fn write(&mut self, address: u16, value: u8) -> Result<(), Error> {
        match address {
            0x0000..=0x7FFF => Err(Error::ReadOnly(address)),
            0x8000..=0x9FFF => self.vram.write(address - 0x8000, value).map_err(|_| Error::OutOfRange(address)),
            0xA000..=0xBFFF => self.external_ram.write(address - 0xA000, value).map_err(|_| Error::OutOfRange(address)),
            0xC000..=0xDFFF => self.wram.write(address - 0xC000, value).map_err(|_| Error::OutOfRange(address)),
            0xE000..=0xFDFF => Err(Error::ReadOnly(address)), // Echo RAM not writable
            0xFE00..=0xFE9F => self.oam.write(address - 0xFE00, value).map_err(|_| Error::OutOfRange(address)),
            0xFF80..=0xFFFE => self.hram.write(address - 0xFF80, value).map_err(|_| Error::OutOfRange(address)),
            _ => Ok(()), // Unmapped writes silently ignored
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ROM region (read-only) ---

    #[test]
    fn test_rom_region_reads_loaded_data() {
        let mem = GameBoyMemory::with_rom(vec![0x11, 0x22, 0x33]);
        assert_eq!(mem.read(0x0000).unwrap(), 0x11);
        assert_eq!(mem.read(0x0001).unwrap(), 0x22);
        assert_eq!(mem.read(0x0002).unwrap(), 0x33);
        assert_eq!(mem.read(0x0003).unwrap(), 0x00);
    }

    #[test]
    fn test_rom_region_write_returns_readonly_error() {
        let mut mem = GameBoyMemory::new();
        assert!(matches!(mem.write(0x0000, 0xFF), Err(Error::ReadOnly(_))));
        assert!(matches!(mem.write(0x7FFF, 0xFF), Err(Error::ReadOnly(_))));
    }

    // --- VRAM (0x8000–0x9FFF) ---

    #[test]
    fn test_vram_write_then_read() {
        let mut mem = GameBoyMemory::new();
        mem.write(0x8000, 0xAB).unwrap();
        assert_eq!(mem.read(0x8000).unwrap(), 0xAB);
    }

    #[test]
    fn test_vram_boundary() {
        let mut mem = GameBoyMemory::new();
        mem.write(0x9FFF, 0x55).unwrap();
        assert_eq!(mem.read(0x9FFF).unwrap(), 0x55);
    }

    // --- External RAM (0xA000–0xBFFF) ---

    #[test]
    fn test_external_ram_write_then_read() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xA000, 0x42).unwrap();
        assert_eq!(mem.read(0xA000).unwrap(), 0x42);
    }

    // --- Work RAM (0xC000–0xDFFF) ---

    #[test]
    fn test_wram_write_then_read() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xC000, 0x77).unwrap();
        assert_eq!(mem.read(0xC000).unwrap(), 0x77);
    }

    #[test]
    fn test_wram_boundary() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xDFFF, 0x99).unwrap();
        assert_eq!(mem.read(0xDFFF).unwrap(), 0x99);
    }

    // --- Echo RAM (0xE000–0xFDFF) mirrors WRAM ---

    #[test]
    fn test_echo_ram_mirrors_wram_on_read() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xC100, 0xBE).unwrap();
        assert_eq!(mem.read(0xE100).unwrap(), 0xBE);
    }

    #[test]
    fn test_echo_ram_write_returns_readonly_error() {
        let mut mem = GameBoyMemory::new();
        assert!(matches!(mem.write(0xE000, 0xFF), Err(Error::ReadOnly(_))));
    }

    // --- OAM (0xFE00–0xFE9F) ---

    #[test]
    fn test_oam_write_then_read() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFE00, 0xCC).unwrap();
        assert_eq!(mem.read(0xFE00).unwrap(), 0xCC);
    }

    // --- High RAM (0xFF80–0xFFFE) ---

    #[test]
    fn test_hram_write_then_read() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFF80, 0x10).unwrap();
        assert_eq!(mem.read(0xFF80).unwrap(), 0x10);
    }

    #[test]
    fn test_hram_boundary() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFFFE, 0x20).unwrap();
        assert_eq!(mem.read(0xFFFE).unwrap(), 0x20);
    }

    // --- Unmapped regions ---

    #[test]
    fn test_unmapped_read_returns_0xff() {
        let mem = GameBoyMemory::new();
        assert_eq!(mem.read(0xFEA0).unwrap(), 0xFF); // Restricted OAM
        assert_eq!(mem.read(0xFF00).unwrap(), 0xFF); // I/O registers (stub)
    }

    // --- Error display ---

    #[test]
    fn test_error_readonly_display() {
        let err = Error::ReadOnly(0x1234);
        assert!(format!("{}", err).contains("0x1234"));
    }

    #[test]
    fn test_error_out_of_range_display() {
        let err = Error::OutOfRange(0xABCD);
        assert!(format!("{}", err).contains("0xABCD"));
    }
}
