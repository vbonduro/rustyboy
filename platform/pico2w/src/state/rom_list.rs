use embassy_time::{Duration, Timer};
use rustyboy_pico2w::display::{
    hw::GameDisplay, menu_item_needs_marquee, MENU_MARQUEE_REDRAW_FRAMES,
};
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::menu::{
    MenuFrame, MenuInput, RomListEffect, RomListLogic, RomPageContext, RomPageSelection,
};
use rustyboy_pico2w::sd::RomListEntry;

use super::{LoadingState, MainMenuState};
use crate::{App, AppState, PicoSdMgr};

const ROM_PAGE_SIZE: usize = 7;

pub struct RomListState {
    page: heapless::Vec<RomListEntry, 7>,
    page_offset: usize,
    has_next: bool,
    total_roms: usize,
    logic: RomListLogic,
    marquee_frame: u32,
}

struct RomMenuFrame<'a> {
    items: heapless::Vec<&'a str, 7>,
    enabled: heapless::Vec<bool, 7>,
    marked: Option<usize>,
}

impl RomListState {
    pub async fn new(
        app: &App,
        game_disp: &mut GameDisplay<'static>,
        sd_mgr: &mut PicoSdMgr,
    ) -> Self {
        let (page, has_next, total_roms) = match sd_mgr.list_rom_page(0, ROM_PAGE_SIZE) {
            Ok(result) => (result.entries, result.has_next, result.total),
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
        let frame_data = self.menu_frame_data(app);
        let frame = self.menu_frame(&frame_data, app.crash_pending);
        game_disp.draw_menu(&frame).await;
    }

    async fn draw_row(
        &self,
        app: &App,
        game_disp: &mut GameDisplay<'static>,
        slot: usize,
        text_only: bool,
    ) {
        let frame_data = self.menu_frame_data(app);
        let frame = self.menu_frame(&frame_data, app.crash_pending);
        if text_only {
            game_disp.draw_menu_item_text(&frame, slot).await;
        } else {
            game_disp.draw_menu_item(&frame, slot).await;
        }
    }

    fn menu_frame_data<'a>(&'a self, app: &App) -> RomMenuFrame<'a> {
        let mut items = heapless::Vec::new();
        let mut enabled = heapless::Vec::new();
        for entry in self.page.iter() {
            let _ = items.push(entry.display_name.as_str());
            let _ = enabled.push(true);
        }

        let marked = app.staged_rom_name.as_ref().and_then(|staged| {
            self.page.iter().position(|entry| {
                entry
                    .filename
                    .as_str()
                    .eq_ignore_ascii_case(staged.as_str())
            })
        });

        RomMenuFrame {
            items,
            enabled,
            marked,
        }
    }

    fn menu_frame<'a>(&self, frame_data: &'a RomMenuFrame<'a>, crash_pending: bool) -> MenuFrame<'a> {
        MenuFrame {
            title: "ROMS",
            items: frame_data.items.as_slice(),
            selected: self.logic.selected(),
            marquee_frame: self.marquee_frame,
            enabled: frame_data.enabled.as_slice(),
            marked: frame_data.marked,
            crash_pending,
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

    fn page_context(&self) -> RomPageContext {
        RomPageContext::new(
            self.page_offset,
            ROM_PAGE_SIZE,
            self.total_roms,
            self.has_next,
        )
    }

    async fn flip_page(
        &mut self,
        new_offset: usize,
        sd_mgr: &mut PicoSdMgr,
        selection: RomPageSelection,
    ) {
        match sd_mgr.list_rom_page(new_offset, ROM_PAGE_SIZE) {
            Ok(result) => {
                let page_len: usize = result.entries.len();
                self.page = result.entries;
                self.page_offset = new_offset;
                self.has_next = result.has_next;
                self.total_roms = result.total;
                self.logic.reset(page_len);
                if matches!(selection, RomPageSelection::Last) {
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

        let menu_input = poll_menu_input(app, input);
        if !menu_input.any() {
            self.tick_marquee(app, game_disp).await;
            return;
        }

        let previous_selected = self.logic.selected();
        let effect = self.logic.handle(menu_input);
        self.apply_effect(effect, previous_selected, app, game_disp, sd_mgr)
            .await;
    }

    async fn tick_marquee(&mut self, app: &App, game_disp: &mut GameDisplay<'static>) {
        if !self.selected_needs_marquee(app) {
            return;
        }
        self.marquee_frame = self.marquee_frame.wrapping_add(1);
        if self.marquee_frame % MENU_MARQUEE_REDRAW_FRAMES == 0 {
            self.draw_row(app, game_disp, self.logic.selected(), true)
                .await;
        }
    }

    async fn apply_effect(
        &mut self,
        effect: RomListEffect,
        previous_selected: usize,
        app: &mut App,
        game_disp: &mut GameDisplay<'static>,
        sd_mgr: &mut PicoSdMgr,
    ) {
        match effect {
            RomListEffect::None => {
                self.marquee_frame = 0;
                self.redraw_selection_change(app, game_disp, previous_selected)
                    .await;
            }
            RomListEffect::SelectItem => {
                self.transition_to_selected_rom(app);
            }
            RomListEffect::NextPage => {
                self.flip_requested_page(RomListEffect::NextPage, sd_mgr, app, game_disp)
                    .await;
            }
            RomListEffect::PrevPage => {
                self.flip_requested_page(RomListEffect::PrevPage, sd_mgr, app, game_disp)
                    .await;
            }
            RomListEffect::Back => {
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
        }
    }

    async fn redraw_selection_change(
        &self,
        app: &App,
        game_disp: &mut GameDisplay<'static>,
        previous_selected: usize,
    ) {
        let selected = self.logic.selected();
        if selected == previous_selected {
            return;
        }

        // Write destination first, then source.
        //
        // At 75 MHz SPI we write at 51.2 µs/row; the ILI9341 scans at ~52 µs/row.
        // Our write is fractionally faster, so if we start writing a row just
        // before the scan arrives we outpace it and every row shows new content
        // before the scan reads it.
        //
        // By writing the destination item (the row the cursor is moving TO) first:
        //   • The scan is typically still in the source area — destination hasn't
        //     been reached yet, so we write it cleanly ahead of the scan.
        //   • By the time we write the source, the scan has advanced past it and
        //     new content appears on the next scan pass without a torn line.
        //
        // Writing source first (old order) had the opposite problem: the scan
        // was often inside the source rows, leaving a horizontal tear on the
        // item the cursor was leaving, visible right as the marquee started.
        self.draw_row(app, game_disp, selected, false).await;
        self.draw_row(app, game_disp, previous_selected, false)
            .await;
    }

    fn transition_to_selected_rom(&self, app: &mut App) {
        let Some(entry) = self.page.get(self.logic.selected()).cloned() else {
            return;
        };
        app.transition_to(AppState::Loading(alloc::boxed::Box::new(LoadingState {
            filename: entry.filename,
            display_name: entry.display_name,
        })));
    }

    async fn flip_requested_page(
        &mut self,
        effect: RomListEffect,
        sd_mgr: &mut PicoSdMgr,
        app: &App,
        game_disp: &mut GameDisplay<'static>,
    ) {
        self.marquee_frame = 0;
        let Some(page) = self.page_context().request(effect) else {
            return;
        };

        self.flip_page(page.offset, sd_mgr, page.selection).await;
        self.draw(app, game_disp).await;
    }
}

fn poll_menu_input(app: &mut App, input: &mut InputHandler<'static>) -> MenuInput {
    let (current_buttons, _) = input.poll();
    let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
    app.previous_buttons = current_buttons;
    menu_input
}
