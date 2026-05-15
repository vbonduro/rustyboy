use defmt::info;
use embassy_time::Instant;
use rustyboy_core::GameBoy;

pub struct PerfTracker {
    frame_count: u32,
    window_start: Instant,
}

impl PerfTracker {
    pub fn new() -> Self {
        Self {
            frame_count: 0,
            window_start: Instant::now(),
        }
    }

    pub fn tick(&mut self, _cpu: &mut GameBoy) {
        self.frame_count += 1;
        if self.frame_count < 60 {
            return;
        }

        let elapsed_us = self.window_start.elapsed().as_micros();
        let fps = (self.frame_count as u64 * 1_000_000) / elapsed_us.max(1);
        info!("fps: {}", fps);

        self.frame_count = 0;
        self.window_start = Instant::now();
    }
}
