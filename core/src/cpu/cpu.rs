use core::fmt;

use super::instructions::decoder::Error as DecoderError;
use super::instructions::instructions::Error as InstructionError;
use crate::memory::memory::Error as MemoryError;

#[derive(Debug)]
pub enum CpuError {
    Decode(DecoderError),
    Instruction(InstructionError),
    Memory(MemoryError),
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CpuError::Decode(e) => write!(f, "Decode error: {}", e),
            CpuError::Instruction(e) => write!(f, "Instruction error: {}", e),
            CpuError::Memory(e) => write!(f, "Memory error: {}", e),
        }
    }
}

impl From<DecoderError> for CpuError {
    fn from(e: DecoderError) -> Self {
        CpuError::Decode(e)
    }
}

impl From<InstructionError> for CpuError {
    fn from(e: InstructionError) -> Self {
        CpuError::Instruction(e)
    }
}

impl From<MemoryError> for CpuError {
    fn from(e: MemoryError) -> Self {
        CpuError::Memory(e)
    }
}

pub trait Cpu {
    // Read the next instruction from the program and execute it.
    // Returns the number of ticks from the instruction.
    fn tick(&mut self) -> Result<u8, CpuError>;
}
