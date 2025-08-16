use std::fmt;
use super::add::opcode::{Add16, Add8, AddSP16};
use super::adc::opcode::Adc;
use super::sub::opcode::Sub8;

#[derive(Debug, PartialEq)]
pub enum Error {
    /// Indicates that the operand is invalid for the given opcode.
    InvalidOperand(String),
    /// THe instruction failed to execute.
    Failed(String),
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidOperand(error_string) => write!(f, "Invalid operand: {}", error_string),
            Error::Failed(error_string) => write!(f, "Instruction failed: {}", error_string),
        }
    }
}
/// This trait includes all of the various instructions that a Gameboy CPU must implement.
pub trait Instructions {
    fn add8(&mut self, opcode: &Add8) -> Result<u8, Error>;
    fn add16(&mut self, opcode: &Add16) -> Result<u8, Error>;
    fn add_sp16(&mut self, opcode: &AddSP16) -> Result<u8, Error>;
    fn adc(&mut self, opcode: &Adc) -> Result<u8, Error>;
    fn sub8(&mut self, opcode: &Sub8) -> Result<u8, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_invalid_op_display() {
        let operand = "add8";
        let error = Error::InvalidOperand(operand.to_string());
        assert!(format!("{}", error).contains(&operand));
    }

    #[test]
    fn test_error_failed_display() {
        let reason = "hotdogs";
        let error = Error::Failed(reason.to_string());
        assert!(format!("{}", error).contains(&reason));
    }
}
