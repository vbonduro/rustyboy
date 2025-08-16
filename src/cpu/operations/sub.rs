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
}