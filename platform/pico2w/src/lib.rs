#![cfg_attr(target_arch = "arm", no_std)]

pub mod display;
pub mod input;
#[cfg(target_arch = "arm")]
pub mod sd;
