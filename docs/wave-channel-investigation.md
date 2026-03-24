# Wave Channel T-Cycle Accuracy Investigation

## Status

**RESOLVED — 12/12 blargg dmg_sound tests pass.** Tests 09, 10, and 12 now pass after two root-cause fixes described in Session 3 below.

### Previous state (before fix)

| Test | Expected CRC | Our CRC (before fix) | Description |
|------|-------------|---------------------|-------------|
| 09 - wave read while on | `$118A3620` | `$12821B56` | CPU reads wave RAM while ch3 is playing |
| 10 - wave trigger while on | `$533D6D4D` | `$048BADFB` | Retrigger ch3 while active (DMG wave RAM corruption quirk) |
| 12 - wave write while on | `$3B4538A9` | `$0F95D27E` | CPU writes wave RAM while ch3 is playing |

All three tests use the same structure: trigger ch3 at a varying initial frequency, change to a fast period (4 T-cycles), then interact with wave RAM after a fixed delay. The varying initial frequency shifts the wave channel's phase by 2 T-cycles per iteration, probing sub-M-cycle timing.

---

## What the Tests Do

### Common setup (all 3 tests)
```asm
; 69 iterations. Each adds $99 to 'a', shifting wave phase by 2 T-cycles.
test:
     add  $99
     ld   b,a
     ld   hl,wave
     call load_wave       ; writes NR30=$00 (disables ch3), writes 16 bytes to wave RAM
     wreg NR30,$80        ; re-enable DAC
     wreg NR32,$00        ; silent volume
     ld   a,b
     sta  NR33            ; set initial frequency lo (varying per iteration)
     wreg NR34,$87        ; trigger ch3 with freq hi = 7
     wreg NR33,-2         ; change freq to $7FE → period = 4 T-cycles (2 2MHz-cycles)
     delay_clocks N       ; fixed delay
     ; ... then read/write/retrigger wave RAM
```

The macros expand to: `wreg` = `ld a, imm8` (2M) + `ldh (n), a` (3M) = 5 M-cycles. `sta` = `ldh (n), a` = 3M. `lda` = `ldh a, (n)` = 3M.

### Test 09 — wave read while on
After `delay_clocks 176` (44M), reads wave RAM byte 0 with `lda WAVE`. On DMG, this should return `0xFF` unless the read coincides with the exact 2MHz cycle when the wave channel reads its sample. When it does coincide, it returns the byte the wave channel is currently reading (not the byte at the requested offset).

### Test 10 — wave trigger while on
After `delay_clocks 168` (42M), retriggers ch3 with `wreg NR34,$87`, then waits 40 more clocks. On DMG, retriggering while ch3 is active and the wave position is about to advance causes **wave RAM corruption** — the first 4 bytes of wave RAM get overwritten based on the current wave position.

### Test 12 — wave write while on
After `delay_clocks 168` (42M), writes `$F7` to wave RAM byte 0 with `wreg WAVE,$F7`. On DMG, the write only succeeds if it coincides with the wave channel's sample read. When it does, it writes to the byte the wave channel is currently reading (not byte 0), producing a pattern where `$F7` appears at different wave RAM offsets each iteration.

---

## What We See vs What's Expected

### Test 09
**Our output:** `00 11 11 11 11 22 22 22 22 33 33 33 33 ... FF FF FF FF 00 00 00 00 11 11 11 11`
- Pattern: groups of 4 identical values, progressing through the wave table
- Each sample byte appears 4 times consecutively (except the first, `00`, which appears once)
- This is **M-cycle resolution** — position changes every 4 iterations

**Expected:** Pattern with 2 T-cycle resolution, containing a mix of actual sample values and `0xFF` values. The `just_read` window should make some reads return `0xFF` and others return the sample buffer, with the coincidence pattern shifting by one 2MHz tick per iteration.

### Test 10
**Our output:** Every iteration prints the unmodified wave pattern `00 11 22 33 44 55 66 77 88 99 AA BB CC DD EE FF`.

**Expected:** The wave RAM corruption quirk should produce corrupted patterns on some iterations where the retrigger coincides with the wave channel's sample read.

### Test 12
**Our output:** Shows `$F7` appearing at the correct wave RAM offset (progressing from byte 1 through byte 15 and wrapping), but only on alternating iterations. The pattern advances one byte every 4 iterations.

