use defmt::info;
use embassy_time::Instant;
use rustyboy_pico2w::multicore::PicoGameBoy;

#[cfg(all(feature = "perf", feature = "oc-266"))]
const CYCLES_PER_MS: u64 = 266_000;
#[cfg(all(feature = "perf", not(feature = "oc-266")))]
const CYCLES_PER_MS: u64 = 250_000;

/// Tracks FPS and (when `perf` is enabled) per-component cycle counts.
/// Call `tick` once per game loop iteration.
pub struct PerfTracker {
    frame_count: u32,
    window_start: Instant,
    #[cfg(feature = "perf")]
    scale_cycles: u64,
    #[cfg(feature = "perf")]
    emulate_cycles: u64,
    #[cfg(feature = "perf")]
    render_cycles: u64,
    #[cfg(feature = "perf")]
    audio_wait_cycles: u64,
}

impl PerfTracker {
    pub fn new() -> Self {
        Self {
            frame_count: 0,
            window_start: Instant::now(),
            #[cfg(feature = "perf")]
            scale_cycles: 0,
            #[cfg(feature = "perf")]
            emulate_cycles: 0,
            #[cfg(feature = "perf")]
            render_cycles: 0,
            #[cfg(feature = "perf")]
            audio_wait_cycles: 0,
        }
    }

    /// Accumulate DWT cycles spent in `scale_to_rgb565` for one frame.
    #[cfg(feature = "perf")]
    pub fn record_scale(&mut self, cycles: u32) {
        self.scale_cycles += cycles as u64;
    }

    #[cfg(feature = "perf")]
    pub fn record_emulate(&mut self, cycles: u32) {
        self.emulate_cycles += cycles as u64;
    }

    /// Accumulate DWT cycles spent in `render_game_only_scaled` for one frame.
    #[cfg(feature = "perf")]
    pub fn record_render(&mut self, cycles: u32) {
        self.render_cycles += cycles as u64;
    }

    #[cfg(feature = "perf")]
    pub fn record_audio_wait(&mut self, cycles: u32) {
        self.audio_wait_cycles += cycles as u64;
    }

