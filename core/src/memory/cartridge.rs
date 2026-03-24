/// Cartridge abstraction: handles ROM bank switching and external RAM.
///
/// The Game Boy memory map exposes two cartridge regions:
///   0x0000–0x7FFF  ROM (16 KiB bank 0 + switchable 16 KiB bank N)
///   0xA000–0xBFFF  External RAM (switchable 8 KiB bank, if present)
///
/// Writes to 0x0000–0x7FFF are intercepted by the MBC (not stored in ROM).
use alloc::{boxed::Box, vec, vec::Vec};

// ── Cartridge trait ──────────────────────────────────────────────────────────

pub trait Cartridge {
    fn read_rom(&self, addr: u16) -> u8;
    fn read_ram(&self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
}

// ── Header helpers ───────────────────────────────────────────────────────────

/// ROM header byte 0x0147: cartridge type (MBC variant + peripherals).
const CART_TYPE_ADDR: usize = 0x0147;
/// ROM header byte 0x0148: ROM size code.
const ROM_SIZE_ADDR: usize = 0x0148;
/// ROM header byte 0x0149: RAM size code.
const RAM_SIZE_ADDR: usize = 0x0149;

/// Nintendo logo bytes stored at 0x0104 in the ROM header.
const NINTENDO_LOGO: [u8; 48] = [
    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83,
    0x00, 0x0C, 0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E,
    0xDC, 0xCC, 0x6E, 0xE6, 0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63,
    0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC, 0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
];

/// Returns true if the Nintendo logo appears at offset `base + 0x0104` in `data`.
fn has_logo_at_bank(data: &[u8], bank: usize) -> bool {
    let base = bank * 0x4000;
    let logo_start = base + 0x0104;
    let logo_end = logo_start + NINTENDO_LOGO.len();
    data.get(logo_start..logo_end)
        .map(|s| s == NINTENDO_LOGO)
        .unwrap_or(false)
}

/// Detect MBC1 multicart: a 64-bank MBC1 ROM where every 16th bank contains
/// the Nintendo logo (indicating a compilation of 16-bank sub-games).
fn is_mbc1_multicart(data: &[u8], rom_bank_count: usize) -> bool {
    if rom_bank_count != 64 {
        return false;
    }
    // Check banks 0x10, 0x20, 0x30 for the logo
    has_logo_at_bank(data, 0x10)
        && has_logo_at_bank(data, 0x20)
        && has_logo_at_bank(data, 0x30)
}

/// Construct the appropriate `Cartridge` impl from a ROM image.
///
/// Reads the cartridge type, ROM size, and RAM size from the header and
/// returns a `NoMbc`, `Mbc1`, or `Mbc1Multicart` accordingly. Panics on
/// unsupported types.
///
/// MBC1 multicart mode is detected heuristically: a 64-bank MBC1 ROM with
/// the Nintendo logo present in banks 0x10, 0x20, and 0x30.
pub fn from_rom(data: Vec<u8>) -> Box<dyn Cartridge> {
    let cart_type = *data.get(CART_TYPE_ADDR).unwrap_or(&0);
    let rom_size_code = *data.get(ROM_SIZE_ADDR).unwrap_or(&0);
    let ram_size_code = *data.get(RAM_SIZE_ADDR).unwrap_or(&0);
    let ram_bytes = decode_ram_size(ram_size_code);
    let rom_bank_count = 2usize << rom_size_code;

    match cart_type {
        // ROM only (no MBC)
        0x00 => Box::new(NoMbc::new(data)),
        // MBC1, MBC1+RAM, MBC1+RAM+BATTERY
        0x01 | 0x02 | 0x03 => {
            if is_mbc1_multicart(&data, rom_bank_count) {
                Box::new(Mbc1Multicart::new(data, ram_bytes))
            } else {
                Box::new(Mbc1::new(data, ram_bytes))
            }
        }
        other => panic!("Unsupported cartridge type: 0x{:02X}", other),
    }
}

fn decode_ram_size(code: u8) -> usize {
    match code {
        0x00 => 0,
        0x01 => 2 * 1024,        // 2 KiB (unofficial, treated as 8 KiB by some)
        0x02 => 8 * 1024,        // 8 KiB — 1 bank
        0x03 => 32 * 1024,       // 32 KiB — 4 banks
        0x04 => 128 * 1024,      // 128 KiB — 16 banks
        0x05 => 64 * 1024,       // 64 KiB — 8 banks
        _ => 0,
    }
}

// ── NoMbc ────────────────────────────────────────────────────────────────────

/// Flat ROM with no bank switching. Supports up to 32 KiB ROM and 8 KiB RAM.
pub struct NoMbc {
    rom: Vec<u8>,
    ram: Vec<u8>,
}

impl NoMbc {
    pub fn new(data: Vec<u8>) -> Self {
        Self {
            rom: data,
            ram: vec![0u8; 0x2000],
        }
    }
}

impl Cartridge for NoMbc {
    fn read_rom(&self, addr: u16) -> u8 {
        self.rom.get(addr as usize).copied().unwrap_or(0xFF)
    }

