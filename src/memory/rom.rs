use std::fmt;

pub enum Error {
    OutOfRange(u16),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::OutOfRange(address) => write!(f, "Address 0x{:04X} is out of range", address),
        }
    }
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

pub trait ReadOnlyMemory {
    // Read a byte at the specific address.
    // Error out if the address is beyond the allocated memory.
    fn read(&self, address: u16) -> Result<u8, Error>;
}

pub struct ROMVec {
    memory: Vec<u8>
}

impl ReadOnlyMemory for ROMVec {
    fn read(&self, address: u16) -> Result<u8, Error> {
        let index: usize = address as usize;
        if self.memory.len() <= index {
            return Err(Error::OutOfRange(address));
        }
        Ok(self.memory[index])
    }
}

impl ROMVec {
    // Create a ROMVec, initialized with the data provided.
    pub fn new(data: Vec<u8>) -> Self {
        Self { memory: data }
    }
}

/// A fixed-size writable RAM region. Addresses are relative to the start of the region.
pub struct Ram {
    data: Vec<u8>,
}

impl Ram {
    pub fn new(size: usize) -> Self {
        Self { data: vec![0u8; size] }
    }

    pub fn read(&self, address: u16) -> Result<u8, Error> {
        let index = address as usize;
        if index >= self.data.len() {
            return Err(Error::OutOfRange(address));
        }
        Ok(self.data[index])
    }

    pub fn write(&mut self, address: u16, value: u8) -> Result<(), Error> {
        let index = address as usize;
        if index >= self.data.len() {
            return Err(Error::OutOfRange(address));
        }
        self.data[index] = value;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_romvec() {
        let rom_data = vec![0x12, 0x34, 0x56, 0x78];
        let rom = ROMVec::new(rom_data);
        assert_eq!(rom.read(0).unwrap(), 0x12);
        assert_eq!(rom.read(1).unwrap(), 0x34);
        assert_eq!(rom.read(2).unwrap(), 0x56);
        assert_eq!(rom.read(3).unwrap(), 0x78);
        assert!(rom.read(4).is_err());
    }

    #[test]
    fn test_ram_zero_initialized() {
        let ram = Ram::new(256);
        assert_eq!(ram.read(0).unwrap(), 0x00);
        assert_eq!(ram.read(255).unwrap(), 0x00);
    }

    #[test]
    fn test_ram_write_then_read() {
        let mut ram = Ram::new(256);
        ram.write(0x10, 0xAB).unwrap();
        assert_eq!(ram.read(0x10).unwrap(), 0xAB);
    }

    #[test]
    fn test_ram_write_does_not_affect_other_addresses() {
        let mut ram = Ram::new(256);
        ram.write(0x10, 0xAB).unwrap();
        assert_eq!(ram.read(0x11).unwrap(), 0x00);
    }

    #[test]
    fn test_ram_out_of_range_read() {
        let ram = Ram::new(4);
        assert!(ram.read(4).is_err());
    }

    #[test]
    fn test_ram_out_of_range_write() {
        let mut ram = Ram::new(4);
        assert!(ram.write(4, 0xFF).is_err());
    }
}
