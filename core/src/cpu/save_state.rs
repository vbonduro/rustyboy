//! Typed save state representation for the RBSS v1 format.
//!
//! On the **save path**, `Sm83::save_state()` writes directly to a `Vec<u8>` —
//! no `SaveState` instance is created.
//!
//! On the **load path**, `SaveState::from_blob(blob)` parses and validates the
//! entire blob up front, returning `Err` if anything is wrong before any
//! emulator state is touched. The struct owns the blob and exposes zero-copy
//! slice accessors for large memory regions. Each component's state struct is
//! copied out at parse time and applied via that component's `load_state`.

use alloc::vec::Vec;
use core::mem::size_of;
use core::ops::Range;

use crate::cpu::peripheral::ppu::PpuMode;
use crate::cpu::registers::{Flags, Registers};
use crate::cpu::sm83::ImeState;

// ── Format constants ──────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4] = b"RBSS";
pub const VERSION: u16     = 1;

const MAGIC_SIZE:         usize = 4;
const VERSION_SIZE:       usize = size_of::<u16>();
const HEADER_SIZE:        usize = MAGIC_SIZE + VERSION_SIZE;

const CPU_REGS_SIZE:      usize = 7 * size_of::<u8>()  // A B C D E H L
                                + size_of::<u8>()       // F (flags)
                                + size_of::<u16>()      // SP
                                + size_of::<u16>();     // PC
const IME_SIZE:           usize = size_of::<u8>();
const HALTED_SIZE:        usize = size_of::<u8>();
const CYCLE_COUNTER_SIZE: usize = size_of::<u64>();
const CPU_STATE_SIZE:     usize = CPU_REGS_SIZE + IME_SIZE + HALTED_SIZE + CYCLE_COUNTER_SIZE;

const TIMER_STATE_SIZE:   usize = size_of::<u16>();     // internal_counter

const PPU_STATE_SIZE:     usize = size_of::<u16>()      // dot
                                + size_of::<u8>()       // ly
                                + size_of::<u8>()       // mode
                                + size_of::<u8>();      // window_line_counter

const IO_REGS_SIZE:       usize = 0x80;
const IE_SIZE:            usize = size_of::<u8>();
const WRAM_SIZE:          usize = 0x2000;
const HRAM_SIZE:          usize = 0x7F;
const VRAM_SIZE:          usize = 0x2000;
const OAM_SIZE:           usize = 0xA0;
const MBC_SIZE:           usize = 4 * size_of::<u8>(); // rom_bank_lo upper_bits ram_mode ram_enabled
const CART_RAM_LEN_SIZE:  usize = size_of::<u16>();

/// Minimum valid blob length: everything up through OAM, without optional MBC/cart RAM.
pub const MIN_BLOB_SIZE: usize = HEADER_SIZE + CPU_STATE_SIZE + TIMER_STATE_SIZE
    + PPU_STATE_SIZE + IO_REGS_SIZE + IE_SIZE + WRAM_SIZE + HRAM_SIZE + VRAM_SIZE + OAM_SIZE;

// ── Component state structs ───────────────────────────────────────────────────

/// Full CPU state: registers + IME + halted + cycle counter.
#[derive(Debug, Clone, Copy)]
pub struct CpuState {
    pub a: u8,
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    pub f: Flags,
    pub sp: u16,
    pub pc: u16,
    pub ime: ImeState,
    pub halted: bool,
    pub cycle_counter: u64,
}

impl CpuState {
    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let b = &blob[offset..];
        let ime = match b[CPU_REGS_SIZE] {
            1 => ImeState::Pending,
            2 => ImeState::Enabled,
            _ => ImeState::Disabled,
        };
        let state = CpuState {
            a:  b[0], b: b[1], c: b[2], d: b[3],
            e:  b[4], h: b[5], l: b[6],
            f:  Flags::from_bits_truncate(b[7]),
            sp: u16::from_le_bytes([b[8],  b[9]]),
            pc: u16::from_le_bytes([b[10], b[11]]),
            ime,
            halted:        b[CPU_REGS_SIZE + IME_SIZE] != 0,
            cycle_counter: u64::from_le_bytes(
                b[CPU_REGS_SIZE + IME_SIZE + HALTED_SIZE
                ..CPU_REGS_SIZE + IME_SIZE + HALTED_SIZE + CYCLE_COUNTER_SIZE]
                    .try_into().unwrap()
            ),
        };
        (state, CPU_STATE_SIZE)
    }

    pub fn to_registers(self) -> Registers {
        Registers {
            a: self.a, b: self.b, c: self.c, d: self.d,
            e: self.e, h: self.h, l: self.l, f: self.f,
            sp: self.sp, pc: self.pc,
        }
    }
}

/// Timer peripheral state.
#[derive(Debug, Clone, Copy)]
pub struct TimerState {
    pub internal_counter: u16,
}

impl TimerState {
    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let state = TimerState {
            internal_counter: u16::from_le_bytes([blob[offset], blob[offset + 1]]),
        };
        (state, TIMER_STATE_SIZE)
    }
}

