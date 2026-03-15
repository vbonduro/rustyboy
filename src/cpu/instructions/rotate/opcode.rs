use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::opcode::OpCode;

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum RotateOp {
    Rlca,
    Rla,
    Rrca,
    Rra,
}

pub struct Rotate {
    pub op: RotateOp,
    pub cycles: u8,
}

impl OpCode for Rotate {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.rotate_accumulator(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_rlca() {
        let opcode = Rotate {
            op: RotateOp::Rlca,
            cycles: 4,
        };
        let actual_cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(actual_cycles, 4);
    }

    #[test]
    fn test_execute_rla() {
        let opcode = Rotate {
            op: RotateOp::Rla,
            cycles: 4,
        };
        let actual_cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(actual_cycles, 4);
    }

    #[test]
    fn test_execute_rrca() {
        let opcode = Rotate {
            op: RotateOp::Rrca,
            cycles: 4,
        };
        let actual_cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(actual_cycles, 4);
    }

    #[test]
    fn test_execute_rra() {
        let opcode = Rotate {
            op: RotateOp::Rra,
            cycles: 4,
        };
        let actual_cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(actual_cycles, 4);
    }
}
