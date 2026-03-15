use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

use super::opcode::{Dec16, Dec8, Inc16, Inc8};

pub struct Inc8Decoder;

impl Decoder for Inc8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let (operand, cycles) = match opcode {
            0x04 => (Operand::Register8(Register8::B), 4),
            0x0C => (Operand::Register8(Register8::C), 4),
            0x14 => (Operand::Register8(Register8::D), 4),
            0x1C => (Operand::Register8(Register8::E), 4),
            0x24 => (Operand::Register8(Register8::H), 4),
            0x2C => (Operand::Register8(Register8::L), 4),
            0x3C => (Operand::Register8(Register8::A), 4),
            0x34 => (Operand::Memory(Memory::HL), 12),
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Inc8 { operand, cycles }))
    }
}

pub struct Dec8Decoder;

impl Decoder for Dec8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let (operand, cycles) = match opcode {
            0x05 => (Operand::Register8(Register8::B), 4),
            0x0D => (Operand::Register8(Register8::C), 4),
            0x15 => (Operand::Register8(Register8::D), 4),
            0x1D => (Operand::Register8(Register8::E), 4),
            0x25 => (Operand::Register8(Register8::H), 4),
            0x2D => (Operand::Register8(Register8::L), 4),
            0x3D => (Operand::Register8(Register8::A), 4),
            0x35 => (Operand::Memory(Memory::HL), 12),
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Dec8 { operand, cycles }))
    }
}

pub struct Inc16Decoder;

impl Decoder for Inc16Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let operand = match opcode {
            0x03 => Register16::BC,
            0x13 => Register16::DE,
            0x23 => Register16::HL,
            0x33 => Register16::SP,
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Inc16 { operand, cycles: 8 }))
    }
}

pub struct Dec16Decoder;

impl Decoder for Dec16Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let operand = match opcode {
            0x0B => Register16::BC,
            0x1B => Register16::DE,
            0x2B => Register16::HL,
            0x3B => Register16::SP,
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Dec16 { operand, cycles: 8 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_inc8_b() {
        FakeCpu::new().test_decode_operand(0x04, &Inc8Decoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_inc8_c() {
        FakeCpu::new().test_decode_operand(0x0C, &Inc8Decoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_inc8_d() {
        FakeCpu::new().test_decode_operand(0x14, &Inc8Decoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_inc8_e() {
        FakeCpu::new().test_decode_operand(0x1C, &Inc8Decoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_inc8_h() {
        FakeCpu::new().test_decode_operand(0x24, &Inc8Decoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_inc8_l() {
        FakeCpu::new().test_decode_operand(0x2C, &Inc8Decoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_inc8_a() {
        FakeCpu::new().test_decode_operand(0x3C, &Inc8Decoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_inc8_mem_hl() {
        FakeCpu::new().test_decode_operand(0x34, &Inc8Decoder, 12, Operand::Memory(Memory::HL));
    }

    #[test]
    fn test_decode_inc8_invalid() {
        assert!(Inc8Decoder.decode(0xFF).is_err());
    }

    #[test]
    fn test_decode_dec8_b() {
        FakeCpu::new().test_decode_operand(0x05, &Dec8Decoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_dec8_c() {
        FakeCpu::new().test_decode_operand(0x0D, &Dec8Decoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_dec8_d() {
        FakeCpu::new().test_decode_operand(0x15, &Dec8Decoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_dec8_e() {
        FakeCpu::new().test_decode_operand(0x1D, &Dec8Decoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_dec8_h() {
        FakeCpu::new().test_decode_operand(0x25, &Dec8Decoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_dec8_l() {
        FakeCpu::new().test_decode_operand(0x2D, &Dec8Decoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_dec8_a() {
        FakeCpu::new().test_decode_operand(0x3D, &Dec8Decoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_dec8_mem_hl() {
        FakeCpu::new().test_decode_operand(0x35, &Dec8Decoder, 12, Operand::Memory(Memory::HL));
    }

    #[test]
    fn test_decode_dec8_invalid() {
        assert!(Dec8Decoder.decode(0xFF).is_err());
    }

    #[test]
    fn test_decode_inc16_bc() {
        let decoded = Inc16Decoder.decode(0x03).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_inc16_de() {
        let decoded = Inc16Decoder.decode(0x13).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_inc16_hl() {
        let decoded = Inc16Decoder.decode(0x23).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_inc16_sp() {
        let decoded = Inc16Decoder.decode(0x33).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_inc16_invalid() {
        assert!(Inc16Decoder.decode(0xFF).is_err());
    }

    #[test]
    fn test_decode_dec16_bc() {
        let decoded = Dec16Decoder.decode(0x0B).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_dec16_de() {
        let decoded = Dec16Decoder.decode(0x1B).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_dec16_hl() {
        let decoded = Dec16Decoder.decode(0x2B).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_dec16_sp() {
        let decoded = Dec16Decoder.decode(0x3B).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_dec16_invalid() {
        assert!(Dec16Decoder.decode(0xFF).is_err());
    }
}