    fn read_ram(&self, addr: u16) -> u8 {
        self.ram.get(addr as usize).copied().unwrap_or(0xFF)
    }

    fn write(&mut self, addr: u16, value: u8) {
        if (0xA000..=0xBFFF).contains(&addr) {
            let offset = (addr - 0xA000) as usize;
            if let Some(b) = self.ram.get_mut(offset) {
                *b = value;
            }
        }
        // Writes to ROM space are ignored
    }
}

// ── MBC1 ─────────────────────────────────────────────────────────────────────

/// MBC1 memory bank controller.
///
/// Register map (writes to ROM space):
///   0x0000–0x1FFF  RAM enable: lower 4 bits == 0x0A enables RAM
///   0x2000–0x3FFF  ROM bank number (5-bit, lower bank register)
///   0x4000–0x5FFF  Upper bits register (2-bit): upper ROM bits or RAM bank
///   0x6000–0x7FFF  Banking mode: 0 = ROM mode, 1 = RAM mode
///
/// In ROM mode (default): upper bits extend the ROM bank, RAM is fixed to bank 0.
/// In RAM mode: upper bits select RAM bank, ROM bank 0 area may be remapped.
///
/// Pan Docs reference: <https://gbdev.io/pandocs/MBC1.html>
pub struct Mbc1 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    /// Lower 5-bit ROM bank register (written to 0x2000–0x3FFF).
    rom_bank_lo: u8,
    /// Upper 2-bit register (written to 0x4000–0x5FFF).
    upper_bits: u8,
    /// Banking mode: false = ROM mode, true = RAM mode.
    ram_mode: bool,
    /// Whether external RAM is enabled (RAM enable register).
    ram_enabled: bool,
    /// Number of ROM banks (derived from ROM size). Used for bank masking.
    rom_bank_count: usize,
    /// Number of RAM banks. Used for bank masking.
    ram_bank_count: usize,
}

impl Mbc1 {
    pub fn new(data: Vec<u8>, ram_bytes: usize) -> Self {
        let rom_size_code = *data.get(ROM_SIZE_ADDR).unwrap_or(&0);
        let rom_bank_count = 2usize << rom_size_code; // 2, 4, 8, ..., 512

        let ram_bank_count = if ram_bytes == 0 { 0 } else { ram_bytes / (8 * 1024) }.max(1);

        Self {
            rom: data,
            ram: vec![0u8; ram_bytes.max(0x2000)],
            rom_bank_lo: 1,
            upper_bits: 0,
            ram_mode: false,
            ram_enabled: false,
            rom_bank_count,
            ram_bank_count,
        }
    }

    /// Effective ROM bank for the switchable region (0x4000–0x7FFF).
    ///
    /// Always combines upper 2 bits and lower 5 bits regardless of banking mode.
    /// Maps bank 0 → 1 (the MBC1 quirk: bank 0 is never accessible in the
    /// switchable window). Result is masked to available banks.
    fn rom_bank(&self) -> usize {
        let bank = ((self.upper_bits as usize) << 5) | (self.rom_bank_lo as usize);
        // Bank 0 alias: writing 0 to the lower register selects bank 1
        let bank = if bank == 0 { 1 } else { bank };
        bank % self.rom_bank_count
    }

