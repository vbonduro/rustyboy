use font8x8::{UnicodeFonts, BASIC_FONTS};
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;

use super::layout::{
    CELL_H, CELL_W, MAIN_OPTION_CHAR_ADV, RGBA_LEN, SCREEN_HEIGHT, SCREEN_WIDTH,
};
use super::palette::{color_to_rgba, C0, C2, MENU_TEXT_NORMAL, MENU_TEXT_OUTLINE};

// ---------------------------------------------------------------------------
// Primitive pixel operations
// ---------------------------------------------------------------------------

pub(super) fn put_pixel(rgba: &mut [u8], x: usize, y: usize, color: [u8; 4]) {
    if x >= SCREEN_WIDTH || y >= SCREEN_HEIGHT {
        return;
    }
    let idx = (y * SCREEN_WIDTH + x) * 4;
    rgba[idx..idx + 4].copy_from_slice(&color);
}

pub(super) fn fill_rect(rgba: &mut [u8], x: usize, y: usize, w: usize, h: usize, color: [u8; 4]) {
    for py in y..(y + h).min(SCREEN_HEIGHT) {
        for px in x..(x + w).min(SCREEN_WIDTH) {
            put_pixel(rgba, px, py, color);
        }
    }
}

fn fill_cell(
    rgba: &mut [u8],
    cell_x: usize,
    cell_y: usize,
    cell_w: usize,
    cell_h: usize,
    color: [u8; 4],
) {
    fill_rect(rgba, cell_x * cell_w, cell_y * cell_h, cell_w, cell_h, color);
}

// ---------------------------------------------------------------------------
// Glyph rendering
// ---------------------------------------------------------------------------

