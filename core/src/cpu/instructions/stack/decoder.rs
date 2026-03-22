use alloc::boxed::Box;
use super::opcode::{Pop16, Push16};
use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::Register16;

pub struct Push16Decoder {}

impl Decoder for Push16Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let operand = match opcode {
            0xC5 => Register16::BC,
            0xD5 => Register16::DE,
            0xE5 => Register16::HL,
            0xF5 => Register16::AF,
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Push16 {
            operand,
            cycles: 16,
        }))
    }
}

pub struct Pop16Decoder {}

impl Decoder for Pop16Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let operand = match opcode {
            0xC1 => Register16::BC,
            0xD1 => Register16::DE,
            0xE1 => Register16::HL,
            0xF1 => Register16::AF,
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Pop16 {
            operand,
            cycles: 12,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::operand::Operand;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_push_bc() {
        FakeCpu::new().test_decode_operand(
            0xC5,
            &Push16Decoder {},
            16,
            Operand::Register16(Register16::BC),
        );
    }

    #[test]
    fn test_push_de() {
        FakeCpu::new().test_decode_operand(
            0xD5,
            &Push16Decoder {},
            16,
            Operand::Register16(Register16::DE),
        );
    }

    #[test]
    fn test_push_hl() {
        FakeCpu::new().test_decode_operand(
            0xE5,
            &Push16Decoder {},
            16,
            Operand::Register16(Register16::HL),
        );
    }

    #[test]
    fn test_push_af() {
        FakeCpu::new().test_decode_operand(
            0xF5,
            &Push16Decoder {},
            16,
            Operand::Register16(Register16::AF),
        );
    }

    #[test]
    fn test_push_invalid() {
        assert!(Push16Decoder {}.decode(0xC0).is_err());
    }

    #[test]
    fn test_pop_bc() {
        FakeCpu::new().test_decode_operand(
            0xC1,
            &Pop16Decoder {},
            12,
            Operand::Register16(Register16::BC),
        );
    }

    #[test]
    fn test_pop_de() {
        FakeCpu::new().test_decode_operand(
            0xD1,
            &Pop16Decoder {},
            12,
            Operand::Register16(Register16::DE),
        );
    }

    #[test]
    fn test_pop_hl() {
        FakeCpu::new().test_decode_operand(
            0xE1,
            &Pop16Decoder {},
            12,
            Operand::Register16(Register16::HL),
        );
    }

    #[test]
    fn test_pop_af() {
        FakeCpu::new().test_decode_operand(
            0xF1,
            &Pop16Decoder {},
            12,
            Operand::Register16(Register16::AF),
        );
    }

    #[test]
    fn test_pop_invalid() {
        assert!(Pop16Decoder {}.decode(0xC0).is_err());
    }
}
