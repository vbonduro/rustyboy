//! Crash serviceability layer.
//!
//! # Architecture
//!
//! Crash capture is split into two phases so the fault handler never touches
//! flash (avoiding XIP contention with core 1):
//!
//! 1. **Fault handler** (HardFault / panic): writes a 32-byte [`CrashSnap`] to
//!    the RP2350's Watchdog scratch registers (MMIO, instant, no contention) and
//!    a 32-byte GB-state blob to the POWMAN scratch registers, then calls
//!    `sys_reset()`.
//!
//! 2. **Boot** (`crash::storage::check_and_commit`): detects the magic sentinel
//!    in scratch registers, reconstructs a full 128-byte [`CrashRecord`], and
//!    writes it to the crash log sector in internal flash.  Scratch registers are
//!    cleared afterward so only genuine crashes trigger a commit.
//!
//! # Non-goals
//!
//! Intentional watchdog resets (e.g. ROM switch via `restart_after_rom_switch`)
//! never write the magic sentinel, so they are never confused with crashes.

// Re-export sub-modules.
pub mod storage;

#[cfg(target_arch = "arm")]
pub mod handler;

// ---------------------------------------------------------------------------
// Build-time constants (git hash, firmware version).
// ---------------------------------------------------------------------------
include!(concat!(env!("OUT_DIR"), "/crash_build_info.rs"));

// ---------------------------------------------------------------------------
// Protocol constants.
// ---------------------------------------------------------------------------

/// Sentinel written to `WATCHDOG.scratch0` by the fault handler.
/// Any other value means "no crash pending".
pub const CRASH_MAGIC: u32 = 0xCF_4A_53_11;

/// 4-byte magic that starts every [`CrashRecord`] on flash.
pub const RECORD_MAGIC: [u8; 4] = *b"RCRP";

/// 4-byte magic that starts the sector header on flash.
pub const SECTOR_MAGIC: [u8; 4] = *b"RCLG";

/// Fixed size of one crash record in bytes.
pub const RECORD_SIZE: usize = 128;

/// Fixed size of the sector header in bytes (occupies slot 0).
pub const SECTOR_HEADER_SIZE: usize = RECORD_SIZE;

/// Number of crash records that fit in one erase sector after the header.
pub const MAX_RECORDS_PER_SECTOR: usize = 31; // (4096 / 128) - 1

// ---------------------------------------------------------------------------
// Crash kind and flags.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum CrashKind {
    HardFault = 0,
    Panic = 1,
    /// The hardware watchdog timer expired before the firmware fed it.
    /// No ARM exception frame is available — the CPU was reset by hardware,
    /// not by the fault handler.
    WatchdogTimeout = 2,
    Unknown = 0xFF,
}

impl CrashKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::HardFault,
            1 => Self::Panic,
            2 => Self::WatchdogTimeout,
            _ => Self::Unknown,
        }
    }
}

/// Bit flags describing which fields in a crash record are populated.
pub mod flags {
    /// ARM exception frame (pc, lr, cfsr, …) is valid.
    pub const HAS_ARM_REGS: u8 = 0x01;
    /// GB CPU registers and cycle counter are valid.
    pub const HAS_GB_STATE: u8 = 0x02;
    /// ROM identity (id prefix, bank) is valid.
    pub const HAS_ROM_INFO: u8 = 0x04;
    /// Panic file / line is valid.
    pub const HAS_PANIC_LOC: u8 = 0x08;
}

