use alloc::{boxed::Box, vec::Vec};

use crate::cpu::cpu::CpuError;
use crate::cpu::peripheral::apu::{
    ApuPeripheral, NR10_ADDR, NR52_ADDR, WAVE_RAM_START, WAVE_RAM_END,
};
use crate::cpu::peripheral::joypad::{Button, JoypadPeripheral, JOYP_ADDR, JOYPAD_INTERRUPT_BIT};
use crate::cpu::peripheral::ppu::{
    PpuPeripheral, FRAMEBUFFER_SIZE, LCDC_ADDR, STAT_ADDR,
    LY_ADDR, BGP_ADDR, OBP0_ADDR, OBP1_ADDR,
    VBLANK_INTERRUPT_BIT, STAT_INTERRUPT_BIT,
};
use crate::cpu::peripheral::serial::{SerialPort, SERIAL_INTERRUPT_BIT};
use crate::cpu::peripheral::timer::{
    TimerPeripheral, DIV_ADDR, TIMA_ADDR, TIMER_INTERRUPT_BIT, TMA_ADDR, TAC_ADDR,
};
#[cfg(feature = "perf")]
use crate::cpu::perf::cyccnt;
use crate::cpu::registers::{Flags, Registers};
use crate::cpu::save_state::{CpuState, SaveState};
use crate::cpu::sm83::Sm83;
use crate::memory::cartridge::Cartridge;
use crate::memory::memory::{BusEvent, Error as MemoryError, GameBoyMemory, Memory as MemoryTrait};

const IF_ADDR: u16 = 0xFF0F;
const DMA_ADDR: u16 = 0xFF46;
const SB_ADDR: u16 = 0xFF01;
const SC_ADDR: u16 = 0xFF02;

/// State for an in-progress OAM DMA transfer.
pub(crate) struct DmaState {
    /// Source base address (page << 8).
    pub source: u16,
    /// Number of bytes copied so far (0–159).
    pub progress: u8,
}

/// Top-level Game Boy emulator coordinator.
///
/// Owns the CPU state machine plus all hardware components. Platforms should
/// use this type rather than `Sm83` directly.
///
/// Two constructors are provided:
/// - [`GameBoy::new`]: load from raw ROM bytes (cart type auto-detected from header)
/// - [`GameBoy::with_cartridge`]: for platform-specific cartridge impls (XIP flash, streaming, etc.)
pub struct GameBoy {
    // ── Pure CPU state machine ────────────────────────────────────────────────
    cpu: Sm83,
    // ── Peripheral and memory state ───────────────────────────────────────────
    memory: Box<GameBoyMemory>,
    ppu: PpuPeripheral,
    apu: ApuPeripheral,
    timer: TimerPeripheral,
    joypad: JoypadPeripheral,
    serial: SerialPort,
    dma: Option<DmaState>,
    front_buffer: [u8; FRAMEBUFFER_SIZE],
    /// Reusable scratch buffer for draining bus events — avoids per-call heap allocation.
    bus_event_buf: Vec<BusEvent>,
    /// Total committed T-cycles.
    cycle_counter: u64,
    #[cfg(feature = "perf")]
    perf_enabled: bool,
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

