#[derive(Debug, PartialEq)]
pub enum Error {
    InvalidOpcode(u8),
}

pub trait Decoder {
    type Opcode;
    fn decode(opcode: u8) -> Result<Self::Opcode, Error>;
}

