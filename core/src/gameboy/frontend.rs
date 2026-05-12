use alloc::{boxed::Box, vec::Vec};

use crate::cpu::cpu::CpuError;
use crate::cpu::peripheral::apu::{
    ApuPeripheral, NR10_ADDR, NR52_ADDR, WAVE_RAM_END, WAVE_RAM_START,
};
use crate::cpu::peripheral::joypad::{Button, JoypadPeripheral, JOYPAD_INTERRUPT_BIT, JOYP_ADDR};
use crate::cpu::peripheral::ppu::{
    BGP_ADDR, FRAMEBUFFER_SIZE, LCDC_ADDR, LY_ADDR, OBP0_ADDR, OBP1_ADDR, STAT_ADDR,
};
use crate::cpu::peripheral::serial::{SerialPort, SERIAL_INTERRUPT_BIT};
use crate::cpu::peripheral::timer::{
    TimerPeripheral, DIV_ADDR, TAC_ADDR, TIMA_ADDR, TIMER_INTERRUPT_BIT, TMA_ADDR,
};
use crate::cpu::registers::Registers;
use crate::cpu::save_state::{CpuState, SaveState};
use crate::cpu::sm83::Sm83;
use crate::memory::memory::{BusEvent, Error as MemoryError, GameBoyMemory, Memory as MemoryTrait};

use super::protocol::{WorkerCommand, WorkerLink};

#[cfg(feature = "perf")]
use crate::cpu::perf::cyccnt;

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

#[cfg(feature = "perf")]
#[derive(Default)]
pub struct FrontendPerfProfile {
    pub step_total: u32,
    pub cpu_step: u32,
    pub route_bus_events: u32,
    pub ppu_timing: u32,
    pub ppu_sync: u32,
    pub timer: u32,
    pub apu_state: u32,
    pub apu_send: u32,
    pub rtc: u32,
    pub serial: u32,
    pub dma: u32,
    pub steps: u32,
    pub bus_events: u32,
    pub render_scanlines: u32,
    pub dma_bytes: u32,
}

pub struct GameBoyFrontend {
    cpu: Sm83,
    memory: Box<GameBoyMemory>,
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
    perf_profile: FrontendPerfProfile,
}

impl GameBoyFrontend {
    pub fn from_memory(mut memory: Box<GameBoyMemory>) -> Self {
        let cpu = Sm83::new_pure();
        let joypad = JoypadPeripheral::new();
        let apu = ApuPeripheral::new();

        seed_joypad_io(&mut memory, &joypad);
        seed_apu_io(&mut memory, &apu);

        Self {
            cpu,
            memory,
            apu,
            timer: TimerPeripheral::new(),
            joypad,
            serial: SerialPort::new(),
            dma: None,
            front_buffer: [0u8; FRAMEBUFFER_SIZE],
            bus_event_buf: Vec::with_capacity(4),
            cycle_counter: 0,
            #[cfg(feature = "perf")]
            perf_profile: FrontendPerfProfile::default(),
        }
    }

    /// Construct a minimal frontend for unit tests.
    pub fn for_test(rom_data: Vec<u8>) -> Self {
        let memory = Box::new(GameBoyMemory::with_rom(rom_data));
        Self::from_memory(memory)
    }

    /// Like `for_test()`, but also runs a setup closure on the raw memory
    /// before the frontend is constructed.
    pub fn for_test_with_setup(setup: impl FnOnce(&mut GameBoyMemory), rom_data: Vec<u8>) -> Self {
        let mut mem = GameBoyMemory::with_rom(rom_data);
        setup(&mut mem);
        Self::from_memory(Box::new(mem))
    }

    /// Set initial register state.
    pub fn with_registers(mut self, registers: Registers) -> Self {
        self.cpu.registers = registers;
        self
    }

