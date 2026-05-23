//! Typed save state representation for RBSS save-state blobs.
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
use crate::memory::memory::GameBoyMemory;

// ── Format constants ──────────────────────────────────────────────────────────

pub const MAGIC: &[u8; 4] = b"RBSS";
pub const VERSION_V1: u16 = 1;
pub const VERSION_V2: u16 = 2;
pub const VERSION: u16 = VERSION_V2;

const MAGIC_SIZE: usize = 4;
const VERSION_SIZE: usize = size_of::<u16>();
const HEADER_SIZE: usize = MAGIC_SIZE + VERSION_SIZE;

const CPU_REGS_SIZE: usize = 7 * size_of::<u8>()  // A B C D E H L
                                + size_of::<u8>()       // F (flags)
                                + size_of::<u16>()      // SP
                                + size_of::<u16>(); // PC
const IME_SIZE: usize = size_of::<u8>();
const HALTED_SIZE: usize = size_of::<u8>();
const CYCLE_COUNTER_SIZE: usize = size_of::<u64>();
const CPU_STATE_SIZE: usize = CPU_REGS_SIZE + IME_SIZE + HALTED_SIZE + CYCLE_COUNTER_SIZE;

const TIMER_STATE_SIZE: usize = size_of::<u16>()      // internal_counter
                                + 3 * size_of::<u8>(); // tima, tma, tac

const PPU_STATE_SIZE: usize = size_of::<u16>()      // dot
                                + size_of::<u8>()       // ly
                                + size_of::<u8>()       // mode
                                + size_of::<u8>()       // window_line_counter
                                + 10 * size_of::<u8>(); // lcdc stat scy scx lyc bgp obp0 obp1 wy wx

const IO_REGS_SIZE: usize = 0x80;
const IE_SIZE: usize = size_of::<u8>();
const WRAM_SIZE: usize = 0x2000;
const HRAM_SIZE: usize = 0x7F;
const VRAM_SIZE: usize = 0x2000;
const OAM_SIZE: usize = 0xA0;
const MBC_SIZE_V1_LEGACY: usize = 4 * size_of::<u8>(); // rom_bank_lo upper_bits ram_mode ram_enabled
const MBC_SIZE_V1_MBC3_RTC: usize = 18;
const CART_RAM_LEN_SIZE_V1: usize = size_of::<u16>();
const SECTION_HEADER_SIZE: usize = 8; // 4-byte tag + u32 length
const SECTION_MBC: &[u8; 4] = b"MBC\0";
const SECTION_CART_RAM: &[u8; 4] = b"CRAM";

/// Minimum valid blob length: everything up through OAM, without optional MBC/cart RAM.
pub const MIN_BLOB_SIZE: usize = HEADER_SIZE
    + CPU_STATE_SIZE
    + TIMER_STATE_SIZE
    + PPU_STATE_SIZE
    + IO_REGS_SIZE
    + IE_SIZE
    + WRAM_SIZE
    + HRAM_SIZE
    + VRAM_SIZE
    + OAM_SIZE;

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
    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.push(self.a);
        out.push(self.b);
        out.push(self.c);
        out.push(self.d);
        out.push(self.e);
        out.push(self.h);
        out.push(self.l);
        out.push(self.f.bits());
        out.extend_from_slice(&self.sp.to_le_bytes());
        out.extend_from_slice(&self.pc.to_le_bytes());
        out.push(match self.ime {
            ImeState::Disabled => 0,
            ImeState::Pending => 1,
            ImeState::Enabled => 2,
        });
        out.push(self.halted as u8);
        out.extend_from_slice(&self.cycle_counter.to_le_bytes());
    }

    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let b = &blob[offset..];
        let ime = match b[CPU_REGS_SIZE] {
            1 => ImeState::Pending,
            2 => ImeState::Enabled,
            _ => ImeState::Disabled,
        };
        let state = CpuState {
            a: b[0],
            b: b[1],
            c: b[2],
            d: b[3],
            e: b[4],
            h: b[5],
            l: b[6],
            f: Flags::from_bits_truncate(b[7]),
            sp: u16::from_le_bytes([b[8], b[9]]),
            pc: u16::from_le_bytes([b[10], b[11]]),
            ime,
            halted: b[CPU_REGS_SIZE + IME_SIZE] != 0,
            cycle_counter: u64::from_le_bytes(
                b[CPU_REGS_SIZE + IME_SIZE + HALTED_SIZE
                    ..CPU_REGS_SIZE + IME_SIZE + HALTED_SIZE + CYCLE_COUNTER_SIZE]
                    .try_into()
                    .unwrap(),
            ),
        };
        (state, CPU_STATE_SIZE)
    }

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

/// Timer peripheral state.
#[derive(Debug, Clone, Copy)]
pub struct TimerState {
    pub internal_counter: u16,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
}

