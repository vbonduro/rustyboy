//! rustyboy-pico2w — Bead 3: input handler + debounce
//!
//! Adds 8-button GPIO input with 10 ms software debounce and a
//! Start+Select-hold-1s menu combo.  Button events are logged over RTT for
//! smoke-testing; core integration follows in Bead 5.
//!
//! # Display wiring  (unchanged from Bead 2)
//! GP8=DC  GP9=CS  GP10=CLK  GP11=MOSI  GP12=RST  GP13=BL
//!
//! # Button wiring  (active-low, pull-up enabled)
//! GP21=Up  GP22=Down  GP26=Left  GP27=Right
//! GP0=A    GP1=B      GP2=Start  GP3=Select
//! One side of each button to GPIO, other side to GND — no resistors needed.
//!
//! # Running
//! From platform/pico2w/:   cargo run --release

#![no_std]
#![no_main]

mod display;
mod input;

use embedded_alloc::Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::watchdog::Watchdog;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

use input::ButtonState;
use rustyboy_core::cpu::peripheral::joypad::Button;

const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[unsafe(link_section = ".bi_entries")]
#[used]
pub static PICOTOOL_ENTRIES: [embassy_rp::binary_info::EntryAddr; 3] = [
    embassy_rp::binary_info::rp_program_name!(c"rustyboy-pico2w"),
    embassy_rp::binary_info::rp_cargo_version!(),
    embassy_rp::binary_info::rp_program_build_attribute!(),
];

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    {
        use core::mem::MaybeUninit;
        const HEAP_SIZE: usize = 32 * 1024;
        static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { HEAP.init(HEAP_MEM.as_ptr() as usize, HEAP_SIZE) }
    }

    let p = embassy_rp::init(Default::default());

    let mut watchdog = Watchdog::new(p.WATCHDOG);
    watchdog.start(Duration::from_millis(5_000));

    info!("rustyboy-pico2w v{} starting", FIRMWARE_VERSION);

    let mut led = Output::new(p.PIN_15, Level::Low);

    // Display — splash then idle.
    let mut disp = display::Display::new(
        p.SPI1, p.PIN_10, p.PIN_11, p.PIN_9, p.PIN_8, p.PIN_12, p.PIN_13,
    );
    disp.splash().await;

    // Input handler — all 8 buttons with debounce.
    let mut input = input::InputHandler::new(
        p.PIN_21, p.PIN_22, p.PIN_26, p.PIN_27,
        p.PIN_0,  p.PIN_1,  p.PIN_2,  p.PIN_3,
    );

    info!("entering main loop");

    // TODO Bead 4: load ROM from SD card
    // TODO Bead 5: run core + I2S audio

    let mut prev_state = ButtonState::default();
    let mut tick: u32 = 0;

    loop {
        // ~60 Hz tick
        Timer::after(Duration::from_millis(16)).await;
        watchdog.feed(Duration::from_millis(5_000));

        let (state, menu) = input.poll();

        // Log any button state changes.
        for (btn, pressed) in prev_state.diff(state) {
            if pressed {
                info!("btn press:   {}", btn_name(btn));
            } else {
                info!("btn release: {}", btn_name(btn));
            }
        }
        prev_state = state;

        if menu {
            warn!("menu combo triggered");
        }

        // Blink LED at 1 Hz (every 62 × 16 ms ticks ≈ 1 s).
        tick += 1;
        if tick % 62 == 0 {
            led.toggle();
        }
    }
}

/// defmt-friendly button name (defmt doesn't derive for external types).
fn btn_name(b: Button) -> &'static str {
    match b {
        Button::Up     => "Up",
        Button::Down   => "Down",
        Button::Left   => "Left",
        Button::Right  => "Right",
        Button::A      => "A",
        Button::B      => "B",
        Button::Start  => "Start",
        Button::Select => "Select",
    }
}
