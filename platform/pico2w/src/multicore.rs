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

use crate::crash::{CRASH_CONTEXT, TRANSPORT_SMASH_DIAG};
use crate::display::NativeFrame;
use crate::stack_probe;

unsafe extern "C" {
    fn rustyboy_core1_pointer_guard(
        site: u32,
        shared: u32,
        worker: u32,
        want_shared: u32,
        want_worker: u32,
    ) -> !;

    fn rustyboy_live_ppu_borrow_guard(
        site: u32,
        borrow_word: u32,
        render_version: u32,
        shared: u32,
        worker: u32,
    ) -> !;

    fn rustyboy_gameboy_memory_pointer_guard(
        site: u32,
        gameboy: u32,
        memory: u32,
        want_memory: u32,
        field_addr: u32,
    ) -> !;
}

const CORE1_STACK_SIZE: usize = 8192;
// heapless 0.8 `MpMcQueue` still narrows sequence deltas to i8 under
// `mpmc_large`; keep this queue small enough that wrapped generations stay
// unambiguous. Audio needs deeper buffering, so it uses `AudioQueue` below.
const COMMAND_QUEUE_CAPACITY: usize = 64;
const AUDIO_QUEUE_CAPACITY: usize = 2048;
const _: () = assert!(COMMAND_QUEUE_CAPACITY <= 64);
const _: () = assert!(AUDIO_QUEUE_CAPACITY.is_power_of_two());
const PPU_ADVANCE_BATCH_CYCLES: u16 = 456;
const LCD_TIMING_BATCH_CYCLES: u16 = 80;
const NATIVE_FRAME_SLOT_COUNT: usize = 3;
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
// Cross-core channels.
//
// COMMAND_QUEUE (core 0 → core 1): `MpMcQueue` is lock-free via LDREX/STREX CAS,
// but the RP2350's two M33 cores have per-core exclusive monitors with no global
// arbitration for SRAM, so concurrent enqueue/dequeue can corrupt the queue
// indices. Every access is therefore serialized with the SIO hardware spinlock
// (via `critical_section`). That spinlock's acquire/release barriers ALSO drain
// each core's write buffer, which is what makes the `sync_complete` ticket
// handshake below visible across cores — do not "optimize" the spinlock away
// without replacing those barriers (a lock-free SPSC queue removes them and the
// handshake deadlocks: core 1's ack store sits in its write buffer while it
// sleeps, and core 0 spins forever on the stale value).
//
// AUDIO_QUEUE (core 1 → core 0): the ticket handshake serializes producer and
// consumer (core 1 fills it inside DrainAudio, then core 0 drains it only after
// the ticket lands), so there is never concurrent access — no spinlock needed.
static COMMAND_QUEUE: MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY> = MpMcQueue::new();
static AUDIO_QUEUE: AudioQueue = AudioQueue::new();
#[unsafe(link_section = ".core1_stack")]
static mut CORE1_STACK: Stack<CORE1_STACK_SIZE> = Stack::new();
static CORE1_WORKER: StaticStorage<GameBoyWorker> = StaticStorage::new();
static EXPECTED_WORKER_PPU_STATE_PTR: AtomicU32 = AtomicU32::new(0);
static EXPECTED_GAMEBOY_MEMORY_PTR: AtomicU32 = AtomicU32::new(0);
static DWT_WATCH_ADDRS_LOGGED: AtomicBool = AtomicBool::new(false);

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

/// Triple-buffered raw native frame slot (160×144 palette indices, 23 KB each).
///
/// Core 1 copies the PPU framebuffer into an unpublished slot and then flips
/// `published_frame`.  Core 0 reads the published slot to pre-scale and DMA to
/// the display.  With native slots (3 × 23 KB = 69 KB) instead of pre-scaled
/// slots (3 × 101 KB = 303 KB) we save 234 KB in `SHARED_WORKER_STATE`, of
/// which 101 KB is re-spent on the static `CORE0_SCALE_BUF` in `hw.rs`,
/// for a net saving of 133 KB.
struct SharedNativeFrameSlot(UnsafeCell<NativeFrame>);

impl SharedNativeFrameSlot {
    const fn new() -> Self {
        Self(UnsafeCell::new([0u8; FRAMEBUFFER_SIZE]))
    }