**Expected:** Same general structure but with 2 T-cycle resolution instead of 4.

---

## Root Cause Analysis

### Issue 1: Wave channel ticks at 4 MHz instead of 2 MHz

**Pandocs states clearly:**
> "The wave channel's period divider is clocked at 2097152 Hz, **once per two dots**."

Two dots = 2 T-cycles. The wave frequency timer should decrement once every 2 T-cycles (2 MHz), not every T-cycle (4 MHz).

**Our code** (`apu.rs:212-227`):
```rust
fn clock_frequency(&mut self) {
    // Called every T-cycle from tick() → clocking at 4 MHz!
    self.just_read = false;
    if !self.enabled { return; }
    if self.frequency_timer > 0 { self.frequency_timer -= 1; }
    if self.frequency_timer == 0 {
        self.frequency_timer = (2048 - self.frequency) * 2;  // compensates with *2
        self.position = (self.position + 1) % 32;
        // ...
    }
}
```

We compensate with `* 2` in the reload value, so the effective period in T-cycles is the same: `(2048 - freq) * 2`. However, the `just_read` flag is 1 T-cycle wide, not 2. This matters for the coincidence window.

**SameBoy** (`apu.c:983-1003`):
```c
// Cycles here are 2MHz-cycles (incremented by 2 per M-cycle on DMG)
gb->apu.wave_channel.wave_form_just_read = false;
if (gb->apu.is_active[GB_WAVE]) {
    uint16_t cycles_left = cycles;
    while (cycles_left > gb->apu.wave_channel.sample_countdown) {
        cycles_left -= gb->apu.wave_channel.sample_countdown + 1;
        gb->apu.wave_channel.sample_countdown = gb->apu.wave_channel.sample_length ^ 0x7FF;
        // advance position, read sample...
        gb->apu.wave_channel.wave_form_just_read = true;
    }
    if (cycles_left) {
        gb->apu.wave_channel.sample_countdown -= cycles_left;
        gb->apu.wave_channel.wave_form_just_read = false;  // clear if more cycles remain
    }
}
```

SameBoy processes wave channel in **2 MHz cycles**. The `wave_form_just_read` flag is true only if the position advance happened on the **last 2MHz cycle** of the batch. Since `GB_apu_run` is called right before wave RAM reads, this creates an exact 2MHz-cycle-wide coincidence window.

**SameBoy timing accumulation** (`timing.c:279`):
```c
gb->apu.apu_cycles += 1 << !gb->cgb_double_speed;  // = 2 on DMG (non-double-speed)
```
APU cycles accumulate by 2 per M-cycle, meaning the APU works in 2 MHz cycle units.

### Issue 2: `just_read` window is 1 T-cycle, needs to be ~2 T-cycles

Even if we keep the 4 MHz internal ticking with `*2` reload (functionally equivalent period), the `just_read` flag clears after 1 T-cycle. The bus_read's 3/1 split means the read point is at a fixed T-cycle offset within the M-cycle. With a 1 T-cycle window, the coincidence probability is 1/4 per period tick. With a 2 T-cycle window (matching 2 MHz), it would be 2/4 = 1/2.

The test shifts by 2 T-cycles per iteration. With a 2 T-cycle window, consecutive iterations should alternate between coincident and non-coincident. With a 1 T-cycle window, every other pair alternates — producing the observed "groups of 4" pattern.

### Issue 3: Wave RAM read returns `sample_buffer` unconditionally (no `just_read` gating)

**Current code** (`apu.rs:382-393`):
```rust
pub fn read_wave_ram(&self, offset: u8) -> u8 {
    if self.channel3.enabled {
        // Currently returns sample_buffer always — no just_read check
        self.channel3.sample_buffer
    } else {
        self.channel3.wave_ram[offset as usize]
    }
}
```

**Pandocs:**
> "On monochrome consoles, wave RAM can only be accessed on the same cycle that CH3 does. Otherwise, reads return $FF, and writes are ignored."

**SameBoy** (`apu.c:1128-1136`):
```c
if (reg >= GB_IO_WAV_START && reg <= GB_IO_WAV_END && gb->apu.is_active[GB_WAVE]) {
    if (!GB_is_cgb(gb) && !gb->apu.wave_channel.wave_form_just_read) {
        return 0xFF;
    }
    // redirect to the byte the wave channel is currently reading
    reg = GB_IO_WAV_START + gb->apu.wave_channel.current_sample_index / 2;
}
```

