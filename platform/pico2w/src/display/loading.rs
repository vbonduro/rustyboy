use super::text::{
    fill_row, text_screen_width, write_text_row, write_truncated_text_row, C0_BE, C1_BE, C2_BE,
    C3_BE, CHAR_W,
};
use super::SCREEN_W;

const HEADER_H: u16 = 40;
const SEPARATOR_H: u16 = 4;
const SCALE: usize = 2;
const LOADING_FILENAME_Y: u16 = 140;
const LOADING_BAR_TOP: u16 = 200;
const LOADING_BAR_BOTTOM: u16 = 220;
const LOADING_BAR_X0: usize = 20;
const LOADING_BAR_X1: usize = 220;
const LOADING_FILENAME_X0: usize = 8;
const LOADING_FILENAME_X1: usize = 232;

pub fn loading_bar_y_range() -> (u16, u16) {
    (LOADING_BAR_TOP, LOADING_BAR_BOTTOM)
}

pub fn render_loading_row(
    filename: &str,
    banks_done: u32,
    total_banks: u32,
    _marquee_frame: u32,
    y: u16,
    row: &mut [u8; 480],
) {
    let in_top_sep = y >= HEADER_H && y < HEADER_H + SEPARATOR_H;
    fill_row(row, if in_top_sep { C2_BE } else { C3_BE });

    if y < HEADER_H {
        render_loading_header_row(y, row);
    }
    if y >= LOADING_FILENAME_Y && y < LOADING_FILENAME_Y + 8 {
        render_loading_filename_row(filename, y, row);
    }
    if y >= LOADING_BAR_TOP && y < LOADING_BAR_BOTTOM {
        render_loading_bar_row(banks_done, total_banks, row);
    }
}

fn render_loading_header_row(y: u16, row: &mut [u8; 480]) {
    let title_screen_h = (8 * SCALE) as u16;
    let title_top = (HEADER_H - title_screen_h) / 2;
    if y < title_top || y >= title_top + title_screen_h {
        return;
    }
    let glyph_row = ((y - title_top) as usize) / SCALE;
    let title = b"LOADING";
    let title_w = title.len() * CHAR_W * SCALE;
    let title_x = (SCREEN_W as usize).saturating_sub(title_w) / 2;
    write_text_row(row, title, title_x, glyph_row, SCALE, C0_BE, C3_BE);
}

fn render_loading_filename_row(filename: &str, y: u16, row: &mut [u8; 480]) {
    let glyph_row = (y - LOADING_FILENAME_Y) as usize;
    let text = filename.as_bytes();
    let text_w = text_screen_width(text, 1);
    let window_w = LOADING_FILENAME_X1.saturating_sub(LOADING_FILENAME_X0);
    if text_w > window_w {
        write_truncated_text_row(
            row,
            text,
            LOADING_FILENAME_X0,
            LOADING_FILENAME_X1,
            glyph_row,
            1,
            C1_BE,
            C3_BE,
        );
    } else {
        let text_x = (SCREEN_W as usize).saturating_sub(text_w) / 2;
        write_text_row(row, text, text_x, glyph_row, 1, C1_BE, C3_BE);
    }
}

fn render_loading_bar_row(banks_done: u32, total_banks: u32, row: &mut [u8; 480]) {
    let bar_w = LOADING_BAR_X1 - LOADING_BAR_X0;
    let filled = if total_banks > 0 {
        (bar_w as u64 * banks_done as u64 / total_banks as u64) as usize
    } else {
        0
    };
    for px in LOADING_BAR_X0..LOADING_BAR_X1 {
        row[px * 2] = C2_BE[0];
        row[px * 2 + 1] = C2_BE[1];
    }
    for px in LOADING_BAR_X0..LOADING_BAR_X0 + filled.min(bar_w) {
        row[px * 2] = C0_BE[0];
        row[px * 2 + 1] = C0_BE[1];
    }
}
