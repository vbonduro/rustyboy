#![no_std]
#![no_main]
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use core::future::Future;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

// HardFault and #[panic_handler] are provided by crash::handler.

#[cfg(feature = "fps")]
mod perf;

mod state;
use state::WifiMenuState;
use state::{
    InGameMenuState, LoadingState, MainMenuState, RomListState, RunningState, SettingsState,
};

// Global allocator: the bare `embedded_alloc::Heap`.
use embedded_alloc::Heap;
#[global_allocator]
static HEAP: Heap = Heap::empty();

use defmt::info;
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{
    CORE1, DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, PIN_10, PIN_11, PIN_12, PIN_13, PIN_8, PIN_9, PIO0,
    SPI0, SPI1,
};
use embassy_rp::peripherals::{PIN_23, PIN_24, PIN_25, PIN_29, PIO1};
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
// defmt_rtt provides the RTT logging transport; the panic handler is our own
// crash::handler implementation (panic-probe removed from Cargo.toml).
use defmt_rtt as _;

use rustyboy_pico2w::crash;

use rustyboy_pico2w::audio::{AudioBuffers, SAMPLE_RATE};
use rustyboy_pico2w::display::hw::{GameDisplay, HwDisplay};
use rustyboy_pico2w::flash_rom::{new_onboard_flash, probe_staged_rom};
use rustyboy_pico2w::input::{ButtonState, InputHandler};
use rustyboy_pico2w::multicore::PicoGameBoy;
use rustyboy_pico2w::save_storage::{boot_load_saves, BootSaves, SaveSlot};
use rustyboy_pico2w::sd::{self, DummyClock};
use rustyboy_pico2w::xip_cartridge::XipCartridge;

#[cfg(feature = "oc-300")]
const TARGET_SYS_HZ: u32 = 300_000_000;
// 16 s gives worst-case frame/flash-sync stalls plenty of headroom without false-tripping;
// the multicore livelock that caused the old 5 s freeze→reset is separately fixed by WFE backpressure.
const WATCHDOG_WINDOW_MS: u64 = 16_000;
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
    // CH0 and CH1 are always needed (display DMA, audio DMA).
    // CH2 and CH3 are reserved for the WiFi CYW43439 PIO SPI driver; binding
    // them unconditionally is safe — the handlers are no-ops when unused.
    DMA_IRQ_0  => dma::InterruptHandler<DMA_CH0>,
                  dma::InterruptHandler<DMA_CH1>,
                  dma::InterruptHandler<DMA_CH2>,
                  dma::InterruptHandler<DMA_CH3>;
});

