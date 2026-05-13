# Performance Profiling Infrastructure

## Feature flags

| Flag | Crate | Effect |
|---|---|---|
| `fps` | `rustyboy-pico2w` | Logs FPS to RTT every 60 frames |
| `perf` | `rustyboy-pico2w` | Implies `fps`; enables DWT cycle counters in core and logs per-component breakdowns every 60 frames |
| `perf` | `rustyboy-core` | Activates `#[cfg(feature = "perf")]` instrumentation throughout core |

Build and flash:

```sh
cd platform/pico2w
cargo run --release --features perf   # full breakdown + fps
cargo run --release --features fps    # fps only
```

## Current Pico RTT output

Every 60 frames, the Pico `perf` build logs:

```text
fps: X
core1 transport/60f — enq=... spins=... apu_cmds=... ppu_adv=... frame_pub=... vram_bytes=... oam_bytes=... regs=... audio_drops=...
frontend/60f — total=...ms cpu=...ms route=...ms ppu_timing=...ms ppu_sync=...ms timer=...ms apu_state=...ms apu_send=...ms rtc=...ms serial=...ms dma=...ms steps=... events=... scanlines=... dma_bytes=...
sm83/60f — total=... decode=... mem_r=... mem_w=... route=... fast=... io=... enqueue=... other=...
decode breakdown — pc=... rom=... idle=... raw_rom=... wrap=... bus=... opcode=... cb=... operand=... pc_calls=... bus_calls=... opcode_calls=... cb_calls=... operand_calls=...
ppu breakdown — bg=... window=... sprites=... stat=...
apu breakdown — frame_seq=... pulse=... wave=... noise=... mix=...
cart breakdown — rom=... ram=... control=... sync=... sync_calls=... bank0=... bank0_calls=... banked=... banked_calls=...
display/60f — ...ms total (scale=...ms fill=...ms) avg ...ms/frame
loop/60f — emulate=...ms audio_wait=...ms avg emulate=...ms/frame
```

All cycle counts are DWT `CYCCNT` ticks at 250 MHz on Pico2W builds.

## Instrumentation notes

- DWT is per-core on RP2350, so `perf` must enable the counter on both core 0 and core 1.
- The core 1 worker now initializes DWT before entering its command loop.
- The low-level SM83 counters in `core/src/cpu/sm83.rs` were reconnected on 2026-05-12 after a hot-path refactor had left them defined but no longer recorded.
- `route_bus_events()` now records `mem_write_route`, so SM83 write fan-out is visible again.
- The fine-grained SM83 counters add noticeable overhead to `perf` builds. Use them for hotspot ranking, not for final release-speed estimates.

## Latest hardware capture

Date: 2026-05-12  
Platform: Pico2W @ 250 MHz  
ROM: Tetris, 256 KiB, staged in onboard flash  
Build: `cargo run --release --features perf`

### Warmed steady-state window

Representative warmed windows after boot settled:

| Bucket | Value | Notes |
|---|---|---|
| `fps` | 33-34 | Lower than coarse `perf` runs because the restored SM83 counters are intrusive |
| `loop emulate` | 1497-1534 ms / 60f | About 25 ms / frame in the current `perf` build |
| `display total` | 225 ms / 60f | Almost entirely pre-scaling |
| `display scale` | 224 ms / 60f | About 3.7 ms / frame |
| `audio_wait` | 0 ms / 60f | Audio is not pacing the loop |

### Frontend breakdown

| Bucket | Value / 60 frames | Notes |
|---|---|---|
| `frontend total` | 1464-1500 ms | Core 0 frontend envelope |
| `frontend cpu` | 1060-1091 ms | Biggest core 0 frontend bucket |
| `route_bus_events` | 29-30 ms | Event fan-out to the worker |
| `ppu_timing` | 44-45 ms | Cost of issuing PPU advance commands |
| `ppu_sync` | 68-69 ms | Copying back published worker state and frames |
| `timer` | ~30 ms | Stable |
| `apu_send` | ~33 ms | Command send path only, not core 1 APU execution |
| `dma` | 42-43 ms | Bulk OAM DMA handling on core 0 |

### SM83 breakdown

