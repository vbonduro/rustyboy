use alloc::vec::Vec;

use embassy_rp::peripherals::PIO0;
use embassy_rp::pio_programs::i2s::PioI2sOut;
use rustyboy_pico2w::audio::AudioBuffers;
use rustyboy_pico2w::display::hw::GameDisplay;
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::multicore::PicoGameBoy;

use super::InGameMenuState;
use crate::{
    poll_once, refresh_save_slot_available, App, AppState, PicoSdMgr,
    CYCLES_PER_FRAME,
};

pub struct RunningState;

impl RunningState {
    pub async fn tick(
        &mut self,
        app: &mut App,
        gameboy: &mut PicoGameBoy,
        game_disp: &mut GameDisplay<'static>,
        i2s: &mut PioI2sOut<'static, PIO0, 0>,
        input: &mut InputHandler<'static>,
        sd_mgr: &mut PicoSdMgr,
        audio_buffers: &mut AudioBuffers,
        audio_samples: &mut Vec<i16>,
    ) {
        // Scope the DMA futures so `game_disp` is free after the block.
        let open_menu = {
            let frame_buf = gameboy.published_scaled_frame();
            // Dirty-row bitmap produced by Core 1 at native GB resolution.
            // Must be read after published_scaled_frame() (Acquire ordering).
            let dirty_rows = gameboy.published_dirty_rows();

            // send_frame handles hash check, dirty-range narrowing, blocking
            // CASET/RASET/RAMWR setup (~2 µs), and async pixel DMA.
            //
            // First poll (poll_once): if frame unchanged → Ready immediately.
            // If changed: blocking setup completes, DMA armed → Pending.
            // The pixel DMA then runs concurrently with the ~16 ms emulation below.
            let mut disp_future =
                core::pin::pin!(game_disp.send_frame(frame_buf, &dirty_rows));
            let _ = poll_once(disp_future.as_mut());

            let (front_buf, back_buf) = audio_buffers.front_back_buffers();
            let mut audio_future = core::pin::pin!(i2s.write(front_buf));
            let _ = poll_once(audio_future.as_mut());

            let frame_start = gameboy.cycle_counter();
            while gameboy.cycle_counter().wrapping_sub(frame_start) < CYCLES_PER_FRAME {
                gameboy.tick();
            }

            let (current_buttons, open_menu) = input.poll();
            for (btn, pressed) in app.previous_buttons.diff(current_buttons) {
                gameboy.set_button(btn, pressed);
            }
            app.previous_buttons = current_buttons;

            gameboy.drain_audio_samples_into_i16(audio_samples);
            audio_buffers.queue_next_frame_i16(audio_samples, back_buf);

            // If send_frame returned Ready on the first poll (identical frame),
            // this await is instant.  Otherwise the DMA is already done
            // (~11 ms < ~16 ms emulation) and this returns almost immediately.
            disp_future.as_mut().await;
            gameboy.release_scaled_frame();
            audio_future.as_mut().await;

            // Update the crash context once per frame so the fault handler
            // always has a recent snapshot of the emulator state.
            let rom_id_prefix = app
                .staged_rom_id
                .map(|id| {
                    let mut p = [0u8; 4];
                    p.copy_from_slice(&id.as_bytes()[..4]);
                    p
                })
                .unwrap_or([0u8; 4]);
            gameboy.update_crash_context(rom_id_prefix);

            open_menu
        };

        if open_menu {
            refresh_save_slot_available(app, sd_mgr);
            let next = InGameMenuState::new(game_disp, app).await;
            app.transition_to(AppState::InGameMenu(next));
        }
    }
}
