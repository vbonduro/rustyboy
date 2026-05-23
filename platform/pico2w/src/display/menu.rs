use crate::menu::MenuFrame;

use super::text::{
    fill_row, text_pixel_lit, text_screen_width, write_text_row, write_truncated_text_row, TextRun,
    TextStyle, TextWindow, C0_BE, C1_BE, C2_BE, C3_BE, CHAR_W,
};
use super::{RenderWindow, SCREEN_H, SCREEN_W};

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

pub fn menu_item_window(slot: usize) -> Option<RenderWindow> {
    let y_start = ITEMS_START_Y.checked_add(slot.checked_mul(ITEM_H as usize)? as u16)?;
    if y_start >= FOOTER_SEP_Y {
        return None;
    }
    Some(RenderWindow::full_width_rows(
        y_start,
        (y_start + ITEM_H).min(FOOTER_SEP_Y),
    ))
}

pub fn menu_item_text_window(slot: usize, marked: bool) -> Option<RenderWindow> {
    let item_window = menu_item_window(slot)?;
    let y_start = item_window.y_start + ITEM_TEXT_PAD;
    let y_end = (y_start + (8 * SCALE) as u16).min(item_window.y_end);
    let x_start = ITEM_TEXT_X as u16;
    let x_end = menu_item_text_window_end(marked) as u16;
    if x_end <= x_start || y_end <= y_start {
        return None;
    }
    Some(RenderWindow::new(x_start, x_end, y_start, y_end))
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
        TextRun::new(title_x, glyph_row, TextStyle::new(SCALE, C0_BE, C3_BE)),
    );
}

fn render_menu_items_row(frame: &MenuFrame<'_>, y: u16, row: &mut [u8; 480]) {
    let Some(item) = menu_item_for_row(frame, y) else {
        return;
    };

    fill_row(row, item.bg());
    let Some(glyph_row) = item.glyph_row else {
        return;
    };

    render_item_cursor(row, item, glyph_row);
    render_item_label(row, item, glyph_row);
    render_item_marker(row, item, glyph_row);
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
    write_text_row(
        row,
        FOOTER,
        TextRun::new(footer_x, glyph_row, TextStyle::new(1, C1_BE, C3_BE)),
    );
}

#[derive(Clone, Copy)]
struct MenuItemRow<'a> {
    selected: bool,
    enabled: bool,
    marked: bool,
    text: &'a [u8],
    glyph_row: Option<usize>,
    marquee_frame: u32,
}

impl MenuItemRow<'_> {
    fn bg(self) -> [u8; 2] {
        if self.selected {
            C2_BE
        } else {
            C3_BE
        }
    }

    fn text_style(self) -> TextStyle {
        let text_color = if !self.enabled {
            C2_BE
        } else if self.selected {
            C0_BE
        } else {
            C1_BE
        };
        TextStyle::new(SCALE, text_color, self.bg())
    }

    fn text_window(self, glyph_row: usize) -> TextWindow {
        TextWindow::new(
            ITEM_TEXT_X,
            menu_item_text_window_end(self.marked),
            glyph_row,
            self.text_style(),
        )
    }
}

fn menu_item_for_row<'a>(frame: &MenuFrame<'a>, y: u16) -> Option<MenuItemRow<'a>> {
    if !(ITEMS_START_Y..FOOTER_SEP_Y).contains(&y) {
        return None;
    }
    let slot = ((y - ITEMS_START_Y) / ITEM_H) as usize;
    let text = frame.items.get(slot)?.as_bytes();
    let slot_top = ITEMS_START_Y + slot as u16 * ITEM_H;
    let text_top = slot_top + ITEM_TEXT_PAD;
    let text_bottom = text_top + (8 * SCALE) as u16;
    let glyph_row = if y >= text_top && y < text_bottom {
        Some(((y - text_top) as usize) / SCALE)
    } else {
        None
    };

    Some(MenuItemRow {
        selected: slot == frame.selected,
        enabled: frame.enabled.get(slot).copied().unwrap_or(true),
        marked: frame.marked == Some(slot),
        text,
        glyph_row,
        marquee_frame: frame.marquee_frame,
    })
}

fn render_item_cursor(row: &mut [u8; 480], item: MenuItemRow<'_>, glyph_row: usize) {
    let cursor: &[u8] = if item.selected { b">" } else { b" " };
    write_text_row(
        row,
        cursor,
        TextRun::new(CURSOR_X, glyph_row, item.text_style()),
    );
}

fn render_item_label(row: &mut [u8; 480], item: MenuItemRow<'_>, glyph_row: usize) {
    let text_window = item.text_window(glyph_row);
    if item.selected && text_screen_width(item.text, SCALE) > text_window.width() {
        write_marquee_text_row(row, item.text, text_window, item.marquee_frame);
    } else {
        write_truncated_text_row(row, item.text, text_window);
    }
}

fn render_item_marker(row: &mut [u8; 480], item: MenuItemRow<'_>, glyph_row: usize) {
    if !item.marked {
        return;
    }
    write_text_row(
        row,
        b"*",
        TextRun::new(MARKER_X, glyph_row, TextStyle::new(SCALE, C0_BE, item.bg())),
    );
}

