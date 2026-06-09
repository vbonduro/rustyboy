#![cfg_attr(target_arch = "arm", no_std)]

extern crate alloc;

#[cfg(target_arch = "arm")]
pub mod audio;
pub mod crash;
pub mod display;
#[cfg(target_arch = "arm")]
pub mod dwt_watch;
#[cfg(target_arch = "arm")]
pub mod flash_rom;
/// Redzone global-allocator wrapper for catching heap overruns (CRASH_DEBUG_NOTES #5).
/// ARM-only (wraps `embedded_alloc::Heap`); selected as the `#[global_allocator]`
/// in `main.rs` under the `heap-guard` feature.
#[cfg(target_arch = "arm")]
pub mod guarded_heap;
pub mod input;
pub mod menu;
#[cfg(target_arch = "arm")]
pub mod multicore;
pub mod save_storage;
#[cfg(target_arch = "arm")]
pub mod sd;
/// `-Z stack-protector=all` runtime support (`__stack_chk_guard`/`_fail`) for
/// the #5 stack-overrun hunt (CRASH_DEBUG_NOTES). Symbols are only referenced
/// when the firmware is built with that flag.
#[cfg(target_arch = "arm")]
pub mod stack_chk;
#[cfg(target_arch = "arm")]
pub mod stack_probe;
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
