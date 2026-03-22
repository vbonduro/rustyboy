use alloc::boxed::Box;
use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::jump::opcode::Condition;
use crate::cpu::instructions::opcode::OpCode;

use super::opcode::{Ret, RetOp};

pub struct RetDecoder;

impl Decoder for RetDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let (op, cycles) = match opcode {
            0xC9 => (RetOp::Ret, 16),
            0xC0 => (RetOp::RetCc(Condition::NZ), 20),
            0xC8 => (RetOp::RetCc(Condition::Z), 20),
            0xD0 => (RetOp::RetCc(Condition::NC), 20),
            0xD8 => (RetOp::RetCc(Condition::C), 20),
            0xD9 => (RetOp::Reti, 16),
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Ret { op, cycles }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    fn cycles(opcode: u8) -> u8 {
        RetDecoder
            .decode(opcode)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap()
    }

    #[test]
    fn test_decode_ret() {
        assert_eq!(cycles(0xC9), 16);
    }

    #[test]
    fn test_decode_ret_nz() {
        assert_eq!(cycles(0xC0), 20);
    }

    #[test]
    fn test_decode_ret_z() {
        assert_eq!(cycles(0xC8), 20);
    }

    #[test]
    fn test_decode_ret_nc() {
        assert_eq!(cycles(0xD0), 20);
    }

    #[test]
    fn test_decode_ret_c() {
        assert_eq!(cycles(0xD8), 20);
    }

    #[test]
    fn test_decode_reti() {
        assert_eq!(cycles(0xD9), 16);
    }

    #[test]
    fn test_decode_invalid() {
        assert!(RetDecoder.decode(0x00).is_err());
    }
}
