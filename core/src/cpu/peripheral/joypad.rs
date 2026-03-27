/// Joypad peripheral — Game Boy P1/JOYP register (0xFF00).
///
/// The register is split into two groups selected by writing bits 4–5:
///   Bit 5 = 0: select direction pad (Down, Up, Left, Right)
///   Bit 4 = 0: select action buttons (Start, Select, B, A)
///
/// Bits 0–3 of the register read back the state of the selected group.
/// A bit is 0 when the button is pressed, 1 when released (active-low).
///
/// Writing both select bits low returns the OR of both groups.
/// Writing both high returns 0x0F (no buttons readable).
///
/// Pan Docs reference: <https://gbdev.io/pandocs/Joypad_Input.html>

/// The 8 Game Boy buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Button {
    /// D-pad right.
    Right,
    /// D-pad left.
    Left,
    /// D-pad up.
    Up,
    /// D-pad down.
    Down,
    /// Action button A.
    A,
    /// Action button B.
    B,
    /// Select button.
    Select,
    /// Start button.
    Start,
}

/// Interrupt bit for joypad in the IF register (bit 4).
pub(crate) const JOYPAD_INTERRUPT_BIT: u8 = 4;
/// JOYP register address.
pub(crate) const JOYP_ADDR: u16 = 0xFF00;

/// Joypad state and register logic.
pub struct JoypadPeripheral {
    /// Pressed state for the direction group (bits: Down=3, Up=2, Left=1, Right=0).
    directions: u8,
    /// Pressed state for the action group (bits: Start=3, Select=2, B=1, A=0).
    actions: u8,
    /// Last value written to JOYP (select lines in bits 4–5).
    select: u8,
}

impl JoypadPeripheral {
    pub fn new() -> Self {
        Self {
            directions: 0,
            actions: 0,
            select: 0x30, // both select bits high = nothing selected
        }
    }

    /// Press or release a button. Returns `true` if this press triggers the
    /// joypad interrupt (a button was just pressed and its group is selected).
    pub fn set_button(&mut self, button: Button, pressed: bool) -> bool {
        let (group, bit) = button_to_group_bit(button);
        let group_state = match group {
            Group::Directions => &mut self.directions,
            Group::Actions => &mut self.actions,
        };
        let was_pressed = *group_state & (1 << bit) != 0;
        if pressed {
            *group_state |= 1 << bit;
        } else {
            *group_state &= !(1 << bit);
        }
        // Interrupt fires on the falling edge of any input line (button down).
        let newly_pressed = pressed && !was_pressed;
        if !newly_pressed {
            return false;
        }
        // Select lines are active-low: bit = 0 means that group is selected.
        match group {
            Group::Directions => self.select & 0x10 == 0,
            Group::Actions    => self.select & 0x20 == 0,
        }
    }

    /// Handle a write to JOYP — update the stored select bits.
    pub fn write(&mut self, value: u8) {
        self.select = value & 0x30;
    }

