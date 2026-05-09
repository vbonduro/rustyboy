//! Smoke tests for the GameBoy public API.

use rustyboy_core::cpu::registers::Registers;
use rustyboy_core::GameBoy;

fn make_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x00; // No MBC
    rom[0x0148] = 0x00; // 32 KB
    rom[0x0149] = 0x00;
    // NOP + JR -2 infinite loop at 0x0100
    rom[0x0100] = 0x00; // NOP
    rom[0x0101] = 0x18; // JR
    rom[0x0102] = 0xFE; // -2
    rom
}

#[test]
fn gamboy_new_and_tick() {
    let mut gb = GameBoy::new(make_rom());
    // Should not panic after running many ticks.
    for _ in 0..10_000 {
        gb.tick();
    }
    assert!(gb.cycle_counter() > 0);
}

#[test]
fn gamboy_front_buffer_is_correct_size() {
    let gb = GameBoy::new(make_rom());
    assert_eq!(gb.front_buffer().len(), 160 * 144);
}

/// Test that EI + NOP correctly dispatches a pending interrupt via GameBoy::step().
/// Mirrors the SM83 unit test `test_interrupt_dispatch_jumps_to_vector_and_pushes_pc`.
#[test]
fn gamboy_ei_interrupt_dispatch() {
    // ROM: EI (0xFB), NOP (0x00), then padding
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x00;
    rom[0x0100] = 0xFB; // EI
    rom[0x0101] = 0x00; // NOP
    rom[0x0102] = 0x00; // padding

    let mut gb = GameBoy::new(rom)
        .with_registers(Registers {
            a: 0x01,
            sp: 0xDFFE,
            pc: 0x0100,
            ..Default::default()
        });

    // IE: VBlank enabled (bit 0), IF: VBlank pending (bit 0)
    gb.write_io(0xFFFF, 0x01); // IE
    gb.write_io(0xFF0F, 0x01); // IF

    gb.step().unwrap(); // EI — IME becomes Pending, IF still set
    assert_eq!(gb.read_io(0xFF0F), 0x01, "IF should still be 0x01 after EI");

    gb.step().unwrap(); // NOP — IME activates, interrupt dispatched after

    assert_eq!(gb.registers().pc, 0x0040, "PC should be at VBlank vector 0x0040");
    assert_eq!(gb.registers().sp, 0xDFFC, "SP should be decremented by 2");
    assert_eq!(gb.read_io(0xFF0F) & 0x01, 0, "IF bit 0 should be cleared");
    assert!(!gb.ime(), "IME should be cleared during dispatch");
}

/// Test that EI + HALT with a pending interrupt dispatches immediately.
#[test]
fn gamboy_ei_halt_with_pending_interrupt() {
    // ROM: EI (0xFB), HALT (0x76), then padding
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x00;
    rom[0x0100] = 0xFB; // EI
    rom[0x0101] = 0x76; // HALT
    rom[0x0102] = 0x00; // padding

    let mut gb = GameBoy::new(rom)
        .with_registers(Registers {
            a: 0x01,
            sp: 0xDFFE,
            pc: 0x0100,
            ..Default::default()
        });

    // IE: VBlank enabled (bit 0), IF: VBlank pending (bit 0)
    gb.write_io(0xFFFF, 0x01); // IE
    gb.write_io(0xFF0F, 0x01); // IF

    gb.step().unwrap(); // EI — IME=Pending
    // After EI: IME is Pending (not yet Enabled)
    assert!(!gb.ime(), "IME should NOT be enabled immediately after EI");

    gb.step().unwrap(); // HALT: advance_ime makes IME=Enabled, post_instr dispatches interrupt

    // After HALT+interrupt dispatch: PC should be at VBlank vector
    assert_eq!(gb.registers().pc, 0x0040, "PC should be at VBlank vector 0x0040 after EI+HALT dispatch");
    assert_eq!(gb.registers().sp, 0xDFFC, "SP should be decremented by 2");
    assert_eq!(gb.read_io(0xFF0F) & 0x01, 0, "IF bit 0 should be cleared");
    assert!(!gb.ime(), "IME should be cleared during dispatch");
}

/// Test that RETI restores IME immediately (no delay).
#[test]
fn gamboy_reti_restores_ime_immediately() {
    // This ROM:
    // 0x0100: EI (0xFB)
    // 0x0101: NOP (0x00)   [allows interrupt to dispatch]
    // ISR at 0x0040: RETI (0xD9)
    let mut rom = vec![0u8; 0x8000];
    rom[0x0147] = 0x00;
    rom[0x0100] = 0xFB; // EI
    rom[0x0101] = 0x00; // NOP (allows interrupt after EI)
    rom[0x0040] = 0xD9; // RETI at VBlank vector

    let mut gb = GameBoy::new(rom)
        .with_registers(Registers {
            a: 0x01,
            sp: 0xDFFE,
            pc: 0x0100,
            ..Default::default()
        });

    // IE: VBlank enabled, IF: VBlank pending
    gb.write_io(0xFFFF, 0x01);
    gb.write_io(0xFF0F, 0x01);

    gb.step().unwrap(); // EI
    gb.step().unwrap(); // NOP + ISR dispatch (goes to 0x0040)

    assert_eq!(gb.registers().pc, 0x0040, "should be at ISR vector");
    assert!(!gb.ime(), "IME disabled during ISR");

    // Execute RETI at 0x0040
    gb.step().unwrap(); // RETI: restores PC, sets IME=Enabled immediately

    assert!(gb.ime(), "IME should be enabled immediately after RETI");
}

#[test]
fn gamboy_save_load_roundtrip() {
    use rustyboy_core::cpu::save_state::SaveState;

    let mut gb = GameBoy::new(make_rom());
    for _ in 0..1000 {
        gb.tick();
    }
    let cycles_before = gb.cycle_counter();
    let blob = gb.save_state();

    let mut gb2 = GameBoy::new(make_rom());
    gb2.load_state(SaveState::from_blob(blob).unwrap()).unwrap();
    assert_eq!(gb2.cycle_counter(), cycles_before);
}
