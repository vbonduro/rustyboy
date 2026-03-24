//! Integration tests using Mooneye's MBC1 test ROMs.
//!
//! These tests are from the emulator-only suite, specifically
//! designed for testing MBC1 memory bank controller implementations.

mod common;

use common::assert_mooneye_passed;

#[test]
fn test_mooneye_mbc1_bits_bank1() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/bits_bank1.gb",
        "mbc1/bits_bank1",
    );
}

#[test]
fn test_mooneye_mbc1_bits_bank2() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/bits_bank2.gb",
        "mbc1/bits_bank2",
    );
}

#[test]
fn test_mooneye_mbc1_bits_mode() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/bits_mode.gb",
        "mbc1/bits_mode",
    );
}

#[test]
fn test_mooneye_mbc1_bits_ramg() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/bits_ramg.gb",
        "mbc1/bits_ramg",
    );
}

#[test]
fn test_mooneye_mbc1_ram_64kb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/ram_64kb.gb",
        "mbc1/ram_64kb",
    );
}

#[test]
fn test_mooneye_mbc1_ram_256kb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/ram_256kb.gb",
        "mbc1/ram_256kb",
    );
}

#[test]
fn test_mooneye_mbc1_rom_512kb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/rom_512kb.gb",
        "mbc1/rom_512kb",
    );
}

#[test]
fn test_mooneye_mbc1_rom_1mb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/rom_1Mb.gb",
        "mbc1/rom_1Mb",
    );
}

#[test]
fn test_mooneye_mbc1_rom_2mb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/rom_2Mb.gb",
        "mbc1/rom_2Mb",
    );
}

#[test]
fn test_mooneye_mbc1_rom_4mb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/rom_4Mb.gb",
        "mbc1/rom_4Mb",
    );
}

#[test]
fn test_mooneye_mbc1_rom_8mb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/rom_8Mb.gb",
        "mbc1/rom_8Mb",
    );
}

#[test]
fn test_mooneye_mbc1_rom_16mb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/rom_16Mb.gb",
        "mbc1/rom_16Mb",
    );
}

#[test]
fn test_mooneye_mbc1_multicart_rom_8mb() {
    assert_mooneye_passed(
        "roms/mooneye/emulator-only/mbc1/multicart_rom_8Mb.gb",
        "mbc1/multicart_rom_8Mb",
    );
}
