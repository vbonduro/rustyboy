use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::Register8;

/// The 8 register targets for CB-prefix instructions.
/// HLMem means read/write through (HL).
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum CbTarget {
    Reg(Register8),
    HLMem,
}

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum CbOp {
    Rlc,
    Rrc,
    Rl,
    Rr,
    Sla,
    Sra,
    Swap,
    Srl,
    Bit(u8),
    Res(u8),
    Set(u8),
}

pub struct CbInstruction {
    pub op: CbOp,
    pub target: CbTarget,
    pub cycles: u8,
}

impl OpCode for CbInstruction {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.cb(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_dispatches_to_cb() {
        let opcode = CbInstruction {
            op: CbOp::Rlc,
            target: CbTarget::Reg(Register8::B),
            cycles: 8,
        };
        assert_eq!(opcode.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_execute_hl_mem_dispatches() {
        let opcode = CbInstruction {
            op: CbOp::Bit(3),
            target: CbTarget::HLMem,
            cycles: 12,
        };
        assert_eq!(opcode.execute(&mut FakeCpu::new()).unwrap(), 12);
    }
}
