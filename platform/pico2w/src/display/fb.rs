//! Software framebuffer `DrawTarget` for host-side testing and the
//! display-viewer tool.
//!
//! `FbDisplay` records every pixel written into a `Vec<Rgb565>` so tests can
//! assert on exact pixel values, and the viewer binary can export frames as
//! PNG files.

use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::{OriginDimensions, Size};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::RgbColor;
use embedded_graphics::Pixel;

/// A flat RGBA framebuffer that implements [`DrawTarget`].
///
/// Pixels are stored row-major: index = `y * width + x`.
pub struct FbDisplay {
    pixels: Vec<Rgb565>,
    width: u32,
    height: u32,
}

impl FbDisplay {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            pixels: vec![Rgb565::BLACK; (width * height) as usize],
            width,
            height,
        }
    }

    /// Save the current framebuffer as a PNG file.
    #[cfg(not(target_arch = "arm"))]
    pub fn save_png(&self, path: &str) -> std::io::Result<()> {
        use std::io::BufWriter;

        let file = std::fs::File::create(path)?;
        let ref mut w = BufWriter::new(file);

        let mut encoder = png::Encoder::new(w, self.width, self.height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        })?;

        let data: Vec<u8> = self.pixels.iter().flat_map(|p| {
            // Expand Rgb565 → 8-bit RGB
            let r = (p.r() << 3) | (p.r() >> 2);
            let g = (p.g() << 2) | (p.g() >> 4);
            let b = (p.b() << 3) | (p.b() >> 2);
            [r, g, b]
        }).collect();

        writer.write_image_data(&data).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::Other, e.to_string())
        })
    }
}

impl AsRef<[Rgb565]> for FbDisplay {
    fn as_ref(&self) -> &[Rgb565] {
        &self.pixels
    }
}

impl OriginDimensions for FbDisplay {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for FbDisplay {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(coord, color) in pixels {
            if coord.x >= 0
                && coord.y >= 0
                && (coord.x as u32) < self.width
                && (coord.y as u32) < self.height
            {
                let idx = coord.y as u32 * self.width + coord.x as u32;
                self.pixels[idx as usize] = color;
            }
        }
        Ok(())
    }
}
