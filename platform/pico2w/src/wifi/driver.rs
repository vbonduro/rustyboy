//! CYW43439 WiFi driver initialisation for the Raspberry Pi Pico 2W.
//!
//! # CYW43439 pin assignments (Pico 2W internal)
//!
//! | Signal | GPIO |
//! |--------|------|
//! | WL_ON  | GP23 |
//! | WL_D   | GP24 |
//! | WL_CS  | GP25 |
//! | WL_CLK | GP29 |
//!
//! The driver uses **PIO1** (PIO0 is reserved for I2S audio).
//! Bind `PIO1_IRQ_0` in your `bind_interrupts!` block and pass the token here.

use cyw43::aligned_bytes;
use cyw43_pio::PioSpi;
use defmt::info;
use embassy_net::{Config, Stack, StackResources, StaticConfigV4};
use embassy_rp::clocks::{clk_sys_freq, RoscRng};
use embassy_rp::dma::{self, Channel};
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH2, DMA_CH3, PIN_23, PIN_24, PIN_25, PIN_29, PIO1};
use embassy_rp::pio::{InterruptHandler as PioIrqHandler, Pio};
use embassy_rp::Peri;
use fixed::types::extra::U8;
use fixed::FixedU32;
use static_cell::StaticCell;

// Firmware blobs embedded at build time from the `cyw43-firmware/` directory.
static CYW43_FW: &cyw43::Aligned<cyw43::A4, [u8]> =
    aligned_bytes!("../../cyw43-firmware/43439A0.bin");
static CYW43_CLM: &cyw43::Aligned<cyw43::A4, [u8]> =
    aligned_bytes!("../../cyw43-firmware/43439A0_clm.bin");
static CYW43_NVRAM: &cyw43::Aligned<cyw43::A4, [u8]> =
    aligned_bytes!("../../cyw43-firmware/nvram_rp2040.bin");

// AP configuration.
pub const AP_SSID: &str = "RustyBoy";
pub const AP_CHANNEL: u8 = 6;
/// AP gateway / DHCP server address.
pub const AP_IP_OCTETS: [u8; 4] = [192, 168, 4, 1];
pub const AP_PREFIX_LEN: u8 = 24;

fn ap_ip() -> embassy_net::Ipv4Address {
    embassy_net::Ipv4Address::new(
        AP_IP_OCTETS[0],
        AP_IP_OCTETS[1],
        AP_IP_OCTETS[2],
        AP_IP_OCTETS[3],
    )
}

// Static storage for the CYW43 state and network stack resources.
// These must live for 'static since the driver tasks borrow them.
static CYW43_STATE: StaticCell<cyw43::State> = StaticCell::new();
// Concurrent sockets: DHCP (UDP/67), DNS (UDP/53), HTTP (TCP/80), plus headroom
// for a TCP socket lingering in TIME_WAIT while the next accept starts.
static NET_RESOURCES: StaticCell<StackResources<6>> = StaticCell::new();

/// Concrete type of the CYW43 PIO SPI bus on PIO1/SM0.
pub type Cyw43Spi = PioSpi<'static, PIO1, 0>;

/// Concrete runner type returned by [`cyw43::new`].
pub type Cyw43Runner =
    cyw43::Runner<'static, cyw43::SpiBus<Output<'static>, Cyw43Spi>, cyw43::Cyw43439>;

