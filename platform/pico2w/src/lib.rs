#![cfg_attr(target_arch = "arm", no_std)]

extern crate alloc;

#[cfg(target_arch = "arm")]
pub mod audio;
pub mod crash;
#[cfg(target_arch = "arm")]
mod critical_section_impl;
pub mod display;
#[cfg(target_arch = "arm")]
pub mod flash_rom;
pub mod input;
pub mod menu;
#[cfg(target_arch = "arm")]
pub mod multicore;
pub mod save_storage;
#[cfg(target_arch = "arm")]
pub mod sd;
pub mod xip_cartridge;

/// Pure encode/decode helpers for WiFi credential storage and the captive
/// portal.  No platform gate — available on host for unit tests.
pub mod wifi_codec;

/// WiFi captive-portal configuration support.
///
/// ARM-only (depends on the CYW43439 driver + embassy-net), so it is excluded
/// from host test builds but always present in the firmware.
#[cfg(target_arch = "arm")]
pub mod wifi;
