//! On-flash sector header: [`SectorHeader`], [`SectorDecodeError`], slot discovery.

use super::record::MAX_RECORDS_PER_SECTOR;
use super::{RECORD_SIZE, SECTOR_MAGIC};

// ---------------------------------------------------------------------------
// Sector-level constants
// ---------------------------------------------------------------------------

pub const SECTOR_FULL: u8 = 0xFF;

// ---------------------------------------------------------------------------
// Slot discovery (pure, testable without hardware)
// ---------------------------------------------------------------------------

/// Return the index of the first slot that is completely erased (all bytes
/// 0xFF), or `None` if no such slot exists (sector is full or entirely corrupt).
///
/// **Why 0xFF instead of "not RCRP"?**
/// NOR flash can only transition bits from 1 → 0, never 0 → 1 without an
/// erase.  A slot that contains any non-0xFF byte is considered occupied —
/// whether it holds a valid RCRP record, a corrupt/partially-written record,
/// or garbage from a failed previous write.  Selecting such a slot as the
/// write target would AND the new data with the residual bits, silently
/// corrupting the magic and CRC without returning any error.
///
/// When all 31 slots are occupied (by either valid records or non-erased
/// corruption), this returns `None`, triggering a sector erase in the caller
/// before a fresh record is written.
///
/// The caller reads the leading 4 bytes of each record slot from flash and
/// passes them here.  Keeping this logic pure (no I/O) lets it be unit-tested
/// on the host without any embedded hardware or flash driver.
pub fn find_next_empty_slot(slot_magics: &[[u8; 4]; MAX_RECORDS_PER_SECTOR]) -> Option<usize> {
    slot_magics
        .iter()
        .position(|m| m == &[0xFF, 0xFF, 0xFF, 0xFF])
}

// ---------------------------------------------------------------------------
// SectorHeader — 128-byte header at byte 0 of the crash log flash sector.
// ---------------------------------------------------------------------------
//
//   [0..4]   magic        b"RCLG"
//   [4..8]   erase_count  how many times this sector has been erased
//   [8]      next_slot    next free record slot (0-30); 0xFF = full
//   [9..128] _reserved    zeros

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SectorHeader {
    pub erase_count: u32,
    /// Next free record slot index (0-30).  `SECTOR_FULL` when no room remains.
    pub next_slot: u8,
}

impl SectorHeader {
    /// Fresh header for a newly erased sector.
    pub fn fresh(erase_count: u32) -> Self {
        Self {
            erase_count,
            next_slot: 0,
        }
    }

    pub fn to_bytes(&self) -> [u8; RECORD_SIZE] {
        let mut buf = [0u8; RECORD_SIZE];
        buf[0..4].copy_from_slice(&SECTOR_MAGIC);
        buf[4..8].copy_from_slice(&self.erase_count.to_le_bytes());
        buf[8] = self.next_slot;
        buf
    }

    pub fn from_bytes(buf: &[u8; RECORD_SIZE]) -> Result<Self, SectorDecodeError> {
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
