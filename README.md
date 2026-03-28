[![codecov](https://codecov.io/gh/vbonduro/rustyboy/graph/badge.svg?token=KODKS871ZJ)](https://codecov.io/gh/vbonduro/rustyboy)

# rustyboy

A cycle-accurate Game Boy (DMG) emulator written in Rust.

## Features

- Cycle-accurate SM83 CPU (all official opcodes + CB-prefixed instructions)
- Scanline-based PPU with OAM DMA, sprites, window, and BG rendering
- MBC1 / MBC1 Multicart / MBC3 / No-MBC cartridge support
- APU with all four channels (pulse × 2, wave, noise) and frame sequencer
- Timer peripheral (DIV/TIMA/TMA/TAC) with accurate DIV-reset behavior
- Joypad peripheral (P1 register, joypad interrupt)
- Serial port output (used by Blargg test ROMs)
- `no_std` core — runs on bare metal and WASM

## Test coverage

| Suite | Status |
|---|---|
| Blargg cpu_instrs (11/11) | ✅ |
| Blargg instr_timing | ✅ |
| Blargg mem_timing | ✅ |
| Blargg dmg_sound (12/12) | ✅ |
| dmg-acid2 (PPU) | ✅ |
| Mooneye MBC1 (13/13) | ✅ |
| Mooneye OAM DMA | ✅ |

## Repository layout

```
rustyboy/
├── core/               # no_std emulator core (CPU, PPU, APU, memory)
├── platform/
│   └── web/            # Browser platform (Axum server + WASM client)
│       ├── client/     # wasm-bindgen crate compiled to WASM
│       ├── server/     # Axum HTTP server serving ROMs and static files
│       └── Dockerfile  # Multi-stage Docker build
└── Cargo.toml          # Workspace root
```

## Platforms

| Platform | Description |
|---|---|
| [web](platform/web/README.md) | Docker-hosted browser emulator with DMG Game Boy UI |

## Building

```sh
# Build and test the core
cargo test -p rustyboy-core

# Build the web platform (requires wasm-pack)
# See platform/web/README.md for full instructions
```
