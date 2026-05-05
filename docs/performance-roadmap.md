# Performance Roadmap

## Current state — post display DMA + APU batching

ROM: Tetris, Pico2W @ 250 MHz.

Latest `perf` build measurement:

```
fps: 12
cycles/60f — total=1087658401 ppu=226370324 timer=17928846 apu=88093231 cpu_exec=755266000 (mem_r=29927803 mem_w=226325431 decode=499012766)
ppu breakdown — bg=43526449 window=95196 sprites=8074787 stat=30854900
apu breakdown — frame_seq=312708 pulse=17766610 wave=6626594 noise=6414055 mix=12869944
display/60f — 238ms total (scale=236ms fill=1ms) avg 3ms/frame
```

### Updated breakdown (60 frames, `perf` build)

| Bucket | Time / 60 frames | % of wall clock |
|---|---|---|
| Emulation (DWT) | 4.35 s (1088M cycles) | dominant |
| Display scaler + residual wait | 0.24 s | small |
| **Total** | **~5.02 s** | **100%** |

Target: 1.00 s / 60 frames (60 fps).  
Required speedup from current `perf` run: **~5×**.

### Emulation sub-breakdown (1088M cycles)

| Component | Cycles | % of emulation |
|---|---|---|
| decode / dispatch | 499M | 46% |
| mem_write | 226M | 21% |
| ppu | 226M | 21% |
| apu | 88M | 8% |
| mem_read | 29M | 3% |
| timer | 18M | 2% |

### What changed

- Async display DMA is working: residual display wait is `1ms / 60f`, so the
  SPI transfer is no longer the main limiter.
- Pre-scaling is now the only notable display cost at `236ms / 60f`
  (`~3.9ms/frame`).
- APU batching is working: APU cost fell from `~205M` cycles to `~88M`.

### New hotspot order

1. `decode / dispatch` at `499M`
2. `mem_write` at `226M`
3. `ppu` at `226M`, with `build_stat = 30.9M`

---

## Completed — Async display transfer

**Status: done**

- DMA-backed SPI transfer overlaps with emulation successfully.
- Static letterbox bars are no longer repainted every frame.
- Remaining display work, if needed later, is reducing scaler cost rather than
  fixing transfer latency.

---

## Completed — APU cycle batching

**Status: done**

- Batched APU ticking reduced APU cost from `~205M` cycles to `~88M`.
- No longer a top-three hotspot for this ROM/profile.

---

## Priority 1 — PPU `build_stat` cache

**Expected impact: ~25–30M cycles saved (~2–3% of total emulation)**

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

## Priority 2 — Split `mem_write` before optimizing it

**Expected impact: diagnostic; determines the real next hot path**

`mem_write` is now `226M` cycles, much larger than the old baseline. That is too
large to attribute solely to `Vec` push overhead in `pending_bus_events`.

### What to do

Add separate perf counters around:
- `memory.write_io` / `memory.write_fast`
- enqueueing `pending_bus_events`
- `route_bus_events` / `handle_bus_event`

This tells us whether the cost is queue management, event fan-out, or raw IO writes.

---

## Priority 3 — `pending_bus_events` fixed-size array

**Expected impact: small-to-moderate, only if queueing is a real slice of `mem_write`**

`pending_bus_events: Vec<BusEvent>` does a bounds-check + capacity-check on every
I/O write. Since at most one I/O write can be pending per M-cycle before
`route_bus_events` drains it, the queue never exceeds a handful of entries.

### What to do

Replace `Vec<BusEvent>` with `[BusEvent; 4]` + `len: usize`. Eliminates the heap
indirection and capacity check on the write hot path.

---

## Priority 4 — `decode` remainder (longer term)

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
| done | Async DMA display | large | Medium | 60 fps ceiling |
| done | APU cycle batching | ~117M cycles observed | Small | — |
| 1 | PPU build_stat cache | ~25–30M cycles | Small | — |
| 2 | Split mem_write perf counters | diagnostic | Small | mem_write direction |
| 3 | Fixed pending_bus_events | small-to-moderate | Trivial | — |
| 4 | ROM opcode-fetch fast path / decode follow-up | large | Medium | decode direction |
