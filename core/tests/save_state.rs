//! Integration tests for Sm83 save-state serialization / deserialization.

mod common;

use rustyboy_core::cpu::cpu::Cpu;
use rustyboy_core::cpu::instructions::opcodes::OpCodeDecoder;
use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::sm83::Sm83;
use rustyboy_core::memory::memory::GameBoyMemory;

/// Build a minimal in-memory ROM with a NOP + JR -2 loop at 0x0100.
fn make_rom(cart_type: u8, rom_size_code: u8, ram_size_code: u8) -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000]; // 32 KB, 2 banks
    rom[0x0147] = cart_type;
    rom[0x0148] = rom_size_code; // 0 = 32 KB (2 banks)
    rom[0x0149] = ram_size_code;
    // NOP + JR -2 infinite loop at 0x0100
    rom[0x0100] = 0x00; // NOP
    rom[0x0101] = 0x18; // JR
    rom[0x0102] = 0xFE; // -2
    rom
}

/// Create a fresh Sm83 with DMG post-boot register state.
fn make_emulator(rom: Vec<u8>) -> Sm83 {
    let memory = Box::new(GameBoyMemory::with_rom(rom));
    let decoder = Box::new(OpCodeDecoder::new());
    Sm83::new(memory, decoder)
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

// ── Basic roundtrip ───────────────────────────────────────────────────────────

#[test]
fn test_save_state_roundtrip_basic() {
    let rom = make_rom(0x00, 0, 0);
    let mut cpu = make_emulator(rom.clone());

    // Advance emulator state before saving
    for _ in 0..1000 {
        cpu.tick().unwrap();
    }

    let state = cpu.save_state();
    let regs_before = cpu.registers();

    // Restore into a fresh emulator from the same ROM
    let mut cpu2 = make_emulator(rom);
    cpu2.load_state(&state).expect("load_state failed");

    let regs_after = cpu2.registers();
    assert_eq!(regs_before.a, regs_after.a, "register A mismatch");
    assert_eq!(regs_before.b, regs_after.b, "register B mismatch");
    assert_eq!(regs_before.c, regs_after.c, "register C mismatch");
    assert_eq!(regs_before.d, regs_after.d, "register D mismatch");
    assert_eq!(regs_before.e, regs_after.e, "register E mismatch");
    assert_eq!(regs_before.h, regs_after.h, "register H mismatch");
    assert_eq!(regs_before.l, regs_after.l, "register L mismatch");
    assert_eq!(regs_before.sp, regs_after.sp, "register SP mismatch");
    assert_eq!(regs_before.pc, regs_after.pc, "register PC mismatch");
    assert_eq!(regs_before.f, regs_after.f, "flags mismatch");
}

// ── PC preserved ─────────────────────────────────────────────────────────────

#[test]
fn test_save_state_pc_preserved() {
    let rom = make_rom(0x00, 0, 0);
    let mut cpu = make_emulator(rom.clone());

    // Run enough ticks that the PC has cycled through the NOP+JR loop a few times
    for _ in 0..500 {
        cpu.tick().unwrap();
    }

    let pc_before = cpu.registers().pc;
    let state = cpu.save_state();

    let mut cpu2 = make_emulator(rom);
    cpu2.load_state(&state).expect("load_state failed");

    assert_eq!(pc_before, cpu2.registers().pc, "PC not preserved across save/load");
}

// ── WRAM preserved ───────────────────────────────────────────────────────────

#[test]
fn test_save_state_wram_preserved() {
    let rom = make_rom(0x00, 0, 0);
    let cpu = make_emulator(rom.clone());

    // Write known pattern into WRAM via the save-state blob.
    // WRAM occupies bytes [164..164+0x2000] in the blob.
    let mut state = cpu.save_state();
    let wram_offset: usize = 164;
    let pattern: [u8; 16] = [0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x23, 0x45, 0x67,
                              0x89, 0xAB, 0xCD, 0xEF, 0x10, 0x20, 0x30, 0x40];
    state[wram_offset..wram_offset + 16].copy_from_slice(&pattern);

    // Load the patched state into a fresh emulator
    let mut cpu2 = make_emulator(rom);
    cpu2.load_state(&state).expect("load_state failed");

    // Read back WRAM via the memory bus (0xC000 maps to WRAM base)
    for (i, &expected) in pattern.iter().enumerate() {
        let addr = 0xC000u16 + i as u16;
        let actual = cpu2.read_memory(addr).unwrap_or_else(|_| panic!("read_memory failed at {:#06x}", addr));
        assert_eq!(actual, expected, "WRAM byte at {:#06x} mismatch: got {:#04x}, want {:#04x}", addr, actual, expected);
    }
}

// ── Invalid magic ─────────────────────────────────────────────────────────────

#[test]
fn test_save_state_invalid_magic() {
    let rom = make_rom(0x00, 0, 0);
    let mut cpu = make_emulator(rom);

    // Build a blob of the right length but with wrong magic
    let mut garbage = vec![0xFFu8; 16835];
    garbage[0] = b'X';
    garbage[1] = b'X';
    garbage[2] = b'X';
    garbage[3] = b'X';

    let result = cpu.load_state(&garbage);
    assert!(result.is_err(), "expected Err for invalid magic, got Ok");
}

// ── Too short ─────────────────────────────────────────────────────────────────

#[test]
fn test_save_state_too_short() {
    let rom = make_rom(0x00, 0, 0);
    let mut cpu = make_emulator(rom);

    let short_blob = [0u8; 10];
    let result = cpu.load_state(&short_blob);
    assert!(result.is_err(), "expected Err for too-short blob, got Ok");
}

// ── Cycle counter preserved ───────────────────────────────────────────────────

#[test]
fn test_save_state_roundtrip_preserves_cycle_counter() {
    let rom = make_rom(0x00, 0, 0);
    let mut cpu = make_emulator(rom.clone());

    // Run a known number of ticks to accumulate cycles
    for _ in 0..2000 {
        cpu.tick().unwrap();
    }

    let cycles_before = cpu.cycle_counter();
    assert!(cycles_before > 0, "cycle_counter should be non-zero after ticking");

    let state = cpu.save_state();

    let mut cpu2 = make_emulator(rom);
    cpu2.load_state(&state).expect("load_state failed");

    assert_eq!(cycles_before, cpu2.cycle_counter(), "cycle_counter not preserved across save/load");
}
