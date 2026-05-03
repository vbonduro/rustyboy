# Opcode Dispatch Refactor — Flat Function Pointer Table

## Problem

The current dispatch path per instruction:

1. `opcodes.get(opcode)` → `Arc::clone` (atomic fetch-add on Cortex-M33)
2. `op.execute(self)` → vtable call → `OpCode::execute()` → `cpu.method()` → second vtable call
3. `op` drops → atomic fetch-sub + conditional free check

Two atomic RMWs and two vtable-indirect calls per instruction, every M-cycle.
Profiling shows decode/dispatch at **450 M Pico cycles / 60 frames (48.3% of total)**.

## Solution — Option B: `(fn_ptr, u32)` Table

Replace `Vec<Option<Arc<dyn OpCode>>>` with a `[OpcodeEntry; 256]` flat array.

```
OpcodeEntry { handler: fn(&mut Sm83, u32) -> Result<u8, InstructionError>, data: u32 }
```

`OpcodeEntry` is `Copy` (two words). Copying it out of the array drops the borrow on
`self.opcodes` immediately — no `Arc`, no `unsafe`, no raw pointers needed.

Tick dispatch shrinks to:

```rust
let entry = self.opcodes.main[opcode as usize]; // 8-byte copy, borrow released
(entry.handler)(self, entry.data)?;             // one indirect call, no vtable
```

~30–45 handler functions replace the two-vtable chain. Each handler is a method on
`Sm83` that takes `&mut self` and a packed `u32` argument word.

## Data Word Layout

```
bits [7:0]   = arg0  (operand, condition, CB op, etc.)
bits [15:8]  = arg1  (dst operand for LD, bit index for CB, 0 otherwise)
bits [23:16] = cycles_taken
bits [31:24] = cycles_not_taken (conditional insns only; 0 for unconditional)
```

Helper constructors (all `const fn`):

```rust
const fn pack0(cy: u8) -> u32 { (cy as u32) << 16 }
const fn pack1(a0: u8, cy: u8) -> u32 { a0 as u32 | ((cy as u32) << 16) }
const fn pack2(a0: u8, a1: u8, cy: u8) -> u32 { a0 as u32 | ((a1 as u32) << 8) | ((cy as u32) << 16) }
const fn pack_cc(cond: u8, cy_t: u8, cy_f: u8) -> u32 { cond as u32 | ((cy_t as u32) << 16) | ((cy_f as u32) << 24) }
```

## Flat Operand Encoding (`arg` module)

All operands are encoded as `u8` constants in `core/src/cpu/instructions/arg.rs`:

```rust
// 8-bit register targets (also used as CB targets, same order as SM83 CB encoding)
pub const B:      u8 = 0;
pub const C:      u8 = 1;
pub const D:      u8 = 2;
pub const E:      u8 = 3;
pub const H:      u8 = 4;
pub const L:      u8 = 5;
pub const MEM_HL: u8 = 6;  // (HL) memory — matches SM83 r8 encoding slot 6
pub const A:      u8 = 7;

// 8-bit immediate / signed
pub const IMM8:  u8 = 8;
pub const IMMS8: u8 = 9;

// 16-bit registers
pub const BC: u8 = 10;
pub const DE: u8 = 11;
pub const HL: u8 = 12;
pub const SP: u8 = 13;
pub const AF: u8 = 14;

// Conditions
pub const NZ: u8 = 0;
pub const Z:  u8 = 1;
pub const NC: u8 = 2;
pub const CC: u8 = 3;  // named CC to avoid clash with Flags::C

// CB shift/rotate ops (packed into bits [7:3] of data when target is in bits [2:0])
pub const RLC:  u8 = 0;
pub const RRC:  u8 = 1;
pub const RL:   u8 = 2;
pub const RR:   u8 = 3;
pub const SLA:  u8 = 4;
pub const SRA:  u8 = 5;
pub const SWAP: u8 = 6;
pub const SRL:  u8 = 7;

// Rotate-accumulator ops (RLCA/RRCA/RLA/RRA)
pub const RLCA: u8 = 0;
pub const RRCA: u8 = 1;
pub const RLA:  u8 = 2;
pub const RRA:  u8 = 3;
```

## Handler Families

Each handler is a method on `Sm83` and referenced as `Sm83::handle_*` in the table.
All carry `#[cfg_attr(target_arch = "arm", link_section = ".data")]`.

### ALU (add8 / adc / sub8 / sbc8 / cp8 / and8 / or8 / xor8)

`data = pack1(r8_arg, cycles)`

