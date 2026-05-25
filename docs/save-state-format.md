# Save State Format (RBSS)

rustyboy save states use the **RBSS** (RustyBoy Save State) binary format.
The canonical implementation lives in `core/src/cpu/save_state.rs`.
Save state files are stored on the SD card under `saves/<rom-id>/slotN.rbss`.

## Versions

| Version | Description                                                             |
|---------|-------------------------------------------------------------------------|
| v1      | Legacy — MBC and cart RAM appended with a u16 length prefix            |
| v2      | Current — MBC and cart RAM stored as tagged length-prefixed sections   |

`SaveState::from_blob` accepts both versions.  All new saves use **v2**.

## Binary layout

All multi-byte integers are **little-endian**.  The blob is a flat byte
sequence with no external framing.

### Header (6 bytes, fixed)

| Offset | Size | Field     | Value                                   |
|--------|------|-----------|-----------------------------------------|
| 0      | 4    | `magic`   | `b"RBSS"`                              |
| 4      | 2    | `version` | `1` (v1) or `2` (v2) as u16 LE        |

### CPU state (22 bytes, fixed)

Saved by `CpuState::serialize` / parsed by `CpuState::parse`.

| Offset | Size | Field           | Notes                                   |
|--------|------|-----------------|-----------------------------------------|
| +0     | 1    | `a`             | Accumulator                             |
| +1     | 1    | `b`             |                                         |
| +2     | 1    | `c`             |                                         |
| +3     | 1    | `d`             |                                         |
| +4     | 1    | `e`             |                                         |
| +5     | 1    | `h`             |                                         |
| +6     | 1    | `l`             |                                         |
| +7     | 1    | `f`             | Flags: bits `7-4` = Z N H C             |
| +8     | 2    | `sp`            | Stack pointer, u16 LE                   |
| +10    | 2    | `pc`            | Program counter, u16 LE                 |
| +12    | 1    | `ime`           | `0` = Disabled, `1` = Pending, `2` = Enabled |
| +13    | 1    | `halted`        | `0` = running, `1` = halted            |
| +14    | 8    | `cycle_counter` | Total T-cycles elapsed, u64 LE         |

### Timer state (5 bytes, fixed)

| Offset | Size | Field              | Notes                              |
|--------|------|--------------------|------------------------------------|
| +0     | 2    | `internal_counter` | Raw 16-bit DIV counter, u16 LE    |
| +2     | 1    | `tima`             | Timer counter (0xFF00 + 5)        |
| +3     | 1    | `tma`              | Timer modulo (0xFF00 + 6)         |
| +4     | 1    | `tac`              | Timer control (0xFF00 + 7)        |

### PPU state (15 bytes, fixed)

| Offset | Size | Field                  | Notes                                        |
|--------|------|------------------------|----------------------------------------------|
| +0     | 2    | `dot`                  | Current dot within the scanline, u16 LE     |
| +2     | 1    | `ly`                   | Current scanline (0–153)                    |
| +3     | 1    | `mode`                 | `0`=HBlank `1`=VBlank `2`=OAM `3`=Transfer |
| +4     | 1    | `window_line_counter`  | Internal window line counter                |
| +5     | 1    | `lcdc`                 | LCD control register (0xFF40)               |
| +6     | 1    | `stat`                 | LCD status register (0xFF41)                |
| +7     | 1    | `scy`                  | Scroll Y (0xFF42)                           |
| +8     | 1    | `scx`                  | Scroll X (0xFF43)                           |
| +9     | 1    | `lyc`                  | LY compare (0xFF45)                         |
| +10    | 1    | `bgp`                  | BG palette (0xFF47)                         |
| +11    | 1    | `obp0`                 | OBJ palette 0 (0xFF48)                      |
| +12    | 1    | `obp1`                 | OBJ palette 1 (0xFF49)                      |
| +13    | 1    | `wy`                   | Window Y (0xFF4A)                           |
| +14    | 1    | `wx`                   | Window X (0xFF4B)                           |

### Memory regions (fixed, in order)

| Region          | Size    | Notes                                           |
|-----------------|---------|-------------------------------------------------|
| IO registers    | 128 B   | Full 0xFF00–0xFF7F range                       |
| IE register     | 1 B     | Interrupt enable (0xFFFF)                      |
| WRAM            | 8192 B  | Work RAM (0xC000–0xDFFF)                       |
| HRAM            | 127 B   | High RAM (0xFF80–0xFFFE)                       |
| VRAM            | 8192 B  | Video RAM (0x8000–0x9FFF)                      |
| OAM             | 160 B   | Object Attribute Memory (0xFE00–0xFE9F)        |

