use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::Register8;

use super::opcode::{CbInstruction, CbOp, CbTarget};

pub struct CbDecoder;

/// Map the low 3 bits of a CB opcode to a register target.
fn target(opcode: u8) -> CbTarget {
    match opcode & 0x07 {
        0 => CbTarget::Reg(Register8::B),
        1 => CbTarget::Reg(Register8::C),
        2 => CbTarget::Reg(Register8::D),
        3 => CbTarget::Reg(Register8::E),
        4 => CbTarget::Reg(Register8::H),
        5 => CbTarget::Reg(Register8::L),
        6 => CbTarget::HLMem,
        7 => CbTarget::Reg(Register8::A),
        _ => unreachable!(),
    }
}

impl Decoder for CbDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let tgt = target(opcode);
        let is_hl = matches!(tgt, CbTarget::HLMem);

        let (op, cycles) = match opcode >> 3 {
            0x00 => (CbOp::Rlc, if is_hl { 16 } else { 8 }),
            0x01 => (CbOp::Rrc, if is_hl { 16 } else { 8 }),
            0x02 => (CbOp::Rl, if is_hl { 16 } else { 8 }),
            0x03 => (CbOp::Rr, if is_hl { 16 } else { 8 }),
            0x04 => (CbOp::Sla, if is_hl { 16 } else { 8 }),
            0x05 => (CbOp::Sra, if is_hl { 16 } else { 8 }),
            0x06 => (CbOp::Swap, if is_hl { 16 } else { 8 }),
            0x07 => (CbOp::Srl, if is_hl { 16 } else { 8 }),
            // BIT b, r — (HL) is 12 cycles, registers are 8
            b @ 0x08..=0x0F => (CbOp::Bit(b as u8 - 0x08), if is_hl { 12 } else { 8 }),
            // RES b, r
            b @ 0x10..=0x17 => (CbOp::Res(b as u8 - 0x10), if is_hl { 16 } else { 8 }),
            // SET b, r
            b @ 0x18..=0x1F => (CbOp::Set(b as u8 - 0x18), if is_hl { 16 } else { 8 }),
            _ => unreachable!(),
        };

        Ok(Box::new(CbInstruction {
            op,
            target: tgt,
            cycles,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    fn decode_cycles(opcode: u8) -> u8 {
        CbDecoder
            .decode(opcode)
            .unwrap()
            .execute(&mut FakeCpu::new())
            .unwrap()
    }

    #[test]
    fn test_rlc_b_cycles() {
        assert_eq!(decode_cycles(0x00), 8);
    }

    #[test]
    fn test_rlc_hl_mem_cycles() {
        assert_eq!(decode_cycles(0x06), 16);
    }

    #[test]
    fn test_rrc_c_cycles() {
        assert_eq!(decode_cycles(0x09), 8);
    }

    #[test]
    fn test_rl_d_cycles() {
        assert_eq!(decode_cycles(0x12), 8);
    }

    #[test]
    fn test_rr_hl_cycles() {
        assert_eq!(decode_cycles(0x1E), 16);
    }

    #[test]
    fn test_sla_h_cycles() {
        assert_eq!(decode_cycles(0x24), 8);
    }

    #[test]
    fn test_sra_l_cycles() {
        assert_eq!(decode_cycles(0x2D), 8);
    }

    #[test]
    fn test_swap_a_cycles() {
        assert_eq!(decode_cycles(0x37), 8);
    }

    #[test]
    fn test_swap_hl_cycles() {
        assert_eq!(decode_cycles(0x36), 16);
    }

    #[test]
    fn test_srl_b_cycles() {
        assert_eq!(decode_cycles(0x38), 8);
    }

    #[test]
    fn test_bit_0_b_cycles() {
        assert_eq!(decode_cycles(0x40), 8);
    }

    #[test]
    fn test_bit_7_a_cycles() {
        assert_eq!(decode_cycles(0x7F), 8);
    }

    #[test]
    fn test_bit_3_hl_cycles() {
        assert_eq!(decode_cycles(0x5E), 12);
    }

    #[test]
    fn test_res_0_b_cycles() {
        assert_eq!(decode_cycles(0x80), 8);
    }

    #[test]
    fn test_res_7_hl_cycles() {
        assert_eq!(decode_cycles(0xBE), 16);
    }

    #[test]
    fn test_set_0_b_cycles() {
        assert_eq!(decode_cycles(0xC0), 8);
    }

    #[test]
    fn test_set_7_a_cycles() {
        assert_eq!(decode_cycles(0xFF), 8);
    }

    #[test]
    fn test_set_3_hl_cycles() {
        assert_eq!(decode_cycles(0xDE), 16);
    }
}
