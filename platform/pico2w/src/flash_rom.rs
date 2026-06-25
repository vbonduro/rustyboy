use core::fmt::Debug;

use embassy_rp::flash::{Blocking, Error as FlashError, Flash, ERASE_SIZE};
use embassy_rp::peripherals::FLASH;
use embassy_rp::Peri;

use rustyboy_core::memory::RomReader;
use rustyboy_core::storage::{RomHasher, RomId};

pub const FLASH_CAPACITY_BYTES: usize = 4 * 1024 * 1024;
/// Firmware image slot size.  Must match the `FLASH LENGTH` in `memory.x`.
/// Expanded from 512 KiB to 640 KiB to accommodate ratatui/mousefood UI layer.
pub const FIRMWARE_SLOT_BYTES: usize = 640 * 1024;
pub const ROM_METADATA_BYTES: usize = ERASE_SIZE;
pub const ROM_SLOT_OFFSET: usize = FIRMWARE_SLOT_BYTES;
pub const ROM_DATA_OFFSET: usize = ROM_SLOT_OFFSET + ROM_METADATA_BYTES;

/// One 4 KiB sector reserved for WiFi credentials (SSID + password).
/// Immediately before the crash log sector.
pub const WIFI_CONFIG_OFFSET: usize = FLASH_CAPACITY_BYTES - 2 * ERASE_SIZE;

/// One 4 KiB sector at the very end of flash is reserved for crash records.
/// This is written by `crash::storage::check_and_commit` on each boot when a
/// crash was detected.  ROM data must not be staged here.
pub const CRASH_LOG_OFFSET: usize = FLASH_CAPACITY_BYTES - ERASE_SIZE;

/// ROM data capacity is the full flash minus firmware, metadata, WiFi config, and crash log.
pub const ROM_DATA_CAPACITY_BYTES: usize = FLASH_CAPACITY_BYTES - ROM_DATA_OFFSET - 2 * ERASE_SIZE;

const ROM_BANK_BYTES: usize = 0x4000;
const HEADER_MAGIC: [u8; 8] = *b"RBROM1\0\0";
const HEADER_VERSION_V1: u32 = 1;
const HEADER_VERSION_V2: u32 = 2;
const HEADER_VERSION: u32 = HEADER_VERSION_V2;
// Layout:
//   [0..8]   magic
//   [8..12]  version
//   [12..16] size_bytes
//   [16..20] size_bytes_inv
//   [20..24] bank_count
//   [24..32] reserved (0xFF padding)
//   [32..96] filename (null-terminated UTF-8, max 63 chars + null)
//   [96..128] rom_id SHA-256 bytes (v2+, 0xFF padding if absent)
const HEADER_LEN: usize = 128;
const FILENAME_OFFSET: usize = 32;
const FILENAME_MAX_BYTES: usize = 63; // + 1 null terminator
const ROM_ID_OFFSET: usize = 96;
const ROM_SIZE_CODE_OFFSET: usize = 0x0148;

pub type OnboardFlash<'d> = Flash<'d, FLASH, Blocking, FLASH_CAPACITY_BYTES>;

#[derive(Debug, Clone, Copy)]
pub struct FlashRomInfo {
    pub size_bytes: usize,
    pub bank_count: usize,
    pub rom_id: Option<RomId>,
}

#[derive(Debug, Clone, Copy)]
pub enum FlashRomReadError {
    OutOfBounds,
}

#[derive(Debug)]
pub enum FlashRomStageError<E: Debug> {
    Reader(E),
    Flash(FlashError),
    InvalidRomSizeCode(u8),
    TooLarge { bytes: usize, capacity: usize },
}

pub struct FlashRomReader<'d> {
    flash: OnboardFlash<'d>,
    info: FlashRomInfo,
}

impl<'d> FlashRomReader<'d> {
    pub fn new(flash: OnboardFlash<'d>, info: FlashRomInfo) -> Self {
        Self { flash, info }
    }
}

impl RomReader for FlashRomReader<'_> {
    type Error = FlashRomReadError;

    fn read_bank(
        &mut self,
        bank: usize,
        buf: &mut [u8; ROM_BANK_BYTES],
    ) -> Result<(), Self::Error> {
        if bank >= self.info.bank_count {
            buf.fill(0xFF);
            return Err(FlashRomReadError::OutOfBounds);
        }

        self.flash
            .blocking_read((ROM_DATA_OFFSET + bank * ROM_BANK_BYTES) as u32, buf)
            .map_err(|_| FlashRomReadError::OutOfBounds)?;

        Ok(())
    }
}

pub fn new_onboard_flash<'d>(flash: Peri<'d, FLASH>) -> OnboardFlash<'d> {
    Flash::new_blocking(flash)
}

/// Check for a valid staged ROM header.
///
/// Returns `(FlashRomInfo, staged_filename)` if valid. The filename is `None`
/// when the header was written by an older firmware version that did not
/// include the filename field.
pub fn probe_staged_rom(
    flash: &mut OnboardFlash<'_>,
) -> Option<(FlashRomInfo, Option<heapless::String<64>>)> {
    let header = read_header(flash).ok()?;
    parse_header(&header)
}

