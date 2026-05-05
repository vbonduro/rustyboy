// Read the cycle counter. Bare-metal platforms provide the implementation,
// while host builds use a cheap monotonic fallback so tests and coverage can
// link with `perf` enabled.
#[cfg(feature = "perf")]
/// Per-component DWT cycle accumulator. Drained each time `Sm83::take_perf_profile` is called.
/// `cpu` = `total` − `ppu` − `timer` − `apu` (instruction fetch/decode/execute overhead).
#[derive(Default)]
pub struct Sm83PerfProfile {
    pub ppu: u32,
    pub timer: u32,
    pub apu: u32,
    pub total: u32,
    /// Time spent in memory reads (read_fast) inside bus_read, excluding tick overhead.
    pub mem_read: u32,
    /// Time spent in memory writes (write_fast/write_io) inside bus_write, excluding tick overhead.
    pub mem_write: u32,
    /// Time spent in direct `memory.write_fast(...)` calls from bus_write.
    pub mem_write_fast: u32,
    /// Time spent in `write_fast` for ROM/MBC control writes (0x0000-0x7FFF).
    pub mem_write_fast_rom: u32,
    /// Time spent in `write_fast` for ROM control writes at 0x0000-0x1FFF.
    pub mem_write_fast_rom_0000_1fff: u32,
    /// Time spent in `write_fast` for ROM control writes at 0x2000-0x3FFF.
    pub mem_write_fast_rom_2000_3fff: u32,
    /// Time spent in `write_fast` for ROM control writes at 0x4000-0x5FFF.
    pub mem_write_fast_rom_4000_5fff: u32,
    /// Time spent in `write_fast` for ROM control writes at 0x6000-0x7FFF.
    pub mem_write_fast_rom_6000_7fff: u32,
    /// Time spent in `write_fast` for external RAM / cartridge RAM writes (0xA000-0xBFFF).
    pub mem_write_fast_eram: u32,
    /// Time spent in `write_fast` for VRAM writes (0x8000-0x9FFF).
    pub mem_write_fast_vram: u32,
    /// Time spent in `write_fast` for WRAM writes (0xC000-0xDFFF).
    pub mem_write_fast_wram: u32,
    /// Time spent in `write_fast` for OAM writes (0xFE00-0xFE9F).
    pub mem_write_fast_oam: u32,
    /// Time spent in `write_fast` for HRAM writes (0xFF80-0xFFFE).
    pub mem_write_fast_hram: u32,
    /// Time spent in `write_fast` for unmapped / ignored writes.
    pub mem_write_fast_unmapped: u32,
    /// Time spent in direct `memory.write_io(...)` calls from bus_write.
    pub mem_write_io: u32,
    /// Time spent enqueueing `pending_bus_events` from bus_write.
    pub mem_write_enqueue: u32,
    /// Time spent draining and handling queued bus events on the next M-cycle.
    pub mem_write_route: u32,
}

#[cfg(feature = "perf")]
#[derive(Default)]
pub(crate) struct Sm83PerfRecorder {
    profile: Sm83PerfProfile,
}

#[cfg(feature = "perf")]
impl Sm83PerfRecorder {
    #[inline]
    pub(crate) fn take_profile(&mut self) -> Sm83PerfProfile {
        core::mem::take(&mut self.profile)
    }

    #[inline]
    pub(crate) fn record_mem_read(&mut self, dt: u32) {
        self.profile.mem_read = self.profile.mem_read.wrapping_add(dt);
    }

    #[inline]
    pub(crate) fn record_mem_write(&mut self, dt: u32) {
        self.profile.mem_write = self.profile.mem_write.wrapping_add(dt);
    }