    /// Build the current JOYP register value from select lines and button state.
    ///
    /// Bits 7–6: always 1 (unused).
    /// Bits 5–4: select lines (as written).
    /// Bits 3–0: active-low pressed bits for the selected group(s).
    pub fn read(&self) -> u8 {
        let dir_sel = self.select & 0x10 == 0;
        let act_sel = self.select & 0x20 == 0;

        let mut low = 0x0Fu8;
        if dir_sel {
            low &= !self.directions & 0x0F;
        }
        if act_sel {
            low &= !self.actions & 0x0F;
        }
        0xC0 | self.select | low
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

enum Group {
    Directions,
    Actions,
}

fn button_to_group_bit(button: Button) -> (Group, u8) {
    match button {
        Button::Right  => (Group::Directions, 0),
        Button::Left   => (Group::Directions, 1),
        Button::Up     => (Group::Directions, 2),
        Button::Down   => (Group::Directions, 3),
        Button::A      => (Group::Actions, 0),
        Button::B      => (Group::Actions, 1),
        Button::Select => (Group::Actions, 2),
        Button::Start  => (Group::Actions, 3),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Bit 4 = 0 → directions selected; bit 5 = 0 → actions selected (active-low).
    // To select directions only: write 0x20 (bit 5 high, bit 4 low).
    // To select actions only:    write 0x10 (bit 4 high, bit 5 low).

    #[test]
    fn initial_read_returns_no_buttons_pressed() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x20); // bit 4 = 0 → directions selected
        assert_eq!(joy.read() & 0x0F, 0x0F); // all released
    }

    #[test]
    fn press_right_reflected_in_directions_group() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x20); // directions selected (bit 4 = 0)
        joy.set_button(Button::Right, true);
        assert_eq!(joy.read() & 0x01, 0); // bit 0 low = pressed
    }

    #[test]
    fn release_clears_bit() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x20);
        joy.set_button(Button::Right, true);
        joy.set_button(Button::Right, false);
        assert_eq!(joy.read() & 0x01, 1); // bit 0 high = released
    }

    #[test]
    fn action_buttons_not_visible_when_directions_selected() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x20); // directions only (bit 5 = 1, bit 4 = 0)
        joy.set_button(Button::A, true);
        assert_eq!(joy.read() & 0x0F, 0x0F); // A not visible
    }

    #[test]
    fn action_buttons_visible_when_actions_selected() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x10); // actions selected (bit 5 = 0, bit 4 = 1)
        joy.set_button(Button::A, true);
        assert_eq!(joy.read() & 0x01, 0); // bit 0 low = A pressed
    }

    #[test]
    fn both_groups_visible_when_both_selected() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x00); // both selected (bits 4 and 5 = 0)
        joy.set_button(Button::Right, true); // directions bit 0
        joy.set_button(Button::A, true);     // actions bit 0 — same physical bit
        assert_eq!(joy.read() & 0x01, 0); // bit 0 low (either group pressed)
    }

    #[test]
    fn no_group_selected_returns_0f_low_nibble() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x30); // neither selected (both bits high)
        joy.set_button(Button::A, true);
        joy.set_button(Button::Right, true);
        assert_eq!(joy.read() & 0x0F, 0x0F); // no group readable
    }

    #[test]
    fn select_lines_preserved_in_read() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x10);
        assert_eq!(joy.read() & 0x30, 0x10);
    }

    #[test]
    fn interrupt_fires_on_press_when_group_selected() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x20); // directions selected (bit 4 = 0)
        assert!(joy.set_button(Button::Down, true));
    }

    #[test]
    fn interrupt_does_not_fire_on_release() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x20);
        joy.set_button(Button::Down, true);
        assert!(!joy.set_button(Button::Down, false));
    }

    #[test]
    fn interrupt_does_not_fire_when_group_not_selected() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x10); // actions selected, not directions
        assert!(!joy.set_button(Button::Down, true));
    }

    #[test]
    fn interrupt_does_not_fire_on_repeated_press() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x20);
        joy.set_button(Button::Down, true);
        // Already pressed — should not fire again
        assert!(!joy.set_button(Button::Down, true));
    }

    #[test]
    fn all_direction_bits_independent() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x20); // directions selected
        joy.set_button(Button::Right, true);
        joy.set_button(Button::Up, true);
        let low = joy.read() & 0x0F;
        assert_eq!(low & 0x01, 0); // Right pressed
        assert_eq!(low & 0x02, 2); // Left released
        assert_eq!(low & 0x04, 0); // Up pressed
        assert_eq!(low & 0x08, 8); // Down released
    }

    #[test]
    fn all_action_bits_independent() {
        let mut joy = JoypadPeripheral::new();
        joy.write(0x10); // actions selected
        joy.set_button(Button::B, true);
        joy.set_button(Button::Start, true);
        let low = joy.read() & 0x0F;
        assert_eq!(low & 0x01, 1); // A released
        assert_eq!(low & 0x02, 0); // B pressed
        assert_eq!(low & 0x04, 4); // Select released
        assert_eq!(low & 0x08, 0); // Start pressed
    }
}
