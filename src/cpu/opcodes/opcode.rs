use crate::cpu::cpu::{Cpu, Error};

pub trait OpCode {
    /// Execute this opcode type with the provided CPU implementation.
    /// This uses double dispatch to translate the concrete OpCode type to
    /// the respective function of the Cpu trait.
    /// Returns the number of cycles to execute the OpCode.
    fn execute(&self, cpu: &mut dyn Cpu) -> Result<u8, Error>;
}
