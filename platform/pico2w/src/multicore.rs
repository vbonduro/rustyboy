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
use rustyboy_core::cpu::peripheral::ppu::{PpuMode, FRAMEBUFFER_SIZE};
use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::save_state::{PpuState, SaveState};
use rustyboy_core::gameboy::GameBoy;
use rustyboy_core::ipc::{GameBoyWorker, WorkerCommand, WorkerOutput, WorkerTransport};
use rustyboy_core::memory::cartridge::Cartridge;
use rustyboy_core::memory::memory::{Error as MemoryError, GameBoyMemory};

use crate::crash::CRASH_CONTEXT;
use crate::display::{scale_to_rgb565, ScaledFrame, SCALED_FRAME_PIXELS};
use crate::stack_probe;

const CORE1_STACK_SIZE: usize = 8192;
const COMMAND_QUEUE_CAPACITY: usize = 512;
const AUDIO_QUEUE_CAPACITY: usize = 2048;
const PPU_ADVANCE_BATCH_CYCLES: u16 = 456;
const LCD_TIMING_BATCH_CYCLES: u16 = 80;
const SCALED_FRAME_SLOT_COUNT: usize = 3;
/// Number of u32 words needed to hold one dirty bit per GB scanline (144 rows).
/// ⌈144 / 32⌉ = 5  (160 bits, 16 spare).
pub const DIRTY_BITMAP_WORDS: usize = (VISIBLE_SCANLINES as usize + 31) / 32;
/// Bytes per GB scanline (160 pixels × 1 byte/pixel palette index).
const GB_ROW_BYTES: usize = FRAMEBUFFER_SIZE / VISIBLE_SCANLINES as usize;
const IO_REG_BASE: u16 = 0xFF00;
const IO_REG_END: u16 = 0xFF7F;
const STAT_ADDR: u16 = 0xFF41;
const LY_ADDR: u16 = 0xFF44;
const VBLANK_INTERRUPT_BIT: u8 = 0;
const STAT_INTERRUPT_BIT: u8 = 1;
const LCDC_IO: usize = 0x40;
const STAT_IO: usize = 0x41;
const SCY_IO: usize = 0x42;
const SCX_IO: usize = 0x43;
const LY_IO: usize = 0x44;
const LYC_IO: usize = 0x45;
const BGP_IO: usize = 0x47;
const OBP0_IO: usize = 0x48;
const OBP1_IO: usize = 0x49;
const WY_IO: usize = 0x4A;
const WX_IO: usize = 0x4B;
const DOTS_PER_SCANLINE: u16 = 456;
const OAM_SCAN_DOTS: u16 = 80;
const PIXEL_TRANSFER_DOTS: u16 = 172;
const VISIBLE_SCANLINES: u8 = 144;
const TOTAL_SCANLINES: u8 = 154;
const SCREEN_HEIGHT: u8 = 144;

static SHARED_WORKER_STATE: SharedWorkerState = SharedWorkerState::new();
static COMMAND_QUEUE: MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY> = MpMcQueue::new();
static AUDIO_QUEUE: MpMcQueue<i16, AUDIO_QUEUE_CAPACITY> = MpMcQueue::new();
#[unsafe(link_section = ".core1_stack")]
static mut CORE1_STACK: Stack<CORE1_STACK_SIZE> = Stack::new();
static CORE1_WORKER: StaticStorage<GameBoyWorker> = StaticStorage::new();

