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
use rustyboy_core::cpu::peripheral::joypad::Button;
use rustyboy_core::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::save_state::{PpuState, SaveState};
use rustyboy_core::gameboy::{
    GameBoyFrontend, GameBoyWorker, WorkerCommand, WorkerFrontendState, WorkerLink,
};
use rustyboy_core::memory::cartridge::Cartridge;
use rustyboy_core::memory::memory::{Error as MemoryError, GameBoyMemory};

#[cfg(feature = "perf")]
use rustyboy_core::cpu::peripheral::apu::ApuPerfProfile;
#[cfg(feature = "perf")]
use rustyboy_core::cpu::peripheral::ppu::PpuPerfProfile;
#[cfg(feature = "perf")]
use rustyboy_core::cpu::sm83::Sm83PerfProfile;
#[cfg(feature = "perf")]
use rustyboy_core::gameboy::FrontendPerfProfile;
#[cfg(feature = "perf")]
use rustyboy_core::memory::cartridge::CartridgePerfProfile;

const CORE1_STACK_SIZE: usize = 8192;
const COMMAND_QUEUE_CAPACITY: usize = 1024;
const AUDIO_QUEUE_CAPACITY: usize = 4096;
const PPU_ADVANCE_BATCH_CYCLES: u16 = 912;

static SHARED_WORKER_STATE: SharedWorkerState = SharedWorkerState::new();
static COMMAND_QUEUE: MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY> = MpMcQueue::new();
static AUDIO_QUEUE: MpMcQueue<i16, AUDIO_QUEUE_CAPACITY> = MpMcQueue::new();

#[derive(Clone, Copy)]
enum Core1Command {
    Worker(WorkerCommand),
    SyncApu {
        ticket: u32,
    },
    SyncPpu {
        ticket: u32,
    },
    LoadPpuState {
        ticket: u32,
        state: PpuState,
    },
    #[cfg(feature = "perf")]
    TakeApuPerfProfile {
        ticket: u32,
    },
    #[cfg(feature = "perf")]
    TakePpuPerfProfile {
        ticket: u32,
    },
}

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

struct PpuSnapshot {
    io: [u8; 0x80],
    vram: [u8; 0x2000],
    oam: [u8; 0xA0],
}

impl PpuSnapshot {
    const fn new() -> Self {
        Self {
            io: [0; 0x80],
            vram: [0; 0x2000],
            oam: [0; 0xA0],
        }
    }
}

struct SharedWorkerState {
    sync_snapshot: Mutex<RefCell<PpuSnapshot>>,
    live_ppu_snapshot: Mutex<RefCell<PpuSnapshot>>,
    frame_slots: [Mutex<RefCell<[u8; FRAMEBUFFER_SIZE]>>; 2],
    published_frame: AtomicUsize,
    published_frame_seq: AtomicU32,
    sync_complete: AtomicU32,
    audio_queue_drops: AtomicU32,
    apu_nr52: AtomicU8,
    ppu_ly: AtomicU8,
    ppu_stat: AtomicU8,
    pending_if_bits: AtomicU8,
    ppu_render_version: AtomicU32,
    ppu_state: Mutex<RefCell<PpuState>>,
    #[cfg(feature = "perf")]
    apu_perf: Mutex<RefCell<ApuPerfProfile>>,
    #[cfg(feature = "perf")]
    ppu_perf: Mutex<RefCell<PpuPerfProfile>>,
}

impl SharedWorkerState {
    const fn new() -> Self {
        Self {
            sync_snapshot: Mutex::new(RefCell::new(PpuSnapshot::new())),
            live_ppu_snapshot: Mutex::new(RefCell::new(PpuSnapshot::new())),
            frame_slots: [
                Mutex::new(RefCell::new([0; FRAMEBUFFER_SIZE])),
                Mutex::new(RefCell::new([0; FRAMEBUFFER_SIZE])),
            ],
            published_frame: AtomicUsize::new(0),
            published_frame_seq: AtomicU32::new(0),
            sync_complete: AtomicU32::new(0),
            audio_queue_drops: AtomicU32::new(0),
            apu_nr52: AtomicU8::new(0),
            ppu_ly: AtomicU8::new(0),
            ppu_stat: AtomicU8::new(0),
            pending_if_bits: AtomicU8::new(0),
            ppu_render_version: AtomicU32::new(0),
            ppu_state: Mutex::new(RefCell::new(PpuState {
                dot: 0,
                ly: 0,
                mode: rustyboy_core::cpu::peripheral::ppu::PpuMode::OamScan,
                window_line_counter: 0,
                lcdc: 0,
                stat: 0,
                scy: 0,
                scx: 0,
                lyc: 0,
                bgp: 0,
                obp0: 0,
                obp1: 0,
                wy: 0,
                wx: 0,
            })),
            #[cfg(feature = "perf")]
            apu_perf: Mutex::new(RefCell::new(ApuPerfProfile {
                frame_seq: 0,
                pulse: 0,
                wave: 0,
                noise: 0,
                mix: 0,
            })),
            #[cfg(feature = "perf")]
            ppu_perf: Mutex::new(RefCell::new(PpuPerfProfile {
                render_bg: 0,
                render_window: 0,
                render_sprites: 0,
                build_stat: 0,
            })),
        }
    }

