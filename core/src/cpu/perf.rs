// Read the cycle counter. Bare-metal platforms provide the implementation,
// while host builds use a cheap monotonic fallback so tests and coverage can
// link with `perf` enabled.
use crate::memory::map::{
    EXT_RAM_BASE, EXT_RAM_END, OAM_BASE, OAM_END, ROM_BANKED_END, VRAM_BASE, VRAM_END, WRAM_BASE,
    WRAM_END,
};
#[cfg(feature = "perf")]
/// Per-component DWT cycle accumulator. Drained each time `Sm83::take_perf_profile` is called.
/// `cpu` = `total` − `ppu` − `timer` − `apu` (instruction fetch/decode/execute overhead).
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
    /// Nested decode hotspot: time spent in `read_next_pc`, excluding nested PPU/timer/APU work.
    pub pc_fetch: u32,
    /// Number of `read_next_pc` calls.
    pub pc_fetch_calls: u32,
    /// Nested decode hotspot: `read_next_pc` time for PC reads in cartridge ROM space.
    pub pc_fetch_rom: u32,
    /// Number of `read_next_pc` calls in `0x0000..=0x7FFF`.
    pub pc_fetch_rom_calls: u32,
    /// Nested decode hotspot: post-read `read_next_pc` wrap-up work
    /// (primarily `PC += 1`) after the underlying fetch completes.
    pub pc_fetch_wrapper: u32,
    /// Number of `read_next_pc` wrap-up measurements.
    pub pc_fetch_wrapper_calls: u32,
    /// Nested decode hotspot: ROM `read_next_pc` common-case fast path
    /// (no queued bus events, DMA, active serial transfer, or cartridge RTC).
    pub pc_fetch_rom_idle: u32,
    /// Number of ROM `read_next_pc` calls that used the common-case fast path.
    pub pc_fetch_rom_idle_calls: u32,
    /// Nested decode hotspot: raw direct ROM window read time inside
    /// cartridge-ROM `PC` fetches.
    pub pc_fetch_rom_read: u32,
    /// Number of raw direct ROM window reads in cartridge-ROM `PC` fetches.
    pub pc_fetch_rom_read_calls: u32,
    /// Nested decode hotspot: time spent in generic non-wave `bus_read`, excluding nested PPU/timer/APU work.
    pub bus_read: u32,
    /// Number of generic non-wave `bus_read` calls.
    pub bus_read_calls: u32,
    /// Nested decode hotspot: time spent in opcode-table lookup (`get` / `get_cb`).
    pub opcode_dispatch: u32,
    /// Number of opcode-table lookups.
    pub opcode_dispatch_calls: u32,
    /// Per-opcode execution-body cycles for non-`0xCB` instructions.
    pub opcode_exec_cycles: [u32; 256],
    /// Per-opcode execution-body call counts for non-`0xCB` instructions.
    pub opcode_exec_calls: [u32; 256],
    /// Nested decode hotspot: time spent in the `0xCB` prefix path, including second-byte fetch.
    pub cb_prefix: u32,
    /// Number of `0xCB` prefix dispatches.
    pub cb_prefix_calls: u32,
    /// Per-opcode execution-body cycles for decoded `0xCB` instructions.
    pub cb_opcode_exec_cycles: [u32; 256],
    /// Per-opcode execution-body call counts for decoded `0xCB` instructions.
    pub cb_opcode_exec_calls: [u32; 256],
    /// Nested decode hotspot: time spent executing decoded `0xCB` operations,
    /// excluding the second-byte fetch/dispatch work recorded in `cb_prefix`.
    pub cb_exec: u32,
    /// Number of decoded `0xCB` executions.
    pub cb_exec_calls: u32,
    /// `0xCB` execution time for register targets.
    pub cb_exec_reg: u32,
    /// Number of register-target `0xCB` executions.
    pub cb_exec_reg_calls: u32,
    /// `0xCB` execution time for `(HL)` memory targets.
    pub cb_exec_hlmem: u32,
    /// Number of `(HL)`-target `0xCB` executions.
    pub cb_exec_hlmem_calls: u32,
    /// Nested decode hotspot: time spent in `get_8bit_operand`, excluding nested PPU/timer/APU work.
    pub operand8: u32,
    /// Number of `get_8bit_operand` calls.
    pub operand8_calls: u32,
    /// `get_8bit_operand` time for register operands.
    pub operand8_reg: u32,
    /// Number of register `get_8bit_operand` calls.
    pub operand8_reg_calls: u32,
    /// `get_8bit_operand` time for immediate operands (`d8`, signed `r8`).
    pub operand8_imm: u32,
    /// Number of immediate `get_8bit_operand` calls.
    pub operand8_imm_calls: u32,
    /// `get_8bit_operand` time for memory-backed operands.
    pub operand8_mem: u32,
    /// Number of memory-backed `get_8bit_operand` calls.
    pub operand8_mem_calls: u32,
}

