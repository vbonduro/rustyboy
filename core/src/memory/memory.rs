use alloc::boxed::Box;
use alloc::collections::VecDeque;
use alloc::{vec, vec::Vec};
use core::fmt;

use super::cartridge::{self, Cartridge, CartridgeRomWindows, NoMbc};
use crate::cpu::save_state::SaveState;

/// An event produced when a write occurs to an I/O or IE register address.
#[derive(Debug, PartialEq, Clone, Copy)]
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
    Rom,
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
            0x0000..=0x7FFF => RegionMapping::Rom,
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
///   0x0000–0x7FFF  ROM (cartridge, may be bank-switched by MBC)
///   0x8000–0x9FFF  VRAM
///   0xA000–0xBFFF  External RAM (cartridge, may be bank-switched by MBC)
///   0xC000–0xDFFF  Work RAM (WRAM)
///   0xE000–0xFDFF  Echo RAM (mirrors WRAM reads, writes are read-only)
///   0xFE00–0xFE9F  OAM
///   0xFF00–0xFF7F  I/O registers
///   0xFF80–0xFFFE  High RAM (HRAM)
///   0xFFFF         Interrupt Enable (IE) register
///   Everything else: unmapped (returns 0xFF on read, silently ignored on write)
pub struct GameBoyMemory {
    cartridge: Box<dyn Cartridge>,
    cartridge_has_rtc: bool,
    cartridge_has_rom_windows: bool,
    rom_fixed_full_window: bool,
    rom_banked_full_window: bool,
    rom_fixed_ptr: *const u8,
    rom_fixed_len: usize,
    rom_banked_ptr: *const u8,
    rom_banked_len: usize,
    vram: [u8; 0x2000],
    wram: [u8; 0x2000],
    oam: [u8; 0xA0],
    io: [u8; 0x80],
    hram: [u8; 0x7F],
    ie: u8,
    events: VecDeque<BusEvent>,
}

impl GameBoyMemory {
    pub fn new() -> Self {
        let cartridge: Box<dyn Cartridge> = Box::new(NoMbc::new(vec![0u8; 0x8000]));
        let (cartridge_has_rom_windows, rom_windows) = match cartridge.rom_windows() {
            Some(rom_windows) => (true, rom_windows),
            None => (false, CartridgeRomWindows::EMPTY),
        };
        Self {
            cartridge_has_rtc: cartridge.has_rtc(),
            cartridge_has_rom_windows,
            rom_fixed_full_window: rom_windows.fixed_len == 0x4000,
            rom_banked_full_window: rom_windows.banked_len == 0x4000,
            rom_fixed_ptr: rom_windows.fixed_ptr,
            rom_fixed_len: rom_windows.fixed_len,
            rom_banked_ptr: rom_windows.banked_ptr,
            rom_banked_len: rom_windows.banked_len,
            cartridge,
            vram: [0; 0x2000],
            wram: [0; 0x2000],
            oam: [0; 0xA0],
            io: [0; 0x80],
            hram: [0; 0x7F],
            ie: 0,
            events: VecDeque::with_capacity(8),
        }
    }

    /// Construct memory backed by a pre-built cartridge implementation.
    pub fn with_cartridge(cart: Box<dyn Cartridge>) -> Self {
        let cartridge_has_rtc = cart.has_rtc();
        let (cartridge_has_rom_windows, rom_windows) = match cart.rom_windows() {
            Some(rom_windows) => (true, rom_windows),
            None => (false, CartridgeRomWindows::EMPTY),
        };
        Self {
            cartridge_has_rtc,
            cartridge_has_rom_windows,
            rom_fixed_full_window: rom_windows.fixed_len == 0x4000,
            rom_banked_full_window: rom_windows.banked_len == 0x4000,
            rom_fixed_ptr: rom_windows.fixed_ptr,
            rom_fixed_len: rom_windows.fixed_len,
            rom_banked_ptr: rom_windows.banked_ptr,
            rom_banked_len: rom_windows.banked_len,
            cartridge: cart,
            vram: [0; 0x2000],
            wram: [0; 0x2000],
            oam: [0; 0xA0],
            io: [0; 0x80],
            hram: [0; 0x7F],
            ie: 0,
            events: VecDeque::with_capacity(8),
        }
    }

