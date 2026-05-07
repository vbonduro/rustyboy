# Multithreaded Core Plan

## Goal

Add a 2-thread runtime architecture that works on:

- `platform/pico2w` by using both RP2350 cores
- `platform/web` by moving emulation work off the UI thread

while preserving the existing cycle-accurate `rustyboy-core` emulation model.

## Recommendation

Do **not** split the SM83, PPU, APU, DMA, timer, or bus across threads.

Instead:

- keep one thread as the **emulation owner**
- use the second thread/core as a **runtime worker**
- communicate only with coarse-grained messages and leased buffers

This keeps the hard timing logic single-owner and makes the second thread useful
without turning every peripheral boundary into a synchronization problem.

## Why The Core Should Stay Single-Owner

Today `Sm83` owns:

- CPU registers and instruction execution
- bus and memory
- timer
- PPU
- APU
- DMA state
- serial state
- joypad state

and advances them together inside each instruction/M-cycle.

Important coupling points already in the code:

- `Sm83::tick_impl()` drives the whole machine instruction-by-instruction.
- `Sm83::advance_ppu()` passes live VRAM/OAM slices into the PPU and snapshots the
  finished frame on VBlank.
- `Sm83::tick_apu()` depends on the timer's internal counter.
- `Sm83::tick_cycle_to_t3()` exists specifically to preserve wave-RAM timing at
  T3 within a single M-cycle.
- `Sm83::flush_apu_before_bus_events()` shows that some MMIO writes must be
  synchronized with pending APU work before the write is applied.

That means the hot path is not "CPU + a few optional peripherals". It is one
timing domain.

## Splits That Do Not Make Sense

### CPU thread + PPU thread

Not recommended.

Reasons:

- the PPU reads live VRAM and OAM constantly
- STAT, LY, VBlank, and LCD timing feed directly back into interrupts
- the current implementation snapshots the front buffer at VBlank from inside
  the same owner that advances the CPU and interrupt state

Moving the PPU out-of-thread would require either:

- per-cycle/per-M-cycle messages, which would be very expensive, or
- speculative snapshots of VRAM/OAM/register state, which risks timing bugs

### CPU thread + APU thread

Not recommended.

Reasons:

- APU timing depends on the timer internal counter
- wave RAM reads/writes have T-cycle-sensitive behavior
- MMIO writes already force local flushes before state changes are applied

This is too timing-sensitive to be a good first multicore split.

### Shared `Arc<Mutex<Sm83>>` across threads

Not recommended.

This spreads locks through the whole runtime and makes contention and timing
behavior part of the emulator architecture. The `RustedROM` repo is a useful
caution here: it uses a dedicated CPU thread, but the model is centered around
`Arc<Mutex<...>>` shared state rather than single ownership plus messages.

## Splits That Do Make Sense

### 1. Emulation thread + presentation/audio worker

Recommended first step.

The emulation thread:

- owns `Sm83`
- applies input commands
- runs one frame worth of cycles
- snapshots or leases frame/audio output
- sends output messages to the worker

The worker:

- converts framebuffer formats if needed
- packages audio for the platform sink
- performs blocking or pacing-sensitive output work
- returns reusable buffers to the emulation thread

This turns the second thread into a producer/consumer pipeline instead of a
second owner of Game Boy timing.

### 2. Emulation thread + UI/main thread on web

Recommended web shape.

Even without true Rust/Wasm threads, the browser can still use two execution
contexts:

- a worker for emulation
- the main thread for canvas, DOM, and UI

This is the cleanest way to get emulation off the browser main thread.

### 3. Emulation core + conversion/output core on Pico

Recommended Pico shape.

The Pico worker core should own the work that is expensive but timing-coarse:

- framebuffer scaling/color conversion
- audio sample packing for DMA buffers
- possibly save/persistence packaging later

Display DMA and audio DMA are already overlapped with emulation on core 0. The
best use of core 1 is the CPU work around those outputs, not splitting the
actual Game Boy timing model.