One handler per ALU op. `arg0` decoded by `resolve_r8(arg)` helper:

```rust
fn resolve_r8(&mut self, arg: u8) -> Result<u8, InstructionError> {
    Ok(match arg {
        arg::B      => self.registers.b,
        arg::C      => self.registers.c,
        arg::D      => self.registers.d,
        arg::E      => self.registers.e,
        arg::H      => self.registers.h,
        arg::L      => self.registers.l,
        arg::MEM_HL => self.bus_read(self.registers.hl())?,
        arg::A      => self.registers.a,
        arg::IMM8   => self.read_next_pc()?,
        _           => return Err(InstructionError::Failed("bad r8 arg".into())),
    })
}
```

### LD r8, r8 / LD r8, imm8 / LD r8, (HL) / LD (HL), r8

`data = pack2(dst_arg, src_arg, cycles)`

`handle_ld8` reads src via `resolve_r8(arg1)`, writes dst via `set_r8(arg0, val)`.

`set_r8` is the inverse of `resolve_r8` (no memory write for IMM8 destination).

### INC/DEC r8, INC/DEC r16, ADD HL r16, PUSH/POP

Straightforward; operand in `arg0`, cycles in bits [23:16].

`resolve_r16(arg)` and `set_r16(arg, val)` mirror the r8 helpers but for 16-bit pairs.

### Conditional jumps / calls / returns

`data = pack_cc(cond, cycles_taken, cycles_not_taken)`

Handler checks `self.check_condition_u8(arg0)`, ticks one extra internal cycle if taken,
returns `cycles_taken` or `cycles_not_taken` from bits [23:16] / [31:24].

### Misc (NOP/HALT/STOP/DAA/CPL/SCF/CCF/DI/EI)

Each is its own handler or shares `handle_misc` with op encoded in `arg0`. Since these
are zero/one arg and fairly unique, individual handlers are clearest.

### Ld16 variants (17 distinct ops)

Most get their own small handler. `handle_ld16_rr_imm16` shares one handler with r16
encoded in `arg0`.

### CB prefix

CB table uses four handler families:
- `handle_cb_shift` — RLC/RRC/RL/RR/SLA/SRA/SWAP/SRL
  `data = pack2(target, shift_op, cycles)`
- `handle_cb_bit` — BIT b, r
  `data = pack2(target, bit, cycles)`
- `handle_cb_res` — RES b, r
  `data = pack2(target, bit, cycles)`
- `handle_cb_set` — SET b, r
  `data = pack2(target, bit, cycles)`

`target` follows the SM83 CB encoding: 0=B,1=C,2=D,3=E,4=H,5=L,6=(HL),7=A.

`resolve_cb_target` / `write_cb_target_u8` helpers read/write register or (HL) memory.

## Files Changed

| File | Change |
|---|---|
| `core/src/cpu/instructions/arg.rs` | **New** — flat operand constants + pack helpers |
| `core/src/cpu/instructions/mod.rs` | Add `pub mod arg` |
| `core/src/cpu/instructions/opcodes.rs` | Add `OpcodeEntry`, `Handler`, `OpCodeTable::new()` with explicit 512-entry table |
| `core/src/cpu/sm83.rs` | Add `handle_*` methods + `resolve_r8/r16/cb_target` helpers; update `tick_impl`; remove decoder param from `new()` |
| `core/src/cpu/cpu.rs` | Remove `DecoderError` from `CpuError` if unused |
| `core/tests/common/mod.rs` | Remove decoder arg from `Sm83::new()` calls |
| `core/tests/*.rs` | Remove decoder arg |
| `platform/pico2w/src/main.rs` | Remove decoder arg |
| `platform/web/client/src/lib.rs` | Remove decoder arg |

## Invariants to Preserve

- `OpCodeDecoder` and the full `Decoder` / `OpCode` / `Instructions` trait chain remain
  intact — they are still used by `FakeCpu` unit tests for each instruction family.
- All existing integration tests (blargg, mooneye, dmg_acid2, etc.) must continue to pass.
- `#[cfg(feature = "perf")]` instrumentation in `tick_impl` is unchanged.
- `#[cfg(feature = "trace")]` hook is unchanged.
- `.data` section placement attributes on all hot-path methods are preserved.

## Expected Impact

Eliminates per-instruction:
- 2× atomic RMW (Arc clone/drop)
- 2× vtable-indirect function call
- 1× heap reference dereference

Projected savings: ~200–300 M Pico cycles / 60 frames (~22–32% of total runtime).