// ---------------------------------------------------------------------------
// RomStager — step-by-step staging for async progress updates
// ---------------------------------------------------------------------------

/// Returned by [`RomStager::write_next_bank`].
pub enum WriteResult {
    /// More banks remain; call `write_next_bank` again.
    Continue,
    /// All banks written and metadata header committed. Contains the ROM info.
    Done(FlashRomInfo),
}

/// Stateful ROM stager that allows the caller to interleave progress draws
/// between bank writes.
///
/// Usage:
/// 1. Call `begin` — reads bank 0, parses bank count, erases flash.
/// 2. Loop: call `write_next_bank` until it returns `Ok(WriteResult::Done(_))`.
pub struct RomStager {
    bank_count: usize,
    banks_written: usize,
    buf: alloc::boxed::Box<[u8; ROM_BANK_BYTES]>,
    filename: alloc::string::String,
    rom_hasher: Option<RomHasher>,
    rom_id: Option<RomId>,
}

impl RomStager {
    /// Start staging: read bank 0, determine bank count, erase flash.
    pub fn begin<R: RomReader>(
        flash: &mut OnboardFlash<'_>,
        reader: &mut R,
        filename: &str,
    ) -> Result<Self, FlashRomStageError<R::Error>>
    where
        R::Error: Debug,
    {
        let mut buf = alloc::boxed::Box::new([0u8; ROM_BANK_BYTES]);
        reader
            .read_bank(0, &mut *buf)
            .map_err(FlashRomStageError::Reader)?;
        let mut rom_hasher = RomHasher::new();
        rom_hasher.update(&*buf);

        let rom_size_code = buf[ROM_SIZE_CODE_OFFSET];
        let bank_count = rom_bank_count_from_code(rom_size_code)
            .ok_or(FlashRomStageError::InvalidRomSizeCode(rom_size_code))?;
        let size_bytes = bank_count * ROM_BANK_BYTES;

        if size_bytes > ROM_DATA_CAPACITY_BYTES {
            return Err(FlashRomStageError::TooLarge {
                bytes: size_bytes,
                capacity: ROM_DATA_CAPACITY_BYTES,
            });
        }

        let erase_end = align_up(ROM_DATA_OFFSET + size_bytes, ERASE_SIZE);
        flash
            .blocking_erase(ROM_SLOT_OFFSET as u32, erase_end as u32)
            .map_err(FlashRomStageError::Flash)?;

        flash
            .blocking_write(ROM_DATA_OFFSET as u32, &*buf)
            .map_err(FlashRomStageError::Flash)?;

        Ok(Self {
            bank_count,
            banks_written: 1,
            buf,
            filename: alloc::string::String::from(filename),
            rom_hasher: Some(rom_hasher),
            rom_id: None,
        })
    }

    pub fn total_banks(&self) -> usize {
        self.bank_count
    }

    pub fn banks_written(&self) -> usize {
        self.banks_written
    }

    /// Write the next bank. Returns `Done(info)` on the final bank, which also
    /// commits the metadata header — no separate call required.
    pub fn write_next_bank<R: RomReader>(
        &mut self,
        flash: &mut OnboardFlash<'_>,
        reader: &mut R,
    ) -> Result<WriteResult, FlashRomStageError<R::Error>>
    where
        R::Error: Debug,
    {
        if self.banks_written >= self.bank_count {
            let info = self.make_info();
            return Ok(WriteResult::Done(info));
        }
        reader
            .read_bank(self.banks_written, &mut *self.buf)
            .map_err(FlashRomStageError::Reader)?;
        if let Some(hasher) = self.rom_hasher.as_mut() {
            hasher.update(&*self.buf);
        }
        flash
            .blocking_write(
                (ROM_DATA_OFFSET + self.banks_written * ROM_BANK_BYTES) as u32,
                &*self.buf,
            )
            .map_err(FlashRomStageError::Flash)?;
        self.banks_written += 1;
        if self.banks_written >= self.bank_count {
            let info = self.make_info();
            let header = build_header(info, &self.filename);
            flash
                .blocking_write(ROM_SLOT_OFFSET as u32, &header)
                .map_err(FlashRomStageError::Flash)?;
            Ok(WriteResult::Done(info))
        } else {
            Ok(WriteResult::Continue)
        }
    }

    fn make_info(&mut self) -> FlashRomInfo {
        FlashRomInfo {
            size_bytes: self.bank_count * ROM_BANK_BYTES,
            bank_count: self.bank_count,
            rom_id: Some(self.finalize_rom_id()),
        }
    }

