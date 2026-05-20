use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::jump::opcode::Condition;
use crate::cpu::instructions::opcode::OpCode;
use crate::memory::memory::GameBoyMemory;

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
#[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn execute(&self, cpu: &mut dyn Instructions, memory: &mut GameBoyMemory) -> Result<u8, Error> {
        cpu.call(self, memory)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;
    use crate::memory::memory::GameBoyMemory;

    #[test]
    fn test_execute_call_dispatches() {
        let opcode = Call {
            op: CallOp::Call,
            cycles: 24,
        };
        assert_eq!(
            opcode
                .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
                .unwrap(),
            24
        );
    }

    #[test]
    fn test_execute_call_cc_dispatches() {
        let opcode = Call {
            op: CallOp::CallCc(Condition::Z),
            cycles: 24,
        };
        assert_eq!(
            opcode
                .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
                .unwrap(),
            24
        );
    }
}
