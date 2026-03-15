use crate::cpu::instructions::opcode::OpCode;
use crate::cpu::instructions::operand::Register16;
use crate::cpu::instructions::instructions::{Error, Instructions};

pub enum Ld16Op {
    RrImm16 { dest: Register16 },   // LD rr, nn
    NnSp,                            // LD (nn), SP
    SpHl,                            // LD SP, HL
    HlSpE,                           // LD HL, SP+e
    BcA,                             // LD (BC), A
    DeA,                             // LD (DE), A
    ABc,                             // LD A, (BC)
    ADe,                             // LD A, (DE)
    HliA,                            // LD (HL+), A
    HldA,                            // LD (HL-), A
    AHli,                            // LD A, (HL+)
    AHld,                            // LD A, (HL-)
    NnA,                             // LD (nn), A
    ANn,                             // LD A, (nn)
    LdhNA,                           // LDH (n), A
    LdhAN,                           // LDH A, (n)
    LdCA,                            // LD (C), A
    LdAC,                            // LD A, (C)
}

pub struct Ld16 {
    pub op: Ld16Op,
    pub cycles: u8,
}

impl OpCode for Ld16 {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error> {
        cpu.ld16(&self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::operand::Register16;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_execute_ld16_rr_imm16_bc() {
        let opcode = Ld16 { op: Ld16Op::RrImm16 { dest: Register16::BC }, cycles: 12 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_execute_ld16_nn_sp() {
        let opcode = Ld16 { op: Ld16Op::NnSp, cycles: 20 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 20);
    }

    #[test]
    fn test_execute_ld16_sp_hl() {
        let opcode = Ld16 { op: Ld16Op::SpHl, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_hl_sp_e() {
        let opcode = Ld16 { op: Ld16Op::HlSpE, cycles: 12 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_execute_ld16_bc_a() {
        let opcode = Ld16 { op: Ld16Op::BcA, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_de_a() {
        let opcode = Ld16 { op: Ld16Op::DeA, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_a_bc() {
        let opcode = Ld16 { op: Ld16Op::ABc, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_a_de() {
        let opcode = Ld16 { op: Ld16Op::ADe, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_hli_a() {
        let opcode = Ld16 { op: Ld16Op::HliA, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_hld_a() {
        let opcode = Ld16 { op: Ld16Op::HldA, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_a_hli() {
        let opcode = Ld16 { op: Ld16Op::AHli, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_a_hld() {
        let opcode = Ld16 { op: Ld16Op::AHld, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_nn_a() {
        let opcode = Ld16 { op: Ld16Op::NnA, cycles: 16 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_execute_ld16_a_nn() {
        let opcode = Ld16 { op: Ld16Op::ANn, cycles: 16 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 16);
    }

    #[test]
    fn test_execute_ld16_ldh_n_a() {
        let opcode = Ld16 { op: Ld16Op::LdhNA, cycles: 12 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_execute_ld16_ldh_a_n() {
        let opcode = Ld16 { op: Ld16Op::LdhAN, cycles: 12 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 12);
    }

    #[test]
    fn test_execute_ld16_ld_c_a() {
        let opcode = Ld16 { op: Ld16Op::LdCA, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }

    #[test]
    fn test_execute_ld16_ld_a_c() {
        let opcode = Ld16 { op: Ld16Op::LdAC, cycles: 8 };
        let cycles = opcode.execute(&mut FakeCpu::new()).unwrap();
        assert_eq!(cycles, 8);
    }
}
