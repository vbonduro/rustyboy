use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

use super::opcode::{Dec8, Dec16, Inc8, Inc16};

pub struct IncDecDecoder;

impl Decoder for IncDecDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        match opcode {
            // INC r — 8-bit register increment (4 cycles)
            0x04 => Ok(Box::new(Inc8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0x0C => Ok(Box::new(Inc8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0x14 => Ok(Box::new(Inc8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0x1C => Ok(Box::new(Inc8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0x24 => Ok(Box::new(Inc8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0x2C => Ok(Box::new(Inc8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0x3C => Ok(Box::new(Inc8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            // INC (HL) — increment memory at HL (12 cycles)
            0x34 => Ok(Box::new(Inc8 { operand: Operand::Memory(Memory::HL), cycles: 12 })),
            // DEC r — 8-bit register decrement (4 cycles)
            0x05 => Ok(Box::new(Dec8 { operand: Operand::Register8(Register8::B), cycles: 4 })),
            0x0D => Ok(Box::new(Dec8 { operand: Operand::Register8(Register8::C), cycles: 4 })),
            0x15 => Ok(Box::new(Dec8 { operand: Operand::Register8(Register8::D), cycles: 4 })),
            0x1D => Ok(Box::new(Dec8 { operand: Operand::Register8(Register8::E), cycles: 4 })),
            0x25 => Ok(Box::new(Dec8 { operand: Operand::Register8(Register8::H), cycles: 4 })),
            0x2D => Ok(Box::new(Dec8 { operand: Operand::Register8(Register8::L), cycles: 4 })),
            0x3D => Ok(Box::new(Dec8 { operand: Operand::Register8(Register8::A), cycles: 4 })),
            // DEC (HL) — decrement memory at HL (12 cycles)
            0x35 => Ok(Box::new(Dec8 { operand: Operand::Memory(Memory::HL), cycles: 12 })),
            // INC rr — 16-bit register pair increment (8 cycles), NO flags affected
            0x03 => Ok(Box::new(Inc16 { operand: Register16::BC, cycles: 8 })),
            0x13 => Ok(Box::new(Inc16 { operand: Register16::DE, cycles: 8 })),
            0x23 => Ok(Box::new(Inc16 { operand: Register16::HL, cycles: 8 })),
            0x33 => Ok(Box::new(Inc16 { operand: Register16::SP, cycles: 8 })),
            // DEC rr — 16-bit register pair decrement (8 cycles), NO flags affected
            0x0B => Ok(Box::new(Dec16 { operand: Register16::BC, cycles: 8 })),
            0x1B => Ok(Box::new(Dec16 { operand: Register16::DE, cycles: 8 })),
            0x2B => Ok(Box::new(Dec16 { operand: Register16::HL, cycles: 8 })),
            0x3B => Ok(Box::new(Dec16 { operand: Register16::SP, cycles: 8 })),
            _ => Err(Error::InvalidOpcode(opcode)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    // --- INC r decode tests ---

    #[test]
    fn test_decode_inc8_b() {
        FakeCpu::new().test_decode_operand(0x04, &IncDecDecoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_inc8_c() {
        FakeCpu::new().test_decode_operand(0x0C, &IncDecDecoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_inc8_d() {
        FakeCpu::new().test_decode_operand(0x14, &IncDecDecoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_inc8_e() {
        FakeCpu::new().test_decode_operand(0x1C, &IncDecDecoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_inc8_h() {
        FakeCpu::new().test_decode_operand(0x24, &IncDecDecoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_inc8_l() {
        FakeCpu::new().test_decode_operand(0x2C, &IncDecDecoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_inc8_a() {
        FakeCpu::new().test_decode_operand(0x3C, &IncDecDecoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_inc8_mem_hl() {
        FakeCpu::new().test_decode_operand(0x34, &IncDecDecoder, 12, Operand::Memory(Memory::HL));
    }

    // --- DEC r decode tests ---

    #[test]
    fn test_decode_dec8_b() {
        FakeCpu::new().test_decode_operand(0x05, &IncDecDecoder, 4, Operand::Register8(Register8::B));
    }

    #[test]
    fn test_decode_dec8_c() {
        FakeCpu::new().test_decode_operand(0x0D, &IncDecDecoder, 4, Operand::Register8(Register8::C));
    }

    #[test]
    fn test_decode_dec8_d() {
        FakeCpu::new().test_decode_operand(0x15, &IncDecDecoder, 4, Operand::Register8(Register8::D));
    }

    #[test]
    fn test_decode_dec8_e() {
        FakeCpu::new().test_decode_operand(0x1D, &IncDecDecoder, 4, Operand::Register8(Register8::E));
    }

    #[test]
    fn test_decode_dec8_h() {
        FakeCpu::new().test_decode_operand(0x25, &IncDecDecoder, 4, Operand::Register8(Register8::H));
    }

    #[test]
    fn test_decode_dec8_l() {
        FakeCpu::new().test_decode_operand(0x2D, &IncDecDecoder, 4, Operand::Register8(Register8::L));
    }

    #[test]
    fn test_decode_dec8_a() {
        FakeCpu::new().test_decode_operand(0x3D, &IncDecDecoder, 4, Operand::Register8(Register8::A));
    }

    #[test]
    fn test_decode_dec8_mem_hl() {
        FakeCpu::new().test_decode_operand(0x35, &IncDecDecoder, 12, Operand::Memory(Memory::HL));
    }

    // --- INC rr decode tests ---
    // Note: Inc16/Dec16 use Register16 not Operand, so we test cycles only via execute

    #[test]
    fn test_decode_inc16_bc() {
        let decoded = IncDecDecoder.decode(0x03).unwrap();
        let mut cpu = FakeCpu::new();
        assert_eq!(decoded.execute(&mut cpu).unwrap(), 8);
    }

    #[test]
    fn test_decode_inc16_de() {
        let decoded = IncDecDecoder.decode(0x13).unwrap();
        let mut cpu = FakeCpu::new();
        assert_eq!(decoded.execute(&mut cpu).unwrap(), 8);
    }

    #[test]
    fn test_decode_inc16_hl() {
        let decoded = IncDecDecoder.decode(0x23).unwrap();
        let mut cpu = FakeCpu::new();
        assert_eq!(decoded.execute(&mut cpu).unwrap(), 8);
    }

    #[test]
    fn test_decode_inc16_sp() {
        let decoded = IncDecDecoder.decode(0x33).unwrap();
        let mut cpu = FakeCpu::new();
        assert_eq!(decoded.execute(&mut cpu).unwrap(), 8);
    }

    // --- DEC rr decode tests ---

    #[test]
    fn test_decode_dec16_bc() {
        let decoded = IncDecDecoder.decode(0x0B).unwrap();
        let mut cpu = FakeCpu::new();
        assert_eq!(decoded.execute(&mut cpu).unwrap(), 8);
    }

    #[test]
    fn test_decode_dec16_de() {
        let decoded = IncDecDecoder.decode(0x1B).unwrap();
        let mut cpu = FakeCpu::new();
        assert_eq!(decoded.execute(&mut cpu).unwrap(), 8);
    }

    #[test]
    fn test_decode_dec16_hl() {
        let decoded = IncDecDecoder.decode(0x2B).unwrap();
        let mut cpu = FakeCpu::new();
        assert_eq!(decoded.execute(&mut cpu).unwrap(), 8);
    }

    #[test]
    fn test_decode_dec16_sp() {
        let decoded = IncDecDecoder.decode(0x3B).unwrap();
        let mut cpu = FakeCpu::new();
        assert_eq!(decoded.execute(&mut cpu).unwrap(), 8);
    }

    #[test]
    fn test_invalid_opcode() {
        assert!(IncDecDecoder.decode(0xFF).is_err());
    }
}
