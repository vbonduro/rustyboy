# Rust/LLVM Codegen Investigation

Date: 2026-06-19

## Summary

The observed corruption was initially attributed to a Rust/LLVM register
allocation bug in which an inlined PPU local allegedly reused the same stack
slot as `command_rx.rb.data_ptr` in `run_core1_worker`.

The upstream search found several related LLVM register-allocation and
stack-coloring reports, but no exact issue known to be fixed by a newer Rust or
LLVM release. More importantly, direct inspection of the reproducing binary
does not currently support the claimed stack-slot overlap.

The `#[inline(never)]` changes remain a reasonable structural mitigation because
they greatly reduce `run_core1_worker`'s frame and isolate the rendering locals.
However, the available evidence is not sufficient to call this a confirmed
rustc/LLVM miscompilation.

## Toolchains

The default toolchain in the working environment is:

```text
rustc 1.94.1 (e408947bf 2026-03-25)
LLVM 21.1.8
```

However, the deterministic reproducing ELF embeds:

```text
rustc 1.97.0-nightly (d7f14d3d8 2026-05-15)
LLVM 22.1.4
LLD 22.1.4
```

This means the bug, if it is an LLVM codegen bug, is already present in a
newer LLVM generation than the project's default stable compiler.

For comparison, the source was also built with the newest available nightly on
the investigation date:

```text
rustc 1.98.0-nightly (bc2112ed5 2026-06-18)
LLVM 22.1.7
LLD 22.1.7
```

The comparison build used the same target and important compilation settings:

```text
target: thumbv8m.main-none-eabihf
release opt-level: z
LTO: enabled
-Z stack-protector=strong
```

The two `#[inline(never)]` attributes on `render_scanline` and
`render_sprite_scanline` were removed only in a disposable copy of the
repository. The working tree was not changed.

## Newer Compiler Result

LLVM 22.1.7 still emits essentially the same monolithic function:

```text
May 15 / LLVM 22.1.4:
    run_core1_worker size = 0xBBC
    sub sp, #0xF0

June 18 / LLVM 22.1.7:
    run_core1_worker size = 0xBB8
    sub sp, #0xF0
```

The relevant stack offsets are also unchanged:

```asm
strd r1, r0, [sp, #0x90]
ldr  r2, [sp, #0x94]

add  r2, sp, #0x98
...
add  r6, sp, #0x98
add  r5, r6, r12, lsl #3
str  r3, [r5, #4]
```

Therefore, upgrading from LLVM 22.1.4 to LLVM 22.1.7 does not eliminate the
large inlined frame or materially change this code sequence.

## Stack-Address Contradiction

The current root-cause note says:

- `command_rx.rb.data_ptr` is stored at `sp+0x94`.
- The sprite-collection store also writes `sp+0x94`.
- The watched absolute address was `0x20081F74`.
- The live MSP at the watchpoint was `0x20081ED0`.

The generated instructions do not agree with that interpretation.

Given the live MSP:

```text
0x20081ED0 + 0x94 = 0x20081F64  command_rx.rb.data_ptr
0x20081ED0 + 0xA4 = 0x20081F74  sprite entry field
```

The sprite array begins at `sp+0x98`, and each entry is eight bytes:

```text
entry address = sp + 0x98 + index * 8
field store   = entry address + 4
```

Consequently, the watchpoint at `0x20081F74` is naturally reached by the sprite
array when `index == 1`. It is not the `command_rx` pointer slot relative to the
reported live MSP.

This also explains the watchpoint sequence:

- The first 15 hits came from the sprite-array zeroing loop.
- A later hit stored the sprite value `0x11`.

Those are expected writes if the watched word belongs to `sprites`. If it were
the queue pointer, the zeroing loop would corrupt the queue pointer immediately,
not approximately 9.5 minutes later.

No instruction changes SP after the function's initial:

```asm
push {r7, lr}
mov  r7, sp
sub  sp, #0xf0
```

The 16-byte difference between the crash-derived SP and the live watchpoint MSP
therefore cannot currently be treated as cosmetic. It determines whether the
two addresses overlap.

## Exception-Frame Assumption

The HardFault handler computes the interrupted SP as:

```rust
let sp_before = ef as *const ExceptionFrame as usize + 32;
```

That assumes the exception frame is exactly the eight-word basic frame with no
alignment padding or extended floating-point state affecting the relationship.

The crash-derived value was:

```text
sp_before = 0x20081EE0
```

while the live MSP at the watchpoint was:

```text
MSP = 0x20081ED0
```

This exact 16-byte discrepancy must be explained before using the crash-derived
SP to name an absolute spill-slot address.

