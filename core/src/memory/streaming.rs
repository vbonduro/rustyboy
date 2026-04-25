use alloc::vec;
use alloc::vec::Vec;

use super::cartridge::Cartridge;

// ── RomReader trait ──────────────────────────────────────────────────────────

pub trait RomReader {
    type Error;
    fn read_bank(&mut self, bank: usize, buf: &mut [u8; 0x4000]) -> Result<(), Self::Error>;
}

// ── Header offsets ───────────────────────────────────────────────────────────

const CART_TYPE: usize = 0x0147;
const ROM_SIZE:  usize = 0x0148;
const RAM_SIZE:  usize = 0x0149;

// ── MBC state ────────────────────────────────────────────────────────────────

enum MbcState {
    NoMbc,
    Mbc1 {
        rom_bank_lo:    u8,
        upper_bits:     u8,
        ram_mode:       bool,
        ram_enabled:    bool,
        ram_bank_count: usize,
    },
    Mbc3 {
        rom_bank:        u8,
        bank_or_rtc:     u8,
        ram_rtc_enabled: bool,
    },
}

// ── StreamingCartridge ───────────────────────────────────────────────────────

#[derive(Debug)]
pub enum StreamingError<E> {
    Reader(E),
    UnsupportedCartType(u8),
}

pub struct StreamingCartridge<R: RomReader> {
    reader:           R,
    bank0_cache:      [u8; 0x4000],
    banked_cache:     [u8; 0x4000],
    fixed_bank_num:   usize,
    current_bank_num: usize,
    rom_bank_count:   usize,
    mbc:              MbcState,
    ram:              Vec<u8>,
}

impl<R: RomReader> StreamingCartridge<R> {
    pub fn new(mut reader: R) -> Result<Self, StreamingError<R::Error>> {
        let mut bank0_cache = [0u8; 0x4000];
        reader.read_bank(0, &mut bank0_cache).map_err(StreamingError::Reader)?;

        let cart_type      = bank0_cache[CART_TYPE];
        let rom_bank_count = rom_bank_count_from_code(bank0_cache[ROM_SIZE]);
        let ram_bytes      = ram_bytes_from_code(bank0_cache[RAM_SIZE]);
        let mbc = mbc_state_from_header(cart_type, ram_bytes)
            .ok_or(StreamingError::UnsupportedCartType(cart_type))?;

        let mut banked_cache = [0u8; 0x4000];
        reader.read_bank(1, &mut banked_cache).map_err(StreamingError::Reader)?;

        Ok(Self {
            reader,
            bank0_cache,
            banked_cache,
            fixed_bank_num:   0,
            current_bank_num: 1,
            rom_bank_count,
            mbc,
            ram: vec![0u8; ram_bytes],
        })
    }

    fn effective_fixed_bank(&self) -> usize {
        match &self.mbc {
            MbcState::NoMbc => 0,
            MbcState::Mbc1 { upper_bits, ram_mode, .. } => {
                if *ram_mode {
                    ((*upper_bits as usize) << 5) % self.rom_bank_count
                } else {
                    0
                }
            }
            MbcState::Mbc3 { .. } => 0,
        }
    }

    fn effective_switchable_bank(&self) -> usize {
        match &self.mbc {
            MbcState::NoMbc => 1,
            MbcState::Mbc1 { rom_bank_lo, upper_bits, .. } => {
                let bank = ((*upper_bits as usize) << 5) | (*rom_bank_lo as usize);
                let bank = if bank == 0 { 1 } else { bank };
                bank % self.rom_bank_count
            }
            MbcState::Mbc3 { rom_bank, .. } => *rom_bank as usize,
        }
    }

    fn sync_caches(&mut self) {
        let new_fixed      = self.effective_fixed_bank();
        let new_switchable = self.effective_switchable_bank();

        if new_fixed != self.fixed_bank_num {
            if self.reader.read_bank(new_fixed, &mut self.bank0_cache).is_err() {
                self.bank0_cache.fill(0xFF);
            }
            self.fixed_bank_num = new_fixed;
        }
        if new_switchable != self.current_bank_num {
            if self.reader.read_bank(new_switchable, &mut self.banked_cache).is_err() {
                self.banked_cache.fill(0xFF);
            }
            self.current_bank_num = new_switchable;
        }
    }

