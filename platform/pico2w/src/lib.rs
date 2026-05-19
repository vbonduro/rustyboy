#![cfg_attr(target_arch = "arm", no_std)]

extern crate alloc;

#[cfg(target_arch = "arm")]
pub mod audio;
pub mod display;
pub mod menu;
#[cfg(target_arch = "arm")]
pub mod flash_rom;
pub mod input;
#[cfg(target_arch = "arm")]
pub mod multicore;
#[cfg(target_arch = "arm")]
pub mod sd;
#[cfg(target_arch = "arm")]
pub mod stack_probe;
pub mod xip_cartridge;
