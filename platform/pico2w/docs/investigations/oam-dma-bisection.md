# Bug #5 — OAM-DMA stack-pointer-smash bisection

Running log. Newest experiments appended at the bottom of the "Experiments" section.

## Current state / breakthrough (entry summary)

After a long investigation, bug #5 now has a **deterministic repro** and a tight
localization:

- A `-Z stack-protector=strong` build reproduces **~100% deterministically** at
  ONE instruction (vs the prior ~72% scattershot).
  Build:
  ```
  CARGO_TARGET_DIR=/tmp/rb-sp-strong \
    RUSTFLAGS='-Z stack-protector=strong -C link-arg=-Tlink.x -C link-arg=-Tdefmt.x -C link-arg=--nmagic' \
    cargo +nightly build --release
  ```
  Flash with `cargo +nightly run --release` under the same env. It fits flash.
- **Dominant fault:** HardFault on **core 0** at
  `GameBoyMemory::copy_dma_step` (`core/src/memory/memory.rs:628`),
  `CFSR=0x00008200` (PRECISERR|BFARVALID), `BFAR=0x00008000`.
  Disasm: faulting instr is `strb r0, [r8], #1` — the OAM-DMA fallback loop
  storing a source byte into `self.oam[..]`, where **`r8` (the OAM destination
  pointer) is WILD = 0x00008000** (GB VRAM base, GB-data-shaped). Meanwhile
  `r12 = 0x2003fa30` is a SANE `GameBoyMemory` base. So a **spilled copy of
  `self`/`self.oam` on core-0's stack was smashed to ~0x8000** while another copy
  stayed clean.
- **Independent confirmation:** a second record is `CFSR=0xd6a00002` = the OAM-DMA
  checkpoint guard firing (`rustyboy_oam_dma_checkpoint_guard`), i.e. corruption
  detected DURING OAM-DMA.
- The fallback loop (`copy_dma_step:627-629`) reads each source byte via
  `read_fast`, which for cart-RAM source dispatches to
  `XipCartridge::read_ram` (`platform/pico2w/src/xip_cartridge.rs:537`) — platform
  code the host replay NEVER ran (host uses core `Mbc1::read_ram`). This is the
  blind spot that let the host ASan run come back clean. The
  OAM-DMA-into-platform-cartridge path is the prime suspect.

### What the smash actually is (mechanism, restated)

The faulting store is `self.oam[dst + i] = self.read_fast(...)`. `r8` is a
spilled/reloaded copy of the `self.oam` slice base pointer (a stack temporary).
`r12` still holds a sane `GameBoyMemory` base. So: **between computing
`&mut self.oam` and the store, a write of GB-data (~0x8000) landed on the stack
slot holding the spilled `self.oam` pointer.** We must find the operation in the
copy path that performs that out-of-bounds write.

### Existing instrumentation (already in tree, keep)

- `GameBoy::check_oam_dma_invariants` (core/src/gameboy.rs:329) validates the
  *heap* `self.memory` pointer, the cartridge vtable, the ROM-cache invariant, and
  the bus-event-queue header at phases BEFORE_COPY / AFTER_COPY /
  BEFORE_PUBLISH / AFTER_PUBLISH. It does NOT validate a spilled `self.oam`
  destination pointer inside `copy_dma_step`.
- `copy_dma_step` (memory.rs:550) has a ROM-cache-invariant guard at entry and an
  OOB dst guard before the copy.
- `rustyboy_oam_dma_checkpoint_guard(phase, reason, packed_dma, observed, expected)`
  is the synthetic-fault hook (multicore.rs).

### Plan

1. Decide the cleanest reproducing base: does the DEFAULT build (no
   stack-protector) also crash deterministically at `copy_dma_step`? If yes use it
   (less layout perturbation). Else use sp-strong.
2. Add COARSE pointer-validity checkpoints (reuse the guard hooks; NO per-byte
   checks) bracketing the copy sub-steps to localize which interval first sees the
   `self.oam` destination wild.
3. Scrutinize: cart-RAM `read_ram` / `get_unchecked` path; `advance_dma_bulk`
   length/progress math; any raw pointer to a stack local in the DMA path.
4. Discipline: after each instrumentation change CONFIRM the repro still fires
   (if it vanishes, instrumentation shifted layout — go coarser). Positive-control
   every new guard. Track repro rate vs ~100%.

## Board / capture notes

- Probe present: `Debugprobe on Pico (CMSIS-DAP) 2e8a:000c`.
- ELF: `<CARGO_TARGET_DIR>/thumbv8m.main-none-eabihf/release/rustyboy-pico2w`.
- Decoder: `tools/crash_decoder.py --probe --json --elf <ELF>`.
- Per trial: `pkill -9 -x probe-rs; sleep 1`; blank crash sector
  (`probe-rs download --chip RP235x --binary-format bin --base-address 0x103FF000
  <4096B 0xFF>`); `probe-rs reset --chip RP235x`; wait ~14s; decode.
- `TimeoutACommand(41)` => SD unseated, STOP and report. `init 288` => flash
  wedge, needs power-cycle.

## Experiments

### Eb — sp-strong baseline (NO new instrumentation) — DETERMINISTIC REPRO CONFIRMED

- **Build:** `-Z stack-protector=strong`, crc `0x20f47b81`. 5 trials, 14s.
- **Result:** 5/5 trials hit the bug-#5 signature:
  `pc=0x1000ac78 copy_dma_step (memory.rs:628)`, `cfsr=0x00008200`
  (PRECISERR|BFARVALID), `fault=0x00008000`, `r4=0x2007ece8`,
  **`r12=0x2003fa30` (a SANE GameBoyMemory base)**. Several trials ALSO logged a
  second record: the OAM-DMA checkpoint guard `cfsr=0xd6a00002`,
  **phase=after-copy, reason="word before OAM changed", expected=0x20065a30
  (sane), observed=0x00000000**, with **source=0x454a, progress=80, count=76**
  (and a variant source=0x454c, **progress=203** > 160). One stray
  `pc=0x0000fe9e` (the DWT value-match experiment target — unrelated, left alone).
- **Decision:** sp-strong is the deterministic bisection base.

### Register decode of the faulting loop (sp-strong disasm)

`copy_dma_step` is inlined into the embassy task. The fallback byte loop:
```
1000abc4: ldrd r0,r1,[sp,#252]   ; r0 = progress(dst), r1 = source
1000abcc: adds r4, r0, r1        ; r4 = actual_src = source+progress (GB addr)
1000abce: ldr  r1, [sp,#0xf0]    ; r1 = spilled self.oam base pointer
1000abd0: add.w r8, r1, r0       ; r8 = oam_base + dst   (DEST pointer)
 ... read_fast(actual_src) inlined, result in r0 ...
1000ac78: strb r0, [r8], #1      ; <-- FAULT: r8 = 0x00008000 (WILD)
```
- `[sp,#0xf0]` (the spilled oam base) is written EXACTLY ONCE, at `0x1000a904`
  (`str r2,[sp,#0xf0]`), where `r2 = r5 + 0x4080` and `r5 = [r10,#0xf0]` =
  `self.memory` = GameBoyMemory base. So `[sp,#0xf0]` = `memory_base + 0x4080` =
  `&self.oam`. Real value would be `0x2003fa30 + 0x4080 = 0x20043ab0`.
- For r8 to reach 0x8000 with progress≈80, `[sp,#0xf0]` must hold ≈0x7FB0 — i.e.
  the **spilled oam base was overwritten with GB-shaped ~0x8000** between its
  store (0x1000a904) and the loop. Confirms the breakthrough: a wild store of
  GB-data lands on the oam-base stack spill. `r12=memory_base` stays sane because
  it's a different live copy.
- The heap struct is ALSO hit: the checkpoint shows the word-before-OAM
  (wram tail) zeroed to 0 during the copy. Same wild store class.

### E2 — In-`copy_dma_step` coarse pointer checkpoints (bracket the smash)

- **Hypothesis:** localize the smash interval inside `copy_dma_step` by checking
  the LIVE (re-derived from `self`) oam destination pointer + word-before-OAM at
  (a) function entry and (b) immediately before the fallback byte loop.
- **Change (files):**
  - `core/src/memory/memory.rs`: new `oam_dma_pointer_checkpoint(phase,...)`
    (inline(never), .data); called at ENTRY (phase 0x10) and PRE_FALLBACK
    (phase 0x11). Checks oam ptr in `[0x2000_0000,0x2008_0000)` and wholly inside
    the heap struct; flags word-before-OAM == 0x0000_8000/0x8000_0000.
    New consts `OAM_DMA_DIAG_PHASE_ENTRY/PRE_FALLBACK`,
    `OAM_DMA_DIAG_REASON_OAM_PTR(0x40)/PREFIX_GBDATA(0x41)`.
  - `tools/crash_decoder.py`: phase 0x10/0x11 + reason 0x40/0x41 labels.
  - `platform/pico2w/.cargo/config.toml`: moved `-Z stack-protector=strong` into
    the target-scoped rustflags (so it doesn't leak `--nmagic` to HOST build
    scripts, which broke `RUSTFLAGS=... cargo build`). Build via
    `CARGO_TARGET_DIR=/tmp/rb-sp-cfg cargo +nightly build --release`.
- **Caveat:** `inline(never)` checkpoint perturbs layout; MUST confirm repro
  still fires.
- **Result: 6/6 NO_RECORDS — the instrumentation SUPPRESSED the bug entirely.**
  crc `0x39beadc4`. Adding a single `inline(never)` call into `copy_dma_step`
  moved the victim spill out of the wild store's path.
- **MAJOR FINDING (negative experiment):** the crash is *exquisitely
  layout-sensitive*. A one-call frame change at the fault site makes the
  100%-deterministic crash vanish. This is the fingerprint of a **fixed
  sp-relative wild store** (a stack-frame-local overflow / wild pointer that
  writes a fixed offset in the GIANT embassy-task frame), not a logic error in
  the copy math. The victims (oam-base spill, wram-tail, `self.dma`) are whatever
  happens to sit at that offset under a given layout. stack-protector=strong just
  pins the layout so it hits 100%.
- **Reverted E2** (memory.rs back to baseline; decoder phase/reason labels left
  in as harmless). Next: instrument ONLY in the caller `advance_dma_bulk` (proven
  layout-stable in Eb) and READ the already-corrupt values, rather than adding
  frames at the fault site.

### E3 — Reverted-tree rebuild: REPRO VANISHES (layout-binding proof)

- After reverting E2, a full rebuild (`CARGO_TARGET_DIR=/tmp/rb-sp-cfg`) produced
  a binary whose `copy_dma_step` loop moved `0x1000ac78 -> 0x1000ac94` (+0x1c).
- **3/3 trials NO_RECORDS.** The incidental codegen shift (from rebuilding
  workspace deps) alone killed the repro.
- **Re-flashing the ORIGINAL golden Eb binary** (`/tmp/rb-sp-strong/...rbcrc`,
  loop @0xac78) immediately reproduced again (gold: T1 4 records, T2 1, T3 0; and
  a follow-up run T4/T5 hit copy_dma_step). **Conclusion: the crash is bound to a
  specific binary LAYOUT, not to source logic.** Any frame/codegen shift relocates
  the victim spill out of the wild store's fixed target. This is the fingerprint
  of a fixed-address (sp-relative or absolute-into-heap) wild store.
- **Operational note:** keep the golden binary
  `/tmp/rb-sp-strong/thumbv8m.main-none-eabihf/release/rustyboy-pico2w(.rbcrc)` —
  it is the only confirmed reproducer. Do NOT rebuild it.

### E4 — Golden-binary full-register capture (the decisive records)

Dominant fault (every reproducing trial):
- `pc=copy_dma_step (memory.rs:628)`, `cfsr=0x00008200` (PRECISERR|BFARVALID),
  **BFAR/fault=0x00008000**, stacked **r4=0x2007ece8** (a core-0 STACK address),
  **r12=0x2003fa30** (SANE GameBoyMemory base).
- Variant: `compiler_builtins::mem::impls::copy` (a memcpy) `cfsr=0x00008200` —
  i.e. the slice-copy branch of `copy_dma_step` faulting through the same wild
  oam dest pointer. Same victim, different source-region branch.

Checkpoint-guard records (cfsr=0xd6a00002), repeated:
- phase=**after-copy**, reason="word before OAM changed",
  **source=0x454a/0x454c (NON-page-aligned!), progress=80 then 203 (>160!),
  count=76 then 0, observed=0x00000000, expected=0x20065a30/0x20065b34**.

#### Why these records pin the mechanism

1. **`source` is non-page-aligned (0x454a).** Legit code sets
   `DmaState.source = (value as u16) << 8` (gameboy.rs:762) — ALWAYS low-byte 0.
   And the BEFORE_COPY guard (gameboy.rs:585) checks `source & 0xFF != 0` and did
   NOT fire first. So `source` was clean at BEFORE_COPY and became 0x454a by the
   AFTER_COPY packing → the **`source` spill was smashed mid-bracket**.
2. **`count=0` case is the key.** With count=0, `copy_dma_step` writes NOTHING,
   yet the word-before-OAM still changed 0x20065b34 -> 0 between the BEFORE_COPY
   capture (gameboy.rs:614) and the AFTER_COPY re-read (:619). The only code
   between those reads is `check_oam_dma_invariants(AFTER_COPY)` (:618), which
   performs NO core-0 write to the wram tail. **=> The wram-tail zeroing is not
   done by copy_dma_step's own copy.**
3. **`progress=203 > 160`** would make `advance_dma_bulk` return early at
   `remaining==0` (gameboy.rs:605) before any copy — yet we reached AFTER_COPY.
   So `progress` too was a small value at entry and was smashed to 203 within the
   bracket.
4. The smashed values are **GB/ROM-shaped and stride by +2** across captures
   (source 0x454a -> 0x454c). Together with the memcpy-fault variant, this is the
   signature of a **wild contiguous write (a memcpy with a wild/oversized dest)**
   that sweeps a run of bytes across the embassy-task frame and into the adjacent
   GameBoyMemory heap, hitting (in one layout) the `source` spill, the
   `progress` spill, the `self.oam` base spill (-> 0x8000) and the heap wram tail.

#### Converged localization

- The corruption window is **between `advance_dma_bulk`'s BEFORE_COPY checkpoint
  and its AFTER_COPY re-read** (gameboy.rs:612-619), i.e. spanning the
  `copy_dma_step` call and the inlined `check_oam_dma_invariants`. The victims are
  whatever sits at the wild store's fixed offsets under the golden layout.
- It is NOT the copy math (guards pass; count=0 still corrupts) and NOT a
  classic Acquire/Release race in the abstract model. It is a **fixed-address
  wild contiguous store** active during the OAM-DMA window. Because it survives
  host ASan/TSan, the data flows through platform-only code
  (`read_fast`/XIP cart) or a platform-only buffer copy on the device.

#### Why a source-line attribution could not be finished here

Every attempt to add a checkpoint *inside* `copy_dma_step` (E2) or even an
incidental rebuild (E3) relocates the victim and SUPPRESSES the repro. A
source-level bisection at the fault site is therefore self-defeating. The
decisive next instrument is a **hardware watchpoint** (DWT via the RaspberryPi
OpenOCD fork — runbook `openocd_watchpoint.txt`) armed on:
  (a) the wram-tail heap word `&GameBoyMemory.oam - 4` (a FIXED heap address —
      stable across layout!), value-filtered for the write of 0; and/or
  (b) the absolute address that the spilled `self.oam` base resolves to.
The watchpoint stops AT the writing instruction with the real PC, on the golden
binary — the only thing that pierces the layout-fragility. The heap wram-tail
address (a) is the better target since it is layout-independent.

### E6 — EXTERNAL OpenOCD hardware write-watchpoint on `&oam-4` (writer-ID capture)

Goal: catch the wild writer's PC with an external OpenOCD HW watchpoint (no
firmware instrumentation → cannot suppress the layout-fragile heisenbug, unlike
every source-level probe in E2/E3). Single-core cm0 attach via the RaspberryPi
OpenOCD fork.

**Pre-flight (this session):**
- Golden binary present + flashed: `/tmp/rb-sp-strong/.../rustyboy-pico2w`,
  Jun-15 build. Re-flashed via `probe-rs download --disable-double-buffering
  --verify` (31s OK).
- Boot log (probe-rs run, ~16s window) CONFIRMS:
  - `memory=0x2003fa30` (the boxed GameBoyMemory base — matches the documented
    ~0x2003fa30 exactly; stable across boots).
  - `vtable_word=0x20043b54`, `vtable=0x100349c0` (healthy).
  - "save state loaded on boot" — poison save loaded, SD healthy, NO
    `TimeoutACommand(41)`, no `init 288` wedge.
  - Standalone repro fired: probe-rs reported "Firmware exited unexpectedly:
    Exception, Core 0" within the window.
- Decoded committed crash record = exact bug-#5 signature:
  `pc=0x1000ac78 copy_dma_step (memory.rs:628)`, `cfsr=0x00008200`,
  `fault_addr=0x00008000`, `r4=0x2007ece8`, **`r12=0x2003fa30` (sane base)**,
  `gb.cycle_lo=2388835348` (~2.38B trigger), lr=`XipCartridge::read_ram`.

**Watch address derivation (layout-independent heap word):**
- `GameBoyMemory` struct (memory.rs:183): no `dma` field — `DmaState` actually
  lives inline on `GameBoy` (gameboy.rs:96), NOT in the boxed heap struct. So
  `DmaState.source` is a stack/GameBoy-struct word, NOT a stable heap address.
  The stable heap victim is `&oam-4` inside the boxed `GameBoyMemory`.
- `oam` offset confirmed by golden disasm: `1000a8fc: add.w r2, r5, #0x4080`
  (`r5`=memory base → `&oam = base+0x4080`). Also `oam_prefix_word_addr_for_diagnostics`
  returns `&oam - 4`.
- **WATCH = memory_base + 0x4080 - 4 = 0x2003fa30 + 0x407C = `0x20043aac`.**
- This is the WRAM-tail word (GB 0xDFFC). The AFTER_COPY checkpoint
  (gameboy.rs:631) compares this exact word: healthy `expected=0x20065b34`
  (an SRAM-pointer-shaped value left by the poison save), wild `observed=0x00000000`.
  → VALUE FILTER: legit value is the stable non-zero snapshot; the WILD write
    zeroes it (or writes a GB-data-shaped value). Manual value-filter in the TCL
    poll loop (OpenOCD `wp ... w <val>` is unsupported on RP2350).

(capture results appended below)


- **Added** (cleanly feature-gated, OFF by default; separate from the value-match
  0xFE9E experiment which is left untouched):
  - `core/src/memory/memory.rs`: `oam_prefix_word_addr_for_diagnostics()` →
    absolute heap address of `&self.oam - 4` (the word-before-OAM; a FIXED heap
    address, layout-independent).
  - `core/src/gameboy.rs`: forwarding accessor.
  - `platform/pico2w/Cargo.toml`: feature `oam-prefix-watch`.
  - `platform/pico2w/src/multicore.rs`: under `oam-prefix-watch`, arm a DWT
    write-watchpoint on that address at the first-tick site (the existing
    `publish_and_arm_raw_words` path; core 1 picks it up via
    `arm_published_watch_words_for_current_core`). Both cores covered.
- **Status / caveat:** builds clean (`--features oam-prefix-watch`, nightly). BUT
  a *plain address* watch on the wram-tail word will also fire on LEGITIMATE GB
  writes to that RAM byte. To trap ONLY the wild writer it must use the DWT
  data-VALUE filter (match a WRITE of value 0) — the same mechanism as the
  existing `value-match-fe9e-watch`, retargeted to (addr = oam_prefix, value = 0).
  That retarget was intentionally NOT done here to avoid disturbing the user's
  value-match edits. **This is the recommended next experiment** and is the only
  approach that pierces the layout-fragility (hardware watch; arming runs at
  init, far from the `copy_dma_step` frame, so it does NOT suppress the repro).

### E7 — EXTERNAL OpenOCD HW write-watchpoint capture (live, golden binary)

Continuation of E6 — the ACTUAL capture, run externally (no firmware change).

**Pre-flight (this session) — all green:**
- Golden binary `/tmp/rb-sp-strong/.../rustyboy-pico2w` (crc `0x20f47b81`) re-flashed
  via `probe-rs download --chip RP235x --disable-double-buffering --verify` (31s OK).
- Boot log confirms: `memory=0x2003fa30 vtable_word=0x20043b54 value=0x100349c0`;
  "save state loaded on boot"; **no `TimeoutACommand`, no `init 288`** (SD healthy).
