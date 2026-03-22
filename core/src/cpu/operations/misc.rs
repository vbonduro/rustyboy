use crate::cpu::registers::Flags;

/// DAA — BCD correction of accumulator after addition or subtraction.
/// Returns the corrected accumulator value and updated flags.
/// N flag is unchanged; H is always cleared; Z and C updated.
pub fn daa_u8(a: u8, flags: Flags) -> (u8, Flags) {
    let n = flags.contains(Flags::N);
    let h = flags.contains(Flags::H);
    let mut carry = flags.contains(Flags::C);
    let mut result = a;

    if !n {
        if carry || result > 0x99 {
            result = result.wrapping_add(0x60);
            carry = true;
        }
        if h || (result & 0x0F) > 0x09 {
            result = result.wrapping_add(0x06);
        }
    } else {
        if carry {
            result = result.wrapping_sub(0x60);
        }
        if h {
            result = result.wrapping_sub(0x06);
        }
    }

    let mut new_flags = flags & Flags::N; // preserve N
    new_flags.set(Flags::Z, result == 0);
    new_flags.set(Flags::C, carry);
    // H always cleared
    (result, new_flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_daa_after_addition_no_correction_needed() {
        // 0x05 + 0x03 = 0x08 — already valid BCD
        let (result, flags) = daa_u8(0x08, Flags::empty());
        assert_eq!(result, 0x08);
        assert!(!flags.contains(Flags::C));
    }

    #[test]
    fn test_daa_after_addition_low_nibble_correction() {
        // 0x08 + 0x05 = 0x0D — low nibble > 9, needs +6
        let (result, flags) = daa_u8(0x0D, Flags::empty());
        assert_eq!(result, 0x13);
        assert!(!flags.contains(Flags::C));
    }

    #[test]
    fn test_daa_after_addition_carry_correction() {
        // result > 0x99 sets carry and adds 0x60
        let (result, flags) = daa_u8(0x9A, Flags::empty());
        assert!(flags.contains(Flags::C));
        assert_eq!(result, 0x00); // 0x9A + 0x60 = 0xFA... then +0x06 = 0x00 wrapping
    }

    #[test]
    fn test_daa_after_subtraction() {
        // 0x09 - 0x04 = 0x05, no borrow — no correction
        let (result, flags) = daa_u8(0x05, Flags::N);
        assert_eq!(result, 0x05);
        assert!(!flags.contains(Flags::C));
    }

    #[test]
    fn test_daa_clears_h_flag() {
        let (_, flags) = daa_u8(0x08, Flags::H);
        assert!(!flags.contains(Flags::H));
    }

    #[test]
    fn test_daa_zero_result_sets_z() {
        // Force a zero result
        let (result, flags) = daa_u8(0xA0, Flags::empty()); // 0xA0 > 0x99 → +0x60 → 0x00 wrapping
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::Z));
        assert!(flags.contains(Flags::C));
    }
}