**Fixed payload total: ~16.5 KiB** (`MIN_BLOB_SIZE` = 16848 bytes)

### Variable sections (after OAM)

#### v2 format — tagged sections

Each section uses an 8-byte header followed by its payload:

```
[0..4]  tag     4-byte ASCII tag
[4..8]  length  payload length as u32 LE
[8..]   payload length bytes
```

Defined tags:

| Tag    | Description                                                             |
|--------|-------------------------------------------------------------------------|
| `MBC\0`| MBC register state (see [MBC payload](#mbc-payload))                   |
| `CRAM` | Cartridge external RAM contents                                        |

Sections with an empty payload (`length = 0`) are omitted entirely.
Unknown tags are silently ignored — forward-compatible.

#### v1 format — legacy tail (deprecated)

```
[0..N]  MBC state bytes  (4 bytes for MBC1; 18 bytes for MBC3+RTC; absent if no MBC)
[N..N+2] cart_ram_len    u16 LE — byte count of cart RAM that follows
[N+2..]  cart RAM data   cart_ram_len bytes (absent if cart_ram_len == 0)
```

### MBC payload

The MBC section payload encodes the MBC register state.  The layout depends
on the cartridge type but the loader always checks the payload length to
determine which fields are present.

#### MBC1 / MBC1Multicart (4 bytes)

| Offset | Size | Field          | Notes                                  |
|--------|------|----------------|----------------------------------------|
| 0      | 1    | `rom_bank_lo`  | Lower 5 bits of selected ROM bank (≥1)|
| 1      | 1    | `upper_bits`   | Upper 2 bits (bank set / RAM bank)    |
| 2      | 1    | `ram_mode`     | `0` = ROM banking, `1` = RAM banking  |
| 3      | 1    | `ram_enabled`  | `0` = disabled, `1` = enabled         |

#### MBC3 with RTC (18 bytes)

Starts with the same 4 bytes as MBC1, followed by 14 bytes of RTC state
(seconds, minutes, hours, DL, DH, latched values, and the halt flag).

## Cartridge RAM

The `CRAM` section (v2) or the cart RAM tail (v1) contains a verbatim dump
of the cartridge's external RAM.  Common sizes:

| Cartridge RAM | Size   |
|---------------|--------|
| None          | absent |
| 2 KiB (MBC2) | 2048 B |
| 8 KiB         | 8192 B |
| 32 KiB (MBC1)| 32768 B|

**Note:** When a save state is loaded, the cartridge RAM from the save state
replaces the battery save (`.sav`).  The boot loader skips reading the battery
save file if a valid save state is already present for the same ROM.

## Typical blob sizes

| Scenario                          | Approximate size |
|-----------------------------------|-----------------|
| No-MBC ROM, no cart RAM           | ~16.5 KiB       |
| MBC1 ROM, no cart RAM             | ~16.5 KiB       |
| MBC1 ROM, 8 KiB cart RAM         | ~24.6 KiB       |
| MBC1 ROM, 32 KiB cart RAM        | ~49.3 KiB       |
| MBC3 ROM + RTC, 8 KiB cart RAM   | ~24.6 KiB       |

## API

```rust
// Save — called by Sm83::save_state(), returns a Vec<u8>
let blob: Vec<u8> = gameboy.save_state();
// blob can be written directly to saves/<rom-id>/slot0.rbss

// Load — parse and validate before touching any emulator state
match SaveState::from_blob(blob) {
    Ok(ss) => gameboy.load_state(&ss),
    Err(msg) => { /* bad magic, too short, unsupported version */ }
}
```

`SaveState::from_blob` validates the magic and version and returns `Err` with
a human-readable message if anything is wrong, before modifying any emulator
state.  `Sm83::load_state` applies all component states atomically once
parsing succeeds.

## Adding new fields (extension guide)

1. Add a new v2 tag constant in `save_state.rs` (e.g. `SECTION_APU: &[u8; 4] = b"APU\0"`).
2. Emit the section in `serialize_v2` using `write_v2_section`.
3. Parse it in `parse_v2_sections` — unknown tags are already ignored by
   older firmware, so the format is forward-compatible by default.
4. Bump `VERSION` to `VERSION_V3` only if a **breaking** change is needed
   (e.g. a fixed-region size change or reordering).