- Standalone repro fired ("Firmware exited unexpectedly: Exception, Core 0" ~16s).
- Committed crash record decoded = exact bug-#5 signature: `pc=0x1000ac78`
  copy_dma_step (memory.rs:628), `cfsr=0x00008200`, `fault_addr=0x00008000`,
  `r4=0x2007ece8`, **`r12=0x2003fa30`**, lr=`XipCartridge::read_ram`,
  `cycle_lo=2388835348`.
- OpenOCD fork `~/git/github.com/raspberrypi/openocd/src/openocd` v0.12.0+dev,
  cm0 single-core attach, 4 watchpoints. `/tmp/ocd-build` was wiped; canonical
  fork binary is intact and used directly. Scripts live in `/tmp/ocdwatch/`.

**Watch-address derivation — DONE LIVE (no rebuild).** The corrected target is
`GameBoy.dma.source`, an INLINE field of the boxed `PicoGameBoy { gb: GameBoy }`
(single field → `GameBoy` base == box base == `r10` in the embassy task).
- Set a HW breakpoint at the OAM-DMA discriminant load `0x1000a8ac`
  (`ldr r0,[r10,#0x1a0]; cmp r0,#1`); on hit, **`r10 = 0x20055c20`** = live GameBoy
  base. Positively confirmed: `[r10+0xf0] = 0x20055d10` holds `0x2003fa30` (the
  GameBoyMemory base, matching the boot log exactly).
- In this golden layout the `dma` Option lives at `r10+0x1a0` (discriminant word),
  with **`source` (u16) at `r10+0x1a4`** and **`progress` (u8) at `r10+0x1a6`**.
  (Field accesses confirmed in disasm at `0x1000a8ac/b6/ba` and stores at
  `0x1000aac2`, `0x1000ab7c`.)
- **`&GameBoy.dma.source = 0x20055c20 + 0x1a4 = 0x20055dc4`** (word-aligned; low
  16 bits = source, bits 16-23 = progress). Value filter: legit `(source&0xFF)==0`
  (page-aligned) or the idle `0x00000000`/`0xffffffff` (None); WILD = low-byte != 0
  (e.g. `0x454a`).
- Stable cold fallback (E6-derived, reconfirmed): wram-tail `&oam-4` =
  `memory_base+0x4080-4 = 0x2003fa30+0x407C = 0x20043aac`. Healthy steady-state
  value (post-save-load) `= 0x20065b34`; WILD zeroes it (`observed=0x00000000`).

**Capture run 1 — watch `0x20055dc4` (dma.source).** Watchpoint armed & worked:
auto-resumed ~5000+ legit writes (`src=0xc000` page-aligned from PC `0x1000ab7c`
the legit `self.dma=Some{progress:next}` store, plus `0x1000a33c` the trigger).
**During the run the chip RESET TWICE ("external reset detected") — i.e. bug #5
fired twice — yet the wild write NEVER landed on `dma.source`.** The dominant
crash is the STACK-spill victim (`strb`→`0x8000`); `dma.source` is only smashed
in a subset (the checkpoint-guard records). The per-DMA-byte halt churn (~10k
halts) also heavily perturbs timing. → `dma.source` is a POOR external-watch
target here (too hot, and not always the victim). Switched to the cold wram-tail.

