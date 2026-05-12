# Multithread Plan

## Goal

Increase emulator performance by offloading APU and PPU work onto core1 while keeping the code safe Rust and minimizing blocking on core0.

Primary target:

- move the expensive PPU/APU work off the main CPU loop
- keep core0 hot and avoid waiting on core1 except for the smallest necessary shared-memory access
- preserve a clean architecture that works on both `pico2w` and the web build without putting platform threading primitives into `rustyboy-core`

## Current Shape

Today `GameBoy::step()` advances everything synchronously after each CPU instruction:

- CPU execution
- bus event routing
- PPU
- timer
- APU
- RTC
- serial
- DMA

That means core0 owns the entire emulation loop, and the PPU/APU work is still paid for inline on the critical path.

Relevant files:

- [core/src/gameboy.rs](/home/vince/git/github.com/vbonduro/rustyboy/core/src/gameboy.rs)
- [core/src/memory/memory.rs](/home/vince/git/github.com/vbonduro/rustyboy/core/src/memory/memory.rs)
- [core/src/cpu/peripheral/ppu.rs](/home/vince/git/github.com/vbonduro/rustyboy/core/src/cpu/peripheral/ppu.rs)
- [core/src/cpu/peripheral/apu.rs](/home/vince/git/github.com/vbonduro/rustyboy/core/src/cpu/peripheral/apu.rs)

## Shared State Map

This is the state we need to reason about before moving work across cores.

| Region | Current writers | Current readers | Proposed ownership |
|---|---|---|---|
| ROM / cartridge RAM | CPU/core0 | CPU/core0 | core0 only |
| WRAM / HRAM | CPU/core0 | CPU/core0 | core0 only |
| Timer registers / DIV | CPU/timer core0 | APU needs DIV phase | core0 canonical |
| IF register | timer/PPU/serial/joypad set bits, CPU clears bits | CPU | core0 canonical |
| VRAM | CPU writes, PPU reads | PPU | core0 canonical, mirrored to core1 |
| OAM | CPU/DMA writes, PPU reads | PPU | core0 canonical, mirrored to core1 |
| PPU config regs | CPU writes, PPU reads | PPU | core0 canonical, mirrored as needed |
| PPU timing state | PPU logic today | CPU/interrupt flow | likely keep on core0 initially |
| APU regs / wave RAM | CPU writes, APU reads/writes | CPU reads back | core1 canonical, mirrored to core0 |
| Framebuffer | PPU writes | display code | core1 produces, core0 consumes |
| Audio samples | APU writes | platform drains | core1 produces, core0 drains |

## Recommended Architecture

Use message passing plus small mirrored state, not one giant mutex around `GameBoyMemory`.

### Platform-agnostic boundary

The core crate should define the emulation split, but not the threading model.

That means `rustyboy-core` should expose:

- a CPU-facing frontend state machine
- a peripheral worker state machine
- plain command/result enums that describe ordered work between them
- a synchronous compatibility wrapper that runs both halves inline

That means `rustyboy-core` should not expose or depend on:

- `core0` / `core1` concepts in its public API
- `std::thread`
- `embassy_rp::multicore`
- Pico spinlocks or Embassy channels
- web workers

The platform crates choose how to run the two halves:

- `platform/pico2w`: frontend on core0, worker on core1
- `platform/web`: run both halves inline at first, or later move the worker to a web worker if desired

### Recommended core module layout

To keep the first implementation focused and minimize churn:

| File | Role |
|---|---|
| `core/src/gameboy/mod.rs` | public compatibility wrapper exposing `GameBoy` plus module re-exports |
| `core/src/gameboy/protocol.rs` | command/result enums and shared snapshot structs |
| `core/src/gameboy/frontend.rs` | frontend-owned emulator state and stepping logic |
| `core/src/gameboy/worker.rs` | worker-owned APU/render state and command application |
| `core/src/gameboy/inline.rs` | synchronous inline adapter used by web/tests/default path |

This keeps the filenames short, lets the `gameboy/` directory carry the context, and avoids letting `gameboy/mod.rs` become a large migration diff before the split is proven.

### Recommended core API surface

The first executable version should aim for these core-side concepts:

- `GameBoyFrontend`
- `GameBoyWorker`
- `WorkerCommand`
- `WorkerResult`
- `WorkerLink` trait
- `InlineWorkerLink`
- `GameBoy` compatibility wrapper

The key relationship should be:

- `GameBoyFrontend` produces ordered worker commands
- `WorkerLink` transports those commands and returns results
- `GameBoyWorker` applies commands and produces results
- `GameBoy` owns a frontend plus an inline link so existing callers remain unchanged

### Recommended low-level API style

The lowest-friction core API is:

- keep `GameBoyFrontend` as a concrete state holder, not a type parameterized over transport
- make frontend stepping generic over a passed-in worker link
- keep `GameBoy` as the simple wrapper that owns:
  - `GameBoyFrontend`
  - `InlineWorkerLink`

That points toward an API shape like:

- `GameBoyFrontend::step(&mut self, link: &mut impl WorkerLink) -> Result<u8, CpuError>`
- `GameBoyFrontend::tick(&mut self, link: &mut impl WorkerLink)`
- `GameBoyWorker::handle_command(&mut self, cmd: WorkerCommand)`
- `GameBoyWorker::poll_result(&mut self) -> Option<WorkerResult>`

This keeps the protocol transport-neutral without forcing `GameBoyFrontend` itself to become generic over a platform transport type.

### Public export policy

To keep the core usable from both compatibility callers and platform-specific multicore code:

