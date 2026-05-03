# Display DMA Implementation Plan

## Goal

Hide the ILI9341 SPI frame transfer behind emulation work using async DMA, eliminating the
largest single wall-clock bottleneck. Target: ~58 fps at 250 MHz (up from ~9 fps).

## Context

From the performance-roadmap baseline (Tetris, 250 MHz):

| Bucket | Time / 60 frames | % of wall clock |
|---|---|---|
| Emulation | 3.52 s | 53% |
| Display + other | 3.15 s | 47% |
| **Total** | **6.67 s** | **100%** |

The display path currently blocks Core 0 for the full SPI transfer. `fill_scaled_frame` runs
51,840 iterator steps of multiply/divide + palette lookup while the CPU waits on SPI — Core 0
does both the pixel math and the bus stall simultaneously. This makes ~13 ms of hardware-limited
SPI time appear as much more wall-clock cost.

## Design decisions

**DMA on Core 0, not Core 1.** A second Cortex-M33 at 250 MHz is a compute resource. Spending
it to replace what a DMA controller does for free wastes it and prevents future use for APU or
instruction-level tick batching (roadmap priorities 2 and 5). The only advantage of Core 1 would
be avoiding the mipidsi async transition; that transition is scoped to one method in `hw.rs` and
is not worth the trade-off.

**Keep mipidsi for the splash.** The splash runs once and its timing doesn't matter. Only the
game-loop hot path bypasses mipidsi.

**Single pre-scaled u16 buffer.** Pre-scaling is fast (~0.5 ms) and DMA takes ~13 ms. A single
103 KB buffer suffices: Core 0 starts the DMA future, runs emulation (~16.7 ms), pre-scales the
next frame, then awaits the DMA future (which is already done). No double-buffering needed.

**DMA_CH1 for display.** Audio I2S already uses DMA_CH0 via PIO. SPI1 display gets DMA_CH1.

---

## Phase A — Profile the display path

**Goal:** measure the split between SPI transfer time and iterator/scaling overhead inside
`render_game_only`, so we know which of Phase B and C matters more.

**What to do:**

Add DWT instrumentation around the `render_game_only` call in `main.rs` under the existing
`#[cfg(feature = "perf")]` gate. The `perf` module already exposes `read_dwt` primitives — add
a `render` counter alongside the existing emulation sub-breakdown counters and log it in the
same 60-frame summary.

**Expected outcome:** either the iterator overhead dominates (Phase B has large standalone
impact) or the raw SPI time dominates (skip straight to Phase C).

---

## Phase B — Pre-scaled framebuffer

**Goal:** decouple per-pixel CPU work from SPI timing. Effective even with blocking SPI; also
required for Phase C.

**What to do:**

1. Add to `display/mod.rs`:
   ```rust
   pub fn scale_to_rgb565(src: &[u8; 23040], dst: &mut [u16; 51840]) {
       for i in 0..(240 * 216) {
           let sx = (i % 240) * 2 / 3;
           let sy = (i / 240) * 2 / 3;
           let idx = src[sy * 160 + sx];
           dst[i] = dmg_color(idx).into_storage().to_be();
       }
   }
   ```
   Keep the existing `fill_scaled_frame` path for host tests. Update `render_game_only` to
   accept `&[u16; 51840]` alongside the existing `&[u8; 23040]` overload (or add a new method).

2. Allocate the buffer at startup via `Box::leak`:
   ```rust
   let frame_buf: &'static mut [u16; 51840] =
       Box::leak(Box::new([0u16; 51840]));
   ```
   This avoids reducing the static heap size and keeps the heap intact for ROM/CPU allocations.

3. Replace `hw_disp.inner.render_game_only(cpu.framebuffer())` with:
   ```rust
   scale_to_rgb565(cpu.framebuffer(), frame_buf);
   hw_disp.inner.render_game_only_scaled(frame_buf);
   ```
   `render_game_only_scaled` calls `fill_contiguous` with the pre-computed u16 slice — one
   HAL call for the whole frame, no per-pixel iterator.

**Expected outcome:** iterator overhead eliminated. Measure again under `perf` to confirm.

---

## Phase C — Async DMA transfer

**Goal:** start the SPI transfer at VBlank and run emulation concurrently, hiding the ~13 ms
transfer behind the ~16.7 ms emulation window.

