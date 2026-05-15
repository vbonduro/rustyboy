#![no_std]
extern crate alloc;

pub mod cpu;
pub mod gameboy;
pub mod ipc;
pub mod memory;

pub use gameboy::GameBoy;
