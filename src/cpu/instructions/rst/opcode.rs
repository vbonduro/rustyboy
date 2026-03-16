use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::opcode::OpCode;

pub struct Rst {
    pub vector: u8,
    pub cycles: u8,
}

impl OpCode for Rst {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.rst(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_rst_dispatches() {
        let opcode = Rst {
            vector: 0x08,
            cycles: 16,
        };
        assert_eq!(opcode.execute(&mut FakeCpu::new()).unwrap(), 16);
    }
}
