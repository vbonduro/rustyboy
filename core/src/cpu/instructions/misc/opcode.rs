use crate::cpu::instructions::instructions::{Error, Instructions};
use crate::cpu::instructions::opcode::OpCode;
use crate::memory::memory::GameBoyMemory;

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum MiscOp {
    Nop,
    Halt,
    Stop,
    Daa,
    Cpl,
    Scf,
    Ccf,
    Di,
    Ei,
}

pub struct Misc {
    pub op: MiscOp,
    pub cycles: u8,
}

impl OpCode for Misc {
    fn execute(
        &self,
        cpu: &mut dyn Instructions,
        _memory: &mut GameBoyMemory,
    ) -> Result<u8, Error> {
        cpu.misc(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::test::util::FakeCpu;
    use crate::memory::memory::GameBoyMemory;

    #[test]
    fn test_execute_nop() {
        let opcode = Misc {
            op: MiscOp::Nop,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_halt() {
        let opcode = Misc {
            op: MiscOp::Halt,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_stop() {
        let opcode = Misc {
            op: MiscOp::Stop,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_daa() {
        let opcode = Misc {
            op: MiscOp::Daa,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_cpl() {
        let opcode = Misc {
            op: MiscOp::Cpl,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_scf() {
        let opcode = Misc {
            op: MiscOp::Scf,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_ccf() {
        let opcode = Misc {
            op: MiscOp::Ccf,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_di() {
        let opcode = Misc {
            op: MiscOp::Di,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }

    #[test]
    fn test_execute_ei() {
        let opcode = Misc {
            op: MiscOp::Ei,
            cycles: 4,
        };
        let cycles = opcode
            .execute(&mut FakeCpu::new(), &mut GameBoyMemory::new())
            .unwrap();
        assert_eq!(cycles, 4);
    }
}
