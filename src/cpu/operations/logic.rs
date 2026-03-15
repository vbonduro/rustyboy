use crate::cpu::registers::Flags;

/// AND: A & operand. Flags: Z=(result==0), N=0, H=1, C=0.
pub fn and_u8(a: u8, b: u8) -> (u8, Flags) {
    let result = a & b;
    let mut flags = Flags::H;
    flags.set(Flags::Z, result == 0);
    (result, flags)
}

/// OR: A | operand. Flags: Z=(result==0), N=0, H=0, C=0.
pub fn or_u8(a: u8, b: u8) -> (u8, Flags) {
    let result = a | b;
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    (result, flags)
}

/// XOR: A ^ operand. Flags: Z=(result==0), N=0, H=0, C=0.
pub fn xor_u8(a: u8, b: u8) -> (u8, Flags) {
    let result = a ^ b;
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    (result, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_and_u8_basic() {
        let (result, flags) = and_u8(0xFF, 0x0F);
        assert_eq!(result, 0x0F);
        assert_eq!(flags, Flags::H);
    }

    #[test]
    fn test_and_u8_zero() {
        let (result, flags) = and_u8(0xF0, 0x0F);
        assert_eq!(result, 0x00);
        assert_eq!(flags, Flags::Z | Flags::H);
    }

    #[test]
    fn test_or_u8_basic() {
        let (result, flags) = or_u8(0xF0, 0x0F);
        assert_eq!(result, 0xFF);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_or_u8_zero() {
        let (result, flags) = or_u8(0x00, 0x00);
        assert_eq!(result, 0x00);
        assert_eq!(flags, Flags::Z);
    }

    #[test]
    fn test_xor_u8_basic() {
        let (result, flags) = xor_u8(0xFF, 0x0F);
        assert_eq!(result, 0xF0);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_xor_u8_self_is_zero() {
        let (result, flags) = xor_u8(0x42, 0x42);
        assert_eq!(result, 0x00);
        assert_eq!(flags, Flags::Z);
    }
}
