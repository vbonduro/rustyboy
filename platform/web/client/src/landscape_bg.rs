// Animated title-screen background generated from gemini_generated_video_C5B695AC.mov.
// Source frames are 160x144 at 12 fps with a shared 8-color palette.
//
// ============================================================================
// RBG1 binary format (landscape_bg.bin)
// ============================================================================
//
// All multi-byte integers are little-endian.
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ HEADER  (14 bytes)                                                      │
// ├──────────┬───────┬─────────────────────────────────────────────────────┤
// │ Offset   │ Type  │ Field                                                │
// ├──────────┼───────┼─────────────────────────────────────────────────────┤
// │  0 –  3  │ [u8;4]│ magic = b"RBG1"                                     │
// │  4 –  5  │ u16   │ width         (160)                                  │
// │  6 –  7  │ u16   │ height        (144)                                  │
// │  8 –  9  │ u16   │ frame_count   (120)                                  │
// │ 10       │ u8    │ palette_len   (64 entries)                           │
// │ 11       │ u8    │ bits_per_index (3)                                   │
// │ 12 – 13  │ u16   │ static_rows   (96)                                   │
// └──────────┴───────┴─────────────────────────────────────────────────────┘
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ PALETTE  (palette_len × 4 bytes = 256 bytes)                           │
// │ palette_len RGBA entries, each 4 bytes.  Only the first 8 entries are  │
// │ referenced by the pixel data (indices 0–7 fit in 3 bits).              │
// └─────────────────────────────────────────────────────────────────────────┘
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ STATIC REGION  (ceil(static_rows × width × 3 / 8) bytes = 5 760 B)    │
// │ A single copy of the top `static_rows` rows, which are pixel-identical  │
// │ across all 120 frames (sky + RUSTYBOY title are frozen).               │
// │ Encoding: indices packed LSB-first into a flat bit stream.             │
// │   pixel p → bits [p*3 .. p*3+2] in the stream,                        │
// │   bit i   → (stream[i/8] >> (i%8)) & 1                                │
// └─────────────────────────────────────────────────────────────────────────┘
//
// ┌─────────────────────────────────────────────────────────────────────────┐
// │ ANIMATED REGION  (frame_count × ceil(anim_rows × width × 3 / 8) bytes) │
// │   = 120 × 2 880 = 345 600 bytes                                        │
// │ frame_count back-to-back slabs, each covering only the bottom          │
// │ `anim_rows = height - static_rows` rows (the beach / crab animation).  │
// │ Same LSB-first 3-bit packing as the static region.                     │
// └─────────────────────────────────────────────────────────────────────────┘
//
// Total: 14 + 256 + 5 760 + 345 600 = 351 630 bytes  (was 2 765 056 bytes)
// ============================================================================

pub const FRAME_COUNT: usize = 120;
pub const FRAME_DURATION_MS: u32 = 83; // 12 fps

pub static LANDSCAPE_BG_DATA: &[u8] = include_bytes!("landscape_bg.bin");

// ────────────────────────────────────────────────────────────────────────────
// Struct + parse
// ────────────────────────────────────────────────────────────────────────────

/// Zero-copy, borrowed view of the embedded RBG1 landscape background data.
///
/// Call [`LandscapeBg::parse`] on [`LANDSCAPE_BG_DATA`] (or any RBG1 byte
/// slice) to obtain a value.  The struct holds only slices into the original
/// bytes — no heap allocation occurs during parsing or rendering.
pub struct LandscapeBg<'a> {
    width: usize,
    static_rows: usize,
    anim_rows: usize,
    frame_count: usize,
    /// RGBA bytes for `palette_len` entries, indexed 0..palette_len.
    palette: &'a [u8],
    /// 3-bit-packed pixel indices for the static top region.
    static_data: &'a [u8],
    /// 3-bit-packed pixel indices for all animated frames, concatenated.
    anim_data: &'a [u8],
    anim_bytes_per_frame: usize,
}

impl<'a> LandscapeBg<'a> {
    /// Parse `bytes` as an RBG1 blob.  Validates the magic word and the major
    /// structural constants, then slices the palette / static / animated
    /// sections in O(1) without copying any pixel data.
    ///
    /// # Panics
    /// Panics on a malformed header (wrong magic, out-of-bounds slices).
    pub fn parse(bytes: &'a [u8]) -> Self {
        const HEADER_LEN: usize = 14;
        assert!(
            bytes.len() >= HEADER_LEN,
            "RBG1: too short for header ({} bytes)",
            bytes.len()
        );
        assert_eq!(&bytes[0..4], b"RBG1", "RBG1: bad magic");

        let width = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let height = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        let frame_count = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let palette_len = bytes[10] as usize;
        let bits_per_index = bytes[11] as usize;
        let static_rows = u16::from_le_bytes([bytes[12], bytes[13]]) as usize;

        assert_eq!(bits_per_index, 3, "RBG1: only 3-bit index packing is supported");
        assert!(static_rows <= height, "RBG1: static_rows > height");

        let anim_rows = height - static_rows;
        let static_bytes = (static_rows * width * 3 + 7) / 8;
        let anim_bytes_per_frame = (anim_rows * width * 3 + 7) / 8;
        let anim_total = anim_bytes_per_frame * frame_count;

        let mut off = HEADER_LEN;

        let palette = &bytes[off..off + palette_len * 4];
        off += palette_len * 4;

        let static_data = &bytes[off..off + static_bytes];
        off += static_bytes;

        let anim_data = &bytes[off..off + anim_total];

        Self {
            width,
            static_rows,
            anim_rows,
            frame_count,
            palette,
            static_data,
            anim_data,
            anim_bytes_per_frame,
        }
    }

