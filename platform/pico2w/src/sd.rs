use defmt::info;
use embedded_sdmmc::{
    BlockDevice, Mode, RawDirectory, RawFile, RawVolume, ShortFileName, TimeSource, VolumeIdx,
    VolumeManager,
};

use rustyboy_core::memory::RomReader;

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
    NoRomFound,
}

impl<E: core::fmt::Debug> From<embedded_sdmmc::Error<E>> for SdError<E> {
    fn from(e: embedded_sdmmc::Error<E>) -> Self {
        SdError::Sdmmc(e)
    }
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
    /// plus a flag indicating whether more entries follow.
    ///
    /// Iterates the full root directory each call (up to 100 entries) and
    /// slices the requested page. Stack usage: ~1.1 KB for the temporary array.
    pub fn list_rom_page(
        &self,
        page_offset: usize,
        page_size: usize,
    ) -> Result<(heapless::Vec<heapless::String<64>, 7>, bool), SdError<D::Error>> {
        let volume = self.mgr.open_raw_volume(VolumeIdx(0))?;
        let root = self.mgr.open_root_dir(volume)?;

        let mut all: heapless::Vec<ShortFileName, 100> = heapless::Vec::new();
        let _ = self.mgr.iterate_dir(root, |entry| {
            if !entry.attributes.is_directory() && is_rom_file(&entry.name) && all.len() < 100 {
                let _ = all.push(entry.name.clone());
            }
        });
        let _ = self.mgr.close_dir(root);
        let _ = self.mgr.close_volume(volume);

        let total = all.len();
        let start = page_offset.min(total);
        let has_next = start + page_size < total;
        let page: heapless::Vec<heapless::String<64>, 7> = all[start..]
            .iter()
            .take(page_size)
            .map(sfn_to_string)
            .collect();

        Ok((page, has_next))
    }

    /// Open a specific ROM file by display name (case-insensitive).
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
