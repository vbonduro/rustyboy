//! Typed save state representation for the RBSS v1 format.
//!
//! On the **save path**, `Sm83::save_state()` writes directly to a `Vec<u8>` —
//! no `SaveState` instance is created.
//!
//! On the **load path**, `SaveState::from_blob(blob)` parses and validates the
//! entire blob up front, returning `Err` if anything is wrong before any
//! emulator state is touched. The struct owns the blob and exposes zero-copy
//! slice accessors for large memory regions. Scalar fields are copied out at
//! parse time. Call `Sm83::load_state(state)` to apply a parsed `SaveState`.

use alloc::vec::Vec;
use core::mem::size_of;
use core::ops::Range;

use crate::cpu::peripheral::ppu::PpuMode;
use crate::cpu::registers::{Flags, Registers};
use crate::cpu::sm83::ImeState;

// ── Format constants ──────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4]  = b"RBSS";
pub const VERSION: u16      = 1;

// Fixed-size regions in the blob (bytes).
const MAGIC_SIZE:         usize = 4;
const VERSION_SIZE:       usize = size_of::<u16>();
const HEADER_SIZE:        usize = MAGIC_SIZE + VERSION_SIZE;

const CPU_REGS_SIZE:      usize = 7 * size_of::<u8>()   // A B C D E H L
                                + size_of::<u8>()        // F (flags)
                                + size_of::<u16>()       // SP
                                + size_of::<u16>();      // PC

const IME_SIZE:           usize = size_of::<u8>();
const HALTED_SIZE:        usize = size_of::<u8>();
const CYCLE_COUNTER_SIZE: usize = size_of::<u64>();
const TIMER_SIZE:         usize = size_of::<u16>();      // internal_counter
const PPU_SIZE:           usize = size_of::<u16>()       // dot
                                + size_of::<u8>()        // ly
                                + size_of::<u8>()        // mode
                                + size_of::<u8>();       // window_line_counter

const IO_REGS_SIZE:       usize = 0x80;
const IE_SIZE:            usize = size_of::<u8>();
const WRAM_SIZE:          usize = 0x2000;
const HRAM_SIZE:          usize = 0x7F;
const VRAM_SIZE:          usize = 0x2000;
const OAM_SIZE:           usize = 0xA0;
const MBC_SIZE:           usize = 4 * size_of::<u8>();  // rom_bank_lo upper_bits ram_mode ram_enabled
const CART_RAM_LEN_SIZE:  usize = size_of::<u16>();

/// Minimum valid blob length: everything up through OAM, without optional MBC/cart RAM.
pub const MIN_BLOB_SIZE: usize = HEADER_SIZE
    + CPU_REGS_SIZE + IME_SIZE + HALTED_SIZE + CYCLE_COUNTER_SIZE
    + TIMER_SIZE + PPU_SIZE
    + IO_REGS_SIZE + IE_SIZE + WRAM_SIZE + HRAM_SIZE + VRAM_SIZE + OAM_SIZE;

// ── Scalar field containers ───────────────────────────────────────────────────

/// CPU register state. Parsed from the blob; does not include IME or halted.
#[derive(Debug, Clone, Copy)]
pub struct CpuFields {
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
}

impl CpuFields {
    /// Parse from `blob` at `offset`. Returns `(fields, bytes_consumed)`.
    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let b = &blob[offset..];
        let fields = CpuFields {
            a:  b[0],
            b:  b[1],
            c:  b[2],
            d:  b[3],
            e:  b[4],
            h:  b[5],
            l:  b[6],
            f:  Flags::from_bits_truncate(b[7]),
            sp: u16::from_le_bytes([b[8],  b[9]]),
            pc: u16::from_le_bytes([b[10], b[11]]),
        };
        (fields, CPU_REGS_SIZE)
    }

    pub fn to_registers(self) -> Registers {
        Registers {
            a: self.a, b: self.b, c: self.c, d: self.d,
            e: self.e, h: self.h, l: self.l, f: self.f,
            sp: self.sp, pc: self.pc,
        }
    }
}

/// PPU scalar state.
#[derive(Debug, Clone, Copy)]
pub struct PpuFields {
    pub dot: u16,
    pub ly: u8,
    pub mode: PpuMode,
    pub window_line_counter: u8,
}

impl PpuFields {
    /// Parse from `blob` at `offset`. Returns `(fields, bytes_consumed)`.
    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let b = &blob[offset..];
        let fields = PpuFields {
            dot:                  u16::from_le_bytes([b[0], b[1]]),
            ly:                   b[2],
            mode: match b[3] {
                0 => PpuMode::HBlank,
                1 => PpuMode::VBlank,
                2 => PpuMode::OamScan,
                _ => PpuMode::PixelTransfer,
            },
            window_line_counter:  b[4],
        };
        (fields, PPU_SIZE)
    }
}

