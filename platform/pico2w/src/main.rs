#![no_std]
#![no_main]
extern crate alloc;

use alloc::{boxed::Box, vec::Vec};
use core::future::Future;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use cortex_m_rt::ExceptionFrame;
#[cortex_m_rt::exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    defmt::error!("HardFault: PC=0x{:08x} LR=0x{:08x} PSR=0x{:08x}",
        ef.pc(), ef.lr(), ef.xpsr());
    loop {}
}

#[cfg(feature = "fps")]
mod perf;

use embedded_alloc::Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{
    CORE1, DMA_CH0, DMA_CH1, PIN_10, PIN_11, PIN_12, PIN_13,
    PIN_8, PIN_9, PIO0, SPI0, SPI1,
};
use embassy_rp::Peri;
use embassy_rp::pio::{InterruptHandler as PioIrqHandler, Pio};
use embassy_rp::pio_programs::i2s::{PioI2sOut, PioI2sOutProgram};
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_rp::watchdog::Watchdog;
use embassy_rp::{bind_interrupts, dma};
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::SdCard;
use {defmt_rtt as _, panic_probe as _};

use rustyboy_core::cpu::save_state::SaveState;
use rustyboy_pico2w::audio::{AudioBuffers, SAMPLE_RATE};
use rustyboy_pico2w::display::hw::{GameDisplay, HwDisplay};
use rustyboy_pico2w::flash_rom::{
    new_onboard_flash, probe_staged_rom, FlashRomInfo, OnboardFlash, RomStager,
};
use rustyboy_pico2w::input::{ButtonState, InputHandler};
use rustyboy_pico2w::menu::{
    InGameMenu, MainMenu, MenuEffect, MenuInput, MenuLogic, RomListEffect, RomListLogic,
};
use rustyboy_pico2w::multicore::PicoGameBoy;
use rustyboy_pico2w::sd::{self, DummyClock};
use rustyboy_pico2w::stack_probe;
use rustyboy_pico2w::xip_cartridge::XipCartridge;

#[cfg(feature = "oc-300")]
const TARGET_SYS_HZ: u32 = 300_000_000;
#[cfg(all(not(feature = "oc-300"), feature = "oc-280"))]
const TARGET_SYS_HZ: u32 = 280_000_000;
#[cfg(all(not(feature = "oc-300"), not(feature = "oc-280"), feature = "oc-266"))]
const TARGET_SYS_HZ: u32 = 266_000_000;
#[cfg(all(not(feature = "oc-300"), not(feature = "oc-280"), not(feature = "oc-266")))]
const TARGET_SYS_HZ: u32 = 300_000_000;

#[cfg(feature = "oc-300")]
const TARGET_CORE_VOLTAGE: embassy_rp::clocks::CoreVoltage =
    embassy_rp::clocks::CoreVoltage::V1_30;
#[cfg(feature = "oc-280")]
const TARGET_CORE_VOLTAGE: embassy_rp::clocks::CoreVoltage =
    embassy_rp::clocks::CoreVoltage::V1_25;
#[cfg(all(not(feature = "oc-300"), not(feature = "oc-280")))]
const TARGET_CORE_VOLTAGE: embassy_rp::clocks::CoreVoltage =
    embassy_rp::clocks::CoreVoltage::V1_30;

const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");
const CYCLES_PER_FRAME: u64 = 70_224;

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

type PicoSdCard = SdCard<
    ExclusiveDevice<Spi<'static, SPI0, Blocking>, Output<'static>, Delay>,
    Delay,
>;
type PicoSdMgr = sd::SdManager<PicoSdCard, DummyClock>;

// ---------------------------------------------------------------------------
// Minimal no-alloc waker for poll_once
// ---------------------------------------------------------------------------

unsafe fn noop_waker_clone(_: *const ()) -> RawWaker {
    RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)
}
unsafe fn noop_waker(_: *const ()) {}
static NOOP_WAKER_VTABLE: RawWakerVTable =
    RawWakerVTable::new(noop_waker_clone, noop_waker, noop_waker, noop_waker);

fn poll_once<F: Future>(future: core::pin::Pin<&mut F>) -> bool {
    let waker = unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &NOOP_WAKER_VTABLE)) };
    let mut cx = Context::from_waker(&waker);
    matches!(future.poll(&mut cx), Poll::Ready(_))
}

// ---------------------------------------------------------------------------
// App state machine
// ---------------------------------------------------------------------------

