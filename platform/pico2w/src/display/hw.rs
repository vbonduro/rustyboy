//! Hardware-specific display initialisation for the ACEIRMC ILI9341 module.
//!
//! Constructs a [`Display`] backed by the real SPI peripheral. Only compiled
//! for the embedded target — host builds and tests use [`super::fb::FbDisplay`]
//! instead.

use defmt::{info, warn};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH1, PIN_10, PIN_11, PIN_12, PIN_13, PIN_8, PIN_9, SPI1};
use embassy_rp::spi::{Async, Blocking, Config as SpiConfig, Spi};
use embassy_rp::Peri;
use embassy_rp::{dma, interrupt};

use display_interface_spi::SPIInterface;
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorOrder, Orientation};
use mipidsi::Builder;

use static_cell::ConstStaticCell;

use super::{
    loading_bar_window, menu_item_text_window, menu_item_window, render_loading_row,
    render_menu_row, scale_native_to_rgb565_range, Display, LoadingFrame, LoadingProgress,
    NativeFrame, RenderWindow, ScaledFrame, SCALED_FRAME_PIXELS,
};
use crate::menu::MenuFrame;

// ---------------------------------------------------------------------------
// Core 0 pre-scale buffer
//
// `send_frame` scales the dirty row range from the native GB framebuffer (23 KB)
// into this 101 KB static, then issues a single DMA burst.  Keeping the buffer
// as a `ConstStaticCell`-backed `&'static mut` field on `GameDisplay` avoids
// placing 101 KB on the stack or heap and lets the linker pack it into BSS.
//
// `ConstStaticCell` is const-initialized to all-zeros, so the buffer lands in
// `.bss` with no flash overhead.  `GameDisplay::new_after_splash` calls
// `.take()` once to claim the exclusive `&'static mut ScaledFrame`; after that
// `&mut self.scale_buf` in `send_frame` / `send_frame_raw` provides safe,
// uniquely-owned mutable access — no unsafe required.
// ---------------------------------------------------------------------------

static CORE0_SCALE_BUF: ConstStaticCell<ScaledFrame> =
    ConstStaticCell::new([0u16; SCALED_FRAME_PIXELS]);

// Embassy's RP2350 SPI divider picks the nearest realizable rate at or below
// the requested frequency. 62.5 MHz is exact at 250 MHz sysclk, but at 280 MHz
// it rounds down to 46.7 MHz (280 / 2 / 3), which is slow enough to expose
// display DMA wait time in release builds. Overclock profiles therefore choose
// exact divider-friendly requests that preserve the intended overlap.
#[cfg(feature = "oc-300")]
const DISPLAY_SPI_HZ: u32 = 75_000_000;
#[cfg(feature = "oc-280")]
const DISPLAY_SPI_HZ: u32 = 70_000_000;
#[cfg(all(not(feature = "oc-300"), not(feature = "oc-280")))]
const DISPLAY_SPI_HZ: u32 = 75_000_000;

// GP10=SPI1_CLK, GP11=SPI1_MOSI.  SD card uses SPI0 on GP18/GP19.
type MySpi<'d> = Spi<'d, SPI1, Blocking>;
type MySpiDev<'d> = ExclusiveDevice<MySpi<'d>, Output<'d>, embassy_time::Delay>;
type MyDi<'d> = SPIInterface<MySpiDev<'d>, Output<'d>>;
type MipiDisp<'d> = mipidsi::Display<MyDi<'d>, ILI9341Rgb565, Output<'d>>;

/// Newtype that bundles the backlight pin alongside the inner display so that
/// it stays driven high for the lifetime of the driver.
pub struct HwDisplay<'d> {
    pub inner: Display<MipiDisp<'d>>,
    _bl: Output<'d>,
}

