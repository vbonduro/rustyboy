//! ratatui UI layer for menus and loading screens.
//!
//! Compiles on both ARM (embedded) and host (tests).  ARM-specific parts
//! — the `FbTarget` DrawTarget, `Ui` struct, and `mousefood` colour theme —
//! are gated by `#[cfg(target_arch = "arm")]`.
//!
//! `draw_menu` / `draw_loading` are pure ratatui render functions compiled for
//! ARM firmware and host unit tests; the host test suite exercises them via
//! `TestBackend`.

extern crate alloc;

#[cfg(any(target_arch = "arm", test, feature = "std"))]
use alloc::string::ToString;

#[cfg(any(target_arch = "arm", test, feature = "std"))]
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Widget},
    Frame,
};

#[cfg(target_arch = "arm")]
use ratatui::Terminal;

#[cfg(any(target_arch = "arm", test, feature = "std"))]
use crate::display::LoadingFrame;
#[cfg(any(target_arch = "arm", test, feature = "std"))]
use crate::menu::MenuFrame;

// ARM-specific imports — mousefood + FbTarget
#[cfg(target_arch = "arm")]
use super::eg_target::FbTarget;
#[cfg(target_arch = "arm")]
use mousefood::{
    embedded_graphics::{
        mono_font::ascii::FONT_8X13,
        pixelcolor::{Rgb565, Rgb888},
        prelude::RgbColor,
    },
    ColorTheme, EmbeddedBackend, EmbeddedBackendConfig, TerminalAlignment,
};

// ---------------------------------------------------------------------------
// Marquee constants
// ---------------------------------------------------------------------------

/// Columns visible in the item text area (30 cols total, minus cursor+gap).
///
/// The embedded terminal uses a 30-column grid (240 px / 8 px font width).
/// Regular rows reserve 2 columns for the cursor and, for marked ROMs, 2
/// columns for the marker.
pub const MARQUEE_TEXT_COLS: usize = 28;
pub const MARQUEE_HOLD_FRAMES: u32 = 30;
pub const MARQUEE_FRAMES_PER_STEP: u32 = 1;
/// How many animation-tick calls between redraws (mirrors menu.rs constant).
pub const MENU_MARQUEE_REDRAW_FRAMES: u32 = MARQUEE_FRAMES_PER_STEP;

/// Return true if `text` is too long to fit without scrolling.
///
/// `marked` items reserve two columns for the `*` marker, shrinking the window.
pub fn menu_item_needs_marquee(text: &str, marked: bool) -> bool {
    let cols = if marked {
        MARQUEE_TEXT_COLS.saturating_sub(2)
    } else {
        MARQUEE_TEXT_COLS
    };
    text.len() > cols
}

/// Compute the horizontal scroll offset in *characters* for a marquee.
///
/// Mirrors the pixel-level logic from `display/menu.rs::marquee_scroll_px`
/// but operates in column units.
pub fn marquee_char_offset(text: &str, marquee_frame: u32, cols: usize) -> usize {
    let text_len = text.len();
    if text_len <= cols {
        return 0;
    }
    // Gap: 3 blank characters between end and re-start of the scroll loop.
    const GAP: usize = 3;
    let period = text_len + GAP;
    let scroll_steps = period; // 1 step = 1 char
    let scroll_frames = (scroll_steps as u32).saturating_mul(MARQUEE_FRAMES_PER_STEP);
    let phase = marquee_frame % (MARQUEE_HOLD_FRAMES + scroll_frames);
    if phase < MARQUEE_HOLD_FRAMES {
        0
    } else {
        ((phase - MARQUEE_HOLD_FRAMES) / MARQUEE_FRAMES_PER_STEP) as usize % period
    }
}

// ---------------------------------------------------------------------------
// MarqueeLine widget
// ---------------------------------------------------------------------------

/// A single-row widget that renders a scrolling text line.
///
/// If the text fits within `cols`, it is rendered without scrolling.
/// Otherwise `offset` characters are skipped from the start and the text
/// wraps (period = text + 3-char gap).
#[cfg(any(target_arch = "arm", test, feature = "std"))]
struct MarqueeLine<'a> {
    text: &'a str,
    offset: usize,
    cols: usize,
    style: Style,
}

