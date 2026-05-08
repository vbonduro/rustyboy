use alloc::{boxed::Box, vec::Vec};

use crate::cpu::instructions::opcodes::OpCodeDecoder;
use crate::cpu::peripheral::joypad::Button;
use crate::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use crate::cpu::registers::{Flags, Registers};
use crate::cpu::save_state::SaveState;
use crate::cpu::sm83::Sm83;
use crate::memory::cartridge::Cartridge;
use crate::memory::memory::GameBoyMemory;

/// Top-level Game Boy emulator coordinator.
///
/// Owns the CPU state machine plus all hardware components. Platforms should
/// use this type rather than `Sm83` directly.
///
/// Two constructors are provided:
/// - [`GameBoy::new`]: load from raw ROM bytes (cart type auto-detected from header)
/// - [`GameBoy::with_cartridge`]: for platform-specific cartridge impls (XIP flash, streaming, etc.)
pub struct GameBoy {
    cpu: Sm83,
}

impl GameBoy {
    /// Construct from raw ROM bytes. Cart type is auto-detected from the ROM header.
    pub fn new(rom: Vec<u8>) -> Self {
        let memory = Box::new(GameBoyMemory::with_rom(rom));
        Self::from_memory(memory)
    }

    /// Construct from a pre-built cartridge. Use this when the cartridge is built
    /// externally (e.g. XIP flash mapping on embedded targets, streaming from SD).
    pub fn with_cartridge(cart: Box<dyn Cartridge>) -> Self {
        let memory = Box::new(GameBoyMemory::with_cartridge(cart));
        Self::from_memory(memory)
    }

    fn from_memory(memory: Box<GameBoyMemory>) -> Self {
        let decoder = Box::new(OpCodeDecoder::new());
        let cpu = Sm83::new(memory, decoder)
            .with_registers(Registers {
                a: 0x01,
                f: Flags::from_bits_truncate(0xB0),
                b: 0x00,
                c: 0x13,
                d: 0x00,
                e: 0xD8,
                h: 0x01,
                l: 0x4D,
                pc: 0x0100,
                sp: 0xFFFE,
            })
            .with_dmg_state();
        Self { cpu }
    }

    // ── Emulation loop ────────────────────────────────────────────────────────

    /// Execute one complete SM83 instruction.
    #[inline(always)]
    pub fn tick(&mut self) {
        let _ = self.cpu.step();
    }

    // ── Output ────────────────────────────────────────────────────────────────

    /// Returns the last fully-rendered frame.
    ///
    /// 160×144 pixels, one byte per pixel, 2-bit shade value (0=white, 3=black).
    /// Snapshotted at VBlank so it always contains a complete frame.
    #[inline(always)]
    pub fn front_buffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.cpu.framebuffer()
    }

    /// Drain accumulated PCM samples since the last call.
    ///
    /// Returns interleaved stereo f32 samples `[L, R, L, R, ...]` at 48,000 Hz.
    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.cpu.drain_audio_samples()
    }

    // ── Input ─────────────────────────────────────────────────────────────────

    /// Press or release a joypad button. Fires the joypad interrupt if the
    /// button is newly pressed and its select line is active.
    pub fn set_button(&mut self, btn: Button, pressed: bool) {
        self.cpu.set_button(btn, pressed);
    }

    // ── Timing ────────────────────────────────────────────────────────────────

    /// Total T-cycles elapsed since power-on.
    #[inline(always)]
    pub fn cycle_counter(&self) -> u64 {
        self.cpu.cycle_counter()
    }

    // ── Save state ────────────────────────────────────────────────────────────

    /// Serialize the full emulator state to an RBSS v1 blob.
    pub fn save_state(&self) -> Vec<u8> {
        self.cpu.save_state()
    }

    /// Restore emulator state from a parsed [`SaveState`].
    pub fn load_state(&mut self, state: SaveState) -> Result<(), &'static str> {
        self.cpu.load_state(state)
    }

    // ── Cart RAM (battery save) ───────────────────────────────────────────────

    /// Returns the cartridge external RAM (battery save data), or `None` if the
    /// cartridge has no RAM.
    pub fn external_ram(&self) -> Option<&[u8]> {
        self.cpu.external_ram()
    }

    /// Overwrites the cartridge external RAM with the provided data. No-op if the
    /// cartridge has no external RAM.
    pub fn set_external_ram(&mut self, data: &[u8]) {
        self.cpu.set_external_ram(data);
    }

    // ── Perf profiling ────────────────────────────────────────────────────────

    #[cfg(feature = "perf")]
    pub fn take_perf_profile(&mut self) -> crate::cpu::sm83::Sm83PerfProfile {
        self.cpu.take_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_ppu_perf_profile(&mut self) -> crate::cpu::peripheral::ppu::PpuPerfProfile {
        self.cpu.take_ppu_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_apu_perf_profile(&mut self) -> crate::cpu::peripheral::apu::ApuPerfProfile {
        self.cpu.take_apu_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_cartridge_perf_profile(
        &mut self,
    ) -> crate::memory::cartridge::CartridgePerfProfile {
        self.cpu.take_cartridge_perf_profile()
    }
}
