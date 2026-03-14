use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;
use crate::cpu::instructions::instructions::{Error, Instructions};

/// Loads the value of src into dest.
pub struct Ld8 {
    pub dest: Operand,
    pub src: Operand,
    pub cycles: u8,
}

impl OpCode for Ld8 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.ld8(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_ld8_reg_to_reg() {
        let expected_cycles = 4;
        let dest = Operand::Register8(Register8::B);
        let src = Operand::Register8(Register8::C);
        let opcode = Ld8 { dest, src, cycles: expected_cycles };

        FakeCpu::new().test_execute_ld8_opcode(&opcode, expected_cycles, dest, src);
    }

    #[test]
    fn test_execute_ld8_mem_hl_to_reg() {
        let expected_cycles = 8;
        let dest = Operand::Register8(Register8::A);
        let src = Operand::Memory(Memory::HL);
        let opcode = Ld8 { dest, src, cycles: expected_cycles };

        FakeCpu::new().test_execute_ld8_opcode(&opcode, expected_cycles, dest, src);
    }

    #[test]
    fn test_execute_ld8_reg_to_mem_hl() {
        let expected_cycles = 8;
        let dest = Operand::Memory(Memory::HL);
        let src = Operand::Register8(Register8::A);
        let opcode = Ld8 { dest, src, cycles: expected_cycles };

        FakeCpu::new().test_execute_ld8_opcode(&opcode, expected_cycles, dest, src);
    }

    #[test]
    fn test_execute_ld8_imm8_to_reg() {
        let expected_cycles = 8;
        let dest = Operand::Register8(Register8::B);
        let src = Operand::Imm8;
        let opcode = Ld8 { dest, src, cycles: expected_cycles };

        FakeCpu::new().test_execute_ld8_opcode(&opcode, expected_cycles, dest, src);
    }

    #[test]
    fn test_execute_ld8_imm8_to_mem_hl() {
        let expected_cycles = 12;
        let dest = Operand::Memory(Memory::HL);
        let src = Operand::Imm8;
        let opcode = Ld8 { dest, src, cycles: expected_cycles };

        FakeCpu::new().test_execute_ld8_opcode(&opcode, expected_cycles, dest, src);
    }
}
