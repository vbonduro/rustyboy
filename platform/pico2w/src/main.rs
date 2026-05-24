#![no_std]
#![no_main]
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use core::future::Future;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use cortex_m_rt::ExceptionFrame;
#[cortex_m_rt::exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    defmt::error!(
        "HardFault: PC=0x{:08x} LR=0x{:08x} PSR=0x{:08x}",
        ef.pc(),
        ef.lr(),
        ef.xpsr()
    );
    loop {}
}

#[cfg(feature = "fps")]
mod perf;

mod state;
use state::{InGameMenuState, LoadingState, MainMenuState, RomListState, RunningState};

use embedded_alloc::Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

use defmt::info;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{
    CORE1, DMA_CH0, DMA_CH1, PIN_10, PIN_11, PIN_12, PIN_13, PIN_8, PIN_9, PIO0, SPI0, SPI1,
};
use embassy_rp::pio::{InterruptHandler as PioIrqHandler, Pio};
use embassy_rp::pio_programs::i2s::{PioI2sOut, PioI2sOutProgram};
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_rp::watchdog::Watchdog;
use embassy_rp::Peri;
use embassy_rp::{bind_interrupts, dma};
use embassy_time::{Delay, Duration};
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::SdCard;
use rustyboy_core::storage::RomId;
use {defmt_rtt as _, panic_probe as _};

use rustyboy_pico2w::audio::{AudioBuffers, SAMPLE_RATE};
use rustyboy_pico2w::display::hw::{GameDisplay, HwDisplay};
use rustyboy_pico2w::flash_rom::{new_onboard_flash, probe_staged_rom};
use rustyboy_pico2w::input::{ButtonState, InputHandler};
use rustyboy_pico2w::multicore::PicoGameBoy;
use rustyboy_pico2w::save_storage::SaveSlot;
use rustyboy_pico2w::sd::{self, DummyClock};
use rustyboy_pico2w::stack_probe;
use rustyboy_pico2w::xip_cartridge::XipCartridge;

#[cfg(feature = "oc-300")]
const TARGET_SYS_HZ: u32 = 300_000_000;
#[cfg(all(not(feature = "oc-300"), feature = "oc-280"))]
const TARGET_SYS_HZ: u32 = 280_000_000;
#[cfg(all(not(feature = "oc-300"), not(feature = "oc-280"), feature = "oc-266"))]
const TARGET_SYS_HZ: u32 = 266_000_000;
#[cfg(all(
    not(feature = "oc-300"),
    not(feature = "oc-280"),
    not(feature = "oc-266")
))]
const TARGET_SYS_HZ: u32 = 300_000_000;

#[cfg(feature = "oc-300")]
const TARGET_CORE_VOLTAGE: embassy_rp::clocks::CoreVoltage = embassy_rp::clocks::CoreVoltage::V1_30;
#[cfg(feature = "oc-280")]
const TARGET_CORE_VOLTAGE: embassy_rp::clocks::CoreVoltage = embassy_rp::clocks::CoreVoltage::V1_25;
#[cfg(all(not(feature = "oc-300"), not(feature = "oc-280")))]
const TARGET_CORE_VOLTAGE: embassy_rp::clocks::CoreVoltage = embassy_rp::clocks::CoreVoltage::V1_30;

const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const CYCLES_PER_FRAME: u64 = 70_224;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioIrqHandler<PIO0>;
    DMA_IRQ_0  => dma::InterruptHandler<DMA_CH0>, dma::InterruptHandler<DMA_CH1>;
});

#[unsafe(link_section = ".bi_entries")]
#[used]
pub static PICOTOOL_ENTRIES: [embassy_rp::binary_info::EntryAddr; 3] = [
    embassy_rp::binary_info::rp_program_name!(c"rustyboy-pico2w"),
    embassy_rp::binary_info::rp_cargo_version!(),
    embassy_rp::binary_info::rp_program_build_attribute!(),
];

// ---------------------------------------------------------------------------
// SD type aliases
// ---------------------------------------------------------------------------

type PicoSdCard =
    SdCard<ExclusiveDevice<Spi<'static, SPI0, Blocking>, Output<'static>, Delay>, Delay>;
