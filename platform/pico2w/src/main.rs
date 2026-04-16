//! rustyboy-pico2w — Bead 1: scaffold + blinky
//!
//! Proves the build system, embassy async runtime, defmt RTT logging,
//! and watchdog are all wired up correctly on the RP2350.
//!
//! # Blinky wiring (bare PCB development)
//! Connect an LED + 330Ω resistor between GP15 and GND.
//! The onboard LED on Pico 2W is routed through the CYW43 WiFi chip
//! and requires the cyw43 driver (added in Bead 6) — an external LED
//! on GP15 is used here to keep the scaffold dependency-free.
//!
//! # Running
//! From platform/pico2w/:
//!   cargo run --release
//!
//! Requires probe-rs and a SWD debug probe (e.g. Raspberry Pi Debug Probe).
//! See CLAUDE.md for picotool / BOOTSEL flashing instructions.

#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::watchdog::Watchdog;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

/// Firmware version, sourced from Cargo.toml.
/// Will be embedded at a known symbol in Bead 8 for OTA version comparison.
const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

// Binary info block required by the RP2350 bootrom to identify and boot the image.
// Also surfaced by `picotool info` for firmware identification.
#[unsafe(link_section = ".bi_entries")]
#[used]
pub static PICOTOOL_ENTRIES: [embassy_rp::binary_info::EntryAddr; 3] = [
    embassy_rp::binary_info::rp_program_name!(c"rustyboy-pico2w"),
    embassy_rp::binary_info::rp_cargo_version!(),
    embassy_rp::binary_info::rp_program_build_attribute!(),
];

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // Watchdog: reboot the device if the main loop stalls for more than 5s.
    // Every iteration of the loop must call watchdog.feed() within this window.
    let mut watchdog = Watchdog::new(p.WATCHDOG);
    watchdog.start(Duration::from_millis(5_000));

    info!("rustyboy-pico2w v{} starting", FIRMWARE_VERSION);

    // External LED on GP15 (see module doc for wiring).
    let mut led = Output::new(p.PIN_15, Level::Low);

    let mut tick: u32 = 0;
    loop {
        led.set_high();
        Timer::after(Duration::from_millis(500)).await;
        led.set_low();
        Timer::after(Duration::from_millis(500)).await;

        watchdog.feed(Duration::from_millis(5_000));
        tick += 1;
        info!("heartbeat tick={}", tick);
    }
}
