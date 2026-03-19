# Peripheral Bus Architecture

## Problem

The Game Boy memory map contains I/O registers that, when written, trigger hardware behaviour:
writing `0x80` to `0xFF02` starts a serial transfer; writing to the timer control register
changes the timer frequency; the interrupt controller raises CPU signals. A flat RAM region
cannot express this — logic needs to run in response to specific writes.

We want to avoid coupling that logic into `GameBoyMemory`. It should not know about serial
output buffers, timer counters, or interrupt lines. Each peripheral owns its own state and
behaviour.

---

## Design: Synchronous Pub/Sub Bus

`GameBoyMemory` records every I/O write as a `BusEvent` and stores it in an internal queue.
After each CPU instruction, `Sm83` drains that queue through a `PeripheralBus`, which routes
each event to the peripherals registered for that address range. Peripherals receive the event
and a mutable reference to memory so they can read related registers or write back results.

This is a **synchronous pub/sub** pattern — no threads, no async, no external dependencies.
Events are produced during an instruction and consumed once per tick, which matches the
cycle-accurate nature of the hardware.

---

## Component diagram

```
┌──────────────────────────────────────────────────────────────────┐
│                            Sm83                                  │
│                                                                  │
│  memory: Box<dyn Memory>                                         │
│  bus: PeripheralBus                                              │
│  ime: bool                                                       │
│  halted: bool                                                    │
│                                                                  │
│  tick():                                                         │
│    1. execute instruction (reads/writes memory)                  │
│    2. bus.flush(&mut memory, &mut peripherals)                   │
│    3. check_interrupts()   ← reads IE (0xFFFF) & IF (0xFF0F)    │
└──────────────────────────────────────────────────────────────────┘
                │
                │ flush()
                ▼
┌──────────────────────────────────────────────────────────────────┐
│                         PeripheralBus                            │
│                                                                  │
│  subscriptions: Vec<(RangeInclusive<u16>, Box<dyn Peripheral>)> │
│                                                                  │
│  flush(mem: &mut dyn Memory):                                    │
│    for event in mem.drain_events():                              │
│      for sub in subscriptions where sub.range.contains(addr):   │
│        sub.peripheral.handle(&event, mem)                        │
└──────────────────────────────────────────────────────────────────┘
         │ drain_events()            │ handle(event, mem)
         ▼                           ▼
┌─────────────────────┐   ┌──────────────────────────────────────┐
│   GameBoyMemory     │   │           trait Peripheral           │
│                     │   │                                      │
│  io: Ram            │   │  fn handle(                          │
│  events: VecDeque   │   │    event: &BusEvent,                 │
│          <BusEvent> │   │    mem: &mut dyn Memory,             │
│                     │   │  )                                   │
│  write(addr, val):  │   │                                      │
│    write to RAM     │   ├──────────────────────────────────────┤
│    push BusEvent    │   │  SerialPort                          │
│                     │   │    range: 0xFF01..=0xFF02            │
│  drain_events()     │   │    output: Vec<u8>                   │
│    → VecDeque       │   │    on 0xFF02 write w/ bit7 set:      │
└─────────────────────┘   │      read 0xFF01 from mem, push byte │
                          ├──────────────────────────────────────┤
                          │  InterruptController                 │
                          │    range: 0xFF0F, 0xFFFF             │
                          │    (stub: no-op, memory handles R/W) │
                          ├──────────────────────────────────────┤
                          │  Timer  (future)                     │
                          │    range: 0xFF03..=0xFF07            │
                          └──────────────────────────────────────┘
```

---

## Types

### `BusEvent`

Produced by `GameBoyMemory::write()` for any address in the I/O range (`0xFF00–0xFFFF`).
Read-only — peripherals cannot mutate the event itself, only respond to it.

```rust
pub struct BusEvent {
    pub address: u16,
    pub value: u8,
}
```

### `trait Peripheral`

Implemented by each hardware component. Receives the triggering event and a mutable memory
reference so it can read related registers (e.g. serial reads `SB` at `0xFF01` after being
triggered by a write to `SC` at `0xFF02`) or write back to memory (e.g. interrupt controller
clears a bit in `IF` at `0xFF0F`).

```rust
pub trait Peripheral {
    fn handle(&mut self, event: &BusEvent, mem: &mut dyn Memory);
}
```

### `PeripheralBus`

Owns a list of `(address_range, peripheral)` pairs. `flush()` drains the event queue from
memory and routes each event to matching peripherals. A single event may match multiple
subscribers (e.g. a debug logger could subscribe to all addresses alongside a real peripheral).

```rust
pub struct PeripheralBus {
    subscriptions: Vec<(RangeInclusive<u16>, Box<dyn Peripheral>)>,
}

impl PeripheralBus {
    pub fn subscribe(&mut self, range: RangeInclusive<u16>, peripheral: Box<dyn Peripheral>);
    pub fn flush(&mut self, mem: &mut dyn Memory);
}
```

