/// Canonical Game Boy memory map address range constants.
///
/// All address bounds are `u16` to match the 16-bit address bus.
/// Use `CONST as usize` where an offset calculation requires usize arithmetic.
///
/// ROM bank size is `usize` because it is used as an array size and multiplier,
/// not as an address bound.
pub(crate) const ROM_BANK_SIZE: usize = 0x4000;

pub(crate) const ROM_FIXED_END: u16 = 0x3FFF;
pub(crate) const ROM_BANKED_BASE: u16 = 0x4000;
pub(crate) const ROM_BANKED_END: u16 = 0x7FFF;
pub(crate) const VRAM_BASE: u16 = 0x8000;
pub(crate) const VRAM_END: u16 = 0x9FFF;
pub(crate) const EXT_RAM_BASE: u16 = 0xA000;
pub(crate) const EXT_RAM_END: u16 = 0xBFFF;
pub(crate) const WRAM_BASE: u16 = 0xC000;
pub(crate) const WRAM_END: u16 = 0xDFFF;
pub(crate) const ECHO_BASE: u16 = 0xE000;
pub(crate) const ECHO_END: u16 = 0xFDFF;
pub(crate) const OAM_BASE: u16 = 0xFE00;
pub(crate) const OAM_END: u16 = 0xFE9F;
pub(crate) const IO_REG_BASE: u16 = 0xFF00;
pub(crate) const IO_REG_END: u16 = 0xFF7F;