/// MBC register state.
#[derive(Debug, Clone, Copy)]
pub struct MbcFields {
    pub rom_bank_lo: u8,
    pub upper_bits: u8,
    pub ram_mode: bool,
    pub ram_enabled: bool,
}

impl MbcFields {
    /// Parse from `blob` at `offset`. Returns `(fields, bytes_consumed)`.
    /// Applies the MBC1 bank-0 quirk (rom_bank_lo 0 → 1).
    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let b = &blob[offset..];
        let fields = MbcFields {
            rom_bank_lo: b[0].max(1),
            upper_bits:  b[1] & 0x03,
            ram_mode:    b[2] != 0,
            ram_enabled: b[3] != 0,
        };
        (fields, MBC_SIZE)
    }
}

/// Parse IME state from one byte. Returns `(ImeState, bytes_consumed)`.
fn parse_ime(blob: &[u8], offset: usize) -> (ImeState, usize) {
    let ime = match blob[offset] {
        1 => ImeState::Pending,
        2 => ImeState::Enabled,
        _ => ImeState::Disabled,
    };
    (ime, IME_SIZE)
}

// ── SaveState ─────────────────────────────────────────────────────────────────

/// A parsed, validated RBSS v1 save state blob.
///
/// The blob is owned by this struct. Large memory regions (WRAM, VRAM, etc.)
/// are zero-copy — accessed as slices via stored range indices. Scalar fields
/// are copied out at parse time.
pub struct SaveState {
    blob: Vec<u8>,

    // Scalar fields — cheap to copy, exposed directly.
    cpu: CpuFields,
    pub ime: ImeState,
    pub halted: bool,
    pub cycle_counter: u64,
    pub timer_internal_counter: u16,
    pub ppu: PpuFields,
    mbc: Option<MbcFields>,

    // Ranges into `blob` for the large memory regions.
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

        let (cpu, n) = CpuFields::parse(&blob, cur);   cur += n;

        let (ime, n) = parse_ime(&blob, cur);           cur += n;
        let halted = blob[cur] != 0;                    cur += HALTED_SIZE;

        let cycle_counter = u64::from_le_bytes(
            blob[cur..cur + CYCLE_COUNTER_SIZE].try_into().unwrap()
        );                                              cur += CYCLE_COUNTER_SIZE;

        let timer_internal_counter = u16::from_le_bytes([blob[cur], blob[cur + 1]]);
                                                        cur += TIMER_SIZE;

        let (ppu, n) = PpuFields::parse(&blob, cur);   cur += n;

        let io_range  = cur..cur + IO_REGS_SIZE;        cur += IO_REGS_SIZE;
        let ie_offset = cur;                            cur += IE_SIZE;
        let wram_range = cur..cur + WRAM_SIZE;          cur += WRAM_SIZE;
        let hram_range = cur..cur + HRAM_SIZE;          cur += HRAM_SIZE;
        let vram_range = cur..cur + VRAM_SIZE;          cur += VRAM_SIZE;
        let oam_range  = cur..cur + OAM_SIZE;           cur += OAM_SIZE;

        let mbc = if cur + MBC_SIZE <= blob.len() {
            let (m, n) = MbcFields::parse(&blob, cur); cur += n;
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
            blob, cpu, ime, halted, cycle_counter, timer_internal_counter,
            ppu, mbc, io_range, ie_offset, wram_range, hram_range, vram_range,
            oam_range, cart_ram_range,
        })
    }

    // ── Scalar accessors ──────────────────────────────────────────────────────

    pub fn cpu(&self) -> &CpuFields          { &self.cpu }
    pub fn mbc(&self) -> Option<&MbcFields>  { self.mbc.as_ref() }
    pub fn cycle_counter(&self) -> u64       { self.cycle_counter }

    // ── Zero-copy slice accessors ─────────────────────────────────────────────

    pub fn io_registers(&self) -> &[u8]      { &self.blob[self.io_range.clone()] }
    pub fn ie(&self) -> u8                   { self.blob[self.ie_offset] }
    pub fn wram(&self) -> &[u8]              { &self.blob[self.wram_range.clone()] }
    pub fn hram(&self) -> &[u8]              { &self.blob[self.hram_range.clone()] }
    pub fn vram(&self) -> &[u8]              { &self.blob[self.vram_range.clone()] }
    pub fn oam(&self)  -> &[u8]              { &self.blob[self.oam_range.clone()] }
    pub fn cart_ram(&self) -> Option<&[u8]>  {
        self.cart_ram_range.as_ref().map(|r| &self.blob[r.clone()])
    }
}
