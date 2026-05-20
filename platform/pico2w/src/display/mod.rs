//! ILI9341 display driver.
//!
//! Drives the ACEIRMC 2.8" ILI9341 SPI TFT (240×320).
//!
//! The Game Boy framebuffer (160×144) is scaled 1.5× to 240×216 and centred
//! vertically with 52-pixel letterbox bars above and below.
//!
//! `Display<D>` is generic over any `embedded-graphics` `DrawTarget<Color =
//! Rgb565>` so the rendering logic can be exercised on the host with a
//! framebuffer stub — see [`fb`].

#[cfg(any(not(target_arch = "arm"), feature = "std"))]
pub mod fb;
#[cfg(target_arch = "arm")]
pub mod hw;
pub mod font;

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{Dimensions, Point, Size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::{IntoStorage, RgbColor};
use embedded_graphics::primitives::Rectangle;

// ---------------------------------------------------------------------------
// DMG colour palette — matches the web platform exactly.
// Source: platform/web/client/src/lib.rs PALETTE constant.
// ---------------------------------------------------------------------------

/// Lightest (colour 0) — #E0F8D0
pub const C0: Rgb565 = Rgb565::new(0xE0 >> 3, 0xF8 >> 2, 0xD0 >> 3);
/// Light (colour 1) — #88C070
pub const C1: Rgb565 = Rgb565::new(0x88 >> 3, 0xC0 >> 2, 0x70 >> 3);
/// Dark (colour 2) — #346856
pub const C2: Rgb565 = Rgb565::new(0x34 >> 3, 0x68 >> 2, 0x56 >> 3);
/// Darkest (colour 3) — #081820
pub const C3: Rgb565 = Rgb565::new(0x08 >> 3, 0x18 >> 2, 0x20 >> 3);

// ---------------------------------------------------------------------------
// Geometry constants
// ---------------------------------------------------------------------------

const GB_W: i32 = 160;
const SCALED_W: i32 = 240; // 160 × 1.5
const SCALED_H: i32 = 216; // 144 × 1.5
pub const SCREEN_W: i32 = 240;
pub const SCREEN_H: i32 = 320;
const Y_OFFSET: i32 = (SCREEN_H - SCALED_H) / 2; // 52
pub const SCALED_FRAME_PIXELS: usize = (SCALED_W as usize) * (SCALED_H as usize);
pub type ScaledFrame = [u16; SCALED_FRAME_PIXELS];

// Splash animation geometry
pub const GLYPH_W: i32 = 16; // 8px × 2 scale
pub const GLYPH_H: i32 = 16;
const GAP: i32 = 2;
pub const LOGO_W: i32 = 8 * GLYPH_W + 7 * GAP; // 142
pub const LOGO_X: i32 = (SCREEN_W - LOGO_W) / 2; // 49
pub const LOGO_FINAL_Y: i32 = (SCREEN_H - GLYPH_H) / 2 - 8;
pub const LOGO_START_Y: i32 = -(GLYPH_H + 4);
const LOGO_SPEED: i32 = 4; // px per frame

// ---------------------------------------------------------------------------
// Display<D>
// ---------------------------------------------------------------------------

pub struct Display<D> {
    inner: D,
}

/// Pre-scale a Game Boy framebuffer (160×144, palette indices) to 240×216 Rgb565.
///
/// Stores raw `u16` Rgb565 storage values. Pass the result to
/// [`Display::render_game_only_scaled`] or, in Phase C, directly to DMA.
pub fn scale_to_rgb565(src: &[u8; 23040], dst: &mut ScaledFrame) {
    for sy in 0..216usize {
        let gy = sy * 2 / 3;
        let src_row = &src[gy * 160..(gy + 1) * 160];
        let dst_row = &mut dst[sy * 240..(sy + 1) * 240];
        for sx in 0..240usize {
            // Store big-endian so bytemuck::cast_slice in send_frame_raw gives
            // the correct SPI byte order without an extra copy.
            dst_row[sx] = dmg_color(src_row[sx * 2 / 3]).into_storage().swap_bytes();
        }
    }
}

impl<D: DrawTarget<Color = Rgb565> + Dimensions> Display<D> {
    pub fn from_draw_target(inner: D) -> Self {
        Self { inner }
    }

    // -----------------------------------------------------------------------
    // Frame rendering
    // -----------------------------------------------------------------------

    /// Render a Game Boy framebuffer (160×144, palette indices 0–3).
    ///
    /// Scales 1.5× to 240×216 via nearest-neighbour and fills letterbox bars.
    pub fn render_frame(&mut self, fb: &[u8; 23040]) {
        self.fill_bar(0, Y_OFFSET, C3);
        self.fill_scaled_frame(fb);
        self.fill_bar(Y_OFFSET + SCALED_H, Y_OFFSET, Rgb565::BLACK);
    }

    /// Draw the static letterbox bars above and below the game area.
    ///
    /// Call once before the game loop. Subsequent frames can use
    /// [`render_game_only`] to skip repainting the unchanging bars.
    pub fn draw_letterbox_bars(&mut self) {
        self.fill_bar(0, Y_OFFSET, C3);
        self.fill_bar(Y_OFFSET + SCALED_H, Y_OFFSET, Rgb565::BLACK);
    }

    /// Render only the scaled Game Boy frame (240×216), skipping letterbox bars.
    ///
    /// Saves ~6.4 ms per frame vs [`render_frame`] by not repainting the static
    /// top/bottom bars. Call [`draw_letterbox_bars`] once before the game loop.
    pub fn render_game_only(&mut self, fb: &[u8; 23040]) {
        self.fill_scaled_frame(fb);
    }

    /// Render a pre-scaled 240×216 frame from a [`scale_to_rgb565`] buffer.
    ///
    /// Eliminates per-pixel scaling work during the SPI transfer. The buffer
    /// holds raw `Rgb565` storage values produced by [`scale_to_rgb565`].
    pub fn render_game_only_scaled(&mut self, buf: &ScaledFrame) {
        use embedded_graphics::pixelcolor::raw::RawU16;
        let rect = Rectangle::new(
            Point::new(0, Y_OFFSET),
            Size::new(SCALED_W as u32, SCALED_H as u32),
        );
        // buf stores big-endian u16s; swap back to native-endian before handing
        // to mipidsi, which handles its own byte ordering.
        let _ = self.inner.fill_contiguous(
            &rect,
            buf.iter()
                .map(|&v| Rgb565::from(RawU16::new(v.swap_bytes()))),
        );
    }

    fn fill_bar(&mut self, y: i32, height: i32, color: Rgb565) {
        if height <= 0 {
            return;
        }
        let rect = Rectangle::new(Point::new(0, y), Size::new(SCREEN_W as u32, height as u32));
        let _ = self.inner.fill_contiguous(&rect, core::iter::repeat(color));
    }

    fn fill_scaled_frame(&mut self, fb: &[u8; 23040]) {
        let rect = Rectangle::new(
            Point::new(0, Y_OFFSET),
            Size::new(SCALED_W as u32, SCALED_H as u32),
        );
        let pixels = (0..SCALED_W * SCALED_H).map(|i| {
            let sx = i % SCALED_W;
            let sy = i / SCALED_W;
            let gx = (sx * 2 / 3) as usize;
            let gy = (sy * 2 / 3) as usize;
            dmg_color(fb[gy * GB_W as usize + gx])
        });
        let _ = self.inner.fill_contiguous(&rect, pixels);
    }

    // -----------------------------------------------------------------------
    // Splash animation
    // -----------------------------------------------------------------------

    /// Draw one frame of the splash animation.
    ///
    /// Returns `true` on the final frame (logo settled + hold complete).
    /// Call at ~60 Hz; `frame` is the monotonically incrementing frame index.
    pub fn splash_step(&mut self, frame: u32) -> bool {
        let travel = (LOGO_FINAL_Y - LOGO_START_Y).max(1);
        let total_travel_frames = (travel / LOGO_SPEED + 1) as u32;
        // Hold for ~2 s (120 frames at 60 Hz) after travel completes
        const HOLD_FRAMES: u32 = 120;
        let total_frames = total_travel_frames + HOLD_FRAMES;

        if frame == 0 {
            self.clear(C1);
        }

        if frame <= total_travel_frames {
            let delta = ((frame as i32) * LOGO_SPEED).min(travel);
            let logo_y = LOGO_START_Y + delta;

            // Clear the vacated strip above the logo
            let prev_y = LOGO_START_Y + ((frame as i32 - 1) * LOGO_SPEED).max(0);
            self.clear_strip(prev_y.max(0), logo_y.max(0));

            let show_reg = delta >= travel;
            self.draw_logo(LOGO_X, logo_y, show_reg, C3, C1);
        }

        frame >= total_frames
    }

    fn clear_strip(&mut self, top: i32, bot: i32) {
        if bot > top {
            self.fill_bar(top, bot - top, C1);
        }
    }

    // -----------------------------------------------------------------------
    // Primitives
    // -----------------------------------------------------------------------

    pub fn clear(&mut self, color: Rgb565) {
        let rect = Rectangle::new(
            Point::new(0, 0),
            Size::new(SCREEN_W as u32, SCREEN_H as u32),
        );
        let _ = self.inner.fill_contiguous(&rect, core::iter::repeat(color));
    }

    /// Draw the full VINTENDO logo as a single `fill_contiguous` burst.
    pub fn draw_logo(&mut self, x: i32, y: i32, show_reg: bool, fg: Rgb565, bg: Rgb565) {
        const SEQUENCE: [usize; 8] = [0, 1, 2, 3, 4, 2, 5, 6];
        const SLOT_W: i32 = GLYPH_W + GAP;

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
                    let gx = slot_x / 2;
                    let gy = ry / 2;
                    let row = logo_font::GLYPHS[SEQUENCE[slot]][gy as usize];
                    if (row >> (7 - gx)) & 1 == 1 {
                        fg
                    } else {
                        bg
                    }
                } else {
                    bg
                }
            })
        });
        let _ = self.inner.fill_contiguous(&rect, colors);

        if show_reg {
            self.draw_glyph_1x(7, x + LOGO_W + 1, y, fg, bg);
        }
    }

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
        let bitmap = logo_font::GLYPHS[idx];
        let colors = (y0..y1).flat_map(move |py2| {
            let row = bitmap[(py2 - py) as usize];
            (x0..x1).map(move |px2| {
                if (row >> (7 - (px2 - px))) & 1 == 1 {
                    fg
                } else {
                    bg
                }
            })
        });
        let _ = self.inner.fill_contiguous(&rect, colors);
    }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// FbDisplay convenience — PNG export
