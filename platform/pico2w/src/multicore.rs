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
use heapless::spsc;
use rustyboy_core::cpu::cpu::CpuError;
use rustyboy_core::cpu::peripheral::joypad::Button;
use rustyboy_core::cpu::peripheral::ppu::{PpuMode, FRAMEBUFFER_SIZE};
use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::save_state::{PpuState, SaveState};
use rustyboy_core::gameboy::GameBoy;
use rustyboy_core::ipc::{GameBoyWorker, WorkerCommand, WorkerOutput, WorkerTransport};
use rustyboy_core::memory::cartridge::Cartridge;
use rustyboy_core::memory::memory::{Error as MemoryError, GameBoyMemory};
use static_cell::StaticCell;

use crate::crash::CRASH_CONTEXT;
use crate::display::NativeFrame;

const CORE1_STACK_SIZE: usize = 8192;
const COMMAND_QUEUE_CAPACITY: usize = 64;
const AUDIO_QUEUE_CAPACITY: usize = 2048;
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
// Cross-core channels — both are heapless spsc::Queue split into a Producer
// (sender) and Consumer (receiver) at init time.
//
// COMMAND_QUEUE (core 0 → core 1): spsc::Queue uses only Release/Acquire
// atomic loads and stores — no LDREX/STREX CAS, no SIO spinlock needed.
// The ticket handshake (sync_complete) is made globally visible by the explicit
// DSB in `ack_ticket`, which drains core 1's write buffer before it returns to
// the dequeue loop and potentially sleeps on WFE.
//
// AUDIO_QUEUE (core 1 → core 0): the ticket handshake serializes producer and
// consumer (core 1 fills it inside DrainAudio, then core 0 drains it only after
// the ticket lands), so the ordering is guaranteed by sync_complete's
// Release/Acquire pair — whoever observes the ticket also observes all
// preceding audio enqueues.
//
// spsc::Queue<T, N> has capacity N-1; both statics allocate N+1 slots.
static COMMAND_QUEUE: StaticCell<spsc::Queue<Core1Command, { COMMAND_QUEUE_CAPACITY + 1 }>> =
    StaticCell::new();
static AUDIO_QUEUE: StaticCell<spsc::Queue<i16, { AUDIO_QUEUE_CAPACITY + 1 }>> = StaticCell::new();
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

/// Dirty-row bitmap for one native frame slot: 1 bit per GB scanline
/// (144 rows ⟹ 5 × u32 = 160 bits).
///
/// Core 1 writes only the bitmap belonging to its selected non-busy slot.
/// Core 0 reserves the published slot before reading that slot's bitmap and
/// holds the reservation until display DMA is complete.
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