On DMG: returns `0xFF` unless `wave_form_just_read` is true. When true, returns the byte at the wave channel's current read position (ignoring the requested offset).

### Issue 4: Wave RAM writes have same timing issue

**Pandocs:** same-cycle-only access applies to writes too.

**SameBoy** (`apu.c:1696-1700`):
```c
if (reg >= GB_IO_WAV_START && reg <= GB_IO_WAV_END && gb->apu.is_active[GB_WAVE]) {
    if ((!GB_is_cgb(gb) && !gb->apu.wave_channel.wave_form_just_read) || ...) {
        return;  // write ignored
    }
    reg = GB_IO_WAV_START + gb->apu.wave_channel.current_sample_index / 2;  // redirect
}
```

### Issue 5: DMG wave RAM corruption on retrigger (test 10)

Test 10 specifically tests the DMG quirk where retriggering ch3 while it's active and the wave position is about to advance corrupts wave RAM.

**SameBoy** (`apu.c:1982-2000`):
```c
if (!GB_is_cgb(gb) && gb->apu.is_active[GB_WAVE] &&
    gb->apu.wave_channel.sample_countdown == 0) {
    unsigned offset = ((gb->apu.wave_channel.current_sample_index + 1) >> 1) & 0xF;
    if (offset < 4 && gb->model != GB_MODEL_MGB) {
        gb->io_registers[GB_IO_WAV_START] = gb->io_registers[GB_IO_WAV_START + offset];
    } else {
        memcpy(gb->io_registers + GB_IO_WAV_START,
               gb->io_registers + GB_IO_WAV_START + (offset & ~3), 4);
    }
}
```

This quirk is **not implemented** in our code at all.

### Issue 6: First sample after trigger is index 1, not index 0

**Pandocs:**
> "When CH3 is started, the first sample read is the one at index 1, i.e. the lower nibble of the first byte, NOT the upper nibble."

**Our trigger code:**
```rust
fn trigger(&mut self) {
    // ...
    self.position = 0;  // First advance will go to position 1 ✓
}
```

This is correct — position starts at 0 and the first `clock_frequency` advance goes to 1. However, the sample_buffer is NOT updated at trigger, meaning `sample_buffer` retains its stale value until the first advance. This matches hardware behavior per Pandocs: "the last sample ever read is output until the channel next reads a sample."

---

## How the Code Flows Now vs How It Should Flow

### Current flow for `lda WAVE` (bus_read of 0xFF30):

```
bus_read(0xFF30):
  cycle_counter += 4
  route_bus_events()          // process pending writes from previous M-cycles
  advance_ppu(4)              // PPU gets full M-cycle
  advance_timer(1) × 3       // 3 T-cycles of timer
  advance_apu(1) × 3         // 3 T-cycles of APU (wave ticks 3 times at 4 MHz!)
  value = apu.read_wave_ram(0)  // reads sample_buffer (no just_read check)
  advance_timer(1)            // 1 more T-cycle
  advance_apu(1)              // 1 more T-cycle
  return value
```

### What should happen:

```
bus_read(0xFF30):
  cycle_counter += 4
  route_bus_events()
  advance_ppu(4)
  // Advance APU at 2 MHz (1 tick per 2 T-cycles):
  advance_timer(1), advance_timer(1)  // 2 T-cycles
  advance_apu_2mhz(1)                 // 1 wave tick
  advance_timer(1)                     // T-cycle 3
  // Read point — check if wave_form_just_read is true
  value = apu.read_wave_ram(0)
  // If just_read: return sample_buffer
  // If !just_read: return 0xFF
  advance_timer(1)                     // T-cycle 4
  advance_apu_2mhz(1)                 // 1 wave tick
  return value
```

The exact placement of the 2 MHz ticks within the M-cycle matters for the coincidence window. SameBoy processes all accumulated APU cycles in one batch right before the read, so the `just_read` flag reflects the final state.

---

## Implementation Attempt & Results

### Changes Made

Three changes were implemented based on the analysis above:

#### 1. 2 MHz wave channel divider (`apu.rs`)