// ---------------------------------------------------------------------------
// CrashRecord — 128-byte on-flash crash record.
// ---------------------------------------------------------------------------
//
// Byte layout (all multi-byte fields little-endian):
//
//   [0..4]    magic              b"RCRP"
//   [4]       schema_ver         = 1
//   [5]       crash_kind         CrashKind as u8
//   [6]       flags              flags::* bitmask
//   [7]       slot_seq           slot index within this erase cycle (0-30)
//   [8..11]   fw_version         [major, minor, patch]
//   [11]      _pad0
//   [12..16]  git_hash           first 4 bytes of git SHA as u32 LE
//   [16..20]  arm_pc             program counter at fault
//   [20..24]  arm_lr             link register at fault
//   [24..28]  arm_cfsr           Configurable Fault Status Register
//   [28..32]  arm_hfsr           HardFault Status Register
//   [32..36]  arm_fault_addr     MMFAR or BFAR (whichever is valid)
//   [36..40]  _pad1
//   [40..44]  rom_id_prefix      first 4 bytes of ROM SHA-256
//   [44..46]  rom_bank           currently mapped ROM bank
//   [46]      ram_bank
//   [47]      _pad2
//   [48]      gb_a               GB accumulator
//   [49]      gb_f               GB flags byte
//   [50]      gb_b
//   [51]      gb_c
//   [52]      gb_d
//   [53]      gb_e
//   [54]      gb_h
//   [55]      gb_l
//   [56..58]  gb_sp              GB stack pointer
//   [58..60]  gb_pc              GB program counter (where in the ROM)
//   [60..64]  gb_cycle_lo        lower 32 bits of cycle counter
//   [64]      ppu_ly             current scanline
//   [65]      ppu_lcdc           LCD control register (0 if unavailable)
//   [66]      ppu_stat           LCD status register
//   [67]      _pad3
//   [68..80]  panic_loc          null-terminated filename (last segment, ≤11 chars)
//   [80..82]  panic_line         source line number
//   [82..120] _reserved          zero-filled
//   [120..124] crc32             CRC32 of bytes [0..120]
//   [124..128] _pad4
//
// Total: 128 bytes.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CrashRecord {
    pub schema_ver: u8,
    pub crash_kind: u8,
    pub flags: u8,
    pub slot_seq: u8,
    pub fw_version: [u8; 3],
    pub git_hash: u32,
    pub arm_pc: u32,
    pub arm_lr: u32,
    pub arm_cfsr: u32,
    pub arm_hfsr: u32,
    pub arm_fault_addr: u32,
    pub rom_id_prefix: [u8; 4],
    pub rom_bank: u16,
    pub ram_bank: u8,
    pub gb_a: u8,
    pub gb_f: u8,
    pub gb_b: u8,
    pub gb_c: u8,
    pub gb_d: u8,
    pub gb_e: u8,
    pub gb_h: u8,
    pub gb_l: u8,
    pub gb_sp: u16,
    pub gb_pc: u16,
    pub gb_cycle_lo: u32,
    pub ppu_ly: u8,
    pub ppu_lcdc: u8,
    pub ppu_stat: u8,
    /// Last path segment of the panic source file, null-terminated, ≤ 11 chars.
    pub panic_loc: [u8; 12],
    pub panic_line: u16,
}

/// Opaque 128-byte wire representation.
pub type RecordBytes = [u8; RECORD_SIZE];

impl CrashRecord {
    /// Serialise to the 128-byte on-flash layout, computing and embedding CRC32.
    pub fn to_bytes(&self) -> RecordBytes {
        let mut buf = [0u8; RECORD_SIZE];
        buf[0..4].copy_from_slice(&RECORD_MAGIC);
        buf[4] = self.schema_ver;
        buf[5] = self.crash_kind;
        buf[6] = self.flags;
        buf[7] = self.slot_seq;
        buf[8..11].copy_from_slice(&self.fw_version);
        // buf[11] = 0 (pad)
        buf[12..16].copy_from_slice(&self.git_hash.to_le_bytes());
        buf[16..20].copy_from_slice(&self.arm_pc.to_le_bytes());
        buf[20..24].copy_from_slice(&self.arm_lr.to_le_bytes());
        buf[24..28].copy_from_slice(&self.arm_cfsr.to_le_bytes());
        buf[28..32].copy_from_slice(&self.arm_hfsr.to_le_bytes());
        buf[32..36].copy_from_slice(&self.arm_fault_addr.to_le_bytes());
        // buf[36..40] = 0 (pad)
        buf[40..44].copy_from_slice(&self.rom_id_prefix);
        buf[44..46].copy_from_slice(&self.rom_bank.to_le_bytes());
        buf[46] = self.ram_bank;
        // buf[47] = 0 (pad)
        buf[48] = self.gb_a;
        buf[49] = self.gb_f;
        buf[50] = self.gb_b;
        buf[51] = self.gb_c;
        buf[52] = self.gb_d;
        buf[53] = self.gb_e;
        buf[54] = self.gb_h;
        buf[55] = self.gb_l;
        buf[56..58].copy_from_slice(&self.gb_sp.to_le_bytes());
        buf[58..60].copy_from_slice(&self.gb_pc.to_le_bytes());
        buf[60..64].copy_from_slice(&self.gb_cycle_lo.to_le_bytes());
        buf[64] = self.ppu_ly;
        buf[65] = self.ppu_lcdc;
        buf[66] = self.ppu_stat;
        // buf[67] = 0 (pad)
        buf[68..80].copy_from_slice(&self.panic_loc);
        buf[80..82].copy_from_slice(&self.panic_line.to_le_bytes());
        // buf[82..120] = 0 (reserved)
        // buf[124..128] = 0 (pad)

        let checksum = crc32(&buf[..120]);
        buf[120..124].copy_from_slice(&checksum.to_le_bytes());
        buf
    }

