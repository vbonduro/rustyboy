/// Cartridge abstraction: handles ROM bank switching and external RAM.
///
/// The Game Boy memory map exposes two cartridge regions:
///   0x0000–0x7FFF  ROM (16 KiB bank 0 + switchable 16 KiB bank N)
///   0xA000–0xBFFF  External RAM (switchable 8 KiB bank, if present)
///
/// Writes to 0x0000–0x7FFF are intercepted by the MBC (not stored in ROM).
use alloc::{boxed::Box, vec, vec::Vec};

// ── Cartridge trait ──────────────────────────────────────────────────────────

#[cfg(feature = "perf")]
#[derive(Default)]
pub struct CartridgePerfProfile {
    pub write_rom: u32,
    pub write_ram: u32,
    pub control_write: u32,
    pub sync_caches: u32,
    pub sync_caches_calls: u32,
    pub read_bank_fixed: u32,
    pub read_bank_fixed_calls: u32,
    pub read_bank_switchable: u32,
    pub read_bank_switchable_calls: u32,
}

pub trait Cartridge {
    fn read_rom(&self, addr: u16) -> u8;
    fn read_ram(&self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, value: u8);
    /// Returns the currently mapped ROM bank number for the switchable window (0x4000–0x7FFF).
    fn current_rom_bank(&self) -> usize { 1 }
    /// Advance the cartridge clock by `cycles` T-cycles (4 MHz). Only meaningful for
    /// MBC3 carts with an RTC; other implementations ignore this.
    fn tick_rtc(&mut self, _cycles: u32) {}
    /// Returns the full external RAM contents, or `None` if this cart has no battery-backed RAM.
    fn external_ram(&self) -> Option<&[u8]> { None }
    /// Overwrites the full external RAM from the given bytes. No-op if cart has no external RAM.
    fn set_external_ram(&mut self, _data: &[u8]) {}
    /// Serialize MBC register state (bank numbers, mode bits, etc.) into `out`.
    /// Does NOT include ROM or RAM contents — those are saved separately.
    fn save_mbc_state(&self, _out: &mut Vec<u8>) {}
    /// Restore MBC register state from `data` starting at `offset`.
    /// Returns number of bytes consumed. Default: 0 (no state).
    fn load_mbc_state(&mut self, _data: &[u8], _offset: usize) -> usize { 0 }
    #[cfg(feature = "perf")]
    fn take_perf_profile(&mut self) -> CartridgePerfProfile {
        CartridgePerfProfile::default()
    }
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
/// returns a `NoMbc`, `Mbc1`, `Mbc1Multicart`, or `Mbc3` accordingly.
/// Panics on unsupported types.
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
        // MBC3+TIMER+BATTERY, MBC3+TIMER+RAM+BATTERY, MBC3, MBC3+RAM, MBC3+RAM+BATTERY
        0x0F | 0x10 | 0x11 | 0x12 | 0x13 => {
            let has_timer = matches!(cart_type, 0x0F | 0x10);
            Box::new(Mbc3::new(data, ram_bytes, has_timer))
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
    fn external_ram(&self) -> Option<&[u8]> {
        if self.ram.is_empty() { None } else { Some(&self.ram) }
    }
    fn set_external_ram(&mut self, data: &[u8]) {
        let len = self.ram.len().min(data.len());
        self.ram[..len].copy_from_slice(&data[..len]);
    }
    fn current_rom_bank(&self) -> usize { self.rom_bank() }

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
                let enabled = value & 0x0F == 0x0A;
                if self.ram_enabled != enabled {
                    self.ram_enabled = enabled;
                }
            }
            // ROM bank number (lower 5 bits)
            0x2000..=0x3FFF => {
                let mut bank = value & 0x1F;
                // Writing 0 is treated as 1
                if bank == 0 {
                    bank = 1;
                }
                if self.rom_bank_lo != bank {
                    self.rom_bank_lo = bank;
                }
            }
            // Upper bits register (2 bits)
            0x4000..=0x5FFF => {
                let bits = value & 0x03;
                if self.upper_bits != bits {
                    self.upper_bits = bits;
                }
            }
            // Banking mode select
            0x6000..=0x7FFF => {
                let ram_mode = value & 0x01 != 0;
                if self.ram_mode != ram_mode {
                    self.ram_mode = ram_mode;
                }
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

    fn save_mbc_state(&self, out: &mut Vec<u8>) {
        out.push(self.rom_bank_lo);
        out.push(self.upper_bits);
        out.push(self.ram_mode as u8);
        out.push(self.ram_enabled as u8);
    }

    fn load_mbc_state(&mut self, data: &[u8], offset: usize) -> usize {
        if data.len() < offset + 4 { return 0; }
        self.rom_bank_lo = data[offset].max(1); // 0→1 quirk
        self.upper_bits  = data[offset + 1] & 0x03;
        self.ram_mode    = data[offset + 2] != 0;
        self.ram_enabled = data[offset + 3] != 0;
        4
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
    fn external_ram(&self) -> Option<&[u8]> {
        if self.ram.is_empty() { None } else { Some(&self.ram) }
    }
    fn set_external_ram(&mut self, data: &[u8]) {
        let len = self.ram.len().min(data.len());
        self.ram[..len].copy_from_slice(&data[..len]);
    }
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
                let enabled = value & 0x0F == 0x0A;
                if self.ram_enabled != enabled {
                    self.ram_enabled = enabled;
                }
            }
            // Lower bank register: 4-bit effective, 0→1 alias preserved
            0x2000..=0x3FFF => {
                let mut bank = value & 0x1F;
                if bank == 0 {
                    bank = 1;
                }
                if self.rom_bank_lo != bank {
                    self.rom_bank_lo = bank;
                }
            }
            0x4000..=0x5FFF => {
                let bits = value & 0x03;
                if self.upper_bits != bits {
                    self.upper_bits = bits;
                }
            }
            0x6000..=0x7FFF => {
                let ram_mode = value & 0x01 != 0;
                if self.ram_mode != ram_mode {
                    self.ram_mode = ram_mode;
                }
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

    fn save_mbc_state(&self, out: &mut Vec<u8>) {
        out.push(self.rom_bank_lo);
        out.push(self.upper_bits);
        out.push(self.ram_mode as u8);
        out.push(self.ram_enabled as u8);
    }

    fn load_mbc_state(&mut self, data: &[u8], offset: usize) -> usize {
        if data.len() < offset + 4 { return 0; }
        self.rom_bank_lo = data[offset].max(1);
        self.upper_bits  = data[offset + 1] & 0x03;
        self.ram_mode    = data[offset + 2] != 0;
        self.ram_enabled = data[offset + 3] != 0;
        4
    }
}

// ── MBC3 ─────────────────────────────────────────────────────────────────────

/// MBC3 real-time clock registers, latched on command.
///
/// Register map (selected via 0x4000–0x5FFF write of 0x08–0x0C):
///   0x08  RTC S   — seconds  (0–59)
///   0x09  RTC M   — minutes  (0–59)
///   0x0A  RTC H   — hours    (0–23)
///   0x0B  RTC DL  — day counter low byte
///   0x0C  RTC DH  — day counter high bit + halt flag + day carry flag
///
/// The latch register works by writing 0x00 then 0x01 to 0x6000–0x7FFF.
/// On the second write the current RTC time is copied into the latched
/// registers, which are what the game actually reads.
///
/// Pan Docs reference: <https://gbdev.io/pandocs/MBC3.html>
#[derive(Clone, Default)]
struct RtcRegisters {
    sec: u8,
    min: u8,
    hour: u8,
    day_lo: u8,
    /// Bit 0: day counter bit 8.  Bit 6: halt.  Bit 7: day carry.
    day_hi: u8,
}

impl RtcRegisters {
    fn read(&self, reg: u8) -> u8 {
        match reg {
            0x08 => self.sec,
            0x09 => self.min,
            0x0A => self.hour,
            0x0B => self.day_lo,
            0x0C => self.day_hi,
            _ => 0xFF,
        }
    }

    fn write(&mut self, reg: u8, value: u8) {
        match reg {
            0x08 => self.sec  = value & 0x3F,
            0x09 => self.min  = value & 0x3F,
            0x0A => self.hour = value & 0x1F,
            0x0B => self.day_lo = value,
            0x0C => self.day_hi = value & 0xC1, // bits 0, 6, 7 only
            _ => {}
        }
    }

}

/// MBC3 memory bank controller with optional real-time clock.
///
/// Register map (writes to ROM space):
///   0x0000–0x1FFF  RAM/timer enable: 0x0A enables, anything else disables
///   0x2000–0x3FFF  ROM bank number (7-bit, 0→1)
///   0x4000–0x5FFF  RAM bank (0x00–0x03) or RTC register select (0x08–0x0C)
///   0x6000–0x7FFF  Latch clock: write 0x00 then 0x01 to latch RTC

/// Increments `val` by 1. Resets to 0 and returns `true` (carry) if it reaches `limit`.
fn inc_with_carry(val: &mut u8, limit: u8) -> bool {
    *val += 1;
    if *val >= limit { *val = 0; true } else { false }
}

pub struct Mbc3 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    /// Currently selected ROM bank (1–127).
    rom_bank: u8,
    /// RAM bank (0–3) or RTC register index (0x08–0x0C).
    bank_or_rtc: u8,
    ram_rtc_enabled: bool,
    /// Whether the RTC peripheral is present.
    has_timer: bool,
    /// Current (running) RTC time.
    rtc: RtcRegisters,
    /// Snapshot of RTC captured on latch.
    rtc_latched: RtcRegisters,
    /// Tracks the latch sequence: waiting for 0x01 after seeing 0x00.
    latch_armed: bool,
    /// Sub-second cycle accumulator (4 MHz ticks).
    rtc_cycles: u32,
}

const RTC_CYCLES_PER_SEC: u32 = 4_194_304;

impl Mbc3 {
    pub fn new(data: Vec<u8>, ram_bytes: usize, has_timer: bool) -> Self {
        Self {
            rom: data,
            ram: vec![0u8; ram_bytes.max(if ram_bytes > 0 { ram_bytes } else { 0 })],
            rom_bank: 1,
            bank_or_rtc: 0,
            ram_rtc_enabled: false,
            has_timer,
            rtc: RtcRegisters::default(),
            rtc_latched: RtcRegisters::default(),
            latch_armed: false,
            rtc_cycles: 0,
        }
    }

