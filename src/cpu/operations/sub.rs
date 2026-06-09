use std::collections::btree_set::Difference;

use crate::cpu::registers::Flags;

/// Subtracts two values and returns the difference and flags.
/// Possible Flag values:
/// - Z: When the difference is equal to 0.
/// - N: false
/// - H: Set if overflow from bit 3.
/// - C: Set if overflow from bit 7.
/// 
pub fn sub(a: u8, b: u8) -> (u8, Flags) {
    let (difference, has_carry) = a.overflowing_sub(b);
    (difference, Flags::from_arithemtic(difference.into(), has_carry, has_half_carry(a, b)))
}

fn has_half_carry(a: u8, b: u8) -> bool {
    let half_carry_mask = 0x0F;
    ((a & half_carry_mask) - (b & half_carry_mask)) & 0x10 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_u8_zero_flag() {
        let (diff, flags) = sub(0, 0);
        assert_eq!(diff, 0);
        assert_eq!(flags, Flags::Z);
    }

    #[test]
    fn test_sub_u8_no_flags() {
        let (diff, flags) = sub(3, 1);
        assert_eq!(diff, 2);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_sub_u8_half_carry() {
        let (diff, flags) = sub(16, 1);
        assert_eq!(diff, 15);
        assert_eq!(flags, Flags::H);
    }

    // 1 - x = 16
    // -x = 16 - 1
    // -x = 15
    // x = -15
    #[test]
    fn test_sub_u8_almost_half_carry() {
        let (diff, flags) = sub(15, 1);
        assert_eq!(diff, 14);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_sub_u8_rollover() {
        let (diff, flags) = sub(0, 1);
        assert_eq!(diff, 255);
        assert_eq!(flags, Flags::C);
    }

    #[test]
    fn test_sub_u8_subtraction_corner_case_both_zeros() {
        let (diff, flags) = sub(0, 0);
        assert_eq!(diff, 0);
        assert_eq!(flags, Flags::Z);
    }

    #[test]
    fn test_sub_u8_subtraction_corner_case_no_carry() {
        let (diff, flags) = sub(1, 0);
        assert_eq!(diff, 1);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_sub_u8_subtraction_corner_case_carry_from_b() {
        let (diff, flags) = sub(0, 1);
        assert_eq!(diff, 255);
        assert_eq!(flags, Flags::C);
    }

    #[test]
    fn test_sub_u8_subtraction_corner_case_carry_from_a() {
        let (diff, flags) = sub(127, 128);
        assert_eq!(diff, 255);
        assert_eq!(flags, Flags::C);
    }

    #[test]
    fn test_sub_u8_subtraction_corner_case_no_carry_from_either() {
        let (diff, flags) = sub(128, 127);
        assert_eq!(diff, 1);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_sub_u8_subtraction_corner_case_no_carry_from_both() {
        let (diff, flags) = sub(255, 0);
        assert_eq!(diff, 255);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_sub_u8_subtraction_corner_case_carry_from_both() {
        let (diff, flags) = sub(0, 255);
        assert_eq!(diff, 1);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_sub_u8_subtraction_corner_case_both_max_values() {
        let (diff, flags) = sub(255, 255);
        assert_eq!(diff, 0);
        assert_eq!(flags, Flags::Z);
    }
}

