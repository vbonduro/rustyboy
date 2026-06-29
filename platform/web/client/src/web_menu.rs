use font8x8::{UnicodeFonts, BASIC_FONTS};
use js_sys::Array;
use ratatui::{
    backend::TestBackend,
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use wasm_bindgen::prelude::*;

use crate::landscape_bg::{get_frame_rgba, FRAME_COUNT, FRAME_DURATION_MS};

const SCREEN_WIDTH: usize = 160;
const SCREEN_HEIGHT: usize = 144;
const CELL_W: usize = 6;
// Cell *pitch* is taller than the glyph (MENU_GLYPH_H) so rows have vertical
// breathing room; the glyph is drawn 6×8 centered in each cell.
const CELL_H: usize = 11;
const MENU_GLYPH_H: usize = 8; // actual glyph height inside the 11px cell → ~3px gap between rows
const TERM_W: u16 = (SCREEN_WIDTH / CELL_W) as u16; // 26
const TERM_H: u16 = (SCREEN_HEIGHT / CELL_H) as u16; // 13
const HEADER_ROWS: u16 = 1;
const FOOTER_ROWS: u16 = 1;
const LIST_ROWS: u16 = TERM_H - HEADER_ROWS - FOOTER_ROWS; // 11
const LIST_TOP_PX: f64 = (HEADER_ROWS as usize * CELL_H) as f64; // 11
const RGBA_LEN: usize = SCREEN_WIDTH * SCREEN_HEIGHT * 4;
// Main-menu game list, drawn directly over the title-screen background as
// crisp all-caps text. Glyphs are the 8x8 font8x8 face; lines are left-aligned
// to a common left edge and that block is centered horizontally and placed high
// on the screen (above the water).
const BASIC_FONT_H: usize = 8;
const MAIN_OPTION_SCALE: usize = 1;
const MAIN_OPTION_CHAR_ADV: usize = 7 * MAIN_OPTION_SCALE; // 7px / char (font8x8 is ~6 wide)
const MAIN_OPTION_GLYPH_W: usize = BASIC_FONT_H * MAIN_OPTION_SCALE; // 8px
const MAIN_OPTION_GLYPH_H: usize = BASIC_FONT_H * MAIN_OPTION_SCALE; // 8px
const MAIN_OPTION_STEP_PX: usize = (BASIC_FONT_H + 2) * MAIN_OPTION_SCALE; // 10px / row
const MAIN_OPTION_CENTER_Y: usize = 70; // block vertical center (above water ≈ y98)
const MAIN_OPTION_MIN_TOP_Y: usize = 40; // never collide with the title
const MAIN_OPTION_MAX_VISIBLE: usize = 5; // rows that fit in the sky band; rest scroll
const MAIN_OPTION_MAX_CHARS: usize = 18; // content chars before a name scrolls/truncates
const MARQUEE_MS: u32 = 200; // ms per column of horizontal name scroll
// Number of scroll-column ticks held at position 0 before the marquee starts
// moving. Each tick = MARQUEE_MS, so 6 ticks ≈ 1.2 s initial pause.
const MARQUEE_PAUSE_COLS: usize = 5; // 5 × MARQUEE_MS(200) = 1.0 s pause before scrolling

// Menu palette, sampled from the title screen's own colors so every menu shares
// the muted cream-on-dark-green look (rather than the classic bright DMG lime).
// C0 = background, C1 = bars / selection fill, C2 = text, C3 = bright/highlight.
const C0: [u8; 4] = [0x18, 0x2A, 0x20, 0xFF]; // (24,42,32) dark green
const C1: [u8; 4] = [0x3E, 0x5A, 0x46, 0xFF]; // (62,90,70) mid green
const C2: [u8; 4] = [0xA6, 0xBC, 0x96, 0xFF]; // (166,188,150) cream text
const C3: [u8; 4] = [0xB0, 0xC6, 0xA0, 0xFF]; // (176,198,160) bright cream
// Aliases for the main-menu text drawn over the landscape: cream fill with the
// title's dark-green outline.
const MENU_TEXT_OUTLINE: [u8; 4] = [0x1F, 0x37, 0x28, 0xFF]; // (31,55,40)
const MENU_TEXT_NORMAL: [u8; 4] = C2;
const MENU_TEXT_SELECTED: [u8; 4] = C3;

#[derive(Clone, Default)]
struct MenuState {
    title: String,
    labels: Vec<String>,
    footer: String,
    selected: usize,
    scroll_y: usize,
}

#[wasm_bindgen]
pub struct WasmMenuRenderer {
    terminal: Terminal<TestBackend>,
    state: MenuState,
    // The selected name's horizontal scroll pauses for ~1s after the selection
    // *changes*, so navigating the list always shows the name from the start.
    // We track which item the marquee clock is anchored to and the timestamp it
    // was anchored at; both reset when `selected` changes.
    marquee_sel: usize,
    marquee_epoch_ms: f64,
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
            marquee_sel: usize::MAX,
            marquee_epoch_ms: 0.0,
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
        let top = LIST_TOP_PX + (voff * CELL_H) as f64;
        let rows = (LIST_ROWS as usize) - voff;
        if !(top..(top + (rows * CELL_H) as f64)).contains(&y) {
            return -1;
        }

        let row = ((y - top) / CELL_H as f64).floor() as usize;
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
        // Re-anchor the name marquee whenever the selection changes, so the 1s
        // pause restarts on every move through the list.
        if self.state.selected != self.marquee_sel {
            self.marquee_sel = self.state.selected;
            self.marquee_epoch_ms = frame_ms;
        }
        let item_ms = frame_ms - self.marquee_epoch_ms;

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
            labels: labels
                .into_iter()
                .map(|label| normalize_text(&label))
                .collect(),
            footer: normalize_text(&footer),
            selected: 0,
            scroll_y: 0,
        };
        // Force the marquee to re-anchor (and pause) on the first render of the
        // new menu.
        self.marquee_sel = usize::MAX;
        let _ = self.terminal.clear();
    }

    /// Rows visible at once — fewer on the main menu (the list sits in the sky
    /// band above the water) than on the full-screen ratatui menus.
    fn visible_rows(&self) -> usize {
        if self.state.is_main_menu() {
            MAIN_OPTION_MAX_VISIBLE
        } else {
            LIST_ROWS as usize
        }
    }

    fn clamp_scroll(&mut self) {
        let visible_rows = self.visible_rows();
        if self.state.selected < self.state.scroll_y {
            self.state.scroll_y = self.state.selected;
        } else if self.state.selected >= self.state.scroll_y + visible_rows {
            self.state.scroll_y = self.state.selected + 1 - visible_rows;
        }
        self.state.scroll_y = self.state.scroll_y.min(self.max_scroll());
    }

    fn max_scroll(&self) -> usize {
        self.state.labels.len().saturating_sub(self.visible_rows())
    }
}

