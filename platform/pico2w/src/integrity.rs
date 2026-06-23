//! Boot-time firmware image integrity guard.
//!
//! The SM83 interpreter's hot functions are placed in `.data` so they execute
//! from RAM. cortex-m-rt copies that image from flash (LMA `__sidata`) into RAM
//! at boot. A failed flash page-write in the `.data` image — which probe-rs
//! `--verify` has been observed to miss (XIP-cache false pass) — therefore boots
//! into corrupt RAM code and crashes deep in the emulator (e.g. `dispatch_isr`).
//!
//! `IMAGE_CRC` is patched post-link by the `rb-flash` runner with the
//! CRC32 of the entire flashed firmware image. On boot we CRC the same bytes
//! straight from flash (immutable at runtime, so this is safe before any
//! peripheral init and free of false positives) and compare. On mismatch we
//! exit via semihosting so `probe-rs run` returns non-zero — the user re-runs
//! `cargo run` to retry; we deliberately do not auto-reflash, to avoid needless
//! flash wear.

use cortex_m_semihosting::debug;

extern "C" {
    /// Start of the flashed firmware image (set in `memory.x`).
    static __start_block_addr: u32;
}

/// Sentinel left in place when the image was flashed without `rb-flash`
/// (e.g. via picotool). The guard is skipped so non-probe-rs flashing still
/// boots normally.
const UNPATCHED: u32 = 0xFFFF_FFFF;

/// CRC32 of the **entire** flashed firmware image (`.start_block` + `.text`
/// + `.rodata` + the `.data` load image), patched post-link by `rb-flash`.
///
/// It lives at the very end of the image, in `.end_block` — so the CRC
/// covers every byte *before* it. On boot we re-CRC the same flash bytes in
/// place (immutable, so this is safe before any peripheral init and free of
/// false positives) and compare. This supersedes the old `.data`-only guard:
/// a corrupt `.text` page — which `--verify` has been seen to miss and which
/// boots into a garbage instruction → wild HardFault — is now caught here.
#[no_mangle]
#[used]
#[link_section = ".end_block"]
pub static IMAGE_CRC: u32 = UNPATCHED;

pub fn verify_image() {
    let want = unsafe { core::ptr::read_volatile(core::ptr::addr_of!(IMAGE_CRC)) };
    if want == UNPATCHED {
        return;
    }
    let got = image_crc();
    if got != want {
        defmt::error!(
            "IMAGE CORRUPT: crc {=u32:#010x} != expected {=u32:#010x} — reflash (cargo run)",
            got,
            want
        );
        // Clean exit so the run command fails without faulting (keeps the
        // crash log free of a spurious record). probe-rs reports non-zero.
        loop {
            debug::exit(debug::EXIT_FAILURE);
        }
    }
    defmt::info!("integrity: full image crc {=u32:#010x} OK", got);
}

fn image_crc() -> u32 {
    let start = core::ptr::addr_of!(__start_block_addr) as *const u8;
    // The image ends right before the CRC word (IMAGE_CRC sits at the start
    // of `.end_block`, i.e. at `__end_block_addr`), so [start, &IMAGE_CRC)
    // is exactly the CRC-covered region.
    let end = core::ptr::addr_of!(IMAGE_CRC) as usize;
    let len = end - (start as usize);
    // Safety: [start, end) is the flashed image in XIP flash, produced by
    // the linker and read-only at runtime.
    let bytes = unsafe { core::slice::from_raw_parts(start, len) };
    crc32(bytes)
}

/// CRC-32/ISO-HDLC (reflected, poly 0xEDB88320). Must stay byte-for-byte
/// identical to the implementation in the `rb-flash` runner.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
