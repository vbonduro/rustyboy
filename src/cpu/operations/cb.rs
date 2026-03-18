use crate::cpu::registers::Flags;

pub fn rlc_u8(val: u8) -> (u8, Flags) {
    let bit7 = val >> 7;
    let result = (val << 1) | bit7;
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::C, bit7 != 0);
    (result, flags)
}

pub fn rrc_u8(val: u8) -> (u8, Flags) {
    let bit0 = val & 1;
    let result = (val >> 1) | (bit0 << 7);
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::C, bit0 != 0);
    (result, flags)
}

pub fn rl_u8(val: u8, carry_in: bool) -> (u8, Flags) {
    let bit7 = val >> 7;
    let result = (val << 1) | (carry_in as u8);
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::C, bit7 != 0);
    (result, flags)
}

pub fn rr_u8(val: u8, carry_in: bool) -> (u8, Flags) {
    let bit0 = val & 1;
    let result = (val >> 1) | ((carry_in as u8) << 7);
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::C, bit0 != 0);
    (result, flags)
}

pub fn sla_u8(val: u8) -> (u8, Flags) {
    let bit7 = val >> 7;
    let result = val << 1;
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::C, bit7 != 0);
    (result, flags)
}

pub fn sra_u8(val: u8) -> (u8, Flags) {
    let bit0 = val & 1;
    let result = (val >> 1) | (val & 0x80); // preserve sign bit
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::C, bit0 != 0);
    (result, flags)
}

pub fn swap_u8(val: u8) -> (u8, Flags) {
    let result = (val >> 4) | (val << 4);
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    (result, flags)
}

pub fn srl_u8(val: u8) -> (u8, Flags) {
    let bit0 = val & 1;
    let result = val >> 1;
    let mut flags = Flags::empty();
    flags.set(Flags::Z, result == 0);
    flags.set(Flags::C, bit0 != 0);
    (result, flags)
}

/// BIT b, r — tests bit b of val. Z=!bit, N=0, H=1, C unchanged.
pub fn bit_u8(val: u8, bit: u8, c_flag: Flags) -> Flags {
    let mut flags = Flags::H;
    flags.set(Flags::Z, (val >> bit) & 1 == 0);
    flags.set(Flags::C, c_flag.contains(Flags::C));
    flags
}

/// RES b, r — clears bit b of val.
pub fn res_u8(val: u8, bit: u8) -> u8 {
    val & !(1 << bit)
}

/// SET b, r — sets bit b of val.
pub fn set_u8(val: u8, bit: u8) -> u8 {
    val | (1 << bit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rlc_no_carry() {
        let (result, flags) = rlc_u8(0x01);
        assert_eq!(result, 0x02);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_rlc_carry_wraps() {
        let (result, flags) = rlc_u8(0x80);
        assert_eq!(result, 0x01);
        assert!(flags.contains(Flags::C));
    }

    #[test]
    fn test_rlc_zero() {
        let (result, flags) = rlc_u8(0x00);
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::Z));
    }

    #[test]
    fn test_rrc_no_carry() {
        let (result, flags) = rrc_u8(0x02);
        assert_eq!(result, 0x01);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_rrc_carry_wraps() {
        let (result, flags) = rrc_u8(0x01);
        assert_eq!(result, 0x80);
        assert!(flags.contains(Flags::C));
    }

    #[test]
    fn test_rl_with_carry_in() {
        let (result, flags) = rl_u8(0x00, true);
        assert_eq!(result, 0x01);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_rl_carry_out() {
        let (result, flags) = rl_u8(0x80, false);
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::C));
        assert!(flags.contains(Flags::Z));
    }

    #[test]
    fn test_rr_with_carry_in() {
        let (result, flags) = rr_u8(0x00, true);
        assert_eq!(result, 0x80);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_rr_carry_out() {
        let (result, flags) = rr_u8(0x01, false);
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::C));
        assert!(flags.contains(Flags::Z));
    }

    #[test]
    fn test_sla() {
        let (result, flags) = sla_u8(0x40);
        assert_eq!(result, 0x80);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_sla_carry() {
        let (result, flags) = sla_u8(0x80);
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::C));
        assert!(flags.contains(Flags::Z));
    }

    #[test]
    fn test_sra_preserves_sign() {
        let (result, flags) = sra_u8(0x80);
        assert_eq!(result, 0xC0);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_sra_carry() {
        let (result, flags) = sra_u8(0x01);
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::C));
        assert!(flags.contains(Flags::Z));
    }

    #[test]
    fn test_swap() {
        let (result, flags) = swap_u8(0xAB);
        assert_eq!(result, 0xBA);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_swap_zero() {
        let (result, flags) = swap_u8(0x00);
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::Z));
    }

    #[test]
    fn test_srl() {
        let (result, flags) = srl_u8(0x02);
        assert_eq!(result, 0x01);
        assert_eq!(flags, Flags::empty());
    }

    #[test]
    fn test_srl_carry() {
        let (result, flags) = srl_u8(0x01);
        assert_eq!(result, 0x00);
        assert!(flags.contains(Flags::C));
        assert!(flags.contains(Flags::Z));
    }

    #[test]
    fn test_bit_set() {
        let flags = bit_u8(0xFF, 3, Flags::empty());
        assert!(!flags.contains(Flags::Z));
        assert!(flags.contains(Flags::H));
        assert!(!flags.contains(Flags::N));
    }

    #[test]
    fn test_bit_clear() {
        let flags = bit_u8(0x00, 3, Flags::empty());
        assert!(flags.contains(Flags::Z));
        assert!(flags.contains(Flags::H));
    }

    #[test]
    fn test_bit_preserves_carry() {
        let flags = bit_u8(0xFF, 0, Flags::C);
        assert!(flags.contains(Flags::C));
    }

    #[test]
    fn test_res() {
        assert_eq!(res_u8(0xFF, 3), 0xF7);
        assert_eq!(res_u8(0x00, 3), 0x00);
    }

    #[test]
    fn test_set() {
        assert_eq!(set_u8(0x00, 3), 0x08);
        assert_eq!(set_u8(0xFF, 3), 0xFF);
    }
}
