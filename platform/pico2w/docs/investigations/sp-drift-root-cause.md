# Bug #5: the `memcpy` fault family is a +8 SP drift, not memory corruption

**2026-08-30.** Status: **fault chain CLOSED. Drift mechanism OPEN.**

This document supersedes the "stray write / wild pointer" reading of the
`memcpy` fault family. Nothing writes to memory. `copy_dma_step` reads its own
**intact** frame at the wrong offsets because the stack pointer moved.

---

## 1. The chain

SP is `0x2007cb18` for every *store* in `copy_dma_step` and `0x2007cb20` for
every *load*. Each load therefore reads the slot 8 bytes high:

| load | intended | actually read |
|---|---|---|
| `ldr r0,[sp,#0x14]` | oam base `0x2004cb0c` | `count` = `1` |
| `ldr r0,[sp,#0x10]` | `self` `0x20048a8c` | `actual_src` = `0xC065` |

The WRAM branch then computes `wram_base = r0 + 0x2000`:

```
index(0xC065 + 0x2000, off=0x65, end=0x66)  ->  0xE065 + 0x65  =  0x0000E0CA
index_mut(1,           off=0x65, end=0x66)  ->  dst = 0x66
memcpy(dst=0x66, src=0x0000e0ca, len=1)     ->  precise bus error
```

**`0x0000e0ca` is NOT `ECHO_BASE + 0xCA`.** It is `0xC065 + 0x2000 + 0x65`.
Both pointers handed to `memcpy` are garbage, not just the source.

Generalising, since `actual_src = 0xC000 + progress` and the `+0x2000` is
applied to it instead of to `self`:

> **BFAR = 0xE000 + 2 x progress**
> **r12  = count + progress**

and because `memcpy` does `sub.w lr,r2,#1`, the stacked **LR = len - 1**, so
`count = LR + 1`.

---

## 2. The falsifier

The relation above was derived from the disassembly **before** any crash data
was consulted, and predicts a *different* `r12` for different `(LR, BFAR)`.
One violating record kills it.

Tested against the on-board crash sector (`0x103FF000`, dumped with OpenOCD
`dump_image`, decoded via `tools/crash_decoder.py --json`; fields `arm.lr`,
`arm.fault_addr`, `ext_regs.r12`, `ext_regs.r4`):

```
#   LR     BFAR     prog   cnt  pred   obs    verdict    pre_r4
2   0x0    0xe0ca   0x65   1    0x66   0x66   PASS       0x2007cacc
4   0x0    0xe0ca   0x65   1    0x66   0x66   PASS       0x2007cacc
6   0x0    0xe0ca   0x65   1    0x66   0x66   PASS       0x2007cacc
7   0x2    0xe0cc   0x66   3    0x69   0x69   PASS       0x2007cacc
8   0x0    0xe0ca   0x65   1    0x66   0x66   PASS       0x2007cacc
10  0x0    0xe0ca   0x65   1    0x66   0x66   PASS       0x2007cacc

RESULT: 6 PASS / 0 FAIL
```

**Record #7 is the load-bearing one**: distinct inputs (`LR=2` so `count=3`,
`BFAR=0xe0cc` so `progress=0x66`), predicted `r12=0x69`, observed `0x69`.

**Independent corroboration the formula never uses:** all six records carry
`pre_r4 = 0x2007cacc`, exactly the `memcpy` SP the +8 theory requires
(`0x2007cb20 - 0x54`). The no-drift counterfactual gives `0x2007cac4`.

---

## 3. Frame decode (the live catch)

SP is authoritative from `DWT_CATCH[9]`, which `copy_dma_step` writes with its
own SP. Prologue: `push {r4-r7,lr}` / `add r7,sp,#0xc` / `push.w {r8-r11}` /
`sub sp,#0x4c`. SP never moves again until the epilogue — the *entire* function
contains exactly two SP writes.

```
[sp,#0x10] self       0x2007cb28 = 0x20048a8c   valid heap ptr
[sp,#0x14] oam base   0x2007cb2c = 0x2004cb0c   == self+0x4080  EXACT
[sp,#0x18] actual_src 0x2007cb30 = 0x0000c065   == 0xC000+0x65
[sp,#0x1c] count      0x2007cb34 = 0x00000001
[sp,#0x20] progress   0x2007cb38 = 0x00000065
[sp,#0x44] oam(prolog)0x2007cb5c = 0x2004cb0c
[sp,#0x48] oam latch  0x2007cb60 = 0x2004cb0c
```

Every slot intact and self-consistent. `strd r11,r8,[sp,#16]` at `0x200026b8`
initialises both suspect slots (a byte scan for `STR`/`STR.W` misses `STRD`
`0xe9cd` — that mistake was made and corrected).

**Drift window:** `0x200026b8` (strd, SP=0x2007cb18) .. `0x2000270e` (ldr,
SP=0x2007cb20). Entry SP `0x2007cb88` is independently anchored by the saved
`r7` chain, so the caller is clean and the drift is confined to the body.

The only `bl` in that window is `RangeInclusive::contains` @ `0x1001599a`:
`push {r7,lr}` ... `pop {r7,pc}` @ `0x100159c0`. Statically net-zero on every
path; observed net **+8**.

