use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};

use super::cb::decoder::CbDecoder;
use super::adc::decoder::AdcDecoder;
use super::add::decoder::{Add16Decoder, Add8Decoder, AddSP16Decoder};
use super::call::decoder::CallDecoder;
use super::cp::decoder::Cp8Decoder;
use super::decoder::{Decoder, Error};
use super::inc_dec::decoder::{Dec16Decoder, Dec8Decoder, Inc16Decoder, Inc8Decoder};
use super::jump::decoder::JumpDecoder;
use super::ld::decoder::Ld8Decoder;
use super::ld16::decoder::Ld16Decoder;
use super::logic::and::decoder::And8Decoder;
use super::logic::or::decoder::Or8Decoder;
use super::logic::xor::decoder::Xor8Decoder;
use super::misc::decoder::MiscDecoder;
use super::opcode::OpCode;
use super::ret::decoder::RetDecoder;
use super::rotate::decoder::RotateDecoder;
use super::rst::decoder::RstDecoder;
use super::sbc::decoder::Sbc8Decoder;
use super::stack::decoder::{Pop16Decoder, Push16Decoder};
use super::sub::decoder::Sub8Decoder;

pub struct OpCodeDecoder {
    opcodes: Vec<Box<dyn Decoder>>,
}

impl OpCodeDecoder {
    pub fn new() -> Self {
        OpCodeDecoder {
            opcodes: vec![
                Box::new(Ld8Decoder {}),
                Box::new(Add8Decoder {}),
                Box::new(Add16Decoder {}),
                Box::new(AddSP16Decoder {}),
                Box::new(AdcDecoder {}),
                Box::new(Sub8Decoder {}),
                Box::new(Sbc8Decoder {}),
                Box::new(Cp8Decoder {}),
                Box::new(Ld16Decoder {}),
                Box::new(Inc8Decoder {}),
                Box::new(Dec8Decoder {}),
                Box::new(Inc16Decoder {}),
                Box::new(Dec16Decoder {}),
                Box::new(RotateDecoder {}),
                Box::new(JumpDecoder {}),
                Box::new(And8Decoder {}),
                Box::new(Or8Decoder {}),
                Box::new(Xor8Decoder {}),
                Box::new(MiscDecoder {}),
                Box::new(Push16Decoder {}),
                Box::new(Pop16Decoder {}),
                Box::new(CallDecoder),
                Box::new(RetDecoder),
                Box::new(RstDecoder),
            ],
        }
    }
}

impl Decoder for OpCodeDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        self.opcodes
            .iter()
            .find_map(|decoder| decoder.decode(opcode).ok())
            .ok_or_else(|| Error::InvalidOpcode(opcode))
    }
}

/// Pre-decoded opcode table: 256-entry `Arc` arrays built once at startup.
/// `get()` / `get_cb()` are O(1) array index + one `Arc::clone` (atomic increment)
/// — no heap allocation per call.
pub struct OpCodeTable {
    main: Vec<Option<Arc<dyn OpCode>>>,
    cb:   Vec<Option<Arc<dyn OpCode>>>,
}

impl OpCodeTable {
    /// Build the table from any `Decoder` for the main (non-CB) opcodes.
    /// CB opcodes are always decoded via `CbDecoder`.
    pub fn from_decoder(decoder: &dyn Decoder) -> Self {
        let mut main: Vec<Option<Arc<dyn OpCode>>> = Vec::with_capacity(256);
        for i in 0..=255u8 {
            main.push(decoder.decode(i).ok().map(Arc::from));
        }
        let mut cb: Vec<Option<Arc<dyn OpCode>>> = Vec::with_capacity(256);
        for i in 0..=255u8 {
            cb.push(CbDecoder.decode(i).ok().map(Arc::from));
        }
        Self { main, cb }
    }

    /// Return a clone of the pre-decoded handler for this opcode.
    /// The returned `Arc` has a lifetime independent of `self`, so callers can
    /// hold it while mutably borrowing the CPU.
    pub fn get(&self, opcode: u8) -> Result<Arc<dyn OpCode>, Error> {
        self.main[opcode as usize]
            .as_ref()
            .map(Arc::clone)
            .ok_or(Error::InvalidOpcode(opcode))
    }

    pub fn get_cb(&self, opcode: u8) -> Result<Arc<dyn OpCode>, Error> {
        self.cb[opcode as usize]
            .as_ref()
            .map(Arc::clone)
            .ok_or(Error::InvalidOpcode(opcode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::operand::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_from_add8() {
        let opcode = 0x80;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        )
    }

    #[test]
    fn test_from_add16_bc() {
        let opcode = 0x09;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::BC);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_add16_de() {
        let opcode = 0x19;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::DE);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_add16_hl() {
        let opcode = 0x29;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::HL);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_add16_sp() {
        let opcode = 0x39;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::SP);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_adc_a() {
        let opcode = 0x8F;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_sub8_b() {
        let opcode = 0x90;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_sbc8_b() {
        let opcode = 0x98;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_cp8_b() {
        let opcode = 0xB8;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_invalid_opcode() {
        let opcode = 0xFC; // Truly unimplemented opcode
        assert!(OpCodeDecoder::new().decode(opcode).is_err());
    }
}
