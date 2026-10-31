//! Board support: clock profile, interrupt bindings and the pin/peripheral
//! assignment for the Pico 2W + ILI9341 + SD + MAX98357A wiring.
//!
//! [`Board::init`] brings up the HAL once and returns every peripheral grouped
//! by the subsystem that uses it. Callers move a group out of the `Board` and
//! hand it to the driver, so "each peripheral is claimed once" is enforced by
//! the compiler rather than by `steal()` and a comment.
//!
//! Pin numbers live ONLY in this file; the full wiring table is in
//! `docs/wiring.md`.

use embassy_rp::clocks::{ClockConfig, CoreVoltage};
use embassy_rp::peripherals::{
    CORE1, DMA_CH0, DMA_CH1, DMA_CH2, DMA_CH3, FLASH, PIN_0, PIN_1, PIN_10, PIN_11, PIN_12,
    PIN_13, PIN_14, PIN_15, PIN_16, PIN_17, PIN_2, PIN_20, PIN_21, PIN_22, PIN_23, PIN_24, PIN_25,
    PIN_26, PIN_27, PIN_29, PIN_3, PIN_4, PIN_5, PIN_6, PIN_7, PIN_8, PIN_9, PIO0, PIO1, SPI0,
    SPI1, WATCHDOG,
};
use embassy_rp::pio::InterruptHandler as PioIrqHandler;
use embassy_rp::{bind_interrupts, dma, Peri};

// ---------------------------------------------------------------------------
// Clock profile
// ---------------------------------------------------------------------------

/// A clock operating point: frequency and the core voltage it was validated at.
///
/// Frequency and voltage are two halves of ONE decision, so they live in one
/// place. They were previously two independent `#[cfg]` cascades keyed on
/// overlapping-but-different feature sets, which had already drifted: `oc-266`
/// selected 266 MHz but fell through to V1_30, silently *not* reproducing the
/// "266 MHz @ V1_25" operating point it is named for.
struct ClockProfile {
    hz: u32,
    voltage: CoreVoltage,
}

/// Stock overclock. NOT free of the SP-drift fault — see `OC_288`.
const OC_300: ClockProfile = ClockProfile {
    hz: 300_000_000,
    voltage: CoreVoltage::V1_30,
};

/// 288 MHz = PLL FBDIV 120 (VCO 1440) / POSTDIV 5 — one grid step below 300 on
/// the stock PLL config, and the fastest clock measured free of the SP-drift
/// fault. A frequency sweep by live PLL reprogramming put the failure cliff
/// between 288 and 300: at 300 the drift instrument logged 67.5 events per 1000
/// work-ticks, while 288/276/264/252/240 logged zero across 13,567 ticks. A 24 h
/// soak at 288 on the real firmware logged 0 crashes.
///
/// NOT proven safe: the cliff is bracketed only to (288, 300], so 288 may have
/// as little as 0.3% margin, untested against temperature and part-to-part
/// spread. `OC_266` is the more conservative shipping point.
/// See docs/investigations/sp-drift-root-cause.md.
const OC_288: ClockProfile = ClockProfile {
    hz: 288_000_000,
    voltage: CoreVoltage::V1_30,
};

const OC_280: ClockProfile = ClockProfile {
    hz: 280_000_000,
    voltage: CoreVoltage::V1_25,
};

/// 9 h clean on record at V1_25 — the voltage is part of that result.
const OC_266: ClockProfile = ClockProfile {
    hz: 266_000_000,
    voltage: CoreVoltage::V1_25,
};

/// Exactly one profile, chosen in priority order. Adding a profile is one const
/// plus one arm; frequency and voltage cannot desync.
const CLOCK: ClockProfile = if cfg!(feature = "oc-300") {
    OC_300
} else if cfg!(feature = "oc-288") {
    OC_288
} else if cfg!(feature = "oc-280") {
    OC_280
} else if cfg!(feature = "oc-266") {
    OC_266
} else {
    OC_300
};

// ---------------------------------------------------------------------------
// Interrupt bindings
// ---------------------------------------------------------------------------

bind_interrupts!(pub struct Irqs {
    PIO0_IRQ_0 => PioIrqHandler<PIO0>;
    // CH0 and CH1 are always needed (audio DMA, display DMA).
    // CH2 and CH3 are reserved for the WiFi CYW43439 PIO SPI driver; binding
    // them unconditionally is safe — the handlers are no-ops when unused.
    DMA_IRQ_0  => dma::InterruptHandler<DMA_CH0>,
                  dma::InterruptHandler<DMA_CH1>,
                  dma::InterruptHandler<DMA_CH2>,
                  dma::InterruptHandler<DMA_CH3>;
});

// WiFi uses PIO1: bound separately from `Irqs`.
bind_interrupts!(pub struct WifiIrqs {
    PIO1_IRQ_0 => PioIrqHandler<PIO1>;
});

// ---------------------------------------------------------------------------
// Peripheral groups
// ---------------------------------------------------------------------------

