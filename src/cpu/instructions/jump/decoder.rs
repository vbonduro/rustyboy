use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;

use super::opcode::{Condition, Jump, JumpOp};

pub struct JumpDecoder;

impl Decoder for JumpDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let jump = match opcode {
            // JP nn — unconditional absolute jump, 16 cycles
            0xC3 => Jump {
                op: JumpOp::Jp,
                cycles: 16,
            },
            // JP HL — jump to HL, 4 cycles
            0xE9 => Jump {
                op: JumpOp::JpHl,
                cycles: 4,
            },
            // JP cc, nn — conditional absolute jump, 16 cycles if taken
            0xC2 => Jump {
                op: JumpOp::JpCc(Condition::NZ),
                cycles: 16,
            },
            0xCA => Jump {
                op: JumpOp::JpCc(Condition::Z),
                cycles: 16,
            },
            0xD2 => Jump {
                op: JumpOp::JpCc(Condition::NC),
                cycles: 16,
            },
            0xDA => Jump {
                op: JumpOp::JpCc(Condition::C),
                cycles: 16,
            },
            // JR e — unconditional relative jump, 12 cycles
            0x18 => Jump {
                op: JumpOp::Jr,
                cycles: 12,
            },
            // JR cc, e — conditional relative jump, 12 cycles if taken
            0x20 => Jump {
                op: JumpOp::JrCc(Condition::NZ),
                cycles: 12,
            },
            0x28 => Jump {
                op: JumpOp::JrCc(Condition::Z),
                cycles: 12,
            },
            0x30 => Jump {
                op: JumpOp::JrCc(Condition::NC),
                cycles: 12,
            },
            0x38 => Jump {
                op: JumpOp::JrCc(Condition::C),
                cycles: 12,
            },
            _ => return Err(Error::InvalidOpcode(opcode)),
        };

        Ok(Box::new(jump))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_jp_nn() {
        let cycles = JumpDecoder
            .decode(0xC3)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_decode_jp_hl() {
        let cycles = JumpDecoder
            .decode(0xE9)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_decode_jp_nz_nn() {
        let cycles = JumpDecoder
            .decode(0xC2)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_decode_jp_z_nn() {
        let cycles = JumpDecoder
            .decode(0xCA)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_decode_jp_nc_nn() {
        let cycles = JumpDecoder
            .decode(0xD2)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_decode_jp_c_nn() {
        let cycles = JumpDecoder
            .decode(0xDA)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_decode_jr_e() {
        let cycles = JumpDecoder
            .decode(0x18)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_decode_jr_nz_e() {
        let cycles = JumpDecoder
            .decode(0x20)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_decode_jr_z_e() {
        let cycles = JumpDecoder
            .decode(0x28)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_decode_jr_nc_e() {
        let cycles = JumpDecoder
            .decode(0x30)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_decode_jr_c_e() {
        let cycles = JumpDecoder
            .decode(0x38)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_decode_invalid_opcode() {
        assert!(JumpDecoder.decode(0xFF).is_err());
    }
}
