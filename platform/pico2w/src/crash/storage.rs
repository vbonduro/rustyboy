//! Boot-time crash log storage.
//!
//! Call [`check_and_commit`] once early in `main`, after the flash driver is
//! initialised but before starting the emulator.  It is safe to call on every
//! boot — it is a no-op when no crash is pending.
//!
//! # Flash sector layout
//!
//! The last [`ERASE_SIZE`] (4 KiB) bytes of flash are reserved for the crash
//! log.  The sector is structured as 32 × 128-byte slots:
//!
//! ```text
//! Slot 0        → SectorHeader (magic, erase_count, next_slot)
//! Slots 1-31    → CrashRecord  (up to 31 records per erase cycle)
//! ```
//!
//! When all 31 record slots are full the sector is erased and the cycle
//! restarts.  Phase 2 (SD-card offload) will drain the log on each boot so
//! the sector rarely fills.

#[cfg(target_arch = "arm")]
use embassy_rp::flash::{Error as FlashError, ERASE_SIZE};
#[cfg(target_arch = "arm")]
use rp_pac::{POWMAN, WATCHDOG};

#[cfg(target_arch = "arm")]
use crate::flash_rom::OnboardFlash;

#[cfg(target_arch = "arm")]
use super::{
    find_next_empty_slot, CrashRecord, ScratchRegs, SectorDecodeError, SectorHeader,
    MAX_RECORDS_PER_SECTOR, RECORD_SIZE, SECTOR_HEADER_SIZE,
};

/// Byte offset from the start of flash to the crash log sector.
///
/// Re-exported here so callers only need to import `crash::storage`.
#[cfg(target_arch = "arm")]
pub use crate::flash_rom::CRASH_LOG_OFFSET;

// ---------------------------------------------------------------------------
// Boot-time entry point
// ---------------------------------------------------------------------------

/// Read the RP2350 scratch registers and, if a HardFault or panic is pending,
/// write a full [`CrashRecord`] to the crash log sector in flash.
///
/// Must be called from a single-core context (core 1 not yet started) so that
/// flash writes are safe without disabling XIP.
///
/// Returns `true` if a crash record was committed this boot.
#[cfg(target_arch = "arm")]
pub fn check_and_commit(flash: &mut OnboardFlash<'_>) -> bool {
    let snap = read_scratch_registers();
    if !snap.is_crash() {
        return false;
    }

    // Clear the magic immediately — even if the flash write fails we don't
    // want to loop on crash commits.
    clear_crash_magic();

    match write_record_to_flash(flash, |slot| {
        let mut rec = snap.to_crash_record(slot);
        unsafe {
            use crate::crash::handler::{DMA_CRASH_SNAPSHOT, DMA_SNAPSHOT_SENTINEL};
            if DMA_CRASH_SNAPSHOT[0] == DMA_SNAPSHOT_SENTINEL {
                rec.dma_busy_mask = DMA_CRASH_SNAPSHOT[1];
                for i in 0..7usize {
                    rec.dma_write_addrs[i] = DMA_CRASH_SNAPSHOT[2 + i];
                }
                // Log high channels (ch7-ch15) via defmt — not stored in flash record.
                // These are the channels WiFi/PIO DMA would use.
                let busy = DMA_CRASH_SNAPSHOT[1];
                for ch in 7u32..16u32 {
                    let write_addr = DMA_CRASH_SNAPSHOT[2 + ch as usize];
                    let is_busy = (busy >> ch) & 1 != 0;
                    // Only log channels that were busy or have non-zero write addresses
                    // (zero means unconfigured/idle).
                    if is_busy || write_addr != 0 {
                        defmt::warn!(
                            "crash: DMA ch{}: WRITE_ADDR={:#010x}  BUSY={}",
                            ch,
                            write_addr,
                            is_busy,
                        );
                    }
                }
                DMA_CRASH_SNAPSHOT[0] = 0; // consume — don't reuse on next boot
            }
        }
        rec
    }) {
        Ok(true) => {
            defmt::info!("crash: committed crash record to flash");
            true
        }
        Ok(false) => false, // skipped — sector full or duplicate
        Err(e) => {
            defmt::error!("crash: flash commit failed: {:?}", e);
            false
        }
    }
}

