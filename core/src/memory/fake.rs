extern crate std;
use std::collections::HashMap;

use super::memory::{Error, Memory};

/// A simple memory implementation backed by a HashMap, useful for injecting
/// controlled memory state in CPU tests.
pub struct FakeMemory {
    data: HashMap<u16, u8>,
}

impl FakeMemory {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
        }
    }

    /// Pre-populate an address with a value.
    pub fn set(&mut self, address: u16, value: u8) {
        self.data.insert(address, value);
    }
}

impl Memory for FakeMemory {
    fn read(&self, address: u16) -> Result<u8, Error> {
        Ok(*self.data.get(&address).unwrap_or(&0))
    }

    fn write(&mut self, address: u16, value: u8) -> Result<(), Error> {
        self.data.insert(address, value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fake_memory_defaults_to_zero() {
        let mem = FakeMemory::new();
        assert_eq!(mem.read(0x1234).unwrap(), 0x00);
    }

    #[test]
    fn test_fake_memory_set_then_read() {
        let mut mem = FakeMemory::new();
        mem.set(0xC000, 0x55);
        assert_eq!(mem.read(0xC000).unwrap(), 0x55);
    }

    #[test]
    fn test_fake_memory_write_then_read() {
        let mut mem = FakeMemory::new();
        mem.write(0x0001, 0xFF).unwrap();
        assert_eq!(mem.read(0x0001).unwrap(), 0xFF);
    }

    #[test]
    fn test_fake_memory_unset_addresses_return_zero() {
        let mut mem = FakeMemory::new();
        mem.set(0x0000, 0x01);
        assert_eq!(mem.read(0x0001).unwrap(), 0x00);
    }
}
