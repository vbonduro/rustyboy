# PPU/APU Core 1 Split Plan

## Goal

Build a 2-core architecture where:

- core 0 owns CPU execution, cartridge, timer, serial, joypad, and interrupt dispatch
- core 1 owns as much of PPU and APU work as we can move without breaking timing

This is the aggressive path after deciding that offloading only display scaling
and output packaging is not enough.

## Performance Ceiling

Using the current Pico numbers from [performance-roadmap.md](./performance-roadmap.md):

- total: `734M–737M cycles / 60f`
- ppu: `225M–226M cycles / 60f`
- apu: `85M–87M cycles / 60f`

If we could move the full current PPU+APU cost off core 0 with perfect overlap:

- core 0 would keep about `424M cycles / 60f`
- core 1 would take about `311M cycles / 60f`
- at `250 MHz`, the ideal ceiling becomes about `35.4 fps`

That is the best-case ceiling for this direction before sync overhead.

## The Main Constraint

The current code does not have a "PPU module" and an "APU module" that can
simply be moved to another core. It has one CPU owner that:

- advances PPU directly
- advances timer directly
- queues and flushes APU cycles directly
- routes MMIO writes directly
- handles T3-sensitive APU wave accesses directly
- snapshots LY/STAT/NR52 back into the IO register shadow directly

Relevant current seams:

- `Sm83::bus_write()` special-cases APU register and wave RAM writes at T3
- `Sm83::tick_cycle_to_t3()` exists specifically for time-sensitive APU access
- `Sm83::handle_bus_event()` applies LCD/APU MMIO writes directly
- `Sm83::advance_ppu()` passes live VRAM/OAM slices to the PPU and raises VBlank/STAT
- `Sm83::tick_apu()` calls `apu.tick(cycles, timer.internal_counter())`

So the first step is not "spawn core 1". The first step is to create a
timestamped peripheral boundary.

## The Split We Actually Need

To make a core-1 peripheral architecture viable, split both PPU and APU into:

- a **control plane**
- a **data plane**

The control plane handles timing-sensitive CPU-visible semantics.
The data plane handles the heavy work that burns cycles.

## PPU Split

### PPU control plane

This is the part that stays tightly coupled to the CPU at first:

- `dot`, `ly`, `mode`, `window_line_counter`
- STAT line generation and edge detection
- VBlank and STAT interrupt generation
- LCDC/STAT/SCX/SCY/LYC/WX/WY palette register shadowing
- frame boundary / front-buffer swap policy

This logic is tightly coupled because the CPU can read LY/STAT at arbitrary
times and because interrupts feed directly back into execution.

### PPU data plane

This is the part we should move first:

- background scanline render
- window scanline render
- sprite scanline render
- backing frame buffer storage
- optional line-to-platform format conversion later

In the current implementation this corresponds mainly to:

- `render_bg_scanline`
- `render_window_scanline`
- `render_sprite_scanline`
- `draw_sprite`

### PPU job model

Core 0 should stop calling the renderer directly.

Instead, at the end of pixel transfer for a line, it emits a render job:

```rust
pub struct RenderScanlineJob {
    pub ly: u8,
    pub window_line: u8,
    pub lcdc: u8,
    pub scx: u8,
    pub scy: u8,
    pub wx: u8,
    pub wy: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub vram_version: u32,
    pub oam_version: u32,
}
```

Core 1 keeps mirrored VRAM/OAM and renders that scanline into its own frame
buffer.

### Why this is the first PPU move

It preserves:

- local LY/STAT timing
- local interrupt timing
- local CPU-visible register reads

while still moving the most obvious compute-heavy raster work out of the CPU
hot path.

### Optional later move: full PPU ownership on core 1

Once the mirrored-memory and register-shadow machinery is stable, we can move
the PPU timing shell too:

- mode transitions
- LY advancement
- STAT generation
- VBlank/STAT interrupt output

At that point core 1 becomes the authoritative PPU owner and core 0 consumes:

- `ly`
- `stat`
- `if_set_bits`
- `frame_ready`

But this should be a second stage, not the first one.

## APU Split

The APU is different from the PPU. It is more natural to push almost the whole
engine to core 1, because the heavy work and the authoritative state are more
tightly fused.