    fn from_memory(mut memory: Box<GameBoyMemory>) -> Self {
        let cpu = Sm83::new_pure();
        let joypad = JoypadPeripheral::new();
        let apu = ApuPeripheral::new();

        // Seed JOYP with no buttons pressed (all lines high).
        memory.write_io(JOYP_ADDR, joypad.read());
        // Seed IO memory with initial APU register read values.
        for addr in NR10_ADDR..=NR52_ADDR {
            memory.write_io(addr, apu.read_register(addr));
        }
        // Unused APU addresses always read as 0xFF
        for addr in 0xFF27u16..WAVE_RAM_START {
            memory.write_io(addr, 0xFF);
        }
        for addr in WAVE_RAM_START..=WAVE_RAM_END {
            let offset = (addr - WAVE_RAM_START) as u8;
            memory.write_io(addr, apu.read_wave_ram(offset));
        }

        let mut gb = Self {
            cpu,
            memory,
            ppu: PpuPeripheral::new(),
            apu,
            timer: TimerPeripheral::new(),
            joypad,
            serial: SerialPort::new(),
            dma: None,
            front_buffer: [0u8; FRAMEBUFFER_SIZE],
            bus_event_buf: Vec::with_capacity(4),
            cycle_counter: 0,
            #[cfg(feature = "perf")]
            perf_enabled: false,
        };
        // Apply default DMG register state
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
        }).with_dmg_state();
        gb
    }

    // ── Test helpers ──────────────────────────────────────────────────────────

    /// Construct a minimal `GameBoy` for unit tests.
    ///
    /// - `rom_data`: raw bytes placed at the start of address space (PC=0).
    /// - Registers start at `Registers::default()` (PC=0, SP=0, all zeros).
    /// - No DMG state is applied — peripherals start at their hardware-reset
    ///   defaults, which is appropriate for instruction-level unit tests.
    ///
    /// Available in all build configurations (not test-only) so that the
    /// `#[cfg(test)]` module inside `sm83.rs` can use it.
    pub fn for_test(rom_data: alloc::vec::Vec<u8>) -> Self {
        let memory = Box::new(GameBoyMemory::with_rom(rom_data));
        let cpu = Sm83::new_pure();
        let joypad = JoypadPeripheral::new();
        let apu = ApuPeripheral::new();

        // Seed IO memory the same way from_memory() does
        let mut mem = memory;
        mem.write_io(JOYP_ADDR, joypad.read());
        for addr in NR10_ADDR..=NR52_ADDR {
            mem.write_io(addr, apu.read_register(addr));
        }
        for addr in 0xFF27u16..WAVE_RAM_START {
            mem.write_io(addr, 0xFF);
        }
        for addr in WAVE_RAM_START..=WAVE_RAM_END {
            let offset = (addr - WAVE_RAM_START) as u8;
            mem.write_io(addr, apu.read_wave_ram(offset));
        }

        Self {
            cpu,
            memory: mem,
            ppu: PpuPeripheral::new(),
            apu,
            timer: TimerPeripheral::new(),
            joypad,
            serial: SerialPort::new(),
            dma: None,
            front_buffer: [0u8; FRAMEBUFFER_SIZE],
            bus_event_buf: Vec::with_capacity(4),
            cycle_counter: 0,
            #[cfg(feature = "perf")]
            perf_enabled: false,
        }
    }

    /// Like `for_test()`, but also runs a setup closure on the raw memory
    /// before the `GameBoy` is constructed.
    pub fn for_test_with_setup(
        setup: impl FnOnce(&mut GameBoyMemory),
        rom_data: alloc::vec::Vec<u8>,
    ) -> Self {
        let mut mem = GameBoyMemory::with_rom(rom_data);
        setup(&mut mem);
        let cpu = Sm83::new_pure();
        let joypad = JoypadPeripheral::new();
        let apu = ApuPeripheral::new();

        mem.write_io(JOYP_ADDR, joypad.read());
        for addr in NR10_ADDR..=NR52_ADDR {
            mem.write_io(addr, apu.read_register(addr));
        }
        for addr in 0xFF27u16..WAVE_RAM_START {
            mem.write_io(addr, 0xFF);
        }
        for addr in WAVE_RAM_START..=WAVE_RAM_END {
            let offset = (addr - WAVE_RAM_START) as u8;
            mem.write_io(addr, apu.read_wave_ram(offset));
        }

        Self {
            cpu,
            memory: Box::new(mem),
            ppu: PpuPeripheral::new(),
            apu,
            timer: TimerPeripheral::new(),
            joypad,
            serial: SerialPort::new(),
            dma: None,
            front_buffer: [0u8; FRAMEBUFFER_SIZE],
            bus_event_buf: Vec::with_capacity(4),
            cycle_counter: 0,
            #[cfg(feature = "perf")]
            perf_enabled: false,
        }
    }

    // ── Builder pattern ────────────────────────────────────────────────────────

    /// Set initial register state.
    pub fn with_registers(mut self, registers: Registers) -> Self {
        self.cpu.registers = registers;
        self
    }

    /// Seed IO registers to DMG post-boot-ROM state.
    pub fn with_dmg_state(mut self) -> Self {
        self.memory.write_io(LCDC_ADDR, 0x91);
        self.memory.write_io(STAT_ADDR, 0x85);
        self.memory.write_io(BGP_ADDR,  0xFC);
        self.memory.write_io(OBP0_ADDR, 0xFF);
        self.memory.write_io(OBP1_ADDR, 0xFF);
        // Sync prev_stat_line so the first PPU tick doesn't generate a spurious
        // rising-edge STAT interrupt from the seeded STAT/LY/LYC state.
        self.ppu.sync_prev_stat_line(self.memory.io_slice());
        // APU post-boot state
        self.write_apu_register(0xFF26, 0xF1);
        self.write_apu_register(0xFF25, 0xF3);
        self.write_apu_register(0xFF24, 0x77);
        self
    }

    // ── Emulation loop ────────────────────────────────────────────────────────

    /// Execute one complete SM83 instruction.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn tick(&mut self) {
        let _ = self.step();
    }

    /// Execute one complete SM83 instruction, returning the T-cycles elapsed.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn step(&mut self) -> Result<u8, CpuError> {
        let t_cycles = self.cpu.step(&mut self.memory) as u16;
        // Route any IO write events that occurred during CPU execution
        self.route_bus_events();
        // Advance all peripherals by the instruction's T-cycle count
        self.advance_peripherals(t_cycles);
        self.cycle_counter = self.cycle_counter.wrapping_add(t_cycles as u64);
        Ok(t_cycles as u8)
    }

    // ── Peripheral advancement ────────────────────────────────────────────────

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_peripherals(&mut self, cycles: u16) {
        self.advance_ppu(cycles);
        self.advance_timer(cycles);
        self.tick_apu(cycles);
        if self.memory.has_rtc() {
            self.memory.tick_rtc(cycles as u32);
        }
        if !self.serial.is_idle() {
            self.advance_serial(cycles);
        }
        self.advance_dma_bulk(cycles);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_ppu(&mut self, cycles: u16) {
        #[cfg(feature = "perf")]
        let t0 = cyccnt();
        let output = {
            let (io, vram, oam) = self.memory.ppu_tick_data();
            self.ppu.tick(cycles, io, vram, oam)
        };
        if output.vblank_interrupt {
            self.front_buffer.copy_from_slice(self.ppu.framebuffer());
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_ | (1 << VBLANK_INTERRUPT_BIT));
        }
        if output.stat_interrupt {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_ | (1 << STAT_INTERRUPT_BIT));
        }
        #[cfg(feature = "perf")]
        { let _ = t0; }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_timer(&mut self, cycles: u16) {
        if self.timer.tick(cycles) {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_ | (1 << TIMER_INTERRUPT_BIT));
        }
        // Sync dynamic timer registers so CPU reads see current state.
        self.memory.write_io(DIV_ADDR, self.timer.div());
        self.memory.write_io(TIMA_ADDR, self.timer.tima());
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_serial(&mut self, cycles: u16) {
        if self.serial.tick(cycles) {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_ | (1 << SERIAL_INTERRUPT_BIT));
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn tick_apu(&mut self, cycles: u16) {
        let output = self.apu.tick(cycles, self.timer.internal_counter());
        self.memory.write_io(NR52_ADDR, output.nr52);
    }

    /// Advance DMA in bulk: process one DMA byte per 4 T-cycles.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_dma_bulk(&mut self, cycles: u16) {
        let steps = cycles / 4;
        for _ in 0..steps {
            let (source, progress) = match self.dma {
                Some(ref d) => (d.source, d.progress),
                None => break,
            };
            let byte = self.memory.read_fast(source + progress as u16);
            self.memory.write_fast(0xFE00 + progress as u16, byte);
            let next = progress + 1;
            self.dma = if next < 160 {
                Some(DmaState { source, progress: next })
            } else {
                None
            };
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn route_bus_events(&mut self) {
        if !self.memory.has_events() { return; }
        // mem::take swaps out the persistent buffer so we can borrow both
        // self.memory and the buffer without aliasing. The Vec's allocation
        // is retained across calls; only the pointer/len/cap are moved.
        let mut buf = core::mem::take(&mut self.bus_event_buf);
        self.memory.drain_into(&mut buf);
        for i in 0..buf.len() {
            self.handle_bus_event(buf[i].address, buf[i].value);
        }
        buf.clear();
        self.bus_event_buf = buf;
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn handle_bus_event(&mut self, addr: u16, value: u8) {
        match addr {
            a if a == IF_ADDR => {
                // Already written to memory by the CPU; no further action needed.
                // The CPU reads IF directly from memory, so this is a no-op.
            }
            a if a == JOYP_ADDR => {
                self.joypad.write(value);
                self.memory.write_io(JOYP_ADDR, self.joypad.read());
            }
            a if a == SB_ADDR => self.serial.set_sb(value),
            a if a == SC_ADDR => {
                self.serial.handle_sc_write(value);
            }
            a if a == DIV_ADDR => {
                self.timer.reset_div();
            }
            a if a == LY_ADDR => self.ppu.reset_ly(),
            a if a == DMA_ADDR => {
                self.dma = Some(DmaState { source: (value as u16) << 8, progress: 0 });
            }
            a if (NR10_ADDR..=NR52_ADDR).contains(&a) => self.write_apu_register(a, value),
            a if (0xFF27u16..WAVE_RAM_START).contains(&a) => self.memory.write_io(a, 0xFF),
            a if (WAVE_RAM_START..=WAVE_RAM_END).contains(&a) => self.write_wave_ram(a, value),
            a if a == TIMA_ADDR => self.timer.set_tima(value),
            a if a == TMA_ADDR  => self.timer.set_tma(value),
            a if a == TAC_ADDR  => self.timer.set_tac(value),
            // PPU config registers (LCDC, STAT, SCY, SCX, LYC, BGP, OBP0, OBP1, WY, WX) are
            // already written to memory.io[] by Memory::write(). PPU reads them directly
            // from io[] on each tick — no secondary copy to update.
            _ => {}
        }
    }

    fn write_apu_register(&mut self, addr: u16, value: u8) {
        self.apu.write_register(addr, value);
        if addr == NR52_ADDR {
            for a in NR10_ADDR..=NR52_ADDR {
                self.memory.write_io(a, self.apu.read_register(a));
            }
        } else {
            self.memory.write_io(addr, self.apu.read_register(addr));
        }
    }

    fn write_wave_ram(&mut self, addr: u16, value: u8) {
        let offset = (addr - WAVE_RAM_START) as u8;
        self.apu.write_wave_ram(offset, value);
        self.memory.write_io(addr, self.apu.read_wave_ram(offset));
    }

    // ── CPU state ─────────────────────────────────────────────────────────────

    /// Returns a copy of the current CPU registers.
    pub fn registers(&self) -> Registers {
        self.cpu.registers.clone()
    }

    /// Returns true if the interrupt master enable flag is active.
    pub fn ime(&self) -> bool {
        self.cpu.ime()
    }

    /// Returns true if the CPU is halted (waiting for an interrupt).
    pub fn is_halted(&self) -> bool {
        self.cpu.is_halted()
    }

    // ── Output ────────────────────────────────────────────────────────────────

    /// Returns the last fully-rendered frame.
    ///
    /// 160×144 pixels, one byte per pixel, 2-bit shade value (0=white, 3=black).
    #[inline(always)]
    pub fn front_buffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        &self.front_buffer
    }

    /// Drain accumulated PCM samples since the last call.
    ///
    /// Returns interleaved stereo f32 samples `[L, R, L, R, ...]` at 48,000 Hz.
    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.apu.drain_samples()
    }

    /// Returns all bytes captured by the serial port (SB transfers via SC).
    pub fn serial_output(&self) -> &[u8] {
        self.serial.output()
    }

    // ── Memory access ─────────────────────────────────────────────────────────

    /// Read a byte from the memory bus (for test/debug access).
    pub fn read_memory(&self, address: u16) -> Result<u8, MemoryError> {
        MemoryTrait::read(self.memory.as_ref(), address)
    }

    /// Write a byte to the IO region (0xFF00–0xFFFF). For tests and setup.
    pub fn write_io(&mut self, address: u16, value: u8) {
        self.memory.write_io(address, value);
    }

    /// Read a byte from the IO region (0xFF00–0xFFFF). For tests and inspection.
    pub fn read_io(&self, address: u16) -> u8 {
        self.memory.read_io(address)
    }

    // ── Input ─────────────────────────────────────────────────────────────────

    /// Press or release a joypad button. Fires the joypad interrupt if the
    /// button is newly pressed and its select line is active.
    pub fn set_button(&mut self, btn: Button, pressed: bool) {
        let interrupt = self.joypad.set_button(btn, pressed);
        self.memory.write_io(JOYP_ADDR, self.joypad.read());
        if interrupt {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_ | (1 << JOYPAD_INTERRUPT_BIT));
        }
    }

    // ── Timing ────────────────────────────────────────────────────────────────

    /// Total T-cycles elapsed since power-on.
    #[inline(always)]
    pub fn cycle_counter(&self) -> u64 {
        self.cycle_counter
    }

    // ── Cart info ─────────────────────────────────────────────────────────────

    /// Returns the currently mapped ROM bank for the switchable window (0x4000–0x7FFF).
    pub fn current_rom_bank(&self) -> usize {
        self.memory.current_rom_bank()
    }

    // ── Save state ────────────────────────────────────────────────────────────

    /// Serialize the full emulator state to an RBSS v1 blob.
    pub fn save_state(&self) -> Vec<u8> {
        let cpu_state = CpuState {
            a: self.cpu.registers.a,
            b: self.cpu.registers.b,
            c: self.cpu.registers.c,
            d: self.cpu.registers.d,
            e: self.cpu.registers.e,
            h: self.cpu.registers.h,
            l: self.cpu.registers.l,
            f: self.cpu.registers.f,
            sp: self.cpu.registers.sp,
            pc: self.cpu.registers.pc,
            ime: self.cpu.ime,
            halted: self.cpu.halted,
            cycle_counter: self.cycle_counter(),
        };
        SaveState::serialize(cpu_state, self.timer.to_save_state(), self.ppu.to_save_state(self.memory.io_slice()), &self.memory)
    }

    /// Restore emulator state from a parsed [`SaveState`].
    pub fn load_state(&mut self, state: SaveState) -> Result<(), &'static str> {
        self.cpu.registers = state.cpu.to_registers();
        self.cpu.ime = state.cpu.ime;
        self.cpu.halted = state.cpu.halted;
        self.cycle_counter = state.cpu.cycle_counter;
        self.timer.load_state(state.timer);
        self.ppu.load_state(state.ppu);
        self.memory.load_state(&state);
        // Sync PPU prev_stat_line to avoid spurious STAT interrupt on first tick.
        self.ppu.sync_prev_stat_line(self.memory.io_slice());
        Ok(())
    }

    // ── Cart RAM (battery save) ───────────────────────────────────────────────

    /// Returns the cartridge external RAM (battery save data), or `None` if the
    /// cartridge has no RAM.
    pub fn external_ram(&self) -> Option<&[u8]> {
        self.memory.external_ram()
    }

    /// Overwrites the cartridge external RAM with the provided data. No-op if the
    /// cartridge has no external RAM.
    pub fn set_external_ram(&mut self, data: &[u8]) {
        self.memory.set_external_ram(data);
    }

    // ── Perf profiling ────────────────────────────────────────────────────────

    #[cfg(feature = "perf")]
    pub fn take_perf_profile(&mut self) -> crate::cpu::sm83::Sm83PerfProfile {
        self.cpu.perf.take_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_ppu_perf_profile(&mut self) -> crate::cpu::peripheral::ppu::PpuPerfProfile {
        self.ppu.take_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_apu_perf_profile(&mut self) -> crate::cpu::peripheral::apu::ApuPerfProfile {
        self.apu.take_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_cartridge_perf_profile(
        &mut self,
    ) -> crate::memory::cartridge::CartridgePerfProfile {
        self.memory.take_cartridge_perf_profile()
    }
}