#[cfg(any(target_arch = "arm", test, feature = "std"))]
impl<'a> MarqueeLine<'a> {
    fn new(text: &'a str, offset: usize, cols: usize, style: Style) -> Self {
        Self {
            text,
            offset,
            cols,
            style,
        }
    }
}

#[cfg(any(target_arch = "arm", test, feature = "std"))]
impl Widget for MarqueeLine<'_> {
    fn render(self, area: Rect, buf: &mut ratatui::prelude::Buffer) {
        if area.height == 0 || area.width == 0 || self.text.is_empty() {
            return;
        }
        let cols = area.width.min(self.cols as u16) as usize;
        let text = self.text;
        let text_len = text.len();
        let text_bytes = text.as_bytes();

        for col in 0..cols {
            let src_char = if text_len <= cols {
                // No scrolling: render text left-aligned, pad with space.
                if col < text_len {
                    text_bytes[col]
                } else {
                    b' '
                }
            } else {
                // Scrolling: wrap around with 3-char gap.
                const GAP: usize = 3;
                let period = text_len + GAP;
                let src = (col + self.offset) % period;
                if src < text_len {
                    text_bytes[src]
                } else {
                    b' '
                }
            };
            let x = area.x + col as u16;
            if x < buf.area.x + buf.area.width {
                let cell = buf.cell_mut(ratatui::layout::Position { x, y: area.y });
                if let Some(cell) = cell {
                    let ch = char::from(src_char);
                    cell.set_char(ch);
                    cell.set_style(self.style);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// DMG palette as Rgb888 (ARM-only: used for ColorTheme mapping)
// ---------------------------------------------------------------------------

#[cfg(target_arch = "arm")]
const DMG_C0: Rgb888 = Rgb888::new(0xE0, 0xF8, 0xD0);
#[cfg(target_arch = "arm")]
const DMG_C1: Rgb888 = Rgb888::new(0x88, 0xC0, 0x70);
#[cfg(target_arch = "arm")]
const DMG_C2: Rgb888 = Rgb888::new(0x34, 0x68, 0x56);
#[cfg(target_arch = "arm")]
const DMG_C3: Rgb888 = Rgb888::new(0x08, 0x18, 0x20);

/// Build a `ColorTheme` that maps ANSI terminal colours to the DMG palette.
#[cfg(target_arch = "arm")]
fn dmg_theme() -> ColorTheme {
    ColorTheme {
        foreground: DMG_C0,
        background: DMG_C3,
        white: DMG_C0,
        black: DMG_C3,
        red: Rgb888::RED,
        green: DMG_C1,
        yellow: Rgb888::YELLOW,
        blue: Rgb888::BLUE,
        magenta: Rgb888::MAGENTA,
        cyan: DMG_C1,
        light_red: Rgb888::RED,
        light_green: DMG_C0,
        light_yellow: Rgb888::YELLOW,
        light_blue: DMG_C1,
        light_magenta: Rgb888::MAGENTA,
        light_cyan: DMG_C1,
        gray: DMG_C2,
        dark_gray: DMG_C3,
    }
}

// ---------------------------------------------------------------------------
// Ui struct (ARM-only — owns FbTarget + Terminal<EmbeddedBackend>)
// ---------------------------------------------------------------------------

/// Persistent ratatui terminal + FbTarget.
///
/// Owns the `Terminal<EmbeddedBackend<'static, FbTarget, Rgb565>>`.
/// `GameDisplay` holds a `Ui` inside `GameDisplay::ui` and calls
/// `render_menu` / `render_loading`.
#[cfg(target_arch = "arm")]
pub struct Ui {
    fb: *mut FbTarget,
    terminal: Option<Terminal<EmbeddedBackend<'static, FbTarget, Rgb565>>>,
}

#[cfg(target_arch = "arm")]
impl Ui {
    /// Construct from a freshly-claimed static framebuffer.
    pub fn new(fb: FbTarget) -> Self {
        // Leak `fb` into a `&'static mut FbTarget` by boxing it and leaking.
        // SAFETY: `GameDisplay` is the sole owner of this Ui; it is constructed
        // once in `new_after_splash` and lives for the lifetime of the program.
        let fb_ref: &'static mut FbTarget = alloc::boxed::Box::leak(alloc::boxed::Box::new(fb));
        Self {
            fb: fb_ref as *mut FbTarget,
            terminal: None,
        }
    }

    fn terminal_mut(&mut self) -> &mut Terminal<EmbeddedBackend<'static, FbTarget, Rgb565>> {
        if self.terminal.is_none() {
            // SAFETY: `self.terminal` is the only owner of an EmbeddedBackend
            // borrowing this FbTarget.  We only materialize a new &'static mut
            // when no terminal exists; `release_terminal` drops the previous
            // backend before this path can run again.
            let fb_ref: &'static mut FbTarget = unsafe { &mut *self.fb };

            let config = EmbeddedBackendConfig {
                color_theme: dmg_theme(),
                font_regular: FONT_8X13,
                vertical_alignment: TerminalAlignment::Center,
                flush_callback: alloc::boxed::Box::new(|_| {}), // we flush manually
                ..EmbeddedBackendConfig::default()
            };

            let backend = EmbeddedBackend::new(fb_ref, config);
            self.terminal = Some(Terminal::new(backend).expect("ratatui Terminal::new"));
        }
        self.terminal.as_mut().expect("terminal was just created")
    }

    /// Force a full-screen repaint on the next `render_*` call.
    ///
    /// Call from `draw_letterbox_bars` after returning from a game session so
    /// that stale game pixels are overwritten.
    pub fn mark_full_repaint(&mut self) {
        if let Some(terminal) = self.terminal.as_mut() {
            terminal.backend_mut().display_mut().mark_all_dirty();
            // Also clear ratatui's previous-frame buffer so it re-emits every cell.
            let _ = terminal.clear();
        }
    }

    /// Drop ratatui's heap-backed cell buffers before entering game mode.
    pub fn release_terminal(&mut self) {
        self.terminal = None;
    }

    /// Render a [`MenuFrame`] into the framebuffer and return the dirty band.
    ///
    /// Returns `None` when ratatui's cell-diff determined nothing changed.
    pub fn render_menu(&mut self, frame: &MenuFrame<'_>) -> Option<super::RenderWindow> {
        let terminal = self.terminal_mut();
        let _ = terminal.draw(|f| draw_menu(f, frame));
        terminal.backend_mut().display_mut().take_dirty()
    }

    /// Render a [`LoadingFrame`] into the framebuffer and return the dirty band.
    pub fn render_loading(&mut self, frame: &LoadingFrame<'_>) -> Option<super::RenderWindow> {
        let terminal = self.terminal_mut();
        let _ = terminal.draw(|f| draw_loading(f, frame));
        terminal.backend_mut().display_mut().take_dirty()
    }

    /// Borrow the `FbTarget` for row-by-row DMA flushing.
    ///
    /// Called by `GameDisplay::flush_fb_window` to read pixel rows.
    pub fn terminal_backend_display_mut(&mut self) -> &mut FbTarget {
        self.terminal
            .as_mut()
            .expect("framebuffer flush requires an active terminal")
            .backend_mut()
            .display_mut()
    }
}

// ---------------------------------------------------------------------------
// Menu render function (works on both ARM and host)
// ---------------------------------------------------------------------------

#[cfg(any(target_arch = "arm", test, feature = "std"))]
pub fn draw_menu(f: &mut Frame, frame: &MenuFrame<'_>) {
    let area = f.area();

    const HEADER_ROWS: u16 = 3;
    const ITEM_ROWS: u16 = 2;
    const FOOTER_ROWS: u16 = 3;
    const CONTENT_MAX_COLS: u16 = 30;

    f.render_widget(
        Block::default().style(Style::default().bg(Color::Black)),
        area,
    );

    let footer_h = FOOTER_ROWS.min(area.height);
    let footer_y = area.y + area.height.saturating_sub(footer_h);
    let header_h = HEADER_ROWS.min(footer_y.saturating_sub(area.y));
    let header_area = Rect::new(area.x, area.y, area.width, header_h);
    let items_bottom = footer_y;
    let items_area = Rect::new(
        area.x,
        area.y + header_h,
        area.width,
        items_bottom.saturating_sub(area.y + header_h),
    );
    let footer_area = Rect::new(area.x, footer_y, area.width, footer_h);
    let content_w = area.width.min(CONTENT_MAX_COLS).max(1);
    let content_x = area.x + area.width.saturating_sub(content_w) / 2;

    // --- Header ---
    if header_area.height > 0 {
        let title_style = Style::default()
            .fg(Color::White)
            .bg(Color::Black)
            .add_modifier(Modifier::BOLD);
        let title_text = if frame.crash_pending {
            alloc::format!("{} !", frame.title)
        } else {
            frame.title.to_string()
        };
        let title_line = Line::from(Span::styled(title_text, title_style)).centered();
        let header = Paragraph::new(title_line)
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(Color::Gray)),
            )
            .alignment(Alignment::Center);
        f.render_widget(header, header_area);
    }

    // --- Items ---
    if frame.selected >= frame.items.len() {
        draw_info_lines(f, frame, items_area, content_x, content_w);
    } else {
        let rendered_items = frame.items.len() as u16;
        let item_block_h = rendered_items
            .saturating_mul(ITEM_ROWS)
            .min(items_area.height);
        let mut item_y = items_area.y + items_area.height.saturating_sub(item_block_h) / 2;

        for (i, &item_text) in frame.items.iter().enumerate() {
            if item_y >= items_bottom {
                break;
            }

            let item_h = ITEM_ROWS.min(items_bottom - item_y);
            let item_area = Rect::new(content_x, item_y, content_w, item_h);
            item_y = item_y.saturating_add(ITEM_ROWS);

            let selected = i == frame.selected;
            let enabled = frame.enabled.get(i).copied().unwrap_or(true);
            let marked = frame.marked == Some(i);

            let row_bg = if selected { Color::Gray } else { Color::Black };
            // Disabled items use C2 (Color::Gray) so they stay visible on the
            // C3 (Color::Black) unselected background — matching the old renderer.
            let text_fg = if !enabled {
                Color::Gray
            } else if selected {
                Color::White
            } else {
                Color::Green
            };

            if selected {
                f.render_widget(
                    Block::default().style(Style::default().bg(row_bg)),
                    item_area,
                );
            }

            let text_row_y = item_area.y + item_area.height / 2;
            let line_area = Rect::new(item_area.x, text_row_y, item_area.width, 1);
            let marker_w = if marked { 2u16 } else { 0u16 };
            let inner = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(2),
                    Constraint::Min(1),
                    Constraint::Length(marker_w),
                ])
                .split(line_area);

            let cursor_area = inner[0];
            let text_area = inner[1];
            let marker_area = inner[2];

            let cursor_sym = if selected { ">" } else { " " };
            let cursor_span = Span::styled(cursor_sym, Style::default().fg(text_fg).bg(row_bg));
            f.render_widget(Paragraph::new(Line::from(cursor_span)), cursor_area);

            let text_style = Style::default().fg(text_fg).bg(row_bg);
            let marquee_cols = text_area.width as usize;
            let needs_scroll = selected && item_text.len() > marquee_cols;
            if needs_scroll {
                let offset = marquee_char_offset(item_text, frame.marquee_frame, marquee_cols);
                let marquee = MarqueeLine::new(item_text, offset, marquee_cols, text_style);
                f.render_widget(marquee, text_area);
            } else {
                let visible_cols = text_area.width as usize;
                let visible = truncate_to_cols(item_text, visible_cols);
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(visible, text_style))),
                    text_area,
                );
            }

            if marked {
                let marker_span = Span::styled("*", Style::default().fg(Color::White).bg(row_bg));
                f.render_widget(Paragraph::new(Line::from(marker_span)), marker_area);
            }
        }
    }

    // --- Footer ---
    if footer_area.height > 0 {
        let footer_style = Style::default().fg(Color::Green).bg(Color::Black);
        let footer_text = if frame.selected >= frame.items.len() {
            "B:BACK"
        } else {
            "A:SELECT  B:BACK"
        };
        let footer_line = Line::from(Span::styled(footer_text, footer_style)).centered();
        let footer = Paragraph::new(footer_line)
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(Color::Gray)),
            )
            .alignment(Alignment::Center);
        f.render_widget(footer, footer_area);
    }
}