// ---------------------------------------------------------------------------

#[cfg(not(target_arch = "arm"))]
impl Display<fb::FbDisplay> {
    /// Save the current framebuffer as a PNG file.
    pub fn save_png(&self, path: &str) -> std::io::Result<()> {
        self.inner.save_png(path)
    }
}

/// Map a DMG palette index (0–3) to an RGB565 colour.
#[inline(always)]
pub fn dmg_color(idx: u8) -> Rgb565 {
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

mod logo_font {
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
        // ®
        [0x3C, 0x42, 0xBD, 0xB1, 0xAD, 0x42, 0x3C, 0x00],
    ];
}

// ---------------------------------------------------------------------------
// Menu rendering
// ---------------------------------------------------------------------------

use crate::menu::MenuFrame;

// DMG palette in big-endian RGB565 — the byte order the ILI9341 SPI bus expects.
const C0_BE: [u8; 2] = [0xE7, 0xDA]; // #E0F8D0
const C1_BE: [u8; 2] = [0x8E, 0x0E]; // #88C070
const C2_BE: [u8; 2] = [0x33, 0x4A]; // #346856
const C3_BE: [u8; 2] = [0x08, 0xC4]; // #081820

// Layout constants (pixels on the 240×320 display).
const HEADER_H:        u16 = 40;
const SEPARATOR_H:     u16 = 4;
const ITEMS_START_Y:   u16 = HEADER_H + SEPARATOR_H;
const ITEM_H:          u16 = 32;
const ITEM_TEXT_PAD:   u16 = 8;  // top padding within each item slot
const FOOTER_SEP_Y:    u16 = 280;
const FOOTER_Y:        u16 = FOOTER_SEP_Y + SEPARATOR_H;
const CURSOR_X:        usize = 10;
const ITEM_TEXT_X:     usize = 28;
const MARKER_X:        usize = 196; // right-side "loaded" indicator column
const CHAR_W:          usize = 8;   // glyph width in pixels
const SCALE:           usize = 2;   // render scale for items and title

