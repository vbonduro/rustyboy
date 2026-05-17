use alloc::vec::Vec;
use core::ptr;

use crate::cpu::peripheral::apu::{ApuPeripheral, NR52_ADDR};
use crate::cpu::peripheral::ppu::{
    PpuMode, PpuPeripheral, FRAMEBUFFER_SIZE, LY_ADDR, STAT_ADDR, STAT_INTERRUPT_BIT,
    VBLANK_INTERRUPT_BIT,
};
use crate::cpu::save_state::PpuState;

use crate::memory::map::{IO_REG_BASE, IO_REG_END};
use super::protocol::{WorkerCommand, WorkerOutput};

pub struct GameBoyWorker {
    apu: ApuPeripheral,
    ppu: alloc::boxed::Box<PpuWorkerState>,
    output: WorkerOutput,
}

impl GameBoyWorker {
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn new() -> Self {
        let apu = ApuPeripheral::new();
        let mut ppu = PpuWorkerState::new_boxed();
        let mut output = WorkerOutput::default();
        output.apu_nr52 = apu.read_register(NR52_ADDR);
        output.ppu_ly = ppu.ly();
        output.ppu_stat = ppu.stat();
        ppu.sync_prev_stat_line();
        Self { apu, ppu, output }
    }

    /// Write a `GameBoyWorker` into `dst` (a `MaybeUninit` static slot on Core 1).
    /// `new()` is stack-safe after boxing `PpuWorkerState`; this wrapper exists only
    /// to obtain the required `&'static mut` reference from the raw pointer.
    /// SAFETY: `dst` must be valid for `size_of::<Self>()` bytes and must not alias
    /// any live reference. Caller must ensure this is called exactly once.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub unsafe fn init_in_place(dst: *mut Self) -> &'static mut Self {
        unsafe {
            ptr::write(dst, Self::new());
            &mut *dst
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
                let apu_output = self.apu.tick(cycles, div_counter);
                self.output.apu_nr52 = apu_output.nr52;
            }
            WorkerCommand::AdvancePpu { cycles } => {
                let ppu_output = self.ppu.advance(cycles);
                self.output.ppu_ly = ppu_output.ly;
                self.output.ppu_stat = ppu_output.stat;
                self.output.if_bits |= ppu_output.if_bits;
                self.output.frame_ready |= ppu_output.frame_ready;
            }
            WorkerCommand::WriteApuRegister { addr, value } => {
                self.apu.write_register(addr, value);
                self.output.apu_nr52 = self.apu.read_register(NR52_ADDR);
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
                self.output.ppu_ly = self.ppu.ly();
                self.output.ppu_stat = self.ppu.stat();
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
        self.output.apu_nr52 = self.apu.read_register(NR52_ADDR);
    }

    pub fn sync_ppu_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.ppu.sync_state(io, vram, oam);
        self.output.ppu_ly = self.ppu.ly();
        self.output.ppu_stat = self.ppu.stat();
        self.output.if_bits = 0;
        self.output.frame_ready = false;
    }

    pub fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.ppu.load_state(state, io, vram, oam);
        self.output.ppu_ly = self.ppu.ly();
        self.output.ppu_stat = self.ppu.stat();
        self.output.if_bits = 0;
        self.output.frame_ready = false;
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
        self.output.ppu_ly = self.ppu.ly();
        self.output.ppu_stat = self.ppu.stat();
    }

    pub fn copy_framebuffer(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) {
        out.copy_from_slice(self.ppu.framebuffer());
    }

    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.ppu.framebuffer()
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn poll_output(&mut self) -> WorkerOutput {
        let output = self.output;
        self.output.if_bits = 0;
        self.output.frame_ready = false;
        output
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

// SAFETY: PpuPeripheral is Zeroable and all other fields are byte arrays.
// The zeroed state has mode = HBlank; new_boxed() fixes it up to OamScan.
unsafe impl bytemuck::Zeroable for PpuWorkerState {}

impl PpuWorkerState {
    /// Heap-allocate a zero-initialized `PpuWorkerState` without a ~31 KiB stack
    /// temporary, then fix up the PPU mode to the correct power-on state.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn new_boxed() -> alloc::boxed::Box<Self> {
        let mut s = bytemuck::zeroed_box::<Self>();
        s.ppu.mode = PpuMode::OamScan;
        s
    }

    fn sync_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.io.copy_from_slice(&io[..0x80]);
        self.vram.copy_from_slice(&vram[..0x2000]);
        self.oam.copy_from_slice(&oam[..0xA0]);
        self.ppu.clear_framebuffer();
        self.sync_prev_stat_line();
        self.io[(LY_ADDR - IO_REG_BASE) as usize] = self.ppu.ly();
    }

    fn load_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.io.copy_from_slice(&io[..0x80]);
        self.vram.copy_from_slice(&vram[..0x2000]);
        self.oam.copy_from_slice(&oam[..0xA0]);
        self.ppu.load_state(state);
        self.sync_prev_stat_line();
        self.io[(LY_ADDR - IO_REG_BASE) as usize] = self.ppu.ly();
        self.io[(STAT_ADDR - IO_REG_BASE) as usize] = state.stat;
    }

    fn to_save_state(&self) -> PpuState {
        self.ppu.to_save_state(&self.io)
    }

    fn ly(&self) -> u8 {
        self.io[(LY_ADDR - IO_REG_BASE) as usize]
    }

    fn stat(&self) -> u8 {
        self.io[(STAT_ADDR - IO_REG_BASE) as usize]
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
        if !(IO_REG_BASE..=IO_REG_END).contains(&addr) {
            return;
        }
        if addr == LY_ADDR {
            self.ppu.reset_ly();
            self.io[(LY_ADDR - IO_REG_BASE) as usize] = 0;
            return;
        }
        self.io[(addr - IO_REG_BASE) as usize] = value;
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
            ly: self.io[(LY_ADDR - IO_REG_BASE) as usize],
            stat: self.io[(STAT_ADDR - IO_REG_BASE) as usize],
            if_bits,
            frame_ready: output.vblank_interrupt,
        }
    }

    fn framebuffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.ppu.framebuffer()
    }

}
