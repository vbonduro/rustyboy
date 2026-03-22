//! Integration test using dmg-acid2 PPU test ROM.
//!
//! This test renders the ROM and compares the framebuffer output
//! against a known-good reference image (roms/dmg-acid2/reference.png).
//! A correct PPU implementation produces a smiley face; any bug distorts it.
//!
//! Unlike Blargg/Mooneye tests, this requires:
//! 1. A working PPU that produces a 160x144 framebuffer
//! 2. Pixel-by-pixel comparison against the reference image

#[test]
#[ignore] // Requires PPU implementation and framebuffer comparison
fn test_dmg_acid2() {
    // TODO: Once the PPU is implemented:
    // 1. Run the ROM for a fixed number of frames (a few seconds)
    // 2. Capture the PPU framebuffer
    // 3. Compare against roms/dmg-acid2/reference.png
    panic!("PPU not yet implemented — cannot run dmg-acid2");
}