/// Render one horizontal scan-line of a menu into `row` (240 pixels × 2 bytes BE).
///
/// Call for every `y` in `0..320` to produce a complete menu frame.
/// The buffer is entirely overwritten each call.
pub fn render_menu_row(frame: &MenuFrame<'_>, y: u16, row: &mut [u8; 480]) {
    let in_top_sep    = y >= HEADER_H && y < HEADER_H + SEPARATOR_H;
    let in_footer_sep = y >= FOOTER_SEP_Y && y < FOOTER_Y;
    let in_items      = y >= ITEMS_START_Y && y < FOOTER_SEP_Y;

    fill_row(row, if in_top_sep || in_footer_sep { C2_BE } else { C3_BE });

    if y < HEADER_H       { render_menu_header_row(frame.title, y, row); }
    if in_items           { render_menu_items_row(frame, y, row); }
    if y >= FOOTER_Y      { render_menu_footer_row(y, row); }
}

fn render_menu_header_row(title: &str, y: u16, row: &mut [u8; 480]) {
    let title_screen_h = (8 * SCALE) as u16;
    let title_top = (HEADER_H - title_screen_h) / 2;
    if y < title_top || y >= title_top + title_screen_h { return; }
    let glyph_row = ((y - title_top) as usize) / SCALE;
    let title_screen_w = (title.len() * CHAR_W * SCALE) as usize;
    let title_x = (SCREEN_W as usize).saturating_sub(title_screen_w) / 2;
    write_text_row(row, title.as_bytes(), title_x, glyph_row, SCALE, C0_BE, C3_BE);
}