/// Render a font8x8 glyph downscaled to fit `cell_w × cell_h` pixels.  Each
/// of the 8×8 source bits is mapped to a target pixel via the *rounded*
/// projection `(round(src_x*cell_w/8), round(src_y*cell_h/8))`; the target
/// pixel is lit if any contributing source bit is on (OR reduction).  Rounding
/// (rather than truncation) preserves stroke-width differences that floor()
/// collapses, so the rendering is legible at any cell size without hand-tuned
/// glyphs.
/// A thin (1px-stroke) 5x7 menu font — a lighter alternative to the heavy
/// 2px-stroke font8x8 face, so list/menu text reads as regular weight rather
/// than bold. Bit `x` (1<<x) is column x (0 = leftmost). Text is uppercased
/// upstream by `normalize_text`; unknown glyphs fall back to `?`.
fn thin_glyph(ch: char) -> Option<[u8; 7]> {
    match ch {
        ' ' => Some([0, 0, 0, 0, 0, 0, 0]),
        '!' => Some([4, 4, 4, 4, 4, 0, 4]),
        '&' => Some([6, 9, 5, 2, 21, 9, 22]),
        '\'' => Some([4, 4, 4, 0, 0, 0, 0]),
        '(' => Some([4, 2, 2, 2, 2, 2, 4]),
        ')' => Some([4, 8, 8, 8, 8, 8, 4]),
        '+' => Some([0, 4, 4, 31, 4, 4, 0]),
        ',' => Some([0, 0, 0, 0, 4, 4, 2]),
        '-' => Some([0, 0, 0, 31, 0, 0, 0]),
        '.' => Some([0, 0, 0, 0, 0, 4, 4]),
        '/' => Some([16, 16, 8, 4, 2, 1, 1]),
        '0' => Some([14, 17, 25, 21, 19, 17, 14]),
        '1' => Some([4, 6, 4, 4, 4, 4, 14]),
        '2' => Some([14, 17, 16, 12, 2, 1, 31]),
        '3' => Some([15, 16, 16, 14, 16, 16, 15]),
        '4' => Some([8, 12, 10, 9, 31, 8, 8]),
        '5' => Some([31, 1, 15, 16, 16, 17, 14]),
        '6' => Some([14, 1, 1, 15, 17, 17, 14]),
        '7' => Some([31, 16, 8, 4, 2, 2, 2]),
        '8' => Some([14, 17, 17, 14, 17, 17, 14]),
        '9' => Some([14, 17, 17, 30, 16, 16, 14]),
        ':' => Some([0, 4, 0, 0, 0, 4, 0]),
        '<' => Some([16, 8, 4, 8, 16, 0, 0]),
        '>' => Some([1, 2, 4, 2, 1, 0, 0]),
        '?' => Some([14, 17, 16, 12, 4, 0, 4]),
        'A' => Some([4, 10, 17, 17, 31, 17, 17]),
        'B' => Some([15, 17, 17, 15, 17, 17, 15]),
        'C' => Some([14, 17, 1, 1, 1, 17, 14]),
        'D' => Some([15, 17, 17, 17, 17, 17, 15]),
        'E' => Some([31, 1, 1, 15, 1, 1, 31]),
        'F' => Some([31, 1, 1, 15, 1, 1, 1]),
        'G' => Some([14, 17, 1, 29, 17, 17, 30]),
        'H' => Some([17, 17, 17, 31, 17, 17, 17]),
        'I' => Some([31, 4, 4, 4, 4, 4, 31]),
        'J' => Some([15, 8, 8, 8, 9, 9, 6]),
        'K' => Some([17, 9, 5, 3, 5, 9, 17]),
        'L' => Some([1, 1, 1, 1, 1, 1, 31]),
        'M' => Some([17, 27, 21, 21, 17, 17, 17]),
        'N' => Some([17, 19, 21, 21, 25, 17, 17]),
        'O' => Some([14, 17, 17, 17, 17, 17, 14]),
        'P' => Some([15, 17, 17, 15, 1, 1, 1]),
        'Q' => Some([14, 17, 17, 17, 21, 9, 22]),
        'R' => Some([15, 17, 17, 15, 5, 9, 17]),
        'S' => Some([30, 1, 1, 14, 16, 16, 15]),
        'T' => Some([31, 4, 4, 4, 4, 4, 4]),
        'U' => Some([17, 17, 17, 17, 17, 17, 14]),
        'V' => Some([17, 17, 17, 17, 17, 10, 4]),
        'W' => Some([17, 17, 17, 21, 21, 27, 17]),
        'X' => Some([17, 17, 10, 4, 10, 17, 17]),
        'Y' => Some([17, 17, 10, 4, 4, 4, 4]),
        'Z' => Some([31, 16, 8, 4, 2, 1, 31]),
        '^' => Some([4, 10, 17, 0, 0, 0, 0]),
        _ => None,
    }
}

/// Draw a thin 5x7 glyph centered within a `cell_w x cell_h` menu cell.
pub(super) fn draw_thin_glyph_to_cell(
    rgba: &mut [u8],
    x0: usize,
    y0: usize,
    cell_w: usize,
    cell_h: usize,
    ch: char,
    color: [u8; 4],
) {
    let Some(glyph) = thin_glyph(ch).or_else(|| thin_glyph('?')) else {
        return;
    };
    let x_off = cell_w.saturating_sub(5) / 2;
    let y_off = cell_h.saturating_sub(7) / 2;
    for (row, bits) in glyph.iter().copied().enumerate() {
        for col in 0..5usize {
            if bits & (1 << col) != 0 {
                put_pixel(rgba, x0 + x_off + col, y0 + y_off + row, color);
            }
        }
    }
}