Added `wave_2mhz_phase: bool` to `ApuPeripheral`. Inside `tick()`, this toggles each T-cycle and `channel3.clock_frequency()` is only called when it's `true`. The `frequency_timer` reload was changed from `(2048 - freq) * 2` to `(2048 - freq)` since it now ticks at half the rate.

```rust
self.wave_2mhz_phase = !self.wave_2mhz_phase;
if self.wave_2mhz_phase {
    self.channel3.clock_frequency();
}
```

The `just_read` flag is cleared at the start of each 2 MHz tick and set on position advance — giving a **2 T-cycle coincidence window** (the tick T-cycle plus the next non-tick T-cycle before `just_read` is cleared again).

#### 2. Gate `read_wave_ram` / `write_wave_ram` on `just_read`

```rust
pub fn read_wave_ram(&self, offset: u8) -> u8 {
    if self.channel3.enabled {
        if self.channel3.just_read { self.channel3.sample_buffer } else { 0xFF }
    } else {
        self.channel3.wave_ram[offset as usize]
    }
}
```

Same pattern for writes (ignored unless `just_read`; redirected to current wave position).

#### 3. DMG corruption quirk on retrigger (`apu.rs`, NR34 handler)

When ch3 is active and `just_read` is true at trigger time:

```rust
if self.channel3.enabled && self.channel3.just_read {
    let next_pos_byte = ((self.channel3.position + 1) / 2) as usize & 0x0F;
    if next_pos_byte < 4 {
        self.channel3.wave_ram[0] = self.channel3.wave_ram[next_pos_byte];
    } else {
        let block_start = next_pos_byte & !3;
        // copy 4-byte block to wave_ram[0..4]
    }
}
```

### Results After Initial Changes (2 MHz + just_read + corruption quirk)

| Test | Before | After | Expected |
|------|--------|-------|----------|
| 09 - wave read | `7431D750` | `12821B56` | `118A3620` |
| 10 - wave trigger | `8130733A` | `048BADFB` | `533D6D4D` |
| 12 - wave write | `0F95D27E` | `0F95D27E` | `3B4538A9` |

All 9 previously-passing tests continue to pass (no regressions).

### Analysis of Remaining Failures (after initial changes)

#### Test 09 — Phase offset by 1 iteration

Output: `FF 11 FF 11 FF 22 FF 22...` — correct alternating structure but **phase-shifted by one iteration**. First value should be `11` (coincident), not `FF` (non-coincident). Off by exactly 1 2MHz-cycle (2 T-cycles).

#### Test 10 — Corruption fires but phase-shifted

Corruption block structure visible and matches SameBoy's formula. CRC wrong due to same phase issue as test 09.

#### Test 12 — CRC unchanged

Write pattern structure correct (F7 at progressively shifting offsets) but CRC unchanged — same phase issue.

---

## Session 2 Attempts

### Attempt: Change `bus_read` wave RAM timing (T1 vs T3)

A diagnostic unit test (`probe_wave_phase_at_read`) was written to simulate test 09 iteration 1 at APU level and probe `just_read` at each T-cycle of the bus_read M-cycle:

```
T1=(0x66, pos=13, timer=2, just_read=true)   ← position just advanced
T2=(0x66, pos=13, timer=2, just_read=true)   ← still in window
T3=(0xFF, pos=13, timer=1, just_read=false)  ← window closed
T4=(0xFF, pos=13, timer=1, just_read=false)
T5=(0x77, pos=14, timer=2, just_read=true)   ← next advance
```

The original T3 read misses `just_read` for iteration 1. Changing to T1 read gives:

| Read timing | Test 09 CRC | Test 09 output first values |
|-------------|-------------|----------------------------|
| T3 (original) | `12821B56` | `FF 11 FF 11 FF 22 FF 22...` |
| T1 (changed)  | `503AFF9B` | `00 FF 11 FF 11 FF 22 FF...` |
| T2 (changed)  | `503AFF9B` | same (T1 and T2 are in the same just_read window) |

**T1 and T2 give identical results** (same 2MHz window). T3 and T4 give the other phase. Neither matches expected `118A3620`.

The issue: the test expects position 3 (`value=0x11`) to be coincident for iteration 1, but we see position 1 (`0x00`) at T1 and position 13 (`0x66`) at T3. Neither matches `0x11`.

