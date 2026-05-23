use alloc::string::String;

use sha2::{Digest, Sha256};

pub const ROM_ID_LEN: usize = 32;
pub const ROM_ID_HEX_LEN: usize = ROM_ID_LEN * 2;
pub const ROM_ID_SHORT_HEX_LEN: usize = 8;
pub const MAX_BATTERY_SAVE_BYTES: usize = 128 * 1024;
pub const MAX_SAVE_STATE_BYTES: usize = 256 * 1024;

const HEX: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageValueError {
    EmptyBatterySave,
    BatterySaveTooLarge,
    EmptySaveState,
    SaveStateTooLarge,
    InvalidRomIdHexLength,
    InvalidRomIdHexCharacter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RomId([u8; ROM_ID_LEN]);

impl RomId {
    pub fn for_bytes(bytes: &[u8]) -> Self {
        let mut hasher = RomHasher::new();
        hasher.update(bytes);
        hasher.finalize()
    }

    pub fn from_bytes(bytes: [u8; ROM_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub fn from_hex(hex: &str) -> Result<Self, StorageValueError> {
        if hex.len() != ROM_ID_HEX_LEN {
            return Err(StorageValueError::InvalidRomIdHexLength);
        }

        let mut bytes = [0u8; ROM_ID_LEN];
        let input = hex.as_bytes();
        for (idx, byte) in bytes.iter_mut().enumerate() {
            let hi = decode_hex_nibble(input[idx * 2])?;
            let lo = decode_hex_nibble(input[idx * 2 + 1])?;
            *byte = (hi << 4) | lo;
        }

        Ok(Self(bytes))
    }

    pub fn as_bytes(&self) -> &[u8; ROM_ID_LEN] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        encode_hex(&self.0)
    }

    pub fn short_hex(&self) -> String {
        encode_hex(&self.0[..ROM_ID_SHORT_HEX_LEN / 2])
    }
}

pub struct RomHasher {
    hasher: Sha256,
}

impl RomHasher {
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
        }
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.hasher.update(bytes);
    }

    pub fn finalize(self) -> RomId {
        let digest = self.hasher.finalize();
        let mut bytes = [0u8; ROM_ID_LEN];
        bytes.copy_from_slice(&digest);
        RomId(bytes)
    }
}

impl Default for RomHasher {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatterySaveBytes<'a> {
    data: &'a [u8],
}

impl<'a> BatterySaveBytes<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, StorageValueError> {
        if data.is_empty() {
            return Err(StorageValueError::EmptyBatterySave);
        }
        if data.len() > MAX_BATTERY_SAVE_BYTES {
            return Err(StorageValueError::BatterySaveTooLarge);
        }
        Ok(Self { data })
    }

    pub fn as_slice(&self) -> &'a [u8] {
        self.data
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveStateBytes<'a> {
    data: &'a [u8],
}

impl<'a> SaveStateBytes<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, StorageValueError> {
        if data.is_empty() {
            return Err(StorageValueError::EmptySaveState);
        }
        if data.len() > MAX_SAVE_STATE_BYTES {
            return Err(StorageValueError::SaveStateTooLarge);
        }
        Ok(Self { data })
    }

    pub fn as_slice(&self) -> &'a [u8] {
        self.data
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

fn decode_hex_nibble(byte: u8) -> Result<u8, StorageValueError> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(StorageValueError::InvalidRomIdHexCharacter),
    }
}
