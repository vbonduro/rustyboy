use alloc::vec::Vec;

use defmt::info;
use embedded_sdmmc::{
    BlockDevice, LfnBuffer, Mode, RawDirectory, RawFile, RawVolume, ShortFileName, TimeSource,
    VolumeIdx, VolumeManager,
};

use crate::save_storage::{
    battery_save_filename, rom_save_dir_name, save_state_filename, SaveSlot, SAVE_ROOT_DIR,
};
use core::cmp::Ordering;
use rustyboy_core::memory::RomReader;
use rustyboy_core::storage::{BatterySaveBytes, RomId, SaveStateBytes, StorageValueError};

// ── Time source ───────────────────────────────────────────────────────────────

pub struct DummyClock;
impl TimeSource for DummyClock {
    fn get_timestamp(&self) -> embedded_sdmmc::Timestamp {
        embedded_sdmmc::Timestamp::from_fat(0, 0)
    }
}

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SdError<E: core::fmt::Debug> {
    Sdmmc(embedded_sdmmc::Error<E>),
    InvalidValue(StorageValueError),
    NoRomFound,
    OutOfMemory,
}

impl<E: core::fmt::Debug> From<embedded_sdmmc::Error<E>> for SdError<E> {
    fn from(e: embedded_sdmmc::Error<E>) -> Self {
        SdError::Sdmmc(e)
    }
}

#[derive(Clone)]
pub struct RomListEntry {
    /// FAT short filename used for opening/staging the ROM.
    pub filename: heapless::String<64>,
    /// Long filename when available; falls back to `filename`.
    pub display_name: heapless::String<64>,
}

pub struct RomPage {
    pub entries: heapless::Vec<RomListEntry, 7>,
    pub has_next: bool,
    pub total: usize,
}

struct RomDirEntry {
    filename: ShortFileName,
    display_name: heapless::String<64>,
}

// ── SdManager ─────────────────────────────────────────────────────────────────

/// Owns the `VolumeManager` and provides paginated listing and file opening.
pub struct SdManager<D, T = DummyClock>
where
    D: BlockDevice,
    <D as BlockDevice>::Error: core::fmt::Debug,
    T: TimeSource,
{
    mgr: VolumeManager<D, T>,
}

