//! rustyboy-pico2w — Bead 2: ILI9341 display driver + VINTENDO splash
//!
//! Adds the ILI9341 display over SPI0. On boot the display shows the
//! "VINTENDO" logo sliding in from the top (Game Boy boot animation style).
//!
//! # Blinky wiring (bare PCB development)
//! Connect an LED + 330Ω resistor between GP15 and GND.
//! The onboard LED on Pico 2W is routed through the CYW43 WiFi chip
//! and requires the cyw43 driver (added in Bead 6) — an external LED
//! on GP15 is used here.
//!
//! # Display wiring
//! | ILI9341 | Pico 2W |
//! |---------|---------|
//! | CLK     | GP10    |
//! | MOSI    | GP11    |
//! | CS      | GP9     |
//! | DC      | GP8     |
//! | RST     | GP12    |
//! | LED/BL  | GP13    |
//!
//! # Running
//! From platform/pico2w/:
//!   cargo run --release

#![no_std]
#![no_main]

mod display;

use defmt::info;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::watchdog::Watchdog;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

/// Firmware version, sourced from Cargo.toml.
const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

// Binary info block required by the RP2350 bootrom.
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

    // Watchdog: reboot if the main loop stalls for more than 5s.
    let mut watchdog = Watchdog::new(p.WATCHDOG);
    watchdog.start(Duration::from_millis(5_000));

    info!("rustyboy-pico2w v{} starting", FIRMWARE_VERSION);

    // External blinky LED on GP15.
    let mut led = Output::new(p.PIN_15, Level::Low);

    // Initialise ILI9341 display on SPI1 (GP10=CLK, GP11=MOSI).
    let mut disp = display::Display::new(
        p.SPI1, p.PIN_10, p.PIN_11, p.PIN_9, p.PIN_8, p.PIN_12, p.PIN_13,
    );

    // Splash animation — blocks until it finishes (~3 s total).
    disp.splash().await;

    info!("entering main loop");

    // TODO Bead 3: read buttons
    // TODO Bead 4: load ROM from SD card
    // TODO Bead 5: run core + I2S audio
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