    /// Seed IO registers to DMG post-boot-ROM state.
    pub fn apply_dmg_state(&mut self, link: &mut impl WorkerLink) {
        self.memory.write_io(LCDC_ADDR, 0x91);
        self.memory.write_io(STAT_ADDR, 0x85);
        self.memory.write_io(BGP_ADDR, 0xFC);
        self.memory.write_io(OBP0_ADDR, 0xFF);
        self.memory.write_io(OBP1_ADDR, 0xFF);
        // APU post-boot state
        self.write_apu_register(0xFF26, 0xF1, link);
        self.write_apu_register(0xFF25, 0xF3, link);
        self.write_apu_register(0xFF24, 0x77, link);
        self.sync_ppu_worker(link);
    }

    /// Execute one complete SM83 instruction.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn tick(&mut self, link: &mut impl WorkerLink) {
        let _ = self.step(link);
    }

    /// Execute one complete SM83 instruction, returning the T-cycles elapsed.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn step(&mut self, link: &mut impl WorkerLink) -> Result<u8, CpuError> {
        #[cfg(feature = "perf")]
        let step_start = cyccnt();

        #[cfg(feature = "perf")]
        let cpu_start = cyccnt();
        let t_cycles = self.cpu.step(&mut self.memory) as u16;
        #[cfg(feature = "perf")]
        {
            self.perf_profile.cpu_step = self
                .perf_profile
                .cpu_step
                .wrapping_add(cyccnt().wrapping_sub(cpu_start));
        }

        #[cfg(feature = "perf")]
        let route_start = cyccnt();
        #[cfg(feature = "perf")]
        let bus_events = self.route_bus_events(link);
        #[cfg(not(feature = "perf"))]
        self.route_bus_events(link);
        #[cfg(feature = "perf")]
        {
            self.perf_profile.route_bus_events = self
                .perf_profile
                .route_bus_events
                .wrapping_add(cyccnt().wrapping_sub(route_start));
            self.perf_profile.bus_events =
                self.perf_profile.bus_events.wrapping_add(bus_events as u32);
        }

        self.advance_peripherals(t_cycles, link);
        self.sync_worker_frontend_state(link);
        self.cycle_counter = self.cycle_counter.wrapping_add(t_cycles as u64);

        #[cfg(feature = "perf")]
        {
            self.perf_profile.steps = self.perf_profile.steps.wrapping_add(1);
            self.perf_profile.step_total = self
                .perf_profile
                .step_total
                .wrapping_add(cyccnt().wrapping_sub(step_start));
        }
        Ok(t_cycles as u8)
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_peripherals(&mut self, cycles: u16, link: &mut impl WorkerLink) {
        self.advance_ppu(cycles, link);
        #[cfg(feature = "perf")]
        let timer_start = cyccnt();
        self.advance_timer(cycles);
        #[cfg(feature = "perf")]
        {
            self.perf_profile.timer = self
                .perf_profile
                .timer
                .wrapping_add(cyccnt().wrapping_sub(timer_start));
        }

        self.tick_apu(cycles, link);

        if self.memory.has_rtc() {
            #[cfg(feature = "perf")]
            let rtc_start = cyccnt();
            self.memory.tick_rtc(cycles as u32);
            #[cfg(feature = "perf")]
            {
                self.perf_profile.rtc = self
                    .perf_profile
                    .rtc
                    .wrapping_add(cyccnt().wrapping_sub(rtc_start));
            }
        }
        if !self.serial.is_idle() {
            #[cfg(feature = "perf")]
            let serial_start = cyccnt();
            self.advance_serial(cycles);
            #[cfg(feature = "perf")]
            {
                self.perf_profile.serial = self
                    .perf_profile
                    .serial
                    .wrapping_add(cyccnt().wrapping_sub(serial_start));
            }
        }
        self.advance_dma_bulk(cycles, link);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_ppu(&mut self, cycles: u16, link: &mut impl WorkerLink) {
        #[cfg(feature = "perf")]
        let timing_start = cyccnt();
        link.send(WorkerCommand::AdvancePpu { cycles });
        #[cfg(feature = "perf")]
        {
            self.perf_profile.ppu_timing = self
                .perf_profile
                .ppu_timing
                .wrapping_add(cyccnt().wrapping_sub(timing_start));
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_timer(&mut self, cycles: u16) {
        if self.timer.tick(cycles) {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory
                .write_io(IF_ADDR, if_ | (1 << TIMER_INTERRUPT_BIT));
        }
        self.memory.write_io(DIV_ADDR, self.timer.div());
        self.memory.write_io(TIMA_ADDR, self.timer.tima());
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_serial(&mut self, cycles: u16) {
        if self.serial.tick(cycles) {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory
                .write_io(IF_ADDR, if_ | (1 << SERIAL_INTERRUPT_BIT));
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn tick_apu(&mut self, cycles: u16, link: &mut impl WorkerLink) {
        #[cfg(feature = "perf")]
        let apu_send_start = cyccnt();
        link.send(WorkerCommand::AdvanceApu {
            cycles,
            div_counter: self.timer.internal_counter(),
        });
        #[cfg(feature = "perf")]
        {
            self.perf_profile.apu_state = self.perf_profile.apu_state.wrapping_add(0);
            self.perf_profile.apu_send = self
                .perf_profile
                .apu_send
                .wrapping_add(cyccnt().wrapping_sub(apu_send_start));
        }
    }

    /// Advance DMA in bulk: process one DMA byte per 4 T-cycles.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_dma_bulk(&mut self, cycles: u16, link: &mut impl WorkerLink) {
        #[cfg(feature = "perf")]
        let dma_start = cyccnt();
        let steps = cycles / 4;
        #[cfg(feature = "perf")]
        let mut copied = 0usize;
        let mut dma_completed = false;
        for _ in 0..steps {
            let (source, progress) = match self.dma {
                Some(ref d) => (d.source, d.progress),
                None => break,
            };
            let byte = self.memory.read_fast(source + progress as u16);
            self.memory.write_fast(0xFE00 + progress as u16, byte);
            #[cfg(feature = "perf")]
            {
                copied += 1;
            }
            let next = progress + 1;
            self.dma = if next < 160 {
                Some(DmaState {
                    source,
                    progress: next,
                })
            } else {
                dma_completed = true;
                None
            };
        }
        if dma_completed {
            link.write_oam_range(0, &self.memory.oam()[..0xA0]);
        }
        #[cfg(feature = "perf")]
        {
            self.perf_profile.dma = self
                .perf_profile
                .dma
                .wrapping_add(cyccnt().wrapping_sub(dma_start));
            self.perf_profile.dma_bytes = self.perf_profile.dma_bytes.wrapping_add(copied as u32);
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn route_bus_events(&mut self, link: &mut impl WorkerLink) -> usize {
        if !self.memory.has_events() {
            return 0;
        }
        let mut buf = core::mem::take(&mut self.bus_event_buf);
        self.memory.drain_into(&mut buf);
        let event_count = buf.len();
        let mut i = 0usize;
        while i < buf.len() {
            if let Some((start_offset, len)) = contiguous_region_len(&buf, i, 0x8000, 0x9FFF) {
                let start = start_offset as usize;
                let end = start + len;
                link.write_vram_range(start_offset, &self.memory.vram()[start..end]);
                i += len;
                continue;
            }
            if let Some((start_offset, len)) = contiguous_region_len(&buf, i, 0xFE00, 0xFE9F) {
                let start = start_offset as usize;
                let end = start + len;
                link.write_oam_range(start_offset, &self.memory.oam()[start..end]);
                i += len;
                continue;
            }
            self.handle_bus_event(buf[i].address, buf[i].value, link);
            i += 1;
        }
        buf.clear();
        self.bus_event_buf = buf;
        event_count
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn handle_bus_event(&mut self, addr: u16, value: u8, link: &mut impl WorkerLink) {
        match addr {
            a if a == IF_ADDR => {}
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
            a if (0x8000..=0x9FFF).contains(&a) => {
                let offset = (a - 0x8000) as usize;
                link.write_vram_range(a - 0x8000, &self.memory.vram()[offset..offset + 1]);
            }
            a if (0xFE00..=0xFE9F).contains(&a) => {
                let offset = (a - 0xFE00) as usize;
                link.write_oam_range(a - 0xFE00, &self.memory.oam()[offset..offset + 1]);
            }
            a if a == LY_ADDR => {
                self.memory.write_io(LY_ADDR, 0);
                link.write_ppu_register(LY_ADDR, 0);
            }
            a if a == DMA_ADDR => {
                self.dma = Some(DmaState {
                    source: (value as u16) << 8,
                    progress: 0,
                });
            }
            a if is_ppu_mirror_register(a) => {
                link.write_ppu_register(a, value);
            }
            a if (NR10_ADDR..=NR52_ADDR).contains(&a) => self.write_apu_register(a, value, link),
            a if (0xFF27u16..WAVE_RAM_START).contains(&a) => self.memory.write_io(a, 0xFF),
            a if (WAVE_RAM_START..=WAVE_RAM_END).contains(&a) => {
                self.write_wave_ram(a, value, link)
            }
            a if a == TIMA_ADDR => self.timer.set_tima(value),
            a if a == TMA_ADDR => self.timer.set_tma(value),
            a if a == TAC_ADDR => self.timer.set_tac(value),
            _ => {}
        }
    }

    fn write_apu_register(&mut self, addr: u16, value: u8, link: &mut impl WorkerLink) {
        self.apu.write_register(addr, value);
        self.sync_apu_register_mirror(addr);
        link.send(WorkerCommand::WriteApuRegister { addr, value });
    }

    fn write_wave_ram(&mut self, addr: u16, value: u8, link: &mut impl WorkerLink) {
        let offset = (addr - WAVE_RAM_START) as u8;
        self.apu.write_wave_ram(offset, value);
        self.memory.write_io(
            WAVE_RAM_START + offset as u16,
            self.apu.read_wave_ram(offset),
        );
        link.send(WorkerCommand::WriteWaveRam { offset, value });
    }

    fn sync_apu_register_mirror(&mut self, addr: u16) {
        if addr == NR52_ADDR {
            for register in NR10_ADDR..=NR52_ADDR {
                self.memory
                    .write_io(register, self.apu.read_register(register));
            }
            return;
        }

        self.memory.write_io(addr, self.apu.read_register(addr));
        self.memory
            .write_io(NR52_ADDR, self.apu.read_register(NR52_ADDR));
    }

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

    /// Returns the last fully-rendered frame.
    #[inline(always)]
    pub fn front_buffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        &self.front_buffer
    }

    /// Returns all bytes captured by the serial port (SB transfers via SC).
    pub fn serial_output(&self) -> &[u8] {
        self.serial.output()
    }

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

    pub fn sync_ppu_worker(&self, link: &mut impl WorkerLink) {
        link.sync_ppu_state(
            self.memory.io_slice(),
            self.memory.vram(),
            self.memory.oam(),
        );
    }

    pub fn sync_apu_worker(&self, link: &mut impl WorkerLink) {
        link.sync_apu_state(self.memory.io_slice());
    }

    pub fn sync_worker_state(&self, link: &mut impl WorkerLink) {
        self.sync_apu_worker(link);
        self.sync_ppu_worker(link);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn sync_worker_frontend_state(&mut self, link: &mut impl WorkerLink) {
        #[cfg(feature = "perf")]
        let sync_start = cyccnt();
        let state = link.poll_frontend_state(&mut self.front_buffer);
        self.memory.write_io(NR52_ADDR, state.apu_nr52);
        self.memory.write_io(LY_ADDR, state.ppu_ly);
        self.memory.write_io(STAT_ADDR, state.ppu_stat);
        if state.if_bits != 0 {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_ | state.if_bits);
        }
        #[cfg(feature = "perf")]
        {
            self.perf_profile.ppu_sync = self
                .perf_profile
                .ppu_sync
                .wrapping_add(cyccnt().wrapping_sub(sync_start));
            if state.frame_ready {
                self.perf_profile.render_scanlines =
                    self.perf_profile.render_scanlines.wrapping_add(144);
            }
        }
    }

    /// Press or release a joypad button. Fires the joypad interrupt if the
    /// button is newly pressed and its select line is active.
    pub fn set_button(&mut self, btn: Button, pressed: bool) {
        let interrupt = self.joypad.set_button(btn, pressed);
        self.memory.write_io(JOYP_ADDR, self.joypad.read());
        if interrupt {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory
                .write_io(IF_ADDR, if_ | (1 << JOYPAD_INTERRUPT_BIT));
        }
    }

    /// Total T-cycles elapsed since power-on.
    #[inline(always)]
    pub fn cycle_counter(&self) -> u64 {
        self.cycle_counter
    }

    /// Returns the currently mapped ROM bank for the switchable window (0x4000–0x7FFF).
    pub fn current_rom_bank(&self) -> usize {
        self.memory.current_rom_bank()
    }

    /// Serialize the full emulator state to an RBSS v1 blob.
    pub fn save_state(&self, link: &impl WorkerLink) -> Vec<u8> {
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
        let ppu_state = link.snapshot_ppu_state(self.memory.io_slice());
        SaveState::serialize(
            cpu_state,
            self.timer.to_save_state(),
            ppu_state,
            &self.memory,
        )
    }

    /// Restore emulator state from a parsed [`SaveState`].
    pub fn load_state(
        &mut self,
        state: SaveState,
        link: &mut impl WorkerLink,
    ) -> Result<(), &'static str> {
        self.cpu.registers = state.cpu.to_registers();
        self.cpu.ime = state.cpu.ime;
        self.cpu.halted = state.cpu.halted;
        self.cycle_counter = state.cpu.cycle_counter;
        self.timer.load_state(state.timer);
        self.memory.load_state(&state);
        self.apu.sync_from_io_snapshot(self.memory.io_slice());
        link.sync_apu_state(self.memory.io_slice());
        link.load_ppu_state(
            state.ppu,
            self.memory.io_slice(),
            self.memory.vram(),
            self.memory.oam(),
        );
        self.sync_worker_frontend_state(link);
        Ok(())
    }

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

    #[cfg(feature = "perf")]
    pub fn take_perf_profile(&mut self) -> crate::cpu::sm83::Sm83PerfProfile {
        self.cpu.perf.take_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_frontend_perf_profile(&mut self) -> FrontendPerfProfile {
        core::mem::take(&mut self.perf_profile)
    }

    #[cfg(feature = "perf")]
    pub fn take_cartridge_perf_profile(
        &mut self,
    ) -> crate::memory::cartridge::CartridgePerfProfile {
        self.memory.take_cartridge_perf_profile()
    }
}

fn seed_joypad_io(memory: &mut GameBoyMemory, joypad: &JoypadPeripheral) {
    memory.write_io(JOYP_ADDR, joypad.read());
}

fn seed_apu_io(memory: &mut GameBoyMemory, apu: &ApuPeripheral) {
    for addr in NR10_ADDR..=NR52_ADDR {
        memory.write_io(addr, apu.read_register(addr));
    }
    for addr in 0xFF27u16..WAVE_RAM_START {
        memory.write_io(addr, 0xFF);
    }
    for addr in WAVE_RAM_START..=WAVE_RAM_END {
        let offset = (addr - WAVE_RAM_START) as u8;
        memory.write_io(addr, apu.read_wave_ram(offset));
    }
}

fn is_ppu_mirror_register(address: u16) -> bool {
    matches!(
        address,
        0xFF40 | 0xFF41 | 0xFF42 | 0xFF43 | 0xFF45 | 0xFF47 | 0xFF48 | 0xFF49 | 0xFF4A | 0xFF4B
    )
}

fn contiguous_region_len(
    events: &[BusEvent],
    start_index: usize,
    range_start: u16,
    range_end: u16,
) -> Option<(u16, usize)> {
    let start = events.get(start_index)?.address;
    if !(range_start..=range_end).contains(&start) {
        return None;
    }

    let mut len = 1usize;
    let mut expected = start.wrapping_add(1);
    while let Some(event) = events.get(start_index + len) {
        if event.address != expected || !(range_start..=range_end).contains(&event.address) {
            break;
        }
        len += 1;
        expected = expected.wrapping_add(1);
    }

    Some((start - range_start, len))
}
