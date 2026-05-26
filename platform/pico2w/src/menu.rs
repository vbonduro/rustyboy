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
    Reset,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RomPageSelection {
    First,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RomPageRequest {
    pub offset: usize,
    pub selection: RomPageSelection,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RomPageContext {
    pub offset: usize,
    pub page_size: usize,
    pub total_roms: usize,
    pub has_next: bool,
}

impl RomPageContext {
    pub const fn new(offset: usize, page_size: usize, total_roms: usize, has_next: bool) -> Self {
        Self {
            offset,
            page_size,
            total_roms,
            has_next,
        }
    }

    pub fn request(self, effect: RomListEffect) -> Option<RomPageRequest> {
        rom_page_request(effect, self)
    }
}

// ---------------------------------------------------------------------------
// Render descriptor
// ---------------------------------------------------------------------------

pub struct MenuFrame<'a> {
    pub title: &'a str,
    pub items: &'a [&'a str],
    pub selected: usize,
    /// Monotonic frame used by renderers for selected-item marquee animation.
    pub marquee_frame: u32,
    /// Parallel to `items`; false = greyed out (e.g. Load with no save slot).
    pub enabled: &'a [bool],
    /// Index of the item that carries a "currently loaded" indicator, if any.
    pub marked: Option<usize>,
    /// Show the crash-report badge in the status bar when `true`.
    pub crash_pending: bool,
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

const INGAME_ITEMS: &[&str] = &["RESUME", "SAVE", "LOAD", "RESET", "QUIT"];

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
        static ALL_ENABLED: [bool; 5] = [true, true, true, true, true];
        static LOAD_DISABLED: [bool; 5] = [true, true, false, true, true];
        MenuFrame {
            title: "PAUSED",
            items: INGAME_ITEMS,
            selected: self.selected,
            marquee_frame: 0,
            enabled: if save_available {
                &ALL_ENABLED
            } else {
                &LOAD_DISABLED
            },
            marked: None,
            crash_pending: false,
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
                3 => MenuEffect::Reset,
                4 => MenuEffect::Quit,
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
                marquee_frame: 0,
                enabled: &FULL_ENABLED,
                marked: None,
                crash_pending: false,
            }
        } else {
            MenuFrame {
                title: "MAIN MENU",
                items: MAIN_ITEMS_ROMS,
                selected: 0,
                marquee_frame: 0,
                enabled: &ROMS_ENABLED,
                marked: None,
                crash_pending: false,
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
        if self.page_len == 0 {
            return RomListEffect::None;
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

    /// Move selection to the final item on the current page.
    pub fn select_last(&mut self) {
        self.selected = self.page_len.saturating_sub(1);
    }
}

pub fn rom_page_request(effect: RomListEffect, page: RomPageContext) -> Option<RomPageRequest> {
    if page.page_size == 0 || page.total_roms == 0 {
        return None;
    }

    match effect {
        RomListEffect::NextPage if page.has_next => Some(RomPageRequest {
            offset: page.offset + page.page_size,
            selection: RomPageSelection::First,
        }),
        RomListEffect::NextPage => Some(RomPageRequest {
            offset: 0,
            selection: RomPageSelection::First,
        }),
        RomListEffect::PrevPage if page.offset > 0 => Some(RomPageRequest {
            offset: page.offset.saturating_sub(page.page_size),
            selection: RomPageSelection::Last,
        }),
        RomListEffect::PrevPage => Some(RomPageRequest {
            offset: (page.total_roms.saturating_sub(1) / page.page_size) * page.page_size,
            selection: RomPageSelection::Last,
        }),
        _ => None,
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
    fn page(offset: usize, page_size: usize, total_roms: usize, has_next: bool) -> RomPageContext {
        RomPageContext::new(offset, page_size, total_roms, has_next)
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

    #[test]
    fn menu_input_any_reports_only_actual_edges() {
        assert!(!no_input().any());
        assert!(press_up().any());
        assert!(press_down().any());
        assert!(press_a().any());
        assert!(press_b().any());
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
        assert_eq!(menu.selected, INGAME_ITEMS.len() - 1); // QUIT
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
    fn ingame_confirm_on_reset_returns_reset() {
        let mut menu = InGameMenu::new();
        for _ in 0..3 {
            menu.handle(press_down());
        } // -> RESET
        assert_eq!(menu.handle(press_a()), MenuEffect::Reset);
    }

    #[test]
    fn ingame_confirm_on_quit_returns_quit() {
        let mut menu = InGameMenu::new();
        for _ in 0..4 {
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
        let load_idx = INGAME_ITEMS.iter().position(|&s| s == "LOAD").unwrap();
        assert!(!frame.enabled[load_idx], "LOAD should be disabled with no save slot");
    }

    #[test]
    fn ingame_frame_enables_load_with_save() {
        let menu = InGameMenu::new();
        let frame = menu.frame(true);
        let load_idx = INGAME_ITEMS.iter().position(|&s| s == "LOAD").unwrap();
        assert!(frame.enabled[load_idx], "LOAD should be enabled with a save slot");
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
    fn main_without_game_goes_to_roms_even_after_navigation_input() {
        let mut menu = MainMenu::new();
        assert_eq!(menu.handle_main(press_down(), false), MenuEffect::None);
        assert_eq!(menu.handle_main(press_a(), false), MenuEffect::ShowRoms);
    }

    #[test]
    fn main_back_is_noop() {
        let mut menu = MainMenu::new();
        assert_eq!(menu.handle_main(press_b(), true), MenuEffect::None);
        assert_eq!(menu.handle_main(press_b(), false), MenuEffect::None);
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
    fn romlist_select_last_handles_empty_page() {
        let mut logic = RomListLogic::new(0);
        logic.select_last();
        assert_eq!(logic.selected(), 0);
    }

    #[test]
    fn romlist_reset_updates_page_len_for_boundary_navigation() {
        let mut logic = RomListLogic::new(5);
        logic.reset(1);

        assert_eq!(logic.handle(press_down()), RomListEffect::NextPage);
        assert_eq!(logic.selected(), 0);
    }

    #[test]
    fn romlist_no_input_returns_none() {
        let mut logic = RomListLogic::new(3);
        assert_eq!(logic.handle(no_input()), RomListEffect::None);
    }

    #[test]
    fn rom_page_request_down_moves_to_next_page_top() {
        assert_eq!(
            page(0, 7, 12, true).request(RomListEffect::NextPage),
            Some(RomPageRequest {
                offset: 7,
                selection: RomPageSelection::First,
            })
        );
    }

    #[test]
    fn rom_page_request_up_moves_to_previous_page_bottom() {
        assert_eq!(
            page(7, 7, 12, true).request(RomListEffect::PrevPage),
            Some(RomPageRequest {
                offset: 0,
                selection: RomPageSelection::Last,
            })
        );
    }

    #[test]
    fn rom_page_request_wraps_ends() {
        assert_eq!(
            page(7, 7, 12, false).request(RomListEffect::NextPage),
            Some(RomPageRequest {
                offset: 0,
                selection: RomPageSelection::First,
            })
        );
        assert_eq!(
            page(0, 7, 12, true).request(RomListEffect::PrevPage),
            Some(RomPageRequest {
                offset: 7,
                selection: RomPageSelection::Last,
            })
        );
    }

    #[test]
    fn rom_page_request_wraps_prev_to_final_partial_page() {
        assert_eq!(
            page(0, 7, 20, true).request(RomListEffect::PrevPage),
            Some(RomPageRequest {
                offset: 14,
                selection: RomPageSelection::Last,
            })
        );
    }

    #[test]
    fn rom_page_request_wraps_prev_to_final_full_page() {
        assert_eq!(
            page(0, 7, 14, true).request(RomListEffect::PrevPage),
            Some(RomPageRequest {
                offset: 7,
                selection: RomPageSelection::Last,
            })
        );
    }

    #[test]
    fn rom_page_request_ignores_non_page_effects_and_empty_contexts() {
        assert_eq!(page(0, 7, 12, true).request(RomListEffect::None), None);
        assert_eq!(
            page(0, 7, 12, true).request(RomListEffect::SelectItem),
            None
        );
        assert_eq!(page(0, 0, 12, true).request(RomListEffect::NextPage), None);
        assert_eq!(page(0, 7, 0, false).request(RomListEffect::PrevPage), None);
    }
}