pub(crate) type PicoSdMgr = sd::SdManager<PicoSdCard, DummyClock>;

// ---------------------------------------------------------------------------
// Minimal no-alloc waker for poll_once
// ---------------------------------------------------------------------------

unsafe fn noop_waker_clone(_: *const ()) -> RawWaker {
    RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)
}
unsafe fn noop_waker(_: *const ()) {}
static NOOP_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(noop_waker_clone, noop_waker, noop_waker, noop_waker);

pub(crate) fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> bool {
    let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    matches!(future.poll(&mut cx), Poll::Ready(_))
}

// ---------------------------------------------------------------------------
// App state machine
// ---------------------------------------------------------------------------

pub(crate) struct App {
    pub(crate) state: AppState,
    pub(crate) next_state: Option<AppState>,
    pub(crate) save_slot_available: bool,
    pub(crate) previous_buttons: ButtonState,
    pub(crate) staged_rom_name: Option<heapless::String<64>>,
    pub(crate) staged_rom_id: Option<RomId>,
    pub(crate) core1: Option<Peri<'static, CORE1>>,
}

impl App {
    pub(crate) fn transition_to(&mut self, next: AppState) {
        self.next_state = Some(next);
    }
}

pub(crate) fn load_battery_save(app: &App, sd_mgr: &PicoSdMgr, gameboy: &mut PicoGameBoy) {
    let Some(rom_id) = app.staged_rom_id else {
        return;
    };
    match sd_mgr.read_battery_save(&rom_id) {
        Ok(Some(data)) => gameboy.set_external_ram(&data),
        Ok(None) => {}
        Err(e) => defmt::warn!("battery load failed: {:?}", defmt::Debug2Format(&e)),
    }
}

pub(crate) fn flush_battery_save(app: &App, sd_mgr: &PicoSdMgr, gameboy: &PicoGameBoy) {
    let (Some(rom_id), Some(ram)) = (app.staged_rom_id, gameboy.external_ram()) else {
        return;
    };
    if let Err(e) = sd_mgr.write_battery_save(&rom_id, ram) {
        defmt::warn!("battery save failed: {:?}", defmt::Debug2Format(&e));
    }
}

pub(crate) fn refresh_save_slot_available(app: &mut App, sd_mgr: &PicoSdMgr) {
    let Some(rom_id) = app.staged_rom_id else {
        app.save_slot_available = false;
        return;
    };
    let slot = SaveSlot::new(0).expect("slot 0 is valid");
    app.save_slot_available = sd_mgr.save_state_exists(&rom_id, slot).unwrap_or(false);
}

pub(crate) fn boot_load_saves(rom_id: RomId, sd_mgr: &PicoSdMgr, gameboy: &mut PicoGameBoy) {
    match sd_mgr.read_battery_save(&rom_id) {
        Ok(Some(data)) => gameboy.set_external_ram(&data),
        Ok(None) => {}
        Err(e) => defmt::warn!("battery load failed: {:?}", defmt::Debug2Format(&e)),
    }
    let slot = SaveSlot::new(0).expect("slot 0 is valid");
    match sd_mgr.read_save_state(&rom_id, slot) {
        Ok(Some(blob)) => {
            match rustyboy_core::cpu::save_state::SaveState::from_blob(blob) {
                Ok(state) => {
                    let _ = gameboy.load_state(state);
                    info!("save state loaded on boot");
                }
                Err(msg) => defmt::warn!("boot save state parse failed: {}", msg),
            }
        }
        Ok(None) => {}
        Err(e) => defmt::warn!("boot save state read failed: {:?}", defmt::Debug2Format(&e)),
    }
}

pub(crate) enum AppState {
    Running(RunningState),
    InGameMenu(InGameMenuState),
    MainMenu(MainMenuState),
    RomList(Box<RomListState>),
    Loading(Box<LoadingState>),
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    {
        use core::mem::MaybeUninit;
        const HEAP_SIZE: usize = 128 * 1024;
        static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { HEAP.init(core::ptr::addr_of!(HEAP_MEM) as usize, HEAP_SIZE) }
    }

