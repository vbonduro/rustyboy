#![cfg_attr(target_arch = "arm", no_std)]

#[cfg(target_arch = "arm")]
pub mod audio;
pub mod display;
pub mod input;
#[cfg(target_arch = "arm")]
pub mod sd;
