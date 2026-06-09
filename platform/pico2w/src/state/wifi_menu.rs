//! WiFi menu state: either a "configured" sub-state (SSID + FORGET) or a
//! captive-portal sub-state (AP running, phone connects and submits form).

use defmt::info;
use embassy_time::{Duration, Timer};
use rustyboy_pico2w::display::hw::GameDisplay;
use rustyboy_pico2w::flash_rom::OnboardFlash;
use rustyboy_pico2w::input::InputHandler;
use rustyboy_pico2w::menu::{MenuFrame, MenuInput};
use rustyboy_pico2w::wifi::config::WifiConfig;
use rustyboy_pico2w::wifi::portal::PORTAL_RESULT;

use super::settings::SettingsState;
use crate::{App, AppState};

// ---------------------------------------------------------------------------
// Top-level enum
// ---------------------------------------------------------------------------

pub enum WifiMenuState {
    Configured(WifiMenuConfigured),
    Portal(WifiPortalScreen),
}

impl WifiMenuState {
    /// Determine whether credentials are present and pick the appropriate
    /// sub-state.  Draws the initial screen before returning.
    pub async fn new(
        game_disp: &mut GameDisplay<'static>,
        app: &App,
        flash: &mut OnboardFlash<'_>,
    ) -> Self {
        match WifiConfig::load(flash) {
            Some(cfg) => {
                let state = WifiMenuConfigured::new(game_disp, app, cfg.ssid).await;
                Self::Configured(state)
            }
            None => {
                let state = WifiPortalScreen::new(game_disp, app, flash).await;
                Self::Portal(state)
            }
        }
    }

    pub async fn tick(
        &mut self,
        app: &mut App,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
        flash: &mut OnboardFlash<'_>,
    ) {
        match self {
            Self::Configured(s) => s.tick(app, game_disp, input, flash).await,
            Self::Portal(s) => s.tick(app, game_disp, input, flash).await,
        }
    }
}

// ---------------------------------------------------------------------------
// Configured sub-state
// ---------------------------------------------------------------------------

pub struct WifiMenuConfigured {
    ssid: heapless::String<32>,
    selected: usize,
}

impl WifiMenuConfigured {
    async fn new(
        game_disp: &mut GameDisplay<'static>,
        app: &App,
        ssid: heapless::String<32>,
    ) -> Self {
        let state = Self { ssid, selected: 0 };
        state.draw(game_disp, app).await;
        state
    }

    async fn draw(&self, game_disp: &mut GameDisplay<'static>, app: &App) {
        // Build the SSID display string "SSID: <name>".  The menu item area only
        // fits 13 chars (CHAR_W 8 * SCALE 2 = 16 px / glyph); "SSID: " eats 6, so
        // the name is truncated to 7 chars (with a trailing '.' when cut) to keep
        // the whole line on screen instead of being clipped mid-glyph.
        let mut ssid_label = heapless::String::<40>::new();
        let _ = ssid_label.push_str("SSID: ");
        const SSID_MAX: usize = 7;
        let name = self.ssid.as_str();
        let cut = name
            .char_indices()
            .nth(SSID_MAX)
            .map(|(i, _)| i)
            .unwrap_or(name.len());
        let _ = ssid_label.push_str(&name[..cut]);
        if cut < name.len() {
            let _ = ssid_label.push('.');
        }

        let items: [&str; 2] = [ssid_label.as_str(), "FORGET"];
        let enabled: [bool; 2] = [false, true]; // first item is display-only
        let frame = MenuFrame {
            title: "WIFI",
            items: &items,
            selected: self.selected,
            marquee_frame: 0,
            enabled: &enabled,
            marked: None,
            crash_pending: app.crash_pending,
        };
        game_disp.draw_menu(&frame).await;
    }

    async fn tick(
        &mut self,
        app: &mut App,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
        flash: &mut OnboardFlash<'_>,
    ) {
        Timer::after(Duration::from_millis(33)).await;

        let (current_buttons, _) = input.poll();
        let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
        app.previous_buttons = current_buttons;

        if !menu_input.any() {
            return;
        }

        if menu_input.back {
            let next = SettingsState::new(game_disp, app).await;
            app.transition_to(AppState::Settings(next));
            return;
        }

        if menu_input.up && self.selected > 0 {
            self.selected -= 1;
            self.draw(game_disp, app).await;
            return;
        }
        if menu_input.down && self.selected < 1 {
            self.selected += 1;
            self.draw(game_disp, app).await;
            return;
        }

        if menu_input.confirm && self.selected == 1 {
            // FORGET: erase credentials and return to Settings (portal on re-entry).
            match WifiConfig::erase(flash) {
                Ok(()) => info!("wifi: credentials erased"),
                Err(e) => defmt::error!("wifi: erase failed: {:?}", e),
            }
            let next = SettingsState::new(game_disp, app).await;
            app.transition_to(AppState::Settings(next));
        }
    }
}

