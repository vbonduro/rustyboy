#[cfg(test)]
pub mod util {
    use crate::cpu::instructions::adc::opcode::Adc;
    use crate::cpu::instructions::add::opcode::{Add8, Add16, AddSP16};
    use crate::cpu::instructions::sub::opcode::Sub8;
    use crate::cpu::instructions::sbc::opcode::Sbc8;
    use crate::cpu::instructions::cp::opcode::Cp8;
    use crate::cpu::instructions::ld::opcode::Ld8;
    use crate::cpu::instructions::decoder::Decoder;
    use crate::cpu::instructions::instructions::{Error, Instructions};
    use crate::cpu::instructions::opcode::OpCode;
    use crate::cpu::instructions::operand::Operand;

    pub struct FakeCpu {
        operand: Option<Operand>,
        ld8_dest: Option<Operand>,
        ld8_src: Option<Operand>,
    }

    impl FakeCpu {
        pub fn new() -> Self {
            FakeCpu { operand: None, ld8_dest: None, ld8_src: None }
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

        /// Decodes an opcode using the given decoder and validates dest, src, and cycle count for Ld8.
        pub fn test_decode_ld8(&mut self, opcode: u8, decoder: &dyn Decoder, expected_cycles: u8, expected_dest: Operand, expected_src: Operand) {
            let decoded = decoder.decode(opcode).unwrap();
            let actual_cycles = decoded.execute(self).unwrap();
            assert_eq!(self.ld8_dest.unwrap(), expected_dest);
            assert_eq!(self.ld8_src.unwrap(), expected_src);
            assert_eq!(actual_cycles, expected_cycles);
        }

        /// Executes an Ld8 opcode directly and validates dest, src, and cycle count.
        pub fn test_execute_ld8_opcode(&mut self, opcode: &dyn OpCode, expected_cycles: u8, expected_dest: Operand, expected_src: Operand) {
            let actual_cycles = opcode.execute(self).unwrap();
            assert_eq!(self.ld8_dest.unwrap(), expected_dest);
            assert_eq!(self.ld8_src.unwrap(), expected_src);
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

        fn ld8(&mut self, opcode: &Ld8) -> Result<u8, Error> {
            self.ld8_dest = Some(opcode.dest);
            self.ld8_src = Some(opcode.src);
            Ok(opcode.cycles)
        }
    }
}
