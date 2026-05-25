# Crash Reporting

rustyboy-pico2w captures firmware crashes without writing to flash from the fault
handler — keeping XIP contention and interrupt-safe constraints intact.  On the
next boot the crash is committed to an on-chip flash sector and can be decoded
offline using `tools/crash_decoder.py`.

## Architecture

Crash capture is a two-phase pipeline:

```
Phase 1 — fault handler (ISR context, no flash I/O)
─────────────────────────────────────────────────────
HardFault / panic_handler
  │  write CRASH_MAGIC sentinel to WATCHDOG.scratch0
  │  write ARM exception frame   to WATCHDOG.scratch1-7
  │  write GB emulator state     to POWMAN.scratch0-7
  └─ sys_reset()   ← clean reboot, distinct from watchdog reset

Phase 2 — boot (single-core, flash safe)
─────────────────────────────────────────────────────
main() → crash::storage::check_and_commit()
  │  read WATCHDOG.scratch0 — is CRASH_MAGIC present?
  │  no  → return false (normal boot, nothing to do)
  │  yes → clear magic immediately (prevents double-commit on flash write failure)
  │         reconstruct CrashRecord from scratch registers
  │         write CrashRecord to next free slot in crash log sector
  └─ return true
```

**Why MMIO scratch registers?**  
Writing to WATCHDOG and POWMAN registers is pure MMIO — no XIP, no cache, no
DMA, no interrupts needed.  The fault handler can safely write 16 × u32 values
(512 bits) in microseconds without risking a second fault.

**Why not watchdog reset?**  
`sys_reset()` (SCB AIRCR SYSRESETREQ) keeps the crash magic intact across the
reset.  A watchdog reset (WATCHDOG.CTRL bit) would also clear the scratch
registers.  ROM-switch reboots use the watchdog path deliberately so they are
never mistaken for crashes.

## Flash sector layout

The last 4 KiB of internal flash (`FLASH_CAPACITY_BYTES - ERASE_SIZE`,
mapped at XIP address `0x103FF000`) is reserved as the crash log sector.

The sector is structured as **32 slots of 128 bytes** each:

```
Offset 0x000  Slot 0   SectorHeader  (128 bytes)
Offset 0x080  Slot 1   CrashRecord   (128 bytes)
Offset 0x100  Slot 2   CrashRecord   (128 bytes)
  …
Offset 0xF80  Slot 31  CrashRecord   (128 bytes)
```

Up to **31 crash records** are stored per erase cycle.  When all 31 slots are
full the sector is erased and recording restarts; the `erase_count` in the
header increments each cycle.

Slot discovery uses **RCRP-magic scanning** — the decoder reads the first 4
bytes of each slot and looks for the `RCRP` sentinel rather than trusting
`SectorHeader::next_slot` (a NOR-flash AND-corruption hazard when that field
needs to flip bits from 0 → 1 without an erase).

## SectorHeader format (Slot 0, 128 bytes)

| Offset | Size | Field         | Notes                                          |
|--------|------|---------------|------------------------------------------------|
| 0      | 4    | `magic`       | `b"RCLG"` — crash log magic                   |
| 4      | 4    | `erase_count` | u32 LE — increments on each sector erase cycle |
| 8      | 1    | `next_slot`   | Legacy field; always written as `0`, never updated |
| 9      | 119  | `_reserved`   | Zero-filled                                    |

A missing or corrupted sector header (bad magic) is treated as an empty sector.

## CrashRecord format (Slots 1–31, 128 bytes each)

All multi-byte integers are **little-endian**.

