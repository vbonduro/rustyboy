use defmt::info;
use embassy_time::Instant;
use rustyboy_core::cpu::sm83::Sm83;

/// Tracks FPS and (when `perf` is enabled) per-component cycle counts.
/// Call `tick` once per game loop iteration.
pub struct PerfTracker {
    frame_count: u32,
    window_start: Instant,
    #[cfg(feature = "perf")]
    scale_cycles: u64,
    #[cfg(feature = "perf")]
    render_cycles: u64,
}

impl PerfTracker {
    pub fn new() -> Self {
        Self {
            frame_count: 0,
            window_start: Instant::now(),
            #[cfg(feature = "perf")]
            scale_cycles: 0,
            #[cfg(feature = "perf")]
            render_cycles: 0,
        }
    }

    /// Accumulate DWT cycles spent in `scale_to_rgb565` for one frame.
    #[cfg(feature = "perf")]
    pub fn record_scale(&mut self, cycles: u32) {
        self.scale_cycles += cycles as u64;
    }

    /// Accumulate DWT cycles spent in `render_game_only_scaled` for one frame.
    #[cfg(feature = "perf")]
    pub fn record_render(&mut self, cycles: u32) {
        self.render_cycles += cycles as u64;
    }

    pub fn tick(&mut self, cpu: &mut Sm83) {
        self.frame_count += 1;
        if self.frame_count < 60 {
            return;
        }

        let elapsed_us = self.window_start.elapsed().as_micros();
        let fps = (self.frame_count as u64 * 1_000_000) / elapsed_us.max(1);
        info!("fps: {}", fps);

        #[cfg(feature = "perf")]
        {
            let p = cpu.take_perf_profile();
            let cpu_exec = p
                .total
                .wrapping_sub(p.ppu)
                .wrapping_sub(p.timer)
                .wrapping_sub(p.apu);
            let decode = cpu_exec.wrapping_sub(p.mem_read).wrapping_sub(p.mem_write);
            let mem_write_other = p
                .mem_write
                .wrapping_sub(p.mem_write_fast)
                .wrapping_sub(p.mem_write_io)
                .wrapping_sub(p.mem_write_enqueue);
            info!(
                "cycles/60f — total={} ppu={} timer={} apu={} cpu_exec={} (mem_r={} mem_w={} decode={})",
                p.total, p.ppu, p.timer, p.apu, cpu_exec, p.mem_read, p.mem_write, decode
            );
            info!(
                "decode hotspots/60f (nested) — pc_fetch={} rom_pc_fetch={} rom_pc_fetch_idle={} bus_read={} opcode={} cb_prefix={} operand8={}",
                p.pc_fetch,
                p.pc_fetch_rom,
                p.pc_fetch_rom_idle,
                p.bus_read,
                p.opcode_dispatch,
                p.cb_prefix,
                p.operand8
            );
            info!(
                "decode hotspot calls/60f — pc_fetch={} rom_pc_fetch={} rom_pc_fetch_idle={} bus_read={} opcode={} cb_prefix={} operand8={}",
                p.pc_fetch_calls,
                p.pc_fetch_rom_calls,
                p.pc_fetch_rom_idle_calls,
                p.bus_read_calls,
                p.opcode_dispatch_calls,
                p.cb_prefix_calls,
                p.operand8_calls
            );
            info!(
                "mem_write breakdown — fast={} io={} enqueue={} other={} route={}",
                p.mem_write_fast,
                p.mem_write_io,
                p.mem_write_enqueue,
                mem_write_other,
                p.mem_write_route
            );
            info!(
                "mem_write fast breakdown — rom={} eram={} vram={} wram={} oam={} hram={} unmapped={}",
                p.mem_write_fast_rom,
                p.mem_write_fast_eram,
                p.mem_write_fast_vram,
                p.mem_write_fast_wram,
                p.mem_write_fast_oam,
                p.mem_write_fast_hram,
                p.mem_write_fast_unmapped
            );
            info!(
                "mem_write rom breakdown — 0000-1fff={} 2000-3fff={} 4000-5fff={} 6000-7fff={}",
                p.mem_write_fast_rom_0000_1fff,
                p.mem_write_fast_rom_2000_3fff,
                p.mem_write_fast_rom_4000_5fff,
                p.mem_write_fast_rom_6000_7fff
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
                display_total / 250_000,
                self.scale_cycles / 250_000,
                self.render_cycles / 250_000,
                display_total / 250_000 / 60,
            );
            self.scale_cycles = 0;
            self.render_cycles = 0;
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
