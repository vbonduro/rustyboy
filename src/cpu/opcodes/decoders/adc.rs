use super::decoder::{Decoder, Error};

use crate::cpu::opcodes::adc::Adc;
use crate::cpu::opcodes::opcode::OpCode;
use crate::cpu::opcodes::operand::*;

pub struct AdcDecoder;
impl Decoder for AdcDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            0x88 => Ok(Box::new(Adc { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0x89 => Ok(Box::new(Adc { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0x8A => Ok(Box::new(Adc { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0x8B => Ok(Box::new(Adc { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0x8C => Ok(Box::new(Adc { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0x8D => Ok(Box::new(Adc { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0x8E => Ok(Box::new(Adc { operand: Operand::Memory(Memory::HL), cycles: 8 })),
            0x8F => Ok(Box::new(Adc { operand: Operand::Register8(Register8::A), cycles: 4 })),
            0xCE => Ok(Box::new(Adc { operand: Operand::Imm8, cycles: 8 })),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cpu::opcodes::test_util::operand_test_util::FakeCpu;

    #[test]
    fn test_decode_adc_opcode_regb() {
        let opcode = 0x88;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_regc() {
        let opcode = 0x89;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_regd() {
        let opcode = 0x8A;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_rege() {
        let opcode = 0x8B;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_regh() {
        let opcode = 0x8C;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_regl() {
        let opcode = 0x8D;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_memhl() {
        let opcode = 0x8E;
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_rega() {
        let opcode = 0x8F;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_imm8() {
        let opcode = 0xCE;
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;

        FakeCpu::new().test_decode_operand(opcode, &AdcDecoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_adc_opcode_invalid() {
        let decoder = AdcDecoder {};
        let result = decoder.decode(0xFF);
        assert!(result.is_err());
    }
}