/// Check whether the last reset was a hardware watchdog timeout and, if so,
/// write a [`CrashRecord`] with kind [`CrashKind::WatchdogTimeout`] to flash.
///
/// The RP2350 `WATCHDOG.reason` register records the reset cause and persists
/// across watchdog resets.  It is cleared by:
/// - A power-on reset (POR).
/// - `SYSRESETREQ` — which the HardFault/panic handler calls via `sys_reset()`,
///   so the two paths are mutually exclusive: if `check_and_commit` already
///   committed a HardFault record, `reason.timer()` will be false.
///
/// After detecting the timeout this function writes 0 to the reason register
/// to prevent re-recording on the next boot.
///
/// Must be called from a single-core context, after flash is initialised but
/// before core 1 starts.
///
/// Returns `true` if a watchdog-timeout record was committed this boot.
#[cfg(target_arch = "arm")]
pub fn check_watchdog_reset(flash: &mut OnboardFlash<'_>) -> bool {
    check_reset_reason(flash)
}

/// Capture reset causes that do not flow through the panic/HardFault scratch
/// sentinel.
///
/// This records watchdog timer/force resets plus POWMAN causes that are useful
/// for the silent-reboot crash hunt. POR/debugger resets are logged but not
/// written to flash, so normal flashing and power cycling do not consume crash
/// slots. RUN-low resets are recorded during the hunt because repeated
/// user-observed splash/save-state reboots latched exactly that cause.
#[cfg(target_arch = "arm")]
pub fn check_reset_reason(flash: &mut OnboardFlash<'_>) -> bool {
    let snapshot = ResetReasonSnapshot::read();
    snapshot.log();
    clear_watchdog_reason();

    if !snapshot.should_record() {
        return false;
    }

    match write_record_to_flash(flash, |slot| snapshot.to_crash_record(slot)) {
        Ok(true) => {
            defmt::info!("crash: reset reason - committed record to flash");
            true
        }
        Ok(false) => false,
        Err(e) => {
            defmt::error!("crash: reset-reason flash commit failed: {:?}", e);
            false
        }
    }
}

#[cfg(target_arch = "arm")]
#[derive(Clone, Copy)]
struct ResetReasonSnapshot {
    watchdog_reason: u32,
    powman_chip_reset: u32,
    powman_current_pwrup: u32,
    powman_last_swcore_pwrup: u32,
    powman_intr: u32,
}

#[cfg(target_arch = "arm")]
impl ResetReasonSnapshot {
    const CHIP_HAD_BOR: u32 = 1 << 17;
    const CHIP_HAD_RUN_LOW: u32 = 1 << 18;
    const CHIP_HAD_WATCHDOG_RESET_POWMAN_ASYNC: u32 = 1 << 22;
    const CHIP_HAD_WATCHDOG_RESET_POWMAN: u32 = 1 << 23;
    const CHIP_HAD_WATCHDOG_RESET_SWCORE: u32 = 1 << 24;
    const CHIP_HAD_SWCORE_PD: u32 = 1 << 25;
    const CHIP_HAD_GLITCH_DETECT: u32 = 1 << 26;
    const CHIP_HAD_HZD_SYS_RESET_REQ: u32 = 1 << 27;
    const CHIP_HAD_WATCHDOG_RESET_RSM: u32 = 1 << 28;

    const RECORDABLE_CHIP_RESET_MASK: u32 = Self::CHIP_HAD_BOR
        | Self::CHIP_HAD_RUN_LOW
        | Self::CHIP_HAD_WATCHDOG_RESET_POWMAN_ASYNC
        | Self::CHIP_HAD_WATCHDOG_RESET_POWMAN
        | Self::CHIP_HAD_WATCHDOG_RESET_SWCORE
        | Self::CHIP_HAD_SWCORE_PD
        | Self::CHIP_HAD_GLITCH_DETECT
        | Self::CHIP_HAD_HZD_SYS_RESET_REQ
        | Self::CHIP_HAD_WATCHDOG_RESET_RSM;

    fn read() -> Self {
        let watchdog_reason = WATCHDOG.reason().read().0;
        let powman = POWMAN;
        Self {
            watchdog_reason,
            powman_chip_reset: powman.chip_reset().read().0,
            powman_current_pwrup: powman.current_pwrup_req().read().0,
            powman_last_swcore_pwrup: powman.last_swcore_pwrup().read().0,
            powman_intr: powman.intr().read().0,
        }
    }

