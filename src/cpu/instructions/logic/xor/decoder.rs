use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::logic::opcode::Xor8;
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

pub struct Xor8Decoder;

impl Decoder for Xor8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let mut cycles = 4;
        let operand = match opcode {
            0xA8 => Operand::Register8(Register8::B),
            0xA9 => Operand::Register8(Register8::C),
            0xAA => Operand::Register8(Register8::D),
            0xAB => Operand::Register8(Register8::E),
            0xAC => Operand::Register8(Register8::H),
            0xAD => Operand::Register8(Register8::L),
            0xAE => {
                cycles = 8;
                Operand::Memory(Memory::HL)
            }
            0xAF => Operand::Register8(Register8::A),
            0xEE => {
                cycles = 8;
                Operand::Imm8
            }
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Xor8 { operand, cycles }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_xor8_b() {
        FakeCpu::new().test_decode_operand(0xA8, &Xor8Decoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_xor8_c() {
        FakeCpu::new().test_decode_operand(0xA9, &Xor8Decoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_xor8_d() {
        FakeCpu::new().test_decode_operand(0xAA, &Xor8Decoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_xor8_e() {
        FakeCpu::new().test_decode_operand(0xAB, &Xor8Decoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_xor8_h() {
        FakeCpu::new().test_decode_operand(0xAC, &Xor8Decoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_xor8_l() {
        FakeCpu::new().test_decode_operand(0xAD, &Xor8Decoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_xor8_mem_hl() {
        FakeCpu::new().test_decode_operand(0xAE, &Xor8Decoder, 8, Operand::Memory(Memory::HL));
    }

    #[test]
    fn test_decode_xor8_a() {
        FakeCpu::new().test_decode_operand(0xAF, &Xor8Decoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_xor8_imm8() {
        FakeCpu::new().test_decode_operand(0xEE, &Xor8Decoder, 8, Operand::Imm8);
    }

    #[test]
    fn test_decode_xor8_invalid() {
        assert!(Xor8Decoder.decode(0xFF).is_err());
    }
}
