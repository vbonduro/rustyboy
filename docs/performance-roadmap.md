# Performance Roadmap

## Current state — post display/APU work and direct-XIP cartridge path

ROM: Tetris, Pico2W @ 250 MHz.

### Representative steady-state perf window (fps = 16)

| Bucket | Cycles / 60 frames | Notes |
|---|---|---|
| total | 825M–838M | Perf build, Tetris |
| decode / dispatch | 461M–466M | Now the clearest CPU wall |
| ppu | 224M–226M | Still substantial but secondary |
| apu | 83M–90M | APU batching landed successfully |
| mem_read | 34M–35M | Slightly higher after direct XIP, still small |
| mem_write | 3M–4M | Bank-switch copy cost is gone |
| timer | ~18M | Stable |

### Display status

- `fill` is now ~1 ms / 60f after the async transfer work.
- `scale` is still ~224 ms / 60f, but display is no longer the main ceiling.
- The performance story is now mostly CPU-side again.

---

## Landed — Direct-XIP ROM path for staged flash

The Pico now uses a direct-XIP cartridge path instead of refilling a 16 KiB SRAM
bank cache on every ROM bank switch.

Measured result on Tetris:

- `fps` improved from 13–14 to 16
- `total` dropped from ~976M–1009M to ~825M–838M cycles / 60f
- `mem_write` dropped from ~140M–169M to ~3M–4M cycles / 60f
- Cartridge perf counters now show tiny ROM control-write cost and zero bank-reload work

This change paid off immediately and should stay.

---

## Priority 1 — Decode / ROM fetch fast path

**Expected impact: still likely the next broad CPU win after the cartridge path**

Now that the cartridge bank-switch path is addressed, `decode` remains the biggest
bucket by a wide margin.

### What to do

1. Revisit a ROM opcode-fetch fast path after the cartridge work lands.
2. Prefer this over instruction-level tick batching unless timing evidence says
   batching is required; the ROM fast path is less invasive.

---

## Priority 2 — PPU work that is actually hot

**Expected impact: moderate; only worth revisiting with a simpler design**

The attempted `build_stat` cache did not pay off and added invalidation complexity.
Keep the simple implementation unless we move to a more event-driven STAT update
model later.

### What to do

1. Leave `build_stat` alone for now.
2. If PPU optimization comes back onto the critical path, look for a transition-based
   design rather than another tiny hot-path cache.

---

## Priority 3 — Display scaler cleanup

**Expected impact: useful but no longer urgent**

The remaining display cost is mostly scaling (`~224 ms / 60f`). That matters, but
it is smaller than the current CPU hotspots.

### What to do

Look at scaler-specific wins only after the cartridge and decode work, or if a
non-invasive scaler cleanup becomes obvious.

---

## Priority 4 — Deprioritized experiments

- `pending_bus_events` fixed-size array: measured cost is noise compared to
  `write_fast` and should not be a near-term priority.
- `PPU build_stat` cache: tried and reverted; complexity was not justified by the
  measurements.

---

## Summary table

| # | Change | Est. savings | Effort | Unblocks |
|---|---|---|---|---|
| 1 | ROM fetch / decode fast path | large | Medium | — |
| 2 | Event-driven PPU STAT work | moderate | Medium | — |
| 3 | Display scaler cleanup | small/moderate | Medium | — |
| 4 | Fixed `pending_bus_events` / tiny caches | small | Small | — |
