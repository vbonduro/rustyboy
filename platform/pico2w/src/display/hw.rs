//! Hardware-specific display initialisation for the ACEIRMC ILI9341 module.
//!
//! Constructs a [`Display`] backed by the real SPI peripheral. Only compiled
//! for the embedded target — host builds and tests use [`super::fb::FbDisplay`]
//! instead.

use defmt::info;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{PIN_10, PIN_11, PIN_12, PIN_13, PIN_8, PIN_9, SPI1};
use embassy_rp::spi::{Blocking, Config as SpiConfig, Spi};
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
