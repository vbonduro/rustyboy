#![no_std]
#![no_main]

use embedded_alloc::Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::watchdog::Watchdog;
use embassy_time::{Duration, Timer};
use {defmt_rtt as _, panic_probe as _};

use rustyboy_core::cpu::peripheral::joypad::Button;
use rustyboy_pico2w::display::hw::HwDisplay;
use rustyboy_pico2w::input::{ButtonState, InputHandler};

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

    // GP8=DC  GP9=CS  GP10=CLK  GP11=MOSI  GP12=RST  GP13=BL
    let mut hw_disp = HwDisplay::new(
        p.SPI1, p.PIN_10, p.PIN_11, p.PIN_9, p.PIN_8, p.PIN_12, p.PIN_13,
    );
    hw_disp.splash().await;

    // GP21=Up  GP22=Down  GP26=Left  GP27=Right
    // GP0=A    GP1=B      GP2=Start  GP3=Select
    let mut input = InputHandler::new(
        p.PIN_21, p.PIN_22, p.PIN_26, p.PIN_27,
        p.PIN_0,  p.PIN_1,  p.PIN_2,  p.PIN_3,
    );

    info!("entering main loop");

    // TODO: load ROM from SD card
    // TODO: run core + I2S audio

    let mut prev_state = ButtonState::default();
    let mut tick: u32 = 0;

    loop {
        Timer::after(Duration::from_millis(16)).await;
        watchdog.feed(Duration::from_millis(5_000));

        let (state, menu) = input.poll();

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

        tick += 1;
        if tick % 62 == 0 {
            led.toggle();
        }
    }
}

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
