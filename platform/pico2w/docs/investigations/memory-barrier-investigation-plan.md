# RP2350 Cross-Core Memory-Ordering Investigation

Last updated: 2026-06-15

## Objective

Confirm or reject the hypothesis that crash bug #5 is caused by missing hardware
memory barriers in the RP2350/Embassy cross-core synchronization paths.

The primary candidate is the frame-publication path in
`src/multicore.rs`: Core 1 writes `native_frame_slots` and `dirty_rows`, then
publishes the slot through `published_frame` and `published_frame_seq`; Core 0
observes the atomics and reads the raw buffers.

## Current Evidence

- The portable emulator replay completed 20 million ticks under ASan without
  corruption. This points toward the platform/cross-core layer.
- Focused TSan frame stress found two real ownership races. Reserving the
  published slot under the producer's critical section and storing dirty bits
  per slot removed both reports.
- The pinned Embassy revision's RP critical-section implementation uses a
  compiler fence and SIO spinlock MMIO, with no `DMB` or `DSB`.
- The current release image emits ARMv8-M `STLB`/`LDAB` instructions for the
  frame publication/consumption atomics.
- The RP2350 has a global exclusive monitor. Older investigation notes claiming
  cross-core exclusive atomics are inherently unsafe on RP2350 are incorrect.
- The Raspberry Pi SDK warns about RP2350-E2 affecting raw SIO spinlocks and
  uses atomic-memory synchronization as its workaround. Embassy still uses SIO
  spinlock 31 in the pinned revision.
- A verified real frame-publication `DSB` did not prevent corruption.
- Application-wide critical-section `DMB` barriers did not improve behavior
  and caused repeated resets immediately after main-loop entry in one image.
- The matched local critical-section backend without DMB remained in the main
  loop for more than 65 seconds.
- The crash sector is full, so current resets cannot produce fresh persistent
  records until it is cleared or marked read.
- `cargo check --release` passes in the current diagnostic-heavy tree.

## Hypotheses

### H1: Frame Publication Needs A Completion Barrier

Core 1's raw frame/dirty-bitmap stores are not sufficiently visible before Core
0 observes the publishing atomic. A producer-side `DSB` immediately before
`published_frame.store(Release)` should eliminate direct litmus failures and
crash bug #5.

Status: rejected as a sufficient explanation. The linked real-path DSB image
still reproduced corruption.

### H2: Embassy Critical-Section Lock Needs Hardware Barriers

The SIO spinlock provides exclusion but not the required memory-ordering
semantics for data protected by `critical_section::with`. A `DMB` after outer
lock acquisition and before outer lock release should eliminate direct litmus
failures and crash bug #5.

Status: not supported. The application-wide DMB image passed its litmus but
reset sooner than its no-DMB control.

### H3: The Barrier Theory Is A Layout/Timing False Positive

The release/acquire publication edge is already sufficient. Any apparent
improvement from adding a barrier comes from changed code layout or timing,
rather than corrected ordering.

Status: supported by the current device comparisons, though more repeated
control trials are needed to quantify the layout/timing sensitivity.

### H4: The Frame Protocol Has A Separate Ownership Race

Core 0 can load a published slot before setting its busy flag while Core 1
selects a target slot. The single shared `dirty_rows` buffer may also be updated
for a later frame while Core 0 consumes an earlier slot. These are protocol
issues, not missing memory barriers.

Status: confirmed in the focused host TSan model.

### H5: Embassy's SIO Spinlock Is Affected By RP2350-E2

A spurious release of spinlock 31 breaks mutual exclusion. This can mimic a
missing-barrier failure but will not be repaired reliably by adding a `DSB`.

## Experimental Discipline

- Use standalone runs for crash capture. Probe vector-catch prevents firmware
  HardFault records from committing.
- Keep each experimental change minimal and record the image CRC.
- Include a same-size or equivalent timing control where practical.
- Blank the crash sector before each independent trial.
- Use the poisoned Zelda save-state trajectory used by the existing replay
  harness.
- Run enough baseline trials to establish the current image's failure rate
  before comparing fixes.
- Do not call a soak improvement causal unless a direct synchronization litmus
  also distinguishes the variants.

## Work Plan

### Phase 1: Establish A Minimal Baseline

Status: completed with a negative barrier result

1. Inventory the currently enabled MPU, DWT, OAM checkpoint, pointer guard, and
   crash-record features.
2. Preserve crash-record capture and DMA diagnostics.
3. Gate or remove obsolete layout-heavy instrumentation for the device trial
   matrix.
4. Build and record the baseline image CRC and relevant symbol addresses.
5. Run a baseline trial set to measure failure rate and signatures.

Exit criterion: one reproducible baseline image with a documented CRC, enabled
instrumentation list, and trial failure rate.

Current default-image inventory:

- Core 0 DWT comparators are disarmed at boot.
- Core 0 installs a privileged-read-only MPU region over `.data` RAM code.
- Core 1 disarms DWT and installs an MPU region that makes the Core 0 stack
  privileged-read-only from Core 1.
- `GameBoy::advance_dma_bulk()` runs OAM-DMA invariant checks before and after
  copying and publishing.