impl<'d> HwDisplay<'d> {
    /// Initialise the ILI9341 and return a ready-to-use display.
    ///
    /// The backlight is enabled immediately after hardware init.
    pub fn new(
        spi1: Peri<'d, SPI1>,
        clk: Peri<'d, PIN_10>,
        mosi: Peri<'d, PIN_11>,
        cs_pin: Peri<'d, PIN_9>,
        dc_pin: Peri<'d, PIN_8>,
        rst_pin: Peri<'d, PIN_12>,
        bl_pin: Peri<'d, PIN_13>,
    ) -> Self {
        let mut cfg = SpiConfig::default();
        cfg.frequency = DISPLAY_SPI_HZ;

        let spi = Spi::new_blocking_txonly(spi1, clk, mosi, cfg);
        let cs = Output::new(cs_pin, Level::High);
        let dc = Output::new(dc_pin, Level::Low);
        let rst = Output::new(rst_pin, Level::High);
        let mut bl = Output::new(bl_pin, Level::Low);

        let spi_dev = ExclusiveDevice::new(spi, cs, embassy_time::Delay);
        let di = SPIInterface::new(spi_dev, dc);

        // flip_horizontal corrects the reversed scan direction on this module.
        let mipidsi_display = Builder::new(ILI9341Rgb565, di)
            .reset_pin(rst)
            .color_order(ColorOrder::Bgr)
            .orientation(Orientation::new().flip_horizontal())
            .init(&mut embassy_time::Delay)
            .unwrap();

        bl.set_high();
        info!("display: ILI9341 initialised");

        Self {
            inner: Display::from_draw_target(mipidsi_display),
            _bl: bl,
        }
    }

