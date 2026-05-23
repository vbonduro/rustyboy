//! Integration tests for save-state serialization / deserialization.

mod common;

use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::save_state::{SaveState, VERSION_V1, VERSION_V2};
use rustyboy_core::GameBoy;

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

/// Create a fresh GameBoy with DMG post-boot register state.
fn make_emulator(rom: Vec<u8>) -> GameBoy {
    GameBoy::new(rom)
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
    let mut gb = make_emulator(rom.clone());

    // Advance emulator state before saving
    for _ in 0..1000 {
        gb.step().unwrap();
    }

    let state = gb.save_state();
    let regs_before = gb.registers();

    // Restore into a fresh emulator from the same ROM
    let mut gb2 = make_emulator(rom);
    gb2.load_state(SaveState::from_blob(state).expect("from_blob failed"))
        .expect("load_state failed");

    let regs_after = gb2.registers();
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
    let mut gb = make_emulator(rom.clone());

    // Run enough ticks that the PC has cycled through the NOP+JR loop a few times
    for _ in 0..500 {
        gb.step().unwrap();
    }

    let pc_before = gb.registers().pc;
    let state = gb.save_state();

    let mut gb2 = make_emulator(rom);
    gb2.load_state(SaveState::from_blob(state).expect("from_blob failed"))
        .expect("load_state failed");

    assert_eq!(
        pc_before,
        gb2.registers().pc,
        "PC not preserved across save/load"
    );
}

// ── WRAM preserved ───────────────────────────────────────────────────────────

#[test]
fn test_save_state_wram_preserved() {
    let rom = make_rom(0x00, 0, 0);
    let gb = make_emulator(rom.clone());

    // Write known pattern into WRAM via the save-state blob.
    // WRAM occupies bytes [164..164+0x2000] in the blob.
    let mut state = gb.save_state();
    let wram_offset: usize = 177;
    let pattern: [u8; 16] = [
        0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0x10, 0x20, 0x30,
        0x40,
    ];
    state[wram_offset..wram_offset + 16].copy_from_slice(&pattern);

    // Load the patched state into a fresh emulator
    let mut gb2 = make_emulator(rom);
    gb2.load_state(SaveState::from_blob(state).expect("from_blob failed"))
        .expect("load_state failed");

    // Read back WRAM via the memory bus (0xC000 maps to WRAM base)
    for (i, &expected) in pattern.iter().enumerate() {
        let addr = 0xC000u16 + i as u16;
        let actual = gb2
            .read_memory(addr)
            .unwrap_or_else(|_| panic!("read_memory failed at {:#06x}", addr));
        assert_eq!(
            actual, expected,
            "WRAM byte at {:#06x} mismatch: got {:#04x}, want {:#04x}",
            addr, actual, expected
        );
    }
}

// ── Invalid magic ─────────────────────────────────────────────────────────────

#[test]
fn test_save_state_invalid_magic() {
    let mut garbage = vec![0xFFu8; 16848];
    garbage[0] = b'X';
    garbage[1] = b'X';
    garbage[2] = b'X';
    garbage[3] = b'X';

    assert!(
        SaveState::from_blob(garbage).is_err(),
        "expected Err for invalid magic, got Ok"
    );
}

// ── Too short ─────────────────────────────────────────────────────────────────

#[test]
fn test_save_state_too_short() {
    let short_blob = vec![0u8; 10];
    assert!(
        SaveState::from_blob(short_blob).is_err(),
        "expected Err for too-short blob, got Ok"
    );
}

// ── MBC bank state preserved ──────────────────────────────────────────────────

#[test]
fn test_save_state_mbc1_bank_preserved() {
    // Build a 128-bank MBC1 ROM (cart_type=0x01, rom_size_code=6 → 128 banks)
    let mut rom = vec![0u8; 128 * 0x4000];
    rom[0x0147] = 0x01; // MBC1
    rom[0x0148] = 0x06; // 128 banks
    rom[0x0149] = 0x00; // no RAM

    // Put a sentinel byte at the start of bank 7's switchable window
    rom[7 * 0x4000] = 0xAB;

    // ROM program at 0x0100:
    //   LD HL, 0x2000   ; point HL at MBC1 bank register
    //   LD (HL), 7      ; select bank 7
    //   NOP / JR -2     ; spin
    rom[0x0100] = 0x21; // LD HL, nn
    rom[0x0101] = 0x00;
    rom[0x0102] = 0x20; // 0x2000
    rom[0x0103] = 0x36; // LD (HL), n
    rom[0x0104] = 0x07; // 7
    rom[0x0105] = 0x00; // NOP
    rom[0x0106] = 0x18; // JR -2
    rom[0x0107] = 0xFE;

    let mut gb = make_emulator(rom.clone());

    // Run enough ticks to execute LD HL + LD (HL) = ~16 ticks total
    for _ in 0..30 {
        gb.step().unwrap();
    }

    // Bank 7 should now be mapped
    assert_eq!(gb.current_rom_bank(), 7, "bank 7 should be active");
    assert_eq!(
        gb.read_memory(0x4000).unwrap(),
        0xAB,
        "sentinel should be readable in bank 7"
    );

    let state = gb.save_state();

    // Fresh emulator starts at bank 1 — sentinel not visible
    let mut gb2 = make_emulator(rom);
    assert_eq!(gb2.current_rom_bank(), 1, "fresh emulator starts at bank 1");

    gb2.load_state(SaveState::from_blob(state).expect("from_blob failed"))
        .expect("load_state failed");

    // After restoring, bank 7 should be remapped
    assert_eq!(
        gb2.current_rom_bank(),
        7,
        "MBC bank not restored across save/load"
    );
    assert_eq!(
        gb2.read_memory(0x4000).unwrap(),
        0xAB,
        "sentinel should be readable after load_state"
    );
}

