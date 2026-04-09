use bitflags::bitflags;

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

impl Default for Flags {
    fn default() -> Self {
        Self::empty()
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

    /// Create a new set of flags from the result of a subtraction operation.
    pub fn from_sub(result: usize, borrow: bool, half_borrow: bool) -> Self {
        let mut flags = Self::empty();
        flags.set(Self::C, borrow);
        flags.set(Self::H, half_borrow);
        flags.set(Self::N, true);
        flags.set(Self::Z, result == 0);
        flags
    }
}

/// Registers for the Gameboy CPU
#[derive(Debug, Default, Clone, Copy)]
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
    pub f: Flags,
    /// Stack pointer.
    pub sp: u16,
    /// Program counter.
    pub pc: u16,
}

impl Registers {
    pub fn bc(&self) -> u16 {
        (self.b as u16) << 8 | self.c as u16
    }

    pub fn de(&self) -> u16 {
        (self.d as u16) << 8 | self.e as u16
    }

    pub fn hl(&self) -> u16 {
        (self.h as u16) << 8 | self.l as u16
    }

    pub fn set_bc(&mut self, bc: u16) {
        self.b = (bc >> 8) as u8;
        self.c = (bc & 0xFF) as u8;
    }

    pub fn set_de(&mut self, de: u16) {
        self.d = (de >> 8) as u8;
        self.e = (de & 0xFF) as u8;
    }

    pub fn set_hl(&mut self, hl: u16) {
        self.h = (hl >> 8) as u8;
        self.l = (hl & 0xFF) as u8;
    }

    pub fn af(&self) -> u16 {
        (self.a as u16) << 8 | self.f.bits() as u16
    }

    /// Set AF. The lower nibble of F is always 0 on the Game Boy.
    pub fn set_af(&mut self, af: u16) {
        self.a = (af >> 8) as u8;
        self.f = Flags::from_bits_truncate((af & 0xF0) as u8);
    }


}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bc() {
        let mut registers = Registers::default();
        registers.b = 0xde;
        registers.c = 0xad;
        assert_eq!(registers.bc(), 0xdead);
    }

    #[test]
    fn test_de() {
        let mut registers = Registers::default();
        registers.d = 0xde;
        registers.e = 0xad;
        assert_eq!(registers.de(), 0xdead);
    }

    #[test]
    fn test_hl() {
        let mut registers = Registers::default();
        registers.h = 0xde;
        registers.l = 0xad;
        assert_eq!(registers.hl(), 0xdead);
    }

    #[test]
    fn test_set_bc() {
        let mut registers = Registers::default();
        registers.set_bc(0xdead);
        assert_eq!(registers.b, 0xde);
        assert_eq!(registers.c, 0xad);
    }

    #[test]
    fn test_set_de() {
        let mut registers = Registers::default();
        registers.set_de(0xdead);
        assert_eq!(registers.d, 0xde);
        assert_eq!(registers.e, 0xad);
    }

    #[test]
    fn test_set_hl() {
        let mut registers = Registers::default();
        registers.set_hl(0xdead);
        assert_eq!(registers.h, 0xde);
        assert_eq!(registers.l, 0xad);
    }

    #[test]
    fn test_af() {
        let mut registers = Registers::default();
        registers.a = 0xAB;
        registers.f = Flags::Z | Flags::C;
        assert_eq!(registers.af(), 0xAB90);
    }

    #[test]
    fn test_set_af() {
        let mut registers = Registers::default();
        registers.set_af(0xAB90);
        assert_eq!(registers.a, 0xAB);
        assert_eq!(registers.f, Flags::Z | Flags::C);
    }

    #[test]
    fn test_set_af_clears_low_nibble() {
        let mut registers = Registers::default();
        // Lower nibble of F should always be masked to 0
        registers.set_af(0x001F);
        assert_eq!(registers.f.bits(), 0x10);
    }
}