    fn handle_mbc_write(&mut self, addr: u16, value: u8) {
        match &mut self.mbc {
            MbcState::NoMbc => {}
            MbcState::Mbc1 { rom_bank_lo, upper_bits, ram_mode, ram_enabled, .. } => {
                match addr {
                    0x0000..=0x1FFF => *ram_enabled = value & 0x0F == 0x0A,
                    0x2000..=0x3FFF => {
                        *rom_bank_lo = value & 0x1F;
                        if *rom_bank_lo == 0 { *rom_bank_lo = 1; }
                    }
                    0x4000..=0x5FFF => *upper_bits = value & 0x03,
                    0x6000..=0x7FFF => *ram_mode   = value & 0x01 != 0,
                    _ => {}
                }
            }
            MbcState::Mbc3 { rom_bank, bank_or_rtc, ram_rtc_enabled } => {
                match addr {
                    0x0000..=0x1FFF => *ram_rtc_enabled = value & 0x0F == 0x0A,
                    0x2000..=0x3FFF => {
                        *rom_bank = if value & 0x7F == 0 { 1 } else { value & 0x7F };
                    }
                    0x4000..=0x5FFF => *bank_or_rtc = value,
                    _ => {}
                }
            }
        }
    }

    fn mbc1_ram_bank(&self) -> usize {
        match &self.mbc {
            MbcState::Mbc1 { upper_bits, ram_mode: true, ram_bank_count, .. } => {
                (*upper_bits as usize) % (*ram_bank_count).max(1)
            }
            _ => 0,
        }
    }

    fn is_ram_enabled(&self) -> bool {
        match &self.mbc {
            MbcState::NoMbc => false,
            MbcState::Mbc1 { ram_enabled, .. } => *ram_enabled,
            MbcState::Mbc3 { ram_rtc_enabled, bank_or_rtc, .. } => {
                *ram_rtc_enabled && !matches!(bank_or_rtc, 0x08..=0x0C)
            }
        }
    }
}

impl<R: RomReader> Cartridge for StreamingCartridge<R> {
    fn read_rom(&self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x3FFF => self.bank0_cache[addr as usize],
            0x4000..=0x7FFF => self.banked_cache[(addr - 0x4000) as usize],
            _ => 0xFF,
        }
    }

    fn read_ram(&self, addr: u16) -> u8 {
        if !self.is_ram_enabled() || self.ram.is_empty() { return 0xFF; }
        let offset = self.mbc1_ram_bank() * 0x2000 + addr as usize;
        self.ram.get(offset).copied().unwrap_or(0xFF)
    }

    fn write(&mut self, addr: u16, value: u8) {
        if (0xA000..=0xBFFF).contains(&addr) {
            if self.is_ram_enabled() && !self.ram.is_empty() {
                let offset = self.mbc1_ram_bank() * 0x2000 + (addr - 0xA000) as usize;
                if let Some(b) = self.ram.get_mut(offset) { *b = value; }
            }
        } else {
            self.handle_mbc_write(addr, value);
            self.sync_caches();
        }
    }

    fn current_rom_bank(&self) -> usize { self.current_bank_num }

    fn external_ram(&self) -> Option<&[u8]> {
        if self.ram.is_empty() { None } else { Some(&self.ram) }
    }

    fn set_external_ram(&mut self, data: &[u8]) {
        let len = self.ram.len().min(data.len());
        self.ram[..len].copy_from_slice(&data[..len]);
    }

    fn save_mbc_state(&self, out: &mut Vec<u8>) {
        match &self.mbc {
            MbcState::NoMbc => {}
            MbcState::Mbc1 { rom_bank_lo, upper_bits, ram_mode, ram_enabled, .. } => {
                out.extend_from_slice(&[*rom_bank_lo, *upper_bits, *ram_mode as u8, *ram_enabled as u8]);
            }
            MbcState::Mbc3 { rom_bank, bank_or_rtc, ram_rtc_enabled } => {
                out.extend_from_slice(&[*rom_bank, *bank_or_rtc, *ram_rtc_enabled as u8]);
            }
        }
    }

    fn load_mbc_state(&mut self, data: &[u8], offset: usize) -> usize {
        let consumed = match &mut self.mbc {
            MbcState::NoMbc => 0,
            MbcState::Mbc1 { rom_bank_lo, upper_bits, ram_mode, ram_enabled, .. } => {
                if data.len() < offset + 4 { return 0; }
                *rom_bank_lo = data[offset].max(1);
                *upper_bits  = data[offset + 1] & 0x03;
                *ram_mode    = data[offset + 2] != 0;
                *ram_enabled = data[offset + 3] != 0;
                4
            }
            MbcState::Mbc3 { rom_bank, bank_or_rtc, ram_rtc_enabled } => {
                if data.len() < offset + 3 { return 0; }
                *rom_bank        = data[offset].max(1);
                *bank_or_rtc     = data[offset + 1];
                *ram_rtc_enabled = data[offset + 2] != 0;
                3
            }
        };
        if consumed > 0 {
            self.fixed_bank_num   = usize::MAX;
            self.current_bank_num = usize::MAX;
            self.sync_caches();
        }
        consumed
    }
}

