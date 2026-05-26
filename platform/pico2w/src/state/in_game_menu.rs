use defmt::warn;
use embassy_time::{Duration, Timer};
use rustyboy_core::cpu::save_state::SaveState;
use rustyboy_pico2w::display::hw::GameDisplay;
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::menu::{InGameMenu, MenuEffect, MenuInput, MenuLogic};
use rustyboy_pico2w::multicore::PicoGameBoy;
use rustyboy_pico2w::save_storage::SaveSlot;

use super::{MainMenuState, RunningState};
use crate::{flush_battery_save, App, AppState, PicoSdMgr};

pub struct InGameMenuState {
    menu: InGameMenu,
}

impl InGameMenuState {
    pub async fn new(game_disp: &mut GameDisplay<'static>, app: &App) -> Self {
        let menu = InGameMenu::new();
        let mut frame = menu.frame(app.save_slot_available);
        frame.crash_pending = app.crash_pending;
        game_disp.draw_menu(&frame).await;
        Self { menu }
    }

    pub async fn tick(
        &mut self,
        app: &mut App,
        gameboy: &mut PicoGameBoy,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
        sd_mgr: &mut PicoSdMgr,
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
                let mut frame = self.menu.frame(app.save_slot_available);
                frame.crash_pending = app.crash_pending;
                game_disp.draw_menu(&frame).await;
            }
            MenuEffect::Resume => {
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::Save => {
                flush_battery_save(app, sd_mgr, gameboy);
                if let Some(rom_id) = app.staged_rom_id {
                    let slot = SaveSlot::new(0).expect("slot 0 is valid");
                    let blob = gameboy.save_state();
                    match sd_mgr.write_save_state(&rom_id, slot, &blob) {
                        Ok(()) => app.save_slot_available = true,
                        Err(e) => warn!("state save failed: {:?}", defmt::Debug2Format(&e)),
                    }
                }
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::Load => {
                if let Some(rom_id) = app.staged_rom_id {
                    let slot = SaveSlot::new(0).expect("slot 0 is valid");
                    match sd_mgr.read_save_state(&rom_id, slot) {
                        Ok(Some(blob)) => match SaveState::from_blob(blob) {
                            Ok(save_state) => {
                                let _ = gameboy.load_state(save_state);
                            }
                            Err(message) => {
                                warn!("load failed: {}", message);
                            }
                        },
                        Ok(None) => app.save_slot_available = false,
                        Err(e) => warn!("state read failed: {:?}", defmt::Debug2Format(&e)),
                    }
                }
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::Reset => {
                gameboy.reset();
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::Quit => {
                flush_battery_save(app, sd_mgr, gameboy);
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
            _ => {}
        }
    }
}
