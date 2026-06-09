# WiFi Support — Captive Portal Configuration

## Context

The Pico 2W has an onboard CYW43439 WiFi/BT chip that is currently unused.
The user wants a first-class WiFi setup flow reachable from a new Settings screen
in the main menu. The core loop:

1. **Main Menu → SETTINGS → WIFI**
2. **Unconfigured**: device becomes an AP ("RustyBoy"), starts a captive portal at
   192.168.4.1, scans SSIDs and serves them in a mobile-friendly web page;
   user picks SSID + enters password and submits → credentials saved to flash → device reboots
3. **Configured**: shows SSID and a FORGET option; FORGET erases credentials and returns to Settings

WiFi auto-connect on boot is **out of scope** for this bead (no background STA
connection at startup; WiFi only activates when entering the WiFi menu).

---

## Flash Layout

Add one 4 KB sector for WiFi credentials immediately before the crash log:

```
[0x000000..0x080000]  firmware (512 KB)
[0x080000]            ROM metadata header (4 KB)
[0x081000..0x3FE000]  ROM data (unchanged start; capacity shrinks by 1 sector)
[0x3FE000]            WiFi config (4 KB) ← NEW
[0x3FF000]            crash log (4 KB)
```

**Sector format** (magic + SSID + password, total < 100 bytes):
```
[0..4]    magic b"WIFY"
[4..36]   SSID (32 bytes, null-padded)
[36..100] password (64 bytes, null-padded)
[100..4096] 0xFF (unused)
```

---

## New Dependencies (Cargo.toml, ARM section)

```toml
cyw43         = { git = "https://github.com/embassy-rs/embassy", features = ["defmt"] }
cyw43-pio     = { git = "https://github.com/embassy-rs/embassy", features = ["defmt"] }
embassy-net   = { git = "https://github.com/embassy-rs/embassy", features = [
    "defmt", "tcp", "udp", "dhcpv4", "medium-ethernet",
] }
picoserve     = { version = "0.15", features = ["defmt"] }
static_cell   = { version = "2" }
```

Add a `wifi` cargo feature (gates all WiFi code so non-WiFi builds remain unaffected).

---

## CYW43439 Pins (Pico 2W internal, not user-accessible)

| Signal  | GPIO  |
|---------|-------|
| WL_ON   | GP23  |
| WL_D    | GP24  |
| WL_CS   | GP25  |
| WL_CLK  | GP29  |

The CYW43 PIO driver uses **PIO1** (RP2350A has 3 PIOs; PIO0 is already taken by I2S audio).
Bind `PIO1_IRQ_0` in `bind_interrupts!`.

Firmware blobs come from `cyw43-firmware` crate (or downloaded to `cyw43-firmware/`):
```rust
const CYW43_FW: &[u8] = include_bytes!("../cyw43-firmware/43439A0.bin");
const CYW43_CLM: &[u8] = include_bytes!("../cyw43-firmware/43439A0_clm.bin");
```

---

## Files to Create

### `src/wifi/mod.rs`
Module root; re-exports `WifiConfig`, `WifiDriver`, portal task functions.

### `src/wifi/config.rs`
```rust
pub const WIFI_CONFIG_OFFSET: usize = FLASH_CAPACITY_BYTES - 2 * ERASE_SIZE; // 0x3FE000

pub struct WifiConfig {
    pub ssid: heapless::String<32>,
    pub password: heapless::String<64>,
}

impl WifiConfig {
    pub fn load(flash: &mut OnboardFlash) -> Option<Self>   // reads + validates magic
    pub fn save(flash: &mut OnboardFlash, ssid: &str, password: &str) -> Result<()>
    pub fn erase(flash: &mut OnboardFlash) -> Result<()>    // overwrites magic → 0x00
}
```
Pattern mirrors `crash::storage` (erase sector, then write).

### `src/wifi/driver.rs`
- `WifiDriver::init(spawner, p23, p24, p25, p29, pio1, dma_ch)` → `(NetDevice, Control, Runner)`
- `scan_ssids(control) -> heapless::Vec<heapless::String<32>, 16>` (scan in STA mode before starting AP)
- `start_ap(control, net_stack)` → configures AP SSID "RustyBoy", open auth, IP 192.168.4.1/24

### `src/wifi/portal.rs`
Four concurrent Embassy tasks spawned when entering portal mode:

1. **`cyw43_task`** — `runner.run()` loop (must always be running while WiFi is active)
2. **`net_task`** — `stack.run()` loop
3. **`dns_task`** — UDP/53 listener; returns 192.168.4.1 for every query (triggers captive portal detection on iOS/Android)
4. **`http_task`** — picoserve HTTP on port 80:
   - `GET /` → mobile-friendly HTML page with SSID `<select>` (pre-populated from scan) + manual input + password field
   - `POST /configure` → parses `ssid=...&password=...`, signals result via:
     ```rust
     static PORTAL_RESULT: Signal<CriticalSectionRawMutex, PortalCredentials> = Signal::new();
     ```

Credentials type:
```rust
pub struct PortalCredentials {
    pub ssid: heapless::String<32>,
    pub password: heapless::String<64>,
}
```

The UI task polls `PORTAL_RESULT.try_take()` each tick; on receipt: saves to flash → `cortex_m::peripheral::SCB::sys_reset()`.

**Captive portal HTML** (served inline as `&str`, built dynamically with SSID list):
- `<meta name="viewport" content="width=device-width, initial-scale=1">`
- `<select name="ssid">` with one `<option>` per scanned SSID + an "Other..." option
- `<input name="ssid_manual">` shown only when "Other..." selected (JS toggle, for hidden networks)
- `<input type="password" name="password">`
- Mobile-first CSS (large tap targets, single-column layout, ~2 KB total page size)