    pub fn tick(&mut self, cpu: &mut PicoGameBoy) {
        self.frame_count += 1;
        if self.frame_count < 60 {
            return;
        }

        let elapsed_us = self.window_start.elapsed().as_micros();
        let fps = (self.frame_count as u64 * 1_000_000) / elapsed_us.max(1);
        info!("fps: {}", fps);

        #[cfg(feature = "perf")]
        {
            let transport = cpu.take_transport_profile();
            info!(
                "core1 transport/60f — enq={} spins={} apu_cmds={} ppu_adv={} frame_pub={} vram_bytes={} oam_bytes={} regs={} audio_drops={}",
                transport.command_enqueues,
                transport.command_queue_spins,
                transport.apu_commands,
                transport.ppu_advance_commands,
                transport.frame_publishes,
                transport.ppu_vram_bytes,
                transport.ppu_oam_bytes,
                transport.ppu_register_writes,
                transport.audio_queue_drops,
            );

            let fp = cpu.take_frontend_perf_profile();
            info!(
                "frontend/60f — total={}ms cpu={}ms route={}ms ppu_timing={}ms ppu_sync={}ms timer={}ms apu_state={}ms apu_send={}ms rtc={}ms serial={}ms dma={}ms steps={} events={} scanlines={} dma_bytes={}",
                fp.step_total as u64 / CYCLES_PER_MS,
                fp.cpu_step as u64 / CYCLES_PER_MS,
                fp.route_bus_events as u64 / CYCLES_PER_MS,
                fp.ppu_timing as u64 / CYCLES_PER_MS,
                fp.ppu_sync as u64 / CYCLES_PER_MS,
                fp.timer as u64 / CYCLES_PER_MS,
                fp.apu_state as u64 / CYCLES_PER_MS,
                fp.apu_send as u64 / CYCLES_PER_MS,
                fp.rtc as u64 / CYCLES_PER_MS,
                fp.serial as u64 / CYCLES_PER_MS,
                fp.dma as u64 / CYCLES_PER_MS,
                fp.steps,
                fp.bus_events,
                fp.render_scanlines,
                fp.dma_bytes,
            );

            let p = cpu.take_perf_profile();
            let mem_write_other = p
                .mem_write
                .wrapping_sub(p.mem_write_fast)
                .wrapping_sub(p.mem_write_io)
                .wrapping_sub(p.mem_write_enqueue);
            info!(
                "legacy sm83/60f — total={} mem_r={} mem_w={} route={} fast={} io={} enqueue={} other={}",
                p.total,
                p.mem_read,
                p.mem_write,
                p.mem_write_route,
                p.mem_write_fast,
                p.mem_write_io,
                p.mem_write_enqueue,
                mem_write_other,
            );

            let pp = cpu.take_ppu_perf_profile();
            info!(
                "ppu breakdown — bg={} window={} sprites={} stat={}",
                pp.render_bg, pp.render_window, pp.render_sprites, pp.build_stat
            );

            let ap = cpu.take_apu_perf_profile();
            info!(
                "apu breakdown — frame_seq={} pulse={} wave={} noise={} mix={}",
                ap.frame_seq, ap.pulse, ap.wave, ap.noise, ap.mix
            );

            let cp = cpu.take_cartridge_perf_profile();
            info!(
                "cart breakdown — rom={} ram={} control={} sync={} sync_calls={} bank0={} bank0_calls={} banked={} banked_calls={}",
                cp.write_rom,
                cp.write_ram,
                cp.control_write,
                cp.sync_caches,
                cp.sync_caches_calls,
                cp.read_bank_fixed,
                cp.read_bank_fixed_calls,
                cp.read_bank_switchable,
                cp.read_bank_switchable_calls
            );

            // At 250 MHz, divide cycles by 250_000 to get milliseconds.
            let display_total = self.scale_cycles + self.render_cycles;
            info!(
                "display/60f — {}ms total (scale={}ms fill={}ms) avg {}ms/frame",
                display_total / CYCLES_PER_MS,
                self.scale_cycles / CYCLES_PER_MS,
                self.render_cycles / CYCLES_PER_MS,
                display_total / CYCLES_PER_MS / 60,
            );
            info!(
                "loop/60f — emulate={}ms audio_wait={}ms avg emulate={}ms/frame",
                self.emulate_cycles / CYCLES_PER_MS,
                self.audio_wait_cycles / CYCLES_PER_MS,
                self.emulate_cycles / CYCLES_PER_MS / 60,
            );
            self.scale_cycles = 0;
            self.emulate_cycles = 0;
            self.render_cycles = 0;
            self.audio_wait_cycles = 0;
        }

        // Suppress unused-variable warning when only `fps` (not `perf`) is enabled.
        let _ = cpu;

        self.frame_count = 0;
        self.window_start = Instant::now();
    }
}

/// Enable the DWT cycle counter. Must be called once before `perf_cycle_read` is useful.
#[cfg(feature = "perf")]
pub fn init_dwt() {
    unsafe {
        let demcr = 0xE000_EDFCu32 as *mut u32;
        demcr.write_volatile(demcr.read_volatile() | (1 << 24));
        (0xE000_1004u32 as *mut u32).write_volatile(0);
        let ctrl = 0xE000_1000u32 as *mut u32;
        ctrl.write_volatile(ctrl.read_volatile() | 1);
    }
}

/// Fulfils the `extern "C" fn perf_cycle_read()` contract declared in rustyboy-core.
#[cfg(feature = "perf")]
#[no_mangle]
pub extern "C" fn perf_cycle_read() -> u32 {
    unsafe { (0xE000_1004u32 as *const u32).read_volatile() }
}