    let p = {
        use embassy_rp::clocks::ClockConfig;
        let mut clk =
            ClockConfig::system_freq(TARGET_SYS_HZ).expect("valid PLL params for target clock");
        clk.core_voltage = TARGET_CORE_VOLTAGE;
        embassy_rp::init(embassy_rp::config::Config::new(clk))
    };

    let mut watchdog = Watchdog::new(p.WATCHDOG);
    watchdog.start(Duration::from_millis(10_000));

    info!(
        "rustyboy-pico2w v{} starting @{}MHz",
        FIRMWARE_VERSION,
        TARGET_SYS_HZ / 1_000_000
    );

    // Pull GP20 low before display init so the SD module powers off while the
    // ILI9341 initialises (~265 ms). HwDisplay::new is synchronous, so by the
    // time it returns the 100 ms minimum off-time is satisfied. Setting GP20
    // high immediately after lets the 2.7 s splash cover the 250 ms power-up
    // window — no added delay visible to the user.
    let mut sd_pwr = Output::new(p.PIN_20, Level::Low);

    // GP8=DC  GP9=CS  GP10=CLK  GP11=MOSI  GP12=RST  GP13=BL
    let mut hw_disp = HwDisplay::new(
        p.SPI1, p.PIN_10, p.PIN_11, p.PIN_9, p.PIN_8, p.PIN_12, p.PIN_13,
    );
    sd_pwr.set_high(); // SD module now has ≥265 ms off-time behind it; splash provides the 250 ms power-up window
    stack_probe::paint();
    info!("starting splash");
    hw_disp.splash().await;
    drop(hw_disp);
    stack_probe::paint();

    // GP21=Up  GP22=Down  GP26=Left  GP27=Right
    // GP0=A    GP1=B      GP2=Start  GP3=Select
    let mut input = InputHandler::new(
        p.PIN_21, p.PIN_22, p.PIN_26, p.PIN_27, p.PIN_0, p.PIN_1, p.PIN_2, p.PIN_3,
    );

    // SD card — always initialised so the ROM list is available.
    let mut spi_cfg = spi::Config::default();
    spi_cfg.frequency = 400_000;
    let spi_bus = Spi::new_blocking(p.SPI0, p.PIN_6, p.PIN_7, p.PIN_4, spi_cfg);
    rp_pac::PADS_BANK0.gpio(4).modify(|w| w.set_pue(true));
    let spi_dev = ExclusiveDevice::new(spi_bus, Output::new(p.PIN_5, Level::High), Delay);
    let sdcard = SdCard::new(spi_dev, Delay);
    let mut sd_mgr = PicoSdMgr::new(sdcard, DummyClock);

    let mut onboard_flash = new_onboard_flash(p.FLASH);

    // Check whether a ROM is already staged in flash.
    let staged = probe_staged_rom(&mut onboard_flash);

    let _sd_mode = Output::new(p.PIN_17, Level::High);
    let Pio {
        mut common, sm0, ..
    } = Pio::new(p.PIO0, Irqs);
    let i2s_prog = PioI2sOutProgram::new(&mut common);
    let mut i2s = PioI2sOut::new(
        &mut common,
        sm0,
        p.DMA_CH0,
        Irqs,
        p.PIN_16,
        p.PIN_14,
        p.PIN_15,
        SAMPLE_RATE,
        16,
        &i2s_prog,
    );
    i2s.start();

    // SAFETY: hw_disp was dropped above; SPI1 and all display pins are free.
    let mut game_disp = unsafe {
        GameDisplay::new_after_splash(
            PIN_10::steal(),
            PIN_11::steal(),
            PIN_9::steal(),
            PIN_8::steal(),
            PIN_12::steal(),
            PIN_13::steal(),
            SPI1::steal(),
            p.DMA_CH1,
            Irqs,
        )
    };

    let mut audio_buffers = AudioBuffers::new();
    let mut audio_samples = Vec::with_capacity(2048);