## Proposed Architecture

## Layer 1: Keep `rustyboy-core` single-threaded

`rustyboy-core` should remain the deterministic machine implementation.

Add only small runtime-facing APIs if needed, for example:

- `run_frame()`
- `take_frame_snapshot_into(...)`
- `drain_audio_into(...)`
- `apply_input_delta(...)`

The core should not learn about threads, mutexes, workers, or platform mailboxes.

## Layer 2: Add a runtime boundary

Introduce a new runtime module or crate that defines the message protocol.

Suggested types:

```rust
pub enum CoreCommand {
    SetButton { button: Button, pressed: bool },
    Pause,
    Resume,
    Reset,
    LoadState(Vec<u8>),
    SaveState,
    Shutdown,
}

pub enum CoreEvent {
    FrameReady(FramePacket),
    AudioReady(AudioPacket),
    SaveStateReady(Vec<u8>),
    Stopped,
}

pub struct FramePacket {
    pub frame_id: u64,
    pub format: FrameFormat,
    pub data: FrameBuffer,
}

pub struct AudioPacket {
    pub frame_id: u64,
    pub samples: AudioBuffer,
}
```

The important part is that the runtime boundary is **message-based**, not
`&mut Sm83`-based.

## Layer 3: Use reusable buffers

Avoid allocating a new `Vec` every frame.

Use a small pool:

- 2 frame buffers
- 2 audio buffers

Flow:

1. worker returns an empty buffer token
2. emulation fills it
3. emulation sends it to the worker
4. worker consumes it and returns the buffer token

This is effectively a mailbox plus a double-buffer pool.

## Threading Abstraction

The abstraction should wrap:

- spawning a long-lived worker
- sending commands/events
- exchanging reusable buffers

It should **not** try to abstract every detail of `std::thread` and Pico
multicore semantics into a fake universal thread API.

Prefer a higher-level trait such as:

```rust
pub trait RuntimeBackend {
    type CommandSender;
    type CommandReceiver;
    type EventSender;
    type EventReceiver;

    fn spawn_emu_worker(
        entry: impl FnOnce(Self::CommandReceiver, Self::EventSender) + Send + 'static,
    );
}
```

For buffer exchange, use a separate trait or concrete SPSC queue wrapper rather
than embedding buffer management into the spawn trait.

## Platform Backends

### Pico backend

Use:

- `embassy_rp::multicore::spawn_core1`
- fixed static core-1 stack
- `embassy_sync::channel`
- `SpinlockRawMutex` or the embassy multicore-safe mutex/channel primitives

Recommended ownership:

- core 0: emulation owner
- core 1: frame/audio worker

Why core 0 should keep emulation:

- current Embassy initialization and most hardware setup already live there
- display/audio DMA startup is already integrated there
- the emulation loop is the most timing-sensitive owner

### Web backend

There are two viable options.

#### Option A: Dedicated Web Worker

Recommended first.

Shape:

- main thread owns UI, DOM, canvas, menu, input
- worker owns the emulator instance
- messages pass input, frame buffers, audio batches, save-state requests

Pros:

- works with the current browser architecture model
- does not require turning the Rust wasm build into true threaded wasm first
- keeps UI responsive immediately

Cons:

- this is a browser worker abstraction, not `std::thread`
- some glue will be JS-side rather than pure Rust

#### Option B: True Rust/Wasm threads

Possible, but more expensive.

Constraints:

- current target is `wasm32-unknown-unknown`
- on that target, `std::thread::spawn` panics by default
- real threaded wasm needs atomics-enabled builds and rebuilt std
- browser hosting must be cross-origin isolated

This repo already sends:

- `Cross-Origin-Embedder-Policy: require-corp`
- `Cross-Origin-Opener-Policy: same-origin`

so hosting is much closer to ready than most projects, but the build pipeline
would still need to change.