- `GameBoyMemory::copy_dma_step()` separately validates cached ROM windows.
- `PicoGameBoy::tick()` validates the `GameBoy.memory` Box pointer every tick.
- Core 1 validates its `shared`, `worker`, and worker PPU pointers around every
  command.
- `Core1Transport` checks its queue/shared pointers on hot transport paths.
- Save-state restoration emits phase-by-phase pointer/vtable diagnostics.
- Crash records include DMA channel state. This is cold-path instrumentation
  and should remain enabled.

The hot-path checks and custom MPUs make the current default image useful for
capture, but unsuitable as the only baseline for a layout-sensitive comparison.
The baseline mechanism must make these controls explicit rather than silently
depending on the default feature set.

### Phase 2: Add Direct Cross-Core Litmus Tests

Status: in progress

Add boot-time or diagnostic-mode tests that repeatedly exercise:

1. Raw payload protected only by Embassy `critical_section`.
2. Raw payload followed by a Release publication atomic and an Acquire consume.
3. A frame-sized or chunked payload matching the application publication path.
4. Sequence number, complement, and checksum validation so stale/torn reads are
   detected directly rather than inferred from a later crash.

Variants:

- A: existing synchronization.
- B: `DMB` after critical-section acquire and before release.
- C: producer `DSB` immediately before frame publication.
- D: matching consumer barrier, if needed as a discriminator.
- E: timing/code-size control that performs comparable work without a barrier.

Exit criterion: repeatable pass/fail results that identify whether either
barrier changes visibility or exclusion on the actual RP2350.

Implemented diagnostic features:

- `memory-barrier-litmus`: baseline with no added hardware barrier.
- `memory-barrier-litmus-lock-dmb`: application-wide `DMB SY` after outer
  spinlock acquisition and before outer spinlock release; the lock litmus uses
  that same backend.
- `memory-barrier-litmus-producer-dsb`: `DSB SY` after the synthetic payload
  writes and immediately before Release publication, plus `frame-publish-dsb`
  in the real native-frame publication path.
- `frame-publish-dsb`: `DSB SY` after the real frame copy and immediately
  before publishing the selected frame slot.

The tests run in `Core1Transport::new`, before the normal Core 1 worker starts,
and reuse native frame slot 0 rather than allocating another large SRAM buffer.
Each image performs 4,000 lock-protected handoffs and 4,000 Release/Acquire
publications over a 4,096-byte sequence-dependent payload.

### Phase 3: Exercise The Full Frame Protocol Under TSan

Status: completed

1. Update `tools/tsan_harness` to consume each published frame.
2. Call `published_native_frame()`, copy/hash the frame and dirty bitmap, then
   call `release_native_frame()`.
3. Validate frame sequence/checksum coherence.
4. Add scheduling pressure around slot selection, busy marking, publication,
   and release.
5. Run the TSan self-test first, then the faithful replay.

Exit criterion: either a concrete TSan report/protocol mismatch or a clean run
that genuinely covers frame publication and consumption.

Result: the focused high-rate test reported both predicted races. Reserving the
published slot under the same critical section as producer selection and
storing dirty metadata per frame slot removed both reports.

### Phase 4: Application-Level Device Matrix

Status: in progress

For each variant, run at least 20 standalone trials past the deterministic
failure window:

| Variant | Direct litmus | App trials | Required interpretation |
| --- | --- | --- | --- |
| Baseline | Record result | 20+ | Establish control rate |
| Producer `DSB` | Must improve if H1 | 20+ | Tests H1 |
| Lock `DMB` | Must improve if H2 | 20+ | Tests H2 |
| Timing control | Should match baseline | 20+ | Detects layout/timing masking |
| Protocol fix, if needed | Depends on TSan | 20+ | Tests H4 |

For every trial record:

- Image CRC.
- Trial duration.
- Crash-sector validity.
- Crash kind, core, PC, CFSR/HFSR, and fault address.
- Watchdog-only reset or silent reboot.
- DMA snapshot.
- Whether the direct litmus reported a mismatch.

### Phase 5: Decision

Status: pending

Confirm H1 or H2 only if all of the following hold:

1. The corresponding direct litmus fails without the barrier.
2. The barrier variant fixes the direct litmus.
3. The barrier variant materially reduces or eliminates bug #5.
4. The timing control does not produce the same improvement.
5. The result survives a clean rebuild and repeated standalone trials.

Reject the barrier explanation if the direct litmus remains clean and application
results track layout/timing rather than the barrier variant.

## Progress Log

### 2026-06-15: Initial Review

- Read `CRASH_DEBUG_NOTES.md` through the latest weak-memory audit.
- Inspected all modified and untracked investigation files.
- Verified the proposed frame `DSB` has not yet been applied.
- Verified Embassy critical-section acquire/release contains no hardware memory
  barrier.
- Verified the release image uses `STLB`/`LDAB` for the frame handoff.
- Corrected the older exclusive-monitor premise using the RP2350 datasheet.
- Identified that the TSan harness currently never consumes published frames.
- Ran `cargo check --release`: passed.
- Began Phase 1 by inventorying enabled diagnostics and identifying which ones
  perturb hot-path layout.

### 2026-06-15: Baseline Inventory

- Confirmed most bug #5 diagnostics are unconditional for ARM builds rather
  than selected by a Cargo feature.
