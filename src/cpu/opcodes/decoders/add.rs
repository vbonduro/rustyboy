use super::decoder::{Decoder, Error};

use crate::cpu::opcodes::add::{Add8, Add16, AddSP16};
use crate::cpu::opcodes::opcode::OpCode;
use crate::cpu::opcodes::operand::*;

pub struct Add8Decoder;
impl Decoder for Add8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            0x80 => Ok(Box::new(Add8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0x81 => Ok(Box::new(Add8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0x82 => Ok(Box::new(Add8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0x83 => Ok(Box::new(Add8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0x84 => Ok(Box::new(Add8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0x85 => Ok(Box::new(Add8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0x86 => Ok(Box::new(Add8 { operand: Operand::Memory(Memory::HL), cycles: 8 })),
            0x87 => Ok(Box::new(Add8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            0xC6 => Ok(Box::new(Add8 { operand: Operand::Imm8, cycles: 8 })),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

pub struct Add16Decoder;
impl Decoder for Add16Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            0x09 => Ok(Box::new(Add16 { operand: Operand::Register16(Register16::BC), cycles: 8 })),
            0x19 => Ok(Box::new(Add16 { operand: Operand::Register16(Register16::DE), cycles: 8 })),
            0x29 => Ok(Box::new(Add16 { operand: Operand::Register16(Register16::HL), cycles: 8 })),
            0x39 => Ok(Box::new(Add16 { operand: Operand::Register16(Register16::SP), cycles: 8 })),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

pub struct AddSP16Decoder;
impl Decoder for AddSP16Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            0xE8 => Ok(Box::new(AddSP16 { operand: Operand::ImmSigned8, cycles: 16 })),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::cpu::opcodes::test_util::operand_test_util::FakeCpu;

    #[test]
    fn test_decode_add8_b() {
        let opcode = 0x80;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add8_c() {
        let opcode = 0x81;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::C);

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add8_d() {
        let opcode = 0x82;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::D);

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add8_e() {
        let opcode = 0x83;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::E);

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add8_h() {
        let opcode = 0x84;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::H);

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add8_l() {
        let opcode = 0x85;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::L);

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add8_hl() {
        let opcode = 0x86;
        let expected_cycles = 8;
        let expected_operand = Operand::Memory(Memory::HL);

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add8_a() {
        let opcode = 0x87;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add8_imm8() {
        let opcode = 0xC6;
        let expected_cycles = 8;
        let expected_operand = Operand::Imm8;

        FakeCpu::new().test_decode_operand(opcode, &Add8Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add16_bc() {
        let opcode = 0x09;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::BC);

        FakeCpu::new().test_decode_operand(opcode, &Add16Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add16_de() {
        let opcode = 0x19;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::DE);

        FakeCpu::new().test_decode_operand(opcode, &Add16Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add16_hl() {
        let opcode = 0x29;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::HL);

        FakeCpu::new().test_decode_operand(opcode, &Add16Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_add16_sp() {
        let opcode = 0x39;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::SP);

        FakeCpu::new().test_decode_operand(opcode, &Add16Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_decode_addsp_imm_signed8() {
        let opcode = 0xE8;
        let expected_cycles = 16;
        let expected_operand = Operand::ImmSigned8;

        FakeCpu::new().test_decode_operand(opcode, &AddSP16Decoder, expected_cycles, expected_operand);
    }

    #[test]
    fn test_invalid_opcode_add8() {
        let opcode = 0xFF; // Example invalid opcode for Add8
        assert!(Add8Decoder{}.decode(opcode).is_err());
    }

    #[test]
    fn test_invalid_opcode_add16() {
        let opcode = 0xFF; // Example invalid opcode for Add16
        assert!(Add16Decoder{}.decode(opcode).is_err());
    }

    #[test]
    fn test_invalid_opcode_addsp() {
        let opcode = 0xFF; // Example invalid opcode for AddSP16
        assert!(AddSP16Decoder{}.decode(opcode).is_err());
    }
}