### Step 1 — Switch SPI1 to async mode

In `display/hw.rs`, replace:
```rust
// before
use embassy_rp::spi::{Blocking, Config as SpiConfig, Spi};
let spi = Spi::new_blocking_txonly(spi1, clk, mosi, cfg);
```
with:
```rust
// after
use embassy_rp::spi::{Async, Config as SpiConfig, Spi};
use embassy_rp::peripherals::DMA_CH1;
let spi = Spi::new_txonly(spi1, clk, mosi, dma_ch1, cfg);
```

Update `HwDisplay::new()` to accept a `Peri<'d, DMA_CH1>` parameter.

Add `DMA_CH1` to the interrupt binding in `main.rs`:
```rust
bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioIrqHandler<PIO0>;
    DMA_IRQ_0  => dma::InterruptHandler<DMA_CH0>;
    DMA_IRQ_1  => dma::InterruptHandler<DMA_CH1>;
});
```

**Note:** `Spi<Async>` implements `embedded_hal_async::spi::SpiBus`, not the blocking
`embedded_hal::spi::SpiBus` that `display-interface-spi 0.5` expects. The splash animation
still calls mipidsi drawing primitives. Verify at compile time whether `mipidsi 0.8` accepts
an async SPI via `ExclusiveDevice<Spi<Async>, ...>`. If not, the splash must be ported to raw
commands (see Step 2), or driven via a blocking spin wrapper around the async SPI.

### Step 2 — Add `send_frame_raw()`

Add a method to `HwDisplay` that bypasses mipidsi for the hot path and drives the ILI9341
window protocol directly:

```rust
pub async fn send_frame_raw(&mut self, buf: &[u16; 51840]) {
    // Set column address: 0..239
    self.write_command(0x2A, &[0x00, 0x00, 0x00, 0xEF]);
    // Set row address: 52..267  (Y_OFFSET = 52, SCALED_H = 216)
    self.write_command(0x2B, &[0x00, 0x34, 0x01, 0x0B]);
    // Begin memory write
    self.write_command(0x2C, &[]);
    // DMA transfer — assert DC high (data), blast 103,680 bytes
    self.dc.set_high();
    self.spi_dev.write(bytemuck::cast_slice(buf)).await.ok();
}
```

`write_command` sends one command byte (DC low) followed by optional parameter bytes (DC high),
using the existing CS/DC GPIO pins. The exact byte values come from the ILI9341 datasheet
(CASET/PASET column/row window registers).

### Step 3 — Wire the game loop

Replace the blocking render call with the overlapped DMA pattern:

```rust
loop {
    // Start DMA transfer of the pre-scaled frame buffer (returns immediately).
    let disp_future = hw_disp.send_frame_raw(frame_buf);

    // Run one full Game Boy frame while DMA is in flight.
    let frame_start = cpu.cycle_counter();
    while cpu.cycle_counter().wrapping_sub(frame_start) < CYCLES_PER_FRAME {
        let _ = cpu.tick();
    }

    // Handle input, fill audio back-buffer.
    // ...

    // Pre-scale next frame into the buffer (fast, ~0.5 ms).
    scale_to_rgb565(cpu.framebuffer(), frame_buf);

    // Await DMA — should already be complete; this is just the join point.
    disp_future.await;

    watchdog.feed(...);
}
```

This is structurally identical to how the audio DMA future is handled today.

---

## Expected outcome

At 250 MHz:

| | Time / frame | fps |
|---|---|---|
| Baseline (current) | ~111 ms | ~9 |
| After Phase B | ~30 ms (est.) | ~33 |
| After Phase C | ~17 ms (emulation dominates) | ~58 |

The display transfer (~13 ms) is fully hidden behind the emulation window (~16.7 ms). The
remaining gap to 60 fps is emulation cost alone, which is addressed by roadmap priorities 2–5.

---

## Files touched

| File | Change |
|---|---|
| `src/display/mod.rs` | Add `scale_to_rgb565`, `render_game_only_scaled` |
| `src/display/hw.rs` | Switch to async SPI, add `send_frame_raw`, CS/DC raw access |
| `src/main.rs` | DMA_CH1 binding, `Box::leak` buffer, game loop restructure |
| `src/perf.rs` | Phase A: render DWT counter |
