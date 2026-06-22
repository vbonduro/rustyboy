# Pico 2W — Crash/Freeze Debug Session Notes

Working notes from a long debugging session on the RP2350 firmware. Captures
root causes, fixes applied, diagnostic techniques, and the one **still-open**
bug. Read this before continuing the investigation.

---

## TL;DR

| # | Symptom | Status | Root cause |
|---|---------|--------|-----------|
| 1 | `cargo run` flashes unreliably (page-write fails, "core is running", wild-PC boot crashes) | **FIXED** | Firmware watchdog reset the chip mid-flash |
| 2 | Game freezes ~5 s then reboots, after a few minutes | **FIXED** | Cross-core command-queue **livelock** (spinlock starvation) |
| 3 | Wild-PC boot crashes that survive #1's fix | **MITIGATED** | Intermittent `.text` flash corruption — now **detected** at boot |
| 4 | APU panic (`apu.rs` wave-RAM index) | **FIXED (guarded)** | Out-of-bounds `wave_ram` index |
| 5 | Crash on **gameplay** after seconds–minutes (intermittent): wild PC / smashed pointers / DMA-OOB panic / watchdog hang | **OPEN — residual PPU/transport pointer smash** | Oversized `heapless 0.8` `MpMcQueue` capacities (`512`/`2048`) were a real bug and are now mitigated, but a later re-check still captured `report_transport_smash` panics and core-1 PPU pointer-guard records. Current trail: `GameBoyWorker.ppu` Box pointer / core-0 transport fields are still being clobbered. |

Items 1–4 are fixed; **#5 is still open after the queue mitigation**. Older
notes below describe the route-bus `Vec<BusEvent>` amplifier because it was the
best lead at the time, but that structure has since been removed. Treat the
2026-06-05 block below as the current state of the hunt.

**Confirmed NOT the cause of #5** (each cost a flash+repro cycle to rule out):
SP-limit stack overflow (MSPLIM pre-armed, never STKOF), heap overrun
(`heap-guard` redzone never fired), wild ROM pointer (`rom_window` is checked),
cross-core atomic RMW (all serialized), core-1 emulator OOB (bounds-safe), the
audio packing path (clamped; DMA only reads RAM). The smash is *in-bounds of the
long-lived async task frame/object region* and *not a heap redzone overrun*,
which is why both hardware stack-limit and heap guards stay silent.

**Current firmware catch (2026-06-04):** `route_bus_events` validates the
`GameBoyMemory.events` `VecDeque<BusEvent>` header immediately before
`drain_into`. If cap/ptr/head/len is impossible, the Pico hook records a
synthetic HardFault record with `arm_cfsr=0xD9170001`, `arm_pc` = return address
from the guard callsite, `arm_lr` = bad word index (0=cap, 1=ptr, 2=head,
3=len), `arm_hfsr` = cap, `arm_fault_addr` = ptr, diagnostic tail
(`panic_loc[0..4]`, `[4..8]`) = head/len, and POWMAN scratch[5] low16 =
bad_index. DWT arming is disabled for this guard build so legitimate
`VecDeque::push_back` header writes do not halt `cargo run`. The fallback
`report_transport_smash` scan still classifies duplicate payload copies as
`transport_ptr_triplet`, `bus_event_buf_header`, `core0_stack`, `core1_stack`, or
`sram_static_or_allocator`.

**Latest evidence (2026-06-05):**
- The persistent route-bus `Vec`/`VecDeque` amplifier was structurally removed:
  `GameBoyMemory.events` is now a fixed inline `BusEventQueue`, and
  `GameBoy::route_bus_events()` drains into a fixed stack array. Host tests
  passed (`cargo test-host`: 191/191), and `cargo check --release` passed.
  The crash still reproduced, so route-bus was an amplifier/victim, not the
  root corruptor.
- Nightly `-Z stack-protector=strong` builds caught stack canary smashes in
  `Sm83::inc8` and `Sm83::dec8`, specifically the `INC/DEC (HL)` paths with
  `HL=0xffff` (IE register). In both cases the epilogue failed after
  `bus_write(memory, 0xffff, v)`, making that path the current narrowest
  canary-smash victim window.
- A first DWT canary watch did **not** catch the writer because it was armed as
  a 4-byte access watch. The suspected `bus_write`/bus-event path can write with
  halfword/byte stores, which can alter the canary word without matching the
  word-sized DWT comparator.
- Replaced that DWT experiment with explicit canary checkpoints in
  `Sm83::inc8`: site `0x1c800001` after `read_fast`, `0x1c800002` after
  `inc_u8`, `0x1c800003` after `bus_write`. The Pico hook records
  `arm_cfsr=0xC0110004`, `arm_hfsr=canary_addr`,
  `arm_fault_addr=canary_after`, tail `[canary_before, memory_ptr]`.
- The `0x8b0c3dd8` protected guard image did not hit the canary checkpoint in
  the attached run. Instead it recorded a fresh core-1 `Panic` at
  `library/alloc/src/alloc.rs:573` with plenty of core-1 stack headroom
  (7672 B). That line is Rust's default no-std allocation-failure handler, so
  the observed failure is OOM/allocation failure, not stack overflow.
- Added `heap-guard` allocator-boundary recording: null allocations and
  out-of-heap allocations now record `arm_cfsr=0xC0110005`, `arm_hfsr=size`,
  `arm_fault_addr=align`, tail `[ptr, heap_start]`, and POWMAN scratch[5]
  low16 = site (`0xA1100001` null alloc, `0xA1100002` out-of-heap). The
  protected heap-guard image is built at
  `/tmp/rustyboy-stack-canary-heapguard/thumbv8m.main-none-eabihf/release/rustyboy-pico2w`.
  Hardware flashing was blocked by the approval budget after the build.
- Strong root-cause candidate found: `COMMAND_QUEUE_CAPACITY=512` and
  `AUDIO_QUEUE_CAPACITY=2048` used `heapless 0.8.0` `MpMcQueue` with
  `mpmc_large`. In that crate, `mpmc_large` widens the position atomics to
  `usize`, but `enqueue`/`dequeue` still compute readiness with
  `(seq as i8).wrapping_sub(...)`. These capacities exceed what the `i8`
  sequence-difference logic can distinguish. That explains fresh
  `WatchdogTimeout`/lost-ticket behavior and can also read uninitialized
  `Core1Command` cells (UB for enums), which is a plausible producer for the
  later wild writes/smashed long-lived task fields.
- Experiment: temporarily capped both queues to 64 and flashed release image
  CRC `0x821458d1`. After a 6-minute standalone run, crash decode reported
  `valid=false` and no records. That is the first clean standalone soak after
  the issue was made to repro repeatedly.
- Current mitigation: keep `COMMAND_QUEUE_CAPACITY=64` and restore
  `AUDIO_QUEUE_CAPACITY=2048` using a custom serialized ring (`AudioQueue`)
  instead of `MpMcQueue`. Release image CRC `0x017e9080` also ran standalone for
  6 minutes, then `crash_decoder.py --probe --json` reported `valid=false` and
  `crashes=[]`. `cargo check --release` and `cargo test-host` pass.
- Re-check after additional unattended runtime found fresh records on git
  `22f3352d`: repeated core-0 `report_transport_smash` panics
  (`multicor:1448`), repeated core-1 synthetic `arm_cfsr=0xC0110001` pointer
  guards while rendering sprites, plus a core-0 precise bus fault through a bad
  atomic address. The queue bug is therefore **not** the whole corruptor.
- Instrumentation flaw found: the DWT raw watch for `GameBoyWorker.ppu` was set
  once at worker init, but every core-0 tick replaced it with a watch on
  `GameBoy.memory`. `src/dwt_watch.rs` now supports multiple raw slots; slot 0
  watches the worker PPU Box field and slot 1 watches the main memory Box field.
  Multi-watch image CRC `0x669461cb` built and flashed. Attached run halted under
  probe-rs before a record committed; standalone 90 s and 6 min runs produced an
  invalid crash sector (`crashes=[]`), so the new capture build either perturbs
  the heisenbug or has not yet hit the writer.
- User-visible crash report on the multi-watch image rebooted to the splash
  screen and loaded the save state, but did **not** commit a crash record
  (`valid=false`, `crashes=[]`). This is a real reset/reboot that bypassed both
  the scratch-register crash sentinel and `WATCHDOG.reason.timer()` capture path;
  otherwise boot-time `check_and_commit()` / `check_watchdog_reset()` should have
  made the sector valid again. Live RAM reads after the reboot showed sane
  post-boot pointers (`GameBoy.memory=0x20040300`, worker PPU state
  `0x200455c0`, PPU Box field `0x20003a60`, memory Box field `0x2006547c`) and
  `CRASH_CONTEXT` advancing at the old trigger state (`bank=12`, `GB PC=0x03ce`,
  `HL=0xffaa`), but those samples are post-reboot evidence only.
- Fresh user report: "froze for 5 seconds then rebooted" on image CRC
  `0x32b790ef` committed new records in slots 7-8. Slot 7 is another core-1
  synthetic `CFSR_CORE1_POINTER_GUARD` at `multicore.rs:1736`
  (`assert_worker_ppu_pointer`), with the worker reference stack slot smashed to
  `0x00004554`, `worker.ppu` read through that bogus base as `0xf7ff2036`, and
  expected PPU state still stable at `0x200455f8`. Slot 8 is the watchdog timer
  reset. This repeats the earlier slot-5 pattern (`worker=0x000044a4`,
  `ppu=0x92032738`, `want_ppu=0x200455f8`) and proves the active victim is the
  core-1 `run_core1_worker` stack slot that holds the `worker` reference, not
  the DMA state.
- Disassembly of the current image explains why the line-1735
  `assert_core1_pointers` check did not fire first: LLVM folded the
  shared/worker identity check into an entry-time boolean saved at `[sp,#0xa4]`.
  The later line-1736 PPU check reloads the mutable worker reference from
  `[sp,#0xa8]`, then reads `worker.ppu` at offset `0x94`; when that stack word
  becomes `0x4554`, the PPU pointer read is wild and the PPU guard records it.
  The next diagnostic image should therefore stop watching DMA and DWT-watch the
  worker-reference stack slot plus the static `GameBoyWorker.ppu` Box field.

**Attempted follow-up (2026-06-14):**
- Implemented the narrow MPU bracketing design for the cartridge vtable block:
  `core::memory::GameBoyMemory::refresh_rom_windows()` now routes through a
  tiny optional callback seam (`install_rom_window_cache_refresh_bracket_for_diagnostics`)
  so the Pico layer can unlock only MPU region 2 around the legitimate
  `rom_*` cache-field writes, then immediately relock it. The callback checks
  the vtable word immediately before unlock and immediately after relock and
  fires the existing synthetic `rustyboy_cartridge_vtable_guard` if the word
  changed inside the writable window. Follow-up review made the whole Core 0
  writable interval locally interrupt-atomic, including boot save restoration.
  Core 1 polls the published runtime address and permanently marks the same
  block privileged-read-only in MPU region 1; only Core 0's tightly bracketed
  cache refresh can make its own region writable.
- Local validation only: `cargo check --release` passed and `cargo test-host`
  passed (191/191) after the bracketing change.
- Hardware validation did **not** run because reflashing the board became
  blocked before the new image could be programmed. Every `probe-rs download`
  / `rb-flash` attempt failed inside the target flash algorithm's `init`
  routine with the same report: `init failed with code 288`. A captured
  `probe-rs` debug log shows the flash algorithm call returning `r0 = 0x120`
  (`Routine returned 120.`) during `Call to flash algorithm init`.
- Recovery attempts that did **not** clear the blocker: probe USB reset, target
  watchdog clear (`WATCHDOG.CTRL = 0`), SWD reset, lower SWD speed, explicit
  `MPU_CTRL = 0`, and clearing all four DWT comparator function registers
  before the flash attempt. `probe-rs info --chip RP235x` still attached and
  read the CoreSight ROM successfully, so the failure was specific to flash
  programming, not SWD attach in general.
- No positive-control run, no normal-boot verification, and no standalone bug
  #5 trials were completed on 2026-06-14. No new crash-sector captures were
  produced.

---

## Diagnostic techniques that worked (use these, not probe-rs unwind)

- **`tools/crash_decoder.py --probe --elf <ELF>`** reads the firmware's own
  crash records from flash (real PC, CFSR/HFSR, GB state, stack flag). probe-rs's
  post-crash unwind reports a **useless wild `PC=0x88`** on this board — *ignore it*.
- **HardFaults only commit a record when running STANDALONE.** Under `cargo run`
  (= `probe-rs run`), probe-rs's vector-catch freezes the core on a HardFault
  before the firmware handler can save it. To capture a HardFault: flash, detach
  (kill probe-rs / power-cycle), reproduce, then attach + decode.
- **But a Rust `panic!` (our tripwires) prints its `defmt` message over RTT
  *before* it resets — so `cargo run` DOES surface tripwire output** (e.g.
  `core0 transport ptrs smashed before <label>`), even though no record commits.
  That is how #5 was finally characterised. Use `cargo run` + a long idle window.
- **`defmt::panic!` records `defmt`'s `lib.rs:385` as the panic_loc** (it calls a
  bare `core::panic!()` in `__defmt_default_panic`), hiding the call site. Use
  core `panic!` (+ `#[track_caller]`) in tripwires so the record's panic_loc is
  the real site.
- **#5 is stack-layout-sensitive (a heisenbug):** adding per-tick instrumentation
  shifts the frame and the overrun stops landing on the transport pointers → the
  crash vanishes. Prefer method-entry checks over dense per-step checkpoints.
- The crash sector lives at flash `0x103FF000`. `--mark-read` zeroes its header;
  if captures stop working, blank it: download 4 KiB of `0xFF` to `0x103FF000`.
- **Live hang capture:** `probe-rs gdb --chip RP235x` + `gdb-multiarch -ex ...`
  to halt both cores and `bt`. **CAVEAT:** halting clears the CPU's exclusive
  monitor, so any atomic **read-modify-write** loop (`swap`/`fetch_*`) appears
  permanently stuck under gdb even when it's healthy. Plain-load spins
  (`wait_for_ticket`) are real; RMW "hangs" under gdb are suspect.
- **SIO spinlock state:** `SPINLOCK_ST` at `0xd000005c`, bit n = spinlock n held.

---

## #1 — Flash reliability: the watchdog was resetting the chip mid-flash

**Root cause.** The firmware arms a 10 s hardware watchdog at boot
(`main.rs`, `watchdog.start`). A probe-rs flash takes **30–40 s**.
`pause_on_debug(true)` only pauses the watchdog while the core is *halted*, not
during the seconds it spends running probe-rs's flash loader — so the watchdog
fires mid-flash, resets the chip, and corrupts the half-written image. This is
the true cause of the "flash corruption" lore; the `--disable-double-buffering
--verify` flags only slowed flashing (more watchdog exposure).

**Fix.** `xtask/src/bin/rb-flash.rs` clears the live watchdog before programming:
```
probe-rs write --chip RP235x b32 0x400d8000 0x0   # WATCHDOG.CTRL.ENABLE bit 30 = 0
```
Runs on whatever firmware is currently on the chip, so it works on **every**
flash including the first. Result: 3/3 clean flashes vs ~50% failures before.
The firmware watchdog stays default-on (re-armed each boot).

---

## #2 — The freeze: cross-core command-queue livelock (FIXED via "Option A")

**What it was.** `COMMAND_QUEUE` (core 0 → core 1) is `heapless::MpMcQueue`
guarded by `critical_section` (SIO spinlock), because the lock-free CAS isn't
cross-core safe on RP2350 (per-core exclusive monitors, no global arbiter for
SRAM). When the queue filled, `enqueue_blocking` **busy-retried**, re-taking the
spinlock so fast it **starved core 1's (also-spinlock-guarded) dequeue** → queue
never drains → both cores livelock → watchdog reboots (~5 s freeze, the user's
symptom). Confirmed by live gdb: core 0 spinning in `MpMcQueue::enqueue`, core 1
spinning in `RpSpinlockCs::acquire`.

**The detour (SPSC) and why it failed.** I first migrated both queues to
`heapless::spsc::Queue` (load/store only, no spinlock). That removed the
livelock but **deadlocked the ticket handshake**: the cross-core protocol
silently relied on the spinlock's barriers to drain each core's write buffer.
Without them, core 1's `sync_complete.store(Release)` sat in its write buffer
while it slept (`wfe`), so core 0 spun forever in `wait_for_ticket` reading a
stale value. (Proven: core 0's `lda` read 1413 while SRAM held 1414, visible
only after halting core 1 — which flushes the buffer.) **Lesson: `Release`
orders but does not force completion; only `DSB` drains the write buffer.** A
full SPSC migration would need explicit barriers at every cross-core publish
point — too error-prone. **SPSC was reverted.**

**Fix that shipped — "Option A" (`multicore.rs`).**
1. Restored `MpMcQueue` + `critical_section` on the command queue (provides the
   barriers the handshake depends on).
2. Kept the **real livelock fix**: on a full queue, `enqueue_blocking` does
   `asm::wfe()` (sleep) instead of busy-retrying, so it stops hogging the
   spinlock; core 1 `asm::sev()`s after each dequeue to wake it.
3. Left the audio queue (`AUDIO_QUEUE`) as plain `MpMcQueue` — the ticket
   handshake serializes producer/consumer, so there's never concurrent access.

Result: ran ~294,000 commands (≈2.5 min) with no freeze; user confirmed the
freeze is gone.

---

## #3 — Intermittent `.text` flash corruption + the whole-image CRC guard

Even with #1 fixed, this board's marginal SWD link occasionally corrupts a
`.text` page during flashing, which `--verify` false-passes (XIP-cache). That
boots into a garbage instruction → wild HardFault, indistinguishable from a real
bug. The old `.data`-only CRC guard didn't cover `.text`.

**Fix — whole-image CRC guard.**
- `rb-flash` reconstructs the **exact** bytes probe-rs will flash and CRCs them,
  stamping the result into `IMAGE_CRC` (a `u32` in the `.end_block` section, at
  the very end of the image — `__end_block_addr`).
- Boot re-CRCs all of flash `[__start_block_addr, IMAGE_CRC)` and compares
  (`integrity::verify_image` in `main.rs`). Mismatch → clean semihosting exit, so
  `cargo run` fails with `IMAGE CORRUPT` and the user just reruns.

**Critical reconstruction detail (was a false-positive bug, now fixed):**
probe-rs flashes ELF **LOAD segments**, whose `p_filesz` includes inter-section
alignment padding as `0x00`. A **section**-based reconstruction misses that
padding (leaves `0xFF`) and the CRC falsely mismatches. `rb-flash` now
reconstructs from **program headers (segments)**, placing each `PT_LOAD` segment
at its `p_paddr`, gaps between segments = `0xFF`. Verified byte-exact (0 diffs)
against a real flash read-back, and the guard correctly *caught* a genuine
corrupt flash during the session (stamp `0xd28e4776` vs flash `0xd24d5282`).

CRC variant is CRC-32/ISO-HDLC (poly `0xEDB88320`, init `0xFFFFFFFF`, xorout),
identical in `rb-flash` and `crash/mod.rs::crc32`.

---

## #4 — APU wave-RAM panic (FIXED)

`core/src/cpu/peripheral/apu.rs::write_wave_ram` indexed
`wave_ram[(position/2) as usize]`. `position` is a 5-bit counter (0..=31) so the
index should always be 0..=15 into the 16-byte wave RAM — but it panicked,
meaning `position` was ≥ 32 (corrupted; see #5). Guarded with `& 0x0F` (matching
the existing read path), which is hardware-correct and stops the crash.
*Note:* this was almost certainly a **symptom of #5**, not an independent bug.

---

## #5 — OPEN: pointer corruption after a few minutes

The remaining crash. With a **CRC-verified-clean flash**, running standalone,
captured records (real, from `crash_decoder.py`):

```
Crash #1  HardFault  core 0
  ARM PC  0x68000000   CFSR 0x00000100 IBUSERR   (jumped to garbage → instr fetch fault)
  ARM LR  -> core::sync::atomic::atomic_add
  GB CPU  PC=0x03ce SP=0xdfff AF=0x0080 BC=0x0000 DE=0x5800 HL=0xffaa

Crash #2  HardFault  core 1
  ARM PC  -> atomic_store   CFSR 0x00008200 BFARVALID+PRECISERR   Fault@ 0x000152ac
  ARM LR  -> run_core1_worker (multicore.rs:1308 = publish_worker_state)
  GB CPU  PC=0x03ce SP=0xdfff AF=0x0080 BC=0x00ff DE=0x5800 HL=0xffaa
  core-1 stack headroom 7832 (shallow at crash time)

Crash #3  WatchdogTimeout
```

**Analysis.**
- Both crashes smash a **pointer**, not data: core 0's return address (wild PC
  `0x68000000`), core 1's `shared` base pointer (corrupted toward 0, so
  `0 + 0x152ac` faults — `0x152ac` ≈ the `sync_complete` field offset in the
  large `SharedWorkerState`). Pointer corruption = **stack/buffer-overflow
  class**, not an atomic data race.
- **Reproducible trigger:** both crashes captured the *identical* GB CPU state
  (`PC=0x03ce, AF=0x0080, HL=0xffaa, DE=0x5800`). The game hits one specific
  spot and the corruption happens. `HL=0xffaa` is HRAM — possibly an OAM-DMA /
  HRAM routine.
- **Pre-existing, not introduced this session:** the repo already ships
  `tools/crash-catch.gdb` hunting this exact "core 1 audio-enqueue HardFault /
  smashed `self`/`audio_tx` pointer," prime-suspecting core 1's 8 KiB stack
  (note a `PpuSnapshot` is 8480 bytes > the whole stack). My multicore work
  fixed the freeze that was *masking* this.

**Ruled out so far:**
- Not flash corruption (CRC verified clean).
- Not the command-queue livelock (Option A fixed that; this is a fault, not a hang).
- `WorkerOutput` (5 bytes), `publish_worker_state` locals, `Core1Command`/
  `PpuState` — all small/by-reference; not an obvious large stack local.
- `SPINLOCK_ST` = 0 (no leaked spinlock).
- **NOT a stack overflow on either core** — see "Stack overflow ruled out" below.

**Memory layout relevant to the theory.** Core 1's stack is `CORE1_STACK`
`0x20080000..0x20082000` (8 KiB, SRAM8/9). Core 0's stack is the **top of main
RAM**, ending at `0x20080000` — i.e. **immediately below** core 1's stack. So a
core-1 stack overflow (downward past `0x20080000`) would land in core 0's stack
region → the original (now-disproven) theory for core 0's smashed return address.

### Stack overflow ruled out (2026-06-03 — static audit + MSPLIM finding)

Two independent results retire the stack-overflow hypothesis:

1. **Static stack-size audit** (`RUSTFLAGS="-Z emit-stack-sizes"` nightly build,
   parse `.stack_sizes` vs `.symtab` — see the one-off script in the session).
   The core-1 worker path is **shallow**: `run_core1_worker` 248 B, every callee
   on the worker/PPU/APU path ≤ ~90 B (`sync_ppu_state` 88, `publish_worker_state`
   72, `copy_live_ppu_snapshot` 40, APU handlers 16–56). Worst-case chain ≈ 1–2 KiB
   of the 8 KiB stack. No kilobyte-scale `PpuSnapshot` copy on core 1 (all by-ref).
   - The **single largest frame in the whole firmware** is
     `state::wifi_menu::WifiPortalScreen::tick` at **25,440 B** (it inlines
     `start_portal().await`, holding the cyw43 `Control`/`Runner`, net stack and
     `scan_ssids` result live across awaits). It runs on **core 0**, which has a
     **~79 KiB** stack (`_stack_end`=`0x2006c2f4` … `_stack_start`=`0x20080000`;
     the 160 KiB heap is a *separate* `.bss` array). 25 KiB fits in 79 KiB, and
     the captured crashes happened during **gameplay**, not in the WiFi menu — so
     this frame is a **latent stack bomb worth fixing, but not crash #5**.

2. **MSPLIM was already armed on BOTH cores** (verified in the linked ELF
   disassembly — `msr msplim, r0` sites): core 0 by cortex-m-rt at reset, **core 1
   by `embassy_rp::multicore::spawn_core1::core1_startup`**. A stack-limit
   violation therefore already raises a **STKOF UsageFault** (CFSR bit 20,
   `0x0010_0000`) at the offending instruction. The captured #5 crashes show
   `CFSR=0x0000_0100` (IBUSERR) and `CFSR=0x0000_8200` (PRECISERR+BFARVALID) —
   **bus faults, not STKOF.** The guard that would have caught an overflow was
   present and did not trip with a stack signature. **#5 is not a stack overflow.**

**Instrumentation added this session** (diagnostic, low-risk):
- `MSPLIM` re-asserted explicitly on both cores (`main.rs` core 0,
  `multicore.rs::run_core1_worker` core 1) — belt-and-suspenders + greppable;
  the runtime already arms it, but now the invariant is explicit in our code.
- Core 1 stack now painted **full-region** (was a 256 B bottom canary) and the
  worker loop logs a throttled high-water mark under `--features stack-probe`
  (`stack_probe::region_high_water`). `high_water_core0()` added for core 0.
- `#3` trigger log: `update_crash_context` traces core-0 stack high-water + bank
  when GB `PC == 0x03ce` (the reproducible trigger), at `trace` level.

### Cross-core RMW / concurrency audit (2026-06-03) — mostly CLEARS the RMW theory

Walked every atomic RMW and shared-buffer write in `multicore.rs`:
- `published_frame_seq.fetch_add` — inside `critical_section` (`publish_frame`). OK.
- `pending_if_bits` — `fetch_or` (core 1) and `swap` (core 0) **both inside
  `critical_section`**. OK.
- `ppu_render_version.fetch_add` — **core-0-write-only**; core 1 only `load`s it.
  Single-writer RMW is safe (no competing cross-core exclusive). OK.
- `write_live_vram_range` / `write_live_oam_range` — offset+len are **clamped**
  (`len = data.len().min(buf.len().saturating_sub(start))`). No OOB. OK.
- `AUDIO_QUEUE` / `COMMAND_QUEUE` (plain lock-free `MpMcQueue`) — **ticket-
  serialized**: core 0 blocks in `wait_for_ticket` during core 1's drain, so the
  lock-free CAS is never actually contended cross-core. OK (as designed).

**Conclusion:** the genuinely-shared RMWs are all correctly serialized. This is
consistent with the fault signature — a *torn RMW corrupts a value, not a
pointer*, but #5 smashes the `shared` **base pointer** (→ ~0). So #5 is a
**memory-safety / wild-write** bug (stack-spill smash or a wild store inside the
worker / emulator-core path), not an atomic data race. The remaining unknown is
*which operand* gets smashed.

### Tripwire added to localize the smash

`multicore.rs::assert_core1_pointers(shared, worker)` — both are `&'static` to
fixed module statics (`SHARED_WORKER_STATE`, `CORE1_WORKER`), so any other value
is corruption. Called (a) once per command at the **loop top**, and (b) at
**`publish_worker_state` entry** (immediately before the faulting stores). It
`defmt::panic!`s with the bad vs. expected addresses, so the crash handler
records a clean **Panic** (with the corrupt pointer value + GB state) instead of
a wild atomic-store HardFault. Reading the two firings tells us:
- loop-top fires → corruption persisted across iterations / happened at sleep.
- entry fires but loop-top didn't → smashed **within** this command
  (`worker.send` / `sync_*`) ⇒ culprit is the **emulator-core path**.
- neither fires but it still HardFaults on the store ⇒ `shared`/`worker` are
  fine; the bad operand is computed transiently or lives *inside* the worker.
Always-on (two pointer compares); keep it until #5 is caught at least once.

### Core-1 emulator-path audit (2026-06-03) — path is memory-safe; not the silent smasher

Audited everything core 1 reaches via `GameBoyWorker::send` / `sync_*` /
`update_ppu_render_state` / `load_ppu_state`:
- **No `unsafe`, `get_unchecked`, or raw-pointer writes** anywhere in
  `PpuPeripheral` or `ApuPeripheral` (only two benign `bytemuck::Zeroable` impls).
- PPU render (bg/window/sprites) is **fully bounds-checked**; the per-scanline
  sprite buffer is a fixed `[_;10]` correctly capped (`if count >= 10 { break }`),
  tile/oam/vram indices stay in range (`tile_index*16+… ≤ 4110 < 0x2000`).
