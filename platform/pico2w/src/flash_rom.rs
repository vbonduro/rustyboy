use core::fmt::Debug;

use embassy_rp::flash::{Blocking, Error as FlashError, Flash, FLASH_BASE, ERASE_SIZE};
use embassy_rp::peripherals::FLASH;
use embassy_rp::Peri;

use rustyboy_core::memory::RomReader;

pub const FLASH_CAPACITY_BYTES: usize = 4 * 1024 * 1024;
pub const FIRMWARE_SLOT_BYTES: usize = 512 * 1024;
pub const ROM_METADATA_BYTES: usize = ERASE_SIZE;
pub const ROM_SLOT_OFFSET: usize = FIRMWARE_SLOT_BYTES;
pub const ROM_DATA_OFFSET: usize = ROM_SLOT_OFFSET + ROM_METADATA_BYTES;
pub const ROM_DATA_CAPACITY_BYTES: usize = FLASH_CAPACITY_BYTES - ROM_DATA_OFFSET;

const ROM_BANK_BYTES: usize = 0x4000;
const HEADER_MAGIC: [u8; 8] = *b"RBROM1\0\0";
const HEADER_VERSION: u32 = 1;
const HEADER_LEN: usize = 32;
const ROM_SIZE_CODE_OFFSET: usize = 0x0148;

pub type OnboardFlash<'d> = Flash<'d, FLASH, Blocking, FLASH_CAPACITY_BYTES>;

#[derive(Debug, Clone, Copy)]
pub struct FlashRomInfo {
    pub size_bytes: usize,
    pub bank_count: usize,
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
    TooLarge {
        bytes: usize,
        capacity: usize,
    },
}

pub struct FlashRomReader {
    info: FlashRomInfo,
}

impl FlashRomReader {
    pub fn new(info: FlashRomInfo) -> Self {
        Self { info }
    }
}

impl RomReader for FlashRomReader {
    type Error = FlashRomReadError;

    fn read_bank(&mut self, bank: usize, buf: &mut [u8; ROM_BANK_BYTES]) -> Result<(), Self::Error> {
        if bank >= self.info.bank_count {
            buf.fill(0xFF);
            return Err(FlashRomReadError::OutOfBounds);
        }

        let src = (FLASH_BASE as usize) + ROM_DATA_OFFSET + bank * ROM_BANK_BYTES;
        let src = src as *const u8;

        unsafe {
            let rom = core::slice::from_raw_parts(src, ROM_BANK_BYTES);
            buf.copy_from_slice(rom);
        }

        Ok(())
    }
}

pub fn new_onboard_flash<'d>(flash: Peri<'d, FLASH>) -> OnboardFlash<'d> {
    Flash::new_blocking(flash)
}

pub fn probe_staged_rom() -> Option<FlashRomInfo> {
    let header = read_header();
    parse_header(&header)
}

pub fn stage_rom_from_reader<R: RomReader>(
    flash: &mut OnboardFlash<'_>,
    reader: &mut R,
) -> Result<FlashRomInfo, FlashRomStageError<R::Error>>
where
    R::Error: Debug,
{
    let mut bank0 = [0u8; ROM_BANK_BYTES];
    reader
        .read_bank(0, &mut bank0)
        .map_err(FlashRomStageError::Reader)?;

    let rom_size_code = bank0[ROM_SIZE_CODE_OFFSET];
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
        .blocking_write(ROM_DATA_OFFSET as u32, &bank0)
        .map_err(FlashRomStageError::Flash)?;

    let mut bank_buf = [0u8; ROM_BANK_BYTES];
    for bank in 1..bank_count {
        reader
            .read_bank(bank, &mut bank_buf)
            .map_err(FlashRomStageError::Reader)?;
        flash
            .blocking_write((ROM_DATA_OFFSET + bank * ROM_BANK_BYTES) as u32, &bank_buf)
            .map_err(FlashRomStageError::Flash)?;
    }

    let info = FlashRomInfo {
        size_bytes,
        bank_count,
    };
    let header = build_header(info);
    flash
        .blocking_write(ROM_SLOT_OFFSET as u32, &header)
        .map_err(FlashRomStageError::Flash)?;

    Ok(info)
}

fn read_header() -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    let src = (FLASH_BASE as usize + ROM_SLOT_OFFSET) as *const u8;
    unsafe {
        header.copy_from_slice(core::slice::from_raw_parts(src, HEADER_LEN));
    }
    header
}

fn parse_header(header: &[u8; HEADER_LEN]) -> Option<FlashRomInfo> {
    if header[..8] != HEADER_MAGIC {
        return None;
    }

    let version = u32::from_le_bytes(header[8..12].try_into().ok()?);
    if version != HEADER_VERSION {
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

    Some(FlashRomInfo {
        size_bytes,
        bank_count,
    })
}

fn build_header(info: FlashRomInfo) -> [u8; HEADER_LEN] {
    let mut header = [0xFFu8; HEADER_LEN];
    header[..8].copy_from_slice(&HEADER_MAGIC);
    header[8..12].copy_from_slice(&HEADER_VERSION.to_le_bytes());
    header[12..16].copy_from_slice(&(info.size_bytes as u32).to_le_bytes());
    header[16..20].copy_from_slice(&(!(info.size_bytes as u32)).to_le_bytes());
    header[20..24].copy_from_slice(&(info.bank_count as u32).to_le_bytes());
    header
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
