#![cfg(target_arch = "arm")]

use alloc::{boxed::Box, vec::Vec};
use core::cell::RefCell;
use core::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize, Ordering};

use cortex_m::asm;
use critical_section::Mutex;
use defmt::info;
use embassy_rp::multicore::{self, Stack};
use embassy_rp::peripherals::CORE1;
use embassy_rp::Peri;
use heapless::mpmc::MpMcQueue;
use rustyboy_core::cpu::cpu::CpuError;
#[cfg(feature = "perf")]
use rustyboy_core::cpu::peripheral::apu::ApuPerfProfile;
use rustyboy_core::cpu::peripheral::apu::ApuPeripheral;
use rustyboy_core::cpu::peripheral::joypad::Button;
#[cfg(feature = "perf")]
use rustyboy_core::cpu::peripheral::ppu::PpuPerfProfile;
use rustyboy_core::cpu::peripheral::ppu::{PpuPeripheral, FRAMEBUFFER_SIZE};
use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::save_state::{PpuState, SaveState};
#[cfg(feature = "perf")]
use rustyboy_core::cpu::sm83::Sm83PerfProfile;
#[cfg(feature = "perf")]
use rustyboy_core::gameboy::FrontendPerfProfile;
use rustyboy_core::gameboy::{GameBoyFrontend, WorkerCommand, WorkerFrontendState, WorkerLink};
use rustyboy_core::memory::cartridge::Cartridge;
#[cfg(feature = "perf")]
use rustyboy_core::memory::cartridge::CartridgePerfProfile;
use rustyboy_core::memory::memory::{Error as MemoryError, GameBoyMemory};

const CORE1_STACK_SIZE: usize = 8192;
const AUDIO_QUEUE_CAPACITY: usize = 4096;
const APU_ADVANCE_BATCH_CYCLES: u16 = 256;
const PPU_ADVANCE_BATCH_CYCLES: u16 = 456;
const NR52_ADDR: u16 = 0xFF26;
const LY_ADDR: u16 = 0xFF44;
const STAT_ADDR: u16 = 0xFF41;
const VBLANK_INTERRUPT_BIT: u8 = 0;
const STAT_INTERRUPT_BIT: u8 = 1;
const PPU_IO_LEN: usize = 0x80;
const PPU_VRAM_LEN: usize = 0x2000;
const PPU_OAM_LEN: usize = 0xA0;
const LY_IO_OFFSET: usize = (LY_ADDR - 0xFF00) as usize;
const STAT_IO_OFFSET: usize = (STAT_ADDR - 0xFF00) as usize;

static AUDIO_QUEUE: MpMcQueue<i16, AUDIO_QUEUE_CAPACITY> = MpMcQueue::new();
static SHARED_RUNTIME: SharedRuntime = SharedRuntime::new();

#[derive(Clone, Copy, Default)]
pub struct TransportProfile {
    pub command_enqueues: u32,
    pub command_queue_spins: u32,
    pub apu_commands: u32,
    pub ppu_advance_commands: u32,
    pub frame_publishes: u32,
    pub ppu_vram_bytes: u32,
    pub ppu_oam_bytes: u32,
    pub ppu_register_writes: u32,
    pub audio_queue_drops: u32,
}

struct SharedApuLive {
    apu: ApuPeripheral,
}

impl SharedApuLive {
    fn new() -> Self {
        Self {
            apu: ApuPeripheral::new(),
        }
    }

    fn read_nr52(&self) -> u8 {
        self.apu.read_register(NR52_ADDR)
    }

    fn write_register(&mut self, addr: u16, value: u8) -> u8 {
        self.apu.write_register(addr, value);
        self.read_nr52()
    }

    fn write_wave_ram(&mut self, offset: u8, value: u8) {
        self.apu.write_wave_ram(offset, value);
    }

