use embassy_time::{Duration, Timer};
use rustyboy_pico2w::display::hw::GameDisplay;
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::menu::{MainMenu, MenuEffect, MenuInput, MenuLogic};

use super::{RomListState, RunningState, SettingsState};
use embassy_rp::watchdog::Watchdog;
use crate::{App, AppState, PicoSdMgr};

pub struct MainMenuState {
    menu: MainMenu,
}

impl MainMenuState {
    pub async fn new(game_disp: &mut GameDisplay<'static>, app: &App) -> Self {
        let menu = MainMenu::new();
        let mut frame = menu.frame(app.staged_rom_name.is_some());
        frame.crash_pending = app.crash_pending;
        game_disp.draw_menu(&frame).await;
        Self { menu }
    }

    pub async fn tick(
        &mut self,
        app: &mut App,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
        sd_mgr: &mut PicoSdMgr,
        watchdog: &mut Watchdog,
    ) {
        Timer::after(Duration::from_millis(33)).await;

        let (current_buttons, _) = input.poll();
        let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
        app.previous_buttons = current_buttons;

        if !menu_input.any() {
            return;
        }

        let game_available = app.staged_rom_name.is_some();
        match self.menu.handle_main(menu_input, game_available) {
            MenuEffect::None => {
                let mut frame = self.menu.frame(game_available);
                frame.crash_pending = app.crash_pending;
                game_disp.draw_menu(&frame).await;
            }
            MenuEffect::Continue => {
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::ShowRoms => {
                let next = RomListState::new(app, game_disp, sd_mgr, watchdog).await;
                app.transition_to(AppState::RomList(alloc::boxed::Box::new(next)));
            }
            MenuEffect::ShowSettings => {
                let next = SettingsState::new(game_disp, app).await;
                app.transition_to(AppState::Settings(next));
            }
            _ => {}
        }
    }
}