    /// Advance the RTC clock by `cycles` CPU cycles.
    ///
    /// Call this once per emulator step with the number of cycles elapsed.
    /// No-op if the cart has no timer or the RTC halt flag is set.
    pub fn tick(&mut self, cycles: u32) {
        if !self.has_timer || self.rtc.day_hi & 0x40 != 0 {
            return;
        }
        self.rtc_cycles += cycles;
        if self.rtc_cycles < RTC_CYCLES_PER_SEC {
            return;
        }
        let secs_elapsed = self.rtc_cycles / RTC_CYCLES_PER_SEC;
        self.rtc_cycles %= RTC_CYCLES_PER_SEC;

        for _ in 0..secs_elapsed {
            if inc_with_carry(&mut self.rtc.sec, 60)
                && inc_with_carry(&mut self.rtc.min, 60)
                && inc_with_carry(&mut self.rtc.hour, 24)
            {
                let day = ((self.rtc.day_hi & 0x01) as u16) << 8 | self.rtc.day_lo as u16;
                let day = day + 1;
                self.rtc.day_lo = day as u8;
                self.rtc.day_hi = (self.rtc.day_hi & 0xFE) | ((day >> 8) as u8 & 0x01);
                if day >= 0x200 {
                    // Day counter overflow: set carry flag, reset counter
                    self.rtc.day_hi = (self.rtc.day_hi | 0x80) & !0x01;
                    self.rtc.day_lo = 0;
                }
            }
        }
    }

