use alloc::vec::Vec;

#[cfg(feature = "perf")]
use crate::cpu::peripheral::apu::ApuPerfProfile;
use crate::cpu::peripheral::apu::{ApuPeripheral, NR52_ADDR};
#[cfg(feature = "perf")]
use crate::cpu::peripheral::ppu::PpuPerfProfile;
use crate::cpu::peripheral::ppu::{
    PpuPeripheral, FRAMEBUFFER_SIZE, LY_ADDR, STAT_ADDR, STAT_INTERRUPT_BIT, VBLANK_INTERRUPT_BIT,
};
use crate::cpu::save_state::PpuState;

use super::protocol::WorkerCommand;

pub struct GameBoyWorker {
    apu: ApuPeripheral,
    ppu: PpuWorkerState,
    apu_nr52: u8,
    ppu_ly: u8,
    ppu_stat: u8,
    pending_if_bits: u8,
    frame_ready: bool,
}

impl GameBoyWorker {
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn new() -> Self {
        let apu = ApuPeripheral::new();
        let mut ppu = PpuWorkerState::new();
        let apu_nr52 = apu.read_register(NR52_ADDR);
        let ppu_ly = ppu.ly();
        let ppu_stat = ppu.stat();
        ppu.sync_prev_stat_line();
        Self {
            apu,
            ppu,
            apu_nr52,
            ppu_ly,
            ppu_stat,
            pending_if_bits: 0,
            frame_ready: false,
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn send(&mut self, command: WorkerCommand) {
        match command {
            WorkerCommand::AdvanceApu {
                cycles,
                div_counter,
            } => {
                let output = self.apu.tick(cycles, div_counter);
                self.apu_nr52 = output.nr52;
            }
            WorkerCommand::AdvancePpu { cycles } => {
                let output = self.ppu.advance(cycles);
                self.ppu_ly = output.ly;
                self.ppu_stat = output.stat;
                self.pending_if_bits |= output.if_bits;
                self.frame_ready |= output.frame_ready;
            }
            WorkerCommand::WriteApuRegister { addr, value } => {
                self.apu.write_register(addr, value);
                self.apu_nr52 = self.apu.read_register(NR52_ADDR);
            }
            WorkerCommand::WriteWaveRam { offset, value } => {
                self.apu.write_wave_ram(offset, value);
            }
            WorkerCommand::WriteVram { offset, value } => {
                self.ppu.write_vram_range(offset, &[value]);
            }
            WorkerCommand::WriteOam { offset, value } => {
                self.ppu.write_oam_range(offset, &[value]);
            }
            WorkerCommand::WritePpuRegister { addr, value } => {
                self.ppu.write_register(addr, value);
                self.ppu_ly = self.ppu.ly();
                self.ppu_stat = self.ppu.stat();
            }
        }
    }

    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.apu.drain_samples()
    }

    pub fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>) {
        self.apu.drain_samples_into(out);
    }

    pub fn sync_apu_state(&mut self, io: &[u8]) {
        self.apu.sync_from_io_snapshot(io);
        self.apu_nr52 = self.apu.read_register(NR52_ADDR);
    }