// ---------------------------------------------------------------------------
// Portal sub-state
// ---------------------------------------------------------------------------

pub struct WifiPortalScreen {
    /// Whether the portal tasks have been spawned.
    tasks_started: bool,
}

impl WifiPortalScreen {
    async fn new(
        game_disp: &mut GameDisplay<'static>,
        app: &App,
        flash: &mut OnboardFlash<'_>,
    ) -> Self {
        let _ = flash; // unused until portal tasks are spawned
        let state = Self {
            tasks_started: false,
        };
        state.draw(game_disp, app).await;
        state
    }

    async fn draw(&self, game_disp: &mut GameDisplay<'static>, app: &App) {
        // Custom info screen drawn as a menu with non-selectable text items.
        // The menu text window is only 13 chars wide (CHAR_W 8 * SCALE 2 = 16 px
        // per glyph across the 208 px item area), so every line must stay <= 13
        // chars or render_menu_row truncates it — which previously clipped the
        // instructions and dropped the last digit of the IP address.
        let items: [&str; 6] = [
            "",
            "Join WiFi:",
            " RustyBoy",
            "Then visit:",
            " 192.168.4.1",
            "",
        ];
        let enabled = [false; 6];
        let frame = MenuFrame {
            title: "WIFI SETUP",
            items: &items,
            selected: usize::MAX, // no highlight
            marquee_frame: 0,
            enabled: &enabled,
            marked: None,
            crash_pending: app.crash_pending,
        };
        game_disp.draw_menu(&frame).await;
    }

    async fn tick(
        &mut self,
        app: &mut App,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
        flash: &mut OnboardFlash<'_>,
    ) {
        // Spawn portal tasks on the first tick so we have the spawner available
        // from App.
        if !self.tasks_started {
            self.tasks_started = true;
            self.start_portal(app).await;
        }

        Timer::after(Duration::from_millis(100)).await;

        let (current_buttons, _) = input.poll();
        let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
        app.previous_buttons = current_buttons;

        // Poll for credentials from the portal task.
        if let Some(creds) = PORTAL_RESULT.try_take() {
            info!("portal: saving credentials for '{}'", creds.ssid.as_str());
            match WifiConfig::save(flash, creds.ssid.as_str(), creds.password.as_str()) {
                Ok(()) => {
                    info!("portal: credentials saved, rebooting");
                    cortex_m::peripheral::SCB::sys_reset();
                }
                Err(e) => {
                    defmt::error!("portal: flash save failed: {:?}", e);
                    // Fall through — user sees portal screen again.
                }
            }
        }

        if menu_input.back {
            // Cancel portal — tasks keep running (harmless idle; reboot cleans up).
            let next = SettingsState::new(game_disp, app).await;
            app.transition_to(AppState::Settings(next));
        }
    }

    async fn start_portal(&self, app: &mut App) {
        use rustyboy_pico2w::wifi::{driver, portal};

        let spawner = app.spawner;

        // Take WiFi peripherals from the App.
        let wifi_periphs = match app.wifi_periphs.take() {
            Some(p) => p,
            None => {
                defmt::error!("portal: WiFi peripherals already consumed");
                return;
            }
        };

        let (stack, mut control, cyw43_runner, net_runner) = driver::init(
            wifi_periphs.pwr,
            wifi_periphs.dio,
            wifi_periphs.cs,
            wifi_periphs.clk,
            wifi_periphs.pio1,
            wifi_periphs.dma_ch2,
            wifi_periphs.dma_ch3,
            wifi_periphs.pio_irqs,
            wifi_periphs.dma_irqs,
        )
        .await;

        // Spawn the cyw43 runner FIRST — every control ioctl below (CLM download,
        // scan, AP start) is serviced by it; without it they block forever.
        portal::spawn_cyw43_runner(&spawner, cyw43_runner);

        // Download CLM + set power management (needs the runner running).
        driver::configure(&mut control).await;

        // Scan SSIDs before switching to AP mode.
        let ssids = driver::scan_ssids(&mut control).await;

        // Switch to AP mode.
        driver::start_ap(&mut control).await;

        // Spawn net + DNS + HTTP tasks now that the AP link is up.
        portal::spawn_net_tasks(&spawner, net_runner, stack, ssids);
    }
}
