use alloc::boxed::Box;
use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::logic::opcode::Or8;
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

pub struct Or8Decoder;

impl Decoder for Or8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let mut cycles = 4;
        let operand = match opcode {
            0xB0 => Operand::Register8(Register8::B),
            0xB1 => Operand::Register8(Register8::C),
            0xB2 => Operand::Register8(Register8::D),
            0xB3 => Operand::Register8(Register8::E),
            0xB4 => Operand::Register8(Register8::H),
            0xB5 => Operand::Register8(Register8::L),
            0xB6 => {
                cycles = 8;
                Operand::Memory(Memory::HL)
            }
            0xB7 => Operand::Register8(Register8::A),
            0xF6 => {
                cycles = 8;
                Operand::Imm8
            }
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Or8 { operand, cycles }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_or8_b() {
        FakeCpu::new().test_decode_operand(0xB0, &Or8Decoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_or8_c() {
        FakeCpu::new().test_decode_operand(0xB1, &Or8Decoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_or8_d() {
        FakeCpu::new().test_decode_operand(0xB2, &Or8Decoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_or8_e() {
        FakeCpu::new().test_decode_operand(0xB3, &Or8Decoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_or8_h() {
        FakeCpu::new().test_decode_operand(0xB4, &Or8Decoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_or8_l() {
        FakeCpu::new().test_decode_operand(0xB5, &Or8Decoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_or8_mem_hl() {
        FakeCpu::new().test_decode_operand(0xB6, &Or8Decoder, 8, Operand::Memory(Memory::HL));
    }

    #[test]
    fn test_decode_or8_a() {
        FakeCpu::new().test_decode_operand(0xB7, &Or8Decoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_or8_imm8() {
        FakeCpu::new().test_decode_operand(0xF6, &Or8Decoder, 8, Operand::Imm8);
    }

    #[test]
    fn test_decode_or8_invalid() {
        assert!(Or8Decoder.decode(0xFF).is_err());
    }
}
