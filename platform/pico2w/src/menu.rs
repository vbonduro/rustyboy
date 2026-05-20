use crate::input::ButtonState;

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Derived from two consecutive ButtonState snapshots — no hardware dependency.
#[derive(Default, Clone, Copy)]
pub struct MenuInput {
    pub up: bool,
    pub down: bool,
    pub confirm: bool,
    pub back: bool,
}

impl MenuInput {
    pub fn from_diff(previous: ButtonState, current: ButtonState) -> Self {
        Self {
            up: !previous.up && current.up,
            down: !previous.down && current.down,
            confirm: !previous.a && current.a,
            back: !previous.b && current.b,
        }
    }

    pub fn any(self) -> bool {
        self.up || self.down || self.confirm || self.back
    }
}

// ---------------------------------------------------------------------------
// Output — menus with static items
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum MenuEffect {
    None,
    Resume,
    Save,
    Load,
    Quit,
    Continue,
    ShowRoms,
}

// ---------------------------------------------------------------------------
// Output — ROM list
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub enum RomListEffect {
    None,
    SelectItem,
    NextPage,
    PrevPage,
    Back,
}

// ---------------------------------------------------------------------------
// Render descriptor
// ---------------------------------------------------------------------------

pub struct MenuFrame<'a> {
    pub title: &'a str,
    pub items: &'a [&'a str],
    pub selected: usize,
    /// Parallel to `items`; false = greyed out (e.g. Load with no save slot).
    pub enabled: &'a [bool],
    /// Index of the item that carries a "currently loaded" indicator, if any.
    pub marked: Option<usize>,
}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

pub trait MenuLogic {
    fn frame(&self, context_flag: bool) -> MenuFrame<'_>;
    fn handle(&mut self, input: MenuInput) -> MenuEffect;
}

// ---------------------------------------------------------------------------
// In-game pause menu
// ---------------------------------------------------------------------------

const INGAME_ITEMS: &[&str] = &["RESUME", "SAVE", "LOAD", "QUIT"];

pub struct InGameMenu {
    selected: usize,
}

impl InGameMenu {
    pub fn new() -> Self {
        Self { selected: 0 }
    }
}

impl MenuLogic for InGameMenu {
    /// `context_flag` = save slot available (enables Load).
    fn frame(&self, save_available: bool) -> MenuFrame<'_> {
        static ALL_ENABLED: [bool; 4] = [true, true, true, true];
        static LOAD_DISABLED: [bool; 4] = [true, true, false, true];
        MenuFrame {
            title: "PAUSED",
            items: INGAME_ITEMS,
            selected: self.selected,
            enabled: if save_available {
                &ALL_ENABLED
            } else {
                &LOAD_DISABLED
            },
            marked: None,
        }
    }

    fn handle(&mut self, input: MenuInput) -> MenuEffect {
        if input.back {
            return MenuEffect::Resume;
        }
        if input.up && self.selected > 0 {
            self.selected -= 1;
            return MenuEffect::None;
        }
        if input.down && self.selected < INGAME_ITEMS.len() - 1 {
            self.selected += 1;
            return MenuEffect::None;
        }
        if input.confirm {
            return match self.selected {
                0 => MenuEffect::Resume,
                1 => MenuEffect::Save,
                2 => MenuEffect::Load,
                3 => MenuEffect::Quit,
                _ => MenuEffect::None,
            };
        }
        MenuEffect::None
    }
}

// ---------------------------------------------------------------------------
// Main menu
// ---------------------------------------------------------------------------

const MAIN_ITEMS_FULL: &[&str] = &["CONTINUE", "ROMS"];
const MAIN_ITEMS_ROMS: &[&str] = &["ROMS"];

pub struct MainMenu {
    selected: usize,
}

impl MainMenu {
    pub fn new() -> Self {
        Self { selected: 0 }
    }

    /// Call this from `MainMenuState::tick` so that the handler knows whether
    /// CONTINUE is available. B is always a no-op in the main menu.
    pub fn handle_main(&mut self, input: MenuInput, game_available: bool) -> MenuEffect {
        if !game_available {
            if input.confirm {
                return MenuEffect::ShowRoms;
            }
            return MenuEffect::None;
        }
        if input.up && self.selected > 0 {
            self.selected -= 1;
            return MenuEffect::None;
        }
        if input.down && self.selected < MAIN_ITEMS_FULL.len() - 1 {
            self.selected += 1;
            return MenuEffect::None;
        }
        if input.confirm {
            return match self.selected {
                0 => MenuEffect::Continue,
                1 => MenuEffect::ShowRoms,
                _ => MenuEffect::None,
            };
        }
        MenuEffect::None
    }
}

