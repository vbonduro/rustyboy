//! Integration tests using Blargg's oam_bug test ROMs.
//!
//! Tests OAM corruption behavior on DMG when 16-bit inc/dec
//! targets the FE00-FEFF range during visible scanlines.

mod common;

use common::assert_blargg_passed;

#[test]
#[ignore] // Requires PPU implementation
fn test_oam_bug_01_lcd_sync() {
    assert_blargg_passed(
        "roms/blargg/oam_bug/individual/1-lcd_sync.gb",
        "1-lcd_sync",
    );
}

#[test]
#[ignore] // Requires PPU implementation
fn test_oam_bug_02_causes() {
    assert_blargg_passed(
        "roms/blargg/oam_bug/individual/2-causes.gb",
        "2-causes",
    );
}

#[test]
#[ignore] // Requires PPU implementation
fn test_oam_bug_03_non_causes() {
    assert_blargg_passed(
        "roms/blargg/oam_bug/individual/3-non_causes.gb",
        "3-non_causes",
    );
}

#[test]
#[ignore] // Requires PPU implementation
fn test_oam_bug_04_scanline_timing() {
    assert_blargg_passed(
        "roms/blargg/oam_bug/individual/4-scanline_timing.gb",
        "4-scanline_timing",
    );
}

#[test]
#[ignore] // Requires PPU implementation
fn test_oam_bug_05_timing_bug() {
    assert_blargg_passed(
        "roms/blargg/oam_bug/individual/5-timing_bug.gb",
        "5-timing_bug",
    );
}

#[test]
#[ignore] // Requires PPU implementation
fn test_oam_bug_06_timing_no_bug() {
    assert_blargg_passed(
        "roms/blargg/oam_bug/individual/6-timing_no_bug.gb",
        "6-timing_no_bug",
    );
}

#[test]
#[ignore] // Requires PPU implementation
fn test_oam_bug_07_timing_effect() {
    assert_blargg_passed(
        "roms/blargg/oam_bug/individual/7-timing_effect.gb",
        "7-timing_effect",
    );
}

#[test]
#[ignore] // Requires PPU implementation
fn test_oam_bug_08_instr_effect() {
    assert_blargg_passed(
        "roms/blargg/oam_bug/individual/8-instr_effect.gb",
        "8-instr_effect",
    );
}
