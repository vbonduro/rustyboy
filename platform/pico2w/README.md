# rustyboy — Pico 2W platform

A portable Game Boy emulator running on the Raspberry Pi Pico 2W (RP2350A), using the `rustyboy-core` no_std emulator core with Embassy async firmware.

## Bill of Materials

| Component | Notes |
|---|---|
| Raspberry Pi Pico 2W | RP2350A MCU, 520KB SRAM, 4MB flash, CYW43439 WiFi |
| ACEIRMC ILI9341 2.8" SPI TFT LCD | 240×320, 5V/3.3V, SPI interface |
| MicroSD SPI breakout module | 3.3V compatible, SPI mode |
| MAX98357A I2S DAC breakout | Class D amp, 3.2W @ 5V into 4Ω, mono |
| 8Ω 2W speaker | 28mm–36mm round |
| 8× tactile buttons | 6×6mm or 12×12mm (D-pad ×4, A, B, Start, Select) |
| LiPo battery | 1000–2000mAh single-cell 3.7V (portable use) |
| TP4056 w/ protection | USB-C LiPo charger + over-discharge protection |
| MT3608 boost converter | 3.7V → 5V for MAX98357A (portable use) |
| Power switch | Slide or toggle, rated for battery current |
| 470µF–1000µF electrolytic capacitor | Across VSYS for brown-out save detection |
| 330Ω resistor | Current limiting for dev blinky LED on GP15 |

## GPIO pin assignment

| GPIO | Function | Bead |
|---|---|---|
| GP0 | Button: A | 3 |
| GP1 | Button: B | 3 |
| GP2 | Button: Start | 3 |
| GP3 | Button: Select | 3 |
| GP4 | MAX98357A SD_MODE | 5 |
| GP5 | Brown-out detect | 9 |
| GP8 | Display DC | 2 |
| GP9 | Display CS | 2 |
| GP10 | SPI1 CLK (display) | 2 |
| GP11 | SPI1 MOSI (display) | 2 |
| GP12 | Display RST | 2 |
| GP13 | Display backlight | 2 |
| GP14 | I2S BCLK (MAX98357A) | 5 |
| GP15 | Dev blinky LED / I2S LRCLK | 1/5 |
| GP16 | SPI0 MISO (SD card) | 4 |
| GP17 | SPI0 CS (SD card) | 4 |
| GP18 | SPI0 CLK (SD card) | 4 |
| GP19 | SPI0 MOSI (SD card) | 4 |
| GP20 | I2S DIN (MAX98357A) | 5 |
| GP21 | Button: D-pad Up | 3 |
| GP22 | Button: D-pad Down | 3 |
| GP26 | Button: D-pad Left | 3 |
| GP27 | Button: D-pad Right | 3 |

> Display uses SPI1; SD card uses SPI0. These are separate peripherals and can run concurrently.

## Toolchain setup

```sh
# 1. Add the Cortex-M33 hard-float target
rustup target add thumbv8m.main-none-eabihf

# 2. Install probe-rs for SWD flashing and RTT log streaming
cargo install probe-rs-tools --locked
```

## Building

**Always build from within `platform/pico2w/`** — the `.cargo/config.toml` there sets the correct cross-compilation target.

```sh
cd platform/pico2w

# Firmware (embedded, ARM)
cargo build --release

# Host unit tests (no hardware required)
cargo test-host
```

Running `cargo build` from the workspace root targets the host architecture and will fail — this is by design.

### Host display viewer

The `display-viewer` tool renders display output to PNG files on the host, useful for iterating on the splash animation and framebuffer rendering without flashing hardware.

```sh
# Final splash frame → /tmp/splash_final.png
cargo run -p display-viewer -- splash --last

# All splash frames → /tmp/splash_000.png … splash_final.png
cargo run -p display-viewer -- splash

# DMG palette test frame → /tmp/frame.png
cargo run -p display-viewer -- frame
```

## Flashing

### Via SWD debug probe — recommended for development

Connect a Raspberry Pi Debug Probe to the Pico 2W SWD header:

| Debug Probe | Pico 2W |
|---|---|
| SWDIO | GP_SWDIO |
| SWDCLK | GP_SWDCLK |
| GND | GND |

```sh
cd platform/pico2w
cargo run --release
```

