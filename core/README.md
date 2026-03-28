# rustyboy-core

The `no_std` emulator core. Contains the full Game Boy hardware implementation with no platform dependencies — suitable for native, WASM, and bare-metal targets.

## Architecture

```
core/src/
├── lib.rs
├── cpu/
│   ├── sm83.rs          # Top-level CPU: fetch/decode/execute loop, bus access, peripherals
│   ├── cpu.rs           # Cpu trait (tick())
│   ├── registers.rs     # AF/BC/DE/HL/SP/PC + Flags bitfield
│   ├── instructions/    # One file per instruction group (ld, alu, jump, cb, ...)
│   ├── operations/      # Shared ALU helpers (add, sub, rotate, ...)
│   └── peripheral/
│       ├── ppu.rs       # Scanline PPU, OAM DMA, 160×144 framebuffer
│       ├── apu.rs       # APU: pulse×2, wave, noise, frame sequencer
│       ├── timer.rs     # DIV/TIMA/TMA/TAC with DIV-reset behavior
│       ├── joypad.rs    # P1 register, active-low button matrix
│       └── serial.rs    # Serial port (SC/SB), captures output bytes
└── memory/
    ├── memory.rs        # GameBoyMemory: VRAM, WRAM, OAM, IO, HRAM, IE
    └── cartridge.rs     # NoMbc, Mbc1, Mbc1Multicart, Mbc3
```

## CPU

`Sm83` implements the SM83 core used in the original DMG Game Boy. Each `tick()` call executes one instruction and advances all peripherals by the corresponding number of M-cycles (4 T-cycles each).

Bus reads and writes are M-cycle accurate: peripherals advance *before* the memory access, matching the real hardware timing. The APU additionally advances per T-cycle for wave channel accuracy.

### Skipping the boot ROM

Pass post-boot register values and call `with_dmg_state()` to skip the boot ROM:

```rust
let cpu = Sm83::new(Box::new(memory), Box::new(OpCodeDecoder::new()))
    .with_registers(Registers {
        a: 0x01, f: Flags::from_bits_truncate(0xB0),
        b: 0x00, c: 0x13,
        d: 0x00, e: 0xD8,
        h: 0x01, l: 0x4D,
        pc: 0x0100,
        sp: 0xFFFE,
    })
    .with_dmg_state(); // seeds LCDC, BGP, OBP0, OBP1
```

## PPU

Scanline-based renderer. Each dot advances the PPU state machine through OAM search → pixel transfer → HBlank → VBlank. The framebuffer is `[u8; 160 × 144]` where each byte is a palette index (0–3). Callers apply their own color mapping.

OAM DMA is triggered by writes to `0xFF46` and copies 160 bytes from the source page into OAM over 160 M-cycles.

## APU

All four DMG channels are implemented:

| Channel | Type | Registers |
|---|---|---|
| CH1 | Pulse with sweep | NR10–NR14 |
| CH2 | Pulse | NR21–NR24 |
| CH3 | Wave (4-bit samples) | NR30–NR34, 0xFF30–0xFF3F |
| CH4 | Noise (LFSR) | NR41–NR44 |

The frame sequencer clocks length counters, volume envelopes, and the frequency sweep at the correct DIV-derived rates. CH3 wave RAM reads are T-cycle accurate.

> **Note:** The APU currently tracks channel state and register values but does not yet produce PCM audio output. Audio will be added in a future update.

## Memory / Cartridge

`GameBoyMemory` maps the full 16-bit address space. Cartridge ROM and RAM are abstracted behind the `Cartridge` trait:

| Type code | Struct | Description |
|---|---|---|
| `0x00` | `NoMbc` | 32 KiB ROM only |
| `0x01–0x03` | `Mbc1` | Up to 2 MiB ROM / 32 KiB RAM |
| `0x01–0x03` (64-bank multicart) | `Mbc1Multicart` | Multicart with 4-bit sub-banking |
| `0x0F–0x13` | `Mbc3` | Up to 2 MiB ROM / 32 KiB RAM + RTC stub |

## Running tests

```sh
# All unit tests
cargo test -p rustyboy-core --lib

# Integration test suites (requires ROM files in the paths configured in tests/)
cargo test -p rustyboy-core --test blargg_cpu_instrs
cargo test -p rustyboy-core --test blargg_dmg_sound
cargo test -p rustyboy-core --test dmg_acid2
cargo test -p rustyboy-core --test mooneye_mbc1
```