    fn copy_published_frame(&self, out: &mut [u8; FRAMEBUFFER_SIZE]) {
        let slot = self.published_frame.load(Ordering::Acquire) & 1;
        critical_section::with(|cs| {
            let frame = self.frame_slots[slot].borrow(cs).borrow();
            out.copy_from_slice(&*frame);
        });
    }

    fn publish_frame(&self, worker: &mut GameBoyWorker, slot: usize) {
        critical_section::with(|cs| {
            let mut frame = self.frame_slots[slot].borrow(cs).borrow_mut();
            worker.copy_framebuffer(&mut *frame);
        });
        self.published_frame.store(slot, Ordering::Release);
        self.published_frame_seq.fetch_add(1, Ordering::AcqRel);
    }

    fn clear_published_frames(&self) {
        critical_section::with(|cs| {
            self.frame_slots[0].borrow(cs).borrow_mut().fill(0);
            self.frame_slots[1].borrow(cs).borrow_mut().fill(0);
        });
        self.published_frame.store(0, Ordering::Release);
        self.published_frame_seq.store(0, Ordering::Release);
    }

    fn publish_frontend_state(&self, state: WorkerFrontendState) {
        self.apu_nr52.store(state.apu_nr52, Ordering::Release);
        self.ppu_ly.store(state.ppu_ly, Ordering::Release);
        self.ppu_stat.store(state.ppu_stat, Ordering::Release);
        if state.if_bits != 0 {
            self.pending_if_bits
                .fetch_or(state.if_bits, Ordering::AcqRel);
        }
    }

    fn copy_live_ppu_snapshot(&self, io: &[u8], vram: &[u8], oam: &[u8]) {
        critical_section::with(|cs| {
            let mut snapshot = self.live_ppu_snapshot.borrow(cs).borrow_mut();
            snapshot.io.copy_from_slice(&io[..0x80]);
            snapshot.vram.copy_from_slice(&vram[..0x2000]);
            snapshot.oam.copy_from_slice(&oam[..0xA0]);
        });
        self.ppu_render_version.store(0, Ordering::Release);
    }

    fn write_live_vram_range(&self, start_offset: u16, data: &[u8]) {
        critical_section::with(|cs| {
            let mut snapshot = self.live_ppu_snapshot.borrow(cs).borrow_mut();
            let start = start_offset as usize;
            let len = data.len().min(snapshot.vram.len().saturating_sub(start));
            snapshot.vram[start..start + len].copy_from_slice(&data[..len]);
        });
        self.ppu_render_version.fetch_add(1, Ordering::AcqRel);
    }

    fn write_live_oam_range(&self, start_offset: u16, data: &[u8]) {
        critical_section::with(|cs| {
            let mut snapshot = self.live_ppu_snapshot.borrow(cs).borrow_mut();
            let start = start_offset as usize;
            let len = data.len().min(snapshot.oam.len().saturating_sub(start));
            snapshot.oam[start..start + len].copy_from_slice(&data[..len]);
        });
        self.ppu_render_version.fetch_add(1, Ordering::AcqRel);
    }

    fn write_live_ppu_register(&self, addr: u16, value: u8) {
        if !(0xFF00..=0xFF7F).contains(&addr) {
            return;
        }
        critical_section::with(|cs| {
            self.live_ppu_snapshot.borrow(cs).borrow_mut().io[(addr - 0xFF00) as usize] = value;
        });
    }

    fn snapshot_ppu_state(&self) -> PpuState {
        critical_section::with(|cs| *self.ppu_state.borrow(cs).borrow())
    }
}

#[derive(Clone, Copy)]
struct PendingApuAdvance {
    cycles: u16,
    div_counter: u16,
}