#[cfg(feature = "perf")]
impl Default for Sm83PerfProfile {
    fn default() -> Self {
        Self {
            ppu: 0,
            timer: 0,
            apu: 0,
            total: 0,
            mem_read: 0,
            mem_write: 0,
            mem_write_fast: 0,
            mem_write_fast_rom: 0,
            mem_write_fast_rom_0000_1fff: 0,
            mem_write_fast_rom_2000_3fff: 0,
            mem_write_fast_rom_4000_5fff: 0,
            mem_write_fast_rom_6000_7fff: 0,
            mem_write_fast_eram: 0,
            mem_write_fast_vram: 0,
            mem_write_fast_wram: 0,
            mem_write_fast_oam: 0,
            mem_write_fast_hram: 0,
            mem_write_fast_unmapped: 0,
            mem_write_io: 0,
            mem_write_enqueue: 0,
            mem_write_route: 0,
            pc_fetch: 0,
            pc_fetch_calls: 0,
            pc_fetch_rom: 0,
            pc_fetch_rom_calls: 0,
            pc_fetch_wrapper: 0,
            pc_fetch_wrapper_calls: 0,
            pc_fetch_rom_idle: 0,
            pc_fetch_rom_idle_calls: 0,
            pc_fetch_rom_read: 0,
            pc_fetch_rom_read_calls: 0,
            bus_read: 0,
            bus_read_calls: 0,
            opcode_dispatch: 0,
            opcode_dispatch_calls: 0,
            opcode_exec_cycles: [0; 256],
            opcode_exec_calls: [0; 256],
            cb_prefix: 0,
            cb_prefix_calls: 0,
            cb_opcode_exec_cycles: [0; 256],
            cb_opcode_exec_calls: [0; 256],
            cb_exec: 0,
            cb_exec_calls: 0,
            cb_exec_reg: 0,
            cb_exec_reg_calls: 0,
            cb_exec_hlmem: 0,
            cb_exec_hlmem_calls: 0,
            operand8: 0,
            operand8_calls: 0,
            operand8_reg: 0,
            operand8_reg_calls: 0,
            operand8_imm: 0,
            operand8_imm_calls: 0,
            operand8_mem: 0,
            operand8_mem_calls: 0,
        }
    }
}

#[cfg(feature = "perf")]
#[derive(Default)]
pub(crate) struct Sm83PerfRecorder {
    profile: Sm83PerfProfile,
}

#[cfg(feature = "perf")]
#[derive(Clone, Copy)]
pub(crate) struct NestedPerfSnapshot {
    ppu: u32,
    timer: u32,
    apu: u32,
}

#[cfg(feature = "perf")]
impl Sm83PerfRecorder {
    #[inline]
    pub(crate) fn take_profile(&mut self) -> Sm83PerfProfile {
        core::mem::take(&mut self.profile)
    }

    #[inline]
    pub(crate) fn nested_snapshot(&self) -> NestedPerfSnapshot {
        NestedPerfSnapshot {
            ppu: self.profile.ppu,
            timer: self.profile.timer,
            apu: self.profile.apu,
        }
    }