    fn is_rtc_reg(bank_or_rtc: u8) -> bool {
        matches!(bank_or_rtc, 0x08..=0x0C)
    }
}

impl Cartridge for Mbc3 {
    fn external_ram(&self) -> Option<&[u8]> {
        if self.ram.is_empty() { None } else { Some(&self.ram) }
    }
    fn set_external_ram(&mut self, data: &[u8]) {
        let len = self.ram.len().min(data.len());
        self.ram[..len].copy_from_slice(&data[..len]);
    }
    fn current_rom_bank(&self) -> usize { self.rom_bank as usize }

    fn tick_rtc(&mut self, cycles: u32) {
        self.tick(cycles);
    }

    fn read_rom(&self, addr: u16) -> u8 {
        let physical = match addr {
            0x0000..=0x3FFF => addr as usize,
            0x4000..=0x7FFF => self.rom_bank as usize * 0x4000 + (addr as usize - 0x4000),
            _ => return 0xFF,
        };
        self.rom.get(physical).copied().unwrap_or(0xFF)
    }

    fn read_ram(&self, addr: u16) -> u8 {
        if !self.ram_rtc_enabled {
            return 0xFF;
        }
        if Self::is_rtc_reg(self.bank_or_rtc) {
            return if self.has_timer { self.rtc_latched.read(self.bank_or_rtc) } else { 0xFF };
        }
        if self.ram.is_empty() {
            return 0xFF;
        }
        let offset = self.bank_or_rtc as usize * 0x2000 + addr as usize;
        self.ram.get(offset).copied().unwrap_or(0xFF)
    }