**The sharpest fact:** `contains` is called *twice* in this same invocation —
VRAM (`0x200026b4`) and WRAM (`0x200026f6`) — executing the byte-identical
sequence and exiting through the same unconditional `pop`. Call 1 returned with
SP correct (proved by the `strd` landing at `0x2007cb28/2c`). Call 2 returned
+8, twelve instructions later. **Non-deterministic at the granularity of a
single `pop {r7,pc}`.**

---

## 4. Mechanism: two survivors, indistinguishable from a halted board

Control flow through the window was correct (the WRAM branch ran, `memcpy`
executed), so **SP changed while PC did not**. That kills every push/pop
mismatch variant: a skipped push or restarted pop takes PC from `[0x2007cb1c]`
and returns to `0x2000277a`.

| candidate | status |
|---|---|
| duplicate `POP {r7,pc}` base-register writeback | survives by elimination only; **no positive evidence**; no published erratum for M33 r1p0 (`CPUID=0x411fd210`) |
| transient instruction mis-fetch | survives; fits identically; better prior at 300 MHz on a 150 MHz-rated part; `cbz`(0xB1xx) and `add sp,#8`(0xB002) are adjacent encodings and the window holds two `cbz`s |

**ETM cannot separate them** — it records control flow, not SP or register
writes. Both leave identical visible flow. (It is also feature-gated off in the
shipped image, and this ETM has `TRCIDR4.NUMACPAIRS == 0`, so trace cannot be
filtered.)

---

## 5. Ruled out (do not re-derive)

- **Hardware DMA** — ch0 `AUDIO_BUF_A -> 0x50200010` (PIO TX FIFO), ch1
  `CORE0_SCALE_BUF -> 0x40088008` (SPI1 DR), ch2-15 all zero. Both
  memory->peripheral; no engine writes RAM.
- **Core 1** — MPU R0 enabled, `base 0x20067d80 limit 0x2007ffff AP=RO-priv`
  covering all of core-0's stack; core-1 `CFSR=0 HFSR=0`, it never faulted, so
  it never attempted.
- **Allocator free-list poisoning** — `ALLOC_GUARD` clean, 0 bad allocs, 0 bad
  deallocs. **This refutes the premise `platform/pico2w/src/alloc_guard.rs` was
  built on.**
- **Audio `Vec<i16>` spray** — no `(ptr, cap=0x800, len)` triple in POOL/stack/
  statics; `AUDIO_QUEUE` empty. The `cap` clamp is sound regardless.
- **Stored-image corruption** — full RAM `.data` diff vs ELF (3,500 words): one
  mismatch, `_SEGGER_RTT+0x24`, a ring write pointer. Flash sums match the ELF.
  *Note this rules out stored bytes only, NOT transient mis-fetch.*
- **NMI** — RP2350's per-core NMI mask is in the M33 EPPB block at `0xE0080000`,
  **not** SYSCFG (that is the RP2040 layout). `NMI_MASK0 = NMI_MASK1 = 0`. No
  IRQ is routed to NMI, so the historical `cpsid i` A/B had nothing to be blind to.
- **TrustZone stack banking** — zero `bxns`/`blxns`/`sg` image-wide.
- **CONTROL / MSP / PSP switching** — no `msr control`/`msp`/`psp` image-wide.
- **FPU lazy stacking** — `FPCCR.LSPEN=0`, `CONTROL.FPCA=0`,
  `EXC_RETURN=0xfffffff9`, zero `vpush`/`vpop`/`vldm`/`vstm` image-wide.
- **IT-block `pophi {r7,pc}` hazard** — the `itt hi` never fires for these
  operands (`cmp 0xC000,0xC065` makes HI false); both calls exit via the
  unconditional pop. The cb-site callee `set_r8_enum` has no IT blocks at all.

---

## 6. Retractions of earlier findings in this corpus

- **"COMBINED-WINDOW FINDING" (rule 82) is WRONG.** `0x0000e06a`, `e072`,
  `e07c`, `e084`, `e114` are not Game Boy echo-RAM addresses deposited by a
  stray write. They are `0xE000 + 2*progress` — arithmetic, not data. Every one
  implies a legal `progress <= 0x9F`.
- **`0x0000fe9e` was never a BFAR.** It is a wild PC with `CFSR=0x00000100`
  (IBUSERR), a different family. Not a counterexample to the formula.
- **The `regalloc=basic` comment in `core/src/memory/memory.rs` is STALE** — it
  claims that flag as the root-cause fix; the flag was removed 2026-08-08 as
  dormancy-not-fix.
- **`data_check::init()` is dead code**, never called anywhere in the tree, so
  `DATA_CHECK` counters are cumulative across boots, not per-boot. The `[2]=0`
  negative still holds (every live slot is a monotonic counter or first-failure
  latch, so cross-epoch mixing cannot manufacture a zero).
- **`XIP_CHECK` is void.** Its reference sum `0x98e89352` was captured under a
  different flashed image (true current `0xa58a51da`), so 100% of its 28,008
  mismatches are artifacts.

