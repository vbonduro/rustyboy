//! Settings screen state — a single-level menu with a WIFI entry.

use embassy_time::{Duration, Timer};
use rustyboy_pico2w::display::hw::GameDisplay;
use rustyboy_pico2w::flash_rom::OnboardFlash;
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::menu::{MenuEffect, MenuInput, MenuLogic, SettingsMenu};

use super::{main_menu::MainMenuState, MENU_POLL_MS};
use crate::{App, AppState};

use super::wifi_menu::WifiMenuState;

pub struct SettingsState {
    menu: SettingsMenu,
}

impl SettingsState {
    pub async fn new(game_disp: &mut GameDisplay<'static>, app: &App) -> Self {
        let menu = SettingsMenu::new();
        let mut frame = menu.frame(false);
        frame.crash_pending = app.crash_pending;
        game_disp.draw_menu(&frame).await;
        Self { menu }
    }

    pub async fn tick(
        &mut self,
        app: &mut App,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
        #[allow(unused_variables)] flash: &mut OnboardFlash<'_>,
    ) {
        Timer::after(Duration::from_millis(MENU_POLL_MS)).await;

        let (current_buttons, _) = input.poll();
        let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
        app.previous_buttons = current_buttons;

        if !menu_input.any() {
            return;
        }

        match self.menu.handle(menu_input) {
            MenuEffect::None => {
                let mut frame = self.menu.frame(false);
                frame.crash_pending = app.crash_pending;
                game_disp.draw_menu(&frame).await;
            }
            MenuEffect::Back => {
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
            MenuEffect::ShowWifiMenu => {
                let next = WifiMenuState::new(game_disp, app, flash).await;
                app.transition_to(AppState::WifiMenu(alloc::boxed::Box::new(next)));
            }
            _ => {}
        }
    }
}
