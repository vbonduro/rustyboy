//! Integration tests using Blargg's cpu_instrs test ROMs.
//!
//! These tests load a real Game Boy ROM, run it to completion,
//! and assert the expected serial output.

mod common;

use common::assert_blargg_passed;

// --- Passing tests ---

#[test]
fn test_blargg_01_special() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/01-special.gb",
        "01-special",
    );
}

#[test]
fn test_blargg_04_op_r_imm() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/04-op r,imm.gb",
        "04-op r,imm",
    );
}

#[test]
fn test_blargg_06_ld_r_r() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/06-ld r,r.gb",
        "06-ld r,r",
    );
}

#[test]
fn test_blargg_07_jr_jp_call_ret_rst() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/07-jr,jp,call,ret,rst.gb",
        "07-jr,jp,call,ret,rst",
    );
}

#[test]
fn test_blargg_08_misc_instrs() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/08-misc instrs.gb",
        "08-misc instrs",
    );
}

#[test]
fn test_blargg_09_op_r_r() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/09-op r,r.gb",
        "09-op r,r",
    );
}

#[test]
fn test_blargg_10_bit_ops() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/10-bit ops.gb",
        "10-bit ops",
    );
}

#[test]
fn test_blargg_11_op_a_hl() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/11-op a,(hl).gb",
        "11-op a,(hl)",
    );
}

// --- Known failing tests (red phase for future TDD) ---

#[test]
fn test_blargg_02_interrupts() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/02-interrupts.gb",
        "02-interrupts",
    );
}

#[test]
fn test_blargg_03_op_sp_hl() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/03-op sp,hl.gb",
        "03-op sp,hl",
    );
}

#[test]
fn test_blargg_05_op_rp() {
    assert_blargg_passed(
        "roms/blargg/cpu_instrs/individual/05-op rp.gb",
        "05-op rp",
    );
}