/// PPU peripheral state.
#[derive(Debug, Clone, Copy)]
pub struct PpuState {
    pub dot: u16,
    pub ly: u8,
    pub mode: PpuMode,
    pub window_line_counter: u8,
}

impl PpuState {
    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let b = &blob[offset..];
        let state = PpuState {
            dot:                 u16::from_le_bytes([b[0], b[1]]),
            ly:                  b[2],
            mode: match b[3] {
                0 => PpuMode::HBlank,
                1 => PpuMode::VBlank,
                2 => PpuMode::OamScan,
                _ => PpuMode::PixelTransfer,
            },
            window_line_counter: b[4],
        };
        (state, PPU_STATE_SIZE)
    }
}

/// MBC register state (covers MBC1 and MBC1Multicart layouts).
#[derive(Debug, Clone, Copy)]
pub struct MbcState {
    pub rom_bank_lo: u8,
    pub upper_bits: u8,
    pub ram_mode: bool,
    pub ram_enabled: bool,
}

impl MbcState {
    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let b = &blob[offset..];
        let state = MbcState {
            rom_bank_lo: b[0].max(1),
            upper_bits:  b[1] & 0x03,
            ram_mode:    b[2] != 0,
            ram_enabled: b[3] != 0,
        };
        (state, MBC_SIZE)
    }
}

// ── SaveState ─────────────────────────────────────────────────────────────────

/// A parsed, validated RBSS v1 save state blob.
///
/// Owns the blob. Large memory regions are zero-copy slices via range indices.
/// Each component's state is a typed struct applied via that component's
/// `load_state` method.
pub struct SaveState {
    blob: Vec<u8>,

    pub cpu:   CpuState,
    pub timer: TimerState,
    pub ppu:   PpuState,
    mbc:       Option<MbcState>,

    io_range:       Range<usize>,
    ie_offset:      usize,
    wram_range:     Range<usize>,
    hram_range:     Range<usize>,
    vram_range:     Range<usize>,
    oam_range:      Range<usize>,
    cart_ram_range: Option<Range<usize>>,
}

impl SaveState {
    /// Parse and validate a raw RBSS v1 blob.
    ///
    /// Returns `Err` if the blob is too short, has a bad magic, or has an
    /// unsupported version. No emulator state is modified.
    pub fn from_blob(blob: Vec<u8>) -> Result<Self, &'static str> {
        if blob.len() < MIN_BLOB_SIZE {
            return Err("save state blob too short");
        }
        if &blob[0..MAGIC_SIZE] != MAGIC {
            return Err("invalid save state magic");
        }
        let version = u16::from_le_bytes([blob[MAGIC_SIZE], blob[MAGIC_SIZE + 1]]);
        if version != VERSION {
            return Err("unsupported save state version");
        }

        let mut cur = HEADER_SIZE;

        let (cpu,   n) = CpuState::parse(&blob, cur);   cur += n;
        let (timer, n) = TimerState::parse(&blob, cur);  cur += n;
        let (ppu,   n) = PpuState::parse(&blob, cur);    cur += n;

        let io_range  = cur..cur + IO_REGS_SIZE;          cur += IO_REGS_SIZE;
        let ie_offset = cur;                              cur += IE_SIZE;
        let wram_range = cur..cur + WRAM_SIZE;            cur += WRAM_SIZE;
        let hram_range = cur..cur + HRAM_SIZE;            cur += HRAM_SIZE;
        let vram_range = cur..cur + VRAM_SIZE;            cur += VRAM_SIZE;
        let oam_range  = cur..cur + OAM_SIZE;             cur += OAM_SIZE;

        let mbc = if cur + MBC_SIZE <= blob.len() {
            let (m, n) = MbcState::parse(&blob, cur);    cur += n;
            Some(m)
        } else {
            None
        };

        let cart_ram_range = if cur + CART_RAM_LEN_SIZE <= blob.len() {
            let ram_len = u16::from_le_bytes([blob[cur], blob[cur + 1]]) as usize;
            cur += CART_RAM_LEN_SIZE;
            if ram_len > 0 && cur + ram_len <= blob.len() {
                Some(cur..cur + ram_len)
            } else {
                None
            }
        } else {
            None
        };

        Ok(SaveState {
            blob, cpu, timer, ppu, mbc,
            io_range, ie_offset, wram_range, hram_range, vram_range, oam_range,
            cart_ram_range,
        })
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    pub fn mbc(&self) -> Option<&MbcState>  { self.mbc.as_ref() }

    pub fn io_registers(&self) -> &[u8]     { &self.blob[self.io_range.clone()] }
    pub fn ie(&self) -> u8                  { self.blob[self.ie_offset] }
    pub fn wram(&self) -> &[u8]             { &self.blob[self.wram_range.clone()] }
    pub fn hram(&self) -> &[u8]             { &self.blob[self.hram_range.clone()] }
    pub fn vram(&self) -> &[u8]             { &self.blob[self.vram_range.clone()] }
    pub fn oam(&self)  -> &[u8]             { &self.blob[self.oam_range.clone()] }
    pub fn cart_ram(&self) -> Option<&[u8]> {
        self.cart_ram_range.as_ref().map(|r| &self.blob[r.clone()])
    }
}