If we choose this route later, we should likely model the web backend after a
worker bootstrap approach similar to `wasm-bindgen-rayon`.

## Suggested Data Flow

### Commands into the emulation owner

- button pressed/released
- pause/resume
- save/load state
- shutdown

These are low-frequency and small.

### Events out of the emulation owner

- frame ready
- audio ready
- optional telemetry/perf data
- save-state response

These are coarse-grained and bounded.

### Important rule

Never send per-cycle, per-scanline, or per-register mutation messages across
threads in the normal emulation path.

That would move the timing wall from the CPU to the mailbox.

## Phased Implementation Plan

## Phase 0: Runtime API cleanup

Before threads:

- add a `run_frame()` helper around the current `while cycle_counter < frame_budget`
  loops
- add buffer-filling APIs that write into caller-provided frame/audio buffers
- define the command/event enums in one place

Goal:

- make the current single-threaded runners use the future threaded boundary,
  just without spawning yet

## Phase 1: Pico worker core

Implement the 2-core Pico path first.

Reason:

- the hardware model is clear
- the backend primitives already exist in Embassy
- the repo already has a strong output pipeline on Pico

Split:

- core 0 runs `Sm83`
- core 1 performs framebuffer scaling and audio sample packing

Minimal first payload:

- send completed 160x144 frame snapshots to core 1
- core 1 writes the 240x216 RGB565 buffer
- optionally move `samples_to_i2s` style packing to core 1 as well

## Phase 2: Web worker runtime

Move emulation off the browser main thread.

Recommended first web shape:

- JS main thread
- worker-hosted wasm emulator

Keep the runtime message protocol aligned with Pico as much as possible even if
the transport differs.

## Phase 3: Shared buffer pool

Replace copying `Vec` traffic with reusable frame/audio buffer leases.

This is where the abstraction pays off:

- same producer/consumer model
- same packet types
- only transport/backend differs

## Phase 4: Optional true threaded wasm

Only do this if we explicitly want:

- Rust-managed worker threads in the browser
- shared-memory atomics
- fewer JS-side runtime responsibilities

This should be treated as a separate project, not a prerequisite for Phase 2.

## Concrete Near-Term Refactors

1. Add `Sm83::run_frame(cycles_per_frame)` or a small runner wrapper in `core`.
2. Add `framebuffer_into(dst: &mut [u8; FRAMEBUFFER_SIZE])` and
   `drain_audio_into(dst: &mut Vec<f32>)` style APIs or equivalent packet-filling helpers.
3. Create a shared `runtime` module with `CoreCommand`, `CoreEvent`, `FramePacket`,
   and `AudioPacket`.
4. Build a no-op single-threaded backend first so both current platforms can
   compile against the same runtime boundary.
5. Then add the Pico multicore backend.
6. Then move the web platform to a worker-based runtime.

## Decision Summary

Recommended:

- single-owner `Sm83`
- threaded runtime around the core
- second thread handles coarse output work
- message passing plus reusable buffers
- Pico multicore first
- web worker second

Not recommended:

- CPU/PPU split
- CPU/APU split
- shared `Arc<Mutex<Sm83>>`
- per-cycle inter-thread synchronization

## Research Notes

Current repo observations:

- Pico already overlaps display DMA and audio DMA with emulation, which shows the
  runtime is already moving toward pipeline parallelism.
- The web client currently runs emulation on the main browser thread.
- The web server already sets COOP/COEP/CORP headers, which is a prerequisite
  for shared-memory browser features if we later pursue true threaded wasm.

External inspiration:

- `CFdefense/RustedROM` demonstrates a dedicated CPU thread model, but it also
  shows how quickly `Arc<Mutex<...>>` can become the central architecture.
- Rust's `wasm32-unknown-unknown` target still does not provide working
  `std::thread::spawn` out of the box, so a web threading plan must be explicit
  about whether it means Web Workers or true atomics-enabled threaded wasm.
