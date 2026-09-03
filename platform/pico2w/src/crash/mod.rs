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
pub mod stack_snapshot;
pub mod storage;

#[cfg(target_arch = "arm")]
pub mod handler;

pub mod context;
pub mod record;
pub mod sector;

// ---------------------------------------------------------------------------
// Build-time constants (git hash, firmware version).
// ---------------------------------------------------------------------------
include!(concat!(env!("OUT_DIR"), "/crash_build_info.rs"));

// ---------------------------------------------------------------------------
// Protocol constants (shared across sub-modules).
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

// ---------------------------------------------------------------------------
// CRC32 (IEEE 802.3 polynomial, table-free) — shared by all sub-modules.
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
// Re-exports: keep all existing public paths working unchanged.
// ---------------------------------------------------------------------------

pub use record::{
    flags, CrashDecodeError, CrashKind, CrashRecord, RecordBytes, MAX_RECORDS_PER_SECTOR,
    SECTOR_HEADER_SIZE,
};

pub use sector::{find_next_empty_slot, SectorDecodeError, SectorHeader, SECTOR_FULL};

pub use context::{ScratchRegs, TransportSmashDiag, TransportSmashSnapshot, TRANSPORT_SMASH_DIAG};

#[cfg(target_arch = "arm")]
pub use context::{CrashContext, CrashContextSnapshot, CRASH_CONTEXT};