// ── MBC1 cart RAM preserved across save/load ─────────────────────────────────

#[test]
fn test_save_state_mbc1_cart_ram_preserved() {
    // MBC1 + RAM + Battery, 32KB ROM, 8KB RAM
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x03; // MBC1+RAM+BATTERY
    rom[0x0148] = 0x00; // 32 KB (2 banks)
    rom[0x0149] = 0x02; // 8 KB RAM (1 bank)

    // Program at 0x0100:
    //   LD A, 0x0A      ; RAM enable value
    //   LD (0x0000), A  ; enable external RAM
    //   NOP / JR -2
    rom[0x0100] = 0x3E; // LD A, n
    rom[0x0101] = 0x0A;
    rom[0x0102] = 0xEA; // LD (nn), A
    rom[0x0103] = 0x00;
    rom[0x0104] = 0x00;
    rom[0x0105] = 0x00; // NOP
    rom[0x0106] = 0x18; // JR -2
    rom[0x0107] = 0xFE;

    let mut gb = make_emulator(rom.clone());

    // Run enough ticks to execute LD A + LD (nn) = ~24 ticks
    for _ in 0..40 {
        gb.step().unwrap();
    }

    // Write a known pattern into cart RAM via set_external_ram
    let pattern: [u8; 16] = [
        0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        0x00,
    ];
    let mut ram = vec![0u8; 0x2000];
    ram[..16].copy_from_slice(&pattern);
    gb.set_external_ram(&ram);

    // Verify the write landed (RAM is enabled so bus reads work)
    for (i, &expected) in pattern.iter().enumerate() {
        let actual = gb.read_memory(0xA000 + i as u16).unwrap();
        assert_eq!(actual, expected, "pre-save cart RAM byte {i} mismatch");
    }

    let state = gb.save_state();

    // Fresh emulator — cart RAM is zeroed
    let mut gb2 = make_emulator(rom);
    gb2.load_state(SaveState::from_blob(state).expect("from_blob failed"))
        .expect("load_state failed");

    // Cart RAM must survive the round-trip
    for (i, &expected) in pattern.iter().enumerate() {
        let actual = gb2.read_memory(0xA000 + i as u16).unwrap();
        assert_eq!(
            actual, expected,
            "cart RAM byte {i} lost across save/load: got {actual:#04x}, want {expected:#04x}"
        );
    }
}

// ── SaveState: parse blob, inspect fields, then apply ────────────────────────

#[test]
fn test_save_state_struct_inspect_then_apply() {
    let rom = make_rom(0x00, 0, 0);
    let mut gb = make_emulator(rom.clone());

    for _ in 0..1000 {
        gb.step().unwrap();
    }

    let pc_before = gb.registers().pc;
    let cycles_before = gb.cycle_counter();

    // Serialize to blob, then parse into SaveState
    let blob = gb.save_state();
    let state = SaveState::from_blob(blob).expect("SaveState::from_blob failed");

    // Inspect fields before touching any emulator state
    assert_eq!(state.cpu.pc, pc_before, "SaveState pc field mismatch");
    assert_eq!(
        state.cpu.cycle_counter, cycles_before,
        "SaveState cycle_counter field mismatch"
    );

    // Apply into a fresh emulator and verify
    let mut gb2 = make_emulator(rom);
    gb2.load_state(state).expect("load_state failed");

    assert_eq!(
        gb2.registers().pc,
        pc_before,
        "PC not preserved after apply"
    );
    assert_eq!(
        gb2.cycle_counter(),
        cycles_before,
        "cycle_counter not preserved after apply"
    );
}

// ── SaveState: from_blob rejects invalid input ────────────────────────────────

#[test]
fn test_save_state_from_blob_rejects_bad_magic() {
    let mut blob = vec![0u8; 16848];
    blob[0..4].copy_from_slice(b"XXXX");
    assert!(SaveState::from_blob(blob).is_err());
}

