pub mod cartridge;
#[cfg(test)]
pub mod fake;
pub mod memory;
pub mod rom;

#[cfg(test)]
pub use fake::FakeMemory;
pub use memory::{GameBoyMemory, Memory};
pub use rom::{ROMVec, Ram, ReadOnlyMemory};
