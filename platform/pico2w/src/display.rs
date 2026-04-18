//! ILI9341 display driver — Bead 2
//!
//! Drives the ACEIRMC 2.8" ILI9341 SPI TFT (240×320) over SPI0.
//!
//! # Pin wiring
//! | ILI9341 pin | Pico 2W GPIO |
//! |-------------|--------------|
//! | CLK / SCK   | GP10         |
//! | MOSI / SDA  | GP11         |
//! | CS          | GP9          |
//! | DC / RS     | GP8          |
//! | RST         | GP12         |
//! | LED / BL    | GP13         |
//!
//! No MISO is required — the driver is TX-only.
//!
//! # Coordinate system
//! The ILI9341 is initialised in portrait orientation (240 wide × 320 tall).
//! The Game Boy framebuffer (160×144) is scaled 1.5× to 240×216 and centred
//! vertically with 52-pixel black bars above and below.

use defmt::info;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{PIN_10, PIN_11, PIN_12, PIN_13, PIN_8, PIN_9, SPI1};
use embassy_rp::spi::{Blocking, Config as SpiConfig, Spi};
use embassy_rp::Peri;
use embassy_time::{Duration, Timer};

use display_interface_spi::SPIInterface;
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Point, Size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::RgbColor;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::Pixel;
use embedded_hal_bus::spi::ExclusiveDevice;
use mipidsi::models::ILI9341Rgb565;
use mipidsi::options::{ColorOrder, Orientation};
use mipidsi::Builder;

// ---------------------------------------------------------------------------
// Type aliases — keeps the impl blocks readable
// ---------------------------------------------------------------------------

// GP10=SPI1_CLK, GP11=SPI1_MOSI — display lives on SPI1.
// SD card uses SPI0 (GP18/GP19) on Bead 4.
type MySpi<'d> = Spi<'d, SPI1, Blocking>;
type MySpiDev<'d> = ExclusiveDevice<MySpi<'d>, Output<'d>, embassy_time::Delay>;
type MyDi<'d> = SPIInterface<MySpiDev<'d>, Output<'d>>;
type MipiDisp<'d> = mipidsi::Display<MyDi<'d>, ILI9341Rgb565, Output<'d>>;

// ---------------------------------------------------------------------------
// DMG colour palette as RGB565 — matches the web platform exactly.
// Source: platform/web/client/src/lib.rs PALETTE constant.
// ---------------------------------------------------------------------------

/// Lightest (colour 0) — #E0F8D0  light minty green
const C0: Rgb565 = Rgb565::new(0xE0 >> 3, 0xF8 >> 2, 0xD0 >> 3);
/// Light (colour 1) — #88C070
const C1: Rgb565 = Rgb565::new(0x88 >> 3, 0xC0 >> 2, 0x70 >> 3);
/// Dark (colour 2) — #346856
const C2: Rgb565 = Rgb565::new(0x34 >> 3, 0x68 >> 2, 0x56 >> 3);
/// Darkest (colour 3) — #081820  near-black (background / logo text)
const C3: Rgb565 = Rgb565::new(0x08 >> 3, 0x18 >> 2, 0x20 >> 3);

/// Framebuffer dimensions (Game Boy native)
const GB_W: i32 = 160;
#[allow(dead_code)]
const GB_H: i32 = 144;

/// Output dimensions on the ILI9341 (1.5× scale)
const SCALED_W: i32 = 240; // 160 × 1.5
const SCALED_H: i32 = 216; // 144 × 1.5

/// Screen size
const SCREEN_W: i32 = 240;
const SCREEN_H: i32 = 320;

/// Vertical offset to centre the scaled frame (black bars top and bottom)
const Y_OFFSET: i32 = (SCREEN_H - SCALED_H) / 2; // 52

// ---------------------------------------------------------------------------
// Public driver
// ---------------------------------------------------------------------------

pub struct Display<'d> {
    inner: MipiDisp<'d>,
    /// Backlight pin — kept alive so it is not driven low on drop.
    _bl: Output<'d>,
}

impl<'d> Display<'d> {
    /// Initialise the display.
    ///
    /// Consumes the raw embassy-rp peripherals and returns a ready-to-use
    /// `Display`. The backlight is turned on immediately after init.
    pub fn new(
        spi1: Peri<'d, SPI1>,
        clk: Peri<'d, PIN_10>,
        mosi: Peri<'d, PIN_11>,
        cs_pin: Peri<'d, PIN_9>,
        dc_pin: Peri<'d, PIN_8>,
        rst_pin: Peri<'d, PIN_12>,
        bl_pin: Peri<'d, PIN_13>,
    ) -> Self {
        // SPI at 62.5 MHz — ILI9341 supports up to 66 MHz write.
        let mut cfg = SpiConfig::default();
        cfg.frequency = 62_500_000;

        let spi = Spi::new_blocking_txonly(spi1, clk, mosi, cfg);

        let cs = Output::new(cs_pin, Level::High);
        let dc = Output::new(dc_pin, Level::Low);
        let rst = Output::new(rst_pin, Level::High);
        let mut bl = Output::new(bl_pin, Level::Low);

        // ExclusiveDevice::new is infallible — it does not return a Result.
        let spi_dev = ExclusiveDevice::new(spi, cs, embassy_time::Delay);
        let di = SPIInterface::new(spi_dev, dc);

        // flip() mirrors horizontally — corrects the reversed scan direction
        // on this particular ACEIRMC module wiring.
        let display = Builder::new(ILI9341Rgb565, di)
            .reset_pin(rst)
            .color_order(ColorOrder::Bgr)
            .orientation(Orientation::new().flip_horizontal())
            .init(&mut embassy_time::Delay)
            .unwrap();

        // Backlight on
        bl.set_high();
        info!("display: ILI9341 initialised");

        Self {
            inner: display,
            _bl: bl,
        }
    }