- `GameBoy` should remain the default top-level export
- frontend/worker/protocol types should also be public as an advanced API
- the simple path should stay simple, but Pico should not need private-core internals to run the split directly

In other words:

- callers that do not care about the split keep using `GameBoy`
- `platform/pico2w` is allowed to directly own a `GameBoyFrontend`, a `GameBoyWorker`, and a Pico-specific worker link

Recommended Rust module shape:

- `core/src/lib.rs` keeps `pub mod gameboy;`
- `core/src/gameboy/mod.rs` defines the compatibility wrapper and re-exports advanced split types
- sibling files inside `core/src/gameboy/` use short names:
  - `frontend.rs`
  - `worker.rs`
  - `protocol.rs`
  - `inline.rs`

### Ownership split

- the frontend owns CPU execution and the canonical memory map
- the frontend keeps timer, serial, joypad, DMA, IF, and interrupt dispatch coherent
- the worker owns the expensive peripheral work:
  - APU ticking and sample generation
  - PPU rendering work
  - frame/audio buffer production

### Concrete v1 responsibilities by block

This is the intended first-version ownership split at the register and event level.

| Block | Frontend responsibility | Worker responsibility | Notes |
|---|---|---|---|
| ROM / cart RAM / WRAM / HRAM | canonical owner | none | no worker access in v1 |
| VRAM | canonical owner, emits ordered mirror writes | mirrored copy for rendering | worker never reads canonical VRAM directly |
| OAM | canonical owner, applies DMA, emits mirror writes or bulk DMA updates | mirrored copy for sprite rendering | DMA stays frontend-owned |
| IE / IF | canonical owner | none | interrupt authority stays on frontend |
| Timer (`DIV`, `TIMA`, `TMA`, `TAC`) | canonical owner, interrupt generation | consumes `div_counter` in `AdvanceApu` | worker does not own timer regs |
| Joypad / serial | canonical owner | none | keeps CPU-visible behavior deterministic |
| PPU timing state (`dot`, `ly`, `mode`, STAT edge logic) | canonical owner | none | frontend decides mode changes and interrupt timing |
| PPU config regs (`LCDC`, `SCX`, `SCY`, `LYC`, `BGP`, `OBP0`, `OBP1`, `WY`, `WX`) | canonical owner, emits mirror writes | mirrored copy used during raster work | frontend remains source of truth |
| Framebuffer | owns front/display-visible buffer selection | renders into back buffer and publishes completed output | stale frame reuse is acceptable |
| APU readable register mirror | CPU-visible readback in v1 | produces updates that may refresh the mirror | v1 can keep current project-level simplifications |
| APU live channel state / mixer / sample buffer | none | canonical owner | worker-owned hot audio path |
| Wave RAM CPU read semantics | frontend-owned simplified mirror in v1 | live wave state for audio generation | explicit accuracy tradeoff |

### PPU split in v1

The PPU split needs to be more specific than "frontend vs worker":

- the frontend owns the PPU mode machine
- the frontend owns `LY`, `STAT`, VBlank interrupt generation, and STAT interrupt generation
- the worker owns rasterization and completed framebuffer production

So in v1 the worker is not the authoritative live PPU for CPU-observable timing. It is a rendering worker fed by ordered state from the frontend.

That implies:

- frontend advances dot/scanline state
- frontend decides when a scanline becomes renderable
- frontend sends the worker the state it needs to render that scanline or frame
- worker never decides interrupt timing on its own

### APU split in v1

The APU split is different:

- the worker owns the live APU ticking path and sample generation
- the frontend owns the CPU-visible readback contract in v1

This means v1 should prefer deterministic mirrored readback over synchronous worker queries during CPU execution.

That implies:

- CPU writes to APU regs are applied to the frontend-visible mirror immediately
- the same writes are sent to the worker in strict order
- worker-generated readback-sensitive values such as `NR52` can refresh the mirror asynchronously
- CPU reads should never block on the worker in the hot path

### Why this shape

- It keeps the CPU hot path on core0 free of large locks.
- It avoids borrowing fights across threads because each core owns its own state.
- It lets us choose how much accuracy to trade for speed in a controlled way.

## Data Flow

The naming below is intentional:

- `frontend` and `worker` are core concepts
- `core0` and `core1` are only one possible Pico mapping

### Current instruction-order contract to preserve

The split should preserve the current instruction-level ordering already encoded in `GameBoy::step()` and `advance_peripherals()`:

1. CPU executes one instruction and mutates canonical memory.
2. Frontend routes bus events and applies immediate CPU-visible side effects.
3. Frontend advances PPU timing.
4. Frontend advances timer state.
5. Frontend advances APU using the post-timer `div_counter`.
6. Frontend advances RTC if present.
7. Frontend advances serial if active.
8. Frontend advances DMA bulk copies.

This ordering matters because the current emulator already bakes in visibility rules such as:

- APU sees the timer state after the timer has advanced for the instruction
- DMA bytes copied during an instruction become visible after that instruction's PPU timing/render work, not before

The multicore split should preserve those semantics unless we intentionally decide to change them.

### Frontend loop

1. CPU executes one instruction.
2. The frontend routes bus events.
3. The frontend applies writes that affect CPU-visible state immediately:
   - IF
   - timer regs
   - serial
   - joypad
   - DMA setup
4. The frontend emits worker mirror commands for bus-visible writes in instruction order:
   - `WriteApuReg`
   - `WriteWaveRam`
   - `WriteVram`
   - `WriteOam`
   - `WritePpuReg`
