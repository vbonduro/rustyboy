use crate::menu::MenuFrame;

use super::text::{
    fill_row, text_pixel_lit, text_screen_width, write_text_row, write_truncated_text_row, C0_BE,
    C1_BE, C2_BE, C3_BE, CHAR_W,
};
use super::{SCREEN_H, SCREEN_W};

// Layout constants (pixels on the 240x320 display).
const HEADER_H: u16 = 40;
const SEPARATOR_H: u16 = 4;
const ITEMS_START_Y: u16 = HEADER_H + SEPARATOR_H;
const ITEM_H: u16 = 32;
const ITEM_TEXT_PAD: u16 = 8;
const FOOTER_SEP_Y: u16 = 280;
const FOOTER_Y: u16 = FOOTER_SEP_Y + SEPARATOR_H;
const CURSOR_X: usize = 10;
const ITEM_TEXT_X: usize = 28;
const ITEM_TEXT_RIGHT_X: usize = 236;
const ITEM_MARKER_GAP: usize = 8;
const MARKER_X: usize = 196;
const SCALE: usize = 2;
const MARQUEE_HOLD_FRAMES: u32 = 30;
const MARQUEE_GAP_CHARS: usize = 3;
const MARQUEE_FRAMES_PER_STEP: u32 = 1;

pub const MENU_MARQUEE_REDRAW_FRAMES: u32 = MARQUEE_FRAMES_PER_STEP;

pub fn menu_item_needs_marquee(text: &str, marked: bool) -> bool {
    text_screen_width(text.as_bytes(), SCALE) > menu_item_text_window_width(marked)
}

pub fn menu_item_y_range(slot: usize) -> Option<(u16, u16)> {
    let y_start = ITEMS_START_Y.checked_add(slot.checked_mul(ITEM_H as usize)? as u16)?;
    if y_start >= FOOTER_SEP_Y {
        return None;
    }
    Some((y_start, (y_start + ITEM_H).min(FOOTER_SEP_Y)))
}

pub fn menu_item_text_window(slot: usize, marked: bool) -> Option<(u16, u16, u16, u16)> {
    let (item_y_start, item_y_end) = menu_item_y_range(slot)?;
    let y_start = item_y_start + ITEM_TEXT_PAD;
    let y_end = (y_start + (8 * SCALE) as u16).min(item_y_end);
    let x_start = ITEM_TEXT_X as u16;
    let x_end = menu_item_text_window_end(marked) as u16;
    if x_end <= x_start || y_end <= y_start {
        return None;
    }
    Some((x_start, x_end, y_start, y_end))
}

pub fn render_menu_row(frame: &MenuFrame<'_>, y: u16, row: &mut [u8; 480]) {
    let in_top_sep = y >= HEADER_H && y < HEADER_H + SEPARATOR_H;
    let in_footer_sep = y >= FOOTER_SEP_Y && y < FOOTER_Y;
    let in_items = y >= ITEMS_START_Y && y < FOOTER_SEP_Y;

    fill_row(
        row,
        if in_top_sep || in_footer_sep {
            C2_BE
        } else {
            C3_BE
        },
    );

    if y < HEADER_H {
        render_menu_header_row(frame.title, y, row);
    }
    if in_items {
        render_menu_items_row(frame, y, row);
    }
    if y >= FOOTER_Y {
        render_menu_footer_row(y, row);
    }
}

fn render_menu_header_row(title: &str, y: u16, row: &mut [u8; 480]) {
    let title_screen_h = (8 * SCALE) as u16;
    let title_top = (HEADER_H - title_screen_h) / 2;
    if y < title_top || y >= title_top + title_screen_h {
        return;
    }
    let glyph_row = ((y - title_top) as usize) / SCALE;
    let title_screen_w = title.len() * CHAR_W * SCALE;
    let title_x = (SCREEN_W as usize).saturating_sub(title_screen_w) / 2;
    write_text_row(
        row,
        title.as_bytes(),
        title_x,
        glyph_row,
        SCALE,
        C0_BE,
        C3_BE,
    );
}

fn render_menu_items_row(frame: &MenuFrame<'_>, y: u16, row: &mut [u8; 480]) {
    let slot = ((y - ITEMS_START_Y) / ITEM_H) as usize;
    if slot >= frame.items.len() {
        return;
    }
    let slot_top = ITEMS_START_Y + slot as u16 * ITEM_H;
    let selected = slot == frame.selected;
    let enabled = frame.enabled.get(slot).copied().unwrap_or(true);

    fill_row(row, if selected { C2_BE } else { C3_BE });

    let text_top = slot_top + ITEM_TEXT_PAD;
    let text_bottom = text_top + (8 * SCALE) as u16;
    if y < text_top || y >= text_bottom {
        return;
    }

    let glyph_row = ((y - text_top) as usize) / SCALE;
    let text_color = if !enabled {
        C2_BE
    } else if selected {
        C0_BE
    } else {
        C1_BE
    };
    let item_bg = if selected { C2_BE } else { C3_BE };

    let cursor: &[u8] = if selected { b">" } else { b" " };
    write_text_row(row, cursor, CURSOR_X, glyph_row, SCALE, text_color, item_bg);
    let text = frame.items[slot].as_bytes();
    let text_right = menu_item_text_window_end(frame.marked == Some(slot));
    if selected && text_screen_width(text, SCALE) > text_right.saturating_sub(ITEM_TEXT_X) {
        write_marquee_text_row(
            row,
            text,
            ITEM_TEXT_X,
            text_right,
            glyph_row,
            SCALE,
            frame.marquee_frame,
            text_color,
            item_bg,
        );
    } else {
        write_truncated_text_row(
            row,
            text,
            ITEM_TEXT_X,
            text_right,
            glyph_row,
            SCALE,
            text_color,
            item_bg,
        );
    }
    if frame.marked == Some(slot) {
        write_text_row(row, b"*", MARKER_X, glyph_row, SCALE, C0_BE, item_bg);
    }
}

