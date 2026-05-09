/// The bus interface the CPU uses for all memory and IO access.
/// Implemented by `GameBoyBus<'_>` in gameboy.rs.
pub trait CpuBus {
    fn read(&mut self, addr: u16) -> u8;
    fn write(&mut self, addr: u16, val: u8);
}
