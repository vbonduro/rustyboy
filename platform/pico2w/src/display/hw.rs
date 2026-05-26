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
    /// game loop. The bars are never repainted — `send_frame_raw` only touches
    /// the 240×216 game area.
    pub async fn draw_letterbox_bars(&mut self) {
        info!("display: drawing letterbox bars");

        // Top bar: rows 0..51, colour C3
        self.set_window(RenderWindow::new(
            0,
            DISPLAY_X_END + 1,
            0,
            TOP_BAR_Y_END + 1,
        ))
        .await;
        self.write_command(0x2C, &[]).await;
        self.fill_rect_raw(LETTERBOX_ROWS * DISPLAY_ROW_PIXELS, &C3_BE)
            .await;
        info!("display: top bar done");

        // Bottom bar: rows 268..319, colour black
        self.set_window(RenderWindow::new(
            0,
            DISPLAY_X_END + 1,
            BOTTOM_BAR_Y_START,
            DISPLAY_Y_END + 1,
        ))
        .await;
        self.write_command(0x2C, &[]).await;
        self.fill_rect_raw(LETTERBOX_ROWS * DISPLAY_ROW_PIXELS, &BLACK_BE)
            .await;
        info!("display: letterbox bars done");
    }

    /// Push the ILI9341 panel scan from the default ~70 Hz to ~94 Hz via
    /// FRMCTR1 (0xB1), to break the resonance between the scan rate and our
    /// frame rate that makes tearing appear as visible horizontal bouncing.
    ///
    /// # Why ~94 Hz?
    ///
    /// Our frame period is ~16.74 ms (paced by the 44100 Hz audio DMA).  At the
    /// default 70 Hz scan (14.3 ms period) the fractional phase drift per frame
    /// is only 0.17 periods, so the tear line oscillates at ~10 Hz — squarely
    /// in the range the eye perceives as spatial motion.
    ///
    /// At ~94 Hz (scan period ~10.6 ms) the drift becomes 0.58 periods/frame,
    /// pushing the apparent oscillation to ~34 Hz.  Above ~30 Hz the eye
    /// integrates the flickering tear into a constant slightly-dim band rather
    /// than tracking it as a moving edge.
    ///
    /// Going *faster* than default is safe: the LCD cells are refreshed more
    /// often, which keeps them better charged.  Halving the scan rate (35 Hz)
    /// caused progressive degradation because the cells had too long to drift
    /// between refreshes.
    ///
    /// Call once after [`Self::new_after_splash`], before the first frame.
    pub async fn configure_scan_rate(&mut self) {
        // FRMCTR1 (0xB1): Normal-mode frame rate control.
        //   Byte 0 — DIVA[1:0] = 0x00 → no division (default oscillator speed).
        //   Byte 1 — RTNA[4:0] = 0x10 (16) → 32 clocks/line vs default 43.
        //            f ≈ f_default × (43 / 32) ≈ 70 × 1.34 ≈ 94 Hz.
        self.write_command(0xB1, &[0x00, 0x10]).await;
    }

    /// Send the CASET / RASET / RAMWR window-setup commands for a contiguous
    /// range of display rows within the 240×216 game area.
    ///
    /// `sy_start` and `sy_end` are **game-area-relative** display row indices
    /// (0 = top of game area, 215 = bottom).  `sy_end` is exclusive.
    ///
    /// Use `(0, 216)` for a full-frame write.  For dirty-row DMA, pass the
    /// range returned by [`dirty_display_range`] to write only the rows that
    /// changed.
    ///
    /// Leaves CS LOW and DC HIGH ready for pixel data.
    pub async fn setup_frame_range(&mut self, sy_start: usize, sy_end: usize) {
        debug_assert!(sy_start < sy_end && sy_end <= 216, "invalid row range");
        let abs_y0 = GAME_Y_START + sy_start as u16;
        let abs_y1 = GAME_Y_START + sy_end as u16 - 1; // inclusive for RASET
        let x_params = [0, 0, (DISPLAY_X_END >> 8) as u8, DISPLAY_X_END as u8];
        self.write_command(0x2A, &x_params).await;
        let y_params = [
            (abs_y0 >> 8) as u8,
            abs_y0 as u8,
            (abs_y1 >> 8) as u8,
            abs_y1 as u8,
        ];
        self.write_command(0x2B, &y_params).await;
        self.write_command(0x2C, &[]).await;
        // Leave CS LOW and DC HIGH: the ILI9341 is ready to stream pixel data.
        self.dc.set_high();
        self.cs.set_low();
    }

    /// Send the pixel data for display rows `sy_start..sy_end` and raise CS HIGH.
    ///
    /// **Precondition:** [`setup_frame_range`] must have been called this frame
    /// with the same range so CS is LOW and DC is HIGH.
    ///
    /// In the game loop, start this with `poll_once` right after
    /// [`setup_frame_range`] so the DMA runs concurrently with emulation.
    pub async fn send_frame_range_pixels(
        &mut self,
        buf: &ScaledFrame,
        sy_start: usize,
        sy_end: usize,
    ) {
        let pix_start = sy_start * 240;
        let pix_end = sy_end * 240;
        // buf stores big-endian u16s; cast to bytes for the correct SPI byte order.
        self.spi
            .write(bytemuck::cast_slice(&buf[pix_start..pix_end]))
            .await
            .ok();
        self.cs.set_high();
    }

    /// Send the CASET / RASET / RAMWR window-setup commands for the 240×216
    /// game area, then leave CS LOW and DC HIGH ready for pixel data.
    ///
    /// Convenience wrapper around [`setup_frame_range`] for a full-frame write.
    pub async fn setup_frame(&mut self) {
        self.setup_frame_range(0, 216).await;
    }

    /// Send the 240×216 pixel data and raise CS HIGH.
    ///
    /// Convenience wrapper around [`send_frame_range_pixels`] for a full-frame
    /// write.  See that method for the `poll_once` overlap pattern.
    pub async fn send_frame_pixels(&mut self, buf: &ScaledFrame) {
        self.send_frame_range_pixels(buf, 0, 216).await;
    }

    /// Transfer a pre-scaled 240×216 frame to the display via async DMA.
    ///
    /// Convenience wrapper that calls [`setup_frame`] + [`send_frame_pixels`]
    /// sequentially (no emulation overlap).  Prefer the two-step API in the
    /// hot game-loop path for proper DMA / emulation overlap.
    pub async fn send_frame_raw(&mut self, buf: &ScaledFrame) {
        self.setup_frame().await;
        self.send_frame_pixels(buf).await;
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
    pub fn dirty_display_range(dirty: &[u32; 5]) -> Option<(usize, usize)> {
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
    /// Used to detect identical consecutive frames so the 11 ms pixel DMA can
    /// be skipped when the display content hasn't changed.
    ///
    /// Collision probability: ~1 in 4 × 10⁹ per frame — negligible in
    /// practice; a false skip shows a stale frame for one 16 ms period.
    pub fn hash_frame(buf: &ScaledFrame) -> u32 {
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

    /// Return `true` if `hash` differs from the last committed frame's hash,
    /// meaning a new DMA transfer is required.
    ///
    /// A `false` return means the display already shows this content; the
    /// caller can skip [`setup_frame`] + [`send_frame_pixels`] entirely.
    #[inline]
    pub fn frame_changed(&self, hash: u32) -> bool {
        hash != self.prev_frame_hash
    }

    /// Record `hash` as the hash of the frame now committed to the display.
    ///
    /// Call this before issuing the DMA (the hash is already computed; storing
    /// it here before the transfer avoids any borrow-checker conflict with the
    /// pinned DMA future).
    #[inline]
    pub fn commit_frame_hash(&mut self, hash: u32) {
        self.prev_frame_hash = hash;
    }

    // --- helpers ---

    async fn set_window(&mut self, window: RenderWindow) {
        let Some((x0, x1, y0, y1)) = window.inclusive_bounds() else {
            return;
        };
        let x_params = [(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8];
        self.write_command(0x2A, &x_params).await;
        let y_params = [(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8];
        self.write_command(0x2B, &y_params).await;
    }

    async fn write_command(&mut self, cmd: u8, params: &[u8]) {
        let cmd_buf = [cmd];
        self.cs.set_low();
        self.dc.set_low();
        self.spi.write(&cmd_buf).await.ok();
        if !params.is_empty() {
            self.dc.set_high();
            self.spi.write(params).await.ok();
        }
        self.cs.set_high();
    }

    async fn draw_rendered_rows<F>(&mut self, window: RenderWindow, mut render_row: F)
    where
        F: FnMut(u16, &mut [u8; ROW_BYTES]),
    {
        if window.is_empty() {
            return;
        }

        self.set_window(window).await;
        self.write_command(0x2C, &[]).await;

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
