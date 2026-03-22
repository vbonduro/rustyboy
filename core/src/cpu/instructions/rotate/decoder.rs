use alloc::boxed::Box;
use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;

use super::opcode::{Rotate, RotateOp};

pub struct RotateDecoder;

impl Decoder for RotateDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let op = match opcode {
            0x07 => RotateOp::Rlca,
            0x17 => RotateOp::Rla,
            0x0F => RotateOp::Rrca,
            0x1F => RotateOp::Rra,
            _ => return Err(Error::InvalidOpcode(opcode)),
        };

        Ok(Box::new(Rotate { op, cycles: 4 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_rlca() {
        let decoded = RotateDecoder.decode(0x07).unwrap();
        let actual_cycles = decoded.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(actual_cycles, 4);
    }

    #[test]
    fn test_decode_rla() {
        let decoded = RotateDecoder.decode(0x17).unwrap();
        let actual_cycles = decoded.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(actual_cycles, 4);
    }

    #[test]
    fn test_decode_rrca() {
        let decoded = RotateDecoder.decode(0x0F).unwrap();
        let actual_cycles = decoded.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(actual_cycles, 4);
    }

    #[test]
    fn test_decode_rra() {
        let decoded = RotateDecoder.decode(0x1F).unwrap();
        let actual_cycles = decoded.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(actual_cycles, 4);
    }

    #[test]
    fn test_invalid_opcode() {
        assert!(RotateDecoder.decode(0xFF).is_err());
    }
}
