/// Milliseconds per column of horizontal scroll.
pub(super) const MARQUEE_MS: u32 = 200;
/// Columns held at offset 0 before scrolling begins (~1 s initial pause).
pub(super) const MARQUEE_PAUSE_COLS: usize = 5;

/// Tracks which menu item the marquee clock is anchored to and when that anchor
/// was set.  Resets automatically on selection change so the 1-second pause
/// restarts every time the user moves through the list.
pub(super) struct Marquee {
    sel: usize,
    epoch_ms: f64,
}

impl Marquee {
    pub(super) fn new() -> Self {
        Self {
            sel: usize::MAX,
            epoch_ms: 0.0,
        }
    }

    /// Call once per frame before `item_ms`.  Resets the epoch whenever the
    /// selected index changes so the 1-second pause fires on every navigation
    /// move.
    pub(super) fn on_selection(&mut self, idx: usize, now: f64) {
        if self.sel != idx {
            self.sel = idx;
            self.epoch_ms = now;
        }
    }

    /// Milliseconds elapsed since the current selection was anchored.
    pub(super) fn item_ms(&self, now: f64) -> f64 {
        now - self.epoch_ms
    }
}

/// Scroll `text` horizontally within `width_cols` columns.  Returns `text`
/// unchanged when it fits.  For longer strings, holds at offset 0 for
/// `MARQUEE_PAUSE_COLS * MARQUEE_MS` ms, then advances one column per
/// `MARQUEE_MS`.  The pause repeats at the start of each ring loop.
pub(super) fn marquee_window(text: &str, width_cols: usize, frame_ms: f64) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_returned_verbatim() {
        assert_eq!(marquee_window("HI", 5, 99_999.0), "HI");
        assert_eq!(marquee_window("HELLO", 5, 0.0), "HELLO");
    }

    #[test]
    fn pause_holds_at_offset_zero() {
        // During the pause window (pos < MARQUEE_PAUSE_COLS) the text is
        // anchored at offset 0.
        let text = "ABCDEFGH";
        let width = 4;
        // t=0 → pos=0 < 5 → offset 0 → "ABCD"
        assert_eq!(marquee_window(text, width, 0.0), "ABCD");
        // t=800 ms → pos=4 still within pause → "ABCD"
        let t_still_paused = ((MARQUEE_PAUSE_COLS - 1) * MARQUEE_MS as usize) as f64;
        assert_eq!(marquee_window(text, width, t_still_paused), "ABCD");
    }

    #[test]
    fn scroll_begins_after_pause() {
        // At pos = MARQUEE_PAUSE_COLS + 1 the offset advances to 1.
        let text = "ABCDEFGH";
        let width = 4;
        let t = ((MARQUEE_PAUSE_COLS + 1) * MARQUEE_MS as usize) as f64;
        assert_eq!(marquee_window(text, width, t), "BCDE");
    }

    #[test]
    fn marquee_resets_on_selection_change() {
        let mut m = Marquee::new();
        m.on_selection(0, 0.0);
        assert_eq!(m.item_ms(1000.0), 1000.0);
        // Change selection: epoch moves to 500 ms.
        m.on_selection(1, 500.0);
        assert_eq!(m.item_ms(1000.0), 500.0);
        // Same index again: epoch is NOT updated.
        m.on_selection(1, 999.0);
        assert_eq!(m.item_ms(1000.0), 500.0);
    }
}