    /// Effective ROM bank for the fixed region (0x0000–0x3FFF).
    ///
    /// In ROM mode this is always bank 0. In RAM mode the upper bits select
    /// which 32-bank group is visible (e.g. 0x00, 0x20, 0x40, 0x60).
    fn rom_bank0(&self) -> usize {
        if self.ram_mode {
            ((self.upper_bits as usize) << 5) % self.rom_bank_count
        } else {
            0
        }
    }

    /// Effective RAM bank (0 in ROM mode).
    fn ram_bank(&self) -> usize {
        if self.ram_mode {
            (self.upper_bits as usize) % self.ram_bank_count.max(1)
        } else {
            0
        }
    }
}

impl Cartridge for Mbc1 {
    fn read_rom(&self, addr: u16) -> u8 {
        let physical = match addr {
            0x0000..=0x3FFF => self.rom_bank0() * 0x4000 + addr as usize,
            0x4000..=0x7FFF => self.rom_bank() * 0x4000 + (addr as usize - 0x4000),
            _ => return 0xFF,
        };
        self.rom.get(physical).copied().unwrap_or(0xFF)
    }

    fn read_ram(&self, addr: u16) -> u8 {
        if !self.ram_enabled || self.ram.is_empty() {
            return 0xFF;
        }
        let offset = self.ram_bank() * 0x2000 + addr as usize;
        self.ram.get(offset).copied().unwrap_or(0xFF)
    }

    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            // RAM enable: any write with lower nibble 0x0A enables, anything else disables
            0x0000..=0x1FFF => {
                self.ram_enabled = value & 0x0F == 0x0A;
            }
            // ROM bank number (lower 5 bits)
            0x2000..=0x3FFF => {
                self.rom_bank_lo = value & 0x1F;
                // Writing 0 is treated as 1
                if self.rom_bank_lo == 0 {
                    self.rom_bank_lo = 1;
                }
            }
            // Upper bits register (2 bits)
            0x4000..=0x5FFF => {
                self.upper_bits = value & 0x03;
            }
            // Banking mode select
            0x6000..=0x7FFF => {
                self.ram_mode = value & 0x01 != 0;
            }
            // External RAM write
            0xA000..=0xBFFF => {
                if self.ram_enabled && !self.ram.is_empty() {
                    let offset = self.ram_bank() * 0x2000 + (addr - 0xA000) as usize;
                    if let Some(b) = self.ram.get_mut(offset) {
                        *b = value;
                    }
                }
            }
            _ => {}
        }
    }
}

// ── MBC1 Multicart ───────────────────────────────────────────────────────────

/// MBC1 multicart: a compilation of up to 4 games on a single 64-bank (8Mbit) ROM.
///
/// The hardware is wired so that only 4 bits of BANK1 address the sub-game's
/// ROM banks, and the 2-bit BANK2 register selects the sub-game (shifting by 4
/// rather than 5). This allows each game to see its own 16 banks independently.
///
/// Detected heuristically from `from_rom`: a 64-bank MBC1 ROM where the
/// Nintendo logo appears at banks 0x10, 0x20, and 0x30.
struct Mbc1Multicart {
    rom: Vec<u8>,
    ram: Vec<u8>,
    rom_bank_lo: u8,
    upper_bits: u8,
    ram_mode: bool,
    ram_enabled: bool,
    rom_bank_count: usize,
    ram_bank_count: usize,
}

impl Mbc1Multicart {
    fn new(data: Vec<u8>, ram_bytes: usize) -> Self {
        let rom_size_code = *data.get(ROM_SIZE_ADDR).unwrap_or(&0);
        let rom_bank_count = 2usize << rom_size_code;
        let ram_bank_count = if ram_bytes == 0 { 0 } else { ram_bytes / (8 * 1024) }.max(1);
        Self {
            rom: data,
            ram: vec![0u8; ram_bytes.max(0x2000)],
            rom_bank_lo: 1,
            upper_bits: 0,
            ram_mode: false,
            ram_enabled: false,
            rom_bank_count,
            ram_bank_count,
        }
    }

