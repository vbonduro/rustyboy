#[cfg(test)]
pub mod operand_test_util {
    use crate::cpu::cpu::{Cpu, Error};
    use crate::cpu::opcodes::add::{Add8, Add16, AddSP16};
    use crate::cpu::opcodes::decoders::decoder::Decoder;
    use crate::cpu::opcodes::operand::Operand;
    use crate::cpu::opcodes::opcode::OpCode;

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

    impl Cpu for FakeCpu {
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
    }
}