fn write_marquee_text_row(
    row: &mut [u8; 480],
    text: &[u8],
    window: TextWindow,
    marquee_frame: u32,
) {
    let scroll_px = marquee_scroll_px(text, window.style.scale, marquee_frame);
    let x0 = window.x_start.min(SCREEN_W as usize);
    let x1 = window.x_end.min(SCREEN_W as usize);
    if x1 <= x0 || text.is_empty() || window.style.scale == 0 {
        return;
    }

    let text_w = text_screen_width(text, window.style.scale);
    if text_w == 0 {
        return;
    }
    let period_w = text_w + MARQUEE_GAP_CHARS * CHAR_W * window.style.scale;

    for px in x0..x1 {
        let source_px = (px - x0 + scroll_px) % period_w;
        let color = if source_px < text_w
            && text_pixel_lit(text, window.glyph_row, window.style.scale, source_px)
        {
            window.style.fg
        } else {
            window.style.bg
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

    fn frame<'a>(
        items: &'a [&'a str],
        selected: usize,
        marquee_frame: u32,
        enabled: &'a [bool],
        marked: Option<usize>,
    ) -> MenuFrame<'a> {
        MenuFrame {
            title: "ROMS",
            items,
            selected,
            marquee_frame,
            enabled,
            marked,
        }
    }

    fn render_row(frame: &MenuFrame<'_>, y: u16) -> [u8; 480] {
        let mut row = [0; 480];
        render_menu_row(frame, y, &mut row);
        row
    }

    fn pixel(row: &[u8; 480], x: usize) -> [u8; 2] {
        [row[x * 2], row[x * 2 + 1]]
    }

    fn row_range_contains(row: &[u8; 480], x_start: usize, x_end: usize, color: [u8; 2]) -> bool {
        row[x_start * 2..x_end * 2]
            .chunks_exact(2)
            .any(|pixel| pixel == color)
    }

    #[test]
    fn menu_item_marquee_threshold_reserves_marker_space() {
        assert!(!menu_item_needs_marquee("1234567890123", false));
        assert!(menu_item_needs_marquee("12345678901234", false));
        assert!(!menu_item_needs_marquee("1234567890", true));
        assert!(menu_item_needs_marquee("12345678901", true));
    }

    #[test]
    fn menu_item_windows_use_exclusive_bounds() {
        assert_eq!(
            menu_item_window(0),
            Some(RenderWindow::full_width_rows(
                ITEMS_START_Y,
                ITEMS_START_Y + ITEM_H
            ))
        );
        assert_eq!(
            menu_item_text_window(0, false),
            Some(RenderWindow::new(
                ITEM_TEXT_X as u16,
                ITEM_TEXT_RIGHT_X as u16,
                ITEMS_START_Y + ITEM_TEXT_PAD,
                ITEMS_START_Y + ITEM_TEXT_PAD + (8 * SCALE) as u16,
            ))
        );
    }

    #[test]
    fn marked_menu_text_window_reserves_marker_column() {
        assert_eq!(
            menu_item_text_window(0, true).unwrap().x_end as usize,
            MARKER_X - ITEM_MARKER_GAP
        );
    }

    #[test]
    fn menu_item_window_returns_none_after_item_area() {
        let first_slot_past_footer = ((FOOTER_SEP_Y - ITEMS_START_Y) / ITEM_H + 1) as usize;
        assert_eq!(menu_item_window(first_slot_past_footer), None);
    }

    #[test]
    fn selected_item_row_uses_selected_background() {
        let items = ["FIRST", "SECOND"];
        let enabled = [true, true];
        let menu = frame(&items, 1, 0, &enabled, None);

        let selected = menu_item_window(1).unwrap();
        let unselected = menu_item_window(0).unwrap();
        assert_eq!(pixel(&render_row(&menu, selected.y_start + 1), 0), C2_BE);
        assert_eq!(pixel(&render_row(&menu, unselected.y_start + 1), 0), C3_BE);
    }

    #[test]
    fn disabled_unselected_item_renders_muted_text() {
        let items = ["FIRST", "SECOND"];
        let enabled = [true, false];
        let menu = frame(&items, 0, 0, &enabled, None);
        let text_window = menu_item_text_window(1, false).unwrap();

        let row = render_row(&menu, text_window.y_start);
        assert!(row_range_contains(
            &row,
            ITEM_TEXT_X,
            ITEM_TEXT_X + CHAR_W * SCALE,
            C2_BE
        ));
    }

    #[test]
    fn marked_item_renders_marker_pixels() {
        let items = ["FIRST"];
        let enabled = [true];
        let menu = frame(&items, 0, 0, &enabled, Some(0));
        let text_window = menu_item_text_window(0, true).unwrap();

        let row = render_row(&menu, text_window.y_start + SCALE as u16);
        assert!(row_range_contains(
            &row,
            MARKER_X,
            MARKER_X + CHAR_W * SCALE,
            C0_BE
        ));
    }

    #[test]
    fn short_selected_item_does_not_marquee() {
        let items = ["SHORT"];
        let enabled = [true];
        let first = frame(&items, 0, 0, &enabled, None);
        let later = frame(&items, 0, 100, &enabled, None);
        let text_window = menu_item_text_window(0, false).unwrap();

        assert_eq!(
            render_row(&first, text_window.y_start),
            render_row(&later, text_window.y_start)
        );
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
