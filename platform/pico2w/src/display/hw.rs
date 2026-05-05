//! Hardware-specific display initialisation for the ACEIRMC ILI9341 module.
//!
//! Constructs a [`Display`] backed by the real SPI peripheral. Only compiled
//! for the embedded target — host builds and tests use [`super::fb::FbDisplay`]
//! instead.

use defmt::{info, warn};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH1, PIN_10, PIN_11, PIN_12, PIN_13, PIN_8, PIN_9, SPI1};
use embassy_rp::spi::{Async, Blocking, Config as SpiConfig, Spi};
use embassy_rp::{dma, interrupt};
use embassy_rp::Peri;

use display_interface_spi::SPIInterface;
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorOrder, Orientation};
use mipidsi::Builder;

use super::Display;

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
        cfg.frequency = 62_500_000; // ILI9341 supports up to 66 MHz write

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
            rp_pac::DMA.chan_abort().write_value(rp_pac::dma::regs::ChanAbort(1u32 << 1));
            while rp_pac::DMA.chan_abort().read().chan_abort() & (1u16 << 1) != 0 {}
        }
        // Clear any pending CH1 interrupt flag in INTS0 (W1C: write 1 to clear).
        rp_pac::DMA.ints(0).write_value(1u32 << 1);

        let mut cfg = SpiConfig::default();
        cfg.frequency = 62_500_000;

        let spi = Spi::new_txonly(spi1, clk, mosi, dma, irqs, cfg);
        let cs = Output::new(cs_pin, Level::High);
        let dc = Output::new(dc_pin, Level::Low);
        let rst = Output::new(rst_pin, Level::High);
        let bl = Output::new(bl_pin, Level::High);

        info!("display: async SPI re-initialised for game loop");
        Self { spi, cs, dc, _rst: rst, _bl: bl }
    }

    /// Paint the static letterbox bars (top C3, bottom black) once before the
    /// game loop. The bars are never repainted — `send_frame_raw` only touches
    /// the 240×216 game area.
    pub async fn draw_letterbox_bars(&mut self) {
        info!("display: drawing letterbox bars");

        // Top bar: rows 0..51, colour C3
        self.set_window(0, 0, DISPLAY_X_END, TOP_BAR_Y_END).await;
        self.write_command(0x2C, &[]).await;
        self.fill_rect_raw(LETTERBOX_ROWS * DISPLAY_ROW_PIXELS, &C3_BE).await;
        info!("display: top bar done");

        // Bottom bar: rows 268..319, colour black
        self.set_window(0, BOTTOM_BAR_Y_START, DISPLAY_X_END, DISPLAY_Y_END).await;
        self.write_command(0x2C, &[]).await;
        self.fill_rect_raw(LETTERBOX_ROWS * DISPLAY_ROW_PIXELS, &BLACK_BE).await;
        info!("display: letterbox bars done");
    }

    /// Transfer a pre-scaled 240×216 frame to the display via async DMA.
    ///
    /// `buf` must contain big-endian RGB565 values as produced by
    /// [`super::scale_to_rgb565`]. Returns a future; `.await` it after doing
    /// other work to overlap the ~13 ms transfer with emulation.
    pub async fn send_frame_raw(&mut self, buf: &[u16; 51840]) {
        self.set_window(0, GAME_Y_START, DISPLAY_X_END, GAME_Y_END).await;
        self.write_command(0x2C, &[]).await;

        self.dc.set_high();
        self.cs.set_low();
        // buf stores big-endian u16s; cast to bytes gives the correct SPI byte order.
        self.spi.write(bytemuck::cast_slice(buf)).await.ok();
        self.cs.set_high();
    }

    // --- helpers ---

    async fn set_window(&mut self, x0: u16, y0: u16, x1: u16, y1: u16) {
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

    async fn fill_rect_raw(&mut self, n_pixels: usize, pixel_be: &[u8; 2]) {
        // Send 240 pixels (480 bytes) per row to keep the stack usage bounded.
        let mut row = [0u8; ROW_BYTES];
        let mut i = 0;
        while i < ROW_BYTES {
            row[i]     = pixel_be[0];
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