    /// Deserialise from a 128-byte buffer.
    ///
    /// Returns `Err` if the magic is wrong or the CRC32 does not match.
    pub fn from_bytes(buf: &RecordBytes) -> Result<Self, CrashDecodeError> {
        if buf[0..4] != RECORD_MAGIC {
            return Err(CrashDecodeError::BadMagic);
        }
        let stored_crc = u32::from_le_bytes(buf[120..124].try_into().unwrap());
        let computed_crc = crc32(&buf[..120]);
        if stored_crc != computed_crc {
            return Err(CrashDecodeError::CrcMismatch {
                stored: stored_crc,
                computed: computed_crc,
            });
        }
        Ok(Self {
            schema_ver: buf[4],
            crash_kind: buf[5],
            flags: buf[6],
            slot_seq: buf[7],
            fw_version: buf[8..11].try_into().unwrap(),
            git_hash: u32::from_le_bytes(buf[12..16].try_into().unwrap()),
            arm_pc: u32::from_le_bytes(buf[16..20].try_into().unwrap()),
            arm_lr: u32::from_le_bytes(buf[20..24].try_into().unwrap()),
            arm_cfsr: u32::from_le_bytes(buf[24..28].try_into().unwrap()),
            arm_hfsr: u32::from_le_bytes(buf[28..32].try_into().unwrap()),
            arm_fault_addr: u32::from_le_bytes(buf[32..36].try_into().unwrap()),
            rom_id_prefix: buf[40..44].try_into().unwrap(),
            rom_bank: u16::from_le_bytes(buf[44..46].try_into().unwrap()),
            ram_bank: buf[46],
            gb_a: buf[48],
            gb_f: buf[49],
            gb_b: buf[50],
            gb_c: buf[51],
            gb_d: buf[52],
            gb_e: buf[53],
            gb_h: buf[54],
            gb_l: buf[55],
            gb_sp: u16::from_le_bytes(buf[56..58].try_into().unwrap()),
            gb_pc: u16::from_le_bytes(buf[58..60].try_into().unwrap()),
            gb_cycle_lo: u32::from_le_bytes(buf[60..64].try_into().unwrap()),
            ppu_ly: buf[64],
            ppu_lcdc: buf[65],
            ppu_stat: buf[66],
            panic_loc: buf[68..80].try_into().unwrap(),
            panic_line: u16::from_le_bytes(buf[80..82].try_into().unwrap()),
        })
    }

