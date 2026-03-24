# APU Architecture

This document describes how the APU is implemented in `core/src/cpu/peripheral/apu.rs`
and how it maps to the Game Boy hardware described in Pan Docs.

## Channels

The Game Boy has four sound channels. Each is represented by a dedicated struct:

| Channel | Struct | Pan Docs section |
|---------|--------|-----------------|
| CH1 — pulse with sweep | `SquareChannel` + `SweepState` | [Sound Channel 1](https://gbdev.io/pandocs/Audio_Registers.html#sound-channel-1---pulse-with-wavelength-sweep) |
| CH2 — pulse | `SquareChannel` | [Sound Channel 2](https://gbdev.io/pandocs/Audio_Registers.html#sound-channel-2---pulse) |
| CH3 — wave | `WaveChannel` | [Sound Channel 3](https://gbdev.io/pandocs/Audio_Registers.html#sound-channel-3---wave-output) |
| CH4 — noise | `NoiseChannel` | [Sound Channel 4](https://gbdev.io/pandocs/Audio_Registers.html#sound-channel-4---noise) |

## Frame Sequencer

The frame sequencer drives length counters, the volume envelope, and the frequency
sweep at sub-second rates. It is clocked by the **falling edge of bit 12** of the
timer's internal 16-bit counter (= bit 4 of the DIV register, which ticks at 512 Hz).

The sequencer cycles through 8 steps:

| Step | Length | Sweep | Envelope |
|------|--------|-------|----------|
| 0    | ✓      |       |          |
| 1    |        |       |          |
| 2    | ✓      | ✓     |          |
| 3    |        |       |          |
| 4    | ✓      |       |          |
| 5    |        |       |          |
| 6    | ✓      | ✓     |          |
| 7    |        |       | ✓        |

Pan Docs reference: [Frame Sequencer](https://gbdev.io/pandocs/Audio_details.html#frame-sequencer)

### Extra length clock on NRx4 write

When the length-enable bit (NRx4 bit 6) transitions from 0→1 and the frame
sequencer is currently on a length-clocking step (the step counter is odd, meaning
a length clock just fired), an extra length clock is applied immediately. This
matches the hardware quirk documented in Pan Docs.

## Frequency Timers

Each channel has a `frequency_timer` that counts down T-cycles (or 2MHz ticks for
CH3) and reloads when it hits zero, advancing the channel's waveform position.

| Channel | Timer units | Reload value |
|---------|-------------|--------------|
| CH1/CH2 | T-cycles (4 MHz) | `(2048 − frequency) × 4` |
| CH3     | 2MHz ticks (2 MHz) | `2048 − frequency` |
| CH4     | T-cycles | `NOISE_DIVISORS[divisor_code] << clock_shift` |

Pan Docs reference: [Frequency and timer](https://gbdev.io/pandocs/Audio_details.html#frequency-and-period-timer)

## Ticking

`ApuPeripheral::tick(cycles, div_counter)` is called **once per T-cycle** from
`Sm83::advance_apu`. It receives the timer's internal counter *after* the timer
has advanced, and reconstructs intermediate DIV values per T-cycle to correctly
detect the falling edge of bit 12.

Per-T-cycle ticking is required for CH3: the wave channel's frequency timer runs
at 2 MHz (one tick per 2 T-cycles). Getting the wave position right at each
T-cycle boundary is critical for the wave RAM access timing described below.

## Wave RAM Access Quirks (DMG)

On DMG hardware, the CPU can only access wave RAM (0xFF30–0xFF3F) freely when CH3
is inactive. While CH3 is playing, access is gated by a 2 T-cycle coincidence window:

- **Reads**: return `sample_buffer` (the byte at the current wave position) only
  during the 2 T-cycles immediately after a position advance (`just_read == true`).
  All other reads return 0xFF.
- **Writes**: are redirected to the current wave position (not the requested offset)
  when `just_read` is true. Writes outside this window are ignored.

The `just_read` flag is set when the frequency timer fires and cleared at the start
of the next 2 MHz tick (i.e. it stays set for exactly 2 T-cycles).

Pan Docs reference: [Wave RAM](https://gbdev.io/pandocs/Audio_Registers.html#ff30ff3f--wave-pattern-ram)

### Bus timing for wave RAM accesses

To read/write wave RAM at the correct T-cycle within an M-cycle, `Sm83` splits the
M-cycle around the access:

1. Increment cycle counter and route pending bus events.
2. Advance PPU for the full M-cycle.
3. Advance timer + APU for T1, T2, T3 (3 T-cycles).
4. Perform the wave RAM read or write (at T3).
5. Advance timer + APU for T4.

This matches the T3 data-latch point used by real hardware for memory-mapped IO reads.
See `Sm83::tick_cycle_to_t3()`.

### DMG retrigger corruption

Retriggering CH3 (writing bit 7 of NR34) while the channel is active and
`frequency_timer == 1` (the timer is about to advance on the next 2MHz tick)
corrupts the first 4 bytes of wave RAM:

- If the upcoming position byte index is in the first block (0–3): only that byte
  is copied to index 0.
- Otherwise: the entire 4-byte block containing that index is copied into bytes 0–3.

This matches the SameBoy behaviour where corruption fires when `sample_countdown == 0`.
Our timer model decrements before checking, so `frequency_timer == 1` is equivalent
(it will decrement to 0 and fire on the next tick). See `apply_ch3_retrigger_corruption()`.

### DMG trigger delay

On DMG hardware, triggering CH3 adds **3 extra 2MHz-cycles** to the initial
`frequency_timer` reload:

```rust
// In WaveChannel::trigger():
self.frequency_timer = (2048 - self.frequency) + 3;
```

This delay does NOT apply to the normal `clock_frequency()` reload. It shifts the
wave channel's phase by a fixed amount relative to the trigger point, which is
observable in the blargg `dmg_sound` test 09 (wave read while on).

## Register Read-Back

APU registers are write-only in several bit positions; those bits always read as 1.
`READ_MASKS` stores the OR mask for each register (indexed by `address - NR10_ADDR`).

`ApuPeripheral::read_register()` applies the mask before returning. The raw written
value is stored in `regs[]` for correct read-back of the writable bits.

NR52 is handled specially: `build_nr52()` assembles its value from the live channel
`enabled` flags rather than from `regs[]`, so the channel-active bits always reflect
current state.

Pan Docs reference: [Audio Registers](https://gbdev.io/pandocs/Audio_Registers.html)

## Power State

NR52 bit 7 controls APU power. When powered off:

- All registers except NRx1 (length counter) are frozen; writes are ignored.
- All channels are disabled and their internal state is reset.
- Length counters are preserved (DMG behaviour).
- The frame sequencer step resets to 0 when power is restored.
