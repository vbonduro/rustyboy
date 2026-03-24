//! Integration tests using Blargg's dmg_sound test ROMs.
//!
//! Tests all 4 APU sound channels: registers, length counters,
//! triggers, sweep, envelope, and timing.
//!
//! These ROMs output results to memory at 0xA000, not serial.

mod common;

use common::assert_blargg_mem_passed;

#[test]
fn test_dmg_sound_01_registers() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/01-registers.gb",
        "01-registers",
    );
}

#[test]
fn test_dmg_sound_02_len_ctr() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/02-len ctr.gb",
        "02-len ctr",
    );
}

#[test]
fn test_dmg_sound_03_trigger() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/03-trigger.gb",
        "03-trigger",
    );
}

#[test]
fn test_dmg_sound_04_sweep() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/04-sweep.gb",
        "04-sweep",
    );
}

#[test]
fn test_dmg_sound_05_sweep_details() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/05-sweep details.gb",
        "05-sweep details",
    );
}

#[test]
fn test_dmg_sound_06_overflow_on_trigger() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/06-overflow on trigger.gb",
        "06-overflow on trigger",
    );
}

#[test]
fn test_dmg_sound_07_len_sweep_period_sync() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/07-len sweep period sync.gb",
        "07-len sweep period sync",
    );
}

#[test]
fn test_dmg_sound_08_len_ctr_during_power() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/08-len ctr during power.gb",
        "08-len ctr during power",
    );
}

#[test]
fn test_dmg_sound_09_wave_read_while_on() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/09-wave read while on.gb",
        "09-wave read while on",
    );
}

#[test]
fn test_dmg_sound_10_wave_trigger_while_on() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/10-wave trigger while on.gb",
        "10-wave trigger while on",
    );
}

#[test]
fn test_dmg_sound_11_regs_after_power() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/11-regs after power.gb",
        "11-regs after power",
    );
}

#[test]
fn test_dmg_sound_12_wave_write_while_on() {
    assert_blargg_mem_passed(
        "roms/blargg/dmg_sound/individual/12-wave write while on.gb",
        "12-wave write while on",
    );
}
