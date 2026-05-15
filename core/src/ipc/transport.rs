use alloc::vec::Vec;

use crate::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use crate::cpu::save_state::PpuState;

use super::protocol::{WorkerCommand, WorkerOutput};

pub trait WorkerTransport {
    fn send(&mut self, command: WorkerCommand);
    fn write_vram_range(&mut self, start_offset: u16, data: &[u8]);
    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]);
    fn write_ppu_register(&mut self, addr: u16, value: u8);
    fn write_ppu_registers(&mut self, regs: &[(u16, u8)]) {
        for &(addr, value) in regs {
            self.write_ppu_register(addr, value);
        }
    }
    fn drain_audio_samples(&mut self) -> Vec<f32>;
    fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>);
    fn sync_apu_state(&mut self, io: &[u8]);
    fn sync_ppu_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]);
    fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]);
    fn snapshot_ppu_state(&self, io: &[u8]) -> PpuState;
    fn poll_output(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> WorkerOutput;
}
