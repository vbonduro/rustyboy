use rustyboy_core::storage::RomId;
use rustyboy_pico2w::save_storage::{
    battery_save_filename, rom_save_dir_name, save_state_filename, SaveSlot, SAVE_ROOT_DIR,
};

#[test]
fn save_paths_are_fat_short_name_compatible() {
    let rom_id = RomId::for_bytes(b"abc");

    assert_eq!(SAVE_ROOT_DIR, "SAVES");
    assert_eq!(rom_save_dir_name(&rom_id).as_str(), "BA7816BF");
    assert_eq!(battery_save_filename(), "BATT.SAV");
    assert_eq!(
        save_state_filename(SaveSlot::new(2).unwrap()).as_str(),
        "SLOT2.RBS"
    );
}

#[test]
fn save_slot_rejects_out_of_range_slots() {
    assert_eq!(SaveSlot::new(0).unwrap().index(), 0);
    assert_eq!(SaveSlot::new(2).unwrap().index(), 2);
    assert!(SaveSlot::new(3).is_err());
}

