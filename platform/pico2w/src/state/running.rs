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

            // Read the dirty-row bitmap Core 1 computed when it published this
            // frame (compare raw pixels row-by-row at native 160×144 resolution,
            // ~115 µs).  Must be called after published_scaled_frame() so the
            // Acquire ordering from that call covers this read.
            let dirty_rows = gameboy.published_dirty_rows();

            // Hash the new frame (~345 µs at 300 MHz).  On static title /
            // pause screens the hash matches the previous frame's hash and we
            // skip the DMA entirely: no write race, no bounce.
            let frame_hash = GameDisplay::hash_frame(frame_buf);
            let frame_is_new = game_disp.frame_changed(frame_hash);

            // Narrow the DMA to only the display rows that cover dirty GB rows.
            // dirty_display_range() returns None only when bitmap is all-zero,
            // which shouldn't happen when frame_is_new is true (hash collision
            // corner-case); fall back to full-frame in that event.
            let (sy_start, sy_end) = if frame_is_new {
                GameDisplay::dirty_display_range(&dirty_rows).unwrap_or((0, 216))
            } else {
                (0, 216) // not used; frame is identical so DMA is skipped
            };

            if frame_is_new {
                // Send CASET / RASET / RAMWR to set the write window for the
                // dirty row range.  Completes in < 1 µs; leaves CS LOW / DC HIGH
                // ready for pixels.
                game_disp.setup_frame_range(sy_start, sy_end).await;
                // Record the hash now, before pin! borrows game_disp, so there
                // is no borrow-checker conflict with the DMA future below.
                game_disp.commit_frame_hash(frame_hash);
            }

            // Pin the display future unconditionally so it lives for this
            // scope.  poll_once (which actually starts the DMA hardware) is
            // only called when the frame is new; otherwise the future is
            // never polled and does nothing.
            let mut disp_future =
                core::pin::pin!(game_disp.send_frame_range_pixels(frame_buf, sy_start, sy_end));
            if frame_is_new {
                // Start the pixel DMA now.  poll_once arms the DMA hardware and
                // returns immediately; the transfer runs concurrently with the
                // ~16 ms emulation step below.  DMA time scales with sy_end -
                // sy_start: full frame ≈ 11 ms; a 50 % dirty range ≈ 5.5 ms.
                let _ = poll_once(disp_future.as_mut());
            }

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

            if frame_is_new {
                // The pixel DMA should already be done (~11 ms < ~16 ms
                // emulation); this await returns almost immediately.
                disp_future.as_mut().await;
            }
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