---

## 7. Scope — do not overclaim

This explains the **`memcpy` / `BFAR ~ 0xE0xx` family only**. It does **not**
explain:

- the wild-PC / IBUSERR family (`CFSR=0x00000100`): `pc=0x89000000`,
  `0x13fc21b4`, `0x0000fe9e`
- `BFAR=0x00008000` (a store through `r8` under `-Z stack-protector=strong`)

Crash records #1/#3/#5/#9 in the tested sector are these other families, and the
formula correctly does not apply to them.

---

## 8. Next step

The right instrument is a **register-only SP tripwire at the return boundary**,
giving positive evidence instead of elimination. Immediately after the WRAM
`bl contains`, before the existing `cbz`:

```asm
mov.w r12, r7
sub.w r12, r12, #0x68      ; copy_dma_step body SP = r7 - 0x68
cmp.w r12, sp
bne.w sp_shift_capture
```

`sp_shift_capture` must be naked, stackless, make no Rust calls, record
SP/expected/LR/PC/xPSR/ICI into `.uninit`, then halt. **A gated positive control
in the same image is mandatory** — without it a null result is
indistinguishable from a broken tripwire.

Caveat: this diagnostic moved `_stack_end` `0x20067d90 -> 0x20067e28`, shifting
every frame by 0x98. Given this bug's dormancy history, failure to reproduce is
not evidence of absence.

If it fires, the follow-up is a fixed-binary variant matrix (native `pop` vs
split `ldr/ldr/add sp/bx`, XIP vs SRAM, selected by a planted gate) to separate
duplicate-writeback from mis-fetch.

---

# 24-HOUR PRODUCTION SOAK AT 288 MHz — PASSED (2026-09-01 → 2026-09-02)

```
elapsed   1441 min (24.02 h)
work      3,560,324 emulator ticks
drift     0
reboots   0
```
287 sample blocks, zero read failures, work counter linear throughout
(min 12,381 / max 12,422 / mean 12,405 per 5-min block, 41.4 ticks/s, 0.3% spread).

**Configuration:** real firmware, NO `test-rom` feature. Zelda: Link's Awakening
(512 KiB, 32 banks) staged in flash. Compiled-in 288 MHz via the new `oc-288`
feature (`TARGET_SYS_HZ = 288_000_000`, PLL FBDIV 120 / VCO 1440 / POSTDIV 5),
V1_30, QMI CLKDIV 6 → SCK 48 MHz. Image CRC `0x733173ac`.

**Arithmetic.** Baseline is the corpus's controlled series (`main.rs` clock table):
300 MHz @ V1_25 produced 4 crash records in 2h15m = **1.78 crashes/hour**. Over
24.02 h that predicts **42.7 crashes**. Observed **0** → **≥14.2×** reduction at
95% confidence (expected/3); P(0 | null) = e^-42.7 ≈ 2.7e-19.

## What this does NOT establish

1. **Product validation, not a controlled experiment.** No 300 MHz control of
   comparable duration was run on this build. The comparison is against a
   historical baseline from a different build and workload.
2. **The cliff is bracketed only to (288, 300].** 288 may have as little as 0.3%
   margin. Untested against temperature and part-to-part spread — silicon timing
   moves more than that on its own across a commercial temperature range.
3. **The defect still exists at 150 MHz.** The corpus records a 150 MHz crash
   identical in every field (PC 0x2003bd98, LR 0x2000204d, r4 0x2007ccb0,
   r12 0x100143fb). This is RATE REDUCTION, not elimination.
4. **One board, one voltage (V1_30), one ambient temperature.**
5. **Zero drift is not a null result here.** The ~95 events/min drift rate came
   from the synthetic fast-DMA ROM, which did nothing but issue OAM DMA. Zelda
   issues ~1 DMA per frame, so the drift counter is far less exposed on this
   workload and a zero reading is expected. (An earlier reading of a 4-minute
   zero-drift window at 300 MHz as evidence of dormancy was WRONG: at 1.78/h
   that window predicts 0.12 events, so zero was the expected outcome either way.)

## BLOCKER: staged ROM cannot carry a rom_id

The staged ROM had to be written with **`rom_id` omitted** (header bytes
[96..128] left 0xFF). A valid `rom_id` triggers the boot save-state restore,
which is the documented core-1 boot-crash path (see memory
`project_wifi_menu_crash_repro`). With `rom_id` present the board wedged on boot:
core 0 spinning in `embassy_time::delay::block_for`, core 1 parked in
`run_core1_worker+0x1a4`, no faults, and "entering main loop" never reached.

**This is a SEPARATE, unresolved bug, and it currently blocks shipping a staged
ROM with saves enabled.** It needs its own investigation.

## Recommendation

**Ship 266 MHz (FBDIV 111), not 288.** 266 sits 8–11% below the observed cliff
versus 288's 0–4%, and the corpus already has **9 h clean at 266 with LOWER
voltage (V1_25)**. 288 passed 24 h here, but it is riding an edge whose exact
location is unknown, measured on one part at one temperature. The 11% frame-rate
cost against 300 buys margin for temperature, regulator tolerance and lot spread.
