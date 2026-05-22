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

enum MbcState {
    NoMbc,
    Mbc1 {
        rom_bank_lo: u8,
        upper_bits: u8,
        ram_mode: bool,
        ram_enabled: bool,
        ram_bank_count: usize,
    },
    Mbc3 {
        rom_bank: u8,
        bank_or_rtc: u8,
        ram_rtc_enabled: bool,
    },
    Mbc5 {
        rom_bank_lo: u8,
        rom_bank_hi: u8,
        ram_bank: u8,
        ram_enabled: bool,
        rumble: bool,
    },
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
        let (fixed_bank_num, current_bank_num) = match &self.mbc {
            MbcState::NoMbc => (0, 1),
            MbcState::Mbc1 {
                rom_bank_lo,
                upper_bits,
                ram_mode,
                ..
            } => {
                let fixed = if *ram_mode {
                    ((*upper_bits as usize) << 5) % self.rom_bank_count
                } else {
                    0
                };
                let bank = ((*upper_bits as usize) << 5) | (*rom_bank_lo as usize);
                let bank = if bank == 0 { 1 } else { bank };
                (fixed, bank % self.rom_bank_count)
            }
            MbcState::Mbc3 { rom_bank, .. } => (0, *rom_bank as usize),
            MbcState::Mbc5 {
                rom_bank_lo,
                rom_bank_hi,
                ..
            } => {
                let bank = ((*rom_bank_hi as usize) << 8) | *rom_bank_lo as usize;
                (0, bank % self.rom_bank_count)
            }
        };

        self.fixed_bank_num = fixed_bank_num;
        self.fixed_bank_valid = fixed_bank_num < self.rom_bank_count;
        self.fixed_bank_base = fixed_bank_num * ROM_BANK_BYTES;

