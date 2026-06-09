# Save, State, and Sync Plan

This document plans Pico2W battery saves, persistent save states, and eventual
sync with the web platform. The central rule is: emulator payloads should stay
platform-neutral, while platform-specific and sync metadata lives beside them.

## Goals

- Persist cartridge battery saves on Pico2W.
- Persist save states on Pico2W.
- Keep the web platform able to load old save states.
- Make Pico2W saves compatible with future web sync.
- Avoid performance regressions in the frame loop.
- Keep Pico2W local storage single-user.

## Non-Goals

- Multi-user support on Pico2W.
- Network sync in the first persistence pass.
- SD-card DMA as the first optimization.
- A new save-state format that breaks existing web saves.

## Current State

The shared core already supports:

- Full save-state blobs through `GameBoy::save_state()`.
- Loading blobs through `SaveState::from_blob(...)`.
- Cartridge external RAM through `external_ram()` and `set_external_ram(...)`.

The web platform currently stores:

- Save states as raw `RBSS` blobs in SQLite.
- Battery saves as raw cartridge RAM bytes in SQLite.
- Records keyed by user and ROM name.

The Pico2W platform currently has:

- An in-memory save-state slot in the in-game menu.
- Staged ROMs in onboard flash via `XipCartridge`.
- An SD manager that currently focuses on ROM listing and ROM reading.
- A single local user/device model.

## Compatibility Strategy

Battery saves remain raw cartridge external RAM bytes. This is compatible with:

- Web WASM APIs.
- Pico2W storage.
- Common `.sav` style tooling.

Save states remain raw `RBSS` blobs as emulator payloads. Compatibility comes
from versioned parsing in `rustyboy-core`:

- `RBSS v1` must remain readable.
- `RBSS v2` can be introduced for improved cartridge metadata.
- Writers should continue emitting v1 until all active platforms can read v2.
- After that, writers can switch to v2.

Old web saves are therefore preserved by updating `SaveState::from_blob(...)`
to parse both versions before changing any writer.

## ROM Identity

Do not use filenames as the durable sync identity.

Use a stable `rom_id`, preferably SHA-256 of the full ROM bytes. Display names
and filenames are metadata only.

Reasons:

- Users can rename ROMs.
- Web and Pico may use different short/long filenames.
- Two files can share a display name but contain different ROM revisions.
- Sync must reject loading a save state into the wrong ROM.

On Pico2W, compute `rom_id` while staging the ROM from SD to flash, then store
it in staged ROM metadata. On web, compute the same `rom_id` after fetching ROM
bytes.

## RBSS v2 Direction

`RBSS v1` has fixed assumptions around MBC data and uses a `u16` cart RAM
length. That is tight for 64 KiB and 128 KiB RAM cartridges and awkward for MBC3
RTC state.

`RBSS v2` should use length-tagged sections for cartridge data:

- Magic: `RBSS`
- Version: `2`
- Fixed CPU/timer/PPU/internal memory sections, or tagged sections for all
  components if the migration is small enough.
- MBC payload length: `u32`
- MBC payload bytes.
- Cartridge RAM length: `u32`
- Cartridge RAM bytes.
- Optional future sections can be skipped by length.

Core must parse:

- v1 existing layout.
- v2 tagged/length-aware layout.

Core should expose enough metadata for callers to verify:

- Save-state format version.
- Expected ROM identity, once the outer metadata layer exists.

## Pico2W Local Storage Model

Pico2W local storage is single-user. There is no `user_id` on disk.

Suggested SD layout:

```text
/SAVES/
  INDEX.DAT
  <ROMID8>/
    META.DAT
    BATT.SAV
    SLOT0.RBS
    SLOT1.RBS
    SLOT2.RBS
```

`<ROMID8>` is a FAT-safe short directory name derived from the full `rom_id`.
`INDEX.DAT` maps short directory names to full ROM IDs and display names.

Battery payload:

- `BATT.SAV`
- Raw cartridge RAM bytes.

Save-state payload:

- `SLOTn.RBS`
- Raw `RBSS` blob.

Metadata:

- Full `rom_id`.
- Display name.
- Last local update timestamp if available.
- Local dirty flag.
- Last synced remote revision, once sync exists.
- Payload hash for conflict detection.

## Web Storage Model

The web server can remain multi-user, but it should eventually migrate from
ROM-name-only save keys to ROM identity keys.

Target server fields:

- `user_id`
- `rom_id`
- `rom_name`
- `slot_name`
- `created_at`
- `updated_at`
- `payload_hash`
- `data`

Battery saves remain one latest record per `user_id + rom_id`.

Save states remain multiple records per `user_id + rom_id`, pruned to a fixed
count.

## Launch Semantics

Use the same behavior on web and Pico2W:

1. Start selected ROM.
2. If a save state should be resumed, load that state.
3. Otherwise, load `BATT.SAV` if present.
4. Start emulation.

Manual reset/fresh start should skip save-state loading but still load battery
RAM, matching normal cartridge behavior.

