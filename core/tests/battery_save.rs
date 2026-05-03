//! Integration tests for cartridge external-RAM (battery-save) read/write.

mod common;

use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::sm83::Sm83;
use rustyboy_core::memory::memory::GameBoyMemory;

/// Build a minimal in-memory ROM with a NOP + JR -2 loop at 0x0100.
fn make_rom(cart_type: u8, rom_size_code: u8, ram_size_code: u8) -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000]; // 32 KB, 2 banks
    rom[0x0147] = cart_type;
    rom[0x0148] = rom_size_code; // 0 = 32 KB (2 banks)
    rom[0x0149] = ram_size_code; // 2 = 8 KB
    // NOP + JR -2 infinite loop at 0x0100
    rom[0x0100] = 0x00; // NOP
    rom[0x0101] = 0x18; // JR
    rom[0x0102] = 0xFE; // -2
    rom
}

/// Create a fresh Sm83 with DMG post-boot register state.
fn make_emulator(rom: Vec<u8>) -> Sm83 {
    let memory = Box::new(GameBoyMemory::with_rom(rom));
    Sm83::new(memory)
        .with_registers(Registers {
            a: 0x01,
            f: Flags::from_bits_truncate(0xB0),
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            pc: 0x0100,
            sp: 0xFFFE,
        })
        .with_dmg_state()
}

// ── NoMbc has no external RAM ────────────────────────────────────────────────

#[test]
fn test_no_mbc_has_no_external_ram() {
    // Cart type 0x00 = ROM only (NoMbc), RAM size code 0 = no RAM
    let rom = make_rom(0x00, 0, 0);
    let cpu = make_emulator(rom);

    // The WASM binding returns `cpu.external_ram().map(|s| s.to_vec()).unwrap_or_default()`
    // so an empty Vec signals no external RAM.
    let ram = cpu.external_ram().map(|s| s.to_vec()).unwrap_or_default();
    assert!(
        ram.is_empty(),
        "NoMbc cart should have no external RAM, got {} bytes",
        ram.len()
    );
}

// ── MBC1 external RAM roundtrip ──────────────────────────────────────────────

#[test]
fn test_mbc1_external_ram_roundtrip() {
    // Cart type 0x03 = MBC1+RAM+BATTERY, RAM size 0x02 = 8 KB
    let rom = make_rom(0x03, 0, 0x02);
    let mut cpu = make_emulator(rom);

    // Verify RAM is present
    assert!(
        cpu.external_ram().is_some(),
        "MBC1+RAM cart should expose external RAM"
    );
    let ram_len = cpu.external_ram().unwrap().len();
    assert_eq!(ram_len, 8 * 1024, "MBC1 RAM size should be 8 KB");

    // Write a known pattern
    let mut data = vec![0u8; ram_len];
    for (i, b) in data.iter_mut().enumerate() {
        *b = (i & 0xFF) as u8;
    }
    cpu.set_external_ram(&data);

    // Read back and verify
    let readback = cpu.external_ram().expect("external_ram should still be Some");
    assert_eq!(readback.len(), ram_len, "RAM length changed after set_external_ram");
    for (i, (&expected, &actual)) in data.iter().zip(readback.iter()).enumerate() {
        assert_eq!(
            actual, expected,
            "MBC1 external RAM mismatch at byte {}: got {:#04x}, want {:#04x}",
            i, actual, expected
        );
    }
}

// ── MBC3 external RAM roundtrip ──────────────────────────────────────────────

#[test]
fn test_mbc3_external_ram_roundtrip() {
    // Cart type 0x13 = MBC3+RAM+BATTERY, RAM size 0x02 = 8 KB
    let rom = make_rom(0x13, 0, 0x02);
    let mut cpu = make_emulator(rom);

    assert!(
        cpu.external_ram().is_some(),
        "MBC3+RAM cart should expose external RAM"
    );
    let ram_len = cpu.external_ram().unwrap().len();
    assert_eq!(ram_len, 8 * 1024, "MBC3 RAM size should be 8 KB");

    // Write a known pattern (XOR-based so it's not all zeros)
    let mut data = vec![0u8; ram_len];
    for (i, b) in data.iter_mut().enumerate() {
        *b = ((i ^ 0xA5) & 0xFF) as u8;
    }
    cpu.set_external_ram(&data);

    let readback = cpu.external_ram().expect("external_ram should still be Some after write");
    assert_eq!(readback.len(), ram_len);
    for (i, (&expected, &actual)) in data.iter().zip(readback.iter()).enumerate() {
        assert_eq!(
            actual, expected,
            "MBC3 external RAM mismatch at byte {}: got {:#04x}, want {:#04x}",
            i, actual, expected
        );
    }
}

// ── Partial write does not panic ──────────────────────────────────────────────

#[test]
fn test_external_ram_partial_write() {
    // MBC1+RAM+BATTERY, 8 KB RAM
    let rom = make_rom(0x03, 0, 0x02);
    let mut cpu = make_emulator(rom);

    let ram_len = cpu.external_ram().unwrap().len();

    // Write only the first 64 bytes — should not panic
    let partial: Vec<u8> = (0u8..64u8).collect();
    cpu.set_external_ram(&partial);

    let readback = cpu.external_ram().expect("external_ram should be Some after partial write");

    // The written bytes should match
    for (i, (&expected, &actual)) in partial.iter().zip(readback.iter()).enumerate() {
        assert_eq!(
            actual, expected,
            "partial write: byte {} mismatch: got {:#04x}, want {:#04x}",
            i, actual, expected
        );
    }

    // The rest of the RAM should still be accessible (no panic, no out-of-bounds)
    assert_eq!(readback.len(), ram_len, "RAM length should be unchanged after partial write");
}