| Bucket | Value / 60 frames | Notes |
|---|---|---|
| `sm83 total` | 240.6M-247.6M cycles | CPU-local measured work |
| `decode` | 228.4M-234.8M cycles | `total - mem_r - mem_w` |
| `mem_read` | 6.7M-6.8M cycles | Small relative to decode |
| `mem_write` | 5.4M-6.1M cycles | Also small relative to decode |
| `mem_write_route` | 2.0M-2.2M cycles | Real, but not top-tier |
| `mem_write_fast` | 1.7M-2.0M cycles | Direct region writes |
| `mem_write_enqueue` | 0.31M-0.33M cycles | Event queue push cost |
| `mem_write_io` | ~1.5K cycles | Negligible |

### Decode hotspot breakdown

| Bucket | Value / 60 frames | Notes |
|---|---|---|
| `pc_fetch` | 48.9M-49.3M cycles | Full PC fetch cost |
| `pc_fetch_rom` | 48.4M-48.9M cycles | Almost all fetches are ROM-space |
| `pc_fetch_rom_idle` | 23.1M-23.5M cycles | Common-case ROM fast path |
| `pc_fetch_rom_read` | 18.9M-19.2M cycles | Raw ROM byte read inside ROM fetch |
| `pc_fetch_wrapper` | ~1.42M cycles | Mostly `PC += 1` wrap-up work |
| `bus_read` | 6.6M-6.8M cycles | Generic non-fetch reads |
| `opcode_dispatch` | 25.9M-26.6M cycles | Table lookup + handler clone |
| `cb_prefix` | 20.4M-21.6M cycles | `0xCB` path is still expensive |
| `operand8` | 6.0M-6.6M cycles | 8-bit operand helper |

Typical call counts in the same windows:

| Counter | Value / 60 frames |
|---|---|
| `pc_fetch_calls` | ~709K-711K |
| `bus_read_calls` | ~150K-153K |
| `opcode_dispatch_calls` | ~390K-395K |
| `cb_prefix_calls` | ~108K-115K |
| `operand8_calls` | ~64K-70K |

### Core 1 PPU and APU breakdown

| Bucket | Value / 60 frames | Notes |
|---|---|---|
| `ppu bg` | ~45.5M cycles | Largest measured PPU sub-bucket |
| `ppu sprites` | 6.8M-7.9M cycles | Secondary PPU bucket |
| `ppu window` | ~97K cycles | Negligible for Tetris |
| `ppu stat` | ~123K cycles | Small with current counters |
| `apu mix` | 13.0M-13.2M cycles | Dominant measured APU bucket |
| `apu noise` | 1.1M-2.6M cycles | Next visible APU bucket |
| `apu pulse` | ~20K-25K cycles | Cheap with current skip-ahead logic |
| `apu wave` | ~11K-12K cycles | Cheap with current skip-ahead logic |
| `apu frame_seq` | ~78K-88K cycles | Negligible |

### Core 1 transport counters

Steady windows looked like:

| Bucket | Value / 60 frames | Notes |
|---|---|---|
| `command_enqueues` | ~5.6K-5.8K | Core 0 to core 1 command traffic |
| `ppu_advance_commands` | ~4.9K | Batched PPU advances |
| `apu_commands` | ~456-594 | Batched APU advances and register writes |
| `frame_publishes` | 60 | One frame per Game Boy frame once warmed |
| `audio_queue_drops` | 0 | No observed queue pressure |

## Observations

- Restoring the fine-grained SM83 counters cut `perf`-build FPS from the earlier coarse `43-44 fps` range down to `33-34 fps`. That is expected instrumentation cost.
- Even with the extra detail, `decode` remains the dominant SM83 bucket by a wide margin.
- `pc_fetch` is still large, but it is no longer the only story; `opcode_dispatch` and the `0xCB` path are also expensive.
- `mem_write_route` is visible again and confirms that write fan-out exists, but it is much smaller than the decode envelope.
- The display scaler is still a meaningful wall-clock cost at `225 ms / 60f`, even though SPI transfer itself is already overlapped with emulation.
- Measured PPU work is dominated by background rendering. Window rendering remains negligible for this ROM.
- The current PPU and APU sub-counters still do not explain all worker-side cost. More residual split instrumentation is still worthwhile there.

## 2026-05-13 release-validation experiments

Three follow-ups were tried after the 2026-05-12 capture and were not kept
because they did not produce a reliable release-build FPS win.

