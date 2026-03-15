use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::instructions::{Error, Instructions};

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Condition {
    NZ,
    Z,
    NC,
    C,
}

#[derive(Debug, PartialEq)]
pub enum JumpOp {
    /// JP nn — absolute jump to 16-bit immediate
    Jp,
    /// JP HL — jump to address in HL
    JpHl,
    /// JP cc, nn — conditional absolute jump
    JpCc(Condition),
    /// JR e — relative jump with signed 8-bit offset
    Jr,
    /// JR cc, e — conditional relative jump
    JrCc(Condition),
}

pub struct Jump {
    pub op: JumpOp,
    /// Cycles when the jump is taken.
    pub cycles: u8,
}

impl OpCode for Jump {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.jump(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_jp_dispatches_to_jump() {
        let opcode = Jump { op: JumpOp::Jp, cycles: 16 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_execute_jp_hl_dispatches_to_jump() {
        let opcode = Jump { op: JumpOp::JpHl, cycles: 4 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_jp_cc_nz_dispatches_to_jump() {
        let opcode = Jump { op: JumpOp::JpCc(Condition::NZ), cycles: 16 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_execute_jr_dispatches_to_jump() {
        let opcode = Jump { op: JumpOp::Jr, cycles: 12 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_execute_jr_cc_z_dispatches_to_jump() {
        let opcode = Jump { op: JumpOp::JrCc(Condition::Z), cycles: 12 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 12);
    }
}
