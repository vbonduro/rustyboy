#[cfg(test)]
pub mod util {
    use crate::cpu::instructions::adc::opcode::Adc;
    use crate::cpu::instructions::add::opcode::{Add8, Add16, AddSP16};
    use crate::cpu::instructions::sub::opcode::Sub8;
    use crate::cpu::instructions::sbc::opcode::Sbc8;
    use crate::cpu::instructions::cp::opcode::Cp8;
    use crate::cpu::instructions::decoder::Decoder;
    use crate::cpu::instructions::instructions::{Error, Instructions};
    use crate::cpu::instructions::opcode::OpCode;
    use crate::cpu::instructions::operand::Operand;

    pub struct FakeCpu {
        operand: Option<Operand>,
    }

    impl FakeCpu {
        pub fn new() -> Self {
            FakeCpu{operand: None}
        }

        pub fn test_decode_operand(&mut self, opcode: u8, decoder: &dyn Decoder, expected_cycles: u8, expected_operand: Operand) {
            self.test_execute_opcode(&*decoder.decode(opcode).unwrap(), expected_cycles, expected_operand);
        }

        /// Executes the given opcode against this FakeCpu and validates that the expected_operand was present in the
        /// instruction adn that the expected_cycles are returned.
        pub fn test_execute_opcode(&mut self, opcode: &dyn OpCode, expected_cycles: u8, expected_operand: Operand) {
            let actual_cycles = opcode.execute(self).unwrap();

            assert_eq!(self.operand.unwrap(), expected_operand);
            assert_eq!(actual_cycles, expected_cycles);  
        }
    }

    impl Instructions for FakeCpu {
        fn add8(&mut self, opcode: &Add8) -> Result<u8, Error> {
            self.operand = Some(opcode.operand);
            Ok(opcode.cycles)
        }

        fn add16(&mut self, opcode: &Add16) -> Result<u8, Error> {
            self.operand = Some(opcode.operand);
            Ok(opcode.cycles)
        }

        fn add_sp16(&mut self, opcode: &AddSP16) -> Result<u8, Error> {
            self.operand = Some(opcode.operand);
            Ok(opcode.cycles)
        }

        fn adc(&mut self, opcode: &Adc) -> Result<u8, Error> {
            self.operand = Some(opcode.operand);
            Ok(opcode.cycles)
        }

        fn sub8(&mut self, opcode: &Sub8) -> Result<u8, Error> {
            self.operand = Some(opcode.operand);
            Ok(opcode.cycles)
        }

        fn sbc8(&mut self, opcode: &Sbc8) -> Result<u8, Error> {
            self.operand = Some(opcode.operand);
            Ok(opcode.cycles)
        }

        fn cp8(&mut self, opcode: &Cp8) -> Result<u8, Error> {
            self.operand = Some(opcode.operand);
            Ok(opcode.cycles)
        }
    }
}
