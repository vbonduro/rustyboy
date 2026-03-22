//! Integration tests using Blargg's mem_timing test ROMs.
//!
//! Verifies memory read/write timing for all instructions.

mod common;

use common::assert_blargg_passed;

#[test]
#[ignore] // Requires cycle-accurate memory timing
fn test_blargg_mem_timing_01_read() {
    assert_blargg_passed(
        "roms/blargg/mem_timing/individual/01-read_timing.gb",
        "01-read_timing",
    );
}

#[test]
#[ignore] // Requires cycle-accurate memory timing
fn test_blargg_mem_timing_02_write() {
    assert_blargg_passed(
        "roms/blargg/mem_timing/individual/02-write_timing.gb",
        "02-write_timing",
    );
}

#[test]
#[ignore] // Requires cycle-accurate memory timing
fn test_blargg_mem_timing_03_modify() {
    assert_blargg_passed(
        "roms/blargg/mem_timing/individual/03-modify_timing.gb",
        "03-modify_timing",
    );
}