5. The frontend advances PPU timing locally.
6. If the step reaches a visible-line render point, the frontend emits `RenderScanline { ... }`.
7. The frontend advances timer state locally.
8. The frontend emits `AdvanceApu { cycles, div_after }` using the post-timer `div_counter`.
9. The frontend advances RTC and serial locally.
10. The frontend advances DMA bulk copies locally and emits any resulting OAM mirror updates after the same-step PPU work, matching current instruction-order semantics.
11. The frontend keeps going without waiting unless it must complete a barrier operation.
12. The frontend consumes completed frame/audio buffers when available.

### Worker loop

1. The worker waits for work messages.
2. It applies mirrored register/memory updates in order.
3. It advances APU from the received cycle budget.
4. It renders scanlines or frames when explicitly told to do so by the frontend.
5. It publishes output buffers back to the frontend.

## Transport Constraints

The transport contract needs to be decided up front because it affects nearly every hot path.

### Steady-state hot-path rules

For steady-state emulation:

- no heap allocation in the per-step command path
- no blocking CPU reads on worker state
- no queue payloads that copy full framebuffers or arbitrarily sized audio vectors
- no platform-specific transport behavior encoded into emulator semantics

### Command transport rules

Steady-state `WorkerCommand` variants should be fixed-size, stack-owned data.

That means:

- command variants should not carry `Vec`
- command variants should prefer compact snapshots and small fixed payloads
- the frontend should emit commands directly to a sink rather than building a heap-backed command list per instruction

### Result transport rules

Steady-state `WorkerResult` variants should also be lightweight.

Large produced media should not travel through the queue as copied payload.

Instead:

- frame publication should use preallocated buffer slots plus lightweight completion descriptors
- audio publication should use preallocated sample slots or fixed chunk buffers plus lightweight completion descriptors

### Buffer publication model

Recommended v1 model:

- worker owns writable back buffers
- frontend owns the display-visible front selection
- worker publishes `FrameReady { slot }`
- worker publishes `AudioChunkReady { slot, len }`
- frontend consumes the latest ready slot without stalling

This keeps queue traffic small while still allowing platform-specific storage details.

### Inline adapter rule

The inline adapter should obey the same ordering semantics as the future Pico transport:

- same command order
- same result order
- same barrier semantics

That makes inline mode the semantic reference implementation.

### Bootstrap and resync rule

The worker needs a deterministic way to become fully synchronized at:

- startup
- load-state
- reset
- worker restart

Recommended v1 rule:

- steady-state uses incremental ordered commands
- startup and barrier operations are allowed to use full-state mirror rebuilds

That means the core protocol should explicitly support a resync flow that can rebuild worker state from canonical frontend state without requiring the worker to observe historical commands that happened before it came online.

## PPU Sanity Check

The PPU deserves a deeper pass because it is the trickiest split.

### What the current code is really doing

The current PPU implementation combines two different responsibilities in one type:

- timing and interrupt-visible state:
  - `dot`
  - `ly`
  - `mode`
  - `window_line_counter`
  - `prev_stat_line`
- raster production:
  - framebuffer writes
  - sprite/background/window composition
  - per-scanline background priority scratch data

That is fine in a single-threaded design, but it is exactly what we should separate for v1 multicore.

### Recommended v1 PPU split

In v1:

- the frontend owns the PPU timing state machine
- the worker owns scanline rasterization and backbuffer production

That means the frontend remains authoritative for:

- mode transitions
- `LY`
- `STAT`
- STAT interrupt edge detection
- VBlank interrupt generation
- LCD enable/disable side effects
- `window_line_counter`

That means the worker is responsible for:

- maintaining mirrored VRAM / OAM / readable PPU-reg state needed for rendering
- rendering a requested scanline into a backbuffer
- publishing a completed frame buffer when all visible scanlines for a frame are done

### Why this split matches the current emulator

The current emulator is already scanline-based, not pixel-FIFO-based.

Today, rendering happens when the mode machine crosses from pixel transfer to HBlank for a visible line. That is a clean seam:

- frontend advances timing exactly as it does now
- when a visible line finishes transfer, frontend emits a render job
- worker renders that line using mirrored memory and the provided register snapshot

This keeps CPU-visible PPU timing coherent while still moving the expensive raster work off the hot path.

### PPU state ownership in v1

Frontend-owned PPU state:

- `dot`
- `ly`
- `mode`
- `window_line_counter`
- `prev_stat_line`

Worker-owned PPU state:

- mirrored VRAM
- mirrored OAM
- mirrored render-relevant PPU regs
- backbuffer
- scratch scanline composition buffers

### Render trigger flow

For each frontend emulation step:

1. CPU executes and mutates canonical memory.
2. Frontend applies bus event side effects.
3. Frontend advances PPU timing locally.
4. If the step crosses a visible-line render point, frontend emits:
   - all preceding VRAM / OAM / PPU-reg mirror writes in order
   - `RenderScanline { ly, window_line_counter, regs_snapshot }`
5. Worker renders the scanline into its backbuffer.
6. When the frame is complete, worker publishes `FrameReady`.

The ordering guarantee is the important part: writes that are visible to the current emulator before a scanline render must appear before the corresponding render command in the worker queue.

### `window_line_counter` detail

This counter is easy to get subtly wrong.

In the current renderer, the window line advances only when the window actually participates in rendering for that scanline. Because the frontend owns timing in v1, it should also own the authoritative `window_line_counter` and send its value with the render command.

To avoid frontend/worker drift:

- frontend should send the exact `window_line_counter` snapshot used for that scanline
- worker should treat it as input, not derive its own independent counter
- if possible, the "does window participate on this line?" predicate should live in shared helper logic used by both sides

