use defmt::warn;
use embassy_time::{Duration, Timer};
use rustyboy_core::cpu::save_state::SaveState;
use rustyboy_pico2w::display::hw::GameDisplay;
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::menu::{InGameMenu, MenuEffect, MenuInput, MenuLogic};
use rustyboy_pico2w::multicore::PicoGameBoy;

use super::{MainMenuState, RunningState};
use crate::{App, AppState};

pub struct InGameMenuState {
    menu: InGameMenu,
}

impl InGameMenuState {
    pub async fn new(game_disp: &mut GameDisplay<'static>, app: &App) -> Self {
        let menu = InGameMenu::new();
        let frame = menu.frame(app.save_slot.is_some());
        game_disp.draw_menu(&frame).await;
        Self { menu }
    }

    pub async fn tick(
        &mut self,
        app: &mut App,
        gameboy: &mut PicoGameBoy,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
    ) {
        Timer::after(Duration::from_millis(33)).await;

        let (current_buttons, _) = input.poll();
        let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
        app.previous_buttons = current_buttons;

        if !menu_input.any() {
            return;
        }

        match self.menu.handle(menu_input) {
            MenuEffect::None => {
                let frame = self.menu.frame(app.save_slot.is_some());
                game_disp.draw_menu(&frame).await;
            }
            MenuEffect::Resume => {
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::Save => {
                app.save_slot = Some(gameboy.save_state());
                let frame = self.menu.frame(true);
                game_disp.draw_menu(&frame).await;
            }
            MenuEffect::Load => {
                if let Some(blob) = app.save_slot.as_ref() {
                    match SaveState::from_blob(blob.clone()) {
                        Ok(save_state) => {
                            let _ = gameboy.load_state(save_state);
                        }
                        Err(message) => {
                            warn!("load failed: {}", message);
                        }
                    }
                }
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::Quit => {
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
            _ => {}
        }
    }
}