// ---------------------------------------------------------------------------
// Tests — kept in mod.rs because they test types across multiple sub-modules.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record(slot: u8) -> CrashRecord {
        CrashRecord {
            schema_ver: 2,
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
            stack_headroom: 0,
            ..Default::default()
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

    /// A v1 record's bytes [96..120] held DMA channels 2-6. Schema v2 reuses that
    /// range for the stack snapshot. A v1 record already on flash must therefore
    /// decode with an EMPTY snapshot, not with stale DMA addresses reinterpreted
    /// as stack words — this is the one compatibility invariant of the format
    /// change, and nothing else covers it.
    #[test]
    fn v1_record_decodes_with_empty_stack_snapshot() {
        let mut buf = sample_record(3).to_bytes();
        // Rewrite as schema v1 with plausible v1 payload in the reclaimed range.
        buf[4] = 1;
        for (i, b) in buf[96..120].iter_mut().enumerate() {
            *b = 0xA0u8.wrapping_add(i as u8);
        }
        let crc = crc32(&buf[..120]);
        buf[120..124].copy_from_slice(&crc.to_le_bytes());

        let rec = CrashRecord::from_bytes(&buf).expect("v1 record must still decode");
        assert_eq!(rec.schema_ver, 1);
        assert_eq!(
            rec.stack_snapshot, [0u32; 6],
            "v1 DMA bytes must not be reinterpreted as a stack snapshot"
        );
    }

    /// The stack snapshot must survive a round trip with distinct non-zero words.
    /// `sample_record` leaves it zeroed via `..Default::default()`, so without
    /// this an offset bug in `to_bytes`/`from_bytes` would be invisible.
    #[test]
    fn stack_snapshot_roundtrips() {
        let mut rec = sample_record(4);
        rec.stack_snapshot = [
            0x1111_1111,
            0x2222_2222,
            0x3333_3333,
            0x4444_4444,
            0x5555_5555,
            0x6666_6666,
        ];
        rec.dma_write_addrs = [0x5020_0010, 0x4008_8008];
        let back = CrashRecord::from_bytes(&rec.to_bytes()).expect("roundtrip");
        assert_eq!(back.stack_snapshot, rec.stack_snapshot);
        assert_eq!(back.dma_write_addrs, rec.dma_write_addrs);
    }

    /// Pin the ABSOLUTE byte offsets. `record_roundtrip` passes even if a field
    /// moved, because both sides move together — but this is a PERSISTED format
    /// read back from flash written by older firmware.
    #[test]
    fn persisted_field_offsets_are_stable() {
        let mut rec = sample_record(5);
        rec.dma_write_addrs = [0xAAAA_AAAA, 0xBBBB_BBBB];
        rec.stack_snapshot = [
            0xC0DE_0000,
            0xC0DE_0001,
            0xC0DE_0002,
            0xC0DE_0003,
            0xC0DE_0004,
            0xC0DE_0005,
        ];
        let b = rec.to_bytes();
        assert_eq!(
            &b[88..92],
            &0xAAAA_AAAAu32.to_le_bytes(),
            "dma ch0 @ [88..92]"
        );
        assert_eq!(
            &b[92..96],
            &0xBBBB_BBBBu32.to_le_bytes(),
            "dma ch1 @ [92..96]"
        );
        for i in 0..6 {
            let off = 96 + i * 4;
            assert_eq!(
                &b[off..off + 4],
                &rec.stack_snapshot[i].to_le_bytes(),
                "stack_snapshot[{i}] @ [{off}..]"
            );
        }
        assert_eq!(b[4], 2, "producers must stamp schema_ver = 2");
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
        assert_eq!(
            stored, computed,
            "CRC32 in serialised record does not match"
        );
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
        assert_eq!(
            CrashRecord::from_bytes(&bytes),
            Err(CrashDecodeError::BadMagic)
        );
    }

    // -----------------------------------------------------------------------
    // SectorHeader encode / decode
    // -----------------------------------------------------------------------

    #[test]
    fn sector_header_roundtrip() {
        let h = SectorHeader {
            erase_count: 42,
            next_slot: 5,
        };
        let bytes = h.to_bytes();
        let decoded = SectorHeader::from_bytes(&bytes).expect("header decode");
        assert_eq!(h, decoded);
    }

    #[test]
    fn sector_header_full_detection() {
        assert!(!SectorHeader {
            erase_count: 0,
            next_slot: 0
        }
        .is_full());
        assert!(!SectorHeader {
            erase_count: 0,
            next_slot: 30
        }
        .is_full());
        assert!(SectorHeader {
            erase_count: 0,
            next_slot: 31
        }
        .is_full());
        assert!(SectorHeader {
            erase_count: 0,
            next_slot: SECTOR_FULL
        }
        .is_full());
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
        let fixture_path =
            std::path::PathBuf::from(manifest).join("../../tools/fixtures/test_crash.bin");

        let mut sector = vec![0xFFu8; 4096];

        // Sector header
        let header = SectorHeader {
            erase_count: 1,
            next_slot: 2,
        };
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
    fn find_next_empty_slot_corrupt_treated_as_occupied() {
        // A slot with non-RCRP, non-0xFF bytes (e.g. from a failed flash write)
        // must be treated as occupied — not as an available write target.
        // Writing over a non-erased slot silently corrupts data on NOR flash.
        let mut magics = [RECORD_MAGIC; MAX_RECORDS_PER_SECTOR];
        // Replace the last slot with the "BCB@" pattern seen in real corrupt flash.
        magics[MAX_RECORDS_PER_SECTOR - 1] = [0x42, 0x43, 0x42, 0x40]; // BCB@
                                                                       // All 31 slots are either RCRP or corrupt — no erased slot available.
        assert_eq!(find_next_empty_slot(&magics), None);
    }

    #[test]
    fn find_next_empty_slot_ef_treated_as_occupied() {
        // 0xEF bytes are a partially-programmed state (NOR flash bit 4 = 0 instead
        // of the erased 1).  These must not be selected as a write target.
        let mut magics = [RECORD_MAGIC; MAX_RECORDS_PER_SECTOR];
        magics[MAX_RECORDS_PER_SECTOR - 1] = [0xEF, 0xEF, 0xEF, 0xEF];
        assert_eq!(find_next_empty_slot(&magics), None);
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
        assert_eq!(CrashKind::from_u8(3), CrashKind::ResetReason);
        assert_eq!(CrashKind::from_u8(4), CrashKind::TransportSmash);
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
        rec.crash_kind = 3;
        assert_eq!(rec.crash_kind_name(), "ResetReason");
        rec.crash_kind = 4;
        assert_eq!(rec.crash_kind_name(), "TransportSmash");
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
