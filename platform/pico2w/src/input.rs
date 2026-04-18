//! Input handler — Bead 3
//!
//! Reads 8 tactile buttons over GPIO with software debounce and detects the
//! Start+Select menu combo.
//!
//! All buttons are wired active-low (one side to GPIO, other to GND). Internal
//! pull-ups are enabled so no external resistors are needed.
//!
//! # Pin assignment
//! | Button     | GPIO |
//! |------------|------|
//! | D-pad Up   | GP21 |
//! | D-pad Down | GP22 |
//! | D-pad Left | GP26 |
//! | D-pad Right| GP27 |
//! | A          | GP0  |
//! | B          | GP1  |
//! | Start      | GP2  |
//! | Select     | GP3  |

use embassy_rp::gpio::{Input, Pull};
use embassy_rp::peripherals::{PIN_0, PIN_1, PIN_2, PIN_3, PIN_21, PIN_22, PIN_26, PIN_27};
use embassy_rp::Peri;
use embassy_time::{Duration, Instant};

use rustyboy_core::cpu::peripheral::joypad::Button;

/// Debounce window — a raw state must be stable for this long before it is
/// accepted as a real change.
const DEBOUNCE: Duration = Duration::from_millis(10);

/// How long Start+Select must be held together to trigger the in-game menu.
const MENU_HOLD: Duration = Duration::from_millis(1_000);

// ---------------------------------------------------------------------------
// ButtonState
// ---------------------------------------------------------------------------

/// Snapshot of all 8 button states (true = pressed).
#[derive(Default, Clone, Copy, PartialEq, Eq)]
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
    /// Iterate over buttons whose state differs between `self` and `other`,
    /// yielding `(Button, pressed)` pairs.
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
// InputHandler
// ---------------------------------------------------------------------------

pub struct InputHandler<'d> {
    // GPIO inputs (pull-up, active-low)
    up:     Input<'d>,
    down:   Input<'d>,
    left:   Input<'d>,
    right:  Input<'d>,
    a:      Input<'d>,
    b:      Input<'d>,
    start:  Input<'d>,
    select: Input<'d>,

    // Debounce state: raw reading + when it last changed + debounced output
    raw:           ButtonState,
    debounced:     ButtonState,
    // Per-button: when the raw state last changed (None = stable since boot)
    raw_changed:   [Option<Instant>; 8],

    // Menu combo (Start + Select held for MENU_HOLD)
    combo_since:   Option<Instant>,
    combo_fired:   bool, // gate: require full release before re-triggering
}

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
            raw:           ButtonState::default(),
            debounced:     ButtonState::default(),
            raw_changed:   [None; 8],
            combo_since:   None,
            combo_fired:   false,
        }
    }

    /// Sample all buttons, apply debounce, and return the current debounced
    /// state plus whether the Start+Select menu combo fired this call.
    ///
    /// Call this once per game-loop tick (~60 Hz).
    pub fn poll(&mut self) -> (ButtonState, bool) {
        let now = Instant::now();

        // Read raw GPIO (active-low → pressed when pin is low)
        let new_raw = ButtonState {
            up:     self.up.is_low(),
            down:   self.down.is_low(),
            left:   self.left.is_low(),
            right:  self.right.is_low(),
            a:      self.a.is_low(),
            b:      self.b.is_low(),
            start:  self.start.is_low(),
            select: self.select.is_low(),
        };

        // Per-button debounce: record when raw state changes, accept after
        // DEBOUNCE of stability.
        let raw_bits = state_to_bits(self.raw);
        let new_bits = state_to_bits(new_raw);
        let deb_bits_old = state_to_bits(self.debounced);
        let mut deb_bits = deb_bits_old;

        for i in 0..8u8 {
            let mask = 1u8 << i;
            let old_raw_bit = (raw_bits & mask) != 0;
            let new_raw_bit = (new_bits & mask) != 0;

            if new_raw_bit != old_raw_bit {
                // Raw state just changed — start/reset the timer.
                self.raw_changed[i as usize] = Some(now);
            } else if let Some(changed_at) = self.raw_changed[i as usize] {
                // Raw state stable — accept once DEBOUNCE window has elapsed.
                if now.saturating_duration_since(changed_at) >= DEBOUNCE {
                    if new_raw_bit {
                        deb_bits |= mask;
                    } else {
                        deb_bits &= !mask;
                    }
                    self.raw_changed[i as usize] = None;
                }
            }
        }

        self.raw = new_raw;
        self.debounced = bits_to_state(deb_bits);

        // Menu combo: Start + Select held together for MENU_HOLD.
        let both = self.debounced.start && self.debounced.select;
        let menu_triggered = if both {
            match self.combo_since {
                None => {
                    self.combo_since = Some(now);
                    false
                }
                Some(since) => {
                    if !self.combo_fired
                        && now.saturating_duration_since(since) >= MENU_HOLD
                    {
                        self.combo_fired = true;
                        true
                    } else {
                        false
                    }
                }
            }
        } else {
            // Released — reset so combo can fire again next time.
            self.combo_since = None;
            self.combo_fired = false;
            false
        };

        (self.debounced, menu_triggered)
    }

    /// Return the last debounced state without re-sampling.
    pub fn state(&self) -> ButtonState {
        self.debounced
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Encode ButtonState as a bitmask (bit 0 = up … bit 7 = select).
fn state_to_bits(s: ButtonState) -> u8 {
    (s.up     as u8) << 0
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