impl<D, T> SdManager<D, T>
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    pub fn new(device: D, timesource: T) -> Self {
        Self {
            mgr: VolumeManager::new(device, timesource),
        }
    }

    /// Return up to `page_size` ROM filenames starting at `page_offset`,
    /// sorted alphabetically, plus a flag indicating whether more entries follow.
    ///
    /// Iterates the full root directory each call (up to 100 entries) and
    /// slices the requested page.
    /// List a page of ROMs, calling `on_progress` for every directory entry
    /// visited.
    ///
    /// The callback exists so the caller can feed the watchdog. This traversal
    /// walks the WHOLE root directory (no early exit) and runs synchronously
    /// inside a single main-loop iteration, so with a slow card it can outlast
    /// the 16 s watchdog window — which is exactly what made entering the ROM
    /// list hang and reset with no crash record.
    pub fn list_rom_page_with(
        &self,
        page_offset: usize,
        page_size: usize,
        mut on_progress: impl FnMut(),
    ) -> Result<RomPage, SdError<D::Error>> {
        let mut names: Vec<RomDirEntry> = Vec::new();
        names
            .try_reserve_exact(100)
            .map_err(|_| SdError::OutOfMemory)?;

        let volume = self.mgr.open_raw_volume(VolumeIdx(0))?;
        let root = self.mgr.open_root_dir(volume)?;

        let mut lfn_storage = [0u8; 260];
        let mut lfn_buffer = LfnBuffer::new(&mut lfn_storage);
        let _ = self
            .mgr
            .iterate_dir_lfn(root, &mut lfn_buffer, |entry, lfn| {
                // Every entry, not just matches: the cost is the traversal, and
                // a directory full of non-ROM files is exactly the slow case.
                on_progress();
                if !entry.attributes.is_directory() && is_rom_file(&entry.name) && names.len() < 100
                {
                    let display_name = match lfn.filter(|name| !name.is_empty()) {
                        Some(name) => str_to_heapless_64(name),
                        None => sfn_to_string(&entry.name),
                    };
                    let _ = names.push(RomDirEntry {
                        filename: entry.name.clone(),
                        display_name,
                    });
                }
            });
        let _ = self.mgr.close_dir(root);
        let _ = self.mgr.close_volume(volume);

        sort_rom_names(&mut names);
        let total = names.len();
        let start = page_offset.min(total);
        let has_next = start + page_size < total;
        let mut page: heapless::Vec<RomListEntry, 7> = heapless::Vec::new();
        for entry in names[start..].iter().take(page_size) {
            let _ = page.push(RomListEntry {
                filename: sfn_to_string(&entry.filename),
                display_name: entry.display_name.clone(),
            });
        }

        Ok(RomPage {
            entries: page,
            has_next,
            total,
        })
    }

    /// Convenience wrapper for callers with no watchdog to feed.
    pub fn list_rom_page(
        &self,
        page_offset: usize,
        page_size: usize,
    ) -> Result<RomPage, SdError<D::Error>> {
        self.list_rom_page_with(page_offset, page_size, || {})
    }

    /// Open a specific ROM file by FAT filename (case-insensitive).
    ///
    /// The returned `SdRomReader` holds an exclusive borrow of this manager.
    pub fn open_rom_reader<'a>(
        &'a mut self,
        filename: &str,
    ) -> Result<SdRomReader<'a, D, T>, SdError<D::Error>> {
        let volume = self.mgr.open_raw_volume(VolumeIdx(0))?;
        let root = self.mgr.open_root_dir(volume)?;

        let mut found_name: Option<ShortFileName> = None;
        let _ = self.mgr.iterate_dir(root, |entry| {
            if found_name.is_none() {
                let entry_display = sfn_to_string(&entry.name);
                if entry_display.as_str().eq_ignore_ascii_case(filename) {
                    found_name = Some(entry.name.clone());
                }
            }
        });

        match found_name {
            None => {
                let _ = self.mgr.close_dir(root);
                let _ = self.mgr.close_volume(volume);
                Err(SdError::NoRomFound)
            }
            Some(sfn) => {
                let file = match self.mgr.open_file_in_dir(root, &sfn, Mode::ReadOnly) {
                    Ok(f) => f,
                    Err(e) => {
                        let _ = self.mgr.close_dir(root);
                        let _ = self.mgr.close_volume(volume);
                        return Err(SdError::Sdmmc(e));
                    }
                };
                let _ = self.mgr.close_dir(root);
                info!("sd: opened {}", filename);
                Ok(SdRomReader {
                    mgr: &mut self.mgr,
                    volume,
                    file,
                })
            }
        }
    }

    pub fn write_battery_save(&self, rom_id: &RomId, data: &[u8]) -> Result<(), SdError<D::Error>> {
        BatterySaveBytes::new(data).map_err(SdError::InvalidValue)?;
        self.with_rom_save_dir(rom_id, true, |mgr, dir| {
            write_file(mgr, dir, battery_save_filename(), data)
        })
    }

    pub fn read_battery_save(&self, rom_id: &RomId) -> Result<Option<Vec<u8>>, SdError<D::Error>> {
        let Some(data) = self.with_existing_rom_save_dir(rom_id, |mgr, dir| {
            read_file_optional(mgr, dir, battery_save_filename())
        })?
        else {
            return Ok(None);
        };

        if let Some(data) = data.as_ref() {
            BatterySaveBytes::new(data).map_err(SdError::InvalidValue)?;
        }
        Ok(data)
    }

    pub fn write_save_state(
        &self,
        rom_id: &RomId,
        slot: SaveSlot,
        data: &[u8],
    ) -> Result<(), SdError<D::Error>> {
        SaveStateBytes::new(data).map_err(SdError::InvalidValue)?;
        let filename = save_state_filename(slot);
        self.with_rom_save_dir(rom_id, true, |mgr, dir| {
            write_file(mgr, dir, filename.as_str(), data)
        })
    }

    pub fn read_save_state(
        &self,
        rom_id: &RomId,
        slot: SaveSlot,
    ) -> Result<Option<Vec<u8>>, SdError<D::Error>> {
        let filename = save_state_filename(slot);
        let Some(data) = self.with_existing_rom_save_dir(rom_id, |mgr, dir| {
            read_file_optional(mgr, dir, filename.as_str())
        })?
        else {
            return Ok(None);
        };

        if let Some(data) = data.as_ref() {
            SaveStateBytes::new(data).map_err(SdError::InvalidValue)?;
        }
        Ok(data)
    }

    pub fn save_state_exists(
        &self,
        rom_id: &RomId,
        slot: SaveSlot,
    ) -> Result<bool, SdError<D::Error>> {
        let filename = save_state_filename(slot);
        let Some(exists) = self.with_existing_rom_save_dir(rom_id, |mgr, dir| {
            file_exists(mgr, dir, filename.as_str())
        })?
        else {
            return Ok(false);
        };
        Ok(exists)
    }

    fn with_rom_save_dir<R>(
        &self,
        rom_id: &RomId,
        create: bool,
        f: impl FnOnce(&VolumeManager<D, T>, RawDirectory) -> Result<R, SdError<D::Error>>,
    ) -> Result<R, SdError<D::Error>> {
        let volume = self.mgr.open_raw_volume(VolumeIdx(0))?;
        let root = match self.mgr.open_root_dir(volume) {
            Ok(root) => root,
            Err(e) => {
                let _ = self.mgr.close_volume(volume);
                return Err(SdError::Sdmmc(e));
            }
        };

        let saves = match open_save_dir(&self.mgr, root, SAVE_ROOT_DIR, create) {
            Ok(dir) => dir,
            Err(e) => {
                let _ = self.mgr.close_dir(root);
                let _ = self.mgr.close_volume(volume);
                return Err(e);
            }
        };

        let rom_dir_name = rom_save_dir_name(rom_id);
        let rom_dir = match open_save_dir(&self.mgr, saves, rom_dir_name.as_str(), create) {
            Ok(dir) => dir,
            Err(e) => {
                let _ = self.mgr.close_dir(saves);
                let _ = self.mgr.close_dir(root);
                let _ = self.mgr.close_volume(volume);
                return Err(e);
            }
        };

        let mut result = f(&self.mgr, rom_dir);
        close_dir_if_ok(&self.mgr, rom_dir, &mut result);
        close_dir_if_ok(&self.mgr, saves, &mut result);
        close_dir_if_ok(&self.mgr, root, &mut result);
        close_volume_if_ok(&self.mgr, volume, &mut result);
        result
    }

    fn with_existing_rom_save_dir<R>(
        &self,
        rom_id: &RomId,
        f: impl FnOnce(&VolumeManager<D, T>, RawDirectory) -> Result<R, SdError<D::Error>>,
    ) -> Result<Option<R>, SdError<D::Error>> {
        match self.with_rom_save_dir(rom_id, false, f) {
            Ok(value) => Ok(Some(value)),
            Err(SdError::Sdmmc(embedded_sdmmc::Error::NotFound)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ── SdRomReader ───────────────────────────────────────────────────────────────

/// Borrows a `VolumeManager` for the duration of a sequential ROM read.
pub struct SdRomReader<'a, D, T = DummyClock>
where
    D: BlockDevice,
    <D as BlockDevice>::Error: core::fmt::Debug,
    T: TimeSource,
{
    mgr: &'a mut VolumeManager<D, T>,
    volume: RawVolume,
    file: RawFile,
}

impl<'a, D, T> Drop for SdRomReader<'a, D, T>
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    fn drop(&mut self) {
        let _ = self.mgr.close_file(self.file);
        let _ = self.mgr.close_volume(self.volume);
    }
}

impl<'a, D, T> RomReader for SdRomReader<'a, D, T>
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    type Error = SdError<D::Error>;

    fn read_bank(&mut self, bank: usize, buf: &mut [u8; 0x4000]) -> Result<(), Self::Error> {
        let offset = (bank as u32) * 0x4000;
        self.mgr.file_seek_from_start(self.file, offset)?;
        let mut total = 0;
        while total < 0x4000 {
            let n = self.mgr.read(self.file, &mut buf[total..])?;
            if n == 0 {
                break;
            }
            total += n;
        }
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn is_rom_file(name: &ShortFileName) -> bool {
    let ext = name.extension();
    ext == b"GB" || ext == b"GBC"
}

fn sfn_to_string(name: &ShortFileName) -> heapless::String<64> {
    use core::fmt::Write;
    let mut s: heapless::String<64> = heapless::String::new();
    let _ = write!(s, "{}", name);
    s
}

fn str_to_heapless_64(value: &str) -> heapless::String<64> {
    let mut out = heapless::String::new();
    for ch in value.chars() {
        if out.push(ch).is_err() {
            break;
        }
    }
    out
}

fn sort_rom_names(entries: &mut [RomDirEntry]) {
    for i in 1..entries.len() {
        let mut j = i;
        while j > 0 && compare_rom_display(&entries[j], &entries[j - 1]).is_lt() {
            entries.swap(j, j - 1);
            j -= 1;
        }
    }
}

fn compare_rom_display(a: &RomDirEntry, b: &RomDirEntry) -> Ordering {
    compare_ascii(a.display_name.as_str(), b.display_name.as_str())
}

fn compare_ascii(a: &str, b: &str) -> Ordering {
    let mut a_iter = a.as_bytes().iter().copied();
    let mut b_iter = b.as_bytes().iter().copied();

    loop {
        match (a_iter.next(), b_iter.next()) {
            (Some(a), Some(b)) => {
                let a = ascii_upper(a);
                let b = ascii_upper(b);
                if a != b {
                    return a.cmp(&b);
                }
            }
            (Some(_), None) => return Ordering::Greater,
            (None, Some(_)) => return Ordering::Less,
            (None, None) => return Ordering::Equal,
        }
    }
}

fn ascii_upper(byte: u8) -> u8 {
    if byte.is_ascii_lowercase() {
        byte - 32
    } else {
        byte
    }
}

fn open_save_dir<D, T>(
    mgr: &VolumeManager<D, T>,
    parent: RawDirectory,
    name: &str,
    create: bool,
) -> Result<RawDirectory, SdError<D::Error>>
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    match mgr.open_dir(parent, name) {
        Ok(dir) => Ok(dir),
        Err(embedded_sdmmc::Error::NotFound) if create => {
            match mgr.make_dir_in_dir(parent, name) {
                Ok(()) | Err(embedded_sdmmc::Error::DirAlreadyExists) => {}
                Err(e) => return Err(SdError::Sdmmc(e)),
            }
            mgr.open_dir(parent, name).map_err(SdError::Sdmmc)
        }
        Err(e) => Err(SdError::Sdmmc(e)),
    }
}

fn write_file<D, T>(
    mgr: &VolumeManager<D, T>,
    dir: RawDirectory,
    name: &str,
    data: &[u8],
) -> Result<(), SdError<D::Error>>
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    let file = mgr.open_file_in_dir(dir, name, Mode::ReadWriteCreateOrTruncate)?;
    let mut result = mgr.write(file, data).map_err(SdError::Sdmmc);
    close_file_if_ok(mgr, file, &mut result);
    result
}

fn read_file_optional<D, T>(
    mgr: &VolumeManager<D, T>,
    dir: RawDirectory,
    name: &str,
) -> Result<Option<Vec<u8>>, SdError<D::Error>>
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    let file = match mgr.open_file_in_dir(dir, name, Mode::ReadOnly) {
        Ok(file) => file,
        Err(embedded_sdmmc::Error::NotFound) => return Ok(None),
        Err(e) => return Err(SdError::Sdmmc(e)),
    };

    let result = read_open_file(mgr, file);
    let mut result = result.map(Some);
    close_file_if_ok(mgr, file, &mut result);
    result
}

