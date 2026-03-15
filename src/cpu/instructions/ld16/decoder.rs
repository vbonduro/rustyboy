use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::Register16;

use super::opcode::{Ld16, Ld16Op};

pub struct Ld16Decoder;

impl Decoder for Ld16Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let (op, cycles) = match opcode {
            0x01 => (
                Ld16Op::RrImm16 {
                    dest: Register16::BC,
                },
                12,
            ),
            0x11 => (
                Ld16Op::RrImm16 {
                    dest: Register16::DE,
                },
                12,
            ),
            0x21 => (
                Ld16Op::RrImm16 {
                    dest: Register16::HL,
                },
                12,
            ),
            0x31 => (
                Ld16Op::RrImm16 {
                    dest: Register16::SP,
                },
                12,
            ),
            0x08 => (Ld16Op::NnSp, 20),
            0xF9 => (Ld16Op::SpHl, 8),
            0xF8 => (Ld16Op::HlSpE, 12),
            0x02 => (Ld16Op::BcA, 8),
            0x12 => (Ld16Op::DeA, 8),
            0x0A => (Ld16Op::ABc, 8),
            0x1A => (Ld16Op::ADe, 8),
            0x22 => (Ld16Op::HliA, 8),
            0x32 => (Ld16Op::HldA, 8),
            0x2A => (Ld16Op::AHli, 8),
            0x3A => (Ld16Op::AHld, 8),
            0xEA => (Ld16Op::NnA, 16),
            0xFA => (Ld16Op::ANn, 16),
            0xE0 => (Ld16Op::LdhNA, 12),
            0xF0 => (Ld16Op::LdhAN, 12),
            0xE2 => (Ld16Op::LdCA, 8),
            0xF2 => (Ld16Op::LdAC, 8),
            _ => return Err(Error::InvalidOpcode(opcode)),
        };

        Ok(Box::new(Ld16 { op, cycles }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_decode_ld_bc_nn() {
        let decoded = Ld16Decoder.decode(0x01).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 12);
    }

    #[test]
    fn test_decode_ld_de_nn() {
        let decoded = Ld16Decoder.decode(0x11).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 12);
    }

    #[test]
    fn test_decode_ld_hl_nn() {
        let decoded = Ld16Decoder.decode(0x21).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 12);
    }

    #[test]
    fn test_decode_ld_sp_nn() {
        let decoded = Ld16Decoder.decode(0x31).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 12);
    }

    #[test]
    fn test_decode_ld_nn_sp() {
        let decoded = Ld16Decoder.decode(0x08).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 20);
    }

    #[test]
    fn test_decode_ld_sp_hl() {
        let decoded = Ld16Decoder.decode(0xF9).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_hl_sp_e() {
        let decoded = Ld16Decoder.decode(0xF8).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 12);
    }

    #[test]
    fn test_decode_ld_bc_a() {
        let decoded = Ld16Decoder.decode(0x02).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_de_a() {
        let decoded = Ld16Decoder.decode(0x12).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_a_bc() {
        let decoded = Ld16Decoder.decode(0x0A).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_a_de() {
        let decoded = Ld16Decoder.decode(0x1A).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_hli_a() {
        let decoded = Ld16Decoder.decode(0x22).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_hld_a() {
        let decoded = Ld16Decoder.decode(0x32).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_a_hli() {
        let decoded = Ld16Decoder.decode(0x2A).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_a_hld() {
        let decoded = Ld16Decoder.decode(0x3A).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_nn_a() {
        let decoded = Ld16Decoder.decode(0xEA).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 16);
    }

    #[test]
    fn test_decode_ld_a_nn() {
        let decoded = Ld16Decoder.decode(0xFA).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 16);
    }

    #[test]
    fn test_decode_ldh_n_a() {
        let decoded = Ld16Decoder.decode(0xE0).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 12);
    }

    #[test]
    fn test_decode_ldh_a_n() {
        let decoded = Ld16Decoder.decode(0xF0).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 12);
    }

    #[test]
    fn test_decode_ld_c_a() {
        let decoded = Ld16Decoder.decode(0xE2).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_ld_a_c() {
        let decoded = Ld16Decoder.decode(0xF2).unwrap();
        assert_eq!(decoded.execute(&mut FakeCpu::new()).unwrap(), 8);
    }

    #[test]
    fn test_decode_invalid_opcode() {
        assert!(Ld16Decoder.decode(0xFF).is_err());
    }
}