    /// Effective switchable ROM bank (0x4000–0x7FFF).
    ///
    /// Upper 2 bits shift by 4 (not 5) to address 16-bank sub-games.
    /// The 4-bit lower register uses the 0→1 alias from the MBC1 write.
    fn rom_bank(&self) -> usize {
        let bank = ((self.upper_bits as usize) << 4) | (self.rom_bank_lo & 0x0F) as usize;
        bank % self.rom_bank_count
    }

    /// Effective fixed ROM bank (0x0000–0x3FFF).
    ///
    /// In ROM mode: always the start of the current sub-game (upper << 4).
    /// In RAM mode: same (upper_bits always selects sub-game base).
    fn rom_bank0(&self) -> usize {
        if self.ram_mode {
            ((self.upper_bits as usize) << 4) % self.rom_bank_count
        } else {
            0
        }
    }

    fn ram_bank(&self) -> usize {
        if self.ram_mode {
            (self.upper_bits as usize) % self.ram_bank_count.max(1)
        } else {
            0
        }
    }
}

impl Cartridge for Mbc1Multicart {
    fn read_rom(&self, addr: u16) -> u8 {
        let physical = match addr {
            0x0000..=0x3FFF => self.rom_bank0() * 0x4000 + addr as usize,
            0x4000..=0x7FFF => self.rom_bank() * 0x4000 + (addr as usize - 0x4000),
            _ => return 0xFF,
        };
        self.rom.get(physical).copied().unwrap_or(0xFF)
    }

    fn read_ram(&self, addr: u16) -> u8 {
        if !self.ram_enabled || self.ram.is_empty() {
            return 0xFF;
        }
        let offset = self.ram_bank() * 0x2000 + addr as usize;
        self.ram.get(offset).copied().unwrap_or(0xFF)
    }

    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            0x0000..=0x1FFF => {
                self.ram_enabled = value & 0x0F == 0x0A;
            }
            // Lower bank register: 4-bit effective, 0→1 alias preserved
            0x2000..=0x3FFF => {
                self.rom_bank_lo = value & 0x1F;
                if self.rom_bank_lo == 0 {
                    self.rom_bank_lo = 1;
                }
            }
            0x4000..=0x5FFF => {
                self.upper_bits = value & 0x03;
            }
            0x6000..=0x7FFF => {
                self.ram_mode = value & 0x01 != 0;
            }
            0xA000..=0xBFFF => {
                if self.ram_enabled && !self.ram.is_empty() {
                    let offset = self.ram_bank() * 0x2000 + (addr - 0xA000) as usize;
                    if let Some(b) = self.ram.get_mut(offset) {
                        *b = value;
                    }
                }
            }
            _ => {}
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_rom(size_kb: usize, cart_type: u8) -> Vec<u8> {
        let size = size_kb * 1024;
        let mut data = vec![0u8; size];
        // Fill each bank with its bank number for easy verification
        for bank in 0..(size / 0x4000) {
            for byte in &mut data[bank * 0x4000..(bank + 1) * 0x4000] {
                *byte = bank as u8;
            }
        }
        // Set header bytes after fill (fill would overwrite them otherwise)
        data[CART_TYPE_ADDR] = cart_type;
        // ROM size code: size = 32KB << code → code = log2(size / 32KB)
        let code = (size / (32 * 1024)).trailing_zeros() as u8;
        data[ROM_SIZE_ADDR] = code;
        data
    }

    // ── NoMbc ────────────────────────────────────────────────────────────────

    #[test]
    fn no_mbc_reads_rom() {
        let mut data = vec![0u8; 0x8000];
        data[0x0100] = 0x42;
        let cart = NoMbc::new(data);
        assert_eq!(cart.read_rom(0x0100), 0x42);
    }

    #[test]
    fn no_mbc_ram_read_write() {
        let mut cart = NoMbc::new(vec![0u8; 0x8000]);
        cart.write(0xA000, 0xAB);
        assert_eq!(cart.read_ram(0x0000), 0xAB);
    }

    // ── MBC1 bank switching ───────────────────────────────────────────────────

