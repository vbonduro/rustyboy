use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;
use crate::cpu::instructions::instructions::{Error, Instructions};

/// Adds the value of the operand to the accumulator register (A).
pub struct Add8 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for Add8 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.add8(&self)
    }
}

/// Adds the value of the operand to the 16-bit HL register.
pub struct Add16 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for Add16 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.add16(&self)
    }
}

/// Adds the value of the operand to the stack pointer (SP).
pub struct AddSP16 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for AddSP16 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.add_sp16(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_add8_b() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add8_c() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add8_d() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add8_e() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add8_h() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add8_l() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add8_hl() {
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add8_a() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add8_imm8() {
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;
        let opcode = Add8{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add16_bc() {
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::BC);
        let opcode = Add16{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add16_de() {
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::DE);
        let opcode = Add16{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add16_hl() {
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::HL);
        let opcode = Add16{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_add16_sp() {
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::SP);
        let opcode = Add16{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_addsp_imm_signed8() {
        let expected_cycles = 16;
        let expected_operand = Operand::ImmSigned8;
        let opcode = AddSP16{operand: expected_operand, cycles: expected_cycles};

        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }
}
