use std::fmt;

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Register8 {
    A,
    B,
    C,
    D,
    E,
    H,
    L,
}

impl fmt::Display for Register8 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Register16 {
    BC,
    DE,
    HL,
    SP,
}

impl fmt::Display for Register16 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Memory {
    HL,
    BC,
    DE,
    HLI,
    HLD,
}

impl fmt::Display for Memory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum Operand {
    Register8(Register8),
    Register16(Register16),
    Imm8,
    Imm16,
    ImmSigned8,
    Memory(Memory),
}

impl fmt::Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