    #[test]
    fn mbc1_bank0_always_reads_first_bank() {
        let data = make_rom(128, 0x01);
        let cart = Mbc1::new(data, 0);
        // Bank 0 in data was filled with 0x00
        assert_eq!(cart.read_rom(0x0000), 0x00);
    }

    #[test]
    fn mbc1_default_bank1_switchable() {
        let data = make_rom(128, 0x01);
        let cart = Mbc1::new(data, 0);
        // After reset, rom_bank_lo=1, switchable window reads bank 1
        assert_eq!(cart.read_rom(0x4000), 0x01);
    }

    #[test]
    fn mbc1_write_0_to_bank_register_selects_bank1() {
        let data = make_rom(128, 0x01);
        let mut cart = Mbc1::new(data, 0);
        cart.write(0x2000, 0x00);
        // 0 -> 1 quirk: bank 1 is selected
        assert_eq!(cart.read_rom(0x4000), 0x01);
    }

    #[test]
    fn mbc1_switches_rom_bank() {
        let data = make_rom(128, 0x01);
        let mut cart = Mbc1::new(data, 0);
        cart.write(0x2000, 0x03);
        assert_eq!(cart.read_rom(0x4000), 0x03);
    }

    #[test]
    fn mbc1_upper_bits_extend_rom_bank() {
        let data = make_rom(1024, 0x01); // 1 MiB = 64 banks
        let mut cart = Mbc1::new(data, 0);
        cart.write(0x2000, 0x01); // lower = 1
        cart.write(0x4000, 0x01); // upper = 1 → bank 0x21 = 33
        assert_eq!(cart.read_rom(0x4000), 33);
    }

    // ── MBC1 RAM ──────────────────────────────────────────────────────────────

    #[test]
    fn mbc1_ram_disabled_by_default_returns_0xff() {
        let data = make_rom(64, 0x02);
        let cart = Mbc1::new(data, 8 * 1024);
        assert_eq!(cart.read_ram(0x0000), 0xFF);
    }

    #[test]
    fn mbc1_ram_enable_and_write() {
        let data = make_rom(64, 0x02);
        let mut cart = Mbc1::new(data, 8 * 1024);
        cart.write(0x0000, 0x0A); // enable RAM
        cart.write(0xA000, 0x55);
        assert_eq!(cart.read_ram(0x0000), 0x55);
    }

    #[test]
    fn mbc1_ram_disabled_write_is_ignored() {
        let data = make_rom(64, 0x02);
        let mut cart = Mbc1::new(data, 8 * 1024);
        // RAM not enabled — write should be ignored
        cart.write(0xA000, 0x55);
        cart.write(0x0000, 0x0A); // enable RAM after write
        assert_eq!(cart.read_ram(0x0000), 0x00); // unchanged
    }

    #[test]
    fn mbc1_ram_banking_in_ram_mode() {
        let data = make_rom(64, 0x03);
        let mut cart = Mbc1::new(data, 32 * 1024); // 4 RAM banks
        cart.write(0x0000, 0x0A); // enable
        cart.write(0x6000, 0x01); // RAM mode
        cart.write(0x4000, 0x01); // RAM bank 1
        cart.write(0xA000, 0xBB);
        cart.write(0x4000, 0x00); // RAM bank 0
        assert_eq!(cart.read_ram(0x0000), 0x00); // bank 0 untouched
        cart.write(0x4000, 0x01); // back to bank 1
        assert_eq!(cart.read_ram(0x0000), 0xBB);
    }

    // ── from_rom dispatch ─────────────────────────────────────────────────────

    #[test]
    fn from_rom_no_mbc_for_type_00() {
        let mut data = vec![0u8; 0x8000];
        data[CART_TYPE_ADDR] = 0x00;
        let cart = from_rom(data);
        assert_eq!(cart.read_rom(0x0000), 0x00);
    }

    #[test]
    fn from_rom_mbc1_for_type_01() {
        let mut data = make_rom(64, 0x01);
        data[CART_TYPE_ADDR] = 0x01;
        let cart = from_rom(data);
        assert_eq!(cart.read_rom(0x4000), 0x01); // default bank 1
    }
}
