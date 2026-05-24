use alloc::string::String;
use alloc::vec::Vec;

use rustyboy_core::cpu::save_state::SaveState;
use rustyboy_core::storage::RomId;

pub const SAVE_ROOT_DIR: &str = "SAVES";
pub const SAVE_SLOT_COUNT: u8 = 3;

const BATTERY_SAVE_FILE: &str = "BATT.SAV";
const SAVE_STATE_EXT: &str = ".RBS";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveStorageError {
    InvalidSlot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveSlot(u8);

impl SaveSlot {
    pub fn new(index: u8) -> Result<Self, SaveStorageError> {
        if index < SAVE_SLOT_COUNT {
            Ok(Self(index))
        } else {
            Err(SaveStorageError::InvalidSlot)
        }
    }

    pub fn index(self) -> u8 {
        self.0
    }
}

pub fn rom_save_dir_name(rom_id: &RomId) -> String {
    upper_ascii(rom_id.short_hex().as_str())
}

pub fn battery_save_filename() -> &'static str {
    BATTERY_SAVE_FILE
}

pub fn save_state_filename(slot: SaveSlot) -> String {
    let mut name = String::with_capacity(9);
    name.push_str("SLOT");
    name.push((b'0' + slot.index()) as char);
    name.push_str(SAVE_STATE_EXT);
    name
}

pub enum BootSaves {
    BatterySave(Vec<u8>),
    SaveState(SaveState),
    Both { battery: Vec<u8>, save_state: SaveState },
}

/// Determine the boot start state from raw SD data.
///
/// `battery_data` and `save_state_blob` are the raw bytes read from the SD
/// card (or `None` if the file was absent or the read failed). Returns `None`
/// when neither file was present; otherwise returns the variant that describes
/// which mutations the caller should apply to the GameBoy.
pub fn boot_load_saves(
    battery_data: Option<Vec<u8>>,
    save_state_blob: Option<Vec<u8>>,
) -> Option<BootSaves> {
    let save_state = save_state_blob.and_then(|blob| SaveState::from_blob(blob).ok());
    match (battery_data, save_state) {
        (None, None) => None,
        (Some(data), None) => Some(BootSaves::BatterySave(data)),
        (None, Some(state)) => Some(BootSaves::SaveState(state)),
        (Some(data), Some(state)) => Some(BootSaves::Both {
            battery: data,
            save_state: state,
        }),
    }
}

fn upper_ascii(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        out.push(byte.to_ascii_uppercase() as char);
    }
    out
}