impl TimerState {
    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.internal_counter.to_le_bytes());
        out.push(self.tima);
        out.push(self.tma);
        out.push(self.tac);
    }

    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let state = TimerState {
            internal_counter: u16::from_le_bytes([blob[offset], blob[offset + 1]]),
            tima: blob[offset + 2],
            tma: blob[offset + 3],
            tac: blob[offset + 4],
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
    pub lcdc: u8,
    pub stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub lyc: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,
}

impl PpuState {
    pub fn serialize(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.dot.to_le_bytes());
        out.push(self.ly);
        out.push(self.mode as u8);
        out.push(self.window_line_counter);
        out.push(self.lcdc);
        out.push(self.stat);
        out.push(self.scy);
        out.push(self.scx);
        out.push(self.lyc);
        out.push(self.bgp);
        out.push(self.obp0);
        out.push(self.obp1);
        out.push(self.wy);
        out.push(self.wx);
    }

    fn parse(blob: &[u8], offset: usize) -> (Self, usize) {
        let b = &blob[offset..];
        let state = PpuState {
            dot: u16::from_le_bytes([b[0], b[1]]),
            ly: b[2],
            mode: match b[3] {
                0 => PpuMode::HBlank,
                1 => PpuMode::VBlank,
                2 => PpuMode::OamScan,
                _ => PpuMode::PixelTransfer,
            },
            window_line_counter: b[4],
            lcdc: b[5],
            stat: b[6],
            scy: b[7],
            scx: b[8],
            lyc: b[9],
            bgp: b[10],
            obp0: b[11],
            obp1: b[12],
            wy: b[13],
            wx: b[14],
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
            upper_bits: b[1] & 0x03,
            ram_mode: b[2] != 0,
            ram_enabled: b[3] != 0,
        };
        (state, MBC_SIZE_V1_LEGACY)
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
    version: u16,

    pub cpu: CpuState,
    pub timer: TimerState,
    pub ppu: PpuState,
    mbc: Option<MbcState>,
    mbc_payload_range: Option<Range<usize>>,

    io_range: Range<usize>,
    ie_offset: usize,
    wram_range: Range<usize>,
    hram_range: Range<usize>,
    vram_range: Range<usize>,
    oam_range: Range<usize>,
    cart_ram_range: Option<Range<usize>>,
}

impl SaveState {
    /// Serialize emulator state into the default RBSS format.
    ///
    /// Called by `Sm83::save_state` which constructs the typed state structs
    /// from its own fields and passes them here. This function owns the format.
    pub fn serialize(
        cpu: CpuState,
        timer: TimerState,
        ppu: PpuState,
        memory: &GameBoyMemory,
    ) -> Vec<u8> {
        Self::serialize_v2(cpu, timer, ppu, memory)
    }

    /// Serialize emulator state into an RBSS v1 blob.
    pub fn serialize_v1(
        cpu: CpuState,
        timer: TimerState,
        ppu: PpuState,
        memory: &GameBoyMemory,
    ) -> Vec<u8> {
        let cart_ram_len = memory.external_ram().map_or(0, |r| r.len());
        // v1 tail: MBC state (≤ 18 bytes) + u16 RAM length prefix + cart RAM
        let capacity = MIN_BLOB_SIZE + 18 + 2 + cart_ram_len;
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION_V1.to_le_bytes());
        cpu.serialize(&mut out);
        timer.serialize(&mut out);
        ppu.serialize(&mut out);
        memory.save_state_v1(&mut out);
        out
    }

    /// Serialize emulator state into an RBSS v2 blob.
    pub fn serialize_v2(
        cpu: CpuState,
        timer: TimerState,
        ppu: PpuState,
        memory: &GameBoyMemory,
    ) -> Vec<u8> {
        // Pre-size to avoid realloc: fixed fields + worst-case MBC section
        // (MBC3+RTC = 18 bytes + 8-byte header) + cart RAM section if present.
        let cart_ram_len = memory.external_ram().map_or(0, |r| r.len());
        let capacity = MIN_BLOB_SIZE
            + SECTION_HEADER_SIZE + 18
            + if cart_ram_len > 0 { SECTION_HEADER_SIZE + cart_ram_len } else { 0 };
        let mut out = Vec::with_capacity(capacity);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION_V2.to_le_bytes());
        cpu.serialize(&mut out);
        timer.serialize(&mut out);
        ppu.serialize(&mut out);
        memory.save_state_v2(&mut out);
        out
    }

    /// Parse and validate a raw RBSS blob.
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
        if version != VERSION_V1 && version != VERSION_V2 {
            return Err("unsupported save state version");
        }

        let mut cur = HEADER_SIZE;

        let (cpu, n) = CpuState::parse(&blob, cur);
        cur += n;
        let (timer, n) = TimerState::parse(&blob, cur);
        cur += n;
        let (ppu, n) = PpuState::parse(&blob, cur);
        cur += n;

        let io_range = cur..cur + IO_REGS_SIZE;
        cur += IO_REGS_SIZE;
        let ie_offset = cur;
        cur += IE_SIZE;
        let wram_range = cur..cur + WRAM_SIZE;
        cur += WRAM_SIZE;
        let hram_range = cur..cur + HRAM_SIZE;
        cur += HRAM_SIZE;
        let vram_range = cur..cur + VRAM_SIZE;
        cur += VRAM_SIZE;
        let oam_range = cur..cur + OAM_SIZE;
        cur += OAM_SIZE;

        let (mbc_payload_range, cart_ram_range) = match version {
            VERSION_V1 => parse_v1_tail(&blob, cur)?,
            VERSION_V2 => parse_v2_sections(&blob, cur)?,
            _ => unreachable!(),
        };

        let mbc = mbc_payload_range.as_ref().and_then(|range| {
            if range.len() >= MBC_SIZE_V1_LEGACY {
                Some(MbcState::parse(&blob, range.start).0)
            } else {
                None
            }
        });

        Ok(SaveState {
            blob,
            version,
            cpu,
            timer,
            ppu,
            mbc,
            mbc_payload_range,
            io_range,
            ie_offset,
            wram_range,
            hram_range,
            vram_range,
            oam_range,
            cart_ram_range,
        })
    }

    // ── Accessors ─────────────────────────────────────────────────────────────

    pub fn mbc(&self) -> Option<&MbcState> {
        self.mbc.as_ref()
    }

    pub fn format_version(&self) -> u16 {
        self.version
    }

    pub fn mbc_payload(&self) -> Option<&[u8]> {
        self.mbc_payload_range
            .as_ref()
            .map(|r| &self.blob[r.clone()])
    }

    pub fn io_registers(&self) -> &[u8] {
        &self.blob[self.io_range.clone()]
    }
    pub fn ie(&self) -> u8 {
        self.blob[self.ie_offset]
    }
    pub fn wram(&self) -> &[u8] {
        &self.blob[self.wram_range.clone()]
    }
    pub fn hram(&self) -> &[u8] {
        &self.blob[self.hram_range.clone()]
    }
    pub fn vram(&self) -> &[u8] {
        &self.blob[self.vram_range.clone()]
    }
    pub fn oam(&self) -> &[u8] {
        &self.blob[self.oam_range.clone()]
    }
    pub fn cart_ram(&self) -> Option<&[u8]> {
        self.cart_ram_range.as_ref().map(|r| &self.blob[r.clone()])
    }
}