    #[inline]
    pub(crate) fn record_mem_write_fast(&mut self, addr: u16, dt: u32) {
        self.profile.mem_write_fast = self.profile.mem_write_fast.wrapping_add(dt);
        match addr {
            0x0000..=0x7FFF => {
                self.profile.mem_write_fast_rom = self.profile.mem_write_fast_rom.wrapping_add(dt);
                match addr {
                    0x0000..=0x1FFF => {
                        self.profile.mem_write_fast_rom_0000_1fff = self
                            .profile
                            .mem_write_fast_rom_0000_1fff
                            .wrapping_add(dt);
                    }
                    0x2000..=0x3FFF => {
                        self.profile.mem_write_fast_rom_2000_3fff = self
                            .profile
                            .mem_write_fast_rom_2000_3fff
                            .wrapping_add(dt);
                    }
                    0x4000..=0x5FFF => {
                        self.profile.mem_write_fast_rom_4000_5fff = self
                            .profile
                            .mem_write_fast_rom_4000_5fff
                            .wrapping_add(dt);
                    }
                    0x6000..=0x7FFF => {
                        self.profile.mem_write_fast_rom_6000_7fff = self
                            .profile
                            .mem_write_fast_rom_6000_7fff
                            .wrapping_add(dt);
                    }
                    _ => {}
                }
            }
            0x8000..=0x9FFF => {
                self.profile.mem_write_fast_vram = self.profile.mem_write_fast_vram.wrapping_add(dt);
            }
            0xA000..=0xBFFF => {
                self.profile.mem_write_fast_eram = self.profile.mem_write_fast_eram.wrapping_add(dt);
            }
            0xC000..=0xDFFF => {
                self.profile.mem_write_fast_wram = self.profile.mem_write_fast_wram.wrapping_add(dt);
            }
            0xFE00..=0xFE9F => {
                self.profile.mem_write_fast_oam = self.profile.mem_write_fast_oam.wrapping_add(dt);
            }
            0xFF80..=0xFFFE => {
                self.profile.mem_write_fast_hram = self.profile.mem_write_fast_hram.wrapping_add(dt);
            }
            _ => {
                self.profile.mem_write_fast_unmapped =
                    self.profile.mem_write_fast_unmapped.wrapping_add(dt);
            }
        }
    }

    #[inline]
    pub(crate) fn record_mem_write_io(&mut self, dt: u32) {
        self.profile.mem_write_io = self.profile.mem_write_io.wrapping_add(dt);
    }

    #[inline]
    pub(crate) fn record_mem_write_enqueue(&mut self, dt: u32) {
        self.profile.mem_write_enqueue = self.profile.mem_write_enqueue.wrapping_add(dt);
    }

    #[inline]
    pub(crate) fn record_mem_write_route(&mut self, dt: u32) {
        self.profile.mem_write_route = self.profile.mem_write_route.wrapping_add(dt);
    }

    #[inline]
    pub(crate) fn record_ppu(&mut self, dt: u32) {
        self.profile.ppu = self.profile.ppu.wrapping_add(dt);
    }

    #[inline]
    pub(crate) fn record_timer(&mut self, dt: u32) {
        self.profile.timer = self.profile.timer.wrapping_add(dt);
    }

    #[inline]
    pub(crate) fn record_apu(&mut self, dt: u32) {
        self.profile.apu = self.profile.apu.wrapping_add(dt);
    }

    #[inline]
    pub(crate) fn record_total(&mut self, dt: u32) {
        self.profile.total = self.profile.total.wrapping_add(dt);
    }
}

#[cfg(target_os = "none")]
extern "C" {
    fn perf_cycle_read() -> u32;
}

#[cfg(target_os = "none")]
#[inline(always)]
pub fn cyccnt() -> u32 {
    unsafe { perf_cycle_read() }
}

#[cfg(not(target_os = "none"))]
#[inline(always)]
pub fn cyccnt() -> u32 {
    use core::sync::atomic::{AtomicU32, Ordering};

    static HOST_CYCLE_COUNTER: AtomicU32 = AtomicU32::new(0);
    HOST_CYCLE_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(all(feature = "perf", test))]
mod tests {
    use super::{Sm83PerfProfile, Sm83PerfRecorder};

    #[test]
    fn record_mem_write_fast_tracks_regions() {
        let mut perf = Sm83PerfRecorder::default();
        perf.record_mem_write_fast(0x2000, 3);
        perf.record_mem_write_fast(0xC123, 5);

        let profile = perf.take_profile();
        assert_eq!(profile.mem_write_fast, 8);
        assert_eq!(profile.mem_write_fast_rom, 3);
        assert_eq!(profile.mem_write_fast_rom_2000_3fff, 3);
        assert_eq!(profile.mem_write_fast_wram, 5);
    }

    #[test]
    fn take_profile_drains_counters() {
        let mut perf = Sm83PerfRecorder::default();
        perf.record_total(7);

        let first = perf.take_profile();
        let second = perf.take_profile();

        assert_eq!(first.total, 7);
        assert_eq!(second.total, Sm83PerfProfile::default().total);
    }
}
