//! Integration test using Blargg's instr_timing test ROM.
//!
//! Verifies cycle count accuracy for every CPU instruction.

mod common;

use common::assert_blargg_passed;

#[test]
#[ignore] // Requires cycle-accurate instruction timing
fn test_blargg_instr_timing() {
    assert_blargg_passed(
        "roms/blargg/instr_timing/instr_timing.gb",
        "instr_timing",
    );
}