/// Render a font8x8 glyph scaled up by `scale` (1 = native 8×8).
pub(super) fn draw_basic_glyph_scaled(
    rgba: &mut [u8],
    x0: usize,
    y0: usize,
    ch: char,
    color: [u8; 4],
    scale: usize,
) {
    let Some(glyph) = BASIC_FONTS.get(ch).or_else(|| BASIC_FONTS.get('?')) else {
        return;
    };
    for (row, bits) in glyph.iter().copied().enumerate() {
        for col in 0..8 {
            if bits & (1 << col) != 0 {
                fill_rect(rgba, x0 + col * scale, y0 + row * scale, scale, scale, color);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Composite text rendering
// ---------------------------------------------------------------------------

/// Draw `text` with a 1-pixel outline (all 8 neighbours) then the fill on top,
/// mirroring the title's outlined lettering for legibility over the scene.
pub(super) fn draw_outlined_text(
    rgba: &mut [u8],
    x: usize,
    y: usize,
    text: &str,
    fill: [u8; 4],
    outline: [u8; 4],
    scale: usize,
) {
    const OFFS: [(isize, isize); 8] = [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ];
    for (ci, ch) in text.chars().enumerate() {
        if ch == ' ' {
            continue;
        }
        let cx = x + ci * MAIN_OPTION_CHAR_ADV;
        for (dx, dy) in OFFS {
            let ox = cx as isize + dx;
            let oy = y as isize + dy;
            if ox < 0 || oy < 0 {
                continue;
            }
            draw_basic_glyph_scaled(rgba, ox as usize, oy as usize, ch, outline, scale);
        }
    }
    for (ci, ch) in text.chars().enumerate() {
        if ch == ' ' {
            continue;
        }
        let cx = x + ci * MAIN_OPTION_CHAR_ADV;
        draw_basic_glyph_scaled(rgba, cx, y, ch, fill, scale);
    }
}

/// A small outlined triangle (5×3) centered horizontally at `cx`, pointing up
/// or down — the main-menu scroll indicator.
pub(super) fn draw_scroll_arrow(rgba: &mut [u8], cx: usize, top_y: usize, up: bool) {
    let widths: [usize; 3] = if up { [1, 3, 5] } else { [5, 3, 1] };

    let plot = |rgba: &mut [u8], off: isize, color: [u8; 4]| {
        for (r, w) in widths.iter().enumerate() {
            let half = (*w as isize) / 2;
            let py = top_y as isize + r as isize;
            for o in -half..=half {
                let px = cx as isize + o + off;
                if px >= 0 && py >= 0 {
                    put_pixel(rgba, px as usize, py as usize, color);
                }
            }
        }
    };

    // Draw the dark halo first (1 px in each horizontal direction), then the
    // vertical halo rows, then the cream triangle on top.
    for off in [-1isize, 1] {
        plot(rgba, off, MENU_TEXT_OUTLINE);
    }
    for dy in [-1isize, 1] {
        for (r, w) in widths.iter().enumerate() {
            let half = (*w as isize) / 2;
            let py = top_y as isize + r as isize + dy;
            for o in -half..=half {
                let px = cx as isize + o;
                if px >= 0 && py >= 0 {
                    put_pixel(rgba, px as usize, py as usize, MENU_TEXT_OUTLINE);
                }
            }
        }
    }
    plot(rgba, 0, MENU_TEXT_NORMAL);
}

// ---------------------------------------------------------------------------
// Rasterize a ratatui terminal buffer to RGBA
// ---------------------------------------------------------------------------

/// Rasterize `buffer` using the standard menu cell dimensions.
pub(super) fn rasterize_buffer(buffer: &Buffer) -> Vec<u8> {
    rasterize_buffer_with_cell(buffer, CELL_W, CELL_H)
}

fn rasterize_buffer_with_cell(buffer: &Buffer, cell_w: usize, cell_h: usize) -> Vec<u8> {
    let mut rgba = vec![0u8; RGBA_LEN];
    for cell_y in 0..buffer.area.height as usize {
        for cell_x in 0..buffer.area.width as usize {
            let cell = &buffer[(buffer.area.x + cell_x as u16, buffer.area.y + cell_y as u16)];
            let mut fg = color_to_rgba(cell.fg, C2);
            let mut bg = color_to_rgba(cell.bg, C0);
            if cell.modifier.contains(Modifier::REVERSED) {
                core::mem::swap(&mut fg, &mut bg);
            }
            fill_cell(&mut rgba, cell_x, cell_y, cell_w, cell_h, bg);
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            if ch != ' ' {
                draw_thin_glyph_to_cell(&mut rgba, cell_x * cell_w, cell_y * cell_h, cell_w, cell_h, ch, fg);
            }
        }
    }
    rgba
}