    fn sync_from_io_snapshot(&mut self, io: &[u8]) -> u8 {
        self.apu.sync_from_io_snapshot(io);
        self.read_nr52()
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn advance(&mut self, cycles: u16, div_counter: u16, out: &mut Vec<i16>) -> u8 {
        let output = self.apu.tick(cycles, div_counter);
        self.apu.drain_samples_into(out);
        output.nr52
    }

    #[cfg(feature = "perf")]
    fn take_perf_profile(&mut self) -> ApuPerfProfile {
        self.apu.take_perf_profile()
    }
}

#[derive(Clone, Copy)]
struct SharedPpuAdvanceOutput {
    ly: u8,
    stat: u8,
    if_bits: u8,
    frame_ready: bool,
}

struct SharedPpuCore {
    ppu: PpuPeripheral,
}

impl SharedPpuCore {
    fn new() -> Self {
        Self {
            ppu: PpuPeripheral::new(),
        }
    }

    fn sync_state(&mut self, io: &[u8]) -> u8 {
        self.ppu.clear_framebuffer();
        self.ppu.sync_prev_stat_line(io);
        self.ppu.ly()
    }

    fn load_state(&mut self, state: PpuState, io: &[u8]) -> u8 {
        self.ppu.load_state(state);
        self.ppu.sync_prev_stat_line(io);
        self.ppu.ly()
    }