- APU is bounds-checked; wave-RAM index masked `& 0x0F`, `position` masked `& 0x1F`.
- `write_vram_range`/`write_oam_range`/`write_register` are clamped/guarded.
⇒ **Any OOB on core 1 PANICS (clean record); it cannot silently smash a pointer.**
This *eliminates the emulator compute path* as the source of the silent
`shared`-pointer smash.

Sharpens #4's clue: `apu.position` is `& 0x1F`-masked in the tick path, yet #4
saw it ≥ 32 — and `ApuPeripheral` lives **inline in the `CORE1_WORKER` static**.
So a wild write *landed on that static from outside the APU*; the APU didn't
corrupt itself.

**Allocator cross-core safety — CONFIRMED OK.** Core 1 allocates (`ApuPeripheral::
sample_buffer: Vec<i16>`, audio drain). `embedded-alloc` serializes alloc/free
with `critical_section::with`, and the linked impl is **embassy-rp's dual-core
`RpSpinlockCs`** (SIO **Spinlock 31** + interrupt-disable + recursive-owner
tracking — verified in `embassy-rp/src/critical_section_impl.rs`). So concurrent
heap access from both cores is properly excluded; **not** a free-list race.

**Net:** stack overflow, the core-1 emulator OOB, cross-core RMW on the shared
atomics, bounds-clamped buffer writes, and allocator races are **all ruled out**.
No silent corruptor remains in the reviewed core-1 + cross-core software. Prime
suspects now: (a) the **core-0 emulator path** (CPU / MBC / memory-map /
save-state) — crash #1 was on core 0 and that code is **not yet audited**; (b) a
specific untested path caught only by the live tripwire; (c) silicon.

### #5 REPRODUCED + CAPTURED (2026-06-03) — 5 real records, core-0 pointer corruption

Flashed the instrumented build, played to repro, decoded the firmware's own
records (`crash_decoder.py --probe`). **All 5 share the identical GB state**:
`PC=0x03ce  HL=0xffaa  SP=0xdfff`, **LCD OFF (LCDC=0x00)**, ROM id `21f712e2`
bank 12, after **billions** of cycles. Three distinct ARM fault sites:

| rec | ARM site | CFSR | detail |
|-----|----------|------|--------|
| #1,#4 | `gameboy::write_apu_register` → returns to **PC=0x68000000** | IBUSERR (FORCED) | smashed return addr / fn-ptr |
| #5 | `gameboy::route_bus_events` (gameboy.rs:468) | **UNALIGNED** | R0=`0x2007febc`, R1=`0x0004` |
| #2 | core-1 `apu::write_wave_ram` → `unchecked_add` | PRECISERR, Fault@`0x152ac` | the `shared` base-ptr smash |
| #3 | — | WatchdogTimeout | (livelock/hang variant) |

**Unified reading:** all three faults are **downstream of a smashed pointer on
core 0** — a corrupted return address (`0x68000000`), a misaligned multi-word
(`ldm`/`stm`/`ldrd`) access through a bad base (the UNALIGNED in `route_bus_events`
where `self.bus_event_buf` is taken/reassigned), and a corrupted `shared` pointer
that core 1 then dereferences. `HL=0xffaa` is **HRAM — where the OAM-DMA wait
routine runs** — and the path touches DMA setup + APU register/bus-event routing.

**The tripwire did NOT fire** ⇒ core 1's `shared`/`worker` were intact *at the
loop-top/publish checks*; the core-1 fault (#2) is a stale/transiently-bad base,
i.e. the corruption is **published from core 0**, not generated on core 1.

**Audit of the implicated core-0 path is bounds-clean** (no silent OOB found by
reading): `route_bus_events`/`contiguous_region_len` slice math stays in range;
`advance_dma_bulk` clamps `to_copy ≤ 160-progress`; `copy_dma_step`'s
`from_raw_parts` reads are length-guarded; `read_region_fast`/`write_region_fast`
`get_unchecked` ranges all match their array sizes. So the smasher is **not** an
obvious logic OOB — prime remaining suspects: **(a) heap/allocator corruption**
from the `bus_event_buf`/`events`/APU-`sample_buffer` `Vec` churn during the
LCD-off runaway (the one path whose interior is `unsafe` and not bounds-checked),
or **(b) a stale cached ROM raw pointer** (`rom_fixed_ptr`/`rom_banked_ptr`) used
by `read_cached_rom_window` / `copy_dma_step`.

### Heap-overrun guard added + flashed (2026-06-03)

`platform/pico2w/src/guarded_heap.rs` — a redzone ("electric fence") wrapper
around `embedded_alloc::Heap`, selected as the `#[global_allocator]` under the
**`heap-guard`** cargo feature. Pads every allocation with 16 guard bytes on each
side and verifies them on free (a `Vec` growth frees the old block via the
default `realloc`, so the churning emulator `Vec`s are checked constantly). A
heap buffer overrun — suspect (a) for the #5 free-list/pointer smash — clobbers a
guard and is caught with `heap-guard: <front|back> redzone clobbered: user=…
size=… align=… bad+N`, which the crash handler records as a Panic naming the
**victim allocation's size/align** (⇒ identifies *which* allocation overflowed).

Flashed `--features stack-probe,heap-guard`; boots clean and **save-state load
succeeds** (the redzone overhead does not OOM the ~150 KiB boot peak). Now on the
board, game running — replay to repro.

Interpreting the next repro:
- **`heap-guard: … redzone clobbered`** ⇒ confirmed heap overrun; the size/align
  fingerprints the buffer — chase the writer of that allocation.
- **`core1 ptr corruption`** (the tripwire) ⇒ the published pointer itself.
- **Still a wild HardFault with neither** ⇒ not a heap overrun → pivot to
  suspect (b), the stale cached ROM raw pointer (`rom_fixed_ptr`/`rom_banked_ptr`
  in `read_cached_rom_window`/`copy_dma_step`).

### Deterministic repro + both suspects ruled out + core-0 tripwire (2026-06-03 cont.)

Flashed `--features stack-probe,heap-guard`; the poisoned **save state** (loaded
on boot, ~2.39 B cycles, GB `PC=0x03ce`) made it crash-loop while the user
*played*, producing records #6-#31 — all the same three faults (UNALIGNED
`atomic_load` LR→`GameBoy::tick:214`; IBUSERR `PC=0x68000000`; `Panic memory.r:410`
= DMA `progress>160`; `Panic multicor:400`).

- **`heap-guard` never fired** (no `redzone clobbered` record) ⇒ the smash is
  **not a cross-allocation heap overrun**. Suspect (a) ruled out.
- **Cached-ROM-pointer path is safe** — `XipCartridge::rom_window` uses checked
  `self.rom.get(base..)` (null on OOB) and clamps `len` to `ROM_BANK_BYTES`;
  `refresh_mappings` sets `*_valid = bank < rom_bank_count`. Pointers are never
  wild. Suspect (b) ruled out.
- **The smashed thing is core 0's `Core1Transport.shared`** (`&'static
  SharedWorkerState`, a fixed address): the dominant fault is an UNALIGNED atomic
  load through it (`tick:214`→`read_worker_output`→`poll_output`). My earlier
  tripwire only checked *core 1's* copy, so it never fired.
- **The crash needs GAMEPLAY INPUT.** Idle runs (528 s, then 16 s) never crash;
  the game spins at `PC=0x03ce` (wait-for-input) until a button drives it into
  the crashing routine.

**Added `Core1Transport::check_shared(label)`** — compares the `self.shared`
field bits against `&SHARED_WORKER_STATE` (no deref) at the entry of every
per-tick transport method (`send`, `write_vram_range`, `write_oam_range`,
`write_ppu_register(s)`, `poll_output`). On the next play-repro it `defmt::panic!`s
`core0 transport.shared smashed before <label>` — the label + cycle count names
the tick sub-step that ran just before the smash, bisecting the writer.
Flashed; runs clean idle. **Needs a play-repro to fire.**

### KEY: `defmt::panic!` hides the call site — switched tripwires to core `panic!` (2026-06-03)

A standalone repro committed a **`Panic` at `lib.rs:385`** — which is
`defmt`'s `__defmt_default_panic` (`defmt-1.0.1/src/lib.rs:385`, a bare
`core::panic!()`). **Every `defmt::panic!` records `lib.rs:385`**, losing the
real site. So that record proves **one of our tripwires fired** (check_shared /
assert_core1_pointers / heap-guard redzone) — but not which.

Fix: switched all three tripwires to **`defmt::error!(detail)` + core `panic!`**,
and made `check_shared` **`#[track_caller]`**. Now `panic_loc` is the real site:
- `multicor:<call-site line>` for `check_shared` → names the **tick sub-step**
  (`poll_output` / `wait_for_ticket` / `send` / `write_*` / `published_native_frame`).
- `multicor:<assert line>` for `assert_core1_pointers` (core-1 smash).
- `guarded_:<line>` for a **heap-guard redzone** hit (would reopen suspect a!).
Core panics already record the true site (proven by `memory.r:410`).