    fn write(&mut self, addr: u16, value: u8) {
        match addr {
            // RAM/timer enable
            0x0000..=0x1FFF => {
                let enabled = value & 0x0F == 0x0A;
                if self.ram_rtc_enabled != enabled {
                    self.ram_rtc_enabled = enabled;
                }
            }
            // ROM bank number (7-bit, 0→1)
            0x2000..=0x3FFF => {
                let bank = if value & 0x7F == 0 { 1 } else { value & 0x7F };
                if self.rom_bank != bank {
                    self.rom_bank = bank;
                }
            }
            // RAM bank / RTC register select
            0x4000..=0x5FFF => {
                if self.bank_or_rtc != value {
                    self.bank_or_rtc = value;
                }
            }
            // Latch clock data: 0x00 arms, 0x01 latches
            0x6000..=0x7FFF => {
                if value == 0x00 {
                    self.latch_armed = true;
                } else if value == 0x01 && self.latch_armed {
                    self.rtc_latched = self.rtc.clone();
                    self.latch_armed = false;
                } else {
                    self.latch_armed = false;
                }
            }
            // External RAM write
            0xA000..=0xBFFF => {
                if !self.ram_rtc_enabled {
                    return;
                }
                if Self::is_rtc_reg(self.bank_or_rtc) {
                    if self.has_timer {
                        self.rtc.write(self.bank_or_rtc, value);
                    }
                    return;
                }
                if self.ram.is_empty() {
                    return;
                }
                let offset = self.bank_or_rtc as usize * 0x2000 + (addr - 0xA000) as usize;
                if let Some(b) = self.ram.get_mut(offset) {
                    *b = value;
                }
            }
            _ => {}
        }
    }

    fn save_mbc_state(&self, out: &mut Vec<u8>) {
        out.push(self.rom_bank);
        out.push(self.bank_or_rtc);
        out.push(self.ram_rtc_enabled as u8);
        out.push(self.latch_armed as u8);
        // RTC running state
        out.push(self.rtc.sec);
        out.push(self.rtc.min);
        out.push(self.rtc.hour);
        out.push(self.rtc.day_lo);
        out.push(self.rtc.day_hi);
        // RTC latched state
        out.push(self.rtc_latched.sec);
        out.push(self.rtc_latched.min);
        out.push(self.rtc_latched.hour);
        out.push(self.rtc_latched.day_lo);
        out.push(self.rtc_latched.day_hi);
        out.extend_from_slice(&self.rtc_cycles.to_le_bytes());
    }

