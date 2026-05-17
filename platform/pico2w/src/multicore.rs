#![cfg(target_arch = "arm")]

use alloc::{boxed::Box, vec::Vec};
use core::cell::{RefCell, UnsafeCell};
use core::mem::MaybeUninit;
use core::ptr::addr_of_mut;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};

use cortex_m::asm;
use critical_section::Mutex;

use embassy_rp::multicore::{self, Stack};
use embassy_rp::peripherals::CORE1;
use embassy_rp::Peri;
use heapless::mpmc::MpMcQueue;
use rustyboy_core::cpu::cpu::CpuError;
use rustyboy_core::cpu::peripheral::joypad::Button;
use rustyboy_core::cpu::peripheral::ppu::FRAMEBUFFER_SIZE;
use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::save_state::{PpuState, SaveState};
use rustyboy_core::gameboy::GameBoy;
use rustyboy_core::ipc::{GameBoyWorker, WorkerCommand, WorkerOutput, WorkerTransport};
use rustyboy_core::memory::cartridge::Cartridge;
use rustyboy_core::memory::memory::{Error as MemoryError, GameBoyMemory};

use crate::display::{scale_to_rgb565, ScaledFrame, SCALED_FRAME_PIXELS};
use crate::stack_probe;

const CORE1_STACK_SIZE: usize = 8192;
const COMMAND_QUEUE_CAPACITY: usize = 512;
const AUDIO_QUEUE_CAPACITY: usize = 2048;
const AUDIO_SCRATCH_CAPACITY: usize = 2048;
const PPU_ADVANCE_BATCH_CYCLES: u16 = 912;
const SCALED_FRAME_SLOT_COUNT: usize = 3;
const IO_REG_BASE: u16 = 0xFF00;
const IO_REG_END: u16 = 0xFF7F;

static SHARED_WORKER_STATE: SharedWorkerState = SharedWorkerState::new();
static COMMAND_QUEUE: MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY> = MpMcQueue::new();
static AUDIO_QUEUE: MpMcQueue<i16, AUDIO_QUEUE_CAPACITY> = MpMcQueue::new();
#[unsafe(link_section = ".core1_stack")]
static mut CORE1_STACK: Stack<CORE1_STACK_SIZE> = Stack::new();
static CORE1_WORKER: StaticStorage<GameBoyWorker> = StaticStorage::new();
static CORE1_AUDIO_SCRATCH: StaticStorage<Vec<i16>> = StaticStorage::new();
static mut CORE1_AUDIO_SCRATCH_BUF: [MaybeUninit<i16>; AUDIO_SCRATCH_CAPACITY] =
    [MaybeUninit::uninit(); AUDIO_SCRATCH_CAPACITY];

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

struct SharedScaledFrameSlot(UnsafeCell<ScaledFrame>);

impl SharedScaledFrameSlot {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; SCALED_FRAME_PIXELS]))
    }

    fn as_ptr(&self) -> *const ScaledFrame {
        self.0.get() as *const ScaledFrame
    }

    fn as_mut_ptr(&self) -> *mut ScaledFrame {
        self.0.get()
    }
}

// Safety: access is synchronized by the published-frame atomics; core 1 only
// writes the next unpublished slot and only flips the published slot after the
// writes complete.
unsafe impl Sync for SharedScaledFrameSlot {}

struct StaticStorage<T>(UnsafeCell<MaybeUninit<T>>);

impl<T> StaticStorage<T> {
    const fn new() -> Self {
        Self(UnsafeCell::new(MaybeUninit::uninit()))
    }

    unsafe fn init(&self, value: T) -> &'static mut T {
        (*self.0.get()).write(value)
    }

    unsafe fn as_mut_ptr(&self) -> *mut T {
        (*self.0.get()).as_mut_ptr()
    }
}

unsafe impl<T> Sync for StaticStorage<T> {}