fn render_menu_items_row(frame: &MenuFrame<'_>, y: u16, row: &mut [u8; 480]) {
    let slot = ((y - ITEMS_START_Y) / ITEM_H) as usize;
    if slot >= frame.items.len() { return; }
    let slot_top = ITEMS_START_Y + slot as u16 * ITEM_H;
    let selected = slot == frame.selected;
    let enabled  = frame.enabled.get(slot).copied().unwrap_or(true);

    fill_row(row, if selected { C2_BE } else { C3_BE });

    let text_top    = slot_top + ITEM_TEXT_PAD;
    let text_bottom = text_top + (8 * SCALE) as u16;
    if y < text_top || y >= text_bottom { return; }

    let glyph_row  = ((y - text_top) as usize) / SCALE;
    let text_color = if !enabled { C2_BE } else if selected { C0_BE } else { C1_BE };
    let item_bg    = if selected { C2_BE } else { C3_BE };

    let cursor: &[u8] = if selected { b">" } else { b" " };
    write_text_row(row, cursor, CURSOR_X, glyph_row, SCALE, text_color, item_bg);
    write_text_row(row, frame.items[slot].as_bytes(), ITEM_TEXT_X, glyph_row, SCALE, text_color, item_bg);
    if frame.marked == Some(slot) {
        write_text_row(row, b"*", MARKER_X, glyph_row, SCALE, C0_BE, item_bg);
    }
}

fn render_menu_footer_row(y: u16, row: &mut [u8; 480]) {
    const FOOTER: &[u8] = b"A:SELECT  B:BACK";
    let footer_text_h = 8u16;
    let footer_top = FOOTER_Y + (SCREEN_H as u16 - FOOTER_Y - footer_text_h) / 2;
    if y < footer_top || y >= footer_top + footer_text_h { return; }
    let glyph_row = (y - footer_top) as usize;
    let footer_w  = FOOTER.len() * CHAR_W;
    let footer_x  = (SCREEN_W as usize).saturating_sub(footer_w) / 2;
    write_text_row(row, FOOTER, footer_x, glyph_row, 1, C1_BE, C3_BE);
}

// ---------------------------------------------------------------------------
// Loading screen rendering
// ---------------------------------------------------------------------------

const LOADING_FILENAME_Y:    u16 = 140;
const LOADING_BAR_TOP:       u16 = 200;
const LOADING_BAR_BOTTOM:    u16 = 220;
const LOADING_BAR_X0:        usize = 20;
const LOADING_BAR_X1:        usize = 220;