    fn load_mbc_state(&mut self, data: &[u8], offset: usize) -> usize {
        const SIZE: usize = 18; // 4 + 5 + 5 + 4
        if data.len() < offset + SIZE { return 0; }
        let d = &data[offset..];
        self.rom_bank        = d[0].max(1);
        self.bank_or_rtc     = d[1];
        self.ram_rtc_enabled = d[2] != 0;
        self.latch_armed     = d[3] != 0;
        self.rtc.sec         = d[4];
        self.rtc.min         = d[5];
        self.rtc.hour        = d[6];
        self.rtc.day_lo      = d[7];
        self.rtc.day_hi      = d[8];
        self.rtc_latched.sec    = d[9];
        self.rtc_latched.min    = d[10];
        self.rtc_latched.hour   = d[11];
        self.rtc_latched.day_lo = d[12];
        self.rtc_latched.day_hi = d[13];
        self.rtc_cycles = u32::from_le_bytes([d[14], d[15], d[16], d[17]]);
        SIZE
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

    // ── NoMbc edge cases ─────────────────────────────────────────────────────

    #[test]
    fn no_mbc_read_rom_oob_returns_0xff() {
        // ROM is only 1 byte; any address beyond it should return open-bus 0xFF.
        let cart = NoMbc::new(vec![0x42]);
        assert_eq!(cart.read_rom(0x0001), 0xFF);
        assert_eq!(cart.read_rom(0x7FFF), 0xFF);
    }

    #[test]
    fn no_mbc_write_to_rom_space_is_ignored() {
        let mut cart = NoMbc::new(vec![0u8; 0x8000]);
        // Writes into ROM space (0x0000–0x9FFF) must not corrupt anything.
        cart.write(0x0000, 0xFF);
        cart.write(0x7FFF, 0xFF);
        assert_eq!(cart.read_rom(0x0000), 0x00);
        assert_eq!(cart.read_rom(0x7FFF), 0x00);
    }

    // ── MBC1 edge cases ───────────────────────────────────────────────────────

    #[test]
    fn mbc1_read_rom_oob_address_returns_0xff() {
        let data = make_rom(64, 0x01);
        let cart = Mbc1::new(data, 0);
        // Addresses outside 0x0000–0x7FFF are not part of ROM space.
        assert_eq!(cart.read_rom(0x8000), 0xFF);
        assert_eq!(cart.read_rom(0xFFFF), 0xFF);
    }

    #[test]
    fn mbc1_bank0_remapped_in_ram_mode() {
        let data = make_rom(1024, 0x01); // 64 banks
        let mut cart = Mbc1::new(data, 0);
        cart.write(0x6000, 0x01); // RAM mode
        cart.write(0x4000, 0x01); // upper = 1 → bank0 window remaps to bank 32
        // In RAM mode the fixed window shows the start of the 32-bank group.
        assert_eq!(cart.read_rom(0x0000), 32);
    }

    #[test]
    fn mbc1_bank0_not_remapped_in_rom_mode() {
        let data = make_rom(1024, 0x01); // 64 banks
        let mut cart = Mbc1::new(data, 0);
        // ROM mode (default): upper bits do not affect bank0 window.
        cart.write(0x4000, 0x03); // upper = 3
        assert_eq!(cart.read_rom(0x0000), 0x00);
    }

    #[test]
    fn mbc1_ram_disable_after_enable() {
        let data = make_rom(64, 0x02);
        let mut cart = Mbc1::new(data, 8 * 1024);
        cart.write(0x0000, 0x0A); // enable
        cart.write(0xA000, 0x77);
        cart.write(0x0000, 0x00); // disable
        assert_eq!(cart.read_ram(0x0000), 0xFF); // disabled → open bus
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

    #[test]
    fn from_rom_mbc1_ram_for_type_02() {
        let mut data = make_rom(64, 0x02);
        data[RAM_SIZE_ADDR] = 0x02; // 8 KiB RAM
        let mut cart = from_rom(data);
        cart.write(0x0000, 0x0A); // enable RAM
        cart.write(0xA000, 0xCC);
        assert_eq!(cart.read_ram(0x0000), 0xCC);
    }

    #[test]
    fn from_rom_mbc1_battery_for_type_03() {
        let mut data = make_rom(64, 0x03);
        data[RAM_SIZE_ADDR] = 0x02; // 8 KiB RAM
        let mut cart = from_rom(data);
        cart.write(0x0000, 0x0A);
        cart.write(0xA000, 0xDD);
        assert_eq!(cart.read_ram(0x0000), 0xDD);
    }

    // ── Multicart detection ───────────────────────────────────────────────────

    fn make_multicart_rom() -> Vec<u8> {
        let mut data = make_rom(1024, 0x01); // 64 banks
        // Plant the Nintendo logo at banks 0x10, 0x20, 0x30.
        for &bank in &[0x10usize, 0x20, 0x30] {
            let base = bank * 0x4000 + 0x0104;
            data[base..base + NINTENDO_LOGO.len()].copy_from_slice(&NINTENDO_LOGO);
        }
        data
    }

    #[test]
    fn from_rom_detects_multicart_by_logo_heuristic() {
        let data = make_multicart_rom();
        let mut cart = from_rom(data);
        // Multicart: BANK2=1 shifts by 4, so bank = (1<<4)|1 = 17.
        cart.write(0x4000, 0x01); // upper = 1
        cart.write(0x2000, 0x01); // lower = 1
        assert_eq!(cart.read_rom(0x4000), 17);
    }

    #[test]
    fn from_rom_no_multicart_without_logos() {
        // 64-bank MBC1 but logos missing → normal MBC1 (upper << 5 shift).
        let data = make_rom(1024, 0x01);
        let mut cart = from_rom(data);
        cart.write(0x4000, 0x01); // upper = 1
        cart.write(0x2000, 0x01); // lower = 1
        // Normal MBC1: bank = (1<<5)|1 = 33.
        assert_eq!(cart.read_rom(0x4000), 33);
    }

    #[test]
    fn from_rom_no_multicart_for_non_64bank_rom() {
        // 32-bank MBC1 with logos at 0x10 etc. → not multicart (wrong bank count).
        let mut data = make_rom(512, 0x01); // 32 banks
        for &bank in &[0x10usize, 0x20, 0x30] {
            let base = bank * 0x4000 + 0x0104;
            if base + NINTENDO_LOGO.len() <= data.len() {
                data[base..base + NINTENDO_LOGO.len()].copy_from_slice(&NINTENDO_LOGO);
            }
        }
        let mut cart = from_rom(data);
        cart.write(0x4000, 0x01);
        cart.write(0x2000, 0x01);
        // Normal MBC1: bank = (1<<5)|1 = 33, masked to 32 banks → 33 % 32 = 1.
        assert_eq!(cart.read_rom(0x4000), 1);
    }

    // ── MBC3 ─────────────────────────────────────────────────────────────────

    fn make_mbc3_rom(size_kb: usize, cart_type: u8) -> Vec<u8> {
        let size = size_kb * 1024;
        let mut data = vec![0u8; size];
        for bank in 0..(size / 0x4000) {
            for byte in &mut data[bank * 0x4000..(bank + 1) * 0x4000] {
                *byte = bank as u8;
            }
        }
        data[CART_TYPE_ADDR] = cart_type;
        let code = (size / (32 * 1024)).trailing_zeros() as u8;
        data[ROM_SIZE_ADDR] = code;
        data
    }

    #[test]
    fn mbc3_default_reads_bank0_and_bank1() {
        let data = make_mbc3_rom(1024, 0x13);
        let cart = Mbc3::new(data, 0, false);
        assert_eq!(cart.read_rom(0x0000), 0x00); // fixed bank 0
        assert_eq!(cart.read_rom(0x4000), 0x01); // switchable bank 1 (default)
    }

    #[test]
    fn mbc3_switches_rom_bank() {
        let data = make_mbc3_rom(1024, 0x13);
        let mut cart = Mbc3::new(data, 0, false);
        cart.write(0x2000, 0x05);
        assert_eq!(cart.read_rom(0x4000), 0x05);
    }

    #[test]
    fn mbc3_bank_0_write_selects_bank1() {
        let data = make_mbc3_rom(1024, 0x13);
        let mut cart = Mbc3::new(data, 0, false);
        cart.write(0x2000, 0x00);
        assert_eq!(cart.read_rom(0x4000), 0x01);
    }

    #[test]
    fn mbc3_full_7bit_bank_range() {
        let data = make_mbc3_rom(8192, 0x13); // 256 banks (more than 7-bit, but test 0x7F)
        let mut cart = Mbc3::new(data, 0, false);
        cart.write(0x2000, 0x7F);
        assert_eq!(cart.read_rom(0x4000), 0x7F);
    }

    #[test]
    fn mbc3_ram_disabled_returns_0xff() {
        let data = make_mbc3_rom(1024, 0x13);
        let cart = Mbc3::new(data, 32 * 1024, false);
        assert_eq!(cart.read_ram(0x0000), 0xFF);
    }

    #[test]
    fn mbc3_ram_enable_and_write() {
        let data = make_mbc3_rom(1024, 0x13);
        let mut cart = Mbc3::new(data, 32 * 1024, false);
        cart.write(0x0000, 0x0A); // enable
        cart.write(0x4000, 0x00); // select RAM bank 0
        cart.write(0xA000, 0x42);
        assert_eq!(cart.read_ram(0x0000), 0x42);
    }

    #[test]
    fn mbc3_ram_banking() {
        let data = make_mbc3_rom(1024, 0x13);
        let mut cart = Mbc3::new(data, 32 * 1024, false);
        cart.write(0x0000, 0x0A); // enable
        cart.write(0x4000, 0x01); // bank 1
        cart.write(0xA000, 0xBB);
        cart.write(0x4000, 0x00); // bank 0
        assert_eq!(cart.read_ram(0x0000), 0x00); // bank 0 untouched
        cart.write(0x4000, 0x01);
        assert_eq!(cart.read_ram(0x0000), 0xBB);
    }

    #[test]
    fn mbc3_rtc_latch_and_read() {
        let data = make_mbc3_rom(1024, 0x0F); // MBC3+TIMER+BATTERY
        let mut cart = Mbc3::new(data, 0, true);
        // Advance 2 seconds worth of cycles
        cart.tick(RTC_CYCLES_PER_SEC * 2);
        // Latch the time
        cart.write(0x6000, 0x00);
        cart.write(0x6000, 0x01);
        // Select RTC seconds register
        cart.write(0x4000, 0x08);
        cart.write(0x0000, 0x0A); // enable RAM/RTC
        assert_eq!(cart.read_ram(0x0000), 2);
    }

    #[test]
    fn mbc3_rtc_latch_freezes_time() {
        let data = make_mbc3_rom(1024, 0x0F);
        let mut cart = Mbc3::new(data, 0, true);
        cart.tick(RTC_CYCLES_PER_SEC * 3);
        // Latch at 3 seconds
        cart.write(0x6000, 0x00);
        cart.write(0x6000, 0x01);
        // Advance more — latched value should not change
        cart.tick(RTC_CYCLES_PER_SEC * 5);
        cart.write(0x4000, 0x08);
        cart.write(0x0000, 0x0A);
        assert_eq!(cart.read_ram(0x0000), 3); // still reads 3
    }

    #[test]
    fn mbc3_rtc_minute_rollover() {
        let data = make_mbc3_rom(1024, 0x0F);
        let mut cart = Mbc3::new(data, 0, true);
        cart.tick(RTC_CYCLES_PER_SEC * 60);
        cart.write(0x6000, 0x00);
        cart.write(0x6000, 0x01);
        cart.write(0x0000, 0x0A);
        cart.write(0x4000, 0x08); // seconds
        assert_eq!(cart.read_ram(0x0000), 0); // rolled over
        cart.write(0x4000, 0x09); // minutes
        assert_eq!(cart.read_ram(0x0000), 1);
    }

    #[test]
    fn mbc3_rtc_halt_stops_time() {
        let data = make_mbc3_rom(1024, 0x0F);
        let mut cart = Mbc3::new(data, 0, true);
        cart.tick(RTC_CYCLES_PER_SEC); // 1 second
        // Halt: write 0x40 to DH register (0x0C)
        cart.write(0x0000, 0x0A);
        cart.write(0x4000, 0x0C);
        cart.write(0xA000, 0x40); // set halt flag
        // Advance time — should not change
        cart.tick(RTC_CYCLES_PER_SEC * 10);
        cart.write(0x6000, 0x00);
        cart.write(0x6000, 0x01);
        cart.write(0x4000, 0x08);
        assert_eq!(cart.read_ram(0x0000), 1); // still 1 second
    }

    #[test]
    fn mbc3_no_timer_ignores_rtc_writes() {
        let data = make_mbc3_rom(1024, 0x13); // MBC3+RAM+BATTERY (no timer)
        let mut cart = Mbc3::new(data, 0, false);
        cart.tick(RTC_CYCLES_PER_SEC * 5);
        cart.write(0x6000, 0x00);
        cart.write(0x6000, 0x01);
        cart.write(0x0000, 0x0A);
        cart.write(0x4000, 0x08);
        // No timer → reads 0xFF (RTC disabled, ram_rtc_enabled but no rtc reg)
        assert_eq!(cart.read_ram(0x0000), 0xFF);
    }

    #[test]
    fn from_rom_dispatches_mbc3_types() {
        for &cart_type in &[0x0Fu8, 0x10, 0x11, 0x12, 0x13] {
            let data = make_mbc3_rom(1024, cart_type);
            let mut cart = from_rom(data);
            // Basic sanity: bank switching works
            cart.write(0x2000, 0x02);
            assert_eq!(cart.read_rom(0x4000), 0x02);
        }
    }

    // ── external_ram / set_external_ram ──────────────────────────────────────

    #[test]
    fn no_mbc_has_no_external_ram() {
        let cart = NoMbc::new(vec![0u8; 0x8000]);
        assert!(cart.external_ram().is_none());
    }

    #[test]
    fn mbc1_external_ram_roundtrips() {
        let data = make_rom(64, 0x03);
        let mut cart = Mbc1::new(data, 8 * 1024);
        let payload: Vec<u8> = (0u8..=127).collect();
        cart.set_external_ram(&payload);
        let ram = cart.external_ram().expect("mbc1 should have external ram");
        assert_eq!(&ram[..payload.len()], &payload[..]);
    }

    #[test]
    fn mbc3_external_ram_roundtrips() {
        let data = make_mbc3_rom(1024, 0x13);
        let mut cart = Mbc3::new(data, 8 * 1024, false);
        let payload = vec![0xAB; 64];
        cart.set_external_ram(&payload);
        let ram = cart.external_ram().expect("mbc3 should have external ram");
        assert_eq!(&ram[..payload.len()], &payload[..]);
    }

    #[test]
    fn mbc3_no_ram_external_ram_is_none() {
        let data = make_mbc3_rom(1024, 0x11); // MBC3, no RAM
        let cart = Mbc3::new(data, 0, false);
        assert!(cart.external_ram().is_none());
    }

    // ── save_mbc_state / load_mbc_state ──────────────────────────────────────

    #[test]
    fn mbc1_save_load_mbc_state_roundtrip() {
        let data = make_rom(1024, 0x03); // 64 banks, 4 RAM banks
        let mut cart = Mbc1::new(data.clone(), 32 * 1024);
        // Set a non-default state: bank 5, upper=1, RAM mode, RAM enabled
        cart.write(0x2000, 0x05);
        cart.write(0x4000, 0x01);
        cart.write(0x6000, 0x01); // RAM mode
        cart.write(0x0000, 0x0A); // enable RAM

        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);
        assert_eq!(blob.len(), 4);

        let mut cart2 = Mbc1::new(data, 32 * 1024);
        let consumed = cart2.load_mbc_state(&blob, 0);
        assert_eq!(consumed, 4);

        // Verify ROM bank restored
        assert_eq!(cart2.read_rom(0x4000), cart.read_rom(0x4000));
        // Verify RAM mode restored (upper bits select RAM bank 1 in RAM mode)
        assert_eq!(cart2.read_rom(0x0000), cart.read_rom(0x0000));
    }

    #[test]
    fn mbc1_load_mbc_state_with_offset() {
        let data = make_rom(64, 0x01);
        let mut cart = Mbc1::new(data.clone(), 0);
        cart.write(0x2000, 0x03);

        let mut blob = vec![0xFFu8; 10]; // 10 bytes padding
        cart.save_mbc_state(&mut blob);

        let mut cart2 = Mbc1::new(data, 0);
        let consumed = cart2.load_mbc_state(&blob, 10); // read from offset 10
        assert_eq!(consumed, 4);
        assert_eq!(cart2.read_rom(0x4000), 3);
    }

    #[test]
    fn mbc1_load_mbc_state_too_short_returns_zero() {
        let data = make_rom(64, 0x01);
        let mut cart = Mbc1::new(data, 0);
        let consumed = cart.load_mbc_state(&[0u8; 2], 0); // only 2 bytes, need 4
        assert_eq!(consumed, 0);
    }

    #[test]
    fn mbc3_save_load_mbc_state_roundtrip() {
        let data = make_mbc3_rom(1024, 0x0F); // with timer
        let mut cart = Mbc3::new(data.clone(), 32 * 1024, true);
        cart.write(0x2000, 0x0A);        // ROM bank 10
        cart.write(0x4000, 0x02);        // RAM bank 2
        cart.write(0x0000, 0x0A);        // enable
        cart.tick(RTC_CYCLES_PER_SEC * 7); // advance 7 seconds

        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);
        assert_eq!(blob.len(), 18);

        let mut cart2 = Mbc3::new(data, 32 * 1024, true);
        let consumed = cart2.load_mbc_state(&blob, 0);
        assert_eq!(consumed, 18);

        // ROM bank restored
        assert_eq!(cart2.read_rom(0x4000), cart.read_rom(0x4000));
    }

