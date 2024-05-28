use crate::cpu::opcodes::opcode::OpCode;

use super::add::{Add16Decoder, Add8Decoder, AddSP16Decoder};
use super::adc::AdcDecoder;
use super::decoder::{Decoder, Error};

pub struct OpCodeDecoder {
    opcodes: Vec<Box<dyn Decoder>>,
}

impl OpCodeDecoder {
    pub fn new() -> Self {
        OpCodeDecoder {
            opcodes: vec![
                Box::new(Add8Decoder {}),
                Box::new(Add16Decoder {}),
                Box::new(AddSP16Decoder {}),
                Box::new(AdcDecoder{} ),
            ],
        }
    }
}

impl Decoder for OpCodeDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        self.opcodes.iter().find_map(|decoder| decoder.decode(opcode).ok()).ok_or_else(||Error::InvalidOpcode(opcode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::opcodes::operand::*;
    use crate::cpu::opcodes::test_util::operand_test_util::FakeCpu;

    #[test]
    fn test_from_add8() {
        let opcode = 0x80;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(opcode, &OpCodeDecoder::new(), expected_cycles, expected_operand)
    }

    #[test]
    fn test_from_add16_bc() {
        let opcode = 0x09;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::BC);

        FakeCpu::new().test_decode_operand(opcode, &OpCodeDecoder::new(), expected_cycles, expected_operand);
    }

    #[test]
    fn test_from_add16_de() {
        let opcode = 0x19;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::DE);

        FakeCpu::new().test_decode_operand(opcode, &OpCodeDecoder::new(), expected_cycles, expected_operand);
    }

    #[test]
    fn test_from_add16_hl() {
        let opcode = 0x29;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::HL);

        FakeCpu::new().test_decode_operand(opcode, &OpCodeDecoder::new(), expected_cycles, expected_operand);
    }

    #[test]
    fn test_from_add16_sp() {
        let opcode = 0x39;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::SP);

        FakeCpu::new().test_decode_operand(opcode, &OpCodeDecoder::new(), expected_cycles, expected_operand);
    }

    #[test]
    fn test_from_adc_a() {
        let opcode = 0x8F;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);

        FakeCpu::new().test_decode_operand(opcode, &OpCodeDecoder::new(), expected_cycles, expected_operand);
    }
    
    #[test]
    fn test_invalid_opcode() {
        let opcode = 0xFF; // Example invalid opcode
        assert!(OpCodeDecoder::new().decode(opcode).is_err());
    }
}