### LCD disable behavior

When LCDC disables the LCD:

- frontend resets timing-visible state immediately:
  - `LY = 0`
  - mode = HBlank
  - `dot = 0`
  - `window_line_counter = 0`
  - `prev_stat_line = false`
- frontend emits an LCD-reset style worker command
- worker discards any partial in-progress frame and resets its backbuffer state

The previous completed display frame may remain on screen until a later `FrameReady`.

### VBlank and frame publication

At VBlank start:

- frontend sets the VBlank interrupt exactly as it does now
- frontend does not wait for the worker
- if a completed frame is available, frontend may swap it in
- if not, frontend continues using the previous completed frame

This is the core "freshness over blocking" tradeoff for PPU output.

### Save/load/reset implications

This split is actually friendly to save-state behavior:

- save-state remains frontend-authoritative for timing-visible PPU state
- worker render buffers do not need to be serialized in v1
- after load-state or reset, the worker can be resynchronized by:
  - resetting worker state
  - replaying full mirrored VRAM / OAM / PPU-reg state

That is slower than incremental steady-state operation, but these are barrier operations and are not on the hot path.

### PPU-specific risks

The main risks in this split are:

- queue ordering bugs between VRAM/OAM writes and render jobs
- `window_line_counter` drift
- LCD disable/reset edge-case mismatches
- worker backlog causing stale frames

Of these, only the first three are correctness risks. Backlog and stale frames are acceptable performance tradeoffs under the v1 accuracy policy.

### PPU v1 conclusion

The v1 PPU design should treat the worker as a raster engine, not a second PPU authority.

That means:

- no `AdvancePpu { cycles }` in v1
- frontend keeps the live timing and interrupt contract
- worker renders only when frontend explicitly tells it what line or frame to render
- frame freshness may be sacrificed before CPU-visible correctness is sacrificed

## APU Sanity Check

The APU deserves a separate deep pass because its split is almost the inverse of the PPU split.

### What the current code is really doing

The current APU path combines three concerns:

- CPU-facing register readback
- live channel timing and quirks
- audio sample production

Today, CPU writes to APU regs are handled synchronously in the main thread:

- writes are applied to the live APU state
- readable values are mirrored into IO immediately
- `NR52` is refreshed after ticking

That is why the current code can cheaply answer most CPU reads without extra coordination.

### Recommended v1 APU split

In v1:

- the worker owns the live APU timing path
- the worker owns channel state, frame sequencer state, wave-channel phase, mixer state, and audio sample buffers
- the frontend owns a lightweight CPU-visible APU readback mirror

This means the APU split is not:

- frontend owns timing-visible state

Instead it is:

- frontend owns CPU readback semantics at the project’s current accuracy level
- worker owns the expensive live sound engine

### Why this split matches the performance goal

The expensive APU work is in:

- frame-sequencer-driven live channel updates
- per-channel frequency advancement
- wave-channel phase handling
- noise LFSR stepping
- sample mixing and output accumulation

Duplicating that on the frontend would eat into the performance win we are trying to create.

So the v1 design should not try to keep two fully live APUs in sync. It should keep:

- one live worker-owned APU
- one cheap frontend-owned readback mirror

### APU state ownership in v1

Frontend-owned APU mirror state:

- raw readable register mirror for `NR10..NR51`
- power/readback bookkeeping needed to emulate masked register reads
- CPU-visible wave RAM shadow at the current project accuracy level
- latest worker-published `NR52` view

Worker-owned live APU state:

- frame sequencer step
- DIV-edge-driven channel progression
- all channel enable/length/envelope/sweep/live timer state
- live wave-channel `just_read` / position / retrigger behavior
- live `NR52` channel-status bits
- audio sample buffer

### CPU read policy by address class

Not all APU reads need the same treatment.

#### `NR10..NR51` except `NR52`

These are mostly write-backed register reads with masks.

Recommended v1 policy:

- frontend answers these from its local mirror
- writes update the frontend mirror immediately
- the same writes are sent to the worker in strict order

This keeps the hot path lock-free and should remain deterministic.

#### `NR52`

`NR52` is special because some bits reflect live channel enable state.

Recommended v1 policy:

- bit 7 power state is updated immediately on the frontend
- worker publishes refreshed channel-status bits back to the frontend
- frontend answers CPU reads from the latest mirrored `NR52`
- CPU does not synchronously query the worker in the hot path

Tradeoff:

- channel-status bits may be slightly stale relative to the live worker
- this is an explicit v1 accuracy trade for performance

#### Wave RAM `FF30..FF3F`

This is the hardest CPU-facing APU read path because DMG behavior depends on the live wave-channel read window.

Recommended v1 policy:

- keep wave RAM CPU reads at the project’s current simplified level
- frontend answers from its wave RAM shadow / mirror
- worker owns the live coincidence-window behavior for audio generation only
- CPU does not synchronously query the worker for active wave-window reads

Tradeoff:

- active wave RAM read/write quirks remain intentionally simplified
- this matches the current project direction and existing ignored tests

### APU write ordering

Write ordering matters more than exact instantaneous cross-core freshness.

For v1:

- frontend applies CPU-visible readback changes immediately
- frontend emits the same `WriteApuReg` / `WriteWaveRam` commands to the worker in strict order
- `AdvanceApu` commands must stay ordered relative to those writes

In other words:

- if a CPU write should affect the next chunk of live audio evolution, it must appear in the worker queue before the next corresponding `AdvanceApu`

### DIV / frame sequencer boundary

The worker must not own timer state in v1.

