pub mod cartridge;
#[cfg(test)]
pub mod fake;
pub mod memory;
pub mod rom;
pub mod streaming;

#[cfg(test)]
pub use fake::FakeMemory;
pub use memory::{GameBoyMemory, Memory};
pub use rom::{ROMVec, ReadOnlyMemory};
pub use streaming::{RomReader, StreamingCartridge, StreamingError};