#[derive(Clone, Copy)]
enum Core1Command {
    Worker(WorkerCommand),
    SyncApu {
        ticket: u32,
    },
    DrainAudio {
        ticket: u32,
    },
    SyncPpu {
        ticket: u32,
    },
    LoadPpuState {
        ticket: u32,
        state: PpuState,
    },
    /// Drain the queue and halt core1 in a WFE loop. After the ticket is
    /// acknowledged, core1 will never access flash again — safe to erase/write.
    Halt {
        ticket: u32,
    },
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

/// Per-scanline hashes from the previous published raw framebuffer.
///
/// 144 × u32 = 576 bytes (vs 22.5 KB for a full raw-frame copy).  Core 1
/// hashes each 160-byte GB scanline and compares against the stored value to
/// determine whether that row is dirty.  Hash collisions are ~1 in 4 × 10⁹
/// per row per frame — negligible in practice.
///
/// Only accessed from Core 1; never touched by Core 0.
struct SharedPrevRowHashes(UnsafeCell<[u32; VISIBLE_SCANLINES as usize]>);

impl SharedPrevRowHashes {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; VISIBLE_SCANLINES as usize]))
    }

    fn as_mut_ptr(&self) -> *mut [u32; VISIBLE_SCANLINES as usize] {
        self.0.get()
    }
}

// Safety: only accessed from Core 1 (in publish_frame and clear_published_frames
// while Core 1 is halted); no concurrent access from Core 0.
unsafe impl Sync for SharedPrevRowHashes {}

/// Dirty-row bitmap: 1 bit per GB scanline (144 rows ⟹ 5 × u32 = 160 bits).
///
/// Core 1 writes the bitmap before the Release store to `published_frame`;
/// Core 0 reads the bitmap after the Acquire load from `published_frame`.
/// The Acquire/Release pair on `published_frame` provides the necessary
/// memory ordering guarantee — no atomic operations on the bitmap itself.
struct SharedDirtyBitmap(UnsafeCell<[u32; DIRTY_BITMAP_WORDS]>);

impl SharedDirtyBitmap {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; DIRTY_BITMAP_WORDS]))
    }

    fn as_ptr(&self) -> *const [u32; DIRTY_BITMAP_WORDS] {
        self.0.get() as *const _
    }

    fn as_mut_ptr(&self) -> *mut [u32; DIRTY_BITMAP_WORDS] {
        self.0.get()
    }
}

// Safety: Core 1 writes before published_frame.store(Release);
// Core 0 reads after published_frame.load(Acquire).
unsafe impl Sync for SharedDirtyBitmap {}

struct StaticStorage<T>(UnsafeCell<MaybeUninit<T>>);

