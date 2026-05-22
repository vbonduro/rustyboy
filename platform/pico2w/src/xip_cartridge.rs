use alloc::vec::Vec;

use rustyboy_core::memory::cartridge::{Cartridge, CartridgeRomWindows};

#[cfg(target_arch = "arm")]
use crate::flash_rom::{FlashRomInfo, ROM_DATA_OFFSET};
#[cfg(target_arch = "arm")]
use embassy_rp::flash::FLASH_BASE;

const ROM_BANK_BYTES: usize = 0x4000;
const CART_TYPE: usize = 0x0147;
const ROM_SIZE: usize = 0x0148;
const RAM_SIZE: usize = 0x0149;

#[derive(Clone, Copy)]
struct RomMapping {
    fixed_bank: usize,
    switchable_bank: usize,
}

struct NoMbc;

struct Mbc1 {
    rom_bank_lo: u8,
    upper_bits: u8,
    ram_mode: bool,
    ram_enabled: bool,
    ram_bank_count: usize,
}

struct Mbc3 {
    rom_bank: u8,
    bank_or_rtc: u8,
    ram_rtc_enabled: bool,
}

struct Mbc5 {
    rom_bank_lo: u8,
    rom_bank_hi: u8,
    ram_bank: u8,
    ram_enabled: bool,
    rumble: bool,
}

enum MbcState {
    NoMbc(NoMbc),
    Mbc1(Mbc1),
    Mbc3(Mbc3),
    Mbc5(Mbc5),
}

impl NoMbc {
    fn rom_mapping(&self, _rom_bank_count: usize) -> RomMapping {
        RomMapping {
            fixed_bank: 0,
            switchable_bank: 1,
        }
    }

    fn write_register(&mut self, _addr: u16, _value: u8) -> bool {
        false
    }

    fn ram_bank(&self) -> usize {
        0
    }

    fn ram_enabled(&self) -> bool {
        false
    }

    fn save_state(&self, _out: &mut Vec<u8>) {}

    fn load_state(&mut self, _data: &[u8], _offset: usize) -> usize {
        0
    }
}

impl Mbc1 {
    fn new(ram_bytes: usize) -> Self {
        let ram_bank_count = if ram_bytes == 0 {
            0
        } else {
            (ram_bytes / 0x2000).max(1)
        };
        Self {
            rom_bank_lo: 1,
            upper_bits: 0,
            ram_mode: false,
            ram_enabled: false,
            ram_bank_count,
        }
    }

    fn rom_mapping(&self, rom_bank_count: usize) -> RomMapping {
        let fixed_bank = if self.ram_mode {
            ((self.upper_bits as usize) << 5) % rom_bank_count
        } else {
            0
        };
        let bank = ((self.upper_bits as usize) << 5) | (self.rom_bank_lo as usize);
        let bank = if bank == 0 { 1 } else { bank };
        RomMapping {
            fixed_bank,
            switchable_bank: bank % rom_bank_count,
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) -> bool {
        match addr {
            0x0000..=0x1FFF => {
                self.ram_enabled = value & 0x0F == 0x0A;
                false
            }
            0x2000..=0x3FFF => {
                let mut bank = value & 0x1F;
                if bank == 0 {
                    bank = 1;
                }
                if self.rom_bank_lo == bank {
                    false
                } else {
                    self.rom_bank_lo = bank;
                    true
                }
            }
            0x4000..=0x5FFF => {
                let bits = value & 0x03;
                if self.upper_bits == bits {
                    false
                } else {
                    self.upper_bits = bits;
                    true
                }
            }
            0x6000..=0x7FFF => {
                let new_ram_mode = value & 0x01 != 0;
                if self.ram_mode == new_ram_mode {
                    false
                } else {
                    self.ram_mode = new_ram_mode;
                    true
                }
            }
            _ => false,
        }
    }

    fn ram_bank(&self) -> usize {
        if self.ram_mode {
            (self.upper_bits as usize) % self.ram_bank_count.max(1)
        } else {
            0
        }
    }

    fn ram_enabled(&self) -> bool {
        self.ram_enabled
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[
            self.rom_bank_lo,
            self.upper_bits,
            self.ram_mode as u8,
            self.ram_enabled as u8,
        ]);
    }

