use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

use super::opcode::{And8, Or8, Xor8};

pub struct LogicDecoder;

impl Decoder for LogicDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            // AND r / AND (HL) / AND n
            0xA0 => Ok(Box::new(And8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0xA1 => Ok(Box::new(And8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0xA2 => Ok(Box::new(And8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0xA3 => Ok(Box::new(And8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0xA4 => Ok(Box::new(And8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0xA5 => Ok(Box::new(And8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0xA6 => Ok(Box::new(And8 { operand: Operand::Memory(Memory::HL), cycles: 8 })),
            0xA7 => Ok(Box::new(And8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            0xE6 => Ok(Box::new(And8 { operand: Operand::Imm8, cycles: 8 })),

            // OR r / OR (HL) / OR n
            0xB0 => Ok(Box::new(Or8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0xB1 => Ok(Box::new(Or8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0xB2 => Ok(Box::new(Or8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0xB3 => Ok(Box::new(Or8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0xB4 => Ok(Box::new(Or8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0xB5 => Ok(Box::new(Or8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0xB6 => Ok(Box::new(Or8 { operand: Operand::Memory(Memory::HL), cycles: 8 })),
            0xB7 => Ok(Box::new(Or8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            0xF6 => Ok(Box::new(Or8 { operand: Operand::Imm8, cycles: 8 })),

            // XOR r / XOR (HL) / XOR n
            0xA8 => Ok(Box::new(Xor8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0xA9 => Ok(Box::new(Xor8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0xAA => Ok(Box::new(Xor8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0xAB => Ok(Box::new(Xor8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0xAC => Ok(Box::new(Xor8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0xAD => Ok(Box::new(Xor8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0xAE => Ok(Box::new(Xor8 { operand: Operand::Memory(Memory::HL), cycles: 8 })),
            0xAF => Ok(Box::new(Xor8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            0xEE => Ok(Box::new(Xor8 { operand: Operand::Imm8, cycles: 8 })),

            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    // AND decode tests
    #[test]
    fn test_decode_and8_b() {
        FakeCpu::new().test_decode_operand(0xA0, &LogicDecoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_and8_c() {
        FakeCpu::new().test_decode_operand(0xA1, &LogicDecoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_and8_d() {
        FakeCpu::new().test_decode_operand(0xA2, &LogicDecoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_and8_e() {
        FakeCpu::new().test_decode_operand(0xA3, &LogicDecoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_and8_h() {
        FakeCpu::new().test_decode_operand(0xA4, &LogicDecoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_and8_l() {
        FakeCpu::new().test_decode_operand(0xA5, &LogicDecoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_and8_mem_hl() {
        FakeCpu::new().test_decode_operand(0xA6, &LogicDecoder, 8, Operand::Memory(Memory::HL));
    }

    #[test]
    fn test_decode_and8_a() {
        FakeCpu::new().test_decode_operand(0xA7, &LogicDecoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_and8_imm8() {
        FakeCpu::new().test_decode_operand(0xE6, &LogicDecoder, 8, Operand::Imm8);
    }

    // OR decode tests
    #[test]
    fn test_decode_or8_b() {
        FakeCpu::new().test_decode_operand(0xB0, &LogicDecoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_or8_c() {
        FakeCpu::new().test_decode_operand(0xB1, &LogicDecoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_or8_d() {
        FakeCpu::new().test_decode_operand(0xB2, &LogicDecoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_or8_e() {
        FakeCpu::new().test_decode_operand(0xB3, &LogicDecoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_or8_h() {
        FakeCpu::new().test_decode_operand(0xB4, &LogicDecoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_or8_l() {
        FakeCpu::new().test_decode_operand(0xB5, &LogicDecoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_or8_mem_hl() {
        FakeCpu::new().test_decode_operand(0xB6, &LogicDecoder, 8, Operand::Memory(Memory::HL));
    }

    #[test]
    fn test_decode_or8_a() {
        FakeCpu::new().test_decode_operand(0xB7, &LogicDecoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_or8_imm8() {
        FakeCpu::new().test_decode_operand(0xF6, &LogicDecoder, 8, Operand::Imm8);
    }

    // XOR decode tests
    #[test]
    fn test_decode_xor8_b() {
        FakeCpu::new().test_decode_operand(0xA8, &LogicDecoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_xor8_c() {
        FakeCpu::new().test_decode_operand(0xA9, &LogicDecoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_xor8_d() {
        FakeCpu::new().test_decode_operand(0xAA, &LogicDecoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_xor8_e() {
        FakeCpu::new().test_decode_operand(0xAB, &LogicDecoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_xor8_h() {
        FakeCpu::new().test_decode_operand(0xAC, &LogicDecoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_xor8_l() {
        FakeCpu::new().test_decode_operand(0xAD, &LogicDecoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_xor8_mem_hl() {
        FakeCpu::new().test_decode_operand(0xAE, &LogicDecoder, 8, Operand::Memory(Memory::HL));
    }

    #[test]
    fn test_decode_xor8_a() {
        FakeCpu::new().test_decode_operand(0xAF, &LogicDecoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_xor8_imm8() {
        FakeCpu::new().test_decode_operand(0xEE, &LogicDecoder, 8, Operand::Imm8);
    }

    #[test]
    fn test_decode_invalid_opcode() {
        assert!(LogicDecoder.decode(0xFF).is_err());
    }
}
