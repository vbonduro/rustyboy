#![no_std]
#![no_main]
extern crate alloc;

#[cfg(feature = "fps")]
mod perf;

use embedded_alloc::Heap;

#[global_allocator]
static HEAP: Heap = Heap::empty();

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH0, PIO0};
use embassy_rp::pio::{InterruptHandler as PioIrqHandler, Pio};
use embassy_rp::pio_programs::i2s::{PioI2sOut, PioI2sOutProgram};
use embassy_rp::spi::{self, Spi};
use embassy_rp::watchdog::Watchdog;
use embassy_rp::{bind_interrupts, dma};
use embassy_time::{Delay, Duration, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{SdCard, VolumeManager};
use {defmt_rtt as _, panic_probe as _};

use rustyboy_core::cpu::cpu::Cpu;
use rustyboy_core::cpu::instructions::opcodes::OpCodeDecoder;
use rustyboy_core::cpu::peripheral::joypad::Button;
use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::sm83::Sm83;
use rustyboy_core::memory::{GameBoyMemory, StreamingCartridge};
use rustyboy_pico2w::audio::{AudioBuffers, SAMPLE_RATE};
use rustyboy_pico2w::display::hw::HwDisplay;
use rustyboy_pico2w::display::scale_to_rgb565;
use rustyboy_pico2w::flash_rom::{
    new_onboard_flash, probe_staged_rom, stage_rom_from_reader, FlashRomReader,
};
use rustyboy_pico2w::input::{ButtonState, InputHandler};
use rustyboy_pico2w::sd::{DummyClock, SdRomReader};

#[cfg(feature = "oc-266")]
const TARGET_SYS_HZ: u32 = 266_000_000;
#[cfg(not(feature = "oc-266"))]
const TARGET_SYS_HZ: u32 = 250_000_000;

const TARGET_CORE_VOLTAGE: embassy_rp::clocks::CoreVoltage =
    embassy_rp::clocks::CoreVoltage::V1_20;

const FIRMWARE_VERSION: &str = env!("CARGO_PKG_VERSION");
const CYCLES_PER_FRAME: u64 = 70_224;

bind_interrupts!(struct Irqs {
    PIO0_IRQ_0 => PioIrqHandler<PIO0>;
    DMA_IRQ_0  => dma::InterruptHandler<DMA_CH0>;
});

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
        const HEAP_SIZE: usize = 256 * 1024;
        static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        unsafe { HEAP.init(core::ptr::addr_of!(HEAP_MEM) as usize, HEAP_SIZE) }
    }

    // Allocate the pre-scaled display frame buffer from the heap so it does not
    // live in .bss and eat into the stack guard region.
    let frame_buf: &'static mut [u16; 51840] = {
        let layout = core::alloc::Layout::new::<[u16; 51840]>();
        let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) } as *mut [u16; 51840];
        unsafe { alloc::boxed::Box::leak(alloc::boxed::Box::from_raw(ptr)) }
    };

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

    // GP8=DC  GP9=CS  GP10=CLK  GP11=MOSI  GP12=RST  GP13=BL
    let mut hw_disp = HwDisplay::new(
        p.SPI1, p.PIN_10, p.PIN_11, p.PIN_9, p.PIN_8, p.PIN_12, p.PIN_13,
    );
    hw_disp.splash().await;

    // GP21=Up  GP22=Down  GP26=Left  GP27=Right
    // GP0=A    GP1=B      GP2=Start  GP3=Select
    let mut input = InputHandler::new(
        p.PIN_21, p.PIN_22, p.PIN_26, p.PIN_27, p.PIN_0, p.PIN_1, p.PIN_2, p.PIN_3,
    );

    let mut onboard_flash = new_onboard_flash(p.FLASH);
    let flash_info = if let Some(info) = probe_staged_rom(&mut onboard_flash) {
        info!(
            "staged ROM found in flash: {} banks ({} KiB)",
            info.bank_count,
            info.size_bytes / 1024
        );
        info
    } else {
        info!("no staged ROM in flash; loading from SD");

        let mut spi_cfg = spi::Config::default();
        spi_cfg.frequency = 400_000;
        let spi_bus = Spi::new_blocking(p.SPI0, p.PIN_6, p.PIN_7, p.PIN_4, spi_cfg);
        // SD card MISO (GP4) is open-collector — enable the internal pull-up.
        rp_pac::PADS_BANK0.gpio(4).modify(|w| w.set_pue(true));
        let spi_dev = ExclusiveDevice::new(spi_bus, Output::new(p.PIN_5, Level::High), Delay);
        let sdcard = SdCard::new(spi_dev, Delay);
        let mgr = VolumeManager::new(sdcard, DummyClock);

        let mut reader = match SdRomReader::new(mgr) {
            Ok(r) => r,
            Err(e) => {
                error!("SD init failed: {:?}", defmt::Debug2Format(&e));
                loop {
                    Timer::after(Duration::from_millis(2_000)).await;
                }
            }
        };

        let info = match stage_rom_from_reader(&mut onboard_flash, &mut reader) {
            Ok(info) => info,
            Err(e) => {
                error!("ROM staging failed: {:?}", defmt::Debug2Format(&e));
                loop {
                    Timer::after(Duration::from_millis(2_000)).await;
                }
            }
        };

        info!(
            "ROM staged to flash: {} banks ({} KiB)",
            info.bank_count,
            info.size_bytes / 1024
        );
        info
    };

    let cart = match StreamingCartridge::new(FlashRomReader::new(onboard_flash, flash_info)) {
        Ok(c) => c,
        Err(e) => {
            error!("flash ROM load failed: {:?}", defmt::Debug2Format(&e));
            loop {
                Timer::after(Duration::from_millis(2_000)).await;
            }
        }
    };

    let memory = GameBoyMemory::with_cartridge(alloc::boxed::Box::new(cart));
    let decoder = alloc::boxed::Box::new(OpCodeDecoder::new());
    let mut cpu = Sm83::new(alloc::boxed::Box::new(memory), decoder)
        .with_registers(Registers {
            a: 0x01,
            f: Flags::from_bits_truncate(0xB0),
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            pc: 0x0100,
            sp: 0xFFFE,
        })
        .with_dmg_state();
    info!("ROM loaded, entering game loop");

    // I2S audio: GP14=BCLK  GP15=LRCLK  GP16=DIN  GP17=SD_MODE (MAX98357A).
    // Drive SD_MODE high to enable the amplifier.
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
        p.PIN_16, // DIN
        p.PIN_14, // BCLK
        p.PIN_15, // LRCLK
        SAMPLE_RATE,
        16, // bit depth
        &i2s_prog,
    );
    i2s.start();

    // Draw static letterbox bars once; game loop only repaints the 240×216 area.
    hw_disp.inner.draw_letterbox_bars();

    #[cfg(feature = "perf")]
    perf::init_dwt();

    let mut audio_buffers = AudioBuffers::new();
    let mut prev_state = ButtonState::default();

    #[cfg(feature = "fps")]
    let mut tracker = perf::PerfTracker::new();

    loop {
        // Start DMA transfer of last frame's audio output while we fill the
        // other buffer from the current frame's APU samples.
        let (front_buf, back_buf) = audio_buffers.front_back_buffers();
        let dma_future = i2s.write(front_buf);

        // Run exactly one Game Boy frame (70 224 T-cycles ≈ 16.74 ms).
        let frame_start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(frame_start) < CYCLES_PER_FRAME {
            let _ = cpu.tick();
        }

        // Propagate button changes to the CPU.
        let (state, menu) = input.poll();
        for (btn, pressed) in prev_state.diff(state) {
            cpu.set_button(btn, pressed);
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

        // Pre-scale into the static buffer then push to display.
        // Letterbox bars are static and not repainted.
        #[cfg(feature = "perf")]
        let scale_start = perf::perf_cycle_read();
        scale_to_rgb565(cpu.framebuffer(), frame_buf);
        #[cfg(feature = "perf")]
        tracker.record_scale(perf::perf_cycle_read().wrapping_sub(scale_start));

        #[cfg(feature = "perf")]
        let render_start = perf::perf_cycle_read();
        hw_disp.inner.render_game_only_scaled(frame_buf);
        #[cfg(feature = "perf")]
        tracker.record_render(perf::perf_cycle_read().wrapping_sub(render_start));

        // Convert APU f32 output into I2S-packed u32 words for the next DMA.
        let samples = cpu.drain_audio_samples();
        audio_buffers.queue_next_frame(&samples, back_buf);

        // Await DMA completion — this naturally paces the loop to ~59.7 fps.
        dma_future.await;
        watchdog.feed(Duration::from_millis(5_000));

        #[cfg(feature = "fps")]
        tracker.tick(&mut cpu);
    }
}

fn btn_name(b: Button) -> &'static str {
    match b {
        Button::Up => "Up",
        Button::Down => "Down",
        Button::Left => "Left",
        Button::Right => "Right",
        Button::A => "A",
        Button::B => "B",
        Button::Start => "Start",
        Button::Select => "Select",
    }
}