- Identified the per-tick, per-command, and OAM-copy checks listed in Phase 1.
- Decided to exercise the missing TSan frame-consumer path before changing
  firmware layout. A host-detected protocol race would change the device matrix
  and avoid unnecessary flash/soak cycles.

### 2026-06-15: TSan Frame-Consumer Coverage

- Extended `tools/tsan_harness` so Core 0 observes each new
  `published_frame_seq`, acquires the published slot, hashes the full native
  frame and dirty bitmap, and releases the slot.
- Added scheduler yields in the published-slot load/reserve window and before
  reading the shared dirty bitmap.
- `cargo check -p rb-tsan-harness --target x86_64-unknown-linux-gnu` passed.
- Ran the `tsan_selftest` positive control. TSan reported the intentionally
  injected cross-thread race, confirming that the instrumentation is active.
  The short 10,000-tick run then stopped at the frame-coverage assertion because
  it had not yet reached a published frame.
- Ran the faithful harness without `tsan_selftest` for 3,000,000 ticks. It
  consumed 243 published frames, completed with digest `0xef36f984`, and
  produced no TSan report.
- This clean run rejects an always-present host-level frame-buffer race on the
  exercised schedule. It does not validate RP2350 memory ordering, and the
  normal publication cadence may still miss the narrow load-then-reserve or
  shared-dirty-bitmap overlap. Phase 3 remains open for a focused high-rate
  protocol stress mode.

### 2026-06-15: First On-Device Litmus Matrix

- Added the feature-gated RP2350 lock and publication litmus tests described in
  Phase 2. Normal release firmware still compiles with the tests absent.
- The first 20,000-iteration baseline exceeded the 16-second watchdog window
  before reporting. Reduced the bounded startup test to 4,000 iterations; this
  completes in about 4.0-4.2 seconds.
- Disabled probe-rs HardFault vector catch for the recorded trials so firmware
  crash handling remains active. The earlier caught "Exception" occurred after
  both test loops had completed and was a misleading observation point, not a
  litmus mismatch.
- Verified the linked instructions:
  - Baseline publication uses `STLB`/`LDAB` with no added `DMB` or `DSB`.
  - Lock variant emits `DMB SY` after acquire and before release on both cores.
  - Synthetic producer variant emits `DSB SY` directly before the litmus
    publication stores. At this stage it did not alter the real application
    frame-publication path.
- Device results:

| Variant | Image CRC | Result | Litmus duration |
| --- | --- | --- | --- |
| Baseline | `0xd0cf9254` | PASS, 4,000 + 4,000 handoffs | 4.073 s |
| Lock `DMB` | `0x2bf100c6` | PASS, 4,000 + 4,000 handoffs | 4.046 s |
| Synthetic producer `DSB` | `0x39de671a` | PASS, 4,000 + 4,000 handoffs | 4.155 s |

- All three images proceeded through poisoned save-state restoration and
  entered the main loop after the litmus.
- The baseline PASS means this bounded direct test did not reproduce a
  visibility or exclusion failure that either barrier could fix. It does not
  satisfy Phase 2's exit criterion because there is no barrier-sensitive
  baseline failure yet.

### 2026-06-15: Focused Frame-Protocol Stress And Fix

- Added `RB_FRAME_STRESS_ITERATIONS` to `tools/tsan_harness`. This bypasses ROM
  replay and publishes full 23,040-byte synthetic frames at high rate while
  Core 0 exercises the production slot and dirty-bitmap API.
- The unchanged protocol produced two independent TSan reports:
  - Core 0 copied the single shared dirty bitmap while Core 1 rewrote it for a
    later publication.
  - With dirty reads suppressed to isolate the slot path, Core 0 read a frame
    slot while Core 1 filled the same slot. This is the predicted
    load-published-slot, then mark-busy race.
- Applied the protocol correction to firmware and the host model:
  - Core 0 now loads and marks the published slot busy while holding the same
    critical section used for producer slot selection.
  - Dirty-row metadata is stored per native-frame slot and is read from the
    held slot.
  - The shared-state increase is 40 bytes: two additional five-word bitmaps.
- Acceptance results:
  - 50,000 full-frame synthetic publications under TSan: no report, zero torn
    frames, zero dirty mismatches.
  - Faithful poisoned-save replay for 3,000,000 ticks: no report, 243 consumed
    frames, digest `0xef36f984`.
  - Normal `cargo check --release`: passed.

### 2026-06-15: Protocol-Fix Device Smoke And Real DSB Correction

- Flashed the protocol fix with no added frame-publication barrier, image CRC
  `0x54af43be`. The direct litmus passed, save restoration completed, and the
  firmware rebooted shortly after entering the main loop.
- The reboot reported that the crash-log sector was full, so this run could not
  commit a fresh crash record. A direct probe read was unavailable in this
  session; the reset cause is therefore not classified.
- Flashed the then-current `memory-barrier-litmus-producer-dsb` image, CRC
  `0x6ef2efdf`. It remained in the main loop for more than 50 seconds.
- Auditing that image showed the feature only changed the synthetic litmus.
  The application result is reclassified as a layout/timing observation, not
  evidence for H1.
