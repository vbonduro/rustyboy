# CPU Instruction Architecture

## Overview

The SM83 CPU executes instructions via a **double-dispatch pipeline**. Each tick fetches one
opcode byte from memory, decodes it into a typed `OpCode` value, and dispatches execution back
through the `Instructions` trait to the concrete CPU implementation. This keeps decoding,
dispatch, and execution cleanly separated and fully unit-testable at each layer.

```
  Sm83::tick()
       │
       │ read byte from PC
       ▼
  OpCodeDecoder::decode(u8)
       │
       │ returns Box<dyn OpCode>
       ▼
  OpCode::execute(&mut dyn Instructions)
       │
       │ calls e.g. cpu.add8(&self)
       ▼
  Sm83::add8() / Sm83::sub8() / ...
       │
       │ reads operands, updates registers/memory, returns cycles
       ▼
  tick() returns cycle count
```

The 0xCB prefix is handled specially: `tick()` detects `0xCB`, reads the second byte, and
dispatches directly to `CbDecoder` rather than through `OpCodeDecoder`.

---

## Layers

### 1. `Decoder` trait (`instructions/decoder.rs`)

```rust
pub trait Decoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error>;
}
```

Each instruction group has its own decoder struct (e.g. `Add8Decoder`, `JumpDecoder`). The
top-level `OpCodeDecoder` holds a `Vec<Box<dyn Decoder>>` and tries each in turn, returning the
first successful decode or `Error::InvalidOpcode`.

### 2. `OpCode` trait (`instructions/opcode.rs`)

```rust
pub trait OpCode {
    fn execute(&self, cpu: &mut dyn Instructions) -> Result<u8, Error>;
}
```

Each concrete opcode struct (e.g. `Add8`, `Jump`, `CbInstruction`) carries the data decoded from
the byte stream — operand, cycle count, any variant flags — and implements `execute()` by calling
the corresponding method on `Instructions`. This is the first half of double dispatch.

### 3. `Instructions` trait (`instructions/instructions.rs`)

```rust
pub trait Instructions {
    fn add8(&mut self, opcode: &Add8) -> Result<u8, Error>;
    fn jump(&mut self, opcode: &Jump) -> Result<u8, Error>;
    fn cb(&mut self, opcode: &CbInstruction) -> Result<u8, Error>;
    // ... one method per instruction group
}
```

The second half of double dispatch. `Sm83` is the production implementation. `FakeCpu` is the
test stub used for isolated decoder and opcode unit tests.

### 4. `Sm83` (`sm83.rs`)

Implements `Instructions`. Each method reads its operands (registers, immediate bytes from PC,
or memory via HL), delegates arithmetic/flag logic to a pure function in `operations/`, writes
results back to registers or memory, and returns the cycle count.

---

## File layout

```
src/cpu/
├── sm83.rs                        # Sm83 struct — implements Instructions, owns memory + registers
├── registers.rs                   # Registers, Flags (bitflags)
├── instructions/
│   ├── mod.rs
│   ├── decoder.rs                 # Decoder trait + Error
│   ├── opcode.rs                  # OpCode trait
│   ├── opcodes.rs                 # OpCodeDecoder — top-level dispatch table
│   ├── instructions.rs            # Instructions trait
│   ├── operand.rs                 # Operand, Register8, Register16, Memory enums
│   ├── test/mod.rs                # FakeCpu — test stub for Instructions
│   ├── add/
│   │   ├── decoder.rs             # Add8Decoder, Add16Decoder, AddSP16Decoder
│   │   └── opcode.rs              # Add8, Add16, AddSP16 structs
│   ├── cb/
│   │   ├── decoder.rs             # CbDecoder — decodes all 256 0xCB-prefixed opcodes
│   │   └── opcode.rs              # CbInstruction, CbOp enum, CbTarget enum
│   └── ...                        # one directory per instruction group
└── operations/
    ├── add.rs                     # add_u8(), adc_u8(), add_sp_u16() — pure arithmetic + flags
    ├── sub.rs                     # sub_u8(), sbc_u8(), cp_u8()
    ├── cb.rs                      # rlc_u8(), bit_u8(), res_u8(), set_u8(), ...
    ├── inc_dec.rs                 # inc_u8(), dec_u8()
    ├── logic.rs                   # and_u8(), or_u8(), xor_u8()
    └── misc.rs                    # daa_u8()
```

---

## Instruction group directory structure

Every instruction group follows the same layout:

```
instructions/<group>/
├── mod.rs        — pub mod decoder; pub mod opcode;
├── decoder.rs    — XxxDecoder: matches opcode bytes, constructs typed opcode structs
└── opcode.rs     — Xxx struct with operand + cycles fields, implements OpCode
```

---

## Adding a new instruction

1. Create `instructions/<name>/opcode.rs` — define the struct, implement `OpCode::execute()` to call `cpu.my_instruction(&self)`.
2. Create `instructions/<name>/decoder.rs` — match the relevant opcode byte(s), return the struct.
3. Create `instructions/<name>/mod.rs` — declare both modules.
4. Declare `pub mod <name>` in `instructions/mod.rs`.
5. Add `fn my_instruction(&mut self, opcode: &MyInstruction) -> Result<u8, Error>` to the `Instructions` trait.
6. Add a stub to `FakeCpu` in `instructions/test/mod.rs`.
7. Implement `my_instruction()` in `sm83.rs`, delegating arithmetic to a pure function in `operations/` if needed.
8. Register the decoder in `OpCodeDecoder::new()` in `instructions/opcodes.rs`.

---

## Operations layer

Pure functions in `src/cpu/operations/` handle all arithmetic and flag logic. They take only
primitive values (`u8`, `u16`, `bool`) and return `(result, Flags)`. They have no access to
registers or memory. This makes them trivially unit-testable and keeps `Sm83` methods thin.

```rust
// operations/add.rs
pub fn add_u8(a: u8, b: u8) -> (u8, Flags) { ... }
pub fn adc_u8(a: u8, b: u8, carry: bool) -> (u8, Flags) { ... }

// sm83.rs
fn add8(&mut self, opcode: &Add8) -> Result<u8, InstructionError> {
    let operand = self.get_8bit_operand(opcode.operand)?;
    let (result, flags) = add_u8(self.registers.a, operand);
    self.registers.a = result;
    self.registers.f = flags;
    Ok(opcode.cycles)
}
```

---

## Testing strategy

| Layer | Test approach |
|---|---|
| `operations/` | Unit tests inline in each file — pure functions, no mocks needed |
| `opcode.rs` | Unit tests using `FakeCpu` — verify correct method called with correct operand + cycles |
| `decoder.rs` | Unit tests using `FakeCpu` — verify each byte decodes to correct operand + cycles |
| `sm83.rs` | Integration tests — construct `Sm83` with `FakeMemory`, tick, assert register/memory state |

`FakeCpu` captures the last operand seen per instruction type and records cycle counts, letting
decoder and opcode tests verify dispatch without touching real CPU logic.
