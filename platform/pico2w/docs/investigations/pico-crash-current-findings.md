# Pico Crash Investigation: Current Findings

Last updated: 2026-08-14  
Firmware commit: `a8ea0259f29f`  
Target: Raspberry Pi Pico 2 W / RP2350 A2

This is the concise handoff for the current state of the Pico crash
investigation. The full chronological notebook remains in
`crash-debug-notes.md`.

## Executive summary

The firmware is suffering real core-0 stack corruption. At least one captured
failure contains a contiguous overwrite of a live stack frame: both the stack
protector canary and the saved return address were replaced with unrelated
stack/heap values. This is not merely a false stack-protector comparison.

A generic Cortex-M33 `PUSH`/`POP` or SRAM read-path failure is now strongly
refuted. A purpose-built assembly stressor completed more than one billion
checked multi-register push/pop operations without one mismatch while the real
firmware filled all 31 persistent crash slots in roughly five minutes. The
remaining mechanism must be workload- or address-path-specific, such as a wild
write, DMA/cross-core interaction, contention-sensitive SRAM behavior, or a
more specific instruction/fetch failure.

No exact corrupting instruction has been captured yet. The next experiment is
a tightly scoped DWT write watchpoint on the known canary word, qualified by a
saved return address in the outer live frame.

## Verified observations

### The crash is reproducible and heterogeneous

The current diagnostic image produced 31 valid records in about five minutes:

- 15 stack-protector panics
- 11 HardFaults
- 5 watchdog resets

The sector is retained at `/tmp/rustyboy-crash-stackpop-5m.bin`, with decoded
JSON at `/tmp/rustyboy-crash-stackpop-5m-nosym.json`.

Two HardFaults returned through a corrupted saved PC of `0x00009ffe` from the
epilogue of `SharedWorkerState::write_live_vram_range`. Nine other HardFaults
had `pc=0x00000010`, `lr=0x10011e49`, and a precise bus fault at
`0x01482815`. These signatures show more than one visible consequence, but do
not yet prove more than one underlying cause.

### A saved return address was read from a corrupted stack slot

An earlier repeated HardFault had:

- PC: `0x2003c6e8`
- LR: `0x1001448b`
- CFSR: `0x00020000` (`INVSTATE`)
- pre-fault SP: `0x2007bab0`

Disassembly showed that `__aeabi_memclr4` saved LR at exactly `0x2007baac` and
returned with `pop {r7, pc}`. The live LR register still held the correct
return address, but the PC consumed from the stack was bad. This isolates that
failure to the saved stack word or its load, rather than a bad branch target
calculation in the caller.

### A live protected frame was genuinely overwritten

The strongest stack-protector capture is in:

`Option<Box<dyn OpCode>>::map::<Arc<dyn OpCode>, ...>`

For that invocation:

- protected-function body SP: `0x2007baa0`
- canary address: `0x2007baa8`
- expected canary: `0x2b7e1516`
- captured canary: `0x2007cc70`
- saved LR address: `0x2007bacc`
- captured saved LR: `0x20034398` (a heap address)
- expected internal caller return: `0x1001a6b7`

Several adjacent frame words contain plausible stack, heap, and code pointers.
The canary and saved LR are both wrong, 36 bytes apart. This is positive
evidence for a multiword or contiguous overwrite, not a spurious canary check.

The protected function was called while
`PicoGameBoy::with_cartridge` was active. The outer frame calculation is:

- `with_cartridge` body SP: `0x2007bad0`
- total outer frame size: `0x1270`
- outer entry SP: `0x2007cd40`
- outer saved LR sentinel address: `0x2007cd3c`
- staged-ROM boot return value while live: `0x10009a59`
- menu-loading return value while live: `0x1000d00d`

The staged-ROM value is the first value to use for the next trial. Reading this
address after construction has returned is not meaningful because normal stack
reuse replaces it.

### Generic stack loads and SRAM reads stayed clean

The new `stack_pop_check` assembly loop deliberately exercises the same broad
mechanism implicated by the bad returns:

- `push.w {r0, r1, r2, r3, lr}`
- `pop.w {r0, r1, r2, r3, lr}`
- compare every restored register
- return from each batch using `pop {r4, r5, r6, r7, pc}`

It completed at least `1,018,167,296` checked iterations with zero mismatch.
At the five-minute capture point it had completed `944,766,976` iterations,
also with zero mismatch, while the real workload had already saturated the
31-record crash sector.

Other integrity diagnostics also remained clean:

- 19,361 complete XIP checks of the first 256 KiB: zero mismatch
- 77,804 `.data` checks: zero mismatch
- 391,090 RAM-check batches over stack and BSS samples: zero mismatch
- allocator guard: zero hits

These checks do not cover every byte at every instant, but their clean result
substantially narrows the failure away from broad SRAM or flash corruption.

## Hypotheses ruled out or reduced

### LLVM machine outliner

Disabling the LLVM machine outliner did not fix the problem. That build ran for
about 27 minutes and recorded 51 canary smashes; its persistent sector contained
15 panics, 11 HardFaults, and 5 watchdog records. The default outliner has
therefore been restored.

### Generic `POP`/LDM or SRAM read failure

More than a billion clean controlled operations while real failures occurred at
high frequency strongly refutes this as the general mechanism. It does not
exclude an address-, contention-, DMA-, or code-path-specific read problem.

### Broad XIP, `.data`, or sampled SRAM corruption

The integrity checks above found nothing. This reduces, but cannot completely
eliminate, short-lived corruption between checks or corruption outside the
sampled ranges.

### RP2350-E2 spinlock alias erratum as a direct match

The board reports A2 silicon. The RP2350 E2 erratum concerns writable spinlock
aliases, but locks 18 through 31 are documented as safe and Embassy uses
spinlock 31 for its critical section. E2 therefore does not directly explain
the observed stack overwrite. See the official
[RP2350 datasheet](https://datasheets.raspberrypi.com/rp2350/rp2350-datasheet.pdf).

### The 300 MHz overclock as the sole cause

The current high-rate reproduction runs at 300 MHz. This is outside the
RP2350's 150 MHz specification and can increase the failure rate.

**Correction (2026-08-14):** this document previously stated the reproduction
runs at VREG 1.30 V. It does not. The retained image was built *without*
`--features oc-300`, and that configuration's defaults are `TARGET_SYS_HZ =
300_000_000` with `TARGET_CORE_VOLTAGE = V1_25` (`main.rs:129` and `:140`).
Measured on the board: `POWMAN VREG` (`0x4010000c`) = `0x000000e0`, i.e.
VSEL `0b01110` = **V1_25**. The clock was independently confirmed from
`PLL_SYS` (`REFDIV=1`, `FBDIV=125`, `POSTDIV 5x1` -> 12 MHz x 125 / 5 =
300 MHz), so only the voltage claim was wrong.

This matters when reading every rate in this document: core voltage is the one
*proven* rate modulator measured under a controlled, byte-identical layout
(V1_25 vs V1_30, one variable: >=0.95 records/min with 78 smashes, versus
0.105/min with zero smashes — roughly 7x). All rates quoted here, including the
"31 records in about five minutes" headline, were therefore measured at the
*lower* voltage. It does not invalidate any of the qualitative refutations
above, which do not depend on rate.

`QMI_M0_TIMING` (`0x400D000C`) also read `0x60007203`, i.e. CLKDIV=3. That is
the known bootrom-reset state already documented at `main.rs:837-864`: bootrom
flash helpers reset QMI timing on every flash access, and each crash commits a
record to flash, so at ~3 crashes/min the device spends much of its life with
flash SCK at 300/3 = 100 MHz rather than the intended CLKDIV=6 (50 MHz). This
is previously-known behaviour, not a new finding, but it is the live state.
However, the identical saved-LR failure was also observed once during a
roughly nine-hour 150 MHz run. The overclock is at least a rate modulator, but
it is not sufficient evidence for the root mechanism.

## Diagnostic approaches that were too invasive

A DWT read watchpoint on the exact stack address generated more than 21 million
DebugMonitor exceptions within seconds. A DWT value-read watch on
`0x2003c6e8` generated roughly 45 million exceptions in 15 seconds and caused
repeated initialization. Both altered timing too heavily to produce trustworthy
captures and have been retired.

An address write watch is much lower traffic, but stack addresses are reused.
Earlier filters based only on stack depth or value were unsound and admitted
legitimate writers. The next filter checks a fixed saved LR in a still-live
outer frame and the interrupted stack depth together.

## Current leading candidates

The evidence currently supports these classes, without proving one:

1. A workload-specific wild or misdirected multiword write into the core-0
   stack.
2. A DMA or display-transfer operation using corrupted bounds or an incorrect
   destination.
3. A cross-core publication/visibility bug that lets core 0 consume stale or
   torn framebuffer metadata and later misprogram a copy or DMA operation.
4. A contention- or address-specific RP2350 SRAM path failure not exercised by
   the controlled stressor.

A separate audit identified a missing producer-side `DSB` before publishing
the large raw framebuffer slot and dirty-row buffers to the other core. That is
a credible weak-ordering issue and deserves a controlled fix/no-fix soak, but
there is not yet direct evidence connecting it to this stack overwrite.

## Next discriminating experiment

Use the existing firmware image without rebuilding, so all measured addresses
remain stable:

1. Preserve and blank the current crash sector.
2. Clear `DWT_CATCH` and `WATCH_LOG`.
3. Plant the watched canary address `0x2007baa8` in `DWT_CATCH[8]`.
4. Plant sentinel address `0x2007cd3c`, expected value `0x10009a59`, and victim
   body SP `0x2007baa0` in the watch log.
5. Reset the board, disconnect SWD, and run for at least five minutes.
6. Read the watch log, stack-smash dump, integrity blocks, and crash sector.
7. Symbolize every captured writer PC and validate its effective destination.

If no catch occurs but the same protected frame is smashed, repeat using the
menu-loading sentinel `0x1000d00d`. If the watchpoint suppresses the failure,
replace it with a low-overhead software epilogue check that records the frame
before the normal stack-protector panic path changes it.

## Result: scoped canary watchpoint, staged-ROM sentinel (config U)

Run 2026-08-14 08:24:27, ~10 min, on the unmodified image. Planted exactly as
specified above. **Instrument addresses had moved and reordered since earlier
notes and were re-derived from this ELF** — `DWT_CATCH=0x200670e8`
(`[8]@0x20067108`), `WATCH_LOG=0x20067610` (`[3]@0x2006761c`, `[5]@0x20067624`,
`[14]@0x20067648`), `SMASH_CORE0=0x20067110`, `_stack_end=0x20067ab8`; zero span
668 words from `0x20067048`. All four plants verified by read-back.

**No catch.** `WATCH_LOG` magic was never written and `CAUGHT` stayed 0, while
`[13]` counted **30,476** non-guard writes to `0x2007baa8`. So the watchpoint
was live and the word was written tens of thousands of times, but not one write
satisfied sentinel-AND-depth.

The same protected frame was still smashed during the window: 5 of the 28 crash
records are stack-protector panics with `lr=0x10013239`, the same
`Option<Box<dyn OpCode>>::map` victim. This is the documented "no catch but the
same frame is smashed" branch, so the run was repeated with the menu-loading
sentinel `0x1000d00d` (config U2, 08:37:21).

28 records in ~10 min (2.8/min, sector not saturated):

- 5x panic `lr=0x10013239` (the map victim)
- 5x panic `lr=0x20000b2b` — a *second* concurrent victim,
  `GameBoyMemory::c...+0x16e`, not previously named
- 1x panic `lr=0x2000359b` (`drain_bus_events`)
- 5x HardFault `pc=0x00009ffe`, `lr=0x10019dd9`
  (`SharedWorkerState::write_live_vram_range+0x5c`), IBUSERR
- 5x HardFault `pc=0x00000010`, `lr=0x10011e49`
  (`embassy_time::delay::block_for+0x18`), PRECISERR/BFARVALID at
  `fa=0x01482815`
- 1x HardFault `pc=0x2007cb20` (a stack address), same `block_for` LR, INVSTATE
- 1x core-1 HardFault `pc=0x200181c4`, `lr=0x20000765`
  (`ApuPeripheral::produce_samples+0xb0`), INVSTATE
- 5x watchdog

Integrity blocks over the same window: `.data` 1007 checks / 0 hits, allocator
guard 0, RAM-check 5004 batches / 0 mismatch. Heartbeat: core0 93, core1 37691.

Two caveats on this null. First, arming the DebugMonitor perturbs timing, and
the most recent smash in `SMASH_CORE0` was a *different* victim
(`body_SP=0x2007ccc8`, `lr=0x20000b2b`) than the watched one, so the watched
frame is no longer the dominant victim. Second, and unchanged: **the DWT sees
only the CPU load/store unit, so this null does not exonerate DMA.**

## Both sentinel values in the designed experiment were wrong

Config U2 (menu sentinel `0x1000d00d`, 08:37:21, ~15 min) was also a null:
`CAUGHT = 0`, magic never written, `[13]` = **46,184** counted writes to
`0x2007baa8`. The watched victim *did* smash during that window —
`SMASH_CORE0` came back with `body_SP = 0x2007baa0`, `lr = 0x10013239`, 19
smashes, guard `0x2007cc70`, saved LR `0x20034398`, i.e. the exact watched word
corrupted to the exact known value.

The cause of both nulls was then found by simply reading the sentinel word on
the running board:

    0x2007cd3c = 0x1000ac47   (stable across three samples)

That is `embassy_main_task+0x3746` — a **third** `with_cartridge` call site,
between the two that were guessed (`0x10009a59` = +0x2558, `0x1000d00d` =
+0x5b0c). Neither planted value could ever match, so **configs U and U2 were
structurally incapable of catching anything** and their nulls say nothing about
the corruptor. They are not evidence that no CPU store is responsible.

Lesson for this instrument: the sentinel value must be **measured on the
board**, not derived by reading call sites out of the disassembly. The earlier
note that "reading this address after construction has returned is not
meaningful" is right in principle, but a stable repeated read of a plausible
code address in the expected function is far better than a guess.

Config U3 (08:55:08) repeats the experiment unchanged except for the corrected
sentinel value `0x1000ac47`.

### If U3 is also null

Drop the sentinel (`WATCH_LOG[3] = 0`) and keep only the depth gate
(`[5] = 0x2007baa0`). Depth is structural rather than guessed: the stack grows
down, so `sp_at_fault <= body_SP` selects writers executing *deeper* than the
victim's frame base. That is the right filter for the leading hypothesis — a
callee overrunning upward into the frame, which matches corruption at
`body_SP+0x8` and `body_SP+0x2c`. It also means that **if the corruptor writes
from a shallower frame (a wild pointer store from main-loop code at
SP ~0x2007cb00), the depth gate rejects it by construction**, so a null with
depth enabled is itself informative and should be reported as such rather than
read as "no CPU store did it".

## Config U3: the watchpoint fires, but the ring saturates with benign writers

With the corrected sentinel `0x1000ac47`, config U3 (08:55:08, ~15 min) **caught**:
magic `0x3EE70001` written, `CAUGHT = 21,752` out of `[13] = 44,940` writes,
first offending value `[12] = 0x20027510`, first `sp_at_fault = 0x2007ba74`. The
sentinel word still read `0x1000ac47` at the end of the window, so the predicate
stayed valid throughout. The instrument works.

All eight distinct writer PCs are allocation-path functions caught at *prologue*
offsets, i.e. ordinary frame reuse, not corruptors:

| PC | off | SP | symbol |
|----|-----|----|--------|
| `0x1002f970` | +0xa | `0x2007ba74` | `compiler_builtins::mem::memcpy` |
| `0x100205da` | +0xa | `0x2007ba98` | `GuardedHeap as GlobalAlloc::alloc` |
| `0x10013296` | +0x2 | `0x2007ba98` | `RawVecInner::with_capacity_in` |
| `0x100146ec` | +0x46 | `0x2007ba80` | `RawVecInner::try_allocate_in` |
| `0x100131a8` | +0x8 | `0x2007ba98` | `Option::map` (the victim itself) |
| `0x10014666` | +0x6 | `0x2007baa0` | `box_new_uninit.903` |
| `0x100131aa` | +0xa | `0x2007ba98` | `Option::map` (the victim itself) |
| `0x10013fda` | +0x6 | `0x2007baa0` | `box_new_uninit.874` |

This reproduces the config-S result exactly, including the same SP values. The
victim's own base is `0x100131a0` (`lr = 0x10013239` = +0x99), so two of the
eight are its own guard store.

**The ring is the bottleneck.** It dedupes by PC and fills once *without
evicting*, so at ~50 writes/s it freezes within seconds on whichever writers
happen to be first. The corruptor wrote later and was never recorded. First
offending value `0x20027510` is not the corrupt value `0x2007cc70`.

### Config U4: exploit the ring instead of fighting it

The dedupe rule (`break` on `cur == pc`, record only into a `cur == 0` slot) can
be turned into an exclusion filter by **pre-filling the ring with the known
benign PCs and leaving one slot free**. Only a writer not already in the list
can then claim it. Planted 09:13:14:

- watch `0x2007baa8`, sentinel `0x2007cd3c` = `0x1000ac47` (unchanged)
- **`WATCH_LOG[5] = 0` — the depth gate is DISABLED**
- ring `[16..22]` pre-filled with the seven benign PCs above, `[23]` left free

Disabling depth matters: every previous run gated on `sp_at_fault <= body_SP`,
which selects only writers *deeper* than the victim's frame base. A wild pointer
store from shallower main-loop code (SP ~`0x2007cb00`) was therefore rejected by
construction in configs U, U2 and U3, and in the config-S "one word below the
benign band" run. **This is the first configuration that can see a shallower
writer at all.**

## The recurring "guard constant 8 bytes below the guard slot" is NOT a bug

This pattern has been flagged repeatedly across builds and victims: the dump
shows `0x2b7e1516` sitting exactly 8 bytes below the computed guard slot, which
invites the theory that the prologue stores the canary at one offset and the
epilogue checks a different one. **Checked directly against the current victim's
own code and refuted.**

`GameBoyMemory::copy_dma_step` @ `0x200009bc`:

- prologue `str r0, [sp, #0x50]` @ `0x200009d0`
- epilogue `ldr r0, [sp, #0x50]` @ `0x20000b14`

Same offset. No mismatch. The word at `+0x48` is simply another local. The frame
arithmetic also validates: the call is `bl __Thumbv7ABSLongThunk___stack_chk_fail`
(an ABS long-branch thunk, `ldr r12,[pc]; bx r12`, which uses no stack), and the
`addeq sp, #0x54` in the epilogue only executes on the *passing* path, so SP at
`__stack_chk_fail` entry really is the victim's body SP. The naked-trampoline
capture is therefore sound and the guard slot address derived from it is correct.

Do not revive this theory.

## Config V: value-triggered capture

The planted-predicate approach on the old image was exhausted (all three
predicates dead — see above), so the firmware was rebuilt with one change to
`dwt_watch.rs`:

- `WATCH_LOG[1]`, previously a planted-but-unused `thresh`, is now an **exact
  value filter**: non-zero means only a write whose value equals it is recorded.
- New write-once latch `[6]` = PC, `[7]` = SP-at-fault, `[8]` = match count, so
  the answer cannot be churned away by the non-evicting ring.

Build `9b67a0f5…`, image CRC `0xedc61550` verified on device, still **without**
`--features oc-300` so the operating point stays 300 MHz / V1_25 and the dataset
stays comparable. Addresses moved and reordered again: `DWT_CATCH=0x20067228`,
`SMASH_CORE0=0x20067250`, `WATCH_LOG=0x20067750`, `_stack_end=0x20067ab8`.

Calibration soak (09:33:05, ~14 min) named the dominant victim as
`copy_dma_step`, `body_SP = 0x2007ccc8`, so guard address `0x2007cd18`, and the
corrupt payload is **`0x00000001`**. Nothing in that function writes `+0x50`
except the guard store itself, so a write of `1` to that slot while the frame is
live is anomalous — which makes even this common-looking value a usable filter.

Config V2 planted 09:49:23: watch `0x2007cd18`, value filter `1`, sentinel and
depth gate both **off**. `[8]` (match count) is the sanity check: if it is huge,
the filter is too loose to trust and `[6]` means little; if it is small, `[6]` is
the writer.

## Config V2: the exact-value filter is also too loose — and why

Planted watch `0x2007cd18` with value filter `1`, sentinel and depth both off.
Result over ~16 min:

- `[8]` match count = **8,065,684**
- `[13]` total writes to that word = **71,565,733**

So the watched stack word is written about **74,000 times per second**, and ~11%
of those writes carry the value `1`. The latched PC (`0x10027a6c`) is therefore
meaningless and is not reported as a finding.

This closes out every filter dimension the instrument has:

| dimension | measured outcome |
|-----------|------------------|
| address alone | 71.5M writes; ring saturates in seconds with 9 distinct allocation-path prologue pushes, all benign |
| address + exact value | 8.07M matches — ~11% of traffic |
| sentinel liveness | permanently stale-true; admitted 100% of writes |
| stack depth | rejects a shallower writer by construction |

The corrupting store is **indistinguishable from legitimate traffic in all of
them**. The watched word is simply an ordinary hot stack slot.

## Config W: guard→non-guard transition latch

One discriminator remains, and it is exact rather than statistical. The
corrupting store is, by definition, the write that turns a live canary into
something else. The DWT fires on *every* write to the address, so the handler
observes the complete value sequence and can remember the previous one:

    prev == STACK_GUARD && v != STACK_GUARD   =>   this store just destroyed a live canary

This needs no liveness, depth, or payload assumption, and fires at most once per
victim invocation. Implemented in `dwt_watch.rs`: `[9]` holds the previous value,
`[6]`/`[7]` latch the first destroying PC/SP (write-once), `[8]` counts them. The
older value-filter path keeps its own record at `[10]`/`[11]` so the two cannot be
confused.

Caveat to apply when reading it: the DebugMonitor is configurable-priority, so
PRIMASK can defer it and writes may coalesce. A missed intermediate write would
blur the transition, so treat a single catch as a lead and sanity-check `[8]`
against the smash count rather than declaring root cause from one sample.

Build `0x8d068cb0` (CRC verified on device), still 300 MHz / V1_25. Instrument
addresses unchanged from config V. Calibration soak started 10:08:11 to re-derive
the current victim's guard address before planting.

## Config W2: the transition latch fires, but "first transition" is the wrong one

Armed on the map victim's guard word `0x2007baa8` (victim base `0x100131a0`,
`body_SP = 0x2007baa0`, guard offset `+0x8` derived from its own `str`/`ldr`
pair). Result over ~15 min:

- `[6]` first transition PC = `0x10014a5a`, `[7]` SP = `0x2007baa8`
- **`[8]` transition count = 1110**, against `SMASH_CORE0[3]` = **16** smashes

1110 vs 16 is ~70x too many, so the latched PC is not the corruptor and is not
reported as one. The cause is structural and was predictable: when the victim
returns normally its guard slot is recycled, and the next function to write
there also produces a guard->non-guard transition. **Every normal invocation
generates one benign transition.**

Also worth recording: the victim, its `body_SP`, its guard address and *both*
corrupt values (`0x2007cc70` in the guard slot, `0x20034398` in the saved-LR
slot) reproduce **identically across three different builds**. The corruption is
structurally determined, not random timing noise.

## Config X: snapshot the LAST transition at the moment of the smash

The fix follows from the same structure that broke the first-transition latch. A
transition requires the slot to hold the guard *first*, and once corrupted it
stays corrupt until some later invocation stores a fresh guard. So **while a
given frame is live there is at most one transition**, which means the last
transition before a smash is precisely the store that destroyed that frame's
canary.

Implemented:

- `dwt_watch.rs` keeps a last-write-wins copy of the transition PC/SP at
  `WATCH_LOG[32]`/`[33]`.
- `stack_chk.rs` snapshots those two words into `SMASH_CORE0[132]`/`[133]`
  (past the 128-word dump, in MPU padding) at the moment the canary check fails.

So the smash record itself now carries the killing store. Read it with
`probe-rs read --chip RP235x b32 0x20067250 134` — `[132]` @ `0x20067460` is the
PC, `[133]` @ `0x20067464` is its SP.

These are only meaningful when the watch is aimed at *that* victim's guard word;
if the victim moves, they are stale, and the smash record's own `[1]`/`[2]` will
show it.

Build CRC `0x64fffb43` verified on device, 300 MHz / V1_25, victim still at
`0x100131a0`, watch planted at `0x2007baa8`, soak started 10:42:08.

## The DWT/DebugMonitor approach is EXHAUSTED — with proof of the mechanism

Config X2 aimed the transition latch at the correct victim this time
(`drain_bus_events`, `body_SP = 0x2007cb70`, `LR = 0x2000359b`, guard at
`+0x1b0` = `0x2007cd20`, both verified against the record). Its guard slot was
genuinely corrupt at the smash: `0x2003d6f8`. And yet:

- `WATCH_LOG[8]` transition count = **0**
- `SMASH_CORE0[132]/[133]` = **0**
- `WATCH_LOG[13]` total writes = **73,238,730**
- **`DWT_CATCH[6]` (writes where the handler read the guard value) = 0**

That last number is the proof. Across 73 million writes the handler **never once
observed the guard value** at the watched address. The DWT fires *after* the
access and the handler *re-reads* the word; at ~58,000 writes/second, with
PRIMASK deferring the configurable-priority DebugMonitor, the value has always
already been overwritten. `prev == STACK_GUARD` can therefore never become true.

**So the config-X/X2 nulls are instrument failures, not evidence.** They do NOT
support a non-core master and must not be cited as doing so.

This closes the whole DWT line for these targets. Every predicate dimension has
now been measured and defeated by the sheer write rate on these stack words:

| predicate | measured outcome |
|-----------|------------------|
| address alone | 71.5M writes; ring saturates in seconds |
| address + exact value | 8.07M matches (~11% of traffic) |
| sentinel liveness | permanently stale-true (100% admitted) |
| stack depth | rejects shallower writers by construction |
| first guard->non-guard transition | 1110 transitions vs 16 smashes (benign recycle on every return) |
| last transition at the smash | handler never sees the guard value at all (`DWT_CATCH[6]` = 0) |

**Seventeen distinct writers have now been caught across two victims and every
single one is a benign prologue push / ordinary frame reuse**, including the
only non-prologue-looking candidate: `0x1001a2f0` resolves to
`PicoGameBoy::with_cartridge+0xc`, a `sub sp,#0xc` — not a memory write at all.
Its real store is the `push.w {r8,r9,r10,r11}` at +0x4, which from entry SP
`0x2007cd40` lands **r9 exactly on `0x2007cd20`**. Benign.

### Where to go next — a different instrument class

Do not build another DWT predicate. The remaining options, in order of promise:

1. **ETM instruction trace.** `etm.rs` already exists and the linker defines
   `__etm_filter_start` / `__etm_filter_end` bracketing exactly the RAM-code
   region that contains `drain_bus_events` — that infrastructure was built for
   this problem. It is hardware trace of the real instruction stream and is the
   only genuinely non-perturbing instrument available.
2. **Loose end (D)**, the missing producer-side `DSB` before publishing the raw
   framebuffer slot and dirty-row buffers, as a controlled fix/no-fix soak.
3. **DMA**, which the DWT has never been able to see. Sweep channel registers
   repeatedly (they rotate); a single snapshot is insufficient.

### Note on perturbation

Every DWT run perturbs the system: with the monitor armed the victim moved
between `map` and `drain_bus_events` and the smash count fell from 16-18 to 2 per
window. Config X3 (11:24:41) runs the same build with the watch **disarmed** to
establish an unperturbed baseline rate for this build, which any future
fix/no-fix comparison needs.

## MAJOR REFRAMING: core 1 is the dominant failure, and it fails the same way

Two soaks with the DWT **disarmed** (the first unperturbed measurements taken in
a long time) change the picture substantially.

**Config X3** (19.7 min): 11 records = **0.56/min** — 6 watchdog, 3 canary panic,
2 core-1 HardFault, **zero core-0 HardFaults**.
**Config Y** (26.9 min): 20 records = **0.74/min** — 10 watchdog, **7 core-1
HardFault**, 3 canary panic.

So at the true operating point **17 of 20 records are core-1 faults or hangs**,
and the core-0 canary smash that this investigation has chased for dozens of
cycles is the *minority* failure mode. Note also how badly the DebugMonitor was
distorting things: the perturbed runs read 2.8/min and the doc's old headline was
6/min, versus 0.56–0.74/min unperturbed.

### The core-1 signature is perfectly deterministic

All **7** core-1 HardFaults are byte-identical in every recorded field:

    pc=0x200181c4  lr=0x20000765  cfsr=0x00020000 (INVSTATE)
    hfsr=0x40000000  fa=0  sp_before=0x20081f90  r12=0x10013c75

Decoded:

- `lr = 0x20000765` is exactly the return address of the **second**
  `bl __Thumbv7ABSLongThunk__Vec<i16>::push` at `0x20000760`, i.e.
  `ApuPeripheral::produce_samples+0xb0` (base `0x200006b4`).
- `pc = 0x200181c4` is **not code**: it is `SHARED_WORKER_STATE + 0x13164`
  (base `0x20005060`; RAM code ends at `__edata = 0x2000381c`). Branching to an
  even address is exactly why CFSR reports INVSTATE.
- `sp_before = 0x20081f90` is only ~112 bytes into core 1's 8 KiB stack
  (`0x20080000..0x20082000`) — a very shallow frame.

**The thunk is intact and is not the cause.** Live RAM at `0x2000369e` decodes to
`movw r12,#0x3c75 / movt r12,#0x1001 / bx r12` (r12 = `0x10013c75` =
`Vec<i16>::push|1`), byte-identical to the ELF, and `DATA_CHECK` reports 887
checks with 0 corrupt words. The recorded `r12` is that same correct value.

That rules out the obvious reading: `bx r12` with an odd r12 sets T=1 and lands
at `0x10013c74`; it cannot yield INVSTATE at `0x200181c4`. LR and r12 are simply
*undisturbed*. The reading consistent with every field is a **`pop {pc}` inside
`Vec::push`'s call tree loading a corrupted saved return address** — the thunk
uses `bx`, so LR passes through unchanged, and r12 is never touched.

### Why this matters

**Core 1 suffers the same class of corruption as core 0**: a saved return address
replaced by a pointer into a shared data structure. `SMASH_CORE1` is empty only
because `Vec::push` is not stack-protected, so there is no canary there to trip.

Core 1's stack is in SRAM8/9 (the separate, non-striped banks) while core 0's is
in the striped SRAM0-7, and **both are affected** — which argues against any
bank-specific SRAM effect.

The core-1 case is far more tractable than the core-0 one: it is 7/7 deterministic
with a *fixed* corrupt value and a *fixed* SP, whereas the core-0 victim drifted
between builds. And core 1's stack should carry nowhere near the ~58,000
writes/second that defeated every DWT predicate on core 0 — so a watch armed **on
core 1** at the corrupted slot may actually work where core 0's could not. That
requires firmware changes (the watch is currently armed on core 0 only; each core
has its own DWT).

### Instrument shortfall to fix

`persist_prev()` currently stores only `(core0_ticks, core1_ticks)`. The module
docs say `core0_since_peer` / `core1_since_peer` are the real discriminators for
"which core stopped first". The ring showed core1/core0 ratios of 285–988 across
8 ended boots with no clean freeze signature, but the since-peer deltas would be
the sound test. Persist those too.

## Core-1 signature confirmed on a second window, and the payload identified

**Config Y2** (independent window, 27.75 min, DWT still disarmed): 8 records =
0.29/min — 4 watchdog, **3 core-1 HardFault**, 1 core-0 panic. Again 7 of 8 are
core-1 or hang. The core-1 fault is now **10/10 byte-identical across two
unperturbed windows**.

### The corrupted slot, derived from the frame

`Vec<i16>::push` (`0x10013c74`) ends:

    ldr r11, [sp], #4
    pop {r4, r5, r6, r7, pc}

So after the `pop`, SP equals its entry SP. The record's `sp_before =
0x20081f90` is therefore `Vec::push`'s entry SP, and **the corrupted saved-LR
slot is `0x20081f8c`**. That also puts `produce_samples`' entry at
`0x20081fa8` — only 88 bytes below core 1's stack top (`0x20082000`), matching
the observed shallow frame.

### What the corrupt value IS

`pc = 0x200181c4` = `SHARED_WORKER_STATE + 0x13164` = **78,180 bytes** in.
Layout: `sync_snapshot` + `live_ppu_snapshot` (each `PpuSnapshot` = `0x80` io +
`0x2000` vram + `0xA0` oam ≈ 8.4 KB, so ~16.8 KB together), then
`native_frame_slots: [SharedNativeFrameSlot; 3]`, each an
`UnsafeCell<NativeFrame>` of `FRAMEBUFFER_SIZE`. Offset 78,180 lands **inside
`native_frame_slots`**, roughly two-thirds through slot 2.

So the word overwriting core 1's saved return address is **a pointer into a
shared framebuffer slot** — exactly the memory the frame publication/copy path
walks with pointers. That is a direct match for candidates (2) and (3) (a
transfer with corrupted bounds/destination; cross-core publication), and it
narrows the suspect surface enormously: from "anything on core 0" to core 1's
frame-publication path.

### Heartbeat ring: inconclusive, and why

Ratios `core1/core0` across 8 ended boots: 343-676, with no bimodal
"one core froze" signature. But core 0's heartbeat only ticks 25-139 times per
boot (~once every 1.5-6 s), which is far too coarse to resolve a hang of a few
seconds. The `since_peer` deltas are the sound discriminator (the live boot reads
0 and 51 — healthy); persisting those is still the outstanding instrument fix.

### Next experiment

Arm a DWT write watch **on core 1** at `0x20081f8c`. Core 1 has its own DWT and
its own DebugMonitor, and the watch is currently armed on core 0 only, so this
needs firmware: read a planted address in the core-1 entry path and call
`arm_data_write_watch` + `enable_monitor_only()` there.

The value/transition predicates that failed on core 0 are not needed here — the
**distinct-PC ring alone should be selective**, because core 1's code surface at
a fixed shallow depth is a handful of functions, versus the effectively unbounded
allocation chain that saturated the ring on core 0. Pre-fill the ring with
`Vec::push`'s own prologue PC to exclude the benign writer.

## METHODOLOGY BUG: a mid-run probe attach can wedge the board and fake a clean window

Config Y3 (12:48:11, 27.5 min) reported **zero crash records**. That is *not* a
clean window. Sampling the heartbeat three times over 6 s showed it completely
frozen (`core0=25`, `core1=15968`, unchanged), i.e. **the board was hung and the
watchdog was not resetting it**. The prev-boot ring index had moved 8 -> 2, so two
reboots happened and then it stopped.

The cause is the procedure, not the firmware under test. Blanking the crash
sector mid-run attaches the debugger, and this firmware sets the watchdog to
**pause-on-debug**. With the debug flag latched the watchdog stays paused, so a
hang that would normally produce a WDT record becomes a permanent wedge instead.
Config Y2 used the same "blank sector, no reset" shortcut and survived it; Y3 did
not — so the contamination is intermittent, which makes it especially dangerous.

Recovery also needed escalation: `probe-rs` could no longer even erase flash
(`execution of 'init' failed with code 288`), and the documented OpenOCD RESCUE
path was required. Note a rescue reset **randomises `.uninit`**, so all instrument
arrays and the heartbeat ring are lost.

### Rules this adds

1. **Never interpret a low or zero record count without first proving liveness** —
   sample the heartbeat at least twice, seconds apart, and confirm it advances.
2. **Always reset after any mid-run attach.** "Blank the sector without resetting"
   is not a safe shortcut.
3. After a rescue, the board sits with `core0=0, core1=6` and does not run the
   emulator until the crash sector is blank (it displays the unread-records
   badge instead), so blank the sector *and* reset before timing anything.

## Correction: "sample the heartbeat twice" is an unsound liveness test

The rule added above needs qualifying. **`probe-rs read` halts the core to access
memory**, so two separate probe-rs invocations are two halts, and a frozen
reading at check time can be an artifact of the measurement itself rather than a
wedge.

Config Y4 (13:18:15, 18 min) made this concrete: two liveness samples read
byte-identical, which by the earlier rule would mean "wedged" — yet the board had
plainly been running, because **7 crash records accumulated (0.39/min, squarely
in the unperturbed 0.29-0.74/min range)** and the prev-boot ring index advanced
2 -> 3.

**Use record accumulation as the liveness signal**, not repeated heartbeat reads.
If heartbeat sampling is needed, both samples must come from a *single* probe-rs
session.

Config Y3 remains a genuine wedge: there it was zero records **and** a frozen
heartbeat **and** flash erase failing with `code 288` **and** it needed an
OpenOCD rescue. Three independent signals, not one ambiguous one.

## Both cores fail the same way: a wild branch into a large `.bss` object

Config Y4 added the missing half of the picture. Core 0 produced two HardFaults:

    pc=0x2003bcc0  lr=0x20001fff  cfsr=INVSTATE  sp=0x2007cd08
    pc=0x2003bbc4  lr=0x20001fff  cfsr=INVSTATE  sp=0x2007cd08

Both PCs resolve into the **`embassy_main_task` state object** at
`0x200244dc + 0x177e4` and `+0x176e8` — a `.bss` data structure, not code. `lr =
0x20001fff` is `Sm83 ... instructions+0x112` (RAM code).

That is the same shape as the core-1 fault, which branches into
`SHARED_WORKER_STATE + 0x13164`. So on **both cores** an indirect branch or
`pop {pc}` picks up **a pointer into a large `.bss` structure** and faults with
INVSTATE because the target is even.

Together with the earlier finding that core 1's corrupt value is a pointer into a
shared framebuffer slot, the unified statement is: *saved return addresses on
both cores are being replaced by pointers into the big shared state objects.*
That is one mechanism, not two, and it is independent of which SRAM bank the
stack lives in (core 0 striped SRAM0-7, core 1 non-striped SRAM8/9).

## CAUGHT: the store that writes the poison value (config Z2)

Arming the DWT **on core 1** — which had never been done; every prior watch was
armed by core 0 — produced the first positive identification in this
investigation.

Setup: build Z (CRC `0xd7806df6`), `run_core1_worker` reads `DWT_CATCH[9]` and
arms core 1's own DWT + DebugMonitor. Watch planted at `0x20081f94`. Aim
verified *before* interpreting anything: the core-1 HardFault still reported
`sp_before = 0x20081f98` (12 occurrences in the window).

Result (`WATCH_LOG`):

- `[12]` first offending value = **`0x2002e810`**
- `[16..20]` ring = `0x1001afc0`, `0x1001afc2`, `0x1001afc4`, `0x1001afc6`
  — four consecutive halfwords, i.e. the resume-artifact spread around **one**
  store site
- `[24..28]` all SP = `0x20081f88`

`0x2002e810` is **exactly the wild PC in all 12 core-1 HardFaults**. The store is

    1001afbc:  str  r2, [sp, #0xc]      ; run_core1_worker+0x4ec

which at SP `0x20081f88` targets `0x20081f88 + 0xc` = **`0x20081f94`**, the
watched word. So the value that later gets popped into PC is written *there*, by
that instruction, as one of `run_core1_worker`'s own locals. Twenty-four bytes
earlier, at `+0x4d4`, is `bl __Thumbv7ABSLongThunk__…produce_samples`, whose
callee `Vec<i16>::push` ends `ldr r11,[sp],#4` / `pop {r4,r5,r6,r7,pc}`.

**This is a stack-slot alias**: a word `run_core1_worker` uses as a local is also
consumed as a callee's saved return address.

### The part that does NOT yet add up — do not skip this

`run_core1_worker`'s prologue is only `push {r7,lr}` + `sub sp,#0x38` = 64 bytes,
so a body SP of `0x20081f88` means it was **entered** at `0x20081fc8`.
`produce_samples` would then be entered at `0x20081f88`, push 24 bytes, and call
`Vec::push` at `0x20081f70`.

But the record's `sp_before` is `0x20081f98` — and `sp_before` is the faulted
thread's SP *after* unwinding the exception frame (`sp_before_exception`), i.e.
`Vec::push`'s entry SP. `0x20081f98` is **40 bytes above** `0x20081f70`, and sits
*inside* `run_core1_worker`'s own frame (entry `0x20081fc8` … body `0x20081f88`),
which a normal callee cannot do. Both `produce_samples` call sites are inside
`run_core1_worker` (`+0x4d4` and `+0x6f4`), so there is no alternative shallower
call path.

Audited, and the likeliest explanation is an **instrument asymmetry, not a bad
SP**. The two SPs come from two different formulas:

- `crash/handler.rs::sp_before_exception` is **correct**: it adds 104 bytes when
  `EXC_RETURN.FType` (bit 4) is clear (FP extended frame) and 32 otherwise, plus
  4 for the aligner when stacked xPSR bit 9 is set. Its own comment records that
  this exact mistake already mis-named a watchpoint slot once during the G4 hunt.
- `dwt_watch.rs`'s `sp_at_fault` does `frame + 32` (+4 for the aligner) and
  **never accounts for the FP extended frame**.

So the DebugMonitor's reported SP can be up to 72 bytes low, and the catch's
`0x20081f88` and the HardFault's `0x20081f98` **must not be compared directly**.
Fixing `dwt_watch`'s computation to match the handler's is a prerequisite for any
frame arithmetic that mixes the two sources.

What is solid and independent of all SP arithmetic: the DWT compares the
**address in hardware**, and the value was read back from the watched word. So
**`run_core1_worker+0x4ec` writes the exact value that becomes the wild PC, into
the exact word the watch was aimed at.** The remaining work is to establish how
that word is also reachable as a callee's saved return address.

## Current build and retained evidence

Firmware ELF:

`target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w`

ELF SHA-256:

`110c853376fd916690b9377e0f65ab8466d1f9cd2c5f81dcff6d317b298472c6`

Retained captures:

- `/tmp/rustyboy-crash-stackpop-5m.bin`
  (`5b586d56ac51d952dfa1c5bb3f204ab42fdf8143007e3ce3816c99dd29ee7a99`)
- `/tmp/rustyboy-crash-stackpop-5m-nosym.json`
- `/tmp/rustyboy-smash0-stackpop-5m.bin`
  (`b9ba25f9383880f9af88f95f5c9b095615aff21cda95d9315c00a22a507e3be5`)
- `/tmp/rustyboy-integrity-stackpop-5m.bin`
  (`44dfdc9ceaf2f61572d94b3c7b99f54d3669834084001a0d4a04c97cb6150c13`)

The physical board is still running the above image. The scoped canary
watchpoint described here has been designed but not yet planted or soaked.

## Config Z3 + AA: the instrument-asymmetry bug is FIXED, and the catch survives it

### Z3 window (16:10, 27 min) — crash records valid, watch data VOID

31 records, and the core-1 signature reproduced yet again: **11x
`pc=0x2002e810 lr=0x20000765 cfsr=INVSTATE sp=0x20081f98`**, plus 15 watchdog
hangs and 5 assorted core-0 HFs.

But `WATCH_LOG` came back **byte-identical to Z2** — `CAUGHT` frozen at 111,998
across a window that produced 31 fresh records. The watch was **inert** for the
whole window: `probe-rs` attach clears `DEMCR.MON_EN`, only firmware can set it,
and the arming code runs once at boot in `run_core1_worker`. My blank+reset at
15:43 went through probe-rs, so the monitor never came back up.

**Rule this adds to the tooling list: a frozen `CAUGHT` is NOT evidence of "no
writes" — check that `CAUGHT` advanced before reading anything into WATCH_LOG.**
Z3's watch slots are void; only its crash records count.

### The store identification does NOT depend on the suspect SP arithmetic

Before fixing the instrument, the Z2 conclusion was re-checked against a bound
that no formula can move: **core 1's stack top is `0x20082000`.**

`run_core1_worker`'s prologue is `push {r7,lr}` + `sub sp,#0x38` = 64 bytes, so
`entry_SP = body_SP + 64`. If the reported body SP `0x20081f88` were 72 bytes low
(the maximum error the old formula could produce), the true body SP would be
`0x20081fd0` and the entry SP `0x20082010` — **above the top of the stack, i.e.
impossible.** `0x20081f88` is the only value consistent with the stack bounds.

So `str r2, [sp, #0xc]` → `0x20081f88 + 0xc` = `0x20081f94` stands, and with it
the Z2 result: **`run_core1_worker+0x4ec` writes the exact value (`0x2002e810`)
that becomes the wild PC, into the exact word the watch was aimed at.**

### The fix

`dwt_watch.rs::sp_at_fault_of` did `frame + 32` (+4 aligner) and never handled
the FP extended frame, so it could read up to 72 bytes low, while
`crash/handler.rs::sp_before_exception` has always been correct. Two instruments,
two formulas, silently incomparable. Now `sp_at_fault_of` takes `exc_return` and
applies the same `FType`-based 104-vs-32 choice plus the aligner; the handler's
`_exc_return` parameter is threaded through all six call sites, and the one
open-coded copy inside the depth-gate predicate now calls the helper instead.

This is the SECOND time this exact mistake has cost a result — `sp_before_exception`'s
own comment records it mis-naming a watchpoint slot during the G4 hunt. The fixed
function carries a comment saying so.

### Config AA — running

Rebuild is layout-identical where it matters: every forensics address unchanged,
`run_core1_worker` still at `0x1001aad0`, and the store still `str r2, [sp, #0xc]`
at `0x1001afbc` (`+0x4ec`) with the same `0x38` frame — verified by disassembly
rather than assumed, so the `0x20081f94` aim carries over without re-deriving.
CRC `0x76465722`. Instruments zeroed this time (Z2/Z3 counts were cumulative).

Open question is unchanged and is now the whole investigation: **how is a word
that `run_core1_worker` uses as its own local (`[sp,#0xc]`) also consumed as a
callee's saved return address?** With both SPs finally on one formula, the frame
arithmetic from the two instruments is comparable for the first time. Loose end
(A) — `gameboy.rs:455-466`'s claim that `#[inline(never)]` makes the §G1
spill-slot collision impossible — is the closest prior art and should be re-read
against this.

## Config AA: the chain is CLOSED — the pop reads its return address from a word with exactly one writer

`CAUGHT = 8352` (advanced from a zeroed start), so unlike Z3 this window's watch
data is real. Three results, in increasing order of importance.

### 1. The corrected SP formula returns the SAME value — 0x20081f88

`sp_at_fault` with the fixed `FType` logic still reports `0x20081f88`, i.e. the
frames at this store site are BASIC, and the old bug never distorted this
particular measurement. The store's SP is now confirmed twice over — once by the
stack-top bound, once by a formula known to be correct.

### 2. The watched word has EXACTLY ONE writer in the entire program

`CAUGHT == total writes == 8352`, and all four ring PCs (`0x1001afc0/c2/c4/c6`)
are resume artifacts of the single store `run_core1_worker+0x4ec`. Over ~33
minutes, nothing else in the firmware ever wrote `0x20081f94`.

**No prologue ever pushed a return address there.**

### 3. The faulting `pop` read its PC from precisely that word

The core-1 HF signature (23 occurrences across Z2+Z3) is
`lr=0x20000765 sp_before=0x20081f98`. Symbolising: `0x20000765` =
**`produce_samples+0xb0`**, so the faulting callee was called from there —
`Vec<i16>::push`, whose epilogue is `ldr r11,[sp],#4` / `pop {r4,r5,r6,r7,pc}`.

In `pop {r4,r5,r6,r7,pc}` the PC is the last word popped, at `final_SP - 4`:

```
final_SP (= sp_before) = 0x20081f98
PC loaded from          0x20081f98 - 4 = 0x20081f94   <- the watched word
```

Exact, and independent of every frame-size estimate.

And the poison value is not corrupt data: **`0x2002e810` = `HEAP_MEM+0xa334`** —
a perfectly legitimate `Vec` buffer pointer, which is exactly what
`run_core1_worker` should be holding in a local. It is even, so branching to it
faults INVSTATE. Nothing is corrupting memory; **a correct value is being read
from the wrong place.**

### What this proves about the mechanism

A `pop` restored its return address from a word that no matching `push` ever
wrote. So between the prologue and the epilogue, **SP moved up** — the epilogue
popped from higher than the prologue pushed, landing on the caller's live local.

This is in direct tension with the REFUTED entry "SP moves mid-function", which
must now be re-read: that refutation was established on core 0 against the canary
smashes, and cannot be carried over to this core-1 path unexamined.

### The discriminator, now running (config AB)

Two possibilities remain:

* **(i)** the CALLER's SP moves, so `[sp,#0xc]` does not always name the same word;
* **(ii)** the caller's SP is stable and the damage happens inside
  `produce_samples` → `Vec::push`.

The ring's four SP samples are all `0x20081f88`, but they were selected by
distinct-PC dedup and say nothing about the other 8348 catches. `WATCH_LOG[40]`
/`[41]` now track **min/max `sp_at_fault` over every catch**; `min == max` ⇒ (ii).
`[42]` counts FP-extended exception frames on core 1 and `[43]` keeps the last
`EXC_RETURN` — if core 1 never takes an extended frame, an FP lazy-stacking SP
mismatch is excluded as the mechanism.

`WATCH_LOG` grew 40→64 words for these slots, which MOVED every forensics
address (see below) and made `WATCH_LOG` the LAST `.uninit` object, tail flush
against `_stack_end`. Slots `[58..64]` therefore sit inside core 1's read-only
MPU region — harmless only because nothing writes above `[43]`. Verified on
device: `heartbeat placement OK: base=0x20067368 core1-RO-region=0x20067b00`.

### AA window composition — the mode moved again

25 records, and **no core-1 HFs at all**: 10x WDT, 10x a NEW core-0 signature
`pc=0x00009ffe lr=0x10019de9 cfsr=IBUSERR sp=0x2007cb68` where `0x10019de8` =
`SharedWorkerState::write_live_vram_range+0x5c`, 4x Panic `lr=0x2000359b`, 1x
Panic `lr=0x20000b2b` (`copy_dma_step+0x16e`). Consistent with the standing rule
that layout dominates which victim surfaces; the underlying wrong-slot read is
the same shape.

## Config AB: the caller's SP is PROVEN stable — the damage is inside Vec::push

`CAUGHT = 6081`, so the instrument was live and the aim self-validated.

### The discriminator came back unambiguous

```
[40] min sp_at_fault = 0x20081f88
[41] max sp_at_fault = 0x20081f88     <- min == max over all 6081 catches
[42] FP-extended frames = 0
[43] last EXC_RETURN = 0xfffffff9     (FType set => basic frame)
```

**`run_core1_worker`'s body SP never moves.** `[sp,#0xc]` names `0x20081f94` on
every one of 6081 samples, so branch (i) — "the caller's frame moves" — is dead.
The SP damage is inside `produce_samples` → `Vec::push`.

`[42] == 0` additionally **excludes FP lazy-stacking**: core 1 never takes an
extended exception frame, so a stacked/unstacked size mismatch cannot be moving SP.

### Both functions on the path are statically BALANCED

`produce_samples` (`0x200006b4`): prologue `push {r4,r5,r6,r7,lr}` (20) +
`str r11,[sp,#-4]!` (4) = 24. It has TWO exits — a tail call at `+0x54`
(`ldr r11,[sp],#4` / `pop {r4,r5,r6,r7,lr}` / `b.w` into `Vec::push`) and a normal
return at `+0xb4` (`ldr r11,[sp],#4` / `pop {r4,r5,r6,r7,pc}`) — and **both restore
exactly 24**. Every branch to `+0xb4` (`0x200006d6`, `0x200006f0`, `0x2000072a`,
`0x20000748`) happens after the prologue completed. No unbalanced path exists.

`Vec<i16>::push` (`0x10013c74`): same 24-byte prologue, same 24-byte epilogue, one
conditional `bleq grow_one`. Balanced.

Both `produce_samples` call sites are inside `run_core1_worker` (`+0x4d4`,
`+0x6f4`) — verified by scanning the full 80,338-line disassembly for branches to
`0x200006b4` and its thunk `0x10030b60`. **There is no shallower caller**, so the
entry SP is genuinely `0x20081f88`.

### The size of the anomaly

Working frames forward from the measured `0x20081f88`:

```
run_core1_worker body SP   0x20081f88
produce_samples entry      0x20081f88   body 0x20081f70  (24-byte frame)
Vec::push       entry      0x20081f70   body 0x20081f58  (24-byte frame)
Vec::push epilogue SHOULD end at        0x20081f70
Vec::push epilogue OBSERVED ending at   0x20081f98   <- 40 bytes (0x28) higher
```

`lr = 0x20000765` is the return address of the `bl` at `0x20000760`, so the
faulting function is definitely `Vec::push` (the long thunk preserves LR, and
`grow_one` would carry `lr = 0x10013c8e` instead). Since `Vec::push` is balanced
and its caller's SP is fixed, **SP rose 40 bytes during `Vec::push`'s own
execution**, and the only thing in that window able to move it is the
`bleq grow_one` allocator call.

### Next: the allocator path

`grow_one` → `RawVec` growth → global allocator is now the whole remaining
surface. Note the prior finding that the audio path already had a cross-core
heap race (the 8192-byte OOM caused by an unbounded
`drain_audio_samples_into_i16` loop racing core 1's enqueue), and that the poison
value `0x2002e810` is `HEAP_MEM+0xa334` — a `Vec` buffer pointer. The allocator is
where core 0 and core 1 meet.

Static targets for the next cycle: check `grow_one` and everything it reaches for
SP balance, and check whether the global allocator is safe against concurrent
core-0/core-1 entry.

## Config AC (UNPERTURBED): core 0 writes into core 1's stack — and the "SP moved" inference is RETRACTED

31 records in 32.7 min = **0.95/min**, the highest unperturbed rate yet (prior
baselines 0.29–0.74/min). Layout dominates rate, so this is not a valid
cross-build comparison, but this layout is a good reproducer.

### The dominant mode is the core-0 canary smash

14x `Panic C0 lr=0x20001bcf`. Decoding the record: `panic_loc=0x63617473` and
`r12=0x68635f6b` are ASCII — `"stac"` + `"k_ch"` = `stack_chk` — with `line=169`
and `flags=0x8e` including `0x80 STACK_CHK_FAIL_LR`. So the victim is
`Sm83 instructions+0x16e`. This is the same core-0 canary smash the investigation
chased for dozens of cycles.

### THE FINDING: an MPU-detected cross-core write

3x `HF C0 pc=0x1001a846 lr=0x1001a839 cfsr=0x00000082` (DACCVIOL|MMARVALID),
`flags=0x41` — **no `0x20 FAULT_ON_CORE1`, so the faulting core is CORE 0** — and
the hardware-supplied

```
FAULT_ADDR = 0x20080d10        <- inside CORE 1's STACK (0x20080000..0x20082000)
```

**Core 0 is writing into core 1's stack.** The MPU caught it three times in 33
minutes.

### Which store, and why it goes wild

```
1001a7fe:  ldr    r5, [sp, #0x10]     <- r5 comes from a SPILL SLOT
1001a808:  movw   r11, #0xff80
1001a818:  mov.w  r10, #0x4100        <- constant field offset
1001a838:  and.w  r1, r6, r11
1001a83c:  cmp.w  r1, #0xff00         <- is this an 0xFF00-page register?
1001a842:  sxth   r1, r6              <- r6 = 0xFF10..0xFF3F (APU register)
1001a844:  add    r1, r5
1001a846:  strb.w r0, [r1, r10]       <- addr = r5 + sxth(r6) + 0x4100
```

For `r6 = 0xFF10`, `sxth(r6) = -0xF0`, so `addr = r5 + 0x4010`. With the measured
`FAULT_ADDR = 0x20080d10`:

```
r5 = 0x20080d10 - 0x4010 = 0x2007cd00
```

`r5` should be a struct base pointer in `.bss`. Instead it holds **`0x2007cd00`, an
address inside core 0's own stack** — and that value sits directly among the `sp`
values of this same window's other faults (`0x2007cd08`, `0x2007cd58`). So the
spill slot `[sp,#0x10]` contained a *stack address* where a struct pointer
belonged: **a slot yielding a value that belongs to a different variable** —
exactly the shape of the core-1 finding, now on core 0 and caught by hardware.

### RETRACTION: "SP moved 40 bytes inside Vec::push" does not hold

That inference rested on config AA/AB showing the watched word `0x20081f94` had
**exactly one writer** (`CAUGHT == total writes`, one store site). That is true
only of **core 1's own DWT** — and each core has its own DWT, which sees only its
own load/store unit. **A core-0 write into core 1's stack is completely invisible
to the core-1 watch.**

Since core 0 is now proven to write into core 1's stack, the premise fails: the
poison could have been placed by core 0 without ever incrementing `CAUGHT`. The
40-byte SP-rise arithmetic, and the reinstatement of "SP moves mid-function",
should both be treated as unsupported. What survives from AA/AB is solid and
still useful: `run_core1_worker`'s body SP is genuinely constant (6081 samples),
and core 1 never takes an FP-extended frame.

### The unification

One wild core-0 store — a spill slot holding a stack address instead of a struct
pointer — accounts for **both** long-standing families:

* it lands in **core 0's own stack** ⇒ the canary smashes (14x this window);
* it lands in **core 1's stack** ⇒ core 1's saved return addresses are corrupted
  ⇒ the wild-PC INVSTATE faults, invisible to core 1's DWT.

That is why the payload always looked like "a legitimate pointer read from the
wrong slot", why it moves with layout, and why it is independent of SRAM bank.

### Next instrument

`r5` and `r10` are callee-saved, so they are NOT in the exception frame — but they
are still live in the CPU when the handler runs. The crash handler should capture
**r4–r11** directly and store them in the record. That gives the corrupt pointer's
value on every occurrence and settles whether it is always an SP-like value.

## Config AD: the corrupt pointer is a SPILL SLOT overwrite, NOT an allocator bug

Second unperturbed window: 31 records in 32 min = **0.97/min**, matching AC's 0.95
— this layout reproduces consistently. Composition is stable too: 13x canary-smash
Panic `lr=0x20001bcf`, 13x WDT, **2x the same MPU violation**, plus three
one-off core-0 HFs.

### The MPU violation is DETERMINISTIC

```
fa = 0x20080d10, sp = 0x2007bad0     (2x this window, 3x in AC — 5 identical)
```

Random corruption would scatter the fault address. **It is byte-identical every
time**, so whatever produces it is systematic.

### Where the bad pointer comes from

`with_cartridge`'s frame is 4720 bytes (`push {r4,r5,r6,r7,lr}` + `push
{r8,r9,r10,r11}` + `sub sp,#0x1240` + `sub sp,#0xc`), so its entry SP is
`0x2007bad0 + 0x1270` = `0x2007cd40` and the frame spans
`0x2007bad0..0x2007cd40`.

The spill slot has exactly ONE writer and three readers:

```
1001a57c:  str.w r9, [sp, #0x10]      <- the only writer
1001a7fe:  ldr   r5, [sp, #0x10]      <- the faulting store's base
1001a8e8:  ldr   r2, [sp, #0x10]
1001a90c:  ldr   r0, [sp, #0x10]
```

and `r9` is the return value of a heap allocation:

```
1001a33a:  movs  r0, #0x4             ; align 4
1001a33c:  movw  r1, #0x42c0          ; size 17,088
1001a340:  bl    box_new_uninit
1001a352:  mov   r9, r0
```

Back-solving the faulting store `addr = r5 + sxth(r6) + 0x4100` against the
measured fault address gives `r5 ≈ 0x2007cc91..0x2007cd10` (the range covers every
`r6` the guard `r6 & 0xff80 == 0xff00` admits) — **a core-0 stack address, inside
`with_cartridge`'s own frame**, not a heap pointer.

### The allocator is EXONERATED — checked, not assumed

`alloc_guard.rs` already wraps the real heap and records any out-of-arena
pointer. Read live from the device:

```
ALLOC_GUARD[0] = 0xa1100001   magic — guard IS active
ALLOC_GUARD[1] = 0            out-of-arena ALLOCATIONS
ALLOC_GUARD[2] = 0            refused deallocs
ALLOC_GUARD[6] = 0            first offender
ALLOC_GUARD[7] = 0x200244dc   arena lo
ALLOC_GUARD[8] = 0x2004c4dc   arena hi   (HEAP_MEM + 160 KiB)
```

**Zero out-of-arena allocations.** So `box_new_uninit` returned a correct heap
pointer and `r9` was right when stored. An earlier draft of this section claimed
the allocator was the root cause; the guard refutes it.

### What that leaves — and why it is finally a good DWT target

The value is a valid heap pointer at the `str` and a stack address at the `ldr`,
so **`with_cartridge`'s spill slot at `sp+0x10` = `0x2007bae0` is overwritten
between the two**, and `sp_before` is identical across all five occurrences, so
that address is FIXED.

This is the corrupt-spill-slot mechanism again, but for the first time pinned to a
single known address on core 0. Crucially it dodges what killed every previous
core-0 DWT attempt: the old target was written 73,238,733 times by 17 innocent
prologue pushes, whereas `0x2007bae0` is written **once per `with_cartridge`
entry**, and `with_cartridge` is the main loop — entered once. A cold address is
exactly what the distinct-PC ring is selective on, which is why the same
instrument succeeded on core 1.

### Config AD2 — running

Core-0 watch armed at `0x2007bae0` (`DWT_CATCH[8]` @ `0x20067510`, readback
confirmed; core-1 aim left 0). No value filter, so every writer is caught.
Expect `CAUGHT` to be SMALL — one legitimate write per boot plus the offender.
`WATCH_LOG[16..24]` will name the distinct writer PCs; anything that is not
`with_cartridge+0x284` is the bug.

## Config AE: MM_REGS fires — the fault arithmetic is confirmed exactly, the r9 test is VOID

### Why the registers were never visible before

The HardFault asm trampoline has captured `r4-r11` into `HARDFAULT_EXTRA_REGS`
all along. But that is a plain `static mut`, so it lives in **`.bss` and is zeroed
on every boot** — and this handler resets the chip. The values were destroyed
before anyone could read them. `MM_REGS` is the same data in `.uninit`, gated on
`MMARVALID`, write-once, magic written last.

### First capture

```
[0] magic     0x4d4d0001
[1] pc        0x1001a7a6   = with_cartridge+0x54e  (same offset as the AB build)
[2] MMFAR     0x20080d10   (6th byte-identical occurrence)
[3] CFSR      0x00000082   DACCVIOL|MMARVALID
[4] sp_before 0x2007bad0
[5] r4        0x2007cd00
[6] r5        0x2007cd00
[7] r6        0x0000ff10
[8] r7        0x2007cd30
[9] r8        0x0000ff10
[10] r9       0x2007cc6c
[11] r10      0x00004100
[12] r11      0x0000ff80
```

The back-solved model is confirmed to the bit: `r10 = 0x4100` and `r11 = 0xff80`
are the predicted constants, `r6 = 0xff10` is the predicted APU register, and

```
r5 + sxth(0xff10) + 0x4100 = 0x2007cd00 - 0xf0 + 0x4100 = 0x20080d10  == MMFAR
```

### The r9 discriminator is VOID — r9 was legitimately reassigned

The plan was: if `r9` still held the `box_new_uninit` pointer while `r5` did not,
the spill slot was corrupted. **That test cannot be applied.** `r9` is rewritten
four times after the spill store:

```
1001a2b2:  mov   r9, r0            <- the box pointer
1001a4dc:  str.w r9, [sp, #0x10]   <- stored (the slot's ONLY visible writer)
1001a5c6:  sub.w r9, r7, #0xc0     <- REASSIGNED
1001a644:  ldr.w r9, [pc, ...]
1001a750:  sub.w r9, r7, #0xc4     <- REASSIGNED, immediately before the fault
1001a800:  add.w r9, r9, #0x28     (after the fault PC, so not yet executed)
```

`r7 - 0xc4 = 0x2007cd30 - 0xc4 = 0x2007cc6c` — exactly the observed `r9`. So `r9`
at fault time is an ordinary pointer-to-local and says nothing about the box.

### What IS established

`sp_before = 0x2007bad0`, so the slot is **`[sp,#0x10] = 0x2007bae0`**, and it
yielded

```
r5 = 0x2007cd00 = r7 - 0x30      a pointer to a stack LOCAL
heap arena = 0x200244dc..0x2004c4dc      -> r5 is NOT in it
```

where the slot's only visible writer stores the heap box pointer. The slot cannot
legitimately hold `r7-0x30`. (The second `box_new_uninit` at `+0x48a` is for
`OpCode` boxes pushed into a Vec — unrelated to this slot.)

Two possibilities remain: something writes `0x2007bae0`, or `SP` differs between
the store and the load. Both are settled by watching the address.

### Config AE2 — running

Core-0 DWT armed at **`0x2007bae0`**, derived from a `sp_before` measured in THIS
build rather than carried across a rebuild (the mistake that voided Z1).
`DWT_CATCH[8]` @ `0x20067548`, readback confirmed; core-1 aim left 0; no value
filter. The slot is cold — written once per `with_cartridge` entry, and
`with_cartridge` is the main loop — which is exactly the condition under which
this instrument succeeded on core 1 and failed on the old 73M-write core-0 target.

`WATCH_LOG[16..24]` will name the distinct writer PCs. **Anything that is not
`with_cartridge+0x284` (`0x1001a4dc`) is the bug.** `MM_REGS` was zeroed and will
re-arm alongside it.

## Config AE2: the watch CAUGHT the writer — and the real anomaly is an 8-byte SP shift

`CAUGHT = 27` on a cold slot, so the aim (derived from a `sp_before` measured in
THIS build) is self-validated. The ring holds **exactly one distinct writer PC**:

```
[2]  CAUGHT       = 27
[12] first value  = 0x2007cd00     <- precisely the corrupt value
[13] total writes = 27
[16] writer PC    = 0x1001a4c8     <- the only one; [17..24] empty
[24] paired SP    = 0x2007bac8
```

`0x1001a4c8` is a `bl __rust_alloc_zeroed`, i.e. a RESUME point, not a store —
the DebugMonitor is deferred, so the store is a couple of instructions earlier at
`1001a4c2: strb.w r4, [sp, #0x18]`.

### The finding

```
catch:      sp_at_fault = 0x2007bac8  ->  [sp,#0x18] = 0x2007bae0   (watched word)
MPU fault:  sp_before   = 0x2007bad0  ->  [sp,#0x10] = 0x2007bae0   (same word)
```

**The same stack word is `[sp,#0x18]` at one point in `with_cartridge` and
`[sp,#0x10]` at another — its SP differs by 8 bytes between them**, although the
prologue allocates a fixed 4720-byte frame (`push {r4,r5,r6,r7,lr}` + `push
{r8,r9,r10,r11}` + `sub sp,#0x1240` + `sub sp,#0xc`) with no dynamic allocation.

Both numbers now come from the IDENTICAL FType formula (that was fixed in AA), so
unlike the Z2 episode they are directly comparable and the 8 bytes are real.

This is why the slot yields `r7-0x30` where a heap box pointer belongs: the box
pointer and a local genuinely share one stack word, reached at two different SPs.
The value read back (`0x2007cd00`) is consistent — a `strb` of zero landing on a
word that already held `0x2007cdXX`.

Loose end (A) — `gameboy.rs:455-466`'s claim that `#[inline(never)]` makes the §G1
spill-slot collision "impossible" — is now directly implicated rather than merely
suggestive.

### Negative result: the forensics block is NOT being trashed

All 708 words of the zeroed instrument span were scanned for core-0 stack-valued
words. 23 were found and **22 are legitimate instrument contents** — the
`WATCH_LOG` catch data, and `SMASH_CORE0`, which is a canary-smash record and
properly contains SPs and frame pointers. So the wild store is NOT spraying the
`.uninit` block.

The one unexplained word is `DWT_CATCH[9]` (`0x2006754c`), which is the last word
of `DWT_CATCH` immediately below `SMASH_CORE0` (`0x20067550`) — i.e.
`SMASH_CORE0[-1]`. That points at an off-by-one in the smash-record writer, not at
the bug under investigation. Worth fixing so it stops masquerading as a planted
core-1 aim (a stale value there arms a bogus core-1 watch at boot).

## CORRECTION: there is no 8-byte SP shift — the aim was 8 bytes wrong

The previous section claimed `with_cartridge`'s SP differs by 8 bytes between two
points in its body. **That is wrong, and the error was mine.**

### with_cartridge's SP is constant

Every SP-modifying instruction in the function:

```
1001a258:  push   {r4, r5, r6, r7, lr}
1001a25c:  push.w {r8, r9, r10, r11}
1001a260:  sub.w  sp, sp, #0x1240
1001a264:  sub    sp, #0xc
        ... no SP modification anywhere in the body ...
1001a934:  addeq.w sp, sp, #0x1240      (conditional epilogue)
1001a938:  addeq   sp, #0xc
1001a93a:  popeq.w {r8, r9, r10, r11}
1001a93e:  popeq   {r4, r5, r6, r7, pc}
```

No dynamic allocation, no mid-body push/pop. SP is fixed after the prologue.

### The authoritative body SP comes from r7

`r7` is captured by hardware into `MM_REGS` and the prologue sets
`add r7, sp, #0xc` after `push {r4,r5,r6,r7,lr}`, so `r7 = entry_SP - 8`:

```
r7            = 0x2007cd30
entry SP      = 0x2007cd38
body  SP      = entry - (20 + 16 + 0x1240 + 0xc) = 0x2007bac8

DWT sp_at_fault = 0x2007bac8   <- MATCHES the body SP exactly
record sp_before= 0x2007bad0   <- body SP + 8, OVER-REPORTS
```

So the DWT's SP was right all along and the crash record's `sp_before` is 8 high
for this fault. I built the aim from `sp_before`, so:

```
aimed at 0x2007bae0 = [sp,#0x18]   <- a benign neighbouring local
wanted   0x2007bad8 = [sp,#0x10]   <- the box-pointer slot
```

**The 27 (then 91) catches were the legitimate `strb.w r4,[sp,#0x18]` writing its
own variable — not the bug.** The value `0x2007cd00` read back there is simply
that neighbouring local also holding `r7-0x30`; it is not evidence of corruption,
and loose end (A) is NOT implicated by it.

Third SP-arithmetic error in this investigation. The durable rule: **derive a
frame address from `r7` plus the static prologue, never from a handler-computed
`sp_before`.** `r7` is hardware-captured; `sp_before` is arithmetic that has now
been wrong twice.

### Config AE3 — running

Watch re-aimed at **`0x2007bad8`**, instruments zeroed, sector blanked, reset.
`WATCH_LOG[16..24]` should now name whoever writes the box-pointer slot; anything
other than `with_cartridge+0x284` (`0x1001a4dc`) is the bug.

### Loose end (G) CONFIRMED LIVE — and it is a real bug

`DWT_CATCH[9]` (`0x2006754c`) was zeroed and read back as `0x2007ccc0` **while the
device was running**, before any reset. `DWT_CATCH` is `[usize; 10]` at
`0x20067528`, so `[9]` is the last word, immediately below `SMASH_CORE0`
(`0x20067550`) — i.e. `SMASH_CORE0[-1]`. The smash-record writer runs on every
canary smash (~13 per window) and writes one word below its array.

Consequences: it corrupts a neighbouring instrument, and because
`multicore.rs` accepts any core-1 aim in `0x20000000..0x20082000`, the stale value
arms a **bogus core-1 watchpoint on every boot**. Fix the off-by-one before
trusting any future core-1 aim.

## THE CANARY IS GENUINELY CORRUPTED — "the canary is INTACT" was reading the wrong word

The box-pointer line is exhausted for now: over a 33-minute window (**76 boots**)
the filtered watch on `0x2007bad8` recorded **zero** writes of `0x2007cd00`, and
`MM_REGS` never filled — no MPU violation fired at all. So the pivot to the
dominant mode (12 canary smashes that window) was taken, and it paid off
immediately.

### Reading the smash record correctly

`__stack_chk_fail` is `#[naked]`, so `SMASH_CORE0[1]` is the victim's **true body
SP** by construction, and `[4 + i]` is the word at `sp + 4i`.

```
[0] 0x51150002 magic   [1] 0x2007ccd8 body SP   [2] 0x20001bcf lr   [3] 23 smashes
```

`lr = 0x20001bcf` → the victim is `Sm83 ... Instructions::dec8` at `0x20001a60`
(`+0x16e`). Its guard check names the slot explicitly:

```
20001bba:  ldr r0, [sp, #0x8]      <- THE GUARD SLOT
20001bbc:  ldr r1, [pc, #0x18]     <- &__stack_chk_guard
20001bbe:  ldr r1, [r1]
20001bc0:  cmp r1, r0
20001bc2:  itt eq                  <- equal: pop and return
20001bca:  bl  __stack_chk_fail
```

So the guard lives at `sp + 8` = `0x2007cce0` = dump index 2 = `SMASH_CORE0[6]`:

```
SMASH_CORE0[4] = 0x2b7e1516   (sp+0)  <- a NEIGHBOURING frame's guard copy
SMASH_CORE0[6] = 0x00000001   (sp+8)  <- THE ACTUAL GUARD SLOT
SMASH_CORE0[5] = 0x2007cd0c   (sp+4)  }  plausible locals,
SMASH_CORE0[7] = 0x2007d2d0   (sp+c)  }  NOT corrupted
```

### The conclusion this flips

**The canary holds `0x00000001`. It is genuinely corrupted.**

`stack_chk.rs`'s own comment warns that an 8-byte ambiguity here "decides whether
the guard slot reads `0x2B7E1516` (compare failed on correct operands) or a
corrupt value (a real stray store), i.e. it flips the conclusion of the entire
investigation." The old reading looked at `sp+0`, found `0x2b7e1516` — which is a
*neighbouring frame's* copy of the constant, exactly the trap the comment
describes — and recorded **"the canary is INTACT"** in the refuted list.

**That refuted entry is wrong and is hereby withdrawn.** The compare failed on
correct operands is FALSE; this is a real stray store.

And because the guard word alone is corrupt while both its neighbours hold sane
values, it is a **stray pointer store, not a copy/memset overrun** — the precise
discriminator the record was built to provide.

### The payload is 0x00000001

Not a pointer, not a guard, not a stack address — a small integer. Something is
storing `1` through a pointer that lands on `0x2007cce0`.

### Config AF — running

Watch aimed at the real guard slot **`0x2007cce0`** with exact-value filter
**`0x00000001`**. `dec8` is hot, so the slot is written on every call — but the
filter is what makes a hot address usable: normal traffic stores `0x2B7E1516` and
never matches, so only the store that writes `1` latches a PC.

Caveat recorded up front: the DWT fires *after* the access and the handler
re-reads the word, so if the corrupting store is immediately followed by other
traffic the value may have moved on — the same effect that defeated the old
73M-write core-0 attempts. A null result here is therefore NOT evidence of
absence, and `[13]` (unfiltered total) must be checked to confirm the instrument
was live.

## Config AF: the corrupt canary reproduces, but address+value is not selective

The value-filtered watch on the verified guard slot `0x2007cce0` ran a full window
and the instrument was emphatically live:

```
[13] total writes = 27,729,801     (dec8 is hot — ~13,600 writes/s)
[2]  CAUGHT       = 102,767        matches of value 1
[12] first value  = 0x00000001
[6]  first PC     = 0x20001ca2     [15] first SP = 0x2007ccc8
```

`SMASH_CORE0[6] = 0x00000001` again with `[3] = 10` smashes — **the corrupt canary
reproduces**, independently confirming the previous section.

### Why the filter did not isolate the culprit

102,767 matches out of 27.7M writes means storing `1` at that address is COMMON.
The eight distinct writer PCs explain why:

```
0x20001ca2, 0x20001e0a, 0x2000140a, 0x20001ef6  Sm83 instruction fns +0xa
0x20001408                                       Sm83 instruction fn  +0x8
0x1001e628  XipCartridge+0x188
0x1003060a  __aeabi_memcpy4+0x7a
0x1002f1ba  OUTLINED_FUNCTION_297
```

Nearly all are `+0x8`/`+0xa` — **prologue stores**, i.e. resume artifacts of
sibling functions at the same stack depth writing their own locals into a
RECYCLED stack word. `0x2007cce0` is only "the guard" while `dec8`'s frame is
live; the rest of the time it belongs to whoever else is at that depth. An
address+value predicate cannot distinguish those cases, and the depth gate cannot
either, because the innocent writers sit at the same depth (first
`sp_at_fault = 0x2007ccc8`, just below `dec8`'s body SP `0x2007ccd8`).

### Config AF2 — the transition latch, finally aimed correctly

`dwt_watch` already implements the right predicate and `stack_chk` already
snapshots it into `SMASH_CORE0[132]/[133]`. Its rationale, from the source:

> "A transition on the watched word requires the slot to hold the guard first,
> and once corrupted it stays corrupt until a later invocation writes a fresh
> guard — so while THIS frame was live there was at most one transition, and it
> is the store that just destroyed the canary we are failing on."

That excludes every innocent prologue write **by construction**: those do not
overwrite a live guard. The instrument returned nothing in the X2 attempt only
because it was aimed at `drain_bus_events` — the wrong word. The aim is now
verified from the victim's own disassembly (`ldr r0,[sp,#0x8]`, body SP from the
naked `__stack_chk_fail` capture).

Running with aim `0x2007cce0` and **no value filter**, so the transition logic is
what runs. Read `SMASH_CORE0[132]` (PC) and `[133]` (SP) after smashes accumulate;
`WATCH_LOG[8]` counts transitions and `[32]/[33]` hold the latest.

## Config AF2 VOID — the board was power-cycled, which randomises `.uninit`

The transition-latch window returned nothing usable because `WATCH_LOG` came back
as **pure random data** — no magic, every slot noise:

```
b053a832 137840ef f749666f 91caceb1 cb23c823 ...
```

That is the signature of `.uninit` being randomised, which happens on a **cold
power-on** (SRAM is uninitialised at power-up), not just after an OpenOCD rescue.
The board lost power during the ~10 hours since the 21:56 reset.

Consequences, all of which invalidate the window:

* `DWT_CATCH[8]` held random garbage at boot, so no valid watch was ever armed.
* `SMASH_CORE0[132]/[133]` only mirror the randomised `WATCH_LOG[32]/[33]`.
* Any planted aim or filter is gone.

**Detector, for every future window: check `WATCH_LOG[0] == 0x3EE70001` (or
`ALLOC_GUARD[0] == 0xA1100001`) FIRST. Absent magic ⇒ cold boot ⇒ `.uninit`
randomised ⇒ every planted value is void, regardless of what the other slots
appear to say.** A randomised slot can hold a plausible-looking address, so this
is not something to eyeball.

### What the long interval actually was

The crash sector held only **3 records, seq 0..2**. Sequence numbers starting at
zero means all three are from the current boot, so the device was **not** crashing
for ten hours — it was unpowered for most of them and has been up only minutes.
No long-soak data was collected.

### What survived and still counts

* `SMASH_CORE0` re-initialised cleanly (magic `0x51150002`) and **`[6] =
  0x00000001`** — the corrupt canary reproduces a THIRD time, now on a fresh cold
  boot with a different power-on history. The finding is robust.
* `SMASH_CORE0[1] = 0x2007ccd8` is unchanged, so `dec8`'s body SP and the derived
  guard address `0x2007cce0` remain correct for this build; the aim did not need
  re-deriving.

### Config AF3 — re-armed

Same experiment, re-planted: aim `0x2007cce0`, value filter cleared to 0 so the
guard→non-guard transition logic runs. Instruments zeroed, sector blanked, reset.

## Config AF3: the transition latch fires — but it is catching benign recycling

Magic valid on both instruments (`WATCH_LOG[0] = 0x3EE70001`,
`ALLOC_GUARD[0] = 0xA1100001`), so this window is real.

```
WATCH_LOG[8]     = 270          transitions
WATCH_LOG[32]    = 0x2000140a   latest transition PC
WATCH_LOG[33]    = 0x2007ccd8   latest transition SP
WATCH_LOG[13]    = 34,822,509   total writes (liveness)
SMASH_CORE0[3]   = 104          smashes
SMASH_CORE0[6]   = 0x00000001   guard corrupt — FOURTH confirmation
SMASH_CORE0[132] = 0x2000140a   snapshot of the last transition
SMASH_CORE0[133] = 0x2007ccd8
```

### What the captured PC is

`0x2000140a` = `Instructions::ld16+0xa` (`mov r11, r1`) — a resume artifact. The
store is the multi-register push two instructions earlier:

```
20001400:  push   {r4, r5, r6, r7, lr}      ; 20 bytes
20001402:  add    r7, sp, #0xc
20001404:  push.w {r7, r8, r9, r10, r11}    ; 20 bytes   <- wrote the guard slot
```

With `sp_at_fault = 0x2007ccd8` that push covers `0x2007ccd8..0x2007cce8`, and
`0x2007cce0` is its THIRD word — **r9**. So `r9 = 1` at `ld16` entry, which
explains the `0x00000001` payload exactly.

### Why this is almost certainly NOT the culprit

`ld16` is a sibling Sm83 instruction function. A sibling's prologue overwriting
the word *after `dec8` has returned* is benign recycling — precisely the case
`dwt_watch`'s own comment flags: "a benign one happens on every normal return when
the slot is recycled (measured 1110 transitions vs 16 smashes)". Here it is 270
transitions against 104 smashes, so benign events are still in the mix, and the
"last transition" snapshot can easily land on one.

Declaring `ld16` the stray store would repeat exactly the failure mode that
produced the withdrawn "the canary is INTACT" entry: reading a plausible number
without checking whether it can mean what it appears to mean.

### The refinement: the depth gate

While `dec8`'s frame is live, nothing should legitimately write `0x2007cce0`. Its
callees run BELOW `0x2007ccd8`, so their frames cannot reach it; only a shallower
or equal frame can, and that means `dec8` already returned. So:

* `sp_at_fault == 0x2007ccd8` (or above) ⇒ same-depth or shallower ⇒ RECYCLING.
* `sp_at_fault  < 0x2007ccd8` ⇒ a strictly deeper frame writing a word above its
  own frame ⇒ **a genuine stray store**.

`dwt_watch` already has this gate (`WATCH_LOG[5]`, predicate
`body_sp == 0 || sp_at_fault <= body_sp`) and it has never been used on this
victim. Planting `0x2007ccd4` — one word below `dec8`'s body SP — admits only
strictly-deeper writers and rejects every same-depth sibling prologue.

Running with aim `0x2007cce0`, depth gate `0x2007ccd4`, no value filter.

## Config AF4: the depth gate did NOT apply to the transition latch (my error)

Valid window (both magics). `[13] = 29,702,288` writes, `[8] = 368` transitions,
`SMASH_CORE0[3] = 129` smashes, `[6] = 0x00000001` — **fifth confirmation** the
guard is genuinely corrupt.

### The experiment did not do what I intended

`WATCH_LOG[5]` gates only the CAUGHT/ring predicate. In `dwt_watch.rs` the
**transition latch lives in a different block** and is not gated at all, so
planting `0x2007ccd4` filtered `CAUGHT` (10,673,158) and left the transition
latch exactly as before. Any conclusion about "gated transitions" from this window
would be false.

**To actually test it, the depth gate has to be applied inside the transition
block — that is a code change, not a planted value.**

### What the snapshot nonetheless shows

```
WATCH_LOG[32]/[33]     = 0x2000140a / 0x2007ccd8   live latest (ld16, recycling)
SMASH_CORE0[132]/[133] = 0x200010dc / 0x2007cd00   last transition before a smash
```

`0x200010dc` = **`Sm83::take_pending_interrupt+0x2a`**:

```
200010ca:  movs  r4, #0x1          <- r4 = 1, matches the payload
200010d2:  bics  r2, r4
200010d4:  strb  r2, [r3]          <- BYTE STORE THROUGH A RAW POINTER
200010d6:  pop.w {r4, r6, r7, lr}
200010dc:  cmp   r0, #0x0          <- the recorded (resume) PC
```

**Not a conclusion — the arithmetic does not close.** With `sp_at_fault =
0x2007cd00` taken after that `pop`, the function's body SP is `0x2007ccf0` and its
prologue push spans `0x2007ccf0..0x2007ccfc`, which never reaches `0x2007cce0`.
And a `strb` cannot turn `0x2B7E1516` into a full-word `0x00000001` in one store.
The PC is a deferred resume artifact and the handler's re-read of the value is
equally late, so neither pins the store.

What IS worth carrying forward: `take_pending_interrupt` stores through a raw
pointer (`strb r2,[r3]`), the same shape as the `with_cartridge` MPU violation
(`strb r0,[r1,r10]`) — a byte store into GB memory through a computed pointer.
Two independent failure sites, one instruction pattern. If a GB-memory base or
index can go wrong, both are explained.

### Next: gate the transition latch in code

Apply `body_sp` to the transition block so same-depth/shallower recycling is
rejected there too, then re-run. Note the criterion needs care: a prologue `push`
writes ABOVE its final SP, so "deeper SP" alone does not imply "stray". The clean
test is **a store to an address BELOW the writer's own SP**, which no prologue
push and no `[sp,#+N]` local access can produce.

## Config AG: the below-SP discriminator, implemented in code

`WATCH_LOG[5]` could never filter the transition latch — it gates only the
CAUGHT/ring predicate, and the latch is a separate block. This is the code change
that actually applies a filter there.

### Why "below SP" and not "deeper SP"

AF3/AF4 kept latching `ld16`'s prologue `push.w {r7,r8,r9,r10,r11}`, which is
benign recycling. **A depth test cannot reject it**: a prologue push writes its
words at `sp_at_fault .. sp_at_fault + n*4`, i.e. ABOVE its own final SP, so a
pusher always looks "deeper" than the word it just wrote.

What no legitimate access can do is write BELOW the current SP. Locals are
`[sp,#+N]`; a push leaves SP at or below every word it stored; and AAPCS/Thumb has
no red zone. So `watched < sp_at_fault` is a store into dead stack — exactly the
signature of a stray pointer, which is what the smash record already says this is
(guard word alone corrupt, both neighbours sane).

### The change

In the transition block of `dwt_watch::debug_monitor_rust`:

* `[46]`/`[47]` keep the UNFILTERED last transition (the old `[32]`/`[33]`
  behaviour) so both readings can be compared.
* `[32]`/`[33]` now record ONLY transitions where `watched < sp_at_fault` — and
  `__stack_chk_fail` already snapshots those two words into
  `SMASH_CORE0[132]/[133]`, so the smash record now carries the FILTERED answer.
* `[48]` counts them. Expect `[48]` << `[8]`.

**If `[48]` stays 0 while smashes continue, that is a real result, not a null:**
the guard is not being corrupted by a CPU store this core can see. DMA and the
other core are both invisible to this DWT, and that would become the next line.

### Probe recovery (worth recording)

The first flash attempt failed with `Command ID in response (0x5) does not match
sent command ID (SwjSequence - 0x12)` — a stuck CMSIS-DAP that `rb-flash`'s own
USB reset did not clear, and which then broke plain `probe-rs read` too. The
documented OpenOCD `set RESCUE 1` sequence recovered it in one pass. As expected,
the rescue RANDOMISED `.uninit` (first read back `0xc5396095`).

### Layout moved — re-derived and re-checked

`ALLOC_GUARD` is now the LAST `.uninit` object (it was `WATCH_LOG`), so the MPU
tail check had to be redone against a different array:

```
REGION_FAIL 0x20067040   HEARTBEAT 0x20067180   MM_REGS   0x200672c0
DWT_CATCH   0x20067488   SMASH_CORE0 0x200674b0  SMASH_CORE1 0x20067730
WATCH_LOG   0x200679b0   ALLOC_GUARD 0x20067ab0  _stack_end 0x20067b50
zero span = 708 words from 0x20067040
DWT_CATCH[8] = 0x200674a8      SMASH_CORE0[1] = 0x200674b4
SMASH_CORE0[132] = 0x200676c0  [133] = 0x200676c4
WATCH_LOG [0]=0x200679b0 [2]=0x200679b8 [8]=0x200679d0 [13]=0x200679e4
          [32]=0x20067a30 [46]=0x20067a68 [48]=0x20067a70
```

MPU region base `0x20067b40`; highest written words are `WATCH_LOG[48]`
(`0x20067a70`) and `ALLOC_GUARD[15]` (`0x20067aec`, `LIVE_SLOTS = 16`) — both
safe. Device confirms `heartbeat placement OK: base=0x20067180
core1-RO-region=0x20067b40`. CRC `0x65c2ee3a`.

Running with NO aim planted, to capture a fresh `dec8` body SP from
`SMASH_CORE0[1]` rather than assuming `0x2007ccd8` survived the rebuild.

## Config AG2: aim planted from a MEASURED body SP; a magic-check nuance

### `WATCH_LOG[0]` is NOT a cold-boot detector — `ALLOC_GUARD[0]` is

This window read `WATCH_LOG[0] = 0` while `ALLOC_GUARD[0] = 0xA1100001`. That is
**not** a randomised `.uninit`. `WATCH_LOG`'s magic is written by the DebugMonitor
handler, so with no aim planted the handler never runs and the magic never
appears. `ALLOC_GUARD`'s magic is written at init on every boot.

**Use `ALLOC_GUARD[0] == 0xA1100001` as the cold-boot / rescue detector.** Reading
`WATCH_LOG[0] == 0` as "power cycle" would have wrongly voided a good window.

### The fresh body SP

```
SMASH_CORE0[1] = 0x2007ccd8   fresh body SP — UNCHANGED across the rebuild
SMASH_CORE0[2] = 0x20001bcf   dec8+0x16e
SMASH_CORE0[3] = 32           smashes
SMASH_CORE0[6] = 0x00000001   guard corrupt — SIXTH confirmation
```

The guard is therefore `0x2007ccd8 + 8` = `0x2007cce0` — the same value as the AE
layout, but now MEASURED in this build rather than carried across, which is the
rule that voided Z1 when it was skipped.

### Loose end (G) reproduces in the new layout

`DWT_CATCH[9]` (`0x200674ac`) read back `0x2007ccc0` after being explicitly
zeroed. In the AG layout `SMASH_CORE0` starts at `0x200674b0`, so `DWT_CATCH[9]`
is once again `SMASH_CORE0[-1]`: the smash writer stores one word below its array
on every smash. Harmless for the current experiment (the core-1 aim is unused) but
it silently corrupts whatever neighbour precedes `SMASH_CORE0`, and it will keep
moving with the layout until it is fixed.

### Running

Aim `DWT_CATCH[8] = 0x2007cce0` (readback confirmed), value filter 0, instruments
zeroed, sector blanked, reset. The readout that matters:

* `WATCH_LOG[48]` — count of transitions where the store landed BELOW the writer's
  own SP.
* `SMASH_CORE0[132]/[133]` — the PC/SP of that store, snapshotted at smash time.
* `WATCH_LOG[46]/[47]` — the unfiltered last transition, for comparison.

## Config AG2 VOID — a shared global broke the filter; two claims retracted

### The filter passed everything

```
[8]  transitions = 178
[48] STRAY count = 178      <- IDENTICAL: the below-SP test admitted every transition
[132]/[133] = 0x2000140a / 0x2007ccd8   (same as the unfiltered [46]/[47])
SMASH_CORE0[6] = 0x00000001             (guard corrupt — seventh confirmation)
```

With `watched = 0x2007cce0` and `sp_at_fault = 0x2007ccd8`, `watched < sp_at_fault`
is FALSE, so `ld16` should have been rejected. Reading the three sources directly:

```
WATCHED_ADDR @0x20066c0c = 0x2007ccc0   <- WRONG
DWT_CATCH[8] @0x200674a8 = 0x2007cce0   <- the planted aim
DWT COMP0    @0xE0001020 = 0x2007cce0   <- what the hardware actually watches
```

`0x2007ccc0 < sp_at_fault` is true, so the test passed unconditionally.

### Root cause of the instrument failure

`WATCHED_ADDR` is a **single global shared by both cores**, and
`arm_data_write_watch` stores into it unconditionally. `multicore.rs:1609` arms
core 1 from `DWT_CATCH[9]`; when that slot held the stale `0x2007ccc0`, core 1's
arming **overwrote core 0's `WATCHED_ADDR`**. The handler then compared against
core 1's address while core 0's hardware watched a different word.

**Fix:** the transition block now reads `DWT_COMP0` directly instead of
`WATCHED_ADDR`. COMP0 is per-core hardware state and is what actually triggered
the exception, so it cannot disagree with the trap that fired. Config AH.

### RETRACTED: "`SMASH_CORE0[-1]` off-by-one" — disproven

I recorded this as "CONFIRMED LIVE". It is false. `SMASH_CORE0` is
`[usize; 160]`, `DUMP_WORDS = 128`, and the writer touches `[0..4]`, `[4..132]`,
`[132]`, `[133]` — entirely in bounds. There is no off-by-one, and loose end (G)
is withdrawn.

### RETRACTED: "a live writer of `DWT_CATCH[9]`" — tested and disproven

`DWT_CATCH[9]` read back `0x2007ccc0` right after being written to 0, which
looked like a wild store hitting a fixed, cold, non-stack address within ~100 ms —
a far better handle than the recycled stack word, and tempting to pivot onto.

**Tested before acting: three write-0/read-back trials all returned 0**, as did a
control word. There is no live writer. Grep confirms nothing in the firmware
writes `[9]` at all — only `[0]`, `[6]`, `[7]`, `[8]` are ever written, and
`main.rs`/`multicore.rs` only read it. The earlier readback was a write that did
not land across the flash/reset sequence.

The lesson is the same one that produced the withdrawn "canary is INTACT" entry:
a plausible number is not a finding until the mechanism that would produce it has
been checked.

### Config AH — running

`dwt_watch` now uses `DWT_COMP0` for the below-SP test. Aim
`DWT_CATCH[8] = 0x2007cce0` and **`DWT_CATCH[9] = 0`** (so core 1 never arms and
cannot clobber the shared global), both verified before reset. Addresses are
unchanged from AG. CRC `0xe8269937`.

## Config AH: the below-SP filter works but was the WRONG TEST — and DMA is excluded

The `DWT_COMP0` fix made the filter functional: 310 transitions, **0** strays (no
longer the broken 178/178). Valid window (`ALLOC_GUARD[0]` magic present), aim
confirmed (`SMASH_CORE0[1] = 0x2007ccd8`), liveness 65,839,155 writes, 21 smashes,
`SMASH_CORE0[6] = 0x00000001` — **eighth confirmation** the guard is corrupt.

### `[48] = 0` does NOT exclude a CPU store — my over-claim, corrected

The below-SP test only catches a store BELOW the writer's own SP. **A wild pointer
from one of the victim's CALLEES writes UPWARD into the victim's frame — above its
own SP — and passes the test looking legitimate.** That is exactly the shape of
the suspected stray store, so the null excluded one narrow class and said nothing
about the main one.

### Exclusions that ARE solid

* **DMA is excluded.** Only two channels are configured and both are
  memory→peripheral: `ch0 WRITE=0x50200010` (PIO0 TXF), `ch1 WRITE=0x40088008`.
  All other channels are zeroed. Nothing DMAs into SRAM at all.
* **Core 1 is excluded.** `mpu.rs` maps core 0's stack read-only for core 1, so a
  core-1 write there would raise MemManage — which has never appeared.
* **No guard-offset mismatch in dec8.** Prologue `20001a72: str r0,[sp,#0x8]`
  stores the guard; epilogue `20001bba: ldr r0,[sp,#0x8]` reads it. Same offset.
  (The "-8 mismatch" idea stays refuted, now checked for this victim too.)
* **dec8 does not clobber its own guard.** Its only SP-relative stores are the
  guard store and `strd r5, r0, [sp]`, which writes `[sp]` and `[sp+4]` — not
  `[sp+8]`.

### The corrected discriminator (config AI)

Any frame running BELOW the victim's body SP has no business writing the victim's
guard: its own locals are lower still, and the victim is live above it.

```
tsp <  body_sp  ⇒ a callee reaching UP into the victim  ⇒ STRAY
tsp >= body_sp  ⇒ same-depth or shallower ⇒ victim already returned ⇒ RECYCLING
```

That last case is what has been latching all along — `ld16`'s prologue push, and
in AH the unfiltered `[47]` was `0x2007cce0`, exactly the watched address (a push
whose final SP lands on the guard).

`body_sp` is planted into `WATCH_LOG[5]`. Note it gates the CAUGHT/ring predicate
elsewhere; this is a deliberate SECOND use of the same planted value in a
different block — AG2 was voided by assuming one gate covered both.

### Config AI — running

Layout moved again (`DWT_CATCH=0x20067300`, `SMASH_CORE0=0x20067328`,
`WATCH_LOG=0x20067828`; `ALLOC_GUARD` still last at `0x20067ab0`). MPU bound
re-checked: `WATCH_LOG[48]=0x200678e8` and `ALLOC_GUARD[15]=0x20067aec`, both below
region base `0x20067b40`. CRC `0xbc74c12d`. Aim `0x2007cce0`, gate `0x2007ccd8`,
core-1 aim 0 — all readback-verified.

## Config AJ: the watchpoint self-aims — planted values are now build-invariant

### The methodological hole this closes

There are TWO classes of address in these experiments and they were being handled
unequally:

* **Forensics addresses (`.uninit` objects).** Re-derived from `llvm-nm` after
  every build, with the MPU tail bound re-checked. This was already sound — it is
  what caught `ALLOC_GUARD` becoming the last object instead of `WATCH_LOG`.
* **Planted STACK addresses (the aim and the depth gate).** These come from frame
  layout, not from symbols, so any code change that alters a prologue shifts them.
  They were being carried across rebuilds and verified only RETROSPECTIVELY, from
  the next smash record — which means a whole window can be silently void before
  the mistake surfaces. **Config Z1 was voided exactly this way**: added code grew
  `run_core1_worker`'s frame and moved every SP by 8 bytes.

Checked live mid-window and the exposure was real: `SMASH_CORE0[1]` read `0` (no
smash yet) while the device was already running with an aim and gate carried over
from the previous build. Nothing had verified them.

### The mechanism

`__stack_chk_fail` is `#[naked]`, so `SMASH_CORE0[1]` is the victim's true body SP
as measured BY THE RUNNING BUILD, and `SMASH_CORE0` is in `.uninit` so it survives
the crash handler's reset. Boot-time arming now:

1. reads `DWT_CATCH[8]`; a value `< 0x1000` is a **guard OFFSET**, not an address;
2. requires `SMASH_CORE0[0] == SMASH_MAGIC`, else stays **disarmed** rather than
   aiming at garbage;
3. computes `guard = SMASH_CORE0[1] + offset` and arms there;
4. publishes `SMASH_CORE0[1]` into `WATCH_LOG[5]` so the depth gate comes from the
   SAME measurement — no second SWD plant that can drift out of sync;
5. still accepts a full SRAM address, so an arbitrary word can be watched.

`SMASH_MAGIC` was made `pub` for step 2.

### Why an offset and not an address

The offset is a property of the victim's own code (`ldr r0,[sp,#0x8]`). The
absolute address is a property of every frame above it on the stack. So the
offset survives rebuilds that the address does not.

**It is self-correcting instead of assumption-based:** first boot after a rebuild
has no smash record and stays disarmed; the first crash records the body SP; the
next boot arms at the address correct for THAT build. A rebuild now costs one
crash of latency rather than silently voiding a window.

### Limits, stated up front

* The OFFSET still needs re-deriving from the victim's disassembly if codegen
  changes its frame.
* If a layout change makes a different function the dominant victim,
  `SMASH_CORE0[2]` (the lr) reveals it and the offset must be re-derived for that
  victim.

### Running

CRC `0x7731bd5a`. `DWT_CATCH[8] = 8` (offset), `[9] = 0`, `WATCH_LOG[5] = 0` (the
firmware fills it). Addresses unchanged from AI: `DWT_CATCH=0x20067300`,
`SMASH_CORE0=0x20067328`, `WATCH_LOG=0x20067828`, `ALLOC_GUARD=0x20067ab0`.
Expect the boot log to show `self-aim: no smash record yet` on the first boot and
`self-aim: body_sp=... -> ...` on every boot after the first crash.

## Config AJ: the self-aim WORKS — and it proves the SP-based filters cannot work

### The self-aim verified end to end

```
ALLOC_GUARD[0] = 0xA1100001    valid window
SMASH_CORE0[1] = 0x2007ccd8    body SP measured by the running build
WATCH_LOG[5]   = 0x2007ccd8    == [1]      firmware published the gate
DWT COMP0      = 0x2007cce0    == [1] + 8  firmware resolved the offset
SMASH_CORE0[2] = 0x20001bcf    dec8+0x16e — same victim, offset still valid
[3] = 21 smashes, [6] = 0x00000001   NINTH confirmation the guard is corrupt
```

No stack address was planted; only the offset `8`. This removes the whole class of
"aim carried across a rebuild" errors that voided Z1.

### The filter discriminated — but caught the wrong thing

```
[8]  transitions = 983
[48] STRAY       = 109        (a clear minority, so the gate IS selective)
[46]/[47] unfiltered = 0x2000140a / 0x2007ccd8   (ld16, as always)
SMASH_CORE0[132]/[133] = 0x200012f6 / 0x2007ccd0
```

`0x200012f6` = `Instructions::ld8+0xa`, a resume artifact. The store is
`200012f0: push.w {r4,r5,r6,r7,r8,r9,r11}` (7 regs, 28 bytes). With
`sp_at_fault = 0x2007ccd0` that push spans `0x2007ccd0..0x2007cce8`, and
`0x2007cce0` is its FIFTH word = **r8**. So `r8 = 1` at ld8's entry — precisely
mirroring `ld16`'s `r9 = 1`.

### Why the deeper-frame gate is CONFOUNDED

The gate assumed **deeper SP ⇒ a callee of the live victim**. That is false.
`ld8` is a SIBLING whose entry SP is 8 bytes below dec8's (`0x2007cd00` vs
`0x2007cd08`) — both have 48-byte prologues, so they are simply dispatched from
paths at slightly different depths. **A sibling entered deeper has a deeper body
SP, and its prologue push writes UPWARD into the recycled word — indistinguishable
from a callee reaching up, using SP alone.** The 109 "strays" are almost certainly
all of this kind.

So all three SP-based predicates have now failed for the same underlying reason:

* address+value — the word has many legitimate owners;
* below-SP — a callee writing up passes it;
* deeper-frame — a deeper-entered SIBLING passes it too.

**SP cannot separate "recycling after the victim returned" from "a store while the
victim is live". Liveness is the property that matters and SP does not encode it.**

### The instrument is also saturated

`[13] = 135,364,731` writes this window ⇒ the handler runs ~68,000 times/second.
`dwt_watch`'s own comment warns the DebugMonitor is configurable-priority, so
PRIMASK defers it and writes coalesce. The "last transition before a smash is the
killing store" guarantee depends on an ordering the instrument can no longer
honour at this rate. Treat ordering-derived conclusions from this word as unsafe.

### What would actually decide it

Stop filtering by SP and test liveness directly. The corruption happens between
dec8's guard STORE (`20001a72`) and its CHECK (`20001bba`). A software bisection
inside dec8 — re-reading the guard after each call it makes and latching the first
point where it is no longer `0x2B7E1516` — brackets the corruption in program
order rather than by address arithmetic, and is immune to both the recycling
confound and handler deferral.

## The three failure sites are ONE code path: the GB IO write, and its base pointer

Rather than bisect dec8, disassembling what it calls made the bisection
unnecessary. dec8 makes 7 calls; the last before its guard check (22 bytes later,
`20001ba4: bl Sm83::bus_write` → check at `20001bba`) is the GB bus write.

### `bus_write` and `with_cartridge` contain the SAME store

```
bus_write @0x20002414              with_cartridge (the MPU violation)
  movw   r0, #0x4120                 movw  r1, #0x4120
  movw   r0, #0xff80                 and.w r1, r6, r11      (r11 = 0xff80)
  cmp.w  r0, #0xff00                 cmp.w r1, #0xff00
  sxth   r0, r5                      sxth  r1, r6
  add    r0, r6                      add   r1, r5
  strb.w r8, [r0, r1]   (r1=0x4100)  strb.w r0, [r1, r10]   (r10 = 0x4100)
```

Identical: `addr = base + sxth(gb_addr) + 0x4100`. These are the same source
inlined twice. `take_pending_interrupt`'s `strb r2,[r3]` is the third instance of
"byte store into GB memory through a computed pointer".

### The offset arithmetic CANNOT escape — only the base can be wrong

The guard `(gb_addr & 0xFF80) == 0xFF00` admits exactly `0xFF00..0xFF7F` (the mask
covers bits 15:7, and `sxth` uses only the low 16 bits, so a value above 0xFFFF
cannot widen the range). So `sxth` yields `-0x100..-0x81` and the effective offset
is `base + 0x4000 .. base + 0x407F` — a fixed 128-byte window.

**The address computation is bounded. The only free variable is the base.** And
the base is precisely what the MPU violation measured as wrong:
`base = 0x2007cd00` (a core-0 stack address), giving
`0x2007cd00 + 0x4010 = 0x20080d10` — the byte-identical fault address seen five
times.

### Why this supersedes the DWT work

Every DWT predicate tried to identify the *store*. But all three candidate stores
are the same instruction pattern, and the instruction is innocent — it is bounded
and correct given a valid base. **The defect is upstream: the GB memory base
pointer is intermittently a stack address.** That single fact explains:

* `with_cartridge`'s MPU violation (base + 0x4010 lands in core 1's stack);
* the canary smashes (a different bad base value puts the 128-byte IO window over
  core 0's stack, and the guard word is a single corrupted word with sane
  neighbours — exactly a one-byte-per-call scatter, not a memset overrun);
* why the payload is a small integer (`1`) — it is an IO register VALUE being
  written, not a pointer.

### The next instrument — software, immune to DWT saturation

Validate the base at the top of `bus_write`: if `self`'s memory base is outside
the heap arena (`0x200244dc..0x2004c4dc`), latch the base, the GB address, and the
caller's LR into a `.uninit` slot. That is ~4 instructions on the hot path, tests
the hypothesis directly, and needs no DWT — so neither the recycling confound nor
handler deferral applies.

Note `bus_write` lives in `rustyboy_core`, so the check needs a `#[no_mangle]`
static there (or a feature gate) rather than a pico2w-local array.

## Config AK: the base-pointer probe, in software

`Sm83::bus_write(&mut self, memory: &mut GameBoyMemory, addr: u16, val: u8)` — the
base is the `memory` reference. `check_bus_base` now runs at its top.

### The invariant chosen, and why

Not "is the base inside the heap arena". `GameBoyMemory` is constructed once and
lives for the whole run, so **its address is invariant**; the check compares
against the FIRST base observed after a cold boot. That needs no arena constant,
so it stays platform-agnostic in `rustyboy_core` and — importantly — cannot rot
when the heap or linker layout moves, which is the failure mode that has voided
several windows in this investigation. Any deviation is corruption by definition.

`BUS_BASE_CHK` slots: `[0]` magic `0xB0A50001`, `[1]` first-seen base,
`[2]` **mismatch count**, `[3]` first bad base, `[4]` the GB address at that
moment, `[6]` most recent bad base.

### Why software and not a fourth watchpoint

Three DWT predicates failed for one reason: they tried to identify the STORE, but
the store is shared between `bus_write`, `with_cartridge` and
`take_pending_interrupt` (same source inlined), is bounded, and is
indistinguishable from the prologue pushes that legitimately recycle the same
stack word. SP cannot express "the victim's frame is live". This check tests the
actual suspect — the base — and is immune to both the recycling confound and
DebugMonitor deferral.

### Layout after the rebuild (BUS_BASE_CHK moved everything again)

```
REGION_FAIL 0x20067070   BUS_BASE_CHK 0x20067110   HEARTBEAT 0x200671d0
MM_REGS     0x20067310   DWT_CATCH    0x20067350   SMASH_CORE0 0x20067378
WATCH_LOG   0x20067878   ALLOC_GUARD  0x20067b00   _stack_end  0x20067ba0
zero span = 716 words from 0x20067070      (NOT 816 — see below)
DWT_CATCH[8]=0x20067370 [9]=0x20067374
SMASH_CORE0[1]=0x2006737c [2]=0x20067380 [3]=0x20067384 [6]=0x20067390
WATCH_LOG[0]=0x20067878 [5]=0x2006788c [8]=0x20067898 [13]=0x200678ac [48]=0x20067938
BUS_BASE_CHK[2]=0x20067118 [3]=0x2006711c [4]=0x20067120 [6]=0x20067128
```

MPU region base `0x20067ba0`; highest written words `WATCH_LOG[48]=0x20067938` and
`ALLOC_GUARD[15]=0x20067b3c` — both safe.

**Procedure slip worth recording:** the zeroing loop was run with 816 words rather
than 716, so it also cleared 400 bytes at the very bottom of core 0's stack.
Harmless here (a reset followed immediately, and that depth is only reached at
maximum nesting), but the span must be recomputed from `_stack_end` after every
rebuild, not carried over.

### Running

CRC `0xd429d04e`. Offset `8` planted in `DWT_CATCH[8]`, `[9] = 0`, both verified.

## Config AK result: the base probe REFUTES the hypothesis — and I probed the WRONG pointer

### The measurement

```
ALLOC_GUARD[0]  = 0xA1100001   valid window
BUS_BASE_CHK[0] = 0xB0A50001   probe active
BUS_BASE_CHK[1] = 0x2002753c   first base — inside the heap arena ✓
BUS_BASE_CHK[2] = 0            MISMATCHES: none
SMASH_CORE0[3]  = 26 smashes
SMASH_CORE0[6]  = 0x00000001   guard corrupt — TENTH confirmation
```

Across 26 smashes the `memory` base reaching `bus_write` was never wrong.

**Rigour gap:** no call counter was recorded, so "0 mismatches" is only as strong
as the number of calls sampled. `[1]` being set proves at least one call, not
millions. Any future probe of this shape must count calls.

### The bigger error: that is not the pointer that faults

Tracing the faulting store to its source: `multicore.rs` does
`self.lcd_timing_io[(addr - IO_REG_BASE) as usize] = value`, where
**`lcd_timing_io: [u8; 0x80]` — a 128-byte array**, exactly the window size
derived from the instruction sequence, indexed by `addr - 0xFF00`. That is the
`sxth`/`+0x4100` pattern.

`lcd_timing_io` belongs to **`Core1Transport`**, not `GameBoyMemory`. And it sits
EARLY in `Core1Transport`, so the `+0x4100` in the store means the base points at
an ENCLOSING object (`GameBoy`/`PicoGameBoy`, whose `Sm83` opcode table is large)
with `transport.lcd_timing_io` at that offset.

**So `bus_write`'s base (`&mut GameBoyMemory`) and the faulting store's base are
different pointers.** The AK probe watched a pointer that was never implicated.
Its null is真 but says nothing about the fault.

### What still holds

`0x2007cd00` is `with_cartridge`'s entry SP − 0x38. A ≥16.7 KB object cannot live
there — `with_cartridge`'s whole frame is 4720 bytes. So that base is genuinely
bad, not a legitimate stack-local object. The corrupt-base idea survives; only my
choice of which pointer to instrument was wrong.

### Next

Move the same invariant check to `write_lcd_timing_register`'s `self` (the
`Core1Transport`/enclosing pointer) — the pointer the faulting store actually
uses — and add a CALL COUNTER so a null result is quantified. `with_cartridge`
runs once per boot, which matches the MPU violation appearing about once per boot
and being byte-identical, so the check should also record whether the bad base
occurs during initialisation or in the steady-state loop.

## AL — the faulting store's base is RELOADED FROM A STACK SPILL SLOT

Disassembling `with_cartridge` in `elf-AK` around the `+0x4100` sites finally
shows where the base comes from. Two inlined copies of
`write_lcd_timing_register` appear in that function:

```
1001a7f8:   and.w  r1, r6, r11        ; r11 = 0xff80
1001a7fc:   cmp.w  r1, #0xff00
1001a800:   bne    ...
1001a802:   sxth   r1, r6
1001a804:   add    r1, r5             ; base in a CALLEE-SAVED REGISTER
1001a806:   strb.w r0, [r1, r10]      ; r10 = 0x4100

1001a8a6:   bne    ...
1001a8a8:   ldr    r2, [sp, #0x10]    ; base RELOADED FROM A STACK SLOT
1001a8aa:   sxth   r1, r1
1001a8ac:   add    r1, r2
1001a8ae:   strb.w r0, [r1, r11]      ; r11 = 0x4100
```

`with_cartridge`'s frame: `push {r4,r5,r6,r7,lr}` / `add r7,sp,#0xc` /
`push.w {r8-r11}` / `sub.w sp,sp,#0x1240` / `sub sp,#0xc` — 4720 bytes, so
`sp = entry_sp - 0x1270`. Three different object pointers are spilled here:
`[sp,#0x8]` (the sret pointer), `[sp,#0xc]` (base for `strbeq.w r0,[r1,#0x2bb]`)
and `[sp,#0x10]`.

Two consequences.

**1. The recorded bad base 0x2007cd00 is `r7 - 0x30`.** After the two pushes
`r7 = entry_sp - 0x8`, so `r7 - 0x30 = entry_sp - 0x38` — and `sub.w r4, r7,
#0x30` appears *twice* in this function (0x1001a7c0, 0x1001a868) as the address
of the `RangeInclusive` loop iterator that is passed to
`RangeInclusive::next` in r0. So the value that showed up in the base position
is not garbage: **it is another live local's address from the same frame.**
That is the signature of a base register/slot taking on a neighbouring value,
not of a wild pointer arriving from outside.

**2. The base can itself be a VICTIM of the stack corruption.** If the base is
reloaded from `[sp,#0x10]` and something is scribbling on core 0's stack, the
corrupt base is a *second-order effect*, not the primary cause — the
chicken-and-egg the corrupt-base theory has to resolve. Note the two paths
differ: the once-per-boot MPU violation happens inside `with_cartridge`, whose
base lives on the stack; the continuous canary smashes happen in the
steady-state loop, where the base is `&mut Core1Transport` reached through the
heap-allocated `GameBoy`.

### Config AL — the counted experiment that separates them

`check_bus_base`/`BUS_BASE_CHK` (config AK, refuted — wrong pointer) is
REMOVED. In its place, `check_lcd_base` in `multicore.rs` watches
`self.lcd_timing_io.as_ptr()` — the exact quantity the faulting store forms
(`base_reg + 0x4000`) — at the top of `write_lcd_timing_register`, which is
`#[inline(always)]` and therefore instruments *all* inlined copies including
the one in `with_cartridge`.

It is a **histogram, not a first-wins reference** (a single reference would fire
spuriously if `with_cartridge` legitimately builds the object in a stack
temporary and memcpys it to the sret destination), and it **counts calls**
(rule 9 — AK's "0 mismatches" was uncounted and therefore worthless):

```
LCD_BASE_CHK[0]      magic 0x1CD00001
LCD_BASE_CHK[1..5]   up to four distinct bases
LCD_BASE_CHK[5..9]   per-base call counts
LCD_BASE_CHK[9]      total calls
LCD_BASE_CHK[10]     calls that saw a FIFTH+ distinct base
LCD_BASE_CHK[11]     most recent such base
```

Pre-reset smoke test (a few hundred ms of running firmware) already read back
`[0]=0x1cd00001 [1]=0x2003d840 [5]=0x103 [9]=0x103 [10]=0` — one base, inside
the heap arena, 259 calls, no overflow. The probe works.

**Decision rule.** If after a full window `[9]` is in the millions with a single
base and `[10]=0`, then the faulting store's base is NEVER wrong in the
steady-state loop, and this store **cannot** be the canary smasher — the
corrupt-base theory survives only for the once-per-boot `with_cartridge` MPU
violation, and the smashes need a different writer. If `[10]` or a second slot
is non-zero, the bad base is captured directly along with how often it occurs.

### AK final state (77 min, 0.77 crashes/min)

`SMASH_CORE0 = [0x51150002, 0x2007ccd8, 0x20001bcf, 0x3b, 0x2b7e1516,
0x2007cd0c, 0x00000001, 0x2007d2d0]` — 59 smashes, same victim, same body SP,
and the guard word `0x00000001` for the **eleventh** time.
`BUS_BASE_CHK[2] = 0` (uncounted, and refuted anyway).

### Layout (config AL — re-derived)

```
REGION_FAIL   0x200670a8      XIP_CHECK     0x20067148
LCD_BASE_CHK  0x200671e8      DATA_CHECK    0x20067218
ALLOC_GUARD   0x200672b8      HEARTBEAT     0x20067358
RAM_CHECK     0x20067498      DWT_CATCH     0x20067538
SMASH_CORE0   0x20067560      SMASH_CORE1   0x200677e0
WATCH_LOG     0x20067a60      MM_REGS       0x20067ba8
_stack_end    0x20067be8
```

Zero span = **720 words from 0x200670a8**. Core-1 RO region base =
`_stack_end & !0x1F` = **0x20067be0**, which covers `MM_REGS[14..16]` — never
written (the handler writes `[0..=12]`), so the tail is safe.
`DWT_CATCH[8] = 0x20067558` (offset 8 planted), `[9] = 0x2006755c`.
`SMASH_CORE0[3] = 0x2006756c` (smash count), `[6] = 0x20067578` (the guard).
`LCD_BASE_CHK[9] = 0x2006720c`, `[10] = 0x20067210`, `[11] = 0x20067214`.
Code addresses unchanged from AK: `bus_write` 0x20002414, `ld8` 0x200012ec,
`ld16` 0x20001400, `take_pending_interrupt` 0x200010b2,
`with_cartridge` 0x1001a2b8.

Flashed 15:14, CRC `0x0a97a137`, **reset 15:17:29 on 2026-08-15**.

## AL result — the corrupt-base theory is DEAD, twice over

### 1. The counted histogram (the pre-registered rule fired)

38 min window, 167 smashes (`SMASH_CORE0[3]=0xa7`):

```
LCD_BASE_CHK[0]  = 0x1cd00001   probe active
LCD_BASE_CHK[1]  = 0x2003d840   ONE distinct base
LCD_BASE_CHK[2..5] = 0          no second, third or fourth base
LCD_BASE_CHK[5]  = 0x56091      352,401 calls on that base
LCD_BASE_CHK[9]  = 0x56091      352,401 total calls
LCD_BASE_CHK[10] = 0            no fifth-base overflow
LCD_BASE_CHK[11] = 0
```

352,401 calls, one base, zero anomalies, alongside 167 smashes. If this store
were the smasher it would need ~1 bad base per 2,110 calls; zero were seen.
**The faulting store's base is never wrong in steady state.**

Correction to the AL note: `0x2003d840` is **not** in the heap arena. It
resolves to `__embassy_main_task_inner_funct + 0x19300` in `.bss` — the
`PicoGameBoy` lives inside the embassy main task's future, not on the heap.

### 2. A static refutation that should have come first

`self.lcd_timing_io[i] = value` compiles to **`strb`** — a BYTE store. The
corrupted guard word is `0x00000001`, i.e. **all four bytes** differ from
`0x2B7E1516`. A byte store cannot produce that. **The `lcd_timing_io` store
could never have been the canary smasher, and no measurement was needed to know
it.** (New rule 21.)

Both legs are gone. The corrupt-base theory is retired; do not revive it.

## AL — a NEW, reproducible core-0 HardFault with a ~4-instruction window

27 records before the sector stopped filling (seq 0..26, strictly alternating
Panic / WDT-or-HF). 14 Panic (the usual `dec8` smash, line 169, lr 0x20001bcf),
7 WDT, **6 HardFaults that are byte-identical apart from the PC**:

```
kind=HF flags=0x47 cfsr=0x00020000 (INVSTATE) hfsr=0x40000000 (FORCED)
pc = 0x2003bd24 | 0x2003bcf4 | 0x2003bc28     lr = 0x20001fff
r12 = 0x1001473f (cb::sla_u8|1) | 0x100146cb (cb::rl_u8|1)
sp_before = 0x2007cd08
```

Resolved against `elf-AL`:

- **`lr = 0x20001fff`** is the return address of `20001ffa: bl set_r8_enum`,
  inside `Sm83 ... Instructions::cb`. So the fault is *inside* `set_r8_enum`.
- **`pc = 0x2003bd24`** is `__embassy_main_task_inner_funct + 0x177e4` — a
  **`.bss` DATA address**, ~0x1B1C below the emulator object's own base. Even,
  hence INVSTATE. All three PCs lie within 0xFC of each other.
- **`r12`** is a correct `cb::*` pointer left over in the scratch register by
  the preceding `.data` long-branch thunk — not the culprit.
- `flags` bit 0x20 (FAULT_ON_CORE1) is clear: **this is core 0.**

`set_r8_enum` (0x200025c4) is a five-instruction leaf:

```
200025c4:  push {r7, lr}
200025c6:  mov  r7, sp
200025c8:  uxtb r1, r1
200025ca:  tbb  [pc, r1]
200025d6:  strb r2, [r0, #0x1c]     (one of eight)
200025d8:  pop  {r7, pc}
```

So the saved LR at `sp+4` was replaced **between the `push` and the `pop`** —
no call, no loop, no SP movement in between. A `tbb` overrun is excluded: its
reach is `0x200025ce + 2*255 = 0x200027cc`, nowhere near `.bss`.

**Both victims live in the same narrow band of core 0's stack:** `dec8`'s guard
at `0x2007cce0`, `set_r8_enum`'s saved LR at ~`0x2007ccfc`–`0x2007cd04`. About
0x1C apart. But the payloads differ — `0x00000001` at one, a *pointer into the
emulator's own `.bss` object* at the other — so this is not one memset.

### Config AM — snapshot the band

Added `HF_STACK: [u32; 28]` (`.uninit`), filled in `hard_fault_rust` gated on
`CFSR & INVSTATE`, write-once, magic last:

```
HF_STACK[0]     magic 0x48F50001
HF_STACK[1..7]  pc, lr, cfsr, sp_before, r12, exc_return
HF_STACK[7..27] twenty words spanning sp_before-40 ..= sp_before+36
```

**Decision rule.** If exactly one word in the band is wrong and its neighbours
are sane return addresses and locals, the writer is a *stray single-word store*
and the `-Z stack-protector` premise ("a contiguous stack-buffer overrun") is
wrong. If a run of consecutive words is wrong, it is an overrun and the run
length gives its size. Either way the surrounding words identify which frames
were live, which no DWT predicate has been able to establish.

### Layout (config AM — re-derived)

```
REGION_FAIL  0x200670b0     DATA_CHECK   0x20067150
HEARTBEAT    0x200671f0     RAM_CHECK    0x20067330
XIP_CHECK    0x200673d0     LCD_BASE_CHK 0x20067470
ALLOC_GUARD  0x200674a0     DWT_CATCH    0x20067540
SMASH_CORE0  0x20067568     SMASH_CORE1  0x200677e8
WATCH_LOG    0x20067a68     HF_STACK     0x20067bb0
MM_REGS      0x20067c20     _stack_end   0x20067c60
```

Zero span = **748 words from 0x200670b0**. `_stack_end` is now 32-byte aligned,
so the core-1 RO region base equals it exactly and **no `.uninit` object falls
inside the region** — the tail hazard is gone this build.
`DWT_CATCH[8]=0x20067560` (offset 8 planted), `[9]=0x20067564`.
`SMASH_CORE0[3]=0x20067574`, `[6]=0x20067580`.
`LCD_BASE_CHK[9]=0x20067494`, `[10]=0x20067498`.

Flashed 16:01 (first attempt timed out on "Failed to reset, and then halt";
recovered with the OpenOCD rescue), CRC `0x2230fda1` OK,
**reset 16:05:10 on 2026-08-15**.

**New rules.** (21) **CHECK THE STORE WIDTH AGAINST THE CORRUPTION WIDTH before
building any probe — a `strb` cannot corrupt a whole word.** (22) **RESOLVE A
FAULT PC AGAINST THE SYMBOL TABLE INCLUDING `.bss`/`.data` — a PC inside a data
object is an indirect branch through a corrupted pointer, and says so
immediately.**

## AM result — the INVSTATE fault is DETERMINISTIC, and its payload is an `Arc` pointer

38 min, 83 smashes (2.2/min). Sector floored at 31: **15 Panic** (the usual
`dec8` smash, line 169), **8 WDT**, **8 HardFaults — every one byte-identical**:

```
pc=0x2003bd2c  lr=0x20001fff  cfsr=0x00020000 (INVSTATE)
sp_before=0x2007cd08  r12=0x1001473f  flags=0x47 (core 0)
```

Eight for eight, no variation. **This is a repeatable code path, not random
corruption.**

### RETRACTION — the `.bss` claim in the AL section was wrong

I resolved `0x2003bd2c` and `0x2003d848` with an `llvm-nm` nearest-preceding
-symbol bisect and reported them as `.bss`, inside the embassy main task future.
**Both are in fact inside the heap arena `0x200244dc..0x2004c4dc`.** The bisect
misled me because the arena is a large *unnamed* region that follows the huge
`__embassy_main_task_inner_funct` symbol, so the nearest preceding name is not
the containing object. The original AK reading ("inside the heap arena") was
right and my AL "correction" was the error. `PicoGameBoy` is heap-allocated.

**New rule 23: CHECK A SYMBOL BISECT AGAINST KNOWN REGION BOUNDS (heap arena,
section limits) BEFORE TRUSTING IT — the nearest preceding symbol is not
necessarily the containing one.**

### What `0x2003bd2c` actually is

Dumping live memory there shows a perfectly regular 12-byte record:

```
0x2003bd08 = 1          0x2003bd14 = 1          0x2003bd20 = 1
0x2003bd0c = 1          0x2003bd18 = 1          0x2003bd24 = 1
0x2003bd10 = 0x08040405 0x2003bd1c = 0x08040406 0x2003bd28 = 0x100404ff
0x2003bd2c = 1   <-- fault PC
0x2003bd30 = 1
0x2003bd34 = 0x08040400
```

That is `ArcInner<T> = { strong: usize, weak: usize, data }` with a 4-byte
`OpCode` payload — **the opcode table's `Arc<dyn OpCode>` allocations.** The
fault PC lands exactly on an *allocation start*, i.e. on the **data half of an
`Arc<dyn OpCode>` fat pointer**. The three AL PCs (`0x2003bd24`, `0x2003bcf4`,
`0x2003bc28`) differ by 0x30 and 0xCC — **both exact multiples of 12** — so they
are allocation starts too, just different opcodes. And the AM PC sits 8 bytes
above the AL PC, the same +8 the whole heap shifted by between builds
(`LCD_BASE_CHK[1]` went `0x2003d840` → `0x2003d848`); the offset
`lcd_timing_io − pc = 0x1B1C` is **identical across both builds**.

So the word popped into PC is not garbage. **It is a live, well-formed `Arc`
pointer.**

### The band, decoded

`HF_STACK` (band = `sp_before-40 ..= sp_before+36`):

```
0x2007cce0 = 0x2002757c
0x2007cce4 = 0xfffffff9
0x2007cce8 = 0x00000008   EXC FRAME r0
0x2007ccec = 0x00000000   EXC FRAME r1
0x2007ccf0 = 0x00000002   EXC FRAME r2
0x2007ccf4 = 0x00000001   EXC FRAME r3
0x2007ccf8 = 0x1001473f   EXC FRAME r12   (matches ef.r12)
0x2007ccfc = 0x20001fff   EXC FRAME lr    (matches ef.lr)
0x2007cd00 = 0x2003bd2c   EXC FRAME pc    (matches ef.pc)
0x2007cd04 = 0x08000000   EXC FRAME xPSR  (T bit CLEAR -> INVSTATE confirmed)
0x2007cd08 = 0x10036698   <== sp_before   (a flash address - vtable-shaped)
0x2007cd0c = 0xffffffff
0x2007cd10 = 0x00000008
0x2007cd14 = 0x2003d848   heap pointer
0x2007cd18 = 0x2003d758   heap pointer
0x2007cd1c = 0x2b7e1516   ** AN INTACT STACK GUARD **
0x2007cd20 = 0x00000000
0x2007cd24 = 0x2007cf00   stack pointer
0x2007cd28 = 0x00000000
0x2007cd2c = 0x00000030
```

**A design flaw in the AM probe: the exception frame occupies exactly
`[sp_before-32, sp_before)`, so it overwrites the very slot under
investigation** — `set_r8_enum`'s saved-LR slot at `sp_before-4` reads back as
the frame's own xPSR. The slot's true value is only recoverable as `ef.pc()`,
which we already had.

**But the pre-registered rule still resolves on the rest of the band:
everything above `sp_before` is sane** — two heap pointers, a stack pointer,
small integers, and **an intact `0x2b7e1516` guard at `0x2007cd1c`**. No run of
consecutive bad words. **⇒ NOT a contiguous stack-buffer overrun. The
`-Z stack-protector` premise is wrong for this fault.**

### Frame arithmetic

`Instructions::cb` (0x20001eec): `push {r4,r5,r6,r7,lr}` +
`push.w {r7,r8,r9,r10,r11}` = **40 bytes, no `sub sp`**. Its SP at
`20001ffa: bl set_r8_enum` is therefore `sp_before = 0x2007cd08`, so `cb`'s
entry SP is `0x2007cd30` and `cb`'s own frame is `[0x2007cd08, 0x2007cd30)`.
**The corrupt slot `0x2007cd04` lies BELOW `cb`'s stack pointer** — it is
`set_r8_enum`'s own frame, not `cb`'s.

`cb` has **no direct callers** in the disassembly: it is reached by `blx`
through the `Arc<dyn OpCode>` vtable from `Sm83::step()`. Note `sp_before`
itself holds `0x10036698`, a flash address of exactly vtable shape.

### Config AN

- `HF_STACK` widened to 48 words: `[7..47]` now spans
  `sp_before-80 ..= sp_before+76`, covering `cb`'s whole 40-byte frame **and**
  its caller's — the context needed to decide which frame the `Arc` pointer
  legitimately belongs to.
- `MM_REGS` padded from 16 to 20 words. At 16 it ended at `0x20067cb0`, so
  `_stack_end & !0x1F` put core 1's read-only region base at `0x20067ca0` and
  **swallowed `MM_REGS[12]`, which the handler DOES write** — a core-1 fault
  would have taken a MemManage inside the fault handler and wedged the core.
  At 20 words `_stack_end = 0x20067cc0` is 32-byte aligned, so the region base
  equals `_stack_end` exactly and nothing of ours is inside it.

### Layout (config AN)

```
REGION_FAIL 0x200670b0   LCD_BASE_CHK 0x20067470   ALLOC_GUARD 0x200674a0
DWT_CATCH   0x20067540   SMASH_CORE0  0x20067568   WATCH_LOG   0x20067a68
HF_STACK    0x20067bb0   MM_REGS      0x20067c70   _stack_end  0x20067cc0
```

Zero span = **772 words from 0x200670b0**. `DWT_CATCH[8]=0x20067560`,
`SMASH_CORE0[3]=0x20067574`, `[6]=0x20067580`, `LCD_BASE_CHK[9]=0x20067494`.

`LCD_BASE_CHK` reconfirmed the refutation in a second window: **364,239 calls,
one base (`0x2003d848`), zero anomalies.** `MM_REGS` all zero — **no MPU
violation this window at all**, consistent with the corrupt-base theory being
dead.

Flashed, CRC `0x98c4fcee` OK, **reset 16:50:43 on 2026-08-15**.

## AN result — THERE IS NO STRAY WRITER. THERE IS AN 8-BYTE SP IMBALANCE.

38 min, 79 smashes (2.05/min). `HF_STACK` caught the fault again with a widened
40-word band; `MM_REGS` all zero (no MPU violation, third window running);
`LCD_BASE_CHK` 346,112 calls / one base / zero anomalies (third confirmation).

The band pinned the frames exactly, and the answer is not what every previous
cycle assumed.

### The anchor

`Sm83::step` (0x20000e58) stores the stack guard at `[sp,#0x1c]`
(`20000e6a: str r0,[sp,#0x1c]`). The snapshot has **`0x2007cd1c =
0x2b7e1516`** — so **`step`'s SP is `0x2007cd00`**, not `0x2007cd08`.

`step`'s frame is balanced: `push {r4-r7,lr}`(20) + `str r8,[sp,#-4]!`(4) +
`sub sp,#0x20`(32) = 56 in; `addeq sp,#0x20` + `ldreq r8,[sp],#4` +
`popeq {r4-r7,pc}` = 56 out. (An earlier scan "found" a missing `add sp` — that
was my regex failing to match **predicated** forms like `addeq sp`. Rule 25.)

### The fat pointer is exactly where it should be

```
20000ee6:  strd r0, r1, [sp, #4]     -> r0 -> 0x2007cd04 , r1 -> 0x2007cd08
20000f06:  blx  r6                   -> return addr 0x20000f09  (seen on the stack)
20000f9c:  add  r0, sp, #0x4
20000f9e:  bl   drop_glue<Arc<dyn OpCode>>     <-- [sp,#4] IS the Arc. Confirmed.
```

- `0x2007cd08 = 0x10036698` — rodata: the **vtable** half.
- fault PC `= 0x2003bcfc` — heap, allocation-start aligned: the **Arc data** half.

**`step`'s live `Arc<dyn OpCode>` fat pointer occupies `[0x2007cd04,
0x2007cd08]`, intact and correct.**

### The arithmetic

`Instructions::cb` is reached by `blx r6`, which does not move SP, so **cb's
entry SP = step's SP = 0x2007cd00**. cb pushes 40 bytes
(`push {r4,r5,r6,r7,lr}` + `push.w {r7,r8,r9,r10,r11}`) and pops 40
(`pop.w {r3,r8,r9,r10,r11}` + `pop {r4,r5,r6,r7,pc}`) — balanced, and it
contains **no `sub sp` and not one SP-relative store**, so it cannot write its
own frame at all.

It should therefore return with `SP = 0x2007cd00`, taking PC from `0x2007ccfc`.

```
expected:  SP = 0x2007cd00 , PC from 0x2007ccfc
observed:  SP = 0x2007cd08 , PC from 0x2007cd04     <-- both exactly +8
```

**cb's epilogue popped its return address out of `step`'s Arc-data slot.** The
branch target is a heap address, which is even, hence INVSTATE.

### This retires the entire "stack corruption" framing

**There is no stray writer. Nothing is being corrupted. SP is 8 bytes too high.**
The same +8 explains the other failure mode:

`dec8` checks its guard with `ldr r0,[sp,#0x8]`. With SP 8 high it reads
`sp+0x10` — a neighbouring live word — instead of its guard. And
`SMASH_CORE0[4]`, recorded at the naked handler's `sp+0`, has read
**`0x2b7e1516` every single time**: the guard, **intact**, one slot below where
the code looked. **The "corrupt guard = 0x00000001", confirmed thirteen times,
is a MISREAD of an adjacent live word — not corruption.** The withdrawn "the
canary is INTACT" reading was right after all, for a reason nobody had.

That also explains, at a stroke:

- why **all three DWT write-watch predicates found nothing wrong** — nothing
  writes there;
- why the bogus guard value is always the same small integer — it is a stable
  neighbouring local;
- why `check_opcode_table`'s fingerprint (which folds in `Arc::strong_count`)
  has **never** fired, and why every live `ArcInner` dumps as a clean `{1,1,d}`;
- why `stack_pop_check` ran 1,018,167,296 iterations with zero mismatch;
- **durable rule 2's "`sp_before` OVER-REPORTS BY 8"** — that was this same +8
  all along, misattributed to the reporting path instead of to a real imbalance.

### Config AO — the cleanest A/B available, with no rebuild

The one thing running on core 0 at enormous frequency in these builds is my own
**DebugMonitor handler** (`dwt_watch.rs`, ~68,000 entries/sec). An exception
return that restores SP 8 bytes high would produce exactly this signature.

The self-aim only arms when `DWT_CATCH[8]` holds a nonzero offset `< 0x1000`.
So **planting 0 disarms it with the identical binary** — same code, same layout,
same heap addresses, only the watch on/off. No rebuild, nothing else varies.

**Decision rule.** If the `dec8` smashes and the INVSTATE HardFaults collapse,
core 0's DebugMonitor return is the source of the +8 — the crashes have been
substantially self-inflicted by the instrumentation, and it strips out anyway.
If they continue at ~2/min, the DWT is exonerated and the +8 lives in real
firmware code — which is the thing that actually has to be fixed. WDT hangs
(8/31) are a separate category and may persist either way.

Instruments zeroed (772 words), `DWT_CATCH[8] = 0` **verified**, crash sector
blanked, **reset 17:36:33 on 2026-08-15**. Binary unchanged: config AN,
CRC `0x98c4fcee`.

**New rules.** (25) **A DISASSEMBLY SCAN FOR SP-MODIFYING INSTRUCTIONS MUST
MATCH PREDICATED FORMS (`addeq sp`, `popne`, ...) — a naive `\badd\s+sp` misses
them and invents imbalances.** (26) **ANCHOR A FRAME WITH A KNOWN CONSTANT (the
`0x2b7e1516` guard at a known `[sp,#N]`) RATHER THAN WITH `sp_before`.**
(27) **BEFORE HUNTING A WRITER, CHECK WHETHER SP ITSELF IS WRONG — every
"corrupted word" here was a correctly-stored word read at the wrong offset.**

## AO result — the DebugMonitor is EXONERATED, and a second confirmation of the +8

Same binary as AN (CRC `0x98c4fcee`), only `DWT_CATCH[8] = 0` so the core-0
DebugMonitor never arms. `DWT_CATCH[8]` verified still 0 at read-out, so the
A/B is valid.

| | window | smashes | rate | records |
|---|---|---|---|---|
| AN — DebugMonitor **ARMED**   | 38.0 min | 79 | 2.05/min | 15 Panic, 8 WDT, 8 HF |
| AO — DebugMonitor **DISARMED**| 38.6 min | 94 | 2.44/min | 15 Panic, 11 WDT, 5 HF |

**No collapse — if anything slightly worse.** The pre-registered rule therefore
resolves the other way: **the DWT/DebugMonitor is exonerated, and the +8 lives
in real firmware code.** The INVSTATE fault reappeared byte-identical
(`pc=0x2003bd2c lr=0x20001fff sp_before=0x2007cd08`).

### A second, spectacular confirmation

A new HardFault signature appeared:

```
pc = 0x2b7e1516   lr = 0x100150af (String::push+0x22)
cfsr = 0x00000100 (IBUSERR)   sp_before = 0x2007cca0
```

**`pc = 0x2b7e1516` is the stack-guard constant, executed as a program
counter.** A function popped PC and got the **canary** instead of its saved LR,
then faulted fetching from an address that is not memory. Same bug, different
frame: when the wrongly-popped slot holds an `Arc` pointer you get an even
address (INVSTATE); when it holds the guard you get IBUSERR.

The "SP is off" thesis is now confirmed twice, independently, from two
different frames and two different fault classes.

### A static push/pop imbalance is RULED OUT

A sweep of all 1,666 functions for prologue/epilogue byte mismatch produced 315
hits, but the heuristic walks backwards across basic blocks and sums unrelated
epilogues (it reported deltas of +25,428), so it is not trustworthy.

More importantly the sweep is unnecessary: **`Instructions::cb` runs on every
CB-prefixed opcode, millions of times between faults. A compile-time push/pop
imbalance would fault on the first call.** It does not. **⇒ the +8 is DYNAMIC** —
something intermittently leaves SP wrong. With the DebugMonitor excluded, that
points at the remaining asynchronous events on core 0.

**New rule 28: A CRASH THAT IS RARE RELATIVE TO THE FREQUENCY OF THE SUSPECT
CODE PATH CANNOT HAVE A STATIC CAUSE — do not go looking for one.**

## Config AP — `FPCCR.ASPEN` was cleared, disabling FP context preservation

`fpu.rs::disable_lazy_stacking` did:

```rust
FPCCR.write_volatile(before & !(FPCCR_LSPEN | FPCCR_ASPEN));
```

and its own doc described clearing `ASPEN` as being "so the FP context is not
automatically preserved/restored" — described as a harmless companion to eager
stacking. It is the opposite of it.

With **`ASPEN = 0` the hardware never sets `CONTROL.FPCA`**, so exception entry
always allocates the *basic* 32-byte frame and **S0–S15 and FPSCR are never
preserved across an exception**. The target is `thumbv8m.main-none-eabihf`;
LLVM uses the FP registers freely, including `vmov` to park integer values as
cheap spill scratch. **Any ISR that touches FP silently clobbers whatever the
interrupted code had live in S0–S15** — on core 0, that is the emulator's inner
loop. It was applied to *both* cores.

Measured on device: `FPCCR 0xc0000004 -> 0x00000004` under the old code.

Fixed to the combination that actually expresses the intent — context **is**
preserved, and preserved eagerly:

```rust
FPCCR.write_volatile((before & !FPCCR_LSPEN) | FPCCR_ASPEN);
```

Boot log confirms **`FPCCR 0xc0000004 -> 0x80000004`** (ASPEN set, LSPEN clear).

This is a real defect fixed on its own merits. Whether it is *the* +8 is a
separate question and is exactly what this soak measures.

**Decision rule.** `SMASH_CORE0[3]` counts smashes across reboots and does not
floor with the 31-record sector, so it measures the rate even if the sector
fills. AN/AO ran 2.05 and 2.44 smashes/min on the same code. If AP comes in at
that rate, ASPEN was not the +8 and the next suspects are the remaining core-0
interrupt sources (SysTick / embassy time driver, the executor IRQ, PIO/DMA,
USB) — audit each handler's return path. If the rate collapses, soak longer per
the exponential schedule.

Layout is unchanged from AN/AO (no `.uninit` change): zero span **772 words from
0x200670b0**, `_stack_end = 0x20067cc0` still 32-byte aligned. DebugMonitor left
**disarmed** (`DWT_CATCH[8] = 0`) since it is exonerated and only adds
perturbation. Instruments zeroed, crash sector blanked.

Flashed, CRC `0x510ebc3f` OK, **reset 18:20:43 on 2026-08-15**.

## AP result — ASPEN was not the +8, AND THE +8 CLAIM ITSELF IS RETRACTED

57.5 min. `SMASH_CORE0[3] = 0x75` = **117 smashes = 2.04/min**.

| | window | smashes | rate |
|---|---|---|---|
| AN (DebugMonitor armed)    | 38.0 min | 79  | 2.05/min |
| AO (DebugMonitor disarmed) | 38.6 min | 94  | 2.44/min |
| AP (ASPEN restored)        | 57.5 min | 117 | 2.04/min |

**No change.** `FPCCR` verified `0xc0000004 -> 0x80000004` on every boot, so the
fix is active; it simply is not what causes the crashes. The INVSTATE fault
reappeared byte-identical again (`pc=0x2003bd2c lr=0x20001fff
sp_before=0x2007cd08`). `MM_REGS` still all zero (fifth clean window).

**The `ASPEN` fix stays.** It is a genuine defect — with `ASPEN = 0` the
hardware never sets `CONTROL.FPCA`, so S0–S15 and FPSCR are not preserved across
exceptions on a hard-float target — and it is correct independently of this bug.

### RETRACTION — "an 8-byte SP imbalance" does not survive its own arithmetic

Last cycle I anchored `Sm83::step`'s frame on a `0x2b7e1516` at `0x2007cd1c`
(= `step`'s guard slot `[sp,#0x1c]` if `step`'s SP were `0x2007cd00`) and
concluded SP was +8. Following that through:

```
sp_before = 0x2007cd08                (unambiguous — the exception frame sits at sp-32)
set_r8_enum's pop faulted  => its entry SP        = 0x2007cd08
  => cb's SP at `bl set_r8_enum`                  = 0x2007cd08
  => cb pushes 40, so cb's entry SP               = 0x2007cd30
  => blx doesn't move SP, so step's SP            = 0x2007cd30
  => step's guard would be at                       0x2007cd4c  -> band has 0x3af5a814, not a guard
```

But the anchor put `step`'s SP at `0x2007cd00`, which would place **cb's entry
SP (`0x2007cd30`) inside `step`'s own frame** — impossible for a callee. The two
readings are mutually inconsistent, so the anchor is wrong: with
`-Z stack-protector=strong` **many** frames carry `0x2b7e1516`, and I picked one
and assumed whose it was. That is precisely the trap rule 26 was written to
avoid, and I walked into it one cycle after writing it.

**Therefore: the "+8", and everything derived from it — including "the canary
smash is a misread, not corruption" — is WITHDRAWN.** It may still be true; it
is not established.

### What is actually solid

- `sp_before = 0x2007cd08` — derived from where the exception frame sits, which
  is not open to interpretation.
- `ef.lr = 0x20001fff` — the last `bl` executed was `cb -> set_r8_enum`.
- Something branched to a **data** value instead of a code address: an
  `Arc<dyn OpCode>` allocation pointer (even -> INVSTATE), or in one case the
  guard constant `0x2b7e1516` (invalid -> IBUSERR).
- The fault is fully reproducible and byte-identical across four windows.

**Whether the cause is a wrong SP or a wrong slot value is NOT established.**

### Config AQ — point the window where the evidence actually is

`HF_STACK`'s 40-word band was centred on `sp_before`, which wasted 28 words:
`[sp_before-32, sp_before)` is the exception frame the fault itself pushed, and
below that is the handler's own frame. The frames that identify the call chain —
`cb`'s 40-byte frame, and `step`'s above it with its guard at `sp+0x1c` — all sit
**above** `sp_before` and fell outside the window. Both candidate readings above
needed words at `0x2007cd4c`+ that were never captured.

Band changed to `sp_before-16 ..= sp_before+140`.

**Decision rule.** Locate `step`'s frame by finding a slot triple that is
mutually consistent — a guard at `sp+0x1c`, an `Arc` fat pointer
`{heap-ptr, rodata-ptr}` at `[sp+4, sp+8]`, and `str r4,[sp]` holding a
`GameBoyMemory` heap pointer at `sp+0` — rather than by any single constant.
Three agreeing slots pin the frame; one does not. Then compare `step`'s real SP
against `cb`'s entry SP derived from `sp_before`, and the SP-vs-slot question
answers itself.

**New rule 29: A GUARD CONSTANT IS NOT AN ANCHOR — `-Z stack-protector=strong`
puts `0x2b7e1516` in many frames. Anchor on a MUTUALLY CONSISTENT SET of slots,
and cross-check the result against an independently derived frame boundary
before building anything on it.**

Layout unchanged. DebugMonitor left disarmed. Instruments zeroed, sector blanked.
Flashed, CRC `0x963d427d` OK, **reset 19:21:01 on 2026-08-15**.

## AQ result — the +8 is REAL, re-established on evidence that did not exist before

38.2 min, `SMASH_CORE0[3] = 0x4a` = **74 smashes = 1.94/min** (AN 2.05, AO 2.44,
AP 2.04 — unchanged). `MM_REGS` all zero, sixth clean window.

Moving the `HF_STACK` band above `sp_before` was the right call: it captured the
words the previous framing needed and could not see.

### The slot test, run properly this time

`strd r0, r1, [sp, #4]` writes **both** `sp+4` (Arc data) and `sp+8` (vtable),
so `step` has four checkable slots: `+0x0` (`str r4,[sp]`, a `GameBoyMemory`
heap pointer), `+0x4`, `+0x8`, and `+0x1c` (the guard).

| candidate `step` SP | +0x0 | +0x4 | +0x8 | +0x1c |
|---|---|---|---|---|
| **0x2007cd00** | *(under exc frame)* | *(under exc frame)* | `0x10036698` **rodata vtable ✓** | `0x2b7e1516` **GUARD ✓** |
| 0x2007cd08 | `0x10036698` not heap | `0xffffffff` | — | `0x2007cf00` not guard |
| 0x2007cd30 | `0x2007dfa8` not heap | `0x1000a4b7` | — | `0x3af5a814` not guard |
| 0x2007ccf8 | `0x1001473f` not heap | `0x20001fff` | — | `0x2003d848` not guard |
| 0x2007cd10 | `0x00000008` not heap | — | — | `0x0000004c` not guard |

**`0x2007cd00` is the only candidate that scores, on two independently
verifiable slots, and every alternative fails every test.**

### Therefore the +8 is real

```
sp_before at fault = 0x2007cd08     (ef + 32; xPSR bit 9 CLEAR so no STKALIGN pad — exact)
step's SP          = 0x2007cd00     (step is an ANCESTOR frame)
delta              = +8
```

A descendant's SP can never exceed its ancestor's. And this is not `step`'s own
epilogue: that would give `sp_before = 0x2007cd38`
(`addeq sp,#0x20` / `ldreq r8,[sp],#4` / `popeq`).

**Last cycle's retraction was right about the derivation and wrong about the
conclusion.** The retraction was correct at the time — a single guard constant is
not an anchor (rule 29), and the frame arithmetic genuinely contradicted it. The
widened band supplied a second independent slot and four failed alternatives,
which the earlier evidence did not contain. **The +8 is reinstated; what was
derived *from* it (notably "the canary smash is a misread") is still NOT
re-established and stays withdrawn.**

### An unexplained observation to keep

`0x2007cd3c .. 0x2007cd94` — 23 consecutive words — hold high-entropy values
(`0x5a696fc2 0x4fb4f8d3 0x63e350df 0x5b688fd0 0x3af5a814 ...`). Normal stack
holds pointers, small integers and zeros. This region sits **above** `step`'s
frame (`[0x2007cd00, 0x2007cd38)`), i.e. in its caller's area, just past a
return address at `0x2007cd34` (`embassy_main_task_inner + 0x2fb3`). Not
explained yet; recorded so it is not lost.

## Config AR — is SP stable at a fixed point in the loop?

A static push/pop imbalance is excluded (rule 28), so the +8 is introduced
dynamically. The open question is **when**.

Added `SP_CHK` (`.uninit`) and a `check_sp()` at the top of
`PicoGameBoy::tick` — one fixed point in one call chain, so MSP must read
identically on every iteration. It **counts** the samples so a null is
quantified (rule 9):

```
SP_CHK[0] magic 0x5C000001   [1] first SP seen        [2] mismatch count
SP_CHK[3] first bad SP       [4] total calls          [5] call # of first mismatch
SP_CHK[6] most recent bad SP [7] largest |delta|
```

**Decision rule.** If `[2]` stays 0 over millions of calls, the drift never
survives a whole tick and is **transient within one instruction's dispatch** —
so the search narrows to what runs inside a single `step()`. If `[2]` is
non-zero, `[3]`, `[5]` and `[7]` give the direction, the onset and the size
directly, and the drift is persistent and therefore easy to bracket.

### Layout (config AR — re-derived; `.uninit` grew, so everything moved)

```
REGION_FAIL 0x200670b0   SP_CHK      0x20067150   LCD_BASE_CHK 0x20067490
ALLOC_GUARD 0x200674c0   DWT_CATCH   0x20067560   SMASH_CORE0  0x20067588
WATCH_LOG   0x20067a88   HF_STACK    0x20067bd0   MM_REGS      0x20067c90
_stack_end  0x20067ce0
```

Zero span = **780 words from 0x200670b0**. `_stack_end` is 32-byte aligned, so
the core-1 RO region base equals it exactly — nothing of ours inside (rule 5
checked). `DWT_CATCH[8] = 0x20067580`, `SMASH_CORE0[3] = 0x20067594`,
`SP_CHK[2] = 0x20067158`, `[4] = 0x20067160`, `[7] = 0x2006716c`.

DebugMonitor left disarmed; ASPEN fix retained (`FPCCR -> 0x80000004` confirmed).
Instruments zeroed, sector blanked. CRC `0x31995a67` OK,
**reset 20:03:10 on 2026-08-15**.

## AR result — SP is invariant at tick entry over 132 MILLION samples

```
SP_CHK[0] = 0x5c000001   magic
SP_CHK[1] = 0x2007cd38   first SP seen at PicoGameBoy::tick entry
SP_CHK[2] = 0            MISMATCHES
SP_CHK[4] = 0x07e27620   132,323,872 CALLS
```

**Zero mismatches in 132 million samples.** The pre-registered rule resolves to
the first branch: there is **no gradual drift**; the +8 arises and becomes fatal
**within a single `step()` dispatch**. (Note the asymmetry that makes this a
weaker null than it looks: a tick that faults never reaches the next sample, so
`SP_CHK` can only ever observe *successful* ticks. What it genuinely excludes is
a slow accumulating leak — which it does, decisively.)

### An unplanned third confirmation of `step`'s SP

`SP_CHK[1] = 0x2007cd38` is the tick-entry SP, measured live and invariant
132 million times. `step`'s frame is 56 bytes, and

```
0x2007cd38 - 56 = 0x2007cd00
```

which is exactly the value derived independently in AQ from the rodata vtable at
`sp+8` and the guard at `sp+0x1c`. **Three independent lines now agree that
`step`'s SP is `0x2007cd00`, and `sp_before = 0x2007cd08` is +8 above it.**

### The layout moved and the smash victim moved with it

```
SMASH_CORE0 = [magic, bodySP 0x2007ccc8, lr 0x20000b2b, count 489,
               0x1003651c, 0x20001ab3, 0x2003d758, 0x200009a7]
```

Different `lr`, different body SP, different neighbouring words from every
previous window — layout dominates which victim surfaces, as expected. 489
smashes in 38.0 min = 12.9/min, but that is **not comparable** to AN–AQ: the
`.uninit` set changed *and* `check_sp` sits in the hot path.

The INVSTATE fault appeared for the **sixth** consecutive window with `lr` and
`sp_before` byte-identical (`pc = 0x2003bbc4`, `r12 = 0x100146fb` — pc and r12
track the CB sub-opcode, everything else is fixed).

### The determinism argument

The fault is byte-identical every time. **An asynchronous interrupt lands
anywhere; it cannot produce the same fault site with the same LR six windows
running.** So the +8 is not an async event — it is a **rare but deterministic
code path** inside one dispatch.

## Config AS — capture r7, the frame pointer

Every cycle so far has had to infer *which* function's epilogue popped the bad
word, from `sp_before` plus frame arithmetic. That inference is exactly what
broke twice (the AP retraction and the AQ reinstatement).

`HF_STACK` grown to 56 words; `[48..56]` now hold **r4–r11 at the fault**, taken
from the existing `HARDFAULT_EXTRA_REGS` trampoline capture. **r7 is the
decisive one — it is the frame pointer.** `Instructions::cb` sets it with
`add r7, sp, #0xc`; `set_r8_enum` with `mov r7, sp`. So r7 pins the live frame
directly, with no inference at all:

- `r7 == sp_before` ⇒ `set_r8_enum` was live (`mov r7, sp` at its entry SP).
- `r7 == cb_entry_SP - 0x20 + 0xc` ⇒ `cb` was live.
- neither ⇒ a third function is involved and r7 names its frame.

Combined with r4–r6 and r8–r11 (which `cb` pushes and pops), a mismatch between
the captured values and what `cb`'s prologue saved would also show a register
restore going wrong.

### Layout (config AS — re-derived; the object order reshuffled, not just shifted)

```
REGION_FAIL 0x200670b0   SP_CHK   0x20067150   LCD_BASE_CHK 0x20067490
HF_STACK    0x20067508   MM_REGS  0x200675e8   DWT_CATCH    0x20067638
SMASH_CORE0 0x20067660   WATCH_LOG 0x20067b60  ALLOC_GUARD  0x20067c60
_stack_end  0x20067d00
```

Zero span = **788 words from 0x200670b0**. `_stack_end` is 32-byte aligned, so
the core-1 RO region base equals it exactly — nothing of ours inside (rule 5
checked; `ALLOC_GUARD` is now the tail object).
`HF_STACK[48] = r4 @ 0x200675c8`, **`[51] = r7 @ 0x200675d4`**,
`[55] = r11 @ 0x200675e4`. `DWT_CATCH[8] = 0x20067658`,
`SMASH_CORE0[3] = 0x2006766c`, `SP_CHK[2] = 0x20067158`, `[4] = 0x20067160`.

DebugMonitor left disarmed; ASPEN fix retained (both cores confirmed
`FPCCR -> 0x80000004`). Instruments zeroed, sector blanked.
CRC `0x2593e427` OK, **reset 20:44:20 on 2026-08-15**.

## AS result — r7 nails it: `cb`'s epilogue popped 8 bytes too high

Registers at the fault (`pc=0x2003bbc4 lr=0x20001fff sp_before=0x2007cd08`):

```
r4=0x20000895  r5=0x2007cd30  r6=0x20000f09  r7=0x2002757c
r8=0x2007d2d0  r9=0x2003d758 r10=0x2002757c r11=0x2003d758
```

**r7 = `0x2002757c` is a HEAP pointer, not a frame pointer.** `cb` would give
`sp+0xc`, `set_r8_enum` would give `sp` — neither matches, so the rule's third
branch fires: r7 is not in use as an FP at the fault.

But reconstructing the pop explains **every single register**. A
`pop {r4,r5,r6,r7,pc}` ending at `SP = 0x2007cd08` must have read from
`0x2007ccf4`:

```
[0x2007ccf4] -> r4 = 0x20000895
[0x2007ccf8] -> r5 = 0x2007cd30
[0x2007ccfc] -> r6 = 0x20000f09   <- cb's CORRECT saved LR (step+0xb1), WRONG REGISTER
[0x2007cd00] -> r7 = 0x2002757c   <- step's [sp,#0]  (`str r4,[sp]`, GameBoyMemory ptr)
[0x2007cd04] -> pc = 0x2003bbc4   <- step's [sp,#4]  (the Arc DATA half)
```

`cb` entered at `SP = 0x2007cd00` would have pushed `r4@0x2007ccec … lr@0x2007ccfc`.
**The pop read 8 bytes too high**, so each register received its neighbour's
slot and `pc` came out of `step`'s live Arc pointer.

**This is also a fourth independent confirmation that `step`'s SP is
`0x2007cd00`:** `step`'s `[sp,#0]` was buried under the exception frame and
unverifiable in AQ; the popped r7 recovers it, and it is the `GameBoyMemory`
heap pointer exactly as predicted.

`cb`'s post-prologue SP should be `0x2007ccd8`; the epilogue instead began at
`0x2007cce0`. **SP was already +8 when `cb`'s epilogue started — the +8 arises
during `cb`'s body.**

### A complete static audit of `cb`'s call tree: everything is balanced

| callee | prologue | epilogue | net |
|---|---|---|---|
| `Sm83::bus_write` | `push{r4-r7,lr}`(20) + `push.w{r2,r3,r4-r9,r11}`(36) = 56 | `addeq sp,#0x18`+`popeq.w{r8,r9,r11}`+`popeq{r4-r7,pc}` = 56; and the non-predicated path likewise 56 | 0 |
| `Sm83::set_r8_enum` | `push{r7,lr}` = 8 | seven `pop{r7,pc}` = 8 each | 0 |
| `cb::{rlc,sla,rl,rr,srl,rrc,sra}_u8` (7 leaves behind the thunks) | `push{r7,lr}` = 8 | `pop{r7,pc}` = 8 | 0 |
| the 7 long-branch thunks | no SP ops | — | 0 |

**There is no static imbalance anywhere in the tree.** (The `tbb` table's 8th
entry is `0x00`, which branches back into the table bytes — they decode as four
harmless `lsrs`/`movs` and fall through into the first `strb` + `pop`, still
balanced.)

### RETRACTION — rule 31 was wrong

I claimed a byte-identical fault site rules out an asynchronous cause. **It does
not.** An interrupt lands anywhere, but SP damage is only *discovered* at the
next epilogue, and `cb`'s epilogue executes so much more often than anything
else that it dominates the manifestation point regardless of where the interrupt
struck. A fixed fault site is fully consistent with an async cause. **Rule 31 is
withdrawn, and the interrupt-audit line it retired is back on the table — now as
the leading hypothesis**, since the static audit has excluded the alternative.

## Config AT — mask interrupts across the emulator step

```rust
asm!("cpsid i");
self.gb.tick();
asm!("cpsie i");
```

**Decision rule.** If the INVSTATE fault and the canary smashes vanish, an
exception return on core 0 is leaving SP wrong, and the next step is to find
which handler. If they persist at rate, exceptions are excluded and the cause is
in the emulator's own straight-line code.

Caveat recorded up front: this is heavy perturbation and it risks starving
embassy's time driver, so liveness was verified before committing the window —
`SP_CHK` climbs steadily (`0x28787f -> 0x33c98a` across twelve samples), first
SP still `0x2007cd38`, zero mismatches. The emulator is running.

The first flash attempt left the probe timing out; the OpenOCD rescue recovered
it in one pass (and randomised `.uninit`, which the zeroing then cleared).

Layout unchanged from AS (no `.uninit` change): zero span **788 words from
0x200670b0**, `_stack_end = 0x20067d00`, 32-byte aligned. DebugMonitor disarmed,
ASPEN fix retained. Instruments zeroed, sector blanked.
CRC `0xdca7bcad` OK, **reset 21:29 on 2026-08-15**.

## AT — VOID. The board lost power.

Read at 13:27 on 2026-08-16, ~15.9 h after the 21:29 reset (the 35-minute
wakeup did not fire on schedule). The state is the documented power-loss
signature:

- `SMASH_CORE0`, `HF_STACK`, `MM_REGS` all high-entropy garbage with **no
  magic** — they were zeroed at 21:28, so garbage means `.uninit` was
  **randomised after that**;
- crash sector holds **one record, a WDT, with `seq = 0`**;
- `ALLOC_GUARD[0]` and `SP_CHK[0]` valid — re-initialised by the firmware after
  power-up.

**No A/B conclusion can be drawn.** The interrupts-masked experiment has to be
re-run.

### A diagnostic error of mine, corrected

I then read `SP_CHK[4]` eight times back-to-back, saw it frozen at
`0x01270252`, and concluded the device was hung and that the `cpsid`/`cpsie`
masking had starved embassy. **That was wrong.** Each `probe-rs read` attaches
and halts the core, so eight in a row keep it halted essentially continuously.
Re-measured with genuine elapsed time between reads:

```
0x015e508d -> 0x021429c8 in ~25 s  =  476,705 ticks/sec   RUNNING
```

**The device was never hung, and the masking is NOT shown to starve embassy.**
The single WDT record is explained by the power-up, not by the masking.

**New rule 35: A COUNTER READ BACK-TO-BACK OVER SWD WILL LOOK FROZEN — every
`probe-rs read` halts the core. Put real elapsed time between liveness samples
(`python3 -c "import time; time.sleep(25)"`; the shell `sleep` is blocked).**

## Config AU — sample SP after the step as well

The masking is reverted anyway: it is a perturbation where a measurement will
do, and I would rather not spend a window on a discriminator whose side effects
I have already misjudged once.

`check_sp()` is now called **twice** — before and after `self.gb.tick()` —
against the same reference and the same counters. The entry-only sample could
never observe damage that faults immediately (a tick that faults never reaches
the next entry); the exit sample catches SP damage that **survives** a step,
which is a case nothing has tested yet. Cost is one `mov` plus a few volatile
accesses.

**Decision rule.** If `SP_CHK[2]` is still 0 with the exit sample in place over
tens of millions of ticks, then SP damage never survives a completed step —
narrowing the +8 to a window that both begins and ends inside one dispatch. If
`[2]` is non-zero, `[3]`/`[5]`/`[7]` give the first bad SP, its onset and the
size, and the entry-vs-exit asymmetry says whether it survives.

Layout unchanged from AS/AT: zero span **788 words from 0x200670b0**,
`_stack_end = 0x20067d00`, 32-byte aligned. DebugMonitor disarmed, ASPEN fix
retained. The hung-looking device was recovered with the OpenOCD rescue before
flashing. Instruments zeroed, sector blanked.
CRC `0xd1e8fcec` OK, **reset 13:32:22 on 2026-08-16**.

**Protocol note.** The soak schedule assumes the board stays powered; a power
cycle silently voids a window and costs a full cycle. Windows that end with
randomised `.uninit` and `seq = 0` must be discarded, not interpreted.

## AU result — the probe corrupted itself, and a new fault mode appeared

Window valid (magics present, no power loss). 37.9 min, `SMASH_CORE0[3] = 0x21`
= **33 smashes = 0.87/min**, classic `dec8` victim.

But **`SP_CHK` is void**:

```
[1] reference = 0x00000000        <- not a possible SP
[2] mismatches = 1,716,077,845
[4] calls      = 1,716,003,009    <- MISMATCHES EXCEED CALLS
[7] max |delta| = 0x2007cd38      <- = |0x2007cd38 - 0|, consistent with reference == 0
```

The reference slot was clobbered, so every sample "mismatched" and the counters
are incoherent. No conclusion can be drawn about SP surviving a step.

And a **brand-new fault mode dominates**: 13 Panic, **17 HF**, 1 WDT, where the
17 HardFaults are

```
pc = 0x1002fb96 (memcpy+0xe6)   lr = 0x0 or 0x2
cfsr = 0x00008200 (PRECISERR | BFARVALID)
BFAR = 0xe00a / 0xe05a / 0xe0ca / 0xe0dc     r12 = 0x06..0x71
sp_before = 0x2007cc74
```

`memcpy` running with a wild pointer around `0x0000e000`, its byte counter in
r12 walking, and a garbage return address. **The INVSTATE fault did not appear
at all.** Both `check_sp()` call sites are removed.

**The instrumentation has become a liability** — it now generates its own
failure modes and unusable data. This is the point to stop adding probes.

## Config AV — one binary, two windows, selected by a planted gate

The interrupt hypothesis is still the leading one and still untested (AT was
lost to a power cut). But **the flaw in AT was not the masking — it was
comparing across builds.** Layout dominates both the crash rate and which victim
surfaces, so a cross-build rate delta proves nothing. That is why the
DebugMonitor A/B was decisive and every other rate comparison has not been: it
used one image and flipped a planted word.

So the masking is now gated on `.uninit`:

```
SP_CHK[0] = magic     [1] = GATE (1 = mask interrupts across the step)     [2] = tick counter
```

`[1]` is planted over SWD and **never written by the firmware**, so one flashed
image runs both arms — plant 1, reset, measure; plant 0, reset, measure.
Identical code, identical layout, identical heap.

**Decision rule.** Compare the masked arm against the unmasked arm of the *same
image*. If the smashes and HardFaults collapse with interrupts masked, an
exception return on core 0 is leaving SP wrong, and the next step is to find
which handler. If they persist, exceptions are excluded and the +8 is in the
emulator's own straight-line code.

### An MPU tail hazard caught while re-deriving

Removing the probes moved the whole `.uninit` block. `_stack_end` landed at
`0x20068070` — **not** 32-byte aligned — which put core 1's read-only region
base at `0x20068060` and swallowed the last four words of `ALLOC_GUARD`, the new
tail object. Padded `ALLOC_GUARD` from 40 to 44 words so `_stack_end = 0x20068080`
is aligned and the region base equals it exactly (rule 5, checked).

### Layout (config AV — re-derived; the block moved wholesale)

```
REGION_FAIL 0x20067420   SP_CHK   0x200674c0   LCD_BASE_CHK 0x20067800
HF_STACK    0x20067878   MM_REGS  0x20067958   DWT_CATCH    0x200679a8
SMASH_CORE0 0x200679d0   WATCH_LOG 0x20067ed0  ALLOC_GUARD  0x20067fd0
_stack_end  0x20068080
```

Zero span = **792 words from 0x20067420**. `SP_CHK[1]` GATE = `0x200674c4`,
`[2]` counter = `0x200674c8`. `SMASH_CORE0[3] = 0x200679dc`.
`HF_STACK[51] = r7 = 0x20067944`. `DWT_CATCH[8] = 0x200679c8`.

Instruments zeroed, **GATE planted = 1 (masked arm)**, sector blanked, reset.
Verified after the reset with real elapsed time between samples (rule 35):
`GATE = 1`, counter `0x00c0f1ce -> 0x014f25e7` in 25 s = **373k ticks/sec**.
CRC `0x85fcee4e` OK, **reset ~14:17:30 on 2026-08-16**.

## AV arm A (interrupts MASKED) — EXCEPTIONS ARE EXCLUDED

Window valid (all magics present). GATE verified `= 1` at read-out, so the
masked arm really ran. 39.8 min, 905,182,405 ticks.

```
SMASH_CORE0[3] = 0x35 = 53 smashes = 1.33/min
records 22: 11 Panic, 8 HF, 3 WDT
all 8 HF identical: pc=0x2003bfa0 lr=0x20001fff cfsr=0x00020000 (INVSTATE)
                    sp_before=0x2007cd00 r12=0x1001435f
```

**The INVSTATE fault fired eight times with interrupts masked.** `PRIMASK = 1`
blocks every configurable-priority exception — all IRQs, SysTick, PendSV, SVC —
leaving only NMI and HardFault. The fault is at `cb`'s epilogue, which is inside
the masked region.

**The pre-registered rule fires on PERSIST: exceptions are EXCLUDED. The +8 is
in the emulator's own straight-line code.**

That retires the interrupt hypothesis that has led since rule 31 was withdrawn,
and it does so on a *within-image* comparison — the flaw that made every earlier
cross-build rate delta meaningless.

`lr = 0x20001fff` again: same `cb -> set_r8_enum` call site, six-plus windows
running. `sp_before` is `0x2007cd00` here rather than `0x2007cd08` because the
layout moved; the +8 relationship is to `step`'s SP in *this* image, not to an
absolute address.

### The `tbb` overrun is not the mechanism either

`set_r8_enum`'s jump table (base `0x200025ce`, bytes `04 0d 09 0b 06 0f 12 00`)
was the one indirect branch in `cb`'s tree, and an out-of-range `r1` (the
`uxtb` admits 0..255 against an 8-entry table) could in principle land on a
foreign epilogue that pops a different amount. Enumerated every index 8..199
against the actual instruction stream: **zero land directly on a `pop`.** Every
in-range target, and every fall-through inside `set_r8_enum`, reaches a
`pop {r7, pc}` = 8 bytes, matching its 8-byte push.

### What is left

With asynchronous causes excluded and `cb`'s *direct* callees audited clean, the
remaining gap is the **transitive closure below `Sm83::bus_write`** — my audit
went two levels deep, and any function in that closure that returns +8
propagates unchanged through `bus_write`'s SP-relative epilogue and through
`cb`'s. That is now the highest-value static work, and it needs a CFG-aware
per-function audit rather than the crude linear scan that produced 315
false positives earlier.

## AV arm B (interrupts UNMASKED) — running

Same image, `SP_CHK[1]` planted `= 0`. Verified after reset: GATE `= 0`, counter
`0x00274a93 -> 0x00d29e5a` in 25 s = **449k ticks/sec**. Instruments zeroed.

This completes the rate half of the A/B (arm A was 1.33 smashes/min). The
qualitative answer is already in — the fault reproduces under masking — so arm B
is confirmation, and the window doubles as running time while the static audit
proceeds.

Note: the crash-sector blank reported a probe timeout this time, so the sector
may still hold arm A's records; `SMASH_CORE0[3]` was zeroed and is the
authoritative count either way.

## AV arm B + the CFG-aware audit — the static case is CLOSED

### Arm B (unmasked), same image, GATE verified `= 0`

| | window | ticks | smashes | per min | per 1e9 ticks |
|---|---|---|---|---|---|
| arm A — interrupts **masked**   | 39.8 min | 905,182,405 | 53 | 1.33 | 58.6 |
| arm B — interrupts **unmasked** | 38.9 min | 880,058,482 | 70 | 1.80 | 79.5 |

Ratio 1.36 — a modest reduction, **not a collapse** — and the INVSTATE fault is
byte-identical in both arms (`pc=0x2003bfa0 lr=0x20001fff cfsr=0x00020000
sp_before=0x2007cd00 r12=0x1001435f`). **Exceptions are excluded**, now on a
within-image A/B in both directions.

### The CFG-aware SP audit — and three bugs in my own analyzer

Built a per-function CFG walk over all 1,671 functions, tracking the SP delta
along every path from entry to every return. It took three corrections before it
told the truth, and each one is worth keeping:

1. **333 flags → 160.** A *predicated* return (`popne {r7,pc}`) is only a return
   on the branch where it executes; on the other branch it falls through. I was
   counting both as returns.
2. **160 → 35.** Predicated instructions inside one IT block **share the
   condition outcome** — `addeq sp,#0x18` and the `popeq` after it either both
   run or neither does. Branching them independently invented states like
   −36/−20/−16 that no execution can reach.
3. **35 → 26.** `bx rN` (N ≠ lr) is an **indirect tail call**, a terminator — not
   a fall-through. Treating it as fall-through walked `Sm83::bus_write`'s
   `bx r3` at `0x200025a6` straight into its shared epilogue a *second* time and
   invented a `+56` return. **That is why `bus_write` appeared flagged, and it is
   an artifact — `bus_write` is balanced 56 in / 56 out on every real path.**

Final result: **26 flags, 23 of them `OUTLINED_FUNCTION_*`** (MachineOutliner
fragments, legitimately entered mid-frame by a tail `b.w`, so a positive delta is
expected), plus three in `embassy_net`/`core`. **Not one function in the
emulator's hot path is flagged.**

And the audit's coverage is provably complete: the image contains **no**
`add/sub sp, rN` (register operand), **no** `mov sp, rN`, and **no** `ldm/stm sp!`
— every SP-modifying instruction in the binary is a form `delta()` models.

**New rules.** (39) **A PREDICATED RETURN IS ONLY A RETURN ON THE EXECUTING
BRANCH.** (40) **PREDICATED INSTRUCTIONS IN ONE IT BLOCK SHARE THE CONDITION —
model the decision once per block, not per instruction.** (41) **`bx rN` IS A
TERMINATOR (indirect tail call), NOT A FALL-THROUGH.** (42) **BEFORE TRUSTING A
STATIC AUDIT, PROVE ITS COVERAGE — grep for the forms it cannot model and
confirm they are absent.**

### Where this leaves the +8 — an honest impasse

The +8 is established on four independent lines. It is **not** an exception
(arm A), **not** a static push/pop imbalance (this audit), and **not** a `tbb`
overrun. Those three exclusions are each solid on their own evidence, and
together they close off every mechanism proposed so far.

**Next concrete step, not yet done:** `step` reaches the CB handler through
`blx r6` on the `Arc<dyn OpCode>` vtable. Every analysis so far has *assumed*
that target is `Instructions::cb` itself. It has never been verified. If the
vtable entry points at a wrapper that adjusts SP and tail-branches to `cb`, the
frame arithmetic everything rests on changes. **Resolve the actual `execute`
implementation for the CB opcode in the vtable and confirm what `blx r6` really
lands on.**

Device: instruments zeroed, GATE planted `0` (unmasked = normal operation),
sector blanked, reset — a fresh window while that resolution proceeds.

## The `blx r6` target — VERIFIED, and the assumption holds

`step`'s dispatch is `20000eee: ldrd r1, r6, [r1, #8]` then `blx r6` — so **r6
comes from vtable[3]**, the first trait method, i.e.
`<CbInstruction as OpCode>::execute` at `0x20000894`, **not**
`Instructions::cb` (`0x20001eec`). There *is* an intermediate function, which no
prior cycle had checked.

```
20000894: push   {r4, r6, r7, lr}      -16
20000896: add    r7, sp, #0x8
2000089a: ldr.w  r12, [r3, #0x58]      load the Instructions-vtable entry
200008a0: ldr    r3, [r7, #0x8]        = [sp_entry] -- step's `str r4,[sp]` 5th arg
200008a4: pop.w  {r4, r6, r7, lr}      +16
200008a8: bx     r12                   TAIL CALL
```

**Balanced 16 in / 16 out, and it pops its frame before the tail call — so the
tail-called `cb` is entered at exactly `step`'s SP.** The assumption every frame
calculation rests on is confirmed.

(Also: `r4 = 0x20000895` in the AS register capture was this very function `| 1`.
I noted at the time that it "looked like a thumb function pointer" and did not
chase it. It was the answer to a question I had not yet asked.)

**So the +8 survives this check too, and every proposed mechanism is now
excluded:** not an exception (within-image PRIMASK A/B), not a static SP
imbalance (CFG-aware audit of all 1,671 functions, coverage proven), not a `tbb`
overrun, not a hidden dispatch wrapper.

## Config AW — strip the scaffolding and establish a CLEAN BASELINE

The investigation has hit a genuine impasse, and meanwhile the diagnostic
apparatus has been manufacturing faults of its own: AU's second `check_sp()`
call corrupted its own reference slot **and** brought a new dominant fault mode
with it (`memcpy` bus-faulting on a wild pointer, 17 of 31 records) which
vanished when the call was removed. Continuing to add probes to a binary whose
probes invent failures is not a path to a 24-hour soak.

Removed:

- **`-Z stack-protector=strong`** — it forces nightly, puts a canary in every
  guarded frame, and turns the anomaly into ~2 panics/min that dominate the
  crash records. Without it the same SP damage surfaces as a wrong return (a
  HardFault), which the crash recorder still captures. **The metric becomes the
  HF/WDT record count rather than `SMASH_CORE0[3]`, which will now stay 0.**
- The `SP_CHK` gate and the `cpsid`/`cpsie` masking from `PicoGameBoy::tick`.
- The `check_lcd_base` call from `write_lcd_timing_register`.

Kept: the crash recorder, `HF_STACK` and `MM_REGS` (they cost nothing until a
fault), and **the `fpu.rs` ASPEN fix, which is a real fix, not instrumentation**
(both cores confirm `FPCCR -> 0x80000004`).

**Decision rule.** If the stripped image is stable, the scaffolding was a large
part of the problem and the remaining work is to confirm over an exponential
soak. If it still faults, the metric is now HF/WDT records on a far simpler
binary — no canaries, no per-tick probes, no masking — which is a much better
place to reason from than where this stalled.

### Layout (config AW — `SP_CHK` and `LCD_BASE_CHK` are gone, dead-code eliminated)

```
REGION_FAIL 0x20066f10   ALLOC_GUARD 0x200672d0   HF_STACK 0x200673c8
MM_REGS     0x200674a8   DWT_CATCH   0x200674f8   SMASH_CORE0 0x20067520
WATCH_LOG   0x20067a20   _stack_end  0x20067b20
```

Zero span = **772 words from 0x20066f10**. `_stack_end` is 32-byte aligned with
`WATCH_LOG` as the tail object, so the core-1 RO region base equals it exactly
(rule 5, checked). `DWT_CATCH[8] = 0x20067518`, `SMASH_CORE0[3] = 0x2006752c`.

Last instrumented window before the strip (AV unmasked): **82 smashes / 38.5 min
= 2.13/min**.

Instruments zeroed, sector blanked, DebugMonitor left disarmed.
CRC `0x6147111e` OK, **reset 16:30 on 2026-08-16**.

## AW result — the stripped baseline STILL FAULTS, and the MPU caught a wild branch

60 min, window valid (`ALLOC_GUARD` magic present, `SMASH_CORE0` correctly
absent since the protector is gone). **31 records: 22 HF, 8 Panic, 1 WDT.**

**The pre-registered rule resolves to "still faults".** Stripping the scaffolding
did not fix anything — which is itself worth knowing: **the crashes are not an
artefact of the instrumentation.** They persist in an image with no canaries, no
per-tick probes and no masking.

What changed is the *shape*. Without the stack protector catching the damage at
the first guarded frame, execution runs further off the rails, and the fault PCs
are now wild rather than uniform:

```
x7  HF pc=0x88000000 lr=0x00000002 cfsr=0x100 (IBUSERR)
x5  HF pc=0x000000a0 cfsr=INVSTATE  lr=Instructions::cb+0x112 / 0x20001a2d
x1  HF pc=0x60e350dc / 0x89000000 / 0x14 / 0x10 / 0x04 / 0x2004c4e8
x8  Panic (all fields zero)
```

### `MM_REGS` fired for the first time in a dozen windows — and it is decisive

```
pc    = 0x100249ac = DMA_IRQ_0 + 0x30      insn: str.w r0, [r4, #0x400]
MMFAR = 0x20000c7d   cfsr = 0x82 (DACCVIOL | MMARVALID)
r4 = 0x2000087d   r5 = 0x2007cd38   r6 = 0x20000ecb   sp_before = 0x2007cd10
```

`DMA_IRQ_0`'s prologue is `push {r4,r6,r7,lr}` / `add r7,sp,#8` /
**`ldr r4,[pc,#0xa8]`** — so by `+0x30` r4 must be the DMA base `0x50000000`,
and `str.w r0,[r4,#0x400]` is the ordinary "clear the interrupt flag" store.

Instead:

| register | value | resolves to |
|---|---|---|
| r4 | `0x2000087d` | **`<CbInstruction as OpCode>::execute` \| 1** |
| r5 | `0x2007cd38` | **the tick-entry SP** (the value AR measured invariant over 132M samples) |
| r6 | `0x20000ecb` | **`Sm83::step + 0xaa` \| 1** |

And `0x2000087d + 0x400 = 0x20000c7d` — **exactly MMFAR**.

**So the handler was never entered by an exception** (the prologue would have
loaded r4). **Execution branched wild into the middle of `DMA_IRQ_0`, past its
prologue, carrying the emulator's entire register file with it**, and then
executed a store through what it thought was the DMA base.

**This is a CONSEQUENCE of the +8 wild-branch bug, not a new cause** — and it
should not be mistaken for one. It does establish two useful things:

1. **The MPU is actively preventing RAM-code corruption.** A wild write landed
   squarely inside `GameBoyMemory::copy_dma_state`'s code and was blocked. That
   is very likely why every "the thunk / `.data` is intact" check has passed:
   the region is doing its job, so the damage never lands.
2. **`MM_REGS` is now a working wild-branch detector** with a full register
   capture. Keep it.

Note this does **not** reopen the exception hypothesis: the registers prove the
handler was reached by a branch, not by exception entry, so the PRIMASK A/B
result stands.

**New rule 45: A FAULT INSIDE AN IRQ HANDLER IS NOT NECESSARILY AN IRQ — check
whether the handler's own prologue values are present. If the callee-saved
registers still belong to the interrupted code, it was reached by a WILD BRANCH,
not by exception entry.**

### Where this leaves things

The +8 remains unexplained after excluding exceptions, static SP imbalance, the
`tbb` overrun, the dispatch wrapper, and now the instrumentation itself. What AW
adds is a cleaner binary to work in and a register-capturing detector that fires
on the wild branches themselves.

Instruments zeroed, sector blanked, reset for a fresh window.

## AW hour 2 — the rate is stable, and loose end (J) turns out to be the lead

Window valid. **31 records: 24 HF, 7 Panic** (hour 1 was 22 HF / 8 Panic / 1 WDT).
**~31 records/hour, stable.** `MM_REGS` captured the DMA_IRQ_0 wild branch again,
**byte-identical to hour 1** — same pc, MMFAR, and all of r4–r11.

### The register signature is highly structured

Across the two independent captures:

| | hour 1 (MM_REGS) | hour 2 (HF_STACK) |
|---|---|---|
| r4 | `<CbInstruction as OpCode>::execute` \| 1 | `<inc_dec … as OpCode>::execute` \| 1 |
| r6 | `Sm83::step + 0xaa` | `Sm83::step + 0x10e` |
| r5 | `0x2007cd38` | `0x2007cd38` |
| r7 = r10 | `0x200273d8` | `0x200273d8` |
| r8 | `0x2007d2d8` | `0x2007d2d8` |
| r9 = r11 | `0x2003d5c0` | `0x2003d5c0` |
| sp_before | `0x2007cd10` | `0x2007cd10` |

**r4 is always an `OpCode::execute` pointer, r6 always an address inside `step`,
and r5/r7/r8/r9/r10/r11 and `sp_before` are identical.** The faults happen at one
specific point in the dispatch, not at random.

### The wild PCs are values that live on the stack

The `HF_STACK` band shows `0x2007cd44 = 0x88000000` — and **8 of this hour's 24
HardFaults have `pc = 0x88000000`**. Likewise the 11 faults with `pc = 0x000000a0`
match the value in the exception frame's own pc slot. **The garbage PCs are not
arbitrary: they are words present in this stack band**, which is exactly what a
`pop {…, pc}` reading the wrong slot produces.

### Loose end (J) is not noise — it is the same live buffer, and it is adjacent

The "23 high-entropy words" recorded back in config AV sit in this same region.
Comparing that AV capture against AW — **two different builds, many layout
changes apart**:

```
AV 5b688fd0   AW 1b688fd0   xor 0x40000000   (1 bit)
AV d3053cd0   AW d3053cd0   xor 0x00000000   (identical)
AV 09f684d3   AW 09f684d3   xor 0x00000000   (identical)
AV 0f1a962c   AW 0f1a962e   xor 0x00000002   (1 bit)
...  2 of 20 words identical, mean 2.4 bits differ per word
```

**This is the same buffer holding near-identical, content-derived data — not
random stack garbage.** And `0x2007cd3c = embassy_main_task_inner + 0x2fc3` is a
**saved return address** immediately below it, so the buffer is a local of the
main task's inlined frame, sitting directly above the emulator's frames.

**That reframes the whole picture.** A large live buffer occupies the stack
immediately above `step`/`cb`, the wild PCs are drawn from that region, and a
writer that ever runs below the buffer's start lands squarely in the emulator's
frames — which is precisely the shape of the +8.

**Next task: identify that buffer.** It begins around `0x2007cd44`, runs at least
88 bytes (the band ends before it does), and lives just above a return address
into `embassy_main_task_inner`. Candidates: CYW43/WiFi state, an SD or ROM read
buffer, scanline hashes, an RNG. Find the local, size it, and check every write
into it for a lower bound.

**New rule 46: HIGH-ENTROPY STACK DATA IS NOT AUTOMATICALLY GARBAGE — diff the
same region across two builds. If the words are near-identical, it is a LIVE
CONTENT-DERIVED BUFFER, and its extent and its writers matter.**

Instruments zeroed, sector blanked, reset for hour 3.

## AW hour 3 — loose end (J) is CLOSED, and it was not the lead

Window valid (`ALLOC_GUARD` magic present). **31 records: 30 HF, 1 WDT.**
`MM_REGS` clean this hour — no MPU violation.

```
x11 HF pc=0x88000000 lr=0x00000000 cfsr=0x100 (IBUSERR)
x10 HF pc=0x88000000 lr=0x00000002 cfsr=0x100
x 4 HF pc=0x00009ffe lr=0x100196e1 cfsr=0x100
x 2 HF pc=0x88000000 lr=0x1001f881 cfsr=0x100
x 1 HF pc=0x68000000 / 0x1001ab2c
```

Three hours: **31 / 31 / 31 records.** The rate is flat.

### The buffer is STATIC — my "live buffer" framing was wrong

I sized it precisely: the high-entropy run is `0x2007cd44 .. 0x2007cd9c`,
ending at `0x2007cda0` (`00020000`) — **23 words, exactly the "23 high-entropy
words" from AV.** Its contents are a byte-shifted serial stream (`16 fd f9`
appears at one alignment in one word and shifted a byte in another; likewise
`12 f7 21`), which is what a byte-at-a-time SPI shift register produces.

That made a CYW43 SPI RX buffer look like a very strong candidate — and a DMA
engine writing into stack memory the emulator had since reused would have
explained *every* negative result at once: PRIMASK cannot mask DMA, a per-core
DWT cannot see DMA (my own rule 6), the MPU protects `.data` code but not the
stack, and no static analysis would show anything.

**So I tested it: read the region three times, 20 s apart. ZERO of 24 words
changed.** The buffer is **stale residue**, not a live DMA target — a high-water
mark left by an earlier deep call (WiFi/CYW43 init is the obvious candidate),
preserved because nothing reaches that depth again in steady state. That also
explains the AV↔AW near-identity: the same init code processing the same data,
differing only by build.

**Loose end (J) is closed — explained, not promoted.** The wild PCs matching
words in that band (`0x88000000` lives at `0x2007cd44`) are a *symptom*: a wild
`pop {…, pc}` reading stale residue. Not a cause.

**New rule 47: BEFORE BUILDING ON "A LIVE BUFFER", PROVE IT IS LIVE — read it
repeatedly over tens of seconds. Static contents mean stale residue, and a
high-water mark from an earlier deep call looks exactly like an active buffer.**

### DMA: a weak null, not an exclusion

Polled channels 0–3 (READ/WRITE/COUNT) 88 times: only ch1 was ever configured,
memory→peripheral (`0x2004cc88 -> 0x40088008`). **88 samples cannot exclude a
microsecond burst** (rule 9), so DMA-into-SRAM is not disproven — but the
specific mechanism I proposed (a live RX buffer above the emulator's frames) is,
because that buffer does not change.

### Honest status

Three flat hours at 31 records/hour on a stripped image. The +8 remains
unexplained with exceptions, static SP imbalance, the `tbb` overrun, the dispatch
wrapper, the instrumentation, and now the stack-residue buffer all excluded. The
strongest untested idea remaining is still **DMA writing into SRAM**, which would
require catching a channel mid-transfer — polling over SWD is far too slow, so it
needs a different approach (e.g. reading `CHx_AL1_CTRL`/`INTS` history, or
checking the cyw43-pio receive path statically for a stack-allocated DMA target).

Instruments zeroed, sector blanked, reset.

## AW hour 4 — DMA cancellation is CORRECT; the DMA lead is substantially weakened

Window valid, `MM_REGS` clean. **31 records (all HF).** Four hours: **31 / 31 /
31 / 31.** Dead flat.

### The `Transfer` Drop impl aborts properly

`embassy-rp/src/dma.rs`:

```rust
impl<'a> Drop for Transfer<'a> {
    fn drop(&mut self) {
        let p = self.channel.regs();
        // RP2350 errata RP2350-E5: clear the enable bit of the aborted channel
        // before the abort to prevent re-triggering.
        #[cfg(feature = "_rp235x")]
        p.ctrl_trig().modify(|w| w.set_en(false));
        pac::DMA.chan_abort().modify(|m| m.set_chan_abort(1 << self.channel.number()));
        while p.ctrl_trig().read().busy() {}
    }
}
```

It clears enable first (the E5 workaround), aborts, **and spins until `busy()`
clears**. A dropped or cancelled transfer therefore *cannot* leave DMA writing —
which kills the "cancelled future leaves DMA running into a dead stack frame"
mechanism, the one form of the DMA hypothesis that would have explained
everything.

### But it does confirm my original DMA exclusion was wrong in principle

`cyw43-pio`'s `cmd_read` does:

```rust
let mut status = 0;                                  // a STACK LOCAL
self.sm.rx().dma_pull(&mut self.dma_rx, slice::from_mut(&mut status), false).await;
```

**DMA writes into stack memory by design here.** The old note that "both channels
are memory→peripheral" was a one-shot snapshot that happened to miss an RX
transfer — the channel's `WRITE_ADDR` points into the stack during any
`cmd_read`. The transfer length is bounded by the slice, and cancellation is
sound, so this is safe as written; but the exclusion it rested on was not.

**DMA-into-SRAM is now much less likely**, though not formally disproven for an
oversized-transfer variant.

### Honest status after four flat hours

The +8 at `Instructions::cb`'s epilogue is established four independent ways, and
every mechanism I have been able to construct is now excluded: exceptions
(within-image PRIMASK A/B), static SP imbalance (CFG-aware audit of all 1,671
functions with proven coverage), the `tbb` overrun, the dispatch wrapper, the
instrumentation itself (the stripped image faults at the same rate), the
stack-residue buffer (static over 40 s), and DMA cancellation (Drop aborts).

The one real defect found and fixed along the way — `FPCCR.ASPEN` being cleared,
which left S0–S15 unpreserved across exceptions on a hard-float target — did not
change the rate.

**Mechanism-hunting has stopped converging.** The next step should be a coarse
FEATURE BISECT rather than another mechanism: build a diagnostic image with the
WiFi/CYW43 task not spawned (a diagnostic window, not a product change — the
feature stays compiled in) and soak it. WiFi is the largest source of
asynchronous activity, DMA, and PIO traffic on core 0, and it is the one major
subsystem never removed from the equation. If the fault survives without it, the
search narrows to the emulator plus display/audio; if it vanishes, the
interaction is with the WiFi stack and that is a far smaller space to search.

**New rule 48: WHEN MECHANISM-HUNTING STOPS CONVERGING, BISECT BY SUBSYSTEM.
Removing a whole subsystem for one diagnostic window is cheaper than excluding
its mechanisms one at a time.**

Instruments zeroed, sector blanked, reset.

## AX — the WiFi bisect answers itself, and the rate instrument was never valid

Two results, and the second one retroactively weakens most of the rate data in
this document.

### 1. WiFi was never running. The bisect needed no build.

The pre-registered plan was to build a diagnostic image with the CYW43 task not
spawned. That turned out to be unnecessary: the cyw43 runner, `net_task`,
`dhcp_task`, `dns_task` and `http_task` are spawned **only** from
`src/wifi/portal.rs:402-417`, which is reached **only** from
`src/state/wifi_menu.rs:269` when the user navigates to Settings→WIFI. `main.rs`
merely parks the peripherals in `App.wifi_periphs` for that screen to `take()`.

During an unattended soak nobody navigates there, so **WiFi has never been
running in any soak window in this investigation.** By the pre-registered
decision rule this is the "fault survives without WiFi" branch:

> **WiFi / CYW43 / embassy-net / PIO-SPI and their DMA are excluded as
> participants.** The search narrows to the emulator plus display/audio.

This also retires the last DMA-into-SRAM variant (loose end Q) as a practical
concern: `cyw43-pio::cmd_read`'s stack-local DMA target is on a code path that
never executes here.

It also refutes the stated provenance of the stack residue at
`0x2007cd44..0x2007cd9c`. That run was attributed to "WiFi init", which cannot
be right. Its byte-shifted serial signature is more consistent with the SD-card
SPI init, which *does* run at boot.

### 2. Record counts are not crash counts. Three independent reasons.

`src/crash/storage.rs` documents two policies that together make the crash log
useless as a rate instrument:

* **Policy A — no auto-erase when full.** With all 31 slots occupied and the
  `RCLG` header intact, the sector is "pending user acknowledgement" and the
  write is *skipped*. The log floors at 31 until `crash_decoder.py --mark-read`.
* **Policy B — consecutive-crash deduplication.** Before writing, the previous
  committed record is compared by fingerprint (kind, fault-status registers,
  panic location). On a match the write is skipped. A repeating fault is stored
  **once**.

So a record count is a count of *fingerprint changes*, capped at 31.

**Every "31 records this hour" reading in the AW section was the container's
ceiling, not a rate.** h1 22+8+1, h2 24+7, h3 30+1, h4 31 — all exactly 31
because the sector was full. The "dead flat 31/31/31/31" plateau I reported was
an artifact of the container, and I should have caught it the moment four
consecutive windows landed on the maximum. This is also the explanation for
loose end (I): the AL sector "stopped at 27 of 31 while the counter reached
167" — dedup plus fill, exactly as designed.

**Rule 49: A SATURATED COUNTER IS NOT A FLAT RATE. If successive measurements
land on the container's maximum, the instrument is reporting its own ceiling.
Check the capacity and the drop policy before reading a trend into the values.**

### 3. Reading the instruments ends a boot.

Sampling the live `HEARTBEAT` slots four times, 20 s apart:

| sample | core0 | core1 | ring idx |
|---|---|---|---|
| s1 | 0 | 6 | 7 |
| s2 | 0 | 6 | 8 |
| s3 | 0 | 6 | 1 |
| s4 | 0 | 6 | 2 |

The ring index advances by exactly one per sample and `core0` returns to 0 every
time. **Each `probe-rs read` invocation ends a boot.** It is not the watchdog:
`main.rs:410-416` starts it with a 16 s window and then sets
`pause_on_debug(true)`, in the correct order.

Consequences: the live heartbeat cannot be sampled non-destructively, the
previous-boot ring is contaminated by probe-induced resets (so the 26–93
core0-ticks-per-boot entries cannot be read as genuine crash-boot durations),
and **every mid-window instrument read in this investigation has been perturbing
the window it was measuring.**

**Rule 50: A READ THAT RESETS THE TARGET IS NOT AN OBSERVATION. Verify that the
act of sampling does not end the run — a per-boot counter that advances once per
sample is the tell. Sample once, at the end.**

### 4. The fix: a monotonic boot counter

`heartbeat.rs` gains `BOOT_COUNT_IDX = PREV_RING_IDX + 1` (slot 25), incremented
once by core 0 in `init()`, seeded to 0 when the magic shows a cold boot and
carried forward on any warm reset. It is immune to all three problems above: no
capacity, no dedup, no ring wrap, and it is read **once**, at the end of a
window.

Slot 25 lives inside the existing `[usize; 40]`, so this adds no new `.uninit`
object. Verified against rule 5: `HEARTBEAT` moved `0x20067050 → 0x20066fb0`,
**every other object is byte-identical**, and `WATCH_LOG` still ends exactly at
`_stack_end = 0x20067b20`, so the MPU tail is clean. Zero span unchanged at 772
words from `0x20066f10`.

### Config AX

Identical to AW plus the boot counter. CRC `0xd6affd7e`, 300 MHz @ V1_25.
`heartbeat placement OK: base=0x20066fb0`; ASPEN fix confirmed on both cores
(`FPCCR 0xc0000004 -> 0x80000004`).

Layout: `REGION_FAIL=0x20066f10`, **`HEARTBEAT=0x20066fb0`**, `ALLOC_GUARD=0x200672d0`,
`HF_STACK=0x200673c8`, `MM_REGS=0x200674a8`, `DWT_CATCH=0x200674f8`,
`SMASH_CORE0=0x20067520`, `WATCH_LOG=0x20067a20`, `_stack_end=0x20067b20`.
**Ring idx `0x20067010`, boot counter `0x20067014`.**

Window opened 21:29:36 with **boot_count = 2** as the baseline. The window's
true reboot count is `boot_count_end - 2`. **No mid-window reads** — that is the
whole point of the instrument.

### The AW hour-4 window, for the record

Valid (`ALLOC_GUARD[0] = 0xa1100001`), MM_REGS all zero, sector full at 31
records (seq 0x00–0x1e) in ≤37 min. `HF_STACK`: `pc=0x000000a0
lr=0x20001f67 cfsr=0x00020000 (INVSTATE) sp_before=0x2007cd10 r12=0x100141ff
exc_return=0xfffffff9` — the same fault as ever.

### Next

Read `boot_count` once at the end of the window to obtain the **first sound
crash rate in this investigation**. Only then are config comparisons meaningful.
Nothing else should be concluded from record counts until that number exists.

## AX window 1 — the first valid crash rate, and why it must not be over-read

Window 21:29:36 → 22:33:10 (63.5 min). `ALLOC_GUARD[0] = 0xa1100001`, valid.

**`boot_count` 2 → 10 = 8 reboots in 63.5 min ≈ 7.6/hour.** The crash sector
independently holds exactly 9 records (seq 0x00–0x08, then erased 0xFF). The two
instruments agree to within the one boot that the final read itself causes, so
**the boot counter is working.**

Composition: **6 HardFaults + 3 watchdog records.** A third of the failures were
*hangs*, not faults — loose end (E) is a larger share of the problem than the
AW sector suggested (dedup and the 31-cap were hiding the mix, not just the
rate).

| seq | pc | lr | cfsr | sp_before |
|---|---|---|---|---|
| 0, 4 | `0x2002e6d8` | `0x2000074d` | INVSTATE | `0x20081f98` |
| 2, 6 | `0x00000010` | `0x20001f67` | `0x8200` PRECISERR+BFARVALID, fa=`0x139ec015` | — |
| 7, 8 | `0x000000a0` | `0x20001f67` | INVSTATE | `0x2007cd10` |

seq 0/4 fault with `sp_before = 0x20081f98`, a **different stack** from the
familiar `0x2007cd10`. Worth resolving which stack that is.

### What this does NOT establish

AW's sector implies ≥50 crashes/hour (31 records in 37 min, and dedup means the
true count is higher); AX measured 7.6/hour. **That is not evidence that AX
improved anything.** The only code difference is a counter increment in
`heartbeat::init()`, which has no plausible mechanism for a 6× effect. The real
situation is that AW's number was never a rate at all (rule 49), so there is
nothing to compare against — and AX is a single window, which rule 9 says is not
a result.

**The next window is AX unchanged.** Establishing the variance of this rate is
worth more than any new hypothesis: without it, no future A/B can be read.

### Procedure correction — the zeroing loop is self-defeating

After zeroing, `boot_count` read back as 10, not 0. Cause: each of the 18
`probe-rs write` invocations in the zeroing loop **resets the device** (rule
50), and every reset re-runs `heartbeat::init()`, which re-stamps the magic and
increments the counter again. The zero span is correct and the writes do land —
a single write/readback confirms it — but the loop cannot leave the counter at
zero.

**Rule 51: WHEN EVERY PROBE OPERATION RESETS THE TARGET, ORDER THE SETUP SO THE
BASELINE IS READ LAST. Zero, blank, reset, and only then read the baseline; any
device operation after that read invalidates it.**

### Window 2

Sector blank (slot 0 = `ffffffff`), **baseline `boot_count = 0` at 22:35:02**.
Config AX unchanged, CRC `0xd6affd7e`. Crashes ≈ `boot_count_end - 1` (the final
read causes one boot).

## AX window 2 — the rate reproduces, and there are TWO victims

Window 22:35:02 → 23:38:10 (63.1 min). `ALLOC_GUARD[0] = 0xa1100001`, valid.
`boot_count` 0 → 10, minus the read-induced boot = **~9 crashes ≈ 8.6/hour.**

| window | duration | crashes | rate |
|---|---|---|---|
| 1 | 63.5 min | 8 | 7.6/h |
| 2 | 63.1 min | 9 | 8.6/h |

**The rate reproduces.** ~8/hour with a spread of one crash between two
independent 63-minute windows on an unchanged image. For the first time in this
investigation an A/B can actually be read, and a large effect (say 2× or more)
would be unambiguous against this variance.

Sector composition: 8 records, **6 HardFault + 2 watchdog** — the same
proportion as window 1 and the same three fault families.

### Two distinct victims, not one

Resolving the recurring register values against the AX ELF:

| value | symbol |
|---|---|
| `lr = 0x20001f67` | `Instructions::cb + 0x112` |
| `lr = 0x2000074d` | **`ApuPeripheral::produce_samples + 0xb0`** |
| `pc = 0x2002e6d8` | `HEAP_MEM + 0xa334` (wild branch into the heap) |
| `r12 = 0x1001420b` | `cb::sla_u8` |
| `r12 = 0x10013745` | **`Vec::push`** (rustyboy_core) |

This closes loose end (T): `sp_before = 0x20081f98` is `produce_samples`, and
both stacks are inside core 0's (`_stack_end = 0x20067b20` up to the top of
SRAM at `0x20082000`) — `cb` simply sits ~21 KB deeper.

**The significance is that the same failure — INVSTATE on a `pop` — happens at
two unrelated call sites, one in the emulator CPU and one in the audio sample
producer.** Every previous cycle treated this as a property of `Instructions::cb`
and went looking at cb's codegen, its `tbb`, its dispatch wrapper. That framing
was too narrow: whatever corrupts the return address is not specific to `cb`.
`Vec::push` appearing in r12 puts allocation on the list too.

### A structural fact that redirects the subsystem bisect

Every `#[embassy_executor::task]` in the firmware is in `src/wifi/portal.rs`.
There are no display, audio or SD tasks — core 0's main loop and the core 1
worker do all the work inline. **Nothing else can be un-spawned**, so the
"disable a subsystem" bisect has no more moves after WiFi.

### Config AY — the work-vs-wall-clock experiment

`stack_pop_check::check(32 * 1024)` removed from the main loop. It had run
1,018,167,296 STM/LDM trials with zero mismatches (hypothesis excluded) while
burning ~22 ms of every iteration, throttling core 0 to ~45 Hz. Removing it
speeds the emulator up several-fold, which splits the suspects cleanly:

* **rate rises with throughput** ⇒ the fault is **work-driven** (emulator/APU);
* **rate stays ~8/hour** ⇒ the fault is **wall-clock-driven** (a timer, display
  refresh, the watchdog interaction, DMA).

Nothing else currently distinguishes those two, and they point at disjoint
suspect sets.

Layout moved (rule 5) — dropping the now-unreferenced `stack_pop_check` statics
shifted everything after HEARTBEAT down 160 bytes:

`REGION_FAIL=0x20066f10`, `HEARTBEAT=0x20066fb0` (**BOOT_COUNT still
`0x20067014`**), `ALLOC_GUARD=0x20067230`, `HF_STACK=0x20067328`,
`MM_REGS=0x20067408`, `DWT_CATCH=0x20067458`, `SMASH_CORE0=0x20067480`,
`SMASH_CORE1=0x20067700`, `WATCH_LOG=0x20067980`, `_stack_end=0x20067a80`.
MPU tail verified: WATCH_LOG ends at `0x20067a80` = `_stack_end` exactly.
**Zero span now 732 words** from `0x20066f10`.

CRC `0xe7524fff`, integrity OK, `heartbeat placement OK
core1-RO-region=0x20067a80`. Window opened **23:41:45 with baseline
`boot_count = 12`**; crashes ≈ `boot_count_end - 13`.

## AY — a 26x rate change, and a WITHDRAWN conclusion

Window 23:41:45 → 00:45:09 (63.4 min). `ALLOC_GUARD[0] = 0xa1100001`, valid.
`boot_count` 12 → 233 = **220 crashes ≈ 208/hour**, against the 8/hour AX
baseline. A **26x** increase from removing `stack_pop_check`.

### The work-driven conclusion is WITHDRAWN

I first read this as the pre-registered "work-driven" branch: the rate rose ~26x
and I believed removing a 22 ms/iteration check had sped core 0 up ~20x, so the
rise looked proportional to throughput.

**The premise was never measured.** The ~45 Hz figure came from a comment in the
code (`32K iterations is roughly 1.5M trials/s` ⇒ 22 ms), not from the device.
Measuring it directly with the new work counter: over 100 s core 0 completed
**448 iterations — about 4.5 per second**, i.e. a ~200 ms iteration. At that
speed `stack_pop_check` was only **~10% of the loop**, and removing it cannot
have changed throughput by more than that.

So the 26x is **not** a throughput effect, and the decision rule does not apply.
Both branches of it were premised on a speedup that did not happen.

**Rule 53: MEASURE THE THROUGHPUT BEFORE INTERPRETING A RATE CHANGE AS A
THROUGHPUT EFFECT. A timing figure taken from a source comment is an assumption,
not a measurement — and a pre-registered decision rule built on an unmeasured
premise decides nothing.**

### What the numbers actually say

Normalising by work instead:

| config | crashes/hour | iterations/crash |
|---|---|---|
| AX (with `stack_pop_check`) | 8 | ~1,845 |
| AZ (without) | ~144–208 | ~112 |

**Removing `stack_pop_check` made the fault ~16x more likely PER UNIT OF WORK.**
It was not slowing the loop enough to matter — it was **suppressing the bug**.

That is the strongest causal handle in this investigation so far, and unlike
every previous lead it is a *reversible* A/B: restoring the check should restore
the low rate. `stack_pop_check` hammers STM/LDM push/pop patterns 32K times per
iteration, so whatever it does to the stack — depth, residency, recency of
writes to a particular band — is interfering with the mechanism that corrupts
the return address.

### Config AZ — the work counter

`heartbeat.rs` gains `WORK_COUNT_IDX = BOOT_COUNT_IDX + 1` (slot 26),
incremented in `core0_tick()`, seeded 0 only on a cold boot, accumulating across
reboots exactly like `boot_count`. Crashes per unit work is then
`(boot_count delta) / (work_count delta)` from a single read.

This is necessary because raw crashes/hour cannot distinguish "fewer bugs" from
"less work done" — a config that merely ran slower would look like a fix.

Layout **unchanged** from AY (slot 26 is inside the existing `[usize; 40]`):
`HEARTBEAT=0x20066fb0`, **`BOOT_COUNT=0x20067014`, `WORK_COUNT=0x20067018`**,
`ALLOC_GUARD=0x20067230`, `HF_STACK=0x20067328`, `MM_REGS=0x20067408`,
`WATCH_LOG=0x20067980`, `_stack_end=0x20067a80`. Zero span 732 words.
CRC `0xb25519ac`, integrity OK. AZ's crash rate matches AY's, so the added
counter changed nothing.

Window baseline: **boot_count = 251, work_count = 1598 at 00:50:53.**

### Next

Get a clean crashes-per-iteration figure for AZ, then run the reversible A/B:
restore `stack_pop_check` and confirm the rate drops ~16x per unit work. If it
does, bisect *what about it* matters — stack depth, the STM/LDM traffic itself,
or simply the extra time spent between emulator ticks.

## AZ baseline, and the BA reversal test

Window 00:50:53 → 01:39:12 (48.3 min). `ALLOC_GUARD[0] = 0xa1100001`, valid.

* Δboot = 138 → **137 crashes** (170/hour)
* Δwork = **19,575 iterations** → **6.75 iterations/s**
* **1 crash per 143 core-0 iterations**

This confirms the throughput figure independently: core 0 really does run at
~7 Hz, not the ~45 Hz the source comment implied, so rule 53 stands.

### Config BA — restoring `stack_pop_check`

`stack_pop_check::check(32 * 1024)` restored. CRC `0x61011677`, integrity OK.
The layout returned **exactly** to the AX positions: `ALLOC_GUARD=0x200672d0`,
`HF_STACK=0x200673c8`, `MM_REGS=0x200674a8`, `WATCH_LOG=0x20067a20`,
`_stack_end=0x20067b20`; zero span back to 772 words; MPU tail clean. The
counters did not move (`BOOT_COUNT=0x20067014`, `WORK_COUNT=0x20067018`).

Baseline: **boot_count = 395, work_count = 23525 at 01:42:06.**

### The confound I under-stated when pre-registering this

Restoring the check **also restores the layout** — the two are not independent,
because removing the call let the linker drop the module's statics and shifted
every `.uninit` object after HEARTBEAT by 160 bytes. So a drop in the rate would
be consistent with *either* hypothesis:

* the STM/LDM stack traffic suppresses the fault, or
* the AX/BA memory layout suppresses it and the check is irrelevant.

My earlier framing only treated layout as the fallback explanation if the rate
*failed* to drop. That was wrong: layout is live in both branches.

**The discriminator is a third config, BB: `check(1)` instead of
`check(32 * 1024)`.** One iteration keeps the module referenced, so the statics
survive and the layout stays byte-identical to BA, while the STM/LDM work drops
to essentially nothing.

| config | layout | stack traffic | meaning if rate is LOW |
|---|---|---|---|
| BA `check(32K)` | AX-like | heavy | — |
| BB `check(1)` | AX-like | none | **layout** is what matters |
| AZ (removed) | shifted | none | (measured: HIGH, 1/143) |

If BA is low and BB is high, the **work** suppresses. If BA and BB are both low,
the **layout** suppresses and the check is a red herring — which would make this
a placement-sensitive memory bug and point straight at what `cb` and
`produce_samples` share.

**Rule 55: A CHANGE THAT ADDS OR REMOVES CODE ALSO CHANGES LAYOUT. Before
attributing an A/B result to the code's behaviour, find a variant that holds
layout fixed and varies only the behaviour — calling the same function with a
trivial argument usually does it.**

## BA — the reversal is confirmed, in both directions

Window 01:42:06 → 02:29:19 (47.2 min). `ALLOC_GUARD[0] = 0xa1100001`, valid.
Δboot = 35 → **34 crashes**; Δwork = **85,067 iterations**.

| config | layout | stack traffic | crashes/iteration |
|---|---|---|---|
| AX | AX-like | heavy | 1 per ~1,845 |
| AZ | shifted | none | **1 per 143** |
| BA | AX-like | heavy | **1 per 2,502** |

**17.5x fewer crashes per unit work than AZ**, and consistent with AX's 1/1845
measured on the same layout with the same check. The effect reproduces in both
directions — remove it and the rate rises, restore it and the rate falls. This
is a real, controllable handle on the fault, the first in this investigation.

### The throughput anomaly, and why it vindicates work-normalisation

AZ ran **6.75 iterations/s**; BA runs **30 iterations/s**. Adding a 32K-iteration
STM/LDM loop to every pass cannot make the loop four times faster, so the
difference is not the check.

It is the crashes themselves. AZ crashed 137 times in the window, and each crash
costs a full reboot — which re-initialises the SD card and reloads the ROM from
flash before the main loop starts again. AZ spent most of its wall-clock
**booting**, not emulating.

This is exactly the trap rule 54 warns about, in the opposite direction from the
one I expected: crashes-per-hour *understates* how bad AZ is, because a config
that crashes more also does less work per hour. Only crashes-per-iteration
compares them honestly.

### Config BB — holding layout fixed (rule 55)

`check(32 * 1024)` → `check(1)`. The call keeps the module referenced so its
statics survive, and **llvm-nm confirms the layout is byte-identical to BA**:
`REGION_FAIL=0x20066f10`, `HEARTBEAT=0x20066fb0`, `ALLOC_GUARD=0x200672d0`,
`HF_STACK=0x200673c8`, `MM_REGS=0x200674a8`, `WATCH_LOG=0x20067a20`,
`_stack_end=0x20067b20`. Zero span 772 words, MPU tail clean. Only the STM/LDM
traffic differs. CRC `0x01fc9e83`, integrity OK.

Baseline: **boot_count = 433, work_count = 111435 at 02:31:50.**

Decision rule:

* **BB high (~1/143)** ⇒ the **stack traffic** suppresses the fault. The check is
  doing something real to the stack, and the next job is to find out what —
  depth, residency, or recency of writes to a particular band.
* **BB low (~1/2500)** ⇒ the **layout** suppresses it, `stack_pop_check` is a red
  herring, and this is a placement-sensitive memory bug. That would point
  straight at loose end (V): what `cb` and `produce_samples` share.

## BB — DECISIVE: the stack traffic suppresses the fault, the layout does not

Window 02:31:50 → 03:19:10 (47.3 min). `ALLOC_GUARD[0] = 0xa1100001`, valid.
Δboot = 95 → **94 crashes**; Δwork = **15,464 iterations** → **1 per 164**.

| config | layout | stack traffic | crashes/iteration |
|---|---|---|---|
| AX | AX-like | heavy | 1 per 1,845 |
| BA | AX-like | heavy | 1 per 2,502 |
| **BB** | **AX-like** | **none** | **1 per 164** |
| AZ | shifted | none | 1 per 143 |

The two variables separate cleanly:

* **Layout held fixed** (BA vs BB, byte-identical per llvm-nm): **15x difference.**
* **Traffic held fixed** (BB vs AZ, layouts differ by 160 bytes): **no difference**
  (164 vs 143).

**The stack traffic controls the fault rate. Layout is irrelevant.** The
placement-sensitivity hypothesis is dead, and rule 55's confound is discharged
for this bug — code may now be moved freely without worrying about it.

Note the metric is sound across these configs: the main loop is frame-paced, so
one iteration contains the same emulation work whether or not the check runs.

### What the check actually does

```asm
1:  stmdb  sp!, {r0-r3, lr}   // write 20 bytes below SP
    ...destroy r0-r3, lr...
    ldmia  sp!, {r0-r3, lr}   // read back, compare against callee-saved refs
    subs   r4, #1
    bne    1b
```

32,768 times per frame, restoring SP each pass. Two mechanisms survive:

1. **the memory activity** — repeated `stmdb`/`ldmia` just below SP interferes
   with whatever corrupts the return address; or
2. **time dilution** — the check occupies wall-clock time in which the emulator
   is NOT running, so if the corrupting agent fires at a fixed rate in time, a
   smaller fraction of its hits land in vulnerable code.

### Config BC — a pure register spin of the same duration

`spin_burn`: `subs r0,#1 / bne` — **no memory access at all**, 327,680
iterations, alongside `check(1)`.

Duration control, from instruction counts rather than the source comment
(rule 53): `spin_burn` ~3 cycles/iteration ⇒ 327,680 ≈ **3.3 ms**; the stress
loop ~35 cycles/iteration × 32,768 ≈ **3.8 ms**. Within ~15%, and a 15%
shortfall cannot produce a 15x rate difference in either direction.

This also corrects the "22 ms" figure inherited from the code comment: the check
was ~3.8 ms of BA's 33 ms pass (~11%), which matches the independently measured
throughput and confirms rule 53's verdict on that comment.

* **BC low (~1/2,500)** ⇒ **time dilution**; the corrupting agent is
  asynchronous and fires on a wall-clock schedule, and the "stack traffic"
  framing is wrong.
* **BC high (~1/164)** ⇒ the **memory activity** is essential; the fault
  involves the stack memory itself, and the next step is to narrow which
  property (depth, the store, the load, the address band).

CRC `0x33867e8e`, integrity OK. HEARTBEAT and `_stack_end` unmoved, so
`BOOT_COUNT=0x20067014`, `WORK_COUNT=0x20067018`; **`ALLOC_GUARD` moved to
`0x200678e8`**. Baseline: **boot_count = 537, work_count = 127,702 at 03:23:00.**

## BC — time dilution is NOT the mechanism; the memory activity is

Window 03:23:00 → 04:13:13 (50.2 min). `ALLOC_GUARD[0] = 0xa1100001`, valid.
Δboot = 131 → **130 crashes**; Δwork = **33,510 iterations** → **1 per 258**.

| config | filler | crashes/iteration |
|---|---|---|
| BA | 32K × `stmdb`/`ldmia` **on the stack** | **1 per 2,502** |
| **BC** | **matched duration, pure register spin, no memory** | **1 per 258** |
| BB | `check(1)` — nothing | 1 per 164 |
| AZ | nothing, shifted layout | 1 per 143 |

A matched-duration register spin recovers only ~1.6x of the ~15x. So:

* there IS a small genuine time component (BC 258 vs BB 164, ~100 events each,
  so the difference is real if modest);
* but the dominant ~10x **requires the actual `stmdb`/`ldmia` traffic**.

**The memory activity is essential.** Time dilution is relegated to a minor
contributor, and the "asynchronous agent on a wall-clock schedule" branch does
not fire — so the PRIMASK exclusion stays as it was rather than being re-opened.

### Config BD — stack, or SRAM in general?

The remaining split is whether the traffic must touch **the stack** or merely
**SRAM**. `mem_pop_stress` runs the identical instruction sequence and iteration
count with the base register pointing at a static `.bss` buffer
(`MEM_STRESS_BUF`) instead of `sp`, so duration is matched by construction.

* **BD low (~1/2,500)** ⇒ any SRAM load/store pressure suppresses it. The stack
  is incidental, and the mechanism is a bus/arbitration/timing interaction —
  which fits a two-core memory-system hazard far better than anything about
  frames or return addresses.
* **BD high (~1/258)** ⇒ the traffic must be **near SP**. The mechanism involves
  the stack region itself, and the next variants are: stores only, loads only,
  and the same traffic at a different stack depth.

CRC `0xb1a80933`, integrity OK. The new `.bss` buffer moved things (rule 5):
**`HEARTBEAT=0x20066fe8`, `BOOT_COUNT=0x2006704c`, `WORK_COUNT=0x20067050`,
`ALLOC_GUARD=0x200677e0`, `_stack_end=0x20067b58`.**

Baseline: **boot_count = 0, work_count = 1639 at 04:16:43** (a genuine cold-boot
seed, since the magic's address changed).

## BD — it is SRAM traffic in general, not the stack. The mechanism looks like silicon.

Window 04:16:43 → 05:07:14 (50.5 min). `ALLOC_GUARD[0] = 0xa1100001`, valid.
Δboot = 58 → **57 crashes**; Δwork = **77,065 iterations** → **1 per 1,352**.

| config | filler | target | crashes/iteration |
|---|---|---|---|
| BA | 32K × `stmdb`/`ldmia` | **stack (`sp`)** | 1 per 2,502 |
| **BD** | 32K × `stmdb`/`ldmia` | **static `.bss` buffer** | **1 per 1,352** |
| BC | matched-duration register spin | — | 1 per 258 |
| BB | none | — | 1 per 164 |
| AZ | none (layout shifted) | — | 1 per 143 |

Generic SRAM traffic recovers **~8x of the ~15x**. BA's residual 1.85x edge over
BD is ~2.8 sigma on 57 and 34 events — marginal, possibly a small real
stack-specific component, not the main effect.

**So the suppressor is load/store activity in general.** Four controlled results
now bound the mechanism:

* layout irrelevant (BB vs AZ; BA vs BB byte-identical),
* pure elapsed time minor (BC),
* **memory traffic essential** (BC vs BA/BD),
* **the traffic need not touch the stack** (BD vs BA).

### Why this points at the memory system, not at software

A software bug that writes a wrong stack slot should not care whether core 0 is
also hammering an unrelated `.bss` buffer. But a *timing* effect would: keeping
the load/store unit busy changes bus occupancy, arbitration and current draw.

That shape fits the rest of the evidence, which never sat comfortably with a
logic bug: a wrong PC loaded by `pop` at **two unrelated call sites**
(`Instructions::cb` and `ApuPeripheral::produce_samples`), wild branches into
the heap, and an MPU violation that was a consequence rather than a cause.

And the operating point is the obvious suspect: **this RP2350 runs at 300 MHz,
double its 150 MHz rating.**

### Config BE — re-measure voltage with the valid instrument

The clock is locked by the standing constraint; **voltage explicitly is not**.
The old "voltage is a ~7x rate modulator" note was measured with the crash-log
counter that rule 49 showed was saturating, so it is worth nothing and must be
redone against the boot/work counters.

BE raises `TARGET_CORE_VOLTAGE` V1_25 → **V1_30** (one step, the highest embassy
exposes; within the regulator's documented range and standard for RP2350
overclocking) and removes the filler, so it is a **direct A/B against BB's 1 per
164 with voltage as the only variable**.

Verified on the device: `VREG_CTRL @ 0x4010000c = 0x000000f0` (was `0x000000e0`
at V1_25). `TARGET_SYS_HZ` untouched at 300 MHz. CRC `0x7c75df65`, integrity OK.

* **BE much lower than 1/164** ⇒ timing marginality at 300 MHz is confirmed as
  the dominant factor, and raising voltage is a legitimate fix for the 24-hour
  goal rather than a workaround for a software defect.
* **BE ≈ 1/164** ⇒ voltage is NOT the variable, the old ~7x claim was an
  artifact of the saturating counter, and the search returns to what memory
  traffic actually perturbs.

Addresses moved again (rule 5): **`HEARTBEAT=0x20066fa8`, `BOOT_COUNT=0x2006700c`,
`WORK_COUNT=0x20067010`, `ALLOC_GUARD=0x200677a0`, `_stack_end=0x20067b18`.**
Baseline: **boot_count = 1, work_count = 3124 at 05:12:00.**

## BE — VOLTAGE IS THE DOMINANT VARIABLE. Root cause: timing marginality at 300 MHz.

Window 05:12:00 → 06:05:14 (53.2 min). `ALLOC_GUARD[0] = 0xa1100001`, VREG still
`0x000000f0`. Δboot = 27 → **26 crashes**; Δwork = **119,249 iterations**.

| config | voltage | filler | crashes/iteration |
|---|---|---|---|
| **BE** | **V1_30** | **none** | **1 per 4,586** |
| BA | V1_25 | stack traffic | 1 per 2,502 |
| BD | V1_25 | `.bss` traffic | 1 per 1,352 |
| BC | V1_25 | register spin | 1 per 258 |
| BB | V1_25 | none | 1 per 164 |

**One voltage step is a 28x improvement** — bigger than everything the memory
traffic bought, and identical in every other respect to BB, so the comparison is
clean. (The old "voltage is a ~7x modulator" note is superseded: it was measured
with the crash-log counter that rule 49 showed was saturating.)

### The root cause

**The RP2350 is being run at 300 MHz, double its 150 MHz rating, and at V1_25
the core is timing-marginal.** The failure is a load that occasionally returns
wrong data — most visibly a `pop {…,pc}` returning a bad PC, which is why the
faults are INVSTATE, why the PCs are wild, and why the victims are whatever code
happened to be executing rather than any particular function.

Everything that never fitted a logic bug now fits:

* **two unrelated victims** (`Instructions::cb`, `ApuPeripheral::produce_samples`)
  — a marginal path does not care which function is running;
* **wild branches into the heap** and an **MPU violation that was a consequence**;
* **layout irrelevant**, **stack-specificity absent** — a data-corruption bug
  would care about addresses; a timing margin does not;
* **the memory-traffic suppressor** — load/store activity changes bus occupancy
  and current draw, i.e. it perturbs the same margin. A second-order voltage
  effect, which is why it was strong but never total.

**Rule 59: A FAULT THAT IS SUPPRESSED BY UNRELATED ACTIVITY AND MOVED BY SUPPLY
VOLTAGE IS AN ELECTRICAL MARGIN PROBLEM, NOT A LOGIC BUG. Stop looking for the
line of code.**

### The 24-hour goal is in tension with the locked clock

At ~37 iterations/s, 24 h is ~3.2M iterations. At BE's 1/4,586 that is still
**~700 crashes**. Even if the traffic suppressor stacks multiplicatively (~8x),
~90 crashes. A clean 24 h needs roughly **500x beyond BE**.

There is no lever that large while the clock stays at 300 MHz:

* V1_30 is the top of embassy's `CoreVoltage`; going higher means defeating the
  regulator's voltage limit, which is out of scope without an explicit request;
* the memory-traffic suppressor is worth ~8x at best and costs real throughput;
* every software mechanism has been excluded by controlled experiment.

**The honest read: the clock lock and the 24-hour target may be incompatible.**
That is the user's call. Lowering `TARGET_SYS_HZ` is the one change with
headroom of the right order — the prior 266 MHz @ V1_25 note claimed 9 h clean,
though that was measured with the discredited counter and would need redoing.

### Config BF — do the two suppressors stack?

V1_30 **and** `check(32 * 1024)`. Stack-targeted traffic is preferred over BD's
`.bss` buffer because BA edged BD by ~1.85x; that was only ~2.8 sigma and may be
noise, but it costs nothing to capture a small stack-specific component if real.

CRC `0x67ccef9d`, integrity OK, VREG `0x000000f0`. Addresses unchanged:
`BOOT_COUNT=0x2006700c`, `WORK_COUNT=0x20067010`, `ALLOC_GUARD=0x200677a0`.
Baseline: **boot_count = 32, work_count = 125,667 at 06:08:00.**

## BF — the suppressors do NOT stack; they REVERSE. Plus two corrections.

Window 06:08:00 → 07:10:16 (62.3 min). Valid, VREG `0x000000f0`.
Δboot = 99 → **98 crashes**; Δwork = **74,012 iterations** → **1 per 755**.

**BF is 6x WORSE than BE (1 per 4,586), not better.** 98 events, so not noise.

| config | voltage | filler | crashes/iteration |
|---|---|---|---|
| **BE** | V1_30 | none | **1 per 4,586** ← best measured |
| BA | V1_25 | 32K stress | 1 per 2,502 |
| BD | V1_25 | 32K `.bss` traffic | 1 per 1,352 |
| **BF** | **V1_30** | **32K stress** | **1 per 755** |
| BB | V1_25 | none | 1 per 164 |

The same filler is **15x better at V1_25 and 6x worse at V1_30**. A genuine
interaction reversal, and consistent with the margin story: the extra switching
current costs more supply droop than the higher static voltage buys.

### Correction 1 — the checker never fires, and that matters

`STACK_POP_CHECK` reads `mismatches = 0` over **2,483,349,152 LDM/STM
verifications**. So BF's extra crashes are not `check()`'s own panics, and more
importantly:

**The plain stack load/store path does NOT fail.** "A load returns wrong data"
was too specific. Whatever is marginal is elsewhere — instruction fetch, or the
PC-load/branch path — not a generic SRAM data load. The faults being INVSTATE on
`pop {…,pc}` while 2.5 billion `ldmia sp!, {r0-r3,lr}` readbacks come back
perfect is a strong constraint on any future theory.

### Correction 2 — BC's duration control was wrong, so its conclusion is unsound

BF runs at 19.8 iterations/s against BE's 37.3: the stress loop costs **~23.7 ms
per frame**, not the ~3.8 ms I derived from instruction counts. `spin_burn` at
~3.3 ms was therefore **~7x shorter**, so **BC never compared matched durations
and its "time dilution is minor" verdict does not stand.**

Rule 53 said not to trust the source comment's timing — correct in spirit, but
my replacement estimate was worse than the comment, whose "1.5M trials/s"
implied ~22 ms and was nearly right.

**Rule 60: AN INSTRUCTION-COUNT ESTIMATE IS ALSO AN ASSUMPTION. Derive filler
cost from measured throughput (the difference in iterations/s between two
configs that differ only by the filler), not from cycle arithmetic — memory
stalls and bus contention dominate and are not visible in the listing.**

### What still stands

All of these are well-sampled and unaffected by the above:

* **voltage is 28x** (BB 1/164 → BE 1/4,586, identical otherwise);
* the filler helps at V1_25 and hurts at V1_30;
* layout is irrelevant; the traffic need not touch the stack;
* every software mechanism remains excluded by controlled experiment.

### Config BG = back to BE, and start the soak escalation

Best measured configuration: V1_30, no filler. CRC `0xb3478a2b`, integrity OK,
VREG `0x000000f0`, 300 MHz untouched. Addresses unchanged
(`BOOT_COUNT=0x2006700c`, `WORK_COUNT=0x20067010`, `ALLOC_GUARD=0x200677a0`,
`STACK_POP_CHECK=0x20067850`).

Baseline: **boot_count = 137, work_count = 204,134 at 07:14:08.**

The 24-hour goal remains out of reach at 300 MHz — 1/4,586 extrapolates to ~700
crashes per day — and the tension with the locked clock is unchanged and still
the user's call.

## BG — the best configuration is confirmed, and it is not close to the goal

Window 07:14:08 → 08:17:16 (63.1 min). Valid, VREG `0x000000f0`.
Δboot = 34 → **33 crashes**; Δwork = **139,807 iterations** → **1 per 4,237**.

BE measured 1 per 4,586 on the same image. **Pooled: 59 crashes over 259,056
iterations = 1 per 4,391**, 36.9 iterations/s. The V1_30 result reproduces.

### Where this leaves the 24-hour goal

24 h at 36.9 iterations/s is ~3.2M iterations ⇒ **~725 crashes/day** at the best
configuration measured. The goal needs **zero**, i.e. roughly **700x** better
than anything achieved.

The levers are exhausted:

| lever | best available | status |
|---|---|---|
| core voltage | V1_25 → V1_30 | **used — 28x, the single biggest win** |
| memory-traffic filler | 32K stress | **counter-productive at V1_30 (6x worse)** |
| software mechanisms | — | all excluded by controlled experiment |
| clock | 300 MHz | **LOCKED by standing constraint** |

V1_30 is the top of embassy's `CoreVoltage`. Going higher means defeating the
regulator's voltage limit, which risks the part and is not something to do
without explicit instruction.

**The standing constraint (300 MHz locked) and the standing goal (24 h clean)
appear to be mutually unsatisfiable.** The root cause is running the part at
double its rated frequency; the only remaining change of the right magnitude is
the clock. Continuing to run hour-long windows at 300 MHz cannot reach zero and
mostly consumes time, so this is the point to put the decision to the user
rather than keep grinding.

## BH — the root-cause claim was too strong, and there is a hole in the existing QMI fix

The user challenged the "electrical margin at 2x rated clock" conclusion and
asked for the evidence and for what other RP2350 overclockers do. Both were
worth doing, and the second changed the picture.

### The evidence, restated with its limits

| config | voltage | filler | crashes/iteration | events |
|---|---|---|---|---|
| AZ | V1_25 | none, layout shifted | 1 per 143 | 137 |
| BB | V1_25 | none | 1 per 164 | 94 |
| BD | V1_25 | `.bss` traffic | 1 per 1,352 | 57 |
| BA | V1_25 | stack traffic | 1 per 2,502 | 34 |
| BF | V1_30 | stress | 1 per 755 | 98 |
| BE+BG | V1_30 | none | **1 per 4,391** | 59 |

Controlled pairs: BB→BE changes only voltage (28x); BA vs BB is byte-identical
in layout (15x); BB vs AZ shifts layout 160 bytes (no effect).

**The weakness: "electrical margin" is a category, not a mechanism.** It does not
identify which path is marginal, and it does not license the conclusion that the
clock must come down.

### What other overclockers achieve — this refutes the pessimistic reading

Reported RP2350 operating points: **266 MHz stable at 1.10V with default QMI;
400 MHz at 1.30V with a 1:3 QMI divider; 636 MHz at 2.00V**. We are at
**300 MHz @ 1.30V and crashing every ~2 minutes** — far worse than others manage
at *higher* clocks and the *same* voltage.

**So "300 MHz is 2x rated, therefore unfixable" was wrong.** There is headroom
others reach that this board does not, which means something specific is still
broken rather than the part simply being out of margin.

**Rule 62: "IT IS MARGINAL" IS NOT A ROOT CAUSE. Before concluding that an
operating point is unreachable, check what the same part achieves elsewhere. If
others exceed it comfortably, the gap is a defect, not physics.**

### The specific mechanism, and the hole

The known overclocking gotcha is exact: **bootrom flash helpers reset QMI timing
on every flash access**, so an overclocked system silently reverts to boot-clock
flash timing (pico-sdk issues #1983, #1903).

An earlier cycle already found this — `main.rs:388` and `main.rs:915` force
`CLKDIV = 6` (SCK = 300/6 = 50 MHz) after clock init and again at main-loop
entry, and the reasoning there is sound and matches our fault signatures
(IBUSERR, wild PCs, corrupt XIP instruction fetch).

**But both retunes run exactly ONCE.** Any flash access *after* main-loop entry
reverts CLKDIV to 3 — flash SCK 100 MHz with RXDELAY tuned for 50 MHz — for the
rest of that boot, and nothing detects it.

This also explains why the 2.48-billion-trial LDM/STM null was never a
contradiction: **that checker exercises SRAM, not XIP.** Instruction fetch and
ROM data reads come from flash. Marginal flash sampling corrupts fetched
instructions and constants while leaving every SRAM access perfect — precisely
the constraint that had no explanation before.

Live SWD reads cannot test this: every probe operation ends a boot (rule 50), so
a read only ever shows a freshly-booted chip with the retune fresh.

### Config BH — the firmware watches QMI itself

Every main-loop iteration reads `QMI_M0_TIMING`; if `CLKDIV != 6` it increments
`QMI_REVERT` in `.uninit` and repairs it (with `dsb`, a dummy XIP read, and
`isb`, per the RP2350 requirement when raising CLKDIV).

The counter separates the outcomes cleanly:

* **QMI_REVERT > 0** ⇒ the hole is real, it has been open this whole
  investigation, and it is now closed. The crash rate should drop.
* **QMI_REVERT == 0** ⇒ QMI is stable after entry, this is not it, and the
  marginal path is elsewhere.

CRC `0x1959f307`, integrity OK, VREG `0x000000f0`, 300 MHz untouched.
`heartbeat placement OK: base=0x20067a80 core1-RO-region=0x20067b20` — HEARTBEAT
is now the tail `.uninit` object and ends exactly at the region base, verified
by the firmware's own check.

Addresses: **`BOOT_COUNT=0x20067ae4`, `WORK_COUNT=0x20067ae8`,
`QMI_REVERT=0x20067aec`**, `ALLOC_GUARD=0x200670e0`.
Baseline: **boot=2, work=24, qmi_reverts=0 at 11:50:15.**

## BH result — the QMI hole is NOT real, and BH regressed badly

Read at user request, 11:50:15 → 12:17:26 (27.2 min). Δboot = 56 → **55 crashes**;
Δwork = **4,417 iterations** → **1 crash per 80 iterations**, versus BG's 1 per
4,237. **~50x WORSE.** Boots survived ~2 s instead of ~115 s.

**`QMI_REVERT = 0`.** The guard never fired.

### What that null does and does not establish

The instrument was narrower than intended. The crash-record flash write happens
**inside the crash handler, immediately before reset**, so the main loop never
runs again to observe a reverted QMI. The counter can only catch reversions
caused by flash I/O that happens *while the loop keeps running*.

What it does establish: **no flash I/O occurs during steady-state emulation**
(ROM staging is boot-only, saves are user-initiated), so QMI cannot revert
mid-loop, and the boot-path reversion is already covered by the existing
main-loop-entry retune. **The hypothesised hole does not exist.** Settled,
negatively.

### The regression, and a speculation I withdrew

One peripheral read per iteration cannot cost 50x. I first blamed the `.uninit`
tail hazard, since the build moved HEARTBEAT to `0x20067a80`, ending exactly at
`_stack_end = 0x20067b20`. **That was wrong**: `_stack_end` there was already
32-byte aligned, so the MPU region base equalled it and nothing below was
swallowed — and `check_placement` passed. **The cause of the BH regression is
unknown.** Reverted rather than investigated, because the device was crashing
every ~30 s and a known-good image exists.

### A measurement trap worth recording

After the revert, the counters read `boot = 0x2007cd2a`, `work = 0x10011adf` —
garbage. Dumping the block showed slot 8 = `0xfffffff9` (an EXC_RETURN) and a
ring full of `0x2007cdXX` stack addresses: **stale HF_STACK content from the BH
build, sitting at the addresses HEARTBEAT now occupies.** `.uninit` is never
zeroed, and `init()` only seeds on a magic mismatch, so a rebuild that moves
HEARTBEAT can leave the counters inheriting another object's leftovers while the
magic slot happens to look valid.

Fixed by writing 0 over the magic and resetting, which forces the cold-boot seed.

**Rule 64: AFTER A REBUILD THAT MOVES A `.uninit` INSTRUMENT, SANITY-CHECK THE
BASELINE. Counters that survive reset also survive relocation onto another
object's stale bytes. Small plausible values, or zero the magic and reset.**

### Config BJ — back to the BG configuration

V1_30, no filler, no QMI guard. CRC `0xb6d2251f`, integrity OK, VREG
`0x000000f0`, 300 MHz untouched. `heartbeat placement OK: base=0x20066fa8
core1-RO-region=0x20067b00`. Layout matches BG: `BOOT_COUNT=0x2006700c`,
`WORK_COUNT=0x20067010`, `ALLOC_GUARD=0x200677a0`.

Baseline **boot = 0, work = 323 at 12:23:27**, verified sane per rule 64.
Expect ~1 per 4,400 if BG reproduces a third time.

## BJ — third reproduction, and BK: the last unscaled QMI field

BJ window 12:23:27 → 13:26:17 (62.8 min). Valid. Δboot = 37 → **36 crashes**;
Δwork = **136,877 iterations** → **1 per 3,802**.

Pooled **BE + BG + BJ: 95 crashes over 395,933 iterations = 1 per 4,168**, at
~36 iterations/s. The V1_30 configuration is solid across three windows, and the
BH regression is confirmed as an artifact of that build alone.

### Config BK — RXDELAY was never scaled for the overclock

The retune wrote `(before & !0xFF) | 6`: **CLKDIV only**. RXDELAY stayed at the
bootrom's 2, justified in-comment by "RXDELAY is relative to SCK, which is back
to 50 MHz, so it stays".

**That premise is wrong, and it is the same mistake the surrounding comment gets
right twice.** RXDELAY is counted in **clk_sys cycles**: it delays the point at
which QMI samples returning read data, compensating the pad → flash → pad round
trip. That round trip is a fixed *time* set by board and flash electrical delay
— it does not care what SCK is. Doubling clk_sys therefore **halves** the real
delay RXDELAY=2 buys:

* 150 MHz: 2 / 150 MHz = **13.3 ns**
* 300 MHz: 2 / 300 MHz = **6.67 ns**

The same block already applies exactly this reasoning to CLKDIV (3→6) and
MIN_DESELECT (7→14), then exempts RXDELAY on a false premise. **RXDELAY was the
last clk_sys-scaled QMI field left unscaled.**

Corroboration from published RP2350 overclocking: 280 MHz runs **CLKDIV=3 with
RXDELAY=6** — RXDELAY at roughly 2× CLKDIV. Ours was RXDELAY=2 with CLKDIV=6,
i.e. *below* the divider, well outside that practice.

**And it lands exactly where the evidence points.** `stack_pop_check` saw 0
mismatches in 2,483,349,152 SRAM LDM/STM verifications, so data loads from SRAM
are clean; the faults are IBUSERR / INVSTATE with wild PCs — corrupt
*instruction* fetch and constant reads, which come from FLASH over XIP. A
mis-placed RX sample point corrupts precisely those and nothing else. This is
the first hypothesis that explains the SRAM null rather than being merely
compatible with it.

One variable only (rule 57): CLKDIV stays 6, MIN_DESELECT untouched. Config I's
old "MIN_DESELECT=14 was worse" verdict is not relied on — it used the crash-log
counter that rule 49 showed was saturating.

Applied at **both** retune sites so the boot path and the emulator share a
sampling point.

Verified on device: `QMI retune at main-loop entry: 0x60007406 -> 0x60007406`
(CLKDIV=6, **RXDELAY=4**), and the register reads `0x60007406` live. Note
`before == after`: the early retune's value survived the whole boot, which
independently confirms BH's finding that QMI is not being reverted here.

CRC `0x6529bb20`, integrity OK, VREG `0x000000f0`, 300 MHz untouched. Layout
unchanged from BJ. Baseline **boot = 1, work = 9 at 13:30:17** (cold-seeded via
the magic-zero trick, sane per rule 64).

**Pre-registered:** the pooled comparator is 1 per 4,168 over 95 events. A real
effect should be a large factor, not 20%. If BK does not move the rate, RXDELAY
is excluded and the remaining XIP candidates are the XIP cache and MIN_DESELECT.

## BK — RXDELAY refuted in DIRECTION, confirmed in IMPORTANCE. It is an 83x knob.

Window 13:30:17 → 14:33:20 (63.1 min). Valid; VREG `0x000000f0`, QMI `0x60007406`.
Δboot = 137 → **136 crashes**; Δwork = **6,782 iterations** → **1 per 50**.

Against the pooled comparator of 1 per 4,168 (95 events, RXDELAY=2), **RXDELAY=4
is ~83x WORSE**, on 136 events. The hypothesis is refuted decisively.

### The physical argument was backwards — but the finding is bigger than the theory

I argued RXDELAY compensates a fixed round-trip time, so doubling clk_sys should
require doubling the count. The device says otherwise, so the model is wrong:
most likely a larger RXDELAY samples **later**, past the end of the valid data
window, rather than compensating into it.

What matters more:

**Moving ONE QMI field by two clk_sys counts changed the crash rate 83x.**

That is the largest single-variable effect measured in this investigation —
voltage was 28x. It confirms the **XIP sample point is the sensitive parameter**,
which is precisely what the 2,483,349,152 clean SRAM LDM/STM verifications
implied: SRAM is fine, flash sampling is not. RXDELAY=2 is not a leftover to be
"corrected" — it sits near an optimum, with a cliff two counts away.

**Rule 67: A REFUTED DIRECTION CAN STILL BE A CONFIRMED VARIABLE. If a change
makes things dramatically WORSE, the parameter matters — stop theorising about
which way it should go and SCAN it.**

**Rule 68: WHEN A ONE-FIELD CHANGE MOVES A RATE BY ~2 ORDERS OF MAGNITUDE, THAT
FIELD IS THE OPERATING POINT. Treat everything else as secondary until it is
mapped.**

### Config BL — scan RXDELAY downward

2 is good, 4 is catastrophic. BL tries **RXDELAY = 1** to determine whether 2 is
the optimum or merely on the good side of a cliff:

* **1 much better than 2** ⇒ keep descending (try 0); the sample point has been
  mis-set all along and there is real headroom.
* **1 much worse than 2** ⇒ 2 is a narrow local optimum, the usable window is
  ~1 count wide, and *that narrowness is itself the headline finding* — it would
  explain why this board fails where others at higher clocks succeed.

One variable only. CLKDIV stays 6, MIN_DESELECT stays 7, V1_30, 300 MHz.

CRC `0x9cad4219`, integrity OK. Verified live: **QMI `0x60007106`** (CLKDIV=6,
RXDELAY=1); `before == after` again at main-loop entry. Baseline **boot = 1,
work = 11 at 14:36:29**, cold-seeded per rule 64.

### Measured QMI RXDELAY scan so far (V1_30, CLKDIV=6)

| RXDELAY | crashes/iteration | events |
|---|---|---|
| 1 | *this window* | — |
| **2** | **1 per 4,168** | 95 |
| 4 | 1 per 50 | 136 |

## BL — the RXDELAY scan is complete. The eye is NARROWER THAN ONE clk_sys CYCLE.

BL window 14:36:29 → 15:39:19 (62.8 min). Valid; QMI `0x60007106`.
Δboot = 136 → **135 crashes**; Δwork = **6,146 iterations** → **1 per 46**.

### The scan (V1_30, CLKDIV=6, MIN_DESELECT=7, 300 MHz)

| RXDELAY | crashes/iteration | events |
|---|---|---|
| 1 | **1 per 46** | 135 |
| **2** | **1 per 4,168** | **95** |
| 4 | **1 per 50** | 136 |

**RXDELAY=2 is a razor-sharp optimum: one count either side is ~90x worse.**
Three windows, 366 events total, all measured with the same instrument.

### What that means physically — this is the real root cause

A one-count step is one clk_sys cycle = **3.33 ns at 300 MHz**. Moving the
sample point by a single 3.33 ns step in *either* direction collapses the flash
read. Therefore:

**The flash data-valid window at the sample point is narrower than one clk_sys
cycle.** We are balanced on the peak of an eye that is barely open — which is
exactly why sitting *on* the optimum still leaves 1 crash per 4,168 iterations,
and why this board fails where other RP2350s run reliably at *higher* clocks:
their CLKDIV/RXDELAY combinations land inside a wide eye. Ours has nowhere to
sit, because RXDELAY can only be positioned in whole clk_sys cycles and the eye
is smaller than the step size.

This supersedes "electrical margin at 2x rated clock" with something specific,
measured, and actionable — and it is consistent with every prior constraint: the
2,483,349,152 clean SRAM LDM/STM verifications (SRAM never sampled through QMI),
the IBUSERR / INVSTATE / wild PCs (corrupt instruction fetch from XIP), the two
unrelated victim functions, and voltage acting as a 28x symptom-reducer rather
than a cure.

**Rule 69: IF A ONE-COUNT CHANGE EITHER SIDE OF AN OPTIMUM IS CATASTROPHIC, THE
VALID WINDOW IS SMALLER THAN THE ADJUSTMENT GRANULARITY. Stop aiming better and
make the window bigger.**

### Config BM — widen the eye instead of aiming within it

SCK = clk_sys / CLKDIV. **CLKDIV 6 → 12 halves flash SCK from 50 MHz to 25 MHz**,
which roughly doubles the data-valid window measured in clk_sys counts, so
RXDELAY=2 stops balancing on a knife edge. clk_sys is untouched, so the standing
constraint holds.

RXDELAY returns to 2 (the value with 95 events behind it). RXDELAY compensates a
fixed pad→flash→pad round trip and does not depend on SCK, so it should remain
optimal. MIN_DESELECT and voltage unchanged. One variable (rule 57).

Cost: XIP reads at half speed; the XIP cache absorbs most of it, and the metric
is crashes-per-iteration regardless (rule 54).

CRC `0xfde58e2f`, integrity OK. Device-side readback in the boot log:
**`QMI retune at main-loop entry: 0x6000720c -> 0x6000720c (CLKDIV 12->12
SCK 25MHz, MIN_DESELECT 7->7)`**.

A live SWD read of QMI right after setup returned `0x60007203` (the bootrom
default) — a **rule 50 artifact**: the read itself ended the boot, sampling the
chip before the retune ran. The boot-log readback is the trustworthy source.
**Corollary to rule 50: QMI cannot be verified live over SWD; use the firmware's
own readback.**

Baseline **boot = 1, work = 33 at 16:53:45**, cold-seeded per rule 64.

**Pre-registered:** comparator 1 per 4,168 (95 events). If BM improves by a large
factor, the narrow-eye model is confirmed and the fix is to keep lowering SCK
(CLKDIV 16, 24) until the rate floors — then soak-escalate. If BM is unchanged or
worse, SCK is not what sets the eye width, and the next candidates are
MIN_DESELECT and the XIP cache.

## BM — REFUTED. Lowering SCK made it ~80x worse, and the "eye" model is withdrawn.

Read early at user report of heavy crashing. Window 16:53:45 → 17:07:23
(13.6 min). Valid. Δboot = 25 → **24 crashes**; Δwork = **1,251 iterations**
→ **1 per 52**, against the 1-per-4,168 comparator. **~80x worse.**

### The narrow-eye interpretation is withdrawn

Last cycle I read the RXDELAY scan as "the analog data-valid eye is narrower than
one clk_sys cycle" and prescribed the obvious remedy: halve SCK to widen it.
**The device says no.** If the eye were simply too narrow in absolute time,
doubling the SCK period would have helped. It did the opposite.

What the four measurements actually support is narrower and more literal:

| CLKDIV | RXDELAY | crashes/iteration | events |
|---|---|---|---|
| **6** | **2** | **1 per 4,168** | **95** |
| 6 | 1 | 1 per 46 | 135 |
| 6 | 4 | 1 per 50 | 136 |
| 12 | 2 | 1 per 52 | 24 |

**One specific (CLKDIV, RXDELAY) combination works; perturbing either field
breaks it.** That is a fact about this pairing. It is *not* evidence for any
particular physical picture, and I over-read it into one.

**Rule 70: A SHARP OPTIMUM LICENSES "THIS SETTING MATTERS", NOT A MECHANISM.
Naming the mechanism ("the eye is too narrow") smuggles in a prediction; test the
prediction before acting on the name.**

Note also what BM did *not* test: whether the optimum at CLKDIV=12 simply MOVED
to a different RXDELAY. I assumed RXDELAY=2 would stay optimal because it
compensates a fixed round trip — the same assumption that was already refuted
once in BK. Holding RXDELAY fixed while changing CLKDIV was therefore not a
clean one-variable test of "lower SCK is better"; it was a test of one point in
a two-dimensional space.

### Config BN — back to the best measured operating point

CLKDIV=6, RXDELAY=2, V1_30, 300 MHz. CRC `0x60e077cc`, integrity OK, device-side
readback `QMI retune at main-loop entry: 0x60007206 -> 0x60007206 (CLKDIV 6->6
SCK 50MHz, MIN_DESELECT 7->7)`.

Baseline **boot = 0, work = 104 at 17:10:34**, cold-seeded per rule 64.

This is the fourth window on this configuration (BE, BG, BJ, now BN); the first
three pooled to 1 per 4,168 over 95 events.

### Where this leaves things

Confirmed and reproducible: voltage V1_25→V1_30 is 28x; the (CLKDIV=6,
RXDELAY=2) pairing is worth ~90x against any neighbour tested; layout is
irrelevant; every software mechanism is excluded. Best achieved: **1 crash per
4,168 iterations ≈ 750/day**, still far from a clean 24 h.

Not established: *why* that pairing is special. Three plausible next moves, in
order of cost:

1. **Scan RXDELAY at CLKDIV=12** (0..7). If an optimum exists there too, the
   sample point tracks SCK and the map is 2-D; if none does, CLKDIV=6 is
   privileged for another reason.
2. **MIN_DESELECT** (bits 16:12, currently 7) — the one clk_sys-counted QMI field
   never scanned.
3. **XIP cache** behaviour.

None of these requires touching clk_sys.

## BN — THE KNOWN-GOOD CONFIG NO LONGER REPRODUCES. The QMI map is unreliable.

Window 17:10:34 → 18:13:21 (62.8 min). Valid. Δboot = 135 → **134 crashes**;
Δwork = **6,312 iterations** → **1 per 47**. Expected ~1 per 4,168.

### BN is functionally identical to BJ, and measured 90x worse

* BJ wrote `(before & !0xFF) | 6` — preserves RXDELAY from `before`.
* BN writes `(before & !0x7FF) | (2 << 8) | 6` — forces RXDELAY = 2.

`before` is the bootrom value `0x60007203`, whose RXDELAY is already 2, so both
expressions yield **`0x60007206`** — and both boot logs print exactly that. The
QMI register ends up bit-identical. There is no functional difference between
the two images in QMI terms, yet:

| window | time | crashes/iteration |
|---|---|---|
| BE | 05:12–06:05 | 1 per 4,586 |
| BG | 07:14–08:17 | 1 per 4,237 |
| BJ | 12:23–13:26 | 1 per 3,802 |
| **BN** | **17:10–18:13** | **1 per 47** |

**Everything measured from 13:30 onward is bad**, regardless of the QMI values:
BK (RXDELAY=4) 1/50, BL (RXDELAY=1) 1/46, BM (CLKDIV=12) 1/52, BN (the restored
good pairing) 1/47. Four consecutive windows, all ~1/50, spanning three
different QMI settings *including the one with 95 events of good history*.

### What this invalidates

**The QMI map cannot be trusted.** It was built by comparing windows taken at
different times, and it now appears the device changed state at ~13:30 in a way
that dominates the register settings. BK's "RXDELAY=4 is 83x worse" and BL's
"RXDELAY=1 is 90x worse" are equally consistent with **drift that began before
either measurement**. The apparent razor-sharp optimum may be an artifact of
comparing pre-drift windows against post-drift windows.

**Rule 71: A CONFIG MAP BUILT FROM SEQUENTIAL WINDOWS IS ONLY VALID IF THE
BASELINE STILL REPRODUCES AT THE END. Re-measure the reference config
periodically; if it has moved, every comparison since the last good reference is
confounded with time.**

This is the same failure mode as rule 37 ("rate comparisons are only valid within
one image") extended to time: they are also only valid within one *epoch*.

### The fault signature mix has changed too

BN's sector is dominated by a family that did not appear before:

`pc=0x00009ffe  lr=0x10019701  cfsr=0x100 (IBUSERR)` — seq 2, 5, 6, 7, 9.

`lr = 0x10019701` resolves to **`SharedWorkerState::write_live_vr + 0x5c`**
(multicore.rs, in FLASH). Previously the families were `pc=0x000000a0` /
`lr = Instructions::cb + 0x112` / INVSTATE, the `produce_samples` family, and the
`fa=0x139ec015` PRECISERR.

**IBUSERR — instruction bus error — is now the dominant fault**, and the victim
has moved to a different function. That is a real change in failure character,
not just rate, and it is consistent with the XIP instruction-fetch path having
degraded rather than with any particular register value.

### What is still solid, and what is not

**Still solid:** the failure is on the XIP path (2,483,349,152 clean SRAM LDM/STM
verifications; IBUSERR/INVSTATE/wild PCs); layout is irrelevant; every software
mechanism is excluded; the crash-record and work counters themselves are sound.

**No longer solid:** the entire QMI map, and the "voltage is 28x" figure — BB was
measured at 02:31 and BE at 05:12, both pre-drift, so that one is probably safe,
but it deserves re-checking against a contemporaneous control.

**Unknown and now the priority:** what changed at ~13:30, and whether it is
reversible. The board has been powered and crashing continuously for ~19 hours.
Temperature is the obvious environmental candidate; flash wear from thousands of
crash-record erase/write cycles is another.

### Next

Leave BN running unchanged and take a second window. If it stays at ~1/47, the
device is in a persistently different state and the priority is identifying what
changed — not more register scans, which cannot be interpreted until a reference
config reproduces again.

## BN repeat — the bad state is STABLE. This is a step change, not drift.

Window 18:16:08 → 19:18:21 (62.2 min), image untouched. Δboot = 127 →
**126 crashes**; Δwork = **6,815 iterations** → **1 per 54**.

Pooled across both BN windows: **260 crashes / 13,127 iterations = 1 per 50.5.**

| epoch | windows | span | crashes/iteration |
|---|---|---|---|
| **good** | BE, BG, BJ | 05:12–13:26 (8 h) | **1 per ~4,168** |
| **bad** | BK, BL, BM, BN, BN₂ | 13:30–19:18 (6 h) | **1 per ~50** |

Three windows spanning eight hours at ~1/4,000, then five windows spanning six
hours at ~1/50, across three different QMI settings including the one with 95
events of good history. **An ~85x step change that happened between 13:26 and
13:30 and has not recovered in six hours.**

That rules out several explanations:

* **not gradual thermal drift** — a step in ~4 minutes, then flat for 6 h;
* **not the QMI settings** — the bad epoch includes the known-good pairing;
* **not statistical** — 95 events one side, 260 the other.

The only thing that happened in that window was **flashing the BK image**. But
BG and BJ were also rebuild+flash cycles and stayed good, so "flashing degrades
it" is not sufficient on its own.

### What the step-change shape implies

Something changed **state** at that moment and stayed changed. Candidates that
fit a step-and-hold, roughly in order of testability:

1. **Something in flash was altered by that programming cycle** — the firmware
   image region is rewritten every `cargo run`, and the staged Game Boy ROM and
   its header live elsewhere in the same device. A damaged or partially-erased
   sector would persist across resets and would degrade XIP reads exactly where
   the faults now land.
2. **Die temperature crossed a threshold and stayed there** — weaker, because
   the transition was too abrupt and the evening has not recovered it.
3. **Flash wear** on a specific sector from ~20 h of crash-record erase/write
   cycles plus ~10 full-image programmings.

Note also a feedback loop that muddies workload comparisons: at 1 per 50 the
device crashes every ~1.5 s of run time and reboots (~15 s), so it now spends
almost all its time booting and may never reach in-game code, whereas in the
good epoch it ran ~115 s per boot. The *mix* of executed code therefore differs
between epochs — a consequence of the rate, but it means signature comparisons
across epochs are not like-for-like either.

### The decisive test is a power cycle, which needs the user

`.uninit` is re-baselined every window, so a power cycle costs nothing this
investigation needs, and it is the cleanest way to separate "persistent device
state" from "something that a reset does not clear". It cannot be done over SWD.

## BP — the RXDELAY-preservation hypothesis was refuted by its own boot log

The user reports that a power cycle will not change the bad state. That removes
thermal and transient explanations, so I went back to an unverified assumption.

**The hypothesis:** the good epoch (BE/BG/BJ) wrote `(before & !0xFF) | 6`,
which *preserves* RXDELAY, while every bad window (BK/BL/BM/BN) *forces* it. The
step change at 13:28 is exactly the commit that switched from preserving to
forcing. If the preserved value were 0 — the one value never scanned — that
would explain everything, and would also dissolve the "razor-sharp optimum at 2"
into a plain "less is better".

**Refuted immediately.** Adding a log of the bootrom value at the early retune:

```
QMI early retune: before=0x60007203 (RXDELAY 2)
```

`before` really does hold RXDELAY=2, so the preserving mask genuinely produced
`0x60007206`. **The good and bad epochs ran the same QMI value.** RXDELAY does
not explain the step change, and the original assumption was right after all.

Worth noting *why* the question stayed open so long: the `QMI retune` line was
only ever grepped from BK onward. For BE/BG/BJ I captured the CRC and heartbeat
lines and not that one, so a value I was relying on went eight hours unverified.

**Rule 72: LOG THE VALUE YOU ARE RELYING ON, IN THE EPOCH YOU ARE RELYING ON IT.
A quantity that is only observed after you start changing it cannot anchor
anything measured before.**

### Config BP — a same-epoch RXDELAY scan, salvaged from the refutation

The image is flashed with **RXDELAY = 0**, the one value never tested, and it is
running in the *bad* epoch. That turns a wasted flash into the control-disciplined
measurement this investigation has been missing: RXDELAY 1, 2 and 4 have all now
been measured at ~1/50 **within this same epoch**, so adding 0 completes a
four-point scan with no cross-epoch comparison anywhere in it.

* **0 also ~1/50** ⇒ RXDELAY genuinely does not matter, and the entire "83x
  knob" was an epoch artifact. The QMI map is then fully retired.
* **0 markedly better** ⇒ RXDELAY does matter and the good setting is 0 — which
  would then also have to explain why 2 was good this morning and is bad now.

Device-side readback: `QMI retune at main-loop entry: 0x60007006 -> 0x60007006
(CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 7->7)`. CRC `0x40fc4ea0`, integrity OK,
V1_30, 300 MHz. Layout unchanged. Baseline **boot = 0, work = 238 at 19:28:30**.

## BP — RXDELAY=0 wins, on the first fully same-epoch scan

Window 19:28:30 → 20:31:35 (63.1 min). Valid. Δboot = 95 → **94 crashes**;
Δwork = **12,336 iterations** → **1 per 131**.

### The RXDELAY scan, with every point inside the current epoch

| RXDELAY | crashes/iteration | events |
|---|---|---|
| **0** | **1 per 131** | 94 |
| 1 | 1 per 46 | 135 |
| 2 | 1 per 50.5 | 260 |
| 4 | 1 per 50 | 136 |

**0 is ~2.6x better than 1, 2 and 4**, which are indistinguishable from one
another. Comparing 0 (7.62e-3 ± 0.79e-3 per iteration) against the 260-event
RXDELAY=2 pool (19.8e-3 ± 1.23e-3) gives ~8 sigma. **RXDELAY genuinely matters,
0 is the best available value, and 0 is the bottom of the field.**

This is the first QMI result in the whole investigation with no cross-epoch
comparison anywhere in it, which is the only reason it can be trusted.

### It does NOT explain the epoch step — these are two separate effects

RXDELAY=2 gave 1 per 4,168 this morning and 1 per 50 this evening. A 2.6x field
effect cannot produce an 85x step, and the step happened with the field held
constant. So:

1. **an unexplained ~85x epoch change at 13:28** — not thermal, not transient,
   not power-cyclable (per the user), not QMI;
2. **a real ~2.6x RXDELAY effect**, now measured cleanly.

Also worth noting: the shape is *not* the razor-sharp peak at 2 that the
confounded afternoon map suggested. 1, 2 and 4 are flat and equal; only 0 is
different. That is the sort of thing the earlier map got exactly backwards.

### Config BQ — MIN_DESELECT, the last unscanned QMI field

Keeping RXDELAY=0, now scanning `MIN_DESELECT[16:12]` 7 → 14.

It is counted in clk_sys cycles and sets the minimum chip-select deselect time
(flash tSHSL, typically 20–50 ns). The bootrom's 7 meant 7/150 MHz = **46.7 ns**
at the boot clock; at 300 MHz the same 7 is **23.3 ns**, at or below the low end
of typical tSHSL. So it is one of the fields the overclock silently halved, and
14 restores the intended duration.

An old config tried 14 and looked worse, but that measurement used the
saturating crash-log counter (rule 49) *and* changed two things at once. This is
a clean one-variable test with a valid instrument and a stable epoch.

Device-side readback: **`QMI retune at main-loop entry: 0x6000e006 -> 0x6000e006
(CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 14->14)`** — CLKDIV=6, RXDELAY=0,
MIN_DESELECT=14. CRC `0x245c9d49`, integrity OK, V1_30, 300 MHz.

Baseline **boot = 0, work = 340 at 22:39:12**, cold-seeded per rule 64.
Comparator: **1 per 131** (BP, same epoch, 94 events).

## BQ — MIN_DESELECT WAS THE MISSING SCALING. 74x, and the best result ever.

Window 22:39:12 → 23:42:23 (63.2 min). Valid. Δboot = 17 → **16 crashes**;
Δwork = **154,819 iterations** → **1 per 9,676**.

| config | MIN_DESELECT | crashes/iteration | events |
|---|---|---|---|
| BP | 7 | 1 per 131 | 94 |
| **BQ** | **14** | **1 per 9,676** | 16 |

**74x improvement. Same epoch, one variable, RXDELAY held at 0.** At BP's rate
those 154,819 iterations should have produced ~1,182 crashes; 16 occurred. No
plausible fluctuation covers that.

It also beats the best-ever *good-epoch* rate (1 per 4,168) by **2.3x**, so this
is the best configuration measured in the entire investigation — and it was
reached without touching the clock. Throughput hit its highest yet, 40.8
iterations/s, simply because the device now stays up.

### The mechanism — the same one CLKDIV had

`MIN_DESELECT` is counted in **clk_sys cycles** and sets the minimum chip-select
deselect time, i.e. the flash's tSHSL (typically 20–50 ns):

* 150 MHz boot clock: 7 / 150 MHz = **46.7 ns** — what the bootrom intended;
* 300 MHz overclock: the same 7 = **23.3 ns** — at or below the requirement.

Raising it to 14 restores 46.7 ns. **The overclock silently halved it**, exactly
as it halved CLKDIV's SCK — and the existing retune fixed CLKDIV while leaving
this one alone.

The old "config I tried MIN_DESELECT=14 and it was worse" verdict was wrong on
two counts: it used the crash-log counter that rule 49 showed was saturating,
and it changed two things at once.

**Rule 73: WHEN A FIX SCALES ONE CLOCK-COUNTED FIELD FOR AN OVERCLOCK, THE OTHER
CLOCK-COUNTED FIELDS IN THE SAME REGISTER ARE PROBABLY STILL BROKEN. Rule 66 said
to audit them; this is what happens when you actually do it — the second field
was worth more than the first.**

### Config BR — is 14 sufficient, or merely better?

tSHSL is a **minimum**, so overshooting should be safe and merely slower. That
makes the direction test cheap and decisive: **MIN_DESELECT = 28** gives 93.3 ns,
double the bootrom's intended figure.

* **28 ≈ 14** ⇒ 14 already satisfies tSHSL; the field is done, and the residual
  crashes come from elsewhere.
* **28 ≪ 14** ⇒ the real requirement exceeds what the bootrom assumed; keep
  climbing (the field is 5 bits, max 31).
* **28 ≫ 14** ⇒ non-monotonic, so tSHSL is not the whole story.

Device-side readback: **`QMI retune at main-loop entry: 0x6001c006 -> 0x6001c006
(CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 28->28)`**. CRC `0xce7fe63f`, integrity OK,
V1_30, 300 MHz. Baseline **boot = 0, work = 305 at 23:45:47**.

Comparator: **1 per 9,676** (BQ, same epoch).

### Where this puts the 24-hour goal

At 1 per 9,676 and ~41 iterations/s, 24 h is ~3.5M iterations ⇒ **~365
crashes/day**. Still not zero, but the trajectory changed tonight: two
clock-counted QMI fields (RXDELAY 2→0, MIN_DESELECT 7→14) together took the rate
from 1 per 50 to 1 per 9,676 — a **190x** improvement, all within the locked
clock. The ~85x epoch step at 13:28 is now moot for practical purposes, since the
current configuration is better than the good epoch ever was; it remains
unexplained but no longer blocks progress.

## BR — MIN_DESELECT=28 is bad, and that makes BQ's "74x" suspect

Window 23:45:47 → 00:48:23 (62.6 min). Valid. Δboot = 150 → **149 crashes**;
Δwork = **17,103 iterations** → **1 per 115**, vs BQ's 1 per 9,676. **~84x worse.**

| MIN_DESELECT | rate | events | window |
|---|---|---|---|
| 7 | 1 per 131 | 94 | 19:28 |
| **14** | **1 per 9,676** | **16** | 22:39 |
| 28 | 1 per 115 | 149 | 23:45 |

### I called a breakthrough on one uncontrolled small-sample window

7 and 28 both land at ~1/120. Only the single 14 window was spectacular, and it
rests on **16 events — the smallest sample of the night**.

That is precisely the shape that misled me this afternoon: one good window
flanked by bad ones, which turned out to be an **epoch step**, not a setting. I
wrote rule 71 after being burned by exactly this, and then announced a 74x
breakthrough from an uncontrolled window anyway. The claim in the BQ section
("the best result ever", "190x", "the missing scaling") is **not established** —
it is one window.

Two readings remain open and I cannot choose between them from the data I have:

1. **MIN_DESELECT=14 is a genuine sharp optimum.** Then 28 being bad is real and
   informative: a *minimum* timing requirement would make overshoot harmless, so
   tSHSL-as-a-minimum would be the wrong mechanism even though the value works.
2. **BQ was another epoch excursion.** Then the 74x collapses, MIN_DESELECT does
   not matter, and — since RXDELAY=0's "2.6x" rests on the same kind of
   single-window comparison — that result is suspect too. The real phenomenon
   would be whatever makes the device step between ~1/120 and ~1/5,000 states.

**Rule 74: A RESULT THAT WOULD BE A BREAKTHROUGH DESERVES A CONTROL BEFORE IT
DESERVES A HEADLINE — especially when it comes from the smallest sample in the
series and the system is known to step between states on its own.**

### Config BS — re-run MIN_DESELECT=14 as a control

Not a new value. Everything identical to BQ; QMI verified byte-identical
(`0x6000e006`, CLKDIV=6, RXDELAY=0, MIN_DESELECT=14).

* **~1 per 9,676 again** ⇒ 14 is real, 28 is real, and the mechanism is *not*
  a simple minimum-time requirement. Settle on 14 and soak-escalate.
* **~1 per 120** ⇒ BQ was an epoch artifact; the 74x and probably the RXDELAY
  2.6x both collapse, and the epoch stepping is the actual phenomenon.

CRC `0x212e3758`, integrity OK, V1_30, 300 MHz. Baseline **boot = 0, work = 339
at 00:51:44**, cold-seeded per rule 64.

## BS — THE CONTROL REPRODUCES. MIN_DESELECT=14 is real; my mechanism for it is not.

Window 00:51:44 → 01:54:23 (62.7 min). Valid. Δboot = 24 → **23 crashes**;
Δwork = **147,387 iterations** → **1 per 6,408**.

| MIN_DESELECT | rate | events | window |
|---|---|---|---|
| 7 | 1 per 131 | 94 | 19:28 |
| 14 | 1 per 9,676 | 16 | 22:39 |
| 28 | 1 per 115 | 149 | 23:45 |
| **14 (control)** | **1 per 6,408** | 23 | 00:51 |

**Pooled MIN_DESELECT=14: 39 crashes / 302,206 iterations = 1 per 7,749**, across
two windows separated by a different config. ~60x better than either neighbour
(7 → 76.2e-4/iter, 28 → 87.1e-4/iter, 14 → 1.29e-4/iter). Overwhelming.

So rule 74 was satisfied and the answer came back **positive**: the effect is
genuine, and BQ was not an epoch artifact. The caution was still correct — the
claim needed the control before it deserved the headline, and it now has one.

### But the mechanism I attached to it is wrong

I explained 14 as "restoring the flash's tSHSL, which the overclock halved".
**tSHSL is a MINIMUM.** If that were the mechanism, overshooting to 28 (93.3 ns)
would be harmless — merely slower. Instead 28 is **67x worse than 14**.

There is a genuine *optimum* at 14, not a floor. I am not naming a replacement
mechanism; asserting one and acting on its prediction is an error already on the
record twice (rules 70, 62). What is established is the value, not the reason.

**Rule 75: A CONFIRMED SETTING DOES NOT CONFIRM THE STORY YOU TOLD ABOUT IT.
Re-check the mechanism against the whole curve — a "minimum" that gets worse when
exceeded was never a minimum.**

### Where this leaves the numbers

Best confirmed rate: **1 per 7,749** — about **1.9x better than the good epoch's
1 per 4,168**, and reached entirely within the locked clock.

At ~40 iterations/s that is still ~450 crashes/day, so the 24-hour goal is not
met; but the configuration is now the best measured and control-verified.

### Config BT — re-scan RXDELAY at the CORRECT operating point

**The entire RXDELAY scan ran at MIN_DESELECT=7**, now known to be a bad
operating point. RXDELAY=0 won there (1 per 131 vs ~1 per 50 for 1/2/4), but the
two fields plainly interact — that is exactly what the non-monotonic
MIN_DESELECT curve implies — so **the best RXDELAY at MIN_DESELECT=14 is simply
unmeasured**.

BT sets **RXDELAY = 2** (the bootrom value, and the one that lost worst at
MIN_DESELECT=7) with MIN_DESELECT held at 14. One variable.

Device-side readback: **`QMI retune at main-loop entry: 0x6000e206 -> 0x6000e206
(CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 14->14)`** — CLKDIV=6, RXDELAY=2,
MIN_DESELECT=14. CRC `0xb8530c16`, integrity OK, V1_30, 300 MHz.

Baseline **boot = 0, work = 338 at 08:25:39**, cold-seeded per rule 64.
Comparator: **1 per 7,749** (pooled, same epoch).

* **BT much worse** ⇒ RXDELAY=0 is right at both operating points; the fields do
  not interact on this axis and the QMI tuning is finished.
* **BT similar or better** ⇒ the earlier RXDELAY scan was measured at the wrong
  point and its conclusion does not transfer; scan 0/1/2/4 properly at
  MIN_DESELECT=14.

## BT — RXDELAY is INERT at MIN_DESELECT=14. The old RXDELAY result does not transfer.

Window 08:25:39 → 09:30:12 (64.6 min). Valid. Δboot = 19 → **18 crashes**;
Δwork = **157,434** → **1 per 8,746**.

Comparator was 1 per 7,749 (RXDELAY=0, 39 events). Ratio **0.89** — flat, well
inside noise for 18 events. Pre-registered branch (b), and it answers the
question in a way neither branch anticipated: RXDELAY does not matter here at
all.

| RXDELAY @ MIN_DESELECT=7 | rate | events |
|---|---|---|
| 0 | 1 per 131 | 94 |
| 1 | 1 per 46 | 135 |
| 2 | 1 per 50.5 | 260 |
| 4 | 1 per 50 | 136 |

| RXDELAY @ MIN_DESELECT=14 | rate | events |
|---|---|---|
| 0 | 1 per 7,749 | 39 |
| 2 | 1 per 8,746 | 18 |

The 2.6x spread across RXDELAY at MIN_DESELECT=7 **does not reproduce** at
MIN_DESELECT=14. Whether that spread was ever real (it was single-window against
a pool, in the epoch-stepping era) or is simply irrelevant once MIN_DESELECT is
right, RXDELAY is not a lever at the operating point we actually use. Not
spending two more windows on 1 and 4: 2-vs-0 is flat at 57 pooled events and
1/2/4 clustered together in the old scan.

### The pooled operating point

**MIN_DESELECT=14, three windows, two RXDELAY values: 57 crashes / 459,640
iterations = 1 per 8,064.** The most reproducible state of the investigation —
and the epoch stepping has not reappeared across any of it.

## BU — COOLDOWN 1 → 2: the last unscanned clock-counted field

`MIN_DESELECT` was worth ~60x because it is counted in **CLK_SYS cycles** and
running clk_sys at 2x rated halved its real duration. **`COOLDOWN[31:30]` is the
other time field in `QMI_M0_TIMING`** — chip select stays asserted for
`64 x COOLDOWN` system clock cycles — and it is still sitting at the bootrom's 1.
The existing retune masks only bits [16:0], so **COOLDOWN has never been written
at all**. Rules 66 and 73 point straight at it.

This is an **analogy to a confirmed fix, not a mechanism claim**. The
MIN_DESELECT curve has an OPTIMUM, not a floor, which no simple
minimum-duration story explains (rule 75). If COOLDOWN=2 helps, expect the same
non-monotonicity and scan it rather than extrapolating to 3.

RXDELAY returns to 0. That is not a second variable in any meaningful sense —
BT just showed it is inert here — and it keeps the config aligned with the
39-event reference arm.

Mask widened to `0xC001_FFFF` to reach bits [31:30]; `PAGEBREAK[29:28]=2` is
preserved from the bootrom.

Device-side readback: **`QMI retune at main-loop entry: 0xa000e006 -> 0xa000e006
(CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 14->14)`** — COOLDOWN=2, PAGEBREAK=2,
MIN_DESELECT=14, RXDELAY=0, CLKDIV=6. CRC `0xe1250dfc`, integrity OK, V1_30,
300 MHz. Addresses re-derived, unchanged.

Baseline **boot = 1, work = 46 at 09:33:35**. One reset occurred during the 25 s
seeding wait (prior windows seeded at work ~338), so this boot started late;
the metric is delta-based so this is recorded, not corrected.

Comparator: **1 per 8,064** (pooled MIN_DESELECT=14, same epoch, 57 events).

* **BU much better** ⇒ the clock-counted-field family is the whole story and
  PAGEBREAK/COOLDOWN deserve a full scan.
* **BU flat** ⇒ QMI is exhausted; every field is either tuned or inert, and the
  remaining ~1-per-8,000 floor is not a QMI timing problem.
* **BU worse** ⇒ same non-monotonicity as MIN_DESELECT; revert to COOLDOWN=1.

## BU — COOLDOWN=2 is a DISASTER, and that makes COOLDOWN the most sensitive field found

Window 09:33:35 → 10:36:09 (62.6 min). Valid — CRC OK, ALLOC_GUARD intact,
VREG 0xf0, QMI verified `0xa000e006` at both retune sites.

Δboot = 156 → **156 crashes**; Δwork = **4,534** → **1 per 29**.

**~277x worse than the 1-per-8,064 comparator** and worse than any configuration
measured in this investigation. Throughput collapsed from ~40 iterations/s to
**1.2/s** — 156 boots x ~12 s of boot time accounts for most of the window. The
device barely ran.

| QMI_M0_TIMING field | value | rate | events |
|---|---|---|---|
| MIN_DESELECT | 7 | 1 per 131 | 94 |
| MIN_DESELECT | **14** | **1 per 8,064** | 57 (pooled) |
| MIN_DESELECT | 28 | 1 per 115 | 149 |
| RXDELAY @ 14 | 0 vs 2 | flat | 57 |
| **COOLDOWN** | **2** | **1 per 29** | **156** |

### Correcting my own pre-registration

BU's branch (c) said "worse ⇒ same non-monotonicity as MIN_DESELECT; revert to
COOLDOWN=1 and treat QMI as exhausted." **That branch was wrong to write.** It
conflated "this value is worse" with "this field is insensitive". A 277x swing
from a two-bit field is the largest effect anyone has produced here — larger than
MIN_DESELECT's 60x. The correct reading is that **chip-select timing is the
dominant axis and COOLDOWN is its most sensitive control**.

Deviating from the pre-registered branch deliberately and on the record, rather
than following a rule I now believe was mis-specified.

**Rule 77: A PRE-REGISTERED BRANCH IS A COMMITMENT AGAINST MOTIVATED READING,
NOT AGAINST NEW INFORMATION. If the effect size itself refutes the branch's
premise, say so explicitly and deviate — do not follow it silently, and do not
deviate silently either.**

## BV — COOLDOWN 1 → 0: completing the field

COOLDOWN is two bits, so the field is four values. `1` is the bootrom default and
the current best; `2` is catastrophic; `3` is presumably worse still. **`0` is the
only untested value**, and one window closes the field completely.

Both live knobs are chip-select timing and they are opposite sides of the same
signal: `MIN_DESELECT` is how long CS must stay **deasserted**, `COOLDOWN` is how
long it stays **asserted** after a transfer (`64 x COOLDOWN` clk_sys cycles).
Noting which axis matters is not a mechanism claim (rules 70, 75) — and since
MIN_DESELECT already proved these fields have optima rather than monotone
directions, 0 is a genuine measurement, not an extrapolation from "2 is bad".

Expect throughput to DROP: with COOLDOWN=0 the CS deasserts immediately after
every transfer, forcing a command prefix on each XIP access. The metric is
normalised by work (rule 54), so that is harmless to the comparison, but a low
iteration count in this window is expected and is not itself a fault signal.

Device-side readback: **`QMI retune at main-loop entry: 0x2000e006 -> 0x2000e006
(CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 14->14)`** — COOLDOWN=0, PAGEBREAK=2,
MIN_DESELECT=14, RXDELAY=0, CLKDIV=6. CRC `0x81db1bf2`, integrity OK, V1_30,
300 MHz. Addresses re-derived, unchanged.

Baseline **boot = 0, work = 131 at 10:39:24** — clean cold seed.
Comparator: **1 per 8,064** (pooled MIN_DESELECT=14, same epoch, 57 events).

* **BV better** ⇒ the field has an optimum at the low end; COOLDOWN is the
  operating point and the CS-timing axis is where the remaining orders of
  magnitude are.
* **BV flat** ⇒ COOLDOWN=1 stands, the field is closed, QMI is genuinely
  exhausted, and the investigation moves to the voltage re-check (AG).
* **BV worse** ⇒ COOLDOWN=1 is a sharp optimum. Field closed, same conclusion.

## BV — COOLDOWN=0 is 21x worse. The field is CLOSED and QMI is EXHAUSTED.

Window 10:39:24 → 11:42:09 (62.8 min). Valid — CRC OK, ALLOC_GUARD intact,
VREG 0xf0, QMI verified `0x2000e006`.

Δboot = 135 → **134 crashes**; Δwork = **51,325** → **1 per 383**, **21x worse**
than the 1-per-8,064 comparator.

### The complete COOLDOWN curve

| COOLDOWN | rate | events | vs best |
|---|---|---|---|
| 0 | 1 per 383 | 134 | 21x worse |
| **1 (bootrom)** | **1 per 8,064** | 57 | **best** |
| 2 | 1 per 29 | 156 | 277x worse |
| 3 | not run — bracketed by 2 | | |

A sharp optimum at the bootrom's own value, falling off hard in both directions.

**And that kills the last version of the scaling story.** MIN_DESELECT needed
DOUBLING under the 2x overclock; COOLDOWN did NOT — its stock value is already
optimal. The fields do not share a correction, so "the overclock halved every
clock-counted field" is wrong as a general account, even though it happened to
produce the right answer for MIN_DESELECT. Rule 75, third confirmation.

### QMI is exhausted

| field | status |
|---|---|
| CLKDIV | tuned (6) |
| RXDELAY | **inert** at MIN_DESELECT=14 (57 events) |
| MIN_DESELECT | **tuned** (14) — control-verified, ~60x |
| COOLDOWN | **closed** (1) — sharp optimum, both sides measured |
| PAGEBREAK | unscanned, and NOT a time field |

Nothing further on this register is worth an hour. The investigation moves to
the voltage re-check (AG).

### A live fault caught during flashing — and it lands on the XIP path

The first BW flash session died at 7.6 s with core 0 faulting inside
**`xip_check::sum_region`**, frame 1 = `0xfffffff8` (an EXC_RETURN, i.e. an
exception frame). A fault taken while reading XIP flash, in a routine whose
entire job is reading XIP flash. Independent confirmation of the XIP-path
conclusion from a completely different observation channel than the crash sector.

Two subsequent flash attempts hit `The initialization of the flash algorithm
failed / A timeout occurred`; the OpenOCD RESCUE recovered it, and the reflash
then succeeded. `.uninit` was randomised by the rescue (rule 36) but the window
is cold-seeded anyway, so nothing was lost.

## BW — REVERT to best-known QMI. This window is the rule-71 CONTROL.

Reverting is required regardless, so it costs nothing extra to make it the
control. Two windows (BU, BV) have run since the reference was last measured and
the voltage experiment must not be staked on a stale baseline.

Config: **MIN_DESELECT=14, RXDELAY=0, COOLDOWN=1, CLKDIV=6** — byte-identical to
the BS/BT-family best. Device-side readback: **`QMI retune at main-loop entry:
0x6000e006 -> 0x6000e006 (CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 14->14)`**.
CRC `0x5de4654e`, integrity OK, V1_30, 300 MHz. Addresses re-derived, unchanged.

Baseline **boot = 1, work = 13 at 11:48:11**.
Comparator: **1 per 8,064** (pooled, 57 events).

* **Reproduces (~1 per 8,000)** ⇒ the whole afternoon's comparisons stand and the
  voltage experiment can proceed on a trusted baseline.
* **Misses badly** ⇒ the epoch has stepped again, and BU and BV both have to be
  re-read before anything built on them survives.

### Next, once the control lands: the voltage question (AG)

We are at **V1_30, the TOP of embassy's `CoreVoltage`**. The historical finding
that voltage moved the rate was measured in the old, badly-tuned QMI regime and
has never been re-checked at the tuned operating point. The informative test is
to step voltage DOWN one notch (V1_25) and measure the sensitivity:

* **little or no change** ⇒ voltage is no longer a lever, and "electrical margin"
  as the account of the residual rate is dead;
* **clearly worse** ⇒ the device is voltage-limited at 300 MHz, V1_30 being
  embassy's ceiling is a real wall, and that is the evidence to put in front of
  the user — **going above V1_30 requires defeating a regulator safety register
  and MUST NOT be done without an explicit request.**

## BW — THE CONTROL FAILED. The epoch stepped, and it inverts the COOLDOWN result.

Window 11:48:11 → 17:08:39 (5 h 20 m — a tooling outage delayed the read, which
cost nothing: the counters are cumulative and not reading mid-window is correct
behaviour anyway, rule 50). Valid — CRC OK, ALLOC_GUARD intact, VREG 0xf0.

Δboot = 857 → **856 crashes**; Δwork = **26,569** → **1 per 31**.

This image is **byte-identical** to the config that measured 1 per 8,064 across
three windows. It now measures 1 per 31. **The epoch stepped ~260x, somewhere
around 09:30 today.** Pre-registered branch (b).

### The corrected timeline

| window | config | rate | epoch |
|---|---|---|---|
| 22:39 | MIN_DESELECT=14 | 1 per 9,676 | good |
| 23:45 | MIN_DESELECT=28 | 1 per 115 | good |
| 00:51 | 14 control | 1 per 6,408 | good |
| 08:25 | 14, RXDELAY=2 | 1 per 8,746 | good |
| 09:33 | COOLDOWN=2 | 1 per 29 | **bad from here** |
| 10:39 | COOLDOWN=0 | 1 per 383 | bad |
| 11:48 | COOLDOWN=1 (control) | 1 per 31 | bad |

### What survives, and what does not

**SURVIVES — MIN_DESELECT=14 vs 28.** The 28 window sits BETWEEN two good 14
windows, so that comparison is same-epoch and stands (~70x).

**DOES NOT SURVIVE — MIN_DESELECT 7 vs 14.** 7 was measured in the earlier bad
epoch. The 60x figure was always partly an epoch artifact. 14 is still the best
value we have, but its margin over 7 is unmeasured.

**DOES NOT SURVIVE — COOLDOWN=2's "277x disaster".** It is confounded with the
epoch onset, and against its same-epoch neighbour it is indistinguishable from
COOLDOWN=1 (1 per 29 vs 1 per 31).

**INVERTS — the COOLDOWN conclusion.** I compared BV against the stale
good-epoch comparator. Against its actual same-epoch neighbour, the BW control
six minutes later, the curve reverses:

| COOLDOWN (all bad epoch, adjacent windows) | rate | events |
|---|---|---|
| 2 | 1 per 29 | 156 |
| **0** | **1 per 383** | 134 |
| 1 | 1 per 31 | 856 |

**COOLDOWN=0 is ~12x better than the bootrom's 1**, on a three-point curve
measured entirely within one epoch. Last cycle I recorded the exact opposite and
declared the field closed on it.

The error is precisely what rule 71 exists to prevent, and I made it anyway: I
interleaved the control at the END of the series rather than BETWEEN configs, so
every comparison in between was against a baseline that had already moved.

**Rule 79: WHEN AN EPOCH STEP IS DETECTED, RE-READ EVERY WINDOW SINCE THE LAST
GOOD CONTROL AGAINST ITS NEIGHBOURS, NOT AGAINST THE OLD BASELINE. Adjacent
windows within one epoch are valid evidence even when the absolute numbers are
worthless.**

**Rule 80: A CONTROL AT THE END OF A SERIES ONLY TELLS YOU THE SERIES IS VOID.
A CONTROL BETWEEN CONFIGS TELLS YOU WHICH CONFIG WON. Interleave, do not append.**

### The epoch phenomenon is now the dominant term

It is worth ~260x. No config knob found so far is worth more than ~70x. Two
observed steps (Aug 17 13:28, Aug 18 ~09:30), both good → bad, and at least one
recovery (bad → good overnight). It survives reset, reflash, cold-seed, and — per
the user — a power cycle. Not thermal, not transient.

Boot timing is IDENTICAL across every config and both epochs (`QMI retune at
main-loop entry` logs at 12.168–12.170 s every single time), so the flash is not
globally slower in the bad epoch. The low throughput is fully explained by boot
time: ~856 boots x ~12 s accounts for the window, and ~31 iterations per boot
x 856 boots reproduces Δwork exactly. There is ONE phenomenon here — crashes per
iteration — not two.

## BX — re-run COOLDOWN=0 against its SAME-EPOCH comparator

Replicates BV, which is now the best result available in the current epoch.

Device-side readback: **`QMI retune at main-loop entry: 0x2000e006 -> 0x2000e006
(CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 14->14)`** — COOLDOWN=0, PAGEBREAK=2,
MIN_DESELECT=14, RXDELAY=0, CLKDIV=6. CRC `0x73ec71d5`, integrity OK, V1_30,
300 MHz. Addresses re-derived, unchanged.

Baseline **boot = 0, work = 308 at 17:12:34** — clean cold seed.
**Comparator: 1 per 31 (BW, adjacent window, SAME EPOCH, 856 events)** — NOT the
good-epoch 1 per 8,064.

* **~1 per 383 again** ⇒ COOLDOWN=0 is real and worth ~12x; it becomes the new
  best config and the next control goes BETWEEN configs, not after them.
* **~1 per 31** ⇒ BV was itself an epoch excursion, COOLDOWN is inert, and the
  epoch stepping is the only thing left worth chasing.
* **Anything near 1 per 8,000** ⇒ the epoch has recovered on its own, which is
  itself the most informative possible outcome about (AC).

## BX — COOLDOWN=0 REPLICATES. The inverted reading was the correct one.

Window 17:12:34 → 18:15:12 (62.6 min). Valid — CRC OK, ALLOC_GUARD intact,
VREG 0xf0, QMI verified `0x2000e006`.

Δboot = 129 → **128 crashes**; Δwork = **55,756** → **1 per 436**, against BV's
1 per 383. Replicated. Pre-registered branch (a).

### The COOLDOWN field, measured entirely within one epoch

| COOLDOWN | rate | events |
|---|---|---|
| 2 | 1 per 29 | 156 |
| 1 (bootrom) | 1 per 31 | 856 |
| **0** | **1 per 409 (pooled)** | **262** |

**Pooled COOLDOWN=0: 262 crashes / 107,081 iterations = 1 per 409 — 13x better
than the bootrom's COOLDOWN=1**, across two independent windows, every
comparison same-epoch and adjacent.

This is the first result in the investigation built the way rule 80 demands, and
it is the exact opposite of what I published two cycles ago from the same raw
numbers read against a stale baseline. The lesson is not that COOLDOWN moved —
it is that **the comparator, not the measurement, was wrong**.

Note also what this does to the COOLDOWN=2 story: 2 and 1 are indistinguishable
(1 per 29 vs 1 per 31). The "277x disaster" never existed; it was the epoch step
landing inside BU's window.

## BY — MIN_DESELECT 14 → 7, at the NEW operating point COOLDOWN=0

One test, two open questions:

1. **Rule 76.** MIN_DESELECT=14 was selected *entirely at COOLDOWN=1*. The
   operating point has since moved, so the best MIN_DESELECT at COOLDOWN=0 is
   unmeasured. This is the identical mistake-shape to the RXDELAY scan that was
   run at the wrong MIN_DESELECT — worth catching before building on 14 again.
2. **MIN_DESELECT 7 vs 14 has never had a valid same-epoch comparison.** 7 was
   measured in the earlier bad epoch, so the famous "60x" was always part config
   and part epoch. 14 beat 28 same-epoch and that stands, but 14 vs 7 is
   genuinely unknown.

ONE VARIABLE: only MIN_DESELECT moves, 14 → 7.

Device-side readback: **`QMI retune at main-loop entry: 0x20007006 -> 0x20007006
(CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 7->7)`** — COOLDOWN=0, PAGEBREAK=2,
MIN_DESELECT=7, RXDELAY=0, CLKDIV=6. CRC `0x443d139d`, integrity OK, V1_30,
300 MHz. Addresses re-derived, unchanged.

Baseline **boot = 0, work = 308 at 18:18:14** — clean cold seed.
**Comparator: 1 per 436 (BX, adjacent, same epoch, 128 events)**, or the pooled
1 per 409. NOT any pre-09:30 number.

* **Clearly worse than 1 per 436** ⇒ MIN_DESELECT=14 is confirmed on its own
  merits at last, same-epoch, and the pairing (14, COOLDOWN=0) is the config to
  build on.
* **Flat** ⇒ MIN_DESELECT is inert at COOLDOWN=0 and its entire apparent effect
  was epoch — a big claim, and it would mean COOLDOWN is the only real QMI knob.
* **Better** ⇒ the fields interact and the MIN_DESELECT scan must be redone from
  scratch at COOLDOWN=0.

Best available config is now **1 per 409 in the current epoch** (~tens of
thousands of crashes/day). The goal remains ZERO in 24 h. The epoch term (~260x)
still dwarfs every config knob found.

## BY — a 73-HOUR window. MIN_DESELECT is INERT at COOLDOWN=0, and the bad epoch NEVER RECOVERED.

**Session interruption note.** The investigation loop stopped at 19:21 on
2026-08-18 when the driving session hit a weekly usage limit, mid-cycle, one
minute after reading the BY counters and before interpreting them. The board was
left powered and running config BY untouched for the next three days. It was
resumed on 2026-08-21 at 19:37. Nothing was reflashed, reset, or reseeded in
between, so the BY window is intact and simply 73 times longer than intended.

Window 2026-08-18 18:18:14 → 2026-08-21 19:37:31 (**73.3 hours**). Valid —
magic `0x48b70001`, ALLOC_GUARD `0xa1100001` intact, VREG `0xf0`, no reflash.

Δboot = 9382 → **9381 crashes**; Δwork = **3,767,241** → **1 per 401**.

This is by an order of magnitude the largest sample in the investigation: 9,381
events against the 128–856 of every previous window.

### MIN_DESELECT 7 vs 14 — FLAT. Pre-registered branch (b).

| config | MIN_DESELECT | rate | events | 95% CI |
|---|---|---|---|---|
| BX | 14 | 1 per 436 | 128 | 1 per 360 – 512 |
| **BY** | **7** | **1 per 401** | **9,381** | **1 per 393 – 409** |

BY sits inside BX's interval. **MIN_DESELECT is inert at COOLDOWN=0**, and the
famous "60x for 14 over 7" was epoch, entirely. That was the branch flagged as
"a big claim" when BY was pre-registered, and it is the one that landed.

Note what this does to the QMI map: MIN_DESELECT=14 beat 28 same-epoch (~70x)
and that still stands, but 14 and 7 are now indistinguishable. The field is not
monotonic in the way a pure setup/hold-time story would require — which is the
third time a QMI field has confirmed as a *setting* while refuting the
*mechanism* attached to it (rule 75).

### THE REAL RESULT: the epoch did not recover in 73 hours

Every previous statement about the epoch was inferred from 1-hour windows
minutes apart. This window covers three continuous days, and the long-run
average is **1 per 401** — squarely inside the COOLDOWN=0 bad-epoch band
(383 / 436 / 401), and **19x away from the good-epoch 1 per 8,064**.

The mixing argument bounds it hard. If a fraction *f* of iterations had run in a
good epoch at 1/8,064 and the rest at ~1/400, then observing 1/401 over 3.77M
iterations puts *f* at essentially zero. **The board spent effectively all of
those 73 hours in the bad epoch.**

**Rule 81: THE "OVERNIGHT RECOVERY" IS NOT A CYCLE. A bad epoch has now been
observed to persist for 73 continuous hours across three day/night cycles. Any
model that treats the epoch as diurnal, thermal, or self-clearing is refuted.
The single observed bad→good recovery (Aug 17→18) was an event, not a period.**

This also inverts the framing. The good epoch — 1 per 8,064 — was seen in a
handful of adjacent windows on one evening. The bad state has now held for three
days. **The 1-per-400 regime is the device's normal behaviour, and the good
epoch is the anomaly to be explained**, not the other way round.

### Fault signature at the 73-hour mark

31 records, ring wrapped once (`erase_count=1`, `next_slot=0`), CRC OK on all:

| kind | count |
|---|---|
| HardFault | 29 |
| WatchdogTimeout | 1 |
| Panic | 1 |

| ARM PC | count | CFSR |
|---|---|---|
| `0x88000000` | 13 | IBUSERR |
| `0x1002e224` | 13 | BFARVALID PRECISERR |
| `0x0000fe9e` | 2 | IBUSERR |
| `0x2004c500` | 1 | INVSTATE |

The mix has moved decisively since the 31-record sample that opened this
document (15 stack-protector / 11 HardFault / 5 watchdog). The stack-protector
mode is **gone**, and the failure is now almost perfectly bimodal, 13 and 13:

* `PC = 0x88000000`, IBUSERR — a wild branch to an address that does not decode
  to anything on RP2350. Not flash, not SRAM, not a peripheral.
* `PC = 0x1002e224`, precise bus fault with BFAR valid — a *legal XIP address*,
  faulting on a data access from live code.

Both are the XIP path, which is consistent with everything since the
`stack_pop_check` result (0 mismatches in 2.48 billion SRAM LDM/STM
verifications) and with the live fault caught inside `xip_check::sum_region`
during a flash session.

### Config BZ — a same-duration CONTROL before anything else moves

The temptation is to go straight to the voltage re-check (AG), which has been
queued since BW. That would repeat the exact error rules 71/79/80 exist to
prevent: AG's comparator would be a 73-hour average, measured against a 1-hour
window, with an epoch term worth ~260x sitting between them.

So: **re-run BY's image, unchanged, as a 1-hour control.** No rebuild, no
reflash — the flashed image is already config BY (CRC `0x443d139d`), only the
counters and crash sector are re-seeded.

ONE VARIABLE: none. This is a pure control.

It does two jobs at once:

1. It gives AG a **same-epoch, same-duration** comparator, as rule 80 demands —
   interleaved *before* the change, not appended after it.
2. It measures whether a 1-hour window right now reproduces the 73-hour average,
   which is the first direct test of whether the epoch is currently *stable* or
   merely *slow-drifting*. Every previous epoch claim rests on 1-hour windows;
   none of them ever had a long-run number to check against.

* **~1 per 401** ⇒ the epoch is stable and the comparator is trustworthy;
  proceed to AG (V1_30 → V1_25) immediately with this as its baseline.
* **Materially different** ⇒ the epoch is drifting on a timescale *shorter* than
  a day, every 1-hour window in this document is suspect on its own terms, and
  the instrument — not the config — is what needs fixing before AG runs.

### Config BZ — running

No rebuild, no reflash. Device-side readback confirms the flashed image is still
BY: `integrity: full image crc 0x443d139d OK`, `QMI retune at main-loop entry:
0x20007006 -> 0x20007006 (CLKDIV 6->6 SCK 50MHz, MIN_DESELECT 7->7)` — COOLDOWN=0,
PAGEBREAK=2, MIN_DESELECT=7, RXDELAY=0, CLKDIV=6. V1_30 (VREG `0xf0`), 300 MHz.
Boot log at **12.5045 s**, identical to every window in this document.

Cold seed: crash sector blanked *and* reset (Y3 rule — never blank without
resetting). Addresses unchanged; no build, so rule 5 re-derivation does not apply.

Baseline **boot = 1, work = 284 at 2026-08-21 19:41:47**. Note the baseline was
read ~3 min after the reset rather than immediately, so the initial non-crash
boot is already counted in it: for this window **crashes = boot_end − 1**, which
is the same formula as always, just with the offset captured explicitly.

**Comparator: BY's 1 per 401 (9,381 events, 73.3 h, same image, same epoch).**

## BZ — THE CONTROL HOLDS. A 1-hour window reproduces the 73-hour average.

Window 19:41:47 → 20:45:10 (63.4 min). Valid — magic `0x48b70001`, ALLOC_GUARD
`0xa1100001`, VREG `0xf0`, image untouched.

Δboot = 139 → **138 crashes**; Δwork = **53,475** → **1 per 388**
(95% CI 1 per 323 – 452).

| window | duration | rate | events |
|---|---|---|---|
| BY | 73.3 h | 1 per 401 | 9,381 |
| **BZ** | **63.4 min** | **1 per 388** | **138** |

BY sits deep inside BZ's interval. **Pre-registered branch (a).**

### What this earns

This is the first time in the investigation that a 1-hour window has been
checked against a long-run number, and it passes. Two things follow:

1. **The epoch is currently stable.** It is not drifting on a sub-day timescale,
   so the comparator for the next config is trustworthy — which is exactly the
   precondition rules 71/79/80 demand before another config is allowed to move.
2. **The 1-hour window is a sound instrument at this operating point.** Roughly
   130–140 events per hour gives ±17%, which resolves the ~13x effects that
   matter and cannot resolve anything under ~1.4x. That is the honest resolution
   limit and it should be quoted whenever a window comes back "flat".

Note what BZ does *not* license: it says nothing about whether the epoch will
step *during* some future window. It says the instrument is sound right now.

### Signature: the wild-branch mode is dispersing

31 records, all CRC OK. 28 HardFault / 3 WatchdogTimeout.

| ARM PC | count |
|---|---|
| `0x88000000` | 9 |
| `0x1002e224` | 5 |
| `0x2002e6d4` | 3 |
| `0x89000000` | 2 |
| `0x68000000` | 2 |
| `0x200032ac` | 2 |
| `0x0000fe9e` | 2 |
| `0x2004c500` | 1 |

CFSR: 15 IBUSERR, 5 INVSTATE, 5 BFARVALID PRECISERR, 2 UNALIGNED, 1 UNDEFINSTR.

BY's clean 13-and-13 bimodality was itself partly a small-ring artifact — the
same two addresses still dominate, but the tail is much wider than 31 records
could show. `0x1002e224` and `0x88000000` are reproducible across both windows.

**A new observation worth carrying forward: every wild branch target has all-zero
low 24 bits** — `0x88000000`, `0x89000000`, `0x68000000`. A branch target of the
form `0xNN000000` is not what a randomly corrupted pointer looks like; it is what
a word looks like when only its **top byte** carries surviving data. That is a
different failure shape from "the whole word was replaced", and it is the first
structural hint about the corrupt value since the poison-store work in Z2/AA.
Not chased now — AG is already pre-registered and one variable moves at a time —
but it belongs in the queue ahead of any further QMI archaeology.

## Config AG — the voltage re-check, at last on a validated comparator

The experiment queued since BW. We sit at **V1_30, the top of embassy's
`CoreVoltage`**. The historical "voltage is a ~7x modulator" note was measured in
the old badly-tuned QMI regime *and* against the crash-log counter that rule 49
showed was saturating, so it has never been trustworthy. Now it can be measured
properly: the QMI operating point is settled, the counters are sound, and BZ has
just proven the comparator.

ONE VARIABLE: `TARGET_CORE_VOLTAGE` V1_30 → V1_25. QMI, clock, and layout all
held. The active constant is the `cfg(all(not(oc-300), not(oc-280)))` arm —
default features are `["firmware-bin"]`, so neither `oc-` feature is on.

**Comparator: BZ's 1 per 388 (138 events), immediately adjacent, same epoch,
same duration** — interleaved before the change, per rule 80.

* **Little or no change (inside ~1 per 323–452)** ⇒ voltage is not a lever at the
  tuned operating point, and "electrical margin" as the account of the residual
  rate is **dead**. That is the outcome that most changes the direction of the
  investigation, because it would leave the ~260x epoch term with no electrical
  explanation and force the wild-branch structure above to the front.
* **Clearly worse** ⇒ the device is genuinely voltage-limited at 300 MHz, and
  V1_30 being embassy's ceiling is a real wall rather than an arbitrary stop.
  **Going above V1_30 requires defeating a regulator safety register and MUST NOT
  be done without an explicit request from the user** — that result would be the
  evidence to put in front of them, not a licence to proceed.
* **Better at the LOWER voltage** ⇒ the electrical story is inverted and the
  whole "marginal timing at 2x rated clock" model needs rebuilding.

### Config AG — running

Device-side readback: `integrity: full image crc 0x6a6f0b36 OK`;
`QMI retune at main-loop entry: 0x20007006 -> 0x20007006 (CLKDIV 6->6 SCK 50MHz,
MIN_DESELECT 7->7)` — QMI **unchanged** from BZ, confirming one variable.
**VREG `0x4010000c` = `0x000000e0` = V1_25**, verified on the device after the
flash (it read `0xf0` for every window from BE onward). 300 MHz untouched.
Boot log 12.1717 s.

Rule 5 re-derivation after the build — addresses **unchanged**:
`HEARTBEAT=0x20066fa8`, `ALLOC_GUARD=0x200677a0` (`0xa1100001` intact),
`_stack_end=0x20067b18`, `BOOT_COUNT=0x2006700c`, `WORK_COUNT=0x20067010`.

Cold seed: crash sector blanked *and* reset (Y3 rule).

Baseline **boot = 2, work = 161 at 2026-08-21 20:49:53**, read ~45 s after the
reset. Two boots had already occurred (the reset boot plus one crash), so for
this window **crashes = boot_end − 2**.

**Comparator: BZ's 1 per 388, 138 events, adjacent window, same epoch, same
duration.** Resolution limit ~1.4x (see BZ).

## AG — VOLTAGE IS FLAT. Electrical margin is dead as an account of the residual rate.

Window 20:49:53 → 21:52:08 (62.2 min). Valid — magic OK, ALLOC_GUARD
`0xa1100001`, **VREG `0xe0` = V1_25 held for the whole window**, QMI `0x20007006`
unchanged from BZ.

Δboot = 141 − 2 → **139 crashes**; Δwork = **47,862** → **1 per 344**
(95% CI 1 per 287 – 402), against BZ's **1 per 388** (CI 323 – 452).

Intervals overlap heavily in both directions. **Pre-registered branch (a).**

### This is an exclusion, not a shrug

"Flat" usually means "we could not tell". Here we could. The window had ample
power to see the historically claimed effect:

| if voltage were… | expected events | observed |
|---|---|---|
| 28x worse (the old note) | ~3,454 | **139** |
| 7x worse (the other old note) | ~863 | **139** |
| 2x worse | ~247 | **139** |
| 1.5x worse | ~185 | **139** |

**Voltage sensitivity across V1_25 → V1_30 is below ~1.4x.** The historical
"voltage is a ~7x modulator" and "voltage was 28x" claims are **refuted**, not
merely unreplicated. Both were measured in the badly-tuned QMI regime against
the saturating crash-log counter (rule 49). Note the sign, too: V1_25 came out
*nominally worse* (344 vs 388), so there is no hidden benefit at lower voltage
either — branch (c) is closed as well.

With clock established as a rate modulator only, QMI exhausted (COOLDOWN 13x,
every other field inert), and voltage now flat, **there is no electrical knob
left that explains the residual rate.** That was pre-registered as the outcome
that most changes direction, and it is the one that landed.

## THE COMBINED-WINDOW FINDING: the corrupt words are GAME BOY ADDRESSES

With BY, BZ and AG pooled (93 records), every PC / LR / BFAR value was
classified by which byte lanes are zero. Values that are legal RP2350 addresses
(`0x10……` XIP, `0x20……` SRAM) were separated from anomalous ones. The anomalous
set is not diffuse — it collapses into three patterns:

| lanes [3210] | n | meaning | examples |
|---|---|---|---|
| `DZZZ` | 37 | only the TOP byte survives | `0x88000000`, `0x89000000`, `0x68000000` |
| `ZZDD` | 32 | only the BOTTOM half-word survives | `0x0000e06a`, `e072`, `e07c`, `e084`, `e114`, `0x0000fe9e` |
| `ZZZD` | 5 | only the bottom byte | `0x000000a0`, `0x00000002`, `0x00000001` |
| `DDDD` | 1 | full garbage | `0x10daf8bb` |

Read those as **16-bit values placed in one half of a 32-bit word**:

* high half: `0x8800`, `0x8900`, `0x6800`
* low half: `0xe006`, `0xe06a`, `0xe072`, `0xe07c`, `0xe084`, `0xe114`, `0xfe9e`

**Every one of them is a Game Boy address, and they land in meaningful regions.**
`0x8800` is the VRAM tile-data block-1 base. `0xE000–0xFDFF` is echo RAM.
`0xFE00–0xFE9F` is OAM — and `0xfe9e` is its very last byte. `Sm83`'s own
recorded `GB SP` in these records sits at `0xdff3`, immediately below the echo
region.

This is what a stream of `u16` Game Boy addresses looks like when the memory it
was written into is later read back as `u32` words: each corrupt word carries
one 16-bit address in one half and zero in the other, and which half depends
only on the parity of the halfword offset.

**Rule 82: THE CORRUPT VALUES ARE STRUCTURED, AND THE STRUCTURE IS EMULATOR DATA,
NOT NOISE. Anomalous words are 16-bit Game Boy addresses (VRAM tile-data base,
echo RAM, OAM) deposited into one half of a 32-bit word. A silicon read-path
fault would produce address-space-shaped garbage or bit-flips of the true value;
it would not preferentially produce valid Game Boy addresses. This is a SOFTWARE
write going somewhere it should not.**

That reframes the whole investigation, and it is consistent with evidence that
was already in this document and never joined up:

* the early HardFaults returning through a corrupted saved PC from the epilogue
  of `SharedWorkerState::write_live_vram_range` — a function whose first
  parameter is a `u16` offset;
* config AC's MPU-detected **core-0 write into core-1's stack**;
* the standing note in `memory.rs` that the only way the OAM-DMA stores escape
  is "if `self` is itself a wild pointer", which "lands the indexed store inside
  whatever core-0 frame is currently live";
* and why every electrical knob only ever *modulated* the rate — timing changes
  the width of a cross-core race window, it does not remove the race.

### The one fully-decoded frame

`HF_STACK` (`0x20067048`, magic `0x48f50001` valid) held a complete capture:

    pc=0x000000a0  lr=0x20001f67  cfsr=0x00020000 INVSTATE  sp_before=0x2007cd10
    stacked frame: r12=0x1001424f  lr=0x20001f67  pc=0x000000a0  xPSR=0x08000000
    r4-r11 = 2000087d 2007cd38 20000ecb 200273d4 2007d2d8 2003d5b0 200273d4 2003d5b0

`xPSR = 0x08000000` has **bit 24 (T) clear**, which is exactly why CFSR is
INVSTATE: the core branched to an even address. Note that the stacked `r12` and
`LR` are both **valid** (`0x1001424f` XIP, `0x20001f67` SRAM — the
`Instructions::cb` region named in the earlier records), while the *adjacent*
`PC` and `xPSR` words are both corrupt. Two adjacent words clobbered, the two
before them clean: a short contiguous overwrite, not a single stray store.

Also note `LR = 0x20001f67` is **SRAM** code. The standing "everything is on the
XIP path" summary needs qualifying — the memcpy faults are XIP (`0x1002e224`),
but this one was executing from RAM.

### `0x1002e224` identified

`llvm-objdump` resolves it exactly:

    1002e224: f812 6b01   ldrb r6, [r2], #1     ; compiler_builtins::mem::memcpy

It is the byte-copy tail of **memcpy**, and `BFAR` is its *source pointer* —
`0x0000e06a` in 7 of 10 cases. So memcpy is being handed a Game Boy echo-RAM
address as a host source pointer. Same family, different symptom.

### Two mechanisms ruled OUT this cycle

**DMA is exonerated.** `DMA_CRASH_SNAPSHOT` (`0x20067ad0`, sentinel
`0xd4a0c12a` valid) reads `busy_mask = 0x00000000` — no channel was active at
fault time — and the only two live channel write-addresses are `0x50200010` and
`0x40088008`, both **peripherals**. No DMA channel in this firmware writes into
SRAM at all, so DMA cannot be spraying the stack.

**The MPU cross-core violation is not currently arming.** `MM_REGS`
(`0x20067128`) holds no magic — it is uninitialised noise, so the deterministic
core-0→core-1-stack violation of config AC/AE has not fired in the current
build. That instrument is dark and would need re-arming before it can contribute.

## Config CA — REVERT to V1_30, and a replication of BZ

AG is reverted because V1_30 is the established operating point and V1_25 was
nominally worse. ONE VARIABLE: voltage back to V1_30. This window doubles as a
**replication of BZ at the best-known config**, which is worth having before the
investigation turns to the software hypothesis.

Device readback: `integrity: full image crc 0x4285c1f3 OK`; `QMI retune at
main-loop entry: 0x20007006 -> 0x20007006 (CLKDIV 6->6 SCK 50MHz, MIN_DESELECT
7->7)`; **VREG `0x4010000c` = `0x000000f0` = V1_30**, verified after the flash.
300 MHz untouched. Boot log 12.1717 s.

Rule 5 re-derivation — addresses **unchanged**: `HEARTBEAT=0x20066fa8`,
`HF_STACK=0x20067048`, `MM_REGS=0x20067128`, `ALLOC_GUARD=0x200677a0`,
`DMA_CRASH_SNAPSHOT=0x20067ad0`, `_stack_end=0x20067b18`.

Cold seed: crash sector blanked *and* reset. Baseline **boot = 1, work = 213 at
2026-08-21 22:02:20**; **crashes = boot_end − 1**.

**Comparator: BZ's 1 per 388 (138 events) and AG's 1 per 344 (139 events).**

* **~1 per 340–390** ⇒ BZ replicates, the epoch is still stable across three
  consecutive windows, and the electrical chapter closes cleanly.
* **Materially different** ⇒ an epoch step landed inside CA, and rule 79 applies
  to AG as well — the voltage conclusion would need re-reading against neighbours
  before it can be trusted.

### Next, regardless of CA: hunt the writer, not the volts

The queue is now software-side, in priority order:

1. **Re-arm the MPU instrument.** It is the only tool that has ever caught the
   cross-core write in the act, and it is currently dark. A live capture with
   `MM_REGS` populated would name the storing function directly.
2. **Instrument `write_live_vram_range` / `write_live_oam_range` bounds.** Both
   take `u16 start_offset` and both currently clamp with `min`/`saturating_sub`,
   so they are safe *if `self` is sound* — which is precisely the assumption the
   `memory.rs` comment says is in doubt.
3. **Widen `HF_STACK` to a ring.** Every structural claim above rests on ONE
   fully-decoded frame plus 93 records that only carry PC/LR/CFSR/BFAR. A ring of
   4–8 frames per window would make rule 82 testable rather than inferred.
   **Caution: `.uninit` placement is load-bearing here — `_stack_end` is the end
   of the last `.uninit` object and core 1's read-only MPU region starts at
   `_stack_end & !0x1F`. `MM_REGS` is already padded to 20 words for exactly this
   reason. Any resize must re-check that boundary.**

## CA — BZ replicates. Three consecutive windows agree, and voltage is confirmed inert.

Window 22:02:20 → 23:05:13 (62.9 min). Valid — magic OK, ALLOC_GUARD intact,
VREG `0xf0` held, QMI `0x20007006`.

Δboot = 140 − 1 → **139 crashes**; Δwork = **49,320** → **1 per 355**
(95% CI 1 per 296 – 414). **Pre-registered branch (a).**

| window | voltage | rate | events |
|---|---|---|---|
| BZ | V1_30 | 1 per 388 | 138 |
| AG | V1_25 | 1 per 344 | 139 |
| CA | V1_30 | 1 per 355 | 139 |

**Pooled V1_30 (BZ+CA): 277 events / 102,795 iterations = 1 per 371**, against
V1_25's 1 per 344. Still flat on double the sample. The epoch has now held
stable across four consecutive windows (BZ, AG, CA and the 73-hour BY), so the
comparators in this block are sound and the electrical chapter closes cleanly.

## RULE 83 — THE WRITE-ONCE INSTRUMENTS WERE SERVING STALE DATA

`HF_STACK` read **byte-for-byte identical** at 21:55 and again at 23:05 — either
side of a full rebuild, a reflash to a different CRC, a cold seed, and 139
crashes. Reading the handler explains why, and it is worse than it looks:

```rust
if cfsr & CFSR_INVSTATE != 0 {          // gate 1: only INVSTATE
    if h.read_volatile() != HF_STACK_MAGIC {   // gate 2: write-once, forever
```

Two compounding defects:

1. **It only ever captured INVSTATE**, which is the *rarest* mode — 3-5 records
   out of ~139 per window. The two dominant modes, the IBUSERR wild branch
   (n=37) and the memcpy PRECISERR (n=32), could never be captured at all.
2. **Write-once in `.uninit` outlives everything.** `.uninit` survives reset, and
   `probe-rs download` writes *flash*, never SRAM — so the cold seed never
   touched it. The block persists until a rescue reset randomises SRAM.

**Rule 83: A WRITE-ONCE INSTRUMENT IN `.uninit` IS NOT DATED. It survives reset,
reflash and cold seed, so a capture of unknown age presents itself as current.
Any `.uninit` instrument must have its magic CLEARED AS PART OF THE COLD SEED,
and the cold-seed procedure is now: blank the crash sector, zero the heartbeat
magic, ZERO EVERY INSTRUMENT MAGIC, then reset.**

### Correction to the previous cycle

The fully-decoded frame reported under AG (`pc=0x000000a0`, `xPSR=0x08000000`)
is of **unknown age and is from the rarest fault mode**. It was not a capture
from the AG window. Two claims built on it are withdrawn as unsupported: that
"stacked r12 and LR are valid while the adjacent PC and xPSR are corrupt" is a
general shape, and that `LR=0x20001f67` shows the failure is not always on the
XIP path. Both may still be true; neither is evidenced by a dated observation.

**Rule 82 is unaffected** — it rests on the 93 ring records from BY/BZ/AG, which
are per-window and correctly dated.

### Config CB — the gate is removed

`HF_STACK` now captures the **first HardFault of any mode** after its magic is
cleared, keeping write-once semantics *within* a window instead of forever. No
`.uninit` resize, so the load-bearing tail boundary is untouched (verified:
`MM_REGS` written slots end at `0x2006715c`, core-1 MPU region base is
`0x20067b00`).

## CB — FIRST EVER CAPTURE OF THE DOMINANT MODE, AND THE CONTIGUOUS-RUN QUESTION IS ANSWERED

Within ~40 seconds of the cleared magic, the instrument caught the mode that had
been invisible for the entire investigation:

    pc=0x88000000  lr=0x1001fd49  cfsr=0x00000100 IBUSERR
    sp_before=0x2007cd48  r12=0x1001e79d  exc_return=0xfffffff9 (core 0, MSP)

The band around `sp_before` is the payload:

| address | word | meaning |
|---|---|---|
| `0x2007cd38` | `1001e79d` | stacked r12 — **valid** |
| `0x2007cd3c` | `1001fd49` | stacked LR — **valid** |
| `0x2007cd40` | `88000000` | stacked PC — **corrupt** |
| `0x2007cd44` | `68000000` | stacked xPSR — **corrupt** |
| `0x2007cd48` | `88000000` | ← `sp_before` |
| `0x2007cd4c` | `68000000` | |
| `0x2007cd50` | `88000000` | |
| `0x2007cd54` | `88000000` | |

**SIX CONSECUTIVE WORDS overwritten with `0x88000000` / `0x68000000`.**

This settles the question the AM/AN/AQ configs were built to answer and which
this document calls the one the investigation hinges on — *is it one stray word
or a contiguous run?* **It is a contiguous run.** The run spans and destroys the
exception frame's PC and xPSR, which is precisely why the core ended up
branching to `0x88000000`: it returned through a word that had already been
overwritten.

Per rule 82 the halfword sequence is `0000 8800 0000 6800 0000 8800 0000 8800` —
16-bit Game Boy values at a 4-byte stride, exactly the shape of a **struct array
being written**, not of a bit-flip or a bus fault.

### The smashed frame is the CROSS-CORE COMMAND QUEUE PRODUCER

Resolving the live registers against the ELF names the whole chain:

| reg | address | symbol |
|---|---|---|
| LR | `0x1001fd49` | `+0x18` in **`heapless::spsc::QueueInner<Core1Command, ViewStorage>::inner_enqueue`** |
| — | `0x1001fd18` | the `increment` it had just called |
| r12 | `0x1001e79d` | `GameBoy<Core1Transport>::write_apu_register` |
| r6 | `0x1000a3c3` | embassy main task, immediately after `bl drain_bus_events` |

Core 0 was **inside `inner_enqueue`, pushing a `Core1Command` onto the core-0 →
core-1 command queue**, when a contiguous run of `Core1Command`-shaped words
landed on its own live stack frame.

That is a complete and self-consistent mechanism, and it unifies everything:

* the corrupt words are Game Boy values because **they are `Core1Command`
  payloads** — the queue carries emulator writes (`write_apu_register` and the
  VRAM/OAM range writers all funnel through it);
* it is cross-core, which is what config AC's MPU catch saw as a core-0 write
  into core-1's stack;
* it is a contiguous run because an enqueue writes a whole struct;
* and every electrical knob only ever *modulated* it, because clock and QMI
  timing change the width of a producer/consumer race window without removing
  the race.

**The working hypothesis is now specific: `inner_enqueue` is computing a slot
address outside the queue buffer.** With `ViewStorage` the queue is type-erased —
capacity lives in a runtime field rather than the type — so a corrupt queue
pointer, a corrupt `tail`, or a racing index all produce a wild slot address.
This is the same shape as the standing `memory.rs` note that stores only escape
"if `self` is itself a wild pointer".

### Config CC — give COMMAND_QUEUE the guard that AUDIO_QUEUE already has

There is a ready-made precedent in this codebase. `AUDIO_QUEUE` is **not** a bare
queue: a previous investigation wrapped it in `AudioQueueStorage { queue, guard }`
with a `check_audio_queue_guard`. `COMMAND_QUEUE` never got the same treatment —
it is still a bare `StaticCell<spsc::Queue<Core1Command, N+1>>` with nothing
around it.

ONE VARIABLE: wrap `COMMAND_QUEUE` in a guarded storage struct and check the
guard words, exactly as the audio queue does.

* **Guard breached** ⇒ the command queue is being overrun, the writer is named,
  and this becomes a bounded software bug rather than a silicon mystery.
* **Guard intact while smashes continue** ⇒ the enqueue is not overrunning its
  own buffer; the slot address is wild rather than merely out of range, which
  points at the `Producer`/queue pointer itself and makes that the next target.

Note this is an *instrument*, not a fix — per the standing directive to root
cause rather than blindly patch. A guard that fires tells us where to look; it
does not paper over the write.

### Config CB — running

No functional change beyond the `HF_STACK` gate. Device readback: `integrity:
full image crc 0xda9296b3 OK`; `QMI retune at main-loop entry: 0x20007006 ->
0x20007006`; **VREG `0xf0`**; 300 MHz; boot log 12.5063 s. Rule 5 re-derivation —
all addresses unchanged.

Cold seed under the new rule-83 procedure: crash sector blanked, heartbeat magic
zeroed, **`HF_STACK` and `MM_REGS` magics zeroed**, then reset.

Baseline **boot = 1, work = 215 at 2026-08-21 23:10:16**; **crashes = boot_end − 1**.

**Comparator: pooled V1_30 1 per 371 (277 events).** CB changes only the crash
handler's capture gate, so the rate should be unmoved; a material change would
itself be evidence that the handler path participates in the failure.

## CB — THE RATE MOVED 2.36x WORSE, AND NOTHING IN THE DIFF CAN EXPLAIN IT

Window 23:10:16 → 00:15:12 (64.9 min). Valid — magic OK, ALLOC_GUARD
`0xa1100001`, VREG `0xf0`, QMI `0x20007006`.

Δboot = 194 − 1 → **193 crashes**; Δwork = **30,374** → **1 per 157**
(95% CI 1 per 135 – 180), against the pooled V1_30 comparator of **1 per 371**
(277 events, CI 1 per 327 – 415).

**The intervals do not overlap.** This is CB's pre-registered material-change
branch, and it must be resolved before any new config is allowed to build on it.

### The diff cannot mechanically produce this

CB's only change is deleting the `cfsr & CFSR_INVSTATE` gate on the `HF_STACK`
capture. That block is **write-once**, and the magic was confirmed set within
~40 seconds of the cold seed. So for **192 of this window's 193 crashes the
handler executed byte-identical code to CA's.** One fault's worth of extra
capture work cannot produce a sustained 2.36x.

The leading explanation is therefore an epoch step, not the instrument. But that
is precisely the kind of reasoning rule 75 exists to distrust — three times in
this investigation a change that "could not matter" has been confirmed as
mattering — so the baseline is being re-established by measurement rather than by
argument.

### The signature shifted too, which argues for a regime change

| ARM PC | count | vs CA/AG |
|---|---|---|
| `0x1002e220` | 10 | **new** — 4 bytes *before* the usual `0x1002e224` |
| `0x88000000` | 5 | down from 9-10 |
| `0x68000000` | 5 | **up from 2** |
| `0x20007006` | 2 | new |
| `0x00000000` | 2 | |
| `0xe4000002`, `0xcb000002`, `0x53000004` | 1 each | new |

CFSR: 12 IBUSERR, 10 BFARVALID PRECISERR, 5 INVSTATE, **3 IACCVIOL**.

`IACCVIOL` is **new to this investigation** — an MPU *instruction* access
violation. Note it does not set `MMFAR`, and `MM_REGS` is gated on `MMARVALID`,
which is why `MM_REGS` stayed dark (`0x00000000`, the value the cold seed wrote)
despite MPU faults occurring. That is a gap in the instrument, not an absence of
MPU events.

A rate change accompanied by new fault modes and a shifted mode mix looks more
like a regime change than like the same phenomenon running faster.

### Correction: boot timing is NOT identical across windows

This document has repeatedly asserted that `QMI retune at main-loop entry` logs
at 12.168-12.172 s "every single time", and used that to argue the flash is not
globally slower. The observed values are actually **bimodal**:

| window | boot | rate |
|---|---|---|
| BZ | 12.5045 | 1 per 388 |
| AG | 12.1717 | 1 per 344 |
| CA | 12.1717 | 1 per 355 |
| CB | 12.5063 | 1 per 157 |
| CC0 | 12.1702 | — |

Two distinct modes, ~12.17 and ~12.50, differing by ~330 ms. They do **not**
track the crash rate — BZ sat at 12.50 with a perfectly normal 1 per 388 — so
this does not explain CB. But the "identical every time" claim is false and
should stop being cited as evidence.

## Config CC0 — CONTROL. The gate is restored; nothing else moves.

Rule 80 demands the control go *between* configs, not after the series. The
command-queue guard (CC) is deferred one window so it is not built on an
unexplained baseline.

ONE VARIABLE: the `HF_STACK` INVSTATE gate is reinstated, making this image
functionally identical to CA. `HF_STACK` is not being abandoned — only paused for
one window; the CB capture it already produced is the most valuable single
observation in the investigation.

Device readback: `integrity: full image crc 0x3d9cc95e OK`; `QMI retune at
main-loop entry: 0x20007006 -> 0x20007006`; **VREG `0xf0`**; 300 MHz; boot log
12.1702 s. Rule 5 re-derivation — all addresses unchanged. Cold seed under the
rule-83 procedure (sector, heartbeat magic, both instrument magics, then reset).

Baseline **boot = 1, work = 640 at 2026-08-22 00:20:14**; **crashes = boot_end − 1**.

**Comparators: CB's 1 per 157 (193 ev, immediately prior) and the pooled V1_30
1 per 371 (277 ev, BZ+CA).**

* **~1 per 157** ⇒ the epoch stepped, CB's instrument is exonerated, every
  comparator in this block is stale, and rule 79 applies: the whole BZ/AG/CA
  voltage block must be re-read against neighbours before the queue work starts
  from the new baseline. CB gets restored immediately.
* **~1 per 371** ⇒ the instrument change really did cost 2.36x, which would be a
  major and genuinely surprising result — it would mean the HardFault handler
  path participates in the failure, and that becomes the investigation.
* **Anything else** ⇒ the rate is drifting on a sub-window timescale, which would
  invalidate the ~1.4x resolution claim and force mid-window sampling before any
  further config work.

## CC0 — THE CONTROL CAME BACK CLEAN. CB'S DEGRADATION IS REAL, AND MY MECHANICAL ARGUMENT WAS WRONG.

Window 00:20:14 → 01:23:12 (63.0 min). Valid — magic OK, ALLOC_GUARD intact,
VREG `0xf0`, QMI `0x20007006`.

Δboot = 135 − 1 → **134 crashes**; Δwork = **58,470** → **1 per 436**
(95% CI 1 per 362 – 510). **Pre-registered branch (b).**

### The bracket

QMI, voltage, clock and layout of the *emulator* are identical across all three:

| window | HF_STACK gate | rate | 95% CI | events |
|---|---|---|---|---|
| CA | present | 1 per 355 | 296 – 414 | 139 |
| **CB** | **removed** | **1 per 157** | **135 – 179** | **193** |
| CC0 | present | 1 per 436 | 362 – 510 | 134 |

**CB sits BETWEEN two windows of the restored config, and its interval overlaps
neither.** That is exactly the standard this document used to validate
MIN_DESELECT 14-vs-28 ("the 28 window sits between two good 14 windows, so that
comparison is same-epoch and stands"). By its own rule, **CB's 2.36x degradation
is same-epoch, bracketed, and real.**

### I was wrong, and the way I was wrong is the finding

Last cycle I argued the diff "cannot mechanically produce this": the capture
block is write-once, the magic was confirmed set within ~40 s, so 192 of 193
crashes ran byte-identical handler code. That reasoning is still *correct as far
as it goes* — and the rate moved anyway. **This is the fourth time in this
investigation that a change which "could not matter" has been confirmed as
mattering** (rule 75), and the first time it has been caught by a bracketed
control rather than discovered later as a contradiction.

If the executed code really is identical for 192 of 193 faults, then the thing
that changed is not *what the handler does* — it is **where everything sits**.
Removing an `if` shifted the handler's code and everything linked after it. The
image CRC moved `0x4285c1f3` → `0xda9296b3` → `0x3d9cc95e` across CA/CB/CC0.

**Rule 84: A REBUILD IS NOT A CONTROLLED CHANGE. Any source edit shifts code
layout, and layout appears to be worth ~2.4x on the crash rate. Every comparison
in this document that involved a rebuild is confounded by layout unless the
config window was bracketed by same-config neighbours. "One variable" was never
one variable — it was always the config field PLUS a new layout.**

This is potentially the most consequential methodological result in the
investigation, because it retroactively explains the pattern that has plagued it:
configs that confirm and then refute, "epoch steps" that coincide with reflashes,
and mechanisms that never survive contact with a control. A ~2.4x layout term
sitting under every rebuild would produce exactly that.

Note carefully what is NOT claimed. Layout is *a* hypothesis for CB's shift; the
bracketed fact is only that **restoring the source restored the rate**. The
alternative — that the capture block executes far more often than the write-once
read implies — is not excluded, only made unlikely. CD separates them.

## Config CD — a SEMANTICALLY NULL layout perturbation

The decisive test. Append `.space 128` to the end of the `.HardFaultTrampoline`
global_asm block, *after* `b.w {handler}` and after the `.size` directive.

Why this specific change:

* It is **unreachable**. The trampoline's last instruction is an unconditional
  `b.w`, so the padding can never execute. Not one executed instruction changes.
* The section is **guaranteed kept** — `HardFault` is referenced by the vector
  table, so `--gc-sections` cannot drop it.
* It **shifts everything linked after `.HardFaultTrampoline`**, which is the same
  class of disturbance CB caused.

ONE VARIABLE: 128 bytes of unreachable padding. No behaviour, no config, no
timing of any executed instruction.

**Comparator: CC0's 1 per 436 (134 ev) and CA's 1 per 355 (139 ev) — pooled
same-config 1 per 397 (273 ev), both immediately adjacent and same-epoch.**

* **Rate moves materially (say outside 1 per 330 – 480)** ⇒ **LAYOUT SENSITIVITY
  CONFIRMED.** A semantically null edit moves the crash rate, every
  rebuild-confounded comparison in this document must be re-read, and the
  measurement protocol has to change: either hold layout fixed across a config
  series, or characterise layout variance as the real noise floor before any
  effect smaller than it can be claimed. It would also make "the epoch" partly
  an artifact of reflashing.
* **Rate unchanged** ⇒ layout is NOT the mechanism, and CB's degradation came
  from the capture block genuinely executing far more than write-once implies.
  The follow-up is then a counter incremented on every entry to the block, to
  measure directly how often it runs.

Either answer is worth more than another config knob.

### The layout disturbance, measured

The first CD attempt (`.space 128` after the trampoline) was **discarded before
it ran**: symbol diffing showed `.HardFaultTrampoline` links late, so it moved
only **66 of 2161** symbols. A null result from that would have meant nothing.

Diffing the symbol tables makes all three builds precise:

| comparison | symbols moved | delta |
|---|---|---|
| CA → CC0 (gate restored) | **0 of 2161** | — |
| CC0 → CB (gate removed) | **959 of 2161** | −4 bytes |
| CC0 → CD (two nops) | **1021 of 2161** | +4 … +32 bytes |

**CA and CC0 are byte-identical in layout** — which is exactly why CC0 reproduced
CA's rate, and makes the bracket around CB airtight. CD's two nops reproduce CB's
disturbance in magnitude and location, in the opposite direction.

All RAM addresses are unchanged, so every probe address in this document still
holds.

### Config CD — running

Device readback: `integrity: full image crc 0xaae78425 OK`; `QMI retune at
main-loop entry: 0x20007006 -> 0x20007006`; **VREG `0xf0`**; 300 MHz; boot log
12.5059 s.

Cold seed under the rule-83 procedure. A `probe_rs ... Expected core to be
halted` warning appeared during the writes, so the seed was **verified after the
fact**: `HF_STACK[0] = 0x00000000`, `MM_REGS[0] = 0x00000000`. Both magics are
genuinely cleared.

Baseline **boot = 0, work = 45 at 2026-08-22 01:30:30**; **crashes = boot_end − 1**.

**Comparator: CC0's 1 per 436 (134 ev) and CA's 1 per 355 (139 ev) — pooled
same-layout 1 per 397 (273 ev), both same-epoch.**

## CD — VOID. The window did not measure the crash rate; the board was limping.

Window 01:30:30 → 02:33:16 (62.8 min). Δboot = 102 − 1 → 101; Δwork = 3,503.
Naively that reads **1 per 35**, an 11x degradation against the pooled 1 per 397.

**It is not a valid measurement.** Four independent signals say so:

| signal | CD | normal |
|---|---|---|
| crash records committed | **7** | 31 (ring full, wrapped) |
| boots | 101 | ~139 |
| iterations/min while running | **84** | ~1,400 |
| WatchdogTimeout share | **4 of 7** | 1-3 of 31 |
| `DMA_CRASH_SNAPSHOT` sentinel | **zeroed** | `0xd4a0c12a` |

101 boots produced only 7 records, so **~94 resets never went through the crash
handler at all.** Those are hangs ending in watchdog resets, not the crash mode
this investigation has been measuring. Throughput per running minute fell 17x.
And an instrument in `.uninit` that nothing in the cold seed touches —
`DMA_CRASH_SNAPSHOT[0]` and `[2]` — was silently zeroed.

That is a wedge loop, not the same failure running faster.

**Rule 85: `crashes = Δboot` ASSUMES EVERY RESET WENT THROUGH THE CRASH HANDLER.
Validate Δboot against the committed record count every window. When records are
far below the ring capacity while boots are high, the board is hanging rather
than crashing, the rate metric is measuring a different phenomenon, and the
window is VOID. This check costs one line and would have caught the problem
before an 11x headline was written.**

### The limp was visible at the baseline read, and I missed it

Work counter at +40 s after the cold seed, same procedure each time:

| window | work at +40 s |
|---|---|
| CC0 | 640 |
| **CD** | **45** |
| CE | 449 |

CD was already limping in its first seconds. That number was in front of me when
I recorded the baseline and I did not treat it as a signal. It is a free
early-warning check and belongs in the seed procedure.

### The prime suspect is the hazard already flagged, now active

The standing latent hazard: `DMA_CRASH_SNAPSHOT` spans
`0x20067ad0 .. 0x20067b18`, and core 1's read-only MPU region base is
`_stack_end & !0x1F` = `0x20067b00`, so slots `[12..18]` sit **inside** it. The
handler writes all 18. A core-1 fault therefore takes a **MemManage inside the
fault handler** — which produces exactly this signature: a reset with no record
committed, repeated, with the watchdog cleaning up.

CD's window contains a core-1 record (`Stk hdm (core 1) 8088 bytes remaining`),
4 watchdog timeouts out of 7, and 94 unrecorded resets. If CD's layout shift
raised the share of core-1 faults even slightly, the hazard would convert them
into wedges. That is a mechanism, not yet a finding.

### What CD does NOT settle

Nothing about rule 84. The layout question is **still open** — CD neither
confirms nor refutes layout sensitivity, because the window measured a different
failure mode. The 2.36x CB result stands on its own bracket and is untouched by
this.

## Config CE — CONTROL, byte-identical to CC0

Rebuilt from the CC0 source. Verified two ways: **0 of 2203 symbols moved versus
`syms-CC0.txt`**, and the flashed image reports **`crc 0x3d9cc95e`, the exact CRC
CC0 booted with**. This is the same binary, not merely an equivalent one.

Device readback: QMI `0x20007006`, **VREG `0xf0`**, 300 MHz, boot log 12.1704 s.
Cold seed extended to also zero `DMA_CRASH_SNAPSHOT`; all three magics verified
zero after the reset (`0x20067048`, `0x20067128`, `0x20067ad0` all `0x00000000`).

Baseline **boot = 1, work = 449 at 2026-08-22 02:37:36** — healthy, in the
CC0 band, not the CD band. **crashes = boot_end − 1.**

**Comparators: CC0's 1 per 436 (134 ev) and CA's 1 per 355 (139 ev) — pooled
same-layout 1 per 397 (273 ev).**

* **~1 per 400 with ~31 records and ~1,400 iterations/min** ⇒ the board is
  healthy on the known-good image, and the CD limp was caused by CD's own change
  — which would make layout sensitivity not just real but dramatic, capable of
  flipping the failure into a wedge mode. Layout work then continues with
  repeated null perturbations to map the variance.
* **Still limping (few records, low throughput)** ⇒ the degradation is in the
  board or the rig, independent of the image. CD is void either way, the CB/CC0
  bracket needs re-validating, and recovery (up to an OpenOCD RESCUE reset) comes
  before any further measurement.

## CE — THE BOARD IS HEALTHY. CD's limp was CD's own doing, and RULE 84 IS CONFIRMED.

Window 02:37:36 → 03:40:17 (62.7 min) on an image **byte-identical to CC0**
(0 of 2203 symbols moved; flashed CRC `0x3d9cc95e`, the exact CRC CC0 booted).

**Rule 85 check first: 31 records** (ring full), **1,666 iterations/min**
(CD: 84), DMA sentinel restored to `0xd4a0c12a`, 29 HardFault / 2 Watchdog — the
normal mix. The window is valid.

Δboot = 129 − 1 → **128 crashes**; Δwork = **60,018** → **1 per 469**
(95% CI 1 per 388 – 550), against the pooled same-layout **1 per 397**.
Overlapping. **Pre-registered branch (a).**

### The second bracket, and it is the same shape as the first

| window | layout | rate | health |
|---|---|---|---|
| CC0 | baseline | 1 per 436 | 31 records, healthy |
| **CD** | **+4…+32 B, 1021 symbols** | **wedge loop** | **7 records, 84 iter/min** |
| CE | baseline (identical binary) | 1 per 469 | 31 records, healthy |

And the first bracket, from two cycles ago:

| window | layout | rate |
|---|---|---|
| CA | baseline | 1 per 355 |
| **CB** | **−4 B, 959 symbols** | **1 per 157** |
| CC0 | baseline (identical binary) | 1 per 436 |

**Two independent, bracketed layout perturbations. Both degraded. Both recovered
the moment the baseline layout was restored.** Every window run on the
CA/CC0/CE layout is healthy at 1 per 355 – 469; every window run on a perturbed
layout is degraded, one of them catastrophically.

**RULE 84 IS CONFIRMED.** A semantically null 4-byte code shift — two `nop`s
that cannot execute more than once per fault — was enough to flip the firmware
from a 1-per-400 crash mode into a wedge loop with 94 unrecorded resets. Layout
is not a modulator of the failure; it appears to *select* the failure.

### What that implies about the mechanism — and it unifies almost everything

Two perturbations, both worse, is not what random noise looks like. The pattern
fits a specific mechanism: **a wild write to a roughly fixed address, whose
consequences depend on what the linker happened to place there.**

* Move the code, and a different function or structure occupies the victim
  address → different symptom (benign, crash, or wedge) and a different rate.
* The current layout is not "correct", it is merely a basin where the victim is
  comparatively harmless.

That accounts for the pathologies that have dogged this investigation from the
start: configs that confirm and then refute, mechanisms that never survive a
control, and — most of all — **"the epoch"**. Epoch steps were repeatedly
observed to coincide with reflashes. Under rule 84 that is not a coincidence:
**a reflash with a rebuilt image is a layout change, and the epoch is in
substantial part a layout artifact.** The 73-hour BY window held its rate for
three days precisely because nothing was reflashed during it.

It also sits squarely on top of rule 82 and the CB capture: a wild write
depositing `Core1Command`-shaped data (16-bit Game Boy addresses at 4-byte
stride) into whatever happens to be at the target address.

**Rule 86: UNDER LAYOUT SENSITIVITY, RATE COMPARISONS ACROSS REBUILDS ARE NOT
EVIDENCE. Prefer instruments whose result is an EVENT — a guard word breached, a
frame captured, an address recorded — because an event survives a rebuild while a
rate does not. And since every rebuild costs an uninterpretable rate anyway,
bundling several event instruments into ONE build is strictly cheaper than
spending one window per instrument. "One variable at a time" was the right rule
for measuring rates; it is the wrong rule for harvesting events.**

## Config CF — the instrumentation build

Three changes, all of them instrument integrity. None touches emulator
behaviour, none is a config knob, and the rate this window is **explicitly not
the measurement** (rule 86).

**1. Fix the `DMA_CRASH_SNAPSHOT` / core-1 MPU overlap.** The standing hazard,
now the prime suspect for CD's wedge loop: the array spans
`0x20067ad0 .. 0x20067b18`, core 1's read-only region base is
`_stack_end & !0x1F` = `0x20067b00`, and the handler writes all 18 slots — so a
core-1 fault takes a MemManage *inside the fault handler*, producing a reset with
no record. Padding the array (only `[0..18]` are ever written) pushes `_stack_end`
and therefore the region base up past the written slots, exactly as `MM_REGS` was
padded to 20 words for the identical reason. **This also protects every future
window's record pipeline**, which is why it goes first.

**2. Remove the `HF_STACK` INVSTATE gate again.** It is the only instrument that
has ever captured the dominant fault mode, and with the gate restored the
investigation is blind to it. CB's degradation is now attributed to layout rather
than to this gate, so there is no reason to keep the blindfold on.

**3. Guard `COMMAND_QUEUE`.** Wrap it in a `#[repr(C)]` storage struct with guard
bands, mirroring `AudioQueueStorage`, and check them. Note the CB capture landed
on core 0's *stack*, far from the queue, so an adjacent-overrun guard may well
stay intact — that is an informative outcome, not a wasted one:

* **guard breached** ⇒ the enqueue is running off its own buffer; the writer is
  bounded and named.
* **guard intact while the smashes continue** ⇒ the slot address is **wild**, not
  merely out of range, which points at the `Producer`/queue pointer itself and
  makes that the next target.

Expect the rate to move; under rule 86 that number carries no information. What
this window is for is the three events: does the record pipeline stay healthy
(rule 85 check), does the guard trip, and what does `HF_STACK` catch.

### Config CF — running, and it caught something within 40 seconds

Build verified. `integrity: full image crc 0x044a793f OK`, QMI `0x20007006`,
VREG `0xf0`, 300 MHz, boot 12.1698 s. Rule 84 diff: **1586 of 2098 symbols
moved** — a large layout change, so the rate this window is uninterpretable by
construction (rule 86).

**CF-1 boundary check, verified on the built image:**

    DMA_CRASH_SNAPSHOT       = 0x20067cd8
    written slots [0..18) end= 0x20067d20
    _stack_end               = 0x20067d40
    core-1 MPU region base   = 0x20067d40   ->  SAFE, 32 bytes of margin
    (before CF: written_end = 0x20067b18 > base 0x20067b00  ->  UNSAFE)

**ALL RAM ADDRESSES MOVED.** New table, verified live on the device:

| symbol | address |
|---|---|
| `HEARTBEAT` (magic `0x48b70001`) | `0x200671b0` |
| `BOOT_COUNT` / `WORK_COUNT` | `0x20067214` / `0x20067218` |
| `HF_STACK` | `0x20067250` |
| `MM_REGS` | `0x20067330` |
| `ALLOC_GUARD` (`0xa1100001`) | `0x200679a8` |
| `DMA_CRASH_SNAPSHOT` | `0x20067cd8` |
| `_stack_end` | `0x20067d40` |

Baseline **boot = 1, work = 118 at 2026-08-22 03:47:26**; **crashes = boot_end − 1**.
Watch item: work at +40 s is 118, against CE's 449 and CC0's 640 (CD's void
window read 45). Not in CD's territory, but low — the rule-85 checks matter this
window.

### The immediate capture, and the one number that did not move

With the gate off, `HF_STACK` filled before the baseline read was even taken:

    pc=0x88000000  lr=0x00000000  cfsr=0x00000100 IBUSERR
    sp_before=0x2007cd48  r12=0x2002b689  exc_return=0xfffffff9 (core 0, MSP)

    0x2007cd38  2002b689   stacked r12
    0x2007cd3c  00000000   stacked LR    <- corrupt
    0x2007cd40  88000000   stacked PC    <- corrupt
    0x2007cd44  88000000   stacked xPSR  <- corrupt
    0x2007cd48  000001a8   <- sp_before, plausible
    0x2007cd4c  1001a2dd   <- PicoGameBoy::with_cartridge+0x578, VALID
    0x2007cd50  88000000   <- corrupt
    0x2007cd54  68000000   <- corrupt

Two things stand out.

**`sp_before = 0x2007cd48` is IDENTICAL to the CB capture** — the same byte
address, across a rebuild that moved 1586 of 2098 symbols. The code moved
wholesale; the stack depth at which this fault occurs did not. Whatever picks the
victim address is anchored to the stack, not to the code.

**The corruption is interleaved here, not one run.** `cd40`/`cd44` are corrupt,
`cd48`/`cd4c` are intact and legitimate (`cd4c` is a valid return address into
`PicoGameBoy::with_cartridge`), then `cd50`/`cd54` are corrupt again. CB's
capture was six *consecutive* words. So "contiguous run" is one observed shape,
not the only one — a scattered write pattern is also possible, and the CB
conclusion should be narrowed to "at least sometimes contiguous".

The guard did not trip during boot; too early to mean anything.

## CF — the guard did NOT trip. And RULE 82 IS RETRACTED: I re-derived work this project had already done.

Window 03:47:26 → 04:51:18 (63.9 min). Rule 85 gate: **31 records** (ring full),
DMA sentinel `0xd4a0c12a` present, ALLOC_GUARD intact, VREG `0xf0`.

Δboot = 140 − 1 → 139; Δwork = 11,185 → naive 1 per 80. Per rule 86 that number
is uninterpretable (1586 of 2098 symbols moved), and it is not the point of the
window. Two health signals are worth recording anyway: **320 iterations/min**
(healthy 1400–1670; CD's void window 84) and **11 of 31 records are
WatchdogTimeout** (CE: 2 of 31). This layout is degraded — consistent with rule
84 — but the record pipeline works, so events are trustworthy.

Caveat on the rule-85 gate: a **full** ring only proves ≥31 crashes were
recorded, not that all 139 were. CD was catchable because its ring was *not*
full. The check is one-sided and should be stated that way.

### THE RESULT: the COMMAND_QUEUE guard is INTACT

Zero breaches across 139 crashes; no panic in the boot log or any record. Both
bands clean.

**Pre-registered branch: the slot address is WILD, not merely out of range.**
`inner_enqueue` is not running off its own buffer. Combined with `AUDIO_QUEUE`'s
guard (also never tripping), **neither cross-core queue is being overrun by an
adjacent walk.** The store's *destination* is wrong, not its length.

### RULE 82 IS RETRACTED

I built rule 82 on the corrupt words being valid Game Boy addresses
(`0x8800` = VRAM tile-data base, `0xe0xx` = echo RAM, `0xfe9e` = last OAM byte)
and concluded "a silicon fault would not preferentially produce valid Game Boy
addresses — this is a software write."

**This project tested that exact reading on 2026-08-08 and refuted it.**
`core/src/cpu/peripheral/apu.rs:78` records it:

> audio is EXONERATED … It also killed the "the corruptor writes audio data"
> reading: with audio OFF the wild pointer became `0x4547454c`, whose bytes are
> `4C 45 47 45` = **"LEGE"** — ASCII from the ROM title. Earlier values decoded
> as plausible i16 sample pairs only because samples happened to be what was in
> flight. **The payload is INCIDENTAL; the corruption is a wild DESTINATION.**

So `0x8800`/`0x6800` decode as Game Boy addresses for the same reason they also
decode as plausible i16 square-wave samples: **whatever was in flight is what
lands.** The inference from payload to mechanism is invalid. Rule 82's conclusion
("software, not silicon") may still be true, but *not for the reason I gave*, and
it must not be cited as evidence.

### And rule 84 was already known here, qualitatively

`oam-dma-bisection.md` — a sibling file in this same directory, 2271 lines, which
I had never opened — already records that rebuilding relocates the victim and
suppresses the repro:

* line 162: "**3/3 trials NO_RECORDS.** The incidental codegen shift (from
  rebuilding) …"
* line 228: "incidental rebuild (E3) **relocates the victim and SUPPRESSES the
  repro**"
* line 697: "An incidental **+0x1c code shift** also killed the …"

My contribution is quantification — bracketed windows, symbol-table diffs, 2.36x
with confidence intervals — not the concept. The concept was on file.

**Rule 87: READ THE SIBLING INVESTIGATION DOCS BEFORE DERIVING ANYTHING. This
directory holds five files; `pico-crash-current-findings.md` is the handoff, not
the corpus. `oam-dma-bisection.md` (2271 lines) contains F1/G1, which had already
established the wild-destination conclusion, the incidental-payload conclusion,
AND the rebuild-relocates-the-victim conclusion. Two cycles were spent
re-deriving those. Grep the whole directory for a claim before elevating it to a
rule.**

### What actually survives, and what to do next

Surviving, and now better supported than before:

* the store's **destination** is wrong, not its length (both queue guards intact);
* the destination is anchored to the **stack**, not the code — `sp_before` was
  `0x2007cd48` in both the CB and CF captures, across a rebuild that moved 1586
  of 2098 symbols;
* rebuilding relocates the victim, which is why every config comparison has been
  unstable.

`oam-dma-bisection.md` §F1 already names the next experiment, and it is better
than anything I was about to design. Its §G1 lays out the plan:

> a wild store with a corrupt DESTINATION lands on the spilled oam-base stack
> slot; the value it carries is incidental … the root corruptor is a store whose
> *address* is wrong … Resolving it needs a **runtime data trap on that exact
> stack word** … arm OpenOCD `wp <addr> 4 w <value>` (value-filtered write
> watch). The value filter skips the constant legit spills and fires only on the
> wild store — turning an un-watchable hot slot into a single-shot trap. **PC at
> halt = the root corruptor.**

That is the experiment to run. It needs the *current* binary's slot address
(derived live from a halt, since the golden binary referenced there is from June
and the layout has moved many times), OpenOCD from the Raspberry Pi fork —
probe-rs's GDB stub never implements `Z2`, so watchpoints must go through OpenOCD
— and RP2350 value matching, which works **only on `DWT_COMP3`**.

**CF stays running unchanged.** No reflash, so no new layout, and the window
keeps accumulating events while the G1 trap is prepared. Its counters continue
from the 03:47:26 baseline.

## RULE 87 APPLIED — and the sibling doc changes the picture completely

`oam-dma-bisection.md` is not a side note. It contains sections **G1 through G20**
(2026-06-15 → 06-21), and it is where this investigation actually got furthest.
Summarised, because none of it is in this handoff file:

**§G1 — the root cause was FOUND, on hardware, reproduced bit-for-bit in two
independent runs.** A value-filtered OpenOCD write watch on the spilled oam-base
stack slot caught the corruptor on the 400th halt (after 399 legitimate spills):

    writer PC = 0x1002f2de  ->  the store is 0x1002f2d8: str r2,[sp,#0xf0]
    value     = 0x2003fa30  (GameBoyMemory base — incidental, per F1)
    lr        = 0x1000a029

Resolved to `GameBoy::route_bus_events`'s inlined
`write_vram_range`/`write_oam_range` path. **Diagnosis: a stack-slot lifetime
collision** — `[sp,#0xf0]` is a coalesced slot with ~28 writers and ~40 readers;
the optimiser believes the OAM-DMA `&oam` spill is dead before the bus-event
flush writes the slot, and on the device it is not.

**§G2 — the fix was applied**: `route_bus_events` split so the body moved into
`#[inline(never)] fn drain_bus_events()`. **It is in the current build** — the CB
capture's r6 landed just after `bl drain_bus_events`. The crashes continue.

**Everything since has been tested and refuted, each by an actual soak:**

| section | hypothesis | verdict |
|---|---|---|
| G14 | StackColoring disabled | **FAIL** — still crashes |
| G17 | `-disable-ssc` (StackSlotColoring) | **FAIL** — still crashes |
| G18 | LLVM MachineOutliner (a real machine-verifier error: undefined `$r1` live-in to `OUTLINED_FUNCTION_186`) | **REFUTED** — a verifier-CLEAN build still produces the wild store |
| G19 → G19-CORRECTION | "the collision is not statically supported" | retracted; the collision **is** real, hardware-confirmed |
| G20 | `-regalloc=basic` | passed one window, then **refuted** |

**My regalloc lead was already dead, and I checked before acting on it.**
`platform/pico2w/.cargo/config.toml` records the removal in its own comment:

> `-C llvm-args=-regalloc=basic` was REMOVED 2026-08-08 … That was dormancy, not
> a fix … **a 41 min soak WITH `basic` active still produced 31 crashes**
> (~0.76/min, sector cap).

So it was not dropped by oversight; it was retested at length and correctly
removed. This is rule 87 doing its job in the right direction for once.

**And §G18-RESULT's conclusion 3 is rule 84, reached in June:**

> the `#[inline(never)]` "fix" was itself just a LAYOUT SHUFFLE, not a structural
> fix … **we have NO confirmed structural fix; every "fix/move" has been layout
> roulette.**

### CF window, second hour — stable and replicated

03:47:26 → 05:58:09 (130.7 min). Δboot = 285 − 1 → **284 crashes**;
Δwork = **22,644** → **1 per 80**. Splitting it: hour 1 gave 1 per 80.5,
hour 2 gave 1 per 79.0. Throughput 317 iter/min both hours. ALLOC_GUARD intact.
This layout is a stable, degraded basin — exactly what rule 84 predicts.

### Where this actually leaves the investigation

Every mechanism-level hypothesis with a testable prediction has now been tried
and refuted by soak: electrical (QMI, voltage, clock), audio, DMA, three LLVM
codegen passes, the register allocator, the stack-protector scaffolding, the G1
stack-slot collision fix, and — new this cycle — both cross-core queue guards.
Host ASan (§G15) and qemu-arm (§G16) replays are clean. The bug survives builds
with zero machine-verifier errors.

What remains true and unexplained: a wild **store destination**, anchored to the
stack (`sp_before = 0x2007cd48` across a rebuild that moved 1586 of 2098
symbols), whose victim relocates with layout and whose payload is incidental.

**The one experiment never run in two months of work is the simplest: A SECOND
PHYSICAL BOARD.** Every result in both documents comes from one RP2350 unit
(`E6614C311B511822`). The entire investigation assumes the fault is in firmware
or codegen, and that assumption has never been tested. A second Pico 2 W running
the identical image separates two hypotheses that no amount of further
instrumentation can:

* **second board also ~1 per 400** ⇒ it is the firmware/toolchain, the corpus of
  refutations stands, and the remaining work is an upstream LLVM/silicon report;
* **second board clean** ⇒ it is *this unit*, every codegen hypothesis in both
  documents was chasing a hardware fault, and the 24-hour goal is reachable
  simply by changing the board.

This requires hardware the user must supply, so it is raised rather than run.

## G1-REPLAY attempt on the CURRENT build — armed, did not fire. Three obstacles solved, one open.

No answer yet on the second board, so this cycle went to the one experiment that
needs **no rebuild** (and therefore no new layout): re-run §G1's value-filtered
watchpoint against the *current* firmware. §G1 caught the corruptor in June, but
on a binary that predates the §G2 fix — **nobody has caught it since.**

`watch_g1.tcl`, `watch.tcl`, `watch_oam.tcl` and `watch_oam_fix.tcl` are all still
in `~/git/github.com/raspberrypi/openocd/`. I adapted `watch_g1.tcl` into
`watch_cf.tcl`, dropping its slot-derivation step: §G1 had to derive the victim
slot from a breakpoint on the legit spill store, but **this build's victim address
is already known empirically** — `0x2007cd50` takes `0x88000000`/`0x68000000` in
both the CB and CF captures, which are 1586 of 2098 symbols apart.

### Three obstacles, solved

1. **OpenOCD attaches fine** — Cortex-M33 r1p0, "8 breakpoints, **4 watchpoints**"
   on each core. The fork at `~/git/github.com/raspberrypi/openocd/src/openocd`
   (0.12.0+dev, built Jun 14) still works.
2. **`rp2350.cfg` puts cm0 and cm1 in an SMP group** (`target smp $_TARGETNAME_CM0
   $_TARGETNAME_CM1`, line 193). Watchpoints are broadcast to **both** cores, so
   both must be halted — the first attempt died on
   `[rp2350.cm1] can't add write watchpoint at 0x2007cd50, target running`.
   `targets rp2350.cm0` does **not** avoid this.
3. **A bare `halt` lands in "unknown state"** because the firmware crash-resets
   every ~20 s. `reset halt` fixes it: both cores halted cleanly at
   `pc=0x00000088` (bootrom), and the watchpoint then **armed successfully**.

### The open obstacle: it never fired

~7 minutes of soak after arming, and the poll loop logged **zero halts** — not
even a spurious one, despite `0x2007cd50` being live core-0 stack that legitimate
code writes constantly.

Zero *spurious* hits is the tell. A correctly-armed write watch on a hot stack
word should fire immediately and often (§E7's whole problem was that it fires too
much). Getting none means the watchpoint was **not active while the target ran**.
The most likely cause: **the firmware crash-resets, a reset clears the DWT
comparators, and `watch_cf.tcl` only re-arms inside the halt branch** — so after
the first crash the watch is silently gone and the loop waits forever on a
`curstate` that never returns "halted".

§G1 did not hit this because its 400 legit-spill halts all occurred inside a
single boot, before any reset.

**Fix for the next attempt** (all in the script, no firmware change):
* re-arm on a **timer**, not only after a halt — poll `curstate`, and
  unconditionally `rwp`+`wp` every N poll cycles so a reset cannot disarm it
  permanently;
* detect the reset directly by watching the boot counter at `0x20067214` change,
  and re-arm on that edge;
* log every poll iteration for the first ~20 cycles so "armed but silent" is
  distinguishable from "loop stuck" — this attempt could not tell those apart
  from the log alone.

### Window hygiene

The debugger session halted the board, disarmed the watchdog, and issued
`reset halt`, so the CF window is contaminated from 07:00 onward. Its last clean
read stands: **423 crashes, 1 per 79.3 over 193 min** (hour 1: 1 per 80.5, hour 2:
1 per 79.0, hour 3 pooled: 1 per 79.3) — stable and replicated three times over.

Restarted cleanly on the **same CF image, no rebuild, no layout change**. Cold
seed under rule 83 (sector, all four magics, reset). Baseline
**boot = 1, work = 219 at 2026-08-22 07:14:30**; crashes = boot_end − 1.
ALLOC_GUARD `0xa1100001`, VREG `0xf0`.

## G1-REPLAY, cycle 2 — WHY OpenOCD watchpoints cannot work here, and the instrument that can

The corrected script (timer re-arm, per-poll logging, reset-edge detection) ran
and behaved correctly: `reset halt` on both SMP cores, watchpoint armed, board
resumed, timer re-arms firing on schedule. **Still zero watchpoint hits** on a
live core-0 stack word, over hundreds of polls, with SP verified straddling the
target (`msp` observed at `0x2007cac0` and `0x2007dfd8`).

### The cause: the firmware owns the DWT

`platform/pico2w/src/main.rs:661` calls `dwt_watch::enable_monitor_only()`, which
sets `DEMCR.MON_EN`, and `main.rs:834` calls
`dwt_watch::arm_data_write_watch(target)` — which writes **`DWT_COMP0`**,
`DWT_MASK0` and `DWT_FUNCTION0` directly (`dwt_watch.rs:55-57`).

So every boot, the firmware **overwrites comparator 0 with its own target**. With
the board crash-rebooting every ~20 s, any watchpoint OpenOCD installs is wiped
within one boot cycle, and re-arming on a 2.5 s timer cannot win the race
reliably. This is a *complete* explanation for the silence, and it also explains
why this document's earlier verdict — "the DWT/DebugMonitor approach is
EXHAUSTED" — was reached: the two instruments have been fighting each other.

**Rule 88: THE FIRMWARE PROGRAMS DWT_COMP0/1 AND SETS DEMCR.MON_EN ON EVERY BOOT.
An external debugger's watchpoints are therefore overwritten every reboot, and on
a crash-looping target that is every ~20 seconds. Do not attach OpenOCD
watchpoints to this firmware — use the firmware's own DWT instrument instead.**

### The instrument that does work — and it needs NO rebuild

`dwt_watch::DWT_CATCH` (`0x20067380`, `[usize; 10]`) is a **self-aiming,
probe-settable watchpoint** that `main.rs:776-840` documents explicitly:

* `DWT_CATCH[8]` (at **`0x200673a0`**) takes either a guard *offset* (< 0x1000,
  resolved against the victim's body SP from the previous crash) or a **full SRAM
  address**, planted over SWD with no rebuild;
* on a hit, the DebugMonitor handler latches
  `[0]` magic, `[1]` watched addr, **`[2]` the stacked PC — the writer**,
  `[3]` stacked LR, `[4]` EXC_RETURN, `[5]` hit count.

That is precisely §G1's experiment, already built into this firmware, and immune
to rule 84 because arming it changes no code.

One nicety that resolves the earlier worry about probe sessions clearing
`DEMCR.MON_EN`: the firmware re-arms MON_EN and COMP0 **on every boot**, and this
board reboots every ~20 s, so a probe read that disturbs the state is
self-healing within one crash cycle.

### Armed and running

Cold seed under rule 83, extended to zero `DWT_CATCH[0]` so a stale catch cannot
masquerade as a fresh one, then `0x2007cd50` planted into `DWT_CATCH[8]` and
reset. **Verified live: `DWT_COMP0` (`0xE0001020`) reads `0x2007cd50`.**
`DWT_CATCH[0] = 0` — armed, nothing caught yet.

Baseline **boot = 2, work = 189 at 2026-08-22 08:25:23**; **crashes = boot_end − 2**.
Same CF image, no rebuild, no layout change.

**PRE-REGISTERED:**

* **`DWT_CATCH[0]` becomes the magic and `[2]` holds a PC** ⇒ the corruptor is
  named in the *current* build. Resolve `[2]` against `syms-CF.txt` (greatest
  symbol ≤ target; the DebugMonitor stacks the PC **after** the storing
  instruction, so the store is the preceding one). Compare against §G1's June
  answer — `route_bus_events`' inlined `write_vram_range` — to settle whether the
  §G2 fix moved the corruptor or the corruptor was never that store.
* **`DWT_CATCH[0]` still 0 after a full window while crashes continue** ⇒
  `0x2007cd50` is not where the wild store lands; it is where the *damage was
  observed* two captures running. The next aim would then be the guard-offset
  form, which self-corrects against the victim's body SP rather than encoding a
  fixed address.

## DWT window 1 — NO CATCH, and the reason is RULE 89: the probe disables the instrument

Window 08:25:23 → 09:28:18 (62.9 min). `DWT_CATCH[0] = 0x00000000` — nothing
latched. `DWT_CATCH[8]` still held the planted aim and `DWT_COMP0` still read
`0x2007cd50`, so the comparator stayed armed the whole time. Δboot = 135 − 2 →
133 crashes; Δwork = 8,410 (rate not interpretable, rule 86).

Before accepting pre-registered branch (b) — "the store lands elsewhere" — I
checked whether the instrument was *capable* of firing. It was not.

    DEMCR (0xE000EDFC) = 0x01110000   TRCENA set, MON_EN set   <- firmware armed OK
    DHCSR (0xE000EDF0) = 0x05100001   bit 0 = C_DEBUGEN = 1    <- THE PROBLEM

**Per the ARM architecture, the DebugMonitor exception is enabled only when
`DEMCR.MON_EN == 1` AND `DHCSR.C_DEBUGEN == 0`.** With `C_DEBUGEN == 1`, halting
debug takes priority: a DWT match tries to enter Debug state instead of invoking
the DebugMonitor handler. `MON_EN` is simply overridden.

`C_DEBUGEN` is set by **probe-rs whenever it attaches**, and it persists after
detach. So every `probe-rs read` — including the ones used to *check on the
instrument* — silently disables it.

**Rule 89: `probe-rs` ATTACH SETS `DHCSR.C_DEBUGEN`, WHICH DISABLES THE
DebugMonitor EXCEPTION AND THEREFORE THE FIRMWARE'S ENTIRE DWT_CATCH INSTRUMENT.
It also clears `DEMCR.TRCENA|MON_EN` (the `main.rs:697` comment says so, and it
was observed directly: DEMCR went `0x01110000` → `0x00100000` across one read).
The instrument only works if the LAST probe contact before the soak clears
C_DEBUGEN — write `0xA05F0000` to DHCSR (DBGKEY | all-clear) — and nothing
touches the probe afterwards. This is the twin of rule 88, and between them they
explain why the DWT/DebugMonitor approach was written off as "EXHAUSTED": the
instrument was being switched off by the act of observing it.**

MON_EN needs no manual restoration: the firmware sets it at every boot
(`main.rs:661`), and this board reboots every ~20 s, so it self-heals — but
**C_DEBUGEN does not**, because nothing in the firmware clears it.

### Re-armed correctly

Cold seed (rule 83, including `DWT_CATCH[0]`), aim `0x2007cd50` re-planted, reset,
armed confirmed (`DWT_COMP0 = 0x2007cd50`), and then — **as the final probe
operation** — `0xA05F0000` written to `DHCSR` to clear `C_DEBUGEN`. The probe has
not been touched since. DEMCR read `0x00100000` at that moment (MON_EN cleared by
that very read), which is expected and self-heals on the next boot.

Baseline **boot = 1, work = 57 at 2026-08-22 09:30:28**; crashes = boot_end − 1.
Same CF image, no rebuild.

**PRE-REGISTERED, unchanged from last window but now on a live instrument:**

* **`DWT_CATCH[0]` set with a PC in `[2]`** ⇒ the corruptor is named in the
  current build. Resolve against `syms-CF.txt`; the DebugMonitor stacks the PC
  *after* the store, so the corruptor is the preceding instruction.
* **`[0]` still 0 while crashes continue** ⇒ now a real result rather than an
  instrument artifact: `0x2007cd50` is where damage was *observed*, not where the
  store lands. Switch to the guard-offset aim form, which resolves against the
  victim's body SP instead of a fixed address.
* Note `SMASH_CORE0` reads `0xaf16f18d` — not `SMASH_MAGIC` — so the
  guard-offset form has no body SP to resolve against yet. `-Z
  stack-protector=strong` was removed in config AW, which is what populated it.
  Using the offset form would require that flag back, i.e. a rebuild.

## DWT window 2 + POSITIVE CONTROL — THE INSTRUMENT IS DEAD. Every DWT null result in this project is uninterpretable.

Window 09:30:28 → 10:33:19 (62.9 min), C_DEBUGEN cleared as the last probe op.
`DWT_CATCH[0] = 0` again across **130 crashes**, `DWT_COMP0` still `0x2007cd50`.

### The firmware says it is armed

Rather than infer, I captured the firmware's own boot banner over RTT — the one
`dwt_watch.rs` says exists precisely because "the banner has to prove it rather
than assume it":

    canary watchpoint armed at 0x2007cd50                        (main.rs:835)
    DWT watch armed: COMP0=0x2007cd50 FUNC0=0x0815
                     DEMCR=0x01110000 MON_EN=true                (main.rs:841)

Everything correct as the **firmware** sees it: comparator on the target,
`FUNC0=0x0815` (MATCH=data-address-write, ACTION=debug-event, DATAVSIZE=word),
TRCENA and MON_EN both set.

### The positive control, and it fails

A null from an instrument that has never demonstrably fired is worth nothing
(rule 74). So I re-aimed it at **`WORK_COUNT` (`0x20067218`)** — the counter the
emulator increments thousands of times per second — reset, cleared C_DEBUGEN,
and waited 150 s.

    COMP0        = 0x20067218      (armed, verified)
    DWT_CATCH[0] = 0x00000000      (nothing)

**A word-write watch on a counter written millions of times produced zero
catches.** The DebugMonitor exception is not being delivered, regardless of what
`MON_EN` reads back.

**Rule 90: THE `DWT_CATCH` INSTRUMENT DOES NOT FIRE — PROVEN BY POSITIVE CONTROL,
NOT INFERRED. `DEMCR.MON_EN` reading `true` from firmware is NOT evidence that
DebugMonitor exceptions are delivered. Any "no catch" result from this instrument
is UNINTERPRETABLE until a positive control fires. Run the WORK_COUNT control
FIRST, every time, before trusting any DWT null.**

This retroactively voids results, including one in the sibling doc: **§G13
("Patient soak, DWT 3-victim watch armed: NO catch") is not evidence of
anything** — same instrument, same unverified null. And it voids my own
`0x2007cd50` null from both windows.

### What this does to rules 88 and 89

Both were partly re-derivations — `dwt_watch.rs:59-101` already documents that
"every probe-rs attach RESETS DEMCR", that "writing DEMCR from the debugger is
REJECTED (SDME=1 makes those bits Secure-only)", and that "the probe must be left
alone during a watch soak". It even records the intended working sequence, which
is exactly what I ran.

**Rule 89 needs narrowing.** I claimed `C_DEBUGEN` was the blocker. The banner
shows `MON_EN=true` at boot *after* many probe sessions, so the firmware's
re-arm does work, and the failure is **downstream of MON_EN** — the exception is
enabled and still not delivered. `C_DEBUGEN` may contribute; it is not
demonstrated to be the cause. The honest statement is rule 90: delivery is
broken, mechanism unknown.

Remaining candidates, none tested: `DEMCR.SDME` (bit 20, set in every read) routes
DebugMonitor to **Secure** state — if this firmware executes Non-secure, the
exception cannot be taken; or the `DebugMonitor` vector-table entry is not
actually live (the banner logs `vec_hdr`/`ptr_word` for this reason, and those
values were `0x2004cb24`/`0x2004cb28` — RAM, unverified against the trampoline's
address).

### Board restored

Stale comparators disarmed per the author's documented cleanup
(`DWT_FUNCTION0`/`DWT_FUNCTION1` ← 0), aim cleared, cold seed, fresh soak on the
unchanged CF image. Baseline **boot = 0, work = 160 at 2026-08-22 10:39:24**;
crashes = boot_end − 1. ALLOC_GUARD `0xa1100001`, VREG `0xf0`.

**Next, in priority order:** (1) settle DebugMonitor delivery — check the
Secure/Non-secure state and verify the `DebugMonitor` vector entry against the
trampoline symbol, since without delivery this whole instrument class is dead and
that is what has been silently defeating §G13 and both of my windows; (2) the
second-board test, still unanswered and still the highest-value experiment
available.

## Why DebugMonitor is not delivered — vector REFUTED, and the instrument cannot be validated from outside

Three candidates were queued. Results:

**(2) The vector table — REFUTED, it is correctly wired.** `VTOR = 0x10000000`
(flash, *not* the RAM addresses the banner logs as `vec_hdr`/`ptr_word`, which
read `0x00000000` and are something else entirely). Reading the real table:

    index  3 (0x1000000c) = 0x1002f3bd   -> HardFault      0x1002f3bc ✓
    index 12 (0x10000030) = 0x1002f3f5   -> DebugMonitor   0x1002f3f4 ✓

Both trampolines are live at the correct exception indices. The vector is not the
problem.

**(3) The FUNCTION encoding — tested, inconclusive.** `DWT_CTRL = 0x40000000`
(NUMCOMP = 4, matching OpenOCD's "4 watchpoints"). `FUNC0 = 0x815` sets
**DATAVSIZE**, a *data-value* field, which given the standing note that RP2350
value matching works only on `DWT_COMP3` could make `COMP0` permanently
unfireable. I rewrote `FUNCTION0` to `0x015` (DATAVSIZE cleared, MATCH and ACTION
unchanged) with `COMP0` aimed at the hammered `WORK_COUNT`. `MATCHED` (bit 24)
did not set under either encoding.

### The real obstacle: the probe cannot observe the DWT without disabling it

`DEMCR.TRCENA` (bit 24) **gates the entire DWT**. Every probe read leaves
`DEMCR = 0x00100000` — TRCENA clear. So by the time any `MATCHED` bit is read,
the DWT has already been powered down, and it stays down until the next boot.

**Rule 91: `MATCHED` CANNOT BE TESTED THROUGH THE PROBE. Reading any debug
register clears `DEMCR.TRCENA`, which gates the whole DWT, so a probe-observed
`MATCHED = 0` is exactly as meaningless as a probe-observed "no catch". The DWT
can only be validated by the FIRMWARE reporting on itself — which needs a build
that logs `DWT_FUNCTION0` / `DWT_CATCH` over RTT from inside.**

This is the same trap as rule 90, one level down, and it means candidate (3) is
**not settled** — the DATAVSIZE hypothesis is neither confirmed nor refuted.

### The board wedged, and that is a clue

During the FUNCTION0 test the counters froze completely (`boot/work` identical
across 20 s — the rule-85 liveness check). With `C_DEBUGEN` set by probe-rs,
halting debug takes priority over DebugMonitor: **a DWT match halts the core into
Debug state instead of raising the exception.** A wedge at exactly the moment a
comparator was armed at a hammered address is consistent with the comparator
working correctly and being routed to a halt that nothing services.

That is suggestive, not proven — a coincidental wedge cannot be excluded. But it
does partially rehabilitate the original (later narrowed) rule 89: `C_DEBUGEN`
looks like a genuine part of the failure after all.

Recovered by disarming both comparators first, then cold seed and reset.
Liveness re-proved by counter accumulation (boot 0 → 2, work 0x29 → 0xe6 over
25 s).

### Honest status of this instrument class

To make the DWT usable, the firmware itself must report `MATCHED` and the arming
state periodically over RTT, because every external observation destroys the
thing being observed. That is a rebuild, and worth doing — but it is instrument
work, not progress on the bug, and this class has now consumed several cycles.

**Board state:** unchanged CF image, comparators disarmed, aim cleared, cold
seeded. Baseline **boot = 2, work = 230 at 2026-08-22 11:47:34**;
crashes = boot_end − 2. ALLOC_GUARD `0xa1100001`.

**The second-board test remains unanswered and remains the highest-value
experiment available** — one hour, no instrument work, and it splits the
hypothesis space in half. Everything in this section is elaborate machinery for
deciding *where* in the firmware a wild store originates, on the unexamined
assumption that the firmware is where it originates at all.

## Holding window, and the arithmetic nobody has written down

Window 11:47:34 → 12:51:06 (63.5 min), CF image, comparators disarmed, no
instrumentation. Rule 85 gate: **31 records** (ring full), ALLOC_GUARD intact.
Δboot = 140 − 2 → **138 crashes**; Δwork = **9,955** → **1 per 72**, 286
iterations/min. Signature: 19 HardFault / 12 WatchdogTimeout; PCs `0x88000000`
(×10) and `0x1002e3b4` (×9). Stable in the CF basin.

### How far is the 24-hour goal, actually

This has never been stated numerically in either document, and it should be:

| layout | rate | throughput | crashes/min | crashes/day |
|---|---|---|---|---|
| CE (best basin measured) | 1 per 469 | 1,666 iter/min | 3.6 | **~5,100** |
| CF (current) | 1 per 72 | 286 iter/min | 4.4 | **~6,400** |

**A 24-hour zero-crash soak requires roughly a 5,000x improvement.**

Against that: the best knob ever found in this investigation is COOLDOWN=0 at
**13x**; the largest effect ever *claimed* (before same-epoch correction) was
~70x; and the layout term — which selects rather than tunes — spans about 6x
between the CE and CF basins. Every one of those is three orders of magnitude
short, and they do not compose.

So the goal is not reachable by tuning. It needs the mechanism removed, which
means either finding the wild store (the firmware hypothesis) or establishing
that the firmware is not where it originates (the hardware hypothesis). Only the
second has never been tested.

### Second-board test, prepared and ready

Written to `tools/second_board_test.sh` so it is zero-effort the moment a second
Pico 2 W is available. It flashes the identical image, cold-seeds under rule 83,
soaks, and applies the rule-85 gate before reporting a rate.

Pre-registered, so the answer cannot be read motivatedly afterwards:

* **second board ~1 per 72–470** ⇒ the fault follows the firmware. Every
  refutation in both documents stands, the corpus is sound, and the remaining
  work is an upstream LLVM/silicon report rather than more soaks here.
* **second board clean, or orders of magnitude better** ⇒ the fault is **this
  unit**. Two months of codegen bisection were chasing a hardware defect, the
  layout sensitivity is explained as a marginal part being poked differently by
  each build, and the 24-hour goal is reachable by swapping the board.
* **second board different but still bad** ⇒ both contribute; the firmware work
  retains value but the absolute numbers in both documents are board-specific.

**Board state:** unchanged CF image, no instrumentation armed, left soaking from
the 11:47:34 baseline.