- Changed `memory-barrier-litmus-producer-dsb` to include a dedicated
  `frame-publish-dsb` feature in the real `publish_frame_locked` path.
- Verified the rebuilt linked sequence: the 23,040-byte frame copy returns at
  `0x1001c092`, followed by `DSB SY`, then the frame-slot `STLB` publication at
  `0x1001c098`.
- Flashed that verified real-path image, CRC `0xc8bb9970`. The direct litmus
  passed in 4.21 seconds and the firmware entered the main loop normally.
- At 54.30 seconds from boot it reported
  `ppu=0x2002b62c/want 0x0000001c`, then watchdog-reset. The live worker PPU
  pointer was still correct; the corrupted value was the diagnostic baseline
  `EXPECTED_WORKER_PPU_STATE_PTR` at `0x20065be4`.
- This is a direct negative result for H1: a correctly placed producer `DSB`
  did not prevent the application corruption in this trial.
- The next device variant will test H2 by replacing Embassy's feature-gated
  critical-section backend with an equivalent local backend and adding
  `DMB SY` only after outer spinlock acquisition and before outer release.
- Implemented that local backend and changed
  `memory-barrier-litmus-lock-dmb` to enable it application-wide. Both the
  baseline and DMB configurations pass `cargo check --release`.
- Linked-code verification:
  - Local-backend baseline has no hardware barrier in
    `_critical_section_1_0_acquire` or `_critical_section_1_0_release`.
  - H2 variant emits `DMB SY` after outer acquisition at `0x1001e7d2` and
    before outer release at `0x1001e7ee`; recursive entries bypass both.
- Flashed the H2 image, CRC `0x5bbe94a9`. Its 4,000-iteration lock litmus
  passed in about 4.05 seconds on each boot.
- The image then watchdog-reset almost immediately after entering the main loop
  and repeated that cycle. It did not improve the application failure and
  changed its timing substantially.
- Because this experiment also replaced Embassy's backend with the equivalent
  local implementation, the next mandatory control is the same local backend
  without DMB. That separates a barrier effect from backend/layout effects.
- Flashed that no-DMB local-backend control, CRC `0xae006b1e`. Its baseline
  litmus passed in 4.18 seconds and it remained in the main loop for more than
  65 seconds with no reset.
- The backend substitution therefore does not explain the H2 image's immediate
  reset loop. Adding application-wide DMB changed timing/behavior but did not
  repair the crash.

## Results

H4 is confirmed and has a host-verified correction. This is a genuine Rust data
race and must be removed before application soaks can meaningfully attribute
any remaining crash to hardware memory ordering.

The first RP2350 litmus matrix remains clean in the no-barrier baseline, so it
does not confirm H1 or H2 and weakens the claim that these synchronization paths
fail routinely without an added barrier.

The protocol fix remains required, but verified frame-publication DSB and
application-wide critical-section DMB variants both failed as crash fixes. The
missing-memory-barrier hypothesis is rejected as the primary explanation for
bug #5 on the evidence collected so far.

The next investigation branch is writer identification:

1. Clear or mark-read the full crash sector before more standalone trials.
2. Use the no-DMB local-backend image as the matched control and repeat enough
   runs to establish its failure distribution.
3. Arm an exact DWT watchpoint or a post-initialization MPU read-only region on
   `EXPECTED_WORKER_PPU_STATE_PTR` and adjacent diagnostic words on both cores.
4. Preserve DMA state in the crash record because CPU watchpoints do not catch
   DMA writes.
5. If the exact watch does not fire before corruption, bracket the target with
   canaries and use MPU/DMA evidence to distinguish a CPU wild store from DMA.

> Update (2026-06-15): writer-ID is now a `-Z stack-protector=strong`
> deterministic repro (golden binary `/tmp/rb-sp-strong`, crc `0x20f47b81`);
> bug #5 is a fixed-address wild contiguous write/memcpy hitting `copy_dma_step`
> (the VICTIM). DWT value-match is dead on this silicon; next is an external
> OpenOCD heap write-watchpoint. See the 2026-06-15 OAM-DMA section at end of
> file and `platform/pico2w/OAM_DMA_BISECTION.md`.

### 2026-06-15: Exact Expected-PPU-Word Watch

- Decoded the previously full crash sector and invalidated its header with
  `crash_decoder.py --probe --mark-read`; fresh records can now be committed.
- Added feature `expected-worker-ppu-watch`.
- After the sole legitimate initialization of
  `EXPECTED_WORKER_PPU_STATE_PTR`, Core 0 publishes and arms an exact 4-byte DWT
  write watch. Core 1 arms the same published address at worker startup.
- Only comparator 0 is active; adjacent diagnostic words are not watched
  because they may still receive legitimate writes.
- The standalone capture image uses the no-DMB local critical-section backend
  plus the baseline memory litmus.
- Linked symbol verification places the watched word at `0x20065be4`, exactly
  the address whose value changed to `0x0000001c` in the real-DSB failure.
- `cargo check --release --features
  memory-barrier-litmus,expected-worker-ppu-watch` passed.
- Flashed image CRC `0x3bbe1012`. Both cores reported comparator 0 armed on
  `0x20065be4`; the baseline 4,000-iteration litmus passed before the main loop.
