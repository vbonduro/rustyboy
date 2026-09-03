use alloc::boxed::Box;

use defmt::info;
use embassy_time::Duration;
use rustyboy_pico2w::display::{hw::GameDisplay, LoadingFrame, LoadingProgress};
use rustyboy_pico2w::flash_rom::{
    probe_staged_rom, FlashRomInfo, OnboardFlash, RomStager, WriteResult,
};
use rustyboy_pico2w::multicore::PicoGameBoy;
use rustyboy_pico2w::wdt;
use rustyboy_pico2w::xip_cartridge::XipCartridge;

use super::{MainMenuState, RunningState};
use crate::{
    flush_battery_save, load_battery_save, refresh_save_slot_available, App, AppState, PicoSdMgr,
};

pub struct LoadingState {
    pub(crate) filename: heapless::String<64>,
    pub(crate) display_name: heapless::String<64>,
}

impl LoadingState {
    pub async fn tick(
        &mut self,
        app: &mut App,
        gameboy: &mut Option<Box<PicoGameBoy>>,
        flash: &mut OnboardFlash<'_>,
        sd_mgr: &mut PicoSdMgr,
        game_disp: &mut GameDisplay<'static>,
    ) {
        if let Some(existing_gb) = gameboy.as_deref() {
            flush_battery_save(app, sd_mgr, existing_gb);
        }
        let had_game = halt_running_game(gameboy);
        let stage_result = self.ensure_rom_staged(app, flash, sd_mgr, game_disp).await;

        match stage_result {
            Ok(info) => {
                self.enter_staged_rom(app, gameboy, sd_mgr, game_disp, had_game, info)
                    .await;
            }
            Err(_) => {
                defmt::error!("ROM staging failed for {}", self.filename.as_str());
                transition_to_main_menu(app, game_disp).await;
            }
        }
    }

    async fn ensure_rom_staged(
        &self,
        app: &App,
        flash: &mut OnboardFlash<'_>,
        sd_mgr: &mut PicoSdMgr,
        game_disp: &mut GameDisplay<'static>,
    ) -> Result<FlashRomInfo, ()> {
        // If the ROM is already staged in flash, skip the erase/write cycle.
        // Avoids the flash-pause handshake with the halted core1 and is
        // idempotent when the user re-selects the currently loaded ROM.
        if rom_matches_staged(app, self.filename.as_str()) {
            return probe_staged_rom(flash).map(|(info, _)| info).ok_or(());
        }

        stage_rom_from_sd(
            self.filename.as_str(),
            self.display_name.as_str(),
            flash,
            sd_mgr,
            game_disp,
        )
        .await
    }

    async fn enter_staged_rom(
        &self,
        app: &mut App,
        gameboy: &mut Option<Box<PicoGameBoy>>,
        sd_mgr: &mut PicoSdMgr,
        game_disp: &mut GameDisplay<'static>,
        had_game: bool,
        info: FlashRomInfo,
    ) {
        app.staged_rom_name = Some(self.filename.clone());
        app.staged_rom_id = info.rom_id;
        if had_game {
            restart_after_rom_switch();
        } else {
            start_first_rom(app, gameboy, sd_mgr, game_disp, info).await;
        }
    }
}

fn halt_running_game(gameboy: &mut Option<Box<PicoGameBoy>>) -> bool {
    // Halt core1 if a game is running — must stop before writing flash.
    // Then drop the GameBoy to free its heap (~70 KB: front_buffer, GameBoyMemory,
    // OpCodeTable arcs, APU buffer) before RomStager::begin allocates its 16 KB
    // read buffer. Core 1 remains halted in its WFE loop after the drop.
    let had_game = gameboy.is_some();
    if let Some(existing_gb) = gameboy.as_mut() {
        existing_gb.halt();
    }
    if had_game {
        *gameboy = None;
    }
    had_game
}

fn rom_matches_staged(app: &App, filename: &str) -> bool {
    app.staged_rom_name
        .as_deref()
        .map(|n| n.eq_ignore_ascii_case(filename))
        .unwrap_or(false)
}

async fn start_first_rom(
    app: &mut App,
    gameboy: &mut Option<Box<PicoGameBoy>>,
    sd_mgr: &mut PicoSdMgr,
    game_disp: &mut GameDisplay<'static>,
    info: FlashRomInfo,
) {
    let core1 = app
        .core1
        .take()
        .expect("CORE1 consumed without a running game");
    match XipCartridge::from_staged_flash(info) {
        Ok(cart) => {
            let mut gb = Box::new(PicoGameBoy::with_cartridge(core1, Box::new(cart)));
            load_battery_save(app, sd_mgr, &mut gb);
            *gameboy = Some(gb);
            refresh_save_slot_available(app, sd_mgr);
            game_disp.draw_letterbox_bars().await;
            app.transition_to(AppState::Running(RunningState));
        }
        Err(e) => {
            defmt::error!("XIP cart error: {:?}", defmt::Debug2Format(&e));
            app.core1 = Some(core1);
            transition_to_main_menu(app, game_disp).await;
        }
    }
}

fn restart_after_rom_switch() -> ! {
    // ROM switch — core1 is halted; trigger watchdog reset to boot fresh with
    // the newly staged ROM.
    info!("ROM staged, restarting via watchdog");
    wdt::force_reset()
}

async fn transition_to_main_menu(app: &mut App, game_disp: &mut GameDisplay<'static>) {
    let next = MainMenuState::new(game_disp, app).await;
    app.transition_to(AppState::MainMenu(next));
}

async fn stage_rom_from_sd(
    filename: &str,
    display_name: &str,
    flash: &mut OnboardFlash<'_>,
    sd_mgr: &mut PicoSdMgr,
    game_disp: &mut GameDisplay<'static>,
) -> Result<FlashRomInfo, ()> {
    let mut reader = sd_mgr.open_rom_reader(filename).map_err(|e| {
        defmt::error!("sd open failed: {:?}", defmt::Debug2Format(&e));
    })?;
    game_disp
        .draw_loading_progress(LoadingFrame::new(
            display_name,
            LoadingProgress::new(0, 0),
            0,
        ))
        .await;

    // RP2350 watchdog maximum is 16,777,215 µs (~16.7 s). Use 16 s so the
    // erase phase (which can take several seconds) does not starve the watchdog.
    wdt::feed_for(Duration::from_millis(16_000));
    let mut stager = RomStager::begin(flash, &mut reader, filename).map_err(|e| {
        defmt::error!("stager begin failed: {:?}", defmt::Debug2Format(&e));
    })?;
    info!("stager begin done: {} banks", stager.total_banks());
    let total_banks = stager.total_banks();
    game_disp
        .draw_loading_bar(LoadingProgress::new(
            stager.banks_written() as u32,
            total_banks as u32,
        ))
        .await;

    loop {
        wdt::feed_for(Duration::from_millis(5_000));
        match stager.write_next_bank(flash, &mut reader).map_err(|e| {
            defmt::error!("bank write failed: {:?}", defmt::Debug2Format(&e));
        })? {
            WriteResult::Continue => {
                game_disp
                    .draw_loading_bar(LoadingProgress::new(
                        stager.banks_written() as u32,
                        total_banks as u32,
                    ))
                    .await;
            }
            WriteResult::Done(info) => {
                game_disp
                    .draw_loading_bar(LoadingProgress::new(total_banks as u32, total_banks as u32))
                    .await;
                return Ok(info);
            }
        }
    }
}
