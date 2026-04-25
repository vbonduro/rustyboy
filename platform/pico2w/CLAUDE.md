# rustyboy-pico2w

Embassy-based Game Boy emulator firmware for the Raspberry Pi Pico 2W (RP2350A).

## Crate layout

```
platform/pico2w/
├── .cargo/config.toml   # Cross-compilation target + probe-rs runner
├── src/
│   └── main.rs          # Entry point
├── build.rs             # Exposes memory.x to the linker
├── memory.x             # Flash/RAM layout for RP2350A
└── CLAUDE.md            # This file
```

## Toolchain setup

```sh
# Add the Cortex-M33 (hard-float) target
rustup target add thumbv8m.main-none-eabihf

# Install probe-rs for flashing + RTT logging via SWD debug probe
cargo install probe-rs-tools --locked

# Optional: install picotool for BOOTSEL / drag-and-drop flashing
# https://github.com/raspberrypi/picotool
```

## Building

**Always build from within `platform/pico2w/`** so that `.cargo/config.toml`
is picked up and the correct cross-compilation target is used.

```sh
cd platform/pico2w

# Debug build
cargo build

# Release build (use this for flashing)
cargo build --release
```

Running `cargo build` from the workspace root targets the host architecture
and will fail — this is expected. Use `cargo build -p rustyboy-pico2w` only
if you have the target set workspace-wide.

## Flashing

### Via SWD debug probe (probe-rs) — recommended for development

Connect a Raspberry Pi Debug Probe (or any CMSIS-DAP probe) to the Pico 2W
SWD header (SWDIO, SWDCLK, GND).

```sh
cd platform/pico2w
cargo run --release
```

`probe-rs run` flashes the binary and immediately starts streaming defmt RTT
logs to the terminal.

### Via picotool (BOOTSEL) — no debug probe required

1. Hold BOOTSEL on the Pico 2W while connecting USB → appears as mass storage.
2. Build the ELF:
   ```sh
   cargo build --release
   ```
3. Flash with picotool:
   ```sh
   picotool load -f target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w
   picotool reboot
   ```

Note: picotool does not stream RTT logs. Use a separate RTT viewer or fall
back to syslog (available after Bead 7).

## Logging

Logging uses `defmt` over RTT. When running via `cargo run` / probe-rs,
logs appear in the terminal automatically.

Log level is controlled by the `DEFMT_LOG` environment variable
(default: `debug`, set in `.cargo/config.toml`):

```sh
DEFMT_LOG=trace cargo run --release   # verbose
DEFMT_LOG=info  cargo run --release   # quieter
```

## Hardware notes

### RP2350A specs
- Dual ARM Cortex-M33 @ up to 150MHz
- 520KB SRAM
- 4MB QSPI flash (XIP-mapped at 0x10000000)

### Onboard LED
The Pico 2W LED is routed through the CYW43439 WiFi chip, not a GPIO pin.
Controlling it requires the cyw43 driver (added in Bead 6). During Bead 1,
use an external LED on GP15 (LED + 330Ω resistor to GND).

### memory.x (Bead 1 — single-app layout)
Bead 8 (OTA) will restructure flash into dual-bank partitions for
`embassy-boot-rp`. When that happens, `memory.x` and this document will be
updated with the new layout.

## GPIO pin assignment

Full pin table is maintained in `docs/wiring.md` (created in Bead 11).
Summary of allocations so far:

| GPIO | Function                  | Bead |
|------|---------------------------|------|
| GP4  | SPI0 MISO (SD card)       |  4   |
| GP5  | SPI0 CS   (SD card)       |  4   |
| GP6  | SPI0 CLK  (SD card)       |  4   |
| GP7  | SPI0 MOSI (SD card)       |  4   |
| GP15 | Dev blinky LED            |  1   |
| GP16 | I2S DIN (MAX98357A)       |  5   |
| GP17 | MAX98357A SD_MODE         |  5   |
| GP18 | Brown-out detect          |  9   |
| GP20 | SD module power enable    |  4   |

### SD module power switch (GP20)

GP20 controls a P-channel MOSFET (or a load switch IC such as AP2112/TPS22860)
that gates VBUS (5 V) to the SD module's VCC pin. This allows the firmware to
power-cycle the module for reliable recovery after a stuck SPI transaction:

```
Pico GP20 ──[1 kΩ]──► NPN base   (e.g. MMBT3904)
                        NPN emitter ── GND
                        NPN collector ──[10 kΩ pull-up to VBUS]──► PMOS gate
VBUS ──────────────────────────────────────────────────────────────► PMOS source
                                                                      PMOS drain ── Module VCC
```

Or replace the discrete circuit with a dedicated load switch (EN active-high):
```
GP20 ── EN,  VBUS ── VIN,  VOUT ── Module VCC
```

Logic: GP20 HIGH = module powered, GP20 LOW = module off.
Hold LOW for ≥ 100 ms (cap discharge), then HIGH + 250 ms (card power-up)
before reinitialising the SdCard object.