- While probe-rs remained attached, the target stopped on a watchpoint just
  after entering the main loop. This only proves that the comparator can match:
  with external debug enabled, the core halts instead of entering the firmware
  DebugMonitor handler, so no writer PC was captured.
- Reset the target with a one-shot probe command, detached, and allowed it to
  run standalone for 90 seconds. The fresh sector contained one watchdog reset
  followed by four cascading HardFault records, but no record with the
  `0xD7170001` DWT sentinel.
- The first useful HardFault was on Core 0 in
  `GameBoyMemory::write_fast`, with a precise data bus fault at `0x20082be0`.
  Later records included Core 0 in `copy_forward_bytes` with an invalid
  destination and Core 1 executing outside mapped code. DMA was idle in every
  captured record.
- This is not yet proof that DMA wrote `0x20065be4`: the crash cascade may have
  begun before that diagnostic word changed in this particular run. Before
  changing the watch mechanism, read the live watched word and determine
  whether this trial actually corrupted it. If it remained intact, move the
  watch to an earlier, repeatable victim identified by the first-fault records.
- A direct post-run RAM read showed `0x20065be4 = 0x2002b628`, still the correct
  worker PPU pointer. The run therefore failed before corrupting the watched
  diagnostic word; its absent DWT record is not evidence for a DMA writer.
- The first useful record instead faulted in `GameBoyMemory::write_fast`, and
  the prior investigation already identified the layout-sensitive primary
  victim as the task-resident `GameBoy.memory` Box pointer field. Added feature
  `gameboy-memory-field-watch` to watch that exact field:
  - Core 0 publishes and arms the address on the first tick, after the async
    task has reached its stable runtime location.
  - Core 1 polls the published address before queue activity and re-arms when
    Core 0 publishes it.
  - Normal builds are unchanged, and the earlier expected-PPU watch remains a
    separate feature.
- `cargo check --release --features
  memory-barrier-litmus,gameboy-memory-field-watch` and `git diff --check`
  passed.
- Flashed the GameBoy-memory-field image, CRC `0xcb371212`. The final field was
  `0x2004b5ac`, containing the healthy heap pointer `0x20026368`; the litmus
  passed and the comparator was armed after the first tick.
- A 90-second standalone run produced the known Core 1 SPSC failure:
  `PC=0x1001b4f0`, precise bus fault at `0xc0000000`. The watched
  `GameBoy.memory` field still contained `0x20026368` afterward, so it was not
  the victim in this layout.
- Current disassembly shows the faulting load computes
  `command_buffer + 20 * command_head`. With command buffer
  `0x20003aa0`, the observed address solves to command head
  `0x2e666378 (mod 2^30)`, far outside the valid `0..65` index range. A live
  post-reset read showed healthy head/tail values `44/44`.
- Next capture: arm only Core 0's DWT bank on the command-queue head word. Core
  0 never legitimately writes the consumer-owned head, so this has no
  per-command DebugMonitor overhead. A hit identifies a rogue Core 0 writer; a
  repeat with a corrupt head and no hit narrows the source to Core 1 or DMA.
- Added feature `command-queue-head-watch`. It publishes and arms the exact
  queue-head address on Core 0 immediately after queue initialization and
  before Core 1 starts; Core 1's DWT bank remains disabled.
- `cargo check --release --features
  memory-barrier-litmus,command-queue-head-watch` and `git diff --check`
  passed.
- Flashed the command-head image, CRC `0x955ce4af`. Two standalone samples
  reproduced Core 0 wild-PC `0x0000fe9e` failures rather than a corrupt command
  head; post-run head/tail values remained in range.
- The second sample also committed a `TransportSmash` record while the live
  command, audio, and shared pointers were all healthy. Therefore the failed
  comparison was against one of the two stored expected queue-address fields,
  which the old record did not preserve.
- Current linked layout places those immutable fields at transport offsets
  `+0x18` and `+0x1c` (`0x2004b508` and `0x2004b50c` in that image). Added
  feature `transport-expected-fields-watch` to arm Core 0 on both words from the
  first stable `check_shared` call. Core 1 remains disarmed and its MPU already
  protects the containing Core 0 task region.
- `cargo check --release --features
  memory-barrier-litmus,transport-expected-fields-watch` and
  `git diff --check` passed.
- Device execution is paused at this point because the environment rejected
  further probe escalation after its usage allowance was exhausted. The next
  exact action is to mark the current crash sector read, flash
  `memory-barrier-litmus,transport-expected-fields-watch`, log the two linked
  field addresses, detach, run standalone, and decode without marking read.

### 2026-06-15: Transport-Immutable MPU Region (writer-ID via grouped block)

- Grouped the five write-once immutable `Core1Transport` fields (`command_tx`,
  `command_queue_addr`, `audio_rx`, `audio_queue_addr`, `shared`) into a new
  `#[repr(C, align(32))] struct TransportImmutable` placed as the first field of
  `Core1Transport`. All accesses rewritten to `self.imm.*`. The H4 frame-protocol
  fix (per-slot dirty bitmap + reserve-published-slot-under-critical-section) is
  untouched. `cargo build --release` (default), `--features
  memory-barrier-litmus,transport-immutable-mpu`, and the
  `-selftest` variant all compile.
