use alloc::vec::Vec;

#[cfg(feature = "perf")]
use crate::cpu::peripheral::apu::ApuPerfProfile;
#[cfg(feature = "perf")]
use crate::cpu::peripheral::ppu::PpuPerfProfile;
use crate::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use crate::cpu::save_state::PpuState;

use super::protocol::{WorkerCommand, WorkerFrontendState, WorkerLink};
use super::worker::GameBoyWorker;

pub struct InlineWorkerLink {
    worker: GameBoyWorker,
}

impl InlineWorkerLink {
    pub fn new() -> Self {
        Self {
            worker: GameBoyWorker::new(),
        }
    }
}

impl WorkerLink for InlineWorkerLink {
    fn send(&mut self, command: WorkerCommand) {
        self.worker.send(command)
    }

    fn write_vram_range(&mut self, start_offset: u16, data: &[u8]) {
        self.worker.write_vram_range(start_offset, data);
    }

    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]) {
        self.worker.write_oam_range(start_offset, data);
    }

    fn write_ppu_register(&mut self, addr: u16, value: u8) {
        self.worker.write_ppu_register(addr, value);
    }

    fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.worker.drain_audio_samples()
    }

    fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>) {
        self.worker.drain_audio_samples_into_i16(out);
    }

    fn sync_apu_state(&mut self, io: &[u8]) {
        self.worker.sync_apu_state(io);
    }

    fn sync_ppu_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.worker.sync_ppu_state(io, vram, oam);
    }

    fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.worker.load_ppu_state(state, io, vram, oam);
    }

    fn snapshot_ppu_state(&self, _io: &[u8]) -> PpuState {
        self.worker.snapshot_ppu_state()
    }

    fn poll_frontend_state(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> WorkerFrontendState {
        let state = self.worker.poll_frontend_state();
        if state.frame_ready {
            self.worker.copy_framebuffer(out);
        }
        state
    }

    #[cfg(feature = "perf")]
    fn take_apu_perf_profile(&mut self) -> ApuPerfProfile {
        self.worker.take_apu_perf_profile()
    }

    #[cfg(feature = "perf")]
    fn take_ppu_perf_profile(&mut self) -> PpuPerfProfile {
        self.worker.take_ppu_perf_profile()
    }
}