// ── Header decoders ──────────────────────────────────────────────────────────

fn rom_bank_count_from_code(code: u8) -> usize {
    2usize << code
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

fn mbc_state_from_header(cart_type: u8, ram_bytes: usize) -> Option<MbcState> {
    let ram_bank_count = if ram_bytes == 0 { 0 } else { (ram_bytes / 0x2000).max(1) };
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
        _ => None,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    struct MockRomReader {
        banks:    Vec<[u8; 0x4000]>,
        read_log: Vec<usize>,
    }

    impl MockRomReader {
        fn new(num_banks: usize, cart_type: u8, ram_size_code: u8) -> Self {
            let mut banks = vec![[0u8; 0x4000]; num_banks];
            for (i, bank) in banks.iter_mut().enumerate() {
                bank.fill(i as u8);
            }
            banks[0][CART_TYPE] = cart_type;
            banks[0][ROM_SIZE]  = rom_size_code_for(num_banks);
            banks[0][RAM_SIZE]  = ram_size_code;
            Self { banks, read_log: Vec::new() }
        }
    }

    fn rom_size_code_for(num_banks: usize) -> u8 {
        match num_banks {
            2  => 0,
            4  => 1,
            8  => 2,
            16 => 3,
            32 => 4,
            64 => 5,
            _  => 0,
        }
    }

    impl RomReader for MockRomReader {
        type Error = ();
        fn read_bank(&mut self, bank: usize, buf: &mut [u8; 0x4000]) -> Result<(), ()> {
            self.read_log.push(bank);
            *buf = *self.banks.get(bank).unwrap_or(&[0xFF; 0x4000]);
            Ok(())
        }
    }

    fn no_mbc() -> StreamingCartridge<MockRomReader> {
        StreamingCartridge::new(MockRomReader::new(2, 0x00, 0x00)).unwrap()
    }

    fn mbc1(num_banks: usize) -> StreamingCartridge<MockRomReader> {
        StreamingCartridge::new(MockRomReader::new(num_banks, 0x01, 0x00)).unwrap()
    }

    fn mbc1_with_ram() -> StreamingCartridge<MockRomReader> {
        // cart type 0x03 = MBC1+RAM+BATTERY, ram_size_code 0x02 = 8 KiB
        StreamingCartridge::new(MockRomReader::new(4, 0x03, 0x02)).unwrap()
    }

    fn mbc3(num_banks: usize) -> StreamingCartridge<MockRomReader> {
        // cart type 0x13 = MBC3+RAM+BATTERY
        StreamingCartridge::new(MockRomReader::new(num_banks, 0x13, 0x00)).unwrap()
    }

    // ── Construction ──────────────────────────────────────────────────────────

    #[test]
    fn new_reads_bank0_and_bank1_on_init() {
        let cart = no_mbc();
        assert_eq!(cart.reader.read_log, [0, 1]);
    }

    #[test]
    fn new_unsupported_cart_type_returns_error() {
        assert!(matches!(
            StreamingCartridge::new(MockRomReader::new(2, 0xFF, 0x00)),
            Err(StreamingError::UnsupportedCartType(0xFF))
        ));
    }

    // ── read_rom ──────────────────────────────────────────────────────────────

    #[test]
    fn read_rom_fixed_window_hits_bank0_cache() {
        let cart = mbc1(4);
        assert_eq!(cart.read_rom(0x0000), 0x00);
        assert_eq!(cart.read_rom(0x3FFF), 0x00);
    }

    #[test]
    fn read_rom_banked_window_hits_current_bank_cache() {
        let cart = mbc1(4);
        assert_eq!(cart.read_rom(0x4000), 0x01);
        assert_eq!(cart.read_rom(0x7FFF), 0x01);
    }

    // ── MBC1 ──────────────────────────────────────────────────────────────────

    #[test]
    fn mbc1_bank_switch_loads_new_bank() {
        let mut cart = mbc1(4);
        cart.write(0x2000, 0x02);
        assert_eq!(cart.current_rom_bank(), 2);
        assert_eq!(cart.read_rom(0x4000), 0x02);
    }

    #[test]
    fn mbc1_writing_zero_selects_bank1() {
        let mut cart = mbc1(4);
        cart.write(0x2000, 0x00);
        assert_eq!(cart.current_rom_bank(), 1);
    }

    #[test]
    fn mbc1_same_bank_reselect_skips_reload() {
        let mut cart = mbc1(4);
        cart.reader.read_log.clear();
        cart.write(0x2000, 0x01); // already on bank 1
        assert!(cart.reader.read_log.is_empty());
    }

    #[test]
    fn mbc1_upper_bits_extend_bank_number() {
        let mut cart = mbc1(64);
        cart.write(0x2000, 0x01); // lower = 1 (no change — already bank 1)
        cart.write(0x4000, 0x01); // upper = 1 → (1<<5)|1 = 33
        assert_eq!(cart.current_rom_bank(), 33);
        assert_eq!(cart.read_rom(0x4000), 33);
    }

    #[test]
    fn mbc1_ram_mode_remaps_fixed_bank() {
        let mut cart = mbc1(64);
        cart.write(0x4000, 0x01); // upper = 1
        cart.write(0x6000, 0x01); // RAM mode → fixed = (1<<5)%64 = 32
        assert_eq!(cart.fixed_bank_num, 32);
        assert_eq!(cart.read_rom(0x0000), 32);
    }

    // ── MBC3 ──────────────────────────────────────────────────────────────────

    #[test]
    fn mbc3_bank_switch_loads_new_bank() {
        let mut cart = mbc3(8);
        cart.write(0x2000, 0x03);
        assert_eq!(cart.current_rom_bank(), 3);
        assert_eq!(cart.read_rom(0x4000), 0x03);
    }

    #[test]
    fn mbc3_writing_zero_selects_bank1() {
        let mut cart = mbc3(8);
        cart.write(0x2000, 0x00);
        assert_eq!(cart.current_rom_bank(), 1);
    }

    #[test]
    fn mbc3_fixed_window_always_bank0() {
        let mut cart = mbc3(8);
        cart.write(0x2000, 0x05);
        assert_eq!(cart.fixed_bank_num, 0);
        assert_eq!(cart.read_rom(0x0000), 0x00);
    }

    // ── External RAM ──────────────────────────────────────────────────────────

    #[test]
    fn ram_disabled_returns_0xff() {
        let cart = mbc1_with_ram();
        assert_eq!(cart.read_ram(0x0000), 0xFF);
    }

    #[test]
    fn ram_enable_write_read_roundtrip() {
        let mut cart = mbc1_with_ram();
        cart.write(0x0000, 0x0A); // enable
        cart.write(0xA000, 0x42);
        assert_eq!(cart.read_ram(0x0000), 0x42);
    }

    #[test]
    fn ram_disable_blocks_read() {
        let mut cart = mbc1_with_ram();
        cart.write(0x0000, 0x0A);
        cart.write(0xA000, 0x42);
        cart.write(0x0000, 0x00); // disable
        assert_eq!(cart.read_ram(0x0000), 0xFF);
    }

    // ── NoMBC ─────────────────────────────────────────────────────────────────

    #[test]
    fn no_mbc_banked_window_is_bank1() {
        let cart = no_mbc();
        assert_eq!(cart.read_rom(0x4000), 0x01);
    }

    #[test]
    fn no_mbc_writes_do_not_reload_cache() {
        let mut cart = no_mbc();
        cart.reader.read_log.clear();
        cart.write(0x2000, 0x01);
        assert!(cart.reader.read_log.is_empty());
    }

    // ── save / load MBC state ─────────────────────────────────────────────────

    #[test]
    fn mbc1_save_load_state_restores_bank() {
        let mut cart = mbc1(4);
        cart.write(0x2000, 0x03);
        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);

        let mut cart2 = mbc1(4);
        cart2.load_mbc_state(&blob, 0);
        assert_eq!(cart2.current_rom_bank(), 3);
    }

    #[test]
    fn mbc3_save_load_state_restores_bank() {
        let mut cart = mbc3(8);
        cart.write(0x2000, 0x05);
        let mut blob = Vec::new();
        cart.save_mbc_state(&mut blob);

        let mut cart2 = mbc3(8);
        cart2.load_mbc_state(&blob, 0);
        assert_eq!(cart2.current_rom_bank(), 5);
    }
}