    fn snapshot(&self, io: &[u8]) -> PpuState {
        self.ppu.to_save_state(io)
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn advance(
        &mut self,
        cycles: u16,
        io: &mut [u8; PPU_IO_LEN],
        vram: &[u8; PPU_VRAM_LEN],
        oam: &[u8; PPU_OAM_LEN],
    ) -> SharedPpuAdvanceOutput {
        let output = self.ppu.tick(cycles, io, vram, oam);
        let mut if_bits = 0u8;
        if output.vblank_interrupt {
            if_bits |= 1 << VBLANK_INTERRUPT_BIT;
        }
        if output.stat_interrupt {
            if_bits |= 1 << STAT_INTERRUPT_BIT;
        }
        SharedPpuAdvanceOutput {
            ly: io[LY_IO_OFFSET],
            stat: io[STAT_IO_OFFSET],
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

struct SharedRuntime {
    apu: Mutex<RefCell<Option<SharedApuLive>>>,
    ppu: Mutex<RefCell<Option<SharedPpuCore>>>,
    ppu_io: Mutex<RefCell<[u8; PPU_IO_LEN]>>,
    ppu_vram: Mutex<RefCell<[u8; PPU_VRAM_LEN]>>,
    ppu_oam: Mutex<RefCell<[u8; PPU_OAM_LEN]>>,
    frame_slots: [Mutex<RefCell<[u8; FRAMEBUFFER_SIZE]>>; 2],
    published_frame: AtomicUsize,
    published_frame_seq: AtomicU32,
    pending_apu_cycles: AtomicU32,
    pending_apu_div_counter: AtomicU32,
    pending_ppu_cycles: AtomicU32,
    audio_queue_drops: AtomicU32,
    apu_nr52: AtomicU8,
    ppu_ly: AtomicU8,
    ppu_stat: AtomicU8,
    pending_if_bits: AtomicU8,
}

impl SharedRuntime {
    const fn new() -> Self {
        Self {
            apu: Mutex::new(RefCell::new(None)),
            ppu: Mutex::new(RefCell::new(None)),
            ppu_io: Mutex::new(RefCell::new([0; PPU_IO_LEN])),
            ppu_vram: Mutex::new(RefCell::new([0; PPU_VRAM_LEN])),
            ppu_oam: Mutex::new(RefCell::new([0; PPU_OAM_LEN])),
            frame_slots: [
                Mutex::new(RefCell::new([0; FRAMEBUFFER_SIZE])),
                Mutex::new(RefCell::new([0; FRAMEBUFFER_SIZE])),
            ],
            published_frame: AtomicUsize::new(0),
            published_frame_seq: AtomicU32::new(0),
            pending_apu_cycles: AtomicU32::new(0),
            pending_apu_div_counter: AtomicU32::new(0),
            pending_ppu_cycles: AtomicU32::new(0),
            audio_queue_drops: AtomicU32::new(0),
            apu_nr52: AtomicU8::new(0),
            ppu_ly: AtomicU8::new(0),
            ppu_stat: AtomicU8::new(0),
            pending_if_bits: AtomicU8::new(0),
        }
    }

    fn copy_published_frame(&self, out: &mut [u8; FRAMEBUFFER_SIZE]) {
        let slot = self.published_frame.load(Ordering::Acquire) & 1;
        critical_section::with(|cs| {
            let frame = self.frame_slots[slot].borrow(cs).borrow();
            out.copy_from_slice(&*frame);
        });
    }

    fn clear_published_frames(&self) {
        critical_section::with(|cs| {
            self.frame_slots[0].borrow(cs).borrow_mut().fill(0);
            self.frame_slots[1].borrow(cs).borrow_mut().fill(0);
        });
        self.published_frame.store(0, Ordering::Release);
        self.published_frame_seq.store(0, Ordering::Release);
    }
}

fn take_pending_cycles(counter: &AtomicU32, max_batch: u16) -> u16 {
    loop {
        let pending = counter.load(Ordering::Acquire);
        if pending == 0 {
            return 0;
        }
        let take = pending.min(max_batch as u32);
        if counter
            .compare_exchange_weak(pending, pending - take, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return take as u16;
        }
    }
}

struct Core1WorkerLink {
    audio_rx: &'static MpMcQueue<i16, AUDIO_QUEUE_CAPACITY>,
    shared: &'static SharedRuntime,
    last_frame_seq: u32,
    last_profile_frame_seq: u32,
    transport_profile: TransportProfile,
}

impl Core1WorkerLink {
    fn new(core1: Peri<'static, CORE1>) -> Self {
        let shared = &SHARED_RUNTIME;
        let audio_rx = &AUDIO_QUEUE;
        let core1_stack = Box::leak(Box::new(Stack::<CORE1_STACK_SIZE>::new()));
        let audio_scratch = Box::leak(Box::new(Vec::with_capacity(2048)));
        let ppu_io_scratch = Box::leak(Box::new([0u8; PPU_IO_LEN]));
        let ppu_vram_scratch = Box::leak(Box::new([0u8; PPU_VRAM_LEN]));
        let ppu_oam_scratch = Box::leak(Box::new([0u8; PPU_OAM_LEN]));

        while audio_rx.dequeue().is_some() {}
        critical_section::with(|cs| {
            *shared.apu.borrow(cs).borrow_mut() = Some(SharedApuLive::new());
            *shared.ppu.borrow(cs).borrow_mut() = Some(SharedPpuCore::new());
        });
        shared.pending_apu_cycles.store(0, Ordering::Release);
        shared.pending_apu_div_counter.store(0, Ordering::Release);
        shared.pending_ppu_cycles.store(0, Ordering::Release);
        shared.audio_queue_drops.store(0, Ordering::Release);
        shared.apu_nr52.store(0, Ordering::Release);
        shared.ppu_ly.store(0, Ordering::Release);
        shared.ppu_stat.store(0, Ordering::Release);
        shared.pending_if_bits.store(0, Ordering::Release);
        shared.clear_published_frames();

        info!("spawning core1 worker");
        let shared_for_core1 = shared;
        multicore::spawn_core1(core1, core1_stack, move || {
            run_core1_worker(
                audio_rx,
                shared_for_core1,
                audio_scratch,
                ppu_io_scratch,
                ppu_vram_scratch,
                ppu_oam_scratch,
            )
        });
        info!("core1 worker spawned");

        Self {
            audio_rx,
            shared,
            last_frame_seq: 0,
            last_profile_frame_seq: 0,
            transport_profile: TransportProfile::default(),
        }
    }

    fn take_transport_profile(&mut self) -> TransportProfile {
        let mut profile = core::mem::take(&mut self.transport_profile);
        let frame_seq = self.shared.published_frame_seq.load(Ordering::Acquire);
        profile.frame_publishes = frame_seq.wrapping_sub(self.last_profile_frame_seq);
        self.last_profile_frame_seq = frame_seq;
        profile.audio_queue_drops = self.shared.audio_queue_drops.swap(0, Ordering::AcqRel);
        profile
    }
}

impl WorkerLink for Core1WorkerLink {
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn send(&mut self, command: WorkerCommand) {
        match command {
            WorkerCommand::AdvanceApu {
                cycles,
                div_counter,
            } => {
                self.shared
                    .pending_apu_div_counter
                    .store(div_counter as u32, Ordering::Release);
                let prev_pending = self
                    .shared
                    .pending_apu_cycles
                    .fetch_add(cycles as u32, Ordering::AcqRel);
                self.transport_profile.apu_commands =
                    self.transport_profile.apu_commands.wrapping_add(1);
                if prev_pending == 0 {
                    asm::sev();
                }
            }
            WorkerCommand::AdvancePpu { cycles } => {
                let prev_pending = self
                    .shared
                    .pending_ppu_cycles
                    .fetch_add(cycles as u32, Ordering::AcqRel);
                self.transport_profile.ppu_advance_commands =
                    self.transport_profile.ppu_advance_commands.wrapping_add(1);
                if prev_pending == 0 {
                    asm::sev();
                }
            }
            WorkerCommand::WriteApuRegister { addr, value } => {
                critical_section::with(|cs| {
                    let nr52 = self
                        .shared
                        .apu
                        .borrow(cs)
                        .borrow_mut()
                        .as_mut()
                        .unwrap()
                        .write_register(addr, value);
                    self.shared.apu_nr52.store(nr52, Ordering::Release);
                });
                self.transport_profile.apu_commands =
                    self.transport_profile.apu_commands.wrapping_add(1);
            }
            WorkerCommand::WriteWaveRam { offset, value } => {
                critical_section::with(|cs| {
                    self.shared
                        .apu
                        .borrow(cs)
                        .borrow_mut()
                        .as_mut()
                        .unwrap()
                        .write_wave_ram(offset, value);
                });
                self.transport_profile.apu_commands =
                    self.transport_profile.apu_commands.wrapping_add(1);
            }
            WorkerCommand::WriteVram { offset, value } => self.write_vram_range(offset, &[value]),
            WorkerCommand::WriteOam { offset, value } => self.write_oam_range(offset, &[value]),
            WorkerCommand::WritePpuRegister { addr, value } => self.write_ppu_register(addr, value),
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_vram_range(&mut self, start_offset: u16, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let start = start_offset as usize;
        let len = data.len().min(PPU_VRAM_LEN.saturating_sub(start));
        critical_section::with(|cs| {
            let mut vram = self.shared.ppu_vram.borrow(cs).borrow_mut();
            vram[start..start + len].copy_from_slice(&data[..len]);
        });
        self.transport_profile.ppu_vram_bytes = self
            .transport_profile
            .ppu_vram_bytes
            .wrapping_add(len as u32);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        let start = start_offset as usize;
        let len = data.len().min(PPU_OAM_LEN.saturating_sub(start));
        critical_section::with(|cs| {
            let mut oam = self.shared.ppu_oam.borrow(cs).borrow_mut();
            oam[start..start + len].copy_from_slice(&data[..len]);
        });
        self.transport_profile.ppu_oam_bytes = self
            .transport_profile
            .ppu_oam_bytes
            .wrapping_add(len as u32);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_ppu_register(&mut self, addr: u16, value: u8) {
        if !(0xFF00..=0xFF7F).contains(&addr) {
            return;
        }
        critical_section::with(|cs| {
            if addr == LY_ADDR {
                self.shared
                    .ppu
                    .borrow(cs)
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .ppu
                    .reset_ly();
                self.shared.ppu_io.borrow(cs).borrow_mut()[LY_IO_OFFSET] = 0;
            } else {
                self.shared.ppu_io.borrow(cs).borrow_mut()[(addr - 0xFF00) as usize] = value;
            }
        });
        if addr == LY_ADDR {
            self.shared.ppu_ly.store(0, Ordering::Release);
        } else if addr == STAT_ADDR {
            self.shared.ppu_stat.store(value, Ordering::Release);
        }
        self.transport_profile.ppu_register_writes =
            self.transport_profile.ppu_register_writes.wrapping_add(1);
    }

    fn drain_audio_samples(&mut self) -> Vec<f32> {
        let mut raw = Vec::new();
        self.drain_audio_samples_into_i16(&mut raw);
        let mut out = Vec::with_capacity(raw.len());
        for sample in raw {
            out.push(sample as f32 / 32767.0);
        }
        out
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>) {
        out.clear();
        while let Some(sample) = self.audio_rx.dequeue() {
            out.push(sample);
        }
    }

    fn sync_apu_state(&mut self, io: &[u8]) {
        self.shared.pending_apu_cycles.store(0, Ordering::Release);
        critical_section::with(|cs| {
            let nr52 = self
                .shared
                .apu
                .borrow(cs)
                .borrow_mut()
                .as_mut()
                .unwrap()
                .sync_from_io_snapshot(io);
            self.shared.apu_nr52.store(nr52, Ordering::Release);
        });
    }

    fn sync_ppu_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.shared.pending_ppu_cycles.store(0, Ordering::Release);
        critical_section::with(|cs| {
            self.shared
                .ppu_io
                .borrow(cs)
                .borrow_mut()
                .copy_from_slice(&io[..PPU_IO_LEN]);
            self.shared
                .ppu_vram
                .borrow(cs)
                .borrow_mut()
                .copy_from_slice(&vram[..PPU_VRAM_LEN]);
            self.shared
                .ppu_oam
                .borrow(cs)
                .borrow_mut()
                .copy_from_slice(&oam[..PPU_OAM_LEN]);
            let mut ppu = self.shared.ppu.borrow(cs).borrow_mut();
            let ppu = ppu.as_mut().unwrap();
            let ly = ppu.sync_state(io);
            self.shared.ppu_io.borrow(cs).borrow_mut()[LY_IO_OFFSET] = ly;
            self.shared.ppu_ly.store(ly, Ordering::Release);
            self.shared
                .ppu_stat
                .store(io[STAT_IO_OFFSET], Ordering::Release);
        });
        self.shared.pending_if_bits.store(0, Ordering::Release);
        self.shared.clear_published_frames();
        self.last_frame_seq = 0;
        self.last_profile_frame_seq = 0;
    }

    fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.shared.pending_ppu_cycles.store(0, Ordering::Release);
        critical_section::with(|cs| {
            self.shared
                .ppu_io
                .borrow(cs)
                .borrow_mut()
                .copy_from_slice(&io[..PPU_IO_LEN]);
            self.shared
                .ppu_vram
                .borrow(cs)
                .borrow_mut()
                .copy_from_slice(&vram[..PPU_VRAM_LEN]);
            self.shared
                .ppu_oam
                .borrow(cs)
                .borrow_mut()
                .copy_from_slice(&oam[..PPU_OAM_LEN]);
            let mut ppu = self.shared.ppu.borrow(cs).borrow_mut();
            let ppu = ppu.as_mut().unwrap();
            let ly = ppu.load_state(state, io);
            let mut shared_io = self.shared.ppu_io.borrow(cs).borrow_mut();
            shared_io[LY_IO_OFFSET] = ly;
            shared_io[STAT_IO_OFFSET] = state.stat;
            self.shared.ppu_ly.store(ly, Ordering::Release);
            self.shared.ppu_stat.store(state.stat, Ordering::Release);
        });
        self.shared.pending_if_bits.store(0, Ordering::Release);
        self.shared.clear_published_frames();
        self.last_frame_seq = 0;
        self.last_profile_frame_seq = 0;
    }

    fn snapshot_ppu_state(&self, _io: &[u8]) -> PpuState {
        critical_section::with(|cs| {
            let io = self.shared.ppu_io.borrow(cs).borrow();
            self.shared
                .ppu
                .borrow(cs)
                .borrow()
                .as_ref()
                .unwrap()
                .snapshot(&*io)
        })
    }

    fn poll_frontend_state(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> WorkerFrontendState {
        let frame_seq = self.shared.published_frame_seq.load(Ordering::Acquire);
        let frame_ready = frame_seq != self.last_frame_seq;
        if frame_ready {
            self.shared.copy_published_frame(out);
            self.last_frame_seq = frame_seq;
        }
        WorkerFrontendState {
            apu_nr52: self.shared.apu_nr52.load(Ordering::Acquire),
            ppu_ly: self.shared.ppu_ly.load(Ordering::Acquire),
            ppu_stat: self.shared.ppu_stat.load(Ordering::Acquire),
            if_bits: self.shared.pending_if_bits.swap(0, Ordering::AcqRel),
            frame_ready,
        }
    }

    #[cfg(feature = "perf")]
    fn take_apu_perf_profile(&mut self) -> ApuPerfProfile {
        critical_section::with(|cs| {
            self.shared
                .apu
                .borrow(cs)
                .borrow_mut()
                .as_mut()
                .unwrap()
                .take_perf_profile()
        })
    }

    #[cfg(feature = "perf")]
    fn take_ppu_perf_profile(&mut self) -> PpuPerfProfile {
        critical_section::with(|cs| {
            self.shared
                .ppu
                .borrow(cs)
                .borrow_mut()
                .as_mut()
                .unwrap()
                .take_perf_profile()
        })
    }
}

pub struct PicoGameBoy {
    frontend: GameBoyFrontend,
    link: Core1WorkerLink,
}

impl PicoGameBoy {
    pub fn with_cartridge(core1: Peri<'static, CORE1>, cart: Box<dyn Cartridge>) -> Self {
        let memory = Box::new(GameBoyMemory::with_cartridge(cart));
        let frontend = GameBoyFrontend::from_memory(memory);
        let link = Core1WorkerLink::new(core1);
        let mut gb = Self { frontend, link };
        info!("syncing worker state");
        gb.frontend.sync_worker_state(&mut gb.link);
        info!("worker state synced");
        gb.frontend = gb.frontend.with_registers(Registers {
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
        info!("applying dmg state");
        gb.frontend.apply_dmg_state(&mut gb.link);
        info!("dmg state applied");
        gb
    }

    #[inline(always)]
    pub fn tick(&mut self) {
        self.frontend.tick(&mut self.link);
    }

    pub fn step(&mut self) -> Result<u8, CpuError> {
        self.frontend.step(&mut self.link)
    }

    #[inline(always)]
    pub fn front_buffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.frontend.front_buffer()
    }

    #[inline(always)]
    pub fn cycle_counter(&self) -> u64 {
        self.frontend.cycle_counter()
    }

    pub fn set_button(&mut self, btn: Button, pressed: bool) {
        self.frontend.set_button(btn, pressed);
    }

    pub fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>) {
        self.link.drain_audio_samples_into_i16(out);
    }

    pub fn read_memory(&self, address: u16) -> Result<u8, MemoryError> {
        self.frontend.read_memory(address)
    }

    pub fn save_state(&self) -> Vec<u8> {
        self.frontend.save_state(&self.link)
    }

    pub fn load_state(&mut self, state: SaveState) -> Result<(), &'static str> {
        self.frontend.load_state(state, &mut self.link)?;
        Ok(())
    }

    pub fn take_transport_profile(&mut self) -> TransportProfile {
        self.link.take_transport_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_perf_profile(&mut self) -> Sm83PerfProfile {
        self.frontend.take_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_frontend_perf_profile(&mut self) -> FrontendPerfProfile {
        self.frontend.take_frontend_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_ppu_perf_profile(&mut self) -> PpuPerfProfile {
        self.link.take_ppu_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_apu_perf_profile(&mut self) -> ApuPerfProfile {
        self.link.take_apu_perf_profile()
    }

    #[cfg(feature = "perf")]
    pub fn take_cartridge_perf_profile(&mut self) -> CartridgePerfProfile {
        self.frontend.take_cartridge_perf_profile()
    }
}

#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn run_core1_worker(
    audio_tx: &'static MpMcQueue<i16, AUDIO_QUEUE_CAPACITY>,
    shared: &'static SharedRuntime,
    mut audio_scratch: &'static mut Vec<i16>,
    ppu_io_scratch: &'static mut [u8; PPU_IO_LEN],
    ppu_vram_scratch: &'static mut [u8; PPU_VRAM_LEN],
    ppu_oam_scratch: &'static mut [u8; PPU_OAM_LEN],
) -> ! {
    info!("core1 worker loop start");
    let mut publish_slot = 1usize;

    loop {
        let mut did_work = false;

        let ppu_cycles = take_pending_cycles(&shared.pending_ppu_cycles, PPU_ADVANCE_BATCH_CYCLES);
        if ppu_cycles != 0 {
            let slot = publish_slot;
            critical_section::with(|cs| {
                let io = shared.ppu_io.borrow(cs).borrow();
                ppu_io_scratch.copy_from_slice(&*io);
            });
            critical_section::with(|cs| {
                let vram = shared.ppu_vram.borrow(cs).borrow();
                ppu_vram_scratch.copy_from_slice(&*vram);
            });
            critical_section::with(|cs| {
                let oam = shared.ppu_oam.borrow(cs).borrow();
                ppu_oam_scratch.copy_from_slice(&*oam);
            });
            let (output, frame_ready) = critical_section::with(|cs| {
                let mut ppu = shared.ppu.borrow(cs).borrow_mut();
                let ppu = ppu.as_mut().unwrap();
                let output = ppu.advance(
                    ppu_cycles,
                    ppu_io_scratch,
                    ppu_vram_scratch,
                    ppu_oam_scratch,
                );
                if output.frame_ready {
                    let mut frame = shared.frame_slots[slot].borrow(cs).borrow_mut();
                    frame.copy_from_slice(ppu.framebuffer());
                }
                (output, output.frame_ready)
            });
            critical_section::with(|cs| {
                let mut io = shared.ppu_io.borrow(cs).borrow_mut();
                io[LY_IO_OFFSET] = output.ly;
                io[STAT_IO_OFFSET] = output.stat;
            });

            shared.ppu_ly.store(output.ly, Ordering::Release);
            shared.ppu_stat.store(output.stat, Ordering::Release);
            if output.if_bits != 0 {
                shared
                    .pending_if_bits
                    .fetch_or(output.if_bits, Ordering::AcqRel);
            }
            if frame_ready {
                shared.published_frame.store(slot, Ordering::Release);
                shared.published_frame_seq.fetch_add(1, Ordering::AcqRel);
                publish_slot ^= 1;
            }
            did_work = true;
        }

        let apu_cycles = take_pending_cycles(&shared.pending_apu_cycles, APU_ADVANCE_BATCH_CYCLES);
        if apu_cycles != 0 {
            let div_counter = shared.pending_apu_div_counter.load(Ordering::Acquire) as u16;
            let nr52 = critical_section::with(|cs| {
                shared
                    .apu
                    .borrow(cs)
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .advance(apu_cycles, div_counter, &mut audio_scratch)
            });
            shared.apu_nr52.store(nr52, Ordering::Release);

            let mut dropped = 0u32;
            for sample in audio_scratch.drain(..) {
                if audio_tx.enqueue(sample).is_err() {
                    dropped = dropped.wrapping_add(1);
                }
            }
            if dropped != 0 {
                shared
                    .audio_queue_drops
                    .fetch_add(dropped, Ordering::AcqRel);
            }
            did_work = true;
        }

        if !did_work {
            asm::wfe();
        }
    }
}
