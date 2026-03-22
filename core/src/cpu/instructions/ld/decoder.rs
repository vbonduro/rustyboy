use alloc::boxed::Box;
use crate::cpu::instructions::decoder::{Decoder, Error};
use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::*;

use super::opcode::Ld8;

fn reg_from_bits(bits: u8) -> Option<Operand> {
    match bits {
        0b000 => Some(Operand::Register8(Register8::B)),
        0b001 => Some(Operand::Register8(Register8::C)),
        0b010 => Some(Operand::Register8(Register8::D)),
        0b011 => Some(Operand::Register8(Register8::E)),
        0b100 => Some(Operand::Register8(Register8::H)),
        0b101 => Some(Operand::Register8(Register8::L)),
        0b110 => Some(Operand::Memory(Memory::HL)),
        0b111 => Some(Operand::Register8(Register8::A)),
        _ => None,
    }
}

pub struct Ld8Decoder;

impl Decoder for Ld8Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        let mut cycles = 8;
        let (dest, src) = match opcode {
            0x06 => (Operand::Register8(Register8::B), Operand::Imm8),
            0x0E => (Operand::Register8(Register8::C), Operand::Imm8),
            0x16 => (Operand::Register8(Register8::D), Operand::Imm8),
            0x1E => (Operand::Register8(Register8::E), Operand::Imm8),
            0x26 => (Operand::Register8(Register8::H), Operand::Imm8),
            0x2E => (Operand::Register8(Register8::L), Operand::Imm8),
            0x36 => {
                cycles = 12;
                (Operand::Memory(Memory::HL), Operand::Imm8)
            }
            0x3E => (Operand::Register8(Register8::A), Operand::Imm8),
            0x40..=0x7F if opcode != 0x76 => {
                let dest =
                    reg_from_bits((opcode >> 3) & 0x07).ok_or(Error::InvalidOpcode(opcode))?;
                let src = reg_from_bits(opcode & 0x07).ok_or(Error::InvalidOpcode(opcode))?;
                if !matches!(
                    (&dest, &src),
                    (Operand::Memory(_), _) | (_, Operand::Memory(_))
                ) {
                    cycles = 4;
                }
                (dest, src)
            }
            _ => return Err(Error::InvalidOpcode(opcode)),
        };

        Ok(Box::new(Ld8 { dest, src, cycles }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    // LD B, C — register to register, 4 cycles
    #[test]
    fn test_decode_ld8_b_c() {
        // dest=B (001 => wait, B=000), src=C (001)
        // opcode = 0x40 | (B<<3) | C = 0x40 | (0<<3) | 1 = 0x41
        let opcode = 0x41; // LD B, C
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            4,
            Operand::Register8(Register8::B),
            Operand::Register8(Register8::C),
        );
    }

    // LD A, (HL) — load from memory, 8 cycles
    #[test]
    fn test_decode_ld8_a_mem_hl() {
        // dest=A (111), src=(HL) (110)
        // opcode = 0x40 | (7<<3) | 6 = 0x40 | 0x38 | 6 = 0x7E
        let opcode = 0x7E;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Register8(Register8::A),
            Operand::Memory(Memory::HL),
        );
    }

    // LD (HL), A — store to memory, 8 cycles
    #[test]
    fn test_decode_ld8_mem_hl_a() {
        // dest=(HL) (110), src=A (111)
        // opcode = 0x40 | (6<<3) | 7 = 0x40 | 0x30 | 7 = 0x77
        let opcode = 0x77;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Memory(Memory::HL),
            Operand::Register8(Register8::A),
        );
    }

    // LD B, B — register to itself, 4 cycles
    #[test]
    fn test_decode_ld8_b_b() {
        let opcode = 0x40; // LD B, B
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            4,
            Operand::Register8(Register8::B),
            Operand::Register8(Register8::B),
        );
    }

    // HALT (0x76) must return error
    #[test]
    fn test_decode_halt_returns_err() {
        assert!(Ld8Decoder.decode(0x76).is_err());
    }

    // LD B, n (immediate), 8 cycles
    #[test]
    fn test_decode_ld8_b_imm8() {
        let opcode = 0x06;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Register8(Register8::B),
            Operand::Imm8,
        );
    }

    // LD C, n (immediate), 8 cycles
    #[test]
    fn test_decode_ld8_c_imm8() {
        let opcode = 0x0E;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Register8(Register8::C),
            Operand::Imm8,
        );
    }

    // LD D, n (immediate), 8 cycles
    #[test]
    fn test_decode_ld8_d_imm8() {
        let opcode = 0x16;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Register8(Register8::D),
            Operand::Imm8,
        );
    }

    // LD E, n (immediate), 8 cycles
    #[test]
    fn test_decode_ld8_e_imm8() {
        let opcode = 0x1E;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Register8(Register8::E),
            Operand::Imm8,
        );
    }

    // LD H, n (immediate), 8 cycles
    #[test]
    fn test_decode_ld8_h_imm8() {
        let opcode = 0x26;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Register8(Register8::H),
            Operand::Imm8,
        );
    }

    // LD L, n (immediate), 8 cycles
    #[test]
    fn test_decode_ld8_l_imm8() {
        let opcode = 0x2E;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Register8(Register8::L),
            Operand::Imm8,
        );
    }

    // LD A, n (immediate), 8 cycles
    #[test]
    fn test_decode_ld8_a_imm8() {
        let opcode = 0x3E;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            8,
            Operand::Register8(Register8::A),
            Operand::Imm8,
        );
    }

    // LD (HL), n (immediate to memory), 12 cycles
    #[test]
    fn test_decode_ld8_mem_hl_imm8() {
        let opcode = 0x36;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            12,
            Operand::Memory(Memory::HL),
            Operand::Imm8,
        );
    }

    // Invalid opcode
    #[test]
    fn test_decode_invalid_opcode() {
        assert!(Ld8Decoder.decode(0xFF).is_err());
    }

    // Boundary: first opcode in range (0x40 = LD B, B)
    #[test]
    fn test_decode_ld8_boundary_0x40() {
        let opcode = 0x40;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            4,
            Operand::Register8(Register8::B),
            Operand::Register8(Register8::B),
        );
    }

    // Boundary: last opcode in range (0x7F = LD A, A)
    #[test]
    fn test_decode_ld8_boundary_0x7f() {
        let opcode = 0x7F;
        FakeCpu::new().test_decode_ld8(
            opcode,
            &Ld8Decoder,
            4,
            Operand::Register8(Register8::A),
            Operand::Register8(Register8::A),
        );
    }

    // All register source/dest mappings in the 0x40–0x7F range
    #[test]
    fn test_decode_ld8_all_reg_to_reg() {
        let regs = [
            Register8::B,
            Register8::C,
            Register8::D,
            Register8::E,
            Register8::H,
            Register8::L,
            Register8::A,
        ];
        // bits: B=0, C=1, D=2, E=3, H=4, L=5, (HL)=6, A=7
        let bits: [u8; 7] = [0, 1, 2, 3, 4, 5, 7];
        for (dest_reg, dest_bit) in regs.iter().zip(bits.iter()) {
            for (src_reg, src_bit) in regs.iter().zip(bits.iter()) {
                let opcode = 0x40 | (dest_bit << 3) | src_bit;
                FakeCpu::new().test_decode_ld8(
                    opcode,
                    &Ld8Decoder,
                    4,
                    Operand::Register8(*dest_reg),
                    Operand::Register8(*src_reg),
                );
            }
        }
    }
}
