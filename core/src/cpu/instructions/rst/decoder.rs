use alloc::boxed::Box;
use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;

use super::opcode::Rst;

pub struct RstDecoder;

impl Decoder for RstDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let vector = match opcode {
            0xC7 => 0x00,
            0xCF => 0x08,
            0xD7 => 0x10,
            0xDF => 0x18,
            0xE7 => 0x20,
            0xEF => 0x28,
            0xF7 => 0x30,
            0xFF => 0x38,
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Rst { vector, cycles: 16 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    fn cycles(opcode: u8) -> u8 {
        RstDecoder
            .decode(opcode)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap()
    }

    #[test]
    fn test_decode_rst_00() {
        assert_eq!(cycles(0xC7), 16);
    }

    #[test]
    fn test_decode_rst_08() {
        assert_eq!(cycles(0xCF), 16);
    }

    #[test]
    fn test_decode_rst_10() {
        assert_eq!(cycles(0xD7), 16);
    }

    #[test]
    fn test_decode_rst_18() {
        assert_eq!(cycles(0xDF), 16);
    }

    #[test]
    fn test_decode_rst_20() {
        assert_eq!(cycles(0xE7), 16);
    }

    #[test]
    fn test_decode_rst_28() {
        assert_eq!(cycles(0xEF), 16);
    }

    #[test]
    fn test_decode_rst_30() {
        assert_eq!(cycles(0xF7), 16);
    }

    #[test]
    fn test_decode_rst_38() {
        assert_eq!(cycles(0xFF), 16);
    }

    #[test]
    fn test_decode_invalid() {
        assert!(RstDecoder.decode(0x00).is_err());
    }
}