    #[test]
    fn mbc3_load_mbc_state_too_short_returns_zero() {
        let data = make_mbc3_rom(1024, 0x13);
        let mut cart = Mbc3::new(data, 0, false);
        let consumed = cart.load_mbc_state(&[0u8; 10], 0); // need 18
        assert_eq!(consumed, 0);
    }

    #[test]
    fn no_mbc_save_load_mbc_state_is_noop() {
        let mut cart = NoMbc::new(vec![0u8; 0x8000]);
        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);
        assert_eq!(blob.len(), 0);
        let consumed = cart.load_mbc_state(&[1, 2, 3], 0);
        assert_eq!(consumed, 0);
    }

    // ── Multicart save/load state ─────────────────────────────────────────────

    #[test]
    fn mbc1_multicart_save_load_mbc_state_roundtrip() {
        let data = make_multicart_rom();
        let mut cart = from_rom(data.clone());
        // Select sub-game 1 (upper=1) bank 3 (lower=3) → bank (1<<4)|3 = 19
        cart.write(0x4000, 0x01);
        cart.write(0x2000, 0x03);
        assert_eq!(cart.read_rom(0x4000), 19);

        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);

        let mut cart2 = from_rom(data);
        cart2.load_mbc_state(&blob, 0);
        assert_eq!(cart2.read_rom(0x4000), 19);
    }

    // ── NoMbc set_external_ram is no-op ──────────────────────────────────────

    #[test]
    fn no_mbc_set_external_ram_is_noop() {
        let mut cart = NoMbc::new(vec![0u8; 0x8000]);
        cart.set_external_ram(&[0xAB; 16]); // should not panic
        assert!(cart.external_ram().is_none());
    }
}