**Capture procedure that works (cargo run does NOT commit):**
1. `cd platform/pico2w && cargo run … ` to flash, let it boot, kill it (detach probe).
2. `probe-rs download --chip RP235x --binary-format bin --base-address 0x103FF000 <4KiB-0xFF>` to blank the crash sector.
3. `probe-rs reset --chip RP235x` to run **standalone** (no fault-catch).
4. **Play (needs input)** → crash → fault handler commits on the auto-reboot.
5. `crash_decoder.py --probe` (read-only; doesn't catch faults).
Idle standalone is timing-dependent and unreliable; gameplay input is the
reliable trigger. `defmt::error!` detail (got/want, label) only shows on RTT, so
it is lost in a standalone capture — the `panic_loc` line is the durable signal.

### Earlier hypothesis: core-0 stack overrun onto the transport (superseded 2026-06-04)

The tripwire fired (idle, ~42 s) with the smashed value:
```
core0 transport.shared smashed before send: got=0xb10a68ae want=0x20012438
```
`0xb10a68ae` is **not a pointer** (RAM is `0x2000_xxxx`) — it's wild *data* written
over the `&'static` pointer fields at the **start of `Core1Transport`**
(`command_tx@0`, `audio_rx@4`, `shared@8`).

**Where the transport lives — the key fact:** `PicoGameBoy` is a **stack local in
`main`** (`let mut gameboy: Option<PicoGameBoy>`), and it embeds `GameBoy` →
`Core1Transport` inline. So the transport's pointer fields sit on **core 0's
stack** (the crash record's `R0=0x2007febc` is right there, near the top of the
~79 KiB stack). `SHARED_WORKER_STATE` is 86,704 B at `0x20014470`; the core-1
`Fault@0x152ac` ≈ its last field (base≈0 + end-offset), consistent.

⇒ **#5 is a core-0 STACK BUFFER OVERRUN** that writes onto the transport's
pointer fields. It is *in-bounds* of the stack (SP never crosses the limit, so
`MSPLIM` doesn't fire) and not on the heap (so `heap-guard` doesn't fire) — which
is exactly why both guards stayed silent. It is **stack-layout-sensitive**:
adding the per-tick `checkpoint`s shifted the frame and moved the overrun off the
pointer fields → the crash stopped reproducing (the heisenbug). Those checkpoints
were **reverted**; the build is back to the method-entry `check_shared` (broadened
to all 3 transport pointers) that catches it at ~42 s idle.

Ruled out (cumulative): SP-limit stack overflow, heap overrun, wild ROM pointer,
the audio packing path (`samples_i16_to_i2s` clamps to `buf.len()`; DMA only
reads RAM), cross-core atomic-RMW, core-1 emulator OOB.

### RCA correction: `route_bus_events` old record was missing the destination register (2026-06-04)

The previous "fixed stack address" reading over-interpreted the
`route_bus_events` UNALIGNED record. In the current linked image, the relevant
restore is:

```text
ldrd  r0, r1, [r12, #216]     ; load old self.bus_event_buf for RawVec::drop
bl    RawVec::drop
sub   r0, r7, #0xf4           ; source: local Vec header scratch
ldr   r12, [sp, #0xdc]        ; GameBoy base
ldr   r4, [sp, #0xb8]         ; destination: saved &mut self.bus_event_buf
ldm   r0, {r1, r2, r3}
stm   r4!, {r1, r2, r3}
```

So the recorded `R0=0x2007febc, R1=0x0004` is **not** proof that
`0x2007febc` was the overwritten victim. It is the source stack copy of the
`Vec<BusEvent>` header; `R1=4` is a normal first header word. The actual store
destination is `r4`, which the old `cortex-m-rt` HardFault path did not capture.

This keeps the immediate-corrupting instruction highly suspicious: the final
`self.bus_event_buf = buf` is a deterministic 12-byte multiword store, and
`Core1Transport` begins exactly after `bus_event_buf` in `GameBoy`. But the
precise RCA is now:

> a `route_bus_events` Vec-header restore, or the saved destination feeding it,
> performs a 12-byte store into/near the transport pointer fields. The missing
> operand is the pre-handler `r4` destination, not the stacked `r0`.

Diagnostic added in `src/crash/handler.rs`: a custom HardFault trampoline now
captures pre-handler `r4..r11` before Rust prologue code can reuse them. For
UNALIGNED HardFaults, crash records with flag `0x40`
(`HAS_HARDFAULT_EXTENDED_REGS`) store:

- `panic_loc[0..4]` = pre-handler `r4` (the actual `stm` destination in the
  current route restore)
- `panic_loc[4..8]` = stacked `r12` (the current `GameBoy` base)

Interpret the next UNALIGNED route record this way:

- `r4 == r12 + 216`: the destination is correct; look for source-header
  corruption or a different unaligned instruction.
- `r4 == r12 + 228` (or otherwise within transport): the Vec-header restore is
  directly overwriting the transport pointer fields.
- `r12` sane but `r4` wild/misaligned: the saved destination slot
  (`[sp,#0xb8]` in this build) was corrupted before the restore.
- both `r12` and `r4` bad: the `GameBoy` object path/base was corrupted earlier.

Validated: `RUSTFLAGS='-Z stack-protector=all' cargo +nightly check --release`
passes, but a linked `all` build now exceeds FLASH by ~12 KiB. A linked
`strong` build **does fit**:

```text
CARGO_TARGET_DIR=/tmp/rustyboy-stack-protector-strong \
RUSTFLAGS='-Z stack-protector=strong -C link-arg=-Tlink.x -C link-arg=-Tdefmt.x -C link-arg=--nmagic' \
cargo +nightly build --release
```

The resulting image is ~512,848 bytes of text/data and contains
`__stack_chk_fail`, `__stack_chk_guard`, and `LAST_FAIL_LR`. Reading the existing
flash log also showed older `stack_ch:39` panic records, so stack-protector
**has** caught at least some repros. Those records lost the useful LR because
standalone captures do not preserve RTT output. `src/stack_chk.rs` now stashes
the `__stack_chk_fail` LR and the panic handler stores it in `arm_lr` with flag
`0x80` (`HAS_STACK_CHK_FAIL_LR`), so the next stack-protector repro can be
symbolized directly.

### Fresh `stack-protector=strong` standalone idle repro (2026-06-04)

Flashed the fitting `strong` stack-protector image, blanked the crash sector,
reset standalone, and let it idle ~75 s. It committed:

```text
Crash #1 WatchdogTimeout
Crash #2 Panic memory.r:410
  GB PC=0x07b9 SP=0xdffb AF=0x19a0 BC=0x00c6 DE=0x01c6 HL=0xc1c9
  ROM bank=2, LCDC=0x00, cycle_lo=2,395,577,292
```

`memory.rs:410` is the WRAM branch of `copy_dma_step`:

```rust
self.oam[dst..dst + n].copy_from_slice(&self.wram[off..off + n]);
```

The source side is guarded by `off + n <= self.wram.len()`, so the panic is the
destination slice: `dst + n > self.oam.len()`. In normal flow,
`advance_dma_bulk` computes `to_copy = steps.min(OAM_DMA_BYTES - progress)`, so
that can only happen when `self.dma.progress` was already invalid (for example
`progress > 160`, making the release-build `u8` subtraction wrap). This is
another downstream corruption target, not the root writer, but it confirms the
smash reaches `GameBoy` control state beyond the transport pointers.

Practical next move: harden `advance_dma_bulk` with a saturating/early-complete
guard for `progress >= OAM_DMA_BYTES` so this symptom stops consuming captures;
then rerun the `strong` image to catch either:

- a `stack_ch` panic with durable `arm_lr` (`HAS_STACK_CHK_FAIL_LR`), or
- a `route_bus_events` UNALIGNED record with `r4/r12`
  (`HAS_HARDFAULT_EXTENDED_REGS`).

### Boxing `PicoGameBoy` MASKED but did NOT fix — corruption targets a FIXED stack address (2026-06-03)

Moved `PicoGameBoy` to `Box<PicoGameBoy>` (main.rs + loading.rs; `gameboy:
Option<Box<PicoGameBoy>>`, `.as_deref_mut()` at call sites). Builds clean,
host 191/0. **Result: it STILL crashes on idle (~43 s), but `check_shared` /
the transport tripwire NO LONGER fires** — the transport (now on the heap) is
*not* smashed. Standalone the loop now panics at `multicor:937`
(`published_dirty_rows`, a `self.shared` deref) and `memory.r:410` (DMA
`progress>160`) — i.e. the wild write now corrupts whatever *else* sits at that
stack slot.

**⇒ The corruption writes to a FIXED core-0 STACK ADDRESS** (high in the stack,
~`0x2007febc`, in `main`'s frame region where the transport used to live), **not
to "the transport object."** Moving the object out of the way just relocates the
victim. So it is NOT a wild pointer that chases the transport, and NOT a heap
issue — it is something writing a *fixed high-stack location* in `main`'s frame.
Candidates: a dangling pointer to a `main` local, or a stack array in the
run-frame path overrunning up into `main`'s frame. **`heap-guard` still silent;
boxing did not let it catch the writer (the writer is not a heap overrun).**

Open question: keep the boxing (good practice; transport pointers now safe, but
we lose `check_shared`'s value-bearing catch and the crash moves to murkier
downstream faults) **or revert it** (restore `check_shared` catching the smash
with the `0xb10a68ae` value) while hunting what writes `main`'s frame.

### Core-0 per-frame audit (2026-06-03) — clean; bug is exquisitely layout-sensitive

Reverted the boxing (back to `Option<PicoGameBoy>` + value-bearing `check_shared`).
Audited the entire core-0 per-frame path for a write into `main`'s stack frame:
- `running.rs::tick` — `disp_future`/`audio_future` are `pin!`ed locals **scoped to
  the block and awaited within it**; `dirty_rows` (`[u32;5]`) is borrowed by
  `disp_future` but outlives it. No dangling pointer.
- `display/hw.rs::send_frame` → `scale_native_to_rgb565_range` writes the **static**
  `CORE0_SCALE_BUF` (`.bss`), not the stack; the pixel DMA only **reads** it.
- `setup_frame_range`/`write_command`/`spi1_tx_bytes` — fixed `[u8;4]` stack arrays,
  output to the **SPI peripheral** register. Bounded.
- Audio: `i2s.write` DMA **reads** `&'static` `AUDIO_BUF_*`; packing is clamped.
⇒ **No dangling-pointer or stack-overrun write to `main`'s frame in the obvious
per-frame path.** The writer is subtler (candidates not yet excluded: an
interrupt/DMA-completion handler on core 0's MSP, or the GB-CPU opcode path).

**Tried + REVERTED: in-situ memory dump in `check_shared`.** Adding the dump (even
in the cold panic path) shifted the frame and the crash **stopped reproducing**
in 130 s + 212 s runs. Two earlier builds caught it at ~6 s and ~42 s. ⇒ The bug
is **so stack-layout-sensitive that any edit near `check_shared` suppresses it** —
which is itself strong evidence for a **stack-adjacency overrun** (shifting the
layout changes which neighbour the overrun lands on). It also makes detailed
in-situ instrumentation infeasible; the smashed *value* `0xb10a68ae` + the 3-ptr
`cmd/aud/shr` message (whichever fields are corrupt ⇒ overrun size) remain the
only non-perturbing signals.

### IRQ/DMA + GB-opcode audit (2026-06-03) — both CLEAN. All core-0 code now audited.

- **IRQ/DMA handlers:** all embassy-provided (`dma::InterruptHandler`,
  `PioIrqHandler` in `Irqs`/`WifiIrqs`); they only ack + wake, no user-RAM writes.
  The only custom handler is the HardFault exception (fires on fault only). **No
  DMA has a stack RAM destination** — display `spi.write` and audio `i2s.write`
  are TX-only (RAM→peripheral); SD DMA is idle during gameplay. The 480 B stack
  buffers are menu/loading-only, not the gameplay path.
- **GB-CPU/opcode path** (`sm83.rs`/`cpu.rs`/`bus.rs`/`instructions/`/`operations/`):
  no `unsafe`, no `get_unchecked`, no oversized stack arrays — bounds-safe Rust.

**⇒ Every core-0 code path is now audited (per-frame display/audio/input/SPI, IRQ/
DMA, GB CPU/opcode, the multicore transport) and NONE contains an out-of-bounds
stack write.** Yet the corruption is real (caught with `0xb10a68ae`). The writer
is therefore NOT in the audited application code — remaining possibilities:
the async/embassy executor + waker machinery, embassy-rp DMA buffer-management
internals, or silicon. **Every fine-grained diagnostic is confounded by the
layout-sensitivity** (boxing moved the victim; the in-situ dump suppressed it;
even subsystem-disable would shift layout ambiguously). The non-confounded tools
left are **(a) nightly `-Z stack-protector=all`** (canary trap at the overrunning
function's return — layout-uniform, victim-independent), or **(b) an MPU/`-Z
sanitizer`-style hardware trap** on the write. That is the recommended next move.

### Recommended next steps for #5
1. ~~**Move `PicoGameBoy` off the stack**~~ — DONE; masks the transport symptom
   but does not fix #5 (the corruption targets a fixed stack slot in `main`'s
   frame, not the object). Decide: keep boxing vs revert to restore the
   value-bearing `check_shared` catch.
2. **Find what writes `main`'s high-stack frame** (~`0x2007febc`): audit the
   run-frame path for (a) a dangling pointer to a `main` local that outlives its
   scope, or (b) a fixed stack array overrunning upward into `main`'s frame.
   `run_frame` loop, `GameBoy::tick` → `route_bus_events` (`ppu_reg_buf[16]` —
   bounded), the display scale/blit, the audio drain `Vec`. Look for a fixed
   stack array indexed by an emulator-derived value that can exceed its length
   *without* a bounds check (an `unsafe`/`get_unchecked`, or a `copy_from_slice`
   into a stack array sized by a runtime value).
3. Use `stack_probe::high_water_core0()` (feature `stack-probe`) logged each frame
   to confirm core-0 depth and watch for a spike at the trigger.

### Older recommended next steps (superseded by the stack-overrun finding)
1. **Reproduce with the tripwire live** (already in the tree). On the next #5
   repro the crash record should be a **Panic** "core1 ptr corruption: …" with
   the bad value — read which operand (`shared` vs `worker`) and whether the
   loop-top or `publish_worker_state`-entry firing caught it (see decode table
   above). That single record narrows #5 from "somewhere" to one of three paths.
2. **If `assert_core1_pointers` never fires but it still HardFaults** on the
   store: the operand is fine and the fault is a transient bad base or corruption
   *inside* the worker — pivot to a `probe-rs gdb` hardware **write-watchpoint**
   on `SHARED_WORKER_STATE`'s base / a `GameBoyWorker` guard field, run to the
   `PC=0x03ce` trigger, and catch the writer red-handed.
3. **Audit the CORE-0 emulator path** (the core-1 path is now cleared — see
   above). Crash #1 was on core 0 (`PC=0x68000000` = smashed return address).
   Core 0 runs the full GB **CPU / MBC / memory map / OAM-DMA / save-state** in
   `rustyboy-core` — hunt an unchecked write there (esp. the `HL=0xffaa` HRAM /
   OAM-DMA path at the `PC=0x03ce` trigger). A wild write there smashes core-0's
   stack *and* can feed corruption into the published snapshot that core 1 reads.
4. **Confirm the margins** (closes the stack door fully): build
   `--features stack-probe`, reproduce, read `core1 stack high-water NNNB /
   8192B` — expect well under 8 KiB. If not, reopen the static conclusion.

### HARDWARE WATCHPOINT on the writer — the decisive, non-perturbing catch (2026-06-04)

Every software tripwire (`check_shared`, the in-situ dump, boxing) is confounded
by #5's layout-sensitivity: touching code near the victim shifts the frame and
the overrun moves or vanishes. A **hardware data write-watchpoint** sidesteps
this entirely — it is external to the firmware (no code change, no layout shift)
and halts the CPU **at the exact store instruction** that writes the watched
address, *before* the corrupted value propagates and before the firmware's own
crash handler (`sys_reset`) can run. That store's PC + base register **is** the
root-cause writer.

**KEY GEOMETRY CORRECTION — the transport is NOT on the hardware stack.** Earlier
notes concluded #5 smashed a "fixed core-0 **stack** address ~`0x2007febc`." That
is wrong. This firmware uses `#[embassy_executor::main]`, so the async `main`
task's locals — including `let mut gameboy: Option<PicoGameBoy>` and therefore the
embedded `Core1Transport` — live in the **embassy task arena (a static `POOL`)**,
not on the MSP stack. Confirmed live in gdb:

```text
GameBoy  self      = 0x20051740  <__embassy_main::POOL+296>
transport (cmd/aud/shr) at 0x20051828..0x20051834  <POOL+528>
  command_tx = 0x20002f34 <COMMAND_QUEUE>
  audio_rx   = 0x20005f3c <AUDIO_QUEUE>
  shared     = 0x2000a2fc <SHARED_WORKER_STATE>   ← watch THIS word @ 0x20051830
GB memory arrays live in HEAP_MEM (heap), e.g. 0x2002c648
```

The `0x2007xxxx` region holds only the executor poll frames + `sm83::step`
locals + **spilled copies** of the transport pointers (the *source* of the
12-byte `stm`, not the victim). So the right thing to watch is the POOL word
`0x20051830`, a fixed address for a given image (and re-derivable at runtime via
`&transport.shared`, so it survives rebuilds/layout shifts).

**probe-rs's GDB stub cannot set hardware watchpoints on RP235x.** `watch …`
fails with *"Could not insert hardware watchpoint … too many hardware
breakpoints/watchpoints"* on the very first one (FPB **breakpoints** work — it
auto-used one for `tbreak` — but it reports **0 DWT watchpoint** comparators).
gdb also cannot read the DWT MMIO (`0xE0001000`) through the probe-rs stub
(*Cannot access memory*), so the comparators can't be poked manually either.

**Use the RaspberryPi OpenOCD fork instead** (built at `/tmp/ocd-build/openocd`,
v0.12.0+dev, has `target/rp2350.cfg`). It drives the same CMSIS-DAP probe and
correctly reports **`4 watchpoints` per Cortex-M33** and inserts them.

Working recipe (single-core attach avoids SMP pitfalls):

```sh
# probe-rs and OpenOCD cannot share the probe — kill probe-rs first.
pkill -9 -x probe-rs; pkill -9 -x gdb-multiarch
cd /tmp/ocd-build/openocd
./src/openocd -s tcl -f interface/cmsis-dap.cfg -c "adapter speed 5000" \
    -c "set USE_CORE cm0" -f target/rp2350.cfg -f /tmp/wp.tcl
```

`/tmp/wp.tcl`:
```tcl
init
halt
mww 0x400d8000 0                 ;# disable WATCHDOG.CTRL.ENABLE so a debug-halt
                                  ;#   cannot trigger a reset while we hold cm0
wp 0x20051830 4 w                ;# hardware write-watchpoint on transport.shared
resume
while { 1 } {                     ;# poll forever (no wait_halt timeout cap)
    sleep 300
    catch { poll }
    if { [string compare [rp2350.cm0 curstate] "halted"] == 0 } { break }
}
reg                               ;# PC = the corrupting store; dump base regs
mdw 0x20051828 3                  ;# the smashed value
mdw 0x2007e900 64                 ;# stack context
shutdown
```

**Gotchas learned (each cost a run):**
- **SMP replicates the watchpoint to BOTH cores**, and `wp` aborts if the other
  core isn't halted (`[cm1] can't add … target running` → `Failure setting
  watchpoints`). With the wp failing to arm, the firmware runs free and the next
  real crash just `sys_reset`s → OpenOCD logs *"external reset detected"* and you
  catch **nothing**. Fix: attach to **one core only** with `set USE_CORE cm0`
  (the writer is on core 0) — note this must be `USE_CORE`, not the internal
  `_USE_CORE`, which `rp2350.cfg` overwrites. Then `wp` touches only cm0's DWT and
  cm1 keeps running harmlessly (the cross-core handshake just stalls while cm0 is
  halted; with the watchdog disabled that's fine).
- **Disable the watchdog** (`mww 0x400d8000 0`) after halting — the firmware
  re-arms the 10 s watchdog every boot, and it will reset the chip out from under
  a held halt otherwise.
- **`wait_halt` has a timeout cap**; for an open-ended wait use a `poll` + `sleep`
  TCL loop checking `curstate` instead (the event loop doesn't auto-poll inside a
  blocking TCL loop, so call `poll` explicitly).
- `reg pc force` / `ocd_reg` return formatted strings, not clean integers — don't
  feed them to `expr`/`mdh`; just `reg`-dump and resolve the PC afterward with
  `arm-none-eabi-addr2line -e <ELF> 0x<pc>` (or `addr2line`).

**Idle does NOT reproduce with the watchpoint correctly armed.** A full **290 s
idle** run (cm0-only, watchdog off) timed out with **no hit and no false
positive** (cm0 still healthy: `pc=0x10011918`) — proving (a) the wp is silent on
legitimate execution (transport.shared is written exactly once, at construction)
and (b) **#5 requires gameplay INPUT** (the poisoned save state crash-loops only
while buttons drive the GB CPU into the faulting routine at `PC=0x03ce`). So the
catch must run while the device is *played*.

Current state: OpenOCD is armed on `0x20051830` with **no timeout** (poll loop),
running detached; a file monitor watches `/tmp/wp_result.txt` for the halt. On the
next play-repro it freezes cm0 at the writer's store — resolve `reg pc` →
`addr2line`, and read which base register (r4/r12/…) held the wild POOL
destination to name the overrunning code. **This is expected to finally identify
the actual writer.**

### KEY EVIDENCE: 22-record crash-loop dump — wild PCs are GB-address values (2026-06-04)

A poisoned-save crash-loop produced **22 records** (`crash_decoder.py --probe --json`).
Decoded, they cluster into a few deterministic signatures — and the smoking gun is
that **the wild jump targets are GB address-space values**:

| Variant | core | ARM PC | ARM LR (sym) | CFSR | fault_addr |
|---|---|---|---|---|---|
| A | 0 | `0x00009ffe` | `atomic_add` | IBUSERR | 0 |
| A' | 0 | `0x0000fe9e` | `Sm83 …instruction` | IBUSERR | 0 |
| B | 1 | `0x200338ec` (.bss as code) | `apu::produce_samples` | PRECISERR | `0x00033434`/`0xf0000011`/`0x0718fe85` |
| C | 1 | `0x1001ae54` `mem::replace` | `run_core1_worker::{{closure}}` (multicore.rs:1480) | UNALIGNED | `0x00004142`/`0x0000391f` |
| D | 0 | — | — | WatchdogTimeout | — (freeze) |

**The wild PCs are GB addresses.** `0x9ffe` ∈ GB VRAM (`0x8000–0x9FFF`); `0xfe9e`
∈ GB OAM (`0xFE00–0xFE9F`); `0x4142`/`0x391f` are GB data bytes. So **emulated GB
memory data is being written over host return addresses / pointers**, on *both*
cores. On return / indirect-branch the core jumps to a GB-valued address (→ near-
zero / wild PC), or dereferences a GB-valued pointer (UNALIGNED `mem::replace`).

**Victims span multiple distant regions** (so the corrupting store's destination
varies, i.e. it is a wild *pointer* write, not a fixed overflow):
- core-0 stack return addresses (variant A/A' — return to a GB address),
- core-1 `CORE1_WORKER` (`0x2000a23c`) — the APU `sample_buffer` `Vec` header,
  whose smash makes `produce_samples`' `push`/`mix_sample` branch into `.bss`
  (variant B). `CORE1_WORKER` and `SHARED_WORKER_STATE` (`0x2000a2fc`) are
  **adjacent statics**.
- `SHARED_WORKER_STATE` / its `live_ppu_snapshot` `RefCell` (variant C — the
  `mem::replace` of the borrow flag faults on a GB-valued base),
- POOL transport `0x20051830` (the earlier `check_shared` `0xb10a68ae` catch),
- core-1 stack (the `PC=0xda`, wild `SP=0xf0000000` freeze caught live by gdb).

**`assert_core1_pointers` passes at the loop top but the deref still faults** ⇒
the smash happens **within a single command handler**, after the check. The
publish/snapshot writers (`copy_live_ppu_snapshot`, `write_live_vram_range`,
`write_live_oam_range`) are all length-clamped, so the corruptor is not those.

**Working hypothesis (sharpened):** a store that uses a **GB-derived value as (or
into) a host pointer** writes GB bytes over a host return-address / pointer. The
recurring exact targets (`0x9ffe`, `0xfe9e`) mean it is deterministic under the
poisoned save. Prime places to re-audit with this lens: anything that takes a
**GB address / DMA `source` / bank / offset** and forms a **host** address or
indexes a **host** array without a host-side bound (OAM-DMA `source` handling,
`route_bus_events` destination math, any `get_unchecked`/raw-ptr/`from_raw_parts`
on the publish path), and the **`bus_event_buf` / snapshot `Vec`** churn.

### r4/r12 extracted (decoder patched) — `shared` is wholesale-replaced by a GB address (2026-06-04)

`crash_decoder.py` now prints the trampoline-captured `r4`/`r12` for UNALIGNED
HardFaults (flag `0x40`). For the variant-C records (#12, #17,
`multicore.rs:1480` → `RefCell::borrow`):

```
#12  Stk R0 0x00004142   Pre r4 0x000000c4   Stk r12 0x1001ae45 → RefCell::borrow
#17  Stk R0 0x0000391f   Pre r4 0x000000c4   Stk r12 0x1001ae45 → RefCell::borrow
```

`r4 = 0xC4` is **constant** = the byte offset of `live_ppu_snapshot`'s RefCell
inside `SharedWorkerState`. The faulting address `0x4142 = base + 0xC4` ⇒
**base ≈ `0x407E`** (a GB **ROM-bank** address, `0x4000–0x7FFF`); #17 ⇒ base
≈ `0x385B`, also GB ROM space. So core-1's **`shared` pointer is wholesale-
overwritten with a GB-address value** — confirming the **wild-pointer-write**
class (not a corrupted index), and unifying with the GB-valued wild PCs on core
0 (`0x9ffe` VRAM, `0xfe9e` OAM). The corruptor stores a **GB-address-space
value into a host pointer slot.**

### Writer-chase with the GB-value lens — obvious paths re-audited, all bounded (2026-06-04)

Re-audited every path that handles GB data with the new "GB-address value stored
into a host pointer slot" lens:
- **Core memory** (`memory.rs`): `write_fast`/`read_fast` dispatch through `match`
  ranges so each `address - BASE` offset is provably `< N` before the
  `get_unchecked`/`get_unchecked_mut` — sound. `copy_dma_step`'s destination
  `self.oam[dst..dst+n]` is a **checked** array slice (panics, not OOB); the
  `from_raw_parts` ROM reads are length-guarded; `advance_dma_bulk` is now guarded.
- **Platform publish/sync** (`multicore.rs`): `copy_live_ppu_snapshot`,
  `write_live_vram_range`, `write_live_oam_range`, `sync_ppu_state`,
  `load_ppu_state`, `sync_apu_state` — all fixed-size `copy_from_slice`
  (`..0x80`/`..0x2000`/`..0xA0`) or `.min()`-clamped; they **panic** on mismatch,
  never overrun.

⇒ The writer is **not** in the obvious GB-data-handling code (confirming the
earlier audits). The corruption is real and deterministic (GB-address values
scattered over host pointers/return-addresses across all of RAM), so the writer
is somewhere not reachable by reading: a lifetime/aliasing bug the borrow checker
can't see (e.g. a `&'static mut` handed out twice, a raw pointer cached across a
realloc), the embassy executor/DMA buffer-management internals, or silicon.
**Static audit is exhausted.** Pinning the writer now needs the firmware-side
DWT+DebugMonitor catch (records the writer PC to flash, auto-re-arms through the
reset-loop) — the one tool that survives both the self-reset loop and the
layout-sensitivity.

### PLAN (superseded/partially implemented): firmware-side DWT watchpoint + DebugMonitor catcher (2026-06-04)

Since static audit is exhausted and external live-catch loses to the self-reset
loop, the firmware itself will arm the watchpoint and record the writer.

**Design (`src/dwt_watch.rs` + a `DebugMonitor` exception handler):**
- After the victim is constructed each boot, firmware programs a **DWT data
  write-watchpoint** on the victim address and enables **monitor-mode debug**
  (`DEMCR.TRCENA | DEMCR.MON_EN`). Because the address is taken at runtime, it
  survives layout shifts; because it re-runs every boot, it **auto-re-arms
  through the crash-loop**.
- On a watched write, the CPU takes the **`DebugMonitor`** exception. The handler
  reads the stacked PC (the storing instruction) + r0-r3/r12 and commits a crash
  record via the existing WATCHDOG/POWMAN-scratch path (new `CrashKind` or reuse
  HardFault), then `sys_reset`. Next boot commits it to flash →
  `crash_decoder.py` symbolizes the **writer PC**.

**Target(s).** Start with the proven-catchable fixed victim: core-0
`&transport.shared` in POOL (caught once as `0xb10a68ae`). DWT is **banked
per-core**, so arm it from *each* core (core 0 at boot, core 1 inside
`run_core1_worker`) to catch a cross-core writer whichever side it runs on. 4
comparators available — can also watch `SHARED_WORKER_STATE`/`CORE1_WORKER`.

**Encoding (OpenOCD ARMv8-M v2.x path):** `DWT_COMPn` (`0xE0001020+n*0x10`) =
addr; `DWT_MASKn` = 0 for an exact 4-byte watch; `DWT_FUNCTIONn` writable bits =
`0x815` (`FUNCTION=write`, linked data-address compare, 4-byte access size).
Readback includes high read-only status/ID bits (e.g. `0x58000815`), so do not
write only the high `0x58000000` bits. `DEMCR` (`0xE000EDFC`) needs `TRCENA`
(bit24) + `MON_EN` (bit16).

**Known limitations to verify:** (a) monitor-mode is disabled while a debugger
holds `C_DEBUGEN` — so this only fires **standalone** (matches the existing
capture procedure); (b) a watchpoint debug event raised while execution priority
is ≥ DebugMonitor priority (e.g. inside a `critical_section` / interrupts masked)
is **pended**, so the recorded PC for *those* instances may be imprecise — but
writes outside masked regions (incl. a cross-core writer on the other core) are
caught precisely. Only **one** precise catch is needed to name the writer.

### BREAKTHROUGH: the DWT encoding bug that silently disabled every prior watchpoint (2026-06-04)

**All earlier mww-direct DWT attempts wrote a *disabled* comparator.** The
RP2350 M33 `DWT_FUNCTION0` (`0xE0001028`) reads back `0x58000000` even after
writing 0 — those are **read-only status/ID bits**. The *writable* bits are the
low ones, and a write-watchpoint needs them set. The authoritative encoding,
read back from OpenOCD's own working `wp` (ARMv8-M v2.x path,
`cortex_m.c:2082`): for a 4-byte data **write** watchpoint,
`FUNCTION = 5 | (1<<4) | (data_size<<10)` with `data_size = len>>1 = 2` ⇒
**`0x815`** (reads back as `0x58000815`); on RP2350 DWTv2, the access size is in
`FUNCTION.DATAVSIZE`, and `MASK`(+4) must be **0** for an exact-word match;
`COMP` = address.

So my mww-direct value `0x58000000` had **writable bits = 0 = DISABLED** — which
is why the firmware-armed / auto-rearm watchpoints never fired and the A/B test
"failed". Verified `0x815` **works**: armed on a hot static (`CRASH_CONTEXT`), it
halted immediately.

**Working external rig (no firmware change):** OpenOCD cm0-only, programs DWT
COMP0=`0x20051830` (POOL `transport.shared`) and COMP1=`0x20081fe8` (core-1
`shared` arg-spill, top of `CORE1_STACK`) with `MASK=0`, `FUNCTION=0x815`;
re-arms every 5s via **halt→mww→resume** (DWT writes only take effect while
halted, and this re-arms after each self-reset). Value-filter to skip legit
writes: the corruptor signature is a **GB-address value `0 < v < 0x10000`**
(excludes boot `.bss`-zero
`0`, construction `0x2000a2fc`, stack paint `0xA5A5A5A5`, `0xffffffff`).

**Proven end-to-end:** with a loose filter it caught the boot `.bss`-zero of the
POOL at PC `0x10000148` = `Reset` (a *false* positive, now filtered out) —
confirming the comparator, the halt, the read-back, and the addresses are all
correct. The rig is armed and correct; it now needs the **corruption to fire**
(gameplay — idle does not reproduce). On the real catch it prints the writer
`reg pc`/`r0-r7`/`r12` + which victim word + the GB value.

### The victim varies — fixed-address watchpoints can't cover it; trap the wild BRANCH instead (2026-06-04)

With the now-working rig (correct `0x815` encoding) a freeze repro showed the
problem with watching fixed victims: **both** watched pointer slots were intact
(`transport.shared`=`0x2000a2fc`, core-1 `shared` spill=`0x2000a2fc`), yet core 1
still died (PC=`0xda`, SP=`0xf0000000`). The kill was a **smashed return address
on core 1's stack** (a *varying* location), and separately the `sync_snapshot`
RefCell borrow flag at `0x2000a2fc` was found smashed to `0x01010101` (GB `0x01`
bytes). So the corruptor is a **wild store whose destination varies** across
return addresses / borrow flags / the `shared` pointer — no single fixed-address
data watchpoint covers all variants (and the borrow flag is legitimately written
every snapshot borrow, so it can't be watched without constant false halts).

**PLAN (option 2): trap the wild BRANCH, not the victim.** Every fatal variant
ends with a core executing a **wild PC in the boot-ROM / low region**
(`0x00000000–0x00007FFF`: core 1 → `0xda`/`0x147`, core 0 → `0x9ffe`/`0xfe9e`).
A **DWT instruction-address comparator** with a mask covering that range traps
the core *on the first wild instruction* — at which point `LR` + the stacked
frame name the **function whose return address was smashed** (the divergence
point), which localizes the corruption to one stack frame regardless of which
byte the wild store hit. This is variant-independent (catches the dominant freeze
too).

Steps: (a) empirically determine the ARMv8-M instruction-match `FUNCTION`
encoding (the data-write value was `0x815`; instruction match is a different
`FUNCTION`/MATCH — verify with OpenOCD by trapping a known-executed PC like the
idle loop `0x10019fbe`); (b) program COMP=`0x00000000`, MASK to cover
`0x0..0x7FFF`, instruction-match FUNCTION, on **cm1** (and/or cm0); (c) on the
wild branch, dump `LR`, `SP`, and the stacked return chain → the smashed frame.
OpenOCD external (no firmware change) first; fall back to a firmware DWT +
DebugMonitor that records `LR` to flash if the reset-loop interferes.

### Tooling reality check: live-catch is defeated by the self-reset loop (2026-06-04)

The firmware `sys_reset`s on every fault and the watchdog resets the freeze
variant — **each reset clears the DWT/FPB**, so an externally-set OpenOCD
watchpoint/breakpoint is wiped before the *next* crash's writer runs. Re-arming
across resets is unreliable (a software self-reset leaves the core *running*, so
there's no clean halt to re-arm at; periodic `mww`-direct DWT programming did
**not** produce a functional watchpoint in an A/B test, and even OpenOCD's own
`wp` couldn't be validated because the fast crash-loop never renders a frame).
Net: **purely-external live catch is the wrong tool against a self-resetting
crash-loop.** The robust catch is **firmware-side**: either (a) a DWT watchpoint
+ DebugMonitor handler the firmware arms *after* constructing the victim each
boot (auto re-arms through the loop, records the writer PC to flash), or (b)
tighter in-code tripwires placed **immediately before** the variant-C deref
(`multicore.rs:1480`) and inside `produce_samples`, plus capturing the UNALIGNED
`r4`/`r12` the trampoline already saves (records #12/#17 have flag `0x40` set, but
`crash_decoder.py` does not yet *print* the extended r4/r12 — add that to the
decoder to read the base register of the variant-C deref for free).

### Assembly update: route_bus_events is the rogue 12-byte write primitive (2026-06-04)

Current optimized layout puts `GameBoy.bus_event_buf` immediately before
`GameBoy.transport`:

- `bus_event_buf`: `GameBoy + 0xd8`, 12-byte `Vec<BusEvent>` header.
- `transport`: `GameBoy + 0xe4`, 172 bytes.
- actual transport pointer fields: `command_tx` at `GameBoy + 0xe8`,
  `audio_rx` at `GameBoy + 0xec`, `shared` at `GameBoy + 0xf0`.

That means a 12-byte write beginning at `GameBoy + 0xe8` exactly replaces the
three checked pointer fields. The old inline comments saying
`Core1Transport(command_tx@0,audio_rx@4,shared@8)` are stale in optimized layout:
`pending_ppu` is at transport offset 0, so the checked pointer triplet starts at
transport offset 4.

The current ELF has `route_bus_events` inlined into the main async poll body
around `0x10004036`. The relevant code is:

```text
0x10004044  add.w r0, r12, #0xd8      ; &self.bus_event_buf
0x10004048  str   r0, [sp, #0xc4]     ; saved restore destination
0x1000404a  sub.w r1, r7, #0x2c       ; local Vec header
0x1000404e  ldm.w r0, {r2, r3, r4}
0x10004054  stm   r1!, {r2, r3, r4}   ; mem::take copy to stack
...
0x1000486e  sub.w r0, r7, #0x2c
0x10004876  ldr   r4, [sp, #0xc4]
0x1000487c  ldm.w r0, {r1, r2, r3}
0x10004880  stm   r4!, {r1, r2, r3}   ; self.bus_event_buf = buf
```

So the proven rogue-write primitive is the final `self.bus_event_buf = buf`
restore: if `[sp,#0xc4]` is corrupted, it becomes a 12-byte arbitrary write; if
`r4` is unaligned, it directly explains the old `route_bus_events` UNALIGNED
record. If the local Vec header at `r7 - 0x2c` is also poisoned, the payload is
not expected to look like a valid Vec header; this matches the observed
transport smash bytes:

```text
cmd=0x2300d1f5 aud=0xe7f8682e shr=0xb10a68ae
bytes: f5 d1 00 23 2e 68 f8 e7 ae 68 0a b1
```

There is an earlier route-bus write primitive too. `self.memory.drain_into(&mut
buf)` trusts the copied `Vec` header immediately:

```text
0x10004080  ldr   r5, [r7, #-36]      ; local Vec len
0x10004090  ldr   r0, [r7, #-44]      ; local Vec cap
0x10004098  ldr   r0, [r7, #-40]      ; local Vec ptr
0x100040a2  strh.w r6,  [r0, r5, lsl #2]
0x100040aa  strb.w r11, [r0, #2]
```

If the persistent `bus_event_buf` header was already corrupted before
`route_bus_events`, the drain loop writes `(u16 address, u8 value)` BusEvents
through the corrupt pointer. Since `tick()` calls `route_bus_events()` immediately
after the SM83 instruction, those values naturally look like GB addresses and IO
data.

**What this proves:** the source of the observed 12-byte transport smash is
`route_bus_events`'s `Vec<BusEvent>` header restore/drain path, not a normal
transport method. This also explains why tiny layout changes move or suppress the
crash: the victim is whichever object/stack slot the corrupted Vec pointer or
saved restore destination names.

**What is still not proved:** the first write that corrupts the `bus_event_buf`
header or the `[sp,#0xc4]`/`r7-0x2c` stack slots. Fixed-address watchpoints on
`transport.shared` miss variants because they watch a later victim. The next
capture should watch the header and the route-bus stack slots instead:

- persistent header: `&self.bus_event_buf` (`GameBoy + 0xd8`; in older GDB
  captures with `self=0x20050650`, this was `0x20050728`).
- route restore destination slot while inside route-bus: `[sp,#0xc4]`.
- route local Vec header while inside route-bus: `r7 - 0x2c` through `r7 - 0x20`.

The most useful firmware-side tripwire is a small guard inside `route_bus_events`
that snapshots the persistent header before `take`, verifies the local header
after `drain_into`, and records `r4/r12` plus the three header words before the
final restore. A structural mitigation would avoid storing a reusable
`Vec<BusEvent>` next to transport at all: drain the memory event queue directly
or into a fixed-size stack buffer, so there is no persistent Vec header to poison
and no final 12-byte header restore.

### CRITICAL FIX: dwt_watch MASK was 3 → comparator disabled; set to 0; firmware DWT now works (2026-06-04)

The firmware DWT catch (below) never fired because `dwt_watch.rs` set
`DWT_MASK_WORD = 3`. On RP2350 (ARMv8-M v2.x) the access size is in
`FUNCTION`'s `DATAVSIZE` field (`2 << 10`) and **`DWT_MASKn` must be 0** for an
exact-word match — verified empirically: OpenOCD's working `wp` leaves `MASK0=0`,
and `COMP=addr, MASK=0, FUNCTION=0x815` traps; with `MASK=3` the comparator
never matches. Fixed to `0`. After the fix, a live read of the firmware-armed
DWT shows `COMP0=0x200518a0, MASK0=0, FUNCTION0=0x59000815` with the **MATCHED
bit (24) set** — the watchpoint on `command_tx` *does* now fire. (`0x58000000`
high bits are read-only status; `0x815` is the writable part.)

**Capture status / remaining friction (2026-06-04):**
- Under `cargo run` (probe attached) the transport smash ("Path A") reproduces
  2/2 at 8–17 s; the software tripwire logs e.g.
  `cmd=0x1001ddd5 aud=0x2007e890 shr=0x00000020`. But probe-rs holds
  `C_DEBUGEN`, so the DWT event **halts** (no DebugMonitor record).
- Standalone (`probe-rs reset`, no probe), the dominant variants are **Path B
  panics that fire *before* the transport restore**: `gameboy.rs:455`
  (`self.memory.vram()[start..end]` OOB from corrupted bus events) and
  `multicor:401` (`write_live_vram_range`). No `arm_cfsr=0xD7170001` DWT record
  captured yet — Path A is probe-timing-dependent and rare standalone.
- ⇒ The DWT-on-transport catches a **late** victim. The proximate victim is the
  **`bus_event_buf` header** (`GameBoy+0xd8`), hit in *both* paths. Next: watch
  the header word with a **value-filtered** DebugMonitor (skip a valid heap
  `ptr`, record a garbage one), or pursue the structural fix (drop the persistent
  `Vec<BusEvent>`).

**Smash payload is an `embassy_time::every` STACK FRAME.** The poisoned 12-byte
header `{0x1001ddd5, 0x2007e890, 0x20}` decodes as `{ret-addr into
embassy_time::every, a core-0 stack addr, 0x20}` — and the **same 12 bytes recur
at several core-0 stack depths** (`0x2007e964/ec78/f58c`) and in POOL/statics.
So `bus_event_buf.ptr` (which should be a heap addr `0x2002xxxx`) is being
overwritten with a flash return address from a **timer future's stack frame**.
This points the root cause at an **embassy timer-future / task-arena overlap**
writing a stack frame into the main task's POOL where `GameBoy.bus_event_buf`
lives — i.e. *not* application logic, consistent with the long-standing "writer
is in the executor/future machinery" suspicion. The `route_bus_events` restore
then amplifies that poisoned header into the 12-byte arbitrary write.

### Header-watch (option 1) implemented; value-filter fires but doesn't yet record corruptor (2026-06-04)

Re-pointed the firmware DWT from the transport triplet to the **`bus_event_buf`
header** (`GameBoy + 0xd8`, the proximate victim). Two filtering strategies tried:

1. **Disarm-around-`gb.tick()`** (`dwt_watch::disarm_for_current_core()` before
   `gb.tick`, re-arm after): catches only out-of-band writes, no value filter.
   **Result: SUPPRESSED the bug** — 90 s under `cargo run` with zero crashes.
   The extra hot-path call shifts the layout-sensitive bug off the header. Bad.
2. **Value-filter in DebugMonitor** (no hot-path change — same shape as the
   reproducing transport-watch build, just offset `0xd8`; the handler skips a
   legit `Vec` ptr `< 0x1000` or `0x2002_0000..0x2006_0000` and **returns**,
   records only a wild flash/stack value). Needed a returning `DebugMonitor`
   trampoline (`push {r12,lr}; bl handler; pop; bx r12`). **Does NOT suppress** —
   confirmed: under `cargo run` it halts at `route_bus_events` `0x10004098` (a
   legit header access), so the watch is armed and firing correctly.

**Process trap that cost several cycles:** `cargo run` MUST be run **from
`platform/pico2w/`** (bare, no `cd` to the workspace root) — from the root it
builds/runs the *web-server*, silently NOT flashing the firmware. Several
"suppressed / bad magic" results were actually stale firmware. Always confirm the
new `IMAGE_CRC` in the boot log.

**3-minute standalone capture of the value-filter build** (correctly flashed)
gave 2 records but **no `0xD7170001`**:
- `multicor:1374` (transport-smash tripwire), and
- **HardFault PC=`__aeabi_memset4`, LR=`NonNull::add`, Fault@`0x00008000`** — a
  `Vec` memset through a **`ptr` corrupted to GB VRAM `0x8000`**. This is the
  `bus_event_buf` (or another `Vec`) whose backing pointer was overwritten with a
  GB address → the next grow/drain memsets through `0x8000` and bus-faults.

**Open puzzle:** the header watch fires on legit writes, and a `0x8000`-class
write to the header `ptr` should fail `is_legit_header_word` and be recorded —
yet no DWT record appears. Hypotheses to check next: (a) the corruptor writes the
header `ptr` while the comparator that trips first is the `cap`/`len` word with a
*small* value (passes the filter → skipped), and the `ptr` write is missed
because the 12-byte store trips only one comparator per access; (b) the corruptor
is a `memset`/`memcpy` block write that the value read-back at `hit.address`
no longer reflects; (c) DWT-on-return re-fires/timing. Fix idea: in the handler,
check **all three** header words for a wild value (not just `hit.address`), and/or
record the pre-write context regardless and post-filter offline.

### Status after option-1 header-watch attempts — corruptor PC still uncaught (2026-06-04)

Current worktree state: firmware DWT watches the `bus_event_buf` header
(`GameBoy+0xd8`), DebugMonitor value-filters by the **`ptr` word** (skip
heap/dangling, record wild). Flashed + capturing. Findings:

- The watch **fires and is armed** (halts at `route_bus_events 0x10004098` under
  `cargo run`); it does **not** suppress the bug (unlike the disarm approach).
- The crash is **intermittent**: a 180 s standalone run produced a transport-smash
  tripwire (`multicor:1374`) + the **memset-through-GB-ptr HardFault**
  (`__aeabi_memset4`, `NonNull::add`, Fault@`0x8000`); a 100 s run produced
  nothing.
- **No `0xD7170001` DWT record yet.** Even when it crashes, the header watch is
  not catching the corruptor's write — the corruption surfaces only as downstream
  faults. This suggests the `bus_event_buf.ptr` reaches a GB value **not via a
  single direct CPU store to `+0xd8`** that the DWT sees, but via e.g. a
  `RawVec` realloc storing an allocator-returned (already-bad) pointer, or the
  allocator free-list itself being corrupted, or a `memcpy`/`memset` block that
  the DWT comparator coalesces differently. The memset@`0x8000` (a `Vec` grow/
  zero through `ptr=0x8000`) is consistent with **allocator/free-list
  corruption** feeding a bad pointer into the Vec, rather than a plain pointer
  store.

**Strongest remaining leads for the actual root cause (next session):**
1. **Allocator/free-list corruption** — `embedded-alloc`'s heap is shared by both
   cores (the `bus_event_buf`, APU `sample_buffer`, and embassy futures all
   allocate). A corrupted free-list node would make `RawVec::reserve` return a
   wild `ptr` (→ memset@`0x8000`). Watch the heap free-list head / a guarded
   allocator (the `heap-guard` redzone never fired, but a free-list *pointer*
   smash is different from a redzone overrun).
2. **embassy timer-future / POOL overlap** — the smash payload is an
   `embassy_time::every` stack frame; chase the task-arena sizing.

The DWT header-watch infra is sound and reusable; the open work is choosing a
victim the corruptor *directly* stores (the allocator free-list head is the
prime candidate).

### Free-list experiment: NEGATIVE — allocator is intact (2026-06-04)

Replaced `guarded_heap.rs` with a lean, zero-padding `alloc`-return validator
(panics if `embedded_alloc::Heap::alloc` ever returns a pointer outside
`[heap_start, heap_end)`). Built `--features heap-guard`, standalone capture: 5
records, **no `free-list: alloc OUT-OF-HEAP` panic**. So the allocator never hands
out a wild pointer — the **free-list is not corrupted**.

⇒ The wild `Vec` pointers (the `memset@0x8000`) are **in-place overwrites of a
live `Vec` header *after* a valid allocation**, not bad allocations. This points
back at an external wild *store* scattering GB-address values over host pointer
words (transport, `bus_event_buf` header, other `Vec` headers, return
addresses) — not an allocator/free-list defect. The validator is layout-neutral
(no padding) and the bug still reproduced (5 records, same variants:
`0xfe9e`/`0x8000` wild PCs, core-1 `produce_samples`→.bss, plus a new
`UNDEFINSTR` in `RpSpinlockCs::acquire` Fault@`0xe8d03101`).

**Hypotheses remaining** (allocator now excluded): the embassy timer-future /
task-arena overlap writing a stack frame into the POOL, or a wild store whose
destination is a GB-derived host address. The DWT header-watch is the right
instrument but the in-place overwrite is intermittent and has not yet produced a
`0xD7170001` record.

### Header-watch filter widened to all three `Vec` header words (2026-06-04)

Follow-up fix in this worktree: the DebugMonitor value filter no longer trusts
one named `ptr` word. Optimized `Vec` field order is layout-dependent, and a
12-byte store can trip the DWT comparator on the small cap/len word while a
different word in the same header is already wild. The handler now snapshots all
three watched words and records if **any** is impossible (`>=0x1000` and outside
the plausible heap/POOL range `0x2002_0000..0x2006_0000`). Legit take/restore
writes still clear DFSR and resume.

This confirmed the filter shape, but the next clean-log capture below showed
the `bus_event_buf` header was still one hop too late for one standalone
variant.

### Clean-log capture moved the target earlier: watch `memory.events` (2026-06-04)

After marking the full old crash sector read, a fresh 3-minute standalone run
recorded four new downstream records but still no `0xD7170001`: one watchdog,
two wild-PC HardFaults (`PC=LR=0xff138a02`, `PC=LR=0x09000000`), and a precise
fault in `GameBoyMemory::drain_into`:

- `ARM PC=0x1000303a` = `VecDeque::Drain::next` / `GameBoyMemory::drain_into`
  at `memory.rs:645`, called from `route_bus_events`.
- Disassembly: `r8 = *(GameBoy+0x190)`; `ldm r8,{cap,ptr,head}`; faulting
  instruction `ldrh.w r6, [ptr, index, lsl #2]`.
- `Fault@ 0x20181809`, outside valid SRAM, means the source `events` deque
  buffer pointer/head state was already corrupted before the drain copied into
  `bus_event_buf`.

So the bus-event-buffer watch was one hop too late for this variant. Current
worktree retargets DWT to the four-word `GameBoyMemory.events` `VecDeque` header
and expands the firmware watch helper from 3 to 4 comparators. Legit small
cap/head/len writes and heap pointers still resume; a wild queue header word
should now produce the desired `0xD7170001` writer-PC record.

Follow-up after flashing the retargeted events-header watch (`IMAGE_CRC
0xc9a9e32f`): under `cargo run`, probe-rs halted at `VecDeque::push_back`
immediately after `str r0, [r5,#0xc]` (the legitimate `len += 1` header write),
confirming the watch is armed on the header. A standalone 3-minute run appended
two fresh downstream records but still no DWT record: wild PC
`PC=LR=0xff247702`, then the same `drain_into` precise bus fault
`PC=0x10003032`, `Fault@0x20181809`.

That means the value filter was still too weak: it treated each word
independently, so a small-but-impossible `head`/`len` could pass even when
`head >= cap` or `len > cap`. Current worktree tightens the DebugMonitor filter
to validate the actual VecDeque header invariant: small cap, heap/POOL ptr,
`head < cap`, `len <= cap` (with the empty cap=0 case handled separately). Next
flash should use this stricter filter.

Strict-filter build flashed (`IMAGE_CRC 0xd3884c6d`) and verified under
`cargo run`: the probe halt again lands immediately after the legitimate
`VecDeque::push_back` len update, so the watch is still on the header. A 3-minute
standalone run appended five more records but **still no `0xD7170001`**:
wild PCs (`0xfe9e`, `0x09000000`, `0xff122702`) and repeated route drain faults
at `PC=0x10003032`, `Fault@0x20181809`.

Interpretation: either (a) the corrupting write occurs while DebugMonitor is
masked/pended and the downstream HardFault happens before monitor mode can run,
or (b) the bad address is produced without a direct CPU write to the watched
header words that DWT observes. The watch placement is verified, and the crash
log has room; the next useful instrument is not another range tweak but a
pre-fault route/drain guard that records the live VecDeque header words and
stacked `r0/r1/r2` for the `drain_into` fault, or a carefully PC-filtered DWT
mode that records non-`push_back`/non-drain header writes regardless of value.

### Route-drain pre-fault guard build (2026-06-04)

Implemented the pre-drain guard in this worktree. Core-side
`GameBoyMemory::bus_event_queue_header()` snapshots the ARM `VecDeque` header
as cap/ptr/head/len, and `GameBoy::route_bus_events()` calls the guard
immediately after `has_events()` and again immediately after
`core::mem::take(&mut self.bus_event_buf)`, before `drain_into`.

If the header is impossible, `rustyboy_route_drain_guard` writes a synthetic
record instead of letting the later `drain_into` bus fault consume the evidence:

- `arm_cfsr=0xD9170001` = route-drain guard sentinel.
- `arm_pc` = LR captured inside the platform hook (guard callsite return).
- `arm_lr` and POWMAN scratch[5] low16 = bad word index
  (`0=cap`, `1=ptr`, `2=head`, `3=len`).
- `arm_hfsr` = cap, `arm_fault_addr` = ptr.
- diagnostic tail (`panic_loc[0..4]`, `[4..8]` in the decoder) = head/len.

DWT arming is disabled for this build; keep the DWT DebugMonitor code in place,
but do not arm it while testing the guard because probe-rs stops on legitimate
`VecDeque::push_back` writes before the guard can run.

First guard-only capture (`IMAGE_CRC 0x0faff75e`) appended new downstream
records but no `0xD9170001`: repeated faults still landed at the `drain_into`
loop (`PC=0x10003032`, `Fault@0x20181809`). Disassembly shows the first guard
runs before the `core::mem::take(&mut self.bus_event_buf)` header move/emptying
sequence, while the faulting loop reloads the `events` cap/ptr/head afterward.
The second guard now tests whether the take path or its stack slots corrupt the
source queue header between those two points.

Second-guard capture (`IMAGE_CRC 0x6519d344`) did not produce a route guard
record in the first short run; a longer run produced new **core-1** tripwire
records instead: `Panic multicor:1517` (`live_ppu_snapshot.borrow()` while
refreshing render state) and `Panic multicor:1434` (generic
`shared`/`worker` pointer assertion). That moves the active trail back to the
shared PPU snapshot / core-1 worker boundary.

Current instrumentation upgrades those panics to structured synthetic records:

- `arm_cfsr=0xC0110001` = core-1 pointer guard. `arm_pc` = hook caller LR,
  `arm_lr` and POWMAN scratch[5] low16 = callsite line, `arm_hfsr` = observed
  `shared`, `arm_fault_addr` = observed `worker`, diagnostic tail =
  expected `shared`/`worker`.
- `arm_cfsr=0xC0110002` = `live_ppu_snapshot` borrow guard. `arm_pc` = hook
  caller LR, `arm_lr` and POWMAN scratch[5] low16 = callsite line, `arm_hfsr` =
  raw `RefCell` borrow word, `arm_fault_addr` = `ppu_render_version`,
  diagnostic tail = `shared`/`worker`.

Structured tripwire image (`IMAGE_CRC 0x019df5ec`) reproduced immediately under
`cargo run`, but as a different symptom: `Panic memory.r:459`, the OAM DMA
`copy_from_slice` destination slice. `advance_dma` already clamps `count` to the
remaining OAM bytes, so `dst + count > 160` means the `DmaState` or call-frame
arguments were corrupted after that clamp. Current instrumentation adds:

- `arm_cfsr=0xD6A00001` = OAM DMA guard. `arm_pc` = hook caller LR, `arm_lr` and
  POWMAN scratch[5] low16 = `copy_dma_step` caller line, `arm_hfsr` packs
  `(source << 16) | (progress << 8) | count`, `arm_fault_addr` = `actual_src`,
  diagnostic tail = destination offset/count as `usize` words.

Clean run after blanking the sector produced a sharper fresh fault:
`PpuPeripheral::tick` on core 1 faulted at `PC=0x2000259e`,
`Fault@0xf5008082`. Disassembly:

- `r0 = [sp,#0x98]`
- `r1 = [r0,#0x94]`
- faulting `ldrsb r5, [r1,#0x2040]`

`r0` is the `GameBoyWorker` pointer and `+0x94` is the worker's boxed
`PpuWorkerState` pointer. Therefore the worker object survived, but its `ppu`
Box pointer was corrupted to about `0xf5006042` before `AdvancePpu`.

Current worktree repoints firmware DWT to the fixed `GameBoyWorker.ppu` Box
field word immediately after worker initialization. This is a much better watch
target than `events`: it is fixed, single-word, and should never be written
again after init. DebugMonitor raw mode records any write with
`arm_cfsr=0xD7170001`, `arm_pc` = writer PC, `arm_fault_addr` = watched field
address, `arm_hfsr` = raw DWT `FUNCTIONn`, diagnostic tail = pre-handler
`r4`/stacked `r12`.

Worker-PPU-pointer DWT build flashed as `IMAGE_CRC 0xa25d1e42`. Under
`cargo run` it booted cleanly, entered the main loop, then probe-rs stopped on
an exception before detach. Core 1 was idle in `run_core1_worker`'s empty-queue
`wfe` path (`multicore.rs:1585` / `0x20002374`), so the visible frame was not
the writer. The intended next step is to decode the fresh records and/or rerun
standalone with this image; expected winning record is either:

- `arm_cfsr=0xD7170001` raw DWT write to the `GameBoyWorker.ppu` Box field
  (writer PC captured), or
- `arm_cfsr=0xC0110001` backup assertion showing observed worker/ppu pointers
  versus expected values.

Clean run with the matching `0xa25d1e42` ELF did **not** hit the worker-PPU
DWT target. Instead, all fresh crashes were core-0 precise bus faults in
`GameBoyMemory::write_io`:

- `PC=0x10003a0c/0x10003a16`, `Fault@0x400660d8/e3`.
- Disassembly: `r2 = [GameBoy + 0x190]`, then `strb` to `r2 + 0x40cc/0x40d7`.
- Therefore the main `GameBoy.memory` Box pointer, or the `GameBoy` base
  register used to load it, was wrong. The effective memory base was
  `0x4006200c`, a peripheral/MMIO-looking address.

Current worktree retargets the raw DWT watch to the live `GameBoy.memory` Box
field from `PicoGameBoy::tick()` (so the watched address is after all moves into
the async task frame). Synthetic backup guard:

- `arm_cfsr=0xC0110003` = `GameBoy.memory` pointer guard. `arm_hfsr` =
  `GameBoy` address, `arm_fault_addr` = observed memory pointer, diagnostic
  tail = expected memory pointer / watched field address.
- Ordinary HardFaults now always preserve diagnostic tail
  (`panic_loc[0..4]`, `[4..8]`) = pre-handler `r4` / stacked `r12`, not only
  for UNALIGNED faults. This should distinguish "Box field was corrupted" from
  "GameBoy base register/stack slot was corrupted."

### Firmware DWT transport-triplet catch added (2026-06-04)

Implemented platform-only firmware catch in this worktree:

- `src/dwt_watch.rs` programs three ARMv8-M DWT v2 data write-watchpoints on the
  runtime transport pointer triplet (`GameBoy+0xe8`, `+0xec`, `+0xf0`).
- Core 0 publishes/arms the target from `PicoGameBoy::tick`; core 1 polls that
  published address in `run_core1_worker` and arms its own banked DWT.
- `DebugMonitor` now has a custom trampoline beside `HardFault`, capturing
  `r4..r11` before Rust prologue code can reuse them.
- A DebugMonitor hit commits a normal crash record through WATCHDOG/POWMAN
  scratch, then resets. Decode as:
  - `crash_kind=HardFault`, `flags` includes `HAS_ARM_REGS` and
    `HAS_HARDFAULT_EXTENDED_REGS`.
  - `arm_cfsr=0xD7170001` means "DWT watchpoint", not an architectural CFSR.
  - `arm_pc` is the writer PC. If the route-bus hypothesis is right, this should
    symbolize to the final `route_bus_events` restore (`stm r4!, {r1,r2,r3}`).
  - `arm_fault_addr` is the watched word (`GameBoy+0xe8/+0xec/+0xf0`).
  - `arm_hfsr` holds raw DWT `FUNCTIONn` readback for the matched/fallback
    comparator.
  - diagnostic tail word 0 = pre-handler `r4` (the route restore destination);
    diagnostic tail word 1 = stacked `r12` (often the current `GameBoy` base in
    the route-bus path).

This does **not** catch the first corruptor if it only poisons the route-bus
stack/local Vec header and the final restore later copies from it. It does,
however, convert the transport smash from a post-facto tripwire into a precise
writer-PC record, and it is armed from both cores so a cross-core store to the
core-0 object is still caught. The `report_transport_smash` fallback scanner now
also classifies every duplicate `cmd`/`aud`/`shr` payload hit:
`transport_ptr_triplet`, `bus_event_buf_header`, `core0_stack`, `core1_stack`,
or `sram_static_or_allocator`. A `core0_stack` 12-byte match outside the transport
would point at the route local header / restore-destination spill copy to watch
next.

Operational note: if `crash_decoder.py --probe` says `Probe not found`, check
for a stale OpenOCD session too, not just `probe-rs`. On 2026-06-04 the holder
was `/tmp/ocd-build/openocd/src/openocd ... -f /tmp/itrap.tcl`; `kill -9` freed
the probe, and the decoder successfully read 31 old crash records. Those records
pre-date the DWT transport-triplet firmware catch, so none had
`arm_cfsr=0xD7170001`.

### Assembly update: core1 tripwire is optimized into a one-time check (2026-06-04)

`run_core1_worker` currently precomputes `shared + offset` pointers into stack
slots once at function entry and reduces the loop pointer check to a cached
boolean at `[sp,#0x94]`. It does not reload and recompare `shared`/`worker` at
the top of every loop. Relevant cached pointers include:

- `[sp,#0x40] = shared + 0x13164` (`live_ppu_snapshot`)
- `[sp,#0x88] = shared + 0x11040` (`sync_snapshot`)
- `[sp,#0x90] = shared + 0x1528c` (`sync_complete`)

That explains the variant-C observation where an assert appears to pass but the
subsequent `RefCell::borrow` faults: the assertion tested the entry-time cached
boolean, while the later deref used a cached stack pointer that could have been
corrupted after the prologue. If keeping this tripwire, force volatile reloads or
make it out-of-line/non-hoistable.

### Confirmed bug, not sole #5 cause: heapless MPMC sequence arithmetic (2026-06-04/05)

`heapless 0.8.0`'s MPMC implementation compares sequence numbers as `i8`:

```rust
let dif = (seq as i8).wrapping_sub(pos as i8);
```

The old queues used capacities 512 and 2048. The codegen showed `uxtb`/`sxtb`
in enqueue/dequeue, so this is a real bug for those capacities and can produce
full/empty misclassification or livelock. It can also let the consumer observe a
cell as ready before a valid producer has written a current payload. For
`Core1Command` that is enum UB: a stale or uninitialized payload can turn into an
invalid cross-core worker command, lost ticket, or mismatched queue state.

This bug was worth fixing, but the later re-check reproduced the pointer smash
after the mitigation. Treat it as a fixed amplifier/noise source, not the final
RCA.

Validation so far:

- Reducing both queues to 64 produced a release image (`0x821458d1`) that ran
  standalone for 6 minutes with `crash_decoder.py --probe --json` reporting an
  invalid sector and `crashes=[]`.
- The current fix keeps `COMMAND_QUEUE_CAPACITY=64` and replaces the
  2048-sample audio `MpMcQueue` with a simple ring protected by the existing
  DrainAudio ticket protocol. That preserves audio buffering without relying on
  oversized `MpMcQueue` sequence arithmetic. Release image `0x017e9080` also ran
  standalone for 6 minutes with the decoder reporting an invalid sector and
  `crashes=[]`.
- A subsequent longer unattended check did capture fresh records, so do not
  close #5 based on the `0x017e9080` clean soak.

### Silent reboot follow-up: reset-cause capture (2026-06-05)

The user-observed failure after the DWT build was a real reboot: the device
returned to the splash screen and loaded the save state. The crash sector was
still invalid with `crashes=[]`, so the previous "no crash record means visual
wedge" conclusion was wrong for that event.

Next diagnostic step implemented in firmware:

- `check_reset_reason` runs at boot before core 1 starts, logs WATCHDOG.reason
  plus POWMAN reset/powerup/interrupt registers, then clears WATCHDOG.reason.
- It commits a crash record for watchdog timer/force resets and unexpected
  POWMAN causes: BOR, watchdog reset paths, SWCORE powerdown, glitch detect,
  and HZD SYSRESETREQ.
- It intentionally does not commit POR/RUN/debugger reset causes, so flashing
  and normal power cycling should not consume crash slots.
- Reset-cause records use `CrashKind::ResetReason`; watchdog timer records keep
  `CrashKind::WatchdogTimeout`. The decoder maps the ARM register fields to the
  raw reset registers instead of symbolizing them.

If the next splash/save-state event still leaves no record, the reset source is
likely POR/RUN/debugger-like, or flash commit is failing before a sector record
can be created. In that case, capture the boot RTT log immediately after the
reboot and compare the printed reset register snapshot.

### User-reported reboot after reset-cause build (2026-06-05)

After flashing image CRC `0x4daf18aa`, the user reported another splash +
save-state reboot. The crash sector was still invalid:

```json
{ "sector": { "valid": false }, "crashes": [] }
```

Live reset-status registers read over SWD after the reboot:

- `WATCHDOG.reason = 0x00000000`
- `POWMAN.chip_reset = 0x00040000` = `had_run_low`
- `POWMAN.current_pwrup_req = 0x00000020` = coresight/debug pwrup request
  present after probe attach
- `POWMAN.last_swcore_pwrup = 0x00000001`
- `POWMAN.intr = 0x00000000`

Interpretation: this reboot did not flow through panic/HardFault scratch, was
not a watchdog timer reset, and did not latch BOR/glitch/sysreset/watchdog
POWMAN bits. The latched reset source is RUN/RSTn being driven low. That points
away from the existing memory-corruption tripwires for this specific reboot and
toward external reset-line hardware, debug/probe reset, or something connected
to the RUN net. Firmware has no GPIO token for RUN; the documented external
brown-out signal is GP18 and is not currently sampled by firmware.

User hardware observation: a male-to-female Dupont jumper was plugged into RUN
on the breadboard, with the female end disconnected. That matches the
`had_run_low` evidence well: the dangling lead can act as an antenna/noisy
capacitive pickup on the reset line and intermittently assert RUN low.

The user then removed that RUN jumper and observed another reset. The signature
remained the same:

- crash sector invalid, `crashes=[]`
- `WATCHDOG.reason = 0x00000000`
- `POWMAN.chip_reset = 0x00040000` = `had_run_low`
- `POWMAN.current_pwrup_req = 0x00000020` after probe attach
- `POWMAN.last_swcore_pwrup = 0x00000001`

So the loose RUN jumper was plausible but not the sole cause. Continue treating
the current reboot as an external RUN/RSTn assertion until a different reset
bit appears.

Firmware follow-up staged locally: record `had_run_low` only when the boot-time
snapshot does **not** show a coresight/debug power-up request. This should catch
standalone RUN/RSTn pulses while filtering the `probe-rs run` reset used for
flashing.

This follow-up was flashed as image CRC `0x7f97687a`. Boot under `probe-rs run`
logged:

```text
watchdog_reason=0x00000000
powman_chip_reset=0x00040000
current_pwrup=0x00000020
last_swcore_pwrup=0x00000001
powman_intr=0x00000004
```

No reset-reason record was committed, and the crash sector remained invalid
with `crashes=[]`, so the debug/probe RUN-low baseline did not pollute slot 0.
The next standalone RUN/RSTn reset should now leave a `ResetReason` record.

Follow-up after more user-observed resets: the filtered build still left the
sector invalid while live POWMAN stayed at `had_run_low`. The boot-time
`current_pwrup=0x20`/coresight bit appears to be present in this setup even
when no `probe-rs` process is active, so the filter was suppressing the reset we
wanted to record.

Firmware changed again:

- `had_run_low` is now recorded unconditionally.
- `write_record_to_flash` now erases the crash sector immediately when the
  sector header is invalid/missing, instead of scanning hidden old record slots
  behind a zeroed `RCLG` header. This makes `--mark-read` a true "next write
  starts fresh" marker and prevents hidden records from participating in dedupe.

Flashed image CRC `0xc125e7dd`. The expected probe/debug reset baseline was
captured as:

```json
{
  "crash_kind": "ResetReason",
  "reset": {
    "watchdog_reason": "0x00000000",
    "powman_chip_reset": "0x00040000",
    "powman_chip_reset_desc": "had_run_low",
    "powman_current_pwrup": "0x00000020",
    "powman_current_pwrup_desc": "coresight_pwrup",
    "powman_last_swcore_pwrup": "0x00000001",
    "powman_intr": "0x00000004"
  }
}
```

Then `crash_decoder.py --mark-read` invalidated the header, but the next verify
immediately found a new valid record:

```json
{
  "crash_kind": "WatchdogTimeout",
  "reset": {
    "watchdog_reason": "0x00000001",
    "watchdog_reason_desc": "timer",
    "powman_chip_reset": "0x00040000",
    "powman_chip_reset_desc": "had_run_low",
    "powman_current_pwrup": "0x00000000",
    "powman_current_pwrup_desc": "chip_reset",
    "powman_last_swcore_pwrup": "0x00000001",
    "powman_intr": "0x00000004"
  }
}
```

Live `WATCHDOG.reason` still read `0x00000001` afterward. Treat this latest
record carefully: it may be a real reproduction during the repeated reset
window, or it may have been induced by the probe/flash-sector mark-read
operation starving or perturbing the watchdog. Either way, the diagnostic state
advanced: the current image can record RUN-low, and the latest committed record
shows a watchdog timer reset rather than the previous `watchdog_reason=0`
RUN-only cases.

### Repeated resets with unfiltered RUN-low recorder (2026-06-05)

After another user-observed reset, the crash sector had grown into an
alternating sequence:

- `Panic multicor:1453` (`report_transport_smash` -> `panic!("core0 transport
  ptrs smashed")`)
- `ResetReason` with `powman_chip_reset=0x00040000` (`had_run_low`) and
  `watchdog_reason=0`

The first decode saw 11 records; a rich-text decode seconds later saw 13
records, meaning the board was still adding reset/panic pairs while being
inspected. Representative panic context:

- file/line: `multicor:1453`
- ROM id prefix: `21f712e2`
- several records at GB `PC=0x03ce`, later records at `PC=0x28c8`, `0x5d41`,
  and `0x5301`
- all panic records report large core-0 stack headroom, so this is not a stack
  overflow symptom

Durable instrumentation follow-up:

- Added `CrashKind::TransportSmash = 4`.
- `report_transport_smash` now records the `Core1Transport` base, corrupted
  `cmd`/`aud`/`shr` pointer triplet, and the first duplicate triplet found in
  SRAM into a global diagnostic before panicking.
- The panic handler consumes that diagnostic and writes a `TransportSmash`
  record with those values in the ARM fields instead of a plain `Panic`.
- The decoder now prints `transport_smash` fields and avoids symbolizing them.

Flashed diagnostic image CRC `0xd96a51e4`. Boot reached the main loop and wrote
the expected probe/reset baseline in slot 13:

```json
{
  "slot_index": 13,
  "crash_kind": "ResetReason",
  "reset": {
    "watchdog_reason": "0x00000000",
    "powman_chip_reset": "0x00040000",
    "powman_current_pwrup": "0x00000020"
  }
}
```

No `probe-rs` process was left attached. On the next `report_transport_smash`
event, look for a new `TransportSmash` record after slot 13; its
`source_triplet` should tell whether the duplicate payload came from the
route/bus-event header, stack, or another SRAM object.

### Latest reboot read: sector full, fresh DMA-copy panics (2026-06-05)

After the user reported another reboot, `pgrep -af probe-rs` was empty and
`crash_decoder.py --probe --elf ../../target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w --json`
read a full sector: `valid=true`, `erase_count=1`, slots 0..30 occupied.

Fresh records since the DWT-retarget baseline:

- slot 25: `Panic`, `panic.file="gameboy."`, `panic.line=449`,
  ROM bank 5, GB `pc=0x4bb4`, `sp=0xdff9`, PPU `ly=31`.
- slot 27: `Panic`, `panic.file="gameboy."`, `panic.line=449`,
  ROM bank 25, GB `pc=0x3916`, `sp=0xdff9`, PPU `ly=28`.
- slot 29: `Panic`, `panic.file="gameboy."`, `panic.line=449`,
  ROM bank 5, GB `pc=0x3b87`, `sp=0xdff7`, PPU `ly=37`.
- slots 26/28/30: reset markers with `WATCHDOG.reason=0` and
  `POWMAN.chip_reset=0x00040000` (`had_run_low`).

`gameboy.rs:449` is the `self.memory.copy_dma_step(source, progress, to_copy)`
call in `advance_dma_bulk`. These records are **ordinary panics**, not the
structured `CFSR_DMA_OAM_GUARD` record, so the explicit destination-slice guard
inside `copy_dma_step` did not fire. The latest symptom therefore points at an
earlier write corrupting either the inline `GameBoy.dma` state or memory/source
copy state before the downstream copy path panics.

Follow-up implemented in this worktree: retarget the firmware DWT from the full
transport triplet to the two aligned words covering `GameBoy.dma`, the
`GameBoy.memory` box field, and the transport `command_tx` field. The cheap
transport triplet software guard remains, but the next DWT hit should identify a
writer PC for the newly dominant DMA/memory symptom.

### DWT retarget false-positive and correction (2026-06-05)

First DMA/memory retarget build:

- CRC `0xa3e58bb8`: watched two aligned words covering `GameBoy.dma`, the
  `GameBoy.memory` box field, and transport `command_tx`.
- Under probe, this immediately halted as a DWT watchpoint at
  `Core1Transport::advance_lcd_timing`.
- Standalone, it filled the sector with alternating reset markers and DWT
  records:
  - `arm_cfsr=0xd7170001`
  - `arm_pc=0x10003948/0x10003950` (`advance_lcd_timing`)
  - `arm_fault_addr=0x200654c8`
  - `arm_hfsr=0xd1000815`

Interpretation: this was not the corruptor. The raw word watch was aligned down
onto a word that also contains legitimate mutable LCD-timing state, so normal
transport timing writes tripped the DMA watch before the emulator did anything
interesting.

Correction implemented:

- `DmaState` is now `#[repr(C, align(4))]` so a word watch on `GameBoy.dma`
  does not share a 32-bit comparator word with adjacent hot fields.
- Dropped the raw DWT watch on transport `command_tx`; the software transport
  pointer triplet guard remains as the fallback for that symptom.
- Active raw DWT targets are now only `dma0`, `dma1`, `GameBoy.memory` box
  field, and an empty fourth slot.
- CRC `0x5721f78e`: after a short standalone check, no new `0xd7170001`
  `0x200654c8` DWT loop records appeared, so the false-positive was removed.

New boot-noise issue encountered while testing: attached/standalone boots began
timing out SD CMD41 during boot save-state/battery reads. Because the watchdog
starts before splash and was only fed in the main loop, this produced watchdog
timer records before the diagnostic could reach a stable game tick. Added
watchdog feeds around the boot-time save-state/battery reads. CRC `0x0753eec8`
then reached `ROM loaded, entering main loop` despite the SD timeouts, but the
attached probe still exited with a bogus-looking low-address exception before
the first DWT target log. Sector contents after that were only watchdog/reset
records, not DWT or memory-corruption records.

Second correction:

- Widened the diagnostic watchdog window to 16 s for startup and main-loop
  feeds, matching the existing ROM-staging watchdog budget.
- CRC `0x83bfd117` reached the first game tick and printed:
  `gb=0x20065328 dma0=0x20065328 dma1=0x2006532c memory_field=0x200654b4 memory=0x20040330`.
- Attached probe halted on the first normal OAM-DMA start in
  `handle_bus_event` (`gameboy.rs:506` / DMA register write), and the prior
  standalone unfiltered run recorded that legitimate transition repeatedly:
  `arm_cfsr=0xd7170001`, `arm_fault_addr=0x20065328`, GB `pc=0x299b`, LY 0.

Final filter added for this round:

- DebugMonitor raw mode now treats slots 0/1 as the aligned
  `Option<DmaState>` words and ignores valid DMA transitions:
  - word0 `0` = `None`
  - word0 `1` and word1 low16 page-aligned source plus progress `<=160` =
    valid `Some(DmaState)`
  - anything else still commits a DWT record.
- CRC `0x32b790ef` flashed. Attached mode still halts at the first DMA
  watchpoint before DebugMonitor can filter (expected with `C_DEBUGEN`), but
  after `probe-rs reset --chip RP235x` detached and a 25 s standalone run,
  `crash_decoder.py --probe --json` showed only reset/watchdog bookkeeping and
  **no new `0xd7170001` DMA-start records**. This is the current build to leave
  running for the next real reboot.

---

## Files touched this session
- `xtask/src/bin/rb-flash.rs` — watchdog disable before flash; whole-image
  (segment-based) CRC stamping into `IMAGE_CRC`.
- `platform/pico2w/src/main.rs` — `integrity::verify_image` (whole-image CRC,
  replaces `.data`-only guard; `IMAGE_CRC` in `.end_block`).
- `platform/pico2w/src/multicore.rs` — Option A (MpMcQueue + critical_section
  restored; `wfe`/`sev` livelock fix; SPSC reverted).
- `core/src/cpu/peripheral/apu.rs` — wave-RAM index `& 0x0F` guard.
- (Earlier in the session, unrelated WiFi work also present in the tree:
  `src/wifi/*`, `src/wifi_codec.rs`, `src/state/wifi_menu.rs`.)

### #5-investigation session (2026-06-03)
- `platform/pico2w/src/stack_probe.rs` — `region_high_water` + `high_water_core0`
  (full-region high-water scan; was a 256 B bottom canary).
- `platform/pico2w/src/multicore.rs` — core-1 `MSPLIM` re-assert; full-region
  paint of `CORE1_STACK`; throttled high-water log (feature `stack-probe`); `#3`
  GB-`PC=0x03ce` trigger trace in `update_crash_context`.
- `platform/pico2w/src/main.rs` — core-0 `MSPLIM` re-assert at entry.
- `platform/pico2w/src/multicore.rs` — `assert_core1_pointers` tripwire (loop
  top + `publish_worker_state` entry) after the cross-core RMW audit cleared the
  torn-RMW theory.
- `platform/pico2w/src/guarded_heap.rs` (+ `lib.rs`, `Cargo.toml`, `main.rs`) —
  `heap-guard` redzone allocator wrapper for catching heap overruns.
- `platform/pico2w/src/multicore.rs` — `Core1Transport::check_shared` core-0
  tripwire on `self.shared`, at every per-tick transport method entry.
- Builds clean default + `--features stack-probe[,heap-guard]`; host tests 191/0.

### #5-investigation session (2026-06-04)
- `core/src/gameboy.rs` — **`advance_dma_bulk` hardened** against a corrupted
  `progress > OAM_DMA_BYTES`: `OAM_DMA_BYTES.saturating_sub(progress)` + early
  `self.dma = None; return` when `remaining == 0`. Stops the downstream
  `memory.rs:410` OOM panic (release `u8` underflow → huge `to_copy` → OOB OAM
  copy) from consuming crash slots, so the r4/r12 + canary diagnostics (and the
  watchpoint) can catch the *earlier* writer. `cargo build -p rustyboy-core`
  clean. This is a **symptom guard**, not the #5 fix.
- Hardware-watchpoint hunt (no firmware change): OpenOCD (RP2350 fork at
  `/tmp/ocd-build/openocd`) cm0-only DWT write-watchpoint on the POOL transport
  word `0x20051830`; scripts in `/tmp/wp.tcl`. See the "HARDWARE WATCHPOINT"
  section. probe-rs's gdb stub can't do watchpoints on RP235x.
- Assembly pass over the current ELF: identified `route_bus_events`'s
  `Vec<BusEvent>` drain/restore as the observed 12-byte rogue-write primitive;
  fixed the TL;DR and added the exact `0x10004044..0x10004880` evidence above.
- `platform/pico2w/src/multicore.rs` — comment-only correction: the optimized
  `Core1Transport` pointer triplet starts at transport offset 4, not 0.

Nothing has been committed; all changes are in the working tree.

### #5-investigation session (2026-06-10) — heapless MpMcQueue i8 overflow deep-dive

#### The bug (heapless 0.8.0)

`MpMcQueue` uses Dmitry Vyukov's bounded MPMC algorithm. The readiness check
computes a *signed* difference between the cell's sequence number and the current
position, to distinguish three states: ready (dif==0), full/empty (dif<0),
contention (dif>0).

In heapless 0.8.0 the comparison is **hardcoded to `i8`** regardless of the
`mpmc_large` feature:

```rust
// enqueue
let dif = (seq as i8).wrapping_sub(pos as i8);
// dequeue
let dif = (seq as i8).wrapping_sub((pos.wrapping_add(1)) as i8);
```

With `mpmc_large` enabled, `AtomicTargetSize = AtomicUsize` and `IntSize =
usize` — the atomic is 64-bit on RP2350 — but the comparison still casts both
operands to `i8` before subtracting. The sequence numbers and position counters
can exceed 255 for large N, so the bottom-8-bit truncation produces wrong
`dif` values.

**Safe capacity limit for this comparison: N ≤ 127.** The Vyukov algorithm
requires that the signed difference fits in the comparison type. For `i8` that
is [-128, 127]; cell sequence numbers advance by N each full cycle, so the
observable difference is up to ±N. At N=128 it barely fits; at N=256 it
overflows.

The const assert in 0.8.0 is:
```rust
Self::ASSERT[!(N < (IntSize::MAX as usize)) as usize];
// With mpmc_large: IntSize = usize → MAX = usize::MAX → passes for any N
```
It checks against `usize::MAX`, not `i8::MAX`. No compile-time guard prevents
N=256 from compiling.

#### Why the original algorithm used a larger signed type

Vyukov's reference implementation used `int` (32-bit on x86), wide enough for
any practical queue. heapless chose `u8`/`i8` to minimize per-cell overhead on
embedded targets (1 byte vs 4). That trade-off is fine for N ≤ 127. When
`mpmc_large` was added for larger queues, the atomic/storage type aliases were
updated but the comparison casts were not — a copy-paste miss.

#### The consequence for our queues

| Queue | N | Bug triggered? |
|---|---|---|
| `COMMAND_QUEUE` | 512 | Yes — far exceeds i8 range |
| `AUDIO_QUEUE` | 2048 | Yes |

A consumer can observe a cell as "ready" (dif==0) when the producer has not yet
written a valid payload. For `Core1Command` (a Rust enum) this is **enum UB**:
reading an uninitialized or stale discriminant can produce any variant and any
field bytes. The resulting garbage command dispatched on Core 1 can write to
arbitrary memory — including the `.data` thunk table.

#### Fix in heapless 0.9.0

The changelog entry is explicit:

> **Fixed `MpMcQueue` with `mpmc_large` feature.**

In 0.9.x the type aliases become:
```rust
#[cfg(feature = "mpmc_large")]
type IntSize = isize;   // was: hardcoded i8 in comparison
#[cfg(not(feature = "mpmc_large"))]
type IntSize = i8;
```
And the comparison uses `IntSize` throughout:
```rust
let dif = (seq as IntSize).wrapping_sub(pos as IntSize);
```
With `mpmc_large` this is `isize` (64-bit on RP2350), so any practical queue
capacity is safe. heapless 0.9.2 is already present in `Cargo.lock` (pulled in
by another dependency); the platform crate pins `"0.8"`.

heapless 0.9.2 also **deprecates** the entire `mpmc` module (issue #583),
signalling that the design is considered flawed and users should migrate to
other primitives.

#### Is this a known / documented issue?

Yes — it was fixed as a named bug in the 0.9.0 release. The GitHub issue #583
discusses deprecating the module. Before the fix landed, it was not documented
in any warning or doc comment in 0.8.x, so it was a silent bug.

#### Recommended action

Upgrade `heapless` to `"0.9"` in `platform/pico2w/Cargo.toml`. The 0.9.x
breaking changes affecting us:
- `Q2`/`Q4`/…/`Q64` type aliases removed → use `MpMcQueue<T, N>` directly
  (already done).
- `Drop` impl added for `MpMcQueue` → static queues unaffected.
- `mpmc` marked deprecated → will produce a warning; acceptable for now.

Alternatively, cap `COMMAND_QUEUE_CAPACITY` to ≤ 127 as a short-term
mitigation (proved to stop the crash for ≥ 6-minute runs in the 2026-06-04
session), then upgrade heapless in a follow-up.

==================================================================
**CORRUPTOR IDENTIFIED (2026-06-11) — BusEvent scratch buffer on hot stack.**
==================================================================

### DWT standalone-run methodology

All prior DWT runs were under `probe-rs run` which holds C_DEBUGEN set. With
C_DEBUGEN set, DWT watchpoint hits cause the CPU to HALT (caught by probe-rs)
instead of firing the DebugMonitor exception. To get DebugMonitor to fire (and
write a crash record) the firmware must run WITHOUT the debugger attached.

Workflow used:
1. `STAMP_ONLY=1 cargo run --release` — CRC-stamps the ELF but skips flashing.
2. `probe-rs download --chip RP235x --speed 1000 <elf>.rbcrc` — flashes silently
   and exits, clearing C_DEBUGEN on session Drop.
3. `probe-rs reset --chip RP235x` — resets target and exits, C_DEBUGEN stays 0.
4. Firmware runs standalone; DebugMonitor fires on DWT hit and writes a crash record.
5. `crash_decoder.py --probe --elf <elf>` reads the record.

`STAMP_ONLY` support was added to `xtask/src/bin/rb-flash.rs`.

### Watch configuration

`multicore.rs` armed DWT in `WATCH_MODE_STACK_LR` mode watching `0x2007EACC × 4`
comparators. The DebugMonitor handler skips records for Thumb (odd) values and
values ≤ 0xFFFF (legitimate LR pushes are odd; the corrupt write is even and
≥ 0x10000). The victim address `0x2007EACC` was derived from crash #31's sp_before.

### DWT hit — crash record (crash #2 of the cleared log)

```
ARM PC = 0x1002e396 = __aeabi_memcpy
ARM LR = 0x1000316b = core::ptr::copy_nonoverlapping
Stk R0 = 0x2007eacc          ← memcpy DESTINATION = the watchpoint address
```

The DWT fired on a WRITE to `0x2007EACC`, with memcpy as the writer.

### Full call chain (llvm-addr2line on 0x1000316b)

```
embassy_main_task (main.rs:649)
  → RunningState::tick (running.rs:53)
    → PicoGameBoy::tick (multicore.rs:1225)
      → GameBoy::tick (gameboy.rs:222)
        → GameBoy::route_bus_events (gameboy.rs:483)
          → GameBoyMemory::drain_into_slice (memory.rs:784)
            → BusEventQueue::drain_into_slice (memory.rs:56)
              → [u8]::copy_from_slice → copy_nonoverlapping → __aeabi_memcpy
```

The **entire chain is inlined** into `embassy_main_task_inner_function` at
`0x100002d4`. Its prologue: `push {r4-r7,lr}` + `push.w {r8-r11}` +
`sub.w sp, sp, #5312` = **5348-byte stack frame**.

At `0x10002ec8`: `add.w r11, sp, #2000`
→ r11 = SP + 2000 = the base of the `events` array from:
```rust
let mut events = [BusEvent { address: 0, value: 0 }; BUS_EVENT_QUEUE_CAP]; // 256 bytes
```

At runtime: SP + 2000 = **0x2007EACC** = the watchpoint address.

The `events` array (`let mut events` in `route_bus_events`) therefore occupied
`[0x2007EACC, 0x2007FACC)` on the stack. The address `0x2007EACC` is also the
**saved LR slot** of some other function at a shallower stack depth. When the
embassy main task polls, the frame is allocated fresh (SP decremented by 5348);
when it yields, the frame is freed. Code running at shallower depth (embassy
executor callbacks, timer tasks, etc.) can use `0x2007EACC` as a saved LR.
If the main task's next poll fills `0x2007EACC` with BusEvent bytes from
`drain_into_slice`, those bytes are later loaded as a PC → INVSTATE / IBUSERR.

BusEvent #0 at [0x2007EACC]:
- bytes = (addr_lo, addr_hi, value, padding) = (addr[0], addr[1], value, ?)
- e.g. address=0xFE9E, value=0x00, padding=0x00 → word = **0x0000_FE9E** (≤ 0xFFFF,
  DWT filter skips → firmware continues → IBUSERR crash later, recorded as crash #31)
- or address=0x8000, value=0x02, padding=0x20 → word = **0x2002_6E8C** (even >0xFFFF,
  DWT filter triggers → DebugMonitor records it as crash #2)

### Fix applied (2026-06-11)

`core/src/gameboy.rs`: added `bus_event_scratch: [BusEvent; BUS_EVENT_QUEUE_CAP]`
as a field of `GameBoy<W>`. `GameBoy` is `Box`-allocated (heap) in the pico2w
platform → heap address ~0x2003xxxx, never overlapping the MSP stack.

`route_bus_events` changed from:
```rust
let mut events = [BusEvent { address: 0, value: 0 }; BUS_EVENT_QUEUE_CAP]; // stack!
let event_count = self.memory.drain_into_slice(&mut events);
```
to:
```rust
let event_count = self.memory.drain_into_slice(&mut self.bus_event_scratch); // heap!
```

This eliminates the 256-byte stack allocation from the hot frame. Confirmed by
disassembly of the new build: `add.w rN, sp, #2000` pattern is completely absent;
all events-array accesses are `[r10, #offset]` (r10 = GameBoy self pointer, heap).

All 14 core crate tests pass. Firmware cross-compiles clean for thumbv8m.main-none-eabihf.

**Status: fix built and confirmed in disassembly. Pending hardware flash + runtime
verification per [[feedback-flash-before-claiming-fix]].**

### Critical gap: the `bus_event_scratch` fix is a layout shift, not an RCA (2026-06-11)

The DWT catch (crash #2) recorded `ARM PC = __aeabi_memcpy`, `LR = copy_nonoverlapping`,
destination = `0x2007EACC`. The other agent concluded that BusEvent bytes left on the
dead stack after a poll yield were later loaded as a PC by a shallower function. That
mechanism is **physically impossible** in cooperative single-threaded execution: any
function saving an LR at `0x2007EACC` does so with a prologue `push {…,lr}`, which
**overwrites** whatever stale bytes were there before the LR is ever loaded. The stale
BusEvent bytes cannot survive to be popped as PC.

The DWT crash #2 record is a **false positive**: the STACK_LR filter read the *written
value* (BusEvent bytes at `events[0]`, e.g. `address=0x8000` → word with bit 0 = 0 and
value > 0xFFFF) and recorded it as the corruptor. This `sys_reset` fired before the
actual corruptor could produce its own record. Every prior DWT iteration that
"never caught the corruptor" was suffering from this: the filter fires on the legit
drain, resets, and the real writer never gets a turn.

**What the fix actually does:** moving `events` from the hot frame to a `GameBoy` field
removes the source of the false positive by making the drain write to the POOL region
instead of the MSP stack, so the DWT at `0x2007EACC` can no longer false-positive.
It is good hygiene regardless; the POOL address also avoids the stack-layout-sensitive
bug class entirely for this buffer. But it is a layout shift — the real corruptor
(which writes GB-address values into host pointer/return-address slots across POOL,
CORE1_WORKER, and the MSP stack) has not been identified.

**Evidence:** every confirmed victim spans distant regions simultaneously — POOL
transport triplet, CORE1_WORKER `apu.sample_buffer` Vec header, `live_ppu_snapshot`
RefCell borrow flag, core-0/1 stack return addresses. A simple stack BusEvent-bytes
overflow hits only one victim at one address; the corruptor hits many addresses. The
writer is something that scatters GB-address-valued bytes using a wild pointer, on
either core.

### DWT filter corrected to geometry-based (2026-06-11)

Root cause of every prior false-positive DWT record: the STACK_LR filter read the
*written value* and skipped on `written & 1 == 1` (Thumb) or `written <= 0xFFFF`
(GB constant). This allowed any BusEvent write where the address word happened to be
even and > `0xFFFF` to trigger a record and reset.

**Fix (handler.rs):** replaced the value filter with a geometry check on `sp_before`:

- **Skip** if: core 0 AND `watched_address >= sp_before` — the write is into the
  writer's own live frame or a caller's frame (normal prologue push, struct store,
  or the legit drain). Stack grows downward; `sp_before` is the bottom of the live
  frame, so `addr >= sp_before` means "inside the frame."
- **Record** if: core 1 (cross-core write — core 1's SP is in `CORE1_STACK`
  at `0x2008xxxx`, always above any core-0 victim, so `addr < sp_before` on core 1),
  OR if the write is to an address below the writer's own SP (write outside the live
  frame from core 0 — suspicious).

This filter is value-independent and layout-independent. It correctly ignores the
drain memcpy (writes into its own live frame) and correctly catches a cross-core store
from core 1 that scatters GB bytes into core-0's stack or POOL.

### Watch target changed to `worker.ppu` Box pointer field (2026-06-11)

Previous target: hardcoded `0x2007EACC` (stack LR slot from crash #31). That address
is layout-specific and shifts with every image rebuild; the `bus_event_scratch` fix
reduces the frame by 256 bytes, putting `0x2007EACC` at a different relative position.

**New target (multicore.rs):** `worker.ppu_box_field_addr_for_diagnostics()` — the
address of the `Box<PpuWorkerState>` pointer field inside `CORE1_WORKER` static. This
field is:
- Written **exactly once** at construction (`GameBoyWorker::init_in_place`), before
  `run_core1_worker` is entered and before the DWT is armed.
- Never written by any normal code path afterward.
- A confirmed previous victim (crash records showed `worker.ppu ≈ 0xf5006042`).

Watching it in **raw word mode** (no filter): any write after we arm = the corruptor.
The DebugMonitor geometry filter in raw mode skips all non-STACK_LR modes anyway, so
this comparator fires on every write and records the writer PC unconditionally.

### Remaining open investigation items (2026-06-11)

1. **Flash and soak.** Build and flash the new image. Play to reproduce. The first
   DWT record should now be the real corruptor rather than the drain false positive.

2. **DMA channel dump.** The cyw43 WiFi PIO/SPI RX DMA writes RAM and is invisible
   to all CPU-side watchpoints. A DMA channel with a corrupted `WRITE_ADDR` register
   could scatter GB-valued words across RAM without triggering any CPU DWT. Add a
   quick dump of all active DMA channel `WRITE_ADDR`/`CTRL_TRIG` registers to the
   crash record (read from `0x50000000 + n*0x40`). This closes the only remaining
   blind spot. Cheap: 12 DMA channels × 2 registers = 24 word reads, can fit in an
   existing diagnostic tail.

3. **Identify the corruptor class.** Once the writer PC is recorded, determine:
   - Flash address → `addr2line` → which function owns the store instruction.
   - If it's core-0 application code: audit the store's base register source (r4/r12
     from the extended regs) to find how a GB address ended up there.
   - If it's core 1: the base register was a `shared`/`worker` pointer corrupted
     earlier; trace how *that* pointer got the GB value (the `shared` pointer smash
     records showing base ≈ `0x407E` GB ROM address are the clearest prior evidence).
   - If `arm_pc` is inside `__aeabi_memcpy`/`RawVec`/embassy internals: the base
     register contains the corrupt destination address; determine what provided it.

4. **ACCESSCTRL SRAM firewall + MPU read-only.** Structural backstop that converts
   any code-execution from a corrupted PC in `.data`-resident RAM code into a precise
   MemFault with a writer PC, rather than unpredictable behaviour. Not needed to find
   the root cause, but provides a safety net once identified.

---

## #5 investigation continued (2026-06-12) — Core 1 MPU experiment + DMA ruled out

### Context

After the 2026-06-11 session (see "CORRUPTOR IDENTIFIED" and retraction above),
the DWT was re-armed in RAW mode on `worker.ppu_box_field_addr_for_diagnostics()`
(`0x20004598`) — a field that is written exactly once at construction and should
never change. The hypothesis entering this session: the corruptor is Core 1's CPU
executing wild code and writing to Core 0's stack LR slots.

### Experiment: Core 1 PMSAv8-M MPU protecting Core 0's stack

Added `setup_core1_mpu()` called from `run_core1_worker` (immediately after the
MSPLIM assertion). Configured Core 1's MPU region 0:

```
RBAR = 0x20066B7B  (BASE=0x20066B60, XN=1, AP=10=priv-RO, SH=11=inner-shareable)
RLAR = 0x2007FFE1  (LIMIT=0x2007FFFF, AttrIndx=0, EN=1)
MPU_MAIR0 = 0xFF   (Normal, write-back, read/write-allocate)
MPU_CTRL = 0x05    (ENABLE | PRIVDEFENA — background full-access for all other addresses)
HFNMIENA = 0       (MPU off in HardFault so crash handler can read/write freely)
```

Rationale: any write by Core 1's CPU to Core 0's stack range (`0x20066B60–0x2007FFFF`)
fires MemManage → escalates to HardFault (MEMFAULTENA=0) → existing handler records
stacked PC = exact corrupt store instruction. CFSR.DACCVIOL would be set.

Flashed using STAMP_ONLY methodology (no probe attached during soak).

### Result: 7 crashes, zero DACCVIOL — Core 1 CPU is definitively NOT the corruptor

After a multi-hour soak, `crash_decoder.py --probe` read 7 new records (fresh flash
page, erase_count=1 — the crash page was erased during the firmware flash). Every
crash showed the same LR-slot corruption pattern as before; **not one record had
CFSR.DACCVIOL**. The crash badge never appeared on screen — the firmware was
crashing during boot before the badge display sequence could run.

**This definitively rules out Core 1's CPU as the corruptor.** If Core 1 were writing
to any address in `0x20066B60–0x2007FFFF`, Core 1's MPU would fire. It did not.

### DMA analysis: channels 2–6 have WRITE_ADDR=0 (unused)

The crash decoder skips DMA channels with WRITE_ADDR=0 (`if addr == 0: continue` in
`crash_decoder.py:800`). In all 7 new crashes the DMA section shows only:

```
DMA ch0  WRITE_ADDR=0x50200010   (PIO0 SM0 TX FIFO — audio/WiFi)
DMA ch1  WRITE_ADDR=0x40088008   (I2S peripheral)
```

Channels 2–6 = 0x00000000. Both active channels write to peripheral registers,
not SRAM. DMA is an increasingly unlikely suspect, though it cannot be fully ruled
out: a DMA transfer that briefly writes to SRAM and completes before the downstream
fault fires would leave WRITE_ADDR at the post-transfer peripheral address.

### Crash #2 notable: stacked R0 = victim LR slot address

Crash #2 had an unusually corrupted state:
```
CFSR     0xd7170001  (reserved bits set — CFSR value itself is corrupted)
Stk R0   0x2007eacc  (stacked R0 — this is one of the victim LR slot addresses)
SP_bef   0x2002b0ac  (Core 0's SP was in the heap region, not the stack)
```

Stk R0 = `0x2007eacc` means some code was running with R0 holding a Core 0 stack
address at the moment of exception. This is consistent with a pattern like
`STR Rx, [R0]` where R0 was loaded with a stack slot address. The CFSR with reserved
bits set suggests the CFSR register itself was corrupted by the same writer.

### LR slot victim addresses (consistent across all firmware versions)

The corruption consistently hits Core 0's stack in a specific region:
- `0x2007e9fc` — seen in crash #4 (LR zeroed, Fault@=0xFFFFFFFE)
- `0x2007ead4` — seen in crash #3 (written with `0x2007eb10`, INVSTATE — executing from stack)
- `0x2007ea54` — seen in crash #6 (LR zeroed, Fault@=0x4f21d8b9)
- `0x2007eacc` — seen in crash #7 (written with `0x0000FE9E`, IBUSERR)

The value `0x0000FE9E` in LR slots is **extremely consistent** across firmware versions
(13 of 22 crashes in the older firmware set, and crash #7 in the new set). See the
2026-06-04 analysis: `0xfe9e` ∈ GB OAM address space (`0xFE00–0xFE9F`). This is a
GB-address-valued word written over a host return address — consistent with the
"GB-derived value used as host pointer" pattern identified earlier. The corruptor
is still writing GB-address-space values into host return address slots.

### Current state

All three hardware corruptor hypotheses have now been tested:
- **Core 0 CPU**: Core 0 MPU tested earlier — no violations.
- **Core 1 CPU**: Core 1 MPU tested this session — no DACCVIOL in 7 crashes.
- **DMA ch0/ch1**: both write to peripheral FIFOs, not SRAM; ch2–6 unused.

The corruptor bypasses all of these. Remaining possibilities:
1. Core 0 is the writer but Core 0's MPU couldn't catch it writing to its own stack.
2. DMA channel with brief SRAM write that completes before crash snapshot.
3. Some other AHB bus master (CYW43 WiFi PIO-based SPI RX?).

### Updated next steps

1. **Search firmware ELF for the constant `0x0000FE9E`** — this value is too consistent
   to be random. If it appears as a literal in the program data or ROM tables it
   identifies the code path producing the corrupt value.

2. **Audit Core 0's own code paths** for a dangling pointer or stack-address escape.
   The stacked R0 = victim address in crash #2 suggests some code computes a stack
   address into R0 and then stores through it. A use-after-return of a `&local` passed
   to an async closure or callback is the classic pattern. Focus on Embassy timer
   callbacks, SPI completion handlers, and any closure capturing a reference to a
   stack variable.

3. **Arm Core 0's DWT on the victim address** to determine if Core 0 is the writer.
   In raw mode on `0x2007eacc`, it fires on every write — which will include normal
   function prologue pushes. The STACK_LR geometry filter (skip if writer==core0 and
   addr >= sp_before) handles this. If Core 0 IS the writer, the DWT will catch it.
   If DWT on Core 0 also never fires, then only DMA or another bus master remains.

4. DMA snapshot now extended to capture all 16 channels (ch0–ch15). The crash record
   format still only stores ch0–ch6 (unchanged), but ch7–ch15 are logged via defmt
   at the next boot. If CYW43 WiFi SPI RX (suspected to use ch8–ch11) was writing
   to Core 0 stack, it will appear as a `crash: DMA chN: WRITE_ADDR=0x2007...` warning
   at boot after the next crash.

---

## #5 investigation continued (2026-06-12) — crash pattern analysis + DMA ch7-15

### Crash log analysis

Decoded 24 crash records from crash_log.txt (firmware git=71713c6b, old firmware).
**14 of 24 crashes** show the **identical** pattern:

```
ARM PC   0x0000FE9E  → not in firmware (IBUSERR: fetch from GB OAM address space)
ARM LR   0x2000048F  → maps to set_r8_enum in current ELF (may differ in old fw)
CFSR     0x00000100  IBUSERR
GB CPU   PC=0x03CE  SP=0xDFFF  AF=0x0080  BC=0x00FF  DE=0x5800  HL=0xFFAA
```

The GB CPU state is **byte-for-byte identical** across all 14 crashes (same cycle count phase,
same game state at frame start), consistent with a deterministic race that fires at the same
point in the emulation loop.

Other crash types seen in the same log: BSS execution (PC in BSS region, LR=0x00000001), cyw43
init/UNALIGNED faults (crash #9, #11, #21), sio.rs panic (#19), WiFi driver drop_in_place (#22).

### Key observation: 0x0000FE9E is a BusEvent value

`BusEvent { address: u16, value: u8 }` with no repr annotation has size=4 on ARM (u16 at offset 0,
u8 at offset 2, 1-byte padding). For address=0xFE9E, value=0x00, padding=0x00:

    bytes = [0x9E, 0xFE, 0x00, 0x00]  →  u32 LE = 0x0000FE9E

`0xFE9E` is GB OAM address 0xFE9E = sprite 39's tile number slot. The game writes byte value
0x00 to that OAM slot (clearing/hiding sprite 39). The BusEvent for this write, if it landed
at a Core 0 stack return-address slot, would corrupt that return address to 0x0000FE9E.

### LR=0x2000048F is NOT a valid set_r8_enum call site

All BL calls to set_r8_enum in the current ELF produce return addresses:
0x200001B1, 0x200001D5, 0x200001E7, 0x200009A1, 0x20000ED3, 0x200010B7

None is 0x2000048F. The crash decoder maps 0x2000048F to set_r8_enum only because it falls
within the function's address range in the **current** ELF. The old firmware (git=71713c6b)
likely has different .data layout. The consistent LR value is likely either:
- The actual CPU LR at crash time, set by a BL in the old firmware at a different address
- Or: a value restored from a corrupted stack slot via pop/ldmia

### DMA ch7-ch15 extension (2026-06-12)

Extended `DMA_CRASH_SNAPSHOT` from 9 to 18 words and `capture_dma_snapshot` loop from 8
to 16 channels. Changes in `check_and_commit` now log ch7–ch15 via defmt at boot (not stored
in flash record). Newly flashed — waiting for first crash to see high-channel data.

---

## #5 investigation continued (2026-06-12) — DMA fully ruled out + bus_write layout

### DMA ch4–ch15 are never allocated

`bind_interrupts!` in `driver.rs` only registers handlers for ch0–ch3. No code ever
calls `Channel::new(DMA_CH4..DMA_CH15, …)`. Channels 4–15 are never allocated and
cannot have a valid WRITE_ADDR. **All 16 DMA channels are definitively ruled out as
the corruptor.**

### WiFi DMA channels (ch2/ch3) do not reach victim addresses

Confirmed from the actual cyw43-pio source (git revision c722d94, the compiled version):

- ch2 = WiFi TX → writes to PIO1 TX FIFO (peripheral address, not SRAM)
- ch3 = WiFi RX → writes into `cyw43_task` POOL at `0x20003e08`

The `cyw43_task` POOL is ~270 KB below the victim addresses (`0x2007e???`). DMA
physically cannot reach the victim. The DMA snapshot at the next crash will show
ch7–ch15, but this is now mostly academic — DMA is not the corruptor.

### bus_write disassembly: GameBoyMemory struct layout

Traced the full `bus_write` function (compiled into `.data` at `0x20000310`). The
compiler-optimized `GameBoyMemory` layout, reverse-engineered from the IO write path
(`strb [r0 + 0x4100]` where r0 = GameBoyMemory + sign_extend_16(addr)):

| Offset | Field | Size |
|--------|-------|------|
| 0x0000 | vram [u8; 0x2000] | 8192 |
| 0x2000 | wram [u8; 0x2000] | 8192 |
| 0x3F00 | io [u8; 0x80] | 128 |
| 0x3F80 | (other fields) | ... |
| 0x4000 | oam [u8; 0xA0] | 160 |
| 0x40A0 | hram / other | ... |
| **0x4120** | **cartridge: Box<dyn Cartridge>** (data_ptr) | 4 |
| **0x4124** | **cartridge: Box<dyn Cartridge>** (vtable_ptr) | 4 |
| 0x4128 | rom window cache fields | ... |

The `io` array does NOT end right at 0x4120 — the compiler reordered fields. The
`cartridge` fat pointer is at 0x4120 with the vtable at 0x4124.

### Dominant crash mechanism (14 of 24 in old firmware log)

In `bus_write`, the MBC dispatch path at `0x200003b6`:
```
ldrd r9, r6, [r4]    ; r9 = cartridge.data_ptr, r6 = cartridge.vtable_ptr
ldr r3, [r6, #0x14]  ; r3 = vtable.write method
blx r3               ; if vtable_ptr is wrong SRAM addr → PC = garbage
```

If `GameBoyMemory + 0x4124` (vtable_ptr) is corrupted to a SRAM address X where
`[X + 0x14]` = `0x0000FE9E`, the branch to r3 causes IBUSERR. The bus_write stores
after MBC return write to offsets +8 through +20 (NOT +4), so bus_write itself
cannot corrupt the vtable_ptr.

### LR=0x2000048F is probably a valid return address in old firmware

The crash decoder uses the *current* firmware's symbol table to decode crashes from
old firmware (git=71713c6b). In the old firmware, `0x2000048F` was likely a valid
return address from a `bl bus_write` in the SM83 dispatch — it only falls inside
`set_r8_enum` in the *current* .data layout. LR is probably not corrupted; only the
vtable pointer at `GameBoyMemory + 0x4124` is the corruption target.

### Status

- **All DMA channels**: ruled out
- **Core 1 CPU**: ruled out (MPU, 7 crashes, zero DACCVIOL)
- **Core 0 CPU**: cannot catch Core 0 writing to its own stack via MPU; not ruled out
- **Corruption target**: `GameBoyMemory + 0x4124` (cartridge vtable pointer)
- **Mechanism**: some code writes a SRAM address to that word; `bus_write` then
  dereferences it as a vtable, reads `0x0000FE9E` at vtable+0x14, branches → IBUSERR

### Next steps

1. **Software vtable guard in `PicoGameBoy::tick()`**: check each tick that
   `GameBoyMemory.cartridge`'s vtable pointer is ≥ `0x10000000` (valid flash). Log the
   corrupted value via defmt the moment it goes wrong. No DWT needed, no timing shift.
   The existing `EXPECTED_GAMEBOY_MEMORY_PTR` guard checks the Box pointer, not the
   contents — this would guard the *contents*.

2. **DWT on `GameBoyMemory + 0x4124`** at runtime: arm after construction, geometry
   filter to skip self-writes. First write that is not the initial construction =
   the corruptor. This is layout-dependent (address shifts with each build) so needs
   runtime derivation.

3. **Investigate Core 0 for GB-address-to-host-pointer paths**: the corruptor writes
   a value that causes `[X+0x14]` = `0x0000FE9E`. X is some SRAM address. Something
   on Core 0 computes that SRAM address as a function of GB data (BusEvent, DmaState,
   etc.) and stores it into the cartridge fat pointer.

---

## #5 investigation continued (2026-06-12) — boot-window clustering + WiFi ruled out

### Crash timing: boot-clustered, not gameplay-triggered

User observation: the most recent crashes happened soon after boot. An 8-hour soak
after the agents' recent changes (bus_event_scratch heap move etc.) produced no crash.

This is consistent with the notes: the poisoned save state resumes at ~2.39 B cycles and
all captured records show `cycle_lo=2,395,577,292` — only a few million cycles past the
save point. The "deterministic trigger" at `PC=0x03CE` is simply the state the save
resumes into, firing within a second or two of boot, not after minutes of gameplay.

**The 8-hour soak is not a valid test for a boot-clustered crash.** One soak = one boot
= one trial. If the corruption probability is concentrated in the first ~60 seconds, a
single clean soak proves nothing. The earlier 6-minute clean soaks on image `0x017e9080`
showed the same pattern before later reproducing. Do not treat the soak as a fix
confirmation.

**Implication for test methodology.** Replace open-ended soaks with a **reboot-loop
harness**: `probe-rs reset` (standalone, no debugger), wait ~90 s, run
`crash_decoder.py --probe --json`, log, repeat. This gives ~30–40 trials per hour instead
of one per soak. An A/B across old/new images with N≥20 trials each is the minimum
meaningful test.

### WiFi as a corruptor: likely ruled out

User confirmed going back to an old PR (pre-WiFi) produced the same crashes. The
`crash_log.txt` records all carry `git=71713c6b` = "Add crash reporting and save state
format documentation", which predates commit `fd1bb00` (WiFi captive portal work) in
the repo history.

**Caveat:** `build.rs` stamps `git rev-parse HEAD` with no dirty-tree flag. Some of the
22 records in that log carry crashes symbolizing coherently to `cyw43::runner::init` and
`wifi::driver::configure`, which cannot appear in a clean `71713c6b` build — they came
from dirty-tree builds with WiFi present in the working tree. The git stamp alone cannot
distinguish "clean old-PR build" from "dirty-tree session build at the same HEAD."

**To make the WiFi-out ruling airtight (one-time verification, low cost):** clean
checkout of `71713c6b`, confirm the ELF contains no `cyw43` symbol (`nm | grep cyw43`),
flash, run the reboot-loop, and observe a `0x0000FE9E` record. One such record from a
provably WiFi-free image closes this permanently. Without it, WiFi remains "likely out"
rather than "definitively out."

**Assuming WiFi is out**, the suspects list is:
- **All DMA channels**: ruled out
- **Core 1 CPU**: ruled out (MPU experiment, 7 crashes, zero DACCVIOL)
- **Core 0 CPU**: not yet ruled out; cannot catch Core 0 writing to its own stack via MPU
- **WiFi/cyw43**: likely ruled out by old-PR repro (see caveat above)
- **Corruption target**: `GameBoyMemory + 0x4124` (cartridge vtable pointer)

### RAM-resident hot code as an additional hypothesis

`bus_write` lives at `0x20000310` in `.data` (RAM-resident code). The CRC guard
verifies flash at boot but does **not** verify the SRAM copy of the code after it is
copied from flash. If the `.data` region itself is corrupted at runtime, audited store
instructions can silently go wrong — which would explain why every source-level audit
comes back clean.

Supporting evidence: crash #2 from the MPU-soak session had `CFSR` with reserved bits
set (the CFSR register itself was corrupted) and `SP_before=0x2002b0ac` (Core 0's SP
was in the heap region, not the stack). Both are signatures of the CPU having gone wild
before the fault frame was committed — consistent with executing corrupted code, not
clean code with a bad operand.

### Updated priority next steps (revised from above)

These supersede the three steps at the end of the previous section:

1. **Tick-time vtable guard + RAM-code CRC** — two cheap per-tick checks, no DWT,
   no layout sensitivity:
   - Check `GameBoyMemory.cartridge` vtable ptr ≥ `0x10000000` (valid flash range). Log
     and record the moment it goes wrong. This is the front-door check for the dominant
     crash signature.
   - Snapshot a CRC (or a few sentinel words) of the `.data` code region at boot and
     recheck each tick. A mismatch means RAM code was corrupted, not application data —
     the two hypotheses produce different failures here, so one capture discriminates them.

2. **MPU read-only on the `.data` / RAM-code region** — arm Core 0's MPU region to
   make `0x20000000..end_of_.data` non-writable after the copy-from-flash startup.
   Any write to that region from Core 0 fires DACCVIOL with the exact writer PC. Unlike
   the stack-range MPU experiment, Core 0 never legitimately writes its own code, so
   there are no false positives and no filter needed. This is the single most decisive
   trap if the RAM-code hypothesis is correct.

3. **Reboot-loop harness as the standard test** — script: `probe-rs reset`, wait 90 s,
   `crash_decoder.py --probe --json`, repeat. Use N≥20 trials for any A/B comparison.
   Do not use open-ended soaks to confirm fixes for a boot-clustered bug.

4. **DWT on `GameBoyMemory + 0x4124`** (runtime-derived, geometry filter) — confirmatory
   once the vtable guard names the corruption window. If the vtable guard says the word
   changed but the DWT never fires, the write didn't come from a CPU store — which points
   back at corrupted code executing a store through a bad operand.

5. **Clean old-PR WiFi-out verification** — one reboot-loop run on a provably WiFi-free
   `71713c6b` build. Low cost, closes the last exotic-bus-master question permanently.

---

## #5 investigation continued (2026-06-13) — valid current-image fault capture

### Crash-record review and symbol-mapping correction

The flash crash sector contains 31 records, all stamped `git=fd1bb003`. They are
not records from the current dirty-tree image, and current-ELF symbolization of
their addresses is invalid. The sector is full, so current faults cannot append
new flash records. A raw backup was saved as
`/tmp/rustyboy-crash-sector.bin`.

The first current-image fault instead survived in watchdog scratch:

```
PC=0x200024bc LR=0x1001a6e5 CFSR=0x00000001 HFSR=0x40000000
```

Disassembly showed `0x200024bc` was the first instruction of
`GameBoy::read_worker_output`. This was not application corruption: Core 0's
MPU RBAR had been encoded as `0x2000000b`, which sets XN on PMSAv8-M. Executing
RAM code in the protected `.data` region therefore caused IACCVIOL.

The same RBAR field misunderstanding affected Core 1:

```
old Core 0 RBAR = 0x2000000b  (wrong: XN=1)
new Core 0 RBAR = 0x2000001c  (SH=11, AP=10 privileged-RO, XN=0)

old Core 1 RBAR = 0x20066b7b  (wrong AP/SH/XN combination)
new Core 1 RBAR = 0x20066b7d  (SH=11, AP=10 privileged-RO, XN=1)
```

Therefore the earlier conclusion "Core 1 CPU ruled out by seven MPU-protected
crashes" is invalid. Those runs did not have the intended MPU permissions.

### DWT instrumentation removed

After fixing the MPU encodings, an attached run halted after tick 0 with
`CFSR=0`, `HFSR=0`. DWT comparator 0 was still armed on the worker PPU field and
had matched a legitimate write. This was another instrumentation-induced stop.

The runtime no longer publishes or rearms DWT watches. Both cores call
`dwt_watch::disarm_for_current_core()` during startup to clear comparator state
that can survive a warm reset. The same worker PPU field remains available as
ordinary software-checkpoint metadata, so load-state and tick-0 logs still
verify its value without a hardware watchpoint.

### Definitive current-image HardFault

Image CRC `0xba7f05b0` repeatedly reached every load-state checkpoint and both
tick-0 checkpoints with stable values:

```
GameBoy memory = 0x20026184
cartridge vtable = 0x100322ac
worker.ppu field @ 0x200038d0 = 0x2002b444
```

Using an in-session breakpoint at the current image's HardFault vector captured
the exception before the handler or a second debugger connection could resume
the target:

```
HardFault LR       = 0xfffffff9
exception SP       = 0x2007eab0
stacked R0         = 0x00000004
stacked R1         = 0xffffff80
stacked R2         = 0x00000000
stacked R3         = 0x20000045
stacked R12        = 0x1001443f
stacked LR         = 0x20000087
stacked PC         = 0x0000fe9e
stacked xPSR       = 0x29000000
CFSR               = 0x00000100 (IBUSERR)
HFSR               = 0x40000000 (FORCED)
```

`0x20000087` is a valid Thumb return address in RAM-resident
`Instructions::rotate_accumulator`. It follows:

```
0x20000082: bl  rr_u8
0x20000086: b   0x20000090
```

`rr_u8` is a leaf:

```
0x1001443e: push {r7,lr}
...
0x1001445e: pop  {r7,pc}
```

At this stack depth its saved LR slot is `0x2007eacc`. The function contains no
calls and no stores between push and pop, yet its return loaded
`0x0000fe9e`. This is direct evidence that another execution context overwrote
the live Core 0 LR slot while `rr_u8` was active. It is not stale data from a
previous stack frame.

### Hardware-source checks at the exact fault

Core 1's corrected MPU was read live after the fault:

```
MPU_CTRL = 0x00000005
RBAR     = 0x20066b7d
RLAR     = 0x2007ffe1
MAIR0    = 0x000000ff
```

Thus a Core 1 CPU store to `0x2007eacc` would have raised DACCVIOL on Core 1
before Core 0 returned. No such Core 1 fault occurred. This corrected capture,
not the malformed earlier experiment, excludes Core 1 as writer for this event.

All DMA registers were also read while the HardFault was stopped:

```
ch0 WRITE_ADDR=0x50200010  (PIO FIFO)
ch1 WRITE_ADDR=0x40088008  (peripheral FIFO)
ch2-ch15 all zero/unconfigured
```

No DMA channel was configured to write SRAM at the fault instant.

### Current hypothesis and next experiment

The remaining execution context capable of changing a live Core 0 MSP slot
during a leaf function is a Core 0 interrupt handler. Enabled handlers include
`TIMER0_IRQ_0`, `DMA_IRQ_0`, `PIO0_IRQ_0`, `PIO1_IRQ_0`, and
`SIO_IRQ_FIFO`.

First isolation iteration: preserve PRIMASK, disable Core 0 interrupts only
around `self.gb.tick()`, then restore PRIMASK immediately afterward.

Expected outcomes:

- If repeated boot faults disappear, an interrupt handler is the writer. Next,
  mask individual IRQ classes to identify which handler.
- If the same `0x0000fe9e` LR-slot fault persists, interrupt preemption is not
  required and investigation returns to Core 0 thread execution or another bus
  master.

### Interrupt-masked iteration 1 — partial result

Built and flashed image CRC `0xe959be1e`. The generated code confirms the
intended local interrupt window:

```
0x1000301e: mrs   r0, primask
0x10003024: cpsid i
              ... inlined GameBoy::tick() ...
0x10003e0a: bl    GameBoy::read_worker_output
0x10003e1e: ldr   r0, [sp, #saved_primask]
0x10003e24: cpsie i     (only when interrupts were enabled on entry)
```

This masks Core 0 interrupts only while the emulator tick is active and restores
the caller's prior PRIMASK state afterward.

Two boot trials completed every load-state checkpoint, both tick-0 checkpoints,
and remained alive beyond the immediate crash window. Stable values were:

```
GameBoy memory = 0x20026184
cartridge vtable = 0x100322dc
worker.ppu field @ 0x200038d0 = 0x2002b444
```

For comparison, the immediately preceding unmasked image reproduced the
post-tick HardFault in two attached boots. A separate unmasked GDB-controlled
run had also survived 90 seconds, so two clean masked trials are suggestive but
not sufficient to conclude that an ISR is the corruptor.

### Interrupt-masked iteration 2 — IBUSERR eliminated, new SPSC panic (2026-06-13)

Continued the same image (`0xe959be1e`) with 3 more trials (trials 3–5, 90 s each).

**All 3 trials produced an IDENTICAL set of 3 records — the `0x0000fe9e` IBUSERR crash
pattern is completely absent:**

```
Crash #1  WatchdogTimeout (prior session's watchdog, committed at boot)
Crash #2  Panic  core 1  spsc.rs:185  ROM bank=2  GB PC=0x63e6  Cycles=2,403,934,428
Crash #3  WatchdogTimeout  (watchdog after the spsc panic reset)
```

The same 3 records appeared in all 3 read-backs; the sector fills during the first
boot after mark-read (the spsc panic fires at ~8 s, then the board crash-loops).

**Significance:**

- The dominant `0x0000fe9e IBUSERR` / LR-slot corruption crash (14 of 24 records in
  the prior log) is **gone**. `PRIMASK` during `gb.tick()` definitively eliminates it.
  A Core 0 ISR was writing `0x0000fe9e` to the LR slot at `0x2007EACC` while `rr_u8`
  had that address as its saved return address on the stack.

- A new crash appears: `spsc.rs:185` = `(val + 1) % self.n()` in heapless 0.9.3's
  SPSC queue `increment` helper. Division by zero if `self.n()` (= AUDIO_QUEUE's
  buffer length through its fat pointer) is 0. This is a **corruption of the
  AUDIO_QUEUE's fat pointer** (buffer-length word smashed to 0), not a queue-overflow.
  It fires on Core 1.

- **Interpretation:** the same wild writer is still active, but the PRIMASK shifted
  the stack layout during tick so the wild write lands on the AUDIO_QUEUE static's
  fat pointer instead of the `rr_u8` LR slot. This is consistent with the
  layout-sensitivity documented throughout this session. The root corruptor is an ISR
  that writes GB-derived bytes to memory — the victim changes when the ISR can no
  longer fire during tick().

- `spsc.rs:185` is still the corruption pattern (GB bytes landing on a host pointer),
  not a new independent bug.

**Next step: identify which IRQ.**

Mask individual interrupt classes during `gb.tick()` to find which handler is the writer.
Candidates (per `bind_interrupts!`):

| IRQ | Handler | Suspicion |
|-----|---------|-----------|
| `TIMER0_IRQ_0` | embassy_time timer | Fires on GB-cycle-aligned ticks; could re-enter poll machinery |
| `DMA_IRQ_0` | CH0-CH3 completion | Fires on display/audio DMA done |
| `PIO0_IRQ_0` | I2S audio | Fires per I2S word |
| `PIO1_IRQ_0` | WiFi | Fires on WiFi PIO events |
| `SIO_IRQ_FIFO` | inter-core FIFO | Not in bind_interrupts!, may be implicit |

The `TIMER0_IRQ_0` / embassy timer is the primary suspect: it runs on Core 0, fires
frequently (every embassy timer tick), and the timer machinery can execute async waker
code with a non-trivial stack frame that could be positioned to overwrite `0x2007EACC`
with whatever happens to be 4 bytes into its frame. Mask it first via
`NVIC::mask(embassy_rp::pac::Interrupt::TIMER0_IRQ_0)` around `gb.tick()`.

### Core-0 ISR hypothesis ruled out; Core 1 is the remaining CPU writer (2026-06-13)

The linked DWT experiment did not capture the `0x0000FE9E` write. Image CRC
`0x0cdb5906` reproduced repeated current-image faults, including the exact
`PC=0x0000FE9E`, `SP_before=0x2007EAD0` pattern, but committed no DWT record.
Clearing `C_DEBUGEN` from firmware was therefore not sufficient; the linked
comparator encoding and/or halting-debug state remained a capture problem.

A raw DWT watch on the hot Core 0 LR slot suppressed the crash for three reboot
trials, confirming that DebugMonitor traffic on a frequently-written stack word
perturbs this race too heavily to be useful.

The investigation then returned to the reproducible interrupt-masked image:

- `cortex_m::interrupt::free` masks Core 0 interrupts only around `gb.tick()`.
- `run_core1_worker` keeps its original 0x100-byte frame.
- Disassembly places the `audio_tx: spsc::Producer<i16>` QueueView metadata word
  at `0x20081F44`; its healthy value is `0x00000801` (2049).
- Core 0 MPU region 1 marked `0x20081F40..=0x20081F5F` privileged-read-only,
  while region 0 continued protecting `.data` RAM code.

MPU-only image CRC `0x306dd4dd` immediately restored the failure, producing five
records in one 30-second trial. The records included the same Core 1
`heapless::spsc.rs:185` panic and **no Core 0 DACCVIOL** for the protected
`audio_tx` block. Therefore Core 0, including all Core 0 IRQ handlers, did not
write the queue-length word. This supersedes the earlier interpretation that
PRIMASK proved a Core 0 ISR was the corruptor.

The remaining CPU writer is Core 1. (The existing DMA register/allocation audit
still rules out DMA as the practical writer.) The next capture image arms Core
1's own raw DWT comparator on `0x20081F44` without taking `audio_tx`'s address,
preserving the failing frame layout:

```
image CRC:       0x46672172
watch address:   0x20081F44
healthy value:   0x00000801
run_core1_worker frame: 0x100 bytes
```

The comparator is programmed after the initial metadata store. If halting debug
remains enabled, Core 1 should stop at the corrupting instruction and can be
read through `probe-rs gdb` / GDB thread 2. If DebugMonitor is active instead,
the firmware will commit `CFSR_DWT_WATCHPOINT`.

The image was flashed and reset standalone, then allowed to run for 30 seconds.
Reading the result was blocked only by the external-tool approval quota. Resume
by attaching without another reset, inspect GDB thread 2 first, and read:

```
0x20081F44   audio_tx QueueView length metadata
0xE0001020   Core 1 DWT_COMP0
0xE0001028   Core 1 DWT_FUNCTION0 (MATCHED bit 24)
```

## #5 investigation continued (2026-06-13) — REVIEW: "Core 1 is the writer" is not sound

A review of the elimination chain that led to the "remaining CPU writer is Core 1"
conclusion. Summary: that conclusion is the weakest link and contradicts
better-supported evidence in the same session. The original core-1 ruling was
likely sound; the experiment that re-opened it was invalidated by an MPU encoding
bug, and the experiment that re-closed it onto core 1 is shaky for the same reason.

### The unreconciled contradiction

Two MPU experiments, run separately, exclude OPPOSITE cores for what the latest
agent treats as the SAME bug (one writer, victim moved by layout shift):

- Victim A — LR slot `0x2007EACC` (the dominant `0x0000FE9E` IBUSERR crash):
  at the exact fault, Core 1's MPU (`RBAR=0x20066b7d`, `RLAR=0x2007ffe1`) covered
  that address and NO Core 1 DACCVIOL fired. Conclusion drawn: "excludes Core 1."
- Victim B — `audio_tx` queue length word `0x20081F44` (the `spsc.rs:185`
  div-by-zero panic): Core 0 MPU marked `0x20081F40..5F` privileged-RO and NO
  Core 0 DACCVIOL fired. Conclusion drawn: "remaining CPU writer is Core 1."

If A and B are one writer (the agent's own assumption — "PRIMASK shifted the
layout, victim moved A->B"), then by this logic NEITHER core wrote it. That is
impossible for a single CPU writer. At least one "no DACCVIOL" result is a FALSE
NEGATIVE.

This investigation has already been burned by exactly that failure mode: the
earlier "Core 1 ruled out by 7 MPU-protected crashes" was retracted because the
RBAR was encoded with XN wrong. That is two documented MPU mis-encodings already.
A region that is not actually armed / not actually covering the address produces a
silent "no DACCVIOL" that is indistinguishable from "this core didn't write it."
The core-1 conclusion rests on a SINGLE 30-second trial whose region encoding is
not even recorded in the notes.

### The cleanest evidence points at Core 0, not Core 1

The PRIMASK experiment is the strongest single result in the log because it is
pure software with no MPU encoding to get wrong. Masking CORE 0 interrupts during
`gb.tick()` made the dominant `0x0000FE9E` crash vanish across 3 trials. If the
writer were Core 1, masking Core 0's interrupts would have no reason to affect it
— core 1 runs independently. The crash being COUPLED to Core 0 interrupt state is
direct evidence the writer is a Core 0 ISR (or a Core-0 path whose preemption
timing the mask changed). The agent originally read it this way, then walked it
back on the strength of the much weaker audio_tx MPU trial.

The value clue reinforces core 0: `0x0000FE9E` is exactly
`BusEvent { address: 0xFE9E, value: 0 }` byte-for-byte. BusEvents are generated by
the CPU bus-write path, which runs on core 0. A BusEvent-shaped value landing on a
host return-address slot is a core-0-side wild store.

### Meta-lesson

The investigation keeps concluding by ELIMINATION ("DMA out, core 0 out, therefore
core 1"). The A-vs-B contradiction proves at least one elimination is false, so
elimination is no longer safe. Switch to POSITIVE identification — catch the writer
with a validated trap, not infer it from an absence.

### Corrected next steps (supersede "arm Core 1 DWT on 0x20081F44")

1. Add a POSITIVE CONTROL to every MPU experiment: after arming a region, do a
   deliberate test store to the protected address from the core under test and
   confirm DACCVIOL fires with the expected PC. Until that passes, no "no DACCVIOL"
   result is trustworthy — including the one the core-1 conclusion depends on.
2. Discriminating experiment: arm the SAME victim word RO on BOTH cores' MPUs at
   once (queue length metadata is written once at construction, never during
   operation, so RO is safe on both). Whichever core faults is the writer. If
   neither faults and it still smashes -> not a CPU store, or the encoding is wrong
   (see step 1).
3. Lean back into the PRIMASK lead — it is the safe, high-signal path. Continue the
   per-IRQ bisection: mask `TIMER0_IRQ_0` alone during `gb.tick()`, then
   `DMA_IRQ_0`, etc. If masking one specific IRQ kills the crash, that core-0
   handler is the writer, with no MPU encoding risk. `TIMER0_IRQ_0` (embassy timer
   waker) is the first target.
4. Methodology: stop concluding from single 30s/90s trials. The bug is
   boot-clustered and probabilistic. Use the reboot-loop harness, N>=20, for any
   "ruled out."

## #5 investigation continued (2026-06-13) — BISECTION: Core 0 TIMER0_IRQ_0 is the writer

The IRQ-mask bisection from the corrected next steps was carried out. Result:
the writer is a **Core 0 interrupt handler**, specifically **`TIMER0_IRQ_0`**
(the embassy time-driver ISR). This confirms the Core-0 reading and **refutes the
earlier "Core 1 is the writer" conclusion**.

### Experiment

Changed the `gb.tick()` wrapper in `src/multicore.rs` (~line 1302) from masking ALL
Core 0 interrupts (`cortex_m::interrupt::free`) to masking ONLY `TIMER0_IRQ_0`:

```rust
cortex_m::peripheral::NVIC::mask(rp_pac::Interrupt::TIMER0_IRQ_0);
self.gb.tick();
unsafe { cortex_m::peripheral::NVIC::unmask(rp_pac::Interrupt::TIMER0_IRQ_0) };
```

Flashed image CRC `0x5bdff527`. Ran a 10-trial standalone reboot-loop (blank crash
sector at `0x103FF000`, `probe-rs reset`, wait ~120s, decode with
`crash_decoder.py --probe --json`).

### Result (10 trials)

- **`0x0000FE9E` IBUSERR (the dominant target signature): 0 occurrences.** Gone —
  exactly as under full PRIMASK masking.
- **`spsc.rs:185` Core-1 panic: 2 trials** — the SAME secondary victim the full-mask
  build produced.
- New dominant crash: HardFault at `0x10003e4c` (`GameBoy::cycle_counter`,
  `core/src/gameboy.rs:293`), Core 0, LR=`0x20002521` (inside `GameBoy::read_worker_output`),
  CFSR `0x01000000` (IBUSERR) or `0x00008200`.
- Also seen: a Core-1 `atomic_load` IBUSERR (1 trial), WatchdogTimeouts, 3 clean trials.

Note: `CFSR=0x00008200` is PRECISERR + BFARVALID (a precise BUS fault from
dereferencing a corrupted pointer), NOT a DACCVIOL/MPU hit. The `audio_tx` MPU
region did not fire here.

### Interpretation

Among all Core 0 interrupts, masking ONLY `TIMER0_IRQ_0` during `tick()` reproduces
the full-mask outcome (same elimination of `0x0000FE9E`, same `spsc.rs:185` secondary
victim). So `TIMER0_IRQ_0` is the IRQ whose masking matters — the embassy time-driver
ISR is in the corruption path. This vindicates the PRIMASK/Core-0 evidence and refutes
the Core-1 elimination conclusion.

**Masking it did NOT fix the bug — it relocated the victim.** TIMER0 is only masked
*during tick*, so the ISR still fires outside the tick window and its wild write lands
on whatever stack/static is live then (the new `cycle_counter`/`read_worker_output`
crash, and the `spsc.rs:185` Core-1 victim). This is a diagnostic confirmation of the
writer, not a fix. The `multicore.rs` masking change remains in the working tree as a
diagnostic only.

### Remaining gap + next step

The bisection is layout-confounded: the mask wrapper shifts the frame AND stops the
ISR firing during tick. A NEGATIVE CONTROL is needed to prove the elimination is
TIMER0-specific and not a generic layout shift: mask a different single frequently-firing
IRQ (`DMA_IRQ_0`) during tick, same code structure.

- If `0x0000FE9E` RETURNS under DMA-mask → confirms TIMER0-specific (locked).
- If `0x0000FE9E` STAYS GONE under DMA-mask → the effect is generic layout perturbation
  and TIMER0 is not proven.

After the control confirms TIMER0, the investigation moves to the **embassy-rp time
driver ISR** — the alarm/waker queue handling — hunting a dangling/stale `Waker` whose
wake performs a wild write. This is a firmware/embassy-level bug, not the GB emulator core.

## #5 investigation continued (2026-06-13) — NEGATIVE CONTROL REFUTES TIMER0; masking-bisection is layout-confounded

The `DMA_IRQ_0` negative control for the TIMER0 finding was run. It refutes the
"TIMER0 is the writer" conclusion. The masking-bisection method is confounded by
the mask wrapper's own stack-frame perturbation and cannot identify the writer.

### Experiment

Changed only the masked interrupt in the `gb.tick()` wrapper (`src/multicore.rs`
~line 1302) from `TIMER0_IRQ_0` to `DMA_IRQ_0`, keeping the exact same code
structure so the wrapper's layout perturbation is held constant and only WHICH
firing interrupt is masked changes. `DMA_IRQ_0` (CH0-CH3: display SPI + audio I2S
completion) does fire during gameplay. Flashed image CRC `0x92b9fff1`. Ran an
8-trial standalone reboot-loop (the 9th/10th were lost when the board entered a
tight crash-loop needing a physical power-cycle).

### Result (8 valid trials)

- **`0x0000FE9E` IBUSERR (the dominant target signature): 0 occurrences.** It did
  NOT return under DMA-mask.
- All 8 trials showed the SAME new victim as the TIMER0-mask build:
  `PC=0x10003e4e` (`GameBoy::cycle_counter`, `core/src/gameboy.rs:293`), Core 0,
  LR=`0x20002521` (`read_worker_output` / `critical_section::release`),
  `CFSR=0x01000000` (IBUSERR), `HFSR=0x40000000` (FORCED),
  `fault_addr=0x00000004` (null+4 fetch, consistent with a corrupted vtable/fn-ptr),
  `ext_regs.r4=0x2007ead0`.

### Interpretation — TIMER0 refuted

Single-ISR logic: if TIMER0 were the writer, masking DMA (leaving TIMER0 free to
fire) should have let `0x0000FE9E` return. It did not. Symmetrically the TIMER0
experiment masked TIMER0 (leaving DMA free) and also eliminated it. So NEITHER IRQ
alone is the writer.

Clinching detail: both masks (TIMER0 and DMA) produce the IDENTICAL new victim
(`cycle_counter`, `fault_addr=4`). If the masked-IRQ identity mattered, different
masks would relocate the victim to different addresses. Same victim across both ⇒
the only thing that differs between unmasked-vs-wrapped is the wrapper's
stack-frame shape, not the interrupt. **The `0x0000FE9E` elimination is a LAYOUT
artifact of adding the mask wrapper, not evidence of an IRQ writer.**

This is the same extreme layout-sensitivity documented throughout #5 (boxing moved
the victim; the in-situ dump suppressed it; per-tick checkpoints suppressed it).
The masking-bisection approach measures its own perturbation and is therefore
abandoned. The earlier PRIMASK result is now best read as another layout artifact,
not as evidence of a Core-0 ISR writer.

### What still holds

- Unmasked baseline (no wrapper, plain `self.gb.tick()`): `0x0000FE9E` reproduces
  at `0x2007EACC`. Any mask/perturbation wrapper relocates the victim and hides it.
- The corruption is real and on Core 0. The writer is NOT identified.
- Same poisoned save state as all prior captures (live boot log shows
  `cycles=15267416632`, whose low 32 bits ≈ 2.382 B match the historical
  `cycle_lo≈2.3955B` records).

### Decisive next step — layout-immune hardware watchpoint

Stop software-masking experiments; they perturb the frame. Revert the shim to plain
`self.gb.tick()` (the only build that reproduces `0x0000FE9E`), then set an OpenOCD
hardware WRITE-watchpoint on `0x2007EACC` on that unmasked build. The watchpoint is
external to the firmware (no code change, no layout shift) and halts the CPU at the
exact store instruction that writes the victim — its PC + base register names the
writer. The recipe is documented earlier in this file (RaspberryPi OpenOCD fork,
`set USE_CORE cm0` single-core attach, `mww 0x400d8000 0` to disable the watchdog,
open-ended `poll`+`sleep` TCL loop). Prior attempts were blocked only by approval
budget, not by the method.

Optional cheap pre-check: a pure-layout control — a wrapper that shifts the frame
identically but masks NO firing interrupt (mask an unused IRQ vector, or insert
equivalent `black_box` stack churn). If `0x0000FE9E` still vanishes, layout is
definitively the variable and the IRQ theory is closed for good.

## #5 investigation continued (2026-06-14) — MPU vtable-trap capture run: BLOCKED on flash wedge (board needs power-cycle)

Decisive-capture session for the cold write-once victim (the `Box<dyn Cartridge>`
vtable pointer inside `GameBoyMemory`, at `GameBoyMemory_base + 0x4124`, healthy =
a flash ptr ≥0x10000000; corruptor writes a GB `BusEvent` like `0x0000FE9E` over
it, later dispatch through the smashed vtable → IBUSERR at a wild PC).

### Prior census recap (for the record)
- Standalone repro ~13/18 boots (72%), watchdog on, NO manual input.
- Vtable word `base+0x4124` is the victim in ~50% of records.
- `audio_tx` length word `0x20081F44` never smashed (0/18).
- Masking-bisection (TIMER0/DMA) was abandoned as layout-confounded (see prior
  2026-06-13 sections); the only build that reproduces `0x0000FE9E` is the
  unmasked baseline, BUT the current trap build deliberately keeps the
  `cortex_m::interrupt::free(|_| self.gb.tick())` mask wrapper because in that
  build the vtable word is the dominant *cold* victim and the MPU can trap it.

### Trap implementation under test (src/multicore.rs, already in tree)
- MPU **region 2** = priv-RO (AP=10, XN=1, SH=11) over the 32-byte block holding
  the vtable word, armed at boot in `setup_core0_vtable_mpu` once the `Box` is
  built and healthy (`arm_cartridge_vtable_watch`). MEMFAULTENA left OFF on
  purpose, so a write → MemManage DACCVIOL → **escalates to HardFault** (priority
  -1, not maskable by the per-tick PRIMASK). Region temporarily unlocked RW
  around legit rom-window-cache writes (`refresh_rom_window_cache_with_mpu_bracket`),
  relocked after. MPU_CTRL=0x5 (ENABLE|PRIVDEFENA, HFNMIENA=0 so the MPU is off
  inside the HardFault handler and it can read crash regs).
- HardFault handler (`src/crash/handler.rs::hard_fault_rust`) records CFSR, HFSR,
  MMFAR/BFAR (picks BFARVALID then MMARVALID), and the stacked PC. A
  DACCVIOL→HardFault with MMARVALID gives MMFAR == faulting addr and stacked
  PC == the exact corrupting store. THIS IS THE PRIZE PATH (classification A).
- Secondary/diagnostic layers (layout-perturbation risk, candidates for stripping
  in contingency D): per-tick software vtable guard (`rustyboy_cartridge_vtable_guard`),
  RAM-code sentinel snapshot (`rustyboy_ram_code_guard`), core-1 stack-RO MPU
  region 0, optional core-1 vtable MPU region 1.

### STEP 0 — crash sector on arrival
Decoded `0x103FF000` (`crash_decoder.py --probe --json --elf <abs ELF>`): sector
`valid:false`, `crashes:[]` — BLANK. The previous agent left no committed record;
no free capture. Full validation + capture required.
NOTE: the decoder's `--elf` must be the ABSOLUTE workspace path
`/.../rustyboy/target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w` (build
output lands in the workspace `target/`, not under `platform/pico2w/target/`); a
relative path fails silently to "(addr2line not available)". Host GNU `addr2line`
and `objdump` are present and read the ARM ELF fine; ARM-specific binutils are NOT
installed (`rust-objdump`/`cargo-objdump` available as fallback).

### STEP 1 — POSITIVE CONTROL build prepared
Changes made for the positive control (TEMPORARY — revert after validation):
1. `src/multicore.rs:139` `VTABLE_TRAP_POSITIVE_CONTROL` set `false → true`.
2. Added a guaranteed one-shot test store at the END of `arm_cartridge_vtable_watch`
   (right after `setup_core0_vtable_mpu` relocks region 2 RO), gated by the same
   flag+PENDING latch. RATIONALE: the existing positive-control write lives inside
   `refresh_rom_window_cache_with_mpu_bracket`, which only fires on an MBC ROM-bank
   switch and may NEVER run for a no-MBC cart — so it cannot be relied on. The new
   site does `write_volatile(vtable_addr, 0x0000_FE9E)` to the now-RO word; it must
   trap → DACCVIOL → HardFault, stacked PC == this site, MMFAR == vtable word.
   It logs a loud `defmt::error!` if the write RETURNS (trap failed to fire).
Build: clean, ~13s. **rb-flash image CRC `0x7ff2c8f4`** (over [0x10000114,0x10075980)).

### BLOCKER — flash loader `init` fails (code 288); board needs PHYSICAL POWER-CYCLE
Could NOT flash the positive-control build. Every flash-WRITE path fails at the
flash-algorithm `init` with **"execution of 'init' failed with code 288"**:
- `cargo run --release` (rb-flash) — fails 288 (after its USB-reset + watchdog-disable).
- `probe-rs erase --chip RP235x` (direct) — fails 288.
- `probe-rs download ... --base-address 0x103FF000 ff4k.bin` (sector-blank) — fails 288.
- Retried after `probe-rs reset` (succeeds), after a manual watchdog disable via
  `probe-rs write b32 0x400d8000 0` (succeeds), at `--speed 1000` — all still 288.
- `--connect-under-reset` times out (Pico 2W SWD header doesn't wire RUN/reset).

Crucially the SWD link is HEALTHY: `probe-rs list` shows the Debugprobe, and
`probe-rs info --chip RP235x` reads the full CoreSight ROM (Cortex-M33, DWT/FPB/ITM,
both cores). `probe-rs reset` and register writes work. ONLY the QSPI flash loader's
`init` is wedged — the classic RP2350 post-crash-loop flash-subsystem wedge that
SWD-only recovery cannot clear. Per the task's own contingency ("If the board
wedges unrecoverably, report it needs a physical power-cycle and stop"), this run
is halted here.

probe-rs version: 0.29.1.

### RESUME RECIPE (after a physical power-cycle of the Pico 2W)
1. `pkill -9 -x probe-rs; sleep 1`. Confirm `probe-rs erase --chip RP235x` now
   inits (no 288). If still 288, re-power again / reseat USB.
2. Positive control is ALREADY staged in the tree (flag=true + one-shot write).
   `cd platform/pico2w && cargo run --release` to flash (note the CRC; should be
   `0x7ff2c8f4` unless source changed). Watch RTT for
   "POSITIVE CONTROL one-shot test write (arm site)". Then run STANDALONE
   (blank sector → `probe-rs reset` → `pkill -f probe-rs` → wait ~100s → decode)
   and CONFIRM a record: MMFAR == the boot-logged vtable word, stacked PC == the
   one-shot write site in `arm_cartridge_vtable_watch`. If it does NOT fire, the
   trap is broken (check SHCSR.MEMFAULTENA expectation / region encoding / handler).
3. On pass: set `VTABLE_TRAP_POSITIVE_CONTROL` back to `false`, REMOVE the
   temporary one-shot block in `arm_cartridge_vtable_watch`, rebuild, reflash, and
   run the 15–20-trial standalone reboot-loop (STEP 2). Track repro rate vs the 72%
   census; classify each record A/B/C/D per the plan.

## #5 investigation continued (2026-06-14, RESUMED after power-cycle) — POSITIVE CONTROL PASSED

### Flash wedge: CLEARED by the physical power-cycle
- `probe-rs reset --chip RP235x` OK; `cargo run --release` (rb-flash) flashed clean,
  NO code-288. Image CRC `0x7ff2c8f4` (matches staged expectation). Boot integrity
  `full image crc 0x7ff2c8f4 OK`. Flash program took ~30s (single-buffered+verify).

### Boot-logged vtable target (this build/heap layout)
- `bug#5 trap: arm core0 MPU @ vtable_word=0x2002a2d8 value=0x1003255c block=0x2002a2c0`
- region 2 = [0x2002a2c0, 0x2002a2df] priv-RO.
- Core 1 also arms its own vtable MPU (region 1, same block) at top of run_core1_worker
  loop — so BOTH cores trap writes to 0x2002a2d8 in this build (contingency B partly
  pre-covered; the committed record's `core` field tells us which core stored).

### Positive control — under cargo run (vector-catch)
RTT showed, in order:
- `POSITIVE CONTROL one-shot test write (arm site) to 0x2002a2d8` (multicore.rs:1874 WARN)
- then `Firmware exited unexpectedly: Exception` (the store TRAPPED — vector-catch
  caught the escalated HardFault). The `trap did NOT fire` error did NOT print. Good.

### Positive control — STANDALONE (the real validation)
Procedure: blank crash sector (download 4 KiB 0xFF @ 0x103FF000) → `probe-rs reset`
→ `pkill -9 probe-rs` (detach) → wait 65s → decode.
Committed record (slot 10, newest):
- crash_kind HardFault, **core 0**
- CFSR `0x00000082` = MMARVALID(b7) | DACCVIOL(b1) → MemManage data-access violation, MMFAR valid
- HFSR `0x40000000` = FORCED → DACCVIOL escalated to HardFault (as designed; MEMFAULTENA off)
- MMFAR (fault_addr) `0x2002a2d8` == the boot-logged vtable word ✓
- PC `0x1001aec2` → `core::ptr::write_volatile` inlined into
  `rustyboy_pico2w::multicore::arm_cartridge_vtable_watch` at **multicore.rs:1879**
  (the one-shot test store `write_volatile(vtable_addr, 0x0000_FE9E)`) ✓
- LR `0x1001aebf` → multicore.rs:1874 (the WARN just above the store) ✓
- ext_regs r4=0x2007e740, r12=0x20065bc0. stack not overflowed (headroom sentinel).

**VERDICT: POSITIVE CONTROL PASSES.** MPU region 2 traps core-0 writes to the
vtable word, the DACCVIOL escalates to a forced HardFault, and the handler records
MMFAR == faulting addr and stacked PC == the exact corrupting store. The capture
path (classification A) is proven sound. Proceeding to STEP 2 (revert + real loop).

## #5 investigation continued (2026-06-14) — *** THE CAPTURE: corruptor is a memcpy overrun into GameBoyMemory ***

### Real-capture build (positive control reverted)
- `VTABLE_TRAP_POSITIVE_CONTROL` set back to `false`; one-shot test-store block
  REMOVED from `arm_cartridge_vtable_watch`. (The pre-existing rom-window-bracket
  positive-control `if` at multicore.rs ~1830 is left in place but is dead with the
  flag false.) Rebuilt clean. **rb-flash image CRC `0x41817bf7`** over
  [0x10000114,0x10075920). Boot integrity OK.
- Boot-logged target: `vtable_word=0x2002a2d8 value=0x100324f4 block=0x2002a2c0`,
  region 2 = [0x2002a2c0, 0x2002a2df] priv-RO. (Core 1 also arms its mirror region 1.)

### Classification A captured on trial 01 AND trial 02 (reproducible)
Standalone reboot loop (blank sector -> reset -> detach -> wait 100s -> decode).
Both trials committed an IDENTICAL record:
- crash_kind HardFault, **core 0**
- CFSR `0x00000082` = MMARVALID(b7) | DACCVIOL(b1) -> MemManage data-access violation
- HFSR `0x40000000` = FORCED (DACCVIOL escalated to HardFault; MEMFAULTENA off)
- **MMFAR (fault_addr) = `0x2002a2c0`** (the MPU block base; first protected word the store reached)
- **stacked PC = `0x1002eaaa`**
- LR `0x000000a0` (garbage — leaf memcpy clobbered LR; no useful caller frame)
- ext_regs: **r4=0x2007ea00, r12=0x2002a2d4**
- stack NOT overflowed; DMA busy_mask 0x00 (no DMA in flight)

### RESOLVED writer — it is `memcpy`
`addr2line 0x1002eaaa` -> `compiler_builtins::mem::impls::copy_forward::copy_forward_aligned_words`
(memcpy.rs:130), inlined into `compiler_builtins::mem::memcpy`.
Disassembly (the aligned-words unrolled copy loop, function @ 0x1002e9e8):
```
1002eaa6: 68cd        ldr   r5, [r1, #0xc]     ; load source word
1002eaa8: 3110        adds  r1, #0x10          ; src += 16
1002eaaa: f843 5b04   str   r5, [r3], #4       ; <-- FAULTING STORE: *r3 = r5; r3 += 4
1002eaae: 4563        cmp   r3, r12            ; r3 == dest-end?
1002eab0: d3ea        blo   0x1002ea88         ; loop while r3 < r12
```
Register meaning at fault:
- **r3 = destination write pointer** = 0x2002a2c0 (== MMFAR) — the wild dest.
- **r5 = the word being written** (loaded from source [r1, #0xc]).
- **r12 = destination END pointer = 0x2002a2d4**.
- **r1 = source pointer** (value at fault not stacked; it's mid-loop).

### Geometry — the destination END is EXACTLY the GameBoyMemory struct base
`GameBoyMemory::cartridge_vtable_word_addr_for_diagnostics() = addr_of!(self.cartridge)+4`.
Boot log vtable word = 0x2002a2d8 => **GameBoyMemory base = addr_of!(self.cartridge) = 0x2002a2d4**
(the Box<dyn Cartridge> fat-ptr: data_ptr @0x2002a2d4, vtable_ptr @0x2002a2d8).
- memcpy dest-END r12 = **0x2002a2d4 == the GameBoyMemory struct base.**
- The MPU block [0x2002a2c0,0x2002a2df] starts 0x14 below the struct base, so the
  copy hits the RO region (at 0x2002a2c0) BEFORE finishing at 0x2002a2d4 and traps.
- WITHOUT the MPU this copy ends at the struct base; a copy a hair longer (or this
  same copy on a build where the box sits a few bytes lower) overruns the data_ptr
  and the vtable_ptr at +4 — i.e. the classic `0x0000FE9E`-over-vtable smash.

### CONCLUSION
Bug #5 is **NOT a stray wild-pointer store** and **NOT a cross-core RMW** and **NOT
DMA** (DMA idle). It is a **`memcpy` whose destination region runs up to / over the
base of the `GameBoyMemory` heap allocation** — a heap buffer-overrun / adjacent
allocation written by a memcpy whose destination (r3 base) + length reaches into the
GameBoyMemory struct's first words (the cartridge fat pointer). The object being
copied lives in the heap pool immediately below GameBoyMemory; its copy length or
its destination pointer is wrong, so the tail of the copy lands on the cartridge
data_ptr/vtable_ptr.

NEXT (for parent / next session): identify the memcpy CALL SITE. The leaf memcpy
clobbered LR (0x000000a0), so the crash record can't name the caller. To get it:
either (a) widen the MPU block / move trap so the handler also walks the stack for a
flash-range return address, or (b) set an additional capture that records r1 (source)
— the source buffer identity will name the structure. Candidate callers are any
copy_from_slice/clone/to_vec near a heap object adjacent to GameBoyMemory (e.g. the
512KiB staged-ROM XipCartridge build, save-state restore, or the rom_window cache).
The decisive fact is locked in: **the corrupting instruction is memcpy @0x1002eaaa,
dest pointer r3, dest-end r12=struct-base 0x2002a2d4, written value r5 from src r1.**

### Reproduction rate (real-capture loop, 100s/trial standalone)
Every decoded trial committed the SAME single record (memcpy @0x1002eaaa, MMFAR
0x2002a2c0, CFSR 0x82, r12=0x2002a2d4). First 3 decoded trials: 3/3 = 100% — far
above the 72% census, because the MPU trap is precise and the boot save-state restore
is a DETERMINISTIC trigger (no manual input needed, no race window). No B/C/D records
observed; no contingency needed. The store is ALWAYS a core-0 CPU store inside memcpy
(rules out core-1 and DMA definitively for this trigger path).

## #5 investigation (2026-06-14, NEW AGENT) — PRECISE DWT switch + positive control PASS

### CORRECTION to the prior "memcpy IS the bug" section above
The prior agent's MPU build captured a memcpy @0x1002eaaa whose dest-END r12 =
0x2002a2d4 == the GameBoyMemory struct base. That copy is the BOUNDED load_state
restore of the GB-memory array that abuts `cartridge`; its last word lands at
0x2002a2d0 (< struct base) and it tripped only the MPU block LOW end (0x2002a2c0),
**never the cold vtable word 0x2002a2d8**. That is a FALSE POSITIVE of the coarse
32-byte MPU region — it fires on EVERY boot during load_state, resets before
gameplay, and MASKS the real corruptor that writes 0x0000FE9E over the vtable
during emulation ticks. (Do NOT treat the prior section's "CONCLUSION: heap
buffer-overrun memcpy" as the resolved #5 writer — it is the abutting-array
restore, not a store to the vtable word.) Switched to a PRECISE 4-byte DWT watch
on EXACTLY 0x2002a2d8 so the array's writes (all <= 0x2002a2d3) cannot trip it.

### Code changes (src/multicore.rs)
- MPU region 2 (vtable block) DISARMED: removed `setup_core0_vtable_mpu` and
  `setup_core1_vtable_mpu`; `arm_cartridge_vtable_watch` now arms a DWT raw-word
  write-watch (`dwt_watch::publish_and_arm_raw_words([vtable_addr,0,0,0])`, fixed
  0x815 encoding) on core 0, and `run_core1_worker` arms the same on core 1.
- `refresh_rom_window_cache_with_mpu_bracket` no longer unlocks/relocks an MPU
  region (region 2 gone); keeps only the value asserts around the cache refresh.
- Per-tick software vtable guard (`rustyboy_cartridge_vtable_guard`) and RAM-code
  sentinel KEPT as independent cross-checks.
- Core 0 .data MPU (region 0) and audio_tx MPU (region 1) in main.rs left as-is;
  core 1's "core-0 stack RO" MPU (setup_core1_mpu) left as-is. None touch 0x..d8.

### POSITIVE CONTROL — PASS (both vector-catch and standalone)
Build w/ VTABLE_TRAP_POSITIVE_CONTROL=true + one-shot write_volatile(vtable,0xFE9E)
at end of arm_cartridge_vtable_watch. rb-flash image CRC **0xc9db77d5**, integrity OK.
Boot log: `arm core0 DWT write-watch @ vtable_word=0x2002a2d8 value=0x1003285c`.
- Under cargo run: vector-catch reported "Firmware exited: Watchpoint @
  arm_cartridge_vtable_watch"; the "write RETURNED — trap did NOT fire" error did
  NOT print. The DWT caught the store.
- STANDALONE (blank sector -> reset -> detach -> wait 70s -> decode), slot 0:
  HardFault core 0; CFSR 0xd7170001 (DWT-watchpoint sentinel); HFSR 0x59000815
  (low bits carry the 0x815 DWT FUNCTION); **fault_addr/watched = 0x2002a2d8** ==
  vtable word; **stacked PC = 0x1001af54** -> arm_cartridge_vtable_watch
  multicore.rs:1838 (the test store); **ext_regs r4 = 0x0000fe9e** == the written
  value; DMA idle. VERDICT: DWT comparator + DebugMon handler fire with NO external
  debugger and record watched addr + written value + store PC. Classification-A
  path proven. Proceeding to remove the test write and run the real capture loop.

### REAL-CAPTURE LOOP (precise DWT, positive control removed) — DWT fires ZERO times
Real-capture build: VTABLE_TRAP_POSITIVE_CONTROL=false, one-shot write removed.
rb-flash image CRC **0x0df3fb08** over [0x10000114,0x10075c60). Integrity OK.
Boot: `arm core0 DWT write-watch @ vtable_word=0x2002a2d8 value=0x1003282c`; core 1
also armed its DWT @ 0x2002a2d8. NOTABLE: the full load_state sequence
(cpu/timer/memory/.../worker-output phases) completed with NO DWT hit — the
abutting GB-memory array restore does NOT trip the precise watch (the coarse MPU
would have false-positived here). tick-0 pre-gb.tick shows vtable=0x1003282c healthy.

18-trial standalone reboot loop (blank sector -> reset -> detach -> 22-30s -> decode):
- Repro rate: ~10 records / 17 conclusive trials ≈ **59%** (trial 5 was a transient
  probe-rs init glitch, re-verified board healthy immediately after; excluded).
  Below the 72% census but NOT a collapse — the passive DWT is not suppressing the bug.
- Signature tally:
  - (A) DWT watchpoint hit on 0x2002a2d8: **0 trials.**  THE VTABLE WORD IS NOT THE VICTIM.
  - (B) rustyboy_cartridge_vtable_guard panic: **0 trials.**
  - DOMINANT (8 trials: 1,2,6,8,11,14,15,16) = classification **C**:
    HardFault **core 0**, CFSR **0x00008200** (BFARVALID|PRECISERR), HFSR 0x40000000
    (forced), **fault_addr 0x4f220158**, **PC 0x1000329a** = BusEventQueue::is_empty
    (memory.rs:60), r4=0x2007ead8, r12=0x20001bdf. Identical every time.
  - SECONDARY (2 trials: 10,18) = classification **C**: HardFault **core 1**, CFSR
    **0x00000001** (IBUSERR), PC **0xfffffffe** (wild), r4=0x20081e80, r12=0x20065b77;
    trial 18 also committed a downstream WatchdogTimeout.

### RESOLVED downstream fault — it is a CORRUPT GameBoyMemory POINTER, not the vtable
Disasm at the dominant fault PC 0x1000329a (BusEventQueue::is_empty inlined into the
embassy_main tick loop):
```
10003292: ldr  r0, [sp, #0xd4]   ; r0 = a pointer spilled on the core-0 stack
10003294: movw r1, #0x4238       ; r1 = 0x4238 = byte offset of events.len in GameBoyMemory
10003298: ldr  r0, [r0]          ; r0 = *r0  (the GameBoyMemory pointer)
1000329a: ldr  r1, [r0, r1]      ; <-- FAULT: load events.len at [r0 + 0x4238]
1000329c: cmp  r1, #0x0          ; is_empty()
```
fault_addr 0x4f220158 => r0 = 0x4f220158 - 0x4238 = **0x4f21bf20**, a WILD pointer
(not in 0x2000_0000 SRAM). r0 came from `ldr r0,[r0]` after `ldr r0,[sp,#0xd4]`, i.e.
a smashed pointer-to-GameBoyMemory living on the core-0 stack (or the object it points
to). `events` is the LAST field of GameBoyMemory (offset 0x4238); `cartridge`/vtable is
the FIRST (offset 0/4). So in THIS build/heap layout the corruptor's victim is a
**stack-resident pointer to GameBoyMemory**, ~0x4238 BELOW and structurally unrelated
to the cold vtable word at 0x2002a2d8 — which is why the precise DWT (and the value
guard) never fire. The MPU "memcpy capture" earlier was a load_state false positive AND
the vtable word is simply not where this build's smash lands.

CONCLUSION: classification **C** across the board (downstream faults, DWT/guard silent,
rate not collapsed). The cold vtable word is NOT the #5 victim in this build. The smash
is a corrupt pointer-to-GameBoyMemory on the core-0 stack (dominant) plus occasional
core-1 wild-PC. Next per STEP 4: strip the per-tick software guard (layout-perturbation
suspect) and re-run DWT-only; if the victim relocates, that confirms layout sensitivity.

### STEP-4 FALLBACK — DWT-ONLY (per-tick software guard + RAM-code sentinel STRIPPED)
Stripped the per-tick `rustyboy_cartridge_vtable_guard` value-check and the `__sdata`
RAM-code sentinel from `PicoGameBoy::tick` (kept only the passive DWT comparator, the
least-perturbing on-device trap). rb-flash image CRC **0x82055da4**. Boot: DWT armed
@ 0x2002a2d8 value=0x10032714 (heap shifted vs 0x1003282c — removing the guards moved
the layout, as expected). load_state again completed with NO DWT hit.

10-trial DWT-only loop: repro **4/10 ≈ 40%** (down from 59% with the guard). Still
**0 DWT hits** on the vtable word. The victim RELOCATED:
- New dominant (trials 2,6,7,8): HardFault **core 1**, CFSR 0x00008200 (BFARVALID|
  PRECISERR), **fault_addr 0xc0000000**, **PC 0x1001b1c6** inside
  `multicore::run_core1_worker` — `core::ptr::read` / slice `get_unchecked`
  (`ldr.w r10,[r2,r0,lsl #2]`), r2=slice base, r0=index; r4=0x20081ec8, r12=0.
  The few preceding instrs (`ldr r1,[r12]; add r0,r12,#4; lda r0,[r0]; cmp r1,r0`)
  are an spsc queue head/tail compare — i.e. the core-1 dequeue path indexing through
  a CORRUPT queue/slice pointer. Downstream symptom, not the corrupting store.

### VERDICT for the parent
1. The precise DWT comparator + DebugMonitor handler are PROVEN GOOD (positive control:
   standalone record with watched addr 0x2002a2d8, written value 0x0000FE9E, store PC).
2. Across 28 real-capture trials (18 with guard + 10 DWT-only), the DWT fired on the
   cold vtable word **ZERO** times, and the software value-guard panicked ZERO times,
   yet the bug reproduced at 40-59%. **The cold vtable word 0x2002a2d8 is NOT the #5
   victim.** The earlier MPU "memcpy capture" was a load_state false positive; the real
   smash lands on a DIFFERENT, LAYOUT-DEPENDENT victim:
     - guard build: corrupt pointer-to-GameBoyMemory spilled on the core-0 stack,
       faulting in BusEventQueue::is_empty (`[r0+0x4238]`, r0 wild=0x4f21bf20).
     - DWT-only build: corrupt queue/slice pointer in core-1 run_core1_worker,
       faulting in a get_unchecked (`[0xc0000000]`).
   The victim MOVING when per-tick code is added/removed CONFIRMS layout sensitivity.
3. Classification: **C** throughout (downstream faults; DWT + value-guard silent;
   rate not collapsed → the passive DWT is not what suppresses, the victim simply
   isn't the watched word). No classification-A (real) capture is achievable by
   watching the vtable word.

### RECOMMENDED NEXT (do NOT thrash; for parent/next session)
The corruptor is a wild/overrun STORE whose victim address is layout-dependent and is
NOT the vtable word. To catch the WRITER we must watch the victim that the CURRENT
build actually smashes, or bisect by time:
  (a) Point a DWT WRITE watch at the build's ACTUAL victim — for the guard build that
      is the core-0 stack slot [sp,#0xd4]'s pointer, or better the GameBoyMemory base
      pointer copy; for the DWT-only build the core-1 queue/slice pointer. These move
      per build, so capture the boot-logged victim each flash.
  (b) Re-introduce ONE software guard that records the cycle counter + GB state at the
      first tick the corruption is detectable (value-guard on the GameBoyMemory base
      pointer, not the vtable), to bisect WHEN the store lands (a non-DWT path).
  (c) The two downstream faults both implicate POINTER/REFERENCE smashes near the
      cross-core queue + GameBoyMemory — re-examine the spsc Producer/Consumer handles
      and the GameBoyMemory `&`/Box on the stack as the overrun target, not the vtable.
Stopping here per the task's "report and stop for parent guidance" instruction.

### Files changed this session (for parent review/revert)
- platform/pico2w/src/multicore.rs:
  * VTABLE_TRAP_POSITIVE_CONTROL back to false; one-shot test write removed;
    PENDING static kept under #[allow(dead_code)].
  * arm_cartridge_vtable_watch: MPU region 2 arming REMOVED, replaced with
    dwt_watch::publish_and_arm_raw_words([vtable_addr,0,0,0]) (precise 4-byte 0x815).
  * run_core1_worker: setup_core1_vtable_mpu arming REPLACED with the same DWT arm.
  * setup_core0_vtable_mpu, setup_core1_vtable_mpu, write_region2_rbar_rlar,
    relock/unlock_core0_vtable_region_* REMOVED (now unused with the MPU gone).
  * refresh_rom_window_cache_with_mpu_bracket: MPU unlock/relock removed; keeps the
    vtable value asserts around the cache refresh only.
  * PicoGameBoy::tick: STEP-4 FALLBACK strips the per-tick vtable value-guard and the
    RAM-code sentinel (keeps the TICK_COUNTER increment and the GameBoyMemory-pointer
    guard). RAM_CODE_SENTINELS kept under #[allow(dead_code)]; rustyboy_ram_code_guard
    extern now unused (one harmless warning).
- platform/pico2w/CRASH_DEBUG_NOTES.md: appended this session's dated subsections.
Current flashed build: DWT-only fallback, CRC 0x82055da4. Board healthy (no 288 wedge).

## #5 investigation (2026-06-14, CODEX) - OAM DMA phase brackets

### Goal and instrumentation

This pass followed the handoff's OAM-DMA hypothesis directly. It added four
software checkpoints around the emulated Game Boy OAM DMA path:

1. before `copy_dma_step`
2. after `copy_dma_step`
3. before publishing the completed OAM image to core 1
4. after publishing

Each checkpoint validates the outer `GameBoy.memory` Box pointer, cartridge
vtable, bus-event queue header, core-1 transport handles, and (in the final
build) cached ROM-window flag/pointers/lengths. The copy interval also snapshots
the words immediately before and after the embedded OAM array. A failure records
sentinel `CFSR=0xD6A00002`, phase, reason, DMA source/progress/count, observed,
and expected values.

The previous layout-confounding `interrupt::free(|_| gb.tick())`, fixed-address
audio-tx MPU range, and stale vtable DWT arming were removed. The core-0 RAM-code
MPU upper limit now follows the actual `_SEGGER_RTT` address.

### Positive control - PASS

A one-shot synthetic checkpoint was flashed and captured standalone:

- phase: `after-copy`
- reason: `word after OAM changed`
- DMA: source `0xC000`, progress `4`, count `4`
- observed `0xDEADBEEF`, expected `0xFEEDFACE`
- record sentinel: `0xD6A00002`

This proves the checkpoint guard, scratch persistence, flash commit, and decoder.
The synthetic trigger was then removed.

### Hardware results

Artifacts:

- `/tmp/pre_oam_dma_checkpoints_20260614.bin` - crash sector before this pass
- `/tmp/oam_dma_trial_01.json`
- `/tmp/oam_dma_trials_20260614/trial_02.json` through `trial_41.json`

#### Build A - initial phase brackets

ELF SHA-256: `83e5f6fb0a20128b33a8fec5cfe490a8f2016be4289ae1cfb77259f61626b9fa`

20 trials, 2 failing trials, 3 HardFault records, zero real OAM checkpoint
sentinels.

The important capture is trial 8:

- first fault: core 0, `PC=0x10003C74`, `CFSR=0x00008200`,
  `BFAR=0x2A112127`
- symbolization: the post-copy `oam_boundary_words_for_diagnostics` read,
  immediately after `copy_dma_step` returned
- the before-copy pointer/invariant checkpoint and pre-copy boundary read had
  both succeeded

Thus corruption first became observable across the `copy_dma_step` call
interval in this layout. The diagnostic dereferenced the now-bad memory pointer
before the after-copy invariant check could convert it into a sentinel.

Trial 18 reproduced the known downstream core-1 queue read:
`PC=0x1001B0E2`, `BFAR=0xC0000000`.

#### Build B - raw Box-pointer check moved before post-copy boundary read

ELF SHA-256: `6bed31ed580007ae52a236cbd44e29ce37bc0b97578e041c1eeb8e72d24bae49`

Trials 21-23 all produced the same core-0 panic:

- panic location `gameboy.rs:607`
- this is the tracked caller line for `copy_dma_step`
- zero OAM checkpoint sentinels

This strongly associates the active failure with the OAM-copy call in this
layout, but the panic record does not contain the internal panic message or DMA
source state.

#### Build C - cached ROM-window raw guards

ELF SHA-256: `1f973909150a4a771a412720f68a77a9ddd003c32eaf7b99b5fc72fa7c274f07`

Added non-dereferencing checks for the cached ROM-window flag, pointers, and
lengths before any OAM-DMA raw ROM slice can be formed.

Trials 24-41:

- 17 readable trials; trial 28 had a transient SWD read failure
- 10 failing trials
- 2 panic records (`spsc.rs:185`)
- 11 HardFault records plus 2 downstream watchdog records
- zero OAM phase sentinels
- zero ROM-window cache sentinels

The deterministic `gameboy.rs:607` panic disappeared when these guards changed
the layout. The original signatures remained frequent: `PC=0x0000FE9E`, wild
SRAM/invalid PCs, `spsc.rs:185`, and cross-core queue faults.

### Verdict

OAM DMA **does track as a narrow timing hotspot**:

- one layout first observed the bad `GameBoy.memory` pointer on the first load
  immediately after `copy_dma_step`
- the next layout panicked at the `copy_dma_step` call on 3/3 boots

However, this pass does **not** prove that the bounded OAM copy is the corrupting
writer:

- no real `0xD6A00002` checkpoint fired across 40 readable real trials
- DMA source/progress bounds and OAM-adjacent words did not report a violation
- ROM-window cache guards did not report a bad raw source pointer/length
- small instrumentation changes moved the victim/signature again

The best current interpretation is still a layout-sensitive wild store or stack
smash whose first visible use can land inside the OAM-DMA interval. OAM DMA is a
good temporal bracket, not yet a convicted writer.

Recommended next capture: keep the minimal before/after raw Box-pointer check,
record the active packed OAM DMA state in a single global word so panic/HardFault
records always include source/progress/count, and watch the current build's
actual `GameBoy.memory` pointer slot rather than adding broader layout-changing
guards.

Current flashed build: Build C above, synthetic control removed.

Validation:

- `cargo check --release` passed
- `cargo build --release` passed
- `cargo test-host`: 191/191 passed
- decoder AST parse and `git diff --check` passed

## #5 investigation continued (2026-06-14) — HOST REPLAY EXONERATES CORE EMULATION; bug is cross-core/platform

After ~12 on-device capture attempts kept moving the victim per build, we took
the fight off-device. A host replay harness reproduces the device's exact
trajectory under sanitizers, immune to the on-device layout-sensitivity.

### Harness + fidelity

New host test `core/tests/replay_poisoned_save.rs` loads the device's exact
poison ROM + save state and replays `GameBoy::<LocalTransport>::tick()` on the
host, where ASan/Miri can see an illegal access directly.

- Fixtures: ROM dumped from XIP flash — rom_id `21f712e2`, MBC1+RAM+BATTERY
  (`[0x0147]=0x03`), 512 KiB, "ZELDA" / Link's Awakening.
- The poison save state is NOT in flash. It lives on the microSD at
  `SAVES/21F712E2/SLOT0.RBS` and was read directly off the card: 25060 B,
  `RBSS` v2 blob, exactly what `SaveState::from_blob` expects.
- Same generic `tick()` source as the device; the portable MBC provides
  `rom_windows()` so the unsafe OAM-DMA `from_raw_parts` path is exercised
  identically.

### Validation gate — PASSED exactly

Host post-load state matched the device boot log to the digit:

| Field           | Value          |
|-----------------|----------------|
| `cycle_counter` | `15267416632`  |
| `rom_bank`      | `2`            |
| `PC`            | `0x1807`       |
| `HL`            | `0x17bb`       |

The replay is provably on the same trajectory as the device.

### Result — 20M ticks, ZERO corruption

ASan (`-Zsanitizer=address`), 20,000,000 ticks — ~10x past the device's
deterministic crash point (~tick 2M / cycle_lo ≈ 2.395B). The run reached
`PC=0x03ce` (the historical trigger) repeatedly and completed clean: no panic,
no ASan trap, no divergence.

### Conclusion — core emulation is exonerated as the root

The replay is deterministic and faithful. A core-logic bug (bad DMA length, OOB
index, stale ROM pointer in CPU/MBC/memory/bus-events) would fire at the SAME
cycle on host — it does not. A safe-Rust OOB would have panicked even without
ASan; an unsafe heap OOB (e.g. `copy_dma_step`'s `from_raw_parts`) would have
tripped ASan. Neither happened. So the portable `rustyboy-core` emulation is NOT
the corruptor.

By elimination the writer is in the **platform / cross-core layer** — what the
host lacks: `Core1Transport`, `SharedWorkerState`, the cross-core copies of
vram/oam/bus-event/audio data, real core0/core1 concurrency, embassy/async, and
32-bit layout. This reconciles all prior evidence:

- victims were almost always transport-related
- the GB-data payload (`0x0000FE9E`) is emulator data the transport copies
  across cores
- the bug is timing- and layout-sensitive (race hallmarks)
- the notes already hit the "Release orders but does not force completion; only
  DSB drains the write buffer" landmine

**Second fingerprint:** the microSD `SAVES/` is corrupted *only* in the
`21F712E2` directory — 220 trashed entries with GB-data-looking byte-soup
filenames, multi-GB sizes, impossible timestamps — while every other ROM's save
dir is clean. Consistent with a platform-layer corruptor reaching the SD/FAT
write buffers while this ROM runs.

### Caveat

ASan misses intra-allocation OOB and the host is 64-bit. Miri at `i686` would
close both gaps but cannot practically reach the ~2M-tick crash cycle
(interpreted, too slow). That residual is itself the platform-layout/concurrency
hypothesis, so it sharpens rather than changes the direction.

### Next

Hunt the cross-core transport for a data race / missing barrier:

1. A ThreadSanitizer 2-thread host harness — core-0 `GameBoy` + core-1
   `GameBoyWorker` through a `Core1Transport`-faithful transport.
2. A targeted audit of `Core1Transport` / `SharedWorkerState` / the
   `write_*_range` / `write_ppu_registers` delivery + audio path for
   unsynchronized cross-core access.

Note: x86 (TSO) is more strongly ordered than ARM, so pure weak-ordering bugs
may not reproduce under TSan even though genuine data races will.

---

## #5 Weak-memory-ordering audit (2026-06-14) — cross-core publish/consume review

Pure code review of the cross-core transport for an ARM weak-ordering /
write-buffer-drain bug, after ASan (single-thread) and TSan (two-thread x86/TSO)
both ran clean. Files: `platform/pico2w/src/multicore.rs`,
`core/src/ipc/{worker,transport,local}.rs`, plus the dependency
`embassy-rp/src/critical_section_impl.rs` and `heapless-0.9` `spsc.rs`.

### KEY FINDING (load-bearing): `critical_section` on RP2350 has NO DSB

`embassy-rp/src/critical_section_impl.rs` `RpSpinlockCs::acquire/release` use
ONLY `core::sync::atomic::compiler_fence(SeqCst)` (a compile-time-only barrier,
emits zero instructions) plus the SIO Spinlock-31 MMIO read/write. `Spinlock31`
(`embassy-rp/src/spinlock.rs`) `try_claim` is a device read, `release` is a
device write — and **there is no `dsb()` anywhere in acquire or release**.

Consequence: a `critical_section::with` block on this silicon does NOT drain the
store buffer for **Normal SRAM** writes made inside it. A device-ordered MMIO
write to the spinlock register orders the MMIO access, but on Cortex-M33 it does
not force completion/visibility of buffered Normal-memory stores. This is the
exact landmine from #2's reverted-SPSC writeup: *"Release orders but does not
force completion; only DSB drains the write buffer."* The whole transport was
(re)built on the assumption that `critical_section` supplies the cross-core
barrier for the shared **data buffers**. For the small atomics it nominally does
(Acquire/Release pair on the SAME location), but for the raw `UnsafeCell` buffers
published *under the lock but gated by a SEPARATE atomic*, it does not guarantee
physical visibility before the consumer observes the gating atomic.

### Ranked suspicious sites

#### 1. STRONGEST — frame publish: raw buffers gated by a separate atomic, no DSB
- Producer (core 1): `publish_frame_locked`, `multicore.rs:330-390`. Inside
  `critical_section::with` (entered at :327): writes `dirty_rows` (:362-372) and
  `native_frame_slots[target]` (:381-383) — both raw `UnsafeCell<[…]>` in Normal
  SRAM — then `published_frame.store(Release)` (:387) and
  `published_frame_seq.fetch_add(AcqRel)` (:389). **No `dsb()` after the buffer
  writes and before the publishing atomics.**
- Consumer (core 0): `poll_output` Acquire-loads `published_frame_seq`
  (:1199) to set `frame_ready`; later `published_native_frame` Acquire-loads
  `published_frame` (:967), marks the slot busy (:968), and hands out
  `&native_frame_slots[slot]` (:974); `published_dirty_rows` (:992-997) reads the
  raw `dirty_rows` buffer with **no atomic of its own** (:996), relying purely on
  the earlier `published_frame` Acquire.
- Hazard ARM permits: the `dirty_rows` / slot stores can still be sitting in core
  1's store buffer when core 1's `published_frame.store(Release)` /
  `published_frame_seq.fetch_add` become visible to core 0 (Release on M33 is a
  plain `str`/`strex`, no implicit DSB; the surrounding `critical_section` adds no
  DSB either). Core 0 then observes the new seq/slot via Acquire and reads the
  raw slot + dirty bitmap **before core 1's buffer stores have drained** → it
  consumes a torn/stale 23 KB frame and a stale dirty bitmap. The Acquire/Release
  edge is correct in the C++ abstract model, but #2 already PROVED empirically on
  this exact board that a Release store can be observed while the data it
  "published" is still buffered (core 0 `lda` read 1413 while SRAM held 1414).
- Why it best matches #5: this is the only large (KB-scale) cross-core buffer
  publish, and it is gated by a *separate* atomic (the textbook fragile pattern),
  unlike every ticket path which is hard-drained by `ack_ticket`'s DSB. It fires
  ~once per rendered frame, i.e. at a frame-rate cadence that reaches "~2M ticks"
  in the minutes-scale window before the crash, and it is deterministic per ROM
  because dirty-row content is ROM/scene-determined. It is invisible to TSan-on-
  x86 (TSO retires the store buffer in order, so the Release is never observed
  ahead of the data; and TSan treats the Acquire/Release pair as a valid
  happens-before, suppressing any race report) and invisible to the single-thread
  host (no second core). The consumed data is **GB framebuffer bytes / dirty-
  bitmap words** — exactly "GB-shaped data" — and `send_frame` then drives DMA
  setup (CASET/RASET ranges) and a `&dirty_rows` array from that data; a stale/
  torn dirty bitmap or slot can mis-size a copy/DMA and scribble GB-shaped bytes
  over adjacent core-0 stack objects (the transport pointer triplet, the worker-
  reference stack slot — precisely the observed victims).
- Proposed minimal fix: add `cortex_m::asm::dsb()` in `publish_frame_locked`
  immediately AFTER the slot+dirty buffer writes and BEFORE
  `published_frame.store(Release)` (i.e. between :383 and :387). That drains core
  1's store buffer so the buffers are physically visible before the gating atomic
  can be observed by core 0 — the same discipline `ack_ticket` already uses.
  (Belt-and-suspenders: a matching `asm::dsb()` on core 0 after the
  `published_frame` Acquire in `published_native_frame`, before dereferencing the
  slot, to defeat any speculative/early read — though the producer-side DSB is the
  necessary one.)
- Validation: standalone soak past the deterministic repro point with the
  producer DSB added; if the transport-smash / worker-ptr-guard records stop, the
  ordering gap is confirmed. Cheap (one DSB per published frame, ~60/s).

#### 2. Frame "unpublish" / clear path — same missing-DSB pattern, reset cadence
- `clear_published_frames_locked` (:401-420): inside `critical_section`, zeroes
  all slots and `prev_row_hashes` (raw buffers), clears `native_frame_busy`, then
  `published_frame.store(0, Release)` / `published_frame_seq.store(0, Release)`.
  No DSB. Same class as #1 but only on save-state / sync / reset, so far rarer.
  The bigger subtlety: this runs on **core 0** while **core 1 is still live**
  (per the corrected comment at :392-398) — but #1's repro is in steady-state
  gameplay, so this is secondary. Fix: DSB before the two Release stores.

#### 3. PPU snapshot (`live_ppu_snapshot`) gated by `ppu_render_version`
- Producer (core 0): `write_live_vram_range`/`write_live_oam_range`
  (:448-466) and `copy_live_ppu_snapshot` (:438-446) write the snapshot inside
  `critical_section`, then bump `ppu_render_version` (`fetch_add`/`store Release`)
  **outside** the lock.
- Consumer (core 1): loop at :1890 Acquire-loads `ppu_render_version`; if changed,
  re-enters `critical_section` and reads the snapshot via `try_borrow()`
  (:1892-1899).
- Assessment: LOWER risk than #1 because the consumer re-takes the SAME
  `critical_section` and reads the buffer THROUGH the `RefCell` inside it, and the
  version load is Acquire vs the producer's Release on the same location. The data
  read is itself inside a (re-acquired) lock that the producer also held, so there
  is a lock-ordered edge on the buffer, not just a separate-atomic gate. Still,
  there is no DSB, so a torn snapshot is theoretically possible if the version
  Release is observed before the in-lock writes drain; but the in-lock re-read
  narrows the window vs #1's lock-free raw-pointer read. Watch, don't fix first.
  If fixing: DSB after the snapshot copy, before the `ppu_render_version` bump.

#### 4. Command queue (spsc) — SOUND
- `heapless-0.9` `spsc::inner_enqueue` writes the slot then `tail.store(Release)`;
  `inner_dequeue` does `tail.load(Acquire)` then reads the slot. Producer (core 0)
  additionally `asm::dsb()` after enqueue, before `sev()` (:784). Consumer (core
  1) `asm::dsb()` before `wfe()` when the queue is empty (:1870). The Release/
  Acquire on `tail` plus the producer DSB give a correct, drained edge. OK.

#### 5. Audio queue (spsc, core 1 → core 0) — SOUND via ticket DSB
- Core 1 `audio_tx.enqueue` per sample in `DrainAudio` (:1908) has NO per-enqueue
  DSB, BUT the path is ticket-serialized: core 1 fills the queue, then
  `ack_ticket` (:1910) does `sync_complete.store(Release)` + **`dsb()`** (:1687-
  1688); core 0 spins in `wait_for_ticket` (Acquire, :957) and only drains AFTER
  the ticket lands. The `ack_ticket` DSB drains the audio slot stores before the
  ticket is observable. OK — this is the model #1 should copy.

#### 6. `pending_if_bits` cross-core RMW — SOUND
- `fetch_or` (core 1, :431-434) and `swap` (core 0, :1207) both inside
  `critical_section` (AcqRel). Single small value, both sides serialized; no
  separate-atomic-gated buffer. OK (matches prior 2026-06-03 audit).

#### 7. `apu_nr52` / `ppu_ly` / `ppu_stat` — SOUND
- Release stores in `publish_worker_output` (:423-425), Acquire load of
  `apu_nr52` in `poll_output` (:1213). Independent scalar bytes, no buffer gated
  behind them; a one-frame-stale scalar is harmless and not pointer-shaped. OK.
  (Note `ppu_ly`/`ppu_stat` in `poll_output` are actually read from the core-0-
  local `lcd_timing_io` mirror, not the shared atomics, so even less exposure.)

### Bottom line
The single best-fit candidate is **#1, the frame publish in
`publish_frame_locked` (multicore.rs:330-390)**: raw KB-scale GB-data buffers
(`native_frame_slots`, `dirty_rows`) published under a `critical_section` that —
on RP2350 — performs no store-buffer drain, gated by a *separate* atomic
(`published_frame` / `published_frame_seq`) that core 0 Acquire-loads before
reading the raw buffers with no DSB on either side. This is the exact failure
mode #2 proved real on this board, at frame cadence, deterministic per ROM,
producing GB-shaped data that downstream `send_frame`/DMA can scatter onto
core-0 stack/pointer objects — and structurally invisible to both the
single-thread ASan host and the x86/TSO TSan host. Recommended first change: one
`cortex_m::asm::dsb()` between the buffer writes (:383) and the
`published_frame.store(Release)` (:387). If a soak still smashes after that, the
remaining hypothesis narrows to silicon or a non-transport wild store.