    #[inline]
    pub(crate) fn nested_cycles_since(&self, snapshot: NestedPerfSnapshot) -> u32 {
        self.profile
            .ppu
            .wrapping_sub(snapshot.ppu)
            .wrapping_add(self.profile.timer.wrapping_sub(snapshot.timer))
            .wrapping_add(self.profile.apu.wrapping_sub(snapshot.apu))
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
            0x0000..=ROM_BANKED_END => {
                self.profile.mem_write_fast_rom = self.profile.mem_write_fast_rom.wrapping_add(dt);
                match addr {
                    0x0000..=0x1FFF => {
                        self.profile.mem_write_fast_rom_0000_1fff =
                            self.profile.mem_write_fast_rom_0000_1fff.wrapping_add(dt);
                    }
                    0x2000..=0x3FFF => {
                        self.profile.mem_write_fast_rom_2000_3fff =
                            self.profile.mem_write_fast_rom_2000_3fff.wrapping_add(dt);
                    }
                    0x4000..=0x5FFF => {
                        self.profile.mem_write_fast_rom_4000_5fff =
                            self.profile.mem_write_fast_rom_4000_5fff.wrapping_add(dt);
                    }
                    0x6000..=ROM_BANKED_END => {
                        self.profile.mem_write_fast_rom_6000_7fff =
                            self.profile.mem_write_fast_rom_6000_7fff.wrapping_add(dt);
                    }
                    _ => {}
                }
            }
            VRAM_BASE..=VRAM_END => {
                self.profile.mem_write_fast_vram =
                    self.profile.mem_write_fast_vram.wrapping_add(dt);
            }
            EXT_RAM_BASE..=EXT_RAM_END => {
                self.profile.mem_write_fast_eram =
                    self.profile.mem_write_fast_eram.wrapping_add(dt);
            }
            WRAM_BASE..=WRAM_END => {
                self.profile.mem_write_fast_wram =
                    self.profile.mem_write_fast_wram.wrapping_add(dt);
            }
            OAM_BASE..=OAM_END => {
                self.profile.mem_write_fast_oam = self.profile.mem_write_fast_oam.wrapping_add(dt);
            }
            0xFF80..=0xFFFE => {
                self.profile.mem_write_fast_hram =
                    self.profile.mem_write_fast_hram.wrapping_add(dt);
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
    pub(crate) fn record_pc_fetch(&mut self, addr: u16, dt: u32) {
        self.profile.pc_fetch = self.profile.pc_fetch.wrapping_add(dt);
        self.profile.pc_fetch_calls = self.profile.pc_fetch_calls.wrapping_add(1);
        if addr <= ROM_BANKED_END {
            self.profile.pc_fetch_rom = self.profile.pc_fetch_rom.wrapping_add(dt);
            self.profile.pc_fetch_rom_calls = self.profile.pc_fetch_rom_calls.wrapping_add(1);
        }
    }

    #[inline]
    pub(crate) fn record_pc_fetch_wrapper(&mut self, dt: u32) {
        self.profile.pc_fetch_wrapper = self.profile.pc_fetch_wrapper.wrapping_add(dt);
        self.profile.pc_fetch_wrapper_calls = self.profile.pc_fetch_wrapper_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_pc_fetch_rom_idle(&mut self, dt: u32) {
        self.profile.pc_fetch_rom_idle = self.profile.pc_fetch_rom_idle.wrapping_add(dt);
        self.profile.pc_fetch_rom_idle_calls = self.profile.pc_fetch_rom_idle_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_pc_fetch_rom_read(&mut self, dt: u32) {
        self.profile.pc_fetch_rom_read = self.profile.pc_fetch_rom_read.wrapping_add(dt);
        self.profile.pc_fetch_rom_read_calls = self.profile.pc_fetch_rom_read_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_bus_read(&mut self, dt: u32) {
        self.profile.bus_read = self.profile.bus_read.wrapping_add(dt);
        self.profile.bus_read_calls = self.profile.bus_read_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_opcode_dispatch(&mut self, dt: u32) {
        self.profile.opcode_dispatch = self.profile.opcode_dispatch.wrapping_add(dt);
        self.profile.opcode_dispatch_calls = self.profile.opcode_dispatch_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_opcode_exec(&mut self, opcode: u8, dt: u32) {
        let slot = opcode as usize;
        self.profile.opcode_exec_cycles[slot] =
            self.profile.opcode_exec_cycles[slot].wrapping_add(dt);
        self.profile.opcode_exec_calls[slot] = self.profile.opcode_exec_calls[slot].wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_cb_prefix(&mut self, dt: u32) {
        self.profile.cb_prefix = self.profile.cb_prefix.wrapping_add(dt);
        self.profile.cb_prefix_calls = self.profile.cb_prefix_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_cb_opcode_exec(&mut self, opcode: u8, dt: u32) {
        let slot = opcode as usize;
        self.profile.cb_opcode_exec_cycles[slot] =
            self.profile.cb_opcode_exec_cycles[slot].wrapping_add(dt);
        self.profile.cb_opcode_exec_calls[slot] =
            self.profile.cb_opcode_exec_calls[slot].wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_cb_exec_reg(&mut self, dt: u32) {
        self.profile.cb_exec = self.profile.cb_exec.wrapping_add(dt);
        self.profile.cb_exec_calls = self.profile.cb_exec_calls.wrapping_add(1);
        self.profile.cb_exec_reg = self.profile.cb_exec_reg.wrapping_add(dt);
        self.profile.cb_exec_reg_calls = self.profile.cb_exec_reg_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_cb_exec_hlmem(&mut self, dt: u32) {
        self.profile.cb_exec = self.profile.cb_exec.wrapping_add(dt);
        self.profile.cb_exec_calls = self.profile.cb_exec_calls.wrapping_add(1);
        self.profile.cb_exec_hlmem = self.profile.cb_exec_hlmem.wrapping_add(dt);
        self.profile.cb_exec_hlmem_calls = self.profile.cb_exec_hlmem_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_operand8_reg(&mut self, dt: u32) {
        self.profile.operand8 = self.profile.operand8.wrapping_add(dt);
        self.profile.operand8_calls = self.profile.operand8_calls.wrapping_add(1);
        self.profile.operand8_reg = self.profile.operand8_reg.wrapping_add(dt);
        self.profile.operand8_reg_calls = self.profile.operand8_reg_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_operand8_imm(&mut self, dt: u32) {
        self.profile.operand8 = self.profile.operand8.wrapping_add(dt);
        self.profile.operand8_calls = self.profile.operand8_calls.wrapping_add(1);
        self.profile.operand8_imm = self.profile.operand8_imm.wrapping_add(dt);
        self.profile.operand8_imm_calls = self.profile.operand8_imm_calls.wrapping_add(1);
    }

    #[inline]
    pub(crate) fn record_operand8_mem(&mut self, dt: u32) {
        self.profile.operand8 = self.profile.operand8.wrapping_add(dt);
        self.profile.operand8_calls = self.profile.operand8_calls.wrapping_add(1);
        self.profile.operand8_mem = self.profile.operand8_mem.wrapping_add(dt);
        self.profile.operand8_mem_calls = self.profile.operand8_mem_calls.wrapping_add(1);
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn record_ppu(&mut self, dt: u32) {
        self.profile.ppu = self.profile.ppu.wrapping_add(dt);
    }

    #[allow(dead_code)]
    #[inline]
    pub(crate) fn record_timer(&mut self, dt: u32) {
        self.profile.timer = self.profile.timer.wrapping_add(dt);
    }

    #[allow(dead_code)]
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
    fn record_decode_hotspots_track_cycles_and_calls() {
        let mut perf = Sm83PerfRecorder::default();
        perf.record_pc_fetch(0x1234, 3);
        perf.record_pc_fetch(0xC123, 5);
        perf.record_pc_fetch_wrapper(2);
        perf.record_pc_fetch_rom_idle(7);
        perf.record_pc_fetch_rom_read(11);
        perf.record_bus_read(7);
        perf.record_opcode_dispatch(11);
        perf.record_opcode_exec(0x12, 17);
        perf.record_opcode_exec(0x12, 19);
        perf.record_cb_prefix(13);
        perf.record_cb_opcode_exec(0x86, 23);
        perf.record_cb_exec_reg(19);
        perf.record_cb_exec_hlmem(23);
        perf.record_operand8_reg(17);
        perf.record_operand8_imm(29);
        perf.record_operand8_mem(31);

        let profile = perf.take_profile();
        assert_eq!(profile.pc_fetch, 8);
        assert_eq!(profile.pc_fetch_calls, 2);
        assert_eq!(profile.pc_fetch_rom, 3);
        assert_eq!(profile.pc_fetch_rom_calls, 1);
        assert_eq!(profile.pc_fetch_wrapper, 2);
        assert_eq!(profile.pc_fetch_wrapper_calls, 1);
        assert_eq!(profile.pc_fetch_rom_idle, 7);
        assert_eq!(profile.pc_fetch_rom_idle_calls, 1);
        assert_eq!(profile.pc_fetch_rom_read, 11);
        assert_eq!(profile.pc_fetch_rom_read_calls, 1);
        assert_eq!(profile.bus_read, 7);
        assert_eq!(profile.bus_read_calls, 1);
        assert_eq!(profile.opcode_dispatch, 11);
        assert_eq!(profile.opcode_dispatch_calls, 1);
        assert_eq!(profile.opcode_exec_cycles[0x12], 36);
        assert_eq!(profile.opcode_exec_calls[0x12], 2);
        assert_eq!(profile.cb_prefix, 13);
        assert_eq!(profile.cb_prefix_calls, 1);
        assert_eq!(profile.cb_opcode_exec_cycles[0x86], 23);
        assert_eq!(profile.cb_opcode_exec_calls[0x86], 1);
        assert_eq!(profile.cb_exec, 42);
        assert_eq!(profile.cb_exec_calls, 2);
        assert_eq!(profile.cb_exec_reg, 19);
        assert_eq!(profile.cb_exec_reg_calls, 1);
        assert_eq!(profile.cb_exec_hlmem, 23);
        assert_eq!(profile.cb_exec_hlmem_calls, 1);
        assert_eq!(profile.operand8, 77);
        assert_eq!(profile.operand8_calls, 3);
        assert_eq!(profile.operand8_reg, 17);
        assert_eq!(profile.operand8_reg_calls, 1);
        assert_eq!(profile.operand8_imm, 29);
        assert_eq!(profile.operand8_imm_calls, 1);
        assert_eq!(profile.operand8_mem, 31);
        assert_eq!(profile.operand8_mem_calls, 1);
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
