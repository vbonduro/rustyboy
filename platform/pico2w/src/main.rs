#![no_std]
#![no_main]
extern crate alloc;

use embedded_alloc::Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::spi::{self, Spi};
use embassy_rp::watchdog::Watchdog;
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{SdCard, VolumeManager};
use {defmt_rtt as _, panic_probe as _};

use rustyboy_core::cpu::peripheral::joypad::Button;
use rustyboy_core::memory::{GameBoyMemory, StreamingCartridge};
use rustyboy_pico2w::display::hw::HwDisplay;
use rustyboy_pico2w::input::{ButtonState, InputHandler};
use rustyboy_pico2w::sd::{DummyClock, SdRomReader};

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
        unsafe { HEAP.init(core::ptr::addr_of!(HEAP_MEM) as usize, HEAP_SIZE) }
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

    // SD card: power-cycle via GP20, then init SPI0 on GP4-GP7.
    let mut sd_power = Output::new(p.PIN_20, Level::Low);
    Timer::after(Duration::from_millis(200)).await;
    sd_power.set_high();
    Timer::after(Duration::from_millis(250)).await;

    let mut spi_cfg = spi::Config::default();
    spi_cfg.frequency = 400_000;
    let spi_bus = Spi::new_blocking(p.SPI0, p.PIN_6, p.PIN_7, p.PIN_4, spi_cfg);
    // SD card MISO (GP4) is open-collector — enable the internal pull-up.
    rp_pac::PADS_BANK0.gpio(4).modify(|w| w.set_pue(true));
    let spi_dev = ExclusiveDevice::new(spi_bus, Output::new(p.PIN_5, Level::High), Delay);
    let sdcard  = SdCard::new(spi_dev, Delay);
    let mgr     = VolumeManager::new(sdcard, DummyClock);

    let reader = match SdRomReader::new(mgr) {
        Ok(r) => r,
        Err(e) => {
            error!("SD init failed: {:?}", defmt::Debug2Format(&e));
            loop { Timer::after(Duration::from_millis(2_000)).await; }
        }
    };
    let cart = match StreamingCartridge::new(reader) {
        Ok(c) => c,
        Err(e) => {
            error!("ROM load failed: {:?}", defmt::Debug2Format(&e));
            loop { Timer::after(Duration::from_millis(2_000)).await; }
        }
    };
    let _memory = GameBoyMemory::with_cartridge(alloc::boxed::Box::new(cart));
    info!("ROM loaded, entering main loop");

    // TODO: run core + I2S audio (Bead 5)

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
