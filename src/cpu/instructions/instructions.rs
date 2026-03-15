use std::fmt;
use super::add::opcode::{Add16, Add8, AddSP16};
use super::adc::opcode::Adc;
use super::sub::opcode::Sub8;
use super::sbc::opcode::Sbc8;
use super::cp::opcode::Cp8;
use super::ld::opcode::Ld8;
use super::ld16::opcode::Ld16;
use super::inc_dec::opcode::{Inc8, Dec8, Inc16, Dec16};
use super::rotate::opcode::Rotate;
use super::logic::opcode::{And8, Or8, Xor8};
use super::jump::opcode::Jump;
use super::misc::opcode::Misc;

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
    fn sbc8(&mut self, opcode: &Sbc8) -> Result<u8, Error>;
    fn cp8(&mut self, opcode: &Cp8) -> Result<u8, Error>;
    fn ld8(&mut self, opcode: &Ld8) -> Result<u8, Error>;
    fn ld16(&mut self, opcode: &Ld16) -> Result<u8, Error>;
    fn inc8(&mut self, opcode: &Inc8) -> Result<u8, Error>;
    fn dec8(&mut self, opcode: &Dec8) -> Result<u8, Error>;
    fn inc16(&mut self, opcode: &Inc16) -> Result<u8, Error>;
    fn dec16(&mut self, opcode: &Dec16) -> Result<u8, Error>;
    fn rotate_accumulator(&mut self, opcode: &Rotate) -> Result<u8, Error>;
    fn and8(&mut self, opcode: &And8) -> Result<u8, Error>;
    fn or8(&mut self, opcode: &Or8) -> Result<u8, Error>;
    fn xor8(&mut self, opcode: &Xor8) -> Result<u8, Error>;
    fn jump(&mut self, opcode: &Jump) -> Result<u8, Error>;
    fn misc(&mut self, opcode: &Misc) -> Result<u8, Error>;
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