### `GameBoyMemory` changes

The I/O range (`0xFF00–0xFF7F`) and the IE register (`0xFFFF`) move from `Unmapped` to actual
R/W RAM. Every write to these addresses additionally pushes a `BusEvent` onto the queue.

```rust
pub struct GameBoyMemory {
    // existing fields ...
    io: Ram,           // 0xFF00–0xFF7F (128 bytes)
    ie: u8,            // 0xFFFF — single byte, special-cased
    events: VecDeque<BusEvent>,
}

impl GameBoyMemory {
    pub fn drain_events(&mut self) -> VecDeque<BusEvent> { ... }
}
```

---

## Event flow: serial transfer example

```
ROM writes 'H' (0x48) to SB (0xFF01)
  → GameBoyMemory::write(0xFF01, 0x48)
  → io.write(0x01, 0x48)
  → events.push(BusEvent { address: 0xFF01, value: 0x48 })

ROM writes 0x81 to SC (0xFF02) to start transfer
  → GameBoyMemory::write(0xFF02, 0x81)
  → io.write(0x02, 0x81)
  → events.push(BusEvent { address: 0xFF02, value: 0x81 })

Sm83::tick() completes instruction
  → bus.flush(&mut memory)
    → BusEvent { 0xFF01, 0x48 }: no subscriber reacts (SC not set yet... but events are sequential)
    → BusEvent { 0xFF02, 0x81 }: SerialPort::handle() called
         → value & 0x80 != 0 → transfer triggered
         → read mem.read(0xFF01) → 0x48
         → output.push(0x48)

Test asserts: serial_port.output() == b"H"
```

---

## Interrupt flow

The `InterruptController` is not a peripheral in the reactive sense — `IF` (`0xFF0F`) is written
by hardware (timer overflow, serial complete, VBlank) and read by the CPU. For now:

- `IF` and `IE` are plain R/W bytes in memory (no peripheral needed)
- The serial peripheral writes a bit into `IF` when a transfer completes (future work)
- `Sm83::check_interrupts()` reads `IE & IF` from memory each tick, dispatches if `IME` is set

This avoids cross-peripheral references for now. If the timer peripheral needs to raise an
interrupt, it writes directly to `0xFF0F` via the `mem` reference passed to `handle()`.

---

## IME and HALT in `Sm83`

```rust
pub struct Sm83 {
    memory: Box<dyn Memory>,
    bus: PeripheralBus,
    registers: Registers,
    opcodes: Box<dyn Decoder>,
    ime: bool,
    halted: bool,
}
```

- `MiscOp::Di` → `self.ime = false`
- `MiscOp::Ei` → `self.ime = true`
- `MiscOp::Halt` → `self.halted = true`
- `tick()` returns early with 4 cycles if `self.halted` and `(IE & IF) == 0`
- `pub fn is_halted() -> bool` — test harness stop condition

---

## File layout

```
src/
├── memory/
│   ├── memory.rs          # GameBoyMemory — adds io: Ram, ie: u8, events queue
│   └── rom.rs             # ROMVec, Ram (unchanged)
└── cpu/
    ├── sm83.rs            # adds ime, halted, PeripheralBus, check_interrupts()
    └── peripheral/
        ├── mod.rs
        ├── bus.rs         # PeripheralBus, BusEvent, trait Peripheral
        └── serial.rs      # SerialPort — captures serial output
```

---

## Testing strategy

| Component | Approach |
|---|---|
| `BusEvent` queue | Unit test `GameBoyMemory`: write to I/O address, assert `drain_events()` returns event |
| `PeripheralBus` routing | Unit test with a spy `Peripheral` that records received events |
| `SerialPort` | Unit test: write SB + SC directly, call `handle()`, assert `output()` |
| `Sm83` integration | Tick a ROM that writes serial output, assert `serial_port.output()` matches expected string |

---

## Implementation order

| Step | Work |
|---|---|
| 1 | Add `io: Ram` and `ie: u8` to `GameBoyMemory`; map `0xFF00–0xFF7F` and `0xFFFF` |
| 2 | Add `events: VecDeque<BusEvent>` to `GameBoyMemory`; push on every I/O write |
| 3 | Implement `trait Peripheral`, `PeripheralBus`, `BusEvent` in `cpu/peripheral/` |
| 4 | Implement `SerialPort` peripheral |
| 5 | Add `ime`, `halted`, `bus` to `Sm83`; implement `check_interrupts()`, `is_halted()` |
| 6 | Wire `bus.flush()` into `tick()` |
| 7 | Integration test: load Blargg ROM, tick to HALT, assert serial output |