Instead:

- frontend remains canonical owner of timer and DIV behavior
- frontend sends `AdvanceApu { cycles, div_counter }`
- worker derives frame-sequencer edges from the provided `div_counter`

This preserves the current coupling between timer state and APU timing without forcing shared timer ownership.

### Power-state behavior

APU power-off semantics are CPU-visible and must remain deterministic.

Recommended v1 policy:

- frontend updates the APU readback mirror immediately on `NR52` power writes
- frontend applies the same readback rules the current implementation uses:
  - register zeroing behavior
  - preserved length-counter-write semantics where relevant to readback
- worker receives the same `NR52` write in order and resets live channel state

This keeps CPU-visible power behavior coherent even if the worker is slightly behind.

### Audio publication

Audio output is the worker’s responsibility:

- worker accumulates PCM samples
- worker publishes chunks or frame-aligned sample buffers back to the frontend
- frontend consumes whatever is ready without stalling CPU execution

This is the main acceptable APU-side freshness tradeoff:

- slightly stale audio output is acceptable
- blocking CPU execution to wait for fresh audio is not

### Save/load/reset implications

Like the PPU split, this wants barrier semantics for non-steady-state operations.

For save/load/reset:

- frontend is authoritative for the CPU-visible mirror state it exposes
- worker live APU state must be paused and synchronized before completion
- load-state or reset should rebuild both:
  - frontend mirror state
  - worker live channel/audio state

These are not hot-path operations, so stronger synchronization is acceptable here.

### APU-specific risks

The main risks in this split are:

- write-order bugs between `WriteApuReg` and `AdvanceApu`
- `NR52` mirror lag causing slightly stale channel-status reads
- frontend/worker disagreement on power-state readback rules
- wave RAM shadow drifting from worker state after control-plane events

Of these:

- write ordering and power-state disagreement are correctness risks
- `NR52` staleness and wave-window simplification are accepted v1 tradeoffs

### APU v1 conclusion

The v1 APU design should treat the worker as the single live sound engine and the frontend as the cheap CPU-facing readback facade.

That means:

- no synchronous worker query on the CPU hot path for normal APU reads
- `NR10..NR51` readback comes from a frontend mirror
- `NR52` comes from the latest worker-published mirror view
- wave RAM CPU access remains simplified at the project’s current level
- audio freshness may be sacrificed before CPU throughput is sacrificed

### Audio output format in v1

The transport plan should take advantage of what the current APU already supports.

Recommended v1 policy:

- worker produces integer PCM internally
- worker publishes integer PCM chunks
- Pico should consume integer PCM directly
- the existing `f32` drain path remains only as a compatibility wrapper for current callers

That means the multicore refactor should not deepen the current `i16 -> f32 -> i16` conversion cost on Pico.

### Current save-state scope note

The current RBSS v1 save-state format serializes:

- CPU state
- timer state
- PPU timing-visible state
- memory and cartridge state

It does not currently serialize full live APU state, serial state, joypad state, or DMA-in-flight state.

So for v1:

- the multicore refactor should preserve current save-state scope, not expand it
- after load-state or reset, worker APU/render state can be rebuilt from frontend-visible mirror state and canonical memory
- exact audio-phase continuity across save/load is explicitly out of scope for this refactor

## Event Ownership

The main rule is:

- events that affect CPU-visible state stay frontend-authoritative
- events that affect produced media may originate from the worker

### Frontend-authoritative events

These events are created and resolved entirely on the frontend:

- CPU memory writes to canonical memory
- IO writes that change timer, joypad, serial, DMA, IE, or IF state
- timer overflow setting the timer interrupt bit
- joypad interrupt edge detection
- serial completion interrupt generation
- DMA start and DMA stepping
- PPU mode transitions
- `LY` updates
- `STAT` edge detection
- VBlank interrupt generation

These should continue to happen in the same relative per-instruction order as the current `GameBoy` loop unless a later measured optimization intentionally changes that contract.

### Frontend-to-worker commands

These are the messages the frontend sends in strict order:

- `AdvanceApu { cycles, div_counter }`
- `WriteApuReg { addr, value }`
- `WriteWaveRam { offset, value }`
- `WriteVram { addr, value }`
- `WriteOam { addr, value }`
- `BulkOamDma { source_page, bytes... }` or equivalent bulk mirror update
- `WritePpuReg { addr, value }`
- `RenderScanline { ly, regs_snapshot }` or equivalent render trigger
- `FrameBoundary`
- control-plane events such as `Reset`, `LoadState`, or `PauseAndSync`

Recommended coalescing rule:

- normal VRAM writes can stay one-command-per-write in v1
- DMA-driven OAM updates should be coalesced into ranges or bulk mirror commands so the queue does not get flooded with one-byte OAM traffic

Important ordering note:

- DMA-driven OAM mirror updates must preserve the current instruction-order visibility contract
- if a same-instruction PPU render trigger happened before DMA advancement in the current emulator, the mirrored OAM updates must also land after that render trigger in the worker command stream

### Worker-to-frontend results

These are the messages the worker may publish back:

- `FrameReady`
- `AudioChunkReady`
- `ApuMirrorUpdate { nr52, readable_regs... }`
- `WorkerSynced` for pause/barrier operations such as save-state or shutdown

### Events that need barriers

These operations should force both halves to synchronize before completion:

- save-state
- load-state
- reset / power cycle
- teardown or worker restart

They are rare, so we can afford stronger coordination here without harming steady-state performance.

## Suggested Message Types

These are the kinds of commands I’d expect the worker to receive in v1:

