//! Smoke tests for the GameBoy public API.

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