struct App {
    state:             AppState,
    next_state:        Option<AppState>,
    save_slot:         Option<Vec<u8>>,
    previous_buttons:  ButtonState,
    staged_rom_name:   Option<heapless::String<64>>,
    core1:             Option<Peri<'static, CORE1>>,
}

impl App {
    fn transition_to(&mut self, next: AppState) {
        self.next_state = Some(next);
    }
}

enum AppState {
    Running(RunningState),
    InGameMenu(InGameMenuState),
    MainMenu(MainMenuState),
    RomList(Box<RomListState>),
    Loading(Box<LoadingState>),
}

// ---------------------------------------------------------------------------
// Running state
// ---------------------------------------------------------------------------

struct RunningState;

impl RunningState {
    async fn tick(
        &mut self,
        app: &mut App,
        cpu: &mut PicoGameBoy,
        game_disp: &mut GameDisplay<'static>,
        i2s: &mut PioI2sOut<'static, PIO0, 0>,
        input: &mut InputHandler<'static>,
        audio_buffers: &mut AudioBuffers,
        audio_samples: &mut Vec<i16>,
    ) {
        // Scope the DMA futures so `game_disp` is free after the block.
        let open_menu = {
            let frame_buf = cpu.published_scaled_frame();

            let mut disp_future = core::pin::pin!(game_disp.send_frame_raw(frame_buf));
            let _ = poll_once(disp_future.as_mut());

            let (front_buf, back_buf) = audio_buffers.front_back_buffers();
            let mut audio_future = core::pin::pin!(i2s.write(front_buf));
            let _ = poll_once(audio_future.as_mut());

            let frame_start = cpu.cycle_counter();
            while cpu.cycle_counter().wrapping_sub(frame_start) < CYCLES_PER_FRAME {
                cpu.tick();
            }

            let (current_buttons, open_menu) = input.poll();
            for (btn, pressed) in app.previous_buttons.diff(current_buttons) {
                cpu.set_button(btn, pressed);
            }
            app.previous_buttons = current_buttons;

            cpu.drain_audio_samples_into_i16(audio_samples);
            audio_buffers.queue_next_frame_i16(audio_samples, back_buf);

            disp_future.as_mut().await;
            cpu.release_scaled_frame();
            audio_future.as_mut().await;

            open_menu
        };

        if open_menu {
            let next = InGameMenuState::new(game_disp, app).await;
            app.transition_to(AppState::InGameMenu(next));
        }
    }
}

// ---------------------------------------------------------------------------
// In-game pause menu state
// ---------------------------------------------------------------------------

struct InGameMenuState {
    menu: InGameMenu,
}

impl InGameMenuState {
    async fn new(game_disp: &mut GameDisplay<'static>, app: &App) -> Self {
        let menu = InGameMenu::new();
        let frame = menu.frame(app.save_slot.is_some());
        game_disp.draw_menu(&frame).await;
        Self { menu }
    }