- `AdvanceApu { cycles: u16, div_counter: u16 }`
- `WriteVram { addr: u16, value: u8 }`
- `WriteOam { addr: u16, value: u8 }`
- `BulkOamDma { source_page: u8, data: [u8; 160] }` or equivalent chunked form
- `WritePpuReg { addr: u16, value: u8 }`
- `WriteApuReg { addr: u16, value: u8 }`
- `WriteWaveRam { offset: u8, value: u8 }`
- `RenderScanline { ly: u8, regs_snapshot: ... }` or equivalent render trigger
- `FrameBoundary`
- `LcdReset`
- `Reset`
- `LoadState`
- `PauseAndSync`

These are the kinds of results I’d expect back:

- `FrameReady`
- `AudioChunkReady`
- `ApuMirrorUpdate`
- `WorkerSynced`

The key rule is that the frontend sends ordered updates and the worker applies them in the same order.

These should be plain Rust data types owned by the core crate. The platform crate decides whether they move through:

- a synchronous inline adapter
- a Pico multicore queue
- a future web-worker transport

The command set should stay intentionally small in v1. If a command exists only for transport convenience and not emulator semantics, it should stay platform-side instead of leaking into the core protocol.

## Mutex Strategy

Use mutexes only for narrow shared access points, not for the whole emulator state.

This is a platform concern, not a core public-API concern.

Good candidates for mutex protection:

- the command queue between cores
- the frame/audio output buffers
- any small shared snapshot state that must be read by both cores

What should not be mutex-wrapped:

- full `GameBoyMemory`
- CPU execution state
- the active PPU/APU state machines

Prefer a single-producer/single-consumer channel or ring buffer for work dispatch over repeated lock/unlock around every read.

In other words:

- core defines ordering and ownership rules
- platform chooses the concrete queue, lock, or buffer primitive

## Accuracy Tradeoffs To Review

These are the decisions I would treat as explicit performance trades, not accidental behavior changes.

### Guiding rule

Keep CPU-visible hardware state accurate, and spend performance tradeoffs on media production latency.

In practice, that means:

- protect correctness of CPU-observable state first
- allow video freshness and audio output freshness to be slightly relaxed if needed
- prefer deterministic ordered mirroring over shared live state with lock timing

### Keep accurate

These should remain frontend-owned and deterministic in the first multicore version:

- interrupt flag behavior (`IF`)
- timer state and DIV-derived behavior
- DMA visibility and ordering
- CPU-visible `LY` / `STAT` behavior
- MMIO write ordering as observed by the CPU
- joypad and serial behavior

If these become stale or transport-dependent, we stop making performance tradeoffs and start introducing emulation bugs.

### Acceptable relaxations

These are the areas where the first multicore version can intentionally trade precision for speed:

- rendered video may lag behind the frontend by a scanline or, if core1 is late, by a full frame
- a completed frame may be dropped and the previous frame reused instead of stalling the frontend
- audio output may incur a small additional buffering delay
- APU wave-channel edge quirks can remain simplified at the current project level

These tradeoffs affect presentation quality more than game logic correctness.

### Do not relax in v1

These are the boundaries I would avoid crossing in the first implementation:

- do not move interrupt-generation authority away from the frontend
- do not let `LY`, `STAT`, timer state, or `IF` become eventually consistent
- do not make worker reads depend on ad hoc locks against canonical live memory
- do not create different emulation semantics between Pico multicore mode and web inline mode

The transport may differ by platform, but emulator semantics should stay the same.

### 1. PPU timing split

Option:

- keep LY/STAT/interrupt generation on the frontend
- send only render-relevant work to the worker

Tradeoff:

- fastest and safest
- less cycle-perfect than a fully live PPU on a worker
- CPU-visible reads of `LY`/`STAT` stay coherent on the frontend

### 2. APU register mirroring

Option:

- the worker owns live APU state
- the frontend keeps a mirrored readback view for CPU reads

Tradeoff:

- good performance
- possible staleness if we do not flush writes in strict order
- easier to keep safe Rust than sharing live channel structs

### 3. Frame deadline policy

Option:

- if the worker misses a frame deadline, the frontend keeps the previous frame instead of waiting

Tradeoff:

- best for FPS stability
- visual frame drops can happen under load
- avoids frontend stalls on the worker

### 4. Wave RAM behavior

Option:

- keep current simplified APU behavior around wave RAM access windows

Tradeoff:

- already aligned with the repo’s existing tradeoff direction
- may not satisfy the most timing-sensitive sound tests
- strong performance win for the multicore path

### Recommended v1 accuracy envelope

For the first version of the multicore design:

- the frontend owns all CPU-visible timing and interrupt state
- the worker owns expensive rendering and sample production
- video and audio outputs may lag or drop under load
- MMIO ordering must stay deterministic
- wave-channel quirks remain at the project’s current simplified level unless profiling later proves we can afford more accuracy

## Non-goals For v1

To keep the execution plan realistic, these are explicitly out of scope for the first implementation:

- full dot-accurate or FIFO-accurate PPU behavior
- buying back ignored wave-channel accuracy tests
- expanding the RBSS save-state format
- introducing a web-worker implementation
- sharing canonical live memory directly between frontend and worker
- rewriting the CPU core, memory map, or cartridge abstractions as part of the split

## Refactor Shape

I would split the work into phases.

### Phase 0

- split the current monolithic `GameBoy` scheduler into two core-side concepts:
  - frontend
  - worker
- define transport-neutral command/result types in `rustyboy-core`
- keep the existing `GameBoy` API by adding a synchronous inline adapter around both halves

