use alloc::boxed::Box;

use defmt::info;
use embassy_rp::watchdog::Watchdog;
use embassy_time::Duration;
use rustyboy_pico2w::display::hw::GameDisplay;
use rustyboy_pico2w::flash_rom::{
    probe_staged_rom, FlashRomInfo, OnboardFlash, RomStager, WriteResult,
};
use rustyboy_pico2w::multicore::PicoGameBoy;
use rustyboy_pico2w::xip_cartridge::XipCartridge;

use super::{MainMenuState, RunningState};
use crate::{App, AppState, PicoSdMgr};

pub struct LoadingState {
    pub(crate) filename: heapless::String<64>,
}

impl LoadingState {
    pub async fn tick(
        &mut self,
        app: &mut App,
        gameboy: &mut Option<PicoGameBoy>,
        flash: &mut OnboardFlash<'_>,
        sd_mgr: &mut PicoSdMgr,
        game_disp: &mut GameDisplay<'static>,
        watchdog: &mut Watchdog,
    ) {
        // Halt core1 if a game is running — must stop before writing flash.
        // Then drop the GameBoy to free its heap (~70 KB: front_buffer, GameBoyMemory,
        // OpCodeTable arcs, APU buffer) before RomStager::begin allocates its 16 KB
        // read buffer.  Core 1 remains halted in its WFE loop after the drop.
        let had_game = gameboy.is_some();
        if let Some(existing_gb) = gameboy.as_mut() {
            existing_gb.halt();
        }
        if had_game {
            *gameboy = None;
        }

        // If the ROM is already staged in flash, skip the erase/write cycle.
        // Avoids the flash-pause handshake with the halted core1 and is
        // idempotent when the user re-selects the currently loaded ROM.
        let already_staged = app
            .staged_rom_name
            .as_deref()
            .map(|n| n.eq_ignore_ascii_case(self.filename.as_str()))
            .unwrap_or(false);

        let stage_result: Result<FlashRomInfo, ()> = if already_staged {
            probe_staged_rom(flash).map(|(info, _)| info).ok_or(())
        } else {
            stage_rom_from_sd(self.filename.as_str(), flash, sd_mgr, game_disp, watchdog).await
        };

        match stage_result {
            Ok(info) => {
                app.staged_rom_name = Some(self.filename.clone());
                if !had_game {
                    // First ROM load — create the GameBoy and start Running.
                    let core1 = app
                        .core1
                        .take()
                        .expect("CORE1 consumed without a running game");
                    match XipCartridge::from_staged_flash(info) {
                        Ok(cart) => {
                            *gameboy = Some(PicoGameBoy::with_cartridge(core1, Box::new(cart)));
                            game_disp.draw_letterbox_bars().await;
                            app.transition_to(AppState::Running(RunningState));
                        }
                        Err(e) => {
                            defmt::error!("XIP cart error: {:?}", defmt::Debug2Format(&e));
                            app.core1 = Some(core1);
                            let next = MainMenuState::new(game_disp, app).await;
                            app.transition_to(AppState::MainMenu(next));
                        }
                    }
                } else {
                    // ROM switch — core1 is halted; trigger watchdog reset to
                    // boot fresh with the newly staged ROM.
                    info!("ROM staged, restarting via watchdog");
                    watchdog.start(Duration::from_millis(100));
                    loop {}
                }
            }
            Err(_) => {
                defmt::error!("ROM staging failed for {}", self.filename.as_str());
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
        }
    }
}

async fn stage_rom_from_sd(
    filename: &str,
    flash: &mut OnboardFlash<'_>,
    sd_mgr: &mut PicoSdMgr,
    game_disp: &mut GameDisplay<'static>,
    watchdog: &mut Watchdog,
) -> Result<FlashRomInfo, ()> {
    let mut reader = sd_mgr.open_rom_reader(filename).map_err(|e| {
        defmt::error!("sd open failed: {:?}", defmt::Debug2Format(&e));
    })?;

    // RP2350 watchdog maximum is 16,777,215 µs (~16.7 s). Use 16 s so the
    // erase phase (which can take several seconds) does not starve the watchdog.
    watchdog.feed(Duration::from_millis(16_000));
    let mut stager = RomStager::begin(flash, &mut reader, filename).map_err(|e| {
        defmt::error!("stager begin failed: {:?}", defmt::Debug2Format(&e));
    })?;
    info!("stager begin done: {} banks", stager.total_banks());
    let total_banks = stager.total_banks();
    game_disp
        .draw_loading_progress(filename, 0, total_banks as u32)
        .await;

    loop {
        watchdog.feed(Duration::from_millis(5_000));
        match stager.write_next_bank(flash, &mut reader).map_err(|e| {
            defmt::error!("bank write failed: {:?}", defmt::Debug2Format(&e));
        })? {
            WriteResult::Continue => {
                game_disp
                    .draw_loading_progress(
                        filename,
                        stager.banks_written() as u32,
                        total_banks as u32,
                    )
                    .await;
            }
            WriteResult::Done(info) => {
                game_disp
                    .draw_loading_progress(filename, total_banks as u32, total_banks as u32)
                    .await;
                return Ok(info);
            }
        }
    }
}
