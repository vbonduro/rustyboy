use crate::cpu::registers::Flags;

/// Adds two u8 values and returns the sum and the flags.
/// Possible Flag values:
/// - Z: When the sum is equal to 0.
/// - N: false
/// - H: Set if overflow from bit 3.
/// - C: Set if overflow from bit 7.
pub fn add_u8(a: u8, b: u8) -> (u8, Flags) {
    let (sum, carry) = a.overflowing_add(b);
    let nbits: usize = 8;
    (sum, Flags::from_add(sum.into(), carry, add_has_half_carry(a.into(), b.into(), nbits)))
}

/// Adds two u16 values and returns the sum and the flags.
/// Possible Flag values:
/// - Z: When the sum is equal to 0.
/// - N: false
/// - H: Set if overflow from bit 7.
/// - C: Set if overflow from bit 15.
pub fn add_u16(a: u16, b: u16) -> (u16, Flags) {
    let (sum, carry) = a.overflowing_add(b);
    let nbits: usize = 16;
    (sum, Flags::from_add(sum.into(), carry, add_has_half_carry(a.into(), b.into(), nbits)))
}

fn add_has_half_carry(a: usize, b: usize, nbits: usize) -> bool {
    let half_carry_mask = (1 << (nbits / 2)) - 1;
    ((a & half_carry_mask) + (b & half_carry_mask)) > half_carry_mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_u8_zero_flag() {
        let (sum, flags) = add_u8(0, 0);
        assert_eq!(sum, 0);
        assert_eq!(flags, Flags::Z);
    }

    #[test]
    fn test_add_u8_no_flags() {
        let (sum, flags) = add_u8(1, 2);
        assert_eq!(sum, 3);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_add_u8_half_carry() {
        let (sum, flags) = add_u8(15, 1);
        assert_eq!(sum, 16);
        assert_eq!(flags, Flags::H);
    }

    #[test]
    fn test_add_u8_almost_half_carry() {
        let (sum, flags) = add_u8(14, 1);
        assert_eq!(sum, 15);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_add_u8_rollover() {
        let (sum, flags) = add_u8(255, 1);
        assert_eq!(sum, 0);
        assert_eq!(flags, Flags::Z | Flags::C | Flags::H);
    }

    #[test]
    fn test_add_u16_zero_flag() {
        let (sum, flags) = add_u16(0, 0);
        assert_eq!(sum, 0);
        assert_eq!(flags, Flags::Z);
    }

    #[test]
    fn test_add_u16_no_flags() {
        let (sum, flags) = add_u16(1, 2);
        assert_eq!(sum, 3);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_add_u16_half_carry() {
        let (sum, flags) = add_u16(255, 1);
        assert_eq!(sum, 256);
        assert_eq!(flags, Flags::H);
    }

    #[test]
    fn test_add_u16_almost_half_carry() {
        let (sum, flags) = add_u16(254, 1);
        assert_eq!(sum, 255);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_add_u16_rollover() {
        let (sum, flags) = add_u16(65535, 1);
        assert_eq!(sum, 0);
        assert_eq!(flags, Flags::Z | Flags::C | Flags::H);
    }
}
