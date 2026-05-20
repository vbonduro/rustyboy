use embassy_time::{Duration, Timer};
use rustyboy_pico2w::display::hw::GameDisplay;
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::menu::{MenuFrame, MenuInput, RomListEffect, RomListLogic};

use crate::{App, AppState, PicoSdMgr};
use super::{LoadingState, MainMenuState};

pub struct RomListState {
    page:        heapless::Vec<heapless::String<64>, 7>,
    page_offset: usize,
    has_next:    bool,
    logic:       RomListLogic,
}

impl RomListState {
    pub async fn new(
        app: &App,
        game_disp: &mut GameDisplay<'static>,
        sd_mgr: &mut PicoSdMgr,
    ) -> Self {
        let (page, has_next) = match sd_mgr.list_rom_page(0, 7) {
            Ok(result) => result,
            Err(e) => {
                defmt::error!("SD list failed: {:?}", defmt::Debug2Format(&e));
                (heapless::Vec::new(), false)
            }
        };
        let page_len = page.len();
        let logic = RomListLogic::new(page_len);
        let state = Self { page, page_offset: 0, has_next, logic };
        state.draw(app, game_disp).await;
        state
    }

    async fn draw(&self, app: &App, game_disp: &mut GameDisplay<'static>) {
        let mut items_arr: heapless::Vec<&str, 7> = heapless::Vec::new();
        let mut enabled_arr: heapless::Vec<bool, 7> = heapless::Vec::new();
        for name in self.page.iter() {
            let _ = items_arr.push(name.as_str());
            let _ = enabled_arr.push(true);
        }

        let marked = app.staged_rom_name.as_ref().and_then(|staged| {
            self.page.iter().position(|name| name.as_str().eq_ignore_ascii_case(staged.as_str()))
        });

        let frame = MenuFrame {
            title:    "ROMS",
            items:    items_arr.as_slice(),
            selected: self.logic.selected(),
            enabled:  enabled_arr.as_slice(),
            marked,
        };
        game_disp.draw_menu(&frame).await;
    }

    async fn flip_page(&mut self, new_offset: usize, sd_mgr: &mut PicoSdMgr) {
        match sd_mgr.list_rom_page(new_offset, 7) {
            Ok((page, has_next)) => {
                let page_len: usize = page.len();
                self.page = page;
                self.page_offset = new_offset;
                self.has_next = has_next;
                self.logic.reset(page_len);
            }
            Err(e) => {
                defmt::error!("SD flip_page failed: {:?}", defmt::Debug2Format(&e));
            }
        }
    }

    pub async fn tick(
        &mut self,
        app: &mut App,
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

        match self.logic.handle(menu_input) {
            RomListEffect::None => {
                self.draw(app, game_disp).await;
            }
            RomListEffect::SelectItem => {
                let filename = self.page[self.logic.selected()].clone();
                app.transition_to(AppState::Loading(alloc::boxed::Box::new(LoadingState { filename })));
            }
            RomListEffect::NextPage => {
                if self.has_next {
                    let new_offset = self.page_offset + 7;
                    self.flip_page(new_offset, sd_mgr).await;
                    self.draw(app, game_disp).await;
                }
            }
            RomListEffect::PrevPage => {
                if self.page_offset > 0 {
                    let new_offset = self.page_offset.saturating_sub(7);
                    self.flip_page(new_offset, sd_mgr).await;
                    self.draw(app, game_disp).await;
                }
            }
            RomListEffect::Back => {
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
        }
    }
}