### Phase 1

- move APU ticking behind the new worker boundary
- keep memory canonical on the frontend
- mirror only the APU-visible registers that CPU reads back
- keep the default inline adapter as the reference implementation for web/tests

### Phase 2

- move PPU rendering work behind the worker boundary
- keep PPU timing/interrupt bookkeeping on the frontend if needed for coherence
- add frame buffer publication back to the frontend

### Phase 3

- implement the Pico multicore transport in `platform/pico2w`
- map:
  - frontend -> core0
  - worker -> core1
- choose the narrowest synchronization primitives needed for Pico

### Phase 4

- tighten the shared-state protocol
- remove redundant copies
- measure whether any remaining mutex contention matters

## Execution Plan

The implementation should proceed in a way that keeps the emulator runnable at every step and preserves one simple rule:

- first make the split real
- then prove it in synchronous inline mode
- only then add Pico multicore transport

### Acceptance criteria

The high-level acceptance target is:

- web and tests continue to run through the synchronous adapter
- Pico can swap to a multicore transport without changing emulator semantics
- CPU-visible state remains deterministic
- performance improves without introducing frontend stalls

### First execution slice

Before any multicore code lands on Pico, the first code series should produce this shape:

1. `GameBoy` still exists and all current callers still compile.
2. Internally, `GameBoy` owns:
   - a frontend
   - an inline worker link
3. The inline worker link owns:
   - a worker
   - preallocated frame/audio publication buffers
4. Frontend stepping emits worker commands through the same transport abstraction the Pico platform will later implement.

If we do not reach that shape cleanly, we should not move on to real multicore transport yet.

The first behavior-moving slice after that should be APU workerization, not PPU workerization.

Reason:

- the current `ApuPeripheral` is already much closer to a standalone worker-owned state machine
- the current `PpuPeripheral` still mixes timing and raster work in one type
- proving the transport and mirror model on APU first should reduce risk before the more correctness-sensitive PPU split

### Milestone 0: Baseline and invariants

Goal:

- lock down what must not regress before structural changes begin

Work:

- document the v1 accuracy envelope in this plan
- keep the current top-level `GameBoy` API as the public compatibility surface
- identify which current tests are expected to stay green and which are already knowingly relaxed
- capture a fresh Pico perf baseline before structural work begins

Validation:

- `cpu_instrs`, `instr_timing`, `mem_timing`, `dmg-acid2`, `oam_dma`, and current passing `dmg_sound` coverage remain the reference bar
- existing known ignored wave-channel tests remain ignored, not newly broken
- Pico perf logs are saved so later changes can be compared against a real before/after

### Milestone 1: Core split without concurrency

Goal:

- introduce the frontend/worker architecture inside `rustyboy-core` with no platform threading yet

Work:

- split the current monolithic scheduler into:
  - frontend state
  - worker state
- define transport-neutral command/result enums
- define the minimal `WorkerLink` trait used by the frontend
- add a synchronous inline adapter that runs both halves in order inside the current `GameBoy`
- keep all existing callers using the same top-level API

Validation:

- no intended behavior change yet
- all existing core tests that pass today should still pass
- web should continue to run without platform changes
- Pico should still build and run in single-threaded inline mode

Concrete deliverables:

- `GameBoyFrontend` type exists
- `GameBoyWorker` type exists
- `WorkerCommand` / `WorkerResult` types exist
- `InlineWorkerLink` exists
- `GameBoy` is a wrapper rather than the only scheduler implementation

### Milestone 2: PPU workerization in inline mode

Goal:

- move raster production behind the worker boundary while keeping timing on the frontend

Work:

- keep frontend ownership of:
  - `dot`
  - `ly`
  - `mode`
  - `STAT`
  - VBlank/STAT interrupt generation
  - `window_line_counter`
- move raster work to worker-owned scanline/frame production
- emit ordered VRAM/OAM/PPU-reg mirror writes and `RenderScanline` commands
- add worker frame publication and frontend frame swap logic
- introduce slot-based frame publication rather than queueing full frame payloads

Validation:

- `dmg-acid2` still passes
- `mooneye_oam_dma` still passes
- scanline ordering bugs are checked with targeted tests around:
  - VRAM writes before render
  - OAM DMA visibility relative to sprite render matching the current instruction-order contract
  - LCD disable/reset
  - window counter behavior
- the inline adapter remains the correctness oracle for the later Pico transport

### Milestone 3: APU workerization in inline mode

Goal:

- move live APU ticking and sample production behind the worker boundary while keeping a frontend readback mirror

Work:

- move live APU state to the worker
- keep frontend mirror state for CPU-visible reads
- update writes so the frontend:
  - applies mirror-visible effects immediately
  - emits ordered `WriteApuReg` / `WriteWaveRam` commands
  - emits ordered `AdvanceApu` commands carrying `div_counter`
- add worker-published mirror updates such as `NR52`
- keep CPU reads lock-free in the inline adapter model
- keep integer PCM publication available so Pico can avoid the float round-trip later

Validation:

- current passing `blargg_dmg_sound` tests still pass
- ignored wave-channel tests stay in the same known-relaxed bucket unless we intentionally revisit them
- targeted tests should cover:
  - `NR52` power transitions
  - register readback masking
  - ordering of APU writes versus APU advancement
  - frontend mirror refresh after worker updates

### Milestone 4: Control-plane barriers

Goal:

- make rare operations deterministic before introducing real concurrency

Work:

- define explicit barrier flows for:
  - save-state
  - load-state
  - reset
  - shutdown / restart
