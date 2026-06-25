//! Rgb565 framebuffer DrawTarget for the ratatui/mousefood menu layer.
//!
//! `FbTarget` wraps a raw `*mut [u16; 240*320]` flat framebuffer storing native
//! RGB565 words and implements `DrawTarget<Color = Rgb565>` + `OriginDimensions`.
//! Every pixel write extends an internal dirty y-band; `take_dirty()` returns
//! and clears that band so `hw.rs` can DMA only the changed rows to the panel.
//!
//! The buffer is a **raw pointer**, not a `&'static mut`, because the same
//! backing memory is shared (in time, never concurrently) with the game-frame
//! pre-scale buffer in `hw.rs`.  Holding two overlapping `&'static mut` would be
//! instant UB (aliasing); instead each method materializes a short-lived `&mut`
//! that is dropped before control returns, so only one live borrow of the shared
//! region exists at any instant.  See the SAFETY note in
//! `GameDisplay::new_after_splash`.
//!
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{OriginDimensions, Size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::IntoStorage;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::Pixel;

use super::{RenderWindow, SCREEN_H, SCREEN_W};

/// Number of pixels in the full 240×320 framebuffer.
pub const FB_PIXELS: usize = SCREEN_W as usize * SCREEN_H as usize;

/// A 240×320 RGB565 framebuffer backed by raw `u16` storage.
///
/// Tracks the minimum/maximum Y row written since the last `take_dirty` call so
/// the async SPI flush only transfers the changed band.
pub struct FbTarget {
    /// Raw pointer to the framebuffer pixels (native-endian RGB565 words).
    ///
    /// Row y, column x: element `y * SCREEN_W + x`.  Held as a raw pointer so
    /// the same backing memory can be shared with the game-frame pre-scale
    /// buffer; each method derefs it to a transient `&mut`/`&` (see module docs).
    buf: *mut [u16; FB_PIXELS],
    /// Inclusive dirty Y range: `(y_min, y_max)`.  `None` = nothing written.
    dirty: Option<(u16, u16)>,
}

impl FbTarget {
    /// Construct from a raw pointer to the shared framebuffer region.
    ///
    /// Called once from `GameDisplay::new_after_splash`.  The pointer must be
    /// valid for the lifetime of the program and must not be mutably accessed
    /// through any other live `&mut` while a render is in flight (the game-frame
    /// and menu paths are mutually exclusive — see the SAFETY note there).
    pub fn new(buf: *mut [u16; FB_PIXELS]) -> Self {
        Self { buf, dirty: None }
    }

    /// Materialize a transient mutable slice view of the framebuffer.
    ///
    /// # Safety
    /// No other `&mut` to the shared region may be live (guaranteed by the
    /// game/menu mutual-exclusion invariant).
    #[inline(always)]
    fn buf_mut(&mut self) -> &mut [u16; FB_PIXELS] {
        // SAFETY: `buf` is valid for the program lifetime; the game-frame path
        // (the only other user of this memory) is never in flight during a menu
        // render, so this is the sole live `&mut` to the region.
        unsafe { &mut *self.buf }
    }

    /// Mark a row as dirty, widening the tracked band.
    #[inline(always)]
    fn mark_dirty_row(&mut self, y: i32) {
        if y < 0 || y >= SCREEN_H {
            return;
        }
        let y = y as u16;
        self.dirty = Some(match self.dirty {
            None => (y, y),
            Some((mn, mx)) => (mn.min(y), mx.max(y)),
        });
    }

    /// Return the dirty [`RenderWindow`] (exclusive y_end) and clear the band.
    ///
    /// Returns `None` when nothing has been written since the last call.
    pub fn take_dirty(&mut self) -> Option<RenderWindow> {
        let (y_min, y_max) = self.dirty.take()?;
        Some(RenderWindow::full_width_rows(y_min, y_max + 1))
    }

    /// Force the entire screen into the dirty band.
    ///
    /// Call before the first `terminal.draw()` after returning from the game
    /// (so stale game pixels under the menu area are repainted).
    pub fn mark_all_dirty(&mut self) {
        self.dirty = Some((0, SCREEN_H as u16 - 1));
    }

    /// Read a row of native-endian RGB565 words into `dst_be` as big-endian
    /// bytes for SPI transmission.
    ///
    /// `dst_be` must be exactly 480 bytes (240 pixels × 2 bytes).
    pub fn read_row_be(&self, y: u16, dst_be: &mut [u8; 480]) {
        let row_start = y as usize * SCREEN_W as usize;
        // SAFETY: shared-region invariant (no concurrent game-frame write); this
        // is a transient shared view dropped before the function returns.
        let buf = unsafe { &*self.buf };
        let row = &buf[row_start..row_start + SCREEN_W as usize];
        for (i, &px) in row.iter().enumerate() {
            let bytes = px.to_be_bytes();
            dst_be[i * 2] = bytes[0];
            dst_be[i * 2 + 1] = bytes[1];
        }
    }
}

impl OriginDimensions for FbTarget {
    fn size(&self) -> Size {
        Size::new(SCREEN_W as u32, SCREEN_H as u32)
    }
}

impl DrawTarget for FbTarget {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(pt, color) in pixels {
            if pt.x < 0 || pt.x >= SCREEN_W || pt.y < 0 || pt.y >= SCREEN_H {
                continue;
            }
            let idx = pt.y as usize * SCREEN_W as usize + pt.x as usize;
            self.buf_mut()[idx] = color.into_storage();
            self.mark_dirty_row(pt.y);
        }
        Ok(())
    }

    fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        let x0 = area.top_left.x.max(0);
        let y0 = area.top_left.y.max(0);
        let x1 = (area.top_left.x + area.size.width as i32).min(SCREEN_W);
        let y1 = (area.top_left.y + area.size.height as i32).min(SCREEN_H);
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let mut colors = colors.into_iter();
        for ay in area.top_left.y..area.top_left.y + area.size.height as i32 {
            for ax in area.top_left.x..area.top_left.x + area.size.width as i32 {
                let c = match colors.next() {
                    Some(c) => c,
                    None => return Ok(()),
                };
                if ax < 0 || ax >= SCREEN_W || ay < 0 || ay >= SCREEN_H {
                    continue;
                }
                let idx = ay as usize * SCREEN_W as usize + ax as usize;
                self.buf_mut()[idx] = c.into_storage();
                self.mark_dirty_row(ay);
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let x0 = area.top_left.x.max(0) as usize;
        let y0 = area.top_left.y.max(0) as usize;
        let x1 = (area.top_left.x + area.size.width as i32).min(SCREEN_W) as usize;
        let y1 = (area.top_left.y + area.size.height as i32).min(SCREEN_H) as usize;
        if x1 <= x0 || y1 <= y0 {
            return Ok(());
        }
        let buf = self.buf_mut();
        for y in y0..y1 {
            let row_start = y * SCREEN_W as usize;
            for x in x0..x1 {
                buf[row_start + x] = color.into_storage();
            }
        }
        for y in y0..y1 {
            self.mark_dirty_row(y as i32);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::{geometry::Point, pixelcolor::RgbColor, primitives::Rectangle};
    use std::convert::TryInto;

    fn backing() -> Box<[u16; FB_PIXELS]> {
        vec![0u16; FB_PIXELS]
            .into_boxed_slice()
            .try_into()
            .ok()
            .unwrap()
    }

    #[test]
    fn draw_iter_writes_native_rgb565_storage_and_tracks_dirty_rows() {
        let mut backing = backing();
        let mut target = FbTarget::new(backing.as_mut() as *mut [u16; FB_PIXELS]);

        target
            .draw_iter([
                Pixel(Point::new(5, 7), Rgb565::RED),
                Pixel(Point::new(9, 3), Rgb565::GREEN),
                Pixel(Point::new(-1, 4), Rgb565::BLUE),
            ])
            .unwrap();

        assert_eq!(
            backing[7 * SCREEN_W as usize + 5],
            Rgb565::RED.into_storage()
        );
        assert_eq!(
            backing[3 * SCREEN_W as usize + 9],
            Rgb565::GREEN.into_storage()
        );
        assert_eq!(
            target.take_dirty(),
            Some(RenderWindow::full_width_rows(3, 8))
        );
        assert_eq!(target.take_dirty(), None);
    }

    #[test]
    fn fill_solid_tracks_clipped_dirty_band() {
        let mut backing = backing();
        let mut target = FbTarget::new(backing.as_mut() as *mut [u16; FB_PIXELS]);

        target
            .fill_solid(
                &Rectangle::new(Point::new(238, 318), Size::new(8, 8)),
                Rgb565::BLUE,
            )
            .unwrap();

        assert_eq!(
            backing[318 * SCREEN_W as usize + 238],
            Rgb565::BLUE.into_storage()
        );
        assert_eq!(
            target.take_dirty(),
            Some(RenderWindow::full_width_rows(318, 320))
        );
    }

    #[test]
    fn read_row_be_exports_numeric_rgb565_in_panel_byte_order() {
        let mut backing = backing();
        backing[0] = Rgb565::RED.into_storage();
        backing[1] = Rgb565::GREEN.into_storage();
        backing[2] = Rgb565::BLUE.into_storage();
        let target = FbTarget::new(backing.as_mut() as *mut [u16; FB_PIXELS]);

        let mut row = [0u8; 480];
        target.read_row_be(0, &mut row);

        assert_eq!(&row[0..6], &[0xF8, 0x00, 0x07, 0xE0, 0x00, 0x1F]);
    }
}
