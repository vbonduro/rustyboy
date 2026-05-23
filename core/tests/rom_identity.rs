use rustyboy_core::storage::{
    BatterySaveBytes, RomHasher, RomId, SaveStateBytes, MAX_BATTERY_SAVE_BYTES,
    MAX_SAVE_STATE_BYTES,
};

#[test]
fn rom_id_matches_sha256_test_vector() {
    let id = RomId::for_bytes(b"abc");

    assert_eq!(
        id.to_hex().as_str(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(id.short_hex().as_str(), "ba7816bf");
}

#[test]
fn streaming_rom_hasher_matches_one_shot_hash() {
    let mut hasher = RomHasher::new();
    hasher.update(b"ab");
    hasher.update(b"c");

    assert_eq!(hasher.finalize(), RomId::for_bytes(b"abc"));
}

#[test]
fn rom_id_hex_roundtrips_and_rejects_invalid_input() {
    let id = RomId::for_bytes(b"rustyboy");
    let hex = id.to_hex();

    assert_eq!(RomId::from_hex(hex.as_str()).unwrap(), id);
    assert!(RomId::from_hex("ba7816bf").is_err());
    assert!(
        RomId::from_hex("zz7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",)
            .is_err()
    );
}

#[test]
fn battery_save_boundaries_are_enforced_in_core() {
    assert!(BatterySaveBytes::new(&[]).is_err());

    let max = vec![0xAA; MAX_BATTERY_SAVE_BYTES];
    assert_eq!(BatterySaveBytes::new(&max).unwrap().as_slice(), max);

    let too_large = vec![0xAA; MAX_BATTERY_SAVE_BYTES + 1];
    assert!(BatterySaveBytes::new(&too_large).is_err());
}

#[test]
fn save_state_blob_size_boundary_is_enforced_in_core() {
    assert!(SaveStateBytes::new(&[]).is_err());

    let max = vec![0x55; MAX_SAVE_STATE_BYTES];
    assert_eq!(SaveStateBytes::new(&max).unwrap().as_slice(), max);

    let too_large = vec![0x55; MAX_SAVE_STATE_BYTES + 1];
    assert!(SaveStateBytes::new(&too_large).is_err());
}