    fn load_state(&mut self, data: &[u8], offset: usize) -> usize {
        if data.len() < offset + 4 {
            return 0;
        }
        self.rom_bank_lo = data[offset].max(1);
        self.upper_bits = data[offset + 1] & 0x03;
        self.ram_mode = data[offset + 2] != 0;
        self.ram_enabled = data[offset + 3] != 0;
        4
    }
}

impl Mbc3 {
    fn new() -> Self {
        Self {
            rom_bank: 1,
            bank_or_rtc: 0,
            ram_rtc_enabled: false,
        }
    }

    fn rom_mapping(&self, _rom_bank_count: usize) -> RomMapping {
        RomMapping {
            fixed_bank: 0,
            switchable_bank: self.rom_bank as usize,
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) -> bool {
        match addr {
            0x0000..=0x1FFF => {
                self.ram_rtc_enabled = value & 0x0F == 0x0A;
                false
            }
            0x2000..=0x3FFF => {
                let bank = if value & 0x7F == 0 { 1 } else { value & 0x7F };
                if self.rom_bank == bank {
                    false
                } else {
                    self.rom_bank = bank;
                    true
                }
            }
            0x4000..=0x5FFF => {
                self.bank_or_rtc = value;
                false
            }
            _ => false,
        }
    }

    fn ram_bank(&self) -> usize {
        self.bank_or_rtc as usize
    }

    fn ram_enabled(&self) -> bool {
        self.ram_rtc_enabled && !matches!(self.bank_or_rtc, 0x08..=0x0C)
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        // RBSS v1 parses a fixed 4-byte MBC block before cart RAM.
        // Keep the Pico MBC3 save payload aligned with that format.
        out.extend_from_slice(&[
            self.rom_bank,
            self.bank_or_rtc,
            self.ram_rtc_enabled as u8,
            0,
        ]);
    }

    fn load_state(&mut self, data: &[u8], offset: usize) -> usize {
        if data.len() < offset + 3 {
            return 0;
        }
        self.rom_bank = data[offset].max(1);
        self.bank_or_rtc = data[offset + 1];
        self.ram_rtc_enabled = data[offset + 2] != 0;
        if data.len() >= offset + 4 {
            4
        } else {
            3
        }
    }
}

impl Mbc5 {
    fn new(rumble: bool) -> Self {
        Self {
            rom_bank_lo: 1,
            rom_bank_hi: 0,
            ram_bank: 0,
            ram_enabled: false,
            rumble,
        }
    }

    fn rom_mapping(&self, rom_bank_count: usize) -> RomMapping {
        let bank = ((self.rom_bank_hi as usize) << 8) | self.rom_bank_lo as usize;
        RomMapping {
            fixed_bank: 0,
            switchable_bank: bank % rom_bank_count,
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) -> bool {
        match addr {
            0x0000..=0x1FFF => {
                self.ram_enabled = value & 0x0F == 0x0A;
                false
            }
            0x2000..=0x2FFF => {
                if self.rom_bank_lo == value {
                    false
                } else {
                    self.rom_bank_lo = value;
                    true
                }
            }
            0x3000..=0x3FFF => {
                let bit = value & 0x01;
                if self.rom_bank_hi == bit {
                    false
                } else {
                    self.rom_bank_hi = bit;
                    true
                }
            }
            0x4000..=0x5FFF => {
                self.ram_bank = value & 0x0F;
                false
            }
            _ => false,
        }
    }

    fn ram_bank(&self) -> usize {
        if self.rumble {
            (self.ram_bank & 0x07) as usize
        } else {
            (self.ram_bank & 0x0F) as usize
        }
    }

    fn ram_enabled(&self) -> bool {
        self.ram_enabled
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[
            self.rom_bank_lo,
            self.rom_bank_hi,
            self.ram_bank,
            self.ram_enabled as u8,
        ]);
    }

