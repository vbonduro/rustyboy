use super::{font, SCREEN_W};

pub(crate) const C0_BE: [u8; 2] = [0xE7, 0xDA]; // #E0F8D0
pub(crate) const C1_BE: [u8; 2] = [0x8E, 0x0E]; // #88C070
pub(crate) const C2_BE: [u8; 2] = [0x33, 0x4A]; // #346856
pub(crate) const C3_BE: [u8; 2] = [0x08, 0xC4]; // #081820

pub(crate) const CHAR_W: usize = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextStyle {
    pub scale: usize,
    pub fg: [u8; 2],
    pub bg: [u8; 2],
}

impl TextStyle {
    pub const fn new(scale: usize, fg: [u8; 2], bg: [u8; 2]) -> Self {
        Self { scale, fg, bg }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextRun {
    pub x_start: usize,
    pub glyph_row: usize,
    pub style: TextStyle,
}

impl TextRun {
    pub const fn new(x_start: usize, glyph_row: usize, style: TextStyle) -> Self {
        Self {
            x_start,
            glyph_row,
            style,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TextWindow {
    pub x_start: usize,
    pub x_end: usize,
    pub glyph_row: usize,
    pub style: TextStyle,
}

impl TextWindow {
    pub const fn new(x_start: usize, x_end: usize, glyph_row: usize, style: TextStyle) -> Self {
        Self {
            x_start,
            x_end,
            glyph_row,
            style,
        }
    }

    pub fn width(self) -> usize {
        self.x_end.saturating_sub(self.x_start)
    }
}

pub(crate) fn fill_row(row: &mut [u8; 480], color: [u8; 2]) {
    let mut i = 0;
    while i < 480 {
        row[i] = color[0];
        row[i + 1] = color[1];
        i += 2;
    }
}

pub(crate) fn write_text_row(row: &mut [u8; 480], text: &[u8], run: TextRun) {
    let char_screen_w = CHAR_W * run.style.scale;
    for (char_idx, &ch) in text.iter().enumerate() {
        let bitmap = font::glyph_row(ch, run.glyph_row);
        let char_x = run.x_start + char_idx * char_screen_w;
        for glyph_x in 0..CHAR_W {
            // Font bitmaps use bit 0 = leftmost pixel (LSB-first convention).
            let lit = (bitmap >> glyph_x) & 1 != 0;
            let color = if lit { run.style.fg } else { run.style.bg };
            for sx in 0..run.style.scale {
                let px = char_x + glyph_x * run.style.scale + sx;
                if px < SCREEN_W as usize {
                    row[px * 2] = color[0];
                    row[px * 2 + 1] = color[1];
                }
            }
        }
    }
}

pub(crate) fn write_truncated_text_row(row: &mut [u8; 480], text: &[u8], window: TextWindow) {
    let char_screen_w = CHAR_W * window.style.scale;
    if char_screen_w == 0 {
        return;
    }

    let max_chars = window.width() / char_screen_w;
    let visible = text.len().min(max_chars);
    write_text_row(
        row,
        &text[..visible],
        TextRun::new(window.x_start, window.glyph_row, window.style),
    );
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
