use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

use super::opcode::Sbc8;

pub struct Sbc8Decoder;
impl Decoder for Sbc8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            0x98 => Ok(Box::new(Sbc8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0x99 => Ok(Box::new(Sbc8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0x9A => Ok(Box::new(Sbc8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0x9B => Ok(Box::new(Sbc8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0x9C => Ok(Box::new(Sbc8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0x9D => Ok(Box::new(Sbc8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0x9E => Ok(Box::new(Sbc8 { operand: Operand::Memory(Memory::HL), cycles: 8 })),
            0x9F => Ok(Box::new(Sbc8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            0xDE => Ok(Box::new(Sbc8 { operand: Operand::Imm8, cycles: 8 })),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_sbc8_b() {
        let opcode = 0x98;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sbc8_c() {
        let opcode = 0x99;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sbc8_d() {
        let opcode = 0x9A;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sbc8_e() {
        let opcode = 0x9B;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sbc8_h() {
        let opcode = 0x9C;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sbc8_l() {
        let opcode = 0x9D;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sbc8_hl() {
        let opcode = 0x9E;
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sbc8_a() {
        let opcode = 0x9F;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sbc8_imm8() {
        let opcode = 0xDE;
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;

        FakeCpu::new().test_decode_operand(opcode, &Sbc8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_invalid_opcode_sbc8() {
        let opcode = 0xFF; // Example invalid opcode for Sbc8
        assert!(Sbc8Decoder{}.decode(opcode).is_err());
    }
}