/// Render one scan-line of the ROM loading screen.
///
/// `banks_done` / `total_banks` drives the progress bar fill (0–100%).
/// Call for every `y` in `0..320` to produce a complete frame.
pub fn render_loading_row(
    filename: &str,
    banks_done: u32,
    total_banks: u32,
    y: u16,
    row: &mut [u8; 480],
) {
    let in_top_sep = y >= HEADER_H && y < HEADER_H + SEPARATOR_H;
    fill_row(row, if in_top_sep { C2_BE } else { C3_BE });

    if y < HEADER_H                                          { render_loading_header_row(y, row); }
    if y >= LOADING_FILENAME_Y && y < LOADING_FILENAME_Y + 8 { render_loading_filename_row(filename, y, row); }
    if y >= LOADING_BAR_TOP    && y < LOADING_BAR_BOTTOM    { render_loading_bar_row(banks_done, total_banks, row); }
}

fn render_loading_header_row(y: u16, row: &mut [u8; 480]) {
    let title_screen_h = (8 * SCALE) as u16;
    let title_top = (HEADER_H - title_screen_h) / 2;
    if y < title_top || y >= title_top + title_screen_h { return; }
    let glyph_row = ((y - title_top) as usize) / SCALE;
    let title = b"LOADING";
    let title_w = title.len() * CHAR_W * SCALE;
    let title_x = (SCREEN_W as usize).saturating_sub(title_w) / 2;
    write_text_row(row, title, title_x, glyph_row, SCALE, C0_BE, C3_BE);
}

fn render_loading_filename_row(filename: &str, y: u16, row: &mut [u8; 480]) {
    let glyph_row = (y - LOADING_FILENAME_Y) as usize;
    let text = filename.as_bytes();
    let text_w = text.len() * CHAR_W;
    let text_x = (SCREEN_W as usize).saturating_sub(text_w) / 2;
    write_text_row(row, text, text_x, glyph_row, 1, C1_BE, C3_BE);
}

fn render_loading_bar_row(banks_done: u32, total_banks: u32, row: &mut [u8; 480]) {
    let bar_w = LOADING_BAR_X1 - LOADING_BAR_X0;
    let filled = if total_banks > 0 {
        (bar_w as u64 * banks_done as u64 / total_banks as u64) as usize
    } else {
        0
    };
    for px in LOADING_BAR_X0..LOADING_BAR_X1 {
        row[px * 2]     = C2_BE[0];
        row[px * 2 + 1] = C2_BE[1];
    }
    for px in LOADING_BAR_X0..LOADING_BAR_X0 + filled.min(bar_w) {
        row[px * 2]     = C0_BE[0];
        row[px * 2 + 1] = C0_BE[1];
    }
}

/// Fill every pixel in `row` with `color` (big-endian RGB565).
fn fill_row(row: &mut [u8; 480], color: [u8; 2]) {
    let mut i = 0;
    while i < 480 {
        row[i]     = color[0];
        row[i + 1] = color[1];
        i += 2;
    }
}

