use super::opcode::OpCode;
use super::operand::*;

use crate::cpu::cpu::{Cpu, Error};

/// Adds the value of the operand and the carry flag to the accumulator register (A).
pub struct Adc {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for Adc {
    fn execute(&self, cpu: &mut dyn Cpu) -> Result<u8, Error> {
        cpu.adc(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cpu::opcodes::test_util::operand_test_util::FakeCpu;

    #[test]
    fn test_execute_adc_b() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_adc_c() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_adc_d() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_adc_e() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_adc_h() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_adc_l() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_adc_a() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_adc_memhl() {
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_adc_imm8() {
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;
        let opcode = Adc{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }
}