    fn load_state(&mut self, data: &[u8], offset: usize) -> usize {
        if data.len() < offset + 4 {
            return 0;
        }
        self.rom_bank_lo = data[offset];
        self.rom_bank_hi = data[offset + 1] & 0x01;
        self.ram_bank = data[offset + 2] & 0x0F;
        self.ram_enabled = data[offset + 3] != 0;
        4
    }
}

impl MbcState {
    fn from_header(cart_type: u8, ram_bytes: usize) -> Option<Self> {
        match cart_type {
            0x00 => Some(Self::NoMbc(NoMbc)),
            0x01 | 0x02 | 0x03 => Some(Self::Mbc1(Mbc1::new(ram_bytes))),
            0x0F | 0x10 | 0x11 | 0x12 | 0x13 => Some(Self::Mbc3(Mbc3::new())),
            0x19 | 0x1A | 0x1B | 0x1C | 0x1D | 0x1E => Some(Self::Mbc5(Mbc5::new(matches!(
                cart_type,
                0x1C | 0x1D | 0x1E
            )))),
            _ => None,
        }
    }

    fn rom_mapping(&self, rom_bank_count: usize) -> RomMapping {
        match self {
            Self::NoMbc(mbc) => mbc.rom_mapping(rom_bank_count),
            Self::Mbc1(mbc) => mbc.rom_mapping(rom_bank_count),
            Self::Mbc3(mbc) => mbc.rom_mapping(rom_bank_count),
            Self::Mbc5(mbc) => mbc.rom_mapping(rom_bank_count),
        }
    }

    fn write_register(&mut self, addr: u16, value: u8) -> bool {
        match self {
            Self::NoMbc(mbc) => mbc.write_register(addr, value),
            Self::Mbc1(mbc) => mbc.write_register(addr, value),
            Self::Mbc3(mbc) => mbc.write_register(addr, value),
            Self::Mbc5(mbc) => mbc.write_register(addr, value),
        }
    }

    fn ram_bank(&self) -> usize {
        match self {
            Self::NoMbc(mbc) => mbc.ram_bank(),
            Self::Mbc1(mbc) => mbc.ram_bank(),
            Self::Mbc3(mbc) => mbc.ram_bank(),
            Self::Mbc5(mbc) => mbc.ram_bank(),
        }
    }

    fn ram_enabled(&self) -> bool {
        match self {
            Self::NoMbc(mbc) => mbc.ram_enabled(),
            Self::Mbc1(mbc) => mbc.ram_enabled(),
            Self::Mbc3(mbc) => mbc.ram_enabled(),
            Self::Mbc5(mbc) => mbc.ram_enabled(),
        }
    }

    fn save_state(&self, out: &mut Vec<u8>) {
        match self {
            Self::NoMbc(mbc) => mbc.save_state(out),
            Self::Mbc1(mbc) => mbc.save_state(out),
            Self::Mbc3(mbc) => mbc.save_state(out),
            Self::Mbc5(mbc) => mbc.save_state(out),
        }
    }

