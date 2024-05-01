use bitflags::bitflags;

/// Registers for the Gameboy CPU
pub struct Registers {
    /// Accumulator.
    pub a: u8,
    /// B through L data registers.
    pub b: u8,
    pub c: u8,
    pub d: u8,
    pub e: u8,
    pub h: u8,
    pub l: u8,
    /// Flags. See Flags struct below for details.
    pub f: u8,
    /// Stack pointer.
    pub sp: u16,
    /// Program counter.
    pub pc: u16,
}

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct Flags: u8 {
        /// Set when the result of an operation is zero. Used in conditional jumps.
        const Z = 0x80;
        /// Set during DAA when the previous instruction was a subtraction.
        const N = 0x40;
        /// Set during DAA to indicate a carry-over of the lower 4 bits of the result.
        const H = 0x20;
        /// Set when adding numbers overflows, when subtracting numbers underflows, when a "1" is shifted out during bit rotation.
        const C = 0x10;
    }
}

impl Flags {
    /// Create a new set of flags from the result of an addition operation.
    pub fn from_add(sum: usize, carry: bool, half_carry: bool) -> Self {
        let mut flags = Self::empty();
        flags.set(Self::C, carry);
        flags.set(Self::H, half_carry);
        flags.set(Self::N, false);
        flags.set(Self::Z, sum == 0);
        flags
    }
}


