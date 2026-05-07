use alloc::{boxed::Box, vec::Vec};

use super::apu::{ApuOutput, ApuPeripheral, NR52_ADDR};
#[cfg(feature = "perf")]
use super::apu::ApuPerfProfile;
use super::ppu::{
    PpuBackend, PpuInput, PpuOutput, PpuPeripheral, FRAMEBUFFER_SIZE,
};
#[cfg(feature = "perf")]
use super::ppu::PpuPerfProfile;
use super::timer::{TimerInput, TimerOutput, TimerPeripheral};
use crate::cpu::save_state::{PpuState, TimerState};

pub struct PeripheralRegs<'a> {
    pub lcdc: u8,
    pub stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub lyc: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,
    pub tima: u8,
    pub tma: u8,
    pub tac: u8,
    pub ly: u8,
    pub vram: &'a [u8],
    pub oam: &'a [u8],
}

pub struct PeripheralAdvanceOutput {
    pub ly: u8,
    pub stat: u8,
    pub vblank_interrupt: bool,
    pub stat_interrupt: bool,
    pub tima: u8,
    pub div: u8,
    pub timer_interrupt: bool,
    pub nr52: u8,
}

pub struct PeripheralShadowState {
    pub completed_seq: u32,
    pub ly: u8,
    pub stat: u8,
    pub tima: u8,
    pub div: u8,
    pub nr52: u8,
    pub vblank_count: u32,
    pub stat_interrupt_count: u32,
    pub timer_interrupt_count: u32,
}

pub trait PeripheralBackend {
    fn advance(
        &mut self,
        ppu_cycles: u16,
        timer_cycles: u16,
        apu_cycles: u16,
        regs: PeripheralRegs<'_>,
    ) -> PeripheralAdvanceOutput;
    fn supports_queued_advance(&self) -> bool {
        false
    }
    fn try_queue_advance(
        &mut self,
        ppu_cycles: u16,
        timer_cycles: u16,
        apu_cycles: u16,
        regs: PeripheralRegs<'_>,
    ) -> bool {
        let _ = (ppu_cycles, timer_cycles, apu_cycles, regs);
        false
    }
    fn try_take_queued_shadow_state(&mut self, min_completed_seq: u32) -> Option<PeripheralShadowState> {
        let _ = min_completed_seq;
        None
    }
    fn wait_queued_shadow_state(&mut self, min_completed_seq: u32) -> PeripheralShadowState {
        let _ = min_completed_seq;
        panic!("queued shadow state requested from a synchronous peripheral backend")
    }
    fn reset_div(&mut self);
    fn reset_ly(&mut self);
    fn read_apu_register(&mut self, address: u16) -> u8;
    fn write_apu_register(&mut self, address: u16, value: u8);
    fn read_wave_ram(&mut self, offset: u8) -> u8;
    fn write_wave_ram(&mut self, offset: u8, value: u8);
    fn on_vram_write(&mut self, _offset: u16, _value: u8) {}
    fn on_oam_write(&mut self, _offset: u16, _value: u8) {}
    fn on_oam_dma_byte(&mut self, offset: u8, value: u8) {
        self.on_oam_write(offset as u16, value);
    }
    fn sync_memory(&mut self, _vram: &[u8], _oam: &[u8]) {}
    fn snapshot_framebuffer_into(&mut self, dst: &mut [u8; FRAMEBUFFER_SIZE]);
    fn drain_samples(&mut self) -> Vec<f32>;
    fn clear_samples(&mut self);
    fn timer_state(&self) -> TimerState;
    fn ppu_state(&self) -> PpuState;
    fn load_state(&mut self, timer: TimerState, ppu: PpuState, vram: &[u8], oam: &[u8]);
    #[cfg(feature = "perf")]
    fn take_ppu_perf_profile(&mut self) -> PpuPerfProfile;
    #[cfg(feature = "perf")]
    fn take_apu_perf_profile(&mut self) -> ApuPerfProfile;
}

pub struct LocalPeripheralBackend {
    timer: TimerPeripheral,
    ppu: PpuPeripheral,
    apu: ApuPeripheral,
}

impl LocalPeripheralBackend {
    pub fn new() -> Self {
        Self {
            timer: TimerPeripheral::new(),
            ppu: PpuPeripheral::new(),
            apu: ApuPeripheral::new(),
        }
    }

    pub unsafe fn init_in_place(dst: *mut Self) {
        core::ptr::addr_of_mut!((*dst).timer).write(TimerPeripheral::new());
        PpuPeripheral::init_in_place(core::ptr::addr_of_mut!((*dst).ppu));
        core::ptr::addr_of_mut!((*dst).apu).write(ApuPeripheral::new());
    }
}