    fn finalize_rom_id(&mut self) -> RomId {
        if let Some(rom_id) = self.rom_id {
            return rom_id;
        }
        let rom_id = self
            .rom_hasher
            .take()
            .expect("ROM hasher finalized twice")
            .finalize();
        self.rom_id = Some(rom_id);
        rom_id
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

fn read_header(flash: &mut OnboardFlash<'_>) -> Result<[u8; HEADER_LEN], FlashError> {
    let mut header = [0u8; HEADER_LEN];
    flash.blocking_read(ROM_SLOT_OFFSET as u32, &mut header)?;
    Ok(header)
}

fn parse_header(header: &[u8; HEADER_LEN]) -> Option<(FlashRomInfo, Option<heapless::String<64>>)> {
    if header[..8] != HEADER_MAGIC {
        return None;
    }

    let version = u32::from_le_bytes(header[8..12].try_into().ok()?);
    if version != HEADER_VERSION_V1 && version != HEADER_VERSION_V2 {
        return None;
    }

    let size_bytes = u32::from_le_bytes(header[12..16].try_into().ok()?) as usize;
    let size_bytes_inv = u32::from_le_bytes(header[16..20].try_into().ok()?) as usize;
    let bank_count = u32::from_le_bytes(header[20..24].try_into().ok()?) as usize;

    if size_bytes == 0 || size_bytes > ROM_DATA_CAPACITY_BYTES {
        return None;
    }
    if size_bytes ^ size_bytes_inv != u32::MAX as usize {
        return None;
    }
    if size_bytes % ROM_BANK_BYTES != 0 {
        return None;
    }
    if bank_count == 0 || bank_count * ROM_BANK_BYTES != size_bytes {
        return None;
    }

    let info = FlashRomInfo {
        size_bytes,
        bank_count,
        rom_id: parse_rom_id(header, version),
    };

    // Parse filename from bytes [FILENAME_OFFSET..FILENAME_OFFSET+64].
    // Absent (all 0xFF) when written by old firmware — treat as None.
    let name_region = &header[FILENAME_OFFSET..FILENAME_OFFSET + FILENAME_MAX_BYTES + 1];
    let staged_name = parse_filename(name_region);

    Some((info, staged_name))
}

fn parse_filename(region: &[u8]) -> Option<heapless::String<64>> {
    let null_pos = region.iter().position(|&b| b == 0)?;
    if null_pos == 0 {
        return None;
    }
    let bytes = &region[..null_pos];
    // Old firmware leaves 0xFF; treat as absent.
    if bytes.iter().all(|&b| b == 0xFF) {
        return None;
    }
    core::str::from_utf8(bytes)
        .ok()
        .and_then(|s| heapless::String::try_from(s).ok())
}

fn build_header(info: FlashRomInfo, filename: &str) -> [u8; HEADER_LEN] {
    let mut header = [0xFFu8; HEADER_LEN];
    header[..8].copy_from_slice(&HEADER_MAGIC);
    header[8..12].copy_from_slice(&HEADER_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(info.size_bytes as u32).to_le_bytes());
    header[16..20].copy_from_slice(&(!(info.size_bytes as u32)).to_le_bytes());
    header[20..24].copy_from_slice(&(info.bank_count as u32).to_le_bytes());
    let name_bytes = filename.as_bytes();
    let copy_len = name_bytes.len().min(FILENAME_MAX_BYTES);
    header[FILENAME_OFFSET..FILENAME_OFFSET + copy_len].copy_from_slice(&name_bytes[..copy_len]);
    header[FILENAME_OFFSET + copy_len] = 0;
    if let Some(rom_id) = info.rom_id {
        header[ROM_ID_OFFSET..ROM_ID_OFFSET + 32].copy_from_slice(rom_id.as_bytes());
    }
    header
}

fn parse_rom_id(header: &[u8; HEADER_LEN], version: u32) -> Option<RomId> {
    if version < HEADER_VERSION_V2 {
        return None;
    }
    let bytes: [u8; 32] = header[ROM_ID_OFFSET..ROM_ID_OFFSET + 32].try_into().ok()?;
    if bytes.iter().all(|&b| b == 0xFF) {
        None
    } else {
        Some(RomId::from_bytes(bytes))
    }
}

fn rom_bank_count_from_code(code: u8) -> Option<usize> {
    match code {
        0x00..=0x08 => Some(2usize << code),
        0x52 => Some(72),
        0x53 => Some(80),
        0x54 => Some(96),
        _ => None,
    }
}

const fn align_up(value: usize, align: usize) -> usize {
    let rem = value % align;
    if rem == 0 {
        value
    } else {
        value + (align - rem)
    }
}

// ---------------------------------------------------------------------------
// Compile-time flash layout invariants (Finding E)
// ---------------------------------------------------------------------------

/// WiFi config sector must be exactly one ERASE_SIZE before the crash log.
const _: () = assert!(WIFI_CONFIG_OFFSET + ERASE_SIZE == CRASH_LOG_OFFSET);

/// Crash log sector must be the last sector in flash.
const _: () = assert!(CRASH_LOG_OFFSET + ERASE_SIZE == FLASH_CAPACITY_BYTES);

/// ROM data region must not overlap the WiFi config sector.
const _: () = assert!(ROM_DATA_OFFSET + ROM_DATA_CAPACITY_BYTES <= WIFI_CONFIG_OFFSET);
