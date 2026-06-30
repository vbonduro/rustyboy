// ---------------------------------------------------------------------------
// Screen / terminal geometry
// ---------------------------------------------------------------------------

pub(super) const SCREEN_WIDTH: usize = 160;
pub(super) const SCREEN_HEIGHT: usize = 144;
pub(super) const RGBA_LEN: usize = SCREEN_WIDTH * SCREEN_HEIGHT * 4;

/// Pixels per terminal cell (width).
pub(super) const CELL_W: usize = 6;
/// Cell pitch (taller than the glyph to add inter-row spacing).
pub(super) const CELL_H: usize = 11;
/// Actual glyph height inside the 11 px cell (~3 px gap between rows).
pub(super) const MENU_GLYPH_H: usize = 8;

pub(super) const TERM_W: u16 = (SCREEN_WIDTH / CELL_W) as u16; // 26
pub(super) const TERM_H: u16 = (SCREEN_HEIGHT / CELL_H) as u16; // 13

pub(super) const HEADER_ROWS: u16 = 1;
pub(super) const FOOTER_ROWS: u16 = 1;
pub(super) const LIST_ROWS: u16 = TERM_H - HEADER_ROWS - FOOTER_ROWS; // 11
pub(super) const LIST_TOP_PX: f64 = (HEADER_ROWS as usize * CELL_H) as f64; // 11.0

// ---------------------------------------------------------------------------
// Main-menu landscape overlay geometry
// ---------------------------------------------------------------------------

/// Source font height in pixels (font8x8 face).
const BASIC_FONT_H: usize = 8;
pub(super) const MAIN_OPTION_SCALE: usize = 1;
/// Horizontal advance per character (7 px — font8x8 is ~6 px wide).
pub(super) const MAIN_OPTION_CHAR_ADV: usize = 7 * MAIN_OPTION_SCALE;
pub(super) const MAIN_OPTION_GLYPH_W: usize = BASIC_FONT_H * MAIN_OPTION_SCALE;
pub(super) const MAIN_OPTION_GLYPH_H: usize = BASIC_FONT_H * MAIN_OPTION_SCALE;
pub(super) const MAIN_OPTION_STEP_PX: usize = (BASIC_FONT_H + 2) * MAIN_OPTION_SCALE;
/// Vertical center of the option block (above the water horizon ~y 98).
const MAIN_OPTION_CENTER_Y: usize = 70;
/// Never collide with the title banner.
const MAIN_OPTION_MIN_TOP_Y: usize = 40;
/// Max visible rows in the sky band; longer lists scroll.
pub(super) const MAIN_OPTION_MAX_VISIBLE: usize = 5;
/// Content columns before a name scrolls / truncates.
pub(super) const MAIN_OPTION_MAX_CHARS: usize = 18;

// ---------------------------------------------------------------------------
// Pure layout functions
// ---------------------------------------------------------------------------

/// Top-left y (screen pixels) of the game-list block, vertically centered on
/// `MAIN_OPTION_CENTER_Y` and clamped below the title banner.  Shared by the
/// renderer and the touch hit-test so they always agree on row positions.
pub(super) fn main_menu_block_top_y(label_count: usize) -> usize {
    let block_h = label_count.saturating_sub(1) * MAIN_OPTION_STEP_PX;
    MAIN_OPTION_CENTER_Y
        .saturating_sub(block_h / 2)
        .saturating_sub(MAIN_OPTION_GLYPH_H / 2)
        .max(MAIN_OPTION_MIN_TOP_Y)
}

/// Top-left y of row `idx` in the main-menu overlay.
pub(super) fn main_menu_option_y(idx: usize, label_count: usize) -> usize {
    main_menu_block_top_y(label_count) + idx * MAIN_OPTION_STEP_PX
}

/// `(scroll_offset, visible_row_count)` for a main-menu list of `len` items.
/// Shared by the renderer and touch hit-test.
pub(super) fn main_menu_window(len: usize, scroll_y: usize) -> (usize, usize) {
    let visible = MAIN_OPTION_MAX_VISIBLE.min(len);
    let scroll = scroll_y.min(len.saturating_sub(visible));
    (scroll, visible)
}

/// Rows to push the ratatui list down so a short menu sits vertically centered
/// between the header and footer rows; zero once the list fills or scrolls.
pub(super) fn list_voffset(len: usize) -> usize {
    (LIST_ROWS as usize).saturating_sub(len) / 2
}

/// Normalize menu text: replace special Unicode glyphs with ASCII equivalents,
/// then uppercase everything.  Non-graphic non-space characters become spaces.
pub(super) fn normalize_text(input: &str) -> String {
    input
        .chars()
        .map(|ch| match ch {
            '▲' => '^',
            '▼' => 'V',
            '·' | '•' => '-',
            ch if ch.is_ascii_graphic() || ch == ' ' => ch.to_ascii_uppercase(),
            _ => ' ',
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_voffset_centers_short_list() {
        // LIST_ROWS = 11; a 2-item list → voff = (11-2)/2 = 4.
        assert_eq!(list_voffset(2), 4);
        // A 11-item list fills the area → voff = 0.
        assert_eq!(list_voffset(11), 0);
        // Longer list → also 0 (saturating_sub).
        assert_eq!(list_voffset(20), 0);
    }

    #[test]
    fn main_menu_window_visible_capped() {
        // Never shows more than MAIN_OPTION_MAX_VISIBLE rows.
        let (_, visible) = main_menu_window(100, 0);
        assert_eq!(visible, MAIN_OPTION_MAX_VISIBLE);
    }

    #[test]
    fn main_menu_window_scroll_clamped() {
        // scroll_y beyond the last window start is clamped.
        let (scroll, visible) = main_menu_window(3, 99);
        assert_eq!(scroll + visible, 3);
    }

    #[test]
    fn main_menu_window_few_items() {
        // Fewer items than max-visible: scroll=0, visible=len.
        let (scroll, visible) = main_menu_window(2, 0);
        assert_eq!(scroll, 0);
        assert_eq!(visible, 2);
    }

    #[test]
    fn main_menu_option_y_increases_by_step() {
        // Each successive row is MAIN_OPTION_STEP_PX below the previous.
        let y0 = main_menu_option_y(0, 5);
        let y1 = main_menu_option_y(1, 5);
        assert_eq!(y1 - y0, MAIN_OPTION_STEP_PX);
    }

    #[test]
    fn normalize_text_uppercases_and_maps_glyphs() {
        assert_eq!(normalize_text("hello"), "HELLO");
        assert_eq!(normalize_text("▲▼"), "^V");
        assert_eq!(normalize_text("a·b•c"), "A-B-C");
        // Non-graphic characters → space.
        assert_eq!(normalize_text("\x01"), " ");
    }
}