impl MenuState {
    fn is_main_menu(&self) -> bool {
        self.title == "RUSTYBOY"
    }
}

struct MenuView<'a> {
    state: &'a MenuState,
    title_ms: f64, // absolute time → title marquee runs continuously
    item_ms: f64,  // time since selection changed → selected-name marquee pauses on move
}

/// Rows to push the list down so a short menu (fewer items than fit) sits
/// vertically centered between the header and footer; 0 once it fills/scrolls.
fn list_voffset(len: usize) -> usize {
    (LIST_ROWS as usize).saturating_sub(len) / 2
}

impl MenuView<'_> {
    fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        let bg = Style::default().fg(gb_c2()).bg(gb_c0());
        frame.render_widget(Block::default().style(bg), area);

        let header = Rect::new(area.x, area.y, area.width, HEADER_ROWS);
        frame.render_widget(Block::default().style(Style::default().bg(gb_c1())), header);
        let title = marquee_window(
            &normalize_text(&self.state.title),
            header.width as usize,
            self.title_ms,
        );
        frame.render_widget(
            Paragraph::new(title)
                .alignment(Alignment::Center)
                .style(Style::default().fg(gb_c3()).bg(gb_c1())),
            header,
        );

        let footer = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(FOOTER_ROWS),
            area.width,
            FOOTER_ROWS,
        );
        frame.render_widget(Block::default().style(Style::default().bg(gb_c1())), footer);
        frame.render_widget(
            Paragraph::new(normalize_text(&self.state.footer))
                .alignment(Alignment::Center)
                .style(Style::default().fg(gb_c3()).bg(gb_c1())),
            footer,
        );

        // Center the list block (width = widest label + the "> " gutter, capped to
        // the screen) and left-align items within it. Short menus like PAUSED
        // become a centered block; long ROM lists stay near full width.
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
        // Vertically center short menus (e.g. PAUSED) between header and footer.
        let voff = list_voffset(self.state.labels.len()) as u16;
        let list_rows = LIST_ROWS - voff;
        let list_area = Rect::new(lx, area.y + HEADER_ROWS + voff, lw, list_rows);
        let visible_end =
            (self.state.scroll_y + list_rows as usize).min(self.state.labels.len());
        // The selected row is indented by the "> " highlight symbol, so it has 2
        // fewer columns; scroll its name horizontally when it overflows.
        let sel_avail = (list_area.width as usize).saturating_sub(2);
        let items: Vec<ListItem<'_>> = self.state.labels[self.state.scroll_y..visible_end]
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let label = label.to_ascii_uppercase();
                let text = if self.state.scroll_y + i == self.state.selected
                    && label.chars().count() > sel_avail
                {
                    marquee_window(&label, sel_avail, self.item_ms)
                } else {
                    label
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

        if self.state.scroll_y > 0 {
            let indicator = Rect::new(area.x + area.width.saturating_sub(1), list_area.y, 1, 1);
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

fn render_main_menu_rgba(state: &MenuState, animation_ms: u32, item_ms: f64) -> Vec<u8> {
    let frame_idx = ((animation_ms / FRAME_DURATION_MS) % FRAME_COUNT as u32) as usize;
    let mut rgba = get_frame_rgba(frame_idx);
    draw_main_menu_options(&mut rgba, state, item_ms);
    rgba
}

/// Top-left y (in screen pixels) of the game-list block: vertically centered on
/// `MAIN_OPTION_CENTER_Y` and clamped below the title. Shared by the renderer
/// and the touch hit-test so they always agree on row positions.
fn main_menu_block_top_y(label_count: usize) -> usize {
    let block_h = label_count.saturating_sub(1) * MAIN_OPTION_STEP_PX;
    MAIN_OPTION_CENTER_Y
        .saturating_sub(block_h / 2)
        .saturating_sub(MAIN_OPTION_GLYPH_H / 2)
        .max(MAIN_OPTION_MIN_TOP_Y)
}

fn main_menu_option_y(idx: usize, label_count: usize) -> usize {
    main_menu_block_top_y(label_count) + idx * MAIN_OPTION_STEP_PX
}

/// The (scroll offset, visible row count) window for a list of `len` items.
/// Shared by the renderer and the touch hit-test.
fn main_menu_window(len: usize, scroll_y: usize) -> (usize, usize) {
    let visible = MAIN_OPTION_MAX_VISIBLE.min(len);
    let scroll = scroll_y.min(len.saturating_sub(visible));
    (scroll, visible)
}

/// Draw the game list directly onto the title-screen background. Each line is
/// large, outlined and palette-matched to the title lettering. Lines share a
/// common left edge (left-aligned), and that block is centered horizontally and
/// placed high on the screen, above the water. Long lists scroll within a fixed
/// window with up/down arrows.
fn draw_main_menu_options(rgba: &mut [u8], state: &MenuState, item_ms: f64) {
    let len = state.labels.len();
    if len == 0 {
        return;
    }
    let (scroll, visible) = main_menu_window(len, state.scroll_y);

    // Size the box to the widest label (capped at the marquee width, +2 for the
    // selection gutter) and center it. Capping keeps it stable while a long name
    // scrolls; sizing to content keeps short menus (CONTINUE/GAMES) centered
    // rather than left-shifted inside an over-wide fixed box.
    let max_cols = state
        .labels
        .iter()
        .map(|l| l.chars().count().min(MAIN_OPTION_MAX_CHARS))
        .max()
        .unwrap_or(0)
        + 2;
    let block_w = max_cols.saturating_sub(1) * MAIN_OPTION_CHAR_ADV + MAIN_OPTION_GLYPH_W;
    let left_x = SCREEN_WIDTH.saturating_sub(block_w) / 2;

    for row in 0..visible {
        let idx = scroll + row;
        let selected = idx == state.selected;
        let label = state.labels[idx].to_ascii_uppercase();
        // The selected name scrolls horizontally if it overflows; others are
        // truncated to the box width.
        let content = if label.chars().count() > MAIN_OPTION_MAX_CHARS {
            if selected {
                marquee_window(&label, MAIN_OPTION_MAX_CHARS, item_ms)
            } else {
                label.chars().take(MAIN_OPTION_MAX_CHARS).collect()
            }
        } else {
            label
        };
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

    // Scroll arrows when there are off-screen items.
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

/// A small outlined triangle (5x3) centered horizontally at `cx`, pointing up
/// or down — the main-menu scroll indicator.
fn draw_scroll_arrow(rgba: &mut [u8], cx: usize, top_y: usize, up: bool) {
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
    // Dark halo first (1px in each direction), then the cream triangle.
    for off in [-1, 1] {
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

/// Draw `text` with a 1px outline (drawn in all 8 directions) then the fill on
/// top, mirroring the title's outlined lettering for legibility over the scene.
fn draw_outlined_text(
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

fn draw_basic_glyph_scaled(
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

/// Render a font8x8 glyph downscaled to fit `cell_w × cell_h` pixels. Each of
/// the 8×8 source bits is mapped to a target pixel via the *rounded* projection
/// `(round(src_x*cell_w/8), round(src_y*cell_h/8))`; the target pixel is lit if
/// any contributing source bit is on (OR reduction). Rounding (rather than
/// truncation) spreads each source row/column across the target grid in
/// proportion to its real position, which preserves stroke-width differences
/// that floor() collapses — e.g. font8x8's serif 'T' (full-width top bar) stays
/// distinct from 'I' (narrow top bar) even at a 6px cell. Produces legible text
/// at any cell size without hand-designed glyphs.
fn draw_basic_glyph_to_cell(
    rgba: &mut [u8],
    x0: usize,
    y0: usize,
    cell_w: usize,
    cell_h: usize,
    ch: char,
    color: [u8; 4],
) {
    let Some(glyph) = BASIC_FONTS.get(ch).or_else(|| BASIC_FONTS.get('?')) else {
        return;
    };
    // Draw the glyph at a fixed height (MENU_GLYPH_H), vertically centered in the
    // cell, so a taller cell pitch adds inter-row spacing without stretching the
    // letters (8→8 vertically is 1:1, i.e. crisp).
    let glyph_h = MENU_GLYPH_H.min(cell_h);
    let y_off = (cell_h - glyph_h) / 2;
    for (src_y, bits) in glyph.iter().copied().enumerate() {
        let ty = (src_y * glyph_h + 4) / 8 + y_off;
        for src_x in 0..8usize {
            if bits & (1 << src_x) == 0 {
                continue;
            }
            let tx = (src_x * cell_w + 4) / 8;
            put_pixel(rgba, x0 + tx, y0 + ty, color);
        }
    }
}

fn fill_rect(rgba: &mut [u8], x: usize, y: usize, w: usize, h: usize, color: [u8; 4]) {
    for py in y..(y + h).min(SCREEN_HEIGHT) {
        for px in x..(x + w).min(SCREEN_WIDTH) {
            put_pixel(rgba, px, py, color);
        }
    }
}

/// Scroll `text` horizontally within `width_cols` columns. Returns `text`
/// unchanged when it fits. For longer strings, holds at offset 0 for
/// ~`MARQUEE_PAUSE_COLS * MARQUEE_MS` ms, then advances one column per
/// `MARQUEE_MS`. The pause repeats at the start of each ring loop.
fn marquee_window(text: &str, width_cols: usize, frame_ms: f64) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width_cols {
        return text.to_owned();
    }
    // Gap of spaces between end and beginning when the ring wraps.
    const GAP: usize = 3;
    let mut ring = chars;
    for _ in 0..GAP {
        ring.push(' ');
    }
    let ring_len = ring.len();
    let cycle = MARQUEE_PAUSE_COLS + ring_len;
    let pos = (frame_ms / MARQUEE_MS as f64) as usize % cycle;
    let offset = pos.saturating_sub(MARQUEE_PAUSE_COLS).min(ring_len);
    (0..width_cols)
        .map(|i| ring[(offset + i) % ring_len])
        .collect()
}

fn normalize_text(input: &str) -> String {
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

fn rasterize_buffer(buffer: &Buffer) -> Vec<u8> {
    rasterize_buffer_with_cell(buffer, CELL_W, CELL_H)
}

fn rasterize_buffer_with_cell(buffer: &Buffer, cell_w: usize, cell_h: usize) -> Vec<u8> {
    let mut rgba = vec![0; RGBA_LEN];
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
                draw_symbol(&mut rgba, cell_x, cell_y, cell_w, cell_h, ch, fg);
            }
        }
    }
    rgba
}

fn fill_cell(
    rgba: &mut [u8],
    cell_x: usize,
    cell_y: usize,
    cell_w: usize,
    cell_h: usize,
    color: [u8; 4],
) {
    fill_rect(
        rgba,
        cell_x * cell_w,
        cell_y * cell_h,
        cell_w,
        cell_h,
        color,
    );
}

fn draw_symbol(
    rgba: &mut [u8],
    cell_x: usize,
    cell_y: usize,
    cell_w: usize,
    cell_h: usize,
    ch: char,
    color: [u8; 4],
) {
    let x0 = cell_x * cell_w;
    let y0 = cell_y * cell_h;
    if let Some(pattern) = quadrant_pattern(ch) {
        draw_quadrant_symbol(rgba, x0, y0, cell_w, cell_h, pattern, color);
        return;
    }

    match ch {
        '░' | '▒' | '▓' => draw_shade_symbol(rgba, x0, y0, cell_w, cell_h, ch, color),
        ch if is_braille_symbol(ch) => {
            draw_braille_symbol(rgba, x0, y0, cell_w, cell_h, ch, color)
        }
        ch => draw_basic_glyph_to_cell(rgba, x0, y0, cell_w, cell_h, ch, color),
    }
}

fn quadrant_pattern(ch: char) -> Option<u8> {
    match ch {
        '▘' => Some(0b0001),
        '▝' => Some(0b0010),
        '▀' => Some(0b0011),
        '▖' => Some(0b0100),
        '▌' => Some(0b0101),
        '▞' => Some(0b0110),
        '▛' => Some(0b0111),
        '▗' => Some(0b1000),
        '▚' => Some(0b1001),
        '▐' => Some(0b1010),
        '▜' => Some(0b1011),
        '▄' => Some(0b1100),
        '▙' => Some(0b1101),
        '▟' => Some(0b1110),
        '█' => Some(0b1111),
        _ => None,
    }
}

fn draw_quadrant_symbol(
    rgba: &mut [u8],
    x0: usize,
    y0: usize,
    cell_w: usize,
    cell_h: usize,
    pattern: u8,
    color: [u8; 4],
) {
    let positions = [(0, 0, 0), (1, 0, 1), (0, 1, 2), (1, 1, 3)];

    for (quad_x, quad_y, bit) in positions {
        if pattern & (1 << bit) == 0 {
            continue;
        }

        let x1 = x0 + quad_x * cell_w / 2;
        let x2 = x0 + (quad_x + 1) * cell_w / 2;
        let y1 = y0 + quad_y * cell_h / 2;
        let y2 = y0 + (quad_y + 1) * cell_h / 2;
        fill_rect(
            rgba,
            x1,
            y1,
            x2.saturating_sub(x1).max(1),
            y2.saturating_sub(y1).max(1),
            color,
        );
    }
}

fn draw_shade_symbol(
    rgba: &mut [u8],
    x0: usize,
    y0: usize,
    cell_w: usize,
    cell_h: usize,
    ch: char,
    color: [u8; 4],
) {
    for y in 0..cell_h {
        for x in 0..cell_w {
            let on = match ch {
                '░' => (x + y) % 4 == 0,
                '▒' => (x + y) % 2 == 0,
                '▓' => (x + y) % 4 != 0,
                _ => false,
            };
            if on {
                put_pixel(rgba, x0 + x, y0 + y, color);
            }
        }
    }
}

fn is_braille_symbol(ch: char) -> bool {
    ('\u{2800}'..='\u{28ff}').contains(&ch)
}

fn draw_braille_symbol(
    rgba: &mut [u8],
    x0: usize,
    y0: usize,
    cell_w: usize,
    cell_h: usize,
    ch: char,
    color: [u8; 4],
) {
    let dots = ch as u32 - 0x2800;
    let positions = [
        (0, 0, 0),
        (0, 1, 1),
        (0, 2, 2),
        (1, 0, 3),
        (1, 1, 4),
        (1, 2, 5),
        (0, 3, 6),
        (1, 3, 7),
    ];

    for (dot_x, dot_y, bit) in positions {
        if dots & (1 << bit) == 0 {
            continue;
        }

        let x1 = x0 + dot_x * cell_w / 2;
        let x2 = x0 + (dot_x + 1) * cell_w / 2;
        let y1 = y0 + dot_y * cell_h / 4;
        let y2 = y0 + (dot_y + 1) * cell_h / 4;
        fill_rect(
            rgba,
            x1,
            y1,
            x2.saturating_sub(x1).max(1),
            y2.saturating_sub(y1).max(1),
            color,
        );
    }
}

fn put_pixel(rgba: &mut [u8], x: usize, y: usize, color: [u8; 4]) {
    if x >= SCREEN_WIDTH || y >= SCREEN_HEIGHT {
        return;
    }
    let idx = (y * SCREEN_WIDTH + x) * 4;
    rgba[idx..idx + 4].copy_from_slice(&color);
}

fn color_to_rgba(color: Color, fallback: [u8; 4]) -> [u8; 4] {
    match color {
        Color::Rgb(r, g, b) => [r, g, b, 0xFF],
        Color::Black => C0,
        Color::Green => C2,
        Color::LightGreen => C3,
        Color::Reset => fallback,
        _ => fallback,
    }
}

fn gb_c0() -> Color {
    Color::Rgb(C0[0], C0[1], C0[2])
}

fn gb_c1() -> Color {
    Color::Rgb(C1[0], C1[1], C1[2])
}

fn gb_c2() -> Color {
    Color::Rgb(C2[0], C2[1], C2[2])
}

fn gb_c3() -> Color {
    Color::Rgb(C3[0], C3[1], C3[2])
}

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