fn parse_v1_tail(
    blob: &[u8],
    tail_start: usize,
) -> Result<(Option<Range<usize>>, Option<Range<usize>>), &'static str> {
    if tail_start == blob.len() {
        return Ok((None, None));
    }

    for &mbc_len in &[MBC_SIZE_V1_MBC3_RTC, MBC_SIZE_V1_LEGACY, 0] {
        let Some(len_offset) = tail_start.checked_add(mbc_len) else {
            continue;
        };
        if len_offset + CART_RAM_LEN_SIZE_V1 > blob.len() {
            continue;
        }

        let ram_len = u16::from_le_bytes([blob[len_offset], blob[len_offset + 1]]) as usize;
        let ram_start = len_offset + CART_RAM_LEN_SIZE_V1;
        let ram_end = ram_start + ram_len;
        if ram_end == blob.len() {
            let mbc = if mbc_len > 0 {
                Some(tail_start..tail_start + mbc_len)
            } else {
                None
            };
            let ram = if ram_len > 0 {
                Some(ram_start..ram_end)
            } else {
                None
            };
            return Ok((mbc, ram));
        }
    }

    Err("invalid v1 save state tail")
}

fn parse_v2_sections(
    blob: &[u8],
    mut cur: usize,
) -> Result<(Option<Range<usize>>, Option<Range<usize>>), &'static str> {
    let mut mbc_payload_range = None;
    let mut cart_ram_range = None;

    while cur < blob.len() {
        if cur + SECTION_HEADER_SIZE > blob.len() {
            return Err("truncated v2 save state section header");
        }

        let tag = &blob[cur..cur + 4];
        let len = u32::from_le_bytes(
            blob[cur + 4..cur + 8]
                .try_into()
                .map_err(|_| "invalid v2 save state section length")?,
        ) as usize;
        cur += SECTION_HEADER_SIZE;

        let end = cur
            .checked_add(len)
            .ok_or("v2 save state section length overflow")?;
        if end > blob.len() {
            return Err("truncated v2 save state section payload");
        }

        if tag == SECTION_MBC {
            if len > 0 {
                mbc_payload_range = Some(cur..end);
            }
        } else if tag == SECTION_CART_RAM && len > 0 {
            cart_ram_range = Some(cur..end);
        }

        cur = end;
    }

    Ok((mbc_payload_range, cart_ram_range))
}

pub(crate) fn write_v2_section(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(tag);
    out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    out.extend_from_slice(payload);
}

pub(crate) fn write_mbc_section(out: &mut Vec<u8>, payload: &[u8]) {
    if !payload.is_empty() {
        write_v2_section(out, SECTION_MBC, payload);
    }
}

pub(crate) fn write_cart_ram_section(out: &mut Vec<u8>, payload: &[u8]) {
    if !payload.is_empty() {
        write_v2_section(out, SECTION_CART_RAM, payload);
    }
}
