[![codecov](https://codecov.io/gh/vbonduro/rustyboy/graph/badge.svg?token=KODKS871ZJ)](https://codecov.io/gh/vbonduro/rustyboy)

# rustyboy

A Game Boy (DMG) emulator written in Rust, optimised for real-time performance on resource-constrained hardware.

## Accuracy trade-off

Peripherals (PPU, APU, timer, serial) are advanced once per complete SM83
instruction rather than once per M-cycle. This cuts peripheral-advancement
overhead roughly 4× compared to M-cycle granularity. The trade-off is that
behaviour which depends on mid-instruction peripheral state — primarily
wave-channel read/write-while-on quirks — is not reproduced accurately.

## Features

- SM83 CPU (all official opcodes + CB-prefixed instructions)
- Scanline-based PPU with OAM DMA, sprites, window, and BG rendering
- MBC1 / MBC1 Multicart / MBC3 / No-MBC cartridge support
- APU with all four channels (pulse × 2, wave, noise) and frame sequencer
- Timer peripheral (DIV/TIMA/TMA/TAC) with accurate DIV-reset behavior
- Joypad peripheral (P1 register, joypad interrupt)
- Serial port output (used by Blargg test ROMs)
- `no_std` core — runs on bare metal and WASM

## Test coverage

| Suite | Status | Notes |
|---|---|---|
| Blargg cpu_instrs (11/11) | ✅ | |
| Blargg instr_timing | ✅ | |
| Blargg mem_timing | ✅ | |
| Blargg dmg_sound (9/12) | ⚠️ | Tests 09, 10, 12 skipped — wave channel mid-instruction quirks require M-cycle accuracy |
| dmg-acid2 (PPU) | ✅ | |
| Mooneye MBC1 (13/13) | ✅ | |
| Mooneye OAM DMA | ✅ | |

## Repository layout

```
rustyboy/
├── core/               # no_std emulator core (CPU, PPU, APU, memory)
├── platform/
│   ├── web/            # Browser platform (Axum server + WASM client)
│   │   ├── client/     # wasm-bindgen crate compiled to WASM
│   │   ├── server/     # Axum HTTP server serving ROMs and static files
│   │   └── Dockerfile  # Multi-stage Docker build
│   └── pico2w/         # Raspberry Pi Pico 2W embedded platform
│       ├── src/        # Embassy async firmware
│       ├── memory.x    # RP2350A flash/RAM layout
│       └── README.md   # Setup, wiring, and flash instructions
└── Cargo.toml          # Workspace root
```

## Platforms

| Platform | Description |
|---|---|
| [web](platform/web/README.md) | Docker-hosted browser emulator with DMG Game Boy UI |
| [pico2w](platform/pico2w/README.md) | Portable handheld on Raspberry Pi Pico 2W (RP2350A) |

## Building

```sh
# Build and test the core
cargo test -p rustyboy-core

# Build the web platform (requires wasm-pack)
# See platform/web/README.md for full instructions

# Build the Pico 2W firmware (requires cross-compilation target)
# See platform/pico2w/README.md for full instructions
cd platform/pico2w
cargo build --release
```