    async fn tick(
        &mut self,
        app: &mut App,
        cpu: &mut PicoGameBoy,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
    ) {
        Timer::after(Duration::from_millis(33)).await;

        let (current_buttons, _) = input.poll();
        let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
        app.previous_buttons = current_buttons;

        if !menu_input.any() {
            return;
        }

        match self.menu.handle(menu_input) {
            MenuEffect::None => {
                let frame = self.menu.frame(app.save_slot.is_some());
                game_disp.draw_menu(&frame).await;
            }
            MenuEffect::Resume => {
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::Save => {
                app.save_slot = Some(cpu.save_state());
                let frame = self.menu.frame(true);
                game_disp.draw_menu(&frame).await;
            }
            MenuEffect::Load => {
                if let Some(blob) = app.save_slot.as_ref() {
                    match SaveState::from_blob(blob.clone()) {
                        Ok(save_state) => { let _ = cpu.load_state(save_state); }
                        Err(message)   => { warn!("load failed: {}", message); }
                    }
                }
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::Quit => {
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Main menu state
// ---------------------------------------------------------------------------

struct MainMenuState {
    menu: MainMenu,
}

impl MainMenuState {
    async fn new(game_disp: &mut GameDisplay<'static>, app: &App) -> Self {
        let menu = MainMenu::new();
        let frame = menu.frame(app.staged_rom_name.is_some());
        game_disp.draw_menu(&frame).await;
        Self { menu }
    }

    async fn tick(
        &mut self,
        app: &mut App,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
        sd_mgr: &mut PicoSdMgr,
    ) {
        Timer::after(Duration::from_millis(33)).await;

        let (current_buttons, _) = input.poll();
        let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
        app.previous_buttons = current_buttons;

        if !menu_input.any() {
            return;
        }

        let game_available = app.staged_rom_name.is_some();
        match self.menu.handle_main(menu_input, game_available) {
            MenuEffect::None => {
                let frame = self.menu.frame(game_available);
                game_disp.draw_menu(&frame).await;
            }
            MenuEffect::Continue => {
                game_disp.draw_letterbox_bars().await;
                app.transition_to(AppState::Running(RunningState));
            }
            MenuEffect::ShowRoms => {
                let next = RomListState::new(app, game_disp, sd_mgr).await;
                app.transition_to(AppState::RomList(Box::new(next)));
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// ROM list state
// ---------------------------------------------------------------------------

struct RomListState {
    page:        heapless::Vec<heapless::String<64>, 7>,
    page_offset: usize,
    has_next:    bool,
    logic:       RomListLogic,
}

impl RomListState {
    async fn new(
        app: &App,
        game_disp: &mut GameDisplay<'static>,
        sd_mgr: &mut PicoSdMgr,
    ) -> Self {
        let (page, has_next) = match sd_mgr.list_rom_page(0, 7) {
            Ok(result) => result,
            Err(e) => {
                defmt::error!("SD list failed: {:?}", defmt::Debug2Format(&e));
                (heapless::Vec::new(), false)
            }
        };
        let page_len = page.len();
        let logic = RomListLogic::new(page_len);
        let state = Self { page, page_offset: 0, has_next, logic };
        state.draw(app, game_disp).await;
        state
    }

    async fn draw(&self, app: &App, game_disp: &mut GameDisplay<'static>) {
        let mut items_arr: heapless::Vec<&str, 7> = heapless::Vec::new();
        let mut enabled_arr: heapless::Vec<bool, 7> = heapless::Vec::new();
        for name in self.page.iter() {
            let _ = items_arr.push(name.as_str());
            let _ = enabled_arr.push(true);
        }

        let marked = app.staged_rom_name.as_ref().and_then(|staged| {
            self.page.iter().position(|name| name.as_str().eq_ignore_ascii_case(staged.as_str()))
        });

        let frame = rustyboy_pico2w::menu::MenuFrame {
            title:    "ROMS",
            items:    items_arr.as_slice(),
            selected: self.logic.selected(),
            enabled:  enabled_arr.as_slice(),
            marked,
        };
        game_disp.draw_menu(&frame).await;
    }

    async fn flip_page(
        &mut self,
        new_offset: usize,
        sd_mgr: &mut PicoSdMgr,
    ) {
        match sd_mgr.list_rom_page(new_offset, 7) {
            Ok((page, has_next)) => {
                let page_len: usize = page.len();
                self.page = page;
                self.page_offset = new_offset;
                self.has_next = has_next;
                self.logic.reset(page_len);
            }
            Err(e) => {
                defmt::error!("SD flip_page failed: {:?}", defmt::Debug2Format(&e));
            }
        }
    }

    async fn tick(
        &mut self,
        app: &mut App,
        game_disp: &mut GameDisplay<'static>,
        input: &mut InputHandler<'static>,
        sd_mgr: &mut PicoSdMgr,
    ) {
        Timer::after(Duration::from_millis(33)).await;

        let (current_buttons, _) = input.poll();
        let menu_input = MenuInput::from_diff(app.previous_buttons, current_buttons);
        app.previous_buttons = current_buttons;

        if !menu_input.any() {
            return;
        }

        match self.logic.handle(menu_input) {
            RomListEffect::None => {
                self.draw(app, game_disp).await;
            }
            RomListEffect::SelectItem => {
                let filename = self.page[self.logic.selected()].clone();
                app.transition_to(AppState::Loading(Box::new(LoadingState { filename })));
            }
            RomListEffect::NextPage => {
                if self.has_next {
                    let new_offset = self.page_offset + 7;
                    self.flip_page(new_offset, sd_mgr).await;
                    self.draw(app, game_disp).await;
                }
            }
            RomListEffect::PrevPage => {
                if self.page_offset > 0 {
                    let new_offset = self.page_offset.saturating_sub(7);
                    self.flip_page(new_offset, sd_mgr).await;
                    self.draw(app, game_disp).await;
                }
            }
            RomListEffect::Back => {
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Loading state
// ---------------------------------------------------------------------------

struct LoadingState {
    filename: heapless::String<64>,
}

impl LoadingState {
    async fn tick(
        &mut self,
        app: &mut App,
        cpu: &mut Option<PicoGameBoy>,
        flash: &mut OnboardFlash<'_>,
        sd_mgr: &mut PicoSdMgr,
        game_disp: &mut GameDisplay<'static>,
        watchdog: &mut Watchdog,
    ) {
        // Halt core1 if a game is running — must stop before writing flash.
        // Then drop the GameBoy to free its heap (~70 KB: front_buffer, GameBoyMemory,
        // OpCodeTable arcs, APU buffer) before RomStager::begin allocates its 16 KB
        // read buffer.  Core 1 remains halted in its WFE loop after the drop.
        let had_game = cpu.is_some();
        if let Some(existing_cpu) = cpu.as_mut() {
            existing_cpu.halt();
        }
        if had_game {
            *cpu = None;
        }

        // If the ROM is already staged in flash, skip the erase/write cycle.
        // Avoids the flash-pause handshake with the halted core1 and is
        // idempotent when the user re-selects the currently loaded ROM.
        let already_staged = app.staged_rom_name.as_deref()
            .map(|n| n.eq_ignore_ascii_case(self.filename.as_str()))
            .unwrap_or(false);

        let stage_result: Result<FlashRomInfo, ()> = if already_staged {
            probe_staged_rom(flash).map(|(info, _)| info).ok_or(())
        } else {
            stage_rom_from_sd(
                self.filename.as_str(),
                flash,
                sd_mgr,
                game_disp,
                watchdog,
            ).await
        };

        match stage_result {
            Ok(info) => {
                app.staged_rom_name = Some(self.filename.clone());
                if !had_game {
                    // First ROM load — create the GameBoy and start Running.
                    let core1 = app.core1.take().expect("CORE1 consumed without a running game");
                    match XipCartridge::from_staged_flash(info) {
                        Ok(cart) => {
                            *cpu = Some(PicoGameBoy::with_cartridge(core1, Box::new(cart)));
                            game_disp.draw_letterbox_bars().await;
                            app.transition_to(AppState::Running(RunningState));
                        }
                        Err(e) => {
                            defmt::error!("XIP cart error: {:?}", defmt::Debug2Format(&e));
                            app.core1 = Some(core1);
                            let next = MainMenuState::new(game_disp, app).await;
                            app.transition_to(AppState::MainMenu(next));
                        }
                    }
                } else {
                    // ROM switch — core1 is halted; trigger watchdog reset to
                    // boot fresh with the newly staged ROM.
                    info!("ROM staged, restarting via watchdog");
                    watchdog.start(Duration::from_millis(100));
                    loop {}
                }
            }
            Err(_) => {
                defmt::error!("ROM staging failed for {}", self.filename.as_str());
                let next = MainMenuState::new(game_disp, app).await;
                app.transition_to(AppState::MainMenu(next));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ROM staging helper
// ---------------------------------------------------------------------------

async fn stage_rom_from_sd(
    filename: &str,
    flash: &mut OnboardFlash<'_>,
    sd_mgr: &mut PicoSdMgr,
    game_disp: &mut GameDisplay<'static>,
    watchdog: &mut Watchdog,
) -> Result<FlashRomInfo, ()> {
    let mut reader = sd_mgr.open_rom_reader(filename).map_err(|e| {
        defmt::error!("sd open failed: {:?}", defmt::Debug2Format(&e));
    })?;

    // RP2350 watchdog maximum is 16,777,215 µs (~16.7 s). Use 16 s so the
    // erase phase (which can take several seconds) does not starve the watchdog.
    watchdog.feed(Duration::from_millis(16_000));
    let mut stager = RomStager::begin(flash, &mut reader).map_err(|e| {
        defmt::error!("stager begin failed: {:?}", defmt::Debug2Format(&e));
    })?;
    info!("stager begin done: {} banks", stager.total_banks());
    game_disp.draw_loading_progress(filename, 0, stager.total_banks() as u32).await;

    loop {
        watchdog.feed(Duration::from_millis(5_000));
        let done = stager.write_next_bank(flash, &mut reader).map_err(|e| {
            defmt::error!("bank write failed: {:?}", defmt::Debug2Format(&e));
        })?;
        game_disp
            .draw_loading_progress(
                filename,
                stager.banks_written() as u32,
                stager.total_banks() as u32,
            )
            .await;
        if done {
            break;
        }
    }

    drop(reader);

    stager.finish(flash, filename).map_err(|e| {
        defmt::error!("stager finish failed: {:?}", defmt::Debug2Format(&e));
    })
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
    let Pio { mut common, sm0, .. } = Pio::new(p.PIO0, Irqs);
    let i2s_prog = PioI2sOutProgram::new(&mut common);
    let mut i2s = PioI2sOut::new(
        &mut common, sm0, p.DMA_CH0, Irqs,
        p.PIN_16, p.PIN_14, p.PIN_15,
        SAMPLE_RATE, 16, &i2s_prog,
    );
    i2s.start();

    // SAFETY: hw_disp was dropped above; SPI1 and all display pins are free.
    let mut game_disp = unsafe {
        GameDisplay::new_after_splash(
            PIN_10::steal(), PIN_11::steal(),
            PIN_9::steal(),  PIN_8::steal(),
            PIN_12::steal(), PIN_13::steal(),
            SPI1::steal(),   p.DMA_CH1,
            Irqs,
        )
    };

    let mut audio_buffers = AudioBuffers::new();
    let mut audio_samples = Vec::with_capacity(2048);

    // Build initial state: Running if a ROM is staged, else MainMenu.
    let (initial_state, cpu_init, staged_rom_name, core1_token) = match staged {
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
                    info!("building GameBoy, clk={}", embassy_rp::clocks::clk_sys_freq());
                    let gb = PicoGameBoy::with_cartridge(p.CORE1, Box::new(cart));
                    info!("ROM loaded, entering main loop");
                    (AppState::Running(RunningState), Some(gb), name, None)
                }
                Err(e) => {
                    defmt::error!("flash ROM mapping failed: {:?}", defmt::Debug2Format(&e));
                    let next = MainMenuState::new(&mut game_disp, &App {
                        state: AppState::MainMenu(MainMenuState { menu: MainMenu::new() }),
                        next_state: None,
                        save_slot: None,
                        previous_buttons: ButtonState::default(),
                        staged_rom_name: None,
                        core1: None,
                    }).await;
                    (AppState::MainMenu(next), None, None, Some(p.CORE1))
                }
            }
        }
        None => {
            info!("no staged ROM in flash; showing main menu");
            // Build a temporary App stub so MainMenuState::new can read staged_rom_name.
            let stub_app = App {
                state: AppState::Running(RunningState),
                next_state: None,
                save_slot: None,
                previous_buttons: ButtonState::default(),
                staged_rom_name: None,
                core1: None,
            };
            let menu_state = MainMenuState::new(&mut game_disp, &stub_app).await;
            (AppState::MainMenu(menu_state), None, None, Some(p.CORE1))
        }
    };

    stack_probe::paint();
    info!("entering main loop");

    let mut cpu: Option<PicoGameBoy> = cpu_init;

    let mut app = App {
        state:            initial_state,
        next_state:       None,
        save_slot:        None,
        previous_buttons: ButtonState::default(),
        staged_rom_name,
        core1:            core1_token,
    };

    #[cfg(feature = "fps")]
    let mut tracker = perf::PerfTracker::new();

    loop {
        stack_probe::check_current_sp("main loop");
        watchdog.feed(Duration::from_millis(5_000));

        // Take the current state out so we can pass &mut app into tick().
        let mut state = core::mem::replace(&mut app.state, AppState::Running(RunningState));

        match &mut state {
            AppState::Running(running) => {
                running.tick(
                    &mut app,
                    cpu.as_mut().expect("Running without GameBoy"),
                    &mut game_disp,
                    &mut i2s,
                    &mut input,
                    &mut audio_buffers,
                    &mut audio_samples,
                ).await;

                #[cfg(feature = "fps")]
                tracker.tick();
            }

            AppState::InGameMenu(menu_state) => {
                menu_state.tick(
                    &mut app,
                    cpu.as_mut().expect("InGameMenu without GameBoy"),
                    &mut game_disp,
                    &mut input,
                ).await;
            }

            AppState::MainMenu(menu_state) => {
                menu_state.tick(&mut app, &mut game_disp, &mut input, &mut sd_mgr).await;
            }

            AppState::RomList(rom_list_state) => {
                rom_list_state.tick(&mut app, &mut game_disp, &mut input, &mut sd_mgr).await;
            }

            AppState::Loading(loading_state) => {
                loading_state.tick(
                    &mut app,
                    &mut cpu,
                    &mut onboard_flash,
                    &mut sd_mgr,
                    &mut game_disp,
                    &mut watchdog,
                ).await;
            }
        }

        // Restore state or apply a queued transition.
        app.state = app.next_state.take().unwrap_or(state);
    }
}