### 1. Worker frontend dirty-bit sync

Goal:

- avoid unconditional core 0 mirror writes of `NR52`, `LY`, and `STAT` after
  every worker poll

Result:

- the `perf` build did not become clearer or convincingly faster
- warmed `perf` windows actually pushed `ppu_sync` up into roughly
  `87-92 ms / 60f`
- no user-visible release-build FPS improvement was observed on hardware

Status:

- reverted

### 3. Core 1 published audio DMA buffers

Goal:

- mirror the display seam by having core 1 drain APU samples, pack them into
  I2S-ready `u32` stereo words, and publish whole audio buffers for core 0 to
  submit directly

Why it seemed plausible:

- the Pico path already uses DMA-backed I2S output
- the live release loop still pays per-frame audio staging on core 0:
  `drain_audio_samples_into_i16()` plus `queue_next_frame_i16()`

Release validation:

- build: `cargo run --release --features fps`
- experimental build warmed at about `49-51 fps`
- current baseline warmed at about `53 fps`
- conclusion: publishing packed audio buffers from core 1 regressed release FPS
  instead of improving it

Status:

- reverted

### 2. Coalesced bus-event accumulator

Goal:

- replace the raw per-write event queue with dirty VRAM/OAM ranges plus a set
  of distinct dirty IO addresses

What it looked like in `perf`:

- promising at first glance
- warmed windows moved into roughly `40-42 fps`
- `route_bus_events` dropped into about `44-46 ms / 60f`
- `frontend cpu` dropped into about `960-975 ms / 60f`

Release validation:

- build: `cargo run --release --features fps`
- experiment build warmed at about `51 fps`
- baseline commit `0a1ca1b` (`Offload Pico display scaling to core1`) warmed at
  about `51-52 fps`
- conclusion: no meaningful release-build FPS win

Status:

- reverted

### Takeaway

- treat `perf`-build improvements as hotspot clues, not as proof of a real
  release-speed win
- validate any future frontend/bus-routing change with `--features fps` on
  hardware before keeping it
- do not revisit the dirty-bit worker sync, coalesced bus-event accumulator, or
  core 1 published audio buffers without new evidence

## 2026-05-13 release-path CPU wins

Two narrow SM83 follow-ups did hold up in release builds on hardware and are
worth keeping in mind as the new baseline.

### 1. Direct CB execution helpers

Change:

- split the generic `cb()` body into direct register-target and direct `(HL)`
  helpers so the hot CB path does not bounce through the generic target
  read/write helpers

Release validation:

- build: `cargo run --release --features fps`
- early warmup still spent time in the low `50-51 fps` range
- after roughly `24-26s` in the game loop, repeated runs settled at about
  `55-56 fps`
- prior warmed release baseline was about `53 fps`

Notes:

- `perf` did not show a dramatic headline collapse in `cb_hl`, but release FPS
  improved consistently enough to keep the change

### 2. Immediate operand specialization

Change:

- bypass `get_operand8()` for the hottest immediate users:
  `LD r,n`, `LD (HL),n`, and the `A,d8` ALU/logical forms

Release validation:

- build: `cargo run --release --features fps`
- the release build now reaches about `55-56 fps` almost immediately instead of
  needing the long climb seen before
- repeated warmed windows stayed in roughly the same `55-56 fps` band

Perf clue:

- warmed `operand8_imm` dropped into roughly `2.7M-3.3M cycles / 60f`, down
  from the earlier `3.2M-4.1M` range seen after the CB helper change

## 2026-05-13 opcode histogram follow-up

Change:

- add perf-only execution-body histograms for decoded normal opcodes and decoded
  `0xCB` opcodes

Hardware capture:

- build: `cargo run --release --features perf`
- warmed `perf` windows sat around `34-36 fps`

Top warmed normal opcodes by execution-body cycles:

- `0x20` (`JR NZ,r8`): about `27.4M-31.3M cycles / 60f`, with roughly
  `104K-121K` calls and `259-261` cycles/call
- `0x28` (`JR Z,r8`): about `5.0M-6.6M cycles / 60f`, with roughly
  `8.6K-11.4K` calls and `579-583` cycles/call