    fn as_ptr(&self) -> *const NativeFrame {
        self.0.get() as *const NativeFrame
    }

    fn as_mut_ptr(&self) -> *mut NativeFrame {
        self.0.get()
    }
}

// Safety: access is synchronized by the published-frame atomics; core 1 only
// writes the next unpublished slot and only flips the published slot after the
// writes complete.
unsafe impl Sync for SharedNativeFrameSlot {}

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

struct AudioQueue {
    inner: UnsafeCell<AudioQueueInner>,
}

struct AudioQueueInner {
    samples: [i16; AUDIO_QUEUE_CAPACITY],
    head: usize,
    len: usize,
}

unsafe impl Sync for AudioQueue {}

impl AudioQueue {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(AudioQueueInner {
                samples: [0; AUDIO_QUEUE_CAPACITY],
                head: 0,
                len: 0,
            }),
        }
    }

    #[inline(always)]
    fn enqueue(&self, sample: i16) -> Result<(), i16> {
        let inner = unsafe { &mut *self.inner.get() };
        if inner.len == AUDIO_QUEUE_CAPACITY {
            return Err(sample);
        }
        let tail = (inner.head + inner.len) & (AUDIO_QUEUE_CAPACITY - 1);
        inner.samples[tail] = sample;
        inner.len += 1;
        Ok(())
    }

    #[inline(always)]
    fn dequeue(&self) -> Option<i16> {
        let inner = unsafe { &mut *self.inner.get() };
        if inner.len == 0 {
            return None;
        }
        let sample = inner.samples[inner.head];
        inner.head = (inner.head + 1) & (AUDIO_QUEUE_CAPACITY - 1);
        inner.len -= 1;
        Some(sample)
    }
}

struct SharedWorkerState {
    sync_snapshot: Mutex<RefCell<PpuSnapshot>>,
    live_ppu_snapshot: Mutex<RefCell<PpuSnapshot>>,
    native_frame_slots: [SharedNativeFrameSlot; NATIVE_FRAME_SLOT_COUNT],
    native_frame_busy: [AtomicBool; NATIVE_FRAME_SLOT_COUNT],
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
            native_frame_slots: [
                SharedNativeFrameSlot::new(),
                SharedNativeFrameSlot::new(),
                SharedNativeFrameSlot::new(),
            ],
            native_frame_busy: [
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
        // Mutual exclusion with `clear_published_frames` (core 0, save-state /
        // reset path). Both write `native_frame_slots`, `prev_row_hashes`, and
        // the published-frame atomics; the SIO spinlock is the only thing that
        // actually prevents the cross-core race. The old "only accessed from
        // core 1" comment was a breakable contract — `load_ppu_state` violated
        // it while core 1 was live. Held ~once per frame (gated by frame_ready).
        critical_section::with(|_| self.publish_frame_locked(worker));
    }