fn render_menu_footer_row(y: u16, row: &mut [u8; 480]) {
    const FOOTER: &[u8] = b"A:SELECT  B:BACK";
    let footer_text_h = 8u16;
    let footer_top = FOOTER_Y + (SCREEN_H as u16 - FOOTER_Y - footer_text_h) / 2;
    if y < footer_top || y >= footer_top + footer_text_h {
        return;
    }
    let glyph_row = (y - footer_top) as usize;
    let footer_w = FOOTER.len() * CHAR_W;
    let footer_x = (SCREEN_W as usize).saturating_sub(footer_w) / 2;
    write_text_row(row, FOOTER, footer_x, glyph_row, 1, C1_BE, C3_BE);
}

fn write_marquee_text_row(
    row: &mut [u8; 480],
    text: &[u8],
    x_start: usize,
    x_end: usize,
    glyph_row: usize,
    scale: usize,
    marquee_frame: u32,
    fg: [u8; 2],
    bg: [u8; 2],
) {
    let scroll_px = marquee_scroll_px(text, scale, marquee_frame);
    let x0 = x_start.min(SCREEN_W as usize);
    let x1 = x_end.min(SCREEN_W as usize);
    if x1 <= x0 || text.is_empty() || scale == 0 {
        return;
    }

    let text_w = text_screen_width(text, scale);
    if text_w == 0 {
        return;
    }
    let period_w = text_w + MARQUEE_GAP_CHARS * CHAR_W * scale;

    for px in x0..x1 {
        let source_px = (px - x0 + scroll_px) % period_w;
        let color = if source_px < text_w && text_pixel_lit(text, glyph_row, scale, source_px) {
            fg
        } else {
            bg
        };
        row[px * 2] = color[0];
        row[px * 2 + 1] = color[1];
    }
}

fn marquee_scroll_px(text: &[u8], scale: usize, marquee_frame: u32) -> usize {
    let text_w = text_screen_width(text, scale);
    let period_w = text_w + MARQUEE_GAP_CHARS * CHAR_W * scale;
    if period_w == 0 {
        return 0;
    }

    let step_px = scale.max(1);
    let scroll_steps = period_w.div_ceil(step_px);
    let scroll_frames = (scroll_steps as u32).saturating_mul(MARQUEE_FRAMES_PER_STEP);
    let phase = marquee_frame % (MARQUEE_HOLD_FRAMES + scroll_frames);
    if phase < MARQUEE_HOLD_FRAMES {
        0
    } else {
        let step = ((phase - MARQUEE_HOLD_FRAMES) / MARQUEE_FRAMES_PER_STEP) as usize;
        (step * step_px) % period_w
    }
}

fn menu_item_text_window_end(marked: bool) -> usize {
    if marked {
        MARKER_X.saturating_sub(ITEM_MARKER_GAP)
    } else {
        ITEM_TEXT_RIGHT_X
    }
}

fn menu_item_text_window_width(marked: bool) -> usize {
    menu_item_text_window_end(marked).saturating_sub(ITEM_TEXT_X)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_item_marquee_threshold_reserves_marker_space() {
        assert!(!menu_item_needs_marquee("1234567890123", false));
        assert!(menu_item_needs_marquee("12345678901234", false));
        assert!(!menu_item_needs_marquee("1234567890", true));
        assert!(menu_item_needs_marquee("12345678901", true));
    }

    #[test]
    fn marquee_scroll_holds_before_moving() {
        assert_eq!(marquee_scroll_px(b"LONG-ROM-NAME.GB", SCALE, 0), 0);
        assert_eq!(
            marquee_scroll_px(b"LONG-ROM-NAME.GB", SCALE, MARQUEE_HOLD_FRAMES - 1),
            0
        );
        assert_eq!(
            marquee_scroll_px(
                b"LONG-ROM-NAME.GB",
                SCALE,
                MARQUEE_HOLD_FRAMES + MARQUEE_FRAMES_PER_STEP
            ),
            SCALE
        );
    }

    #[test]
    fn marquee_scroll_uses_slowed_full_period_before_wrapping() {
        let text = b"LONG-ROM-NAME.GB";
        let period_w = text_screen_width(text, SCALE) + MARQUEE_GAP_CHARS * CHAR_W * SCALE;
        assert_eq!(
            marquee_scroll_px(
                text,
                SCALE,
                MARQUEE_HOLD_FRAMES
                    + (period_w as u32 / SCALE as u32 - 1) * MARQUEE_FRAMES_PER_STEP
            ),
            period_w - SCALE
        );
        assert_eq!(
            marquee_scroll_px(
                text,
                SCALE,
                MARQUEE_HOLD_FRAMES + period_w as u32 / SCALE as u32 * MARQUEE_FRAMES_PER_STEP
            ),
            0
        );
    }
}