## LLVM Stack-Slot-Sharing Experiment

LLVM exposes these diagnostic options:

```text
-disable-ssc
-no-stack-coloring
-no-stack-slot-sharing
```

A May 15 compiler build with all three options increased
`run_core1_worker`'s frame:

```text
normal:              sub sp, #0xF0
slot sharing off:    sub sp, #0x118
```

The queue pointer and sprite array were moved farther apart:

```text
queue data pointer: sp+0xAC
sprite array base:  sp+0xC0
```

This proves the options affect stack layout, but it does not prove a compiler
bug. The normal build already places the queue pointer and sprite array in
non-overlapping ranges.

These options may still be useful as a hardware A/B diagnostic. A successful
run would show that layout affects the failure, but layout sensitivity alone is
not enough to establish incorrect stack-slot merging.

## Related Upstream Reports

No exact rustc issue matching this ARM/Thumb failure was found.

Potentially related LLVM reports:

- [LLVM #193212](https://github.com/llvm/llvm-project/issues/193212) — Greedy
  register allocation allegedly misses a reload on one CFG path in large,
  heavily inlined Rust functions. This is the closest behavioral match:
  layout-sensitive corruption, high register pressure, and an `inline(never)`
  workaround. It was reported against an out-of-tree SH4 backend and closed as
  `not planned` without maintainer confirmation or a fix.
- [LLVM #191800](https://github.com/llvm/llvm-project/issues/191800) — an open
  LLVM 20–22 x86 miscompile where a spilled value is clobbered and never
  reloaded. The symptom is similar, but the reproducer and backend are
  x86-specific.
- [LLVM #132085](https://github.com/llvm/llvm-project/issues/132085) — open
  incorrect stack-slot merging caused by stack-coloring lifetime handling.
- [LLVM #57725](https://github.com/llvm/llvm-project/issues/57725) — another
  longstanding open incorrect stack-coloring merge involving alloca addresses
  and lifetime markers.
- [LLVM #38502](https://github.com/llvm/llvm-project/issues/38502) — open
  register allocator issue involving a load from a dead spill slot.
- [LLVM #135639](https://github.com/llvm/llvm-project/issues/135639) — open
  Greedy register allocator report about unnecessary repeated spill reloads.

Two relevant register-allocation consistency fixes landed on upstream LLVM
`main` after the reproducing May 15 toolchain:

- [LLVM PR #197773](https://github.com/llvm/llvm-project/pull/197773) — fixes
  stale `LiveRegMatrix` state when spill hoisting shrinks or splits intervals.
- [LLVM PR #197776](https://github.com/llvm/llvm-project/pull/197776) — fixes
  stale `LiveRegMatrix` state when folding creates and extends a copy interval.

They are **not known to be present in Rust's LLVM 22.1.7 fork**. Comparing
Rust's pinned LLVM 22.1.6 and 22.1.7 revisions shows no changes to
`LiveRegMatrix`, `InlineSpiller`, `LiveRangeEdit`, or `RegAllocGreedy`.
Consequently, the local LLVM 22.1.7 comparison build does not test these two
upstream fixes.

The associated verifier work remains open:

- [LLVM PR #197778](https://github.com/llvm/llvm-project/pull/197778)

## Current Conclusion

There is no evidence that a newer released/nightly Rust compiler is known to
fix this failure. LLVM 22.1.7 still generates the same relevant code shape.

There are credible upstream examples of register-allocation and stack-coloring
miscompilations, including failures that are sensitive to inlining and large
functions. They make a compiler bug plausible in general, but none is an exact
match.

The Core 1 `command_rx.rb.data_ptr` collision claim was subsequently disproved
by the live-SP-anchored watchpoint experiment in `OAM_DMA_BISECTION.md` §G4-H.
The previously watched word was a legitimate sprite-array word rather than
`command_rx.rb.data_ptr`.

## Recommended Next Test

Use the reproducing unmitigated binary and:

1. Break immediately after `sub sp, #0xF0` in `run_core1_worker`.
2. Read the live SP on Core 1.
3. Compute the queue pointer word directly as `live_sp + 0x94`.
4. Confirm that the prologue writes `0x20004490` to that exact address.
5. Watch that exact address for writes.
6. Record the writer PC, live SP, written value, and both MSP and PSP.

Using the previously reported live MSP, the expected address is:

```text
0x20081ED0 + 0x94 = 0x20081F64
```

not:

```text
0x20081F74
```

If a rendering instruction writes `0x20081F64`, that would be strong evidence of
an actual codegen or address-generation failure. If only `0x20081F74` receives
the sprite writes, those writes are expected and the corruption source remains
elsewhere.

It would also be useful to capture:

```text
MSP
PSP
CONTROL
xPSR from the exception frame
EXC_RETURN
```

That should resolve whether stack selection, alignment padding, or exception
frame interpretation accounts for the 16-byte discrepancy.

## Mitigation Status

Keeping `render_scanline` and `render_sprite_scanline` as
`#[inline(never)]` is still prudent:

- It reduces `run_core1_worker`'s frame from 240 bytes to 80 bytes.
- It moves rendering locals into separate call frames.
- It causes the explicitly `.data`-section functions to be emitted as
  independently callable SRAM-resident code.
- It reduces register pressure and the number of unrelated objects sharing one
  large frame.

The mitigation should be described as structural isolation while the root cause
is revalidated, rather than as a proven workaround for a confirmed LLVM
spill-slot collision.

---

## June 20 Multi-Agent Upstream Search

Four independent GPT-5.4 searches covered:

1. Greedy register allocation, spilling, rematerialization, and live intervals.
2. StackColoring and StackSlotColoring.
3. ARM/Thumb frame lowering and MachineOutliner bugs.
4. Rust's LLVM version history and newer toolchain contents.

All four searches reached the same broad conclusion:

- No official LLVM issue exactly matches the confirmed Core 0 failure.
- The strongest matches are target-independent regalloc/liveness bugs.
- Ordinary `StackColoring` is a weaker candidate.
- No Rust release through the June 20, 2026 nightly is known to contain a
  specific fix for this failure.

### Which Local Cases Are Actually Relevant

The local evidence needs to be separated into three cases:

#### Confirmed: Core 0 `sp+0xf0`

`OAM_DMA_BISECTION.md` §G1 records a deterministic DWT catch:

```text
0x1000a904  str r2, [sp, #0xf0]  writes &oam
0x1002f2d8  str r2, [sp, #0xf0]  later writes GameBoyMemory base
0x1002fe92  ldr r0, [sp, #0xf0]  later reloads the supposed &oam value
```

The DWT watchpoint caught the second store landing on the word that still needed
to contain `&oam`, in two independent runs.

This is the primary upstream-comparison signature:

> A spill value is still required by a later reload, but allocator/liveness
> decisions permit an unrelated value to overwrite the same stack word first.

#### Retracted: Core 1 `sp+0x94`

The earlier §G4-E claim was disproved by §G4-H. Relative to the live SP:

```text
sp+0x94 = command_rx data pointer
sp+0xa4 = sprite-array field
```

The watchpoint had been armed on `sp+0xa4`; the observed sprite writes were
legitimate. A correctly armed watchpoint on `sp+0x94` saw no foreign write
during the full failure window.

This must not be counted as a second confirmed LLVM stack-slot collision.

#### Suspected: Core 1 ticket-pointer slot

A later build faults after:

```text
0x1001a648  str r0, [sp, #0x34]  save ticket pointer
...
0x1001a8fe  ldr r0, [sp, #0x34]  reload ticket pointer
0x1001a900  stl r9, [r0]         fault with r0 approximately 0x41
```

This has the same broad live-slot-corruption shape, but its overwriting
instruction has not yet been caught. It is supporting evidence, not a second
confirmed allocator bug.

### Best Upstream Candidates

#### 1. LLVM PR #197776 — stale `LiveRegMatrix` after folded copy

- PR: [llvm/llvm-project#197776](https://github.com/llvm/llvm-project/pull/197776)
- Commit:
  [`642bbbaf57ed`](https://github.com/llvm/llvm-project/commit/642bbbaf57ed1d0ce6b7871a8d3dc1386e95926b)
- Merged to LLVM `main`: May 28, 2026

`InlineSpiller::foldMemoryOperand` could extend a copy destination's
`LiveInterval` while `LiveRegMatrix` retained the old, shorter interval.

This is the strongest concrete fixed bug found because the allocator's
interference model can be stale for a value whose real live interval has been
extended. That is adjacent to the §G1 failure: an allocator may believe a
resource is free while a value is still needed.

It is not proven to be the same bug, and its regression test is x86 APX.

#### 2. LLVM PR #197773 — stale matrix state during spill hoisting

- PR: [llvm/llvm-project#197773](https://github.com/llvm/llvm-project/pull/197773)
- Commit:
  [`6519c04eb459`](https://github.com/llvm/llvm-project/commit/6519c04eb459deab1c71756ddfc04fd7ee852904)
- Merged to LLVM `main`: May 26, 2026

`HoistSpillHelper` failed to keep `LiveRegMatrix` synchronized while dead-def
elimination shrank or split intervals. Some cloned or changed virtual-register
intervals were absent from the matrix or retained stale assignments.

This is another strong adjacent mechanism: incorrect spill live-range
bookkeeping can permit conflicting assignments while remaining internally
plausible to later codegen.

Again, it is not an exact ARM reproducer.

#### 3. LLVM issue #193212 — missed reload on one CFG path

- Issue:
  [llvm/llvm-project#193212](https://github.com/llvm/llvm-project/issues/193212)
- Reported against LLVM 22.1.2-rust-dev
- Closed `not planned` on May 22, 2026
- No fix

This report describes Greedy RA splitting and spilling a virtual register,
reloading it on some control-flow paths, but failing to reload it on another
path that still uses the value.

Its observed properties are unusually close to this firmware:

- Large, heavily inlined Rust functions.
- High register pressure.
- Layout-sensitive wrong code.
- Unrelated values appearing where a live scalar or pointer was expected.
- `inline(never)` changing or avoiding the manifestation.

It is the closest symptom match, but it came from an out-of-tree SH4 backend and
was not confirmed or fixed by LLVM maintainers.

#### 4. LLVM issue #38502 — load from a dead/uninitialized spill slot

- Issue:
  [llvm/llvm-project#38502](https://github.com/llvm/llvm-project/issues/38502)
- Open since October 2, 2018

`InlineSpiller` and rematerialization produce a reload from a spill slot that
never received the necessary spill. LLVM's verifier reports:

```text
Instruction loads from dead spill slot
```

This is a strong match for the suspected Core 1 ticket-pointer failure, where a
later reload supplies garbage, but a weaker match for §G1 because §G1 caught an
actual intervening writer.

#### 5. LLVM issue #191800 — spilled value clobbered and not reloaded

- Issue:
  [llvm/llvm-project#191800](https://github.com/llvm/llvm-project/issues/191800)
- Open as of June 20, 2026
- Reported on official LLVM 20.1 through 22.1.3

An x86 return value is spilled, its physical register is clobbered by a later
zeroing operation, and one exit path returns without reloading the spill.

This confirms that current LLVM releases have real silent wrong-code bugs in
the “live spilled value clobbered / reload omitted” family. The specific
reproducer is x86-only.

#### 6. LLVM PR #201094 — rematerialization across a stale live-range gap

- PR: [llvm/llvm-project#201094](https://github.com/llvm/llvm-project/pull/201094)
- Open draft as of June 20, 2026

The proposed change extends an operand's live interval when rematerialization
would otherwise use a stale gap. It checks both `LiveRegMatrix` interference and
physical-register clobbers before extending the interval.

This is not a landed fix, but it is further evidence that LLVM's current
rematerialization/liveness machinery has unresolved cases involving stale
values and gaps in live intervals.

### Why StackColoring Is Now a Weaker Candidate

There are genuine LLVM bugs in this area:

- [#132085](https://github.com/llvm/llvm-project/issues/132085) — overlapping
  allocas incorrectly merged because liveness begins at first use.
- [#57725](https://github.com/llvm/llvm-project/issues/57725) — address-taken
  allocas incorrectly merged.
- [#126252](https://github.com/llvm/llvm-project/issues/126252) — lifetime/DSE
  interaction leaves StackColoring treating an address-taken local as dead.
- [#104776](https://github.com/llvm/llvm-project/issues/104776) — incorrect
  lifetime handling with multiple underlying objects.

These establish that LLVM has merged genuinely overlapping stack objects.
However, they concern IR allocas and lifetime markers.

The firmware's strongest case concerns a spilled machine value and later reload.
In addition:

- `-no-stack-coloring` did not eliminate the hardware crash.
- `-disable-ssc` did not eliminate the hardware crash.
- The Core 1 named-local overlap that initially pointed at StackColoring was
  disproved.

The leading pass family is therefore:

```text
GreedyRegAlloc
InlineSpiller
LiveRangeEdit
LiveIntervals / LiveRegMatrix
rematerialization and spill-hoisting bookkeeping
```

rather than IR `StackColoring` or the final `StackSlotColoring` merge pass.

### ARM and MachineOutliner Search

The ARM-specific search found no M-profile spill/reload issue that cleanly
matches §G1.

It did find an existing ARM MachineOutliner/liveness family related to the
separate §G18 verifier finding:

- [LLVM PR #73492](https://github.com/llvm/llvm-project/pull/73492) — ARM/Thumb
  undefined physical-register verifier failure around tail jumps and LR.
- [LLVM PR #75527](https://github.com/llvm/llvm-project/pull/75527) — merged ARM
  fix for LR restore/liveness around outlined calls.
- [LLVM issue #119556](https://github.com/llvm/llvm-project/issues/119556) —
  open IPRA/MachineOutliner crash involving ARM, AArch64, and RISC-V.
- [LLVM issue #46111](https://github.com/llvm/llvm-project/issues/46111) —
  AArch64 outliner stack-fixup problems under `-Oz` and LTO.

These support filing the local undefined-`r1` MachineOutliner verifier error as
a separate LLVM issue. They do not explain the main corruption because the
outliner-disabled, verifier-clean firmware still crashes.

### Rust Toolchain Inclusion

The reproducing ELF uses:

```text
rustc 1.97.0-nightly (d7f14d3d8 2026-05-15)
Rust LLVM fork eaab4d9841b9...
LLVM 22.1.4
```

Rust moved to an LLVM 22.1.6-based fork in the May 22 nightly and to an LLVM
22.1.7-based fork in mid-June.

The two most interesting upstream fixes, #197773 and #197776, merged into
upstream LLVM `main` on May 26 and May 28. They were not release-branch fixes.

Comparing Rust's pinned 22.1.6 and 22.1.7 LLVM revisions:

```text
08c84e69a84d... -> ec9ab9d68bf7...
```

shows no changes in:

```text
LiveRegMatrix
InlineSpiller
LiveRangeEdit
RegAllocGreedy
```

Therefore:

- Rust's LLVM 22.1.7 nightly should not be assumed to contain #197773/#197776.
- The local LLVM 22.1.7 build did not test the leading upstream fix candidates.
- No current Rust release or nightly can yet be identified as containing a
  known fix for this issue.

The relevant Rust LLVM comparison is:

<https://github.com/rust-lang/llvm-project/compare/08c84e69a84d95936296dfcab0e38b34100725d5...ec9ab9d68bf7a0e86b2ddf3c0e6e3c4620e02961>

### Updated Ranking

For the confirmed Core 0 overwrite-before-reload:

1. LLVM PR #197776 — stale `LiveRegMatrix` after spill folding.
2. LLVM PR #197773 — stale matrix state during spill hoisting/splitting.
3. LLVM issue #193212 — Greedy RA misses reload on one CFG path.
4. LLVM issue #38502 — reload from a dead spill slot.
5. LLVM issue #191800 — current-release missed-reload wrong code.

For the suspected Core 1 ticket-pointer case:

1. LLVM issue #38502.
2. LLVM issue #193212.
3. LLVM issue #191800.
4. LLVM PRs #197773/#197776.

For the independent MachineOutliner verifier error:

1. LLVM PR #75527 and its related ARM history.
2. LLVM PR #73492.
3. LLVM issue #119556.
4. LLVM issue #46111.

### Best Next Compiler Experiment

The most valuable toolchain test is no longer “try ordinary LLVM 22.1.7.”

It is:

1. Build Rust's LLVM fork with upstream commits
   `6519c04eb459` and `642bbbaf57ed` cherry-picked.
2. Rebuild the exact golden firmware with the same source, target, LTO,
   `opt-level=z`, and `-Z stack-protector=strong`.
3. Compare the golden `embassy_main_task` spill/reload sequence and stack frame.
4. Run the normal hardware soak beyond the failure window.

A pass would make one or both `LiveRegMatrix` fixes a serious causal candidate.
A failure would substantially reduce their likelihood.

A second high-value experiment is to swap Greedy RA for Basic RA if the selected
LLVM build permits it. That changes the allocator rather than merely disabling a
late stack-slot merge pass.

### Upstream Filing Shortlist

A new LLVM report should cite:

- [#197776](https://github.com/llvm/llvm-project/pull/197776)
- [#197773](https://github.com/llvm/llvm-project/pull/197773)
- [#193212](https://github.com/llvm/llvm-project/issues/193212)
- [#38502](https://github.com/llvm/llvm-project/issues/38502)
- [#191800](https://github.com/llvm/llvm-project/issues/191800)

The report should lead with the deterministic §G1 facts:

```text
- target: thumbv8m.main-none-eabihf
- LLVM: Rust fork based on 22.1.4
- optimization: -Oz + LTO + stack-protector=strong
- giant inlined Rust function
- DWT derives the spill address from live SP
- first store writes &oam
- later unrelated store writes GameBoyMemory base to the same address
- subsequent reload consumes the overwritten value
- repeated identically in two hardware runs
```

It should avoid presenting the retracted Core 1 sprite-array watchpoint as
supporting evidence.
