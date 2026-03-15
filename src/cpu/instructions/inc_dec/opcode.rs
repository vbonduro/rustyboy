use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

/// Increments an 8-bit register or memory location at (HL).
/// Affects Z (set if result is 0), N=0, H (set if carry from bit 3). C is NOT affected.
pub struct Inc8 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for Inc8 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.inc8(self)
    }
}

/// Decrements an 8-bit register or memory location at (HL).
/// Affects Z (set if result is 0), N=1, H (set if borrow from bit 4). C is NOT affected.
pub struct Dec8 {
    pub operand: Operand,
    pub cycles: u8,
}

impl OpCode for Dec8 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.dec8(self)
    }
}

/// Increments a 16-bit register pair. NO flags affected.
pub struct Inc16 {
    pub operand: Register16,
    pub cycles: u8,
}

impl OpCode for Inc16 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.inc16(self)
    }
}

/// Decrements a 16-bit register pair. NO flags affected.
pub struct Dec16 {
    pub operand: Register16,
    pub cycles: u8,
}

impl OpCode for Dec16 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.dec16(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    // --- Inc8 tests ---

    #[test]
    fn test_execute_inc8_b() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);
        let opcode = Inc8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_inc8_c() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);
        let opcode = Inc8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_inc8_d() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);
        let opcode = Inc8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_inc8_e() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);
        let opcode = Inc8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_inc8_h() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);
        let opcode = Inc8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_inc8_l() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);
        let opcode = Inc8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_inc8_a() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);
        let opcode = Inc8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_inc8_mem_hl() {
        let expected_cycles = 12;
        let expected_operand = Operand::Memory(Memory::HL);
        let opcode = Inc8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    // --- Dec8 tests ---

    #[test]
    fn test_execute_dec8_b() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);
        let opcode = Dec8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_dec8_c() {
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);
        let opcode = Dec8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    #[test]
    fn test_execute_dec8_mem_hl() {
        let expected_cycles = 12;
        let expected_operand = Operand::Memory(Memory::HL);
        let opcode = Dec8 {
            operand: expected_operand,
            cycles: expected_cycles,
        };
        FakeCpu::new().test_execute_opcode(&opcode, expected_cycles, expected_operand);
    }

    // --- Inc16 tests ---

    #[test]
    fn test_execute_inc16_bc() {
        let expected_cycles = 8;
        let opcode = Inc16 {
            operand: Register16::BC,
            cycles: expected_cycles,
        };
        let mut cpu = FakeCpu::new();
        let actual_cycles = opcode.execute(&mut cpu).unwrap();
        assert_eq!(actual_cycles, expected_cycles);
    }

    #[test]
    fn test_execute_inc16_de() {
        let expected_cycles = 8;
        let opcode = Inc16 {
            operand: Register16::DE,
            cycles: expected_cycles,
        };
        let mut cpu = FakeCpu::new();
        let actual_cycles = opcode.execute(&mut cpu).unwrap();
        assert_eq!(actual_cycles, expected_cycles);
    }

    #[test]
    fn test_execute_inc16_hl() {
        let expected_cycles = 8;
        let opcode = Inc16 {
            operand: Register16::HL,
            cycles: expected_cycles,
        };
        let mut cpu = FakeCpu::new();
        let actual_cycles = opcode.execute(&mut cpu).unwrap();
        assert_eq!(actual_cycles, expected_cycles);
    }

    #[test]
    fn test_execute_inc16_sp() {
        let expected_cycles = 8;
        let opcode = Inc16 {
            operand: Register16::SP,
            cycles: expected_cycles,
        };
        let mut cpu = FakeCpu::new();
        let actual_cycles = opcode.execute(&mut cpu).unwrap();
        assert_eq!(actual_cycles, expected_cycles);
    }

    // --- Dec16 tests ---

    #[test]
    fn test_execute_dec16_bc() {
        let expected_cycles = 8;
        let opcode = Dec16 {
            operand: Register16::BC,
            cycles: expected_cycles,
        };
        let mut cpu = FakeCpu::new();
        let actual_cycles = opcode.execute(&mut cpu).unwrap();
        assert_eq!(actual_cycles, expected_cycles);
    }

    #[test]
    fn test_execute_dec16_sp() {
        let expected_cycles = 8;
        let opcode = Dec16 {
            operand: Register16::SP,
            cycles: expected_cycles,
        };
        let mut cpu = FakeCpu::new();
        let actual_cycles = opcode.execute(&mut cpu).unwrap();
        assert_eq!(actual_cycles, expected_cycles);
    }
}