fn file_exists<D, T>(
    mgr: &VolumeManager<D, T>,
    dir: RawDirectory,
    name: &str,
) -> Result<bool, SdError<D::Error>>
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    let file = match mgr.open_file_in_dir(dir, name, Mode::ReadOnly) {
        Ok(file) => file,
        Err(embedded_sdmmc::Error::NotFound) => return Ok(false),
        Err(e) => return Err(SdError::Sdmmc(e)),
    };
    let mut result = Ok(true);
    close_file_if_ok(mgr, file, &mut result);
    result
}

fn read_open_file<D, T>(
    mgr: &VolumeManager<D, T>,
    file: RawFile,
) -> Result<Vec<u8>, SdError<D::Error>>
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    let len = mgr.file_length(file)? as usize;
    let mut data = Vec::new();
    data.try_reserve_exact(len)
        .map_err(|_| SdError::OutOfMemory)?;
    data.resize(len, 0);

    let mut total = 0;
    while total < len {
        let read = mgr.read(file, &mut data[total..])?;
        if read == 0 {
            break;
        }
        total += read;
    }
    data.truncate(total);
    Ok(data)
}

fn close_file_if_ok<D, T, R>(
    mgr: &VolumeManager<D, T>,
    file: RawFile,
    result: &mut Result<R, SdError<D::Error>>,
) where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    if result.is_ok() {
        if let Err(e) = mgr.close_file(file) {
            *result = Err(SdError::Sdmmc(e));
        }
    } else {
        let _ = mgr.close_file(file);
    }
}