#[cfg(any(target_arch = "arm", test, feature = "std"))]
fn draw_info_lines(
    f: &mut Frame,
    frame: &MenuFrame<'_>,
    items_area: Rect,
    content_x: u16,
    content_w: u16,
) {
    let line_count = frame.items.len() as u16;
    if line_count == 0 || items_area.height == 0 {
        return;
    }

    let block_h = line_count.min(items_area.height);
    let first_y = items_area.y + items_area.height.saturating_sub(block_h) / 2;
    let style = Style::default().fg(Color::Green).bg(Color::Black);

    for (i, &line) in frame.items.iter().enumerate() {
        let y = first_y + i as u16;
        if y >= items_area.y + items_area.height {
            break;
        }
        let visible = truncate_to_cols(line, content_w as usize);
        let area = Rect::new(content_x, y, content_w, 1);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(visible, style))).alignment(Alignment::Center),
            area,
        );
    }
}

#[cfg(any(target_arch = "arm", test, feature = "std"))]
fn truncate_to_cols(text: &str, cols: usize) -> &str {
    if text.len() <= cols {
        return text;
    }
    match text.char_indices().nth(cols) {
        Some((idx, _)) => &text[..idx],
        None => text,
    }
}

// ---------------------------------------------------------------------------
// Loading render function (works on both ARM and host)
// ---------------------------------------------------------------------------

