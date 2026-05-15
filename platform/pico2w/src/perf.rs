use defmt::info;
use embassy_time::Instant;
use rustyboy_core::GameBoy;

/// Tracks FPS and wall-clock timing per component.
/// Call `tick` once per game loop iteration.
pub struct PerfTracker {
    frame_count: u32,
    window_start: Instant,
    emulate_wall_us: u64,
    render_wait_wall_us: u64,
    audio_wait_wall_us: u64,
}

impl PerfTracker {
    pub fn new() -> Self {
        Self {
            frame_count: 0,
            window_start: Instant::now(),
            emulate_wall_us: 0,
            render_wait_wall_us: 0,
            audio_wait_wall_us: 0,
        }
    }

    pub fn record_emulate_wall_us(&mut self, us: u64) {
        self.emulate_wall_us += us;
    }

    pub fn record_render_wait_wall_us(&mut self, us: u64) {
        self.render_wait_wall_us += us;
    }

    pub fn record_audio_wait_wall_us(&mut self, us: u64) {
        self.audio_wait_wall_us += us;
    }

    pub fn tick(&mut self, cpu: &mut GameBoy) {
        self.frame_count += 1;
        if self.frame_count < 60 {
            return;
        }

        let elapsed_us = self.window_start.elapsed().as_micros();
        let fps = (self.frame_count as u64 * 1_000_000) / elapsed_us.max(1);
        let pub_fps = 1_000_000_000u64 / elapsed_us.max(1) * self.frame_count as u64 / 1_000;
        info!("fps: {}", fps);
        info!("pub_fps: {}", pub_fps);

        info!(
            "wall/60f — emulate={}us render_wait={}us audio_wait={}us",
            self.emulate_wall_us,
            self.render_wait_wall_us,
            self.audio_wait_wall_us,
        );

        // Suppress unused-variable warning.
        let _ = cpu;

        self.emulate_wall_us = 0;
        self.render_wait_wall_us = 0;
        self.audio_wait_wall_us = 0;
        self.frame_count = 0;
        self.window_start = Instant::now();
    }
}