    /// Render frame `frame` into the caller-provided RGBA buffer `out`.
    ///
    /// `out` must be exactly `width * height * 4` bytes (160 × 144 × 4 =
    /// 92 160 bytes).  The buffer is filled in row-major order, top to bottom.
    /// The top `static_rows` rows are taken from the frozen static region;
    /// the remaining `anim_rows` rows come from the per-frame animated region.
    ///
    /// No heap allocation is performed.
    pub fn render_frame(&self, frame: usize, out: &mut [u8]) {
        let frame = frame % self.frame_count;
        let total_pixels = (self.static_rows + self.anim_rows) * self.width;
        debug_assert_eq!(
            out.len(),
            total_pixels * 4,
            "render_frame: output buffer wrong size"
        );

        // Static region — top rows, identical for every frame.
        let static_pixels = self.static_rows * self.width;
        for p in 0..static_pixels {
            let idx = unpack_3bit(self.static_data, p) as usize;
            let ci = idx * 4;
            out[p * 4..p * 4 + 4].copy_from_slice(&self.palette[ci..ci + 4]);
        }

        // Animated region — bottom rows, varies per frame.
        let anim_start = frame * self.anim_bytes_per_frame;
        let anim_slice =
            &self.anim_data[anim_start..anim_start + self.anim_bytes_per_frame];
        let anim_pixels = self.anim_rows * self.width;
        let out_base = static_pixels * 4;
        for p in 0..anim_pixels {
            let idx = unpack_3bit(anim_slice, p) as usize;
            let ci = idx * 4;
            out[out_base + p * 4..out_base + p * 4 + 4]
                .copy_from_slice(&self.palette[ci..ci + 4]);
        }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Bit-unpacking helper
// ────────────────────────────────────────────────────────────────────────────

/// Extract the 3-bit index for pixel `pixel` from an LSB-first packed stream.
/// Pixel p occupies bits [p*3 .. p*3+2]; bit i is stored as
/// `(stream[i/8] >> (i%8)) & 1`.
#[inline(always)]
fn unpack_3bit(data: &[u8], pixel: usize) -> u8 {
    let bit_pos = pixel * 3;
    let byte_idx = bit_pos >> 3;
    let bit_off = bit_pos & 7;
    let lo = data[byte_idx] >> bit_off;
    if bit_off <= 5 {
        // All 3 bits fit within one byte.
        lo & 0x7
    } else {
        // Spans two bytes (bit_off is 6 or 7).
        let hi = data[byte_idx + 1];
        (lo | (hi << (8 - bit_off))) & 0x7
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect the set of unique RGBA values from the palette (only the first 8
    /// entries are referenced — indices 0–7 fit in 3 bits).
    fn palette_colors(bg: &LandscapeBg) -> Vec<[u8; 4]> {
        (0..8)
            .map(|i| {
                let ci = i * 4;
                [
                    bg.palette[ci],
                    bg.palette[ci + 1],
                    bg.palette[ci + 2],
                    bg.palette[ci + 3],
                ]
            })
            .collect()
    }

    #[test]
    fn decode_frames_correct_length_and_palette() {
        let bg = LandscapeBg::parse(LANDSCAPE_BG_DATA);
        let colors = palette_colors(&bg);
        let mut out = vec![0u8; 160 * 144 * 4];

        for &frame in &[0usize, 60usize] {
            bg.render_frame(frame, &mut out);

            assert_eq!(out.len(), 160 * 144 * 4, "frame {frame}: wrong output length");

            // Every pixel RGBA must be one of the 8 palette colors.
            for px in 0..160 * 144 {
                let rgba = [
                    out[px * 4],
                    out[px * 4 + 1],
                    out[px * 4 + 2],
                    out[px * 4 + 3],
                ];
                assert!(
                    colors.contains(&rgba),
                    "frame {frame} pixel {px}: RGBA {rgba:?} not in palette"
                );
            }
        }
    }

    #[test]
    fn static_rows_identical_across_frames() {
        // Verify the static assumption: frame 0 top rows == frame 60 top rows.
        let bg = LandscapeBg::parse(LANDSCAPE_BG_DATA);
        let mut f0 = vec![0u8; 160 * 144 * 4];
        let mut f60 = vec![0u8; 160 * 144 * 4];

        bg.render_frame(0, &mut f0);
        bg.render_frame(60, &mut f60);

        let static_bytes = bg.static_rows * 160 * 4;
        assert_eq!(
            &f0[..static_bytes],
            &f60[..static_bytes],
            "static region differs between frame 0 and frame 60"
        );
    }

    #[test]
    fn frame_count_constant_matches_header() {
        let bg = LandscapeBg::parse(LANDSCAPE_BG_DATA);
        assert_eq!(bg.frame_count, FRAME_COUNT);
    }
}