#[cfg(any(target_arch = "arm", test, feature = "std"))]
pub fn draw_loading(f: &mut Frame, frame: &LoadingFrame<'_>) {
    let area = f.area();

    // Clear background
    let bg = Block::default().style(Style::default().bg(Color::Black));
    f.render_widget(bg, area);

    // Split: header / spacer / filename / spacer / progress
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(1),
        ])
        .split(area);

    let header_area = chunks[0];
    let filename_area = chunks[2];
    let gauge_area = chunks[4];

    // --- Header ---
    let title_style = Style::default()
        .fg(Color::White)
        .bg(Color::Black)
        .add_modifier(Modifier::BOLD);
    let header = Paragraph::new(Line::from(Span::styled("LOADING", title_style)).centered()).block(
        Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(Color::Gray)),
    );
    f.render_widget(header, header_area);

    // --- Filename ---
    let filename_style = Style::default().fg(Color::Green).bg(Color::Black);
    let filename = frame.filename;
    let filename_cols = filename_area.width as usize;
    let visible_name: &str = if filename.len() > filename_cols {
        &filename[..filename_cols]
    } else {
        filename
    };
    let filename_widget =
        Paragraph::new(Line::from(Span::styled(visible_name, filename_style)).centered())
            .block(Block::default());
    f.render_widget(filename_widget, filename_area);

    // --- Progress gauge ---
    let pct = if frame.progress.total_banks == 0 {
        0u16
    } else {
        ((frame.progress.banks_done as u64 * 100) / frame.progress.total_banks as u64).min(100)
            as u16
    };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(Color::White).bg(Color::Gray))
        .percent(pct)
        .label(Span::styled(
            alloc::format!("{pct}%"),
            Style::default().fg(Color::Black),
        ));
    f.render_widget(gauge, gauge_area);
}