// Safety: access follows the same per-slot publication and busy ownership
// protocol as SharedNativeFrameSlot.
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
    /// Dirty-row metadata travels with each frame slot so a later publication
    /// cannot rewrite the bitmap while Core 0 is displaying an older frame.
    dirty_rows: [SharedDirtyBitmap; NATIVE_FRAME_SLOT_COUNT],
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
            dirty_rows: [
                SharedDirtyBitmap::new(),
                SharedDirtyBitmap::new(),
                SharedDirtyBitmap::new(),
            ],
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
        // `prev_row_hashes`. The target slot is neither published nor busy, so
        // its frame and dirty bitmap have no concurrent reader.
        unsafe {
            let hashes = &mut *self.prev_row_hashes.as_mut_ptr();
            let dirty = &mut *self.dirty_rows[target_slot].as_mut_ptr();
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
            for dirty in &self.dirty_rows {
                (*dirty.as_mut_ptr()).fill(0);
            }
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

/// The write-once, immutable pointer/address fields of `Core1Transport`, grouped
/// into one 32-byte aligned block so a single Core-0 priv-RO MPU region can cover
/// all of them at once (bug #5 writer identification — `transport-immutable-mpu`).
///
/// These five fields total 20 bytes (5 × 4 on 32-bit); `align(32)` pads the block
/// to exactly 32 bytes and guarantees 32-byte alignment for the MPU region. None
/// of these fields is ever written after `Core1Transport::new` constructs the
/// transport, so a Core-0 store anywhere inside this block is a wild store — the
/// exact corruptor we are hunting.
struct TransportImmutable {
    command_tx: spsc::Producer<'static, Core1Command>,
    audio_rx: spsc::Consumer<'static, i16>,
    shared: &'static SharedWorkerState,
}

struct Core1Transport {
    imm: TransportImmutable,
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
        let cq = COMMAND_QUEUE.init(spsc::Queue::new());
        let (command_tx, command_rx) = cq.split();
        let q = AUDIO_QUEUE.init(spsc::Queue::new());
        let (audio_tx, audio_rx) = q.split();
        // Safety: CORE1_STACK is a static mut accessed exactly once here during
        // init, before spawn_core1 transfers ownership to the core 1 thread.
        let core1_stack = unsafe { &mut *addr_of_mut!(CORE1_STACK) };
        // Safety: CORE1_WORKER is MaybeUninit; init_in_place writes it exactly
        // once here before core 1 starts, so there is no concurrent access.
        let worker = unsafe { GameBoyWorker::init_in_place(CORE1_WORKER.as_mut_ptr()) };

        let shared_for_core1 = shared;
        multicore::spawn_core1(core1, core1_stack, move || {
            run_core1_worker(command_rx, audio_tx, shared_for_core1, worker)
        });

        // Diagnostic note (2026-06-11): ACCESSCTRL.sram(8/9) caused PRECISERR
        // (scratch SRAM banks bypass ACCESSCTRL).  MPU on core 0 showed zero
        // violations after 5+ hours — core 0 is NOT the writer.
        // Decision-tree result: corruptor is core-1 wild execution or DMA.
        // DSB-only build is now running to isolate DSB vs MPU as the suppressor.

        Self {
            imm: TransportImmutable {
                command_tx,
                audio_rx,
                shared,
            },
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
            match self.imm.command_tx.enqueue(command) {
                Ok(()) => {
                    // DSB: drain write buffer so core 1 sees the queued data
                    // before the SEV wakes it; without this the Release store
                    // can sit in the write buffer and core 1 reads a stale
                    // empty-queue state → both cores sleep → WatchdogTimeout.
                    asm::dsb();
                    asm::sev();
                    return;
                }
                Err(returned) => {
                    // Queue full — sleep until core 1 dequeues a slot and SEVs.
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
        while self.imm.shared.sync_complete.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
        }
    }

    fn published_native_frame(&mut self) -> &'static NativeFrame {
        if self.held_frame_slot != u8::MAX {
            self.release_native_frame();
        }
        // Producer slot selection uses this same critical section. Reserving
        // the published slot while holding it closes the old load-then-busy
        // window where Core 1 could select and overwrite the slot first.
        let slot = critical_section::with(|_| {
            let slot = self.imm.shared.published_frame.load(Ordering::Acquire) as usize;
            self.imm.shared.native_frame_busy[slot].store(true, Ordering::Release);
            slot
        });
        self.held_frame_slot = slot as u8;
        // Safety: slot was loaded with Acquire from published_frame and then
        // marked busy with Release; core 1 only writes to slots that are
        // neither published nor busy, so exclusive read access is guaranteed
        // until release_native_frame clears the busy flag.
        unsafe { &*self.imm.shared.native_frame_slots[slot].as_ptr() }
    }

    fn release_native_frame(&mut self) {
        if self.held_frame_slot == u8::MAX {
            return;
        }
        self.imm.shared.native_frame_busy[self.held_frame_slot as usize]
            .store(false, Ordering::Release);
        self.held_frame_slot = u8::MAX;
    }

    /// Read the dirty-row bitmap belonging to the held native frame.
    ///
    /// Must be called **after** [`published_native_frame`], which performs the
    /// Acquire load on `published_frame` and reserves that exact slot.
    fn published_dirty_rows(&self) -> [u32; DIRTY_BITMAP_WORDS] {
        let slot = self.held_frame_slot as usize;
        assert!(slot < NATIVE_FRAME_SLOT_COUNT);
        // Safety: the held slot is busy, so Core 1 cannot rewrite its frame or
        // dirty metadata until release_native_frame().
        unsafe { *self.imm.shared.dirty_rows[slot].as_ptr() }
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
        self.imm.shared.write_live_vram_range(start_offset, data);
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    #[inline(always)]
    fn write_oam_range(&mut self, start_offset: u16, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        self.flush_pending_ppu();
        self.imm.shared.write_live_oam_range(start_offset, data);
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
            if let Some(sample) = self.imm.audio_rx.dequeue() {
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
            self.imm
                .shared
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
            let mut snapshots = self.imm.shared.sync_snapshot.borrow(cs).borrow_mut();
            snapshots.io.copy_from_slice(&io[..0x80]);
            snapshots.vram.copy_from_slice(&vram[..0x2000]);
            snapshots.oam.copy_from_slice(&oam[..0xA0]);
        });
        self.imm.shared.copy_live_ppu_snapshot(io, vram, oam);
        let ticket = self.issue_ticket();
        self.enqueue_blocking(Core1Command::SyncPpu { ticket });
        self.wait_for_ticket(ticket);
        self.imm.shared.clear_published_frames();
        self.last_frame_seq = 0;
    }

    fn load_ppu_state(&mut self, state: PpuState, io: &[u8], vram: &[u8], oam: &[u8]) {
        self.flush_pending_ppu();
        self.load_lcd_timing_state(state, io);
        critical_section::with(|cs| {
            let mut snapshots = self.imm.shared.sync_snapshot.borrow(cs).borrow_mut();
            snapshots.io.copy_from_slice(&io[..0x80]);
            snapshots.vram.copy_from_slice(&vram[..0x2000]);
            snapshots.oam.copy_from_slice(&oam[..0xA0]);
        });
        self.imm.shared.copy_live_ppu_snapshot(io, vram, oam);
        let ticket = self.issue_ticket();
        self.enqueue_blocking(Core1Command::LoadPpuState { ticket, state });
        self.wait_for_ticket(ticket);
        self.imm.shared.clear_published_frames();
        self.last_frame_seq = 0;
    }

    fn snapshot_ppu_state(&self, _io: &[u8]) -> PpuState {
        self.lcd_timing.to_save_state(&self.lcd_timing_io)
    }

    fn poll_output(&mut self, out: &mut [u8; FRAMEBUFFER_SIZE]) -> WorkerOutput {
        let _ = out;
        let frame_seq = self.imm.shared.published_frame_seq.load(Ordering::Acquire);
        let frame_ready = frame_seq != self.last_frame_seq;
        if frame_ready {
            self.last_frame_seq = frame_seq;
        }
        // Serialize with core 1's fetch_or (see publish_worker_output): cross-core
        // RMW via the SIO spinlock, not the unreliable per-core exclusive monitor.
        let _worker_if_bits =
            critical_section::with(|_| self.imm.shared.pending_if_bits.swap(0, Ordering::AcqRel));
        let if_bits = self.lcd_timing_if_bits;
        self.lcd_timing_if_bits = 0;
        let lcd_timing_frame_ready = self.lcd_timing_frame_ready;
        self.lcd_timing_frame_ready = false;
        WorkerOutput {
            apu_nr52: self.imm.shared.apu_nr52.load(Ordering::Acquire),
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

fn publish_worker_state(shared: &'static SharedWorkerState, worker: &mut GameBoyWorker) {
    let state = worker.poll_output();
    if state.frame_ready {
        shared.publish_frame(worker);
    }
    shared.publish_worker_output(state);
}

/// Store `ticket` into `sync_complete` with Release semantics, then issue a DSB
/// to drain the write buffer. Without the DSB, the store can sit in core 1's
/// write buffer while the core returns to the dequeue loop and sleeps on WFE —
/// and WFE does NOT drain the write buffer on RP2350 — so core 0's
/// `wait_for_ticket` spin would observe a stale value and deadlock.
#[cfg_attr(target_arch = "arm", link_section = ".data")]
#[inline(always)]
fn ack_ticket(shared: &SharedWorkerState, ticket: u32) {
    shared.sync_complete.store(ticket, Ordering::Release);
    cortex_m::asm::dsb();
}

#[cfg_attr(target_arch = "arm", link_section = ".data")]
/// Configure Core 1's PMSAv8-M MPU to mark Core 0's stack as privileged-read-only.
///
/// Region 0: 0x20066B60–0x2007FFFF, AP=10 (priv RO), XN=1, SH=inner-shareable.
/// PRIVDEFENA=1 leaves all other addresses with full default access.
///
/// When Core 1 writes anywhere in this range → MemManage fault (CFSR.DACCVIOL=1,
/// MMFAR=faulting address) → escalates to HardFault → existing handler records
/// stacked PC = the exact corrupt store instruction.
unsafe fn setup_core1_mpu() {
    const MPU_TYPE: *mut u32 = 0xE000_ED90 as *mut u32;
    const MPU_CTRL: *mut u32 = 0xE000_ED94 as *mut u32;
    const MPU_RNR: *mut u32 = 0xE000_ED98 as *mut u32;
    const MPU_RBAR: *mut u32 = 0xE000_ED9C as *mut u32;
    const MPU_RLAR: *mut u32 = 0xE000_EDA0 as *mut u32;
    const MPU_MAIR0: *mut u32 = 0xE000_EDC0 as *mut u32;

    let dregion = (MPU_TYPE.read_volatile() >> 8) & 0xFF;
    defmt::info!("core1 MPU setup: DREGION={=u32}", dregion);
    if dregion == 0 {
        defmt::warn!("core1 MPU: no MPU regions present — protection disabled");
        return;
    }

    // Disable before reconfiguring.
    MPU_CTRL.write_volatile(0);
    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    // Attr 0 = Normal memory, outer+inner write-back, read/write-allocate (0xFF).
    MPU_MAIR0.write_volatile(0xFF);

    // Region 0: Core 0 stack [_stack_end, _stack_start]. Derive the base from the
    // linker symbol (NOT a hardcode) so the region tracks SRAM layout shifts. §G11:
    // a stale hardcode (0x20066B60) sat ~1 KB below the real stack bottom, inside
    // the defmt RTT buffer (0x20066b3c–0x20066f3c); after the copy_dma_step .data
    // fix grew SRAM, legit core-1 RTT logging wrote into the covered range and
    // crash-looped on MMFAR DACCVIOL. Aligning the base DOWN to the 32-byte MPU
    // granule keeps it above the RTT buffer (which lives below _stack_end in
    // .uninit) while still covering the entire core-0 stack.
    extern "C" {
        static _stack_end: u32;
    }
    let stack_bottom = core::ptr::addr_of!(_stack_end) as u32;
    let base = stack_bottom & !0x1F; // 32-byte aligned MPU region base
    //   RBAR: BASE | SH=11(b4:3) | AP=10(b2:1) | XN=1(b0) = base | 0x1D
    //   RLAR: LIMIT=0x2007FFE0 | AttrIndx=0(b3:1) | EN=1(b0) = 0x2007FFE1
    MPU_RNR.write_volatile(0);
    MPU_RBAR.write_volatile(base | 0x1D);
    MPU_RLAR.write_volatile(0x2007_FFE1);
    defmt::info!("core1 MPU region 0 base (from _stack_end) = 0x{=u32:08x}", base);

    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    // Enable: PRIVDEFENA(b2)=1 background full-access for non-covered addresses,
    // HFNMIENA(b1)=0 MPU off in HardFault so our handler can read crash info,
    // ENABLE(b0)=1.
    MPU_CTRL.write_volatile(0x0000_0005);

    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    defmt::info!("core1 MPU armed: region 0 = [0x20066B60, 0x2007FFFF] priv-RO (Core 0 stack)");
}

fn run_core1_worker(
    mut command_rx: spsc::Consumer<'static, Core1Command>,
    mut audio_tx: spsc::Producer<'static, i16>,
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
    // their CFSR shows bus faults, not STKOF.  See docs/investigations/crash-debug-notes.md.)
    // Safety: __core1_stack_limit is a linker symbol; we only need its address.
    unsafe {
        unsafe extern "C" {
            static __core1_stack_limit: u32;
        }
        cortex_m::register::msplim::write(core::ptr::addr_of!(__core1_stack_limit) as u32);
    }

    // Arm MPU region 0 on Core 1 to make Core 0's stack read-only.
    // Any write by Core 1 to 0x20066B60–0x2007FFFF fires MemManage (escalates
    // to HardFault since MEMFAULTENA=0), capturing stacked PC = exact corrupt
    // store instruction.  PRIVDEFENA=1 keeps all other memory fully accessible.
    unsafe { setup_core1_mpu() };

    let mut last_ppu_render_version = 0u32;

    loop {
        let Some(command) = command_rx.dequeue() else {
            // DSB: drain write buffer before sleeping so core 0's enqueue
            // write is visible; without this a late enqueue can arrive after
            // WFE with no SEV to wake us → both cores sleep → WatchdogTimeout.
            asm::dsb();
            asm::wfe();
            continue;
        };
        // We just freed a queue slot. Wake core 0 in case it's parked in
        // `enqueue_blocking`'s WFE waiting for space.
        asm::sev();

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
                ack_ticket(shared, ticket);
            }
            Core1Command::SyncApu { ticket } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.sync_apu_state(&snapshots.io);
                });
                publish_worker_state(shared, &mut worker);
                ack_ticket(shared, ticket);
            }
            Core1Command::SyncPpu { ticket } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.sync_ppu_state(&snapshots.io, &snapshots.vram, &snapshots.oam);
                });
                last_ppu_render_version = 0;
                publish_worker_state(shared, &mut worker);
                ack_ticket(shared, ticket);
            }
            Core1Command::LoadPpuState { ticket, state } => {
                critical_section::with(|cs| {
                    let snapshots = shared.sync_snapshot.borrow(cs).borrow();
                    worker.load_ppu_state(state, &snapshots.io, &snapshots.vram, &snapshots.oam);
                });
                last_ppu_render_version = 0;
                publish_worker_state(shared, &mut worker);
                ack_ticket(shared, ticket);
            }
            Core1Command::Halt { ticket } => {
                ack_ticket(shared, ticket);
                loop {
                    asm::wfe();
                }
            }
        }
    }
}
