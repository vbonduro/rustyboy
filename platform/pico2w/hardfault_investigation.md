# HardFault Investigation — rustyboy-pico2w

## Problem

The firmware crashes with a HardFault during `StreamingCartridge::new()`.
The crash happens reproducibly on every boot, after both ROM banks are read
from flash and before `GameBoyMemory` is constructed.

## Timeline of Events (from RTT logs)

```
0.000430  [INFO ] rustyboy-pico2w v0.1.0 starting @250MHz
0.265528  [INFO ] display: ILI9341 initialised
0.265537  [INFO ] starting splash
3.021441  [INFO ] staged ROM found in flash: 16 banks (256 KiB)
3.021460  [INFO ] building StreamingCartridge
3.021508  [INFO ] read_bank 0: offset=0x81000
3.022029  [INFO ] read_bank 0: done, cart_type=0x1        ← MBC1
3.022098  [INFO ] read_bank 1: offset=0x85000
3.022624  [INFO ] read_bank 1: done, cart_type=0x37       ← (buf[0x0147] of bank 1, not meaningful)
          [CRASH — HardFault @ 0x1000e5a2]
```

The crash occurs **after** bank 1 is read successfully and **before**
`"building GameBoyMemory"` is logged. This window contains only:

```rust
Ok(Self {
    reader,
    bank0_cache,
    banked_cache,
    fixed_bank_num:   0,
    current_bank_num: 1,
    rom_bank_count,
    mbc,
    ram: vec![0u8; ram_bytes],   // ← heap allocation of external RAM
})
```

## ROM Details

- Cart type byte (bank0[0x0147]) = 0x01 → MBC1 (no external RAM in this variant)
- ROM size: 16 banks = 256 KiB
- RAM size code (bank0[0x0149]): **unknown** — not yet logged

## Memory Layout (current binary)

| Section   | Address    | Size       |
|-----------|------------|------------|
| .text     | 0x1000013c | 58,504 B   |
| .data     | 0x20000000 | 10,148 B   |
| .bss      | 0x200027a8 | 351,484 B  |
| .uninit   | 0x200584a4 |  1,024 B   |
| **STACK** | 0x200588a4→0x20080000 | **161,632 B** |

Heap: 256 KB in .bss starting at `HEAP_MEM`. Frame buffer (103,680 B) is
pre-allocated from heap at startup, leaving ~155 KB free.

## Custom HardFault Handler — Result

A `#[cortex_m_rt::exception] HardFault(ef: &ExceptionFrame)` handler was added
that calls `defmt::error!("HardFault: PC=...")`. **The error log never appeared.**

This means the handler itself faulted (**double-fault → LOCKUP**). probe-rs
halts the CPU in LOCKUP and reports:

```
Frame 0: __Thumbv7ABSLongThunk__Sm83::dispatch_interrupt @ 0x1000e5a2
```

`0x1000e5a2` is near the **end of .text** where the linker places long-branch
thunks. The HardFault handler is also near there. The named symbol is the
nearest one probe-rs can resolve, not necessarily the function that faulted.

## Root Cause Candidates (in priority order)

### 1. Stack overflow corrupting the exception frame (most likely)

When HardFault fires the CPU pushes an 8-word exception frame onto the MSP.
If the MSP has already grown past the bottom of valid stack space (0x200588a4),
that push writes into `.uninit` or `.bss`, which causes a second fault →
LOCKUP, explaining why `defmt::error!()` never runs.

Stack usage during `StreamingCartridge::new()` without NRVO:
- Caller holds `cart: StreamingCartridge` as a local: **~32 KB**
- `new()` frame holds `bank0_cache` + `banked_cache`: **32 KB**
- Embassy executor + async poll overhead: **~2 KB**
- Total peak: **~66 KB** → well within 161 KB, so unlikely to overflow

However, if Sm83 (2× 23,040 B framebuffers = ~46 KB) is being partially
constructed on the stack at the same time (e.g. LLVM spills it from generator
state to stack), peak usage could be higher.

### 2. Heap OOM on `vec![0u8; ram_bytes]`

`ram_bytes` comes from `bank0[0x0149]`. Code 0x04 = 128 KB of external RAM.
Allocating 128 KB from a heap that has ~155 KB free would succeed, but
allocating it **plus** subsequent `Box::new(cart)` (32 KB) might exhaust the
heap. Heap OOM calls the global OOM handler which panics — that would produce
a panic log, not a silent HardFault, so this is less likely.

### 3. Unaligned or invalid access in struct construction

Less likely given the struct uses plain arrays, but worth ruling out.

## Evidence Ruling Things Out

- **Stack layout unchanged** between all test runs: .bss = 351,484 B in all
  builds. The earlier crash-location shift (from StreamingCartridge::new() to
  splash) when +44 bytes of .text was added was caused by a DIFFERENT bug
  that was fixed by adding the splash boundary log (the splash was always
  working; the prior session misidentified the log point).
- **Both XIP reads succeed**: bank 0 and bank 1 are read without error; the
  bug is in what happens next.
- **No OOM panic message seen**: if the heap were exhausted the panic handler
  (from `panic-probe`) would have printed a message before the HardFault.

## Current State of Code

Modified files (relative to pre-investigation baseline):

| File | Change |
|------|--------|
| `src/main.rs` | Custom `HardFault` handler + `info!("starting splash")` |
| `src/flash_rom.rs` | `info!()` before/after `blocking_read` in `read_bank` |

## Next Steps

1. **Use GDB to read MSP at crash** — connect probe-rs GDB server, break on
   HardFault, read `$msp` to determine how much stack was consumed. If MSP is
   near or below `0x200588a4` we have a stack overflow.

   ```sh
   # terminal 1
   probe-rs gdb --chip RP235x target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w
   # terminal 2
   arm-none-eabi-gdb target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w
   (gdb) target remote :1337
   (gdb) monitor reset halt
   (gdb) break HardFault_
   (gdb) continue
   (gdb) info registers   # look at sp, check if it's near 0x200588a4
   ```

2. **Log `ram_bytes` before the `vec!` allocation** — add
   `info!("ram_bytes={}", ram_bytes)` just before `vec![0u8; ram_bytes]` in
   `StreamingCartridge::new()` to confirm the allocation size is reasonable.

3. **Disassemble the binary** — map 0x1000e5a2 to the exact instruction:

   ```sh
   arm-none-eabi-objdump -d target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w \
     | awk '/1000e5[0-9a-f]/{print}' | head -40
   ```

4. **If stack overflow is confirmed** — options:
   - Increase stack by shrinking heap (`HEAP_SIZE` in main.rs), or
   - Heap-allocate the bank caches in `StreamingCartridge` (use `Box<[u8; 0x4000]>`)
     to move them off the stack.

5. **If heap OOM is confirmed** — reduce frame buffer or bank cache allocation
   strategy.
