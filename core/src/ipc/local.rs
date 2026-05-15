use alloc::vec::Vec;

use crate::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use crate::cpu::save_state::PpuState;

use super::protocol::{WorkerCommand, WorkerOutput};
use super::transport::WorkerTransport;
use super::worker::GameBoyWorker;

/// Single-threaded transport: runs the worker directly on the calling thread.
pub struct LocalTransport {
    worker: GameBoyWorker,
}

impl LocalTransport {
    pub fn new() -> Self {
        Self {
            worker: GameBoyWorker::new(),
        }
    }
}

impl WorkerTransport for LocalTransport {
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

    fn poll_output(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> WorkerOutput {
        let output = self.worker.poll_output();
        if output.frame_ready {
            self.worker.copy_framebuffer(out);
        }
        output
    }

}