// ---------------------------------------------------------------------------
// Tests (host-only via ratatui TestBackend)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::loading::LoadingProgress;
    use crate::menu::MenuFrame;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn make_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(width, height)).unwrap()
    }

    fn make_frame<'a>(
        title: &'a str,
        items: &'a [&'a str],
        selected: usize,
        enabled: &'a [bool],
        marked: Option<usize>,
        crash_pending: bool,
        marquee_frame: u32,
    ) -> MenuFrame<'a> {
        MenuFrame {
            title,
            items,
            selected,
            enabled,
            marked,
            crash_pending,
            marquee_frame,
        }
    }

    fn buf_has_symbol(buf: &ratatui::prelude::Buffer, sym: &str) -> bool {
        (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| {
                buf.cell(ratatui::layout::Position { x, y })
                    .map_or(false, |c| c.symbol() == sym)
            })
        })
    }

    /// True if any cell renders `sym` with foreground `fg`.
    fn buf_has_symbol_with_fg(buf: &ratatui::prelude::Buffer, sym: &str, fg: Color) -> bool {
        (0..buf.area.height).any(|y| {
            (0..buf.area.width).any(|x| {
                buf.cell(ratatui::layout::Position { x, y })
                    .map_or(false, |c| c.symbol() == sym && c.fg == fg)
            })
        })
    }

    fn buf_to_text(buf: &ratatui::prelude::Buffer) -> alloc::string::String {
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| {
                        buf.cell(ratatui::layout::Position { x, y })
                            .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
                    })
                    .collect::<alloc::string::String>()
            })
            .collect::<alloc::vec::Vec<_>>()
            .join("\n")
    }

    fn row_text(buf: &ratatui::prelude::Buffer, y: u16) -> alloc::string::String {
        (0..buf.area.width)
            .map(|x| {
                buf.cell(ratatui::layout::Position { x, y })
                    .map_or(' ', |c| c.symbol().chars().next().unwrap_or(' '))
            })
            .collect()
    }

    fn row_has_bg(buf: &ratatui::prelude::Buffer, y: u16, bg: Color) -> bool {
        (0..buf.area.width).any(|x| {
            buf.cell(ratatui::layout::Position { x, y })
                .map_or(false, |c| c.bg == bg)
        })
    }

    #[test]
    fn cursor_symbol_present_on_selected_item() {
        let items = ["RESUME", "SAVE", "LOAD"];
        let enabled = [true, true, true];
        let menu_frame = make_frame("PAUSED", &items, 0, &enabled, None, false, 0);

        let mut terminal = make_terminal(40, 20);
        terminal.draw(|f| draw_menu(f, &menu_frame)).unwrap();
        let buf = terminal.backend().buffer().clone();
        assert!(
            buf_has_symbol(&buf, ">"),
            "cursor '>' should appear for selected item"
        );
    }

    #[test]
    fn disabled_item_uses_visible_muted_fg() {
        let items = ["RESUME", "LOAD"];
        let enabled = [true, false]; // LOAD is disabled, unselected
        let menu_frame = make_frame("PAUSED", &items, 0, &enabled, None, false, 0);

        let mut terminal = make_terminal(40, 20);
        terminal.draw(|f| draw_menu(f, &menu_frame)).unwrap();
        let buf = terminal.backend().buffer().clone();
        // The disabled, unselected LOAD item must use Gray (C2), NOT DarkGray
        // (C3) — DarkGray maps to the same colour as the unselected row
        // background and would be invisible.  Check the 'L' glyph specifically
        // (the footer's 'L' in "SELECT" is Green, so this is unambiguous).
        assert!(
            buf_has_symbol_with_fg(&buf, "L", Color::Gray),
            "disabled LOAD item should render its text with the visible Gray fg"
        );
    }

    #[test]
    fn marker_star_present_for_marked_item() {
        let items = ["ROM.GB"];
        let enabled = [true];
        let menu_frame = make_frame("ROMS", &items, 0, &enabled, Some(0), false, 0);

        let mut terminal = make_terminal(40, 20);
        terminal.draw(|f| draw_menu(f, &menu_frame)).unwrap();
        let buf = terminal.backend().buffer().clone();
        assert!(
            buf_has_symbol(&buf, "*"),
            "marked item should show '*' marker"
        );
    }

    #[test]
    fn crash_badge_shows_exclamation_in_title() {
        let items = ["ROMS"];
        let enabled = [true];
        let menu_frame = make_frame("MAIN", &items, 0, &enabled, None, true, 0);

        let mut terminal = make_terminal(40, 20);
        terminal.draw(|f| draw_menu(f, &menu_frame)).unwrap();
        let buf = terminal.backend().buffer().clone();
        assert!(
            buf_has_symbol(&buf, "!"),
            "crash_pending should show '!' in header"
        );
    }

    #[test]
    fn footer_contains_help_text() {
        let items = ["ROM"];
        let enabled = [true];
        let menu_frame = make_frame("TEST", &items, 0, &enabled, None, false, 0);

        let mut terminal = make_terminal(40, 20);
        terminal.draw(|f| draw_menu(f, &menu_frame)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text = buf_to_text(&buf);
        assert!(
            text.contains("A:SELECT"),
            "footer should contain 'A:SELECT', got:\n{text}"
        );
    }

    #[test]
    fn short_menus_use_compact_fixed_item_rows() {
        let items = ["ROMS", "SETTINGS"];
        let enabled = [true, true];
        let menu_frame = make_frame("MAIN", &items, 0, &enabled, None, false, 0);

        let mut terminal = make_terminal(30, 24);
        terminal.draw(|f| draw_menu(f, &menu_frame)).unwrap();
        let buf = terminal.backend().buffer().clone();

        assert!(row_text(&buf, 11).contains("ROMS"));
        assert!(row_text(&buf, 13).contains("SETTINGS"));
        for y in 3..10 {
            let row = row_text(&buf, y);
            assert!(
                !row.contains("ROMS") && !row.contains("SETTINGS"),
                "item text leaked into top spacer row {y}: {row:?}"
            );
        }
        for y in 15..21 {
            let row = row_text(&buf, y);
            assert!(
                !row.contains("ROMS") && !row.contains("SETTINGS"),
                "item text leaked into bottom spacer row {y}: {row:?}"
            );
        }
    }

    #[test]
    fn selected_last_rom_row_does_not_absorb_footer_spacer() {
        let items = ["A", "B", "C", "D", "E", "F", "G"];
        let enabled = [true; 7];
        let menu_frame = make_frame("ROMS", &items, 6, &enabled, None, false, 0);

        let mut terminal = make_terminal(30, 24);
        terminal.draw(|f| draw_menu(f, &menu_frame)).unwrap();
        let buf = terminal.backend().buffer().clone();

        assert!(row_has_bg(&buf, 17, Color::Gray));
        assert!(row_has_bg(&buf, 18, Color::Gray));
        assert!(
            !row_has_bg(&buf, 19, Color::Gray),
            "selected last item should stop after its fixed 2-row band"
        );
    }

    #[test]
    fn selected_long_item_uses_centered_row_width() {
        let long_name = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.GB";
        let items = [long_name];
        let enabled = [true];
        let menu_frame = make_frame("ROMS", &items, 0, &enabled, None, false, 0);

        let mut terminal = make_terminal(30, 24);
        terminal.draw(|f| draw_menu(f, &menu_frame)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text = buf_to_text(&buf);

        assert!(
            text.contains("ABCDEFGHIJKLMNOPQRSTUVWXYZ01"),
            "selected row should use the full 28-column text window, got:\n{text}"
        );
    }

    #[test]
    fn marquee_offset_changes_render() {
        let long_name = "AVERYLONGROMNAMETHATSHOULDSCROLLFORALONGTIME.GB";
        let items = [long_name];
        let enabled = [true];
        let frame0 = make_frame("ROMS", &items, 0, &enabled, None, false, 0);
        let frame_later = make_frame(
            "ROMS",
            &items,
            0,
            &enabled,
            None,
            false,
            MARQUEE_HOLD_FRAMES + 5,
        );

        let mut terminal0 = make_terminal(40, 20);
        terminal0.draw(|f| draw_menu(f, &frame0)).unwrap();
        let buf0 = terminal0.backend().buffer().clone();

        let mut terminal1 = make_terminal(40, 20);
        terminal1.draw(|f| draw_menu(f, &frame_later)).unwrap();
        let buf1 = terminal1.backend().buffer().clone();

        let text0 = buf_to_text(&buf0);
        let text1 = buf_to_text(&buf1);
        assert!(
            text0 != text1,
            "marquee should produce different output at different frames"
        );
    }

    #[test]
    fn loading_screen_shows_percentage_or_loading_text() {
        let progress = LoadingProgress::new(1, 4); // 25%
        let frame = LoadingFrame::new("ROM.GB", progress, 0);

        let mut terminal = make_terminal(40, 20);
        terminal.draw(|f| draw_loading(f, &frame)).unwrap();
        let buf = terminal.backend().buffer().clone();
        let text = buf_to_text(&buf);
        assert!(
            text.contains("25%") || text.contains("LOADING"),
            "loading screen should show percentage or LOADING, got:\n{text}"
        );
    }
}