    // Build initial state: Running if a ROM is staged, else MainMenu.
    let (initial_state, gameboy_init, staged_rom_name, staged_rom_id, core1_token) = match staged {
        Some((info, name)) => {
            info!(
                "staged ROM found in flash: {} banks ({} KiB)",
                info.bank_count,
                info.size_bytes / 1024
            );
            info!("building XipCartridge");
            match XipCartridge::from_staged_flash(info) {
                Ok(cart) => {
                    game_disp.draw_letterbox_bars().await;
                    info!(
                        "building GameBoy, clk={}",
                        embassy_rp::clocks::clk_sys_freq()
                    );
                    let mut gb = PicoGameBoy::with_cartridge(p.CORE1, Box::new(cart));
                    if let Some(rom_id) = info.rom_id {
                        boot_load_saves(rom_id, &sd_mgr, &mut gb);
                    }
                    info!("ROM loaded, entering main loop");
                    (
                        AppState::Running(RunningState),
                        Some(gb),
                        name,
                        info.rom_id,
                        None,
                    )
                }
                Err(e) => {
                    defmt::error!("flash ROM mapping failed: {:?}", defmt::Debug2Format(&e));
                    let stub_app = App {
                        state: AppState::Running(RunningState),
                        next_state: None,
                        save_slot_available: false,
                        previous_buttons: ButtonState::default(),
                        staged_rom_name: None,
                        staged_rom_id: None,
                        core1: None,
                    };
                    let next = MainMenuState::new(&mut game_disp, &stub_app).await;
                    (AppState::MainMenu(next), None, None, None, Some(p.CORE1))
                }
            }
        }
        None => {
            info!("no staged ROM in flash; showing main menu");
            let stub_app = App {
                state: AppState::Running(RunningState),
                next_state: None,
                save_slot_available: false,
                previous_buttons: ButtonState::default(),
                staged_rom_name: None,
                staged_rom_id: None,
                core1: None,
            };
            let menu_state = MainMenuState::new(&mut game_disp, &stub_app).await;
            (
                AppState::MainMenu(menu_state),
                None,
                None,
                None,
                Some(p.CORE1),
            )
        }
    };

    stack_probe::paint();
    info!("entering main loop");

    let mut gameboy: Option<PicoGameBoy> = gameboy_init;

    let mut app = App {
        state: initial_state,
        next_state: None,
        save_slot_available: false,
        previous_buttons: ButtonState::default(),
        staged_rom_name,
        staged_rom_id,
        core1: core1_token,
    };
    refresh_save_slot_available(&mut app, &sd_mgr);

    #[cfg(feature = "fps")]
    let mut tracker = perf::PerfTracker::new();

    loop {
        stack_probe::check_current_sp("main loop");
        watchdog.feed(Duration::from_millis(5_000));

        // Take the current state out so we can pass &mut app into tick().
        let mut state = core::mem::replace(&mut app.state, AppState::Running(RunningState));

        match &mut state {
            AppState::Running(running) => {
                running
                    .tick(
                        &mut app,
                        gameboy.as_mut().expect("Running without GameBoy"),
                        &mut game_disp,
                        &mut i2s,
                        &mut input,
                        &mut sd_mgr,
                        &mut audio_buffers,
                        &mut audio_samples,
                    )
                    .await;

                #[cfg(feature = "fps")]
                tracker.tick();
            }

            AppState::InGameMenu(menu_state) => {
                menu_state
                    .tick(
                        &mut app,
                        gameboy.as_mut().expect("InGameMenu without GameBoy"),
                        &mut game_disp,
                        &mut input,
                        &mut sd_mgr,
                    )
                    .await;
            }

            AppState::MainMenu(menu_state) => {
                menu_state
                    .tick(&mut app, &mut game_disp, &mut input, &mut sd_mgr)
                    .await;
            }

            AppState::RomList(rom_list_state) => {
                rom_list_state
                    .tick(&mut app, &mut game_disp, &mut input, &mut sd_mgr)
                    .await;
            }

            AppState::Loading(loading_state) => {
                loading_state
                    .tick(
                        &mut app,
                        &mut gameboy,
                        &mut onboard_flash,
                        &mut sd_mgr,
                        &mut game_disp,
                        &mut watchdog,
                    )
                    .await;
            }
        }

        // Restore state or apply a queued transition.
        app.state = app.next_state.take().unwrap_or(state);
    }
}