#[test]
fn test_save_state_from_blob_rejects_too_short() {
    let blob = vec![0u8; 10];
    assert!(SaveState::from_blob(blob).is_err());
}

#[test]
fn test_save_state_v1_blob_still_loads_after_v2_support() {
    let rom = make_rom(0x03, 0, 0x02);
    let mut gb = make_emulator(rom.clone());

    for _ in 0..40 {
        gb.step().unwrap();
    }
    let mut ram = vec![0u8; 0x2000];
    ram[..4].copy_from_slice(&[0xCA, 0xFE, 0xBA, 0xBE]);
    gb.set_external_ram(&ram);

    let blob = gb.save_state_v1();
    assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), VERSION_V1);

    let parsed = SaveState::from_blob(blob).expect("v1 blob should still parse");
    assert_eq!(parsed.format_version(), VERSION_V1);
    assert_eq!(
        parsed.cart_ram().expect("cart RAM should be present")[..4],
        [0xCA, 0xFE, 0xBA, 0xBE]
    );

    let mut restored = make_emulator(rom);
    restored.load_state(parsed).expect("v1 load should work");
    assert_eq!(
        restored.external_ram().expect("restored cart RAM")[..4],
        [0xCA, 0xFE, 0xBA, 0xBE]
    );
}

#[test]
fn test_save_state_v1_mbc3_extended_tail_is_detected() {
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x13; // MBC3+RAM+BATTERY
    rom[0x0148] = 0x00;
    rom[0x0149] = 0x02; // 8 KiB RAM
    rom[0x0100] = 0x00;
    rom[0x0101] = 0x18;
    rom[0x0102] = 0xFE;

    let mut gb = make_emulator(rom.clone());
    gb.set_external_ram(&[0x5A; 0x2000]);

    let blob = gb.save_state_v1();
    let parsed = SaveState::from_blob(blob).expect("v1 MBC3 blob should parse");
    assert_eq!(parsed.format_version(), VERSION_V1);
    assert_eq!(
        parsed
            .mbc_payload()
            .expect("MBC3 payload should be present")
            .len(),
        18
    );
    assert_eq!(
        parsed.cart_ram().expect("cart RAM should be present").len(),
        0x2000
    );

    let mut restored = make_emulator(rom);
    restored
        .load_state(parsed)
        .expect("v1 MBC3 load should work");
    assert_eq!(restored.external_ram().expect("restored cart RAM")[0], 0x5A);
}

#[test]
fn test_save_state_v2_roundtrips_128k_cart_ram() {
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x13; // MBC3+RAM+BATTERY
    rom[0x0148] = 0x00;
    rom[0x0149] = 0x04; // 128 KiB RAM
    rom[0x0100] = 0x00;
    rom[0x0101] = 0x18;
    rom[0x0102] = 0xFE;

    let mut gb = make_emulator(rom.clone());
    let mut ram = vec![0u8; 128 * 1024];
    ram[0] = 0x11;
    ram[0x7FFF] = 0x22;
    ram[0x1_FFFF] = 0x33;
    gb.set_external_ram(&ram);

    let blob = gb.save_state();
    assert_eq!(u16::from_le_bytes([blob[4], blob[5]]), VERSION_V2);

    let parsed = SaveState::from_blob(blob).expect("v2 blob should parse");
    assert_eq!(parsed.format_version(), VERSION_V2);
    assert_eq!(
        parsed.cart_ram().expect("cart RAM should be present").len(),
        128 * 1024
    );

    let mut restored = make_emulator(rom);
    restored.load_state(parsed).expect("v2 load should work");
    let restored_ram = restored.external_ram().expect("restored cart RAM");
    assert_eq!(restored_ram.len(), 128 * 1024);
    assert_eq!(restored_ram[0], 0x11);
    assert_eq!(restored_ram[0x7FFF], 0x22);
    assert_eq!(restored_ram[0x1_FFFF], 0x33);
}

// ── Cycle counter preserved ───────────────────────────────────────────────────

#[test]
fn test_save_state_roundtrip_preserves_cycle_counter() {
    let rom = make_rom(0x00, 0, 0);
    let mut gb = make_emulator(rom.clone());

    // Run a known number of ticks to accumulate cycles
    for _ in 0..2000 {
        gb.step().unwrap();
    }

    let cycles_before = gb.cycle_counter();
    assert!(
        cycles_before > 0,
        "cycle_counter should be non-zero after ticking"
    );

    let state = gb.save_state();

    let mut gb2 = make_emulator(rom);
    gb2.load_state(SaveState::from_blob(state).expect("from_blob failed"))
        .expect("load_state failed");

    assert_eq!(
        cycles_before,
        gb2.cycle_counter(),
        "cycle_counter not preserved across save/load"
    );
}
