use super::adc::opcode::Adc;
use super::add::opcode::{Add16, Add8, AddSP16};
use super::call::opcode::Call;
use super::cb::opcode::CbInstruction;
use super::cp::opcode::Cp8;
use super::inc_dec::opcode::{Dec16, Dec8, Inc16, Inc8};
use super::jump::opcode::Jump;
use super::ld::opcode::Ld8;
use super::ld16::opcode::Ld16;
use super::logic::opcode::{And8, Or8, Xor8};
use super::misc::opcode::Misc;
use super::ret::opcode::Ret;
use super::rotate::opcode::Rotate;
use super::rst::opcode::Rst;
use super::sbc::opcode::Sbc8;
use super::stack::opcode::{Pop16, Push16};
use super::sub::opcode::Sub8;
use crate::memory::memory::GameBoyMemory;
use alloc::string::String;
use core::fmt;

#[derive(Debug, PartialEq)]
pub enum Error {
    /// Indicates that the operand is invalid for the given opcode.
    InvalidOperand(String),
    /// THe instruction failed to execute.
    Failed(String),
}

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
    // Bus-touching instructions receive memory as a parameter.
    fn add8(&mut self, opcode: &Add8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn add_sp16(&mut self, opcode: &AddSP16, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn adc(&mut self, opcode: &Adc, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn sub8(&mut self, opcode: &Sub8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn sbc8(&mut self, opcode: &Sbc8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn cp8(&mut self, opcode: &Cp8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn ld8(&mut self, opcode: &Ld8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn ld16(&mut self, opcode: &Ld16, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn inc8(&mut self, opcode: &Inc8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn dec8(&mut self, opcode: &Dec8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn and8(&mut self, opcode: &And8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn or8(&mut self, opcode: &Or8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn xor8(&mut self, opcode: &Xor8, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn jump(&mut self, opcode: &Jump, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn push16(&mut self, opcode: &Push16, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn pop16(&mut self, opcode: &Pop16, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn call(&mut self, opcode: &Call, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn ret(&mut self, opcode: &Ret, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn rst(&mut self, opcode: &Rst, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    fn cb(&mut self, opcode: &CbInstruction, memory: &mut GameBoyMemory) -> Result<u8, Error>;
    // Pure register — no memory access.
    fn add16(&mut self, opcode: &Add16) -> Result<u8, Error>;
    fn inc16(&mut self, opcode: &Inc16) -> Result<u8, Error>;
    fn dec16(&mut self, opcode: &Dec16) -> Result<u8, Error>;
    fn rotate_accumulator(&mut self, opcode: &Rotate) -> Result<u8, Error>;
    fn misc(&mut self, opcode: &Misc) -> Result<u8, Error>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{format, string::ToString};

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
