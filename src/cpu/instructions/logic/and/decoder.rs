use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::logic::opcode::And8;
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

pub struct And8Decoder;

impl Decoder for And8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let mut cycles = 4;
        let operand = match opcode {
            0xA0 => Operand::Register8(Register8::B),
            0xA1 => Operand::Register8(Register8::C),
            0xA2 => Operand::Register8(Register8::D),
            0xA3 => Operand::Register8(Register8::E),
            0xA4 => Operand::Register8(Register8::H),
            0xA5 => Operand::Register8(Register8::L),
            0xA6 => {
                cycles = 8;
                Operand::Memory(Memory::HL)
            }
            0xA7 => Operand::Register8(Register8::A),
            0xE6 => {
                cycles = 8;
                Operand::Imm8
            }
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(And8 { operand, cycles }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_and8_b() {
        FakeCpu::new().test_decode_operand(0xA0, &And8Decoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_and8_c() {
        FakeCpu::new().test_decode_operand(0xA1, &And8Decoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_and8_d() {
        FakeCpu::new().test_decode_operand(0xA2, &And8Decoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_and8_e() {
        FakeCpu::new().test_decode_operand(0xA3, &And8Decoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_and8_h() {
        FakeCpu::new().test_decode_operand(0xA4, &And8Decoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_and8_l() {
        FakeCpu::new().test_decode_operand(0xA5, &And8Decoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_and8_mem_hl() {
        FakeCpu::new().test_decode_operand(0xA6, &And8Decoder, 8, Operand::Memory(Memory::HL));
    }

    #[test]
    fn test_decode_and8_a() {
        FakeCpu::new().test_decode_operand(0xA7, &And8Decoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_and8_imm8() {
        FakeCpu::new().test_decode_operand(0xE6, &And8Decoder, 8, Operand::Imm8);
    }

    #[test]
    fn test_decode_and8_invalid() {
        assert!(And8Decoder.decode(0xFF).is_err());
    }
}