    /// Play the boot splash animation at ~60 fps using embassy timers.
    pub async fn splash(&mut self) {
        let mut frame = 0u32;
        loop {
            if self.inner.splash_step(frame) {
                break;
            }
            frame += 1;
            embassy_time::Timer::after(embassy_time::Duration::from_millis(16)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// GameDisplay — async SPI with raw ILI9341 protocol for the hot path
// ---------------------------------------------------------------------------

// C3 (darkest DMG palette colour, #081820) in big-endian RGB565 bytes.
// Rgb565::new(1, 6, 4).into_storage() = 0x08C4; swap_bytes → 0xC408.
// Sent MSB-first over SPI: [0x08, 0xC4] → ILI9341 decodes R=1 G=6 B=4. ✓
const C3_BE: [u8; 2] = [0x08, 0xC4];
const BLACK_BE: [u8; 2] = [0x00, 0x00];
const DISPLAY_X_END: u16 = 239;
const DISPLAY_Y_END: u16 = 319;
const GAME_Y_START: u16 = 52;
const GAME_Y_END: u16 = 267;
const TOP_BAR_Y_END: u16 = GAME_Y_START - 1;
const BOTTOM_BAR_Y_START: u16 = GAME_Y_END + 1;
const DISPLAY_ROW_PIXELS: usize = 240;
const LETTERBOX_ROWS: usize = 52;
const ROW_BYTES: usize = DISPLAY_ROW_PIXELS * 2;

pub struct GameDisplay<'d> {
    spi: Spi<'d, SPI1, Async>,
    cs: Output<'d>,
    dc: Output<'d>,
    _rst: Output<'d>,
    _bl: Output<'d>,
    /// Exclusively-owned 101 KB scale buffer, backed by the `CORE0_SCALE_BUF`
    /// BSS static.  Claimed once in `new_after_splash`; accessed via `&mut
    /// self.scale_buf` in `send_frame` / `send_frame_raw`.
    scale_buf: &'static mut ScaledFrame,
    /// Multiplicative hash of the last frame successfully sent to the display.
    ///
    /// Compared against each new frame's hash before deciding whether to
    /// issue DMA.  Identical frames skip the 11 ms pixel DMA entirely so
    /// the display refreshes its own GRAM with no scan-line race.
    /// Cost: 4 bytes (vs 103 KB for a full shadow copy).
    prev_frame_hash: u32,
    /// Hash of the last [`MenuFrame`] passed to [`draw_menu`].
    ///
    /// Hashed at the struct level (before any rendering) using the same Knuth
    /// multiplicative step as [`knuth_hash`].  If the incoming frame hashes
    /// identically to this cached value, `draw_menu` returns immediately
    /// without rendering or DMA-ing anything — the display already shows the
    /// correct content.
    ///
    /// Zeroed in `draw_letterbox_bars` so the first `draw_menu` call after a
    /// game session always performs a full repaint.  Cost: 4 bytes.
    prev_menu_frame_hash: u32,
}

impl<'d> GameDisplay<'d> {
    /// Initialise async SPI for the game loop after `HwDisplay` has been dropped.
    ///
    /// # Safety
    /// `HwDisplay` must have been fully dropped before calling this so that SPI1
    /// and all display GPIO pins are free to be re-claimed.
    pub unsafe fn new_after_splash(
        clk: Peri<'d, PIN_10>,
        mosi: Peri<'d, PIN_11>,
        cs_pin: Peri<'d, PIN_9>,
        dc_pin: Peri<'d, PIN_8>,
        rst_pin: Peri<'d, PIN_12>,
        bl_pin: Peri<'d, PIN_13>,
        spi1: Peri<'d, SPI1>,
        dma: Peri<'d, DMA_CH1>,
        irqs: impl interrupt::typelevel::Binding<
                interrupt::typelevel::DMA_IRQ_0,
                dma::InterruptHandler<DMA_CH1>,
            > + 'd,
    ) -> Self {
        // The DMA_IRQ_0 ISR now calls BOTH InterruptHandler<DMA_CH0> and
        // InterruptHandler<DMA_CH1> unconditionally (combined bind_interrupts!).
        // CH1's on_interrupt panics if ctrl_trig.ahb_error() is set. Clear any
        // stale error/pending-interrupt state before Spi::new_txonly enables
        // the CH1 interrupt in INTE0.
        let ahb_err = rp_pac::DMA.ch(1).ctrl_trig().read().ahb_error();
        if ahb_err {
            warn!("DMA CH1 ahb_error set before init — aborting to clear");
            // Write bit 1 to CHAN_ABORT to trigger a CH1 abort.
            rp_pac::DMA
                .chan_abort()
                .write_value(rp_pac::dma::regs::ChanAbort(1u32 << 1));
            while rp_pac::DMA.chan_abort().read().chan_abort() & (1u16 << 1) != 0 {}
        }
        // Clear any pending CH1 interrupt flag in INTS0 (W1C: write 1 to clear).
        rp_pac::DMA.ints(0).write_value(1u32 << 1);

        let mut cfg = SpiConfig::default();
        cfg.frequency = DISPLAY_SPI_HZ;

        let spi = Spi::new_txonly(spi1, clk, mosi, dma, irqs, cfg);
        let cs = Output::new(cs_pin, Level::High);
        let dc = Output::new(dc_pin, Level::Low);
        let rst = Output::new(rst_pin, Level::High);
        let bl = Output::new(bl_pin, Level::High);

        info!("display: async SPI re-initialised for game loop");
        Self {
            spi,
            cs,
            dc,
            _rst: rst,
            _bl: bl,
            scale_buf: CORE0_SCALE_BUF.take(),
            prev_frame_hash: 0,
            prev_menu_frame_hash: 0,
        }
    }

    /// Paint the static letterbox bars (top C3, bottom black) once before the
    /// game loop. The bars are never repainted — `send_frame` only touches the
    /// 240×216 game area.
    ///
    /// Also resets both frame-hash caches:
    /// - `prev_menu_frame_hash` → 0: next `draw_menu` always does a full repaint.
    /// - `prev_frame_hash` → 0: next `send_frame` always DMA-s the game area,
    ///   even if the native frame content is unchanged.  This is required because
    ///   `draw_menu` paints the entire 240×320 display (including the game area),
    ///   so the game area contains stale menu pixels after the menu closes; without
    ///   this reset, `send_frame` would see a matching hash and skip the DMA,
    ///   leaving menu remnants visible in the game region.
    pub async fn draw_letterbox_bars(&mut self) {
        self.prev_menu_frame_hash = 0;
        self.prev_frame_hash = 0; // force game-area DMA on first send_frame after menu
        info!("display: drawing letterbox bars");

        // Top bar: rows 0..51, colour C3
        self.set_window(RenderWindow::new(0, DISPLAY_X_END + 1, 0, TOP_BAR_Y_END + 1));
        self.write_command(0x2C, &[]);
        self.fill_rect_raw(LETTERBOX_ROWS * DISPLAY_ROW_PIXELS, &C3_BE).await;
        info!("display: top bar done");

        // Bottom bar: rows 268..319, colour black
        self.set_window(RenderWindow::new(
            0,
            DISPLAY_X_END + 1,
            BOTTOM_BAR_Y_START,
            DISPLAY_Y_END + 1,
        ));
        self.write_command(0x2C, &[]);
        self.fill_rect_raw(LETTERBOX_ROWS * DISPLAY_ROW_PIXELS, &BLACK_BE).await;
        info!("display: letterbox bars done");
    }

    /// Transfer a native Game Boy frame (160×144 palette indices) to the
    /// display, handling dirty-row detection, hash-based skip, pre-scaling,
    /// and the DMA / emulation overlap pattern.
    ///
    /// # How to use in the game loop
    ///
    /// ```ignore
    /// let mut f = core::pin::pin!(game_disp.send_frame(frame_buf, &dirty_rows));
    /// let _ = poll_once(f.as_mut());   // ← starts DMA; returns immediately
    /// // ... emulate, handle audio ...
    /// f.as_mut().await;                // ← DMA already done; returns ~instantly
    /// ```
    ///
    /// # How it works
    ///
    /// On the **first poll** (via `poll_once`):
    /// - Hashes the 23 KB native frame (~76 µs at 300 MHz, vs ~345 µs for a
    ///   pre-scaled 101 KB frame).  If identical to the previous frame, returns
    ///   `Poll::Ready` immediately — no DMA, no tearing.
    /// - Otherwise computes the dirty display-row range from the dirty bitmap,
    ///   **pre-scales only those rows** from the native frame into the 101 KB
    ///   `scale_buf` field, programs CASET/RASET/RAMWR **synchronously**
    ///   (blocking TX-FIFO writes, ~2 µs for 15 bytes at 75 MHz), then arms the
    ///   pixel DMA from `scale_buf` and returns `Poll::Pending`.
    ///
    /// On the **second poll** (the `.await` after emulation):
    /// - The DMA has been running concurrently with ~16 ms of emulation.
    ///   Raises CS HIGH and returns `Poll::Ready`.
    ///
    /// # Memory note
    ///
    /// Triple-buffered shared slots now hold raw native frames (3 × 23 KB =
    /// 69 KB) rather than pre-scaled frames (3 × 101 KB = 303 KB).  The single
    /// `scale_buf` (101 KB, backed by a BSS static) replaces the ~133 KB that
    /// was being spent on the two redundant scaled slots, for a net saving of
    /// 133 KB.
    pub async fn send_frame(&mut self, native: &NativeFrame, dirty: &[u32; 5]) {
        let hash = Self::hash_frame(native);
        if !self.frame_changed(hash) {
            return; // identical frame — display GRAM already shows this content
        }
        let (sy_start, sy_end) = Self::dirty_display_range(dirty).unwrap_or((0, 216));
        self.commit_frame_hash(hash);
        // Pre-scale only the dirty row range into the exclusively-owned scale buffer.
        scale_native_to_rgb565_range(native, self.scale_buf, sy_start, sy_end);
        // Blocking setup: CASET + RASET + RAMWR (~2 µs).  Completing synchronously
        // here ensures that the pixel-DMA future below is reached and armed within
        // the same `poll_once` call, so the DMA runs during the emulation step.
        self.setup_frame_range(sy_start, sy_end);
        // Access `self.spi` and `self.cs` directly (not through a `&mut self`
        // method) so that Rust's field-level borrow splitting allows the shared
        // borrow of `self.scale_buf` (via `bytes`) and the mutable borrows of
        // `self.spi` / `self.cs` to coexist.
        let pix_start = sy_start * 240;
        let pix_end = sy_end * 240;
        let bytes: &[u8] = bytemuck::cast_slice(&self.scale_buf[pix_start..pix_end]);
        self.spi.write(bytes).await.ok();
        self.cs.set_high();
    }

    /// Transfer a native Game Boy frame (160×144 palette indices) to the
    /// display via async DMA.
    ///
    /// Convenience wrapper that does a full-frame write without dirty-row
    /// narrowing.  Pre-scales the entire 240×216 region into `scale_buf`
    /// then streams via async pixel DMA; compatible with the `poll_once` /
    /// emulation-overlap pattern.
    pub async fn send_frame_raw(&mut self, native: &NativeFrame) {
        scale_native_to_rgb565_range(native, self.scale_buf, 0, 216);
        self.setup_frame_range(0, 216);
        // Access fields directly (not via a `&mut self` method) so that the
        // shared borrow of `self.scale_buf` and the mutable borrows of
        // `self.spi` / `self.cs` coexist via field-level borrow splitting.
        let bytes: &[u8] = bytemuck::cast_slice(self.scale_buf.as_ref());
        self.spi.write(bytes).await.ok();
        self.cs.set_high();
    }

    /// Paint the full 240×320 screen with a ROM loading progress screen.
    ///
    /// Uses a 480-byte stack buffer; no heap allocation.
    pub async fn draw_loading_progress(&mut self, frame: LoadingFrame<'_>) {
        self.draw_rendered_rows(RenderWindow::screen(), |y, row| {
            render_loading_row(&frame, y, row);
        })
        .await;
    }

    /// Repaint only the loading progress bar. The title/filename stay static.
    pub async fn draw_loading_bar(&mut self, progress: LoadingProgress) {
        let frame = LoadingFrame::new("", progress, 0);
        self.draw_rendered_rows(loading_bar_window(), |y, row| {
            render_loading_row(&frame, y, row);
        })
        .await;
    }

    /// Paint the full 240×320 screen with a menu, row by row.
    ///
    /// Hashes the [`MenuFrame`] descriptor **before** rendering.  If the frame
    /// is identical to the previously rendered one (same title, items, selected
    /// cursor, enabled mask, marked slot, and crash badge state), the function
    /// returns immediately without issuing any DMA — the display already shows
    /// the correct content.
    ///
    /// The skip is transparent to all callers; no state-machine code needs to
    /// change.  The cached hash is reset in `draw_letterbox_bars` so the first
    /// `draw_menu` call after a game session always performs a full repaint.
    ///
    /// Uses a 480-byte stack buffer; no heap allocation.
    pub async fn draw_menu(&mut self, frame: &MenuFrame<'_>) {
        let hash = Self::hash_menu_frame(frame);
        if hash == self.prev_menu_frame_hash {
            return; // identical frame — display already shows this content
        }
        self.prev_menu_frame_hash = hash;
        self.draw_rendered_rows(RenderWindow::screen(), |y, row| {
            render_menu_row(frame, y, row);
        })
        .await;
    }

    /// Repaint a single menu item row. Used by marquee animation to avoid
    /// full-screen refresh jitter while the rest of the ROM menu is static.
    pub async fn draw_menu_item(&mut self, frame: &MenuFrame<'_>, slot: usize) {
        let Some(window) = menu_item_window(slot) else {
            return;
        };

        self.draw_rendered_rows(window, |y, row| {
            render_menu_row(frame, y, row);
        })
        .await;
    }

    /// Repaint only the text window inside a menu item. This keeps marquee
    /// animation from touching static rows, cursor, or loaded marker pixels.
    pub async fn draw_menu_item_text(&mut self, frame: &MenuFrame<'_>, slot: usize) {
        let marked = frame.marked == Some(slot);
        let Some(window) = menu_item_text_window(slot, marked) else {
            return;
        };

        self.draw_rendered_rows(window, |y, row| {
            render_menu_row(frame, y, row);
        })
        .await;
    }

    // --- differential DMA helpers ---

    /// Compute the display-row range `[sy_start, sy_end)` that covers all dirty
    /// GB scanlines, using game-area-relative coordinates (0 = top row, 216 =
    /// one past the bottom row).
    ///
    /// `dirty` is the 5-word (160-bit) bitmap produced by Core 1, where bit `k`
    /// indicates that GB scanline `k` changed relative to the previous frame.
    ///
    /// **Scale mapping** (derived from `scale_to_rgb565`'s `gy = sy * 2 / 3`):
    ///
    /// GB row `k` maps to display rows `sy` where `sy * 2 / 3 == k`, which gives
    /// `sy_start(k) = (3k + 1) / 2` and `sy_end(k) = (3(k+1) + 1) / 2`
    /// (integer division).  In practice every pair of consecutive GB rows maps
    /// to exactly 3 display rows (2 + 1 alternating).
    ///
    /// Returns `None` when no rows are dirty (frame is identical; DMA can be
    /// skipped).  Returns `Some((0, 216))` when every scanline is dirty.
    fn dirty_display_range(dirty: &[u32; 5]) -> Option<(usize, usize)> {
        let mut first_gb: Option<usize> = None;
        let mut last_gb: Option<usize> = None;

        for k in 0..144usize {
            if (dirty[k / 32] >> (k % 32)) & 1 != 0 {
                if first_gb.is_none() {
                    first_gb = Some(k);
                }
                last_gb = Some(k);
            }
        }

        let first = first_gb?;
        let last = last_gb?;

        // sy_start: first display row produced by GB row `first`
        // sy_end:   first display row produced by GB row `last + 1` (exclusive)
        let sy_start = (3 * first + 1) / 2;
        let sy_end = (3 * (last + 1) + 1) / 2;
        Some((sy_start, sy_end))
    }

    /// Knuth multiplicative hash over an arbitrary byte slice.
    ///
    /// Processes 4 bytes at a time (one u32 XOR + multiply + avalanche per
    /// word), with a trailing byte loop for non-aligned tails.
    ///
    /// - **Seed** `0xdead_beef` ensures an all-zero slice never hashes to 0,
    ///   keeping 0 as a safe "cache miss / invalidated" sentinel.
    /// - **Constant** `M = 0x9e37_79b9` is the Fibonacci / Knuth multiplicative
    ///   hash constant for 32-bit words.
    /// - **Avalanche** `h ^= h >> 16` after each multiply spreads the high bits
    ///   into the low half, improving distribution for short inputs.
    fn knuth_hash(bytes: &[u8]) -> u32 {
        const M: u32 = 0x9e37_79b9;
        let chunks = bytes.chunks_exact(4);
        let remainder = chunks.remainder();
        let mut h = chunks.fold(0xdead_beef_u32, |mut h, chunk| {
            let w = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            h ^= w;
            h = h.wrapping_mul(M);
            h ^= h >> 16;
            h
        });
        for &b in remainder {
            h ^= b as u32;
            h = h.wrapping_mul(M);
            h ^= h >> 16;
        }
        h
    }

    /// Compute a fast hash of a native Game Boy frame (160×144 palette indices).
    ///
    /// Delegates to [`knuth_hash`] over the frame's raw 23 KB byte slice.
    /// At 300 MHz on Cortex-M33 this takes ~76 µs — 4.5× faster than hashing
    /// the 101 KB pre-scaled frame (~345 µs).
    ///
    /// Hashing the native frame is equivalent for change-detection purposes:
    /// the scale is a pure function of the native content, so a different native
    /// frame always produces a different scaled frame.
    ///
    /// Used to detect identical consecutive frames so the pixel DMA can
    /// be skipped entirely when the display content hasn't changed.
    ///
    /// Collision probability: ~1 in 4 × 10⁹ per frame — negligible in
    /// practice; a false skip shows a stale frame for one 16 ms period.
    fn hash_frame(buf: &NativeFrame) -> u32 {
        Self::knuth_hash(buf.as_slice())
    }

    #[inline]
    fn frame_changed(&self, hash: u32) -> bool {
        hash != self.prev_frame_hash
    }

    #[inline]
    fn commit_frame_hash(&mut self, hash: u32) {
        self.prev_frame_hash = hash;
    }

    /// Compute a fast hash of a [`MenuFrame`] descriptor.
    ///
    /// Hashes each field that affects rendering: title text, item strings,
    /// cursor position (`selected`), animation counter (`marquee_frame`),
    /// enabled mask, marked slot, and crash badge flag.
    ///
    /// Feeds each field's bytes through [`knuth_hash`], mixing the results
    /// together with the same Knuth multiplicative step.  The result is
    /// non-zero as long as any field contains non-zero bytes (seeded from
    /// `knuth_hash`'s `0xdead_beef` seed), keeping 0 safe as the
    /// "never-rendered" / "invalidated" sentinel stored in
    /// `prev_menu_frame_hash`.
    fn hash_menu_frame(frame: &MenuFrame<'_>) -> u32 {
        const M: u32 = 0x9e37_79b9;
        // Start from the title bytes.
        let mut h = Self::knuth_hash(frame.title.as_bytes());
        // Mix in each item string.
        for item in frame.items {
            h ^= Self::knuth_hash(item.as_bytes());
            h = h.wrapping_mul(M);
            h ^= h >> 16;
        }
        // Mix in selected cursor index.
        h ^= frame.selected as u32;
        h = h.wrapping_mul(M);
        h ^= h >> 16;
        // Mix in marquee animation counter.
        h ^= frame.marquee_frame;
        h = h.wrapping_mul(M);
        h ^= h >> 16;
        // Mix in the enabled-flag bitmask (one bool per item).
        for &enabled in frame.enabled {
            h ^= enabled as u32;
            h = h.wrapping_mul(M);
            h ^= h >> 16;
        }
        // Mix in the "currently loaded" marker (None → 0, Some(n) → n+1).
        h ^= frame.marked.map_or(0u32, |n| n as u32 + 1);
        h = h.wrapping_mul(M);
        h ^= h >> 16;
        // Mix in crash-badge flag.
        h ^= frame.crash_pending as u32;
        h = h.wrapping_mul(M);
        h ^= h >> 16;
        h
    }

    // --- helpers ---

    /// Program the CASET + RASET window for the dirty display-row range.
    ///
    /// `sy_start` / `sy_end` are game-area-relative (0 = top of game area).
    /// Leaves CS LOW and DC HIGH so the caller can stream pixel data immediately.
    ///
    /// Uses **blocking** TX-FIFO writes (≈ 2 µs for 15 bytes at 75 MHz) so that
    /// this function completes synchronously within the first `poll_once` call on
    /// [`send_frame`], allowing the pixel DMA to be armed before the emulation
    /// step begins.
    fn setup_frame_range(&mut self, sy_start: usize, sy_end: usize) {
        debug_assert!(sy_start < sy_end && sy_end <= 216, "invalid row range");
        let abs_y0 = GAME_Y_START + sy_start as u16;
        let abs_y1 = GAME_Y_START + sy_end as u16 - 1; // inclusive for RASET
        let x_params = [0, 0, (DISPLAY_X_END >> 8) as u8, DISPLAY_X_END as u8];
        self.write_command(0x2A, &x_params);
        let y_params = [
            (abs_y0 >> 8) as u8,
            abs_y0 as u8,
            (abs_y1 >> 8) as u8,
            abs_y1 as u8,
        ];
        self.write_command(0x2B, &y_params);
        self.write_command(0x2C, &[]);
        self.dc.set_high();
        self.cs.set_low();
    }

    fn set_window(&mut self, window: RenderWindow) {
        let Some((x0, x1, y0, y1)) = window.inclusive_bounds() else {
            return;
        };
        let x_params = [(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8];
        self.write_command(0x2A, &x_params);
        let y_params = [(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8];
        self.write_command(0x2B, &y_params);
    }

    /// Send an ILI9341 command byte followed by optional parameter bytes.
    ///
    /// Uses direct TX-FIFO register polling rather than `Spi::blocking_write`.
    /// `blocking_write` uses an RX-lockstep loop (one RX read per TX byte) that
    /// has undefined interaction with the DMA-managed RX FIFO state on
    /// `Spi<Async>`.  Direct TX polling is correct here: fill TX FIFO, wait for
    /// TFE + !BSY (all bytes shifted out), then drain the RX FIFO.
    /// Command payloads are always tiny (≤ 5 bytes) so the spin time is
    /// negligible (< 1 µs per call at 75 MHz).
    fn write_command(&mut self, cmd: u8, params: &[u8]) {
        self.cs.set_low();
        self.dc.set_low();
        Self::spi1_tx_bytes(&[cmd]);
        if !params.is_empty() {
            self.dc.set_high();
            Self::spi1_tx_bytes(params);
        }
        self.cs.set_high();
    }

    /// Write `data` to the SPI1 TX FIFO synchronously.
    ///
    /// Fills the TX FIFO byte-by-byte (polling TNF = TX FIFO not full), then
    /// waits for TFE (TX FIFO empty) and !BSY (shift register idle) to confirm
    /// all bytes have been sent.  Finally drains any bytes that accumulated in
    /// the RX FIFO (MISO is floating/unconnected on this board).
    ///
    /// Does **not** use the RX-lockstep loop from `Spi::blocking_write`, which
    /// assumes one RX byte arrives for each TX byte written — an assumption that
    /// can be violated after a preceding async DMA transfer leaves the RX FIFO
    /// in an indeterminate state.
    #[inline]
    fn spi1_tx_bytes(data: &[u8]) {
        let spi = rp_pac::SPI1;
        for &b in data {
            while !spi.sr().read().tnf() {}
            spi.dr().write(|w| w.set_data(b as u16));
        }
        // Wait for TX FIFO empty (all bytes moved to shift register)
        while !spi.sr().read().tfe() {}
        // Wait for shift register idle (last bit clocked out)
        while spi.sr().read().bsy() {}
        // Drain RX FIFO: bytes shifted in from floating MISO during transmission
        while spi.sr().read().rne() {
            let _ = spi.dr().read();
        }
    }

    /// Render and DMA all rows in `window`.
    ///
    /// For each row, calls `render_row(y, &mut row_buf)` to fill a 480-byte
    /// pixel buffer, then streams the window's x-column slice to the display
    /// via async SPI DMA.  A single CASET/RASET/RAMWR setup covers the entire
    /// window; the ILI9341 auto-increments its write pointer across rows.
    ///
    /// Frame-level deduplication (whole-menu skip when nothing changed) is
    /// handled by the callers — see [`draw_menu`].  This function always
    /// renders and transfers every row it is given.
    async fn draw_rendered_rows<F>(&mut self, window: RenderWindow, mut render_row: F)
    where
        F: FnMut(u16, &mut [u8; ROW_BYTES]),
    {
        if window.is_empty() {
            return;
        }

        self.set_window(window);
        self.write_command(0x2C, &[]);
        self.dc.set_high();
        self.cs.set_low();

        let mut row = [0u8; ROW_BYTES];
        let byte_range = window.byte_range();
        for y in window.y_start..window.y_end {
            render_row(y, &mut row);
            self.spi.write(&row[byte_range.clone()]).await.ok();
        }

        self.cs.set_high();
    }

    async fn fill_rect_raw(&mut self, n_pixels: usize, pixel_be: &[u8; 2]) {
        // Send 240 pixels (480 bytes) per row to keep the stack usage bounded.
        let mut row = [0u8; ROW_BYTES];
        let mut i = 0;
        while i < ROW_BYTES {
            row[i] = pixel_be[0];
            row[i + 1] = pixel_be[1];
            i += 2;
        }
        let n_rows = n_pixels / DISPLAY_ROW_PIXELS;
        self.dc.set_high();
        self.cs.set_low();
        for _ in 0..n_rows {
            self.spi.write(&row).await.ok();
        }
        self.cs.set_high();
    }
}
