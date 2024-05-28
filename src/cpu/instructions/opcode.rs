use crate::cpu::instructions::instructions::{Error, Instructions};

pub trait OpCode {
    /// Execute this opcode type with the provided CPU Instruction implementation.
    /// This uses double dispatch to translate the concrete OpCode type to
    /// the respective function of the Instructions trait.
    /// Returns the number of cycles to execute the OpCode.
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error>;
}