impl Default for LocalPeripheralBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl PeripheralBackend for LocalPeripheralBackend {
    fn advance(
        &mut self,
        ppu_cycles: u16,
        timer_cycles: u16,
        apu_cycles: u16,
        regs: PeripheralRegs<'_>,
    ) -> PeripheralAdvanceOutput {
        let ppu_output = if ppu_cycles == 0 {
            PpuOutput {
                ly: regs.ly,
                stat: regs.stat,
                vblank_interrupt: false,
                stat_interrupt: false,
            }
        } else {
            self.ppu.tick(
                ppu_cycles,
                PpuInput {
                    lcdc: regs.lcdc,
                    stat: regs.stat,
                    scy: regs.scy,
                    scx: regs.scx,
                    lyc: regs.lyc,
                    bgp: regs.bgp,
                    obp0: regs.obp0,
                    obp1: regs.obp1,
                    wy: regs.wy,
                    wx: regs.wx,
                    vram: regs.vram,
                    oam: regs.oam,
                },
            )
        };

        let timer_output = if timer_cycles == 0 {
            TimerOutput {
                tima: regs.tima,
                div: self.timer.div(),
                interrupt: false,
            }
        } else {
            self.timer.tick(
                timer_cycles,
                TimerInput {
                    tima: regs.tima,
                    tma: regs.tma,
                    tac: regs.tac,
                },
            )
        };

        let apu_output = if apu_cycles == 0 {
            ApuOutput {
                nr52: self.apu.read_register(NR52_ADDR),
            }
        } else {
            self.apu.tick(apu_cycles, self.timer.internal_counter())
        };

        PeripheralAdvanceOutput {
            ly: ppu_output.ly,
            stat: ppu_output.stat,
            vblank_interrupt: ppu_output.vblank_interrupt,
            stat_interrupt: ppu_output.stat_interrupt,
            tima: timer_output.tima,
            div: timer_output.div,
            timer_interrupt: timer_output.interrupt,
            nr52: apu_output.nr52,
        }
    }

    fn reset_div(&mut self) {
        self.timer.reset_div();
    }

    fn reset_ly(&mut self) {
        self.ppu.reset_ly();
    }

    fn read_apu_register(&mut self, address: u16) -> u8 {
        self.apu.read_register(address)
    }

    fn write_apu_register(&mut self, address: u16, value: u8) {
        self.apu.write_register(address, value);
    }

    fn read_wave_ram(&mut self, offset: u8) -> u8 {
        self.apu.read_wave_ram(offset)
    }

    fn write_wave_ram(&mut self, offset: u8, value: u8) {
        self.apu.write_wave_ram(offset, value);
    }

    fn snapshot_framebuffer_into(&mut self, dst: &mut [u8; FRAMEBUFFER_SIZE]) {
        dst.copy_from_slice(self.ppu.framebuffer());
    }

    fn drain_samples(&mut self) -> Vec<f32> {
        self.apu.drain_samples()
    }

    fn clear_samples(&mut self) {
        self.apu.clear_samples();
    }

    fn timer_state(&self) -> TimerState {
        self.timer.to_save_state()
    }

    fn ppu_state(&self) -> PpuState {
        self.ppu.to_save_state()
    }

    fn load_state(&mut self, timer: TimerState, ppu: PpuState, vram: &[u8], oam: &[u8]) {
        self.timer.load_state(timer);
        self.ppu.load_state(ppu);
        self.ppu.sync_memory(vram, oam);
        self.apu.clear_samples();
    }

    fn on_vram_write(&mut self, offset: u16, value: u8) {
        self.ppu.on_vram_write(offset, value);
    }

    fn on_oam_write(&mut self, offset: u16, value: u8) {
        self.ppu.on_oam_write(offset, value);
    }

    fn sync_memory(&mut self, vram: &[u8], oam: &[u8]) {
        self.ppu.sync_memory(vram, oam);
    }

    #[cfg(feature = "perf")]
    fn take_ppu_perf_profile(&mut self) -> PpuPerfProfile {
        self.ppu.take_perf_profile()
    }

    #[cfg(feature = "perf")]
    fn take_apu_perf_profile(&mut self) -> ApuPerfProfile {
        self.apu.take_perf_profile()
    }
}

impl From<LocalPeripheralBackend> for Box<dyn PeripheralBackend> {
    fn from(value: LocalPeripheralBackend) -> Self {
        Box::new(value)
    }
}