fn close_dir_if_ok<D, T, R>(
    mgr: &VolumeManager<D, T>,
    dir: RawDirectory,
    result: &mut Result<R, SdError<D::Error>>,
) where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    if result.is_ok() {
        if let Err(e) = mgr.close_dir(dir) {
            *result = Err(SdError::Sdmmc(e));
        }
    } else {
        let _ = mgr.close_dir(dir);
    }
}

fn close_volume_if_ok<D, T, R>(
    mgr: &VolumeManager<D, T>,
    volume: RawVolume,
    result: &mut Result<R, SdError<D::Error>>,
) where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    if result.is_ok() {
        if let Err(e) = mgr.close_volume(volume) {
            *result = Err(SdError::Sdmmc(e));
        }
    } else {
        let _ = mgr.close_volume(volume);
    }
}

// ── Unused-dir helper kept for diagnostic logging ─────────────────────────────

#[allow(dead_code)]
fn log_dir<D, T>(mgr: &VolumeManager<D, T>, dir: RawDirectory, path: &str)
where
    D: BlockDevice,
    D::Error: core::fmt::Debug,
    T: TimeSource,
{
    info!("{}:", path);
    let _ = mgr.iterate_dir(dir, |entry| {
        if entry.attributes.is_directory() {
            info!("  [DIR] {}", defmt::Display2Format(&entry.name));
        } else {
            info!("  {} B  {}", entry.size, defmt::Display2Format(&entry.name));
        }
    });
}