- ensure worker state can be fully rebuilt from canonical frontend state and mirrored memory snapshots
- make barrier completion explicit through `WorkerSynced` style results

Validation:

- save/load tests continue to pass
- reset and reload behavior is stable after the split
- no stale frame/audio state leaks across barrier operations

### Milestone 5: Pico transport layer

Goal:

- replace the inline adapter on Pico with a real multicore transport

Work:

- implement Pico-specific frontend/worker transport in `platform/pico2w`
- map:
  - frontend -> core0
  - worker -> core1
- use narrow synchronization only where needed:
  - command queue
  - frame/audio publication
  - barrier synchronization
- avoid sharing canonical live memory directly across cores
- keep queue payloads small and fixed-size
- keep bulky frame/audio data in preallocated shared slots with lightweight completion messages

Validation:

- the same emulator semantics should hold as in inline mode
- web continues using the inline adapter unchanged
- Pico remains stable under long runs, save/load, and reset paths
- correctness issues found only in Pico mode are treated as transport bugs first, not emulator-design bugs

### Milestone 6: Performance tuning on real hardware

Goal:

- turn the architectural split into actual FPS wins

Work:

- measure queue pressure and worker backlog
- measure whether stale-frame reuse is happening and how often
- reduce avoidable copies in mirror-update and framebuffer paths
- revisit audio output packing overhead on Pico
- decide whether batching should change based on observed queue/worker behavior

Validation:

- compare against the original saved baseline
- verify whether the design is meaningfully closing the gap from roughly `36 fps` toward `60 fps`
- if the architecture is correct but performance is still short, profile which remaining bucket is dominant before making new tradeoffs

### Milestone 7: Tighten or relax based on results

Goal:

- make the final decisions based on measured behavior, not guesswork

Work:

- if correctness is solid but video freshness is poor, tune render publication and batching
- if correctness is solid but audio freshness is poor, tune audio chunk sizing and drain cadence
- if specific APU or PPU edge cases matter in practice, selectively buy back accuracy where the profile allows it
- if the worker split underperforms, simplify the transport rather than adding more shared-state complexity

Validation:

- final behavior matches the chosen v1 accuracy envelope
- platform differences remain transport-only, not semantic

## Validation Strategy

Each milestone should be checked in three layers:

### 1. Core correctness

- existing unit and integration tests
- targeted regression tests for the new split boundaries

### 2. Cross-platform semantic consistency

- web inline mode remains the simplest reference path
- Pico multicore mode should agree with inline mode on CPU-visible behavior

### 3. Hardware performance

- Pico perf logging after each major milestone
- compare against the saved baseline, not only against intuition

## Validation Commands

Use the real workspace package name when validating the core crate:

- `cargo test -p rustyboy-core`
- `cargo test -p rustyboy-core --test dmg_acid2`
- `cargo test -p rustyboy-core --test mooneye_oam_dma`
- `cargo test -p rustyboy-core --test blargg_dmg_sound`
- `cargo test -p rustyboy-core --test save_state`
- `cargo test -p rustyboy-core --test gamboy_smoke`

For implementation milestones, the practical default is:

- run `cargo test -p rustyboy-core` after structural changes that should be semantics-preserving
- run the focused ROM/regression suites above after PPU, APU, and barrier work
- use Pico perf logging only after the inline-mode split is already correct

## Recommended implementation order

If we want the safest path with the best signal:

1. Milestone 1: core split without concurrency
2. Milestone 3: APU workerization in inline mode
3. Milestone 2: PPU workerization in inline mode
4. Milestone 4: control-plane barriers
5. Milestone 5: Pico multicore transport
6. Milestone 6: performance tuning

This order first proves the worker transport on the cleaner APU seam, then spends that confidence on the trickier PPU timing/raster split before Pico multicore enters the picture.

## Ready-To-Execute Summary

The plan is ready to execute with these concrete implementation decisions locked in:

- `GameBoy` remains the public compatibility wrapper
- `rustyboy-core` introduces frontend, worker, protocol, and inline-link types
- PPU v1 worker is a raster engine, not a timing authority
- APU v1 worker is the single live sound engine, with a frontend readback mirror
- steady-state command/result traffic is fixed-size and allocation-free
- bulky frame/audio data is published through preallocated slots, not copied through the queue
- save/load/reset are explicit barrier operations
- RBSS save-state scope is not expanded during this refactor
- web stays on the inline adapter while Pico gets the real multicore transport later

## Platform Notes

This needs to work in both:

- `platform/pico2w`
- `platform/web`

That strongly suggests the following rule:

- `rustyboy-core` owns emulator semantics, state ownership, and the frontend/worker protocol
- `platform/pico2w` owns the actual multicore implementation
- `platform/web` keeps using the synchronous adapter unless and until a worker-based version is worth building

The core crate should still expose a simple top-level emulator type for callers that do not care about the split.

## Immediate Next Step

The design is now ready to execute.

The next implementation step should be Milestone 1:

- create `core/src/gameboy/`
- add `core/src/gameboy/protocol.rs`
- add `core/src/gameboy/frontend.rs`
- add `core/src/gameboy/worker.rs`
- add `core/src/gameboy/inline.rs`
- update `core/src/lib.rs` exports
- turn `core/src/gameboy/mod.rs` into the compatibility wrapper around frontend plus inline link

The Milestone 1 patch should be intentionally semantics-preserving:

- same public `GameBoy` API
- same instruction-order behavior
- same web path
- same Pico single-threaded inline behavior

Only after that shape compiles and `cargo test -p rustyboy-core` stays green should the work move on to APU workerization first, then PPU workerization.
