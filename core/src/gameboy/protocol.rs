use alloc::vec::Vec;

#[cfg(feature = "perf")]
use crate::cpu::peripheral::apu::ApuPerfProfile;
#[cfg(feature = "perf")]
use crate::cpu::peripheral::ppu::PpuPerfProfile;
use crate::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use crate::cpu::save_state::PpuState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerCommand {
    AdvanceApu { cycles: u16, div_counter: u16 },
    AdvancePpu { cycles: u16 },
    WriteApuRegister { addr: u16, value: u8 },
    WriteWaveRam { offset: u8, value: u8 },
    WriteVram { offset: u16, value: u8 },
    WriteOam { offset: u16, value: u8 },
    WritePpuRegister { addr: u16, value: u8 },
}

pub trait WorkerLink {
    fn send(&mut self, command: WorkerCommand);
    fn write_vram_range(&mut self, start_offset: u16, data: &[u8]);
    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]);
    fn write_ppu_register(&mut self, addr: u16, value: u8);
    fn drain_audio_samples(&mut self) -> Vec<f32>;
    fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>);
    fn sync_apu_state(&mut self, io: &[u8]);
    fn sync_ppu_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]);
    fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]);
    fn snapshot_ppu_state(&self, io: &[u8]) -> PpuState;
    fn read_apu_nr52(&self) -> u8;
    fn read_ppu_ly(&self) -> u8;
    fn read_ppu_stat(&self) -> u8;
    fn take_pending_if_bits(&mut self) -> u8;
    fn copy_front_buffer_if_ready(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> bool;
    #[cfg(feature = "perf")]
    fn take_apu_perf_profile(&mut self) -> ApuPerfProfile;
    #[cfg(feature = "perf")]
    fn take_ppu_perf_profile(&mut self) -> PpuPerfProfile;
}
