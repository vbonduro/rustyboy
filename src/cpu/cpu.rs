use std::error::Error;

pub trait Cpu {
    // Read the next instruction from the program and execute it.
    // Returns the number of ticks from the instruction.
     fn tick(&mut self) -> Result<u8, Box<dyn Error>>;
}
