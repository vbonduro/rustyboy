//! Integration test using dmg-acid2 PPU test ROM.
//!
//! This test renders the ROM and compares the framebuffer output
//! against a known-good reference image (roms/dmg-acid2/reference.png).
//! A correct PPU implementation produces a smiley face; any bug distorts it.

mod common;

use common::{load_reference_png, run_rom_frames};

#[test]
fn test_dmg_acid2() {
    // Run for ~10 seconds of emulated time (~600 frames) to let the test ROM finish rendering
    let framebuffer = run_rom_frames("roms/dmg-acid2/dmg-acid2.gb", 600);
    let reference = load_reference_png("roms/dmg-acid2/reference.png");

    assert_eq!(
        framebuffer.len(),
        reference.len(),
        "Framebuffer size mismatch: got {} expected {}",
        framebuffer.len(),
        reference.len()
    );

    let mut mismatches = 0;
    for (i, (got, expected)) in framebuffer.iter().zip(reference.iter()).enumerate() {
        if got != expected {
            mismatches += 1;
            if mismatches <= 10 {
                let x = i % 160;
                let y = i / 160;
                eprintln!(
                    "Pixel ({},{}) mismatch: got shade {} expected shade {}",
                    x, y, got, expected
                );
            }
        }
    }

    assert_eq!(
        mismatches, 0,
        "dmg-acid2: {} pixel mismatches out of {} total",
        mismatches,
        framebuffer.len()
    );
}