    fn log(&self) {
        defmt::info!(
            "reset: watchdog_reason={=u32:#010x} powman_chip_reset={=u32:#010x} current_pwrup={=u32:#010x} last_swcore_pwrup={=u32:#010x} powman_intr={=u32:#010x}",
            self.watchdog_reason,
            self.powman_chip_reset,
            self.powman_current_pwrup,
            self.powman_last_swcore_pwrup,
            self.powman_intr,
        );
    }

    fn watchdog_timer(self) -> bool {
        self.watchdog_reason & 0x1 != 0
    }

    fn watchdog_force(self) -> bool {
        self.watchdog_reason & 0x2 != 0
    }

    fn has_recordable_chip_reset(self) -> bool {
        self.powman_chip_reset & Self::RECORDABLE_CHIP_RESET_MASK != 0
    }

    fn should_record(self) -> bool {
        self.watchdog_timer() || self.watchdog_force() || self.has_recordable_chip_reset()
    }

    fn to_crash_record(self, slot_seq: u8) -> CrashRecord {
        let mut record = CrashRecord {
            schema_ver: 1,
            crash_kind: if self.watchdog_timer() {
                super::CrashKind::WatchdogTimeout as u8
            } else {
                super::CrashKind::ResetReason as u8
            },
            flags: super::flags::HAS_ARM_REGS,
            slot_seq,
            fw_version: super::FW_VERSION,
            git_hash: super::GIT_HASH_U32,
            arm_pc: self.watchdog_reason,
            arm_lr: self.powman_chip_reset,
            arm_cfsr: self.powman_current_pwrup,
            arm_hfsr: self.powman_last_swcore_pwrup,
            arm_fault_addr: self.powman_intr,
            ..Default::default()
        };
        if self.watchdog_force() {
            record.crash_kind = super::CrashKind::ResetReason as u8;
        }
        record
    }
}