    /// Construct memory with a cartridge ROM. The cartridge type is auto-detected
    /// from the ROM header (byte 0x0147) to select the correct MBC.
    pub fn with_rom(data: Vec<u8>) -> Self {
        let cartridge = cartridge::from_rom(data);
        let (cartridge_has_rom_windows, rom_windows) = match cartridge.rom_windows() {
            Some(rom_windows) => (true, rom_windows),
            None => (false, CartridgeRomWindows::EMPTY),
        };
        Self {
            cartridge_has_rtc: cartridge.has_rtc(),
            cartridge_has_rom_windows,
            rom_fixed_full_window: rom_windows.fixed_len == 0x4000,
            rom_banked_full_window: rom_windows.banked_len == 0x4000,
            rom_fixed_ptr: rom_windows.fixed_ptr,
            rom_fixed_len: rom_windows.fixed_len,
            rom_banked_ptr: rom_windows.banked_ptr,
            rom_banked_len: rom_windows.banked_len,
            cartridge,
            vram: [0; 0x2000],
            wram: [0; 0x2000],
            oam: [0; 0xA0],
            io: [0; 0x80],
            hram: [0; 0x7F],
            ie: 0,
            events: VecDeque::with_capacity(8),
        }
    }

    #[inline(always)]
    fn read_region_fast<const N: usize>(region: &[u8; N], offset: u16) -> u8 {
        // The caller's address decode guarantees the offset is valid.
        unsafe { *region.get_unchecked(offset as usize) }
    }

    #[inline(always)]
    fn write_region_fast<const N: usize>(region: &mut [u8; N], offset: u16, value: u8) {
        // The caller's address decode guarantees the offset is valid.
        unsafe {
            *region.get_unchecked_mut(offset as usize) = value;
        }
    }

    #[inline(always)]
    fn refresh_rom_windows(&mut self) {
        let (cartridge_has_rom_windows, rom_windows) = match self.cartridge.rom_windows() {
            Some(rom_windows) => (true, rom_windows),
            None => (false, CartridgeRomWindows::EMPTY),
        };
        self.cartridge_has_rom_windows = cartridge_has_rom_windows;
        self.rom_fixed_full_window = rom_windows.fixed_len == 0x4000;
        self.rom_banked_full_window = rom_windows.banked_len == 0x4000;
        self.rom_fixed_ptr = rom_windows.fixed_ptr;
        self.rom_fixed_len = rom_windows.fixed_len;
        self.rom_banked_ptr = rom_windows.banked_ptr;
        self.rom_banked_len = rom_windows.banked_len;
    }

    #[inline(always)]
    fn read_cached_rom_window(ptr: *const u8, len: usize, offset: usize) -> u8 {
        const ROM_WINDOW_BYTES: usize = 0x4000;
        if len == ROM_WINDOW_BYTES {
            // Full ROM windows dominate real cartridge fetches. Skip the bounds
            // compare on the common case and go straight to the cached pointer.
            unsafe { *ptr.add(offset) }
        } else if offset >= len {
            return 0xFF;
        } else {
            // The cached ROM window length is derived from a live
            // cartridge-backed slice, so offsets below `len` are valid for
            // direct reads.
            unsafe { *ptr.add(offset) }
        }
    }

    /// Returns the currently mapped ROM bank number for the switchable window.
    pub fn current_rom_bank(&self) -> usize {
        self.cartridge.current_rom_bank()
    }

    #[inline(always)]
    pub fn has_rtc(&self) -> bool {
        self.cartridge_has_rtc
    }

    #[cfg(feature = "perf")]
    pub fn take_cartridge_perf_profile(&mut self) -> super::cartridge::CartridgePerfProfile {
        self.cartridge.take_perf_profile()
    }

    /// Advance the cartridge RTC by `cycles` T-cycles. No-op for non-RTC carts.
    #[inline(always)]
    pub fn tick_rtc(&mut self, cycles: u32) {
        self.cartridge.tick_rtc(cycles);
    }

