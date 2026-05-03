# Performance Roadmap

## Current state — post dispatch-refactor baseline

ROM: Tetris, Pico2W @ 250 MHz.

### Wall-clock breakdown (fps = 9)

| Bucket | Time / 60 frames | % of wall clock |
|---|---|---|
| Emulation (DWT) | 3.52 s (879M cycles) | 53% |
| Display + other | 3.15 s | 47% |
| **Total** | **6.67 s** | **100%** |

Target: 1.00 s / 60 frames (60 fps).  
Required speedup: **6.7×**.

### Emulation sub-breakdown (879M cycles)

| Component | Cycles | % of emulation |
|---|---|---|
| decode / dispatch | 406M | 46% |
| apu | 205M | 23% |
| ppu | 160M | 18% |
| mem_write | 54M | 6% |
| mem_read | 29M | 3% |
| timer | 26M | 3% |

### Hard ceiling without touching display

Even if emulation cost were zero, the display/other overhead alone gives:

60 ÷ 3.15 s = **~19 fps maximum**

Both tracks (emulation and display) must be attacked together to reach 60 fps.

---

## Priority 1 — Async display transfer

**Expected impact: largest single lever, potentially 2–3× fps gain**

The display currently blocks the CPU for ~13 ms per frame (240×216 × 2 bytes over
62.5 MHz SPI), plus unknown software-scaling overhead. The game loop sits idle
waiting for this transfer to complete.

### What to do

1. **Profile the display path** — add DWT instrumentation around `render_game_only`
   to separate the SPI transfer time from the software scaler (1.5× pixel iterator).

2. **DMA-backed SPI transfer** — `embassy-rp` supports async DMA SPI writes. Start
   the transfer at VBlank, let emulation run for the next frame concurrently, await
   completion only if the next VBlank arrives before the DMA finishes. This hides
   the transfer latency almost entirely behind emulation work.

3. **Pre-scale into a u16 framebuffer** — instead of running the 1.5× scale iterator
   inside the SPI callback, maintain a `[u16; 240 * 216]` front buffer that is updated
   once per VBlank. The DMA transfer then reads directly from this buffer with no
   per-pixel CPU work during the transfer.

---

## Priority 2 — APU cycle batching

**Expected impact: ~80–100M cycles saved (~9–11% of total emulation)**

`advance_apu` is called on every M-cycle (~1.05M times/second). The APU already
implements skip-ahead arithmetic for frequency timers, so it handles large cycle
batches correctly.

### What to do

Add a `pending_apu_cycles: u16` accumulator to `Sm83`. Increment it instead of
calling `apu.tick()` on every `tick_cycle`. Flush via `apu.tick(pending, ...)` at:
- End of each instruction (in `tick_impl`, before the interrupt check)
- Before any APU register write (the `tick_cycle_to_t3` path already handles
  T-cycle precision for writes — flush before entering that path)

This reduces APU tick call frequency from ~1M/s to ~350K/s (once per instruction
on average), cutting function-call overhead by ~3× while the skip-ahead arithmetic
absorbs the larger cycle batches.

---

## Priority 3 — PPU `build_stat` cache

**Expected impact: ~20M cycles saved (~2% of total emulation)**

`build_stat` recomputes the STAT register and STAT interrupt edge on every M-cycle
(~1M times/second). The result only changes when LY changes (154×/frame), the PPU
mode changes (~4×/scanline), or the game writes to STAT.

### What to do

Add `stat_cache: u8` and `stat_line_cache: bool` fields to `PpuPeripheral`.  
Set a `stat_dirty: bool` flag when:
- `self.ly` changes
- `self.mode` changes  
- A new `input.stat` differs from the previous call's stat

In `build_stat`, return the cached value if `!stat_dirty`. Reset the flag after
recomputing. This drops 99%+ of `build_stat` calls to a single branch.

---

## Priority 4 — `pending_bus_events` fixed-size array

**Expected impact: small, reduces mem_write overhead**

`pending_bus_events: Vec<BusEvent>` does a bounds-check + capacity-check on every
I/O write. Since at most one I/O write can be pending per M-cycle before
`route_bus_events` drains it, the queue never exceeds a handful of entries.

### What to do

Replace `Vec<BusEvent>` with `[BusEvent; 4]` + `len: usize`. Eliminates the heap
indirection and capacity check on the write hot path.

---

## Priority 5 — `decode` remainder (longer term)

After the above changes, decode/dispatch will still sit at ~350–400M cycles. The
remaining cost is dominated by:

- `read_next_pc` → `bus_read` → `tick_cycle` → `advance_peripherals` on every
  opcode fetch and operand byte — this is inherently sequential and hard to batch
- `resolve_r8` match per operand fetch

Further gains here would require either:
- **Instruction-level batching**: run N M-cycles of peripheral advance at the end
  of each instruction rather than one per bus access (changes timing semantics,
  needs careful correctness validation against blargg/mooneye)
- **ROM read fast path**: detect that the PC is in ROM (0x0000–0x7FFF) and bypass
  the full `bus_read` path for opcode fetches, since ROM reads have no side effects

---

## Summary table

| # | Change | Est. savings | Effort | Unblocks |
|---|---|---|---|---|
| 1 | Async DMA display | ~2–3× fps | Medium | 60 fps ceiling |
| 2 | APU cycle batching | ~80–100M cycles | Small | — |
| 3 | PPU build_stat cache | ~20M cycles | Small | — |
| 4 | Fixed pending_bus_events | small | Trivial | — |
| 5 | Instruction-level tick batching | ~100M+ cycles | Large | — |
