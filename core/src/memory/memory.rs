use alloc::collections::VecDeque;
use alloc::{vec, vec::Vec};
use core::fmt;

use super::rom::{ROMVec, Ram, ReadOnlyMemory};

/// An event produced when a write occurs to an I/O or IE register address.
#[derive(Debug, PartialEq, Clone)]
pub struct BusEvent {
    pub address: u16,
    pub value: u8,
}

#[derive(Debug)]
pub enum Error {
    OutOfRange(u16),
    ReadOnly(u16),
}

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
    /// Drains and returns all pending bus events. Defaults to returning an empty
    /// vec for Memory implementations that do not produce events (e.g. FakeMemory).
    fn drain_events(&mut self) -> Vec<BusEvent> {
        Vec::new()
    }
}

/// Resolved mapping for a given address: which region and the offset within it.
enum RegionMapping {
    Rom(u16),
    Vram(u16),
    ExternalRam(u16),
    Wram(u16),
    /// Echo RAM: mirrors WRAM on reads, but is not writable.
    EchoRam(u16),
    Oam(u16),
    /// I/O registers: 0xFF00–0xFF7F
    Io(u16),
    Hram(u16),
    /// Interrupt Enable register at 0xFFFF.
    InterruptEnable,
    Unmapped,
}

impl RegionMapping {
    fn for_address(address: u16) -> Self {
        match address {
            0x0000..=0x7FFF => RegionMapping::Rom(address),
            0x8000..=0x9FFF => RegionMapping::Vram(address - 0x8000),
            0xA000..=0xBFFF => RegionMapping::ExternalRam(address - 0xA000),
            0xC000..=0xDFFF => RegionMapping::Wram(address - 0xC000),
            0xE000..=0xFDFF => RegionMapping::EchoRam(address - 0xE000),
            0xFE00..=0xFE9F => RegionMapping::Oam(address - 0xFE00),
            0xFF00..=0xFF7F => RegionMapping::Io(address - 0xFF00),
            0xFF80..=0xFFFE => RegionMapping::Hram(address - 0xFF80),
            0xFFFF => RegionMapping::InterruptEnable,
            _ => RegionMapping::Unmapped,
        }
    }
}

/// Game Boy memory map dispatching reads/writes to the appropriate region.
///
/// Address map:
///   0x0000–0x7FFF  ROM (read-only cartridge)
///   0x8000–0x9FFF  VRAM
///   0xA000–0xBFFF  External RAM
///   0xC000–0xDFFF  Work RAM (WRAM)
///   0xE000–0xFDFF  Echo RAM (mirrors WRAM reads, writes are read-only)
///   0xFE00–0xFE9F  OAM
///   0xFF00–0xFF7F  I/O registers
///   0xFF80–0xFFFE  High RAM (HRAM)
///   0xFFFF         Interrupt Enable (IE) register
///   Everything else: unmapped (returns 0xFF on read, silently ignored on write)
pub struct GameBoyMemory {
    rom: ROMVec,
    vram: Ram,
    external_ram: Ram,
    wram: Ram,
    oam: Ram,
    io: Ram,
    hram: Ram,
    ie: u8,
    events: VecDeque<BusEvent>,
}

impl GameBoyMemory {
    pub fn new() -> Self {
        Self {
            rom: ROMVec::new(vec![0u8; 0x8000]),
            vram: Ram::new(0x2000),
            external_ram: Ram::new(0x2000),
            wram: Ram::new(0x2000),
            oam: Ram::new(0xA0),
            io: Ram::new(0x80),
            hram: Ram::new(0x7F),
            ie: 0,
            events: VecDeque::new(),
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
            io: Ram::new(0x80),
            hram: Ram::new(0x7F),
            ie: 0,
            events: VecDeque::new(),
        }
    }

    /// Perform OAM DMA: copy 160 bytes from the source page to OAM.
    /// Source address = page * 0x100. Reads go through normal memory mapping.
    pub fn dma_to_oam(&mut self, page: u8) {
        let base = (page as u16) << 8;
        for i in 0..0xA0u16 {
            let byte = self.read(base + i).unwrap_or(0xFF);
            let _ = self.oam.write(i, byte);
        }
    }

    pub fn vram(&self) -> &[u8] {
        self.vram.as_slice()
    }

    pub fn oam(&self) -> &[u8] {
        self.oam.as_slice()
    }

    /// Direct read of an IO register. No bus events.
    /// Handles 0xFF00-0xFF7F from io array, 0xFFFF from ie field.
    pub fn read_io(&self, address: u16) -> u8 {
        match address {
            0xFF00..=0xFF7F => self.io.read(address - 0xFF00).unwrap_or(0xFF),
            0xFFFF => self.ie,
            _ => 0xFF,
        }
    }

    /// Direct write to an IO register. No bus events queued.
    /// Used by CPU to write back peripheral state (timer, interrupts).
    pub fn write_io(&mut self, address: u16, value: u8) {
        match address {
            0xFF00..=0xFF7F => {
                let _ = self.io.write(address - 0xFF00, value);
            }
            0xFFFF => {
                self.ie = value;
            }
            _ => {}
        }
    }
}