    fn load_state(&mut self, data: &[u8], offset: usize) -> usize {
        match self {
            Self::NoMbc(mbc) => mbc.load_state(data, offset),
            Self::Mbc1(mbc) => mbc.load_state(data, offset),
            Self::Mbc3(mbc) => mbc.load_state(data, offset),
            Self::Mbc5(mbc) => mbc.load_state(data, offset),
        }
    }
}

#[derive(Debug)]
pub enum XipCartridgeError {
    RomTooSmall {
        expected_bytes: usize,
        actual_bytes: usize,
    },
    UnsupportedCartType(u8),
}

pub struct XipCartridge {
    rom: &'static [u8],
    fixed_bank_num: usize,
    fixed_bank_base: usize,
    fixed_bank_valid: bool,
    current_bank_num: usize,
    current_bank_base: usize,
    current_bank_valid: bool,
    rom_bank_count: usize,
    mbc: MbcState,
    ram: Vec<u8>,
}

impl XipCartridge {
    pub fn new(rom: &'static [u8]) -> Result<Self, XipCartridgeError> {
        let cart_type = *rom.get(CART_TYPE).unwrap_or(&0);
        let rom_bank_count = rom_bank_count_from_code(*rom.get(ROM_SIZE).unwrap_or(&0))
            .ok_or(XipCartridgeError::UnsupportedCartType(cart_type))?;
        let ram_bytes = ram_bytes_from_code(*rom.get(RAM_SIZE).unwrap_or(&0));
        let expected_bytes = rom_bank_count * ROM_BANK_BYTES;
        if rom.len() < expected_bytes {
            return Err(XipCartridgeError::RomTooSmall {
                expected_bytes,
                actual_bytes: rom.len(),
            });
        }
        let mbc = mbc_state_from_header(cart_type, ram_bytes)
            .ok_or(XipCartridgeError::UnsupportedCartType(cart_type))?;

        let mut cart = Self {
            rom,
            fixed_bank_num: 0,
            fixed_bank_base: 0,
            fixed_bank_valid: true,
            current_bank_num: 1,
            current_bank_base: ROM_BANK_BYTES,
            current_bank_valid: rom_bank_count > 1,
            rom_bank_count,
            mbc,
            ram: alloc::vec![0u8; effective_ram_bytes(cart_type, ram_bytes)],
        };
        cart.refresh_mappings();
        Ok(cart)
    }

    #[cfg(target_arch = "arm")]
    pub fn from_staged_flash(info: FlashRomInfo) -> Result<Self, XipCartridgeError> {
        let rom = unsafe {
            // The staged ROM lives in a stable XIP-mapped flash window for the
            // whole program lifetime, and all staging writes are complete
            // before we create the cartridge.
            core::slice::from_raw_parts(
                (FLASH_BASE as *const u8).add(ROM_DATA_OFFSET),
                info.size_bytes,
            )
        };
        Self::new(rom)
    }

    #[inline]
    fn refresh_mappings(&mut self) {
        let mapping = self.mbc.rom_mapping(self.rom_bank_count);

        self.fixed_bank_num = mapping.fixed_bank;
        self.fixed_bank_valid = mapping.fixed_bank < self.rom_bank_count;
        self.fixed_bank_base = mapping.fixed_bank * ROM_BANK_BYTES;

        self.current_bank_num = mapping.switchable_bank;
        self.current_bank_valid = mapping.switchable_bank < self.rom_bank_count;
        self.current_bank_base = mapping.switchable_bank * ROM_BANK_BYTES;
    }

    #[inline(always)]
    fn rom_window(&self, base: usize, valid: bool) -> (*const u8, usize) {
        if !valid {
            return (core::ptr::null(), 0);
        }
        match self.rom.get(base..) {
            Some(slice) => (slice.as_ptr(), slice.len().min(ROM_BANK_BYTES)),
            None => (core::ptr::null(), 0),
        }
    }
}

impl Cartridge for XipCartridge {
    fn rom_windows(&self) -> Option<CartridgeRomWindows> {
        let (fixed_ptr, fixed_len) = self.rom_window(self.fixed_bank_base, self.fixed_bank_valid);
        let (banked_ptr, banked_len) =
            self.rom_window(self.current_bank_base, self.current_bank_valid);
        Some(CartridgeRomWindows {
            fixed_ptr,
            fixed_len,
            banked_ptr,
            banked_len,
        })
    }

