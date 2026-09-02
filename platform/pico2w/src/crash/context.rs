//! Global crash context updated by the emulator loop and consumed by the fault handler.

use core::sync::atomic::{AtomicU32, Ordering};

use super::record::CrashRecord;
use super::{CRASH_MAGIC, FW_VERSION, GIT_HASH_U32};

// ---------------------------------------------------------------------------
// CrashContext global — updated by the emulator loop (core 0), read-only by
// the fault handler.  All fields are AtomicU32 for lock-free cross-core access.
//
// `valid` is written LAST with Release ordering; all other stores use Relaxed.
// The fault handler reads `valid` with Acquire, then reads other fields with
// Relaxed.  If `valid == 0` the fault handler leaves GB fields zeroed.
// ---------------------------------------------------------------------------

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
        self.rom_bank_info.store(
            (rom_bank as u32) | ((ram_bank as u32) << 16),
            Ordering::Relaxed,
        );
        self.gb_af_bc.store(
            (gb_a as u32) | ((gb_f as u32) << 8) | ((gb_b as u32) << 16) | ((gb_c as u32) << 24),
            Ordering::Relaxed,
        );
        self.gb_de_hl.store(
            (gb_d as u32) | ((gb_e as u32) << 8) | ((gb_h as u32) << 16) | ((gb_l as u32) << 24),
            Ordering::Relaxed,
        );
        self.gb_sp_pc
            .store((gb_sp as u32) | ((gb_pc as u32) << 16), Ordering::Relaxed);
        self.gb_cycle_lo.store(gb_cycle_lo, Ordering::Relaxed);
        self.ppu_ly_stat.store(
            (ppu_ly as u32) | ((ppu_stat as u32) << 8),
            Ordering::Relaxed,
        );

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
// TransportSmashDiag — cross-core transport pointer corruption diagnostic.
// ---------------------------------------------------------------------------

/// Durable diagnostic captured immediately before `report_transport_smash`
/// panics. The panic handler consumes this and stores the values in the ARM
/// fields of a `TransportSmash` record:
/// - `arm_pc` = `Core1Transport` base
/// - `arm_lr` = corrupted `command_tx` pointer
/// - `arm_cfsr` = corrupted `audio_rx` pointer
/// - `arm_hfsr` = corrupted `shared` pointer
/// - `arm_fault_addr` = first duplicate triplet found in SRAM, or 0
pub struct TransportSmashDiag {
    active: AtomicU32,
    base: AtomicU32,
    cmd: AtomicU32,
    aud: AtomicU32,
    shr: AtomicU32,
    source_triplet: AtomicU32,
}

impl TransportSmashDiag {
    pub const fn new() -> Self {
        Self {
            active: AtomicU32::new(0),
            base: AtomicU32::new(0),
            cmd: AtomicU32::new(0),
            aud: AtomicU32::new(0),
            shr: AtomicU32::new(0),
            source_triplet: AtomicU32::new(0),
        }
    }

    pub fn record(&self, base: usize, cmd: usize, aud: usize, shr: usize, source_triplet: usize) {
        self.active.store(0, Ordering::Release);
        self.base.store(base as u32, Ordering::Relaxed);
        self.cmd.store(cmd as u32, Ordering::Relaxed);
        self.aud.store(aud as u32, Ordering::Relaxed);
        self.shr.store(shr as u32, Ordering::Relaxed);
        self.source_triplet
            .store(source_triplet as u32, Ordering::Relaxed);
        self.active.store(1, Ordering::Release);
    }

    pub fn take(&self) -> Option<TransportSmashSnapshot> {
        if self.active.swap(0, Ordering::AcqRel) == 0 {
            return None;
        }
        Some(TransportSmashSnapshot {
            base: self.base.load(Ordering::Relaxed),
            cmd: self.cmd.load(Ordering::Relaxed),
            aud: self.aud.load(Ordering::Relaxed),
            shr: self.shr.load(Ordering::Relaxed),
            source_triplet: self.source_triplet.load(Ordering::Relaxed),
        })
    }
}

unsafe impl Sync for TransportSmashDiag {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportSmashSnapshot {
    pub base: u32,
    pub cmd: u32,
    pub aud: u32,
    pub shr: u32,
    pub source_triplet: u32,
}

pub static TRANSPORT_SMASH_DIAG: TransportSmashDiag = TransportSmashDiag::new();

// ---------------------------------------------------------------------------
// ScratchRegs helper — maps WATCHDOG+POWMAN scratch to CrashRecord.
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
        //   [13] = [stack_headroom:16 | panic_line_or_r1:16]
        //   [14] = panic_file[0..4]
        //   [15] = panic_file[4..8]
        let rom_id_prefix = wd[8].to_le_bytes();
        let af_bc = wd[9];
        let de_hl = wd[10];
        let sp_ly = wd[11];
        let gb_cycle_lo = wd[12];
        let panic_line = (wd[13] & 0xFFFF) as u16;
        let stack_headroom = (wd[13] >> 16) as u16;

        let mut panic_loc = [0u8; 12];
        panic_loc[0..4].copy_from_slice(&wd[14].to_le_bytes());
        panic_loc[4..8].copy_from_slice(&wd[15].to_le_bytes());

        CrashRecord {
            schema_ver: 2,
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
            stack_headroom,
            // DMA fields are injected after this call in check_and_commit,
            // once the .uninit DMA_CRASH_SNAPSHOT has been read.
            ..Default::default()
        }
    }
}
