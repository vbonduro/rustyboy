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
    (
        sum,
        Flags::from_add(
            sum.into(),
            carry,
            add_has_half_carry(a.into(), b.into(), nbits),
        ),
    )
}

/// Adds two u16 values and returns the sum and the flags.
/// Per Game Boy spec for ADD HL,rr:
/// - Z: unchanged (not set by this function — caller must preserve it)
/// - N: false
/// - H: Set if overflow from bit 11.
/// - C: Set if overflow from bit 15.
pub fn add_u16(a: u16, b: u16) -> (u16, Flags) {
    let (sum, carry) = a.overflowing_add(b);
    let half_carry = (a & 0x0FFF) + (b & 0x0FFF) > 0x0FFF;
    let mut flags = Flags::empty();
    flags.set(Flags::H, half_carry);
    flags.set(Flags::C, carry);
    (sum, flags)
}

/// Adds two u8 values with a carry bit and returns the sum and the flags.
/// Possible Flag values:
/// - Z: When the sum is equal to 0.
/// - N: false
/// - H: Set if overflow from bit 3.
/// - C: Set if overflow from bit 7.
pub fn adc_u8(a: u8, b: u8, carry: u8) -> (u8, Flags) {
    let (sum1, carry1) = a.overflowing_add(b);
    let (sum2, carry2) = sum1.overflowing_add(carry);
    let half_carry = (a & 0x0F) + (b & 0x0F) + carry > 0x0F;
    (
        sum2,
        Flags::from_add(sum2.into(), carry1 || carry2, half_carry),
    )
}

/// Adds a signed 8-bit offset to SP, returning the result and flags.
/// Per Game Boy spec: Z=0, N=0, H and C are computed from low 8 bits only.
pub fn add_sp_u16(sp: u16, offset: i8) -> (u16, Flags) {
    let offset_u16 = offset as i16 as u16;
    let result = sp.wrapping_add(offset_u16);
    let half_carry = (sp & 0x0F) + (offset_u16 & 0x0F) > 0x0F;
    let carry = (sp & 0xFF) + (offset_u16 & 0xFF) > 0xFF;
    let mut flags = Flags::empty();
    flags.set(Flags::H, half_carry);
    flags.set(Flags::C, carry);
    (result, flags)
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
    fn test_add_u16_zero_result_does_not_set_z() {
        // ADD HL,rr spec: Z is unchanged — add_u16 must NOT set Z even when result is 0
        let (sum, flags) = add_u16(0, 0);
        assert_eq!(sum, 0);
        assert!(!flags.contains(Flags::Z));
        assert!(!flags.contains(Flags::N));
    }

    #[test]
    fn test_add_u16_no_flags() {
        let (sum, flags) = add_u16(1, 2);
        assert_eq!(sum, 3);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_add_u16_half_carry() {
        // Half-carry for ADD HL,rr is at bit 11 (overflow from lower 12 bits)
        let (sum, flags) = add_u16(0x0FFF, 1);
        assert_eq!(sum, 0x1000);
        assert!(flags.contains(Flags::H));
        assert!(!flags.contains(Flags::C));
    }

    #[test]
    fn test_add_u16_almost_half_carry() {
        let (sum, flags) = add_u16(0x0FFE, 1);
        assert_eq!(sum, 0x0FFF);
        assert!(!flags.contains(Flags::H));
    }

    #[test]
    fn test_add_u16_rollover() {
        // Z not set — caller is responsible for preserving existing Z flag
        let (sum, flags) = add_u16(65535, 1);
        assert_eq!(sum, 0);
        assert!(flags.contains(Flags::C));
        assert!(flags.contains(Flags::H));
        assert!(!flags.contains(Flags::Z));
        assert!(!flags.contains(Flags::N));
    }

    #[test]
    fn test_adc_u8_no_carry() {
        let (sum, flags) = adc_u8(1, 2, 0);
        assert_eq!(sum, 3);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_adc_u8_with_carry() {
        let (sum, flags) = adc_u8(1, 2, 1);
        assert_eq!(sum, 4);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_adc_u8_half_carry_from_carry() {
        // 0x0F + 0x00 + carry=1 overflows nibble
        let (sum, flags) = adc_u8(0x0F, 0x00, 1);
        assert_eq!(sum, 0x10);
        assert!(flags.contains(Flags::H));
    }

    #[test]
    fn test_adc_u8_overflow() {
        let (sum, flags) = adc_u8(0xFF, 0x00, 1);
        assert_eq!(sum, 0);
        assert!(flags.contains(Flags::Z));
        assert!(flags.contains(Flags::C));
        assert!(flags.contains(Flags::H));
    }

    #[test]
    fn test_add_sp_u16_positive_offset() {
        let (result, flags) = add_sp_u16(0x0100, 1);
        assert_eq!(result, 0x0101);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_add_sp_u16_negative_offset() {
        // SP=0x0100, offset=-1 (0xFF unsigned): low byte 0x00+0xFF=0xFF, no carry/half-carry
        let (result, flags) = add_sp_u16(0x0100, -1);
        assert_eq!(result, 0x00FF);
        assert!(!flags.contains(Flags::C));
        assert!(!flags.contains(Flags::H));
    }

    #[test]
    fn test_add_sp_u16_half_carry() {
        // SP=0x000F, offset=1: low nibble 0x0F+0x01=0x10, half-carry set
        let (result, flags) = add_sp_u16(0x000F, 1);
        assert_eq!(result, 0x0010);
        assert!(flags.contains(Flags::H));
        assert!(!flags.contains(Flags::C));
    }

    #[test]
    fn test_add_sp_u16_carry() {
        // SP=0x00FF, offset=1: low byte 0xFF+0x01=0x100, carry set
        let (result, flags) = add_sp_u16(0x00FF, 1);
        assert_eq!(result, 0x0100);
        assert!(flags.contains(Flags::C));
        assert!(flags.contains(Flags::H));
    }

    #[test]
    fn test_add_sp_u16_z_and_n_always_clear() {
        let (_, flags) = add_sp_u16(0x0000, 0);
        assert!(!flags.contains(Flags::Z));
        assert!(!flags.contains(Flags::N));
    }
}