    #[inline(always)]
    fn read_rom(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => {
                if !self.fixed_bank_valid {
                    return 0xFF;
                }
                unsafe { *self.rom.get_unchecked(self.fixed_bank_base + addr as usize) }
            }
            0x4000..=0x7FFF => {
                if !self.current_bank_valid {
                    return 0xFF;
                }
                unsafe {
                    *self
                        .rom
                        .get_unchecked(self.current_bank_base + (addr as usize - 0x4000))
                }
            }
            _ => 0xFF,
        }
    }

    fn read_ram(&self, addr: u16) -> u8 {
        if !self.mbc.ram_enabled() || self.ram.is_empty() {
            return 0xFF;
        }
        let offset = self.mbc.ram_bank() * 0x2000 + addr as usize;
        self.ram.get(offset).copied().unwrap_or(0xFF)
    }

    fn write(&mut self, addr: u16, value: u8) {
        if (0xA000..=0xBFFF).contains(&addr) {
            if self.mbc.ram_enabled() && !self.ram.is_empty() {
                let offset = self.mbc.ram_bank() * 0x2000 + (addr - 0xA000) as usize;
                if let Some(b) = self.ram.get_mut(offset) {
                    *b = value;
                }
            }
            return;
        }

        let changed = self.mbc.write_register(addr, value);
        if changed {
            self.refresh_mappings();
        }
    }

    fn current_rom_bank(&self) -> usize {
        self.current_bank_num
    }

    fn external_ram(&self) -> Option<&[u8]> {
        if self.ram.is_empty() {
            None
        } else {
            Some(&self.ram)
        }
    }

    fn set_external_ram(&mut self, data: &[u8]) {
        let len = self.ram.len().min(data.len());
        self.ram[..len].copy_from_slice(&data[..len]);
    }

    fn save_mbc_state(&self, out: &mut Vec<u8>) {
        self.mbc.save_state(out);
    }

    fn load_mbc_state(&mut self, data: &[u8], offset: usize) -> usize {
        let consumed = self.mbc.load_state(data, offset);
        if consumed > 0 {
            self.refresh_mappings();
        }
        consumed
    }
}

fn rom_bank_count_from_code(code: u8) -> Option<usize> {
    match code {
        0x00..=0x08 => Some(2usize << code),
        0x52 => Some(72),
        0x53 => Some(80),
        0x54 => Some(96),
        _ => None,
    }
}

fn ram_bytes_from_code(code: u8) -> usize {
    match code {
        0x01 => 2 * 1024,
        0x02 => 8 * 1024,
        0x03 => 32 * 1024,
        0x04 => 128 * 1024,
        0x05 => 64 * 1024,
        _ => 0,
    }
}

fn effective_ram_bytes(cart_type: u8, ram_bytes: usize) -> usize {
    match cart_type {
        // Match the core MBC1 cartridge path used by the web frontend. Some
        // software touches external RAM even when the header says no RAM.
        0x01 | 0x02 | 0x03 => ram_bytes.max(0x2000),
        _ => ram_bytes,
    }
}

