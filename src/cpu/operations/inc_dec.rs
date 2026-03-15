use crate::cpu::registers::Flags;

/// Increments a u8 value, returns the result and new flags.
/// Flags: Z set if result is 0, N=0, H set if carry from bit 3. C is NOT affected.
pub fn inc_u8(val: u8, c_flag: Flags) -> (u8, Flags) {
    let result = val.wrapping_add(1);
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::H, (val & 0x0F) == 0x0F);
    flags |= c_flag & Flags::C;
    (result, flags)
}

/// Decrements a u8 value, returns the result and new flags.
/// Flags: Z set if result is 0, N=1, H set if borrow from bit 4. C is NOT affected.
pub fn dec_u8(val: u8, c_flag: Flags) -> (u8, Flags) {
    let result = val.wrapping_sub(1);
    let mut flags = Flags::N;
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::H, (val & 0x0F) == 0x00);
    flags |= c_flag & Flags::C;
    (result, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_inc_u8_basic() {
        let (result, flags) = inc_u8(0x05, Flags::empty());
        assert_eq!(result, 0x06);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_inc_u8_zero_result() {
        let (result, flags) = inc_u8(0xFF, Flags::empty());
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::Z));
        assert!(flags.contains(Flags::H));
        assert!(!flags.contains(Flags::N));
    }

    #[test]
    fn test_inc_u8_half_carry() {
        let (result, flags) = inc_u8(0x0F, Flags::empty());
        assert_eq!(result, 0x10);
        assert!(flags.contains(Flags::H));
        assert!(!flags.contains(Flags::Z));
    }

    #[test]
    fn test_inc_u8_preserves_carry() {
        let (_, flags) = inc_u8(0x01, Flags::C);
        assert!(flags.contains(Flags::C));
    }

    #[test]
    fn test_dec_u8_basic() {
        let (result, flags) = dec_u8(0x05, Flags::empty());
        assert_eq!(result, 0x04);
        assert!(flags.contains(Flags::N));
        assert!(!flags.contains(Flags::Z));
    }

    #[test]
    fn test_dec_u8_zero_result() {
        let (result, flags) = dec_u8(0x01, Flags::empty());
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::Z));
        assert!(flags.contains(Flags::N));
    }

    #[test]
    fn test_dec_u8_half_borrow() {
        let (result, flags) = dec_u8(0x10, Flags::empty());
        assert_eq!(result, 0x0F);
        assert!(flags.contains(Flags::H));
        assert!(flags.contains(Flags::N));
    }

    #[test]
    fn test_dec_u8_preserves_carry() {
        let (_, flags) = dec_u8(0x05, Flags::C);
        assert!(flags.contains(Flags::C));
    }
}
