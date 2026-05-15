#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerCommand {
    AdvanceApu { cycles: u16, div_counter: u16 },
    AdvancePpu { cycles: u16 },
    WriteApuRegister { addr: u16, value: u8 },
    WriteWaveRam { offset: u8, value: u8 },
    WriteVram { offset: u16, value: u8 },
    WriteOam { offset: u16, value: u8 },
    WritePpuRegister { addr: u16, value: u8 },
}

/// State reported back from the worker to the CPU coordinator after each tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WorkerOutput {
    pub apu_nr52: u8,
    pub ppu_ly: u8,
    pub ppu_stat: u8,
    pub if_bits: u8,
    pub frame_ready: bool,
}