/// Write `text` into `row` at horizontal pixel offset `x_start`.
///
/// `glyph_row` selects which of the 8 bitmap rows to render.
/// `scale` is 1 or 2 (each source pixel becomes `scale` screen pixels wide).
fn write_text_row(
    row:       &mut [u8; 480],
    text:      &[u8],
    x_start:   usize,
    glyph_row: usize,
    scale:     usize,
    fg:        [u8; 2],
    bg:        [u8; 2],
) {
    let char_screen_w = CHAR_W * scale;
    for (char_idx, &ch) in text.iter().enumerate() {
        let bitmap = font::glyph_row(ch, glyph_row);
        let char_x = x_start + char_idx * char_screen_w;
        for glyph_x in 0..CHAR_W {
            // Font bitmaps use bit 0 = leftmost pixel (LSB-first convention).
            let lit = (bitmap >> glyph_x) & 1 != 0;
            let color = if lit { fg } else { bg };
            for sx in 0..scale {
                let px = char_x + glyph_x * scale + sx;
                if px < SCREEN_W as usize {
                    row[px * 2]     = color[0];
                    row[px * 2 + 1] = color[1];
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::fb::FbDisplay;
    use super::*;
    use embedded_graphics::prelude::IntoStorage;

    fn make_display() -> Display<FbDisplay> {
        Display::from_draw_target(FbDisplay::new(SCREEN_W as u32, SCREEN_H as u32))
    }

    #[test]
    fn dmg_color_palette() {
        assert_eq!(dmg_color(0), C0);
        assert_eq!(dmg_color(1), C1);
        assert_eq!(dmg_color(2), C2);
        assert_eq!(dmg_color(3), C3);
        assert_eq!(dmg_color(4), C3); // out-of-range → darkest
    }

    #[test]
    fn render_frame_letterbox_colors() {
        let mut disp = make_display();
        let fb = [0u8; 23040]; // all C0

        disp.render_frame(&fb);

        let fb_ref = disp.inner.as_ref();
        // Top bar should be C3
        assert_eq!(fb_ref[0], C3, "top bar pixel should be C3");
        // Bottom bar: first pixel after scaled region
        let bot_start = ((Y_OFFSET + SCALED_H) * SCREEN_W) as usize;
        assert_eq!(
            fb_ref[bot_start],
            Rgb565::BLACK,
            "bottom bar pixel should be black"
        );
        // A pixel inside the scaled region should be C0
        let mid = (Y_OFFSET * SCREEN_W) as usize;
        assert_eq!(fb_ref[mid], C0, "game region pixel should be C0");
    }

    #[test]
    fn render_frame_uses_all_palette_entries() {
        let mut disp = make_display();
        // Each quarter of the FB uses a different palette index
        let mut fb = [0u8; 23040];
        for i in 0..23040 {
            fb[i] = (i / (23040 / 4)) as u8;
        }
        disp.render_frame(&fb);
        let pixels = disp.inner.as_ref();
        let colors_present: std::collections::HashSet<u16> =
            pixels.iter().map(|p| p.into_storage()).collect();
        // All four DMG colours should appear
        for c in [C0, C1, C2, C3] {
            assert!(
                colors_present.contains(&c.into_storage()),
                "missing palette entry"
            );
        }
    }

    #[test]
    fn splash_step_first_frame_clears_to_c1() {
        let mut disp = make_display();
        disp.splash_step(0);
        // Every pixel should be C1 (background) or C3 (logo above screen)
        for &px in disp.inner.as_ref() {
            assert!(px == C1 || px == C3, "unexpected pixel {:?} on frame 0", px);
        }
    }

    #[test]
    fn splash_step_returns_true_after_hold() {
        let mut disp = make_display();
        let travel = (LOGO_FINAL_Y - LOGO_START_Y).max(1);
        let travel_frames = (travel / LOGO_SPEED + 1) as u32;
        let total = travel_frames + 120;

        assert!(!disp.splash_step(total - 1), "should still be animating");
        assert!(disp.splash_step(total), "should be done on last frame");
    }

    #[test]
    fn draw_letterbox_bars_colors() {
        let mut disp = make_display();
        disp.draw_letterbox_bars();
        let fb_ref = disp.inner.as_ref();
        assert_eq!(fb_ref[0], C3, "top bar should be C3");
        let bot_start = ((Y_OFFSET + SCALED_H) * SCREEN_W) as usize;
        assert_eq!(
            fb_ref[bot_start],
            Rgb565::BLACK,
            "bottom bar should be black"
        );
    }

    #[test]
    fn render_game_only_updates_game_region() {
        let mut disp = make_display();
        let fb = [1u8; 23040]; // all palette index 1 → C1
        disp.render_game_only(&fb);
        let fb_ref = disp.inner.as_ref();
        let mid = (Y_OFFSET * SCREEN_W) as usize;
        assert_eq!(fb_ref[mid], C1, "game region pixel should be C1");
    }

    #[test]
    fn draw_logo_contains_fg_and_bg() {
        let mut disp = make_display();
        disp.draw_logo(LOGO_X, LOGO_FINAL_Y, false, C3, C1);
        let pixels = disp.inner.as_ref();
        let has_fg = pixels.iter().any(|&p| p == C3);
        let has_bg = pixels.iter().any(|&p| p == C1);
        assert!(has_fg, "logo must contain foreground colour");
        assert!(has_bg, "logo must contain background colour");
    }
}