**Conclusion:** The `bus_read` T-cycle offset is NOT the phase problem. The position itself is wrong — the wave is at a different position than expected at the read point.

### Attempt: Timer reload `(2047 - freq)` = `freq ^ 0x7FF`

SameBoy reloads `sample_countdown = freq ^ 0x7FF = 2047 - freq` (vs our `2048 - freq`). Tried applying this to both trigger and `clock_frequency` reload.

Result: Test 09 CRC `AA06E9A4`, output `11 22 22 33 33 44 44...` — **groups of 2, no `0xFF` values**. The `just_read` window is never coincident with the read point. The shorter reload period shifted the wave position enough that `just_read` is always false at T3.

This reload is incompatible with our per-tick decrement model. SameBoy's `countdown + 1` consumption gives the same period as our `2048 - freq` reload with per-tick decrement — **these are NOT equivalent when the reload value alone is changed**. Reverted.

### The Phase Problem — Updated Understanding

The diagnostic unit test gives iteration 1 position=13 at T3. But the test expects to see `0x11` (wave_ram byte 1, positions 2-3) for the first coincident read. Position 13 is wave_ram[6] = `0x66`.

This means the wave position at the read point is **completely wrong** — not just off by 1, but off by ~10 positions. The `probe_wave_phase_at_read` unit test may not accurately reflect the actual test sequence, or the T-cycle count between trigger and read in the unit test is incorrect.

**Key uncertainty:** The unit test simulates 64 T-cycles for `bus_write NR33` (before the NR33=-2 write) as "16 M-cycles". But the actual test ROM instruction sequence between `wreg NR34,$87` and `wreg NR33,-2` may be different. If the delay is off, the wave position at the freq change will be wrong, cascading into a wrong position at the read.

---

## Session 3 — Root Cause Found

### Discovery: Diagnostic unit test had wrong T-cycle count

The `probe_wave_phase_at_read` test simulated **64 T-cycles** (16 M-cycles) between the NR34 trigger and the NR33=-2 frequency change. The actual instruction sequence is only **16 T-cycles** (4 M-cycles):

```
wreg NR33,-2  expands to:
  ld a,$FE         → 2 M-cycles (opcode fetch + immediate read) = 8T
  ldh ($1D),a      → 3 M-cycles (opcode fetch + read_n + bus_write) = 12T

Between NR34 bus_write completion and NR33 bus_write (the write moment):
  1T  remaining in NR34 M-cycle
  4T  ld a,$FE opcode fetch (tick_cycle)
  4T  ld a,$FE immediate read (tick_cycle)
  4T  ldh opcode fetch (tick_cycle)
  4T  ldh read_n (tick_cycle)
  3T  NR33 bus_write T1-T3 (advance_apu × 3)
  --- freq change applied here ---
  = 20T total (10 2MHz-ticks)
```

The test used 64T = 32 2MHz-ticks, which is **22 extra 2MHz-ticks**. This consumed more of the initial timer, causing 11 extra position advances — explaining why pos=13 instead of the expected pos=2.

### Discovery: Wave trigger needs +3 frequency timer delay

The wave channel `trigger()` reloads `frequency_timer = 2048 - freq`. On real DMG hardware, the trigger adds **+3 extra 2MHz-cycles** to the initial timer reload. This is a well-documented DMG quirk.

The correct formula: `frequency_timer = (2048 - freq) + 3`

This +3 does NOT apply to the normal `clock_frequency()` reload — only to trigger.

### Verification via CRC computation

With the corrected T-cycle count and +3 trigger delay, the exact expected output for test 09 can be computed mathematically:

**Per-iteration model (test 09):**
- `freq_lo = (iter + 0x99) & 0xFF` → freq_lo increments by 1 per iteration (0x99, 0x9A, 0x9B, ...)
- `initial_timer = (2048 - (0x700 | freq_lo)) + 3 = (256 - freq_lo) + 3` → decreases by 1 per iteration
- `timer_at_freq_change = initial_timer - 10` (10 2MHz-ticks between trigger and freq change)
- `ticks_from_freq_change_to_read = 94` (188T / 2T per tick)
- If `timer_at_freq_change > 94`: no position advance → `0xFF`
- Else: `advances = 1 + (94 - timer_at_freq_change) / 2`, `remaining = (94 - timer_at_freq_change) % 2`
  - `just_read = (remaining == 0)` → if true, return `wave_ram[position/2]`; if false, return `0xFF`