impl<T> StaticStorage<T> {
    const fn new() -> Self {
        Self(UnsafeCell::new(MaybeUninit::uninit()))
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
    apu_nr52: AtomicU8,
    ppu_ly: AtomicU8,
    ppu_stat: AtomicU8,
    pending_if_bits: AtomicU8,
    ppu_render_version: AtomicU32,
    /// Per-row hashes of the previous raw framebuffer used to detect dirty rows.
    /// 144 × u32 = 576 bytes.  Written and read exclusively by Core 1.
    prev_row_hashes: SharedPrevRowHashes,
    /// Dirty-row bitmap for the most recently published frame.  Core 1 writes
    /// this before the Release store to `published_frame`; Core 0 reads it after
    /// the Acquire load from `published_frame`.
    dirty_rows: SharedDirtyBitmap,
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
            apu_nr52: AtomicU8::new(0),
            ppu_ly: AtomicU8::new(0),
            ppu_stat: AtomicU8::new(0),
            pending_if_bits: AtomicU8::new(0),
            ppu_render_version: AtomicU32::new(0),
            prev_row_hashes: SharedPrevRowHashes::new(),
            dirty_rows: SharedDirtyBitmap::new(),
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

        // Compute the dirty-row bitmap by hashing each 160-byte GB scanline and
        // comparing against the stored previous-frame hash for that row.
        //
        // Using per-row hashes (576 bytes) instead of a full raw-frame copy
        // (22.5 KB) keeps the .bss footprint within the 520 KB SRAM budget.
        //
        // Safety: prev_row_hashes and dirty_rows are accessed only from Core 1.
        // This is the only place they are written, and publish_frame is only
        // ever called from Core 1's run_core1_worker loop.
        unsafe {
            let hashes = &mut *self.prev_row_hashes.as_mut_ptr();
            let dirty = &mut *self.dirty_rows.as_mut_ptr();
            dirty.fill(0);
            for row in 0..VISIBLE_SCANLINES as usize {
                let start = row * GB_ROW_BYTES;
                let h = hash_raw_row(&framebuffer[start..start + GB_ROW_BYTES]);
                if h != hashes[row] {
                    dirty[row / 32] |= 1u32 << (row % 32);
                    hashes[row] = h;
                }
            }
        }

        // Safety: called only from core 1; target_slot is neither the currently
        // published slot nor marked busy, so core 0 cannot be reading it.
        unsafe {
            scale_to_rgb565(
                framebuffer,
                &mut *self.scaled_frame_slots[target_slot].as_mut_ptr(),
            );
        }
        // dirty_rows is fully written above; the Release store below pairs with
        // the Acquire load in published_scaled_frame() on Core 0, ensuring the
        // dirty bitmap is visible before Core 0 reads it.
        self.published_frame
            .store(target_slot as u8, Ordering::Release);
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
        // Reset per-row hashes so the first publish after reset marks every
        // scanline dirty, ensuring a full redraw of the new game content.
        // Safety: called while Core 1 is halted (after a sync ticket); no race.
        unsafe {
            (*self.prev_row_hashes.as_mut_ptr()).fill(0);
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
}

#[derive(Clone, Copy)]
struct LcdTimingOutput {
    vblank_interrupt: bool,
    stat_interrupt: bool,
}

/// Core 0 only needs the LCD timing state that the CPU can observe through
/// LY, STAT, and LCD interrupts. Keeping this tiny mirror synchronous avoids
/// waiting on the async renderer just to answer those reads correctly.
#[derive(Clone, Copy)]
struct LcdTiming {
    dot: u16,
    ly: u8,
    mode: PpuMode,
    window_line_counter: u8,
    prev_stat_line: bool,
}

impl LcdTiming {
    const fn new() -> Self {
        Self {
            dot: 0,
            ly: 0,
            mode: PpuMode::OamScan,
            window_line_counter: 0,
            prev_stat_line: false,
        }
    }

    fn ly(&self) -> u8 {
        self.ly
    }

    fn reset_ly(&mut self) {
        self.ly = 0;
    }

    fn sync_prev_stat_line(&mut self, io: &[u8]) {
        let lyc = io[LYC_IO];
        let stat = io[STAT_IO];
        let lyc_match = self.ly == lyc;
        self.prev_stat_line = (lyc_match && (stat & 0x40 != 0))
            || (self.mode == PpuMode::HBlank && (stat & 0x08 != 0))
            || (self.mode == PpuMode::VBlank && (stat & 0x10 != 0))
            || (self.mode == PpuMode::OamScan && (stat & 0x20 != 0));
    }

    fn load_state(&mut self, state: PpuState) {
        self.dot = state.dot;
        self.ly = state.ly;
        self.mode = state.mode;
        self.window_line_counter = state.window_line_counter;
    }

    fn to_save_state(&self, io: &[u8]) -> PpuState {
        PpuState {
            dot: self.dot,
            ly: self.ly,
            mode: self.mode,
            window_line_counter: self.window_line_counter,
            lcdc: io[LCDC_IO],
            stat: io[STAT_IO],
            scy: io[SCY_IO],
            scx: io[SCX_IO],
            lyc: io[LYC_IO],
            bgp: io[BGP_IO],
            obp0: io[OBP0_IO],
            obp1: io[OBP1_IO],
            wy: io[WY_IO],
            wx: io[WX_IO],
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn tick(&mut self, cycles: u16, io: &mut [u8]) -> LcdTimingOutput {
        if io[LCDC_IO] & 0x80 == 0 {
            self.reset_lcd(io);
            return LcdTimingOutput {
                vblank_interrupt: false,
                stat_interrupt: false,
            };
        }

        let mut vblank_interrupt = false;
        let mut remaining = cycles;

        while remaining > 0 {
            let threshold = match self.mode {
                PpuMode::OamScan => OAM_SCAN_DOTS,
                PpuMode::PixelTransfer => OAM_SCAN_DOTS + PIXEL_TRANSFER_DOTS,
                PpuMode::HBlank | PpuMode::VBlank => DOTS_PER_SCANLINE,
            };
            let dots_to_threshold = threshold.saturating_sub(self.dot);

            if dots_to_threshold > 0 && remaining < dots_to_threshold {
                self.dot += remaining;
                break;
            }

            self.dot += dots_to_threshold;
            remaining -= dots_to_threshold;

            match self.mode {
                PpuMode::OamScan => {
                    self.mode = PpuMode::PixelTransfer;
                }
                PpuMode::PixelTransfer => {
                    self.mode = PpuMode::HBlank;
                    if self.window_participates_on_current_line(io) {
                        self.window_line_counter = self.window_line_counter.wrapping_add(1);
                    }
                }
                PpuMode::HBlank => {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly >= VISIBLE_SCANLINES {
                        self.mode = PpuMode::VBlank;
                        vblank_interrupt = true;
                    } else {
                        self.mode = PpuMode::OamScan;
                    }
                }
                PpuMode::VBlank => {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly >= TOTAL_SCANLINES {
                        self.ly = 0;
                        self.mode = PpuMode::OamScan;
                        self.window_line_counter = 0;
                    }
                }
            }
        }

        let stat_interrupt = self.build_stat(io);
        io[LY_IO] = self.ly;

        LcdTimingOutput {
            vblank_interrupt,
            stat_interrupt,
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn reset_lcd(&mut self, io: &mut [u8]) {
        self.dot = 0;
        self.ly = 0;
        self.mode = PpuMode::HBlank;
        self.window_line_counter = 0;
        self.prev_stat_line = false;
        io[LY_IO] = 0;
        io[STAT_IO] = (io[STAT_IO] & 0x78) | (PpuMode::HBlank as u8);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn build_stat(&mut self, io: &mut [u8]) -> bool {
        let lyc = io[LYC_IO];
        let lyc_match = self.ly == lyc;
        let new_stat =
            (io[STAT_IO] & 0x78) | if lyc_match { 0x04 } else { 0x00 } | (self.mode as u8);
        io[STAT_IO] = new_stat;

        let stat_line = (lyc_match && (new_stat & 0x40 != 0))
            || (self.mode == PpuMode::HBlank && (new_stat & 0x08 != 0))
            || (self.mode == PpuMode::VBlank && (new_stat & 0x10 != 0))
            || (self.mode == PpuMode::OamScan && (new_stat & 0x20 != 0));

        let interrupt = stat_line && !self.prev_stat_line;
        self.prev_stat_line = stat_line;
        interrupt
    }

    fn window_participates_on_current_line(&self, io: &[u8]) -> bool {
        let lcdc = io[LCDC_IO];
        lcdc & 0x20 != 0
            && lcdc & 0x01 != 0
            && self.ly < SCREEN_HEIGHT
            && self.ly >= io[WY_IO]
            && io[WX_IO] <= 166
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
    lcd_timing: LcdTiming,
    lcd_timing_io: [u8; 0x80],
    pending_lcd_timing_cycles: u16,
    lcd_timing_if_bits: u8,
    lcd_timing_frame_ready: bool,
    pending_apu: Option<PendingApuAdvance>,
    pending_ppu: Option<PendingPpuAdvance>,
    next_ticket: u32,
    last_frame_seq: u32,
    held_frame_slot: u8,
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

        // Safety: writing to the bottom 256 bytes of core 1's stack before the
        // thread starts; no concurrent access to this memory region yet.
        unsafe {
            stack_probe::paint_region(core::ptr::addr_of_mut!(CORE1_STACK) as *mut u8, 256);
        }

        let shared_for_core1 = shared;
        multicore::spawn_core1(core1, core1_stack, move || {
            run_core1_worker(command_tx, audio_rx, shared_for_core1, worker)
        });
        Self {
            command_tx,
            audio_rx,
            shared,
            lcd_timing: LcdTiming::new(),
            lcd_timing_io: [0; 0x80],
            pending_lcd_timing_cycles: 0,
            lcd_timing_if_bits: 0,
            lcd_timing_frame_ready: false,
            pending_apu: None,
            pending_ppu: None,
            next_ticket: 1,
            last_frame_seq: 0,
            held_frame_slot: u8::MAX,
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn enqueue_blocking(&mut self, command: Core1Command) {
        let mut command = command;
        loop {
            match self.command_tx.enqueue(command) {
                Ok(()) => {
                    asm::sev();
                    return;
                }
                Err(returned) => {
                    command = returned;
                    core::hint::spin_loop();
                }
            }
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn advance_lcd_timing(&mut self, cycles: u16) {
        let mut cycles = self.pending_lcd_timing_cycles as u32 + cycles as u32;
        if cycles < LCD_TIMING_BATCH_CYCLES as u32 {
            self.pending_lcd_timing_cycles = cycles as u16;
            return;
        }
        while cycles >= LCD_TIMING_BATCH_CYCLES as u32 {
            self.tick_lcd_timing(LCD_TIMING_BATCH_CYCLES);
            cycles -= LCD_TIMING_BATCH_CYCLES as u32;
        }
        self.pending_lcd_timing_cycles = cycles as u16;
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn flush_pending_lcd_timing(&mut self) {
        let pending = self.pending_lcd_timing_cycles;
        if pending == 0 {
            return;
        }
        self.pending_lcd_timing_cycles = 0;
        self.tick_lcd_timing(pending);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn tick_lcd_timing(&mut self, cycles: u16) {
        let output = self.lcd_timing.tick(cycles, &mut self.lcd_timing_io);
        if output.vblank_interrupt {
            self.lcd_timing_if_bits |= 1 << VBLANK_INTERRUPT_BIT;
            self.lcd_timing_frame_ready = true;
        }
        if output.stat_interrupt {
            self.lcd_timing_if_bits |= 1 << STAT_INTERRUPT_BIT;
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_lcd_timing_register(&mut self, addr: u16, value: u8) {
        if !(IO_REG_BASE..=IO_REG_END).contains(&addr) {
            return;
        }
        self.flush_pending_lcd_timing();
        if addr == LY_ADDR {
            self.lcd_timing.reset_ly();
            self.lcd_timing_io[(LY_ADDR - IO_REG_BASE) as usize] = 0;
            return;
        }
        self.lcd_timing_io[(addr - IO_REG_BASE) as usize] = value;
    }

    fn sync_lcd_timing_state(&mut self, io: &[u8]) {
        self.lcd_timing = LcdTiming::new();
        self.lcd_timing_io.copy_from_slice(&io[..0x80]);
        self.lcd_timing.sync_prev_stat_line(&self.lcd_timing_io);
        self.lcd_timing_io[(LY_ADDR - IO_REG_BASE) as usize] = self.lcd_timing.ly();
        self.pending_lcd_timing_cycles = 0;
        self.lcd_timing_if_bits = 0;
        self.lcd_timing_frame_ready = false;
    }

    fn load_lcd_timing_state(&mut self, state: PpuState, io: &[u8]) {
        self.lcd_timing.load_state(state);
        self.lcd_timing_io.copy_from_slice(&io[..0x80]);
        self.lcd_timing.sync_prev_stat_line(&self.lcd_timing_io);
        self.lcd_timing_io[(LY_ADDR - IO_REG_BASE) as usize] = state.ly;
        self.lcd_timing_io[(STAT_ADDR - IO_REG_BASE) as usize] = state.stat;
        self.pending_lcd_timing_cycles = 0;
        self.lcd_timing_if_bits = 0;
        self.lcd_timing_frame_ready = false;
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn flush_pending_apu(&mut self) {
        if let Some(pending) = self.pending_apu.take() {
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
        self.shared.scaled_frame_busy[self.held_frame_slot as usize]
            .store(false, Ordering::Release);
        self.held_frame_slot = u8::MAX;
    }

    /// Read the dirty-row bitmap for the most recently published frame.
    ///
    /// Must be called **after** [`published_scaled_frame`], which performs the
    /// Acquire load on `published_frame`.  That Acquire pairs with the Release
    /// store Core 1 did after writing the bitmap, so the bitmap is fully
    /// visible by the time this returns.
    fn published_dirty_rows(&self) -> [u32; DIRTY_BITMAP_WORDS] {
        // Safety: dirty_rows was written by Core 1 before the Release store to
        // published_frame; published_scaled_frame() loaded published_frame with
        // Acquire, establishing the happens-before edge.
        unsafe { *self.shared.dirty_rows.as_ptr() }
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
            WorkerCommand::AdvancePpu { cycles } => {
                self.advance_lcd_timing(cycles);
                self.queue_pending_ppu(cycles);
            }
            WorkerCommand::WriteApuRegister { .. } | WorkerCommand::WriteWaveRam { .. } => {
                self.flush_pending_apu();
                self.enqueue_blocking(Core1Command::Worker(command));
            }
            WorkerCommand::WritePpuRegister { addr, value } => {
                self.write_lcd_timing_register(addr, value);
                self.flush_pending_ppu();
                self.enqueue_blocking(Core1Command::Worker(WorkerCommand::WritePpuRegister {
                    addr,
                    value,
                }));
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
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.flush_pending_ppu();
        self.shared.write_live_oam_range(start_offset, data);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_ppu_register(&mut self, addr: u16, value: u8) {
        self.write_lcd_timing_register(addr, value);
        self.flush_pending_ppu();
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
        for &(addr, value) in regs {
            self.write_lcd_timing_register(addr, value);
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
        // Serialize queue access: core 1 enqueues all samples before core 0
        // starts dequeuing, avoiding concurrent MPMC traffic in the hot loop.
        let ticket = self.issue_ticket();
        self.enqueue_blocking(Core1Command::DrainAudio { ticket });
        self.wait_for_ticket(ticket);

        out.clear();
        let cap = out.capacity();
        let mut n = 0usize;
        while n < cap {
            if let Some(sample) = self.audio_rx.dequeue() {
                out.push(sample);
                n += 1;
            } else {
                break;
            }
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
        self.sync_lcd_timing_state(io);
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
    }

    fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.flush_pending_ppu();
        self.load_lcd_timing_state(state, io);
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
    }

    fn snapshot_ppu_state(&self, _io: &[u8]) -> PpuState {
        self.lcd_timing.to_save_state(&self.lcd_timing_io)
    }

    fn poll_output(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> WorkerOutput {
        let _ = out;
        let frame_seq = self.shared.published_frame_seq.load(Ordering::Acquire);
        let frame_ready = frame_seq != self.last_frame_seq;
        if frame_ready {
            self.last_frame_seq = frame_seq;
        }
        let _worker_if_bits = self.shared.pending_if_bits.swap(0, Ordering::AcqRel);
        let if_bits = self.lcd_timing_if_bits;
        self.lcd_timing_if_bits = 0;
        let lcd_timing_frame_ready = self.lcd_timing_frame_ready;
        self.lcd_timing_frame_ready = false;
        WorkerOutput {
            apu_nr52: self.shared.apu_nr52.load(Ordering::Acquire),
            ppu_ly: self.lcd_timing_io[(LY_ADDR - IO_REG_BASE) as usize],
            ppu_stat: self.lcd_timing_io[(STAT_ADDR - IO_REG_BASE) as usize],
            if_bits,
            frame_ready: frame_ready || lcd_timing_frame_ready,
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

    /// Return the dirty-row bitmap for the currently held published frame.
    ///
    /// Each bit k indicates that GB scanline k changed relative to the previous
    /// published frame.  Call this **after** [`published_scaled_frame`] so the
    /// Acquire ordering is already in place.
    #[inline(always)]
    pub fn published_dirty_rows(&mut self) -> [u32; DIRTY_BITMAP_WORDS] {
        self.gb.transport_mut().published_dirty_rows()
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

    pub fn external_ram(&self) -> Option<&[u8]> {
        self.gb.external_ram()
    }

    pub fn set_external_ram(&mut self, data: &[u8]) {
        self.gb.set_external_ram(data);
    }

    pub fn save_state(&self) -> Vec<u8> {
        self.gb.save_state()
    }

    pub fn load_state(&mut self, state: SaveState) -> Result<(), &'static str> {
        self.gb.load_state(state)
    }

    pub fn reset(&mut self) {
        self.gb.reset();
    }

    /// Expose the currently mapped ROM bank number.
    pub fn current_rom_bank(&self) -> usize {
        self.gb.current_rom_bank()
    }

    /// Snapshot the emulator state into the global crash context.
    ///
    /// Called once per frame from the running state so that the fault handler
    /// always has a recent (≤ 1 frame stale) copy of the emulator state.
    ///
    /// `rom_id_prefix` is the first 4 bytes of the ROM's SHA-256 hash.
    pub fn update_crash_context(&self, rom_id_prefix: [u8; 4]) {
        let regs = self.gb.registers();
        let cycle_lo = self.gb.cycle_counter() as u32;
        let rom_bank = self.gb.current_rom_bank() as u16;
        let ppu_ly = SHARED_WORKER_STATE.ppu_ly.load(Ordering::Relaxed);
        let ppu_stat = SHARED_WORKER_STATE.ppu_stat.load(Ordering::Relaxed);

        CRASH_CONTEXT.update(
            rom_id_prefix,
            rom_bank,
            0, // ram_bank: not directly accessible without cartridge ref
            regs.a,
            regs.f.bits(),
            regs.b,
            regs.c,
            regs.d,
            regs.e,
            regs.h,
            regs.l,
            regs.sp,
            regs.pc,
            cycle_lo,
            ppu_ly,
            ppu_stat,
        );
    }

    /// Flush pending commands and halt core1 in a WFE loop.
    ///
    /// After this returns, core1 will never read from flash again, making it
    /// safe to erase and reprogram the ROM staging area.
    pub fn halt(&mut self) {
        let transport = self.gb.transport_mut();
        transport.flush_pending_apu();
        transport.flush_pending_ppu();
        let ticket = transport.issue_ticket();
        transport.enqueue_blocking(Core1Command::Halt { ticket });
        transport.wait_for_ticket(ticket);
    }
}

/// Hash a single 160-byte GB scanline using the Knuth multiplicative hash.
///
/// 160 bytes / 4 bytes per word = 40 words — no remainder.  Runs on Core 1
/// during `publish_frame` to detect which scanlines changed.  At 300 MHz the
/// full 144-row dirty-detection pass costs ~115 µs (≈ 5,760 hash iterations).
#[cfg_attr(target_arch = "arm", link_section = ".data")]
#[inline(always)]
fn hash_raw_row(row: &[u8]) -> u32 {
    const M: u32 = 0x9e37_79b9;
    // GB_ROW_BYTES = 160; 160 / 4 = 40 words, exact — chunks_exact is lossless.
    row.chunks_exact(4).fold(0xdead_beef_u32, |mut h, chunk| {
        let w = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        h ^= w;
        h = h.wrapping_mul(M);
        h ^= h >> 16;
        h
    })
}

#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn publish_worker_state(shared: &'static SharedWorkerState, worker: &mut GameBoyWorker) {
    let state = worker.poll_output();
    if state.frame_ready {
        shared.publish_frame(worker);
    }
    shared.publish_worker_output(state);
}

#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn run_core1_worker(
    command_rx: &'static MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY>,
    audio_tx: &'static MpMcQueue<i16, AUDIO_QUEUE_CAPACITY>,
    shared: &'static SharedWorkerState,
    mut worker: &'static mut GameBoyWorker,
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
                let ppu_cycles = match worker_command {
                    WorkerCommand::AdvancePpu { cycles } => cycles as u32,
                    _ => 0,
                };
                if ppu_cycles != 0 {
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
            Core1Command::DrainAudio { ticket } => {
                worker.drain_audio_samples_to(|sample| {
                    let _ = audio_tx.enqueue(sample);
                });
                shared.sync_complete.store(ticket, Ordering::Release);
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
            Core1Command::Halt { ticket } => {
                shared.sync_complete.store(ticket, Ordering::Release);
                loop {
                    asm::wfe();
                }
            }
        }
    }
}
