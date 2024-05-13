use crate::cpu::opcodes::decoder::{Decoder, Error};
use crate::cpu::opcodes::operand::{Memory, Operand, Register8, Register16};

/// Adds the value of the operand to the accumulator register (A).
pub struct Add8 {
    pub operand: Operand,
    pub cycles: u8,
}

impl Decoder for Add8 {
    type Opcode = Add8;

    fn decode(opcode: u8) -> Result<Self::Opcode, Error> {
        match opcode {
            0x80 => Ok(Add8 { operand: Operand::Register8(Register8::B), cycles: 4 }),
            0x81 => Ok(Add8 { operand: Operand::Register8(Register8::C), cycles: 4 }),
            0x82 => Ok(Add8 { operand: Operand::Register8(Register8::D), cycles: 4 }),
            0x83 => Ok(Add8 { operand: Operand::Register8(Register8::E), cycles: 4 }),
            0x84 => Ok(Add8 { operand: Operand::Register8(Register8::H), cycles: 4 }),
            0x85 => Ok(Add8 { operand: Operand::Register8(Register8::L), cycles: 4 }),
            0x86 => Ok(Add8 { operand: Operand::Memory(Memory::HL), cycles: 8 }),
            0x87 => Ok(Add8 { operand: Operand::Register8(Register8::A), cycles: 4 }),
            0xC6 => Ok(Add8 { operand: Operand::Imm8, cycles: 8 }),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

/// Adds the value of the operand to the 16-bit HL register.
pub struct Add16 {
    pub operand: Operand,
    pub cycles: u8,
}

impl Decoder for Add16 {
    type Opcode = Add16;

    fn decode(opcode: u8) -> Result<Self::Opcode, Error> {
        match opcode {
            0x09 => Ok(Add16 { operand: Operand::Register16(Register16::BC), cycles: 8 }),
            0x19 => Ok(Add16 { operand: Operand::Register16(Register16::DE), cycles: 8 }),
            0x29 => Ok(Add16 { operand: Operand::Register16(Register16::HL), cycles: 8 }),
            0x39 => Ok(Add16 { operand: Operand::Register16(Register16::SP), cycles: 8 }),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

/// Adds the value of the operand to the stack pointer (SP).
pub struct AddSP16 {
    pub operand: Operand,
    pub cycles: u8,
}

impl Decoder for AddSP16 {
    type Opcode = AddSP16;

    fn decode(opcode: u8) -> Result<Self::Opcode, Error> {
        match opcode {
            0xE8 => Ok(AddSP16 { operand: Operand::ImmSigned8, cycles: 16 }),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_add8_b() {
        let opcode = 0x80;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register8(Register8::B));
        assert_eq!(decoded_opcode.cycles, 4);
    }

    #[test]
    fn test_decode_add8_c() {
        let opcode = 0x81;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register8(Register8::C));
        assert_eq!(decoded_opcode.cycles, 4);
    }

    #[test]
    fn test_decode_add8_d() {
        let opcode = 0x82;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register8(Register8::D));
        assert_eq!(decoded_opcode.cycles, 4);
    }

    #[test]
    fn test_decode_add8_e() {
        let opcode = 0x83;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register8(Register8::E));
        assert_eq!(decoded_opcode.cycles, 4);
    }

    #[test]
    fn test_decode_add8_h() {
        let opcode = 0x84;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register8(Register8::H));
        assert_eq!(decoded_opcode.cycles, 4);
    }

    #[test]
    fn test_decode_add8_l() {
        let opcode = 0x85;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register8(Register8::L));
        assert_eq!(decoded_opcode.cycles, 4);
    }

    #[test]
    fn test_decode_add8_hl() {
        let opcode = 0x86;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Memory(Memory::HL));
        assert_eq!(decoded_opcode.cycles, 8);
    }

    #[test]
    fn test_decode_add8_a() {
        let opcode = 0x87;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register8(Register8::A));
        assert_eq!(decoded_opcode.cycles, 4);
    }

    #[test]
    fn test_decode_add8_imm8() {
        let opcode = 0xC6;
        let decoded_opcode = Add8::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Imm8);
        assert_eq!(decoded_opcode.cycles, 8);
    }

    #[test]
    fn test_decode_add16_bc() {
        let opcode = 0x09;
        let decoded_opcode = Add16::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register16(Register16::BC));
        assert_eq!(decoded_opcode.cycles, 8);
    }

    #[test]
    fn test_decode_add16_de() {
        let opcode = 0x19;
        let decoded_opcode = Add16::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register16(Register16::DE));
        assert_eq!(decoded_opcode.cycles, 8);
    }

    #[test]
    fn test_decode_add16_hl() {
        let opcode = 0x29;
        let decoded_opcode = Add16::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register16(Register16::HL));
        assert_eq!(decoded_opcode.cycles, 8);
    }

    #[test]
    fn test_decode_add16_sp() {
        let opcode = 0x39;
        let decoded_opcode = Add16::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::Register16(Register16::SP));
        assert_eq!(decoded_opcode.cycles, 8);
    }

    #[test]
    fn test_decode_addsp_imm_signed8() {
        let opcode = 0xE8;
        let decoded_opcode = AddSP16::decode(opcode).unwrap();
        assert_eq!(decoded_opcode.operand, Operand::ImmSigned8);
        assert_eq!(decoded_opcode.cycles, 16);
    }

    #[test]
    fn test_invalid_opcode_add8() {
        let opcode = 0xFF; // Example invalid opcode for Add8
        assert!(Add8::decode(opcode).is_err());
    }

    #[test]
    fn test_invalid_opcode_add16() {
        let opcode = 0xFF; // Example invalid opcode for Add16
        assert!(Add16::decode(opcode).is_err());
    }

    #[test]
    fn test_invalid_opcode_addsp() {
        let opcode = 0xFF; // Example invalid opcode for AddSP16
        assert!(AddSP16::decode(opcode).is_err());
    }
}

