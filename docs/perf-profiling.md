# Performance Profiling Infrastructure

## Feature flags

| Flag | Crate | Effect |
|---|---|---|
| `fps` | `rustyboy-pico2w` | Logs FPS to RTT every 60 frames |
| `perf` | `rustyboy-pico2w` | Implies `fps`; enables DWT cycle counters in core and logs per-component breakdowns every 60 frames |
| `perf` | `rustyboy-core` | Activates `#[cfg(feature = "perf")]` instrumentation throughout core (APU channels, PPU render stages, memory bus, SM83 dispatch) |

Build and flash:

```sh
cd platform/pico2w
cargo run --release --features perf   # full breakdown + fps
cargo run --release --features fps    # fps only
```

## RTT output format

Every 60 frames (~1 s at target speed):

```
fps: 60
cycles/60f — total=X ppu=X timer=X apu=X cpu_exec=X (mem_r=X mem_w=X decode=X)
ppu breakdown — bg=X window=X sprites=X stat=X
apu breakdown — frame_seq=X pulse=X wave=X noise=X mix=X
```

`cpu_exec = total − ppu − timer − apu`  
`decode   = cpu_exec − mem_r − mem_w`

All values are DWT CYCCNT ticks at 250 MHz (one tick ≈ 4 ns).

## Initial baseline — 2026-05-03 @ 250 MHz

ROM: Tetris (256 KiB, 16 banks, stored on onboard flash).

### Top-level breakdown (930 M cycles / 60 frames)

| Component | Cycles | % |
|---|---|---|
| decode / dispatch | 450 M | **48.3%** |
| apu | 212 M | 22.8% |
| ppu | 160 M | 17.2% |
| mem_write | 54 M | 5.8% |
| mem_read | 29 M | 3.1% |
| timer | 26 M | 2.8% |

### PPU sub-breakdown (160 M total)

| Stage | Cycles | % of PPU |
|---|---|---|
| bg render | 28 M | 17% |
| build_stat | 20 M | 12% |
| sprites | 4 M | 3% |
| window | ~60 K | <0.1% |

### APU sub-breakdown (212 M total)

| Channel | Cycles | % of APU |
|---|---|---|
| pulse (ch1+ch2) | 46 M | 22% |
| noise (ch4) | 26 M | 12% |
| wave (ch3) | 23 M | 11% |
| mix | 10 M | 5% |
| frame_seq | ~240 K | <0.1% |

## Observations

- **Instruction decode/dispatch dominates at 48%.** This is the highest-leverage optimization target. Prior work already placed the hot path in RAM (`.data` section); the next step is reducing dispatch overhead (branch misprediction, opcode table lookup).
- **APU pulse channels are surprisingly expensive** relative to wave/noise. The per-sample envelope and sweep logic runs at 4 MHz effective, firing every M-cycle.
- **PPU `build_stat`** (20 M cycles, 12% of PPU total) is called every M-cycle during active scanlines and handles STAT interrupt edge detection — worth batching or caching.
- **mem_write is 2× mem_read** despite writes being less frequent. Write fan-out (write_io + pending bus events + cache update) is the likely cause.
- **Window rendering is negligible** for this ROM; sprites are cheap. BG render is the PPU bottleneck.