    pub fn sync_ppu_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.ppu.sync_state(io, vram, oam);
        self.ppu_ly = self.ppu.ly();
        self.ppu_stat = self.ppu.stat();
        self.pending_if_bits = 0;
        self.frame_ready = false;
    }

    pub fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.ppu.load_state(state, io, vram, oam);
        self.ppu_ly = self.ppu.ly();
        self.ppu_stat = self.ppu.stat();
        self.pending_if_bits = 0;
        self.frame_ready = false;
    }

    pub fn snapshot_ppu_state(&self) -> PpuState {
        self.ppu.to_save_state()
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn update_ppu_render_state(&mut self, vram: &[u8], oam: &[u8]) {
        self.ppu.update_render_state(vram, oam);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn write_vram_range(&mut self, start_offset: u16, data: &[u8]) {
        self.ppu.write_vram_range(start_offset, data);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn write_oam_range(&mut self, start_offset: u16, data: &[u8]) {
        self.ppu.write_oam_range(start_offset, data);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn write_ppu_register(&mut self, addr: u16, value: u8) {
        self.ppu.write_register(addr, value);
        self.ppu_ly = self.ppu.ly();
        self.ppu_stat = self.ppu.stat();
    }

    pub fn copy_framebuffer(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) {
        out.copy_from_slice(self.ppu.framebuffer());
    }

    pub fn read_apu_nr52(&self) -> u8 {
        self.apu_nr52
    }

    pub fn read_ppu_ly(&self) -> u8 {
        self.ppu_ly
    }

    pub fn read_ppu_stat(&self) -> u8 {
        self.ppu_stat
    }

    pub fn take_pending_if_bits(&mut self) -> u8 {
        let if_bits = self.pending_if_bits;
        self.pending_if_bits = 0;
        if_bits
    }

    pub fn copy_framebuffer_if_ready(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> bool {
        if !self.frame_ready {
            return false;
        }
        self.frame_ready = false;
        self.copy_framebuffer(out);
        true
    }

    #[cfg(feature = "perf")]
    pub fn take_apu_perf_profile(&mut self) -> ApuPerfProfile {
        self.apu.take_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_ppu_perf_profile(&mut self) -> PpuPerfProfile {
        self.ppu.take_perf_profile()
    }
}

struct PpuWorkerOutput {
    ly: u8,
    stat: u8,
    if_bits: u8,
    frame_ready: bool,
}

struct PpuWorkerState {
    ppu: PpuPeripheral,
    io: [u8; 0x80],
    vram: [u8; 0x2000],
    oam: [u8; 0xA0],
}

impl PpuWorkerState {
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn new() -> Self {
        Self {
            ppu: PpuPeripheral::new(),
            io: [0; 0x80],
            vram: [0; 0x2000],
            oam: [0; 0xA0],
        }
    }

    fn sync_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.io.copy_from_slice(&io[..0x80]);
        self.vram.copy_from_slice(&vram[..0x2000]);
        self.oam.copy_from_slice(&oam[..0xA0]);
        self.ppu.clear_framebuffer();
        self.sync_prev_stat_line();
        self.io[(LY_ADDR - 0xFF00) as usize] = self.ppu.ly();
    }

    fn load_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.io.copy_from_slice(&io[..0x80]);
        self.vram.copy_from_slice(&vram[..0x2000]);
        self.oam.copy_from_slice(&oam[..0xA0]);
        self.ppu.load_state(state);
        self.sync_prev_stat_line();
        self.io[(LY_ADDR - 0xFF00) as usize] = self.ppu.ly();
        self.io[(STAT_ADDR - 0xFF00) as usize] = state.stat;
    }

    fn to_save_state(&self) -> PpuState {
        self.ppu.to_save_state(&self.io)
    }

    fn ly(&self) -> u8 {
        self.io[(LY_ADDR - 0xFF00) as usize]
    }

    fn stat(&self) -> u8 {
        self.io[(STAT_ADDR - 0xFF00) as usize]
    }

    fn sync_prev_stat_line(&mut self) {
        self.ppu.sync_prev_stat_line(&self.io);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn update_render_state(&mut self, vram: &[u8], oam: &[u8]) {
        self.vram.copy_from_slice(&vram[..0x2000]);
        self.oam.copy_from_slice(&oam[..0xA0]);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_vram_range(&mut self, start_offset: u16, data: &[u8]) {
        let start = start_offset as usize;
        let len = data.len().min(self.vram.len().saturating_sub(start));
        self.vram[start..start + len].copy_from_slice(&data[..len]);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]) {
        let start = start_offset as usize;
        let len = data.len().min(self.oam.len().saturating_sub(start));
        self.oam[start..start + len].copy_from_slice(&data[..len]);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_register(&mut self, addr: u16, value: u8) {
        if !(0xFF00..=0xFF7F).contains(&addr) {
            return;
        }
        if addr == LY_ADDR {
            self.ppu.reset_ly();
            self.io[(LY_ADDR - 0xFF00) as usize] = 0;
            return;
        }
        self.io[(addr - 0xFF00) as usize] = value;
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn advance(&mut self, cycles: u16) -> PpuWorkerOutput {
        let output = self.ppu.tick(cycles, &mut self.io, &self.vram, &self.oam);
        let mut if_bits = 0u8;
        if output.vblank_interrupt {
            if_bits |= 1 << VBLANK_INTERRUPT_BIT;
        }
        if output.stat_interrupt {
            if_bits |= 1 << STAT_INTERRUPT_BIT;
        }
        PpuWorkerOutput {
            ly: self.io[(LY_ADDR - 0xFF00) as usize],
            stat: self.io[(STAT_ADDR - 0xFF00) as usize],
            if_bits,
            frame_ready: output.vblank_interrupt,
        }
    }

    fn framebuffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.ppu.framebuffer()
    }

    #[cfg(feature = "perf")]
    fn take_perf_profile(&mut self) -> PpuPerfProfile {
        self.ppu.take_perf_profile()
    }
}
