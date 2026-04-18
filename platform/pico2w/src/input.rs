//! 8-button GPIO input handler with software debounce and a Start+Select
//! hold-combo detector.
//!
//! All buttons are wired active-low (one side to GPIO, other to GND). Internal
//! pull-ups are enabled so no external resistors are needed.
//!
//! Pin assignments are an application-level concern; see `main.rs`.

#[cfg(target_arch = "arm")]
use embassy_rp::gpio::{Input, Pull};
#[cfg(target_arch = "arm")]
use embassy_rp::peripherals::{PIN_0, PIN_1, PIN_2, PIN_3, PIN_21, PIN_22, PIN_26, PIN_27};
#[cfg(target_arch = "arm")]
use embassy_rp::Peri;
#[cfg(target_arch = "arm")]
use embassy_time::Instant;

use rustyboy_core::cpu::peripheral::joypad::Button;

/// Debounce window in milliseconds.
const DEBOUNCE_MS: u64 = 10;
/// How long Start+Select must be held together to trigger the menu combo.
const MENU_HOLD_MS: u64 = 1_000;

// ---------------------------------------------------------------------------
// ButtonState
// ---------------------------------------------------------------------------

/// Snapshot of all 8 button states (`true` = pressed).
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct ButtonState {
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
    pub a: bool,
    pub b: bool,
    pub start: bool,
    pub select: bool,
}

impl ButtonState {
    /// Iterate over buttons whose state changed between `self` (previous) and
    /// `other` (current), yielding `(Button, pressed)` pairs.
    pub fn diff(self, other: ButtonState) -> impl Iterator<Item = (Button, bool)> {
        let pairs: [(bool, bool, Button); 8] = [
            (self.up,     other.up,     Button::Up),
            (self.down,   other.down,   Button::Down),
            (self.left,   other.left,   Button::Left),
            (self.right,  other.right,  Button::Right),
            (self.a,      other.a,      Button::A),
            (self.b,      other.b,      Button::B),
            (self.start,  other.start,  Button::Start),
            (self.select, other.select, Button::Select),
        ];
        pairs.into_iter().filter_map(|(prev, next, btn)| {
            if prev != next { Some((btn, next)) } else { None }
        })
    }
}

// ---------------------------------------------------------------------------
// Bitmask helpers
// ---------------------------------------------------------------------------

/// Encode `ButtonState` as a bitmask (bit 0 = up … bit 7 = select).
fn state_to_bits(s: ButtonState) -> u8 {
      (s.up     as u8)
    | (s.down   as u8) << 1
    | (s.left   as u8) << 2
    | (s.right  as u8) << 3
    | (s.a      as u8) << 4
    | (s.b      as u8) << 5
    | (s.start  as u8) << 6
    | (s.select as u8) << 7
}

fn bits_to_state(b: u8) -> ButtonState {
    ButtonState {
        up:     b & (1 << 0) != 0,
        down:   b & (1 << 1) != 0,
        left:   b & (1 << 2) != 0,
        right:  b & (1 << 3) != 0,
        a:      b & (1 << 4) != 0,
        b:      b & (1 << 5) != 0,
        start:  b & (1 << 6) != 0,
        select: b & (1 << 7) != 0,
    }
}

// ---------------------------------------------------------------------------
// Debounce + combo state machine (clock-injectable)
// ---------------------------------------------------------------------------

/// Internal state for the debounce and menu-combo logic.
///
/// Separated from GPIO reads so it can be driven with a synthetic clock in
/// unit tests via [`InputState::update`].
pub struct InputState {
    raw:          ButtonState,
    debounced:    ButtonState,
    // When each button's raw state last changed (ms timestamp; None = stable)
    raw_changed:  [Option<u64>; 8],
    // Menu combo tracking
    combo_since:  Option<u64>,
    combo_fired:  bool,
}

impl Default for InputState {
    fn default() -> Self {
        Self {
            raw:         ButtonState::default(),
            debounced:   ButtonState::default(),
            raw_changed: [None; 8],
            combo_since: None,
            combo_fired: false,
        }
    }
}

impl InputState {
    /// Process a new raw sample at time `now_ms`.
    ///
    /// Returns the debounced state and whether the Start+Select combo fired.
    pub fn update(&mut self, new_raw: ButtonState, now_ms: u64) -> (ButtonState, bool) {
        self.apply_debounce(new_raw, now_ms);
        let menu = self.check_combo(now_ms);
        (self.debounced, menu)
    }

