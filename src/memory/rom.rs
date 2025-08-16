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

pub trait ReadOnlyMemory {
    // Read a byte of the ROM at the specific address.
    // Error out if the address is beyond the allocated memory.
    fn read(&self, address: u16) -> Result<u8, Error>;
}

impl std::fmt::Debug for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        fmt::Display::fmt(self, f)
    }
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
}
