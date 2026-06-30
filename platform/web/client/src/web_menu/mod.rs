mod glyph;
mod layout;
mod marquee;
mod palette;

use js_sys::Array;
use ratatui::{
    backend::TestBackend,
    layout::{Alignment, Rect},
    style::Style,
    widgets::{Block, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use wasm_bindgen::prelude::*;

use crate::landscape_bg::{LandscapeBg, FRAME_COUNT, FRAME_DURATION_MS, LANDSCAPE_BG_DATA};

use self::glyph::{draw_outlined_text, draw_scroll_arrow, rasterize_buffer};
use self::layout::{
    list_voffset, main_menu_block_top_y, main_menu_option_y, main_menu_window, normalize_text,
    CELL_W, FOOTER_ROWS, HEADER_ROWS, LIST_ROWS, LIST_TOP_PX, MAIN_OPTION_CHAR_ADV,
    MAIN_OPTION_GLYPH_H, MAIN_OPTION_GLYPH_W, MAIN_OPTION_MAX_CHARS, MAIN_OPTION_MAX_VISIBLE,
    MAIN_OPTION_SCALE, SCREEN_WIDTH, TERM_H, TERM_W,
};
use self::marquee::{marquee_window, Marquee};
use self::palette::{
    gb_c0, gb_c1, gb_c2, gb_c3, MENU_TEXT_NORMAL, MENU_TEXT_OUTLINE, MENU_TEXT_SELECTED,
};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
struct MenuState {
    title: String,
    labels: Vec<String>,
    footer: String,
    selected: usize,
    scroll_y: usize,
}

impl MenuState {
    fn is_main_menu(&self) -> bool {
        self.title == "RUSTYBOY"
    }
}

// ---------------------------------------------------------------------------
// Public wasm-bindgen API
// ---------------------------------------------------------------------------

#[wasm_bindgen]
pub struct WasmMenuRenderer {
    terminal: Terminal<TestBackend>,
    state: MenuState,
    /// Horizontal scroll state for the selected item; re-anchors on every
    /// navigation move so the 1-second pause restarts at the start of each name.
    marquee: Marquee,
}

#[wasm_bindgen]
impl WasmMenuRenderer {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Self {
        let backend = TestBackend::new(TERM_W, TERM_H);
        let terminal = Terminal::new(backend).expect("TestBackend is infallible");
        Self {
            terminal,
            state: MenuState::default(),
            marquee: Marquee::new(),
        }
    }

    pub fn show(&mut self, title: String, labels: Array, footer: String) {
        let labels = labels
            .iter()
            .filter_map(|value| value.as_string())
            .collect();
        self.show_labels(title, labels, footer);
    }

    pub fn selected_index(&self) -> usize {
        self.state.selected
    }

    pub fn scroll_y(&self) -> usize {
        self.state.scroll_y
    }

    pub fn set_selected(&mut self, selected: usize) {
        if self.state.labels.is_empty() {
            self.state.selected = 0;
            self.state.scroll_y = 0;
            return;
        }
        self.state.selected = selected.min(self.state.labels.len() - 1);
        self.clamp_scroll();
    }

    pub fn move_selection(&mut self, delta: i32) {
        let len = self.state.labels.len();
        if len == 0 {
            self.state.selected = 0;
            self.state.scroll_y = 0;
            return;
        }
        let len = len as i32;
        self.state.selected = (self.state.selected as i32 + delta).rem_euclid(len) as usize;
        self.clamp_scroll();
    }

    pub fn scroll_by(&mut self, delta: i32) {
        let max_scroll = self.max_scroll();
        let next = self.state.scroll_y as i32 + delta;
        self.state.scroll_y = next.clamp(0, max_scroll as i32) as usize;
    }

    pub fn item_at(&self, x: f64, y: f64) -> i32 {
        if !(0.0..SCREEN_WIDTH as f64).contains(&x) {
            return -1;
        }

        if self.state.is_main_menu() {
            let (scroll, visible) = main_menu_window(self.state.labels.len(), self.state.scroll_y);
            let y = y.max(0.0) as usize;
            for row in 0..visible {
                let y0 = main_menu_option_y(row, visible);
                if y >= y0.saturating_sub(2) && y < y0 + MAIN_OPTION_GLYPH_H + 2 {
                    return (scroll + row) as i32;
                }
            }
            return -1;
        }

        // Account for the vertical centering offset applied to short menus.
        let voff = list_voffset(self.state.labels.len());
        let top = LIST_TOP_PX + (voff * layout::CELL_H) as f64;
        let rows = (LIST_ROWS as usize) - voff;
        if !(top..(top + (rows * layout::CELL_H) as f64)).contains(&y) {
            return -1;
        }
        let row = ((y - top) / layout::CELL_H as f64).floor() as usize;
        let idx = self.state.scroll_y + row;
        if idx < self.state.labels.len() {
            idx as i32
        } else {
            -1
        }
    }

    pub fn title_width_px(&self) -> usize {
        self.state.title.chars().count() * CELL_W
    }

    /// `frame_ms` is a monotonically increasing timestamp (ms); the JS side
    /// renders all wasm menus on an animation loop so titles and long names can
    /// scroll horizontally.
    pub fn render_rgba(&mut self, frame_ms: f64) -> Vec<u8> {
        let frame_ms = frame_ms.max(0.0);
        // Re-anchor the name marquee whenever the selection changes, so the
        // 1-second pause restarts on every navigation move.
        self.marquee.on_selection(self.state.selected, frame_ms);
        let item_ms = self.marquee.item_ms(frame_ms);

        let state = self.state.clone();
        if state.is_main_menu() {
            return render_main_menu_rgba(&state, frame_ms as u32, item_ms);
        }

        let view = MenuView {
            state: &state,
            title_ms: frame_ms,
            item_ms,
        };
        let completed = self
            .terminal
            .draw(|frame| view.render(frame))
            .expect("TestBackend is infallible");
        rasterize_buffer(completed.buffer)
    }
}

impl WasmMenuRenderer {
    fn show_labels(&mut self, title: String, labels: Vec<String>, footer: String) {
        self.state = MenuState {
            title: normalize_text(&title),
            labels: labels.into_iter().map(|l| normalize_text(&l)).collect(),
            footer: normalize_text(&footer),
            selected: 0,
            scroll_y: 0,
        };
        // Force the marquee to re-anchor (and pause) on the first render of
        // the new menu.
        self.marquee = Marquee::new();
        let _ = self.terminal.clear();
    }

    fn visible_rows(&self) -> usize {
        if self.state.is_main_menu() {
            MAIN_OPTION_MAX_VISIBLE
        } else {
            LIST_ROWS as usize
        }
    }

    fn clamp_scroll(&mut self) {
        let visible = self.visible_rows();
        if self.state.selected < self.state.scroll_y {
            self.state.scroll_y = self.state.selected;
        } else if self.state.selected >= self.state.scroll_y + visible {
            self.state.scroll_y = self.state.selected + 1 - visible;
        }
        self.state.scroll_y = self.state.scroll_y.min(self.max_scroll());
    }

    fn max_scroll(&self) -> usize {
        self.state.labels.len().saturating_sub(self.visible_rows())
    }
}

// ---------------------------------------------------------------------------
// Ratatui-based list/menu view
// ---------------------------------------------------------------------------

struct MenuView<'a> {
    state: &'a MenuState,
    title_ms: f64, // absolute time → title marquee scrolls continuously
    item_ms: f64,  // time since selection changed → per-item marquee pauses on move
}

impl MenuView<'_> {
    fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        // Fill the full background.
        frame.render_widget(
            Block::default().style(Style::default().fg(gb_c2()).bg(gb_c0())),
            area,
        );
        self.render_header(frame, area);
        self.render_footer(frame, area);
        let (list_area, visible_end) = self.list_layout(area);
        self.render_list(frame, list_area, visible_end);
        self.render_scroll_indicators(frame, area, list_area, visible_end);
    }

    /// Draw the header bar with the scrolling title.
    fn render_header(&self, frame: &mut Frame<'_>, area: Rect) {
        let header = Rect::new(area.x, area.y, area.width, HEADER_ROWS);
        frame.render_widget(
            Block::default().style(Style::default().bg(gb_c1())),
            header,
        );
        let title = marquee_window(&self.state.title, header.width as usize, self.title_ms);
        frame.render_widget(
            Paragraph::new(title)
                .alignment(Alignment::Center)
                .style(Style::default().fg(gb_c3()).bg(gb_c1())),
            header,
        );
    }

    /// Draw the footer bar with centered static text.
    fn render_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let footer = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(FOOTER_ROWS),
            area.width,
            FOOTER_ROWS,
        );
        frame.render_widget(
            Block::default().style(Style::default().bg(gb_c1())),
            footer,
        );
        frame.render_widget(
            Paragraph::new(self.state.footer.clone())
                .alignment(Alignment::Center)
                .style(Style::default().fg(gb_c3()).bg(gb_c1())),
            footer,
        );
    }

    /// Compute the list area rect and the exclusive visible-end index.
    ///
    /// Centers the list block horizontally so short menus (e.g. PAUSED) become
    /// a centered block while long ROM lists stay near full width.  Vertically
    /// centers short menus between the header and footer.
    fn list_layout(&self, area: Rect) -> (Rect, usize) {
        let content_cols = self
            .state
            .labels
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0)
            .min(area.width as usize - 2)
            + 2;
        let lw = (content_cols as u16).min(area.width);
        let lx = area.x + (area.width - lw) / 2;
        let voff = list_voffset(self.state.labels.len()) as u16;
        let list_rows = LIST_ROWS - voff;
        let list_area = Rect::new(lx, area.y + HEADER_ROWS + voff, lw, list_rows);
        let visible_end =
            (self.state.scroll_y + list_rows as usize).min(self.state.labels.len());
        (list_area, visible_end)
    }

    /// Render the scrollable list of items within `list_area`.  The selected
    /// item is indented by `"> "` and its name scrolls horizontally when it
    /// overflows the available columns.
    fn render_list(&self, frame: &mut Frame<'_>, list_area: Rect, visible_end: usize) {
        let sel_avail = (list_area.width as usize).saturating_sub(2);
        let items: Vec<ListItem<'_>> = self.state.labels[self.state.scroll_y..visible_end]
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let text = if self.state.scroll_y + i == self.state.selected
                    && label.chars().count() > sel_avail
                {
                    marquee_window(label, sel_avail, self.item_ms)
                } else {
                    label.clone()
                };
                ListItem::new(text).style(Style::default().fg(gb_c2()).bg(gb_c0()))
            })
            .collect();

        let mut list_state = ListState::default();
        if self.state.selected >= self.state.scroll_y && self.state.selected < visible_end {
            list_state.select(Some(self.state.selected - self.state.scroll_y));
        }

        let list = List::new(items)
            .style(Style::default().fg(gb_c2()).bg(gb_c0()))
            .highlight_symbol("> ")
            .highlight_style(Style::default().fg(gb_c3()).bg(gb_c1()));
        frame.render_stateful_widget(list, list_area, &mut list_state);
    }

    /// Draw `^` / `V` scroll indicators when items are hidden above or below.
    fn render_scroll_indicators(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        list_area: Rect,
        visible_end: usize,
    ) {
        if self.state.scroll_y > 0 {
            let indicator =
                Rect::new(area.x + area.width.saturating_sub(1), list_area.y, 1, 1);
            frame.render_widget(
                Paragraph::new("^").style(Style::default().fg(gb_c3()).bg(gb_c0())),
                indicator,
            );
        }
        if visible_end < self.state.labels.len() {
            let indicator = Rect::new(
                area.x + area.width.saturating_sub(1),
                list_area.y + list_area.height.saturating_sub(1),
                1,
                1,
            );
            frame.render_widget(
                Paragraph::new("V").style(Style::default().fg(gb_c3()).bg(gb_c0())),
                indicator,
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Main-menu landscape overlay
// ---------------------------------------------------------------------------

fn render_main_menu_rgba(state: &MenuState, animation_ms: u32, item_ms: f64) -> Vec<u8> {
    let frame_idx = ((animation_ms / FRAME_DURATION_MS) % FRAME_COUNT as u32) as usize;
    let bg = LandscapeBg::parse(LANDSCAPE_BG_DATA);
    let mut rgba = vec![0u8; 160 * 144 * 4];
    bg.render_frame(frame_idx, &mut rgba);
    draw_main_menu_options(&mut rgba, state, item_ms);
    rgba
}

/// Draw the game list directly onto the title-screen background.
///
/// Each line is large and outlined to match the title lettering.  Lines share a
/// common left edge (left-aligned), and that block is centered horizontally and
/// placed high on the screen, above the water.  Long lists scroll within a
/// fixed window with up/down arrows.
fn draw_main_menu_options(rgba: &mut [u8], state: &MenuState, item_ms: f64) {
    let len = state.labels.len();
    if len == 0 {
        return;
    }
    let (scroll, visible) = main_menu_window(len, state.scroll_y);

    // Size the box to the widest label (capped at the marquee width) and center
    // it.  Capping keeps the layout stable while a long name scrolls.
    let max_cols = state
        .labels
        .iter()
        .map(|l| l.chars().count().min(MAIN_OPTION_MAX_CHARS))
        .max()
        .unwrap_or(0)
        + 2; // +2 for the "> " selection gutter
    let block_w = max_cols.saturating_sub(1) * MAIN_OPTION_CHAR_ADV + MAIN_OPTION_GLYPH_W;
    let left_x = SCREEN_WIDTH.saturating_sub(block_w) / 2;

    for row in 0..visible {
        let idx = scroll + row;
        let selected = idx == state.selected;
        let label = &state.labels[idx];
        // Selected name scrolls horizontally when it overflows; others truncate.
        let content = item_display_text(label, MAIN_OPTION_MAX_CHARS, selected, item_ms);
        let prefix = if selected { "> " } else { "  " };
        let line = format!("{prefix}{content}");
        let y = main_menu_option_y(row, visible);
        let fill = if selected {
            MENU_TEXT_SELECTED
        } else {
            MENU_TEXT_NORMAL
        };
        draw_outlined_text(rgba, left_x, y, &line, fill, MENU_TEXT_OUTLINE, MAIN_OPTION_SCALE);
    }

    // Scroll arrows when items are hidden above or below.
    let cx = SCREEN_WIDTH / 2;
    if scroll > 0 {
        let top = main_menu_block_top_y(visible);
        draw_scroll_arrow(rgba, cx, top.saturating_sub(6), true);
    }
    if scroll + visible < len {
        let last_y = main_menu_option_y(visible.saturating_sub(1), visible);
        draw_scroll_arrow(rgba, cx, last_y + MAIN_OPTION_GLYPH_H + 2, false);
    }
}

/// Return the display text for one main-menu item.  The selected item scrolls
/// horizontally via the marquee when its name exceeds `max_chars`; other items
/// are hard-truncated.
fn item_display_text(label: &str, max_chars: usize, selected: bool, item_ms: f64) -> String {
    if label.chars().count() > max_chars {
        if selected {
            marquee_window(label, max_chars, item_ms)
        } else {
            label.chars().take(max_chars).collect()
        }
    } else {
        label.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_wraps_and_clamps_scroll() {
        let mut menu = WasmMenuRenderer::new();
        menu.show_labels(
            "Test".to_owned(),
            vec!["One".to_owned(), "Two".to_owned(), "Three".to_owned()],
            "Footer".to_owned(),
        );
        menu.move_selection(-1);
        assert_eq!(menu.selected_index(), 2);
        menu.move_selection(1);
        assert_eq!(menu.selected_index(), 0);
    }

    #[test]
    fn tap_coordinates_map_to_visible_rows() {
        let mut menu = WasmMenuRenderer::new();
        menu.show_labels(
            "Test".to_owned(),
            vec!["One".to_owned(), "Two".to_owned()],
            "Footer".to_owned(),
        );
        // 2 items are vertically centered: voff = (LIST_ROWS(11) - 2)/2 = 4, so
        // the list starts at LIST_TOP_PX(11) + 4*CELL_H(11) = 55.
        // row 0 → y in [55, 66), row 1 → y in [66, 77).
        assert_eq!(menu.item_at(80.0, 56.0), 0);
        assert_eq!(menu.item_at(80.0, 67.0), 1);
        assert_eq!(menu.item_at(80.0, 30.0), -1);
    }
}
