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
use crate::ipc::{LocalTransport, WorkerCommand, WorkerTransport};
use crate::memory::cartridge::Cartridge;
use crate::memory::map::{OAM_BASE, OAM_END, VRAM_BASE, VRAM_END};
use crate::memory::memory::{BusEvent, Error as MemoryError, GameBoyMemory, Memory as MemoryTrait};


const IF_ADDR: u16 = 0xFF0F;
const DMA_ADDR: u16 = 0xFF46;
const SB_ADDR: u16 = 0xFF01;
const SC_ADDR: u16 = 0xFF02;
const APU_UNUSED_START: u16 = 0xFF27;
const OAM_DMA_BYTES: u8 = 160;
const PPU_REG_BATCH_CAP: usize = 16;

pub(crate) struct DmaState {
    pub source: u16,
    pub progress: u8,
}


/// Game Boy emulator. `W` is the worker transport — `LocalTransport` runs PPU/APU
/// on the calling thread; platform-specific transports can offload to another core.
pub struct GameBoy<W: WorkerTransport = LocalTransport> {
    cpu: Sm83,
    memory: Box<GameBoyMemory>,
    apu: ApuPeripheral,
    timer: TimerPeripheral,
    joypad: JoypadPeripheral,
    serial: SerialPort,
    dma: Option<DmaState>,
    front_buffer: Box<[u8; FRAMEBUFFER_SIZE]>,
    bus_event_buf: Vec<BusEvent>,
    cycle_counter: u64,
    transport: W,
}

// --- constructors for the default LocalTransport ---

impl GameBoy<LocalTransport> {
    pub fn new(rom: Vec<u8>) -> Self {
        let memory = Box::new(GameBoyMemory::with_rom(rom));
        Self::init_with_transport(memory, LocalTransport::new())
    }

    pub fn with_cartridge(cart: Box<dyn Cartridge>) -> Self {
        let memory = Box::new(GameBoyMemory::with_cartridge(cart));
        Self::init_with_transport(memory, LocalTransport::new())
    }

    pub fn for_test(rom_data: Vec<u8>) -> Self {
        let memory = Box::new(GameBoyMemory::with_rom(rom_data));
        let mut gb = Self::with_transport(memory, LocalTransport::new());
        gb.push_worker_state();
        gb
    }

    pub fn for_test_with_setup(setup: impl FnOnce(&mut GameBoyMemory), rom_data: Vec<u8>) -> Self {
        let mut mem = GameBoyMemory::with_rom(rom_data);
        setup(&mut mem);
        let mut gb = Self::with_transport(Box::new(mem), LocalTransport::new());
        gb.push_worker_state();
        gb
    }

    fn init_with_transport(memory: Box<GameBoyMemory>, transport: LocalTransport) -> Self {
        let mut gb = Self::with_transport(memory, transport);
        gb.push_worker_state();
        gb = gb.with_registers(Registers {
            a: 0x01,
            f: crate::cpu::registers::Flags::from_bits_truncate(0xB0),
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            pc: 0x0100,
            sp: 0xFFFE,
        });
        gb.apply_dmg_state();
        gb
    }
}

// --- generic impl for any transport ---