    /// Human-readable crash kind.
    pub fn crash_kind_name(&self) -> &'static str {
        match CrashKind::from_u8(self.crash_kind) {
            CrashKind::HardFault => "HardFault",
            CrashKind::Panic => "Panic",
            CrashKind::WatchdogTimeout => "WatchdogTimeout",
            CrashKind::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashDecodeError {
    BadMagic,
    CrcMismatch { stored: u32, computed: u32 },
}

// ---------------------------------------------------------------------------
// SectorHeader — 128-byte header at byte 0 of the crash log flash sector.
// ---------------------------------------------------------------------------
//
//   [0..4]   magic        b"RCLG"
//   [4..8]   erase_count  how many times this sector has been erased
//   [8]      next_slot    next free record slot (0-30); 0xFF = full
//   [9..128] _reserved    zeros

pub const SECTOR_FULL: u8 = 0xFF;

// ---------------------------------------------------------------------------
// Slot discovery (pure, testable without hardware)
// ---------------------------------------------------------------------------

/// Return the index of the first slot whose 4-byte magic does not equal
/// `RECORD_MAGIC`, or `None` if every slot is occupied.
///
/// The caller reads the leading 4 bytes of each record slot from flash and
/// passes them here.  Keeping this logic pure (no I/O) lets it be unit-tested
/// on the host without any embedded hardware or flash driver.
pub fn find_next_empty_slot(slot_magics: &[[u8; 4]; MAX_RECORDS_PER_SECTOR]) -> Option<usize> {
    slot_magics.iter().position(|m| m != &RECORD_MAGIC)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorHeader {
    pub erase_count: u32,
    /// Next free record slot index (0-30).  `SECTOR_FULL` when no room remains.
    pub next_slot: u8,
}

impl SectorHeader {
    /// Fresh header for a newly erased sector.
    pub fn fresh(erase_count: u32) -> Self {
        Self { erase_count, next_slot: 0 }
    }

    pub fn to_bytes(&self) -> RecordBytes {
        let mut buf = [0u8; RECORD_SIZE];
        buf[0..4].copy_from_slice(&SECTOR_MAGIC);
        buf[4..8].copy_from_slice(&self.erase_count.to_le_bytes());
        buf[8] = self.next_slot;
        buf
    }

    pub fn from_bytes(buf: &RecordBytes) -> Result<Self, SectorDecodeError> {
        if buf[0..4] != SECTOR_MAGIC {
            return Err(SectorDecodeError::BadMagic);
        }
        Ok(Self {
            erase_count: u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            next_slot: buf[8],
        })
    }

    pub fn is_full(&self) -> bool {
        self.next_slot as usize >= MAX_RECORDS_PER_SECTOR || self.next_slot == SECTOR_FULL
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectorDecodeError {
    BadMagic,
}

// ---------------------------------------------------------------------------
// CrashContext global — updated by the emulator loop (core 0), read-only by
// the fault handler.  All fields are AtomicU32 for lock-free cross-core access.
//
// `valid` is written LAST with Release ordering; all other stores use Relaxed.
// The fault handler reads `valid` with Acquire, then reads other fields with
// Relaxed.  If `valid == 0` the fault handler leaves GB fields zeroed.
// ---------------------------------------------------------------------------

#[cfg(target_arch = "arm")]
use core::sync::atomic::{AtomicU32, Ordering};

#[cfg(target_arch = "arm")]
pub struct CrashContext {
    /// Non-zero when a full snapshot has been committed at least once.
    valid: AtomicU32,
    /// First 4 bytes of the ROM SHA-256 hash packed as LE u32.
    rom_id_prefix: AtomicU32,
    /// [rom_bank:16 | ram_bank:8 | _:8]
    rom_bank_info: AtomicU32,
    /// [gb_a:8 | gb_f:8 | gb_b:8 | gb_c:8]
    gb_af_bc: AtomicU32,
    /// [gb_d:8 | gb_e:8 | gb_h:8 | gb_l:8]
    gb_de_hl: AtomicU32,
    /// [gb_sp:16 | gb_pc:16]
    gb_sp_pc: AtomicU32,
    /// Lower 32 bits of the u64 cycle counter.
    gb_cycle_lo: AtomicU32,
    /// [ppu_ly:8 | ppu_stat:8 | _:16]
    ppu_ly_stat: AtomicU32,
}

#[cfg(target_arch = "arm")]
impl CrashContext {
    pub const fn new() -> Self {
        Self {
            valid: AtomicU32::new(0),
            rom_id_prefix: AtomicU32::new(0),
            rom_bank_info: AtomicU32::new(0),
            gb_af_bc: AtomicU32::new(0),
            gb_de_hl: AtomicU32::new(0),
            gb_sp_pc: AtomicU32::new(0),
            gb_cycle_lo: AtomicU32::new(0),
            ppu_ly_stat: AtomicU32::new(0),
        }
    }

    /// Called once per frame from the emulator tick loop (core 0).
    pub fn update(
        &self,
        rom_id_prefix: [u8; 4],
        rom_bank: u16,
        ram_bank: u8,
        gb_a: u8,
        gb_f: u8,
        gb_b: u8,
        gb_c: u8,
        gb_d: u8,
        gb_e: u8,
        gb_h: u8,
        gb_l: u8,
        gb_sp: u16,
        gb_pc: u16,
        gb_cycle_lo: u32,
        ppu_ly: u8,
        ppu_stat: u8,
    ) {
        // Invalidate while updating so the fault handler can't read a torn snapshot.
        self.valid.store(0, Ordering::Release);

        self.rom_id_prefix
            .store(u32::from_le_bytes(rom_id_prefix), Ordering::Relaxed);
        self.rom_bank_info
            .store((rom_bank as u32) | ((ram_bank as u32) << 16), Ordering::Relaxed);
        self.gb_af_bc.store(
            (gb_a as u32)
                | ((gb_f as u32) << 8)
                | ((gb_b as u32) << 16)
                | ((gb_c as u32) << 24),
            Ordering::Relaxed,
        );
        self.gb_de_hl.store(
            (gb_d as u32)
                | ((gb_e as u32) << 8)
                | ((gb_h as u32) << 16)
                | ((gb_l as u32) << 24),
            Ordering::Relaxed,
        );
        self.gb_sp_pc
            .store((gb_sp as u32) | ((gb_pc as u32) << 16), Ordering::Relaxed);
        self.gb_cycle_lo.store(gb_cycle_lo, Ordering::Relaxed);
        self.ppu_ly_stat
            .store((ppu_ly as u32) | ((ppu_stat as u32) << 8), Ordering::Relaxed);

        // Publish: Release fence ensures all stores above are visible to any
        // subsequent Acquire load of `valid`.
        self.valid.store(1, Ordering::Release);
    }

    /// Read the snapshot atomically.  Returns `None` if no update has been
    /// committed (e.g. crash happened before the first game frame).
    pub fn snapshot(&self) -> Option<CrashContextSnapshot> {
        if self.valid.load(Ordering::Acquire) == 0 {
            return None;
        }
        let rom_id_raw = self.rom_id_prefix.load(Ordering::Relaxed);
        let bank_info = self.rom_bank_info.load(Ordering::Relaxed);
        let af_bc = self.gb_af_bc.load(Ordering::Relaxed);
        let de_hl = self.gb_de_hl.load(Ordering::Relaxed);
        let sp_pc = self.gb_sp_pc.load(Ordering::Relaxed);
        let cycle_lo = self.gb_cycle_lo.load(Ordering::Relaxed);
        let ly_stat = self.ppu_ly_stat.load(Ordering::Relaxed);

        Some(CrashContextSnapshot {
            rom_id_prefix: rom_id_raw.to_le_bytes(),
            rom_bank: (bank_info & 0xFFFF) as u16,
            ram_bank: ((bank_info >> 16) & 0xFF) as u8,
            gb_a: (af_bc & 0xFF) as u8,
            gb_f: ((af_bc >> 8) & 0xFF) as u8,
            gb_b: ((af_bc >> 16) & 0xFF) as u8,
            gb_c: ((af_bc >> 24) & 0xFF) as u8,
            gb_d: (de_hl & 0xFF) as u8,
            gb_e: ((de_hl >> 8) & 0xFF) as u8,
            gb_h: ((de_hl >> 16) & 0xFF) as u8,
            gb_l: ((de_hl >> 24) & 0xFF) as u8,
            gb_sp: (sp_pc & 0xFFFF) as u16,
            gb_pc: ((sp_pc >> 16) & 0xFFFF) as u16,
            gb_cycle_lo: cycle_lo,
            ppu_ly: (ly_stat & 0xFF) as u8,
            ppu_stat: ((ly_stat >> 8) & 0xFF) as u8,
        })
    }
}

#[cfg(target_arch = "arm")]
unsafe impl Sync for CrashContext {}

/// Snapshot of the emulator state captured by the fault handler.
#[derive(Debug, Clone, Copy)]
pub struct CrashContextSnapshot {
    pub rom_id_prefix: [u8; 4],
    pub rom_bank: u16,
    pub ram_bank: u8,
    pub gb_a: u8,
    pub gb_f: u8,
    pub gb_b: u8,
    pub gb_c: u8,
    pub gb_d: u8,
    pub gb_e: u8,
    pub gb_h: u8,
    pub gb_l: u8,
    pub gb_sp: u16,
    pub gb_pc: u16,
    pub gb_cycle_lo: u32,
    pub ppu_ly: u8,
    pub ppu_stat: u8,
}

/// The single global crash context, updated by the emulator loop.
#[cfg(target_arch = "arm")]
pub static CRASH_CONTEXT: CrashContext = CrashContext::new();

// ---------------------------------------------------------------------------
// CRC32 (IEEE 802.3 polynomial, table-free).
// ---------------------------------------------------------------------------

pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg(); // 0xFFFFFFFF or 0x00000000
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// Scratch register helper — maps WATCHDOG+POWMAN scratch to CrashRecord.
// Used by storage.rs at boot time.
// ---------------------------------------------------------------------------

/// Sixteen u32 values read from hardware scratch registers.
///
/// Index 0-7  → WATCHDOG.scratch0-7
/// Index 8-15 → POWMAN.scratch(0)-scratch(7)
pub struct ScratchRegs(pub [u32; 16]);

impl ScratchRegs {
    pub fn is_crash(&self) -> bool {
        self.0[0] == CRASH_MAGIC
    }

    /// Build a [`CrashRecord`] from the scratch blob + build-time constants.
    /// `slot_seq` is supplied by the storage layer.
    pub fn to_crash_record(&self, slot_seq: u8) -> CrashRecord {
        let wd = &self.0;
        // WATCHDOG scratch layout:
        //   [0] = CRASH_MAGIC
        //   [1] = arm_pc
        //   [2] = arm_lr
        //   [3] = arm_cfsr
        //   [4] = arm_hfsr
        //   [5] = arm_fault_addr
        //   [6] = [crash_kind:8 | flags:8 | ram_bank:8 | _:8]
        //   [7] = [rom_bank:16 | gb_pc:16]
        let crash_kind = (wd[6] & 0xFF) as u8;
        let flags_byte = ((wd[6] >> 8) & 0xFF) as u8;
        let ram_bank = ((wd[6] >> 16) & 0xFF) as u8;
        let rom_bank = (wd[7] >> 16) as u16;
        let gb_pc = (wd[7] & 0xFFFF) as u16;

        // POWMAN scratch layout (offset by 8):
        //   [8]  = rom_id_prefix
        //   [9]  = [gb_a:8 | gb_f:8 | gb_b:8 | gb_c:8]
        //   [10] = [gb_d:8 | gb_e:8 | gb_h:8 | gb_l:8]
        //   [11] = [gb_sp:16 | ppu_ly:8 | ppu_stat:8]
        //   [12] = gb_cycle_lo
        //   [13] = panic_line (u32, stored as u32)
        //   [14] = panic_file[0..4]
        //   [15] = panic_file[4..8]
        let rom_id_prefix = wd[8].to_le_bytes();
        let af_bc = wd[9];
        let de_hl = wd[10];
        let sp_ly = wd[11];
        let gb_cycle_lo = wd[12];
        let panic_line = (wd[13] & 0xFFFF) as u16;

        let mut panic_loc = [0u8; 12];
        panic_loc[0..4].copy_from_slice(&wd[14].to_le_bytes());
        panic_loc[4..8].copy_from_slice(&wd[15].to_le_bytes());

        CrashRecord {
            schema_ver: 1,
            crash_kind,
            flags: flags_byte,
            slot_seq,
            fw_version: FW_VERSION,
            git_hash: GIT_HASH_U32,
            arm_pc: wd[1],
            arm_lr: wd[2],
            arm_cfsr: wd[3],
            arm_hfsr: wd[4],
            arm_fault_addr: wd[5],
            rom_id_prefix,
            rom_bank,
            ram_bank,
            gb_a: (af_bc & 0xFF) as u8,
            gb_f: ((af_bc >> 8) & 0xFF) as u8,
            gb_b: ((af_bc >> 16) & 0xFF) as u8,
            gb_c: ((af_bc >> 24) & 0xFF) as u8,
            gb_d: (de_hl & 0xFF) as u8,
            gb_e: ((de_hl >> 8) & 0xFF) as u8,
            gb_h: ((de_hl >> 16) & 0xFF) as u8,
            gb_l: ((de_hl >> 24) & 0xFF) as u8,
            gb_sp: (sp_ly & 0xFFFF) as u16,
            gb_pc,
            gb_cycle_lo,
            ppu_ly: ((sp_ly >> 16) & 0xFF) as u8,
            ppu_lcdc: 0, // not captured in scratch
            ppu_stat: ((sp_ly >> 24) & 0xFF) as u8,
            panic_loc,
            panic_line,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(slot: u8) -> CrashRecord {
        CrashRecord {
            schema_ver: 1,
            crash_kind: CrashKind::HardFault as u8,
            flags: flags::HAS_ARM_REGS | flags::HAS_GB_STATE | flags::HAS_ROM_INFO,
            slot_seq: slot,
            fw_version: [0, 1, 0],
            git_hash: 0xDEAD_C0DE,
            arm_pc: 0x1002_34A8,
            arm_lr: 0x1002_2BC4,
            arm_cfsr: 0x0000_0200,
            arm_hfsr: 0x4000_0000,
            arm_fault_addr: 0xDEAD_BEEF,
            rom_id_prefix: [0xAB, 0xCD, 0xEF, 0x01],
            rom_bank: 7,
            ram_bank: 0,
            gb_a: 0x01,
            gb_f: 0xB0,
            gb_b: 0x00,
            gb_c: 0x13,
            gb_d: 0x00,
            gb_e: 0xD8,
            gb_h: 0x01,
            gb_l: 0x4D,
            gb_sp: 0xCFE0,
            gb_pc: 0x4A31,
            gb_cycle_lo: 12_345_678,
            ppu_ly: 88,
            ppu_lcdc: 0x91,
            ppu_stat: 0x83,
            panic_loc: *b"storage.r\0\0\0",
            panic_line: 0,
        }
    }

    fn sample_panic_record(slot: u8) -> CrashRecord {
        let mut r = sample_record(slot);
        r.crash_kind = CrashKind::Panic as u8;
        r.flags = flags::HAS_PANIC_LOC | flags::HAS_GB_STATE | flags::HAS_ROM_INFO;
        r.arm_pc = 0;
        r.arm_lr = 0;
        r.arm_cfsr = 0;
        r.arm_hfsr = 0;
        r.arm_fault_addr = 0;
        r.panic_loc = *b"storage.r\0\0\0";
        r.panic_line = 47;
        r
    }

    // -----------------------------------------------------------------------
    // CrashRecord encode / decode
    // -----------------------------------------------------------------------

    #[test]
    fn record_is_128_bytes() {
        // Wire format must be exactly RECORD_SIZE — regression guard.
        let r = sample_record(0);
        assert_eq!(r.to_bytes().len(), RECORD_SIZE);
    }

    #[test]
    fn record_roundtrip() {
        let original = sample_record(3);
        let bytes = original.to_bytes();
        let decoded = CrashRecord::from_bytes(&bytes).expect("decode should succeed");
        assert_eq!(original, decoded);
    }

    #[test]
    fn panic_record_roundtrip() {
        let original = sample_panic_record(1);
        let bytes = original.to_bytes();
        let decoded = CrashRecord::from_bytes(&bytes).expect("decode should succeed");
        assert_eq!(original, decoded);
    }

    #[test]
    fn crc32_embedded_correctly() {
        let r = sample_record(0);
        let bytes = r.to_bytes();
        // Bytes [120..124] hold the CRC32 of bytes [0..120].
        let stored = u32::from_le_bytes(bytes[120..124].try_into().unwrap());
        let computed = crc32(&bytes[..120]);
        assert_eq!(stored, computed, "CRC32 in serialised record does not match");
    }

    #[test]
    fn crc32_detects_corruption() {
        let r = sample_record(0);
        let mut bytes = r.to_bytes();
        bytes[42] ^= 0xFF; // corrupt one byte
        assert_eq!(
            CrashRecord::from_bytes(&bytes),
            Err(CrashDecodeError::CrcMismatch {
                stored: u32::from_le_bytes(bytes[120..124].try_into().unwrap()),
                computed: crc32(&bytes[..120]),
            })
        );
    }

    #[test]
    fn bad_magic_rejected() {
        let r = sample_record(0);
        let mut bytes = r.to_bytes();
        bytes[0] = 0xDE; // corrupt magic
        assert_eq!(CrashRecord::from_bytes(&bytes), Err(CrashDecodeError::BadMagic));
    }

    // -----------------------------------------------------------------------
    // SectorHeader encode / decode
    // -----------------------------------------------------------------------

    #[test]
    fn sector_header_roundtrip() {
        let h = SectorHeader { erase_count: 42, next_slot: 5 };
        let bytes = h.to_bytes();
        let decoded = SectorHeader::from_bytes(&bytes).expect("header decode");
        assert_eq!(h, decoded);
    }

    #[test]
    fn sector_header_full_detection() {
        assert!(!SectorHeader { erase_count: 0, next_slot: 0 }.is_full());
        assert!(!SectorHeader { erase_count: 0, next_slot: 30 }.is_full());
        assert!(SectorHeader { erase_count: 0, next_slot: 31 }.is_full());
        assert!(SectorHeader { erase_count: 0, next_slot: SECTOR_FULL }.is_full());
    }

    // -----------------------------------------------------------------------
    // ScratchRegs → CrashRecord
    // -----------------------------------------------------------------------

    #[test]
    fn scratch_regs_to_crash_record() {
        let mut regs = [0u32; 16];
        regs[0] = CRASH_MAGIC;
        regs[1] = 0x1002_34A8; // arm_pc
        regs[2] = 0x1002_2BC4; // arm_lr
        regs[3] = 0x0000_0200; // cfsr
        regs[4] = 0x4000_0000; // hfsr
        regs[5] = 0xDEAD_BEEF; // fault_addr
        regs[6] = (CrashKind::HardFault as u32)
            | (((flags::HAS_ARM_REGS | flags::HAS_GB_STATE | flags::HAS_ROM_INFO) as u32) << 8);
        regs[7] = ((7u32) << 16) | (0x4A31u32); // rom_bank=7, gb_pc=0x4A31
        regs[8] = u32::from_le_bytes([0xAB, 0xCD, 0xEF, 0x01]); // rom_id_prefix
        regs[9] = 0x01 | (0xB0 << 8) | (0x00 << 16) | (0x13 << 24); // a,f,b,c
        regs[10] = 0x00 | (0xD8 << 8) | (0x01 << 16) | (0x4D << 24); // d,e,h,l
        regs[11] = (0xCFE0u32) | (88u32 << 16) | (0x83u32 << 24); // sp, ly, stat
        regs[12] = 12_345_678; // cycle_lo
        // regs[13-15] = 0 (no panic)

        let snap = ScratchRegs(regs);
        assert!(snap.is_crash());
        let record = snap.to_crash_record(0);

        assert_eq!(record.arm_pc, 0x1002_34A8);
        assert_eq!(record.arm_lr, 0x1002_2BC4);
        assert_eq!(record.arm_cfsr, 0x0000_0200);
        assert_eq!(record.arm_fault_addr, 0xDEAD_BEEF);
        assert_eq!(record.rom_id_prefix, [0xAB, 0xCD, 0xEF, 0x01]);
        assert_eq!(record.rom_bank, 7);
        assert_eq!(record.gb_pc, 0x4A31);
        assert_eq!(record.gb_a, 0x01);
        assert_eq!(record.gb_f, 0xB0);
        assert_eq!(record.gb_sp, 0xCFE0);
        assert_eq!(record.ppu_ly, 88);
        assert_eq!(record.gb_cycle_lo, 12_345_678);
        assert_eq!(record.fw_version, FW_VERSION);
        assert_eq!(record.git_hash, GIT_HASH_U32);
    }

    #[test]
    fn no_crash_magic_not_a_crash() {
        let regs = [0u32; 16];
        assert!(!ScratchRegs(regs).is_crash());
    }

    // -----------------------------------------------------------------------
    // CRC32 algorithm
    // -----------------------------------------------------------------------

    #[test]
    fn crc32_known_value() {
        // CRC32 of b"123456789" is the canonical test vector.
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    // -----------------------------------------------------------------------
    // Test fixture generator.
    //
    // Writes `tools/fixtures/test_crash.bin` — a 4096-byte sector image
    // containing:
    //   - Sector header (slot 0, erase_count = 1)
    //   - Record 0: HardFault with well-known field values
    //   - Record 1: Panic  with well-known field values
    //
    // The Python decoder test reads this file and asserts on the same values.
    // Run with: cargo test -p rustyboy-pico2w write_test_fixture -- --nocapture
    // -----------------------------------------------------------------------

    #[test]
    fn write_test_fixture() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let fixture_path = std::path::PathBuf::from(manifest)
            .join("../../tools/fixtures/test_crash.bin");

        let mut sector = vec![0xFFu8; 4096];

        // Sector header
        let header = SectorHeader { erase_count: 1, next_slot: 2 };
        sector[..SECTOR_HEADER_SIZE].copy_from_slice(&header.to_bytes());

        // Record 0: HardFault
        let r0 = sample_record(0);
        let r0_bytes = r0.to_bytes();
        let off0 = SECTOR_HEADER_SIZE;
        sector[off0..off0 + RECORD_SIZE].copy_from_slice(&r0_bytes);

        // Record 1: Panic
        let r1 = sample_panic_record(1);
        let r1_bytes = r1.to_bytes();
        let off1 = SECTOR_HEADER_SIZE + RECORD_SIZE;
        sector[off1..off1 + RECORD_SIZE].copy_from_slice(&r1_bytes);

        // Create fixture directory if needed.
        if let Some(parent) = fixture_path.parent() {
            std::fs::create_dir_all(parent).expect("create fixtures dir");
        }
        std::fs::write(&fixture_path, &sector).expect("write fixture");

        // Verify we can round-trip the records back out.
        let buf: [u8; RECORD_SIZE] = sector[off0..off0 + RECORD_SIZE].try_into().unwrap();
        let decoded0 = CrashRecord::from_bytes(&buf).expect("round-trip record 0");
        assert_eq!(decoded0, r0);

        let buf: [u8; RECORD_SIZE] = sector[off1..off1 + RECORD_SIZE].try_into().unwrap();
        let decoded1 = CrashRecord::from_bytes(&buf).expect("round-trip record 1");
        assert_eq!(decoded1, r1);

        println!("Fixture written to {}", fixture_path.display());
    }

    // -----------------------------------------------------------------------
    // find_next_empty_slot
    // -----------------------------------------------------------------------

    #[test]
    fn find_next_empty_slot_all_empty() {
        // Erased flash = 0xFF bytes; every slot is free.
        let magics = [[0xFFu8; 4]; MAX_RECORDS_PER_SECTOR];
        assert_eq!(find_next_empty_slot(&magics), Some(0));
    }

    #[test]
    fn find_next_empty_slot_some_occupied() {
        let mut magics = [[0xFFu8; 4]; MAX_RECORDS_PER_SECTOR];
        magics[0] = RECORD_MAGIC;
        magics[1] = RECORD_MAGIC;
        assert_eq!(find_next_empty_slot(&magics), Some(2));
    }

    #[test]
    fn find_next_empty_slot_all_occupied() {
        // All 31 slots contain RCRP — sector is full.
        let magics = [RECORD_MAGIC; MAX_RECORDS_PER_SECTOR];
        assert_eq!(find_next_empty_slot(&magics), None);
    }

    // -----------------------------------------------------------------------
    // CrashKind
    // -----------------------------------------------------------------------

    #[test]
    fn crash_kind_from_u8_all_variants() {
        assert_eq!(CrashKind::from_u8(0), CrashKind::HardFault);
        assert_eq!(CrashKind::from_u8(1), CrashKind::Panic);
        assert_eq!(CrashKind::from_u8(2), CrashKind::WatchdogTimeout);
        assert_eq!(CrashKind::from_u8(3), CrashKind::Unknown);
        assert_eq!(CrashKind::from_u8(0xFF), CrashKind::Unknown);
    }

    #[test]
    fn crash_kind_name_strings() {
        let mut rec = sample_record(0);
        rec.crash_kind = 0;
        assert_eq!(rec.crash_kind_name(), "HardFault");
        rec.crash_kind = 1;
        assert_eq!(rec.crash_kind_name(), "Panic");
        rec.crash_kind = 2;
        assert_eq!(rec.crash_kind_name(), "WatchdogTimeout");
        rec.crash_kind = 0xFF;
        assert_eq!(rec.crash_kind_name(), "Unknown");
    }

    // -----------------------------------------------------------------------
    // SectorHeader
    // -----------------------------------------------------------------------

    #[test]
    fn sector_header_fresh() {
        let h = SectorHeader::fresh(7);
        assert_eq!(h.erase_count, 7);
        assert_eq!(h.next_slot, 0);
        assert!(!h.is_full());
    }

    #[test]
    fn sector_header_bad_magic_rejected() {
        let mut buf = [0u8; RECORD_SIZE];
        buf[0..4].copy_from_slice(b"NOPE");
        assert!(SectorHeader::from_bytes(&buf).is_err());
    }
}