/// Initialise the CYW43439 driver and embassy-net stack.
///
/// # Safety
/// Call once; the `'static` resources are claimed here.
///
/// Returns `(net_stack, control, cyw43_runner, net_runner)`.
/// Spawn the two runner tasks immediately after returning.
pub async fn init(
    pwr_pin: Peri<'static, PIN_23>,
    dio_pin: Peri<'static, PIN_24>,
    cs_pin: Peri<'static, PIN_25>,
    clk_pin: Peri<'static, PIN_29>,
    pio1: Peri<'static, PIO1>,
    dma_ch2: Peri<'static, DMA_CH2>,
    dma_ch3: Peri<'static, DMA_CH3>,
    pio_irqs: impl embassy_rp::interrupt::typelevel::Binding<
            <PIO1 as embassy_rp::pio::Instance>::Interrupt,
            PioIrqHandler<PIO1>,
        > + Copy
        + 'static,
    dma_irqs: impl embassy_rp::interrupt::typelevel::Binding<
            embassy_rp::interrupt::typelevel::DMA_IRQ_0,
            dma::InterruptHandler<DMA_CH2>,
        > + embassy_rp::interrupt::typelevel::Binding<
            embassy_rp::interrupt::typelevel::DMA_IRQ_0,
            dma::InterruptHandler<DMA_CH3>,
        > + Copy
        + 'static,
) -> (
    Stack<'static>,
    cyw43::Control<'static>,
    Cyw43Runner,
    embassy_net::Runner<'static, cyw43::NetDriver<'static>>,
) {
    let pwr = Output::new(pwr_pin, Level::Low);
    let cs = Output::new(cs_pin, Level::High);

    // CYW43 PIO-SPI clock divider.
    //
    // The PIO clocks the GSPI bus at half the effective PIO frequency
    // (clk_sys / divider).  The CYW43439's manufacturer-recommended max GSPI
    // clock is 50 MHz.  cyw43-pio's `DEFAULT_CLOCK_DIVIDER` (2.0) assumes a
    // ~150 MHz clock; at our overclocked 300 MHz clk_sys it would yield a
    // 150 MHz PIO / 75 MHz GSPI bus — far over spec — which corrupts the SPI
    // link (manifests as a chip-ID mismatch and a firmware-checksum failure on
    // download, then a wild-PC fault).  Derive the divider from the actual
    // clk_sys so we target <= 75 MHz PIO / <= 37.5 MHz GSPI regardless of the
    // selected overclock (oc-266 / oc-280 / oc-300).
    const PIO_TARGET_HZ: u32 = 75_000_000;
    let divider = clk_sys_freq().div_ceil(PIO_TARGET_HZ).max(1);
    let clock_divider = FixedU32::<U8>::from_num(divider);
    info!(
        "wifi: clk_sys {} Hz -> PIO-SPI divider {} ({} Hz GSPI)",
        clk_sys_freq(),
        divider,
        clk_sys_freq() / divider / 2,
    );

    let mut pio = Pio::new(pio1, pio_irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        clock_divider,
        pio.irq0,
        cs,
        dio_pin,
        clk_pin,
        Channel::new(dma_ch2, dma_irqs),
        Channel::new(dma_ch3, dma_irqs),
    );

    let state = CYW43_STATE.init(cyw43::State::new());
    // `cyw43::new` downloads the firmware + NVRAM blobs synchronously here, so
    // those complete without the runner task.  The CLM download and every other
    // control ioctl, however, are serviced by `runner.run()` — so the caller
    // MUST spawn the cyw43 runner task before calling [`configure`], [`scan_ssids`]
    // or [`start_ap`], otherwise those ioctls block forever waiting on a runner
    // that isn't polled.
    let (net_device, control, runner) = cyw43::new(state, pwr, spi, CYW43_FW, CYW43_NVRAM).await;

    let mut rng = RoscRng;
    let seed = rng.next_u64();

    // Static IP for the AP.
    let config = Config::ipv4_static(StaticConfigV4 {
        address: embassy_net::Ipv4Cidr::new(ap_ip(), AP_PREFIX_LEN),
        dns_servers: Default::default(),
        gateway: None,
    });

    let resources = NET_RESOURCES.init(StackResources::new());
    let (stack, net_runner) = embassy_net::new(net_device, config, resources, seed);
    info!("wifi: driver init complete");
    (stack, control, runner, net_runner)
}

/// Download the CLM blob and configure power management.
///
/// Must be called **after** the cyw43 runner task has been spawned (it issues
/// ioctls serviced by `runner.run()`), and **before** [`scan_ssids`] /
/// [`start_ap`].
pub async fn configure(control: &mut cyw43::Control<'_>) {
    control.init(CYW43_CLM).await;
    control
        .set_power_management(cyw43::PowerManagementMode::None)
        .await;
    info!("wifi: CLM loaded, power management configured");
}

/// Scan for available SSIDs and return up to 16.
///
/// Must be called **before** `start_ap` — the CYW43439 cannot scan while in AP
/// mode without complex firmware tricks.
pub async fn scan_ssids(
    control: &mut cyw43::Control<'_>,
) -> heapless::Vec<heapless::String<32>, 16> {
    let mut results: heapless::Vec<heapless::String<32>, 16> = heapless::Vec::new();
    let mut scanner = control.scan(Default::default()).await;
    while let Some(bss) = scanner.next().await {
        if let Ok(s) = core::str::from_utf8(&bss.ssid[..bss.ssid_len as usize]) {
            if !s.is_empty()
                && !results
                    .iter()
                    .any(|r: &heapless::String<32>| r.as_str() == s)
            {
                if let Ok(hs) = heapless::String::try_from(s) {
                    let _ = results.push(hs);
                    if results.is_full() {
                        break;
                    }
                }
            }
        }
    }
    info!("wifi: scan found {} SSIDs", results.len());
    results
}

/// Start the CYW43439 in open AP mode with SSID "RustyBoy".
pub async fn start_ap(control: &mut cyw43::Control<'_>) {
    control.start_ap_open(AP_SSID, AP_CHANNEL).await;
    info!("wifi: AP '{}' started on channel {}", AP_SSID, AP_CHANNEL);
}
