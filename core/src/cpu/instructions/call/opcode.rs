use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::jump::opcode::Condition;
use crate::cpu::instructions::opcode::OpCode;

#[derive(Debug, PartialEq)]
pub enum CallOp {
    /// CALL nn — unconditional call (0xCD)
    Call,
    /// CALL cc, nn — conditional call
    CallCc(Condition),
}

pub struct Call {
    pub op: CallOp,
    pub cycles: u8,
}

impl OpCode for Call {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.call(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_call_dispatches() {
        let opcode = Call {
            op: CallOp::Call,
            cycles: 24,
        };
        assert_eq!(opcode.execute(&mut FakeCpu::new()).unwrap(), 24);
    }

    #[test]
    fn test_execute_call_cc_dispatches() {
        let opcode = Call {
            op: CallOp::CallCc(Condition::Z),
            cycles: 24,
        };
        assert_eq!(opcode.execute(&mut FakeCpu::new()).unwrap(), 24);
    }
}