    /// Fast infallible memory read used by the CPU hot path.
    #[inline(always)]
    pub fn read_fast(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x7FFF => self.read_rom_fast(address),
            0x8000..=0x9FFF => Self::read_region_fast(&self.vram, address - 0x8000),
            0xA000..=0xBFFF => self.cartridge.read_ram(address - 0xA000),
            0xC000..=0xDFFF => Self::read_region_fast(&self.wram, address - 0xC000),
            0xE000..=0xFDFF => Self::read_region_fast(&self.wram, address - 0xE000),
            0xFE00..=0xFE9F => Self::read_region_fast(&self.oam, address - 0xFE00),
            0xFF00..=0xFF7F => Self::read_region_fast(&self.io, address - 0xFF00),
            0xFF80..=0xFFFE => Self::read_region_fast(&self.hram, address - 0xFF80),
            0xFFFF => self.ie,
            _ => 0xFF,
        }
    }

    /// Fast direct cartridge-ROM read for callers that have already proven the
    /// address is in `0x0000..=0x7FFF`.
    #[inline(always)]
    pub fn read_rom_fast(&self, address: u16) -> u8 {
        match address {
            0x0000..=0x3FFF => self.read_rom_fixed_fast(address),
            0x4000..=0x7FFF => self.read_rom_banked_fast(address),
            _ => 0xFF,
        }
    }

    #[inline(always)]
    pub fn read_rom_fixed_fast(&self, address: u16) -> u8 {
        debug_assert!(address <= 0x3FFF);
        if self.rom_fixed_full_window {
            unsafe { *self.rom_fixed_ptr.add(address as usize) }
        } else if self.cartridge_has_rom_windows {
            Self::read_cached_rom_window(self.rom_fixed_ptr, self.rom_fixed_len, address as usize)
        } else {
            self.cartridge.read_rom(address)
        }
    }

    #[inline(always)]
    pub fn read_rom_banked_fast(&self, address: u16) -> u8 {
        debug_assert!((0x4000..=0x7FFF).contains(&address));
        if self.rom_banked_full_window {
            unsafe { *self.rom_banked_ptr.add((address - 0x4000) as usize) }
        } else if self.cartridge_has_rom_windows {
            Self::read_cached_rom_window(
                self.rom_banked_ptr,
                self.rom_banked_len,
                (address - 0x4000) as usize,
            )
        } else {
            self.cartridge.read_rom(address)
        }
    }

    /// Fast infallible memory write used by hot non-IO paths.
    #[inline(always)]
    pub fn write_fast(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=0x7FFF => {
                self.cartridge.write(address, value);
                self.refresh_rom_windows();
            }
            0xA000..=0xBFFF => self.cartridge.write(address, value),
            0x8000..=0x9FFF => Self::write_region_fast(&mut self.vram, address - 0x8000, value),
            0xC000..=0xDFFF => Self::write_region_fast(&mut self.wram, address - 0xC000, value),
            0xE000..=0xFDFF => {}
            0xFE00..=0xFE9F => Self::write_region_fast(&mut self.oam, address - 0xFE00, value),
            0xFF00..=0xFF7F => Self::write_region_fast(&mut self.io, address - 0xFF00, value),
            0xFF80..=0xFFFE => Self::write_region_fast(&mut self.hram, address - 0xFF80, value),
            0xFFFF => self.ie = value,
            _ => {}
        }
    }

    /// Perform OAM DMA: copy 160 bytes from the source page to OAM.
    /// Source address = page * 0x100. Reads go through normal memory mapping.
    pub fn dma_to_oam(&mut self, page: u8) {
        let base = (page as u16) << 8;
        for i in 0..0xA0u16 {
            let byte = self.read(base + i).unwrap_or(0xFF);
            Self::write_region_fast(&mut self.oam, i, byte);
        }
    }

    pub fn vram(&self) -> &[u8] {
        &self.vram
    }

    pub fn oam(&self) -> &[u8] {
        &self.oam
    }

    pub fn wram(&self) -> &[u8] {
        &self.wram
    }

    pub fn hram(&self) -> &[u8] {
        &self.hram
    }

    pub fn ie(&self) -> u8 {
        self.ie
    }

    pub fn set_wram(&mut self, data: &[u8]) {
        let len = data.len().min(self.wram.len());
        self.wram[..len].copy_from_slice(&data[..len]);
    }

    pub fn set_hram(&mut self, data: &[u8]) {
        let len = data.len().min(self.hram.len());
        self.hram[..len].copy_from_slice(&data[..len]);
    }

    pub fn set_vram(&mut self, data: &[u8]) {
        let len = data.len().min(self.vram.len());
        self.vram[..len].copy_from_slice(&data[..len]);
    }

    pub fn set_oam(&mut self, data: &[u8]) {
        let len = data.len().min(self.oam.len());
        self.oam[..len].copy_from_slice(&data[..len]);
    }

    pub fn set_ie(&mut self, value: u8) {
        self.ie = value;
    }

    /// Serialize memory state into `out`.
    /// IO registers (0x80 bytes) + IE (1 byte) + WRAM + HRAM + VRAM + OAM.
    pub fn save_state(&self, out: &mut alloc::vec::Vec<u8>) {
        for i in 0..0x80u16 {
            out.push(self.read_io(0xFF00 + i));
        }
        out.push(self.ie);
        out.extend_from_slice(self.wram());
        out.extend_from_slice(self.hram());
        out.extend_from_slice(self.vram());
        out.extend_from_slice(self.oam());
        self.cartridge.save_mbc_state(out);
        // External RAM (cart SRAM): prefix with u16 LE length so load_state
        // can handle carts with no RAM (len=0) and varying RAM sizes.
        match self.cartridge.external_ram() {
            Some(ram) => {
                out.extend_from_slice(&(ram.len() as u16).to_le_bytes());
                out.extend_from_slice(ram);
            }
            None => {
                out.extend_from_slice(&0u16.to_le_bytes());
            }
        }
    }

    /// Apply memory state from a parsed [`SaveState`]. Zero-copy for large regions.
    pub fn load_state(&mut self, state: &SaveState) {
        let io = state.io_registers();
        for i in 0..0x80u16 {
            self.write_io(0xFF00 + i, io[i as usize]);
        }
        self.ie = state.ie();
        self.set_wram(state.wram());
        self.set_hram(state.hram());
        self.set_vram(state.vram());
        self.set_oam(state.oam());
        if let Some(mbc) = state.mbc() {
            // Reconstruct MBC register state via the existing load path.
            // We build a minimal 4-byte buffer and reuse load_mbc_state.
            let buf = [mbc.rom_bank_lo, mbc.upper_bits, mbc.ram_mode as u8, mbc.ram_enabled as u8];
            self.cartridge.load_mbc_state(&buf, 0);
            self.refresh_rom_windows();
        }
        if let Some(ram) = state.cart_ram() {
            self.cartridge.set_external_ram(ram);
        }
    }

    /// Returns the cartridge external RAM (battery save data), or `None` if cart has no RAM.
    pub fn external_ram(&self) -> Option<&[u8]> {
        self.cartridge.external_ram()
    }

    /// Overwrites the cartridge external RAM. No-op if cart has no external RAM.
    pub fn set_external_ram(&mut self, data: &[u8]) {
        self.cartridge.set_external_ram(data);
    }

    /// Direct read of an IO register. No bus events.
    /// Handles 0xFF00-0xFF7F from io array, 0xFFFF from ie field.
    pub fn read_io(&self, address: u16) -> u8 {
        match address {
            0xFF00..=0xFF7F => self.io[(address - 0xFF00) as usize],
            0xFFFF => self.ie,
            _ => 0xFF,
        }
    }

    /// Direct write to an IO register. No bus events queued.
    /// Used by CPU to write back peripheral state (timer, interrupts).
    pub fn write_io(&mut self, address: u16, value: u8) {
        match address {
            0xFF00..=0xFF7F => {
                self.io[(address - 0xFF00) as usize] = value;
            }
            0xFFFF => {
                self.ie = value;
            }
            _ => {}
        }
    }

    /// Drain pending bus events into an existing buffer, reusing its allocation.
    pub fn drain_into(&mut self, buf: &mut Vec<BusEvent>) {
        buf.extend(self.events.drain(..));
    }
}

