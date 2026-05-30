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

## Unified build & deploy (`cargo xtask`)

`cargo xtask` wraps the per-platform build, deploy, and flash workflows behind
one command so you don't have to remember the underlying Docker / picotool /
probe-rs invocations. Run it from the workspace root.

```sh
cargo xtask <command> <target>
```

| Command | Target | What it does |
|---|---|---|
| `build`  | `web`  | Build the `rustyboy-web` Docker image |
| `build`  | `pico` | Cross-compile the pico2w firmware (release, ARM Cortex-M33) |
| `deploy` | `web`  | Build the image, (re)start the container, and print the URL |
| `deploy` | `pico` | Build the firmware and flash over USB BOOTSEL — no SWD probe needed |
| `run`    | `pico` | Flash via SWD probe and stream defmt RTT logs (Ctrl-C to stop) |
| `crash`  | `pico` | Pull the crash log from the device over USB and decode it (no probe) |
| `setup`  | `web`  | Install Docker Engine and add the current user to the `docker` group |
| `setup`  | `pico` | Build + install picotool from source and write Raspberry Pi udev rules |

Common workflows:

```sh
# Web: build + run the browser emulator, then open the printed http://<ip>:8080
cargo xtask deploy web

# Pico (no debug probe): hold BOOTSEL, plug in USB, then flash
cargo xtask deploy pico

# Pico (with SWD probe): flash and watch live RTT logs
cargo xtask run pico
```

First-time setup for each platform is a one-off `cargo xtask setup <target>`.

### Web options (`build web` / `deploy web`)

| Flag | Applies to | Effect |
|---|---|---|
| _(default)_ | `deploy web` | **Dev mode**: server runs with `DEV_MODE=1` (Google OAuth bypassed, a local dev user is auto-logged-in) and `RUST_LOG=info` (server logs + browser `[client]` breadcrumbs appear in `docker logs`). |
| `--prod` | `deploy web` | Production mode: disables the dev-mode default and requires real Google sign-in. |
| `--debug-overlay` | `build web`, `deploy web` | Build the WASM client with the on-screen debug overlay (`--features debug-overlay`; adds the "DBG" toggle). Off by default. |
| `--roms <DIR>` | `deploy web` | Bind-mount `<DIR>` as the ROM library (read-only). Defaults to `<workspace>/roms`; a leading `~` is expanded. |

```sh
# Dev mode is the default — no auth, logs visible, custom ROM folder:
cargo xtask deploy web --roms ~/roms/extracted

# Production deploy with real Google auth:
cargo xtask deploy web --prod

# Dev mode with the on-screen debug overlay:
cargo xtask deploy web --debug-overlay
```

> **Note:** `deploy web` serves over plain HTTP on your LAN IP. The client's
> ROM hashing falls back to a pure-JS SHA-256 when `crypto.subtle` is
> unavailable (it only exists in secure contexts — HTTPS or `localhost`), so
> ROMs load correctly regardless of how the page is reached.