    fn apply_debounce(&mut self, new_raw: ButtonState, now_ms: u64) {
        let old_bits = state_to_bits(self.raw);
        let new_bits = state_to_bits(new_raw);
        let mut deb_bits = state_to_bits(self.debounced);

        for i in 0..8u8 {
            let mask = 1u8 << i;
            let old_bit = (old_bits & mask) != 0;
            let new_bit = (new_bits & mask) != 0;

            if new_bit != old_bit {
                self.raw_changed[i as usize] = Some(now_ms);
            } else if let Some(changed_at) = self.raw_changed[i as usize] {
                if now_ms.saturating_sub(changed_at) >= DEBOUNCE_MS {
                    if new_bit { deb_bits |= mask; } else { deb_bits &= !mask; }
                    self.raw_changed[i as usize] = None;
                }
            }
        }

        self.raw = new_raw;
        self.debounced = bits_to_state(deb_bits);
    }

    fn check_combo(&mut self, now_ms: u64) -> bool {
        let both = self.debounced.start && self.debounced.select;
        if both {
            match self.combo_since {
                None => {
                    self.combo_since = Some(now_ms);
                    false
                }
                Some(since) => {
                    if !self.combo_fired
                        && now_ms.saturating_sub(since) >= MENU_HOLD_MS
                    {
                        self.combo_fired = true;
                        true
                    } else {
                        false
                    }
                }
            }
        } else {
            self.combo_since = None;
            self.combo_fired = false;
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Hardware InputHandler (embassy GPIO) — embedded target only
// ---------------------------------------------------------------------------

#[cfg(target_arch = "arm")]
pub struct InputHandler<'d> {
    up:     Input<'d>,
    down:   Input<'d>,
    left:   Input<'d>,
    right:  Input<'d>,
    a:      Input<'d>,
    b:      Input<'d>,
    start:  Input<'d>,
    select: Input<'d>,
    state:  InputState,
}

#[cfg(target_arch = "arm")]
impl<'d> InputHandler<'d> {
    pub fn new(
        up_pin:     Peri<'d, PIN_21>,
        down_pin:   Peri<'d, PIN_22>,
        left_pin:   Peri<'d, PIN_26>,
        right_pin:  Peri<'d, PIN_27>,
        a_pin:      Peri<'d, PIN_0>,
        b_pin:      Peri<'d, PIN_1>,
        start_pin:  Peri<'d, PIN_2>,
        select_pin: Peri<'d, PIN_3>,
    ) -> Self {
        Self {
            up:     Input::new(up_pin,     Pull::Up),
            down:   Input::new(down_pin,   Pull::Up),
            left:   Input::new(left_pin,   Pull::Up),
            right:  Input::new(right_pin,  Pull::Up),
            a:      Input::new(a_pin,      Pull::Up),
            b:      Input::new(b_pin,      Pull::Up),
            start:  Input::new(start_pin,  Pull::Up),
            select: Input::new(select_pin, Pull::Up),
            state:  InputState::default(),
        }
    }

    /// Sample all GPIO pins, apply debounce, and check the menu combo.
    ///
    /// Call once per game-loop tick (~60 Hz).
    pub fn poll(&mut self) -> (ButtonState, bool) {
        let raw = self.read_raw();
        let now_ms = Instant::now().as_millis();
        self.state.update(raw, now_ms)
    }

    fn read_raw(&self) -> ButtonState {
        ButtonState {
            up:     self.up.is_low(),
            down:   self.down.is_low(),
            left:   self.left.is_low(),
            right:  self.right.is_low(),
            a:      self.a.is_low(),
            b:      self.b.is_low(),
            start:  self.start.is_low(),
            select: self.select.is_low(),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- ButtonState::diff --------------------------------------------------

    #[test]
    fn diff_no_change() {
        let s = ButtonState { up: true, ..Default::default() };
        assert_eq!(s.diff(s).count(), 0);
    }

    #[test]
    fn diff_single_press() {
        let prev = ButtonState::default();
        let curr = ButtonState { a: true, ..Default::default() };
        let changes: Vec<_> = prev.diff(curr).collect();
        assert_eq!(changes, vec![(Button::A, true)]);
    }

    #[test]
    fn diff_single_release() {
        let prev = ButtonState { b: true, ..Default::default() };
        let curr = ButtonState::default();
        let changes: Vec<_> = prev.diff(curr).collect();
        assert_eq!(changes, vec![(Button::B, false)]);
    }

    #[test]
    fn diff_simultaneous_changes() {
        let prev = ButtonState { up: true, down: false, ..Default::default() };
        let curr = ButtonState { up: false, down: true, ..Default::default() };
        let changes: Vec<_> = prev.diff(curr).collect();
        assert_eq!(changes.len(), 2);
        assert!(changes.contains(&(Button::Up, false)));
        assert!(changes.contains(&(Button::Down, true)));
    }

    // --- Bitmask round-trip -------------------------------------------------

    #[test]
    fn bits_round_trip_all_zeros() {
        let s = ButtonState::default();
        assert_eq!(bits_to_state(state_to_bits(s)), s);
    }

    #[test]
    fn bits_round_trip_all_ones() {
        let s = ButtonState {
            up: true, down: true, left: true, right: true,
            a: true, b: true, start: true, select: true,
        };
        assert_eq!(state_to_bits(s), 0xFF);
        assert_eq!(bits_to_state(state_to_bits(s)), s);
    }

    #[test]
    fn bits_round_trip_arbitrary() {
        let s = ButtonState { up: true, a: true, select: true, ..Default::default() };
        assert_eq!(bits_to_state(state_to_bits(s)), s);
    }

    // --- Debounce -----------------------------------------------------------

    #[test]
    fn debounce_rejects_before_window() {
        let mut state = InputState::default();
        let pressed = ButtonState { a: true, ..Default::default() };

        // Raw changes at t=0, stable at t=5 — not yet past 10 ms window
        state.update(pressed, 0);
        let (deb, _) = state.update(pressed, 5);
        assert!(!deb.a, "should not be debounced yet at 5 ms");
    }

    #[test]
    fn debounce_accepts_after_window() {
        let mut state = InputState::default();
        let pressed = ButtonState { a: true, ..Default::default() };

        state.update(pressed, 0);
        let (deb, _) = state.update(pressed, 10);
        assert!(deb.a, "should be debounced after 10 ms");
    }

    #[test]
    fn debounce_resets_on_bounce() {
        let mut state = InputState::default();
        let pressed  = ButtonState { a: true,  ..Default::default() };
        let released = ButtonState { a: false, ..Default::default() };

        state.update(pressed, 0);  // raw goes high
        state.update(released, 5); // bounces low before window — resets timer
        let (deb, _) = state.update(pressed, 12); // stable again but window not elapsed from t=5
        assert!(!deb.a, "bounce before window should reset debounce");
    }

    // --- Menu combo ---------------------------------------------------------

    #[test]
    fn combo_fires_after_hold() {
        let mut state = InputState::default();
        let both = ButtonState { start: true, select: true, ..Default::default() };

        // Debounce both buttons first
        state.update(both, 0);
        state.update(both, 10); // debounced

        // Hold for just under 1 s — should not fire
        let (_, fired) = state.update(both, 1_009);
        assert!(!fired, "combo should not fire before 1000 ms");

        // Hold past 1 s — should fire
        let (_, fired) = state.update(both, 1_010);
        assert!(fired, "combo should fire after 1000 ms hold");
    }

    #[test]
    fn combo_fires_only_once() {
        let mut state = InputState::default();
        let both = ButtonState { start: true, select: true, ..Default::default() };

        state.update(both, 0);
        state.update(both, 10);
        state.update(both, 1_010); // fires

        let (_, second) = state.update(both, 1_100);
        assert!(!second, "combo must not fire twice while held");
    }

    #[test]
    fn combo_resets_after_release() {
        let mut state = InputState::default();
        let both     = ButtonState { start: true, select: true, ..Default::default() };
        let released = ButtonState::default();

        state.update(both, 0);
        state.update(both, 10);
        state.update(both, 1_010); // fires

        // Release
        state.update(released, 1_100);
        state.update(released, 1_110); // debounced release

        // Hold again
        state.update(both, 1_200);
        state.update(both, 1_210);
        let (_, refired) = state.update(both, 2_210);
        assert!(refired, "combo should re-arm after full release");
    }
}