#[derive(Clone, Copy)]
struct PendingPpuAdvance {
    cycles: u16,
}

struct Core1WorkerLink {
    command_tx: &'static MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY>,
    audio_rx: &'static MpMcQueue<i16, AUDIO_QUEUE_CAPACITY>,
    shared: &'static SharedWorkerState,
    pending_apu: Option<PendingApuAdvance>,
    pending_ppu: Option<PendingPpuAdvance>,
    next_ticket: u32,
    last_frame_seq: u32,
    last_profile_frame_seq: u32,
    transport_profile: TransportProfile,
}

impl Core1WorkerLink {
    fn new(core1: Peri<'static, CORE1>) -> Self {
        let shared = &SHARED_WORKER_STATE;
        let command_tx = &COMMAND_QUEUE;
        let audio_rx = &AUDIO_QUEUE;
        let core1_stack = Box::leak(Box::new(Stack::<CORE1_STACK_SIZE>::new()));
        let worker = Box::leak(Box::new(GameBoyWorker::new()));
        let audio_scratch = Box::leak(Box::new(Vec::with_capacity(2048)));

        info!("spawning core1 worker");
        let shared_for_core1 = shared;
        multicore::spawn_core1(core1, core1_stack, move || {
            run_core1_worker(
                command_tx,
                audio_rx,
                shared_for_core1,
                worker,
                audio_scratch,
            )
        });
        info!("core1 worker spawned");

        Self {
            command_tx,
            audio_rx,
            shared,
            pending_apu: None,
            pending_ppu: None,
            next_ticket: 1,
            last_frame_seq: 0,
            last_profile_frame_seq: 0,
            transport_profile: TransportProfile::default(),
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn enqueue_blocking(&mut self, command: Core1Command) {
        let mut command = command;
        loop {
            match self.command_tx.enqueue(command) {
                Ok(()) => {
                    self.transport_profile.command_enqueues =
                        self.transport_profile.command_enqueues.wrapping_add(1);
                    asm::sev();
                    return;
                }
                Err(returned) => {
                    command = returned;
                    self.transport_profile.command_queue_spins =
                        self.transport_profile.command_queue_spins.wrapping_add(1);
                    core::hint::spin_loop();
                }
            }
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn flush_pending_apu(&mut self) {
        if let Some(pending) = self.pending_apu.take() {
            self.transport_profile.apu_commands =
                self.transport_profile.apu_commands.wrapping_add(1);
            self.enqueue_blocking(Core1Command::Worker(WorkerCommand::AdvanceApu {
                cycles: pending.cycles,
                div_counter: pending.div_counter,
            }));
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn queue_pending_apu(&mut self, cycles: u16, div_counter: u16) {
        match self.pending_apu {
            Some(mut pending) => {
                let total = pending.cycles as u32 + cycles as u32;
                if total > u16::MAX as u32 {
                    self.flush_pending_apu();
                    self.pending_apu = Some(PendingApuAdvance {
                        cycles,
                        div_counter,
                    });
                } else {
                    pending.cycles = total as u16;
                    pending.div_counter = div_counter;
                    self.pending_apu = Some(pending);
                }
            }
            None => {
                self.pending_apu = Some(PendingApuAdvance {
                    cycles,
                    div_counter,
                });
            }
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn flush_pending_ppu(&mut self) {
        if let Some(pending) = self.pending_ppu.take() {
            self.transport_profile.ppu_advance_commands =
                self.transport_profile.ppu_advance_commands.wrapping_add(1);
            self.enqueue_blocking(Core1Command::Worker(WorkerCommand::AdvancePpu {
                cycles: pending.cycles,
            }));
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn queue_pending_ppu(&mut self, cycles: u16) {
        match self.pending_ppu {
            Some(mut pending) => {
                let total = pending.cycles as u32 + cycles as u32;
                if total >= PPU_ADVANCE_BATCH_CYCLES as u32 || total > u16::MAX as u32 {
                    self.flush_pending_ppu();
                    self.pending_ppu = Some(PendingPpuAdvance { cycles });
                } else {
                    pending.cycles = total as u16;
                    self.pending_ppu = Some(pending);
                }
            }
            None => {
                if cycles >= PPU_ADVANCE_BATCH_CYCLES {
                    self.transport_profile.ppu_advance_commands =
                        self.transport_profile.ppu_advance_commands.wrapping_add(1);
                    self.enqueue_blocking(Core1Command::Worker(WorkerCommand::AdvancePpu {
                        cycles,
                    }));
                } else {
                    self.pending_ppu = Some(PendingPpuAdvance { cycles });
                }
            }
        }
    }

    fn issue_ticket(&mut self) -> u32 {
        let ticket = self.next_ticket.max(1);
        self.next_ticket = self.next_ticket.wrapping_add(1).max(1);
        ticket
    }

    fn wait_for_ticket(&self, ticket: u32) {
        while self.shared.sync_complete.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
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
            } => self.queue_pending_apu(cycles, div_counter),
            WorkerCommand::AdvancePpu { cycles } => self.queue_pending_ppu(cycles),
            WorkerCommand::WriteApuRegister { .. } | WorkerCommand::WriteWaveRam { .. } => {
                self.flush_pending_apu();
                self.transport_profile.apu_commands =
                    self.transport_profile.apu_commands.wrapping_add(1);
                self.enqueue_blocking(Core1Command::Worker(command));
            }
            WorkerCommand::WritePpuRegister { .. } => {
                self.flush_pending_ppu();
                self.transport_profile.ppu_register_writes =
                    self.transport_profile.ppu_register_writes.wrapping_add(1);
                self.enqueue_blocking(Core1Command::Worker(command));
            }
            _ => {
                self.enqueue_blocking(Core1Command::Worker(command));
            }
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_vram_range(&mut self, start_offset: u16, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.flush_pending_ppu();
        self.shared.write_live_vram_range(start_offset, data);
        self.transport_profile.ppu_vram_bytes = self
            .transport_profile
            .ppu_vram_bytes
            .wrapping_add(data.len() as u32);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.flush_pending_ppu();
        self.shared.write_live_oam_range(start_offset, data);
        self.transport_profile.ppu_oam_bytes = self
            .transport_profile
            .ppu_oam_bytes
            .wrapping_add(data.len() as u32);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_ppu_register(&mut self, addr: u16, value: u8) {
        self.flush_pending_ppu();
        self.shared.write_live_ppu_register(addr, value);
        self.transport_profile.ppu_register_writes =
            self.transport_profile.ppu_register_writes.wrapping_add(1);
        self.enqueue_blocking(Core1Command::Worker(WorkerCommand::WritePpuRegister {
            addr,
            value,
        }));
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
        self.flush_pending_apu();
        out.clear();
        while let Some(sample) = self.audio_rx.dequeue() {
            out.push(sample);
        }
    }

    fn sync_apu_state(&mut self, io: &[u8]) {
        self.flush_pending_apu();
        critical_section::with(|cs| {
            self.shared
                .sync_snapshot
                .borrow(cs)
                .borrow_mut()
                .io
                .copy_from_slice(&io[..0x80]);
        });
        let ticket = self.issue_ticket();
        self.enqueue_blocking(Core1Command::SyncApu { ticket });
        self.wait_for_ticket(ticket);
    }

    fn sync_ppu_state(&mut self, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.flush_pending_ppu();
        critical_section::with(|cs| {
            let mut snapshots = self.shared.sync_snapshot.borrow(cs).borrow_mut();
            snapshots.io.copy_from_slice(&io[..0x80]);
            snapshots.vram.copy_from_slice(&vram[..0x2000]);
            snapshots.oam.copy_from_slice(&oam[..0xA0]);
        });
        self.shared.copy_live_ppu_snapshot(io, vram, oam);
        let ticket = self.issue_ticket();
        self.enqueue_blocking(Core1Command::SyncPpu { ticket });
        self.wait_for_ticket(ticket);
        self.shared.clear_published_frames();
        self.last_frame_seq = 0;
        self.last_profile_frame_seq = 0;
    }

    fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.flush_pending_ppu();
        critical_section::with(|cs| {
            let mut snapshots = self.shared.sync_snapshot.borrow(cs).borrow_mut();
            snapshots.io.copy_from_slice(&io[..0x80]);
            snapshots.vram.copy_from_slice(&vram[..0x2000]);
            snapshots.oam.copy_from_slice(&oam[..0xA0]);
        });
        self.shared.copy_live_ppu_snapshot(io, vram, oam);
        let ticket = self.issue_ticket();
        self.enqueue_blocking(Core1Command::LoadPpuState { ticket, state });
        self.wait_for_ticket(ticket);
        self.shared.clear_published_frames();
        self.last_frame_seq = 0;
        self.last_profile_frame_seq = 0;
    }

    fn snapshot_ppu_state(&self, _io: &[u8]) -> PpuState {
        self.shared.snapshot_ppu_state()
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
        self.flush_pending_apu();
        let ticket = self.issue_ticket();
        self.enqueue_blocking(Core1Command::TakeApuPerfProfile { ticket });
        self.wait_for_ticket(ticket);
        critical_section::with(|cs| {
            core::mem::take(&mut *self.shared.apu_perf.borrow(cs).borrow_mut())
        })
    }

    #[cfg(feature = "perf")]
    fn take_ppu_perf_profile(&mut self) -> PpuPerfProfile {
        self.flush_pending_ppu();
        let ticket = self.issue_ticket();
        self.enqueue_blocking(Core1Command::TakePpuPerfProfile { ticket });
        self.wait_for_ticket(ticket);
        critical_section::with(|cs| {
            core::mem::take(&mut *self.shared.ppu_perf.borrow(cs).borrow_mut())
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
fn publish_worker_state(
    shared: &'static SharedWorkerState,
    worker: &mut GameBoyWorker,
    publish_slot: &mut usize,
) {
    let state = worker.poll_frontend_state();
    if state.frame_ready {
        shared.publish_frame(worker, *publish_slot);
        *publish_slot ^= 1;
    }
    shared.publish_frontend_state(state);
    critical_section::with(|cs| {
        *shared.ppu_state.borrow(cs).borrow_mut() = worker.snapshot_ppu_state();
    });
}

#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn run_core1_worker(
    command_rx: &'static MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY>,
    audio_tx: &'static MpMcQueue<i16, AUDIO_QUEUE_CAPACITY>,
    shared: &'static SharedWorkerState,
    mut worker: &'static mut GameBoyWorker,
    mut audio_scratch: &'static mut Vec<i16>,
) -> ! {
    info!("core1 worker loop start");
    let mut publish_slot = 1usize;
    let mut last_ppu_render_version = 0u32;

    loop {
        let Some(command) = command_rx.dequeue() else {
            asm::wfe();
            continue;
        };

        match command {
            Core1Command::Worker(worker_command) => {
                if matches!(worker_command, WorkerCommand::AdvancePpu { .. }) {
                    let render_version = shared.ppu_render_version.load(Ordering::Acquire);
                    if render_version != last_ppu_render_version {
                        critical_section::with(|cs| {
                            let snapshot = shared.live_ppu_snapshot.borrow(cs).borrow();
                            worker.update_ppu_render_state(&snapshot.vram, &snapshot.oam);
                        });
                        last_ppu_render_version = render_version;
                    }
                }
                worker.send(worker_command);
                publish_worker_state(shared, &mut worker, &mut publish_slot);
            }
            Core1Command::SyncApu { ticket } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.sync_apu_state(&snapshots.io);
                });
                publish_worker_state(shared, &mut worker, &mut publish_slot);
                info!("core1 sync apu {}", ticket);
                shared.sync_complete.store(ticket, Ordering::Release);
            }
            Core1Command::SyncPpu { ticket } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.sync_ppu_state(&snapshots.io, &snapshots.vram, &snapshots.oam);
                });
                last_ppu_render_version = 0;
                publish_worker_state(shared, &mut worker, &mut publish_slot);
                info!("core1 sync ppu {}", ticket);
                shared.sync_complete.store(ticket, Ordering::Release);
            }
            Core1Command::LoadPpuState { ticket, state } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.load_ppu_state(state, &snapshots.io, &snapshots.vram, &snapshots.oam);
                });
                last_ppu_render_version = 0;
                publish_worker_state(shared, &mut worker, &mut publish_slot);
                shared.sync_complete.store(ticket, Ordering::Release);
            }
            #[cfg(feature = "perf")]
            Core1Command::TakeApuPerfProfile { ticket } => {
                critical_section::with(|cs| {
                    *shared.apu_perf.borrow(cs).borrow_mut() = worker.take_apu_perf_profile();
                });
                shared.sync_complete.store(ticket, Ordering::Release);
            }
            #[cfg(feature = "perf")]
            Core1Command::TakePpuPerfProfile { ticket } => {
                critical_section::with(|cs| {
                    *shared.ppu_perf.borrow(cs).borrow_mut() = worker.take_ppu_perf_profile();
                });
                shared.sync_complete.store(ticket, Ordering::Release);
            }
        }

        worker.drain_audio_samples_into_i16(&mut audio_scratch);
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
    }
}
