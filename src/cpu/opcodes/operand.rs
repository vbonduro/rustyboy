#[derive(Debug, PartialEq)]
pub enum Register8 {
    A,
    B,
    C,
    D,
    E,
    H,
    L,
}

#[derive(Debug, PartialEq)]
pub enum Register16 {
    BC,
    DE,
    HL,
    SP,
}

#[derive(Debug, PartialEq)]
pub enum Memory {
    HL,
    BC,
    DE,
    HLI,
    HLD,
}

#[derive(Debug, PartialEq)]
pub enum Operand {
    Register8(Register8),
    Register16(Register16),
    Imm8,
    Imm16,
    ImmSigned8,
    Memory(Memory),
}

