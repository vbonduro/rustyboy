use embassy_time::{Duration, Timer};
use rustyboy_pico2w::display::{
    hw::GameDisplay, menu_item_needs_marquee, MENU_MARQUEE_REDRAW_FRAMES,
};
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::menu::{MenuFrame, MenuInput, RomListEffect, RomListLogic};
use rustyboy_pico2w::sd::RomListEntry;

use super::{LoadingState, MainMenuState};
use crate::{App, AppState, PicoSdMgr};

pub struct RomListState {
    page: heapless::Vec<RomListEntry, 7>,
    page_offset: usize,
    has_next: bool,
    total_roms: usize,
    logic: RomListLogic,
    marquee_frame: u32,
}

enum PageSelection {
    First,
    Last,
}

impl RomListState {
    pub async fn new(
        app: &App,
        game_disp: &mut GameDisplay<'static>,
        sd_mgr: &mut PicoSdMgr,
    ) -> Self {
        let (page, has_next, total_roms) = match sd_mgr.list_rom_page(0, 7) {
            Ok(result) => result,
            Err(e) => {
                defmt::error!("SD list failed: {:?}", defmt::Debug2Format(&e));
                (heapless::Vec::new(), false, 0)
            }
        };
        let page_len = page.len();
        let logic = RomListLogic::new(page_len);
        let state = Self {
            page,
            page_offset: 0,
            has_next,
            total_roms,
            logic,
            marquee_frame: 0,
        };
        state.draw(app, game_disp).await;
        state
    }

    async fn draw(&self, app: &App, game_disp: &mut GameDisplay<'static>) {
        let mut items_arr: heapless::Vec<&str, 7> = heapless::Vec::new();
        let mut enabled_arr: heapless::Vec<bool, 7> = heapless::Vec::new();
        for entry in self.page.iter() {
            let _ = items_arr.push(entry.display_name.as_str());
            let _ = enabled_arr.push(true);
        }

        let marked = app.staged_rom_name.as_ref().and_then(|staged| {
            self.page.iter().position(|entry| {
                entry
                    .filename
                    .as_str()
                    .eq_ignore_ascii_case(staged.as_str())
            })
        });

        let frame = MenuFrame {
            title: "ROMS",
            items: items_arr.as_slice(),
            selected: self.logic.selected(),
            marquee_frame: self.marquee_frame,
            enabled: enabled_arr.as_slice(),
            marked,
        };
        game_disp.draw_menu(&frame).await;
    }

    async fn draw_row(
        &self,
        app: &App,
        game_disp: &mut GameDisplay<'static>,
        slot: usize,
        text_only: bool,
    ) {
        let mut items_arr: heapless::Vec<&str, 7> = heapless::Vec::new();
        let mut enabled_arr: heapless::Vec<bool, 7> = heapless::Vec::new();
        for entry in self.page.iter() {
            let _ = items_arr.push(entry.display_name.as_str());
            let _ = enabled_arr.push(true);
        }

        let marked = app.staged_rom_name.as_ref().and_then(|staged| {
            self.page.iter().position(|entry| {
                entry
                    .filename
                    .as_str()
                    .eq_ignore_ascii_case(staged.as_str())
            })
        });

        let frame = MenuFrame {
            title: "ROMS",
            items: items_arr.as_slice(),
            selected: self.logic.selected(),
            marquee_frame: self.marquee_frame,
            enabled: enabled_arr.as_slice(),
            marked,
        };
        if text_only {
            game_disp.draw_menu_item_text(&frame, slot).await;
        } else {
            game_disp.draw_menu_item(&frame, slot).await;
        }
    }

    fn selected_needs_marquee(&self, app: &App) -> bool {
        let selected = self.logic.selected();
        let Some(entry) = self.page.get(selected) else {
            return false;
        };
        let marked = app
            .staged_rom_name
            .as_ref()
            .map(|staged| {
                entry
                    .filename
                    .as_str()
                    .eq_ignore_ascii_case(staged.as_str())
            })
            .unwrap_or(false);
        menu_item_needs_marquee(entry.display_name.as_str(), marked)
    }

    async fn flip_page(
        &mut self,
        new_offset: usize,
        sd_mgr: &mut PicoSdMgr,
        selection: PageSelection,
    ) {
        match sd_mgr.list_rom_page(new_offset, 7) {
            Ok((page, has_next, total_roms)) => {
                let page_len: usize = page.len();
                self.page = page;
                self.page_offset = new_offset;
                self.has_next = has_next;
                self.total_roms = total_roms;
                self.logic.reset(page_len);
                if matches!(selection, PageSelection::Last) {
                    self.logic.select_last();
                }
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
            if self.selected_needs_marquee(app) {
                self.marquee_frame = self.marquee_frame.wrapping_add(1);
                if self.marquee_frame % MENU_MARQUEE_REDRAW_FRAMES == 0 {
                    self.draw_row(app, game_disp, self.logic.selected(), true)
                        .await;
                }
            }
            return;
        }

        let previous_selected = self.logic.selected();
        match self.logic.handle(menu_input) {
            RomListEffect::None => {
                self.marquee_frame = 0;
                let selected = self.logic.selected();
                if selected != previous_selected {
                    self.draw_row(app, game_disp, previous_selected, false)
                        .await;
                    self.draw_row(app, game_disp, selected, false).await;
                }
            }
            RomListEffect::SelectItem => {
                let entry = self.page[self.logic.selected()].clone();
                app.transition_to(AppState::Loading(alloc::boxed::Box::new(LoadingState {
                    filename: entry.filename,
                    display_name: entry.display_name,
                })));
            }
            RomListEffect::NextPage => {
                self.marquee_frame = 0;
                if self.has_next {
                    let new_offset = self.page_offset + 7;
                    self.flip_page(new_offset, sd_mgr, PageSelection::First)
                        .await;
                    self.draw(app, game_disp).await;
                } else if self.total_roms > 0 {
                    self.flip_page(0, sd_mgr, PageSelection::First).await;
                    self.draw(app, game_disp).await;
                }
            }
            RomListEffect::PrevPage => {
                self.marquee_frame = 0;
                if self.page_offset > 0 {
                    let new_offset = self.page_offset.saturating_sub(7);
                    self.flip_page(new_offset, sd_mgr, PageSelection::Last)
                        .await;
                    self.draw(app, game_disp).await;
                } else if self.total_roms > 0 {
                    let last_page_offset = (self.total_roms.saturating_sub(1) / 7) * 7;
                    self.flip_page(last_page_offset, sd_mgr, PageSelection::Last)
                        .await;
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