## Pico2W Runtime Behavior

Battery saves:

- Load once after constructing the emulator.
- Mark dirty on external RAM writes.
- Flush on pause menu entry, save-state creation, quit, ROM switch, and shutdown
  paths where possible.
- Optional autosave: debounce dirty RAM and save outside the hot frame loop.

Save states:

- Save from the pause menu by serializing `gameboy.save_state()`.
- Load from the pause menu by reading an RBSS blob and parsing via
  `SaveState::from_blob(...)`.
- Reject loading if metadata says the ROM ID does not match.

Performance:

- Do not write SD data during the frame loop.
- First SD optimization should be raising SPI0 frequency after card init.
- Consider SD DMA only after measuring real save/load time.

## SD Performance Plan

Initial path:

1. Initialize SD at 400 kHz.
2. Trigger card init.
3. Raise SPI0 to a higher stable frequency, for example 12-24 MHz.
4. Use blocking SD reads/writes from menu/loading states.

DMA is a later phase because:

- `embedded-sdmmc` is synchronous.
- Embassy SPI DMA is async.
- Audio and display already use DMA channels.
- Save files are relatively small compared with ROM staging and frame output.

## Sync Model

Pico2W is single-user locally. Sync auth maps the device to one remote web
account.

Battery save sync:

- One record per `user_id + rom_id`.
- Last-write-wins only when revisions or timestamps prove the winner.
- If both local and remote changed from the same base, prefer explicit conflict
  handling over silent overwrite.

Save-state sync:

- Treat save states as append-style slots.
- Conflicting slots become separate records.
- Prune oldest records after successful sync.

Sync metadata should include:

- Device ID.
- Full `rom_id`.
- Payload kind: battery or state.
- Slot name for save states.
- Updated timestamp.
- Payload hash.
- Remote revision or ETag.

## Implementation Phases

### Phase 1: Shared Core Compatibility

- Update `SaveState::from_blob(...)` to parse v1 and v2.
- Add tests proving old v1 blobs still load.
- Add tests for larger cartridge RAM lengths.
- Normalize cartridge MBC state payload handling between normal cartridges and
  `XipCartridge`.
- Keep writers emitting v1 until web and Pico readers are deployed.

### Phase 2: ROM Identity

- Add a shared ROM hashing helper if practical.
- Compute `rom_id` on web after ROM fetch.
- Compute `rom_id` on Pico while staging ROMs.
- Store `rom_id` in Pico staged-ROM metadata.
- Add server migration fields for `rom_id` while preserving `rom_name`.

### Phase 3: Pico2W SD Persistence

- Extend `SdManager` with directory creation and file read/write helpers.
- Add Pico save metadata/index support.
- Expose `external_ram()` and `set_external_ram(...)` through `PicoGameBoy`.
- Load battery saves on ROM start.
- Save battery RAM from safe menu/loading points.

### Phase 4: Pico2W Persistent Save States

- Replace the in-memory-only save slot with SD-backed slots.
- Add save/load menu state for local slots.
- Validate ROM ID before loading.
- Keep an in-memory fast save optional only if useful.

### Phase 5: Switch Writers To RBSS v2

- Once web and Pico readers both support v2, switch core save-state writing to
  v2.
- Keep v1 parser indefinitely.
- Keep web server serving old blobs unchanged.

### Phase 6: Sync

- Add server API endpoints using `rom_id`.
- Add conflict/revision metadata.
- Add Pico sync client once WiFi/auth flow is ready.
- Sync battery saves and save-state slots independently.

## Test Plan

Core tests:

- v1 save-state blobs still parse.
- v2 save-state blobs parse.
- v1 and v2 round-trip CPU, timer, PPU, WRAM, HRAM, VRAM, OAM.
- MBC1, MBC3, and MBC5 bank state round-trips.
- 64 KiB and 128 KiB cartridge RAM round-trips.
- Invalid magic/version/length cases fail safely.

Web tests:

- Existing v1 server blobs still load in the browser.
- New saves include `rom_id` metadata once migrated.
- Battery saves remain raw bytes.
- Save list and latest-save behavior still work after schema migration.

Pico2W tests:

- Host-test filename/path/index helpers.
- Battery save load/save using an SD test double if possible.
- Save-state slot metadata validation.
- Wrong-ROM save-state rejection.
- Menu state transitions for save, load, quit, and reset.

Hardware checks:

- Measure SD write time for 8 KiB, 32 KiB, 64 KiB, and 128 KiB battery saves.
- Measure save-state write/load time.
- Confirm no frame-loop SD writes.
- Confirm watchdog feeding during longer SD operations.

## Open Decisions

- Exact RBSS v2 section layout.
- Exact metadata binary format for `META.DAT` and `INDEX.DAT`.
- Number of local save-state slots on Pico2W.
- Whether Pico should auto-resume latest save state or only expose manual load.
- Conflict UI for sync.
- Final SD SPI frequency after hardware measurement.

