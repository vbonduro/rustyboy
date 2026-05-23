use super::text::{
    fill_row, text_screen_width, write_text_row, write_truncated_text_row, TextRun, TextStyle,
    TextWindow, C0_BE, C1_BE, C2_BE, C3_BE, CHAR_W,
};
use super::{RenderWindow, SCREEN_W};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadingProgress {
    pub banks_done: u32,
    pub total_banks: u32,
}

impl LoadingProgress {
    pub const fn new(banks_done: u32, total_banks: u32) -> Self {
        Self {
            banks_done,
            total_banks,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoadingFrame<'a> {
    pub filename: &'a str,
    pub progress: LoadingProgress,
    pub marquee_frame: u32,
}

impl<'a> LoadingFrame<'a> {
    pub const fn new(filename: &'a str, progress: LoadingProgress, marquee_frame: u32) -> Self {
        Self {
            filename,
            progress,
            marquee_frame,
        }
    }
}

pub fn loading_bar_window() -> RenderWindow {
    RenderWindow::full_width_rows(LOADING_BAR_TOP, LOADING_BAR_BOTTOM)
}

pub fn render_loading_row(frame: &LoadingFrame<'_>, y: u16, row: &mut [u8; 480]) {
    let in_top_sep = y >= HEADER_H && y < HEADER_H + SEPARATOR_H;
    fill_row(row, if in_top_sep { C2_BE } else { C3_BE });

    if y < HEADER_H {
        render_loading_header_row(y, row);
    }
    if y >= LOADING_FILENAME_Y && y < LOADING_FILENAME_Y + 8 {
        render_loading_filename_row(frame.filename, y, row);
    }
    if y >= LOADING_BAR_TOP && y < LOADING_BAR_BOTTOM {
        render_loading_bar_row(frame.progress, row);
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
    write_text_row(
        row,
        title,
        TextRun::new(title_x, glyph_row, TextStyle::new(SCALE, C0_BE, C3_BE)),
    );
}

fn render_loading_filename_row(filename: &str, y: u16, row: &mut [u8; 480]) {
    let glyph_row = (y - LOADING_FILENAME_Y) as usize;
    let text = filename.as_bytes();
    let text_w = text_screen_width(text, 1);
    let style = TextStyle::new(1, C1_BE, C3_BE);
    let window = TextWindow::new(LOADING_FILENAME_X0, LOADING_FILENAME_X1, glyph_row, style);
    if text_w > window.width() {
        write_truncated_text_row(row, text, window);
    } else {
        let text_x = (SCREEN_W as usize).saturating_sub(text_w) / 2;
        write_text_row(row, text, TextRun::new(text_x, glyph_row, style));
    }
}

fn render_loading_bar_row(progress: LoadingProgress, row: &mut [u8; 480]) {
    let bar_w = LOADING_BAR_X1 - LOADING_BAR_X0;
    let filled = progress_bar_fill_width(progress, bar_w);
    for px in LOADING_BAR_X0..LOADING_BAR_X1 {
        row[px * 2] = C2_BE[0];
        row[px * 2 + 1] = C2_BE[1];
    }
    for px in LOADING_BAR_X0..LOADING_BAR_X0 + filled {
        row[px * 2] = C0_BE[0];
        row[px * 2 + 1] = C0_BE[1];
    }
}

fn progress_bar_fill_width(progress: LoadingProgress, bar_w: usize) -> usize {
    if progress.total_banks == 0 {
        return 0;
    }
    (bar_w as u64 * progress.banks_done as u64 / progress.total_banks as u64).min(bar_w as u64)
        as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(row: &[u8; 480], x: usize) -> [u8; 2] {
        [row[x * 2], row[x * 2 + 1]]
    }

    fn render_row(frame: &LoadingFrame<'_>, y: u16) -> [u8; 480] {
        let mut row = [0; 480];
        render_loading_row(frame, y, &mut row);
        row
    }

    #[test]
    fn loading_bar_window_covers_only_progress_rows() {
        assert_eq!(
            loading_bar_window(),
            RenderWindow::full_width_rows(LOADING_BAR_TOP, LOADING_BAR_BOTTOM)
        );
    }

    #[test]
    fn progress_width_handles_zero_total_and_caps_overflow() {
        assert_eq!(progress_bar_fill_width(LoadingProgress::new(1, 0), 200), 0);
        assert_eq!(progress_bar_fill_width(LoadingProgress::new(1, 4), 200), 50);
        assert_eq!(
            progress_bar_fill_width(LoadingProgress::new(5, 4), 200),
            200
        );
    }

    #[test]
    fn zero_total_loading_bar_renders_empty_progress() {
        let frame = LoadingFrame::new("ROM.GB", LoadingProgress::new(0, 0), 0);
        let row = render_row(&frame, LOADING_BAR_TOP);

        assert_eq!(pixel(&row, LOADING_BAR_X0), C2_BE);
        assert_eq!(pixel(&row, LOADING_BAR_X1 - 1), C2_BE);
    }

    #[test]
    fn loading_header_row_contains_title_pixels() {
        let frame = LoadingFrame::new("ROM.GB", LoadingProgress::new(0, 0), 0);
        let title_row = (HEADER_H - (8 * SCALE) as u16) / 2;
        let row = render_row(&frame, title_row);

        assert!(row.chunks_exact(2).any(|pixel| pixel == C0_BE));
    }

    #[test]
    fn short_loading_filename_is_centered() {
        let frame = LoadingFrame::new("A.GB", LoadingProgress::new(0, 0), 0);
        let row = render_row(&frame, LOADING_FILENAME_Y);
        let text_w = text_screen_width(b"A.GB", 1);
        let text_x = (SCREEN_W as usize).saturating_sub(text_w) / 2;

        assert!(row[..text_x * 2]
            .chunks_exact(2)
            .all(|pixel| pixel == C3_BE));
        assert!(row[text_x * 2..(text_x + text_w) * 2]
            .chunks_exact(2)
            .any(|pixel| pixel == C1_BE));
    }
}