// WiFi uses PIO1: bind it separately, scoped to the wifi feature.
bind_interrupts!(struct WifiIrqs {
    PIO1_IRQ_0 => PioIrqHandler<PIO1>;
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
// WiFi peripheral tokens (consumed once when the portal starts)
// ---------------------------------------------------------------------------

/// All CYW43439 peripheral tokens, bundled for clean transfer from `main` into
/// [`App`] and ultimately into `WifiPortalScreen::start_portal`.
///
/// Wrapped in `Option` so they can be consumed (`take`) exactly once.
pub(crate) struct WifiPeriphs {
    pub pwr: Peri<'static, PIN_23>,
    pub dio: Peri<'static, PIN_24>,
    pub cs: Peri<'static, PIN_25>,
    pub clk: Peri<'static, PIN_29>,
    pub pio1: Peri<'static, PIO1>,
    pub dma_ch2: Peri<'static, DMA_CH2>,
    pub dma_ch3: Peri<'static, DMA_CH3>,
    /// PIO1 interrupt binding (from `WifiIrqs`).
    pub pio_irqs: WifiIrqs,
    /// DMA CH2/CH3 interrupt binding (from `Irqs`, which binds all DMA_IRQ_0 channels).
    pub dma_irqs: Irqs,
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
    /// `true` while the crash log contains records that have not yet been read
    /// by the host decoder tool.  Set once at boot; clears automatically on the
    /// next boot after the decoder runs `--mark-read`.
    pub(crate) crash_pending: bool,
    /// Embassy task spawner — needed by `WifiPortalScreen` to spawn portal tasks.
    pub(crate) spawner: Spawner,
    /// CYW43439 peripheral tokens.  `Some` until the portal is first started,
    /// then `None` for the rest of the session.
    pub(crate) wifi_periphs: Option<WifiPeriphs>,
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

pub(crate) enum AppState {
    Running(RunningState),
    InGameMenu(InGameMenuState),
    MainMenu(MainMenuState),
    RomList(Box<RomListState>),
    Loading(Box<LoadingState>),
    Settings(SettingsState),
    WifiMenu(Box<WifiMenuState>),
}

// ---------------------------------------------------------------------------
// Boot-time .data integrity guard — see `rustyboy_pico2w::integrity`
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // Verify the .data load image before it can be executed as RAM code.
    rustyboy_pico2w::integrity::verify_image();

    // Re-assert the ARMv8-M hardware stack-limit guard for core 0 (MSPLIM).
    // cortex-m-rt already arms this at reset to `_stack_end`; we set it again
    // explicitly so the invariant is greppable and survives a runtime change.
    // With it, any push below `_stack_end` (bottom of the ~79 KiB core-0 stack;
    // the 160 KiB heap is a separate .bss array) raises a STKOF UsageFault that
    // escalates to the HardFault handler at the exact offending instruction.
    // NB: because this guard PRE-EXISTS, the #5 bus-fault crashes (IBUSERR /
    // PRECISERR, not STKOF) are *not* a core-0 stack overflow — see
    // docs/investigations/crash-debug-notes.md.  Safety: `_stack_end` is a linker symbol.
    unsafe {
        unsafe extern "C" {
            static _stack_end: u32;
        }
        cortex_m::register::msplim::write(core::ptr::addr_of!(_stack_end) as u32);
    }

    // Make the .data RAM-code region read-only on Core 0.
    // If the corruptor is a Core-0 CPU write into this region, any such write
    // fires DACCVIOL (MemManage → HardFault) with the exact writer PC stacked.
    // Core 0 never legitimately writes its own code, so zero false positives.
    // Region covers __sdata through the last complete 32-byte block before
    // _SEGGER_RTT, which is written by defmt.
    // See docs/investigations/crash-debug-notes.md — "MPU read-only on .data / RAM-code region".
    // Protect .data / RAM-code region as priv-RO. Any write fires DACCVIOL;
    // MMFAR records the exact write address so we can identify the corruptor.
    unsafe { rustyboy_pico2w::mpu::setup_core0_data_mpu() };

    {
        use core::mem::MaybeUninit;
        // Save-state boot is the allocator high-water mark: by the time the
        // boot path reads SLOT0.RBS, GameBoy and the core1 worker are live.
        //
        // Major live allocations measured from release symbols:
        //   ~40 KiB Box<GameBoyMemory> and cartridge state
        //   ~22 KiB GameBoy::front_buffer
        //   ~31 KiB core1 PPU worker framebuffer/state
        //   ~8 KiB  APU sample buffers, plus smaller CPU/opcode allocations
        //   ~49 KiB SaveState blob for a 32 KiB cart-RAM game
        //
        // 160 KiB leaves room for GameBoy state plus the transient save-state
        // blob while preserving core0 stack headroom. Allocation failures on
        // this path are reported through `try_reserve_exact`, not HardFaults.
        const HEAP_SIZE: usize = 160 * 1024;
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
    watchdog.start(Duration::from_millis(WATCHDOG_WINDOW_MS));
    // Pause the watchdog while a debugger has the cores halted. embassy's
    // `start()` clears the pause-on-debug bits, so without this the 10 s
    // watchdog fires during a probe-rs flash or GDB halt — resetting the chip
    // mid-operation (mid-flash page-write timeouts; GDB "core is running"
    // fatal errors). No effect in production (no debugger => never paused).
    watchdog.pause_on_debug(true);

    info!(
        "rustyboy-pico2w v{} starting @{}MHz",
        FIRMWARE_VERSION,
        TARGET_SYS_HZ / 1_000_000
    );

    // Commit any pending crash record as the VERY FIRST thing after clocks are
    // up.  Flash only needs the system clock — no display, SD, or other
    // peripherals required.  Moving this before HwDisplay::new() ensures the
    // record is written even if the crash happens during the splash screen.
    let mut onboard_flash = new_onboard_flash(p.FLASH);
    if crash::storage::check_and_commit(&mut onboard_flash) {
        defmt::warn!("crash: committed crash record from previous boot");
    }
    if crash::storage::check_reset_reason(&mut onboard_flash) {
        defmt::warn!("crash: committed reset-reason record from previous boot");
    }

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
    info!("starting splash");
    hw_disp.splash().await;
    drop(hw_disp);

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

    // Check for unread crash records so menus can show the crash-report badge.
    // (check_and_commit already ran earlier — before the splash.)
    let crash_pending = crash::storage::has_records(&mut onboard_flash);
    if crash_pending {
        defmt::info!("crash: unread records found in flash — showing badge");
    }

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
                    // Box immediately so the GameBoy + Core1Transport (and its
                    // immutable `imm` block) live on the HEAP from the very first
                    // transport call — including the boot-time save-state restore
                    // below, whose `load_state` drives `check_shared` and arms the
                    // `transport-immutable-mpu` region. If `gb` were a stack local
                    // here, the MPU would arm over the stack and exception stacking
                    // would fault (MSTKERR false positive). See
                    // docs/investigations/memory-barrier-investigation-plan.md.
                    let mut gb: Box<PicoGameBoy> =
                        Box::new(PicoGameBoy::with_cartridge(p.CORE1, Box::new(cart)));
                    if let Some(rom_id) = info.rom_id {
                        // A save state contains cart RAM, so prefer it and only
                        // read the battery file if no state is available. This
                        // avoids holding two large SD blobs at once during boot.
                        let slot = SaveSlot::new(0).expect("slot 0 is valid");
                        watchdog.feed(Duration::from_millis(WATCHDOG_WINDOW_MS));
                        let save_state_blob =
                            sd_mgr.read_save_state(&rom_id, slot).unwrap_or_else(|e| {
                                defmt::warn!(
                                    "boot save state read failed: {:?}",
                                    defmt::Debug2Format(&e)
                                );
                                None
                            });
                        watchdog.feed(Duration::from_millis(WATCHDOG_WINDOW_MS));
                        let has_save_state = save_state_blob.is_some();
                        let battery_data = if has_save_state {
                            None // save state includes cart RAM; skip battery read
                        } else {
                            watchdog.feed(Duration::from_millis(WATCHDOG_WINDOW_MS));
                            sd_mgr.read_battery_save(&rom_id).unwrap_or_else(|e| {
                                defmt::warn!("battery load failed: {:?}", defmt::Debug2Format(&e));
                                None
                            })
                        };
                        match boot_load_saves(battery_data, save_state_blob) {
                            None => {}
                            Some(BootSaves::BatterySave(data)) => gb.set_external_ram(&data),
                            Some(BootSaves::SaveState(state)) => {
                                let _ = gb.load_state(state);
                                info!("save state loaded on boot");
                            }
                            Some(BootSaves::Both {
                                battery,
                                save_state,
                            }) => {
                                gb.set_external_ram(&battery);
                                let _ = gb.load_state(save_state);
                                info!("save state loaded on boot");
                            }
                        }
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
                        crash_pending,
                        spawner,
                        wifi_periphs: None,
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
                crash_pending,
                spawner,
                wifi_periphs: None,
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

    info!("entering main loop");

    // Box the GameBoy so the `GameBoy` + `Core1Transport` (and its immutable
    // `imm` block, used by the `transport-immutable-mpu` writer-ID trap) live on
    // the HEAP at a stable address rather than high in the core-0 stack. A
    // stack-resident transport defeats the priv-RO MPU region: normal exception
    // stacking writes through the protected page and faults (MSTKERR) before the
    // bug #5 wild store runs. See docs/investigations/memory-barrier-investigation-plan.md.
    let mut gameboy: Option<Box<PicoGameBoy>> = gameboy_init;

    let wifi_periphs = Some(WifiPeriphs {
        pwr: p.PIN_23,
        dio: p.PIN_24,
        cs: p.PIN_25,
        clk: p.PIN_29,
        pio1: p.PIO1,
        dma_ch2: p.DMA_CH2,
        dma_ch3: p.DMA_CH3,
        pio_irqs: WifiIrqs,
        dma_irqs: Irqs,
    });

    let mut app = App {
        state: initial_state,
        next_state: None,
        save_slot_available: false,
        previous_buttons: ButtonState::default(),
        staged_rom_name,
        staged_rom_id,
        core1: core1_token,
        crash_pending,
        spawner,
        wifi_periphs,
    };
    refresh_save_slot_available(&mut app, &sd_mgr);

    #[cfg(feature = "fps")]
    let mut tracker = perf::PerfTracker::new();

    loop {
        watchdog.feed(Duration::from_millis(WATCHDOG_WINDOW_MS));

        // Take the current state out so we can pass &mut app into tick().
        let mut state = core::mem::replace(&mut app.state, AppState::Running(RunningState));

        match &mut state {
            AppState::Running(running) => {
                running
                    .tick(
                        &mut app,
                        gameboy.as_deref_mut().expect("Running without GameBoy"),
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
                        gameboy.as_deref_mut().expect("InGameMenu without GameBoy"),
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

            AppState::Settings(settings_state) => {
                settings_state
                    .tick(&mut app, &mut game_disp, &mut input, &mut onboard_flash)
                    .await;
            }

            AppState::WifiMenu(wifi_menu_state) => {
                wifi_menu_state
                    .tick(&mut app, &mut game_disp, &mut input, &mut onboard_flash)
                    .await;
            }
        }

        // Restore state or apply a queued transition.
        app.state = app.next_state.take().unwrap_or(state);
    }
}
