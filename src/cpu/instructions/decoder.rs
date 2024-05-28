use std::fmt;
use crate::cpu::instructions::opcode::OpCode;

#[derive(Debug, PartialEq)]
pub enum Error {
    /// Indicates that the binary opcode is invalid.
    /// The invalid opcode is the value of the enum.
    InvalidOpcode(u8),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidOpcode(opcode) => write!(f, "Opcode {} is not supported.", opcode),
        }
    }
}

pub trait Decoder {
    /// Serialize the given binary opcode into an OpCode type.
    /// Errors out if the given opcode cannot be decoded.
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let opcode = 0xba;
        let error = Error::InvalidOpcode(opcode);
        assert!(format!("{}", error).contains(&format!("{}", opcode)));
    }
}