### APU control shim on core 0

Core 0 keeps a small shim responsible for:

- intercepting MMIO reads/writes in CPU time order
- tagging writes with exact T-cycle timestamps
- synchronizing wave RAM accesses at T3
- holding the CPU-visible readback shadow for NRxx and wave RAM
- merging returned `NR52` / readback updates into the local IO shadow

### APU engine on core 1

Core 1 should own:

- powered state
- frame sequencer step
- 2 MHz wave phase
- channel 1/2/3/4 internals
- wave RAM authoritative contents
- sample accumulator
- mixer state
- sample output buffer

That is essentially the current `ApuPeripheral` state.

### Why the APU is harder than it looks

The current core relies on:

- `apu.tick(cycles, div_counter_after)`
- `flush_apu_before_bus_events()`
- T3-sensitive `read_wave_ram` / `write_wave_ram`

So a remote APU must see:

- exactly when writes happened
- exactly what the timer internal counter was after each batch
- exactly when the CPU touched wave RAM

### APU command model

Normal path:

```rust
pub struct ApuAdvanceBatch {
    pub end_cycle: u64,
    pub cycles: u16,
    pub div_counter_after: u16,
}

pub enum ApuEvent {
    RegWrite { at_cycle: u64, addr: u16, value: u8 },
    WaveWrite { at_cycle: u64, offset: u8, value: u8 },
}
```

Blocking path for rare but timing-sensitive reads:

```rust
pub enum ApuRpc {
    ReadRegAt { at_cycle: u64, addr: u16 },
    ReadWaveAt { at_cycle: u64, offset: u8 },
    SaveState,
    LoadState(...),
    Reset,
}
```

The common path is asynchronous. The weird timing cases are synchronous.

## Shared Cross-Core Infrastructure

## 1. Replace `BusEvent` with a timestamped peripheral event

Today `BusEvent` only has:

- `address`
- `value`

That is enough for the current same-core routing model, but not for a remote
peripheral core.

We need something like:

```rust
pub struct PeripheralEvent {
    pub at_cycle: u64,
    pub kind: PeripheralEventKind,
}

pub enum PeripheralEventKind {
    IoWrite { addr: u16, value: u8 },
    VramWrite { offset: u16, value: u8 },
    OamWrite { offset: u16, value: u8 },
    WaveWrite { offset: u8, value: u8 },
    LyReset,
    DmaStart { page: u8 },
}
```

This is the most important prerequisite in the whole plan.

## 2. Mirror VRAM and OAM to core 1

The current PPU reads live `&[u8]` slices from `GameBoyMemory`.

That cannot work across cores.

Core 1 needs its own:

- `vram_mirror: [u8; 0x2000]`
- `oam_mirror: [u8; 0xA0]`

Core 0 updates them by emitting timestamped writes for:

- CPU writes to `0x8000..=0x9FFF`
- CPU writes to `0xFE00..=0xFE9F`
- OAM DMA byte copies
- save/load/reset

## 3. Keep CPU-visible register shadows on core 0

Core 0 still needs fast local reads for:

- `LY`
- `STAT`
- `NR52`
- wave RAM readback

So core 1 must publish a returned shadow:

```rust
pub struct PeripheralShadow {
    pub ly: u8,
    pub stat: u8,
    pub nr52: u8,
    pub if_set_bits: u8,
}
```

Core 0 merges these into local IO/IF state at sync points.

## 4. Add explicit sync points

We do not want to fence every M-cycle if we can avoid it.

We do need to fence at points where CPU-visible correctness depends on
up-to-date peripheral state:

- before reading `LY` / `STAT`
- before reading APU registers or wave RAM
- before `has_pending_interrupt()` / `take_pending_interrupt()`
- during HALT wake checks
- before save/load/reset

Normal rendering/audio production should remain asynchronous between those points.

## Two Viable Execution Models

## Model A: Full peripheral-core ownership

Core 1 owns the authoritative PPU and APU.

Core 0 sends:

- timed writes
- cycle advance batches
- synchronous read RPCs when needed

Core 1 returns:

- updated shadows
- IF bits to OR into `IF`
- frame and audio buffers

### Pros

