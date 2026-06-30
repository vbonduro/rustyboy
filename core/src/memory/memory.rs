use alloc::boxed::Box;
use alloc::{vec, vec::Vec};
use core::fmt;
use heapless;

use super::cartridge::{self, Cartridge, CartridgeRomWindows, NoMbc};
use super::map::{
    ECHO_BASE, ECHO_END, EXT_RAM_BASE, EXT_RAM_END, IO_REG_BASE, IO_REG_END, OAM_BASE, OAM_END,
    ROM_BANKED_BASE, ROM_BANKED_END, ROM_FIXED_END, VRAM_BASE, VRAM_END, WRAM_BASE, WRAM_END,
};
use crate::cpu::save_state::{write_cart_ram_section, write_mbc_section, SaveState};

/// An event produced when a write occurs to a worker-mirrored address.
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct BusEvent {
    pub address: u16,
    pub value: u8,
}

pub const BUS_EVENT_QUEUE_CAP: usize = 64;

type BusEventQueue = heapless::Vec<BusEvent, BUS_EVENT_QUEUE_CAP>;

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
            0x0000..=ROM_BANKED_END => RegionMapping::Rom,
            VRAM_BASE..=VRAM_END => RegionMapping::Vram(address - VRAM_BASE),
            EXT_RAM_BASE..=EXT_RAM_END => RegionMapping::ExternalRam(address - EXT_RAM_BASE),
            WRAM_BASE..=WRAM_END => RegionMapping::Wram(address - WRAM_BASE),
            ECHO_BASE..=ECHO_END => RegionMapping::EchoRam(address - ECHO_BASE),
            OAM_BASE..=OAM_END => RegionMapping::Oam(address - OAM_BASE),
            IO_REG_BASE..=IO_REG_END => RegionMapping::Io(address - IO_REG_BASE),
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
    events: BusEventQueue,
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
            events: BusEventQueue::new(),
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
            events: BusEventQueue::new(),
        }
    }

    /// Construct a boxed `GameBoyMemory` without creating a large stack temporary.
    ///
    /// `Box::new(GameBoyMemory::with_cartridge(cart))` materialises a ~17 KiB struct
    /// on the call-stack before moving it to the heap; on embedded targets with small
    /// stacks (e.g. Pico 2W Core 0 at ~15 KiB) that overflows.  This function
    /// allocates the heap slot first and writes each field into it in-place.
    ///
    /// SAFETY: every field is written before `assume_init` is called.
    pub fn with_cartridge_boxed(cart: Box<dyn Cartridge>) -> Box<Self> {
        let cartridge_has_rtc = cart.has_rtc();
        let (cartridge_has_rom_windows, rom_windows) = match cart.rom_windows() {
            Some(w) => (true, w),
            None => (false, CartridgeRomWindows::EMPTY),
        };
        let mut b = Box::<Self>::new_uninit();
        let p = b.as_mut_ptr();
        unsafe {
            core::ptr::write(core::ptr::addr_of_mut!((*p).cartridge), cart);
            core::ptr::write(
                core::ptr::addr_of_mut!((*p).cartridge_has_rtc),
                cartridge_has_rtc,
            );
            core::ptr::write(
                core::ptr::addr_of_mut!((*p).cartridge_has_rom_windows),
                cartridge_has_rom_windows,
            );
            core::ptr::write(
                core::ptr::addr_of_mut!((*p).rom_fixed_ptr),
                rom_windows.fixed_ptr,
            );
            core::ptr::write(
                core::ptr::addr_of_mut!((*p).rom_fixed_len),
                rom_windows.fixed_len,
            );
            core::ptr::write(
                core::ptr::addr_of_mut!((*p).rom_banked_ptr),
                rom_windows.banked_ptr,
            );
            core::ptr::write(
                core::ptr::addr_of_mut!((*p).rom_banked_len),
                rom_windows.banked_len,
            );
            core::ptr::write_bytes(core::ptr::addr_of_mut!((*p).vram) as *mut u8, 0, 0x2000);
            core::ptr::write_bytes(core::ptr::addr_of_mut!((*p).wram) as *mut u8, 0, 0x2000);
            core::ptr::write_bytes(core::ptr::addr_of_mut!((*p).oam) as *mut u8, 0, 0xA0);
            core::ptr::write_bytes(core::ptr::addr_of_mut!((*p).io) as *mut u8, 0, 0x80);
            core::ptr::write_bytes(core::ptr::addr_of_mut!((*p).hram) as *mut u8, 0, 0x7F);
            core::ptr::write(core::ptr::addr_of_mut!((*p).ie), 0u8);
            core::ptr::write(core::ptr::addr_of_mut!((*p).events), BusEventQueue::new());
            b.assume_init()
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
            events: BusEventQueue::new(),
        }
    }

    #[inline(always)]
    fn read_region_fast<const N: usize>(region: &[u8; N], offset: u16) -> u8 {
        // Callers only pass offsets produced by a matching address-range decode
        // (for example `0x8000..=0x9FFF => address - 0x8000` for VRAM), so
        // `offset as usize` is always in `0..N`.
        unsafe { *region.get_unchecked(offset as usize) }
    }

    #[inline(always)]
    fn write_region_fast<const N: usize>(region: &mut [u8; N], offset: u16, value: u8) {
        // Callers only pass offsets produced by a matching address-range decode,
        // so `offset as usize` is always in bounds for `region`. The `&mut`
        // borrow also guarantees this unchecked write is the only active access
        // to the destination element.
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
        self.apply_rom_window_cache_refresh(cartridge_has_rom_windows, rom_windows);
    }

    #[inline(always)]
    pub fn apply_rom_window_cache_refresh(
        &mut self,
        cartridge_has_rom_windows: bool,
        rom_windows: CartridgeRomWindows,
    ) {
        self.cartridge_has_rom_windows = cartridge_has_rom_windows;
        self.rom_fixed_ptr = rom_windows.fixed_ptr;
        self.rom_fixed_len = rom_windows.fixed_len;
        self.rom_banked_ptr = rom_windows.banked_ptr;
        self.rom_banked_len = rom_windows.banked_len;
    }

    #[inline(always)]
    fn read_cached_rom_window(ptr: *const u8, len: usize, offset: usize) -> u8 {
        if offset >= len {
            return 0xFF;
        }
        // `refresh_rom_windows()` only caches pointer/length pairs returned by
        // the cartridge for stable ROM storage, and every ROM-control write
        // refreshes those cached windows before the next fast-path read. The
        // guard above proves `offset < len`, so `ptr.add(offset)` stays within
        // the advertised window. When `len == 0` we return early and never
        // dereference the pointer.
        unsafe { *ptr.add(offset) }
    }

    /// Returns the currently mapped ROM bank number for the switchable window.
    pub fn current_rom_bank(&self) -> usize {
        self.cartridge.current_rom_bank()
    }

    #[inline(always)]
    pub fn has_rtc(&self) -> bool {
        self.cartridge_has_rtc
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
            0x0000..=ROM_BANKED_END => self.read_rom_fast(address),
            VRAM_BASE..=VRAM_END => Self::read_region_fast(&self.vram, address - VRAM_BASE),
            EXT_RAM_BASE..=EXT_RAM_END => self.cartridge.read_ram(address - EXT_RAM_BASE),
            WRAM_BASE..=WRAM_END => Self::read_region_fast(&self.wram, address - WRAM_BASE),
            ECHO_BASE..=ECHO_END => Self::read_region_fast(&self.wram, address - ECHO_BASE),
            OAM_BASE..=OAM_END => Self::read_region_fast(&self.oam, address - OAM_BASE),
            IO_REG_BASE..=IO_REG_END => Self::read_region_fast(&self.io, address - IO_REG_BASE),
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
            0x0000..=ROM_FIXED_END => self.read_rom_fixed_fast(address),
            ROM_BANKED_BASE..=ROM_BANKED_END => self.read_rom_banked_fast(address),
            _ => 0xFF,
        }
    }

    #[inline(always)]
    pub fn read_rom_fixed_fast(&self, address: u16) -> u8 {
        debug_assert!(address <= ROM_FIXED_END);
        if self.cartridge_has_rom_windows {
            Self::read_cached_rom_window(self.rom_fixed_ptr, self.rom_fixed_len, address as usize)
        } else {
            self.cartridge.read_rom(address)
        }
    }

    #[inline(always)]
    pub fn read_rom_banked_fast(&self, address: u16) -> u8 {
        debug_assert!((ROM_BANKED_BASE..=ROM_BANKED_END).contains(&address));
        if self.cartridge_has_rom_windows {
            Self::read_cached_rom_window(
                self.rom_banked_ptr,
                self.rom_banked_len,
                (address - ROM_BANKED_BASE) as usize,
            )
        } else {
            self.cartridge.read_rom(address)
        }
    }

    /// Fast infallible memory write used by hot non-IO paths.
    #[inline(always)]
    pub fn write_fast(&mut self, address: u16, value: u8) {
        match address {
            0x0000..=ROM_BANKED_END => {
                self.cartridge.write(address, value);
                self.refresh_rom_windows();
            }
            EXT_RAM_BASE..=EXT_RAM_END => self.cartridge.write(address, value),
            VRAM_BASE..=VRAM_END => {
                Self::write_region_fast(&mut self.vram, address - VRAM_BASE, value)
            }
            WRAM_BASE..=WRAM_END => {
                Self::write_region_fast(&mut self.wram, address - WRAM_BASE, value)
            }
            ECHO_BASE..=ECHO_END => {}
            OAM_BASE..=OAM_END => Self::write_region_fast(&mut self.oam, address - OAM_BASE, value),
            IO_REG_BASE..=IO_REG_END => {
                Self::write_region_fast(&mut self.io, address - IO_REG_BASE, value)
            }
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

    /// Copy `count` bytes from `source + progress` into OAM at `progress`.
    /// Uses a zero-copy slice path for regions with direct backing storage
    /// (VRAM, WRAM, cached ROM); falls back to byte-by-byte for cart RAM.
    // Defense-in-depth for bug #5 (regalloc stack-slot collision): `#[inline(never)]`
    // keeps this OAM-DMA copy in its own stack frame so its base spill cannot be
    // coalesced onto a sibling's slot. Root cause fixed via `-regalloc=basic`; do NOT re-inline.
    // See platform/pico2w/docs/investigations/oam-dma-bisection.md.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(never)]
    pub fn copy_dma_step(&mut self, source: u16, progress: u8, count: u8) {
        // actual_src is the real memory address to read from: base + bytes already transferred.
        let actual_src = source as usize + progress as usize;
        let n = count as usize;
        let dst = progress as usize;
        if dst > self.oam.len() || n > self.oam.len() - dst {
            let packed_dma = ((source as u32) << 16) | ((progress as u32) << 8) | count as u32;
            panic!(
                "oam dma out of range: packed={packed_dma:#010x} actual_src={actual_src:#06x} dst={dst} count={n}"
            );
        }

        let copied = if (VRAM_BASE as usize..=VRAM_END as usize).contains(&actual_src) {
            let off = actual_src - VRAM_BASE as usize;
            off + n <= self.vram.len() && {
                self.oam[dst..dst + n].copy_from_slice(&self.vram[off..off + n]);
                true
            }
        } else if (WRAM_BASE as usize..=WRAM_END as usize).contains(&actual_src) {
            let off = actual_src - WRAM_BASE as usize;
            off + n <= self.wram.len() && {
                self.oam[dst..dst + n].copy_from_slice(&self.wram[off..off + n]);
                true
            }
        } else if (ECHO_BASE as usize..=ECHO_END as usize).contains(&actual_src) {
            let off = actual_src - ECHO_BASE as usize;
            off + n <= self.wram.len() && {
                self.oam[dst..dst + n].copy_from_slice(&self.wram[off..off + n]);
                true
            }
        } else if self.cartridge_has_rom_windows
            && actual_src <= ROM_FIXED_END as usize
            && actual_src + n <= self.rom_fixed_len
        {
            // Safety: rom_fixed_ptr is valid for rom_fixed_len bytes and does not alias oam.
            unsafe {
                let sl = core::slice::from_raw_parts(self.rom_fixed_ptr.add(actual_src), n);
                self.oam[dst..dst + n].copy_from_slice(sl);
            }
            true
        } else if self.cartridge_has_rom_windows
            && (ROM_BANKED_BASE as usize..=ROM_BANKED_END as usize).contains(&actual_src)
            && (actual_src - ROM_BANKED_BASE as usize) + n <= self.rom_banked_len
        {
            let off = actual_src - ROM_BANKED_BASE as usize;
            // Safety: rom_banked_ptr is valid for rom_banked_len bytes and does not alias oam.
            unsafe {
                let sl = core::slice::from_raw_parts(self.rom_banked_ptr.add(off), n);
                self.oam[dst..dst + n].copy_from_slice(sl);
            }
            true
        } else {
            false
        };

        if !copied {
            for i in 0..n {
                self.oam[dst + i] = self.read_fast((actual_src + i) as u16);
            }
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

    /// Serialize memory state into the RBSS v1 tail.
    pub fn save_state_v1(&self, out: &mut alloc::vec::Vec<u8>) {
        self.save_fixed_regions(out);
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

    /// Serialize memory state into the RBSS v2 tail.
    pub fn save_state_v2(&self, out: &mut alloc::vec::Vec<u8>) {
        self.save_fixed_regions(out);

        let mut mbc = alloc::vec::Vec::new();
        self.cartridge.save_mbc_state(&mut mbc);
        write_mbc_section(out, &mbc);

        if let Some(ram) = self.cartridge.external_ram() {
            write_cart_ram_section(out, ram);
        }
    }

    /// Backward-compatible alias for older call sites.
    pub fn save_state(&self, out: &mut alloc::vec::Vec<u8>) {
        self.save_state_v1(out);
    }

    /// Serialize fixed memory regions shared by RBSS v1 and v2.
    /// IO registers (0x80 bytes) + IE (1 byte) + WRAM + HRAM + VRAM + OAM.
    fn save_fixed_regions(&self, out: &mut alloc::vec::Vec<u8>) {
        for i in 0..0x80u16 {
            out.push(self.read_io(0xFF00 + i));
        }
        out.push(self.ie);
        out.extend_from_slice(self.wram());
        out.extend_from_slice(self.hram());
        out.extend_from_slice(self.vram());
        out.extend_from_slice(self.oam());
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
        if let Some(payload) = state.mbc_payload() {
            self.cartridge.load_mbc_state(payload, 0);
            self.refresh_rom_windows();
        } else if let Some(mbc) = state.mbc() {
            // Reconstruct MBC register state via the existing load path.
            // We build a minimal 4-byte buffer and reuse load_mbc_state.
            let buf = [
                mbc.rom_bank_lo,
                mbc.upper_bits,
                mbc.ram_mode as u8,
                mbc.ram_enabled as u8,
            ];
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

    /// Reset RAM regions and MBC registers to power-on state. Cart RAM is preserved.
    pub fn soft_reset(&mut self) {
        self.wram.fill(0);
        self.vram.fill(0);
        self.hram.fill(0);
        self.oam.fill(0);
        self.io.fill(0);
        self.ie = 0;
        self.cartridge.reset_mbc();
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

    /// Queue a worker-mirrored bus event after the caller has already updated
    /// memory through a direct fast path.
    #[inline(always)]
    pub fn enqueue_bus_event(&mut self, address: u16, value: u8) {
        self.events
            .push(BusEvent { address, value })
            .expect("bus event queue overflow");
    }

    /// Returns a read-only view of the IO register array (0xFF00–0xFF7F).
    pub fn io_slice(&self) -> &[u8] {
        &self.io
    }

    /// Split-borrow accessor: returns (io, vram, oam) as separate references so
    /// callers can pass io as `&mut` to the PPU while holding read-only vram/oam.
    pub fn ppu_tick_data(&mut self) -> (&mut [u8], &[u8], &[u8]) {
        (&mut self.io, &self.vram, &self.oam)
    }

    /// Returns true if there are any pending bus events.
    #[inline(always)]
    pub fn has_events(&self) -> bool {
        !self.events.is_empty()
    }

    /// Drain pending bus events into a caller-owned fixed buffer.
    pub fn drain_into_slice(&mut self, buf: &mut [BusEvent]) -> usize {
        let n = self.events.len().min(buf.len());
        buf[..n].copy_from_slice(&self.events[..n]);
        self.events.clear();
        n
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
                self.events
                    .push(BusEvent { address, value })
                    .expect("bus event queue overflow");
                Ok(())
            }
            RegionMapping::Wram(offset) => {
                self.wram[offset as usize] = value;
                Ok(())
            }
            RegionMapping::EchoRam(_) => Err(Error::ReadOnly(address)),
            RegionMapping::Oam(offset) => {
                self.oam[offset as usize] = value;
                self.events
                    .push(BusEvent { address, value })
                    .expect("bus event queue overflow");
                Ok(())
            }
            RegionMapping::Io(offset) => {
                self.io[offset as usize] = value;
                self.events
                    .push(BusEvent { address, value })
                    .expect("bus event queue overflow");
                Ok(())
            }
            RegionMapping::Hram(offset) => {
                self.hram[offset as usize] = value;
                Ok(())
            }
            RegionMapping::InterruptEnable => {
                self.ie = value;
                self.events
                    .push(BusEvent { address, value })
                    .expect("bus event queue overflow");
                Ok(())
            }
            RegionMapping::Unmapped => Ok(()),
        }
    }

    fn drain_events(&mut self) -> Vec<BusEvent> {
        let mut out = Vec::with_capacity(self.events.len());
        out.extend_from_slice(&self.events);
        self.events.clear();
        out
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
