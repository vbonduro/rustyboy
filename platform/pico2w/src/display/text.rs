use super::{font, SCREEN_W};

pub(crate) const C0_BE: [u8; 2] = [0xE7, 0xDA]; // #E0F8D0
pub(crate) const C1_BE: [u8; 2] = [0x8E, 0x0E]; // #88C070
pub(crate) const C2_BE: [u8; 2] = [0x33, 0x4A]; // #346856
pub(crate) const C3_BE: [u8; 2] = [0x08, 0xC4]; // #081820

pub(crate) const CHAR_W: usize = 8;

pub(crate) fn fill_row(row: &mut [u8; 480], color: [u8; 2]) {
    let mut i = 0;
    while i < 480 {
        row[i] = color[0];
        row[i + 1] = color[1];
        i += 2;
    }
}

pub(crate) fn write_text_row(
    row: &mut [u8; 480],
    text: &[u8],
    x_start: usize,
    glyph_row: usize,
    scale: usize,
    fg: [u8; 2],
    bg: [u8; 2],
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
                    row[px * 2] = color[0];
                    row[px * 2 + 1] = color[1];
                }
            }
        }
    }
}

pub(crate) fn write_truncated_text_row(
    row: &mut [u8; 480],
    text: &[u8],
    x_start: usize,
    x_end: usize,
    glyph_row: usize,
    scale: usize,
    fg: [u8; 2],
    bg: [u8; 2],
) {
    let char_screen_w = CHAR_W * scale;
    if char_screen_w == 0 {
        return;
    }

    let max_chars = x_end.saturating_sub(x_start) / char_screen_w;
    let visible = text.len().min(max_chars);
    write_text_row(row, &text[..visible], x_start, glyph_row, scale, fg, bg);
}

pub(crate) fn text_pixel_lit(
    text: &[u8],
    glyph_row: usize,
    scale: usize,
    source_px: usize,
) -> bool {
    let char_screen_w = CHAR_W * scale;
    let char_idx = source_px / char_screen_w;
    let Some(&ch) = text.get(char_idx) else {
        return false;
    };
    let glyph_x = (source_px % char_screen_w) / scale;
    let bitmap = font::glyph_row(ch, glyph_row);
    (bitmap >> glyph_x) & 1 != 0
}

pub(crate) fn text_screen_width(text: &[u8], scale: usize) -> usize {
    text.len() * CHAR_W * scale
}