- Added feature `transport-immutable-mpu`: on the first `check_shared` call
  (after the embassy task reaches its stable runtime location), Core 0 arms a
  priv-RO PMSAv8-M region (region 3, fresh) over `addr_of!(self.imm)` using the
  `setup_core0_data_mpu` encoding (RBAR=base|0x1D, RLAR=(base|0x1F)&!0x1F|0x01,
  MPU_CTRL=0x05). `-selftest` adds a one-shot deliberate `write_volatile` to one
  field after arming.
- POSITIVE CONTROL — PASSED. Selftest image CRC `0x4802388b`. MPU armed at
  `0x2007e7c0` (block base, 32-byte aligned). Deliberate write to
  `command_queue_addr` (`0x2007e7c8`) self-recorded standalone:
  `CFSR=0x00000082` (MMARVALID|DACCVIOL, precise), `Fault@=0x2007e7c8` (exact
  field address), `PC=0x1001a3d2` = the `str.w r0,[r5,#0x88]` write site (the
  `write_volatile`). MMFAR and PC both correct. The existing `hard_fault_rust`
  path records MMFAR via CFSR.MMARVALID as expected.
- SD GATE: the boot log did not show an SD `TimeoutACommand` or `init 288`
  wedge; staged ROM + XipCartridge built and the litmus + MPU arm completed on
  every boot. (Save-state restore logs were not observed before the fault in the
  capture image because the fault fires early — see below.)
- CAPTURE — capture image (no selftest) CRC `0xe47f918b`. MPU armed at
  `0x2007e7c0` every boot. Three independent standalone trials produced a
  BYTE-IDENTICAL record:
  `HardFault core 0, CFSR=0x00000092` (MMARVALID | **MSTKERR(0x10)** | DACCVIOL),
  `HFSR=0x40000000` (FORCED), `Fault@/MMFAR=0x2007e7d0`, `PC=0x2000401c`,
  `LR=0x20002a48`, `r4=0x2007e7e0`, `r12=0x00000801`, flags=0x41 (ARM_REGS +
  HARDFAULT_EXTENDED_REGS only; gb/rom/ppu null → fault fires BEFORE the game
  loop populates CRASH_CONTEXT). DMA idle.
- DECISIVE NEGATIVE / NEW FINDING: the recorded PC and LR are NOT code
  addresses. Resolved against `rust-nm -n`:
  `PC 0x2000401c == &SHARED_WORKER_STATE` (.bss static) and
  `LR 0x20002a48 == &AUDIO_QUEUE` (.bss static) — i.e. the *values* of the
  `shared` and `audio_queue_addr` immutable fields, not instructions. Both are
  past `.data` (ends 0x200028c4), so there is no code there. The MSTKERR bit
  means the DACCVIOL fired during **exception-entry stacking**, not from a
  store instruction.
- ROOT CAUSE of the false positive: in this build `Core1Transport` (and thus
  the `imm` block) is **stack-resident** — `gameboy: Option<PicoGameBoy>` is a
  stack local in the async main loop, and the block sits at `0x2007e7c0`, inside
  the core-0 stack range `0x20066184(_stack_end)..0x20080000`. When an exception
  is taken with PSP≈`0x2007e7e0` (=r4), the hardware pushes the 8-word frame
  down into `0x2007e7c0..0x2007e7df`, which overlaps the priv-RO region →
  MSTKERR DACCVIOL with MMFAR=`0x2007e7d0`. The "PC"/"LR" the handler unwinds
  are just the block's own field contents read back as a stacked frame. This
  fires before bug #5's wild store ever runs, masking it.
- IMPLICATION: a priv-RO MPU region is unsafe over a STACK-RESIDENT object,
  because legitimate exception stacking and normal stack traffic write through
  it. The earlier `transport-expected-fields-watch` build saw the fields at
  `0x2004b5xx` (below `_stack_end`, i.e. NOT on the stack) — the transport's
  residence MOVES between builds, which is the same layout-sensitivity that
  defeats single-word DWT watches. To use this MPU technique, the transport must
  first be pinned to a non-stack static location (e.g. a `StaticCell`/`static
  mut` arena, like `CORE1_WORKER`), so the immutable block does not share pages
  with the live stack. Then re-arm region 3 over that fixed address.
- Files changed: `platform/pico2w/Cargo.toml` (features
  `transport-immutable-mpu`, `transport-immutable-mpu-selftest`),
  `platform/pico2w/src/multicore.rs` (`TransportImmutable` struct, `imm` field +
  all `self.imm.*` rewrites, `arm_transport_immutable_mpu`, arm+selftest hook in
  `check_shared`, `TRANSPORT_IMMUTABLE_MPU_ARMED`). Selftest CRC `0x4802388b`,
  capture CRC `0xe47f918b`. Default/feature builds and `git diff --check` clean.

### 2026-06-15: Boxed PicoGameBoy → off-stack MPU trap → bug #5 is Fork (B) (fixed stack-address writer)

