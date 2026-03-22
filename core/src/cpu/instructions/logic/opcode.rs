use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

/// AND A, operand — bitwise AND of A with operand, result stored in A.
/// Flags: Z = (result == 0), N = 0, H = 1, C = 0
pub struct And8 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for And8 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.and8(&self)
    }
}

/// OR A, operand — bitwise OR of A with operand, result stored in A.
/// Flags: Z = (result == 0), N = 0, H = 0, C = 0
pub struct Or8 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for Or8 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.or8(&self)
    }
}

/// XOR A, operand — bitwise XOR of A with operand, result stored in A.
/// Flags: Z = (result == 0), N = 0, H = 0, C = 0
pub struct Xor8 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for Xor8 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.xor8(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    // AND tests
    #[test]
    fn test_execute_and8_b() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_and8_c() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_and8_d() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_and8_e() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_and8_h() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_and8_l() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_and8_mem_hl() {
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_and8_a() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_and8_imm8() {
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;
        let opcode = And8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    // OR tests
    #[test]
    fn test_execute_or8_b() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);
        let opcode = Or8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_or8_c() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);
        let opcode = Or8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_or8_mem_hl() {
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);
        let opcode = Or8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_or8_imm8() {
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;
        let opcode = Or8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    // XOR tests
    #[test]
    fn test_execute_xor8_b() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);
        let opcode = Xor8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_xor8_a() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);
        let opcode = Xor8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_xor8_mem_hl() {
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);
        let opcode = Xor8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_xor8_imm8() {
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;
        let opcode = Xor8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }
}