impl<W: WorkerTransport> GameBoy<W> {
    /// Construct with a custom transport. Used by platform-specific code (e.g. Pico multicore).
    pub fn with_transport(memory: Box<GameBoyMemory>, transport: W) -> Self {
        let cpu = Sm83::new_pure();
        let joypad = JoypadPeripheral::new();
        let apu = ApuPeripheral::new();

        let mut memory = memory;
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
            front_buffer: unsafe { Box::<[u8; FRAMEBUFFER_SIZE]>::new_zeroed().assume_init() },
            bus_event_buf: Vec::with_capacity(4),
            cycle_counter: 0,
            transport,
        }
    }

    pub fn with_registers(mut self, registers: Registers) -> Self {
        self.cpu.registers = registers;
        self
    }

    pub fn with_dmg_state(mut self) -> Self {
        self.apply_dmg_state();
        self
    }

    /// Seed IO registers to DMG post-boot-ROM state and sync the worker.
    pub fn apply_dmg_state(&mut self) {
        self.memory.write_io(LCDC_ADDR, 0x91);
        self.memory.write_io(STAT_ADDR, 0x85);
        self.memory.write_io(BGP_ADDR, 0xFC);
        self.memory.write_io(OBP0_ADDR, 0xFF);
        self.memory.write_io(OBP1_ADDR, 0xFF);
        self.write_apu_register(0xFF26, 0xF1);
        self.write_apu_register(0xFF25, 0xF3);
        self.write_apu_register(0xFF24, 0x77);
        let io = self.memory.io_slice();
        let vram = self.memory.vram();
        let oam = self.memory.oam();
        self.transport.sync_ppu_state(io, vram, oam);
    }

    /// Push full APU + PPU state to the worker. Called at init and after state loads.
    pub fn push_worker_state(&mut self) {
        let io = self.memory.io_slice();
        let vram = self.memory.vram();
        let oam = self.memory.oam();
        self.transport.sync_apu_state(io);
        self.transport.sync_ppu_state(io, vram, oam);
    }

    /// Execute one complete SM83 instruction.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    pub fn tick(&mut self) {

        let t_cycles = self.cpu.step(&mut self.memory) as u16;

        self.route_bus_events();

        self.advance_peripherals(t_cycles);
        self.read_worker_output();
        self.cycle_counter = self.cycle_counter.wrapping_add(t_cycles as u64);

    }

    /// Execute one complete SM83 instruction, returning the T-cycles elapsed.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn step(&mut self) -> Result<u8, CpuError> {
        let before = self.cycle_counter;
        self.tick();
        Ok((self.cycle_counter.wrapping_sub(before)) as u8)
    }

    pub fn registers(&self) -> Registers {
        self.cpu.registers.clone()
    }

    pub fn ime(&self) -> bool {
        self.cpu.ime()
    }

    pub fn is_halted(&self) -> bool {
        self.cpu.is_halted()
    }

    #[inline(always)]
    pub fn front_buffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        &self.front_buffer
    }

    pub fn set_button(&mut self, btn: Button, pressed: bool) {
        let interrupt = self.joypad.set_button(btn, pressed);
        self.memory.write_io(JOYP_ADDR, self.joypad.read());
        if interrupt {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory
                .write_io(IF_ADDR, if_ | (1 << JOYPAD_INTERRUPT_BIT));
        }
    }

    #[inline(always)]
    pub fn cycle_counter(&self) -> u64 {
        self.cycle_counter
    }

    pub fn current_rom_bank(&self) -> usize {
        self.memory.current_rom_bank()
    }

    pub fn serial_output(&self) -> &[u8] {
        self.serial.output()
    }

    pub fn read_memory(&self, address: u16) -> Result<u8, MemoryError> {
        MemoryTrait::read(self.memory.as_ref(), address)
    }

    pub fn write_io(&mut self, address: u16, value: u8) {
        self.memory.write_io(address, value);
    }

    pub fn read_io(&self, address: u16) -> u8 {
        self.memory.read_io(address)
    }

    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.transport.drain_audio_samples()
    }

    pub fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>) {
        self.transport.drain_audio_samples_into_i16(out);
    }

    pub fn external_ram(&self) -> Option<&[u8]> {
        self.memory.external_ram()
    }

    pub fn set_external_ram(&mut self, data: &[u8]) {
        self.memory.set_external_ram(data);
    }

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
        let ppu_state = self.transport.snapshot_ppu_state(self.memory.io_slice());
        SaveState::serialize(
            cpu_state,
            self.timer.to_save_state(),
            ppu_state,
            &self.memory,
        )
    }

    pub fn load_state(&mut self, state: SaveState) -> Result<(), &'static str> {
        self.cpu.registers = state.cpu.to_registers();
        self.cpu.ime = state.cpu.ime;
        self.cpu.halted = state.cpu.halted;
        self.cycle_counter = state.cpu.cycle_counter;
        self.timer.load_state(state.timer);
        self.memory.load_state(&state);
        self.apu.sync_from_io_snapshot(self.memory.io_slice());
        self.transport.sync_apu_state(self.memory.io_slice());
        self.transport.load_ppu_state(
            state.ppu,
            self.memory.io_slice(),
            self.memory.vram(),
            self.memory.oam(),
        );
        self.read_worker_output();
        Ok(())
    }

    /// Borrow the transport for platform-specific operations.
    pub fn transport(&self) -> &W {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut W {
        &mut self.transport
    }






    // --- hot path internals ---

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_peripherals(&mut self, cycles: u16) {
        self.transport.send(WorkerCommand::AdvancePpu { cycles });

        self.advance_timer(cycles);

        self.transport.send(WorkerCommand::AdvanceApu {
            cycles,
            div_counter: self.timer.internal_counter(),
        });

        if self.memory.has_rtc() {
            self.memory.tick_rtc(cycles as u32);
        }

        if !self.serial.is_idle() {
            self.advance_serial(cycles);
        }

        self.advance_dma_bulk(cycles);
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
    fn advance_dma_bulk(&mut self, cycles: u16) {
        let Some(DmaState { source, progress }) = self.dma else {
            return;
        };
        let steps = (cycles / 4) as u8;
        let to_copy = steps.min(OAM_DMA_BYTES - progress);
        self.memory.copy_dma_step(source, progress, to_copy);
        let next = progress + to_copy;
        if next >= OAM_DMA_BYTES {
            self.dma = None;
            self.transport.write_oam_range(0, &self.memory.oam()[..OAM_DMA_BYTES as usize]);
        } else {
            self.dma = Some(DmaState { source, progress: next });
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn route_bus_events(&mut self) -> usize {
        if !self.memory.has_events() {
            return 0;
        }
        let mut buf = core::mem::take(&mut self.bus_event_buf);
        self.memory.drain_into(&mut buf);
        let event_count = buf.len();
        let mut ppu_reg_buf = [(0u16, 0u8); PPU_REG_BATCH_CAP];
        let mut ppu_reg_count = 0usize;
        let mut i = 0usize;
        while i < buf.len() {
            if let Some((start_offset, len)) = contiguous_region_len(&buf, i, VRAM_BASE, VRAM_END) {
                let start = start_offset as usize;
                let end = start + len;
                self.transport
                    .write_vram_range(start_offset, &self.memory.vram()[start..end]);
                i += len;
                continue;
            }
            if let Some((start_offset, len)) = contiguous_region_len(&buf, i, OAM_BASE, OAM_END) {
                let start = start_offset as usize;
                let end = start + len;
                self.transport
                    .write_oam_range(start_offset, &self.memory.oam()[start..end]);
                i += len;
                continue;
            }
            if let Some(reg) = self.handle_bus_event(buf[i].address, buf[i].value) {
                if ppu_reg_count < ppu_reg_buf.len() {
                    ppu_reg_buf[ppu_reg_count] = reg;
                    ppu_reg_count += 1;
                }
            }
            i += 1;
        }
        if ppu_reg_count > 0 {
            self.transport.write_ppu_registers(&ppu_reg_buf[..ppu_reg_count]);
        }
        buf.clear();
        self.bus_event_buf = buf;
        event_count
    }

    /// Handle a single bus event. Returns `Some((addr, value))` if the event
    /// is a PPU register write that needs to be forwarded to the worker.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn handle_bus_event(&mut self, addr: u16, value: u8) -> Option<(u16, u8)> {
        match addr {
            a if a == IF_ADDR => {}
            a if a == JOYP_ADDR => {
                self.joypad.write(value);
                self.memory.write_io(JOYP_ADDR, self.joypad.read());
            }
            a if a == SB_ADDR => self.serial.set_sb(value),
            a if a == SC_ADDR => self.serial.handle_sc_write(value),
            a if a == DIV_ADDR => self.timer.reset_div(),
            a if (VRAM_BASE..=VRAM_END).contains(&a) => {
                let offset = (a - VRAM_BASE) as usize;
                self.transport
                    .write_vram_range(a - VRAM_BASE, &self.memory.vram()[offset..offset + 1]);
            }
            a if (OAM_BASE..=OAM_END).contains(&a) => {
                let offset = (a - OAM_BASE) as usize;
                self.transport
                    .write_oam_range(a - OAM_BASE, &self.memory.oam()[offset..offset + 1]);
            }
            a if a == LY_ADDR => {
                self.memory.write_io(LY_ADDR, 0);
                return Some((LY_ADDR, 0));
            }
            a if a == DMA_ADDR => {
                self.dma = Some(DmaState {
                    source: (value as u16) << 8,
                    progress: 0,
                });
            }
            a if is_ppu_mirror_register(a) => return Some((a, value)),
            a if (NR10_ADDR..=NR52_ADDR).contains(&a) => self.write_apu_register(a, value),
            a if (APU_UNUSED_START..WAVE_RAM_START).contains(&a) => self.memory.write_io(a, 0xFF),
            a if (WAVE_RAM_START..=WAVE_RAM_END).contains(&a) => self.write_wave_ram(a, value),
            a if a == TIMA_ADDR => self.timer.set_tima(value),
            a if a == TMA_ADDR => self.timer.set_tma(value),
            a if a == TAC_ADDR => self.timer.set_tac(value),
            _ => {}
        }
        None
    }

    fn write_apu_register(&mut self, addr: u16, value: u8) {
        self.apu.write_register(addr, value);
        self.sync_apu_register_mirror(addr);
        self.transport
            .send(WorkerCommand::WriteApuRegister { addr, value });
    }

    fn write_wave_ram(&mut self, addr: u16, value: u8) {
        let offset = (addr - WAVE_RAM_START) as u8;
        self.apu.write_wave_ram(offset, value);
        self.transport
            .send(WorkerCommand::WriteWaveRam { offset, value });
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

    /// Poll the worker for its output and write it back into CPU-visible memory.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn read_worker_output(&mut self) {
        let output = self.transport.poll_output(&mut self.front_buffer);
        self.memory.write_io(NR52_ADDR, output.apu_nr52);
        self.memory.write_io(LY_ADDR, output.ppu_ly);
        self.memory.write_io(STAT_ADDR, output.ppu_stat);
        if output.if_bits != 0 {
            let if_ = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_ | output.if_bits);
        }
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
