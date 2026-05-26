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

use super::{
    loading_bar_window, menu_item_text_window, menu_item_window, render_loading_row,
    render_menu_row, Display, LoadingFrame, LoadingProgress, RenderWindow, ScaledFrame,
};
use crate::menu::MenuFrame;

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
    /// FNV-1a hash of the last frame successfully sent to the display.
    ///
    /// Compared against each new frame's hash before deciding whether to
    /// issue DMA.  Identical frames skip the 11 ms pixel DMA entirely so
    /// the display refreshes its own GRAM with no scan-line race.
    /// Cost: 4 bytes (vs 103 KB for a full shadow copy).
    prev_frame_hash: u32,
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
            prev_frame_hash: 0,
        }
    }

    /// Paint the static letterbox bars (top C3, bottom black) once before the
    /// game loop. The bars are never repainted — `send_frame` only touches the
    /// 240×216 game area.
    pub async fn draw_letterbox_bars(&mut self) {
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

    /// Transfer a pre-scaled 240×216 frame to the display, handling dirty-row
    /// detection, hash-based skip, and the DMA / emulation overlap pattern.
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
    /// - Computes the frame hash.  If identical to the previous frame, returns
    ///   `Poll::Ready` immediately — no DMA, no tearing.
    /// - Otherwise computes the dirty display-row range, programs CASET/RASET/
    ///   RAMWR **synchronously** (blocking TX-FIFO writes, ~2 µs for 15 bytes at
    ///   75 MHz), then arms the pixel DMA and returns `Poll::Pending`.
    ///
    /// On the **second poll** (the `.await` after emulation):
    /// - The DMA has been running concurrently with ~16 ms of emulation.
    ///   Raises CS HIGH and returns `Poll::Ready`.
    pub async fn send_frame(&mut self, buf: &ScaledFrame, dirty: &[u32; 5]) {
        let hash = Self::hash_frame(buf);
        if !self.frame_changed(hash) {
            return; // identical frame — display GRAM already shows this content
        }
        let (sy_start, sy_end) = Self::dirty_display_range(dirty).unwrap_or((0, 216));
        self.commit_frame_hash(hash);
        // Blocking setup: CASET + RASET + RAMWR (~2 µs).  Completing synchronously
        // here ensures that the pixel-DMA future below is reached and armed within
        // the same `poll_once` call, so the DMA runs during the emulation step.
        self.setup_frame_range(sy_start, sy_end);
        // Async pixel DMA: returns Poll::Pending after arming DMA hardware.
        self.send_frame_range_pixels(buf, sy_start, sy_end).await;
    }

    /// Transfer a pre-scaled 240×216 frame to the display via async DMA.
    ///
    /// Convenience wrapper that does a full-frame write without dirty-row
    /// narrowing.  Uses blocking setup + async pixel DMA; compatible with the
    /// `poll_once` / emulation-overlap pattern.
    pub async fn send_frame_raw(&mut self, buf: &ScaledFrame) {
        self.setup_frame_range(0, 216);
        self.send_frame_range_pixels(buf, 0, 216).await;
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
    /// Uses a 480-byte stack buffer; no heap allocation.
    pub async fn draw_menu(&mut self, frame: &MenuFrame<'_>) {
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

    // --- low-level pixel DMA (used by send_frame / send_frame_raw) ---

    /// Send display rows `sy_start..sy_end` via async DMA and raise CS HIGH.
    ///
    /// **Precondition:** [`setup_frame_range`] must have been called this frame
    /// so CS is LOW and DC is HIGH.
    async fn send_frame_range_pixels(&mut self, buf: &ScaledFrame, sy_start: usize, sy_end: usize) {
        let pix_start = sy_start * 240;
        let pix_end = sy_end * 240;
        // buf stores big-endian u16s; cast_slice gives the correct SPI byte order.
        self.spi
            .write(bytemuck::cast_slice(&buf[pix_start..pix_end]))
            .await
            .ok();
        self.cs.set_high();
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

    /// Compute a fast hash of a scaled frame.
    ///
    /// Uses a single Knuth multiplicative step per u32 word (two packed u16s),
    /// giving one multiply per 4 bytes instead of FNV-1a's four.  At 300 MHz
    /// on Cortex-M33 this takes ~345 µs (2 % of a 16.7 ms frame budget).
    ///
    /// Used to detect identical consecutive frames so the pixel DMA can
    /// be skipped entirely when the display content hasn't changed.
    ///
    /// Collision probability: ~1 in 4 × 10⁹ per frame — negligible in
    /// practice; a false skip shows a stale frame for one 16 ms period.
    fn hash_frame(buf: &ScaledFrame) -> u32 {
        // Fibonacci/Knuth multiplicative hash constant.
        const M: u32 = 0x9e37_79b9;
        // buf has an even number of u16 values (240 × 216 = 51 840), so
        // chunks_exact(2) is always lossless — no remainder.
        buf.chunks_exact(2).fold(0xdead_beef_u32, |mut h, chunk| {
            let w = (chunk[0] as u32) | ((chunk[1] as u32) << 16);
            h ^= w;
            h = h.wrapping_mul(M);
            h ^= h >> 16;
            h
        })
    }

    #[inline]
    fn frame_changed(&self, hash: u32) -> bool {
        hash != self.prev_frame_hash
    }

    #[inline]
    fn commit_frame_hash(&mut self, hash: u32) {
        self.prev_frame_hash = hash;
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

    async fn draw_rendered_rows<F>(&mut self, window: RenderWindow, mut render_row: F)
    where
        F: FnMut(u16, &mut [u8; ROW_BYTES]),
    {
        if window.is_empty() {
            return;
        }

        self.set_window(window);
        self.write_command(0x2C, &[]);

        let mut row = [0u8; ROW_BYTES];
        let byte_range = window.byte_range();

        self.dc.set_high();
        self.cs.set_low();
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
