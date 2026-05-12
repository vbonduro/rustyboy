mod frontend;
mod inline;
mod protocol;
mod worker;

use alloc::{boxed::Box, vec::Vec};

use crate::cpu::cpu::CpuError;
use crate::cpu::peripheral::joypad::Button;
use crate::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use crate::cpu::registers::{Flags, Registers};
use crate::cpu::save_state::SaveState;
use crate::memory::cartridge::Cartridge;
use crate::memory::memory::{Error as MemoryError, GameBoyMemory};

#[cfg(feature = "perf")]
pub use frontend::FrontendPerfProfile;
pub use frontend::GameBoyFrontend;
pub use inline::InlineWorkerLink;
pub use protocol::{WorkerCommand, WorkerFrontendState, WorkerLink};
pub use worker::GameBoyWorker;

/// Top-level Game Boy emulator coordinator.
///
/// Owns the CPU-facing frontend plus the default inline worker transport.
/// Platforms should use this type rather than `Sm83` directly unless they are
/// explicitly integrating the split frontend/worker API themselves.
pub struct GameBoy {
    frontend: GameBoyFrontend,
    link: InlineWorkerLink,
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
        let frontend = GameBoyFrontend::from_memory(memory);
        let link = InlineWorkerLink::new();
        let mut gb = Self { frontend, link };
        gb.frontend.sync_worker_state(&mut gb.link);
        gb = gb.with_registers(Registers {
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
        });
        gb.with_dmg_state()
    }

    /// Construct a minimal `GameBoy` for unit tests.
    pub fn for_test(rom_data: Vec<u8>) -> Self {
        let mut gb = Self {
            frontend: GameBoyFrontend::for_test(rom_data),
            link: InlineWorkerLink::new(),
        };
        gb.frontend.sync_worker_state(&mut gb.link);
        gb
    }

    /// Like `for_test()`, but also runs a setup closure on the raw memory
    /// before the `GameBoy` is constructed.
    pub fn for_test_with_setup(setup: impl FnOnce(&mut GameBoyMemory), rom_data: Vec<u8>) -> Self {
        let mut gb = Self {
            frontend: GameBoyFrontend::for_test_with_setup(setup, rom_data),
            link: InlineWorkerLink::new(),
        };
        gb.frontend.sync_worker_state(&mut gb.link);
        gb
    }

    /// Set initial register state.
    pub fn with_registers(mut self, registers: Registers) -> Self {
        self.frontend = self.frontend.with_registers(registers);
        self
    }

    /// Seed IO registers to DMG post-boot-ROM state.
    pub fn with_dmg_state(mut self) -> Self {
        self.frontend.apply_dmg_state(&mut self.link);
        self
    }

    /// Execute one complete SM83 instruction.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn tick(&mut self) {
        self.frontend.tick(&mut self.link);
    }

    /// Execute one complete SM83 instruction, returning the T-cycles elapsed.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn step(&mut self) -> Result<u8, CpuError> {
        self.frontend.step(&mut self.link)
    }

    /// Returns a copy of the current CPU registers.
    pub fn registers(&self) -> Registers {
        self.frontend.registers()
    }

    /// Returns true if the interrupt master enable flag is active.
    pub fn ime(&self) -> bool {
        self.frontend.ime()
    }

    /// Returns true if the CPU is halted (waiting for an interrupt).
    pub fn is_halted(&self) -> bool {
        self.frontend.is_halted()
    }

    /// Returns the last fully-rendered frame.
    #[inline(always)]
    pub fn front_buffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.frontend.front_buffer()
    }

    /// Drain accumulated PCM samples since the last call.
    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.link.drain_audio_samples()
    }

    /// Drain accumulated interleaved stereo i16 PCM samples into a caller-owned buffer.
    pub fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>) {
        self.link.drain_audio_samples_into_i16(out);
    }

    /// Returns all bytes captured by the serial port (SB transfers via SC).
    pub fn serial_output(&self) -> &[u8] {
        self.frontend.serial_output()
    }

    /// Read a byte from the memory bus (for test/debug access).
    pub fn read_memory(&self, address: u16) -> Result<u8, MemoryError> {
        self.frontend.read_memory(address)
    }

    /// Write a byte to the IO region (0xFF00–0xFFFF). For tests and setup.
    pub fn write_io(&mut self, address: u16, value: u8) {
        self.frontend.write_io(address, value);
    }

    /// Read a byte from the IO region (0xFF00–0xFFFF). For tests and inspection.
    pub fn read_io(&self, address: u16) -> u8 {
        self.frontend.read_io(address)
    }

    /// Press or release a joypad button. Fires the joypad interrupt if the
    /// button is newly pressed and its select line is active.
    pub fn set_button(&mut self, btn: Button, pressed: bool) {
        self.frontend.set_button(btn, pressed);
    }

    /// Total T-cycles elapsed since power-on.
    #[inline(always)]
    pub fn cycle_counter(&self) -> u64 {
        self.frontend.cycle_counter()
    }

    /// Returns the currently mapped ROM bank for the switchable window (0x4000–0x7FFF).
    pub fn current_rom_bank(&self) -> usize {
        self.frontend.current_rom_bank()
    }

    /// Serialize the full emulator state to an RBSS v1 blob.
    pub fn save_state(&self) -> Vec<u8> {
        self.frontend.save_state(&self.link)
    }

    /// Restore emulator state from a parsed [`SaveState`].
    pub fn load_state(&mut self, state: SaveState) -> Result<(), &'static str> {
        self.frontend.load_state(state, &mut self.link)?;
        Ok(())
    }

    /// Returns the cartridge external RAM (battery save data), or `None` if the
    /// cartridge has no RAM.
    pub fn external_ram(&self) -> Option<&[u8]> {
        self.frontend.external_ram()
    }

    /// Overwrites the cartridge external RAM with the provided data. No-op if the
    /// cartridge has no external RAM.
    pub fn set_external_ram(&mut self, data: &[u8]) {
        self.frontend.set_external_ram(data);
    }

    #[cfg(feature = "perf")]
    pub fn take_perf_profile(&mut self) -> crate::cpu::sm83::Sm83PerfProfile {
        self.frontend.take_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_frontend_perf_profile(&mut self) -> FrontendPerfProfile {
        self.frontend.take_frontend_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_ppu_perf_profile(&mut self) -> crate::cpu::peripheral::ppu::PpuPerfProfile {
        self.link.take_ppu_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_apu_perf_profile(&mut self) -> crate::cpu::peripheral::apu::ApuPerfProfile {
        self.link.take_apu_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_cartridge_perf_profile(
        &mut self,
    ) -> crate::memory::cartridge::CartridgePerfProfile {
        self.frontend.take_cartridge_perf_profile()
    }
}
