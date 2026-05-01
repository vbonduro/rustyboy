use defmt::{info, warn};
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

// ── SdRomReader ───────────────────────────────────────────────────────────────

pub struct SdRomReader<D, T = DummyClock>
where
    D: BlockDevice,
    <D as BlockDevice>::Error: core::fmt::Debug,
    T: TimeSource,
{
    mgr:    VolumeManager<D, T>,
    volume: RawVolume,
    file:   RawFile,
}

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

impl<D, T> SdRomReader<D, T>
where
    D: BlockDevice,
    <D as BlockDevice>::Error: core::fmt::Debug,
    T: TimeSource,
{
    /// Mount the first partition, search root for a `.gb` or `.gbc` file, and
    /// open it for sequential bank reads.
    pub fn new(mgr: VolumeManager<D, T>) -> Result<Self, SdError<D::Error>> {
        let volume = mgr.open_raw_volume(VolumeIdx(0))?;
        let root   = mgr.open_root_dir(volume)?;

        if let Some(file) = find_rom_in_dir(&mgr, root)? {
            let _ = mgr.close_dir(root);
            return Ok(Self { mgr, volume, file });
        }

        // Nothing found — log card contents before returning the error.
        warn!("no .gb/.gbc file found; listing card contents");
        log_dir(&mgr, root, "/");
        let _ = mgr.close_dir(root);
        Err(SdError::NoRomFound)
    }
}

impl<D, T> Drop for SdRomReader<D, T>
where
    D: BlockDevice,
    <D as BlockDevice>::Error: core::fmt::Debug,
    T: TimeSource,
{
    fn drop(&mut self) {
        let _ = self.mgr.close_file(self.file);
        let _ = self.mgr.close_volume(self.volume);
    }
}

impl<D, T> RomReader for SdRomReader<D, T>
where
    D: BlockDevice,
    <D as BlockDevice>::Error: core::fmt::Debug,
    T: TimeSource,
{
    type Error = SdError<D::Error>;

    fn read_bank(&mut self, bank: usize, buf: &mut [u8; 0x4000]) -> Result<(), Self::Error> {
        let offset = (bank as u32) * 0x4000;
        self.mgr.file_seek_from_start(self.file, offset)?;
        let mut total = 0;
        while total < 0x4000 {
            let n = self.mgr.read(self.file, &mut buf[total..])?;
            if n == 0 { break; }
            total += n;
        }
        Ok(())
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn find_rom_in_dir<D, T>(
    mgr: &VolumeManager<D, T>,
    dir: RawDirectory,
) -> Result<Option<RawFile>, SdError<D::Error>>
where
    D: BlockDevice,
    <D as BlockDevice>::Error: core::fmt::Debug,
    T: TimeSource,
{
    let mut found: Option<ShortFileName> = None;
    mgr.iterate_dir(dir, |entry| {
        if found.is_none() && !entry.attributes.is_directory() && is_rom_file(&entry.name) {
            found = Some(entry.name.clone());
        }
    })?;
    match found {
        Some(name) => Ok(Some(mgr.open_file_in_dir(dir, &name, Mode::ReadOnly)?)),
        None => Ok(None),
    }
}

fn is_rom_file(name: &ShortFileName) -> bool {
    let ext = name.extension();
    ext == b"GB" || ext == b"GBC"
}

/// Log every entry in `dir` at INFO level.  Used only when no ROM is found.
fn log_dir<D, T>(mgr: &VolumeManager<D, T>, dir: RawDirectory, path: &str)
where
    D: BlockDevice,
    <D as BlockDevice>::Error: core::fmt::Debug,
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