    /// Render a Game Boy framebuffer (160×144, palette indices 0–3).
    ///
    /// Scales 1.5× to 240×216 via nearest-neighbour, black-bars the
    /// remaining 52px top and 52px bottom.
    pub fn render_frame(&mut self, fb: &[u8; 23040]) {
        // Top black bar
        let top_bar = (0..SCREEN_W * Y_OFFSET).map(|i| {
            let x = (i % SCREEN_W) as i32;
            let y = (i / SCREEN_W) as i32;
            Pixel(Point::new(x, y), C3)
        });

        // Scaled game image
        let game = (0..SCALED_W * SCALED_H).map(|i| {
            let sx = (i % SCALED_W) as i32;
            let sy = (i / SCALED_W) as i32;
            // Nearest-neighbour: map scaled pixel back to source
            let gx = (sx * 2 / 3) as usize;
            let gy = (sy * 2 / 3) as usize;
            let idx = fb[gy * GB_W as usize + gx];
            let color = dmg_color(idx);
            Pixel(Point::new(sx, sy + Y_OFFSET), color)
        });

        // Bottom black bar
        let bot_y = Y_OFFSET + SCALED_H;
        let bottom_bar = (0..SCREEN_W * Y_OFFSET).map(|i| {
            let x = (i % SCREEN_W) as i32;
            let y = (i / SCREEN_W) as i32;
            Pixel(Point::new(x, y + bot_y), Rgb565::BLACK)
        });

        let all_pixels = top_bar.chain(game).chain(bottom_bar);
        self.inner.draw_iter(all_pixels).unwrap();
    }

    /// Fill the whole screen with a solid colour.
    pub fn clear(&mut self, color: Rgb565) {
        self.inner.clear(color).unwrap();
    }

