use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

use super::opcode::Cp8;

pub struct Cp8Decoder;
impl Decoder for Cp8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            0xB8 => Ok(Box::new(Cp8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0xB9 => Ok(Box::new(Cp8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0xBA => Ok(Box::new(Cp8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0xBB => Ok(Box::new(Cp8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0xBC => Ok(Box::new(Cp8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0xBD => Ok(Box::new(Cp8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0xBE => Ok(Box::new(Cp8 { operand: Operand::Memory(Memory::HL), cycles: 8 })),
            0xBF => Ok(Box::new(Cp8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            0xFE => Ok(Box::new(Cp8 { operand: Operand::Imm8, cycles: 8 })),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_cp8_b() {
        let opcode = 0xB8;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_cp8_c() {
        let opcode = 0xB9;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_cp8_d() {
        let opcode = 0xBA;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_cp8_e() {
        let opcode = 0xBB;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_cp8_h() {
        let opcode = 0xBC;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_cp8_l() {
        let opcode = 0xBD;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_cp8_hl() {
        let opcode = 0xBE;
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_cp8_a() {
        let opcode = 0xBF;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_cp8_imm8() {
        let opcode = 0xFE;
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;

        FakeCpu::new().test_decode_operand(opcode, &Cp8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_invalid_opcode_cp8() {
        let opcode = 0xFF; // Example invalid opcode for Cp8
        assert!(Cp8Decoder{}.decode(opcode).is_err());
    }
}