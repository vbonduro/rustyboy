use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;
use crate::cpu::instructions::instructions::{Error, Instructions};

/// Subtracts the value of the operand from the accumulator register (A).
pub struct Sub8 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for Sub8 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.sub8(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_sub8_b() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_sub8_c() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_sub8_d() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_sub8_e() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_sub8_h() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_sub8_l() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_sub8_hl() {
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_sub8_a() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_sub8_imm8() {
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;
        let opcode = Sub8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }
}