impl Memory for GameBoyMemory {
    fn read(&self, address: u16) -> Result<u8, Error> {
        match RegionMapping::for_address(address) {
            RegionMapping::Rom => Ok(self.read_rom_fast(address)),
            RegionMapping::Vram(offset) => Ok(self.vram[offset as usize]),
            RegionMapping::ExternalRam(offset) => Ok(self.cartridge.read_ram(offset)),
            RegionMapping::Wram(offset) => Ok(self.wram[offset as usize]),
            RegionMapping::EchoRam(offset) => Ok(self.wram[offset as usize]),
            RegionMapping::Oam(offset) => Ok(self.oam[offset as usize]),
            RegionMapping::Io(offset) => Ok(self.io[offset as usize]),
            RegionMapping::Hram(offset) => Ok(self.hram[offset as usize]),
            RegionMapping::InterruptEnable => Ok(self.ie),
            RegionMapping::Unmapped => Ok(0xFF),
        }
    }

    fn write(&mut self, address: u16, value: u8) -> Result<(), Error> {
        match RegionMapping::for_address(address) {
            // ROM writes and external RAM writes go to the cartridge (MBC registers or RAM)
            RegionMapping::Rom => {
                self.cartridge.write(address, value);
                self.refresh_rom_windows();
                Ok(())
            }
            RegionMapping::ExternalRam(_) => {
                self.cartridge.write(address, value);
                Ok(())
            }
            RegionMapping::Vram(offset) => {
                self.vram[offset as usize] = value;
                Ok(())
            }
            RegionMapping::Wram(offset) => {
                self.wram[offset as usize] = value;
                Ok(())
            }
            RegionMapping::EchoRam(_) => Err(Error::ReadOnly(address)),
            RegionMapping::Oam(offset) => {
                self.oam[offset as usize] = value;
                Ok(())
            }
            RegionMapping::Io(offset) => {
                self.io[offset as usize] = value;
                self.events.push_back(BusEvent { address, value });
                Ok(())
            }
            RegionMapping::Hram(offset) => {
                self.hram[offset as usize] = value;
                Ok(())
            }
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

    fn make_mbc1_rom(size_kb: usize) -> Vec<u8> {
        let size = size_kb * 1024;
        let mut data = vec![0u8; size];
        for bank in 0..(size / 0x4000) {
            for byte in &mut data[bank * 0x4000..(bank + 1) * 0x4000] {
                *byte = bank as u8;
            }
        }
        data[0x0147] = 0x01;
        data[0x0148] = (size / (32 * 1024)).trailing_zeros() as u8;
        data
    }

    // --- ROM region (read-only) ---

    #[test]
    fn test_rom_region_reads_loaded_data() {
        let mem = GameBoyMemory::with_rom(vec![0x11, 0x22, 0x33]);
        assert_eq!(mem.read(0x0000).unwrap(), 0x11);
        assert_eq!(mem.read(0x0001).unwrap(), 0x22);
        assert_eq!(mem.read(0x0002).unwrap(), 0x33);
        // Bytes beyond the ROM data read as 0xFF (open bus)
        assert_eq!(mem.read(0x0003).unwrap(), 0xFF);
    }

    #[test]
    fn test_rom_region_write_is_silently_ignored() {
        let mem_with_rom = GameBoyMemory::with_rom(vec![0x11, 0x22]);
        let mut mem = mem_with_rom;
        assert!(mem.write(0x0000, 0xFF).is_ok());
        // ROM data should be unchanged
        assert_eq!(mem.read(0x0000).unwrap(), 0x11);
    }

    #[test]
    fn test_read_fast_rom_cache_tracks_bank_switches() {
        let mut mem = GameBoyMemory::with_rom(make_mbc1_rom(128));

        assert_eq!(mem.read_fast(0x0000), 0x00);
        assert_eq!(mem.read_fast(0x4000), 0x01);

        mem.write(0x2000, 0x03).unwrap();
        assert_eq!(mem.read_fast(0x0000), 0x00);
        assert_eq!(mem.read_fast(0x4000), 0x03);

        mem.write_fast(0x2000, 0x02);
        assert_eq!(mem.read_fast(0x4000), 0x02);
    }

    #[test]
    fn test_read_fast_short_rom_keeps_open_bus_semantics() {
        let mem = GameBoyMemory::with_rom(vec![0x11, 0x22, 0x33]);

        assert_eq!(mem.read_fast(0x0000), 0x11);
        assert_eq!(mem.read_fast(0x0002), 0x33);
        assert_eq!(mem.read_fast(0x0003), 0xFF);
        assert_eq!(mem.read_fast(0x4000), 0xFF);
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
