use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

use super::opcode::Sub8;

pub struct Sub8Decoder;
impl Decoder for Sub8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            0x90 => Ok(Box::new(Sub8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0x91 => Ok(Box::new(Sub8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0x92 => Ok(Box::new(Sub8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0x93 => Ok(Box::new(Sub8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0x94 => Ok(Box::new(Sub8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0x95 => Ok(Box::new(Sub8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0x96 => Ok(Box::new(Sub8 { operand: Operand::Memory(Memory::HL), cycles: 8 })),
            0x97 => Ok(Box::new(Sub8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            0xD6 => Ok(Box::new(Sub8 { operand: Operand::Imm8, cycles: 8 })),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_sub8_b() {
        let opcode = 0x90;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sub8_c() {
        let opcode = 0x91;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sub8_d() {
        let opcode = 0x92;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sub8_e() {
        let opcode = 0x93;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sub8_h() {
        let opcode = 0x94;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sub8_l() {
        let opcode = 0x95;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sub8_hl() {
        let opcode = 0x96;
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sub8_a() {
        let opcode = 0x97;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_sub8_imm8() {
        let opcode = 0xD6;
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;

        FakeCpu::new().test_decode_operand(opcode, &Sub8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_invalid_opcode_sub8() {
        let opcode = 0xFF; // Example invalid opcode for Sub8
        assert!(Sub8Decoder{}.decode(opcode).is_err());
    }
}