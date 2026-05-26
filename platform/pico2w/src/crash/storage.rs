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
    find_next_empty_slot, CrashRecord, SectorDecodeError, SectorHeader, ScratchRegs,
    RECORD_SIZE, SECTOR_HEADER_SIZE, MAX_RECORDS_PER_SECTOR,
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

    if let Err(e) = write_record_to_flash(flash, |slot| snap.to_crash_record(slot)) {
        defmt::error!("crash: flash commit failed: {:?}", e);
        return false;
    }

    defmt::info!("crash: committed crash record to flash");
    true
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
    let reason = WATCHDOG.reason().read();
    if !reason.timer() {
        return false;
    }

    // Clear the reason register immediately so we don't re-record on the
    // next boot (the register persists across watchdog resets).
    clear_watchdog_reason();

    if let Err(e) = write_record_to_flash(flash, |slot| build_watchdog_record(slot)) {
        defmt::error!("crash: watchdog flash commit failed: {:?}", e);
        return false;
    }

    defmt::info!("crash: watchdog timeout — committed record to flash");
    true
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
pub fn read_records(
    flash: &mut OnboardFlash<'_>,
    buf: &mut [CrashRecord],
) -> usize {
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
    flash.blocking_erase(CRASH_LOG_OFFSET as u32, (CRASH_LOG_OFFSET + ERASE_SIZE) as u32)
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
/// the sector header.  Erases the sector first if all 31 slots are full.
///
/// `record_builder` receives the resolved slot index and returns the
/// [`CrashRecord`] to write; this keeps slot assignment inside the storage
/// layer while letting callers supply different record types.
#[cfg(target_arch = "arm")]
fn write_record_to_flash(
    flash: &mut OnboardFlash<'_>,
    record_builder: impl FnOnce(u8) -> super::CrashRecord,
) -> Result<(), FlashError> {
    // Read existing header for erase_count.  If missing, treat as a fresh sector.
    let existing_header = read_sector_header(flash).ok();
    let erase_count = existing_header.map(|h| h.erase_count).unwrap_or(0);

    // Read the leading 4 bytes of each slot from flash, then find the first
    // empty one via the pure `find_next_empty_slot` helper (testable on host).
    let mut slot_magics = [[0u8; 4]; MAX_RECORDS_PER_SECTOR];
    for (i, magic) in slot_magics.iter_mut().enumerate() {
        let offset = CRASH_LOG_OFFSET + SECTOR_HEADER_SIZE + i * RECORD_SIZE;
        let _ = flash.blocking_read(offset as u32, magic);
    }
    let next_slot = find_next_empty_slot(&slot_magics);

    let (slot, erase_count) = match next_slot {
        Some(s) => (s as u8, erase_count),
        None => {
            // All 31 slots full — erase the sector and restart.
            let new_erase = erase_count.wrapping_add(1);
            flash.blocking_erase(
                CRASH_LOG_OFFSET as u32,
                (CRASH_LOG_OFFSET + ERASE_SIZE) as u32,
            )?;
            (0u8, new_erase)
        }
    };

    // Build and write the crash record.
    let record = record_builder(slot);
    let record_bytes = record.to_bytes();
    let record_offset = CRASH_LOG_OFFSET + SECTOR_HEADER_SIZE + (slot as usize) * RECORD_SIZE;
    flash.blocking_write(record_offset as u32, &record_bytes)?;

    // Write the sector header with RCLG magic + erase_count.  next_slot is
    // stored as 0 and never updated — slot discovery uses RCRP scanning, not
    // this field.  Writing the same fixed value on every commit is safe: the
    // RCLG magic and erase_count are identical bytes, so no bits need to go
    // from 0→1 (that would require an erase that already happened above).
    let header = SectorHeader { erase_count, next_slot: 0 };
    flash.blocking_write(CRASH_LOG_OFFSET as u32, &header.to_bytes())?;

    Ok(())
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
