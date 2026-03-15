use super::adc::decoder::AdcDecoder;
use super::add::decoder::{Add16Decoder, Add8Decoder, AddSP16Decoder};
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
use super::rotate::decoder::RotateDecoder;
use super::sbc::decoder::Sbc8Decoder;
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
        let opcode = 0xFF; // Example invalid opcode
        assert!(OpCodeDecoder::new().decode(opcode).is_err());
    }
}