impl Memory for GameBoyMemory {
    fn read(&self, address: u16) -> Result<u8, Error> {
        match RegionMapping::for_address(address) {
            RegionMapping::Rom(offset) => self
                .rom
                .read(offset)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::Vram(offset) => self
                .vram
                .read(offset)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::ExternalRam(offset) => self
                .external_ram
                .read(offset)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::Wram(offset) => self
                .wram
                .read(offset)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::EchoRam(offset) => self
                .wram
                .read(offset)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::Oam(offset) => self
                .oam
                .read(offset)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::Io(offset) => self
                .io
                .read(offset)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::Hram(offset) => self
                .hram
                .read(offset)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::InterruptEnable => Ok(self.ie),
            RegionMapping::Unmapped => Ok(0xFF),
        }
    }

    fn write(&mut self, address: u16, value: u8) -> Result<(), Error> {
        match RegionMapping::for_address(address) {
            RegionMapping::Rom(_) => Ok(()), // Writes to ROM are silently ignored (or handled by MBC)
            RegionMapping::Vram(offset) => self
                .vram
                .write(offset, value)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::ExternalRam(offset) => self
                .external_ram
                .write(offset, value)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::Wram(offset) => self
                .wram
                .write(offset, value)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::EchoRam(_) => Err(Error::ReadOnly(address)),
            RegionMapping::Oam(offset) => self
                .oam
                .write(offset, value)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::Io(offset) => {
                self.io
                    .write(offset, value)
                    .map_err(|_| Error::OutOfRange(address))?;
                self.events.push_back(BusEvent { address, value });
                Ok(())
            }
            RegionMapping::Hram(offset) => self
                .hram
                .write(offset, value)
                .map_err(|_| Error::OutOfRange(address)),
            RegionMapping::InterruptEnable => {
                self.ie = value;
                self.events.push_back(BusEvent { address, value });
                Ok(())
            }
            RegionMapping::Unmapped => Ok(()),
        }
    }

    fn drain_events(&mut self) -> Vec<BusEvent> {
        self.events.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::format;

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
    fn test_rom_region_write_is_silently_ignored() {
        let mem_with_rom = GameBoyMemory::with_rom(vec![0x11, 0x22]);
        let mut mem = mem_with_rom;
        assert!(mem.write(0x0000, 0xFF).is_ok());
        // ROM data should be unchanged
        assert_eq!(mem.read(0x0000).unwrap(), 0x11);
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

    // --- I/O registers (0xFF00–0xFF7F) ---

    #[test]
    fn test_io_write_then_read() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFF00, 0x42).unwrap();
        assert_eq!(mem.read(0xFF00).unwrap(), 0x42);
    }

    #[test]
    fn test_io_boundary_low() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFF00, 0x11).unwrap();
        assert_eq!(mem.read(0xFF00).unwrap(), 0x11);
    }

    #[test]
    fn test_io_boundary_high() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFF7F, 0x99).unwrap();
        assert_eq!(mem.read(0xFF7F).unwrap(), 0x99);
    }

    #[test]
    fn test_io_zero_initialized() {
        let mem = GameBoyMemory::new();
        assert_eq!(mem.read(0xFF01).unwrap(), 0x00);
    }

    // --- IE register (0xFFFF) ---

    #[test]
    fn test_ie_write_stores_and_produces_bus_event() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFFFF, 0x1F).unwrap();
        assert_eq!(mem.read(0xFFFF).unwrap(), 0x1F);
        let events = mem.drain_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].address, 0xFFFF);
        assert_eq!(events[0].value, 0x1F);
    }

    // --- Unmapped regions ---

    #[test]
    fn test_unmapped_read_returns_0xff() {
        let mem = GameBoyMemory::new();
        assert_eq!(mem.read(0xFEA0).unwrap(), 0xFF); // Restricted OAM
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

    // --- BusEvent queue ---

    #[test]
    fn test_io_write_produces_bus_event() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFF01, 0x48).unwrap();
        let events = mem.drain_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].address, 0xFF01);
        assert_eq!(events[0].value, 0x48);
    }

    #[test]
    fn test_non_io_write_produces_no_bus_event() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xC000, 0x42).unwrap(); // WRAM — not I/O
        let events = mem.drain_events();
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn test_drain_events_clears_queue() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFF01, 0x01).unwrap();
        let _ = mem.drain_events();
        let events = mem.drain_events();
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn test_multiple_io_writes_produce_ordered_events() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFF01, 0x41).unwrap(); // 'A'
        mem.write(0xFF02, 0x81).unwrap(); // SC transfer start
        let events = mem.drain_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].address, 0xFF01);
        assert_eq!(events[0].value, 0x41);
        assert_eq!(events[1].address, 0xFF02);
        assert_eq!(events[1].value, 0x81);
    }

    // --- read_io / write_io ---

    #[test]
    fn test_read_io_returns_io_register_value() {
        let mut mem = GameBoyMemory::new();
        mem.write(0xFF01, 0x42).unwrap();
        assert_eq!(mem.read_io(0xFF01), 0x42);
    }

    #[test]
    fn test_write_io_does_not_produce_bus_event() {
        let mut mem = GameBoyMemory::new();
        mem.write_io(0xFF01, 0x42);
        let events = mem.drain_events();
        assert_eq!(events.len(), 0);
    }

    #[test]
    fn test_write_io_ie_roundtrips() {
        let mut mem = GameBoyMemory::new();
        mem.write_io(0xFFFF, 0x1F);
        assert_eq!(mem.read_io(0xFFFF), 0x1F);
    }

    #[test]
    fn test_read_io_ie_matches_memory_read() {
        let mut mem = GameBoyMemory::new();
        mem.write_io(0xFFFF, 0x1F);
        assert_eq!(mem.read(0xFFFF).unwrap(), 0x1F);
    }
}