| Offset  | Size | Field           | Notes                                                                    |
|---------|------|-----------------|--------------------------------------------------------------------------|
| 0       | 4    | `magic`         | `b"RCRP"` — record present sentinel                                      |
| 4       | 1    | `schema_ver`    | Format version — currently `1`                                           |
| 5       | 1    | `crash_kind`    | `0` = HardFault, `1` = Panic, `0xFF` = Unknown                          |
| 6       | 1    | `flags`         | Bitmask — see [Flags](#flags)                                            |
| 7       | 1    | `slot_seq`      | Slot index within this erase cycle (0–30)                                |
| 8       | 3    | `fw_version`    | `[major, minor, patch]` from `Cargo.toml`                               |
| 11      | 1    | `_pad0`         |                                                                          |
| 12      | 4    | `git_hash`      | First 4 bytes of the build's git SHA as u32 LE                          |
| 16      | 4    | `arm_pc`        | ARM program counter at the point of fault                                |
| 20      | 4    | `arm_lr`        | ARM link register at the point of fault (call site)                     |
| 24      | 4    | `arm_cfsr`      | Configurable Fault Status Register — see [CFSR](#cfsr-quick-reference)  |
| 28      | 4    | `arm_hfsr`      | HardFault Status Register                                                |
| 32      | 4    | `arm_fault_addr`| MMFAR (MemManage) or BFAR (BusFault) if valid; `0` otherwise           |
| 36      | 4    | `_pad1`         |                                                                          |
| 40      | 4    | `rom_id_prefix` | First 4 bytes of the ROM's SHA-256 hash                                 |
| 44      | 2    | `rom_bank`      | MBC ROM bank mapped at the time of the crash                            |
| 46      | 1    | `ram_bank`      | MBC RAM bank                                                             |
| 47      | 1    | `_pad2`         |                                                                          |
| 48      | 1    | `gb_a`          | GB accumulator                                                           |
| 49      | 1    | `gb_f`          | GB flags (`Z N H C 0 0 0 0`)                                            |
| 50      | 1    | `gb_b`          |                                                                          |
| 51      | 1    | `gb_c`          |                                                                          |
| 52      | 1    | `gb_d`          |                                                                          |
| 53      | 1    | `gb_e`          |                                                                          |
| 54      | 1    | `gb_h`          |                                                                          |
| 55      | 1    | `gb_l`          |                                                                          |
| 56      | 2    | `gb_sp`         | GB stack pointer                                                         |
| 58      | 2    | `gb_pc`         | GB program counter (address in ROM/RAM)                                  |
| 60      | 4    | `gb_cycle_lo`   | Lower 32 bits of the emulator cycle counter                             |
| 64      | 1    | `ppu_ly`        | Current PPU scanline (0–153)                                            |
| 65      | 1    | `ppu_lcdc`      | LCDC register (`0` if not captured in this build)                       |
| 66      | 1    | `ppu_stat`      | LCDS STAT register                                                       |
| 67      | 1    | `_pad3`         |                                                                          |
| 68      | 12   | `panic_loc`     | Null-terminated last path segment of the panic source file (≤ 11 chars) |
| 80      | 2    | `panic_line`    | Source line number of the panic                                          |
| 82      | 38   | `_reserved`     | Zero-filled; available for future fields                                 |
| 120     | 4    | `crc32`         | CRC32/IEEE-802.3 of bytes `[0..120]`                                    |
| 124     | 4    | `_pad4`         |                                                                          |

### Flags

| Bit | Constant          | Meaning                                          |
|-----|-------------------|--------------------------------------------------|
| 0   | `HAS_ARM_REGS`    | `arm_pc / lr / cfsr / hfsr / fault_addr` valid  |
| 1   | `HAS_GB_STATE`    | `gb_*` registers and `ppu_*` valid              |
| 2   | `HAS_ROM_INFO`    | `rom_id_prefix / rom_bank / ram_bank` valid     |
| 3   | `HAS_PANIC_LOC`   | `panic_loc / panic_line` valid                  |

GB state (`HAS_GB_STATE`) is only present if the emulator completed at least
one frame before the crash.  A crash during boot (e.g. before `RunningState`
starts) will have `HAS_GB_STATE` clear and the `gb_*` fields zeroed.

### CFSR quick reference

The Configurable Fault Status Register is split into three sub-registers.
Common values you'll encounter:

| CFSR value   | Sub-register | Bit name    | Meaning                           |
|--------------|--------------|-------------|-----------------------------------|
| `0x0000_0001`| MMFSR        | `IACCVIOL`  | Instruction fetch from no-access region |
| `0x0000_0002`| MMFSR        | `DACCVIOL`  | Data load/store from no-access region   |
| `0x0000_0100`| BFSR         | `IBUSERR`   | Instruction prefetch bus error         |
| `0x0000_0400`| BFSR         | `IMPRECISERR`| Imprecise data bus error (async)       |
| `0x0100_0000`| UFSR         | `UNALIGNED` | Unaligned memory access                |
| `0x0200_0000`| UFSR         | `DIVBYZERO` | Divide by zero                        |

When `HFSR = 0x4000_0000` (`FORCED`), the HardFault was escalated from a
configurable fault — look at CFSR for the root cause.

## Emulator state update

The emulator calls `PicoGameBoy::update_crash_context()` once per frame from
`RunningState`.  This populates the `CRASH_CONTEXT` global (a set of `AtomicU32`
values) with the latest GB CPU registers, ROM/RAM bank, and PPU scanline.  The
fault handler reads this global (Acquire/Relaxed ordering) and packs the values
into POWMAN scratch registers for the boot-time flash commit.

The `valid` atomic is written last with Release ordering on the update path and
read first with Acquire on the fault-handler path, ensuring the fault handler
never reads a torn snapshot.

## Collecting a crash report

### Via probe-rs (debug probe attached)

```sh
# One-shot: read crash sector + decode in one command
uv run --script tools/crash_decoder.py --probe \
    --elf target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w

# Or read the sector manually then decode:
probe-rs read --chip RP235x -o crash.bin -f binary b8 0x103FF000 4096
uv run --script tools/crash_decoder.py --raw crash.bin \
    --elf target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w
```

> `probe-rs read` accepts XIP-mapped flash addresses (0x10000000+) via the
> Cortex-M33 DAP AHB-AP on RP235x, despite the "NOTE: Only supports RAM
> addresses" warning in its output.

### Via picotool (no debug probe)

```sh
# Put device into BOOTSEL mode first, then:
picotool save -o crash.bin --range 0x103FF000 +0x1000
uv run --script tools/crash_decoder.py --raw crash.bin
```

## crash_decoder.py usage

`tools/crash_decoder.py` is a self-contained [PEP 723](https://peps.python.org/pep-0723/)
script managed by `uv`.  No virtualenv setup required.

```
usage: crash_decoder.py [--raw FILE | --probe] [--elf ELF] [--json] [--chip CHIP]

Input:
  --raw FILE    Read crash sector from a binary file (4096 bytes)
  --probe       Read crash sector directly from a connected device via probe-rs

Symbolisation:
  --elf ELF     Path to firmware ELF — enables addr2line symbolisation of
                arm_pc and arm_lr into source file + line number

Output:
  --json        Emit machine-readable JSON instead of the rich terminal report
  --chip CHIP   Override probe-rs chip name (default: RP235x)
```

### Example output

```
╭──────────────────────────────────────────────────────────╮
│  rustyboy crash record — slot 0 (erase cycle 1)          │
╰──────────────────────────────────────────────────────────╯
  kind       HardFault
  firmware   0.1.0  git=6d2129af
  flags      HAS_ARM_REGS | HAS_GB_STATE | HAS_ROM_INFO

ARM exception
  PC         0x100234a8  → core::ptr::write_volatile  (main.rs:265)
  LR         0x10022bc4  → running::tick               (running.rs:62)
  CFSR       0x01000000  UFSR.UNALIGNED — unaligned memory access
  HFSR       0x40000000  FORCED (UsageFault escalated)
  fault addr 0xdeadbeef

Game Boy state
  ROM        ab:cd:ef:01  bank=6  RAM bank=0
  CPU        A=00 F=b0 B=00 C=13 D=00 E=d8 H=01 L=4d
             SP=fff8  PC=1dc9
  PPU        LY=105  LCDC=91  STAT=83
  cycles     12345678
```

## Erasing the crash log

After collecting records, erase the sector so the next crash starts at slot 0:

```rust
// In firmware (after offloading records):
crash::storage::erase_log(&mut onboard_flash)?;
```

Or from the host:

```sh
# Erase via probe-rs (erases the containing 4 KiB sector):
probe-rs erase --chip RP235x --address 0x103FF000
```