**First 20 bytes of expected output:** `FF FF 00 FF 11 FF 11 FF 22 FF 22 FF 33 FF 33 FF 44 FF 44 FF`

**CRC verification:**
| Test | Computed CRC | Expected CRC | Match? |
|------|-------------|-------------|--------|
| 09 - wave read | `$118A3620` | `$118A3620` | **YES** |
| 12 - wave write | `$3B4538A9` | `$3B4538A9` | **YES** |
| 10 - wave trigger | `$3CB7C715` | `$533D6D4D` | No |

Tests 09 and 12 are confirmed solvable by the +3 trigger delay alone (single line change in `WaveChannel::trigger()`).

Test 10 requires additional investigation of the retrigger corruption quirk. The corruption formula and/or coincidence condition may differ from our current implementation. SameBoy uses `sample_countdown == 0` (timer at zero, about to advance) rather than `just_read` (just advanced). These have different timing windows.

### Key insight: SameBoy `sample_countdown == 0` vs our `just_read`

In SameBoy's corruption check:
```c
if (gb->apu.is_active[GB_WAVE] && gb->apu.wave_channel.sample_countdown == 0)
```
This fires when the timer is at 0 **before** the next advance. In our model, `just_read` is true **after** the advance. The position values differ: SameBoy uses the position BEFORE the advance that's about to happen, while our `just_read` check uses the position AFTER the advance that just happened.

This affects the `next_pos_byte` calculation: `((position + 1) / 2) & 0xF`. If position is off by 1 (pre-advance vs post-advance), the corruption target byte shifts.

---

## Resolution Summary (2026-03-23)

All steps completed. 12/12 dmg_sound tests pass with no regressions.

### Fix 1: +3 trigger delay in `WaveChannel::trigger()` (tests 09, 12)

```rust
// Before:
self.frequency_timer = 2048 - self.frequency;
// After:
self.frequency_timer = (2048 - self.frequency) + 3;
```

DMG hardware quirk: triggering ch3 adds 3 extra 2MHz-cycles to the initial timer reload. This shifts the wave channel's phase so that after the fixed 188T delay from trigger to read, the `just_read` coincidence window falls at the correct T-cycle offsets across all 69 iterations. Normal `clock_frequency()` reloads remain unchanged at `2048 - frequency`.

### Fix 2: Retrigger corruption condition `frequency_timer == 1` (test 10)

```rust
// Before:
if self.channel3.enabled && self.channel3.just_read {
// After:
if self.channel3.enabled && self.channel3.frequency_timer == 1 {
```

SameBoy fires the corruption when `sample_countdown == 0` (about to advance). Our model decrements before checking, so the equivalent is `frequency_timer == 1` (will decrement to 0 and fire on the next 2MHz tick). Using `just_read` was wrong because it fires AFTER the advance, not before.

### Diagnostic unit test fix

The `probe_wave_phase_at_read` test had 64T (16 M-cycles) between trigger and NR33 freq change; actual instruction sequence is 16T (4 M-cycles). Fixed the loop count and updated expected values accordingly.

### Final test results

| Suite | Result |
|-------|--------|
| `cargo test --lib` (unit tests) | 686 passed, 0 failed |
| `cargo test --test blargg_dmg_sound` | 12 passed, 0 failed, 0 ignored |
| `cargo test --test blargg_cpu_instrs` | 11 passed, 0 failed |
| `cargo test --test dmg_acid2` | 1 passed, 0 failed |

---

## Key Reference Links

- **Pandocs — Audio Registers:** wave channel is clocked at 2 MHz, wave RAM access restricted to coincident cycles on DMG
- **SameBoy source — `apu.c`:** `wave_form_just_read` flag, `sample_countdown` in 2 MHz cycles, wave RAM corruption on retrigger
- **SameBoy source — `timing.c`:** `apu_cycles += 2` per M-cycle on DMG, `GB_apu_run` called lazily before reads/writes
- **SameSuite — channel_3:** test ROMs that probe wave channel timing at sub-M-cycle resolution
- **Test ROM source:** `/tmp/test-roms/gb-test-roms/dmg_sound/source/` — assembly source with macro definitions in `common/macros.inc` and `common/delay.s`