/// SD card on SPI0, plus its mode-select pin.
pub struct SdPeriphs {
    pub spi: Peri<'static, SPI0>,
    pub miso: Peri<'static, PIN_4>,
    pub cs: Peri<'static, PIN_5>,
    pub clk: Peri<'static, PIN_6>,
    pub mosi: Peri<'static, PIN_7>,
    /// GP17: selects SPI mode on the card module. Must stay driven for the
    /// lifetime of the card.
    pub mode: Peri<'static, PIN_17>,
}

/// ILI9341 on SPI1. Used first by the blocking splash driver, then by the
/// async game driver (which also uses `dma`).
pub struct DisplayPeriphs {
    pub spi: Peri<'static, SPI1>,
    pub dc: Peri<'static, PIN_8>,
    pub cs: Peri<'static, PIN_9>,
    pub clk: Peri<'static, PIN_10>,
    pub mosi: Peri<'static, PIN_11>,
    pub rst: Peri<'static, PIN_12>,
    pub backlight: Peri<'static, PIN_13>,
    pub dma: Peri<'static, DMA_CH1>,
}

/// The eight active-low Game Boy buttons.
pub struct ButtonPeriphs {
    pub up: Peri<'static, PIN_21>,
    pub down: Peri<'static, PIN_22>,
    pub left: Peri<'static, PIN_26>,
    pub right: Peri<'static, PIN_27>,
    pub a: Peri<'static, PIN_0>,
    pub b: Peri<'static, PIN_1>,
    pub start: Peri<'static, PIN_2>,
    pub select: Peri<'static, PIN_3>,
}

/// MAX98357A I2S amplifier, driven by a PIO0 state machine.
pub struct AudioPeriphs {
    pub pio: Peri<'static, PIO0>,
    pub dma: Peri<'static, DMA_CH0>,
    /// GP16: I2S data out.
    pub data: Peri<'static, PIN_16>,
    /// GP14: bit clock.
    pub bclk: Peri<'static, PIN_14>,
    /// GP15: left/right (word select) clock.
    pub lrclk: Peri<'static, PIN_15>,
}

/// All CYW43439 peripheral tokens, bundled for clean transfer from `main` into
/// the app state and ultimately into `WifiPortalScreen::start_portal`.
pub struct WifiPeriphs {
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
// Board
// ---------------------------------------------------------------------------

/// Every peripheral the firmware uses, grouped by subsystem.
///
/// Destructure by moving fields out; using a group twice is a compile error.
pub struct Board {
    pub flash: Peri<'static, FLASH>,
    pub watchdog: Peri<'static, WATCHDOG>,
    pub core1: Peri<'static, CORE1>,
    pub sd: SdPeriphs,
    /// GP20: gates the SD module's VCC (HIGH = powered, LOW = off). Separate
    /// from `sd` because boot sequencing toggles it around display init,
    /// before the card driver exists.
    pub sd_power: Peri<'static, PIN_20>,
    pub display: DisplayPeriphs,
    pub buttons: ButtonPeriphs,
    pub audio: AudioPeriphs,
    pub wifi: WifiPeriphs,
}

impl Board {
    /// System clock the selected profile runs the chip at, in Hz.
    pub const fn sys_hz() -> u32 {
        CLOCK.hz
    }

    /// Configure clocks and core voltage from the selected profile and take
    /// the HAL peripherals. Call once, after the core-0 memory guards and the
    /// heap are set up.
    pub fn init() -> Self {
        let mut clk =
            ClockConfig::system_freq(CLOCK.hz).expect("valid PLL params for target clock");
        clk.core_voltage = CLOCK.voltage;
        let p = embassy_rp::init(embassy_rp::config::Config::new(clk));

        Self {
            flash: p.FLASH,
            watchdog: p.WATCHDOG,
            core1: p.CORE1,
            sd: SdPeriphs {
                spi: p.SPI0,
                miso: p.PIN_4,
                cs: p.PIN_5,
                clk: p.PIN_6,
                mosi: p.PIN_7,
                mode: p.PIN_17,
            },
            sd_power: p.PIN_20,
            display: DisplayPeriphs {
                spi: p.SPI1,
                dc: p.PIN_8,
                cs: p.PIN_9,
                clk: p.PIN_10,
                mosi: p.PIN_11,
                rst: p.PIN_12,
                backlight: p.PIN_13,
                dma: p.DMA_CH1,
            },
            buttons: ButtonPeriphs {
                up: p.PIN_21,
                down: p.PIN_22,
                left: p.PIN_26,
                right: p.PIN_27,
                a: p.PIN_0,
                b: p.PIN_1,
                start: p.PIN_2,
                select: p.PIN_3,
            },
            audio: AudioPeriphs {
                pio: p.PIO0,
                dma: p.DMA_CH0,
                data: p.PIN_16,
                bclk: p.PIN_14,
                lrclk: p.PIN_15,
            },
            wifi: WifiPeriphs {
                pwr: p.PIN_23,
                dio: p.PIN_24,
                cs: p.PIN_25,
                clk: p.PIN_29,
                pio1: p.PIO1,
                dma_ch2: p.DMA_CH2,
                dma_ch3: p.DMA_CH3,
                pio_irqs: WifiIrqs,
                dma_irqs: Irqs,
            },
        }
    }
}