impl MenuLogic for MainMenu {
    /// `context_flag` = a ROM is staged and the game can be resumed.
    /// When false, CONTINUE is omitted entirely so no blank slot appears.
    fn frame(&self, game_available: bool) -> MenuFrame<'_> {
        static FULL_ENABLED: [bool; 2] = [true, true];
        static ROMS_ENABLED: [bool; 1] = [true];
        if game_available {
            MenuFrame {
                title: "MAIN MENU",
                items: MAIN_ITEMS_FULL,
                selected: self.selected,
                enabled: &FULL_ENABLED,
                marked: None,
            }
        } else {
            MenuFrame {
                title: "MAIN MENU",
                items: MAIN_ITEMS_ROMS,
                selected: 0,
                enabled: &ROMS_ENABLED,
                marked: None,
            }
        }
    }

    fn handle(&mut self, input: MenuInput) -> MenuEffect {
        self.handle_main(input, true)
    }
}

// ---------------------------------------------------------------------------
// ROM list logic
// ---------------------------------------------------------------------------

pub struct RomListLogic {
    selected: usize,
    page_len: usize,
}

impl RomListLogic {
    pub fn new(page_len: usize) -> Self {
        Self {
            selected: 0,
            page_len,
        }
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn handle(&mut self, input: MenuInput) -> RomListEffect {
        if input.back {
            return RomListEffect::Back;
        }
        if input.up {
            if self.selected > 0 {
                self.selected -= 1;
                return RomListEffect::None;
            } else {
                return RomListEffect::PrevPage;
            }
        }
        if input.down {
            if self.selected + 1 < self.page_len {
                self.selected += 1;
                return RomListEffect::None;
            } else {
                return RomListEffect::NextPage;
            }
        }
        if input.confirm {
            if self.page_len > 0 {
                return RomListEffect::SelectItem;
            }
        }
        RomListEffect::None
    }

    /// Reset selection to top (call after a page flip).
    pub fn reset(&mut self, new_page_len: usize) {
        self.selected = 0;
        self.page_len = new_page_len;
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn press_down() -> MenuInput {
        MenuInput {
            down: true,
            ..Default::default()
        }
    }
    fn press_up() -> MenuInput {
        MenuInput {
            up: true,
            ..Default::default()
        }
    }
    fn press_a() -> MenuInput {
        MenuInput {
            confirm: true,
            ..Default::default()
        }
    }
    fn press_b() -> MenuInput {
        MenuInput {
            back: true,
            ..Default::default()
        }
    }
    fn no_input() -> MenuInput {
        MenuInput::default()
    }

    // --- MenuInput::from_diff ---

    #[test]
    fn from_diff_detects_press_edge() {
        let previous = ButtonState::default();
        let current = ButtonState {
            a: true,
            ..Default::default()
        };
        let input = MenuInput::from_diff(previous, current);
        assert!(input.confirm);
        assert!(!input.up);
        assert!(!input.down);
        assert!(!input.back);
    }

    #[test]
    fn from_diff_ignores_held_button() {
        let held = ButtonState {
            a: true,
            ..Default::default()
        };
        let input = MenuInput::from_diff(held, held);
        assert!(!input.confirm, "held button should not re-fire");
    }

    #[test]
    fn from_diff_detects_release_as_no_input() {
        let previous = ButtonState {
            b: true,
            ..Default::default()
        };
        let current = ButtonState::default();
        let input = MenuInput::from_diff(previous, current);
        assert!(!input.back, "release should not count as a press");
    }

    // --- InGameMenu navigation ---

    #[test]
    fn ingame_down_moves_selection() {
        let mut menu = InGameMenu::new();
        menu.handle(press_down());
        assert_eq!(menu.selected, 1);
    }

    #[test]
    fn ingame_up_does_not_wrap_below_zero() {
        let mut menu = InGameMenu::new();
        menu.handle(press_up());
        assert_eq!(menu.selected, 0);
    }

    #[test]
    fn ingame_down_does_not_exceed_last_item() {
        let mut menu = InGameMenu::new();
        for _ in 0..10 {
            menu.handle(press_down());
        }
        assert_eq!(menu.selected, INGAME_ITEMS.len() - 1);
    }

    #[test]
    fn ingame_confirm_on_resume_returns_resume() {
        let mut menu = InGameMenu::new(); // selected = 0 = RESUME
        assert_eq!(menu.handle(press_a()), MenuEffect::Resume);
    }

    #[test]
    fn ingame_confirm_on_save_returns_save() {
        let mut menu = InGameMenu::new();
        menu.handle(press_down()); // -> SAVE
        assert_eq!(menu.handle(press_a()), MenuEffect::Save);
    }

    #[test]
    fn ingame_confirm_on_load_returns_load() {
        let mut menu = InGameMenu::new();
        menu.handle(press_down());
        menu.handle(press_down()); // -> LOAD
        assert_eq!(menu.handle(press_a()), MenuEffect::Load);
    }

    #[test]
    fn ingame_confirm_on_quit_returns_quit() {
        let mut menu = InGameMenu::new();
        for _ in 0..3 {
            menu.handle(press_down());
        } // -> QUIT
        assert_eq!(menu.handle(press_a()), MenuEffect::Quit);
    }

    #[test]
    fn ingame_back_always_resumes() {
        let mut menu = InGameMenu::new();
        menu.handle(press_down());
        menu.handle(press_down()); // on LOAD
        assert_eq!(menu.handle(press_b()), MenuEffect::Resume);
    }

    #[test]
    fn ingame_no_input_returns_none() {
        let mut menu = InGameMenu::new();
        assert_eq!(menu.handle(no_input()), MenuEffect::None);
    }

    #[test]
    fn ingame_frame_disables_load_without_save() {
        let menu = InGameMenu::new();
        let frame = menu.frame(false);
        assert!(
            !frame.enabled[2],
            "LOAD should be disabled with no save slot"
        );
    }

    #[test]
    fn ingame_frame_enables_load_with_save() {
        let menu = InGameMenu::new();
        let frame = menu.frame(true);
        assert!(frame.enabled[2], "LOAD should be enabled with a save slot");
    }

    // --- MainMenu navigation ---

    #[test]
    fn main_confirm_on_continue_returns_continue() {
        let mut menu = MainMenu::new(); // selected = 0 = CONTINUE
        assert_eq!(menu.handle(press_a()), MenuEffect::Continue);
    }

    #[test]
    fn main_confirm_on_roms_returns_show_roms() {
        let mut menu = MainMenu::new();
        menu.handle(press_down()); // -> ROMS
        assert_eq!(menu.handle(press_a()), MenuEffect::ShowRoms);
    }

    #[test]
    fn main_down_does_not_exceed_last_item() {
        let mut menu = MainMenu::new();
        for _ in 0..10 {
            menu.handle(press_down());
        }
        assert_eq!(menu.selected, MAIN_ITEMS_FULL.len() - 1);
    }

    #[test]
    fn main_frame_disables_continue_without_game() {
        let menu = MainMenu::new();
        let frame = menu.frame(false);
        // When no game is available, CONTINUE is omitted entirely; only ROMS appears.
        assert_eq!(frame.items, &["ROMS"]);
        assert!(
            frame.enabled[0],
            "ROMS should be enabled with no game running"
        );
    }

    #[test]
    fn main_frame_enables_continue_with_game() {
        let menu = MainMenu::new();
        let frame = menu.frame(true);
        assert!(
            frame.enabled[0],
            "CONTINUE should be enabled with a ROM staged"
        );
    }

    // --- RomListLogic ---

    #[test]
    fn romlist_down_moves_selection() {
        let mut logic = RomListLogic::new(3);
        assert_eq!(logic.handle(press_down()), RomListEffect::None);
        assert_eq!(logic.selected(), 1);
    }

    #[test]
    fn romlist_up_at_top_requests_prev_page() {
        let mut logic = RomListLogic::new(3);
        assert_eq!(logic.handle(press_up()), RomListEffect::PrevPage);
        assert_eq!(logic.selected(), 0, "selection stays at 0 on page boundary");
    }

    #[test]
    fn romlist_down_at_bottom_requests_next_page() {
        let mut logic = RomListLogic::new(2);
        logic.handle(press_down()); // selected = 1 (last)
        assert_eq!(logic.handle(press_down()), RomListEffect::NextPage);
    }

    #[test]
    fn romlist_confirm_returns_select() {
        let mut logic = RomListLogic::new(3);
        assert_eq!(logic.handle(press_a()), RomListEffect::SelectItem);
    }

    #[test]
    fn romlist_confirm_on_empty_page_returns_none() {
        let mut logic = RomListLogic::new(0);
        assert_eq!(logic.handle(press_a()), RomListEffect::None);
    }

    #[test]
    fn romlist_back_returns_back() {
        let mut logic = RomListLogic::new(3);
        assert_eq!(logic.handle(press_b()), RomListEffect::Back);
    }

    #[test]
    fn romlist_reset_resets_selection() {
        let mut logic = RomListLogic::new(5);
        logic.handle(press_down());
        logic.handle(press_down());
        logic.reset(3);
        assert_eq!(logic.selected(), 0);
    }

    #[test]
    fn romlist_no_input_returns_none() {
        let mut logic = RomListLogic::new(3);
        assert_eq!(logic.handle(no_input()), RomListEffect::None);
    }
}
