use defmt::info;
use embassy_time::Instant;
use rustyboy_pico2w::multicore::TransportProfile;

pub struct PerfTracker {
    frame_count: u32,
    window_start: Instant,
    frame_publishes: u32,
    queue_spins: u32,
    command_enqueues: u32,
    ppu_vram_bytes: u32,
    ppu_register_writes: u32,
    audio_queue_drops: u32,
}

impl PerfTracker {
    pub fn new() -> Self {
        Self {
            frame_count: 0,
            window_start: Instant::now(),
            frame_publishes: 0,
            queue_spins: 0,
            command_enqueues: 0,
            ppu_vram_bytes: 0,
            ppu_register_writes: 0,
            audio_queue_drops: 0,
        }
    }

    pub fn tick(&mut self, profile: TransportProfile) {
        self.frame_count += 1;
        self.frame_publishes = self.frame_publishes.wrapping_add(profile.frame_publishes);
        self.queue_spins = self.queue_spins.wrapping_add(profile.command_queue_spins);
        self.command_enqueues = self.command_enqueues.wrapping_add(profile.command_enqueues);
        self.ppu_vram_bytes = self.ppu_vram_bytes.wrapping_add(profile.ppu_vram_bytes);
        self.ppu_register_writes = self.ppu_register_writes.wrapping_add(profile.ppu_register_writes);
        self.audio_queue_drops = self.audio_queue_drops.wrapping_add(profile.audio_queue_drops);

        if self.frame_count < 60 {
            return;
        }

        let elapsed_us = self.window_start.elapsed().as_micros();
        let fps = (self.frame_count as u64 * 1_000_000) / elapsed_us.max(1);
        info!(
            "fps={} pub={} spins={} enqs={} vram={}B ppu_regs={} audio_drops={}",
            fps,
            self.frame_publishes,
            self.queue_spins,
            self.command_enqueues,
            self.ppu_vram_bytes,
            self.ppu_register_writes,
            self.audio_queue_drops,
        );

        self.frame_count = 0;
        self.window_start = Instant::now();
        self.frame_publishes = 0;
        self.queue_spins = 0;
        self.command_enqueues = 0;
        self.ppu_vram_bytes = 0;
        self.ppu_register_writes = 0;
        self.audio_queue_drops = 0;
    }
}