`probe-rs` flashes the binary and immediately begins streaming defmt RTT logs to the terminal.

### Via BOOTSEL / picotool — no debug probe required

1. Hold **BOOTSEL** on the Pico 2W while connecting USB. It appears as a mass storage device.
2. Build the ELF:
   ```sh
   cd platform/pico2w
   cargo build --release
   ```
3. Flash with picotool:
   ```sh
   picotool load -f ../../target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w
   picotool reboot
   ```

RTT logs are not available without a debug probe. After Bead 7, syslog over WiFi provides equivalent visibility.

## Current firmware behaviour

On boot the firmware:

1. Plays the **VINTENDO** splash animation on the ILI9341 display (~640ms slide-in + 2s hold)
2. Enters a **~60 Hz main loop** that polls all 8 buttons with 10ms software debounce
3. Logs button press/release events over defmt RTT
4. Detects a **Start+Select hold (1s)** and logs a menu-combo event
5. Blinks the dev LED on GP15 at 1Hz as a heartbeat

Example RTT output:

```
INFO  rustyboy-pico2w v0.1.0 starting
INFO  display: ILI9341 initialised
INFO  entering main loop
INFO  btn press:   A
INFO  btn release: A
INFO  btn press:   Start
INFO  btn press:   Select
WARN  menu combo triggered
INFO  btn release: Start
INFO  btn release: Select
```

## Logging

```sh
# Default level is debug (set in .cargo/config.toml)
cargo run --release

# Override log level
DEFMT_LOG=trace cargo run --release
DEFMT_LOG=info  cargo run --release
```

After Bead 7, structured syslog (UDP RFC 5424) is available over WiFi — no debug probe required for log access in the field.

## OTA firmware updates

Firmware is versioned with `pico2w/vMAJOR.MINOR.PATCH` git tags. Pushing a tag triggers a GitHub Actions build that attaches `rustyboy-pico2w-vX.Y.Z.bin` and a SHA256 checksum to the GitHub release.

The device checks `api.github.com/repos/vbonduro/rustyboy/releases/latest` on WiFi connect and prompts the user if a newer version is available. See Bead 8 for implementation details.

## SD card layout

```
/
├── roms/          # .gb and .gbc ROM files
├── saves/
│   └── <rom>/
│       ├── slot0.rbss  # Auto-save (RBSS v1 format)
│       ├── slot1.rbss  # Manual save slots
│       └── battery.sav # Cartridge external RAM
└── config/
    ├── network.toml    # WiFi credentials + syslog host
    └── auth.toml       # Web server sync token (Bead 10)
```

## Controls

| Button | Function |
|---|---|
| D-pad | D-pad |
| A | A button |
| B | B button |
| Start | Start |
| Select | Select |
| Start + Select (hold 1s) | In-game menu (save/load/OTA) |

## Network configuration (first boot)

On first boot with no `/config/network.toml` on the SD card, the device starts in AP mode:

1. Connect to WiFi SSID **`RustyBoy-Setup`** from your phone or laptop
2. A captive portal page opens automatically (or navigate to `192.168.4.1`)
3. Enter your WiFi SSID and password and submit
4. The device writes the config to SD and reboots into station mode

If WiFi credentials fail after 3 connection attempts, the device falls back to AP mode automatically.

### `config/network.toml` format

```toml
ssid = "MyNetwork"
password = "secret"
syslog_host = "192.168.1.10"   # optional
syslog_port = 514               # optional, default 514
```

## Platform implementation status

| Bead | Feature | Status |
|---|---|---|
| 1 | Scaffold, build system, blinky | ✅ Done |
| 2 | ILI9341 display driver + framebuffer | ✅ Done |
| 3 | Input (8 buttons, debounce, menu combo) | ✅ Done |
| 4 | SD card ROM storage + StreamingCartridge | 🔲 Pending |
| 5 | Core integration + game loop + I2S audio | 🔲 Pending |
| 6 | WiFi + captive portal setup | 🔲 Pending |
| 7 | Logging + UDP syslog | 🔲 Pending |
| 8 | OTA via GitHub Releases | 🔲 Pending |
| 9 | Save states + battery saves | 🔲 Pending |
| 10 | Web server sync (low priority) | 🔲 Pending |