- FIX for the prior false positive: boxed `PicoGameBoy` so the `GameBoy` +
  `Core1Transport` + the immutable `imm` block live on the HEAP at a stable
  address instead of high in the core-0 stack. Diff:
  - `main.rs:514` — `let mut gb: Box<PicoGameBoy> =
    Box::new(PicoGameBoy::with_cartridge(...))` (boxed at construction so the
    boot-time `load_state` → `check_shared` → MPU arm runs on the heap instance,
    not the stack temporary).
  - `main.rs:657` — `Option<PicoGameBoy>` → `Option<Box<PicoGameBoy>>`.
  - `main.rs` call sites — `gameboy.as_mut()` → `gameboy.as_deref_mut()` (Running
    + InGameMenu).
  - `state/loading.rs` — all `Option<PicoGameBoy>` signatures → `Option<Box<…>>`,
    `as_ref()` → `as_deref()`, `start_first_rom` boxes its `gb`.
  - `multicore.rs` `arm_transport_immutable_mpu` — added off-stack confirmation
    log (`_stack_end`, `on_stack=`bool); `check_shared` arm hook now gates on
    `base < _stack_end` and only latches `TRANSPORT_IMMUTABLE_MPU_ARMED` on a real
    (off-stack) arm. This is REQUIRED: `with_cartridge` calls `push_worker_state`
    (→ `sync_ppu_state` → `wait_for_ticket` → `check_shared`) on the stack
    temporary BEFORE the `Box::new` move; without the gate the MPU armed on that
    stack copy (`0x2007e7e0`) and reproduced the MSTKERR false positive.
- OFF-STACK CONFIRMED. Capture image CRC `0xea84d346`. Arm log:
  `transport-immutable-mpu armed: r3=priv-RO [0x20055b20..=0x20055b3f]
  _stack_end=0x20065f94 on_stack=false` — the imm block is now in `.bss`
  HEAP_MEM (HEAP_MEM base `0x2003c8be`, `_stack_end` `0x2003_…`/`0x20065_…`), far
  below the stack, so exception-entry stacking can no longer write through the
  priv-RO region. Default `cargo build --release` + both feature builds +
  `git diff --check` all clean.
- SD GATE PASSED: boot log shows `save state loaded on boot` (no
  `TimeoutACommand`, no `init 288`). Poison Zelda save loads on every boot.
- POSITIVE CONTROL RE-VALIDATED (selftest CRC `0x8ef3e63b`, imm block
  `0x20055b60`, on_stack=false). Deliberate `write_volatile` to `0x20055b68`
  self-recorded standalone: `CFSR=0x00000082` (MMARVALID | DACCVIOL) — a clean
  store fault **WITHOUT MSTKERR**; `Fault@/MMFAR=0x20055b68` (the heap imm field);
  `PC=0x1001a506 = core::ptr::write_volatile` (the exact selftest write site). The
  MSTKERR stacking artifact is GONE now that the block is off-stack.
- CAPTURE — image CRC `0xea84d346`, no selftest. Eight independent standalone
  trials (mark-read + erase + `probe-rs reset` + detach + ~100 s soak + decode).
  Every trial crashed inside the window (rate ~100%, consistent with the ~72%+
  baseline → NOT Fork C / not suppressed). In NO trial did the heap imm-block MPU
  fire (no DACCVIOL at `0x20055bxx`, no MSTKERR). Crash signatures:
  - `0x0000fe9e` wild PC, `CFSR=0x00000100 IBUSERR`, LR in `Sm83` (trials 1,2,3,8)
  - `heapless` `spsc.rs:185` Panic (command-queue index/RC corruption) +
    `WatchdogTimeout` (trials 2,4,5,6,7) — the spsc command queue lives right
    after the transport in memory.
  - trial 5 also: HardFault `PC=0x200003ba`, `CFSR=0x00008200 BFARVALID|PRECISERR`,
    `Fault@=0x20082d38` (above `0x20080000`, wild bus access).
  - Post-run live read of the heap imm block (`0x20055b20`) returned coherent,
    intact field values (cmd/audio/shared queue addresses `0x20003af0`,
    `0x20002a40`, `0x20004010`) — the relocated object was NOT corrupted.
- VERDICT — **FORK (B): the writer targets a FIXED address, it does NOT follow the
  object.** Moving the transport to the heap relocated it out of harm's way but
  did not change the crash (same ~72%+ rate, same `0x0000fe9e`/`spsc`/high-RAM
  signatures). This rejects "a wild pointer that chases the transport" and
  confirms the long-standing fixed-stack-address hypothesis from
  CRASH_DEBUG_NOTES.md (2026-06-03 boxing note): the corruption hits a fixed
  high-stack/RAM region that the transport (and the spsc command queue beside it)
  sometimes occupies. A priv-RO MPU over the object can therefore never catch this
  writer. Next tool: a VALUE-MATCHED DWT watch (filter on the written value, e.g.
  the GB-data `0x0000FE9E`/`0x0000fe9e` pattern) on the fixed victim region, or a
  canary-bracketed fixed-address MPU region rather than an object-following one.
- Files changed: `platform/pico2w/src/main.rs`, `platform/pico2w/src/multicore.rs`
  (`arm_transport_immutable_mpu` off-stack log + `check_shared` off-stack arm
  gate), `platform/pico2w/src/state/loading.rs`. H4 frame-protocol fix (per-slot
  `dirty_rows`, `held_frame_slot` reserve) and the `transport-immutable-mpu`
  feature both intact. Capture CRC `0xea84d346`, selftest CRC `0x8ef3e63b`.