- `0x21` (`LD HL,d16`): about `4.4M-5.8M cycles / 60f`, with roughly
  `9.7K-13.3K` calls and `436-451` cycles/call

Top warmed `0xCB` opcodes by execution-body cycles:

- `0x76` (`BIT 6,(HL)`): about `25.3M-29.9M cycles / 60f`, with roughly
  `91.9K-108.8K` calls and about `275` cycles/call
- `0x7E` (`BIT 7,(HL)`): about `430K-681K cycles / 60f`, with roughly
  `600-969` calls and `702-717` cycles/call
- `0x56` (`BIT 2,(HL)`): about `386K-579K cycles / 60f`, with roughly
  `481-707` calls and `804-825` cycles/call

Takeaway:

- the next CPU experiment should target conditional relative jumps, especially
  `JR NZ,r8` and `JR Z,r8`
- the next CB-specific experiment should target `BIT n,(HL)` directly rather
  than more generic `(HL)` CB work
- `LD HL,d16` being consistently third also makes a small `fetch_word()` helper
  or other `d16` fast path worth trying after the branch work

### Follow-up: `JR cc,r8` fast path

Change tried:

- inline the `JR` family directly in `Sm83::step_inner()` to bypass the generic
  `Jump` handler path for opcodes `0x18`, `0x20`, `0x28`, `0x30`, and `0x38`

Validation:

- build: `cargo run --release --features fps`
- two release runs on 2026-05-13 repeated the same pattern:
  early `50 -> 55 -> 56`, then a longer warmed band around `53-54 fps`

Conclusion:

- no meaningful release-build win versus the current `54-55 fps` baseline
- reverted
- next target remains `BIT n,(HL)`, not more `JR` work

### Follow-up: `BIT n,(HL)` fast path

Change tried:

- intercept `BIT n,(HL)` CB opcodes directly after the `0xCB` fetch and execute
  the `(HL)` bit test inline instead of going through the generic CB handler

Validation:

- build: `cargo run --release --features fps`
- the release run started lower than baseline and then settled into a long
  `49-52 fps` band, with repeated `50 fps` windows in warmed steady state

Conclusion:

- clear regression versus the current `54-55 fps` baseline
- reverted
- next remaining CPU experiment is the smaller `d16` path around `LD HL,d16`

### Follow-up: `d16` / `fetch_word()` cleanup

Change tried:

- factor repeated 16-bit immediate fetches into a small helper and route the
  hot `d16` sites through it, including `LD HL,d16`, `JP nn`, `JP cc,nn`,
  `CALL nn`, and `a16` load/store forms

Validation:

- build: `cargo run --release --features fps`
- early warmup reached the familiar `55-56 fps` band
- longer warmed windows then sagged into roughly `51-54 fps`, below the prior
  `53-55 fps` baseline

Conclusion:

- no reliable release-build win
- reverted
- the remaining obvious micro-opcode candidates have now all tested flat or
  worse on hardware

## 2026-05-13 `oc-280` display divider follow-up

Initial result:

- build: `cargo run --release --features "fps,oc-280"`
- the first `280 MHz` attempt booted, but release FPS flattened at about
  `47 fps`, even though the `perf` build showed somewhat better emulation time

Root cause:

- Pico display SPI was still requesting `62.5 MHz`
- that is exact at the default `250 MHz` sysclk (`250 / 2 / 2 = 62.5 MHz`)
- on RP2350 under Embassy SPI divider rules, the same request at `280 MHz`
  rounds down to `280 / 2 / 3 = 46.7 MHz`
- that reduced display bandwidth enough to turn the release build into a
  display-wait bottleneck, while the slower `perf` build still hid most of the
  transfer behind emulation time

Change:

- when `oc-280` is enabled, request `70 MHz` display SPI instead of `62.5 MHz`
- this lands on an exact `280 / 2 / 2 = 70 MHz` bus clock and restores the
  intended display-transfer overlap

Release validation:

- build: `cargo run --release --features "fps,oc-280"`
- after the display clock fix, the run opened at `53 fps` and then spent a long
  early window in repeated `58-59 fps` slices
- longer warmed windows later drifted through roughly `57 -> 55 -> 53-56 fps`
  instead of collapsing to the old flat `47 fps` plateau

Perf cross-check:

