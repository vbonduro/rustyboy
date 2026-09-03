//! On-flash crash record: [`CrashKind`], [`CrashRecord`], flags, and decode errors.

use super::{crc32, RECORD_MAGIC, RECORD_SIZE};

// ---------------------------------------------------------------------------
// Protocol constants used by record layout.
// ---------------------------------------------------------------------------

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
    /// Boot-time reset cause captured from reset-status registers.
    ResetReason = 3,
    /// Core-0 multicore transport pointer triplet was overwritten.
    TransportSmash = 4,
    Unknown = 0xFF,
}

impl CrashKind {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::HardFault,
            1 => Self::Panic,
            2 => Self::WatchdogTimeout,
            3 => Self::ResetReason,
            4 => Self::TransportSmash,
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
    /// The faulting SP was below its stack limit at crash time — stack overflow.
    /// When set, `stack_headroom` holds the overflow depth in bytes.
    /// When clear, `stack_headroom` holds the remaining headroom in bytes.
    /// The relevant stack (and limit) is core 0's unless [`FAULT_ON_CORE1`] is set.
    pub const HAS_STACK_OVERFLOW: u8 = 0x10;
    /// The crash occurred on core 1 (the emulator worker). When set, the stack
    /// measurement (`stack_headroom` / [`HAS_STACK_OVERFLOW`]) is relative to
    /// core 1's dedicated stack limit rather than core 0's `_stack_end`.
    pub const FAULT_ON_CORE1: u8 = 0x20;
    /// HardFault/DebugMonitor diagnostic tail is present in `panic_loc`:
    /// `[68..72] = sp_before (faulted-thread SP; corrupted LR slot = sp_before-4)`,
    /// `[72..76] = stacked r12`.
    pub const HAS_HARDFAULT_EXTENDED_REGS: u8 = 0x40;
    /// Stack-protector panic: `arm_lr` contains the return address captured by
    /// `__stack_chk_fail`, identifying the guarded function whose canary failed.
    pub const HAS_STACK_CHK_FAIL_LR: u8 = 0x80;
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
//                                or stack-canary failure LR when
//                                HAS_STACK_CHK_FAIL_LR is set
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
//                                or HardFault/DebugMonitor diagnostic words when
//                                HAS_HARDFAULT_EXTENDED_REGS is set:
//                                [68..72] pre-handler r4, [72..76] stacked r12
//   [80..82]  panic_line         source line number
//   [82..84]  stack_headroom     bytes between MSP-at-fault and _stack_end:
//                                if HAS_STACK_OVERFLOW set → overflow depth,
//                                else → remaining stack headroom
//   [84..88]  dma_busy_mask      bitmask of DMA channels with BUSY=1 at crash time
//   [88..96]  dma_write_addrs    WRITE_ADDR for DMA channels 0-1 (2 × u32 LE)
//                                Channels 2-6 were dropped in schema v2: a live
//                                read of all 16 channels showed only ch0 (audio
//                                -> PIO TX FIFO) and ch1 (scale buf -> SPI1 DR)
//                                are ever non-zero. The reclaimed 20 bytes hold
//                                the stack snapshot below.
//   [96..120] stack_snapshot     6 × u32 LE: stack words starting at the
//                                PRE-FAULT SP (i.e. above the 8-word exception
//                                frame the fault pushed). For a wild-PC fault
//                                out of a `pop {rlist, pc}` this window spans
//                                the return-address slot and slot+8, which is
//                                what the +8 SP-drift test needs.
//                                Zero for schema v1 records.
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
    /// Bytes between MSP-at-fault and `_stack_end`.
    /// Meaning depends on `flags::HAS_STACK_OVERFLOW`:
    ///   set   → overflow depth (MSP was this many bytes below `_stack_end`)
    ///   clear → remaining headroom (MSP was this many bytes above `_stack_end`)
    pub stack_headroom: u16,
    /// Bitmask of DMA channels (0-7) that had CTRL_TRIG.BUSY=1 at crash time.
    /// Zero if DMA snapshot was not captured (schema v1 records).
    pub dma_busy_mask: u32,
    /// WRITE_ADDR register snapshot for DMA channels 0-1 at crash time.
    /// Zeros for schema v1 records or channels that were not snapshotted.
    /// Channels 2-6 were dropped in schema v2 — a live read of all 16 channels
    /// found them permanently zero, so they were 20 bytes of dead record.
    pub dma_write_addrs: [u32; 2],
    /// Stack words starting at the PRE-FAULT SP (above the exception frame).
    /// Spans the return-address slot and slot+8 for a `pop {rlist, pc}` fault,
    /// which is what testing the +8 SP-drift prediction requires.
    /// Zero for schema v1 records.
    pub stack_snapshot: [u32; 6],
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
        buf[82..84].copy_from_slice(&self.stack_headroom.to_le_bytes());
        buf[84..88].copy_from_slice(&self.dma_busy_mask.to_le_bytes());
        for (i, &w) in self.stack_snapshot.iter().enumerate() {
            let off = 96 + i * 4;
            buf[off..off + 4].copy_from_slice(&w.to_le_bytes());
        }
        for (i, &addr) in self.dma_write_addrs.iter().enumerate() {
            let off = 88 + i * 4;
            buf[off..off + 4].copy_from_slice(&addr.to_le_bytes());
        }
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
            stack_headroom: u16::from_le_bytes(buf[82..84].try_into().unwrap()),
            dma_busy_mask: u32::from_le_bytes(buf[84..88].try_into().unwrap()),
            dma_write_addrs: {
                let mut addrs = [0u32; 2];
                for (i, addr) in addrs.iter_mut().enumerate() {
                    let off = 88 + i * 4;
                    *addr = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
                }
                addrs
            },
            // Schema v1 wrote DMA channels 2-6 here; v2 reuses the space for the
            // stack snapshot. v1 records decode as garbage in this field, which
            // is why the reader must gate on `schema_ver >= 2`.
            stack_snapshot: {
                let mut w = [0u32; 6];
                if buf[4] >= 2 {
                    for (i, word) in w.iter_mut().enumerate() {
                        let off = 96 + i * 4;
                        *word = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
                    }
                }
                w
            },
        })
    }

    /// Human-readable crash kind.
    pub fn crash_kind_name(&self) -> &'static str {
        match CrashKind::from_u8(self.crash_kind) {
            CrashKind::HardFault => "HardFault",
            CrashKind::Panic => "Panic",
            CrashKind::WatchdogTimeout => "WatchdogTimeout",
            CrashKind::ResetReason => "ResetReason",
            CrashKind::TransportSmash => "TransportSmash",
            CrashKind::Unknown => "Unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashDecodeError {
    BadMagic,
    CrcMismatch { stored: u32, computed: u32 },
}