fn mbc_state_from_header(cart_type: u8, ram_bytes: usize) -> Option<MbcState> {
    MbcState::from_header(cart_type, ram_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::boxed::Box;
    use rustyboy_core::memory::{GameBoyMemory, Memory};

    fn rom_size_code_for(num_banks: usize) -> u8 {
        match num_banks {
            2 => 0,
            4 => 1,
            8 => 2,
            16 => 3,
            32 => 4,
            64 => 5,
            128 => 6,
            256 => 7,
            512 => 8,
            _ => 0,
        }
    }

    fn leak_rom(num_banks: usize, cart_type: u8, ram_size_code: u8) -> &'static [u8] {
        let mut rom = alloc::vec![0u8; num_banks * ROM_BANK_BYTES];
        for (i, chunk) in rom.chunks_exact_mut(ROM_BANK_BYTES).enumerate() {
            chunk.fill(i as u8);
        }
        rom[CART_TYPE] = cart_type;
        rom[ROM_SIZE] = rom_size_code_for(num_banks);
        rom[RAM_SIZE] = ram_size_code;
        Box::leak(rom.into_boxed_slice())
    }

    #[test]
    fn mapper_header_dispatch_selects_expected_implementation() {
        assert!(matches!(
            mbc_state_from_header(0x00, 0),
            Some(MbcState::NoMbc(_))
        ));
        assert!(matches!(
            mbc_state_from_header(0x01, 0),
            Some(MbcState::Mbc1(_))
        ));
        assert!(matches!(
            mbc_state_from_header(0x13, 0x8000),
            Some(MbcState::Mbc3(_))
        ));

        let Some(MbcState::Mbc5(plain)) = mbc_state_from_header(0x1B, 0x8000) else {
            panic!("MBC5+RAM+BATTERY should construct an MBC5 mapper");
        };
        assert!(!plain.rumble);

        let Some(MbcState::Mbc5(rumble)) = mbc_state_from_header(0x1E, 0x8000) else {
            panic!("MBC5+RUMBLE+RAM+BATTERY should construct an MBC5 mapper");
        };
        assert!(rumble.rumble);

        assert!(mbc_state_from_header(0xFF, 0).is_none());
    }

    #[test]
    fn no_mbc_mapper_ignores_register_and_state_writes() {
        let mut mbc = NoMbc;
        let mapping = mbc.rom_mapping(2);
        assert_eq!(mapping.fixed_bank, 0);
        assert_eq!(mapping.switchable_bank, 1);

        assert!(!mbc.write_register(0x2000, 0x7F));
        assert_eq!(mbc.ram_bank(), 0);
        assert!(!mbc.ram_enabled());

        let mut blob = Vec::new();
        mbc.save_state(&mut blob);
        assert!(blob.is_empty());
        assert_eq!(mbc.load_state(&[1, 2, 3], 0), 0);
    }

    #[test]
    fn mbc1_reports_mapping_changes_only_for_mapping_registers() {
        let mut mbc = Mbc1::new(0x8000);

        assert!(!mbc.write_register(0x0000, 0x0A));
        assert!(mbc.ram_enabled());

        assert!(mbc.write_register(0x2000, 0x02));
        assert!(!mbc.write_register(0x2000, 0x02));
        assert!(mbc.write_register(0x4000, 0x01));
        assert!(mbc.write_register(0x6000, 0x01));

        let mapping = mbc.rom_mapping(64);
        assert_eq!(mapping.fixed_bank, 32);
        assert_eq!(mapping.switchable_bank, 34);
        assert_eq!(mbc.ram_bank(), 1);
    }

    #[test]
    fn mbc3_loads_legacy_three_byte_state_payloads() {
        let mut mbc = Mbc3::new();

        assert_eq!(mbc.load_state(&[0x05, 0x02, 0x01], 0), 3);
        assert_eq!(mbc.rom_mapping(8).switchable_bank, 5);
        assert_eq!(mbc.ram_bank(), 2);
        assert!(mbc.ram_enabled());

        let mut blob = Vec::new();
        mbc.save_state(&mut blob);
        assert_eq!(blob, [0x05, 0x02, 0x01, 0x00]);
    }

    #[test]
    fn mbc5_reports_mapping_changes_only_for_rom_bank_registers() {
        let mut mbc = Mbc5::new(false);

        assert!(!mbc.write_register(0x0000, 0x0A));
        assert!(mbc.ram_enabled());

        assert!(mbc.write_register(0x2000, 0x02));
        assert!(!mbc.write_register(0x2000, 0x02));
        assert!(mbc.write_register(0x3000, 0x01));
        assert!(!mbc.write_register(0x3000, 0x01));

        assert!(!mbc.write_register(0x4000, 0x08));
        assert_eq!(mbc.ram_bank(), 8);
        assert_eq!(mbc.rom_mapping(512).switchable_bank, 0x102);
    }

    #[test]
    fn mbc5_rumble_mapper_masks_rumble_bit_from_ram_bank() {
        let mut plain = Mbc5::new(false);
        plain.write_register(0x4000, 0x0F);
        assert_eq!(plain.ram_bank(), 0x0F);

        let mut rumble = Mbc5::new(true);
        rumble.write_register(0x4000, 0x0F);
        assert_eq!(rumble.ram_bank(), 0x07);

        rumble.write_register(0x4000, 0x08);
        assert_eq!(rumble.ram_bank(), 0);
    }

    #[test]
    fn mbc1_bank_switch_reads_from_xip_slice() {
        let mut cart = XipCartridge::new(leak_rom(4, 0x01, 0x00)).unwrap();
        assert_eq!(cart.read_rom(0x4000), 0x01);
        cart.write(0x2000, 0x02);
        assert_eq!(cart.current_rom_bank(), 2);
        assert_eq!(cart.read_rom(0x4000), 0x02);
    }

    #[test]
    fn mbc1_without_ram_header_matches_core_ram_fallback() {
        let mut cart = XipCartridge::new(leak_rom(4, 0x01, 0x00)).unwrap();
        cart.write(0x0000, 0x0A);
        cart.write(0xA123, 0x42);
        assert_eq!(cart.read_ram(0x0123), 0x42);
    }

    #[test]
    fn mbc1_ram_mode_remaps_fixed_window() {
        let mut cart = XipCartridge::new(leak_rom(64, 0x01, 0x00)).unwrap();
        cart.write(0x4000, 0x01);
        cart.write(0x6000, 0x01);
        assert_eq!(cart.read_rom(0x0000), 32);
    }

    #[test]
    fn mbc3_out_of_bounds_bank_reads_ff() {
        let mut cart = XipCartridge::new(leak_rom(8, 0x13, 0x00)).unwrap();
        cart.write(0x2000, 0x20);
        assert_eq!(cart.current_rom_bank(), 0x20);
        assert_eq!(cart.read_rom(0x4000), 0xFF);
    }

    #[test]
    fn mbc3_ram_bank_selects_external_ram_bank() {
        let mut cart = XipCartridge::new(leak_rom(8, 0x13, 0x03)).unwrap();
        cart.write(0x0000, 0x0A);
        cart.write(0x4000, 0x01);
        cart.write(0xA123, 0xBB);

        cart.write(0x4000, 0x00);
        assert_eq!(cart.read_ram(0x0123), 0x00);

        cart.write(0x4000, 0x01);
        assert_eq!(cart.read_ram(0x0123), 0xBB);
    }

    #[test]
    fn mbc3_rtc_register_select_does_not_alias_ram() {
        let mut cart = XipCartridge::new(leak_rom(8, 0x13, 0x03)).unwrap();
        cart.write(0x0000, 0x0A);
        cart.write(0x4000, 0x08);
        cart.write(0xA000, 0x55);
        assert_eq!(cart.read_ram(0x0000), 0xFF);

        cart.write(0x4000, 0x00);
        assert_eq!(cart.read_ram(0x0000), 0x00);
    }

    #[test]
    fn mbc3_save_state_payload_stays_rbss_v1_aligned() {
        let mut cart = XipCartridge::new(leak_rom(8, 0x13, 0x03)).unwrap();
        cart.write(0x2000, 0x05);
        cart.write(0x4000, 0x02);
        cart.write(0x0000, 0x0A);

        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);
        assert_eq!(blob.len(), 4);

        let mut restored = XipCartridge::new(leak_rom(8, 0x13, 0x03)).unwrap();
        assert_eq!(restored.load_mbc_state(&blob, 0), 4);
        assert_eq!(restored.current_rom_bank(), 5);
        restored.write(0xA000, 0x42);
        assert_eq!(restored.read_ram(0x0000), 0x42);
    }

    #[test]
    fn mbc5_supports_wario_land_ii_style_header() {
        let mut cart = XipCartridge::new(leak_rom(128, 0x1B, 0x03)).unwrap();
        assert_eq!(cart.read_rom(0x0000), 0x00);
        assert_eq!(cart.read_rom(0x4000), 0x01);

        cart.write(0x2000, 0x42);
        assert_eq!(cart.current_rom_bank(), 0x42);
        assert_eq!(cart.read_rom(0x4000), 0x42);
    }

    #[test]
    fn wario_land_ii_door_bank_switch_updates_cached_memory_window() {
        let cart = XipCartridge::new(leak_rom(64, 0x1B, 0x03)).unwrap();
        let mut memory = GameBoyMemory::with_cartridge(Box::new(cart));

        assert_eq!(memory.current_rom_bank(), 1);
        assert_eq!(memory.read(0x4000).unwrap(), 1);
        assert_eq!(memory.read_fast(0x4567), 1);

        // The first-door regression was an ignored MBC5 bank switch on the
        // cached XIP path, leaving the CPU fetching from the old bank.
        memory.write(0x2000, 0x24).unwrap();
        assert_eq!(memory.current_rom_bank(), 0x24);
        assert_eq!(memory.read(0x4000).unwrap(), 0x24);
        assert_eq!(memory.read_fast(0x4567), 0x24);

        memory.write_fast(0x2000, 0x3F);
        assert_eq!(memory.current_rom_bank(), 0x3F);
        assert_eq!(memory.read_fast(0x4000), 0x3F);
    }

    #[test]
    fn mbc5_allows_bank_zero_in_switchable_window() {
        let mut cart = XipCartridge::new(leak_rom(128, 0x19, 0x00)).unwrap();
        cart.write(0x2000, 0x00);
        assert_eq!(cart.current_rom_bank(), 0);
        assert_eq!(cart.read_rom(0x4000), 0x00);
    }

    #[test]
    fn mbc5_ram_bank_selects_external_ram_bank() {
        let mut cart = XipCartridge::new(leak_rom(128, 0x1B, 0x03)).unwrap();
        cart.write(0x0000, 0x0A);
        cart.write(0x4000, 0x02);
        cart.write(0xA123, 0xCC);

        cart.write(0x4000, 0x00);
        assert_eq!(cart.read_ram(0x0123), 0x00);

        cart.write(0x4000, 0x02);
        assert_eq!(cart.read_ram(0x0123), 0xCC);
    }

    #[test]
    fn mbc5_save_state_round_trip_restores_mapping() {
        let mut cart = XipCartridge::new(leak_rom(128, 0x1B, 0x03)).unwrap();
        cart.write(0x2000, 0x34);
        cart.write(0x3000, 0x01);
        cart.write(0x4000, 0x03);
        cart.write(0x0000, 0x0A);

        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);
        assert_eq!(blob.len(), 4);

        let mut restored = XipCartridge::new(leak_rom(128, 0x1B, 0x03)).unwrap();
        assert_eq!(restored.load_mbc_state(&blob, 0), 4);
        assert_eq!(restored.current_rom_bank(), 0x34);
        restored.write(0xA000, 0x5A);
        assert_eq!(restored.read_ram(0x0000), 0x5A);
    }

    #[test]
    fn save_state_round_trip_restores_mbc1_mapping() {
        let mut cart = XipCartridge::new(leak_rom(64, 0x01, 0x00)).unwrap();
        cart.write(0x4000, 0x01);
        cart.write(0x2000, 0x03);
        cart.write(0x6000, 0x01);

        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);

        let mut restored = XipCartridge::new(leak_rom(64, 0x01, 0x00)).unwrap();
        let consumed = restored.load_mbc_state(&blob, 0);
        assert_eq!(consumed, 4);
        assert_eq!(restored.current_rom_bank(), 35);
        assert_eq!(restored.read_rom(0x0000), 32);
        assert_eq!(restored.read_rom(0x4000), 35);
    }
}