- build: `cargo run --release --features "perf,oc-280"`
- warmed `perf` windows sat around `40-42 fps`
- `display/60f` stayed at `1 ms total` and `audio_wait` stayed at `0 ms`,
  matching the idea that the display path is once again mostly overlapped

Takeaway:

- the earlier `oc-280` regression was not a simple “280 MHz is unstable”
  result
- it was primarily a platform-side display clock quantization issue
- future overclock experiments should always re-check actual realizable SPI bus
  rates, not just the requested target frequency

## 2026-05-13 `oc-300` trial

Configuration:

- add `oc-300`
- run the RP2350 at `300 MHz`
- raise core voltage to `V1_30`
- request `75 MHz` display SPI so Embassy lands on an exact `300 / 2 / 2`
  divider instead of rounding down to a much slower bus rate

Release validation:

- build: `cargo run --release --features "fps,oc-300"`
- the first ~20 seconds mostly sat around `54-57 fps`
- the run then climbed through `58-59 fps`
- from roughly `35s` through about `77s`, it held almost entirely at `59 fps`
- later warmed windows eased slightly into roughly `57-58 fps`
- no crash, hard fault, or obvious instability was observed during the soak

Perf cross-check:

- build: `cargo run --release --features "perf,oc-300"`
- warmed `perf` windows sat around `41-44 fps`
- `frontend cpu` landed around `931-1006 ms / 60f`
- `loop emulate` landed around `1322-1427 ms / 60f`
- `display/60f` stayed at `1 ms total`
- `audio_wait` stayed at `0 ms`

Takeaway:

- `300 MHz` appears viable on this board with the current `V1_30` setting
- it is the first tested profile that spends long warmed windows essentially at
  the `59 fps` pacing ceiling
- the same divider lesson from `oc-280` still applies: overclock results are
  heavily shaped by whether display SPI lands on a good realizable clock
- the plain Pico build has since been moved to this `300 MHz` / `75 MHz` SPI
  baseline; use `oc-280` or `oc-266` only for comparison runs

## Core 1 offload guidance

### Current split

- Core 0 owns SM83 execution, frontend memory, DMA bookkeeping, input, and display submission.
- Core 1 already owns live PPU and APU worker execution.
- A stable 160x144 frame is published back across the worker boundary at `frame_ready`.

Relevant seams:

- `WorkerLink::poll_frontend_state(out)` returns `frame_ready` and copies the published framebuffer.
- The Pico worker publishes frames through the shared frame slots before core 0 scales and submits them.

### Good next offload

The best next core 1 candidate is still framebuffer scaling and palette conversion, not more live emulation work.

Why:

- `scale_to_rgb565()` is isolated, deterministic, and already frame-boundary scoped.
- It operates on a stable published framebuffer, so it does not need live MMIO ordering.
- SPI DMA is already overlapped well enough that moving DMA ownership alone does not buy much.

Recommended shape:

1. Core 0 keeps SM83/frontend ownership.
2. Core 1 keeps live PPU/APU ownership.
3. After `frame_ready`, core 1 scales the immutable 160x144 framebuffer into a double-buffered 240x216 RGB565 surface.
4. Core 0 keeps ownership of actual display submission and SPI DMA kickoff.

### Framebuffer hook direction

If platform code needs direct access to completed frames, the clean hook is the existing worker publish seam rather than a new core-level callback.

Good direction:

- Extend the published-frame path so platform code can consume a published framebuffer slot, or a stable frame reference, without first copying into `GameBoyFrontend::front_buffer`.
- Let the Pico platform consume that frame directly for scaling.
- Keep the hook at the worker/platform boundary, not inside live PPU timing code.

### Avoid for now

- Moving live SM83 execution to core 1
- Moving the PPU mode machine, LY/STAT updates, or interrupt generation off the current worker boundary
- Moving live `apu.tick()` timing or MMIO-sensitive audio behavior to another queue stage
- Moving the whole display pipeline, including pacing and DMA ownership, onto core 1 before the scaler path is measured

## Immediate next move

1. Keep the opcode histogram in place, but stop adding more speculative opcode fast paths without a stronger hypothesis.
2. Pivot back to measurement work or a broader structural target rather than more single-opcode tuning.
3. If we stay on CPU work, choose a candidate that changes memory or synchronization behavior, not just handler shape.