### 2026-06-15: Value-match dead-end, audit, and the OAM-DMA stack-protector BREAKTHROUGH

#### Value-match DWT (catch the writer by payload value) — DEAD END on this silicon

- To beat the moving-victim problem we tried a DWT data-VALUE-match (DATAVMATCH)
  on the wild payload `0x0000FE9E`. The arming path works
  (`supports_value_match()` true via DEVARCH = ARMv8-M DWT v2.0), but the
  comparator never fires the positive control across THREE encodings: v7-M-style
  `MATCH=5|DATAVMATCH(bit8)` in comp0; v8-M `MATCH=9` in comp0; `MATCH=9` in comp1
  (the "data-matching-only-in-comparator-1" rule). OpenOCD documents ARMv8-M
  data-value watchpoints as "not yet supported," and OpenOCD's value-filtered `wp`
  returns "resource not available" on RP2350. Conclusion: **DWT data-value
  matching is unusable on this Cortex-M33** — abandon it. (Feature
  `value-match-fe9e-watch` left in tree, inert/feature-gated.)

#### Dangling-stack-pointer audit — ruled out cross-core; it's a core-0 self-write

- Audited every cross-core pointer path: `run_core1_worker` receives only
  `'static` refs (queues, `SHARED_WORKER_STATE`, `CORE1_WORKER`);
  `SharedWorkerState` has NO raw-pointer fields. So core 1 cannot write into
  core-0's stack, and the spawn closure leaks no stack pointer. The per-frame
  `running.rs::tick` path is lifetime-clean (`&dirty_rows` outlives its future;
  DMA buffers are TX-only/persistent). ⇒ The writer is a **core-0 self-write to
  its own stack**, i.e. a stack overrun / wild contiguous store, not a cross-core
  dangling pointer.

#### BREAKTHROUGH: `-Z stack-protector=strong` gives a DETERMINISTIC repro

- The default build does NOT reproduce deterministically (mostly clean). A
  `-Z stack-protector=strong` build reproduces **5/5** at one instruction. Golden
  binary: `/tmp/rb-sp-strong/.../rustyboy-pico2w`, crc `0x20f47b81`. Signature:
  HardFault core 0 at `copy_dma_step` (memory.rs:628), `CFSR=0x00008200`
  (PRECISERR|BFARVALID), `BFAR=0x00008000`. Disasm: faulting `strb r0,[r8],#1`
  stores an OAM-DMA source byte through `r8` = wild `0x00008000` (the GB VRAM
  base, GB-data-shaped); `r12=0x2003fa30` is a SANE GameBoyMemory base. So a
  SPILLED copy of `self.oam` base (`[sp,#0xf0] = memory_base + 0x4080`) was
  smashed to ~0x7FB0 while another copy stayed clean.

#### OAM-DMA bisection — `copy_dma_step` is the VICTIM, not the writer

- Running log: `platform/pico2w/OAM_DMA_BISECTION.md`. Key results:
  - In-`copy_dma_step` source checkpoints: 6/6 SUPPRESSED the bug (one added call
    frame relocates the victim). An incidental +0x1c code shift also killed the
    repro; re-flashing the golden binary restored it. ⇒ the crash is bound to the
    exact binary LAYOUT — the fingerprint of a fixed-address wild store.
  - The OAM-DMA checkpoint guard caught an IMPOSSIBLE DmaState: `source=0x454a`
    (non-page-aligned; legit code always sets `source=page<<8`), `progress=203`
    (>160), and a `count=0` case that STILL zeroed the word-before-OAM (so
    `copy_dma_step`, which writes nothing at count=0, is NOT the writer). A
    `compiler_builtins::mem::impls::copy` (memcpy) fault variant + a value stride
    point to a **wild contiguous write/memcpy active during the OAM-DMA window**,
    smashing the spilled stack pointers AND stable heap words inside the boxed
    `GameBoyMemory` (the `DmaState` fields, `&oam-4`, the wram tail). It survives
    host ASan/TSan, so it flows through platform-only code (the OAM-DMA cart-RAM
    source path goes through `XipCartridge::read_ram`, never exercised by the host
    replay).

#### Rejected next-steps + the correct one

- A DWT value-0 filter (relies on the dead DATAVMATCH) and a defensive
  `advance_dma_bulk` abort (a symptom-hiding band-aid) were both rejected. The
  correct catch: an **EXTERNAL OpenOCD hardware write-watchpoint on a STABLE HEAP
  victim** (`DmaState.source` inside the boxed GameBoyMemory) on the golden binary
  — layout-immune, firmware-free (won't suppress the heisenbug), aimed at a fixed
  address, with legit page-aligned writes value-filtered manually in the TCL loop.
  This is in progress; the wild write's halt PC is the writer.

#### State

- Golden binary preserved at `/tmp/rb-sp-strong` (keep). H4 frame-protocol race
  fix and the `PicoGameBoy` boxing remain. Value-match (`value-match-fe9e-watch`)
  and `oam-prefix-watch` features are inert/feature-gated. config.toml reverted
  (stable `cargo build`/`run` work). Core tests 649/649 pass.
