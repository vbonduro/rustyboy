use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::jump::opcode::Condition;
use crate::cpu::instructions::opcode::OpCode;

#[derive(Debug, PartialEq)]
pub enum RetOp {
    /// RET — unconditional return (0xC9)
    Ret,
    /// RET cc — conditional return
    RetCc(Condition),
    /// RETI — return and enable interrupts (0xD9)
    Reti,
}

pub struct Ret {
    pub op: RetOp,
    pub cycles: u8,
}

impl OpCode for Ret {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.ret(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_ret_dispatches() {
        let opcode = Ret {
            op: RetOp::Ret,
            cycles: 16,
        };
        assert_eq!(opcode.execute(&mut FakeCpu::new()).unwrap(), 16);
    }

    #[test]
    fn test_execute_ret_cc_dispatches() {
        let opcode = Ret {
            op: RetOp::RetCc(Condition::NZ),
            cycles: 20,
        };
        assert_eq!(opcode.execute(&mut FakeCpu::new()).unwrap(), 20);
    }

    #[test]
    fn test_execute_reti_dispatches() {
        let opcode = Ret {
            op: RetOp::Reti,
            cycles: 16,
        };
        assert_eq!(opcode.execute(&mut FakeCpu::new()).unwrap(), 16);
    }
}
