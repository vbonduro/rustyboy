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
use core::ops::Range;

use crate::cpu::peripheral::ppu::PpuMode;
use crate::cpu::registers::{Flags, Registers};
use crate::cpu::sm83::ImeState;

// ── Scalar field containers ───────────────────────────────────────────────────

/// CPU register state copied out of the blob at parse time.
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
    pub fn to_registers(self) -> Registers {
        Registers {
            a: self.a,
            b: self.b,
            c: self.c,
            d: self.d,
            e: self.e,
            h: self.h,
            l: self.l,
            f: self.f,
            sp: self.sp,
            pc: self.pc,
        }
    }
}

/// PPU scalar state copied out of the blob at parse time.
#[derive(Debug, Clone, Copy)]
pub struct PpuFields {
    pub dot: u16,
    pub ly: u8,
    pub mode: PpuMode,
    pub window_line_counter: u8,
}

/// MBC register state copied out of the blob at parse time.
#[derive(Debug, Clone, Copy)]
pub struct MbcFields {
    pub rom_bank_lo: u8,
    pub upper_bits: u8,
    pub ram_mode: bool,
    pub ram_enabled: bool,
}

// ── SaveState ─────────────────────────────────────────────────────────────────

/// A parsed, validated RBSS v1 save state blob.
///
/// The blob is owned by this struct. Large memory regions (WRAM, VRAM, etc.)
/// are accessed as zero-copy slices via range indices into the blob. Scalar
/// fields are copied out at parse time and accessible directly.
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
    io_range: Range<usize>,
    ie_offset: usize,
    wram_range: Range<usize>,
    hram_range: Range<usize>,
    vram_range: Range<usize>,
    oam_range: Range<usize>,
    cart_ram_range: Option<Range<usize>>,
}

impl SaveState {
    /// Parse and validate a raw RBSS v1 blob.
    ///
    /// Returns `Err` if the blob is too short, has a bad magic, or has an
    /// unsupported version. No emulator state is modified.
    pub fn from_blob(blob: Vec<u8>) -> Result<Self, &'static str> {
        if blob.len() < 16835 {
            return Err("save state blob too short");
        }
        if &blob[0..4] != b"RBSS" {
            return Err("invalid save state magic");
        }
        let version = u16::from_le_bytes([blob[4], blob[5]]);
        if version != 1 {
            return Err("unsupported save state version");
        }

        let mut cur = 6usize;

        // CPU registers (10 bytes)
        let cpu = CpuFields {
            a:  blob[cur],     b: blob[cur + 1], c: blob[cur + 2],
            d:  blob[cur + 3], e: blob[cur + 4], h: blob[cur + 5],
            l:  blob[cur + 6],
            f:  Flags::from_bits_truncate(blob[cur + 7]),
            sp: u16::from_le_bytes([blob[cur + 8],  blob[cur + 9]]),
            pc: u16::from_le_bytes([blob[cur + 10], blob[cur + 11]]),
        };
        cur += 12;

        // IME + halted (2 bytes)
        let ime = match blob[cur] {
            1 => ImeState::Pending,
            2 => ImeState::Enabled,
            _ => ImeState::Disabled,
        };
        cur += 1;
        let halted = blob[cur] != 0;
        cur += 1;

        // Cycle counter (8 bytes)
        let cycle_counter = u64::from_le_bytes([
            blob[cur],     blob[cur + 1], blob[cur + 2], blob[cur + 3],
            blob[cur + 4], blob[cur + 5], blob[cur + 6], blob[cur + 7],
        ]);
        cur += 8;

        // Timer (2 bytes)
        let timer_internal_counter = u16::from_le_bytes([blob[cur], blob[cur + 1]]);
        cur += 2;

        // PPU (5 bytes)
        let ppu = PpuFields {
            dot: u16::from_le_bytes([blob[cur], blob[cur + 1]]),
            ly:  blob[cur + 2],
            mode: match blob[cur + 3] {
                0 => PpuMode::HBlank,
                1 => PpuMode::VBlank,
                2 => PpuMode::OamScan,
                _ => PpuMode::PixelTransfer,
            },
            window_line_counter: blob[cur + 4],
        };
        cur += 5;

        // IO registers (0x80 bytes)
        let io_range = cur..cur + 0x80;
        cur += 0x80;

        // IE register (1 byte)
        let ie_offset = cur;
        cur += 1;

        // WRAM (0x2000 bytes)
        let wram_range = cur..cur + 0x2000;
        cur += 0x2000;

        // HRAM (0x7F bytes)
        let hram_range = cur..cur + 0x7F;
        cur += 0x7F;

        // VRAM (0x2000 bytes)
        let vram_range = cur..cur + 0x2000;
        cur += 0x2000;

        // OAM (0xA0 bytes)
        let oam_range = cur..cur + 0xA0;
        cur += 0xA0;

        // MBC state (optional, 4 bytes)
        let mbc = if cur + 4 <= blob.len() {
            let m = MbcFields {
                rom_bank_lo: blob[cur].max(1),
                upper_bits:  blob[cur + 1] & 0x03,
                ram_mode:    blob[cur + 2] != 0,
                ram_enabled: blob[cur + 3] != 0,
            };
            cur += 4;
            Some(m)
        } else {
            None
        };

        // External cart RAM (optional, u16 LE length prefix)
        let cart_ram_range = if cur + 2 <= blob.len() {
            let ram_len = u16::from_le_bytes([blob[cur], blob[cur + 1]]) as usize;
            cur += 2;
            if ram_len > 0 && cur + ram_len <= blob.len() {
                let range = cur..cur + ram_len;
                Some(range)
            } else {
                None
            }
        } else {
            None
        };

        Ok(SaveState {
            blob,
            cpu,
            ime,
            halted,
            cycle_counter,
            timer_internal_counter,
            ppu,
            mbc,
            io_range,
            ie_offset,
            wram_range,
            hram_range,
            vram_range,
            oam_range,
            cart_ram_range,
        })
    }

    // ── Scalar accessors ──────────────────────────────────────────────────────

    pub fn cpu(&self) -> &CpuFields { &self.cpu }
    pub fn mbc(&self) -> Option<&MbcFields> { self.mbc.as_ref() }
    pub fn cycle_counter(&self) -> u64 { self.cycle_counter }

    // ── Zero-copy slice accessors ─────────────────────────────────────────────

    pub fn io_registers(&self) -> &[u8] { &self.blob[self.io_range.clone()] }
    pub fn ie(&self) -> u8 { self.blob[self.ie_offset] }
    pub fn wram(&self) -> &[u8] { &self.blob[self.wram_range.clone()] }
    pub fn hram(&self) -> &[u8] { &self.blob[self.hram_range.clone()] }
    pub fn vram(&self) -> &[u8] { &self.blob[self.vram_range.clone()] }
    pub fn oam(&self) -> &[u8]  { &self.blob[self.oam_range.clone()] }
    pub fn cart_ram(&self) -> Option<&[u8]> {
        self.cart_ram_range.as_ref().map(|r| &self.blob[r.clone()])
    }
}