        self.current_bank_num = current_bank_num;
        self.current_bank_valid = current_bank_num < self.rom_bank_count;
        self.current_bank_base = current_bank_num * ROM_BANK_BYTES;
    }

    /// Apply an MBC register write.
    ///
    /// Returns true only when the write changes the visible ROM mapping.
    #[inline]
    fn handle_mbc_write(&mut self, addr: u16, value: u8) -> bool {
        match &mut self.mbc {
            MbcState::NoMbc => false,
            MbcState::Mbc1 {
                rom_bank_lo,
                upper_bits,
                ram_mode,
                ram_enabled,
                ..
            } => match addr {
                0x0000..=0x1FFF => {
                    *ram_enabled = value & 0x0F == 0x0A;
                    false
                }
                0x2000..=0x3FFF => {
                    let mut bank = value & 0x1F;
                    if bank == 0 {
                        bank = 1;
                    }
                    if *rom_bank_lo == bank {
                        false
                    } else {
                        *rom_bank_lo = bank;
                        true
                    }
                }
                0x4000..=0x5FFF => {
                    let bits = value & 0x03;
                    if *upper_bits == bits {
                        false
                    } else {
                        *upper_bits = bits;
                        true
                    }
                }
                0x6000..=0x7FFF => {
                    let new_ram_mode = value & 0x01 != 0;
                    if *ram_mode == new_ram_mode {
                        false
                    } else {
                        *ram_mode = new_ram_mode;
                        true
                    }
                }
                _ => false,
            },
            MbcState::Mbc3 {
                rom_bank,
                bank_or_rtc,
                ram_rtc_enabled,
            } => match addr {
                0x0000..=0x1FFF => {
                    *ram_rtc_enabled = value & 0x0F == 0x0A;
                    false
                }
                0x2000..=0x3FFF => {
                    let bank = if value & 0x7F == 0 { 1 } else { value & 0x7F };
                    if *rom_bank == bank {
                        false
                    } else {
                        *rom_bank = bank;
                        true
                    }
                }
                0x4000..=0x5FFF => {
                    *bank_or_rtc = value;
                    false
                }
                _ => false,
            },
            MbcState::Mbc5 {
                rom_bank_lo,
                rom_bank_hi,
                ram_bank,
                ram_enabled,
                ..
            } => match addr {
                0x0000..=0x1FFF => {
                    *ram_enabled = value & 0x0F == 0x0A;
                    false
                }
                0x2000..=0x2FFF => {
                    if *rom_bank_lo == value {
                        false
                    } else {
                        *rom_bank_lo = value;
                        true
                    }
                }
                0x3000..=0x3FFF => {
                    let bit = value & 0x01;
                    if *rom_bank_hi == bit {
                        false
                    } else {
                        *rom_bank_hi = bit;
                        true
                    }
                }
                0x4000..=0x5FFF => {
                    *ram_bank = value & 0x0F;
                    false
                }
                _ => false,
            },
        }
    }

    #[inline]
    fn mbc1_ram_bank(&self) -> usize {
        match &self.mbc {
            MbcState::Mbc1 {
                upper_bits,
                ram_mode: true,
                ram_bank_count,
                ..
            } => (*upper_bits as usize) % (*ram_bank_count).max(1),
            _ => 0,
        }
    }

    #[inline]
    fn ram_bank(&self) -> usize {
        match &self.mbc {
            MbcState::Mbc1 { .. } => self.mbc1_ram_bank(),
            MbcState::Mbc3 { bank_or_rtc, .. } => *bank_or_rtc as usize,
            MbcState::Mbc5 {
                ram_bank, rumble, ..
            } => {
                if *rumble {
                    (*ram_bank & 0x07) as usize
                } else {
                    (*ram_bank & 0x0F) as usize
                }
            }
            MbcState::NoMbc => 0,
        }
    }

    #[inline]
    fn is_ram_enabled(&self) -> bool {
        match &self.mbc {
            MbcState::NoMbc => false,
            MbcState::Mbc1 { ram_enabled, .. } => *ram_enabled,
            MbcState::Mbc3 {
                ram_rtc_enabled,
                bank_or_rtc,
                ..
            } => *ram_rtc_enabled && !matches!(bank_or_rtc, 0x08..=0x0C),
            MbcState::Mbc5 { ram_enabled, .. } => *ram_enabled,
        }
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
        if !self.is_ram_enabled() || self.ram.is_empty() {
            return 0xFF;
        }
        let offset = self.ram_bank() * 0x2000 + addr as usize;
        self.ram.get(offset).copied().unwrap_or(0xFF)
    }

    fn write(&mut self, addr: u16, value: u8) {
        if (0xA000..=0xBFFF).contains(&addr) {
            if self.is_ram_enabled() && !self.ram.is_empty() {
                let offset = self.ram_bank() * 0x2000 + (addr - 0xA000) as usize;
                if let Some(b) = self.ram.get_mut(offset) {
                    *b = value;
                }
            }
            return;
        }

        let changed = self.handle_mbc_write(addr, value);
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
        match &self.mbc {
            MbcState::NoMbc => {}
            MbcState::Mbc1 {
                rom_bank_lo,
                upper_bits,
                ram_mode,
                ram_enabled,
                ..
            } => {
                out.extend_from_slice(&[
                    *rom_bank_lo,
                    *upper_bits,
                    *ram_mode as u8,
                    *ram_enabled as u8,
                ]);
            }
            MbcState::Mbc3 {
                rom_bank,
                bank_or_rtc,
                ram_rtc_enabled,
            } => {
                // RBSS v1 parses a fixed 4-byte MBC block before cart RAM.
                // Keep the Pico MBC3 save payload aligned with that format.
                out.extend_from_slice(&[*rom_bank, *bank_or_rtc, *ram_rtc_enabled as u8, 0]);
            }
            MbcState::Mbc5 {
                rom_bank_lo,
                rom_bank_hi,
                ram_bank,
                ram_enabled,
                ..
            } => {
                out.extend_from_slice(&[*rom_bank_lo, *rom_bank_hi, *ram_bank, *ram_enabled as u8]);
            }
        }
    }

    fn load_mbc_state(&mut self, data: &[u8], offset: usize) -> usize {
        let consumed = match &mut self.mbc {
            MbcState::NoMbc => 0,
            MbcState::Mbc1 {
                rom_bank_lo,
                upper_bits,
                ram_mode,
                ram_enabled,
                ..
            } => {
                if data.len() < offset + 4 {
                    return 0;
                }
                *rom_bank_lo = data[offset].max(1);
                *upper_bits = data[offset + 1] & 0x03;
                *ram_mode = data[offset + 2] != 0;
                *ram_enabled = data[offset + 3] != 0;
                4
            }
            MbcState::Mbc3 {
                rom_bank,
                bank_or_rtc,
                ram_rtc_enabled,
            } => {
                if data.len() < offset + 3 {
                    return 0;
                }
                *rom_bank = data[offset].max(1);
                *bank_or_rtc = data[offset + 1];
                *ram_rtc_enabled = data[offset + 2] != 0;
                if data.len() >= offset + 4 {
                    4
                } else {
                    3
                }
            }
            MbcState::Mbc5 {
                rom_bank_lo,
                rom_bank_hi,
                ram_bank,
                ram_enabled,
                ..
            } => {
                if data.len() < offset + 4 {
                    return 0;
                }
                *rom_bank_lo = data[offset];
                *rom_bank_hi = data[offset + 1] & 0x01;
                *ram_bank = data[offset + 2] & 0x0F;
                *ram_enabled = data[offset + 3] != 0;
                4
            }
        };
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
    let ram_bank_count = if ram_bytes == 0 {
        0
    } else {
        (ram_bytes / 0x2000).max(1)
    };
    match cart_type {
        0x00 => Some(MbcState::NoMbc),
        0x01 | 0x02 | 0x03 => Some(MbcState::Mbc1 {
            rom_bank_lo: 1,
            upper_bits: 0,
            ram_mode: false,
            ram_enabled: false,
            ram_bank_count,
        }),
        0x0F | 0x10 | 0x11 | 0x12 | 0x13 => Some(MbcState::Mbc3 {
            rom_bank: 1,
            bank_or_rtc: 0,
            ram_rtc_enabled: false,
        }),
        0x19 | 0x1A | 0x1B | 0x1C | 0x1D | 0x1E => Some(MbcState::Mbc5 {
            rom_bank_lo: 1,
            rom_bank_hi: 0,
            ram_bank: 0,
            ram_enabled: false,
            rumble: matches!(cart_type, 0x1C | 0x1D | 0x1E),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
