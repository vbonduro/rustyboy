use alloc::boxed::Box;
use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::jump::opcode::Condition;
use crate::cpu::instructions::opcode::OpCode;

use super::opcode::{Call, CallOp};

pub struct CallDecoder;

impl Decoder for CallDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let op = match opcode {
            0xCD => CallOp::Call,
            0xC4 => CallOp::CallCc(Condition::NZ),
            0xCC => CallOp::CallCc(Condition::Z),
            0xD4 => CallOp::CallCc(Condition::NC),
            0xDC => CallOp::CallCc(Condition::C),
            _ => return Err(Error::InvalidOpcode(opcode)),
        };
        Ok(Box::new(Call { op, cycles: 24 }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    fn cycles(opcode: u8) -> u8 {
        CallDecoder
            .decode(opcode)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap()
    }

    #[test]
    fn test_decode_call_nn() {
        assert_eq!(cycles(0xCD), 24);
    }

    #[test]
    fn test_decode_call_nz() {
        assert_eq!(cycles(0xC4), 24);
    }

    #[test]
    fn test_decode_call_z() {
        assert_eq!(cycles(0xCC), 24);
    }

    #[test]
    fn test_decode_call_nc() {
        assert_eq!(cycles(0xD4), 24);
    }

    #[test]
    fn test_decode_call_c() {
        assert_eq!(cycles(0xDC), 24);
    }

    #[test]
    fn test_decode_invalid() {
        assert!(CallDecoder.decode(0x00).is_err());
    }
}