    /// Play the "VINTENDO" boot animation.
    ///
    /// Mirrors the original Game Boy boot sequence:
    ///   1. Screen starts black.
    ///   2. "VINTENDO" logo slides down from off-screen at ~60 fps.
    ///   3. Once centred, the ® symbol appears to the right.
    ///   4. Hold for 2 seconds.
    pub async fn splash(&mut self) {
        const FPS_DELAY_MS: u64 = 16; // ~62.5 fps

        // C1 (#88C070) is more saturated than C0 and reads clearly as green on
        // the physical ILI9341 — C0 is so pale it appears near-white on screen.
        self.inner.clear(C1).unwrap();

        // Logo dimensions: 8 glyphs × 16px wide + 7 gaps × 2px = 142px wide (2× scale)
        const GLYPH_W: i32 = 16; // 8px × 2
        const GLYPH_H: i32 = 16; // 8px × 2
        const GAP: i32 = 2;
        const LOGO_W: i32 = 8 * GLYPH_W + 7 * GAP; // 142
        const LOGO_X: i32 = (SCREEN_W - LOGO_W) / 2; // 49 — left edge of 'V'
        const LOGO_FINAL_Y: i32 = (SCREEN_H - GLYPH_H) / 2 - 8; // slightly above centre

        // Start just off-screen top
        let start_y: i32 = -(GLYPH_H + 4);
        // 4 px/frame → ~40 frames = 640 ms travel time
        let travel = (LOGO_FINAL_Y - start_y).max(1);
        let total_frames = (travel / 4 + 1) as u64;

        let mut prev_y = start_y;

        for frame in 0..=total_frames {
            let delta = ((frame as i32) * 4).min(travel);
            let logo_y = start_y + delta;

            // Clear the thin strip vacated as the logo moves down (at most 4 rows),
            // then draw the full logo — both as fill_contiguous calls so each sets
            // the display window once and streams pixels without address re-arming.
            let clear_top = prev_y.max(0);
            let clear_bot = logo_y.max(0);
            if clear_bot > clear_top {
                let rect = Rectangle::new(
                    Point::new(0, clear_top),
                    Size::new(SCREEN_W as u32, (clear_bot - clear_top) as u32),
                );
                self.inner
                    .fill_contiguous(&rect, core::iter::repeat(C1))
                    .unwrap();
            }

            self.draw_logo(LOGO_X, logo_y, false, C3, C1);
            Timer::after(Duration::from_millis(FPS_DELAY_MS)).await;
            prev_y = logo_y;

            if delta >= travel {
                break;
            }
        }

        // Ensure logo is exactly at final position, then show ®.
        self.draw_logo(LOGO_X, LOGO_FINAL_Y, true, C3, C1);
        Timer::after(Duration::from_millis(2_000)).await;

        info!("splash: done");
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Draw the VINTENDO logo (8 glyphs at 2× scale) as a single fill_contiguous
    /// call, which sets the display window once and streams all pixels in one
    /// SPI burst — eliminates per-glyph tearing and warping during animation.
    fn draw_logo(&mut self, x: i32, y: i32, show_reg: bool, fg: Rgb565, bg: Rgb565) {
        // VINTENDO — glyph indices: V=0 I=1 N=2 T=3 E=4 N=2 D=5 O=6
        const SEQUENCE: [usize; 8] = [0, 1, 2, 3, 4, 2, 5, 6];
        const GLYPH_W: i32 = 16; // 8px × 2
        const GAP: i32 = 2;
        const LOGO_W: i32 = 8 * GLYPH_W + 7 * GAP; // 142px
        const GLYPH_H: i32 = 16;
        const SLOT_W: i32 = GLYPH_W + GAP; // 18

        let x0 = x.max(0);
        let y0 = y.max(0);
        let x1 = (x + LOGO_W).min(SCREEN_W);
        let y1 = (y + GLYPH_H).min(SCREEN_H);
        if x1 <= x0 || y1 <= y0 {
            return;
        }

        let rect = Rectangle::new(
            Point::new(x0, y0),
            Size::new((x1 - x0) as u32, (y1 - y0) as u32),
        );

        let colors = (y0..y1).flat_map(move |py| {
            let ry = py - y;
            (x0..x1).map(move |px| {
                let rx = px - x;
                let slot = (rx / SLOT_W) as usize;
                let slot_x = rx % SLOT_W;
                if slot < 8 && slot_x < GLYPH_W {
                    let glyph_idx = SEQUENCE[slot];
                    let gx = slot_x / 2;
                    let gy = ry / 2;
                    let row = font::GLYPHS[glyph_idx][gy as usize];
                    let bit = 7 - gx;
                    if (row >> bit) & 1 == 1 { fg } else { bg }
                } else {
                    bg
                }
            })
        });

        self.inner.fill_contiguous(&rect, colors).unwrap();

        if show_reg {
            let reg_x = x + LOGO_W + 1;
            self.draw_glyph_1x(7, reg_x, y, fg, bg);
        }
    }

    /// Draw a single 8×8 glyph at 1× scale as a fill_contiguous call.
    /// Used for the ® superscript (drawn once at the final splash position).
    fn draw_glyph_1x(&mut self, idx: usize, px: i32, py: i32, fg: Rgb565, bg: Rgb565) {
        let x0 = px.max(0);
        let y0 = py.max(0);
        let x1 = (px + 8).min(SCREEN_W);
        let y1 = (py + 8).min(SCREEN_H);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let rect = Rectangle::new(
            Point::new(x0, y0),
            Size::new((x1 - x0) as u32, (y1 - y0) as u32),
        );
        let bitmap = font::GLYPHS[idx];
        let colors = (y0..y1).flat_map(move |py2| {
            let ry = (py2 - py) as usize;
            let row = bitmap[ry];
            (x0..x1).map(move |px2| {
                let bit = 7 - (px2 - px);
                if (row >> bit) & 1 == 1 { fg } else { bg }
            })
        });
        self.inner.fill_contiguous(&rect, colors).unwrap();
    }
}

/// Map a DMG palette index (0–3) to an RGB565 colour.
#[inline(always)]
fn dmg_color(idx: u8) -> Rgb565 {
    match idx {
        0 => C0,
        1 => C1,
        2 => C2,
        _ => C3,
    }
}

// ---------------------------------------------------------------------------
// Font bitmaps
// ---------------------------------------------------------------------------

mod font {
    /// 8×8 bitmaps for: V I N T E D O ®
    /// Each `u8` is one row; bit 7 is the leftmost pixel.
    pub const GLYPHS: [[u8; 8]; 8] = [
        // V
        [0xC3, 0xC3, 0xC3, 0x66, 0x66, 0x3C, 0x18, 0x00],
        // I
        [0x7E, 0x18, 0x18, 0x18, 0x18, 0x18, 0x7E, 0x00],
        // N
        [0xC3, 0xE3, 0xF3, 0xDB, 0xCF, 0xC7, 0xC3, 0x00],
        // T
        [0xFF, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x00],
        // E
        [0xFF, 0xC0, 0xC0, 0xFC, 0xC0, 0xC0, 0xFF, 0x00],
        // D
        [0xFC, 0xC6, 0xC3, 0xC3, 0xC3, 0xC6, 0xFC, 0x00],
        // O
        [0x3C, 0x66, 0xC3, 0xC3, 0xC3, 0x66, 0x3C, 0x00],
        // ® (registered trademark)
        [0x3C, 0x42, 0xBD, 0xB1, 0xAD, 0x42, 0x3C, 0x00],
    ];
}
