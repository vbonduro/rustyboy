use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::Register16;
use crate::memory::memory::GameBoyMemory;

pub struct Push16 {
    pub operand: Register16,
    pub cycles: u8,
}

impl OpCode for Push16 {
    fn execute(&self, cpu: &mut dyn Instructions, memory: &mut GameBoyMemory) -> Result<u8, Error> {
        cpu.push16(self, memory)
    }
}

pub struct Pop16 {
    pub operand: Register16,
    pub cycles: u8,
}

impl OpCode for Pop16 {
    fn execute(&self, cpu: &mut dyn Instructions, memory: &mut GameBoyMemory) -> Result<u8, Error> {
        cpu.pop16(self, memory)
    }
}