    fn publish_frame_locked(&self, worker: &GameBoyWorker) {
        let current_slot = self.published_frame.load(Ordering::Acquire) as usize;
        // Scan all slots for a free one that isn't currently being consumed.
        // All initial values are zero so SharedWorkerState stays in .bss.
        let Some(target_slot) = (0..NATIVE_FRAME_SLOT_COUNT).find(|&slot| {
            slot != current_slot && !self.native_frame_busy[slot].load(Ordering::Acquire)
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
        // # Why `unsafe`
        //
        // `SharedPrevRowHashes` and `SharedDirtyBitmap` wrap `UnsafeCell<T>`,
        // whose `get()` method returns a raw `*mut T`.  Dereferencing a raw
        // pointer requires `unsafe` because the compiler cannot verify aliasing
        // rules automatically — that responsibility falls on us.
        //
        // The aliasing invariant we are upholding: `publish_frame` is only ever
        // called from Core 1's `run_core1_worker` loop, and Core 0 never reads
        // `prev_row_hashes`.  Core 0 does read `dirty_rows`, but only after
        // the `published_frame.store(Release)` below; the Acquire load on Core 0
        // side (in `published_native_frame`) establishes a happens-before edge
        // that makes the writes here visible before they are read.  No two
        // threads can hold a mutable reference to the same data simultaneously.
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

        // Copy the raw native framebuffer into the target slot.  Scaling to
        // Rgb565 now happens on Core 0 (in GameDisplay::send_frame via
        // CORE0_SCALE_BUF), which preserves the DMA/emulation overlap.
        //
        // Safety: called only from Core 1; target_slot is neither the currently
        // published slot nor marked busy, so Core 0 cannot be reading it.
        unsafe {
            (*self.native_frame_slots[target_slot].as_mut_ptr()).copy_from_slice(framebuffer);
        }
        // dirty_rows is fully written above; the Release store below pairs with
        // the Acquire load in published_native_frame() on Core 0, ensuring the
        // dirty bitmap is visible before Core 0 reads it.
        self.published_frame
            .store(target_slot as u8, Ordering::Release);
        self.published_frame_seq.fetch_add(1, Ordering::AcqRel);
    }

    fn clear_published_frames(&self) {
        // Serialize against core 1's `publish_frame` via the SIO spinlock. This
        // runs on core 0 from `sync_ppu_state` / `load_ppu_state` *while core 1
        // is live*, so the prior "called during reset before core 1 is
        // re-spawned" assumption does not hold — real mutual exclusion is
        // required, not a contract.
        critical_section::with(|_| self.clear_published_frames_locked());
    }

    fn clear_published_frames_locked(&self) {
        for slot in &self.native_frame_slots {
            // Safety: called during reset before core 1 is re-spawned, so no
            // concurrent access to any slot is possible.
            unsafe {
                (*slot.as_mut_ptr()).fill(0);
            }
        }
        for busy in &self.native_frame_busy {
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
            // Both cores RMW pending_if_bits (core 1 fetch_or here, core 0 swap in
            // poll_output). The RP2350's per-core exclusive monitors make cross-core
            // LDREX/STREX RMW unreliable, so serialize with the SIO spinlock — same
            // hazard class as the command queue.
            critical_section::with(|_| {
                self.pending_if_bits
                    .fetch_or(state.if_bits, Ordering::AcqRel)
            });
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
    audio_rx: &'static AudioQueue,
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
        EXPECTED_WORKER_PPU_STATE_PTR.store(
            worker.ppu_state_ptr_for_diagnostics() as u32,
            Ordering::Release,
        );

        // Paint the WHOLE core 1 stack with the sentinel before the thread
        // starts (no concurrent access yet) so the worker loop can later read a
        // true high-water mark, not just a bottom-of-stack tripwire.
        // Safety: CORE1_STACK is exactly CORE1_STACK_SIZE bytes, owned here.
        unsafe {
            stack_probe::paint_region(
                core::ptr::addr_of_mut!(CORE1_STACK) as *mut u8,
                CORE1_STACK_SIZE,
            );
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

    /// #5 core-0 tripwire: `self.shared` is `&'static SharedWorkerState`, a
    /// fixed address. The crash-loop's dominant fault is an UNALIGNED atomic
    /// load through a *corrupted* `self.shared` (gameboy.rs:214 →
    /// read_worker_output → poll_output). Reading the field's bits and comparing
    /// (no deref) catches the smash at the first transport call after it, with a
    /// label identifying which tick sub-step ran just before — so the
    /// deterministic save-state repro bisects the writer. Cheap; keep on.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn check_shared(&self, label: &'static str) {
        // Validate all three `&'static` pointer fields. In the optimized ARM
        // layout, `pending_ppu` is before them, so this checked triplet starts
        // at transport offset 4 (command_tx@4, audio_rx@8, shared@12). The #5
        // wild write smashes this contiguous region; checking only `shared`
        // missed the cases where it lands on command_tx/audio_rx (enqueue
        // HardFault).
        let cmd = self.command_tx as *const _ as usize;
        let aud = self.audio_rx as *const _ as usize;
        let shr = self.shared as *const SharedWorkerState as usize;
        let want_cmd = core::ptr::addr_of!(COMMAND_QUEUE) as usize;
        let want_aud = core::ptr::addr_of!(AUDIO_QUEUE) as usize;
        let want_shr = core::ptr::addr_of!(SHARED_WORKER_STATE) as usize;
        if cmd != want_cmd || aud != want_aud || shr != want_shr {
            // Out-of-line cold handler: keeps THIS (inlined) hot path minimal so
            // the stack layout barely changes — earlier in-situ dumping bloated
            // the inlined path and suppressed the (layout-sensitive) overrun.
            report_transport_smash(label, self as *const _ as usize, cmd, aud, shr);
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn enqueue_blocking(&mut self, command: Core1Command) {
        let mut command = command;
        loop {
            // Serialize the enqueue with the SIO spinlock (see COMMAND_QUEUE):
            // the lock-free CAS is not cross-core safe on RP2350, and the
            // spinlock's barriers also publish the ticket handshake.
            match critical_section::with(|_| self.command_tx.enqueue(command)) {
                Ok(()) => {
                    // Wake core 1 if it's parked in WFE on an empty queue.
                    asm::sev();
                    return;
                }
                Err(returned) => {
                    // Queue full — core 1 hasn't drained it yet. Sleep on WFE
                    // instead of busy-retrying: a tight retry loop re-takes the
                    // spinlock so fast it STARVES core 1's (also-spinlock-guarded)
                    // dequeue, so the queue never drains and both cores livelock
                    // until the watchdog reboots. WFE releases the core so core 1
                    // can take the spinlock; it SEVs after every dequeue to wake
                    // us, and any interrupt also wakes WFE, so we can't miss it.
                    command = returned;
                    asm::wfe();
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
        // #5: a smashed `self.shared` makes this spin on a garbage address that
        // never matches `ticket` → watchdog hang (the observed standalone
        // WatchdogTimeout). Catch it as a clean panic before the spin instead.
        self.check_shared("wait_for_ticket");
        while self.shared.sync_complete.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
        }
    }

    fn published_native_frame(&mut self) -> &'static NativeFrame {
        self.check_shared("published_native_frame");
        if self.held_frame_slot != u8::MAX {
            self.release_native_frame();
        }
        let slot = self.shared.published_frame.load(Ordering::Acquire) as usize;
        self.shared.native_frame_busy[slot].store(true, Ordering::Release);
        self.held_frame_slot = slot as u8;
        // Safety: slot was loaded with Acquire from published_frame and then
        // marked busy with Release; core 1 only writes to slots that are
        // neither published nor busy, so exclusive read access is guaranteed
        // until release_native_frame clears the busy flag.
        unsafe { &*self.shared.native_frame_slots[slot].as_ptr() }
    }

    fn release_native_frame(&mut self) {
        if self.held_frame_slot == u8::MAX {
            return;
        }
        self.shared.native_frame_busy[self.held_frame_slot as usize]
            .store(false, Ordering::Release);
        self.held_frame_slot = u8::MAX;
    }

    /// Read the dirty-row bitmap for the most recently published frame.
    ///
    /// Must be called **after** [`published_native_frame`], which performs the
    /// Acquire load on `published_frame`.  That Acquire pairs with the Release
    /// store Core 1 did after writing the bitmap, so the bitmap is fully
    /// visible by the time this returns.
    fn published_dirty_rows(&self) -> [u32; DIRTY_BITMAP_WORDS] {
        // Safety: dirty_rows was written by Core 1 before the Release store to
        // published_frame; published_native_frame() loaded published_frame with
        // Acquire, establishing the happens-before edge.
        unsafe { *self.shared.dirty_rows.as_ptr() }
    }
}

impl WorkerTransport for Core1Transport {
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn send(&mut self, command: WorkerCommand) {
        self.check_shared("send");
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
        self.check_shared("write_vram_range");
        if data.is_empty() {
            return;
        }
        self.flush_pending_ppu();
        self.shared.write_live_vram_range(start_offset, data);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]) {
        self.check_shared("write_oam_range");
        if data.is_empty() {
            return;
        }
        self.flush_pending_ppu();
        self.shared.write_live_oam_range(start_offset, data);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_ppu_register(&mut self, addr: u16, value: u8) {
        self.check_shared("write_ppu_register");
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
        self.check_shared("write_ppu_registers");
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
        self.check_shared("poll_output");
        let frame_seq = self.shared.published_frame_seq.load(Ordering::Acquire);
        let frame_ready = frame_seq != self.last_frame_seq;
        if frame_ready {
            self.last_frame_seq = frame_seq;
        }
        // Serialize with core 1's fetch_or (see publish_worker_output): cross-core
        // RMW via the SIO spinlock, not the unreliable per-core exclusive monitor.
        let _worker_if_bits =
            critical_section::with(|_| self.shared.pending_if_bits.swap(0, Ordering::AcqRel));
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
        let memory_field = self.gb.memory_box_field_addr_for_diagnostics();
        let memory = self.gb.memory_ptr_for_diagnostics();
        let expected = EXPECTED_GAMEBOY_MEMORY_PTR.load(Ordering::Acquire);
        if expected == 0 {
            EXPECTED_GAMEBOY_MEMORY_PTR.store(memory as u32, Ordering::Release);
        } else if memory as u32 != expected {
            unsafe {
                rustyboy_gameboy_memory_pointer_guard(
                    core::panic::Location::caller().line(),
                    &self.gb as *const GameBoy<Core1Transport> as u32,
                    memory as u32,
                    expected,
                    memory_field as u32,
                );
            }
        }

        // Core 1 publishes the active corruption-hunt watchpoints once it knows
        // its stack slots. Core 0 still needs to arm its own DWT bank so a
        // cross-core write into those slots/fields is caught.
        crate::dwt_watch::arm_published_watch_words_for_current_core();
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
    pub fn published_native_frame(&mut self) -> &'static NativeFrame {
        self.gb.transport_mut().published_native_frame()
    }

    #[inline(always)]
    pub fn release_native_frame(&mut self) {
        self.gb.transport_mut().release_native_frame();
    }

    /// Return the dirty-row bitmap for the currently held published frame.
    ///
    /// Each bit k indicates that GB scanline k changed relative to the previous
    /// published frame.  Call this **after** [`published_native_frame`] so the
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

        // #5 RCA aid: the two captured pointer-corruption crashes both froze the
        // emulator at GB PC=0x03ce (HRAM, HL=0xffaa). Log the cross-core context
        // when we sample that PC so a capture run shows core 0's stack high-water
        // and bank state at the reproducible trigger. Gated to trace so it costs
        // nothing at the default log level; raise to DEFMT_LOG=trace to see it.
        const TRIGGER_GB_PC: u16 = 0x03ce;
        if regs.pc == TRIGGER_GB_PC {
            #[cfg(feature = "stack-probe")]
            defmt::trace!(
                "RCA trigger: GB pc={=u16:#06x} bank={=u16} core0 stack high-water {=usize}B",
                regs.pc,
                rom_bank,
                stack_probe::high_water_core0(),
            );
        }

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

/// Cold, out-of-line handler for a detected transport-pointer smash (#5).
///
/// Logs the corrupted vs expected pointers, then **scans main SRAM for the
/// corrupting values** to locate the SOURCE buffer (the overrun writes the same
/// bytes there and onto the transport). Reporting the source address — other
/// than `base` (the transport itself) — fingerprints the overrunning buffer.
/// Kept `#[inline(never)]`+`#[cold]` so `check_shared`'s inlined hot path (and
/// thus the stack layout) is unchanged, avoiding the heisenbug.
#[cfg_attr(target_arch = "arm", link_section = ".data")]
#[inline(never)]
#[cold]
fn report_transport_smash(
    label: &'static str,
    base: usize,
    cmd: usize,
    aud: usize,
    shr: usize,
) -> ! {
    let want_cmd = core::ptr::addr_of!(COMMAND_QUEUE) as usize;
    let want_aud = core::ptr::addr_of!(AUDIO_QUEUE) as usize;
    let want_shr = core::ptr::addr_of!(SHARED_WORKER_STATE) as usize;
    defmt::error!(
        "core0 transport smashed before {=str} @ {=usize:#010x}: cmd={=usize:#010x} aud={=usize:#010x} shr={=usize:#010x} (want {=usize:#010x} {=usize:#010x} {=usize:#010x})",
        label,
        base,
        cmd,
        aud,
        shr,
        want_cmd,
        want_aud,
        want_shr,
    );
    let source_triplet = first_duplicate_transport_triplet(base, cmd, aud, shr);
    TRANSPORT_SMASH_DIAG.record(base, cmd, aud, shr, source_triplet);
    scan_sram_for_transport_word("cmd", 0, cmd as u32, base, cmd, aud, shr);
    scan_sram_for_transport_word("aud", 1, aud as u32, base, cmd, aud, shr);
    scan_sram_for_transport_word("shr", 2, shr as u32, base, cmd, aud, shr);
    panic!("core0 transport ptrs smashed");
}

#[inline(never)]
fn first_duplicate_transport_triplet(base: usize, cmd: usize, aud: usize, shr: usize) -> usize {
    let mut triplet = 0x2000_0000usize;
    let end = 0x2008_0000usize.saturating_sub(12);
    while triplet <= end {
        let v0 = unsafe { (triplet as *const u32).read_volatile() } as usize;
        if v0 == cmd {
            let v1 = unsafe { (triplet as *const u32).add(1).read_volatile() } as usize;
            let v2 = unsafe { (triplet as *const u32).add(2).read_volatile() } as usize;
            if v1 == aud && v2 == shr && triplet != base + 4 {
                return triplet;
            }
        }
        triplet += 4;
    }
    0
}

#[inline(never)]
fn scan_sram_for_transport_word(
    name: &'static str,
    word_index: usize,
    target: u32,
    base: usize,
    cmd: usize,
    aud: usize,
    shr: usize,
) {
    let mut addr = 0x2000_0000usize;
    let end = 0x2008_0000usize;
    let mut hits = 0u32;
    while addr < end && hits < 24 {
        // Safety: reading a u32 in the SRAM window.
        let v = unsafe { (addr as *const u32).read_volatile() };
        if v == target {
            let triplet = addr.saturating_sub(word_index * 4);
            let seq = if (0x2000_0000..=0x2007_fff8).contains(&triplet) {
                let v0 = unsafe { (triplet as *const u32).read_volatile() } as usize;
                let v1 = unsafe { (triplet as *const u32).add(1).read_volatile() } as usize;
                let v2 = unsafe { (triplet as *const u32).add(2).read_volatile() } as usize;
                (v0 == cmd && v1 == aud && v2 == shr) as u8
            } else {
                0
            };
            let class = classify_sram_match(addr, base);
            defmt::error!(
                "  {=str}-word {=u32:#010x} @ {=usize:#010x} (triplet@{=usize:#010x} 12B-match={=u8} class={=str})",
                name,
                target,
                addr,
                triplet,
                seq,
                class,
            );
            hits += 1;
        }
        addr += 4;
    }
}

#[inline(never)]
fn classify_sram_match(addr: usize, transport_base: usize) -> &'static str {
    let transport_triplet = transport_base + 4;
    let bus_event_header = transport_base - 12;
    if (transport_triplet..transport_triplet + 12).contains(&addr) {
        return "transport_ptr_triplet";
    }
    if (bus_event_header..bus_event_header + 12).contains(&addr) {
        return "bus_event_buf_header";
    }
    if (transport_base..transport_base + core::mem::size_of::<Core1Transport>()).contains(&addr) {
        return "transport";
    }

    unsafe extern "C" {
        static _stack_end: u32;
    }
    let core0_stack_bottom = core::ptr::addr_of!(_stack_end) as usize;
    let core0_stack_top = core::ptr::addr_of!(CORE1_STACK) as usize;
    if (core0_stack_bottom..core0_stack_top).contains(&addr) {
        return "core0_stack";
    }

    let core1_stack = core::ptr::addr_of!(CORE1_STACK) as usize;
    if (core1_stack..core1_stack + CORE1_STACK_SIZE).contains(&addr) {
        return "core1_stack";
    }

    "sram_static_or_allocator"
}

fn publish_worker_state(shared: &'static SharedWorkerState, worker: &mut GameBoyWorker) {
    // #5: re-check immediately before the atomic stores below. If this fires but
    // the loop-top check did not, the pointer was smashed *within* this command
    // (worker.send / sync_*), narrowing the culprit to the emulator core path.
    assert_core1_pointers(shared, worker);
    let state = worker.poll_output();
    if state.frame_ready {
        shared.publish_frame(worker);
    }
    shared.publish_worker_output(state);
}

/// CRASH_DEBUG_NOTES #5 tripwire.
///
/// The open crash smashes core 1's `shared` base pointer toward 0 and then
/// HardFaults on the next atomic store (fault addr ≈ a `SharedWorkerState`
/// field offset). `shared` and `worker` are both `&'static` to module statics
/// at fixed addresses, so any value other than those is corruption. Checking
/// once per command converts the eventual wild store fault into a *clean panic*
/// — the crash handler records it as a Panic with the bad pointer's value and
/// the GB state, telling us whether `shared`, `worker`, or neither is the
/// corrupted operand (i.e. spilled-pointer smash vs. a wild store target vs.
/// corruption inside the worker). Two pointer compares; cheap enough to keep on.
#[inline(always)]
#[track_caller]
fn assert_core1_pointers(shared: &SharedWorkerState, worker: &GameBoyWorker) {
    let shared_ptr = shared as *const SharedWorkerState as usize;
    let want_shared = core::ptr::addr_of!(SHARED_WORKER_STATE) as usize;
    let worker_ptr = worker as *const GameBoyWorker as usize;
    // Safety: comparing the address only; not dereferencing the storage.
    let want_worker = unsafe { CORE1_WORKER.as_mut_ptr() } as usize;
    if shared_ptr != want_shared || worker_ptr != want_worker {
        defmt::error!(
            "core1 ptr corruption: shared={=usize:#010x}/want {=usize:#010x} worker={=usize:#010x}/want {=usize:#010x}",
            shared_ptr,
            want_shared,
            worker_ptr,
            want_worker,
        );
        unsafe {
            rustyboy_core1_pointer_guard(
                core::panic::Location::caller().line(),
                shared_ptr as u32,
                worker_ptr as u32,
                want_shared as u32,
                want_worker as u32,
            );
        }
    }
}

#[inline(always)]
#[track_caller]
fn assert_worker_ppu_pointer(worker: &GameBoyWorker) {
    let worker_ptr = worker as *const GameBoyWorker as usize;
    let ppu_ptr = worker.ppu_state_ptr_for_diagnostics();
    let want_ppu = EXPECTED_WORKER_PPU_STATE_PTR.load(Ordering::Acquire) as usize;
    if want_ppu != 0 && ppu_ptr != want_ppu {
        defmt::error!(
            "core1 worker ppu ptr corruption: worker={=usize:#010x} ppu={=usize:#010x}/want {=usize:#010x}",
            worker_ptr,
            ppu_ptr,
            want_ppu,
        );
        unsafe {
            rustyboy_core1_pointer_guard(
                core::panic::Location::caller().line(),
                worker_ptr as u32,
                ppu_ptr as u32,
                worker_ptr as u32,
                want_ppu as u32,
            );
        }
    }
}

#[inline(never)]
#[cold]
#[track_caller]
fn report_live_ppu_snapshot_borrow_failure(
    shared: &SharedWorkerState,
    worker: &GameBoyWorker,
    cell: &RefCell<PpuSnapshot>,
) -> ! {
    let shared_ptr = shared as *const SharedWorkerState as usize;
    let worker_ptr = worker as *const GameBoyWorker as usize;
    let borrow_word =
        unsafe { (cell as *const RefCell<PpuSnapshot> as *const u32).read_volatile() };
    let render_version = shared.ppu_render_version.load(Ordering::Relaxed);
    defmt::error!(
        "live_ppu_snapshot borrow failed: borrow={=u32:#010x} render_ver={=u32} shared={=usize:#010x} worker={=usize:#010x}",
        borrow_word,
        render_version,
        shared_ptr,
        worker_ptr,
    );
    unsafe {
        rustyboy_live_ppu_borrow_guard(
            core::panic::Location::caller().line(),
            borrow_word,
            render_version,
            shared_ptr as u32,
            worker_ptr as u32,
        );
    }
}

#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn run_core1_worker(
    command_rx: &'static MpMcQueue<Core1Command, COMMAND_QUEUE_CAPACITY>,
    audio_tx: &'static AudioQueue,
    shared: &'static SharedWorkerState,
    mut worker: &'static mut GameBoyWorker,
) -> ! {
    // Re-assert the ARMv8-M hardware stack-limit guard for core 1.  embassy-rp's
    // spawn_core1 already arms MSPLIM in core1_startup for the stack we passed;
    // we set it again here on core 1's own MSP so the invariant is explicit and
    // greppable.  Any push below the limit raises a STKOF UsageFault that
    // escalates to the HardFault handler at the offending instruction instead of
    // growing down into core 0's stack region just below 0x20080000.  (This
    // guard pre-existed, so the #5 crashes are not a core-1 stack overflow —
    // their CFSR shows bus faults, not STKOF.  See CRASH_DEBUG_NOTES.md.)
    // Safety: __core1_stack_limit is a linker symbol; we only need its address.
    unsafe {
        unsafe extern "C" {
            static __core1_stack_limit: u32;
        }
        cortex_m::register::msplim::write(core::ptr::addr_of!(__core1_stack_limit) as u32);
    }

    let mut last_ppu_render_version = 0u32;
    let core1_stack_bottom = core::ptr::addr_of!(CORE1_STACK) as *const u8;
    let worker_ptr_slot = core::ptr::addr_of!(worker) as *const _ as usize;
    let shared_ptr_slot = core::ptr::addr_of!(shared) as *const _ as usize;
    let audio_tx_ptr_slot = core::ptr::addr_of!(audio_tx) as *const _ as usize;
    let worker_ppu_field = worker.ppu_box_field_addr_for_diagnostics();
    let worker_ppu_ptr = worker.ppu_state_ptr_for_diagnostics();
    if !DWT_WATCH_ADDRS_LOGGED.swap(true, Ordering::AcqRel) {
        defmt::info!(
            "dwt watch targets: worker_slot={=usize:#010x} ppu_field={=usize:#010x} shared_slot={=usize:#010x} audio_slot={=usize:#010x} worker={=usize:#010x} ppu={=usize:#010x}",
            worker_ptr_slot,
            worker_ppu_field,
            shared_ptr_slot,
            audio_tx_ptr_slot,
            worker as *const GameBoyWorker as usize,
            worker_ppu_ptr,
        );
    }
    crate::dwt_watch::publish_and_arm_raw_words([
        worker_ptr_slot,
        worker_ppu_field,
        shared_ptr_slot,
        audio_tx_ptr_slot,
    ]);
    // Throttle the (full-stack) high-water scan so it doesn't run every command.
    #[cfg(feature = "stack-probe")]
    let mut hw_report_countdown: u32 = 0;

    loop {
        crate::dwt_watch::arm_published_watch_words_for_current_core();

        // Backstop the MSPLIM guard with the software sentinel: panic if the
        // bottom 256 bytes were ever disturbed (overflow that somehow slipped
        // past the limit check), and periodically report the high-water mark.
        unsafe { stack_probe::check_region(core1_stack_bottom, 256, "core1") };
        #[cfg(feature = "stack-probe")]
        if hw_report_countdown == 0 {
            hw_report_countdown = 32_768;
            // Safety: the whole CORE1_STACK region was painted before spawn.
            let used =
                unsafe { stack_probe::region_high_water(core1_stack_bottom, CORE1_STACK_SIZE) };
            defmt::debug!(
                "core1 stack high-water {=usize}B / {=usize}B",
                used,
                CORE1_STACK_SIZE
            );
        } else {
            hw_report_countdown -= 1;
        }

        // Serialize the dequeue with the SIO spinlock to pair with the guarded
        // enqueue on core 0 (the lock-free CAS is not cross-core safe on
        // RP2350). Critically, `critical_section::with` runs even when the queue
        // is empty, and its spinlock release barrier drains this core's write
        // buffer — including any `sync_complete` ack a handler just wrote — out
        // to coherent SRAM *before* we sleep. That is what makes core 0's
        // `wait_for_ticket` spin observe the ack; without it the ack lingers in
        // the write buffer and core 0 deadlocks on the stale value.
        let Some(command) = critical_section::with(|_| command_rx.dequeue()) else {
            asm::wfe();
            continue;
        };
        // We just freed a queue slot. Wake core 0 in case it's parked in
        // `enqueue_blocking`'s WFE waiting for space.
        asm::sev();

        // #5 tripwire: catch a smashed `shared`/`worker` pointer at first use
        // (clean panic record) instead of a later wild atomic-store HardFault.
        assert_core1_pointers(shared, worker);
        assert_worker_ppu_pointer(worker);

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
                            let cell = shared.live_ppu_snapshot.borrow(cs);
                            let Ok(snapshot) = cell.try_borrow() else {
                                report_live_ppu_snapshot_borrow_failure(shared, worker, cell);
                            };
                            worker.update_ppu_render_state(&snapshot.vram, &snapshot.oam);
                        });
                        last_ppu_render_version = render_version;
                    }
                }
                assert_worker_ppu_pointer(worker);
                worker.hyhy(worker_command);
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