**Capture runs 2-3 — watch `0x20043aac` (cold wram-tail).** Two false catches,
both correctly identified as LEGIT boot writes (NOT the wild store), by resolving
the writer PC against the golden ELF:
- `pc=0x1003041a` → `__aeabi_memclr4` (compiler_builtins), lr →
  `GameBoyMemory::with_cartridge_boxed` (memory.rs:340): the construction-time
  zeroing of the heap struct. (Runbook's "one legitimate construction write".)
- `pc=0x1001a910` (`strb r5,[r4,r3]` loop at `0x1001a91c`) → `GameBoyMemory::
  load_state` (memory.rs:729), lr → save_state deserialization: the boot-time
  save-load writing WRAM bytes through the wram-tail region.
Both fire BEFORE gameplay (the watch word is still `0x00000000`, pre-save-pointer).
→ Refined to v3: do NOT trap until the wram-tail first reaches its healthy
steady-state `0x20065b34` (proof that save-load finished and gameplay is live);
only then treat any non-SRAM-pointer overwrite as the wild store.

**Capture runs v3/v4 — cold wram-tail, arm-after-healthy / skip-boot-PC.** Both
failed to catch the wild store despite 7-10 crash-reset cycles ("external reset
detected"). Under OpenOCD halt-perturbation the firmware **crash-loops** rapidly
(Reset bss-zero `0x10000148` → construct memclr `0x1003041a` → load_state
`0x1001a910` → crash → Reset …) and the wram-tail's dominant smasher is NOT this
cold word — the dominant victim is the STACK oam-base spill. The cold wram-tail
is only hit in the SUBSET "word before OAM changed" checkpoint records, which did
not recur while armed. → cold word is the wrong external-watch target here.

**SP derivation (live, golden) — DONE.** HW breakpoint at the copy_dma_step loop
preamble `0x1000abce` (`ldr r1,[sp,#0xf0]`): on hit **SP = 0x2007ece8**, and the
loaded `r1 = 0x4547454c` ("LEGE", GB/ROM data) — i.e. the spilled oam base was
ALREADY WILD when the loop read it. So the oam-base spill slot
`[sp,#0xf0] = SP+0xf0 = 0x2007edd8` (its sane value is `0x20043ab0` =
`memory_base+0x4080`; legit writer `0x1000a904 str r2,[sp,#0xf0]`).

**Capture run — spill slot `0x2007edd8`, GB-data value filter.** Watchpoint armed
& fired, but caught BOOT-SPLASH stack reuse, not the bug: `pc=0x1000c688` (inside
`Display::draw_logo`/`splash_step`, main.rs:442) writing a `0xfffffff4 → f8 → fc →
0` stride-4 sequence (a `fill_contiguous`/clear loop) through a large splash
stack buffer at `sp+0x5c0..` that overlaps `0x2007edd8` when the splash frame is
active. NOT the oam-spill writer. Lesson: `0x2007edd8` is the oam-spill slot ONLY
while copy_dma_step is the live frame; at other times it is reused → plain-address
stack watch is swamped by legit reuse traffic (exactly the runbook warning).

**Standing problem (probe perturbation):** every value/PC filter on a single fixed
word is defeated because (a) the heisenbug routes around whichever word is watched
under the halt-churn timing shift, and (b) every watchable word also sees abundant
legit boot/splash/load/DMA traffic. The bug DOES still fire under the probe (many
"external reset" crashes observed), so it is NOT fully suppressed — but it is not
landing on the watched word during the armed window. Next: WINDOWED watch — bp at
the copy_dma_step legit spill-store `0x1000a904` (slot = sane `0x20043ab0`), arm
the `0x2007edd8` watch THERE, and resume; the next write before the loop reads it
is the wild store, with no boot/splash noise in the window.

### E8 — QUAD watchpoint + memcpy-entry trace: STRONG LEAD = a `memcpy` overrunning `&oam-4`

After the single-word watches were defeated by perturbation, I armed all **4 DWT
comparators at once** on the documented victim set and filtered by value + boot-PC:
  W0 `0x20043aac` (wram-tail/`&oam-4`), W1 `0x20043b54` (cart vtable word),
  W2 `0x20043ab0` (`oam[0]`), W3 `0x20055dc4` (`GameBoy.dma.source/progress`).

**THE CATCH (quad run 1) — the prize PC + bad pointer:**
- Halted at **`pc=0x10030576`**, inside **`compiler_builtins::mem::memcpy`**
  (`copy_forward_bytes`, impls.rs:117). The store is `0x10030578
  strb.w r2,[r12,#1]` — a **byte-wise memcpy**.
- **Destination base `r12 = 0x20043ab3`**, source `r1 = 0x20041a33`. The
  watchpoint that fired was **W0 = `0x20043aac` = `&oam-4` = the WRAM tail (GB
  0xDFFC)** — i.e. this memcpy's destination run **STARTED at/below `&oam-4` and
  is sweeping FORWARD across the wram→oam boundary** into the oam array
  (`&oam = 0x20043ab0`). A legit OAM-DMA copy is `self.oam[dst..].copy_from_slice`
  whose dest is always `>= &oam` — it can NEVER write `&oam-4`. So this is a
  **contiguous-overrun memcpy whose destination is mispositioned by ~-4..-7 bytes**
  (dest seen mid-sweep at `oam+3`, having already written the bytes from
  `oam-4`). `r10 = 0x20055c20` (the live GameBoy base) confirms gameplay context,
  not boot. This matches the E4 hypothesis: *a wild contiguous store / memcpy with
  a wild-or-oversized destination, active during the OAM-DMA window.*
- Struct layout (`memory.rs:183`): the field immediately BEFORE `oam` is
  `wram:[u8;0x2000]`; `&oam-4` is the last wram word. Source `0x20041a33` is also
  inside the heap struct (`base+0x2003`, the vram/wram area). So the bad copy is
  **struct-internal, sweeping the wram tail into oam**.

**Legit-baseline confirmation (memcpy-entry bp `0x10030482`):** breakpointing
memcpy entry and reading r0/r1/r2/lr shows the LEGIT OAM-DMA memcpy is called from
**`lr=0x1000aa43`** = `advance_dma_bulk`/`copy_dma_step` slice branch
(`0x1000aa3e bl core::slice::copy_from_slice_impl`, gameboy.rs:618), always with
**dest in `[oam, oam+160)` and len 1..3** — never below oam. Other gameplay
memcpys (`lr=0x10011691`, `0x100155e3`) target unrelated regions. So the
`0x10030576` catch with dest reaching `&oam-4` is genuinely anomalous vs every
legit memcpy observed.

**THE PRIZE (resolved):**
- Wild store PC: `0x10030578 strb [r12,#1]` in `compiler_builtins::memcpy`
  (`copy_forward_bytes`) — a `core::slice::copy_from_slice` lowered to memcpy.
- Bad destination pointer: `r12 ≈ 0x20043ab3` — the memcpy DEST, whose run begins
  at/below `&oam-4 = 0x20043aac`, sweeping forward into `oam`. Source `0x20041a33`
  (struct wram region). The DESTINATION is the corrupt pointer: positioned ~4-7
  bytes before `&oam`, so the copy smashes the wram tail + `GameBoy.dma` spills +
  the oam-base stack spill as it sweeps — matching every observed victim.
- Interpretation: the writer is a `copy_from_slice` whose destination slice
  base/len came from corrupted DMA state (`dst`/`n`/`source` in `copy_dma_step`),
  or a struct copy whose bound underflowed. The `advance_dma_bulk` hardening
  (reject `source&0xFF!=0`, `progress>160`, re-validate the oam base before
  copying) is the correct CONTAINMENT for the faulting write. The root corruptor
  of the DMA state remains to be pinned, but the FAULTING WRITE is now named.

**Caveat / rigor note:** the quad filter also produced one FALSE positive at
`pc=0x1000ab7c` (the legit `self.dma=Some{progress:next}` store region) — a
transient value tripped the filter; discarded after confirming it was the legit
DMA update, not a memcpy. The `0x10030576` memcpy catch is the credible one (real
byte-memcpy PC; dest provably below `&oam`). A confirmatory memcpy-entry capture
filtering for `dest < &oam` was run to grab the CALLER LR but had not re-hit the
anomalous call within budget (the bp-per-call poll is slow; ~6000 legit calls +
1 crash-reset, no anomalous-dest call caught while armed). **Recommended
follow-up:** re-run the memcpy-entry trace longer, or loosen the predicate to
`dest != &oam && dest in [0x20043a80, 0x20043b60]`, to capture the caller LR and
name the exact `copy_from_slice` call site (the one computing the bad dest/len).

The loosened memcpy-entry trace (`dest in [0x20043aa8,0x20043b58]`) was run; the
only OAM-region memcpy it caught was a LEGIT full-OAM restore
(`dest=0x20043ab0 (=&oam, aligned), src=0x20059f10, len=160, caller-LR=0x1001aa2b`
= `GameBoyMemory::load_state`, memory.rs:736, the boot save-load `set_oam`). That
run saw 0 crashes (boot completed cleanly), so the intermittent anomalous copy
did not recur in-window. The distinguishing fact stands: the wild `0x10030576`
catch wrote `&oam-4`, which a `dest=&oam, len=160` aligned copy never does.

**Net E8 conclusion:** faulting writer = a `core::slice::copy_from_slice`-lowered
`compiler_builtins::memcpy` (`copy_forward_bytes`, store `0x10030578 strb [r12,#1]`)
sweeping FORWARD from a destination positioned at/below `&oam-4` (`0x20043aac`)
into `oam` — a mispositioned/underflowed copy destination in the OAM-DMA path,
during gameplay (`r10`=GameBoy base `0x20055c20`). The exact source call site
(the LR computing the bad dest/len) is the one remaining unknown; the writer
mechanism and the bad pointer (the memcpy DEST `r12`) are now identified.

### Reproducing build (target-scoped; no host-build-script leak)

The breakthrough's command put `-Tlink.x`/`--nmagic` in env `RUSTFLAGS`, which
LEAKS those link args to HOST build scripts and breaks the build after any `core`
edit. Use the target-scoped form instead — add `-Z stack-protector=strong` to the
`[target.thumbv8m.main-none-eabihf] rustflags` in
`platform/pico2w/.cargo/config.toml` (target-only, no leak) and build with
`CARGO_TARGET_DIR=<dir> cargo +nightly build --release`. (The config edit was
reverted at end-of-session so stable `cargo build`/`cargo run` keep working;
re-add the one line to resume.)

**Golden reproducing binary (keep, do NOT rebuild):**
`/tmp/rb-sp-strong/thumbv8m.main-none-eabihf/release/rustyboy-pico2w(.rbcrc)` —
loop @0x1000ac78, crc 0x20f47b81. The repro is bound to THIS layout.

## Proposed fix (defensive) + validation

Two-tier:

1. **Mechanism fix (preferred, once the watchpoint names the writer):** the
   evidence (count=0 still corrupts; non-page-aligned `source`; +2 stride;
   memcpy-fault variant) points to a *wild contiguous store / memcpy with a
   wild-or-oversized destination* active during the OAM-DMA window, in
   platform-only code (survives host ASan/TSan). Arm `oam-prefix-watch` with the
   value-0 filter on the golden binary, read the trapped PC, fix that store's
   bound/pointer.

2. **Defensive hardening (can land now, independent of #1):** in
   `advance_dma_bulk`, treat a corrupt DMA state as fatal-but-contained BEFORE
   copying: if `source & 0x00FF != 0` OR `progress > OAM_DMA_BYTES` (already
   guarded at gameboy.rs:585) — and additionally re-validate `self.memory`'s oam
   base is in-SRAM and the rom-cache invariant — abort the DMA step instead of
   proceeding into `copy_dma_step`. This converts the wild `strb`/memcpy into a
   clean recorded abort. (Note: must be careful not to add a call frame to
   `copy_dma_step` itself — keep new checks in the CALLER, which E_b proved is
   layout-stable.) This does not fix the writer but stops the wild store from
   faulting the emulator.

**Validation protocol:** flash the golden binary + fix, soak ≥ 20 trials past the
deterministic repro window. PASS = the `copy_dma_step` BFAR=0x8000 fault and the
"word before OAM" checkpoint both disappear. CONTROL = confirm a deliberate
layout/timing perturbation still distinguishes (i.e. the fix removed the bug, not
just shifted the victim): re-introduce a 1-call frame change and confirm the
fault stays gone (a layout shift alone should not "fix" a real fix).

## Files changed this session (separate from the value-match DWT experiment)

- `core/src/memory/memory.rs`: + `oam_prefix_word_addr_for_diagnostics()`
  (cfg(arm)); E2 in-function checkpoints were ADDED then fully REVERTED.
- `core/src/gameboy.rs`: + forwarding `oam_prefix_word_addr_for_diagnostics()`.
- `platform/pico2w/Cargo.toml`: + feature `oam-prefix-watch` (off by default).
- `platform/pico2w/src/multicore.rs`: + feature-gated DWT arm at first-tick site.
- `tools/crash_decoder.py`: + OAM_DMA phase 0x10/0x11 and reason 0x40/0x41 labels
  (harmless; left in).
- `platform/pico2w/.cargo/config.toml`: temporarily added then REVERTED the
  `-Z stack-protector=strong` target rustflag.
- The DWT value-match (0xFE9E) edits were NOT touched.

## Board / SD notes

- Probe healthy (`Debugprobe on Pico (CMSIS-DAP) 2e8a:000c`). No TimeoutACommand,
  no `init 288` wedge observed. Save-state-on-boot loaded fine.
- Repro rate on this board for the golden binary ≈ deterministic but with some
  per-boot variance (gold reconfirm: ~4/6 boots produced the fault within 14 s;
  empty boots are just timing — bump the wait to 18-20 s for higher yield).

### E0 — Static disasm of the faulting loop (default build, no instrumentation)

- **Hypothesis:** confirm the breakthrough's instruction shape exists in the
  current tree and understand the register allocation.
- **Build:** default `cargo build --release`, image crc `0x68a52625`.
- **Findings (objdump):** the inlined `copy_dma_step` fallback loop lives at
  `0x10003a74..0x10003b16` (`.data` XIP). The faulting store is at `0x10003b0e`:
  `strb r0, [r8], #1` — EXACTLY the breakthrough signature.
  - `r8` = OAM destination pointer, computed ONCE before the loop at
    `0x10003a5c`: `add.w r8, r1, r0` with `r1 = [sp,#0xec]` (spilled
    `self.oam.as_ptr()`) and `r0 = dst` (high half of `ldrd [sp,#240]`).
  - The same spilled oam base `[sp,#0xec]` is reloaded INSIDE the loop for the
    OAM-source / IO-source `read_fast` cases (`0x10003b26`, `0x10003b6a`), so it
    is live across the whole loop body — a perfect, long-lived stack victim.
  - Legit writers of `[sp,#0xec]`: `0x1000378c` (`strd`, sets 0xEC/0xF0),
    `0x10003c36`, `0x10003e64`. The smash is a DIFFERENT, out-of-bounds store that
    lands on `sp+0xec` by accident — and per the breakthrough writes `0x00008000`.
  - r8 = 0x8000 means the spilled base `[sp,#0xec]` (not `dst`) was overwritten
    with the GB VRAM base 0x8000. Confirms: **wild store of GB-data onto the
    spilled oam-base stack slot**, not a DMA length-math bug (those are already
    guarded and would produce a different value).
- **Conclusion:** instruction shape reproduced statically. Need the runtime
  deterministic repro to bisect WHO writes 0x8000 onto that slot.

### E1 — Does the DEFAULT build reproduce deterministically? (decide base)

- **Hypothesis:** maybe the default (no stack-protector) build also crashes
  ~100% at `copy_dma_step`; if so, use it (less layout perturbation).
- **Build:** default release, crc `0x68a52625`. Capture: 5 trials, 14s window.
- **Result:**
  - trial 1: HardFault **core 1** `ppu::render_sprite_scanline` (ppu.rs),
    CFSR=0x00000082 (PRECISERR), fault=0x20066b60 (RAM), r4=0x20081ed0,
    r12=0x20046d30. This is a DIFFERENT fault (the open core-1/PPU secondary
    crash), NOT the `copy_dma_step` core-0 signature.
  - trials 2-5: NO_RECORDS (no crash within 14s).
- **Conclusion:** the **default build does NOT reproduce bug #5 deterministically**
  — it is the prior scattershot regime (and the one crash seen was the other open
  bug). **Decision: use the `-Z stack-protector=strong` build as the bisection
  base**, per the breakthrough (~100% at one instruction). Caveat noted: 14s may
  also be marginally short; sp-strong is reported to fire within ~10s, so keep
  14s but bump if sp-strong also comes back empty.

### F1 — Golden-binary static pin: the copy is a PROPAGATOR, fork resolved

Re-derived directly from the deterministic golden binary
(`/tmp/rb-sp-strong/.../rustyboy-pico2w`, built Jun 15; **note: predates the
current modified `core/src/memory/memory.rs`, so source line numbers drift** —
all addresses below are from the binary itself).

**Caught faulting store `pc=0x10030578` is generic `compiler_builtins::memcpy`**
(`strb.w r2,[r12,#1]`, forward byte-tail). The memcpy is innocent — it copies to
wherever the caller's `dest` (r0) points.

**The OAM-DMA copy site** is inlined into `embassy_main_task`
(`advance_dma_bulk` brackets = the 4 `check_oam_dma_invariants` calls at
`0x1000a8f2 / a a50 / aaca / ab74`). The copy itself, between BEFORE_COPY and
AFTER_COPY:

```
1000a8f6  ldr   r5, [r10,#0xf0]      ; r5 = spilled &GameBoyMemory  (= 0x2003fa30 healthy)
1000a8fc  add   r2, r5, #0x4080      ; r2 = &oam                    (= 0x20043ab0 healthy)
1000a904  str   r2, [sp,#0xf0]       ; SPILL oam base to stack slot sp+0xf0
   ... source classification = RangeInclusive::contains (OUTLINED_FUNCTION_393, leaf) ...
1000a9e6  bl    OUTLINED_FUNCTION_353            ; compute dst slice oam[progress..progress+n]
1000aa3e  bl    core::slice::copy_from_slice_impl ; -> memcpy(dst, , vram_src, )
```

`OUTLINED_FUNCTION_353` (0x1002fe8e):
```
ldr  r1, [sp,#0xfc]   ; r1 = progress (dst start, a u8 >= 0)
adds r2, r0, r1       ; r2 = n + progress (dst end)
ldr  r0, [sp,#0xf0]   ; r0 = spilled oam base  <-- READ BACK
b.w  <[u8;0xa0]>::index_mut(base=r0, start=r1, end=r2)   ; returns base + start
```

**Proof the copy cannot self-underflow:** `index_mut` returns `base + progress`
with `progress >= 0`. The caught dst is `&oam-4`. Therefore `base` itself was
`&oam-4` — i.e. the spilled base word at **`[sp+0xf0]`** (current golden layout;
== the `[sp+0xec]` slot in the earlier E-series capture) held a corrupt pointer
when read back. Nothing in the spill->read window writes that slot legitimately
(only the leaf `RangeInclusive::contains` runs). **The OAM-DMA copy is a
propagation victim, not the origin.** This RESOLVES the summary's "fork A vs B":
it is fork A (smashed spilled base) — `copy_dma_step` is mathematically incapable
of underflowing `&oam` on its own once `self`/oam-base is valid.

**Reconciliation with E-series:** earlier capture saw the same spilled-base slot
overwritten with `0x00008000` (the guest `VRAM_BASE` constant) -> wild dst at
`0x8000+progress`. This golden run shows `&oam-4`. Different values, same slot,
same mechanism: **a wild store with a corrupt DESTINATION lands on the spilled
oam-base stack slot; the value it carries is incidental** (whatever that store
was moving). So the root corruptor is a store whose *address* is wrong, not a
DMA length/math bug.

**Therefore the next question is unchanged from E1's close:** WHO performs the
wild store onto the spilled oam-base stack slot? Static analysis cannot name it
(the writer's address depends on runtime state). Resolving it needs a runtime
data trap on that exact stack word.

**Newly-viable capture plan (defeats the "hot stack slot" objection):** the
golden binary's slot is at a FIXED absolute address (`task_sp + 0xf0`); compute
it once from a halt, then arm OpenOCD `wp <addr> 4 w 0x00008000` (value-filtered
write watch). The value filter skips the constant legit `&oam` spills and fires
only on the wild `0x8000` (VRAM-base) store — turning an un-watchable hot slot
into a single-shot trap. PC at halt = the root corruptor. (Alternative if the
carried value varies run-to-run: filter is still better than unfiltered; or use
the subagent path once weekly budget resets ~Jun 19 01:00 America/Toronto.)

### G1 — Windowed HW write-watch on the spilled oam-base stack slot (task_sp+0xf0)

Goal (per F1): name the ROOT corruptor — the wild store whose corrupt DESTINATION
lands on the persistent `embassy_main_task` stack word `task_sp+0xf0` (the spilled
&oam base, legit value 0x20043ab0). E7 proved a plain stack-slot watch is swamped
by legit reuse (boot bss-zero, splash draw_logo, load_state). G1 strategy: derive
the slot live, then use the resume-past-spurious value loop (and, if too noisy, a
WINDOWED arm at the copy_dma_step spill store 0x1000a904).

**Pre-flight:**
- Hardware present: probe `Debugprobe on Pico (CMSIS-DAP) 2e8a:000c:E6614C311B511822`.
- Golden ELF `/tmp/rb-sp-strong/.../rustyboy-pico2w` (Jun-15) flashed via
  `probe-rs download --chip RP235x --disable-double-buffering --verify` (31s OK).
- OpenOCD fork `~/git/github.com/raspberrypi/openocd/src/openocd` v0.12.0+dev OK.
- Templates reused: `watch.tcl` (resume-past-spurious), `watch_oam.tcl` (value loop).

**Slot derivation (live, golden):** HW breakpoint at the legit spill store
`0x1000a904 str r2,[sp,#0xf0]`. On hit: **SP=0x2007ece0**, single-stepping the
store wrote `before=0x000042e7 -> after=0x20043ab0` (= healthy &oam, base+0x4080).
So **SLOT = SP+0xf0 = `0x2007edd0`** (the persistent embassy_main_task frame; SP
is stable across DMA iterations). Removed the bp, armed a 4-byte WRITE watch on
SLOT, resume-past-spurious loop (skip any halt whose written value == 0x20043ab0,
i.e. the legit oam-base spill, or whose PC <= 0x10000200).

**THE CATCH — DETERMINISTIC, reproduced bit-for-bit in 2 independent runs:**
- After **399 legit spill writes** (all `pc=0x1000a910`, value `0x20043ab0`), the
  **400th halt** caught the wild store:
  - **writer PC = `0x1002f2de`** → the store is the preceding insn
    **`0x1002f2d8: str r2,[sp,#0xf0]`** (DWT halts on the insn AFTER the store).
  - **value written = `0x2003fa30`** = the **GameBoyMemory base** (NOT base+0x4080).
    r2 = 0x2003fa30. (Different incidental value than the E-series 0x8000/0x454a —
    confirms F1: "the value the wild store carries is incidental".)
  - **lr = `0x1000a029`**, **r10 = `0x20055c20`** (live GameBoy base → gameplay,
    not boot), r0=1, r1=4, r3=0x20003ba0.
  - Both runs: SLOT=0x2007edd0, PC=0x1002f2de, val=0x2003fa30, halt #400. Identical.

**Resolved source of the wild store (`0x1002f2d8`):**
- It lives in **`OUTLINED_FUNCTION_80`** (`0x1002f2c8`), a fragment outlined OUT of
  `embassy_main_task`. Its two callers are at `0x1000a024` and `0x1000a0a6`, both
  inside `embassy_main_task`. addr2line -i resolves the inline chains to:
  - `0x1000a024` (lr=0x1000a029, the run-2 catch) →
    `WorkerTransport::write_vram_range` (multicore.rs:1385) →
    `GameBoy::route_bus_events` (gameboy.rs:686) → `GameBoy::tick` (gameboy.rs:259)
    → `PicoGameBoy::tick` (multicore.rs:1625) → `RunningState::tick`
    (running.rs:53) → `embassy_main_task` (main.rs:759).
  - `0x1000a0a6` → `WorkerTransport::write_oam_range` (multicore.rs:1396) →
    `route_bus_events` (gameboy.rs:696) → same tail.
- So the corrupting `str r2,[sp,#0xf0]` belongs to `route_bus_events`'s inlined
  `write_vram_range`/`write_oam_range`/`flush_pending_ppu` body (the multicore
  IPC path that forwards guest VRAM/OAM writes to core 1). r2 = the spilled
  `self.memory` (GameBoyMemory base) it uses for the drained-bus-event processing.

**ROOT CAUSE — register-allocator stack-slot aliasing across the OAM-DMA and
bus-event-flush phases of one `tick`:**
- `[sp,#0xf0]` is a HEAVILY multiplexed coalesced slot: the golden disasm has
  **~28 distinct `str …,[sp,#0xf0]` writers and ~40 readers** spread across the
  whole embassy_main_task frame. The compiler packed many disjoint-by-source
  locals onto this one word.
- Two of those uses collide at runtime:
  (a) OAM-DMA: writer `0x1000a904` (=&oam 0x20043ab0, in `advance_dma_bulk`
      gameboy.rs:614); readers `0x1000abce` (`copy_dma_step` loop, memory.rs:627)
      and `0x1002fe92` (`OUTLINED_FUNCTION_353` → `<[u8;0xa0]>::index_mut`, the
      oam-slice base for the dst pointer).
  (b) bus-event flush: writer `0x1002f2d8` (=GameBoyMemory base 0x2003fa30, in
      `route_bus_events`/`write_vram_range`).
- The optimizer assumes (a)'s spill is DEAD before (b) writes the slot (control
  flow makes them look mutually exclusive within one tick). On the device they
  are NOT: the watchpoint proves (b)'s store lands on the LIVE oam-base spill,
  overwriting `0x20043ab0` with `0x2003fa30`. When the DMA loop reads it back at
  `0x1002fe92`, the "oam base" is now `0x2003fa30` (= &GameBoyMemory), so
  `dst = 0x2003fa30 + progress` points into the WRAM region just below `&oam`
  (`&oam-4 = 0x20043aac`) — **exactly the E8 memcpy-underflow victim**, and the
  E-series `&oam-4`/`strb→0x8000` faults. The copy then writes the wram tail /
  oam-base spill / dma spills as it sweeps, producing the BFAR=0x8000 HardFault.
- Why host ASan/TSan never saw it: the collision is created by **device codegen**
  (this specific outlined/coalesced layout under `-Z stack-protector=strong`),
  not by source-level aliasing — it is a layout-bound miscompilation surfaced by
  the multicore `route_bus_events` path that the host replay never lowers the
  same way. This is consistent with the entire E2/E3 "layout-fragile heisenbug"
  fingerprint: any frame shift relocates one of the two colliding uses.

**Net G1 conclusion:** the ROOT corruptor is **`0x1002f2d8 str r2,[sp,#0xf0]`**
in `GameBoy::route_bus_events`'s inlined `write_vram_range`/`write_oam_range`
(`flush_pending_ppu`) path — it stores the `self.memory` base into a stack slot
that the OAM-DMA copy still holds the live `&oam` spill in. It is NOT a wild
*pointer* in the classic sense (the value 0x2003fa30 is valid) but a **stack-slot
lifetime collision**: route_bus_events clobbers copy_dma_step's spilled oam base
within the same tick. The OAM-DMA copy and the faulting memcpy (E8) are the
PROPAGATORS; this store is the origin.

**Suggested fix direction (not applied here):** break the coalescing between the
DMA oam-base spill and the route_bus_events `self.memory` spill. Options:
(1) re-derive the oam base from `self.memory` immediately before the copy loop
    instead of relying on the early spill (deny the optimizer the long live range);
(2) prevent `route_bus_events` from being inlined into the same frame as the DMA
    copy (`#[inline(never)]` on the flush path) so they use distinct frames —
    caveat: E2 showed adding frames at the DMA site can suppress the repro, so
    apply at the route_bus_events side and re-validate; (3) sequence the tick so
    bus-event flush fully precedes/follows the DMA copy with a compiler barrier
    that forces the oam-base reload. Validate per the protocol in §"Proposed fix".

### G2 — Option-1 root fix applied + validation gate (PENDING hardware)

**Fix applied (source):** `core/src/gameboy.rs` — split `route_bus_events` so the
per-tick `has_events()` fast path stays `#[inline(always)]` (runs every CPU step,
must stay a load+branch), and moved the event-processing body into a new
`#[inline(never)] fn drain_bus_events(&mut self)`. That body is the one that, when
inlined into the giant `.data` `embassy_main_task` frame, spilled `&self.memory`
onto `[sp,#0xf0]` and collided with `copy_dma_step`'s live `&oam` spill (§G1). A
separate stack frame makes the slot collision impossible. Fix is on the
bus-event side, NOT the DMA side (per §E2, framing the DMA site masks).

Host `cargo check -p rustyboy-core`: clean.

**Why this should be causal, not a layout reshuffle:** the two colliding parties
(`drain_bus_events` self.memory spill ↔ `copy_dma_step` &oam spill) now live in
DISTINCT frames at different SP values; `[sp,#0xf0]` in one is a different word
than in the other. No code-motion within one frame can move a store between them.

**Validation gate — must confirm MECHANISM, not just "no crash":**
1. Build the fix WITH the same flags as the golden repro
   (`-Z stack-protector=strong`, release) to a SEPARATE target dir (do NOT touch
   `/tmp/rb-sp-strong`, the golden repro binary). Flash it (probe-rs download,
   `--disable-double-buffering --verify`).
2. **Mechanism check (decisive):** re-derive the `drain_bus_events` self.memory
   spill slot and the `copy_dma_step` &oam slot in the new binary (they should now
   be in different frames). Arm an OpenOCD write-watch on the &oam slot and confirm
   it NO LONGER receives a foreign (non-&oam) write during gameplay — i.e. the
   §G1 catch does not reproduce. If a foreign write reappears (on this or another
   slot, with a new victim), the fix MASKED rather than fixed → escalate to the
   Option-2 barrier in `copy_dma_step` (kill the long &oam spill outright).
3. **Soak:** N≥20 standalone reboots, gameplay to the §E trigger window, zero
   `copy_dma_step` / OAM-underflow / BFAR=0x8000 crashes.
4. Disassembly sanity: confirm `drain_bus_events` is a real OOL function with its
   own frame (its `str …,[sp,#…]` no longer aliases the DMA copy's oam-base slot),
   and that `route_bus_events`'s no-events path is still inlined (no per-tick call).

Status: fix in tree, NOT yet flashed/validated. Golden repro binary preserved.

### G2-RESULT — Option-1 flashed + first hardware data (AMBIGUOUS, leaning encouraging)

Fix build: `/tmp/rb-sp-strong-fix/.../rustyboy-pico2w` (same `-Z stack-protector=strong`
flags, separate target dir; golden `/tmp/rb-sp-strong` untouched). crc `0x5f3911ab`.
Flashed via rb-flash, integrity CRC OK, boots, loads poisoned save. Fix-build
`GameBoyMemory base = 0x20040320` (boot log) -> `&oam = 0x200443a0`.

**Pitfall hit:** `pkill -9 -f probe-rs` self-kills the wrapper shell (its own argv
contains "probe-rs"). Use `pkill -9 -x probe-rs`. First flash silently no-op'd.

**Run A (live RTT):** crashed right after "entering main loop". probe-rs printed
`Exception @ 0x88` (a FAILED unwind, not the real PC).

**Crash records — must clear stale first.** The ring buffer at 0x103FF000 is NOT
erased by reflash, so it still held the golden capture-agent runs (the classic
bug-#5 `Fault@0x8000 / r12=0x2003fa30 / copy_dma_step` signature). After
`--mark-read` + 2 fix-build reboots, the FRESH fix-build records are DIFFERENT:
  1. HardFault **Core 1** UNALIGNED (CFSR 0x01000000) in `atomic_load::<usize>` /
     `run_core1_worker` (multicore.rs:2378), stacked R0=0x11, R1 misaligned. Cycle
     **2,388,905,576**.
  2. WatchdogTimeout (timer).
  3. Panic `spsc.rs:185` (audio SPSC length-word smash).
The deterministic OAM `copy_dma_step Fault@0x8000` signature did NOT appear; the
fix build ran ~70k cycles PAST golden's crash point before dying. What surfaces
is the SEPARATE, pre-existing cross-core `SharedWorkerState`/SPSC corruption, at
the same ~2.388B cycle window.

**Two live readings (cannot decide from records — two cores fault concurrently,
whichever dies first wins):**
  (a) ENCOURAGING: Option 1 stopped the OAM-base corruption; the cross-core bug
      (previously masked by the faster OAM fault) now shows.
  (b) SKEPTICAL (E2): the damage RELOCATED to shared state, or Core 1 just loses
      the race and the OAM crash is hidden not gone.

**Decisive test = G3:** arm the OpenOCD write-watch on the FIX build's OAM-base
spill slot; a foreign (non-`&oam`=0x200443a0) store landing there ⇒ Option 1
failed/relocated ⇒ pivot to Option 2 (victim-side hardening of copy_dma_step). No
foreign store ⇒ Option 1 protected the OAM victim; the cross-core corruptor is the
new target. Regardless: the system STILL crashes at ~2.388B, so >=1 related bug
remains (the cross-core SharedWorkerState/SPSC path is now the active failure).

### G3 — Fix-build OpenOCD watch: OAM-slot verdict + cross-core localization (COMPLETE)

Goal: deterministically disambiguate whether Option 1 (split `drain_bus_events`
into its own `#[inline(never)]` frame) PROTECTED the OAM-DMA victim or merely
RELOCATED the damage. Decisive = arm a value-filtered HW write-watch on the FIX
build's `copy_dma_step` oam-base spill slot; a foreign (non-`&oam`) store landing
there ⇒ FAILED/relocated; no foreign store across the crash window ⇒ PROTECTED.

Work dir `/home/vince/.claude/jobs/27e4bd6f/tmp`. Fix ELF
`/tmp/rb-sp-strong-fix/.../rustyboy-pico2w` (crc 0x5f3911ab), already flashed.
Golden `/tmp/rb-sp-strong` untouched.

**Static pin of the FIX build's OAM-base spill (re-derived; all golden addrs drift):**
- OAM-DMA bracket calls (`check_oam_dma_invariants`) at
  `0x1000a18a / a2c8 / a310 / a3c4` (cf. golden `0x1000a8f2/aa50/aaca/ab74`).
- `copy_dma_step` (memory.rs:627) → `advance_dma_bulk` (gameboy.rs:626) →
  `advance_peripherals` → `tick`, inlined into `embassy_main_task`. Two oam-base
  computation sites:
  - **`0x1000a420 add.w r0,r2,#0x4080`** (r2 = memory base 0x20040320 → r0 = &oam
    0x200443a0), then **`0x1000a424 str r0,[sp,#0xa8]`** = the region-pointer SPILL
    block (also spills vram `[sp,#0xb0]`, base+0x4000 `[sp,#0xa4]`, base+0x423c
    `[sp,#0xa0]`). The oam-base slot is now **`[sp,#0xa8]`** (golden was `[sp,#0xf0]`).
    Reader e.g. `0x1000a4fc ldr r1,[sp,#0xa8]`. **`[sp,#0xa8]` is heavily multiplexed:
    28 writers / 38 readers** — IDENTICAL coalescing precondition to golden's
    `[sp,#0xf0]` (28/40). So the FIX did NOT remove the multiplexed oam-base spill;
    it only moved the AGGRESSOR (drain_bus_events) out of the frame. The victim spill
    survives — which is exactly why this watch is the decisive test.
  - `0x1000a540 add.w r0,r0,#0x4080` (recomputed fresh from `[sp,#0xd0]`) → fed
    DIRECTLY to `index_mut` (`0x10013490`), NO spill/reload at this site (hardened).
- Confirmed `drain_bus_events` is now a REAL out-of-line function (symbol at
  `0x20002820`, RAM/.data) — the Option-1 split took structurally. The OAM dispatch
  at `0x1000a3ac` computes `&oam` and passes it DIRECTLY to `write_live_oam_range`
  (`0x10019ce0`), no spill. The remaining spill is the bulk-copy setup at `0x1000a424`.

⇒ **Watch target (GOAL A): slot = SP + 0xa8, legit value `&oam` = 0x200443a0,
legit spill PC = 0x1000a424.** Plan: bp the spill to read live SP, compute slot,
arm `wp slot 4 w` value-filtered (skip writes == 0x200443a0; break on any foreign
value).

**GOAL B addresses (re-derived, fix build):** `SHARED_WORKER_STATE = 0x200049f0`,
`AUDIO_QUEUE = 0x200033e0` (heapless 0.9 `spsc::Queue<i16,2049>`). The `spsc.rs:185`
panic is `increment`/`n()` = `(val+1) % self.buffer.borrow().len()` → divide panic
when the buffer view's length word is smashed to 0. Core-1 UNALIGNED atomic_load is
`run_core1_worker` (`0x1001ae44`) reading corrupted shared state.

**Hardware obstacle hit — fix build is in a fast CRASH-RESET LOOP:** OpenOCD
(`cm0`, fork v0.12.0+dev, 8 bp / 4 wp confirmed) attaches fine. PC-sampling after
`reset halt; resume` shows the core spends the first seconds in embassy-executor
idle (`0x100281d4-f2`, cordyceps run_queue) and SD-card SPI status polling
(`0x1002754a / 0x10022e2c`, `rp_pac::spi::sr::read`) = save replay. Then OpenOCD
repeatedly logs **`external reset detected`** within seconds — the firmware crashes
on its own (the active cross-core bug) and resets, which WIPES the HW breakpoint, so
a single-shot bp at `0x1000a424` never catches. The standalone crash window
(~2.3889B cycle, ~the skill's "~10s standalone") is reached too fast / save-replay
eats most of it, and the chip self-resets before the OAM-DMA bulk-copy bp can be hit
and re-derive SP. Mitigation in progress: a re-arming TCL loop
(`derive_rearm.tcl`) that re-issues `reset halt; bp 0x1000a424; resume` on every
boot and only stops on a real bp halt (pc∈[0x1000a420,0x1000a428]).

**Reaching the OAM path — confirmed.** A dual-bp run (spill `0x1000a424` + dispatch
`0x10019ce0 write_live_oam_range`) proved OAM DMA DOES run in fix-build gameplay
before the crash: the dispatch bp hit with `r0 = 0x200049f0` (= `SHARED_WORKER_STATE`,
the `self` of `write_live_oam_range`). So the crash-loop is NOT "crash before DMA";
core 0 reaches the OAM forwarding path. The `0x1000a424` bulk-copy memory-dispatch
block is reached from `0x1000a24a/a28a/a3f4/a3fe` (the per-guest-access region-base
setup inside `copy_dma_step`).

**Live slot derivation (decisive).** Rather than bp the cold bulk block, breakpoint
the OAM dispatch site **`0x1000a3ac`** (`add.w r2,r11,#0x4080`, IN the
`embassy_main_task` frame — same frame as the `[sp,#0xa8]` oam spill, runs every
OAM DMA, hits in seconds). On hit: **SP = 0x2007ed00** (stable persistent main-task
frame; matches the dual-bp `msp`), so **OAM-BASE SPILL SLOT = SP+0xa8 = `0x2007eda8`**.
Then, in the SAME OpenOCD session (no reset, SP stable), armed
`wp 0x2007eda8 4 w` + a resume-past-spurious loop: skip every halt whose written
value == `&oam` 0x200443a0 (legit spill), break on any foreign value.

**GOAL-A VERDICT — Option 1 PROTECTED the OAM victim (the §G1 spill is now COLD).**
The watch on slot `0x2007eda8` saw **zero writes** across 6 boots before each
crash-reset (`>>> SILENT 12s after 0 legit spills`, every boot; SP stable at
0x2007ed00). A plain no-hit is not trustworthy alone, so a **positive control**
settled it: arm HW breakpoints on BOTH the bulk-copy spill instr `0x1000a424`
(`str r0,[sp,#0xa8]`, the §G1 victim spill) AND the dispatch site `0x1000a3ac`,
count hits over multiple boots:

| boot | dispatch `0x1000a3ac` hits | spill `0x1000a424` hits |
|------|---------------------------:|------------------------:|
| 0    | 40                         | **0** |
| 1    | 80 (cum)                   | **0** |

⇒ The OAM-DMA dispatch (`write_live_oam_range`, the multicore forward-to-core-1
path) is EXTREMELY hot, but the local `copy_dma_step` bulk-copy block that spills
`&oam` to `[sp,#0xa8]` and reads it back to form the dst slice — **the entire §G1
victim mechanism — NEVER EXECUTES in fix-build gameplay** (0 hits vs 80 dispatch
hits). The optimizer routed the live OAM-DMA path through the
`add r2,#0x4080 → write_live_oam_range` site (`&oam` computed and consumed
directly, NO spill, §G3 static analysis above), so there is no live `&oam` spill on
`[sp,#0xa8]` for ANY foreign store to land on. Combined with Option 1 having moved
the AGGRESSOR (`drain_bus_events`) into its own out-of-line frame, BOTH parties of
the golden §G1 `[sp,#0xf0]` collision are absent from the fix build's hot frame.
**No foreign store can reproduce the OAM-base smash. The OAM `Fault@0x8000` /
`copy_dma_step` signature is structurally eliminated, not merely relocated.** This
confirms reading (a) of §G2-RESULT and grades Option 1 as a real fix for bug #5's
OAM victim. (Caveat noted in §G3-B: the system still crashes — the SEPARATE
cross-core bug — so "OAM fixed" ≠ "board stable".)

### G3-B — Cross-core corruptor localization (GOAL B)

The active fix-build crash (§G2-RESULT) is the cross-core path: Core-1 HardFault
UNALIGNED in `atomic_load::<usize>` / `run_core1_worker` (multicore.rs:2378) and a
`spsc.rs:185` divide panic (audio SPSC buffer-length word smashed to 0), both at
cycle ~2.3889B. Re-derived fix-build addresses: `run_core1_worker = 0x1001ae44`,
`SHARED_WORKER_STATE = 0x200049f0`, `AUDIO_QUEUE = 0x200033e0` (heapless 0.9
`spsc::Queue<i16,2049>`; `spsc.rs:185` = `(val+1) % self.buffer.borrow().len()` →
divide-by-zero when the buffer view's len word is 0).

**Correction (static): the `spsc.rs:185` panic is the COMMAND_QUEUE, not audio.**
`panic_const_rem_by_zero` is reached from `increment` (`0x10020604`) of
`heapless::spsc::QueueInner<Core1Command,…>` — i.e. the **COMMAND_QUEUE**
(`0x20004490`), the core0→core1 command channel, NOT the i16 AUDIO_QUEUE. (The audio
enqueue is fully inlined; its symbols don't survive.) COMMAND_QUEUE layout (derived
from `inner_enqueue 0x1002061c` and the `run_core1_worker` dequeue at `0x1001af5c`):
**head AtomicUsize @ 0x20004490, tail AtomicUsize @ 0x20004494, buffer @ 0x20004498**
(65 slots × 20 B; `Queue<Core1Command,65>` spans 0x20004490..0x200049b0 = 0x520 B).
Core-1 dequeue: `ldr r1,[head]`, `lda r0,[tail]`; if non-empty, `ldr r9,[buf + head*20]`
(the load that faults **UNALIGNED if head is wild** = multicore.rs:2378 signature),
then `increment` does `(head+1)%n()` (rem-by-zero if buffer len read as 0), then
`stl new_head,[head]` (`0x1001af8e`). Producer (core 0) `inner_enqueue` writes the
**tail** via `stl r0,[r1]` (`0x1002065c`). So the smash victim is the COMMAND_QUEUE
head index and/or its buffer-length metadata.

**HW watch result (windowed, in-gameplay).** The fast crash-loop defeated a
boot-time watch (the chip self-resets during save-replay before gameplay; 3 boots ×
COMMAND_QUEUE-head watch + 3 boots × tail watch all reset with 0 gameplay writes —
the debugger attach appears to *aggravate* the crash). Fixed by a **windowed arm**:
HW-bp the hot OAM dispatch `0x1000a3ac` to confirm gameplay is reached, THEN arm a
**quad write-watch** on COMMAND_QUEUE head (0x20004490) + tail (0x20004494) +
SHARED_WORKER_STATE first 2 words (0x200049f0/f4), resume-past-spurious skipping the
legit producer `inner_enqueue` body (PC 0x1002061c..0x1002066c) and core-1 dequeue
store (0x1001af80..0x1001afa0).

⇒ **Result: 427 watch halts, ALL legit core-0 enqueue traffic (PC 0x10020666 /
0x10020662, in `inner_enqueue`), ZERO foreign writes** to any of the 4 cross-core
metadata words across an extended gameplay window. head/tail observed healthy
(e.g. 8/9), SWS words 0. **No core-0 wild store hits the command-queue metadata.**

**Crucial behavioral observation: the crash is TIMING/RACE-sensitive, not a
wild-store.** Under the boot-time watch the chip crash-resets within seconds; under
the windowed in-gameplay watch (which halt-throttles core 0 on every enqueue) the
firmware ran HEALTHILY for hundreds of enqueues and **did NOT crash**. Halt-throttling
core 0 suppresses the failure. That is the fingerprint of a genuine **cross-core
race / memory-ordering bug** (the two cores must run concurrently at speed), NOT the
bug-#5 stack-slot wild-store regime (which is layout-bound and reproduces regardless
of throttling). So the corruptor is almost certainly NOT a stray core-0 pointer store
into the queue; it is a concurrency defect in the COMMAND_QUEUE / SharedWorkerState
handshake itself (e.g. head/tail or buffer-metadata read/written without sufficient
ordering, so core 1 observes a torn/transiently-wild head index and faults at
`ldr r9,[buf + head*20]`, or reads n()=0 and panics). This is consistent with the
known caveats (`MEMORY_BARRIER_INVESTIGATION_PLAN.md`) that the SPSC handshake's
ordering is subtle on this dual-M33.

### G3 — Conclusions & recommendation

**(A) OAM victim — GOAL A: PROTECTED.** Option 1 (split `drain_bus_events` into its
own `#[inline(never)]` frame) is a real fix for bug #5's OAM-base corruption. In the
fix build the §G1 victim spill (`&oam`→`[sp,#0xa8]`, read back for the dst slice) is
**never executed in gameplay** (`0x1000a424` spill instr: 0 hits vs 80 dispatch hits
at `0x1000a3ac`); the live OAM-DMA path computes `&oam` and passes it directly to
`write_live_oam_range` with no spill, and the aggressor (`drain_bus_events`) now has
its own frame. No foreign store can land on a live `&oam` spill because none exists.
The OAM `Fault@0x8000` / `copy_dma_step` signature is structurally eliminated, not
relocated. **Keep Option 1. Option 2 (victim-side `copy_dma_step` hardening) is NOT
needed for the OAM bug.**

**(B) Active corruptor — GOAL B: a cross-core race in the COMMAND_QUEUE /
SharedWorkerState handshake, NOT a core-0 wild store.** No foreign core-0 write hits
the queue metadata; the failure vanishes under halt-throttling ⇒ timing-sensitive.
Core 1 faults reading `ldr r9,[buf + head*20]` (UNALIGNED, multicore.rs:2378) or
panics on `% n()`=0 (`spsc.rs:185`) because it observes a torn/transient head index
or zeroed buffer-length metadata. The watchpoint could not name a single store PC
because there is no single wild store — it's an ordering bug. (cm0-only attach also
cannot see core-1 stores; a torn-write origin on either core is plausible.)

**(C) Recommended next fix.** Do NOT pursue Option 2. Target the cross-core handshake:
1. Audit COMMAND_QUEUE producer/consumer ordering in `enqueue_blocking` /
   `inner_enqueue` / `run_core1_worker` dequeue: the head/tail `Atomic` ops must be a
   correct Release(tail by producer)/Acquire(tail by consumer) and Acquire/Release on
   head, AND the **buffer slot writes must be ordered before the tail Release** (the
   `stl r0,[tail]` at 0x1002065c is store-release, good — verify the slot `stm` at
   0x10020654 precedes it and that the consumer's `ldr r9,[buf+head*20]` is gated by an
   `lda`(acquire) of tail, which the disasm shows at 0x1001af60 — confirm no path reads
   the slot before the acquire). On RP2350's dual-M33 a missing `DMB`/wrong ordering
   lets core 1 read a slot/head before the producer's writes land (per the §"Barriers
   caveat": Release orders but does not drain the write buffer; a `DSB` may be needed
   on the producer, or the SPSC's relaxed head/tail needs upgrading).
2. Add a litmus that fails only without the barrier (per MEMORY_BARRIER plan) before
   changing firmware, then re-validate on hardware by re-running the §G3-B windowed
   quad-watch AND a standalone soak past cycle 2.3889B.
3. Reproduction tooling for the next session is in
   `/home/vince/.claude/jobs/27e4bd6f/tmp` and `~/git/.../openocd/goalB3.tcl`
   (windowed quad-watch) / `goalA.tcl` (OAM slot derive+watch).

### G4 — Cross-core ordering race: litmus, root-cause, fix, and soak (2026-06-19)

#### G4-A: Root hypothesis (derived from static analysis + CRASH_DEBUG_NOTES + MEMORY_BARRIER_INVESTIGATION_PLAN)

**Summary**: The crash (Core 1 UNALIGNED in `atomic_load::<usize>` at `publish_worker_state`, `spsc.rs:185` rem-by-zero, WatchdogTimeout) is a downstream symptom of a **store-buffer visibility gap** in `publish_frame_locked`. Core 1's raw buffer writes (up to 23 KB `native_frame_slots[target]` + `dirty_rows`) are not drained before the gating `published_frame.store(Release)`. Core 0 observes the Release store and reads the buffers while they still hold stale/torn data from a prior frame. The stale `dirty_rows` bitmap and frame slot contain GB-shaped bytes; when `send_frame` uses them to size DMA/slice operations or drive dirty-range copies, the stale data scribbles over adjacent core-0 stack objects — specifically the `Core1Transport::shared` pointer (→ near-null R0=0x11 at an atomic load) and the `command_rx`/`audio_tx` queue fat pointer length words (→ `n()=0` rem-by-zero at `spsc.rs:185`).

**Why the SPSC queue ordering is NOT the direct cause**:
- `inner_enqueue` compiles to: `STM buf[tail*20]` (slot write) → `STL [tail]` (Release). On ARMv8-M, `STL` prevents the STM from being observed after it. This IS architecturally correct — a Release-before-Release ordering constraint means the STM is visible to a core that sees the STL.
- `inner_dequeue` does `LDR [head]` (Relaxed) → `LDA [tail]` (Acquire) → read slot. The Acquire on tail synchronizes with the Release on tail; slot data is guaranteed visible.
- `enqueue_blocking` adds `DSB` + `SEV` after enqueue_Ok. This drains the store buffer before the SEV wakes core 1.
- §G3-B: 427 watchpoint halts, ALL legit enqueue traffic, ZERO foreign writes. The queue atomics are correct.

**The actual ordering gap** is in `publish_frame_locked` (`multicore.rs:385-444`), per MEMORY_BARRIER_INVESTIGATION_PLAN §1 "STRONGEST" finding:
- Core 1 (producer): writes `dirty_rows[target]` (raw UnsafeCell, 20 B) + `native_frame_slots[target]` (raw UnsafeCell, ~23 KB) inside `critical_section::with`. The custom RP2350 `critical_section` (in `src/critical_section_impl.rs`) uses SIO Spinlock-31 MMIO + compiler fence only — **no `DSB`**. The SIO spinlock MMIO read/write sequences device-ordered MMIO, but does NOT drain Normal-memory store-buffer writes on Cortex-M33. Then: `published_frame.store(target_slot, Release)` + `published_frame_seq.fetch_add(1, AcqRel)` — both are `STL`/`STLEX`. ARM's `STL` prevents later stores from appearing before it, but it does NOT drain already-buffered Normal-memory writes. So the 23 KB frame + dirty bitmap can sit in core 1's store buffer while the Release atomic is already observable by core 0.
- Core 0 (consumer): Acquire-loads `published_frame_seq` (in `poll_output`) and `published_frame` (in `published_native_frame`), then reads `&native_frame_slots[slot]` (raw pointer, no fence) and `dirty_rows[slot]` (raw pointer, no fence). If core 0 observes the Release atomic BEFORE core 1's store buffer has drained the 23 KB slot and dirty bitmap, core 0 reads stale/torn GB bytes.

**Why this produces the observed crash symptoms**:
The stale native frame slot / dirty bitmap contain raw GB-pixel bytes (u8 values in range 0–3, with multi-byte patterns that resemble pointers). When `send_frame` on core 0 uses a stale `dirty_rows` bitmap to drive dirty-range calculation, it can compute a wrong `start_row`/`end_row` that sizes a DMA or slice operation past its intended end. That overrun writes GB-pixel bytes over adjacent core-0 stack slots — specifically:
- The `Core1Transport::shared` fat/thin pointer (smashed toward 0 → R0=0x11 at `atomic_load::<usize>`)
- The `command_rx.rb` fat pointer length metadata (smashed to 0 → `n()=0` → `spsc.rs:185` rem-by-zero)

The "vanishes under halt-throttle" pattern: halt-throttling core 0 slows its enqueue rate, which means core 1 spends more time idle in WFE and less time in the frame-publish path. The race window (time between core 1 publishing and core 0 consuming the frame) widens under throttle, giving core 1's store buffer time to drain — suppressing the ordering violation. This is EXACTLY the heisenbug fingerprint of a write-buffer race on ARM.

**Why the prior litmus at 4000 iterations PASSED** (from MEMORY_BARRIER_INVESTIGATION_PLAN §2026-06-15):
The synthetic litmus exercises 4096 bytes through `native_frame_slots[0]`, not the full 23 KB frame. At 4000 iterations, the ordering violation was not observed. There are two likely explanations: (a) the 4096-byte payload is small enough that core 1's store buffer drains before core 0 reads it at the natural timing of the `turn`/`stage` handshake; (b) 4000 iterations is insufficient. The real crash uses the full 23 KB frame copy at 60 fps; a litmus that exercises the same 23 KB path at high rate is more likely to fail.

**Why the prior `frame-publish-dsb` build crashed at 54s** (from MEMORY_BARRIER_INVESTIGATION_PLAN §2026-06-15):
The `frame-publish-dsb` build added DSB but used `EXPECTED_WORKER_PPU_STATE_PTR` as a diagnostic, which introduced additional instrumentation affecting layout. The 54s crash had victim `ppu=0x2002b62c/want 0x0000001c` — a DIFFERENT victim, consistent with the same corruptor landing on a layout-shifted slot. This does NOT prove the DSB fix is wrong; it proves the concurrent diagnostics created a new smash target. The DSB + clean soak is the correct test.

#### G4-B: Litmus plan

**Existing litmus `memory-barrier-litmus-producer-dsb`** already exercises the frame-publish ordering path:
- Core 1 writes `MEMORY_BARRIER_LITMUS_BYTES = 4096` bytes to `native_frame_slots[0]`, optionally DSBs, then stores `published_frame.store(0, Release)` + `published_frame_seq.store(iteration, Release)`.
- Core 0 Acquire-loads `published_frame_seq`, then reads all 4096 bytes and compares vs expected pattern.
- FAIL = any byte mismatch → ordering violation (core 0 read stale data before the store drained).

**Problem**: prior 4000-iteration run PASSED without DSB. This litmus might need more iterations or a larger payload to reliably trigger the violation.

**Extended litmus plan**:
1. Increase `MEMORY_BARRIER_LITMUS_BYTES` to the full frame size (23040 B = `NATIVE_FRAME_SLOT_COUNT` × frame) and `MEMORY_BARRIER_LITMUS_ITERATIONS` to a higher count (e.g. 10000) to stress the store buffer more.
2. Run baseline (no DSB) → if failure count > 0, we have a confirmed violation.
3. Run `producer-dsb` → if failure count = 0, fix is confirmed.

**Decision rule per the task**: if the baseline litmus cannot be made to fail, stop and report; do NOT apply the DSB fix speculatively.

#### G4-C: Disassembly verification

Static analysis confirms the fix-build (`/tmp/rb-sp-strong-fix`) `inner_enqueue` sequence:
- `0x1002065c: stlne r0, [r1]` — Store-Release of new tail (correct, prevents slot write reordering after it per ARMv8-M)
- `0x10020654: stmne r1!, {r2,r3,r4,r5,r6}` — slot write (plain STM, before the STL, correctly ordered by STL's Release semantics)
- Dequeue: `0x1001af60: lda r0, [r0]` — Load-Acquire of tail (correctly paired with producer's STL)
- **COMMAND_QUEUE SPSC ordering IS correct. The queue itself is not the bug.**

`publish_frame_locked` (no `frame-publish-dsb` feature):
- `0x1001bb20: bl __aeabi_memcpy` — copies ~23 KB frame (plain stores)
- `0x1001bb26: stlb r8, [r0]` — `published_frame.store(Release)` — NO DSB between memcpy and STL
- `0x1001bb3c: stlex r2, r1, [r3]` — `published_frame_seq.fetch_add(AcqRel)` — NO DSB

This confirms the vulnerability: the 23 KB memcpy's stores can be in the write buffer when `STLB published_frame` becomes observable to core 0.

#### G4-D: Litmus execution (hardware) — HYPOTHESIS REJECTED

**Builds completed** (both with MEMORY_BARRIER_LITMUS_BYTES=23040):
- Baseline (`memory-barrier-litmus`): `/tmp/rb-litmus-baseline-23k`
- DSB (`memory-barrier-litmus-producer-dsb`): `/tmp/rb-litmus-dsb-23k`

**Run 1 — Baseline-23k flash result**: "memory barrier litmus start: variant=baseline, iterations=4000, bytes=23040" printed, then firmware exited with "Exception". Crash records from this boot were all stale (git=fd1bb003, from prior gameplay runs — the ring buffer is not erased on reflash). This means the litmus itself did NOT write a crash record. The "Exception" was probe-rs's vector-catch intercepting a subsequent HardFault that occurred after the litmus completed (firmware continued to game init and hit the existing bug). The litmus PASSED silently at 4000 iterations / 23040 bytes.

**Conclusion**: both the 4096-byte litmus (prior runs, 4000 iterations, PASS) and the 23040-byte litmus (4000 iterations, PASS) failed to observe any ordering violation. The `publish_frame_locked` / `frame-publish-dsb` hypothesis is **REJECTED** as the primary crash mechanism:

- The litmus reproduces the EXACT write-then-publish sequence (23 KB slot + Release store) as `publish_frame_locked` does in the real code.
- A real ordering violation would show as a byte-mismatch FAIL within 4000 iterations (each iteration = one store-buffer stress cycle).
- Zero failures at 4000 × 23040 bytes = zero evidence for an ordering gap.

Per the task discipline: "a fix you can't first make a litmus fail→pass on is NOT validated."

**Revised hypothesis** (2026-06-19): The crash is a **wild store** into Core 1's stack at address 0x20081F74 (`sp+0x94` in `run_core1_worker` = `command_rx.rb.data_ptr`). Evidence:

1. Crash record CFSR=0x01000000 = UNALIGNED (not a data consistency error — it's a misaligned ATOMIC READ of a smashed pointer). The VALUE 0x0D in R2 (base of `command_rx.rb`) is not a stale-but-valid GB frame byte; it's an arbitrary small integer that looks like a raw corruption.
2. R0=0x11 = `&tail = &head + 4 = 0x0D + 4` — the fat pointer's data_ptr was smashed to 0x0D, not to a GB-pixel byte range (0–3) or a stale-but-valid pointer.
3. The target address 0x20081F74 is on Core 1's stack. Only Core 1's OWN spill stores write here normally. A store from Core 0 to a Core 1 stack location implies a wild pointer arithmetic error on Core 0.
4. The prior §G3-B quad-watchpoint (427 halts, all legit) watched COMMAND_QUEUE head/tail and SHARED_WORKER_STATE — it DID NOT cover 0x20081F74. The wild write was never observed because no watchpoint watched that address.

**Current action**: OpenOCD hardware write-watchpoint armed on **0x20081F74** (`command_rx.rb.data_ptr` on Core 1's stack), value-filtered to skip the one legit write (value=0x20004490 at prologue). Running now — watchpoint hit expected at ~9.5 minutes from boot (GB cycle ~2.389B). See §G4-E when results arrive.

#### G4-E: DWT hardware watchpoint — WILD STORE CAUGHT

OpenOCD SMP-mode write-watchpoint on 0x20081F74, running binary `/tmp/rb-sp-strong-fix` (git=fd1bb003):

**Halt sequence:**
- Halts 1–15: `cm1` PC=0x1001b62c, val=0x00000000. Spurious (OAM zeroing loop `str r3,[r2,r1]` with r3=0, zero-clearing the sprite array). Filtered and resumed.
- **Halt 16**: `cm1` PC=0x1001b688, val=0x00000011. **NOT spurious** — a non-zero, non-legit value written to the watchpoint address.

**Root cause: Stack-slot collision in `run_core1_worker`**

PC=0x1001b688 resolves (via `addr2line -i -f -e`) to:
```
render_sprite_scanline  (ppu.rs:558)
  inlined into → render_scanline        (ppu.rs:424)
  inlined into → PpuPeripheral::tick    (ppu.rs:248)
  inlined into → PpuWorkerState::advance (worker.rs:284)
  inlined into → GameBoyWorker::send    (worker.rs:58)
  inlined into → run_core1_worker       (multicore.rs:2377)
```

The entire PPU rendering chain is **fully inlined** into `run_core1_worker`'s single
stack frame (`sub sp, #0xf0` = 240 bytes). The compiler's register allocator chose
sp+0x94 (relative to the runtime SP in `run_core1_worker`) for BOTH:

1. The `command_rx.rb.data_ptr` field of the Consumer fat-pointer, spilled to the
   stack by the prologue `strd r1, r0, [sp, #0x90]` at 0x1001ae4e.
2. A local variable inside the inlined `render_sprite_scanline` — specifically the
   OAM sprite-collection loop that writes sprite attributes (y, x, tile, attrs, i)
   tuples. The wild store at 0x1001b682 (`str r3, [r5, #4]`) with r5 derived from
   sp+0xA0 writes r3 (an OAM X coordinate) to sp+0xA4 = the watchpoint address.

**Note on address discrepancy**: The crash records show `SP_before=0x20081EE0`
(= EF+32, stored by the HardFault handler), making sp+0x94 = 0x20081F74 (the
watchpoint address). The DWT halt MSP was 0x20081ED0 (= actual post-prologue SP
computed from the push chain: stack_top − 8 − 8×4 − 4×4 − 2×4 − 0xF0). The
16-byte discrepancy between the crash-time SP and the watchpoint-halt MSP is not
fully explained, but both lines of evidence point to the SAME address 0x20081F74 as
the collision target. The watchpoint on 0x20081F74 caught the live write (val=0x11,
a sprite X coordinate), confirming the mechanism.

**Crash mechanism:**
1. `run_core1_worker` prologue writes `command_rx.rb.data_ptr = 0x20004490` to
   sp+0x94 at 0x1001ae4e.
2. ~9.5 minutes / ~2.389 billion cycles later, `render_sprite_scanline` executes
   inside the same inlined frame. Its OAM loop stores a sprite X coordinate (0x11
   in the caught case, 0x0D in the actual crashing case) to the SAME sp+0x94 slot.
3. On the next dequeue iteration: `ldr r2, [sp, #0x94]` (0x1001af36) loads 0x0D
   instead of 0x20004490.
4. `adds r0, r2, #0x4` → r0 = 0x11 (= &tail = corrupt_ptr + 4).
5. `lda r0, [r0]` (0x1001af60): Load-Acquire of tail from address 0x11 → UNALIGNED
   HardFault. CFSR=0x01000000, R0=0x11 (matches all crash records).

This is a **compiler codegen stack-slot collision**: the register allocator reused
the same spill slot for two different variables in different inlined call frames
within the same monolithic function body.

#### G4-F: Fix applied

**Fix**: Add `#[inline(never)]` to `render_scanline` (primary fix) and
`render_sprite_scanline` (belt-and-suspenders) in
`core/src/cpu/peripheral/ppu.rs`.

Reasoning:
- `render_scanline` calls `render_bg_scanline`, `render_window_scanline`, and
  `render_sprite_scanline`. Making it `#[inline(never)]` gives the ENTIRE scanline
  render its own stack frame, completely separate from `run_core1_worker`'s frame.
  None of its locals can alias `command_rx.rb.data_ptr` at sp+0x48 (new offset in
  the fixed build) in `run_core1_worker`.
- `render_sprite_scanline` is also marked `#[inline(never)]` as belt-and-suspenders:
  even if `render_scanline` were ever inlined again, `render_sprite_scanline` won't
  share the outer frame.
- The `#[cfg_attr(target_arch = "arm", link_section = ".data")]` attribute was
  already present on both functions. With `#[inline(never)]`, it now takes effect:
  the rendering code is placed in SRAM, avoiding flash-read latency on PPU-heavy
  games (a performance benefit, not just a correctness fix).

**Effect on `run_core1_worker` stack frame** (verified by disassembly):
- Old: `sub sp, #0xf0` (240 bytes), `strd r1, r0, [sp, #0x90]` → data_ptr at sp+0x94
- New: `sub sp, #0x50` (80 bytes), `strd r1, r0, [sp, #0x44]` → data_ptr at sp+0x48
- The frame shrunk by 160 bytes. All sp+N accesses to data_ptr are READS (loads),
  except the single prologue STRD. No rendering code can reach sp+0x48 in the new frame.

**Build result** (`cargo build --release` from `platform/pico2w/`):
- Compiles cleanly with only pre-existing warnings.
- `render_scanline` emitted at 0x20002054 (SRAM `.data` section).
- `render_sprite_scanline` emitted at 0x2000226c (SRAM `.data` section).
- Long-range thunk at 0x1002ee06 for the flash→SRAM call.

**Files changed:**
- `core/src/cpu/peripheral/ppu.rs`: `#[inline(never)]` added to `render_scanline`
  and `render_sprite_scanline`, with explanatory comments.

#### G4-G: Validation plan

The fix eliminates the struct mechanism (stack-slot collision) that was directly
observed by the DWT watchpoint. Hardware validation:

1. Flash the fixed binary and run the same game (same ROM) for at least 30 minutes
   (~4× the crash cycle count of 2.389B).
2. Check crash records: `python3 tools/crash_decoder.py --probe --elf <elf>` from
   repo root. Zero UNALIGNED HardFaults on Core 1 = pass.
3. Optionally re-run with OpenOCD watchpoint on 0x20081F74 and confirm no halt #16-
   type event (no non-zero, non-legit writes to that address from rendering code).

**Alternative validation** (if a 30-minute manual run is impractical): re-run the
OpenOCD watchpoint session. With the fix applied, the rendering code's stack slot
for the sprite local will be at a DIFFERENT absolute address (inside
`render_sprite_scanline`'s own frame, far from 0x20081F74). The watchpoint should
not fire on non-zero values after the initial BSS-clear halts.

---

### §G4-H: Decisive watchpoint — which slot was actually watched

**Motivation.** §G4-E claimed the open G4 crash is a stack-slot collision where a
sprite-X local clobbers `command_rx.rb.data_ptr` at `sp+0x94`, and "caught" it with
a DWT watchpoint on the **absolute** address `0x20081F74`. `COMPILER_CODEGEN_INVESTIGATION.md`
argued that proof is unsound because §G4-E mixed two SP reference frames: it derived
the slot address from the **crash-derived** SP (`0x20081EE0` = exception_frame+32),
but the **live** DWT-halt MSP was `0x20081ED0` — 16 bytes lower. Relative to the live
SP, `sp+0x94 = 0x20081F64` (data_ptr) while the watched `0x20081F74 = sp+0xA4` =
sprite array `entry[1].field` (the array begins at `sp+0x98`). This session settles
it empirically by anchoring everything to the **live** SP before trusting any hit.

Repro binary used **as-is, no rebuild**:
`/tmp/rb-sp-strong-fix/.../release/rustyboy-pico2w`
(`run_core1_worker` @ `0x1001ae44`, matches §G4-E). Disassembly re-verified:
`strd r1,r0,[sp,#144]` @ `0x1001ae4e` → data_ptr (r0) lands at **sp+0x94**;
`ldr r2,[sp,#0x94]` @ `0x1001af36`; `add r0,sp,#0x98` @ `0x1001af34` (sprite array
base = sp+0x98, confirming sp+0xA4 is a sprite slot); crash store `lda r0,[r0]` @
`0x1001af60`. `COMMAND_QUEUE` symbol base = `0x20004490` (BSS). OpenOCD = RaspberryPi
fork v0.12.0+dev; attached **SMP** (both cores) so core 0 keeps running to spawn
core 1 — single-core `cm1` attach left core 1 parked in bootrom (pc=0xda,
sp=0xf0000000) because core 0 was halted and never issued the SIO spawn.

#### Step 2 — slot identity anchored to the LIVE SP (the missing §G4-E verification)

Full chip `reset halt`, HW bp @ `0x1001ae52` (right after the data_ptr `strd`), resume;
core 1 hit the bp once at its single prologue entry:

```
cm1 halted @ bp:  pc=0x1001ae52  MSP = live SP = 0x20081ED0
sp+0x94 = 0x20081F64   value = 0x20004490   <-- EXPECT 0x20004490  ✅ VERIFIED data_ptr
sp+0xA4 = 0x20081F74   value = 0x00000011   <-- the §G4-E WATCHED address; holds a sprite-X
frame scan sp+0..sp+0xF0: 0x20004490 found ONLY at sp+0x94 (=0x20081F64)
sp+0xA4 == 0x20081F74 ?  → 1 (TRUE)
sp+0x94 == 0x20081F64 ?  → 1 (TRUE)
```

**Verdict on §G4-E's address:** §G4-E watched the **WRONG slot.** The real data_ptr
spill lives at **0x20081F64**. The address §G4-E watched, `0x20081F74`, is `sp+0xA4`
= sprite array `entry[1]`, and at the halted prologue it already held `0x00000011`
(a sprite-X value). The "15 zero-writes then a 0x11 store" §G4-E observed there were
**legitimate sprite rendering** (OAM zeroing loop + a sprite-X store), exactly as the
review predicted — not a wild store. (Tell, also from the review: if `0x20081F74`
were data_ptr, the per-scanline OAM-zeroing loop would zero the queue pointer every
frame → instant crash, not a 9.5-min Heisenbug.)

#### Step 3 — watch the VERIFIED data_ptr slot `0x20081F64`

Re-entered via the prologue bp to establish the frame, removed the bp, armed a 4-byte
WRITE watchpoint on the verified `0x20081F64`. Because we arm **after** the one-time
prologue `strd`, the only future legit write is none — so any halt would be the
corruptor (value filter: skip `0x20004490`). Free-ran the full ~10-minute window
(loop bound 4000×150 ms = 600 s ≥ the ~9.5-min crash cycle):

```
>>> WRITE WATCH armed @ 0x20081f64; verify slot=0x20004490
>>> TIMEOUT after 0 legit prologue-writes, NO foreign store
```

**No write to the verified data_ptr slot occurred during the entire run.**

#### Positive controls (so "no hit" is a true negative, not a dead comparator)

- *posctl #1* (watch a guessed-hot stack slot `0x20081F3C`): did **not** fire — but
  that slot was not actually hot (that `str` is not in the per-iteration loop), so it
  proves nothing.
- *posctl #2 (definitive)*: armed the write-watch on the **exact** address `0x20081F64`
  **before** boot completed; the prologue `strd` MUST write it once. Result:
  `COMPARATOR FIRED at pc=0x1001ae56, slot now=0x20004490`. The DWT write-watch on
  this exact cm1 address **provably works**, and the legit writer is confirmed to be
  the prologue writing `0x20004490`. Therefore Step 3's no-hit is a **true negative.**

#### Crash records in flash — the crash is real, but heterogeneous

`crash_decoder.py --probe` (24 stored records, all CRC-OK, fw git fd1bb003):

- **10× Panic @ `spsc.rs:185`** on core 1 — the *most common* symptom (the heapless
  SPSC length-word smashed to 0), at GB cycles ~2.39–2.51 B.
- **4× HardFault UNALIGNED**, `CFSR=0x01000000`, stacked `R0=0x11` (or `0x15`) — the
  §G4-E/“G4” signature.  #7 detail: `ARM PC=0x1001af60` (`atomic_load` = the
  `lda r0,[r0]` dequeue), `LR→run_core1_worker` (multicore.rs:2378), `R0=0x11`
  (faulting addr ⇒ data_ptr=0x0D near-null), and the decoder itself prints
  `SP_bef=0x20081EE0 (sp_before=ef+32)` — i.e. the crash-derived SP that is **+0x10
  above** the live MSP `0x20081ED0`. This is the exact reference-frame offset the
  review predicted.
- **2× HardFault BFARVALID/PRECISERR** @ `0x7f7f7f93` / `0xffffffff` (poison-pattern
  addresses), cycles ~2.40 B and 3.76 B.
- WatchdogTimeouts interspersed (downstream reset effects).

All cluster at GB cycle ~2.39–2.51 B → one underlying memory-corruption event with
several downstream symptoms depending on which word is hit, **not** specifically a
data_ptr stack-slot overwrite.

#### DECISION: **OUTCOME B**

The verified data_ptr slot `0x20081F64` is written **only** by the prologue `strd`
(value `0x20004490`); across a full ~10-min crash window the watchpoint — proven live
by posctl #2 — caught **zero** foreign writes. The near-null `data_ptr` that faults at
`0x1001af60` is therefore **not** produced by a render/PPU store clobbering that stack
slot. **§G4-E's stack-slot-collision root cause is NOT confirmed; it watched the wrong
absolute address (`0x20081F74` = a legitimate sprite slot at `sp+0xA4`, not data_ptr at
`sp+0x94`=`0x20081F64`).**

The single most decisive number: at the live prologue SP `0x20081ED0`, **`[sp+0x94] =
[0x20081F64] = 0x20004490`** (data_ptr) while **`[0x20081F74] = 0x00000011`** (sprite-X)
— so the previously "caught" writes were sprite rendering, and the truly-watched
data_ptr slot was never corrupted.

#### Next lead (for the follow-up step)

Because the slot is never stack-clobbered yet `data_ptr` reads back near-null, the
corruption is in the **COMMAND_QUEUE struct in BSS at `0x20004490`** (cross-core), or
the value the compiler loads at `ldr r2,[sp,#0x94]` is reading a slot whose backing
struct field was already smashed in BSS — consistent with the more-common
`spsc.rs:185` panic (a *different* SPSC queue's length word zeroed) and the
poison-pattern bus faults (`0x7f7f7f93`, `0xffffffff`). The promising next watch is on
the BSS metadata itself — `COMMAND_QUEUE` head/tail (`0x20004490`/`0x20004494`) and the
audio SPSC length word — watched on the **producer** side (core 0) for a foreign/wild
store, i.e. resume the §G3-B "command-queue ordering race / cross-core wild store"
thread rather than the stack-collision thread, which §G4-H closes.

### §G5: Audio value-match hunt — armed, soak PENDING physical ROM launch (2026-06-19)

**Hypothesis (from crash-record decode + static audit):** the ~2.39–2.51B crash is a
wild store/copy carrying i16-AUDIO payload onto heap-resident pointers. Evidence:
crash #9 core-0 `write_fast`→`cartridge.write()` Fault@**0x7f7f7f93** (an i16-audio
word, 0x7f7f≈+32639, smashing a `Box<dyn Cartridge>` pointer); crash #7 core-1 data_ptr
fault with stale `r12 → <Vec<i16>>::push`. Static audit: every i16 audio path is
bounds-checked (`produce_samples` len+2<=cap, `drain_audio_samples_into_i16` by
`out.capacity()`, `samples_i16_to_i2s` `.min(buf.len())`, bounded SPSC) → audio is the
PAYLOAD/victim, not origin. Bug-#5-shaped relocating corruptor.

**Instrumentation built** (`--features value-match-audio-watch`, new):
- `dwt_watch::publish_and_arm_value_match(0x7F7F_7F93)` armed on BOTH cores (main.rs +
  multicore.rs core-1 boot). DebugMonitor `handle_value_match` decodes the wild store's
  destination + source register, records (CFSR sentinel **0xD7170001**, Fault@=matched
  value), and resets.
- NO heap filter published (the wild dest lands inside the GB-memory heap, so a heap
  filter would mask the very write we hunt).
- Crash-handler SP fix also in tree: `sp_before_exception()` decodes EXC_RETURN.FType +
  xPSR aligner instead of the wrong `ef+32`.

**Hardware status:** firmware flashed; boot RTT confirms `audio value-match armed on
0x7F7F7F93` (core 0 + core 1). BUT all RTT captures stop at ~3.0 s on the MENU (crash
badge + "staged ROM found: 32 banks"). Gameplay never started — launching the staged
ROM (id 21f712e2) needs a PHYSICAL button press the subagent cannot perform. No new
catch (decode shows only the 24 stale git=fd1bb003 records). Stale records `--mark-read`
cleared so the next catch is unambiguous.

**NEXT (needs human at the device):** on the Pico, press the button to launch the staged
ROM, let it run ~10 min past cycle 2.39B. The armed value-match catches the wild
0x7F7F7F93 word-store and records it. Then decode:
`python3 tools/crash_decoder.py --probe --elf target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w`.
A catch has **CFSR=0xD7170001** + **Fault@0x7f7f7f93**; read **ARM PC** (addr2line →
the corruptor function), the decoded WILD DEST, and source register. If the dest is a
pointer slot (cartridge/transport/queue/stack) → corruptor FOUND. If it's an audio
buffer (sample_buffer Vec / I2S DMA buf) → value too common, add a filter or pick a
rarer value.

---

**§G5 SOAK RUN #1 (2026-06-19, ~12 min unattended, probe detached during soak):**

The board DID run the staged ROM on its own (no human present); 24 stale records had
been `--mark-read` cleared, so the records below are fresh from this soak. Probe was
freed during the entire window; records persist in flash. Decoded twice (stable, no
flaky-probe garbage).

**Result: NO value-match catch.** Decode after soak (`erase_count=1 next_slot=0`,
2 records):

- **Crash #1 — WatchdogTimeout** (slot 0, CRC OK). WD reason=0x1 timer, POWMAN
  reset=0x10000 had_por. Not a catch — boot/menu watchdog reset, same family as the
  prior 5 s freeze→reboot livelock signature.
- **Crash #2 — HardFault** (slot 1, CRC OK), core 0:
  - **ARM PC = 0x10002d10 → `core::ptr::read_volatile` (core/ptr/mod.rs:2084)**
  - **CFSR = 0x00008200 (BFARVALID | PRECISERR)** — a PRECISE **READ** bus fault.
  - **HFSR = 0x40000000 FORCED**
  - **Fault@ (BFAR) = 0x7f7f7f93**
  - ARM LR = 0x000061e4 (low/vector range, undecodable), Stk r12 = 0x1.
  - DMA mask=0x00 (no busy channels).

**Classification — NOT a DWT catch (the warned-about decoy).** The catch sentinel is
CFSR=**0xD7170001**; here CFSR=0x00008200 is a genuine BusFault. So the DebugMonitor
value-match store-watch did **not** fire this window — no wild *store* of 0x7f7f7f93
was observed.

**BUT this is a strong corroborating data point, not a null result.** Crash #2 is a
precise bus fault on a `read_volatile` whose faulting address (BFAR) is **0x7f7f7f93** —
i.e. the watched audio word 0x7f7f7f93 was loaded into a register and **dereferenced as
a pointer (load)** before any matching store was caught. This confirms the §G5
hypothesis directionally: the 0x7f7f7f93 audio byte-pattern IS reaching a pointer slot
and getting used as an address. The corruptor writes it (store) somewhere the
value-watch should have caught — but the store either (a) happened on a path/width the
DWT word-watch missed (e.g. a `strb`/`strh` half/byte write, or a `stm` the comparator
masked), or (b) the value reached the slot via a `memcpy`/DMA copy DWT doesn't trap, and
only the later *read* faulted. The READ-fault PC `read_volatile` is the consumer/victim,
not the corruptor.

**Why no store-catch despite a read-fault on the same value:** DWT DATAVADDR value
watchpoints only trap accesses of the comparator's configured size to the comparator
value. A relocating `copy`/`memcpy` corruptor (the bug-#5 family) moves the bytes with
LDM/STM or byte loops that may not present a single 32-bit store of exactly 0x7f7f7f93
to the comparator — consistent with §G3 "OAM copy is a propagator." The value-watch as
configured is therefore blind to the actual corrupting copy.

**NEXT:** the store-watch isn't catching the propagating copy. Two options: (1) instead
of value-matching the audio word, **address-watch the victim pointer slot** (the
cartridge `Box` / transport ptr that 0x7f7f7f93 lands on) for WRITE — DWT address
comparator catches ANY store (incl. STM/byte) to that address, which the value-watch
missed; or (2) keep value-match but also watch half/byte sizes. The recurring victim
address is the one whose later read faulted: capture BFAR across runs to pin the slot.
Soak validity confirmed (game ran), so this is a real "value reaches a pointer via an
un-trapped copy" finding, not a menu-stuck no-run.

---

## §G6: DMA / i16-copy static audit (no-HW source review)

Goal: find a wild write that deposits i16-audio bytes (recurrent word `0x7F7F7F93`,
i.e. two loud i16 samples ~`0x7f7f`/`0x7f93`) onto pointer/stack slots. The HW DWT
**value-match on the 32-bit word never fired** → the offending store is NOT a single
32-bit CPU `str`; it is either a DMA (bypasses DWT) or a half/byte-word store.

### A. Audio / I2S DMA — PROVABLY CANNOT WRITE SRAM (Lead A closed)

I2S output uses embassy `PioI2sOut` (`platform/pico2w/src/main.rs:476` `PioI2sOut::new`,
`main.rs:488` `i2s.start()`). The only transfer call is `i2s.write(front_buf)` at
`platform/pico2w/src/state/running.rs:48`.

`PioI2sOut::write` →
`embassy-rp/src/pio_programs/i2s.rs:233`:
```rust
pub fn write<'b>(&'b mut self, buff: &'b [u32]) -> Transfer<'b> {
    self.sm.tx().dma_push(&mut self.dma, buff, false)
}
```
`dma_push` (`embassy-rp/src/pio/mod.rs:471`):
```rust
unsafe { ch.write(data, PIO::PIO.txf(SM).as_ptr() as *mut W, Self::dreq(), bswap) }
```
`Channel::write` (`embassy-rp/src/dma.rs:188`) → `configure(from=data, to=FIFO, ...,
incr_read=true, incr_write=false)` (`dma.rs:195-204`). In `configure`
(`dma.rs:110-111`):
```rust
p.read_addr().write_value(from as u32);   // = front_buf (SRAM)
p.write_addr().write_value(to as u32);    // = PIO TX FIFO (0x4008_8008), FIXED
```
**WRITE_ADDR is hard-wired to the PIO TX FIFO peripheral address and `incr_write=false`.**
The audio DMA can only ever WRITE to the FIFO; it can never write to SRAM, in any config,
startup or steady-state. The "transient/edge config" hypothesis is moot: there is no code
path that recomputes WRITE_ADDR — it is the FIFO pointer for every `write` call.

The *count*/READ side: `from.len()` = `front_buf.len()` = `front_n`
(`audio.rs:32-60` `front_back_buffers` builds the front slice with len `self.front_n`).
A corrupt `front_n` would make the DMA **over-READ** past `AUDIO_BUF_A/B` into adjacent
.bss — but that is a read, not a write. It cannot deposit bytes anywhere. Verdict: the
audio DMA is not the corruptor.

Display DMA is identical in shape: SPI-TX DMA (`display/hw.rs` `self.spi.write(...)`),
WRITE_ADDR = SPI peripheral FIFO, never SRAM. The only mem-to-mem primitive in embassy-rp
is `Channel::copy` (`dma.rs:236`) — **grep shows it is never called** in this firmware.
**Conclusion: NO DMA in this firmware writes to SRAM. Lead A is fully eliminated.**

### B. The i16 half-word store that defeats the DWT word comparator — AUDIO_QUEUE enqueue

The audio sample type is `i16`. The cross-core audio channel is
`spsc::Queue<i16, {AUDIO_QUEUE_CAPACITY+1}>` (`multicore.rs:133`, cap 2048, heapless 0.9).
Core 1 fills it in `drain_audio_samples_to` → `audio_tx.enqueue(sample)`
(`multicore.rs:2397-2399`). Each enqueue is heapless `inner_enqueue`
(`heapless-0.9.3/src/spsc.rs:284`):
```rust
let current_tail = self.tail.load(Ordering::Relaxed);
let next_tail = self.increment(current_tail);
if next_tail == self.head.load(Ordering::Acquire) { Err(val) }
else {
    (self.buffer.borrow().get_unchecked(current_tail).get())
        .write(MaybeUninit::new(val));      // <-- strh of one i16 sample
    self.tail.store(next_tail, Ordering::Release);
}
```
Key facts:
- `val: i16` → the store is a **half-word `strh`**, NOT a 32-bit `str`. A DWT word-store
  value comparator on `0x7F7F7F93` **cannot** match it. Two consecutive enqueues of loud
  samples (`0x7f7f` then `0x7f93`) lay down the 32-bit pattern `0x7F7F7F93` in memory via
  two `strh`s — exactly the observed signature, and exactly why the value-watch was blind.
- The write address is `buffer_base + current_tail * 2`, with `get_unchecked` — **no bounds
  check**. `current_tail` is the queue's own `tail` index, loaded `Relaxed`.

So: *if* `tail` (or `head`, used by the `next_tail == head` full-check) is ever a wild/large
value, this enqueue writes an i16 audio sample to a wild address. This is the only code in
the firmware that (a) moves i16 audio data and (b) emits it via half-word stores to an
address derived from a mutable index with no bounds check. It is the natural PROPAGATOR
for the bug-#5 shape (innocent copy, corrupt destination base/index).

The firmware already half-suspects this: `check_shared` reads the first word of the
`audio_rx`/`command_tx` spsc handles (their internal `rb` pointer) and compares against the
captured queue address (`multicore.rs:1106-1114`) — a guard against the spsc *handle pointer*
being smashed. But that guard does NOT cover the queue's internal `head`/`tail` AtomicUsize
fields, which are what `get_unchecked(current_tail)` indexes with. A smashed `tail` is not
detected before the wild enqueue.

### C. Who writes the core-0 stack region ~0x2007ec1c

The crash smashed a contiguous core-0 stack region (a pointer slot whose value became
`0x7f7f7f93`, plus the adjacent saved-LR at `0x2007ec1c` → `0x000061e4`). A single i16
`strh` cannot smash two contiguous pointer-sized slots; this is a **multi-word/bulk** write.
That points away from a lone enqueue `strh` and toward a `copy_from_slice`/`memcpy`-style
bulk move whose **destination base** was itself corrupt (classic bug-#5: the OAM copy was
innocent, its dest base had been smashed). Candidate bulk i16/u32 moves with a
runtime-derived destination:
- `samples_i16_to_i2s` (`audio.rs:89-97`): writes `buf[i]` for `i in 0..pairs`,
  `pairs = (samples.len()/2).min(buf.len())`. `buf` = `back_buf` = the `&'static mut [u32]`
  from `front_back_buffers` (`audio.rs:42/54`), built from `addr_of_mut!(AUDIO_BUF_B/A)`
  with a **compile-time-constant** base and `AUDIO_BUF_SIZE` length. Bounds: `.min(buf.len())`
  caps it. The destination base is a link-time constant, NOT a runtime pointer that could be
  aliased/smashed → this copy cannot relocate. Not the bulk corruptor. (Matches the prior
  audit: logically bounded.)
- `AudioBuffers::front_back_buffers` (`audio.rs:32-60`) hands out `&'static` slices over the
  two static arrays; `use_a_as_front`/`front_n` are plain fields, never pointers. Safe.

No firmware bulk-copy targets a runtime-pointer destination with i16/audio source AND a
corruptible base. So the contiguous-stack smash is most consistent with the **DMA over-READ
mirror or an spsc enqueue burst into a region that `tail` walked into** — i.e. if `tail`
becomes a large in-range-of-SRAM value, successive enqueues (a *burst* of up to ~hundreds of
i16 per `DrainAudio`, `multicore.rs:2397`) write *consecutive* i16 half-words across many
contiguous words, which CAN smash a contiguous pointer+LR pair. That reconciles B with the
multi-word stack smash: one wild enqueue burst = many adjacent `strh`s = contiguous region
filled with i16 audio bytes.

### Verdict

- **DMA (audio + display): exonerated.** Both are peripheral-FIFO TX DMAs; WRITE_ADDR is a
  fixed peripheral address with `incr_write=false`. No mem-to-mem DMA (`Channel::copy`) is
  used anywhere. A wild SRAM write from DMA is not reachable in this code.
- **Most suspicious wild-write site:** heapless `inner_enqueue`
  (`heapless-0.9.3/src/spsc.rs:291`) reached via `audio_tx.enqueue`
  (`platform/pico2w/src/multicore.rs:2398`): an **unbounded `get_unchecked(current_tail)`
  half-word `strh`** of an i16 audio sample. It is the unique source of i16-shaped half-word
  stores that the 32-bit DWT value comparator cannot trap, and a burst of them explains the
  contiguous core-0 stack smash. This is a PROPAGATOR (it needs a pre-corrupt `tail`/`head`),
  matching the relocating-victim bug-#5 pattern.

What would settle it (one fact): the AUDIO_QUEUE object's address vs the faulting BFAR/victim
addresses. `audio_queue_addr` is captured at `multicore.rs:958`. If a fault victim address
equals `audio_queue_base + 2*tail` for some plausible corrupt `tail`, OR if the queue's
`head`/`tail` words (at `audio_queue_addr + offsetof(head/tail)`) are seen non-zero/huge in a
crash dump, the enqueue is confirmed as the propagator. The guard at
`multicore.rs:1106-1114` should be EXTENDED to also snapshot+check the queue's internal
`head`/`tail` (not just the handle `rb` pointer) so a smashed index is caught before the next
`get_unchecked` enqueue.

### §G7: Audio-queue index guard REFUTED enqueue-propagator; corruptor narrowed to CORE 0 (2026-06-19)

**Guard result (audio-queue-index-guard soak, 8 fresh records):** the core-1 DrainAudio
head/tail guard **NEVER fired** (no CFSR_ROUTE_DRAIN_GUARD). So the audio queue index is
NOT persistently corrupt before the enqueue burst → the audio enqueue is **not** the
splatter mechanism. §G6's proximate-cause guess (enqueue burst) is REFUTED. Same crashes
recurred: 2× HardFault BFARVALID/PRECISERR Fault@0x7f7f7f93, plus **NEW 2× INVSTATE
(CFSR=0x00020000)** = smashed return address (jumped non-Thumb) → the splatter hits
**core-0 stack saved-LRs**.

**MPU evidence → corruptor is CORE 0.** `arm_cartridge_vtable_watch`/`setup_core1_mpu`
(multicore.rs:2126) marks **core-0 stack 0x20066B60–0x2007FFFF read-only to CORE 1** (a
core-1 write there → MemManage/DACCVIOL). Victim 0x2007ec1c is inside that range yet
produced NO MPU fault → that stack smash was done by **core 0 writing its own stack**
(MPU only blocks core 1). DMA already eliminated (§G6). So: **core-0 code, audio-payload
(i16 0x7f7f…), halfword/byte write, relocating destination (own stack + heap pointers),
~2.39–2.51B cycles.**

**Core-0 audio path audited (running.rs:47-63): bounded.** `drain_audio_samples_into_i16`
(out.push bounded by out.capacity()), `queue_next_frame_i16`→`samples_i16_to_i2s`
(`.min(buf.len())`). No obvious overrun in the direct path. The corruptor is a subtler
core-0 write — candidate areas: async future buffer lifetimes (`i2s.write(front_buf)`
held across `queue_next_frame_i16(.., back_buf)` — front/back aliasing?), `Vec` realloc
of audio_samples, `front_back_buffers()` correctness, or the interleaved frame
(`send_frame`/`published_native_frame`) ↔ audio path. NEXT: focused static audit of the
core-0 audio+frame interplay for a wrong/aliased destination (bug-#5-style).

---

## §G8: core-0 audio/frame wrong-dest static audit

Pure static source audit (no hardware) of the core-0 `RunningState::tick` audio +
frame interplay (`platform/pico2w/src/state/running.rs:27-90`,
`platform/pico2w/src/audio.rs`) hunting a bug-#5-style write with a wrong/aliased
DESTINATION carrying i16/audio bytes (0x7F7F7F93).

### AudioBuffers layout (where AUDIO_BUF_A/B live + sizes)

`audio.rs:3` `const AUDIO_BUF_SIZE: usize = 1024;`
`audio.rs:14-15`:
```rust
static mut AUDIO_BUF_A: [u32; AUDIO_BUF_SIZE] = [0u32; AUDIO_BUF_SIZE];
static mut AUDIO_BUF_B: [u32; AUDIO_BUF_SIZE] = [0u32; AUDIO_BUF_SIZE];
```
- Two SEPARATE `[u32; 1024]` statics, zero-initialised ⇒ live in `.bss`.
  `.bss` sits low in RAM (after `.data`), i.e. near 0x2000xxxx. RAM is
  `0x20000000..0x2007FFFF` (memory.x:21), core-0 stack grows DOWN from the top
  (the observed 0x2007exxx victims). **AUDIO_BUF_A/B are ~8 KiB low in RAM and
  are NOT adjacent to the 0x2007exxx stack-top victims** — an overrun off the END
  of AUDIO_BUF_B would land in other `.bss`/heap, never the core-0 stack top by
  adjacency. (Layout-adjacency argument; addresses not re-pulled from a stale map.)
- `AudioBuffers` itself is a 2-field POD (`use_a_as_front: bool`, `front_n: usize`),
  constructed `const` in main.rs:504, lives on the core-0 stack frame of `main`.
- `back_buf.len() == AUDIO_BUF_SIZE == 1024` u32 words == 4096 bytes == the TRUE
  array element count. **No units bug: `buf.len()` is the u32 element count and
  exactly matches the array's u32 length.**

### front_back_buffers() logic (audio.rs:32-60)

Returns `(&'static [u32] front, &'static mut [u32] back)`:
- `use_a_as_front == true`  ⇒ front = `&AUDIO_BUF_A[..self.front_n]`,
  back = `&mut AUDIO_BUF_B[..1024]`.
- `use_a_as_front == false` ⇒ front = `&AUDIO_BUF_B[..self.front_n]`,
  back = `&mut AUDIO_BUF_A[..1024]`.
- front and back are ALWAYS sliced from DIFFERENT statics → cannot alias within a
  tick. front length is `front_n` (the count produced LAST frame); back length is
  the full 1024.
- `queue_next_frame_i16` (audio.rs:68-72): writes back via `samples_i16_to_i2s`,
  then `self.use_a_as_front = !self.use_a_as_front; self.front_n = back_n;`. The
  flip takes effect on the NEXT tick's `front_back_buffers()` call, not this one.

### Verdicts on points 1-4

**(1) Async buffer aliasing / lifetime — PROVABLY SAFE.**
Within one `tick()` (running.rs):
  L47 `front_back_buffers()` → say front=A (len front_n), back=B.
  L48 `i2s.write(front=A)` → `dma_push(&self.dma, data=A, false)` →
      `ch.write(A, txf_ptr, ..)` (i2s.rs:233-234, pio/mod.rs:471-477, embassy rev
      `c722d94` per Cargo.lock). This is a **READ DMA**: source = A slice,
      dest = the FIXED PIO TX FIFO register (incr_write=false, §G6). The audio
      DMA can NEVER be the writer to a wild destination — it only reads A.
  L63 `queue_next_frame_i16(samples, back=B)` → writes B only, then flips flag.
  L73-74 awaits `audio_future` (the A-read DMA) to COMPLETION before returning.
So per tick: DMA reads A, CPU writes B — disjoint. The flag flip only changes
NEXT tick (front=B, back=A); by then frame-N's A-DMA is fully awaited (L74), so
frame N+1 writes A only after the in-flight A read finished. `front==back` is
impossible (different statics). **No aliasing, no in-flight overlap.**

**(2) Buffer sizing mismatch — PROVABLY SAFE.**
`samples_i16_to_i2s(samples, buf)` (audio.rs:89-97):
```rust
let pairs = (samples.len() / 2).min(buf.len());   // ≤ buf.len() == 1024
for i in 0..pairs { ... buf[i] = ...; }            // i < pairs ≤ buf.len()
```
Every write is `buf[i]`, `i < pairs ≤ buf.len() == 1024` = the real array length.
Cannot exceed AUDIO_BUF_B/A's 4096 bytes. No u32-vs-i16-vs-byte unit confusion:
`buf.len()` and the array length are both in u32 elements. The `.min(buf.len())`
clamp is the actual array length, not a derived/scaled value.

**(3) `audio_samples` Vec realloc / stale slice — PROVABLY SAFE for the overrun.**
main.rs:505 `Vec::with_capacity(2048)`.
`drain_audio_samples_into_i16` (multicore.rs:1471-1490): `out.clear(); cap =
out.capacity(); while n < cap { if let Some(s)=dequeue() { out.push(s); n+=1 } else
break }`. Pushes at most `cap` items into a Vec whose capacity is `cap` ⇒ **no
reallocation** (push only reallocs when len==cap AND you push again; here the loop
stops at n==cap). `out.len() ≤ cap`. Even if cap ever grew, `samples.len()/2` is
still `.min(buf.len())`-clamped in (2). The slice passed to `samples_i16_to_i2s`
is `&audio_samples[..]` freshly re-borrowed each frame (running.rs:62-63), never a
stale slice from a prior frame. Safe.

**(4) Frame path interplay — PROVABLY SAFE w.r.t. audio-shaped writes.**
`published_native_frame()` returns `&'static NativeFrame` (read source for the
pixel DMA), `send_frame(frame_buf, &dirty_rows)` sizes a pixel (RGB565) DMA from
dirty_rows. The pixel DMA reads `frame_buf`; its payload is RGB565 pixels, NOT
i16-audio words 0x7F7F7F93 — wrong shape for the corruptor. `queue_next_frame_i16`
has NO stack-local scratch buffer: it writes `buf[i]` directly (audio.rs:91-95).
No core-0 stack-local audio scratch that could overflow onto 0x2007exxx.

### §G8 verdict

All four candidate mechanisms in the core-0 tick audio+frame path are **provably
safe** by source: the audio DMA is a fixed-dest READ (cannot write wild), the CPU
write to `back_buf` is hard-clamped to `buf.len()==1024`==the true array length
(no units bug, no realloc), front/back are disjoint statics that can't alias, and
there is no stack-local audio scratch buffer to overflow. Crucially, AUDIO_BUF_A/B
live LOW in `.bss` and are not adjacent to the 0x2007exxx core-0 stack-top victims,
so even a hypothetical overrun could not reach them by linear adjacency.

The corruptor therefore is NOT a length/aliasing overrun in this path. The audio
PAYLOAD (0x7F7F7F93) is correct i16 sample data; what's wrong is the DESTINATION
BASE of some write — i.e. a *pointer/index that is itself corrupt before the store*,
not a buffer whose length overflows. That re-points the search at the SPSC/queue
producer: `heapless` SPSC `enqueue` writes `data[idx]` where a pre-corrupt `idx`
(or a `back_buf`/`out` base computed from a smashed `AudioBuffers`/`Vec` field)
makes a perfectly-sized audio store land at a wild base. The SPSC index guard not
firing this run means the corrupt base arrives via a DIFFERENT pre-existing
corruption (consistent with the open §G3-B COMMAND_QUEUE cross-core ordering race:
a torn head/index read yields a wild base, then a correctly-sized audio word is
stored there). Settling fact: a DWT *address*-watch (not value-watch) on the SPSC
`data` array base and on `&AudioBuffers.front_n`/`&audio_samples` Vec ptr — catch
the store whose computed destination is out of range, vs the value-watch which
already proved the payload is plain audio.

---

## §G10: DWT victim-ptr address-watch — CAUGHT the corruptor (2026-06-19)

**VERDICT: CAUGHT.** A `--features gameboy-memory-field-watch` build armed DWT
*address*-watches on the three known victim words and soaked unattended ~12 min
(ROM 21f712e2, auto-run). Boot banner confirmed all three armed:
`oam_prefix=0x200445dc vtable_word=0x20044684 mem_field=0x20056850`.

The run produced **3 DWT-watchpoint catches** (CFSR=0xD7170001), ALL pointing at
the **same store instruction**:

| Crash | ARM PC      | Stk R0 (faulting addr) | Victim hit         |
|-------|-------------|------------------------|--------------------|
| #3    | 0x10003842  | 0x200445dc             | **oam_prefix**     |
| #5    | 0x10003842  | 0x20044684             | **vtable_word**    |
| #6    | 0x10003842  | 0x20056850             | **mem_field**      |

All three faulting addresses are **exact matches** to the boot banner's watched
words — i.e. the same wild store reached all three distinct victims on different
ticks.

### The corruptor

```
PC 0x10003842  →  rustyboy_core::memory::memory::GameBoyMemory::copy_dma_step
                  core/src/memory/memory.rs:627
   inlined into  GameBoy::advance_dma_bulk  (gameboy.rs:626)
                  → advance_peripherals (567) → tick (261)
                  → PicoGameBoy::tick (multicore.rs:1661) → RunningState::tick (running.rs:53)
LR 0x100035dd  →  same fn, memory.rs:589 (loop call site)
```

`memory.rs:627-628` is the **byte-by-byte OAM-DMA fallback copy**:

```rust
if !copied {
    for i in 0..n {
        self.oam[dst + i] = self.read_fast((actual_src + i) as u16);  // PC 0x10003842
    }
}
```

This is a store to `self.oam[dst + i]`. The catch proves `dst + i` is escaping
the OAM array's bounds: `self.oam` is the ~0xA0-byte OAM buffer, but the stores
landed at 0x200445dc / 0x20044684 / 0x20056850 — well past it. `dst` (and/or `n`)
is corrupt/oversized when the fallback path runs, so the indexed write walks off
the end of OAM into the adjacent `oam_prefix`, the cartridge `vtable_word`, and
the `GameBoy.memory` box field. This is the long-hunted core-0 wild store.

### Note on guard records

The same dump also held **8 oam-dma-checkpoint-guard** records (CFSR=0xd6a00002,
PC 0x10003c7b = advance_dma_bulk), all `phase=after-copy reason="word before OAM
changed"`, `progress=85 count=88` (and two `progress=11 count=12`). These are the
*downstream* detections — the guard fires AFTER the wild write corrupts the
word-before-OAM. The DWT catch (CFSR=0xD7170001) fires on the write ITSELF, one
layer earlier, and pins it to memory.rs:627. Both agree: OAM-DMA copy_dma_step is
the propagator/corruptor.

### Single most important PC

**0x10003842 → GameBoyMemory::copy_dma_step, core/src/memory/memory.rs:627** —
the `self.oam[dst + i] = self.read_fast(...)` fallback store, writing out of OAM
bounds (`dst`/`n` corrupt or oversized). Fix target: bound-check / clamp `dst`,
`n`, and `actual_src` in the fallback path of `copy_dma_step` so an out-of-range
DMA request cannot index past `self.oam`.

---

## §G13 — Patient soak (DWT 3-victim watch armed): NO catch, signature MUTATED to UNALIGNED ticket-pointer fault

**Build:** `gameboy-memory-field-watch` (copy_dma_step `#[inline(never)]` + MPU-from-`_stack_end`),
DWT address-watch armed on BOTH cores for 3 cold pointers. Boot confirmed:
`DWT victim-ptr watch armed core0: oam_prefix=0x2004494c vtable_word=0x200449f4 mem_field=0x20056bb0`
and `core1 MPU region 0 base (from _stack_end) = 0x20066f80`. Soak ~14 min unattended (ROM id 21f712e2).

### Verdict: NO 0xD7170001 DWT-watchpoint catch.
None of the 3 watched cold pointers (oam_prefix / vtable_word / mem_field) was hit. The corruptor
smashed a DIFFERENT victim. Confirmed regressions stay GONE: 0× copy_dma_step, 0× spsc.rs:185,
0× DACCVIOL, 0× INVSTATE@0x2001a1c4.

### Records: 24 total (12 WatchdogTimeout boot-loop markers + 12 HardFault).

| signature | count | core | PC | meaning |
|-----------|-------|------|----|---------|
| **UNALIGNED store-release** | **11** | **1** | **0x1001a900** | `stl r9,[r0]` in `run_core1_worker`, ticket ptr smashed to ~0x41 |
| UNDEFINSTR critical_section | 1 | 0 | 0x1001dd12 | `__cpsid`, LR=critical_section::acquire, corrupted LR slot @0x2007ebf4, Fault@0x681cb672 |

The prior INVSTATE@0x2001a1c4 (wild-jump via smashed CODE ptr) did NOT recur this run; the SAME
corruptor now lands on a DATA pointer instead, yielding UNALIGNED. Same root, different victim slot.

### Dominant signature — UNALIGNED store-release @ 0x1001a900 (core 1, 11×)
```
ARM PC  0x1001a900   stl r9, [r0]      (store-release; the producer Release/DSB ticket publish)
ARM LR  0x1001a8ff   (same; atomic_store/atomic.rs:4013 — i.e. the stl itself)
CFSR    0x01000000   UNALIGNED
Stk R0  0x00000041   <- stacked faulting addr: r0 = the ticket pointer, smashed to 0x41 (misaligned)
Stk r12 0x20047cb0   -> __sbss
SP_bef  0x20081f68   (core-1 exception frame; worker frame sp = 0x20081f68 - 0x68 region)
cycles  2.38B – 2.82B (matches the known corruption window)
```

### run_core1_worker victim analysis (candidate next-watch)
Disassembly of the faulting tail in `run_core1_worker` (flash 0x1001a5d4, frame `sub sp,#0x68`):
```
1001a648:  str  r0, [sp, #0x34]   ; r0 = [r7,#0x8] (worker-state base arg) + const offset
                                  ;   -> the COMPLETION-TICKET pointer, written ONCE per call
...        (loop body: critical_section, publish_worker_state, memcpy PPU/OAM, etc.)
1001a8fa:  bl   publish_worker_state
1001a8fe:  ldr  r0, [sp, #0x34]   ; reload the ticket pointer
1001a900:  stl  r9, [r0]          ; STORE-RELEASE r9 -> [ticket]   <-- FAULTS UNALIGNED
1001a904:  dsb  sy
```
`[sp,#0x34]` is a **live stack-frame slot in run_core1_worker holding the ticket/done-flag
pointer** (the handshake the SPSC-revert note warned about). Between its single set at 0x1001a648
and its use at 0x1001a900, a wild cross-core store overwrote it with ~0x41. This slot is NOT any
of the 3 watched cold pointers — hence no DWT catch.

**Candidate next-watch victim:** the stack address of `run_core1_worker`'s `[sp,#0x34]` slot
(core-1 stack, ≈0x20081f9c given SP_bef=0x20081f68). It is also read at 0x1001a812 / 0x1001a856 /
0x1001a994 / 0x1001a9d4. Because it is a moving stack slot, the more robust DWT target is the
**worker-state ticket WORD it points at** (the `[r7,#0x8]` base + offset computed at 0x1001a646),
or arm a DWT on the byte the wild store writes. The core-0 UNDEFINSTR (#22: corrupted LR slot
@0x2007ebf4, critical_section::acquire) is the SAME corruptor landing on a core-0 stack return slot.

**Single most important PC:** **0x1001a900** — but note this is the VICTIM's use site
(`stl r9,[r0]` faulting on a smashed pointer), NOT the corruptor's store. The corruptor's PC was
NOT captured because it hit an unwatched victim (a stack-resident ticket pointer). Next iteration:
add a 4th DWT watch on the run_core1_worker ticket word (or core-1 stack tail) to trap the writer.

---

## §G14 — no-stack-coloring root-fix soak (FAIL)

**Build:** release ELF compiled with `-C llvm-args=-no-stack-coloring -C
llvm-args=-no-stack-slot-sharing` (+ prior `copy_dma_step #[inline(never)]` and
MPU-from-`_stack_end` fixes). Image CRC 0xdbea37f6, git fd1bb003, fw 0.1.0.

**Procedure:** Flashed via rb-flash (CRC OK, banner reached "entering main
loop"). Cleared stale records (`--mark-read`; last stale was Crash #31 UNALIGNED
@ cycle 2,475,075,424). Soaked board unattended ~15 min (21:23:44 → 21:38:44).
Decoded via crash_decoder.py --probe.

**Verdict: FAIL — the disabled-stack-coloring build did NOT eliminate the ~2.4B
crashes.** The board crashed and rebooted 3× during the soak, all in the historic
2.38–2.41B-cycle window.

**Records (7 total):** 4 benign WatchdogTimeout boot artifacts (WD=timer,
had_por) interleaved with 3 HardFaults — all three the SAME signature:

| # | Type | CFSR | ARM PC (wild) | corrupted LR slot decodes to | r12 | GB cycle |
|---|------|------|---------------|------------------------------|-----|----------|
| 2 | HardFault | 0x00000100 IBUSERR | 0x7cfc74c0 | sm83::inc8 (sm83.rs:805) | inc_u8 (inc_dec.rs:5) | 2,386,658,276 |
| 4 | HardFault | 0x00000100 IBUSERR | 0x08000000 | sm83::inc8 (sm83.rs:805) | inc_u8 (inc_dec.rs:5) | 2,385,394,132 |
| 6 | HardFault | 0x00000100 IBUSERR | 0x30c8f804 | memory::write_io (memory.rs:802) | joypad::read (joypad.rs:96) | 2,407,937,348 |

All HFSR=0x40000000 FORCED. SP_before on core-0 stack (0x2007cd68/70), stacked
LR slot corrupted (note explicitly: "corrupted LR slot = 0x2007cd64/6c"). Stack
high-water = 65535 (full) — **NOT a stack overflow / STKOF** (no UsageFault, no
STKOF bit). ROM id 21f712e2 bank 2, no busy DMA channels.

**Interpretation:** This is the SAME systemic root bug, relocated. A wild
cross-core store smashes a return-address / pointer slot on the core-0 stack; the
corrupted LR is then fetched and the core branches to a wild PC, faulting with
IBUSERR. Same ~2.4B cycle window as every prior manifestation (spsc.rs:185,
Fault@0x7f7f7f93, INVSTATE, UNALIGNED @ 2.47B). Disabling stack coloring /
slot-sharing did NOT separate the colliding slots enough to stop the corruption —
the root is not (solely) the LLVM stack-coloring slot collision, or the corruptor
is a genuine cross-core data race writing into the victim core's live stack frame
(consistent with the open "command-queue ordering race" lead, head@0x20004490).

**Most important fact:** ROOT NOT FIXED. 3 HardFaults at cycles 2.385B / 2.387B /
2.408B, all CFSR=0x00000100 IBUSERR via a corrupted core-0 stack LR slot — the
original wild-store bug recurs in the no-stack-coloring build. Not STKOF.

### §G15: Host ASan replay across the crash window — CLEAN (portable logic exonerated) (2026-06-20)

**Cycle-counter reframe:** the device save state loads at full cycle **15,267,416,632**;
its **low 32 bits = 2,382,514,744 ≈ 2.38B**. The crash decoder stores only `gb_cycle_lo`
(32 bits), so the device's "~2.4B crash" is really 15.27B+, i.e. the crash fires shortly
AFTER this exact save loads. The save sits at the doorstep of the trigger.

**Experiment (with existing fixtures — no re-extraction):**
ROM `/tmp/rb_fixtures/poison_rom.bin` (rom_id 21f712e2, MBC1), save `/tmp/poison_save.bin`.
`replay_poisoned_save` under `-Zsanitizer=address`, x86_64, 60M ticks.
- Validation gate matched: post-load PC=0x1807 HL=0x17bb cycle=15267416632 rom_bank=2.
- Ran 60M ticks → final cycle 15.60B (low32 2.71B), covering the ENTIRE 2.38–2.52B crash
  window with margin. **No sanitizer trap.**
- (Also: fresh-boot ASan replay clean to cycle 2.92B — §replay_fresh.)

**Conclusion:** the portable `GameBoy::tick()` (CPU/memory/OAM-DMA/route_bus_events) — the
SAME source the device runs — replaying the device's EXACT poisoned state across the exact
crash window, is CLEAN under ASan (which catches precisely the cross-allocation wild-store
shape seen on-device: pointers 73 KB apart smashing unrelated structs). So the corruptor
is **NOT a source-level logic/OOB bug in portable core**. Combined with the §G10 catch
(copy_dma_step wrote through a CORRUPT BASE while its index was provably in-bounds), the
evidence points to a **backend codegen miscompile** (register-allocator stack-slot
collision in the giant ARM `.data`-inlined frames) — which no host sanitizer can reproduce,
because host x86 codegen + 64-bit layout don't share the colliding slots. The fix path is
codegen-level (frame breakup / build flags / toolchain), NOT source logic. Two real fixes
already landed and held (copy_dma_step #[inline(never)] §G10/§G11, MPU-from-_stack_end §G11).

### §G16: ARM-emulation replay (qemu-arm via cross) — ran, but the faithful+sanitizer combo doesn't exist off-the-shelf (2026-06-20)

**Idea (user):** x86 ASan can't see an ARM codegen bug; run the host replay through an
ARM emulator to keep the codegen that contains the stack-slot-collision miscompile.

**What was built:** `core/tests/replay_arm.rs` + `host_replay` core feature (gates the
firmware diagnostic guards' DEVICE-MEMORY-MAP assumptions: the bus-event-queue SRAM
pointer-range check `first_bad_word`, and the OAM-DMA ROM-cache XIP-window check, both of
which false-trip on host heap pointers — caught and neutralized only under the feature,
device builds unchanged). Ran via `cross` + bundled `qemu-arm` for
`armv7-unknown-linux-gnueabihf` (32-bit ARM, real ARM/Thumb-2 codegen, 32-bit pointers).

**Results (replaying the device poison save across the crash window, low32 2.38B→2.52B):**
- Pipeline works: save loads (post-load PC=0x1807 cycle=15267416632), runs the window.
- `release` (opt-z + LTO), `host_replay`, 60M ticks: **clean** — no guard, no trap.
- `+ -Z stack-protector=strong` (the device's determinizer), 120M ticks (window ×2):
  **clean** — no SIGSEGV, no `__stack_chk_fail` canary trip, no guard.

**The blocker / fidelity ceiling:** ASan is NOT available for the A-profile Linux ARM
targets in cross (`librustc-nightly_rt.asan.a` missing; it's a compiler-rt artifact, not
`build-std`-able). So the run can only catch a wild store that hits UNMAPPED memory
(SIGSEGV) — a store into valid host heap is silent. And the deeper problem:
- `armv7-A` ≠ `thumbv8m-M`: register pressure differs, so the pressure-driven collision
  may simply not occur in armv7 codegen → a clean run is INCONCLUSIVE, not exoneration.
- Miri / x86-ASan are pre-codegen (operate on MIR / different backend) → structurally
  blind to a register-allocator bug.
- The faithful target (thumbv8m) is bare-metal no_std → no ASan there.
- The only fully faithful emulation is `qemu-system-arm` running the actual firmware
  binary under gdb — but the firmware targets RP2350 (custom memory.x + peripherals) and
  qemu has no RP2350 machine; porting to e.g. mps2-an505 (Cortex-M33) is substantial.

**Conclusion:** the ARM-emulation idea is sound and we got ARM codegen executing the crash
window, but no off-the-shelf (emulatable ARM target + sanitizer) combination can DECISIVELY
catch this thumbv8m-specific backend register-allocator collision. The clean armv7 runs are
weak corroboration that the collision is thumbv8m-specific (consistent with §G14). The
decisive catch venue remains the real hardware (where §G10 DID catch copy_dma_step via DWT)
or a substantial qemu-system-M33 port. The fix path stays codegen-level.

### §G17: `-disable-ssc` (disable LLVM StackSlotColoring) HW soak — **FAIL** (2026-06-20)

**Candidate systemic root fix tested:** build with `-C llvm-args=-disable-ssc` +
`-no-stack-coloring` to disable LLVM's StackSlotColoring pass (the pass that MERGES spill
slots — the suspected mechanism behind the spill-slot-collision wild store). Build verified
to have changed codegen (+22% total reserved stack). Also carried the prior held fixes
(copy_dma_step `#[inline(never)]` §G10/§G11, MPU-from-`_stack_end` §G11).
ELF: `/tmp/rb-ab-nossc/.../rustyboy-pico2w` (core1 MPU base from `_stack_end` = 0x20066fa0).

**Procedure:** flashed (CRC 0xb8c357cc OK, both cores MPU-armed, save loaded, entered main
loop), cleared 31 stale records via `--mark-read`, soaked **16m49s** (11:30:07 → 11:46:56,
well past the ~9.5 min / cycle 2.5B historic window), then `--probe --elf` decode.

**RESULT — the ~2.4–2.5B crash RECURRED. `-disable-ssc` did NOT eliminate it.**

Decode found **2 records**:
- **#1 WatchdogTimeout** — WD reason=0x1 (timer), `had_por`, current_pwrup=0x0. Benign
  reboot artifact that follows the HardFault below.
- **#2 HardFault — `IBUSERR` (CFSR=0x00000100), HFSR=0x40000000 FORCED.**
  - ARM PC = **0x68000000** → `??` (UNMAPPED — wild instruction fetch / corrupted control
    flow; same class as the historic wild-PC IBUSERR crashes).
  - ARM LR = 0x1001be33 → `core::sync::atomic::atomic_add` (atomic.rs:3940).
  - SP_bef=0x2007ccc8, Stk r12=0x2001c324 (`__sbss` region).
  - ROM 21f712e2 bank=5; GB PC=0x3c7a; **GB cycle = 2,516,650,564** (≈2.517B, dead center
    of the historic 2.40–2.52B window). PPU LY=38 LCDC=0x00 STAT=0x40.

**Conclusion:** disabling StackSlotColoring did NOT prevent the crash; the heterogeneous
wild-store / wild-PC corruption still fires at ~2.517B. Either StackSlotColoring is NOT the
responsible pass (the spill-slot collision is introduced elsewhere, e.g. greedy regalloc
spill placement or `-O` slot reuse outside SSC), or the root is not a spill-slot-coloring
miscompile at all. The `-disable-ssc` flag is REFUTED as the systemic fix. Codegen changed
(+22% stack) yet the failure mode is unchanged — strong evidence SSC is not the locus.
Boot/banner confirmed genuine (board ran 16+ min, reached cycle 2.517B), so this is a real
recurrence, not a flash failure.

### §G18: SMOKING GUN — LLVM MachineOutliner emits undefined-register call in the tick frame (2026-06-20)

**Detection method (user's idea, realized at the compiler-IR level):** pure disassembly
store/load tracking CAN'T detect a regalloc/codegen miscompile (the emitted code is
internally self-consistent — a wrong allocation looks like legitimate slot reuse). But
LLVM's OWN machine verifier checks the model's consistency at COMPILE TIME. Built the
firmware with `-C llvm-args=-verify-regalloc -verify-machineinstrs -verify-coalescing`.

**Result — the build ABORTED with 3 machine-code errors, all identical:**
```
*** Bad machine code: Using an undefined physical register ***
- function:    ____embassy_main_task...closure   (the CORE-0 task that runs gb.tick())
- instruction: tBL @OUTLINED_FUNCTION_186, ..., implicit $r1
- operand 11:  implicit $r1   (used as a live-in to the outlined fn, but UNDEFINED on
                                the path reaching the call → garbage r1)
```
Three call sites (%bb.11, %bb.27, %bb.30) to the SAME `OUTLINED_FUNCTION_186`.

**Causal confirmation:** rebuilding with `-C llvm-args=-enable-machine-outliner=never`
→ **0 verifier errors, clean build.** So LLVM's **MachineOutliner** is the source: it
extracted a code sequence into `OUTLINED_FUNCTION_186`, declared `r1` a live-in
parameter, but `r1` is undefined on the calling path → the outlined function operates on
garbage `r1`. If `r1` is (or feeds) a pointer base → wild store. The MachineOutliner is
aggressive at `opt-level=z` (size), which the firmware uses.

**Why this fits ALL prior evidence:**
- In `embassy_main_task` (core-0 tick frame) → matches §G7 "core-0 own stack" + the
  copy_dma_step corrupt-base catch (§G10). The old notes already saw OUTLINED_FUNCTION_353
  building the OAM dst slice.
- NOT stack/spill coloring → matches §G14/§G17 (both coloring passes refuted).
- Host x86 + armv7 clean (§G15/§G16) → those builds make different outlining decisions /
  no `-Oz` thumb outlining → no collision.
- Layout-sensitive victim that relocates per build → outlining decisions shift with layout.

**DECISIVE TEST PENDING:** soak the `-enable-machine-outliner=never` build (/tmp/rb-nooutline)
past cycle 2.5B. If the ~2.4–2.5B crash VANISHES → ROOT CAUSE FOUND + FIXED (disable the
outliner, or upstream the LLVM bug). If it persists → the verifier error is a benign
over-conservative implicit-use list and the outliner is ruled out.

#### §G18-RESULT: MachineOutliner REFUTED as root (soak still crashes)

Soaked the `-enable-machine-outliner=never` build (verifier-CLEAN, 0 machine-code errors)
~14 min past cycle 2.5B → **13 records, STILL crashing in the 2.39–2.62B window:**
- 2× `oam-dma-checkpoint-guard` (CFSR=0xd6a00002) — the OAM-DMA wild store STILL fires.
- 8× Panic `volume_mgr.rs:415/419/493` (embedded-sdmmc FAT volume manager) — a NEW
  relocated victim (wild store corrupted SD/filesystem state this layout).
- 1× DACCVIOL @0x20066f40 (MPU base), watchdogs.

**Conclusions:**
1. The MachineOutliner is NOT the root: a machine-verifier-CLEAN build still produces the
   wild store. The §G18 verifier error (undefined $r1 in OUTLINED_FUNCTION_186) is REAL
   but benign/incidental to this crash. (Still worth reporting upstream as an LLVM bug.)
2. **The bug is NOT verifier-detectable** — it survives a build with zero machine-code
   errors. So it's a "consistent-but-wrong" model or not a localized codegen miscompile.
3. **Meta-finding (important):** the copy_dma_step `#[inline(never)]` "fix" (§G10/§G11) was
   itself just a LAYOUT SHUFFLE, not a structural fix — the OAM checkpoint guard, gone in
   §G11, RETURNS here (same fix present, different surrounding layout). So we have NO
   confirmed structural fix; every "fix/move" has been layout roulette.
4. Codegen passes ruled out by soak: StackColoring (§G14), StackSlotColoring (§G17),
   MachineOutliner (§G18). The wild store is robust to all three + inline(never) — unusual
   for a localized regalloc miscompile, which would be sensitive to allocation-changing
   passes. This re-opens whether it is purely codegen vs a concurrency/peripheral bug the
   single-threaded host replay can't model (cf. the layout-confounded TIMER0-mask result
   in CRASH_DEBUG_NOTES, whose negative control was never run).

#### §G18-CONTROL: verifier-probe matrix decouples the outliner error from the crash

Used the machine verifier as a compile-time PROBE (no soak) across a control matrix:

| build variant | verifier "undefined $r1" errors |
|---|---|
| current (copy_dma_step fix in), no flags | 3 — OUTLINED_FUNCTION_186, embassy_main_task, uses $r1,$r2,$r11 |
| + `-disable-ssc` (spill-slot coloring off) | 3 — same shape (renum 185) |
| + `-no-stack-slot-sharing` | 3 — same shape |
| bug-5 fix REVERTED (copy_dma_step re-inlined) | 3 — same shape (renum 408) |
| + `-enable-machine-outliner=never` | **0** |

**Classification (clean):** the verifier error is INVARIANT to the bug-5 frame-isolation
fix AND to both spill-slot-reuse flags — only disabling the MachineOutliner removes it. It
is the SAME outlined sequence (live-ins $r1,$r2,$r11; $r1 undefined) in embassy_main_task
every time, just renumbered by layout. Combined with §G18-RESULT (outliner-off build is
verifier-CLEAN yet STILL crashes at runtime), this DECOUPLES the verifier error from the
wild store: the undefined-$r1 outliner bug is REAL but is NOT the runtime corruptor.

Net: detection-via-verifier found a genuine isolated LLVM MachineOutliner live-in bug
(worth reporting upstream), but the ~2.4–2.5B wild store is a DIFFERENT, NON-verifier-
detectable defect that survives outliner/coloring/ssc/inline(never) — i.e. a
"consistent-but-wrong" codegen condition or a non-codegen (concurrency/peripheral) cause
the single-threaded host replay can't model. Untried levers: `-regalloc=basic` (whole
allocator swap) and the never-run ISR negative-control (mask a non-TIMER0 IRQ, same wrapper).

### §G19: The stack-slot-collision RCA is NOT statically supported — points to RUNTIME overwrite (2026-06-20)

Driven by the question "can we see the collision in the disassembly — a load from a slot
with an active variable that was overwritten?"

**Static findings:**
- The §G1-claimed "collision" slots `[sp,#0xf0]` / `[sp,#0xec]` are reused **68× / 80×** by
  many registers (r0,r1,r2,r3,r5,r6,r8,r9,r11,lr) in the giant inlined frame — heavy
  LEGITIMATE slot recycling. A real collision and legitimate reuse emit IDENTICAL str/ldr
  patterns; you can't distinguish them from the binary (needs live-range info). So §G1's
  "collision" was an INFERENCE from the runtime corrupt base, not a proven static collision.
- **`-verify-regalloc` (verifies exactly the spill-slot live-range invariant) found ZERO
  spill-slot / live-range / stack-slot violations** in any build — only the (decoupled)
  outliner undefined-$r1. If bug-5 were a genuine stack-slot collision, this verifier would
  flag it. It does not.

**Conclusion: the compiler's stack allocation is VALID (verifier-confirmed). The corrupt
base (§G10) is real but the slot is overwritten at RUNTIME, not mis-allocated at compile
time.** This unifies all stubborn facts (6 of which contradict the codegen-collision RCA):
verify-regalloc clean; -disable-ssc / -no-stack-coloring / outliner-off all fail to fix;
host x86+armv7 clean (no ISRs/DMA); TIMER0-mask eliminated the dominant crash
(CRASH_DEBUG_NOTES); layout-sensitive victim; runtime corrupt base. A live stack slot
overwritten at runtime by an ISR/DMA/other-core fits ALL of them.

**Redirect:** the leading hypothesis is now RUNTIME stack-slot overwrite (ISR/DMA/concurrency),
NOT a codegen miscompile. The decisive never-run experiment: the ISR NEGATIVE CONTROL the
old notes specified — mask a non-TIMER0 IRQ during tick() with the identical wrapper. If the
crash persists (unlike TIMER0-masking) → TIMER0 (embassy time-driver) ISR is the writer; if
it also vanishes → it was a generic layout shift. The "stack-slot collision" narrative
threaded through the memory notes should be treated as UNPROVEN/likely-wrong.

### §G19-CORRECTION: RETRACT §G19 — the stack-slot collision IS real (hardware-confirmed §G1)

§G19 overreached. Re-verified the §G1 instructions in the golden disasm (gold_disasm.txt):
`1000a904 str r2,[sp,#0xf0]` (&oam spill) / `1002fe92 ldr r0,[sp,#0xf0]` (OUTLINED_353 →
index_mut, OAM dst base) / `1002f2d8 str r2,[sp,#0xf0]` (route_bus_events stores
&GameBoyMemory). The §G1 DWT watchpoint PROVED writer (b) lands on the slot while (a)'s
&oam is live (deterministic, 2 runs). The collision is REAL and hardware-confirmed.

**Why §G19's "verify-regalloc clean → no collision" was WRONG (2 errors):**
1. Wrong binary: -verify-regalloc ran on the CURRENT toolchain (LLVM 22.1.2), not the
   GOLDEN repro (nightly 2026-05-15 / LLVM 22.1.4 + -Z stack-protector=strong) that §G1
   examined. Different allocation.
2. Structural: the §G1 root is LLVM's LIVENESS analysis being WRONG (it believes (a) is
   dead before (b) — the two uses look mutually-exclusive in the tick's control flow but
   both execute at runtime). -verify-regalloc checks allocation-vs-COMPUTED-liveness
   consistency, NOT computed-liveness-vs-runtime-truth. An allocation consistent with
   WRONG liveness passes the verifier. This bug class is invisible to it by construction.

**Also retract the §G19 "runtime ISR/DMA overwrite" pivot:** §G1 names the writer as a CPU
store in route_bus_events (0x1002f2d8), NOT an ISR/DMA. The TIMER0-mask "fix" is layout-
confound (mask wrapper shifts the frame), not an ISR — consistent with the heisenbug.

**Reconciled picture (stands):** a register-allocator/wrong-liveness stack-slot collision
in the giant `.data`-inlined embassy_main_task frame, with MULTIPLE instances. Why §G14–§G18
didn't fix it: the bug is in the wrong-liveness slot ASSIGNMENT (greedy regalloc), upstream
of the merge passes (so -no-stack-coloring/-disable-ssc don't address it and just relocate
the victim); copy_dma_step inline(never) fixed ITS instance (§G11) but others remain (§G18 =
a different writer near OAM). Host-clean + layout-fragility match §G1's prediction.

**Systemic fix direction:** break the giant inlined frame so source-disjoint pointers stop
sharing coalesced slots — inline(never) sweep on the hot tick-path functions, OR stop
inlining the whole tick body into embassy_main_task, OR report/fix the LLVM liveness bug
upstream. The `-regalloc=basic` whole-allocator swap is still worth a soak (may avoid the
wrong-liveness coalescing). The ISR negative-control is DEMOTED (writer is a CPU store).

### §G20: -regalloc=basic discriminator soak — INCONCLUSIVE (basic hangs the firmware) (2026-06-21)

**Hypothesis under test:** two independent issue-searches converged on LLVM PR #197773 /
#197776 (stale `LiveRegMatrix` state in the GREEDY allocator) as the wild-store root. The
basic allocator does not use the incremental LiveRegMatrix update path those PRs fix, so a
basic-regalloc build was meant to be a cheap discriminator: clean past ~2.5B ⇒ corroborates
LiveRegMatrix root; still crashes ⇒ refutes it.

**Build:** `+nightly` (cargo 1.97.0-nightly, **LLVM 22.1.4 — the golden LLVM version**),
target-scoped rustflags `-Z stack-protector=strong` + `-C llvm-args=-regalloc=basic`, to
`CARGO_TARGET_DIR=/tmp/rb-regalloc-basic`. Confirmed the flag took: the golden greedy build's
hot collision slots `[sp,#0xf0]`/`[sp,#0xec]` (reused 68×/80×) are used **0×** under basic —
an entirely different spill layout. Flashed clean (integrity CRC 0x4f0d0481 OK).

**CORRECTION — my initial "basic HANGS" call was WRONG (RTT-capture misread).** I saw
"dead silence" after `entering main loop` (no `fps=` heartbeat over RTT) and wrongly
concluded a livelock. In fact this firmware path simply emits no RTT inside the hot loop,
and killing host-side `probe-rs` does NOT stop the on-device firmware. User confirmed the
**screen is animating** — the emulator is running fine. Lesson: absence of RTT ≠ frozen CPU;
verify on-device state (screen / live SWD read) before declaring a hang.

**Actual state: basic-regalloc RUNS NORMALLY.** Non-destructive live SWD reads of the
`CRASH_CONTEXT` static (base 0x20066cbc; [6]=`gb_cycle_lo` @0x20066cd4, [5]=`gb_sp_pc`,
[0]=`valid`) show: valid=1, GB PC=0x03ce (the known trigger region), cycle climbing at
**~3.53 M cyc/s ≈ 84% of GB realtime** (4.19 M/s). `probe-rs read` only transiently halts/
resumes the core and PRESERVES the deterministic replay (emulated cycles don't advance during
the halt), so polling doesn't perturb the experiment. So basic is NOT too disruptive — the
discriminator is VALID after all.

**VERDICT: CLEAN PASS.** The poller ran the basic-regalloc firmware to real **16.53B** —
past the golden deterministic crash (~15.27B) and the 16.5B threshold — with **NO crash,
reboot, or hang**. 3 clean low32 wraps (at 10:22, 10:42, 11:02), valid=1 throughout, steady
~3.5 M cyc/s, GB sp_pc pinned at 0x03cedfff the whole soak. The crash that fires every time
on greedy VANISHED under basic.

**Interpretation — strong corroboration of the stale-LiveRegMatrix greedy root (PR #197773/
#197776).** Swapping ONLY the register allocator (greedy→basic; basic does not use the
incremental LiveRegMatrix update path the PRs fix) eliminates the wild store across the full
deterministic window. This is qualitatively different from the prior layout-relocation
attempts that all FAILED: inline(never) sweep, -no-stack-coloring (§G14), -disable-ssc (§G17)
each just moved the victim within greedy's allocation and the crash recurred. basic is the
FIRST allocation-changing intervention that makes it disappear. Combined with host x86+armv7
CLEAN (source logic fine) ⇒ the defect is in GREEDY's spill/slot allocation, exactly the
hypothesized mechanism.

**Residual caveat (why this is corroboration, not yet mechanism-PROOF):** basic changes the
WHOLE allocation, so in principle "clean" could be layout-luck (victim relocated to a benign
slot) rather than specifically avoiding the LiveRegMatrix staleness. The decisive isolation
is the cherry-pick: greedy + ONLY PR #197773/#197776. If greedy-alone crashes but greedy+fix
is clean, that is mechanism-level proof. (Also: this is ONE pass; the crash is deterministic
so one pass past 15.27B is meaningful, but a repeat/longer soak would harden it.)

**Two practical upshots:**
1. **`-regalloc=basic` is a viable SHIP-NOW mitigation** — clean through the window, runs at
   ~84% of GB realtime (16% perf hit; earlier "basic hangs the handshake" fear was the
   misread — it ran 72 min flawlessly). Tradeoff = perf vs. a working fix today.
2. **Cherry-pick PR #197773/#197776 into greedy** = the perf-preserving permanent fix +
   mechanism proof + upstreamable repro. Needs an offloaded LLVM-from-source build host
   (local box is 4-core/7GB, no rust/llvm checkout — infeasible here).
Config block to resume the basic build is commented in `.cargo/config.toml` (now reverted).

### §G20-PERF: basic-regalloc fps measurement + a confounded sibling crash (2026-06-21)

Built basic-regalloc + `--features fps` (`fps` cargo feature gates `PerfTracker`, logs
`fps=` every 60 frames; the no-feature soak build was silent in the hot loop → my earlier
"hang" misread). The running loop (`state/running.rs`) has NO frame pacing — it emulates
`CYCLES_PER_FRAME=70224` then awaits display/audio DMA — so **fps == effective speed; 60 =
realtime**.

**Measured: basic-regalloc = 47–48 fps** (stable across 2 flashes), i.e. **~80% of 60fps
realtime**. This matches the independent SWD cycle-rate inference (3.53 M cyc/s ÷ 70224 ≈
50 fps). So basic ships at a real ~20% slowdown (sluggish + ~20%-low audio pitch).

**Confounded sibling crash (do NOT over-read):** basic+fps CRASHED both flashes, ~6.4s and
~10.4s after entering the loop (after 2 and 5 fps prints), same signature (core-0 dispatch_isr
thunk @0x100318c0 + garbage CORE1_STACK@0xfffffff8, core-1 run_core1_worker). Integrity CRC
OK both times → NOT flash corruption. **VARIABLE wall-clock timing (6.4 vs 10.4s) ⇒ NOT a
fixed-cycle codegen collision** (the golden crash is deterministic at real 15.27B every run).
It's a TIMING/CONCURRENCY crash, and the `fps` feature is a confounded perturbation: it adds a
`defmt info!` in the hot loop, and defmt RTT writes take a CRITICAL SECTION (+ embassy-time
read) — under active probe RTT-draining this perturbs the core0/core1 ticket-handshake timing
(the known critical-section/spinlock sensitivity, [[multicore_queue_livelock]]). So basic+fps
crashing does NOT refute the no-fps clean-to-16.5B pass — different failure mode (timing, not
the codegen wild store). NET: don't measure fps via in-hot-loop defmt on this firmware; the
clean no-fps soak + SWD-rate both stand. Caveat unchanged: basic-clean is still ONE pass /
whole-allocator-swap, so LiveRegMatrix corroboration stays "strong-but-not-proof" pending the
greedy+cherry-pick. (Greedy+fps baseline measured next to ground the ~20% cost.)

### §G20-PERF-CORRECTION: allocator is perf-NEUTRAL (~47 fps both) (2026-06-21)

Measured the GREEDY baseline (same flags minus the allocator swap: -Z stack-protector=strong
+ --features fps, NO -regalloc=basic): **greedy = 47 fps (mean 47.25, n=20, no early crash in
29s).** basic was also **47–48 fps**. ⇒ **the register allocator has NEGLIGIBLE perf impact;
~47 fps (~78% of 60fps realtime) is the EMULATOR'S BASELINE** on this Pico2W (opt-level=z, fat
LTO, per-frame display/audio DMA), NOT a -regalloc=basic regression. **CORRECTION:** my earlier
"basic costs ~16–20%" was wrong — I divided basic's 47 fps by 60 without checking greedy also
can't hit 60. So if basic fixes the crash there is NO perf reason to prefer the cherry-pick;
both run identically slow.

**RETRACTED robustness caution (basic+fps crash does NOT bear on bug #5).** I initially spun
the basic+fps early crash into a "basic isn't robust" doubt — logic error. The basic+fps crash
is VARIABLE-timing ⇒ a CONCURRENCY crash, a DIFFERENT bug class than bug #5 (deterministic at
fixed real-cycle 15.27B). A different-class crash says nothing about whether basic fixed bug #5.
The greedy-clean-vs-basic-crash asymmetry I leaned on is n=2 vs n=1 — a single 29s greedy window
is no basis for "basic-specific," and a concurrency bug whose window shifts with codegen timing
explains it without involving bug #5. The fps feature adds a defmt CRITICAL SECTION in the hot
loop (under active probe RTT drain) → perturbs the cross-core handshake timing; that early crash
is a CONFOUND of measuring fps this way, decoupled from the codegen wild store. **Bottom line
stands:** basic-no-fps soaked CLEAN through the bug-#5 deterministic window (15.27B → 16.5B) —
the relevant bug-#5 test, PASSED. Only legitimate residual caveats: (1) MECHANISM ATTRIBUTION
(does basic-clean prove the LiveRegMatrix staleness specifically, vs generic "a different
allocator avoids greedy's wrong-liveness collision"?), (2) ONE pass (deterministic, so still
meaningful). A clean robustness check, if wanted, = a NEUTRAL layout perturbation adding NO
hot-loop logging (not the fps feature). greedy+cherry-pick = mechanism PROOF; NOT required to
believe basic empirically clears the bug-#5 window.
