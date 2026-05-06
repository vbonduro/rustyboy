# Performance Roadmap

## Current state — post decode/fetch follow-up

ROM: Tetris, Pico2W @ 250 MHz.

### Representative steady-state perf window (fps = 18)

| Bucket | Cycles / 60 frames | Notes |
|---|---|---|
| total | 734M–737M | Perf build, Tetris |
| decode / dispatch | 383M–385M | Still the largest CPU-side bucket |
| ppu | 225M–226M | Now the next big opaque wall |
| apu | 85M–87M | Stable, but still substantial |
| mem_read | 18M–18.3M | Small compared to fetch wrapper cost |
| mem_write | 3M–4M | Still well under control |
| timer | ~18M | Stable |
| display | ~247 ms / 60f | Scaling still dominates display time |

### Representative decode hotspot window

| Bucket | Cycles / 60 frames | Notes |
|---|---|---|
| pc_fetch | 85M–86M | Full `read_next_pc()` cost |
| rom_pc_fetch | 82M–83M | ROM-only subset of `pc_fetch` |
| rom_pc_fetch_idle | 77M–78M | Common-case idle ROM fetch path |
| rom_pc_fetch_read | 11.2M–11.3M | Raw ROM-byte read inside ROM fetch |
| bus_read | 27M–28M | Generic non-wave `bus_read()` |
| opcode | 16M–18M | Opcode table lookup is not the wall |

### Summary

- The decode/fetch work paid off.
- `pc_fetch` and `rom_pc_fetch` are down enough that they no longer look like
  the best next target.
- The next optimization pass should move to the still-large opaque `ppu` bucket,
  not back to opcode dispatch or another micro-tuning pass on ROM fetch.

---

## Landed — Decode / ROM fetch hot-path work

The Pico now has:

- finer-grained decode perf counters around `read_next_pc`, ROM-only fetches,
  generic `bus_read`, opcode dispatch, `0xCB` handling, and operand helpers
- a ROM `PC` fetch fast path
- an idle M-cycle fast path for common instruction fetches
- cached cartridge ROM windows plus full-window shortcuts for hot ROM reads
- cheaper M-cycle cycle-counter accounting by batching `cycle_counter` updates
  per instruction instead of touching the `u64` total on every M-cycle

Measured result on Tetris, relative to the first post-instrumentation decode
baseline:

- `fps` improved from 15 to 18
- `total` dropped from ~900M–906M to ~734M–737M cycles / 60f
- `decode` dropped from ~535M–539M to ~383M–385M
- `pc_fetch` dropped from ~264M–266M to ~85M–86M
- generic `bus_read` dropped from ~238M–239M to ~27M–28M

This line of work clearly paid off. It is no longer the best place to spend the
next iteration.

---

## Priority 1 — PPU residual split

**Expected impact: highest next CPU-side upside**

`ppu` is now about `225M–226M` cycles / 60f, but the current PPU counters only
explain a minority of that bucket. The visible breakdown (`render_bg`,
`render_window`, `render_sprites`, `build_stat`) leaves a large residual, so the
next job is observability.

### What to do

1. Add perf counters around the PPU mode/state-machine shell in `ppu.rs`,
   especially:
   - OAM-scan stepping
   - pixel-transfer shell outside the already-counted render helpers
   - HBlank / VBlank transition handling
   - LY / dot bookkeeping
   - STAT / VBlank interrupt generation and related writeback work
2. Surface those counters in Pico RTT perf logging next to the current PPU
   breakdown.
3. Re-run the Pico `perf` build on hardware and capture a fresh 60-frame window.
4. Optimize the dominant residual PPU sub-bucket before touching fetch again.

### Guardrails

- Do not reintroduce the old `build_stat` cache complexity unless new
  measurements prove it is worth it.
- Keep the current fetch path stable while measuring PPU; fetch is no longer the
  least-understood hotspot.

### Success criteria

- Most of the `ppu` bucket is explained by sub-counters rather than residual
  time.
- The next PPU optimization target is obvious from hardware data.

---

## Priority 2 — APU residual split

**Expected impact: moderate; only after PPU is understood**

`apu` is still about `85M–87M` cycles / 60f. The current APU counters
(`frame_seq`, `pulse`, `wave`, `noise`, `mix`) explain only part of that cost.
If PPU work stops paying off, APU is the next profiling target.

### What to do

1. Add APU-side counters around the remaining shell work that is not currently
   attributed.
2. Re-profile on hardware.
3. Only then decide whether more batching or mixer-side work is justified.

---

## Priority 3 — Display scaler / Core 1 work

**Expected impact: useful, but no longer the first move**

Display scaling is still about `247 ms / 60f`, but SPI transfer is already
overlapped with emulation. The clean multicore candidate is framebuffer scaling,
not CPU/PPU/APU execution.

### What to do

1. Leave display scaling alone while CPU-side profiling is still yielding wins.
2. If emulator-core gains flatten out, revisit a scaler worker on core 1.
3. Do not try to split SM83/PPU/APU timing across cores.

---

## Deprioritized for now

- More `pc_fetch` / `rom_pc_fetch` micro-optimizations
- Another opcode-dispatch rewrite
- Generic `bus_read()` tuning as a primary target
- Reviving the old `build_stat` cache experiment

The reason is simple: those areas are no longer the largest or least-understood
costs in the profile.

---

## Immediate next move

1. Add PPU-side perf splits around the large residual inside the current `ppu`
   bucket.
2. Re-run the Pico `perf` build and capture a fresh 60-frame hardware window.
3. Pick the top PPU sub-bucket and optimize that next.
