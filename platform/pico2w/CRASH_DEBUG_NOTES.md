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