struct SharedWorkerState {
    sync_snapshot: Mutex<RefCell<PpuSnapshot>>,
    live_ppu_snapshot: Mutex<RefCell<PpuSnapshot>>,
    scaled_frame_slots: [SharedScaledFrameSlot; SCALED_FRAME_SLOT_COUNT],
    scaled_frame_busy: [AtomicBool; SCALED_FRAME_SLOT_COUNT],
    published_frame: AtomicU8,
    published_frame_seq: AtomicU32,
    sync_complete: AtomicU32,
    audio_queue_drops: AtomicU32,
    apu_nr52: AtomicU8,
    ppu_ly: AtomicU8,
    ppu_stat: AtomicU8,
    pending_if_bits: AtomicU8,
    ppu_render_version: AtomicU32,
    ppu_state: Mutex<RefCell<PpuState>>,
}

impl SharedWorkerState {
    const fn new() -> Self {
        Self {
            sync_snapshot: Mutex::new(RefCell::new(PpuSnapshot::new())),
            live_ppu_snapshot: Mutex::new(RefCell::new(PpuSnapshot::new())),
            scaled_frame_slots: [
                SharedScaledFrameSlot::new(),
                SharedScaledFrameSlot::new(),
                SharedScaledFrameSlot::new(),
            ],
            scaled_frame_busy: [
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
            ],
            published_frame: AtomicU8::new(0),
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
        }
    }

    fn publish_frame(&self, worker: &GameBoyWorker) {
        let current_slot = self.published_frame.load(Ordering::Acquire) as usize;
        // Scan all slots for a free one that isn't currently being consumed.
        // All initial values are zero so SharedWorkerState stays in .bss.
        let Some(target_slot) = (0..SCALED_FRAME_SLOT_COUNT).find(|&slot| {
            slot != current_slot && !self.scaled_frame_busy[slot].load(Ordering::Acquire)
        }) else {
            return;
        };
        let framebuffer = worker.framebuffer();
        // Safety: called only from core 1; target_slot is neither the currently
        // published slot nor marked busy, so core 0 cannot be reading it.
        unsafe {
            scale_to_rgb565(framebuffer, &mut *self.scaled_frame_slots[target_slot].as_mut_ptr());
        }
        self.published_frame.store(target_slot as u8, Ordering::Release);
        self.published_frame_seq.fetch_add(1, Ordering::AcqRel);
    }

    fn clear_published_frames(&self) {
        for slot in &self.scaled_frame_slots {
            // Safety: called during reset before core 1 is re-spawned, so no
            // concurrent access to any slot is possible.
            unsafe {
                (*slot.as_mut_ptr()).fill(0);
            }
        }
        for busy in &self.scaled_frame_busy {
            busy.store(false, Ordering::Release);
        }
        self.published_frame.store(0, Ordering::Release);
        self.published_frame_seq.store(0, Ordering::Release);
    }