- maximum theoretical offload
- clean ownership once stable

### Cons

- highest correctness risk
- highest synchronization complexity
- requires more CPU/peripheral handshake on reads and interrupts

## Model B: Control-plane/data-plane split

Core 0 keeps PPU timing and interrupt shell.
Core 1 renders scanlines and runs the APU synthesis engine.

### Pros

- lower sync cost
- preserves local interrupt timing
- smaller first step

### Cons

- lower ceiling than full remote ownership
- leaves some PPU cost on core 0

## Recommendation

If the goal is "bigger than a modest gain" but still shippable, use:

- **Model B for PPU first**
- **near-Model A for APU**

That is:

- keep PPU control local at first, move scanline rendering out
- move the APU engine out behind a local bus shim

This gives us a real path to larger gains without forcing the hardest possible
PPU correctness problem on day one.

## Core 0 / Core 1 Responsibility Split

## Core 0

- `Sm83` execution
- `GameBoyMemory`
- cartridge / MBC / RTC
- timer
- serial
- joypad
- IF/IE ownership
- PPU control plane initially
- APU bus shim / readback shadow
- timed event emission

## Core 1

- PPU raster plane
- APU engine
- mirrored VRAM/OAM
- frame buffer ownership
- audio sample buffer ownership
- optional output conversion later

## Migration Plan

## Phase 0: Timestamp everything in single-core mode

Before touching multicore:

1. Replace `BusEvent` with `PeripheralEvent { at_cycle, kind }`.
2. Emit timed events for VRAM and OAM writes, not just IO writes.
3. Route those events back into the current same-core peripherals so behavior
   does not change yet.

This is the prerequisite for every later phase.

## Phase 1: Split PPU into timing shell and renderer

Refactor `ppu.rs` into:

- `PpuTiming`
- `PpuRenderer`
- `PpuFrameBuffer`

`PpuTiming` should stop directly owning all raster logic.

The first target is:

- core 0 computes when a line must be rendered
- core 1 renders that line from mirrored VRAM/OAM

## Phase 2: Introduce APU engine boundary

Refactor `apu.rs` into:

- `ApuBusShim`
- `ApuEngine`
- `ApuMixer`

`ApuEngine` gets:

- timed writes
- `div_counter_after`
- batch advance requests

and returns:

- `nr52`
- optional readback shadow updates
- audio sample buffers

## Phase 3: Add multicore transport on Pico

Use:

- `embassy_rp::multicore::spawn_core1`
- fixed static stack
- one SPSC queue core0 -> core1 for event batches
- one SPSC queue core1 -> core0 for shadows / IRQ effects / ready buffers
- one tiny synchronous mailbox for blocking MMIO reads and save-state fencing

## Phase 4: Move PPU raster + APU engine to core 1

At this point core 1 should own:

- scanline render
- audio advance/mix
- frame/audio buffer completion

Measure again before moving any more shell logic.

## Phase 5: Decide whether full PPU ownership is worth it

Only after profiling the split system should we decide whether to also move:

- LY/dot/mode bookkeeping
- STAT generation
- VBlank/STAT interrupt output

If the control-plane shell is still a major wall after raster offload, then
moving full PPU ownership becomes worth the added complexity.

## Required Refactors Before Any Multicore Work

1. Add timestamps to peripheral-side events.
2. Add explicit VRAM/OAM mirror events.
3. Separate PPU timing from scanline rendering.
4. Separate APU bus semantics from synthesis/mixing.
5. Add a first-class peripheral shadow struct instead of using only the IO array.
6. Add pause/flush/resume hooks for save state and reset.
7. Extend save-state support to cover full APU internal state.

That last point matters because the current save-state path serializes CPU,
timer, PPU, and memory, but not the full APU internal engine state.

## What I Would Build First

If we want to pursue this direction now, I would implement in this order:

1. `PeripheralEvent` with timestamps.
2. PPU timing/render split in single-core mode.
3. APU shim/engine split in single-core mode.
4. Pico core-1 transport.
5. Move APU engine to core 1.
6. Move PPU raster plane to core 1.
7. Re-profile before considering full PPU ownership migration.

That keeps the architecture moving toward the high-upside design without making
the hardest cross-core correctness jump all at once.
