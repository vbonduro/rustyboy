//! WiFi credential flash storage.
//!
//! # Sector format (4 KiB at [`WIFI_CONFIG_OFFSET`])
//!
//! ```text
//! [0..4]    magic b"WIFY"
//! [4..36]   SSID  (32 bytes, null-padded or full 32 bytes when length = 32)
//! [36..100] password (64 bytes, null-padded or full 64 bytes when length = 64)
//! [100..4096] 0xFF (unused)
//! ```
//!
//! The pattern mirrors `crash::storage` — erase the sector first, then write.

use embassy_rp::flash::{Error as FlashError, ERASE_SIZE};

use crate::flash_rom::{OnboardFlash, WIFI_CONFIG_OFFSET};
use crate::wifi_codec::{decode_wifi_config, encode_wifi_config, WIFI_CONFIG_BUF_LEN};

/// Credentials loaded from the WiFi config flash sector.
pub struct WifiConfig {
    pub ssid: heapless::String<32>,
    pub password: heapless::String<64>,
}

impl WifiConfig {
    /// Read and validate the WiFi config from flash.
    ///
    /// Returns `None` if the sector is blank or the magic is absent/corrupt.
    pub fn load(flash: &mut OnboardFlash<'_>) -> Option<Self> {
        let mut buf = [0u8; WIFI_CONFIG_BUF_LEN];
        flash
            .blocking_read(WIFI_CONFIG_OFFSET as u32, &mut buf)
            .ok()?;

        let (ssid, password) = decode_wifi_config(&buf)?;
        Some(Self { ssid, password })
    }

    /// Erase the WiFi config sector and write new credentials.
    ///
    /// # Errors
    /// Returns `FlashError` on hardware fault.
    pub fn save(
        flash: &mut OnboardFlash<'_>,
        ssid: &str,
        password: &str,
    ) -> Result<(), FlashError> {
        // Erase sector first (NOR flash: bits can only go 1→0; erase resets to all-1).
        flash.blocking_erase(
            WIFI_CONFIG_OFFSET as u32,
            (WIFI_CONFIG_OFFSET + ERASE_SIZE) as u32,
        )?;

        let mut buf = [0xFFu8; WIFI_CONFIG_BUF_LEN];
        encode_wifi_config(ssid, password, &mut buf);

        flash.blocking_write(WIFI_CONFIG_OFFSET as u32, &buf)?;
        Ok(())
    }

    /// Erase the WiFi config sector, removing stored credentials.
    ///
    /// After this call, [`WifiConfig::load`] returns `None`.
    pub fn erase(flash: &mut OnboardFlash<'_>) -> Result<(), FlashError> {
        flash.blocking_erase(
            WIFI_CONFIG_OFFSET as u32,
            (WIFI_CONFIG_OFFSET + ERASE_SIZE) as u32,
        )?;
        // Write a zeroed magic so load() always returns None even if erase
        // somehow leaves a valid-looking pattern.
        let zero_magic = [0u8; 4];
        flash
            .blocking_write(WIFI_CONFIG_OFFSET as u32, &zero_magic)
            .ok();
        Ok(())
    }
}