#[cfg(target_arch = "arm")]
#[allow(dead_code)]
fn check_legacy_watchdog_reset(flash: &mut OnboardFlash<'_>) -> bool {
    let reason = WATCHDOG.reason().read();
    if !reason.timer() {
        return false;
    }

    // Clear the reason register immediately so we don't re-record on the
    // next boot (the register persists across watchdog resets).
    clear_watchdog_reason();

    match write_record_to_flash(flash, |slot| build_watchdog_record(slot)) {
        Ok(true) => {
            defmt::info!("crash: watchdog timeout — committed record to flash");
            true
        }
        Ok(false) => false,
        Err(e) => {
            defmt::error!("crash: watchdog flash commit failed: {:?}", e);
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Record iteration (for future SD-card offload phase)
// ---------------------------------------------------------------------------

/// Read up to `buf.len()` crash records from the flash sector into `buf`.
///
/// Returns the number of valid records read.  Records with bad CRC are skipped.
/// Scans all 31 slots by RCRP magic — does not rely on `SectorHeader::next_slot`
/// which is unreliable after two or more writes (NOR flash AND-corruption).
#[cfg(target_arch = "arm")]
pub fn read_records(flash: &mut OnboardFlash<'_>, buf: &mut [CrashRecord]) -> usize {
    // Require a valid sector header before reading anything.
    if read_sector_header(flash).is_err() {
        return 0;
    }

    let mut found = 0;
    for slot in 0..MAX_RECORDS_PER_SECTOR {
        if found >= buf.len() {
            break;
        }
        if let Ok(rec) = read_record_at_slot(flash, slot) {
            buf[found] = rec;
            found += 1;
        }
    }
    found
}

/// Erase the crash log sector, resetting it for fresh use.
///
/// Call after all records have been offloaded to SD / uploaded.
#[cfg(target_arch = "arm")]
pub fn erase_log(flash: &mut OnboardFlash<'_>) -> Result<(), FlashError> {
    flash.blocking_erase(
        CRASH_LOG_OFFSET as u32,
        (CRASH_LOG_OFFSET + ERASE_SIZE) as u32,
    )
}

/// Returns `true` if the crash log sector contains at least one valid record.
///
/// Checks for a valid `RCLG` sector header first, then scans record slots for
/// `RCRP` magic.  Used at boot to decide whether to show the crash indicator
/// in the menu status bar.
///
/// Returns `false` if the sector header magic is absent — this is the state
/// after the Python decoder runs `--mark-read`, which writes zeros over the
/// `RCLG` bytes to invalidate the header without an erase cycle.
#[cfg(target_arch = "arm")]
pub fn has_records(flash: &mut OnboardFlash<'_>) -> bool {
    if read_sector_header(flash).is_err() {
        return false;
    }
    for slot in 0..MAX_RECORDS_PER_SECTOR {
        let offset = CRASH_LOG_OFFSET + SECTOR_HEADER_SIZE + slot * RECORD_SIZE;
        let mut magic = [0u8; 4];
        let _ = flash.blocking_read(offset as u32, &mut magic);
        if magic == super::RECORD_MAGIC {
            return true;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[cfg(target_arch = "arm")]
fn read_scratch_registers() -> ScratchRegs {
    let wd = WATCHDOG;
    let pm = POWMAN;
    let regs = [
        wd.scratch0().read(),
        wd.scratch1().read(),
        wd.scratch2().read(),
        wd.scratch3().read(),
        wd.scratch4().read(),
        wd.scratch5().read(),
        wd.scratch6().read(),
        wd.scratch7().read(),
        pm.scratch(0).read(),
        pm.scratch(1).read(),
        pm.scratch(2).read(),
        pm.scratch(3).read(),
        pm.scratch(4).read(),
        pm.scratch(5).read(),
        pm.scratch(6).read(),
        pm.scratch(7).read(),
    ];
    ScratchRegs(regs)
}

#[cfg(target_arch = "arm")]
fn clear_crash_magic() {
    WATCHDOG.scratch0().write_value(0);
}

/// Clear the watchdog reason register so the timeout is not re-recorded on
/// the next boot.  The register is RW on RP2350; writing 0 clears both the
/// `timer` and `force` sticky bits.
#[cfg(target_arch = "arm")]
fn clear_watchdog_reason() {
    WATCHDOG.reason().write(|w| {
        w.set_timer(false);
        w.set_force(false);
    });
}

/// Build a minimal crash record for a watchdog timeout event.
/// No ARM register frame or GB state is available (the CPU was reset by
/// hardware, not by the fault handler), so all those fields are zeroed.
#[cfg(target_arch = "arm")]
fn build_watchdog_record(slot_seq: u8) -> super::CrashRecord {
    super::CrashRecord {
        schema_ver: 1,
        crash_kind: super::CrashKind::WatchdogTimeout as u8,
        flags: 0,
        slot_seq,
        fw_version: super::FW_VERSION,
        git_hash: super::GIT_HASH_U32,
        ..Default::default()
    }
}

/// Find the next available flash slot and write `record` to it, then update
/// the sector header.
///
/// Returns `Ok(true)` if a record was written, `Ok(false)` if the write was
/// skipped (sector full or duplicate of the previous record), `Err` on a
/// hardware flash fault.
///
/// # Flash-wear protection
///
/// Two policies combine to prevent a crash boot-loop from wearing the crash
/// sector:
///
/// **A — No auto-erase when full.**  When all 31 slots hold records *and* the
/// sector header is valid (`RCLG` magic intact), the sector is treated as
/// "pending user acknowledgement".  The write is skipped without erasing.
/// To resume capture, run `crash_decoder.py --mark-read`; that tool writes
/// zeros over the `RCLG` magic, which signals this function to erase and
/// restart on the next boot.
///
/// **B — Consecutive-crash deduplication.**  Before writing, the previous
/// committed record is read and compared against the pending crash by
/// fingerprint (crash kind, fault-status registers, panic location).  If they
/// match the write is skipped.  For a repeating boot-loop crash this means
/// only the first occurrence is stored — the sector never fills from a single
/// repeating fault.
///
/// `record_builder` receives the resolved slot index and returns the
/// [`CrashRecord`] to write; this keeps slot assignment inside the storage
/// layer while letting callers supply different record types.
#[cfg(target_arch = "arm")]
fn write_record_to_flash(
    flash: &mut OnboardFlash<'_>,
    record_builder: impl FnOnce(u8) -> super::CrashRecord,
) -> Result<bool, FlashError> {
    // Read existing header for erase_count. If missing, the sector is fresh,
    // marked-read, or partially/corruptly written; erase before writing so old
    // hidden records cannot participate in dedupe or reappear under a new header.
    let existing_header = read_sector_header(flash).ok();
    let mut erase_count = existing_header.map(|h| h.erase_count).unwrap_or(0);

    let slot = if existing_header.is_none() {
        erase_count = erase_count.wrapping_add(1);
        flash.blocking_erase(
            CRASH_LOG_OFFSET as u32,
            (CRASH_LOG_OFFSET + ERASE_SIZE) as u32,
        )?;
        0u8
    } else {
        // Read the leading 4 bytes of each slot from flash, then find the first
        // truly-erased (all-0xFF) slot via the pure helper (testable on host).
        let mut slot_magics = [[0u8; 4]; MAX_RECORDS_PER_SECTOR];
        for (i, magic) in slot_magics.iter_mut().enumerate() {
            let offset = CRASH_LOG_OFFSET + SECTOR_HEADER_SIZE + i * RECORD_SIZE;
            let _ = flash.blocking_read(offset as u32, magic);
        }
        match find_next_empty_slot(&slot_magics) {
            Some(s) => s as u8,

            // Header is valid and no erased slot was found: sector is full and
            // the user has not yet acknowledged the existing records. Refuse to
            // erase; run `crash_decoder.py --mark-read` to drain and resume.
            None => {
                defmt::warn!(
                    "crash: log sector full — run crash_decoder.py --mark-read to resume capture"
                );
                return Ok(false);
            }
        }
    };

    // Build the record now so we can fingerprint it before the write.
    let record = record_builder(slot);

    // Deduplication (Option B): if the most recently committed record has the
    // same crash fingerprint as this one, skip the write.  This prevents a
    // repeating boot-loop crash from consuming all 31 slots and triggering
    // repeated sector erases.
    if slot > 0 {
        if let Ok(prev) = read_record_at_slot(flash, (slot - 1) as usize) {
            if is_duplicate_crash(&record, &prev) {
                defmt::warn!("crash: duplicate — same crash site as previous record, skipping");
                return Ok(false);
            }
        }
    }

    // Write the crash record.
    let record_bytes = record.to_bytes();
    let record_offset = CRASH_LOG_OFFSET + SECTOR_HEADER_SIZE + (slot as usize) * RECORD_SIZE;
    flash.blocking_write(record_offset as u32, &record_bytes)?;

    // Write the sector header with RCLG magic + erase_count.  next_slot is
    // stored as 0 and never updated — slot discovery uses RCRP scanning, not
    // this field.  Writing the same fixed value on every commit is safe: the
    // RCLG magic and erase_count are identical bytes, so no bits need to go
    // from 0→1 (that would require an erase that already happened above).
    let header = SectorHeader {
        erase_count,
        next_slot: 0,
    };
    flash.blocking_write(CRASH_LOG_OFFSET as u32, &header.to_bytes())?;

    Ok(true)
}

/// Returns `true` if `new` and `prev` share the same crash fingerprint.
///
/// Compared fields: crash kind, flags, ARM PC and CFSR (for HardFaults), and
/// panic file + line (for panics).  GB register state and cycle counter are
/// deliberately excluded — the emulator can be at a different point each time
/// even when the same code path crashes.
#[cfg(target_arch = "arm")]
#[inline]
fn is_duplicate_crash(new: &super::CrashRecord, prev: &super::CrashRecord) -> bool {
    new.crash_kind == prev.crash_kind
        && new.flags == prev.flags
        && new.arm_pc == prev.arm_pc
        && new.arm_cfsr == prev.arm_cfsr
        && new.panic_loc == prev.panic_loc
        && new.panic_line == prev.panic_line
}

#[cfg(target_arch = "arm")]
fn read_sector_header(flash: &mut OnboardFlash<'_>) -> Result<SectorHeader, SectorDecodeError> {
    let mut buf = [0u8; RECORD_SIZE];
    // blocking_read is infallible for in-range offsets on RP2350.
    let _ = flash.blocking_read(CRASH_LOG_OFFSET as u32, &mut buf);
    SectorHeader::from_bytes(&buf)
}

#[cfg(target_arch = "arm")]
fn read_record_at_slot(
    flash: &mut OnboardFlash<'_>,
    slot: usize,
) -> Result<CrashRecord, super::CrashDecodeError> {
    let offset = CRASH_LOG_OFFSET + SECTOR_HEADER_SIZE + slot * RECORD_SIZE;
    let mut buf = [0u8; RECORD_SIZE];
    let _ = flash.blocking_read(offset as u32, &mut buf);
    CrashRecord::from_bytes(&buf)
}
