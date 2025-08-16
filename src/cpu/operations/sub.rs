use crate::cpu::registers::Flags;

/// Subtracts b from a and returns the result and the flags.
/// Possible Flag values:
/// - Z: When the result is equal to 0.
/// - N: true (always set for subtraction)
/// - H: Set if borrow from bit 4.
/// - C: Set if borrow (a < b).
pub fn sub_u8(a: u8, b: u8) -> (u8, Flags) {
    let (result, borrow) = a.overflowing_sub(b);
    let nbits: usize = 8;
    (result, Flags::from_sub(result.into(), borrow, sub_has_half_borrow(a.into(), b.into(), nbits)))
}

/// Subtracts b and carry from a and returns the result and the flags.
/// Performs: a - b - carry
/// Possible Flag values:
/// - Z: When the result is equal to 0.
/// - N: true (always set for subtraction)
/// - H: Set if borrow from bit 4.
/// - C: Set if borrow occurs.
pub fn sbc_u8(a: u8, b: u8, carry: u8) -> (u8, Flags) {
    let (temp_result, borrow1) = a.overflowing_sub(b);
    let (final_result, borrow2) = temp_result.overflowing_sub(carry);
    let total_borrow = borrow1 || borrow2;
    
    // Calculate half-borrow: borrow from bit 4 during the entire operation
    let half_borrow = (a & 0x0F) < (b & 0x0F) + carry;
    
    (final_result, Flags::from_sub(final_result.into(), total_borrow, half_borrow))
}

/// Compares a with b by performing a - b and returns only the flags.
/// The result of the subtraction is discarded.
/// Possible Flag values:
/// - Z: When a == b.
/// - N: true (always set for subtraction)
/// - H: Set if borrow from bit 4.
/// - C: Set if borrow (a < b).
pub fn cp_u8(a: u8, b: u8) -> Flags {
    let (_result, flags) = sub_u8(a, b);
    flags  // Discard the result, return only flags
}

fn sub_has_half_borrow(a: usize, b: usize, nbits: usize) -> bool {
    let half_mask = (1 << (nbits / 2)) - 1;
    (a & half_mask) < (b & half_mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_u8_zero_flag() {
        let (result, flags) = sub_u8(5, 5);
        assert_eq!(result, 0);
        assert_eq!(flags, Flags::Z | Flags::N);
    }

    #[test]
    fn test_sub_u8_no_flags() {
        let (result, flags) = sub_u8(10, 3);
        assert_eq!(result, 7);
        assert_eq!(flags, Flags::N);
    }

    #[test]
    fn test_sub_u8_half_borrow() {
        let (result, flags) = sub_u8(16, 1);
        assert_eq!(result, 15);
        assert_eq!(flags, Flags::N | Flags::H);
    }

    #[test]
    fn test_sub_u8_borrow() {
        let (result, flags) = sub_u8(0, 1);
        assert_eq!(result, 255);
        assert_eq!(flags, Flags::N | Flags::C | Flags::H);
    }

    #[test]
    fn test_sub_u8_borrow_and_half_borrow() {
        let (result, flags) = sub_u8(0, 16);
        assert_eq!(result, 240);
        assert_eq!(flags, Flags::N | Flags::C);
    }

    #[test]
    fn test_sub_u8_large_values() {
        let (result, flags) = sub_u8(255, 1);
        assert_eq!(result, 254);
        assert_eq!(flags, Flags::N);
    }

    #[test]
    fn test_sbc_u8_no_carry() {
        let (result, flags) = sbc_u8(10, 3, 0);
        assert_eq!(result, 7);
        assert_eq!(flags, Flags::N);
    }

    #[test]
    fn test_sbc_u8_with_carry() {
        let (result, flags) = sbc_u8(10, 3, 1);
        assert_eq!(result, 6);
        assert_eq!(flags, Flags::N);
    }

    #[test]
    fn test_sbc_u8_zero_result() {
        let (result, flags) = sbc_u8(5, 4, 1);
        assert_eq!(result, 0);
        assert_eq!(flags, Flags::Z | Flags::N);
    }

    #[test]
    fn test_sbc_u8_borrow() {
        let (result, flags) = sbc_u8(0, 0, 1);
        assert_eq!(result, 255);
        assert_eq!(flags, Flags::N | Flags::C | Flags::H);
    }

    #[test]
    fn test_sbc_u8_half_borrow() {
        let (result, flags) = sbc_u8(16, 0, 1);
        assert_eq!(result, 15);
        assert_eq!(flags, Flags::N | Flags::H);
    }

    #[test]
    fn test_sbc_u8_complex_borrow() {
        let (result, flags) = sbc_u8(5, 10, 1);
        assert_eq!(result, 250);
        assert_eq!(flags, Flags::N | Flags::H | Flags::C);  // Half-borrow occurs because 5 < (10 + 1)
    }

    #[test]
    fn test_cp_u8_equal() {
        let flags = cp_u8(5, 5);
        assert_eq!(flags, Flags::Z | Flags::N);
    }

    #[test]
    fn test_cp_u8_greater() {
        let flags = cp_u8(10, 5);
        assert_eq!(flags, Flags::N);
    }

    #[test]
    fn test_cp_u8_less() {
        let flags = cp_u8(5, 10);
        assert_eq!(flags, Flags::N | Flags::H | Flags::C);
    }

    #[test]
    fn test_cp_u8_half_borrow() {
        let flags = cp_u8(16, 1);
        assert_eq!(flags, Flags::N | Flags::H);
    }

    #[test]
    fn test_cp_u8_borrow_and_half_borrow() {
        let flags = cp_u8(0, 1);
        assert_eq!(flags, Flags::N | Flags::C | Flags::H);
    }
}