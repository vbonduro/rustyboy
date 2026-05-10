use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::memory::memory::GameBoyMemory;

pub trait OpCode: Send + Sync {
    /// Execute this opcode type with the provided CPU Instruction implementation.
    /// This uses double dispatch to translate the concrete OpCode type to
    /// the respective function of the Instructions trait.
    /// Returns the number of cycles to execute the OpCode.
    fn execute(&self, cpu: &mut dyn Instructions, memory: &mut GameBoyMemory) -> Result<u8, Error>;
}