### `src/state/settings.rs`
```rust
pub struct SettingsState { menu: SettingsMenu }

impl SettingsState {
    pub async fn new(game_disp: &mut GameDisplay, app: &App) -> Self
    pub async fn tick(&mut self, app: &mut App, game_disp: &mut GameDisplay,
                      input: &mut InputHandler, flash: &mut OnboardFlash)
}
```
- On `ShowWifiMenu` → `app.transition_to(AppState::WifiMenu(WifiMenuState::new(...)))`
- On `Back` → `app.transition_to(AppState::MainMenu(...))`

### `src/state/wifi_menu.rs`
```rust
pub enum WifiMenuState {
    Configured(WifiMenuConfigured),
    Portal(WifiPortalScreen),
}
```

**Configured sub-state:**
- Menu items: `["SSID: <name>", "FORGET"]` (first item disabled/display-only, second selectable)
- On FORGET confirm → erase flash → show brief "Forgotten" info screen → transition to Settings
- On Back → transition to Settings

**Portal sub-state:**
- On enter: spawn CYW43/net/dns/http tasks; scan SSIDs first and store in `heapless::Vec`
- Display: custom info screen (not a standard scrollable menu):
  ```
  ┌──────────────────────┐
  │      WIFI SETUP      │
  ├──────────────────────┤
  │                      │
  │  1. Connect phone to │
  │     "RustyBoy"       │
  │                      │
  │  2. Open browser:    │
  │     192.168.4.1      │
  │                      │
  ├──────────────────────┤
  │       [B] Cancel     │
  └──────────────────────┘
  ```
- Each tick: poll `PORTAL_RESULT.try_take()`; on credentials received → save to flash → reboot
- On Back: transition to Settings (tasks remain running; acceptable — portal is harmless idle, full cleanup requires reboot)

---

## Files to Modify

### `src/flash_rom.rs`
```rust
// Add:
pub const WIFI_CONFIG_OFFSET: usize = FLASH_CAPACITY_BYTES - 2 * ERASE_SIZE; // 0x3FE000

// Change (shrink ROM capacity by one sector to make room for wifi config):
pub const ROM_DATA_CAPACITY_BYTES: usize =
    FLASH_CAPACITY_BYTES - ROM_DATA_OFFSET - 2 * ERASE_SIZE;
```

### `src/menu.rs`
- **New `MenuEffect` variants**: `ShowSettings`, `ShowWifiMenu`, `ForgetWifi`
- **`MainMenu`**: add `"SETTINGS"` to both `MAIN_ITEMS_FULL` and `MAIN_ITEMS_ROMS`; handle in `handle_main()` returning `ShowSettings`
- **New `SettingsMenu` struct** implementing `MenuLogic` (items: `["WIFI"]`; B → `Back`)

### `src/main.rs`
- Rename `_spawner` → `spawner`; store in `App` struct: `pub spawner: Spawner`
- Add `PIO1` to peripheral imports; bind `PIO1_IRQ_0 => PioIrqHandler<PIO1>`
- Add new `AppState` variants: `Settings(SettingsState)`, `WifiMenu(Box<WifiMenuState>)`
- Wire new states into the main loop `match` block; pass `spawner`, `onboard_flash`, and the WiFi peripheral tokens as needed

### `src/lib.rs`
```rust
#[cfg(feature = "wifi")]
pub mod wifi;
```

### `src/state/mod.rs`
Re-export `SettingsState`, `WifiMenuState`.

---

## Navigation Flow

```
MainMenu
  ├─ CONTINUE  → Running
  ├─ ROMS      → RomList
  └─ SETTINGS  → Settings
                   └─ WIFI → WifiMenu
                               ├─ [configured]   SSID shown + FORGET option
                               │                   FORGET → erase flash → Settings
                               │                   B      → Settings
                               └─ [unconfigured] Portal screen (AP + HTTP + DNS active)
                                                   form submit → save + reboot
                                                   B           → Settings
```

---

## Verification

1. **Unit tests** — add tests to `menu.rs` for:
   - `SettingsMenu` up/down navigation and B → `Back`
   - `MainMenu` down to SETTINGS → `ShowSettings`
2. **Flash layout invariant** — static assert or unit test: `WIFI_CONFIG_OFFSET + ERASE_SIZE == CRASH_LOG_OFFSET`
3. **Build check** — `cargo build --features wifi` from `platform/pico2w/`
4. **Manual end-to-end**:
   - Flash device; main menu shows SETTINGS
   - Navigate Settings → WIFI (no config) → portal screen appears; phone sees "RustyBoy" AP
   - Navigate to 192.168.4.1 on phone → page loads with SSID dropdown → fill in + submit
   - Device reboots; re-enter Settings → WIFI → shows SSID name + FORGET
   - FORGET → credentials erased → back to unconfigured portal screen on re-entry

---

## Open Decisions / Risks

| Topic | Decision |
|-------|----------|
| `picoserve` version compatibility | Verify against the git Embassy revision in use; fall back to a hand-written ~100-line minimal HTTP parser if the API diverges |
| SSID scan timing | Scan in STA mode *before* starting AP (CYW43439 cannot scan while in AP mode without complex firmware tricks); SSIDs captured once at portal entry and baked into the served HTML |
| Task cleanup on Cancel | Tasks keep running after "Cancel" — no clean Embassy task cancellation exists; acceptable because the portal is harmless while idle and a reboot fully cleans up |
| Auto-connect on boot | Out of scope; future bead (OTA, cloud saves, etc.) can add STA background task |