    fn publish_worker_output(&self, state: WorkerOutput) {
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
        if !(IO_REG_BASE..=IO_REG_END).contains(&addr) {
            return;
        }
        critical_section::with(|cs| {
            self.live_ppu_snapshot.borrow(cs).borrow_mut().io[(addr - IO_REG_BASE) as usize] =
                value;
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

struct Core1Transport {
    command_tx: &'static MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY>,
    audio_rx: &'static MpMcQueue<i16, AUDIO_QUEUE_CAPACITY>,
    shared: &'static SharedWorkerState,
    pending_apu: Option<PendingApuAdvance>,
    pending_ppu: Option<PendingPpuAdvance>,
    next_ticket: u32,
    last_frame_seq: u32,
    last_profile_frame_seq: u32,
    held_frame_slot: u8,
    transport_profile: TransportProfile,
}

impl Core1Transport {
    fn new(core1: Peri<'static, CORE1>) -> Self {
        let shared = &SHARED_WORKER_STATE;
        let command_tx = &COMMAND_QUEUE;
        let audio_rx = &AUDIO_QUEUE;
        // Safety: CORE1_STACK is a static mut accessed exactly once here during
        // init, before spawn_core1 transfers ownership to the core 1 thread.
        let core1_stack = unsafe { &mut *addr_of_mut!(CORE1_STACK) };
        // Safety: CORE1_WORKER is MaybeUninit; init_in_place writes it exactly
        // once here before core 1 starts, so there is no concurrent access.
        let worker = unsafe { GameBoyWorker::init_in_place(CORE1_WORKER.as_mut_ptr()) };
        // Safety: CORE1_AUDIO_SCRATCH_BUF is a static MaybeUninit<i16> array;
        // from_raw_parts takes ownership of it as a Vec backing buffer.
        // Initialized once before core 1 starts, so no concurrent access.
        let audio_scratch = unsafe {
            CORE1_AUDIO_SCRATCH.init(Vec::from_raw_parts(
                addr_of_mut!(CORE1_AUDIO_SCRATCH_BUF) as *mut i16,
                0,
                AUDIO_SCRATCH_CAPACITY,
            ))
        };

        // Safety: writing to the bottom 256 bytes of core 1's stack before the
        // thread starts; no concurrent access to this memory region yet.
        unsafe {
            stack_probe::paint_region(
                core::ptr::addr_of_mut!(CORE1_STACK) as *mut u8,
                256,
            );
        }

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
        Self {
            command_tx,
            audio_rx,
            shared,
            pending_apu: None,
            pending_ppu: None,
            next_ticket: 1,
            last_frame_seq: 0,
            last_profile_frame_seq: 0,
            held_frame_slot: u8::MAX,
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

    fn published_scaled_frame(&mut self) -> &'static ScaledFrame {
        if self.held_frame_slot != u8::MAX {
            self.release_scaled_frame();
        }
        let slot = self.shared.published_frame.load(Ordering::Acquire) as usize;
        self.shared.scaled_frame_busy[slot].store(true, Ordering::Release);
        self.held_frame_slot = slot as u8;
        // Safety: slot was loaded with Acquire from published_frame and then
        // marked busy with Release; core 1 only writes to slots that are
        // neither published nor busy, so exclusive read access is guaranteed
        // until release_scaled_frame clears the busy flag.
        unsafe { &*self.shared.scaled_frame_slots[slot].as_ptr() }
    }

    fn release_scaled_frame(&mut self) {
        if self.held_frame_slot == u8::MAX {
            return;
        }
        self.shared.scaled_frame_busy[self.held_frame_slot as usize].store(false, Ordering::Release);
        self.held_frame_slot = u8::MAX;
    }
}

impl WorkerTransport for Core1Transport {
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

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_ppu_registers(&mut self, regs: &[(u16, u8)]) {
        if regs.is_empty() {
            return;
        }
        self.flush_pending_ppu();
        critical_section::with(|cs| {
            let mut snapshot = self.shared.live_ppu_snapshot.borrow(cs).borrow_mut();
            for &(addr, value) in regs {
                if (IO_REG_BASE..=IO_REG_END).contains(&addr) {
                    snapshot.io[(addr - IO_REG_BASE) as usize] = value;
                }
            }
        });
        self.transport_profile.ppu_register_writes = self
            .transport_profile
            .ppu_register_writes
            .wrapping_add(regs.len() as u32);
        for &(addr, value) in regs {
            self.enqueue_blocking(Core1Command::Worker(WorkerCommand::WritePpuRegister {
                addr,
                value,
            }));
        }
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

    fn poll_output(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> WorkerOutput {
        let _ = out;
        let frame_seq = self.shared.published_frame_seq.load(Ordering::Acquire);
        let frame_ready = frame_seq != self.last_frame_seq;
        if frame_ready {
            self.last_frame_seq = frame_seq;
        }
        WorkerOutput {
            apu_nr52: self.shared.apu_nr52.load(Ordering::Acquire),
            ppu_ly: self.shared.ppu_ly.load(Ordering::Acquire),
            ppu_stat: self.shared.ppu_stat.load(Ordering::Acquire),
            if_bits: self.shared.pending_if_bits.swap(0, Ordering::AcqRel),
            frame_ready,
        }
    }

}

pub struct PicoGameBoy {
    gb: GameBoy<Core1Transport>,
}

impl PicoGameBoy {
    pub fn with_cartridge(core1: Peri<'static, CORE1>, cart: Box<dyn Cartridge>) -> Self {
        let memory = GameBoyMemory::with_cartridge_boxed(cart);
        let transport = Core1Transport::new(core1);
        let mut gb = GameBoy::with_transport(memory, transport);
        gb.push_worker_state();
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
        gb.apply_dmg_state();
        Self { gb }
    }

    #[inline(always)]
    pub fn tick(&mut self) {
        self.gb.tick();
    }

    pub fn step(&mut self) -> Result<u8, CpuError> {
        self.gb.step()
    }

    #[inline(always)]
    pub fn front_buffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.gb.front_buffer()
    }

    #[inline(always)]
    pub fn published_scaled_frame(&mut self) -> &'static ScaledFrame {
        self.gb.transport_mut().published_scaled_frame()
    }

    #[inline(always)]
    pub fn release_scaled_frame(&mut self) {
        self.gb.transport_mut().release_scaled_frame();
    }

    #[inline(always)]
    pub fn cycle_counter(&self) -> u64 {
        self.gb.cycle_counter()
    }

    pub fn set_button(&mut self, btn: Button, pressed: bool) {
        self.gb.set_button(btn, pressed);
    }

    pub fn drain_audio_samples_into_i16(&mut self, out: &mut Vec<i16>) {
        self.gb.drain_audio_samples_into_i16(out);
    }

    pub fn read_memory(&self, address: u16) -> Result<u8, MemoryError> {
        self.gb.read_memory(address)
    }

    pub fn save_state(&self) -> Vec<u8> {
        self.gb.save_state()
    }

    pub fn load_state(&mut self, state: SaveState) -> Result<(), &'static str> {
        self.gb.load_state(state)
    }

    pub fn take_transport_profile(&mut self) -> TransportProfile {
        self.gb.transport_mut().take_transport_profile()
    }

}

#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn publish_worker_state(
    shared: &'static SharedWorkerState,
    worker: &mut GameBoyWorker,
) {
    let state = worker.poll_output();
    if state.frame_ready {
        shared.publish_frame(worker);
    }
    shared.publish_worker_output(state);
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
    let mut last_ppu_render_version = 0u32;
    let core1_stack_bottom = core::ptr::addr_of!(CORE1_STACK) as *const u8;

    loop {
        // Safety: core1_stack_bottom points to the bottom of this thread's own
        // stack; reading the guard region to detect overflow is safe from core 1.
        unsafe { stack_probe::check_region(core1_stack_bottom, 256, "core1") };

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
                publish_worker_state(shared, &mut worker);
            }
            Core1Command::SyncApu { ticket } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.sync_apu_state(&snapshots.io);
                });
                publish_worker_state(shared, &mut worker);
                shared.sync_complete.store(ticket, Ordering::Release);
            }
            Core1Command::SyncPpu { ticket } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.sync_ppu_state(&snapshots.io, &snapshots.vram, &snapshots.oam);
                });
                last_ppu_render_version = 0;
                publish_worker_state(shared, &mut worker);
                shared.sync_complete.store(ticket, Ordering::Release);
            }
            Core1Command::LoadPpuState { ticket, state } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.load_ppu_state(state, &snapshots.io, &snapshots.vram, &snapshots.oam);
                });
                last_ppu_render_version = 0;
                publish_worker_state(shared, &mut worker);
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
