/// APU register addresses.
pub(crate) const NR10_ADDR: u16 = 0xFF10;
pub(crate) const NR52_ADDR: u16 = 0xFF26;
pub(crate) const WAVE_RAM_START: u16 = 0xFF30;
pub(crate) const WAVE_RAM_END: u16 = 0xFF3F;


/// OR masks for APU register reads. Indexed by (address - 0xFF10).
/// Write-only bits read as 1.
const READ_MASKS: [u8; 23] = [
    0x80, // NR10 (0xFF10)
    0x3F, // NR11 (0xFF11)
    0x00, // NR12 (0xFF12)
    0xFF, // NR13 (0xFF13) — write-only
    0xBF, // NR14 (0xFF14)
    0xFF, // 0xFF15 — unused
    0x3F, // NR21 (0xFF16)
    0x00, // NR22 (0xFF17)
    0xFF, // NR23 (0xFF18) — write-only
    0xBF, // NR24 (0xFF19)
    0x7F, // NR30 (0xFF1A)
    0xFF, // NR31 (0xFF1B) — write-only
    0x9F, // NR32 (0xFF1C)
    0xFF, // NR33 (0xFF1D) — write-only
    0xBF, // NR34 (0xFF1E)
    0xFF, // 0xFF1F — unused
    0xFF, // NR41 (0xFF20) — write-only
    0x00, // NR42 (0xFF21)
    0x00, // NR43 (0xFF22)
    0xBF, // NR44 (0xFF23)
    0x00, // NR50 (0xFF24)
    0x00, // NR51 (0xFF25)
    0x70, // NR52 (0xFF26) — bits 4-6 unused
];

/// Square wave duty patterns (8 bits each, indexed by duty code 0-3).
#[allow(dead_code)]
const DUTY_TABLE: [[u8; 8]; 4] = [
    [0, 0, 0, 0, 0, 0, 0, 1], // 12.5%
    [1, 0, 0, 0, 0, 0, 0, 1], // 25%
    [1, 0, 0, 0, 0, 1, 1, 1], // 50%
    [0, 1, 1, 1, 1, 1, 1, 0], // 75%
];

/// Noise channel divisor table.
const NOISE_DIVISORS: [u16; 8] = [8, 16, 32, 48, 64, 80, 96, 112];

/// Result of an APU tick.
pub struct ApuOutput {
    pub nr52: u8,
}

#[derive(Default)]
struct SquareChannel {
    enabled: bool,
    dac_enabled: bool,
    length_counter: u16,
    length_enabled: bool,
    volume: u8,
    volume_initial: u8,
    envelope_add: bool,
    envelope_period: u8,
    envelope_timer: u8,
    frequency: u16,
    frequency_timer: u16,
    duty: u8,
    duty_position: u8,
}

impl SquareChannel {
    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.frequency_timer = (2048 - self.frequency) * 4;
        self.volume = self.volume_initial;
        self.envelope_timer = if self.envelope_period == 0 { 8 } else { self.envelope_period };
    }

    fn clock_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    fn clock_envelope(&mut self) {
        if self.envelope_period == 0 {
            return;
        }
        if self.envelope_timer > 0 {
            self.envelope_timer -= 1;
        }
        if self.envelope_timer == 0 {
            self.envelope_timer = if self.envelope_period == 0 { 8 } else { self.envelope_period };
            if self.envelope_add && self.volume < 15 {
                self.volume += 1;
            } else if !self.envelope_add && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }

    fn clock_frequency(&mut self) {
        if self.frequency_timer > 0 {
            self.frequency_timer -= 1;
        }
        if self.frequency_timer == 0 {
            self.frequency_timer = (2048 - self.frequency) * 4;
            self.duty_position = (self.duty_position + 1) % 8;
        }
    }
}

#[derive(Default)]
struct SweepState {
    enabled: bool,
    period: u8,
    timer: u8,
    negate: bool,
    shift: u8,
    shadow_frequency: u16,
    negate_used: bool,
}

impl SweepState {
    fn trigger(&mut self, channel: &SquareChannel) {
        self.shadow_frequency = channel.frequency;
        self.timer = if self.period == 0 { 8 } else { self.period };
        self.enabled = self.period != 0 || self.shift != 0;
        self.negate_used = false;
        // If shift is nonzero, do overflow check immediately
        if self.shift != 0 {
            self.calculate_frequency(); // just for overflow check
        }
    }

    fn calculate_frequency(&mut self) -> u16 {
        let delta = self.shadow_frequency >> self.shift;
        let new_freq = if self.negate {
            self.negate_used = true;
            self.shadow_frequency.wrapping_sub(delta)
        } else {
            self.shadow_frequency.wrapping_add(delta)
        };
        new_freq
    }

    fn clock(&mut self, channel: &mut SquareChannel) {
        if self.timer > 0 {
            self.timer -= 1;
        }
        if self.timer == 0 {
            self.timer = if self.period == 0 { 8 } else { self.period };
            if self.enabled && self.period != 0 {
                let new_freq = self.calculate_frequency();
                if new_freq > 2047 {
                    channel.enabled = false;
                } else if self.shift != 0 {
                    self.shadow_frequency = new_freq;
                    channel.frequency = new_freq;
                    // Do overflow check again with new frequency
                    let check_freq = self.calculate_frequency();
                    if check_freq > 2047 {
                        channel.enabled = false;
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct WaveChannel {
    enabled: bool,
    dac_enabled: bool,
    length_counter: u16,
    length_enabled: bool,
    volume_code: u8,
    frequency: u16,
    frequency_timer: u16,
    position: u8,
    sample_buffer: u8,
    wave_ram: [u8; 16],
}

impl WaveChannel {
    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 256;
        }
        self.frequency_timer = (2048 - self.frequency) * 2;
        self.position = 0;
    }

    fn clock_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    fn clock_frequency(&mut self) {
        if self.frequency_timer > 0 {
            self.frequency_timer -= 1;
        }
        if self.frequency_timer == 0 {
            self.frequency_timer = (2048 - self.frequency) * 2;
            self.position = (self.position + 1) % 32;
            let byte_index = (self.position / 2) as usize;
            self.sample_buffer = self.wave_ram[byte_index];
        }
    }
}

#[derive(Default)]
struct NoiseChannel {
    enabled: bool,
    dac_enabled: bool,
    length_counter: u16,
    length_enabled: bool,
    volume: u8,
    volume_initial: u8,
    envelope_add: bool,
    envelope_period: u8,
    envelope_timer: u8,
    clock_shift: u8,
    width_mode: bool,
    divisor_code: u8,
    frequency_timer: u16,
    lfsr: u16,
}

impl NoiseChannel {
    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.frequency_timer = NOISE_DIVISORS[self.divisor_code as usize] << self.clock_shift;
        self.volume = self.volume_initial;
        self.envelope_timer = if self.envelope_period == 0 { 8 } else { self.envelope_period };
        self.lfsr = 0x7FFF;
    }

    fn clock_length(&mut self) {
        if self.length_enabled && self.length_counter > 0 {
            self.length_counter -= 1;
            if self.length_counter == 0 {
                self.enabled = false;
            }
        }
    }

    fn clock_envelope(&mut self) {
        if self.envelope_period == 0 {
            return;
        }
        if self.envelope_timer > 0 {
            self.envelope_timer -= 1;
        }
        if self.envelope_timer == 0 {
            self.envelope_timer = if self.envelope_period == 0 { 8 } else { self.envelope_period };
            if self.envelope_add && self.volume < 15 {
                self.volume += 1;
            } else if !self.envelope_add && self.volume > 0 {
                self.volume -= 1;
            }
        }
    }

    fn clock_frequency(&mut self) {
        if self.frequency_timer > 0 {
            self.frequency_timer -= 1;
        }
        if self.frequency_timer == 0 {
            self.frequency_timer = NOISE_DIVISORS[self.divisor_code as usize] << self.clock_shift;
            let xor_bit = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
            self.lfsr >>= 1;
            self.lfsr |= xor_bit << 14;
            if self.width_mode {
                self.lfsr &= !(1 << 6);
                self.lfsr |= xor_bit << 6;
            }
        }
    }
}

/// Bit 12 of the timer's internal counter (= bit 4 of DIV).
/// The frame sequencer clocks on the falling edge of this bit.
const FRAME_SEQ_BIT: u16 = 1 << 12;

/// Game Boy APU peripheral.
pub struct ApuPeripheral {
    powered: bool,
    /// Previous state of FRAME_SEQ_BIT for falling-edge detection.
    prev_div_bit: bool,
    frame_sequencer_step: u8,

    channel1: SquareChannel,
    sweep: SweepState,
    channel2: SquareChannel,
    channel3: WaveChannel,
    channel4: NoiseChannel,

    // Raw register values for read-back
    regs: [u8; 23], // NR10 (0xFF10) through NR52 (0xFF26)
}

impl ApuPeripheral {
    pub fn new() -> Self {
        Self {
            powered: true,
            prev_div_bit: false,
            frame_sequencer_step: 0,
            channel1: SquareChannel::default(),
            sweep: SweepState::default(),
            channel2: SquareChannel::default(),
            channel3: WaveChannel::default(),
            channel4: NoiseChannel::default(),
            regs: [0u8; 23],
        }
    }

    /// Read a register with OR masks applied.
    pub fn read_register(&self, address: u16) -> u8 {
        if address == NR52_ADDR {
            return self.build_nr52();
        }
        if !(NR10_ADDR..NR52_ADDR).contains(&address) {
            return 0xFF;
        }
        let offset = (address - NR10_ADDR) as usize;
        self.regs[offset] | READ_MASKS[offset]
    }

    /// Handle a write to an APU register.
    pub fn write_register(&mut self, address: u16, value: u8) {
        if address == NR52_ADDR {
            self.write_nr52(value);
            return;
        }
        if !(NR10_ADDR..NR52_ADDR).contains(&address) {
            return;
        }
        // When powered off, only length counter writes (NRx1) are allowed on DMG
        if !self.powered {
            match address {
                0xFF11 | 0xFF16 | 0xFF1B | 0xFF20 => {
                    // Update length counter state but don't store in regs[]
                    self.apply_register_write(address, value);
                }
                _ => {}
            }
            return;
        }
        let offset = (address - NR10_ADDR) as usize;
        self.regs[offset] = value;
        self.apply_register_write(address, value);
    }

    /// Read wave RAM byte.
    pub fn read_wave_ram(&self, offset: u8) -> u8 {
        if self.channel3.enabled {
            // Read while ch3 is on: return the byte currently being read
            self.channel3.sample_buffer
        } else {
            self.channel3.wave_ram[offset as usize]
        }
    }

    /// Write wave RAM byte.
    pub fn write_wave_ram(&mut self, offset: u8, value: u8) {
        if self.channel3.enabled {
            // Write while ch3 is on: write to the byte currently being read
            let byte_index = (self.channel3.position / 2) as usize;
            self.channel3.wave_ram[byte_index] = value;
        } else {
            self.channel3.wave_ram[offset as usize] = value;
        }
    }

    /// Advance the APU by `cycles` T-cycles.
    ///
    /// `div_counter` is the timer's internal 16-bit counter *after* the timer
    /// has been advanced for these cycles. The frame sequencer clocks on the
    /// falling edge of bit 12 (DIV bit 4).
    pub fn tick(&mut self, cycles: u16, div_counter: u16) -> ApuOutput {
        if !self.powered {
            self.prev_div_bit = div_counter & FRAME_SEQ_BIT != 0;
            return ApuOutput { nr52: self.build_nr52() };
        }

        // Reconstruct per-T-cycle DIV values to detect falling edge of bit 12.
        let div_start = div_counter.wrapping_sub(cycles);
        for i in 0..cycles {
            let div_now = div_start.wrapping_add(i + 1);
            let cur_bit = div_now & FRAME_SEQ_BIT != 0;

            if self.prev_div_bit && !cur_bit {
                self.clock_frame_sequencer();
            }
            self.prev_div_bit = cur_bit;

            // Frequency timers
            self.channel1.clock_frequency();
            self.channel2.clock_frequency();
            self.channel3.clock_frequency();
            self.channel4.clock_frequency();
        }

        ApuOutput { nr52: self.build_nr52() }
    }

    fn clock_frame_sequencer(&mut self) {
        match self.frame_sequencer_step {
            0 | 4 => {
                self.clock_length_counters();
            }
            2 | 6 => {
                self.clock_length_counters();
                self.sweep.clock(&mut self.channel1);
            }
            7 => {
                self.channel1.clock_envelope();
                self.channel2.clock_envelope();
                self.channel4.clock_envelope();
            }
            _ => {}
        }
        self.frame_sequencer_step = (self.frame_sequencer_step + 1) % 8;
    }

    fn clock_length_counters(&mut self) {
        self.channel1.clock_length();
        self.channel2.clock_length();
        self.channel3.clock_length();
        self.channel4.clock_length();
    }

    fn build_nr52(&self) -> u8 {
        0x70 // bits 4-6 always 1
            | if self.powered { 0x80 } else { 0 }
            | if self.channel1.enabled { 0x01 } else { 0 }
            | if self.channel2.enabled { 0x02 } else { 0 }
            | if self.channel3.enabled { 0x04 } else { 0 }
            | if self.channel4.enabled { 0x08 } else { 0 }
    }

    fn write_nr52(&mut self, value: u8) {
        let was_powered = self.powered;
        self.powered = value & 0x80 != 0;

        if was_powered && !self.powered {
            // Power off: zero all registers, disable channels, reset internal state.
            // On DMG, length counters are NOT reset.
            for i in 0..22 {
                self.regs[i] = 0;
            }
            // Save length counters before resetting channels
            let ch1_len = self.channel1.length_counter;
            let ch2_len = self.channel2.length_counter;
            let ch3_len = self.channel3.length_counter;
            let ch4_len = self.channel4.length_counter;
            self.channel1 = SquareChannel::default();
            self.channel1.length_counter = ch1_len;
            self.channel2 = SquareChannel::default();
            self.channel2.length_counter = ch2_len;
            self.channel3 = WaveChannel { length_counter: ch3_len, wave_ram: self.channel3.wave_ram, ..WaveChannel::default() };
            self.channel4 = NoiseChannel::default();
            self.channel4.length_counter = ch4_len;
            self.sweep = SweepState::default();
            self.frame_sequencer_step = 0;
        } else if !was_powered && self.powered {
            // Power on: reset frame sequencer
            self.frame_sequencer_step = 0;
        }
    }

    /// Returns true if the previous frame sequencer step clocked length
    /// (i.e. we're in the "first half" of a length period, where enabling
    /// length should cause an extra length clock).
    /// After an even step (0,2,4,6) executes and clocks length, the step
    /// counter is incremented to odd. So odd step = just clocked length.
    fn frame_step_clocks_length(&self) -> bool {
        self.frame_sequencer_step % 2 == 1
    }

    fn apply_register_write(&mut self, address: u16, value: u8) {
        match address {
            // Channel 1: Sweep
            0xFF10 => {
                self.sweep.period = (value >> 4) & 0x07;
                let new_negate = value & 0x08 != 0;
                // Negate-then-positive quirk: if sweep was used in negate mode
                // and is now switched to positive, disable channel
                if self.sweep.negate_used && self.sweep.negate && !new_negate {
                    self.channel1.enabled = false;
                }
                self.sweep.negate = new_negate;
                self.sweep.shift = value & 0x07;
            }
            // Channel 1: Duty + Length
            0xFF11 => {
                self.channel1.duty = (value >> 6) & 0x03;
                self.channel1.length_counter = 64 - (value & 0x3F) as u16;
            }
            // Channel 1: Volume Envelope
            0xFF12 => {
                self.channel1.volume_initial = (value >> 4) & 0x0F;
                self.channel1.envelope_add = value & 0x08 != 0;
                self.channel1.envelope_period = value & 0x07;
                self.channel1.dac_enabled = value & 0xF8 != 0;
                if !self.channel1.dac_enabled {
                    self.channel1.enabled = false;
                }
            }
            // Channel 1: Frequency lo
            0xFF13 => {
                self.channel1.frequency = (self.channel1.frequency & 0x700) | value as u16;
            }
            // Channel 1: Frequency hi + Trigger + Length enable
            0xFF14 => {
                self.channel1.frequency =
                    (self.channel1.frequency & 0xFF) | ((value as u16 & 0x07) << 8);
                let new_len_enable = value & 0x40 != 0;
                let on_length_step = self.frame_step_clocks_length();
                // Extra length clock: enabling length on a length-clocking step
                if new_len_enable && !self.channel1.length_enabled && on_length_step {
                    self.channel1.length_enabled = true;
                    self.channel1.clock_length();
                }
                self.channel1.length_enabled = new_len_enable;
                if value & 0x80 != 0 {
                    let len_was_zero = self.channel1.length_counter == 0;
                    self.channel1.trigger();
                    self.sweep.trigger(&self.channel1);
                    if self.sweep.shift != 0 {
                        let new_freq = self.sweep.calculate_frequency();
                        if new_freq > 2047 {
                            self.channel1.enabled = false;
                        }
                    }
                    // If trigger reloaded length (was 0) and length is enabled
                    // on a length-clocking step, extra clock
                    if len_was_zero && new_len_enable && on_length_step {
                        self.channel1.clock_length();
                    }
                }
            }
            // Channel 2: Duty + Length
            0xFF16 => {
                self.channel2.duty = (value >> 6) & 0x03;
                self.channel2.length_counter = 64 - (value & 0x3F) as u16;
            }
            // Channel 2: Volume Envelope
            0xFF17 => {
                self.channel2.volume_initial = (value >> 4) & 0x0F;
                self.channel2.envelope_add = value & 0x08 != 0;
                self.channel2.envelope_period = value & 0x07;
                self.channel2.dac_enabled = value & 0xF8 != 0;
                if !self.channel2.dac_enabled {
                    self.channel2.enabled = false;
                }
            }
            // Channel 2: Frequency lo
            0xFF18 => {
                self.channel2.frequency = (self.channel2.frequency & 0x700) | value as u16;
            }
            // Channel 2: Frequency hi + Trigger + Length enable
            0xFF19 => {
                self.channel2.frequency =
                    (self.channel2.frequency & 0xFF) | ((value as u16 & 0x07) << 8);
                let new_len_enable = value & 0x40 != 0;
                let on_length_step = self.frame_step_clocks_length();
                if new_len_enable && !self.channel2.length_enabled && on_length_step {
                    self.channel2.length_enabled = true;
                    self.channel2.clock_length();
                }
                self.channel2.length_enabled = new_len_enable;
                if value & 0x80 != 0 {
                    let len_was_zero = self.channel2.length_counter == 0;
                    self.channel2.trigger();
                    if len_was_zero && new_len_enable && on_length_step {
                        self.channel2.clock_length();
                    }
                }
            }
            // Channel 3: DAC enable
            0xFF1A => {
                self.channel3.dac_enabled = value & 0x80 != 0;
                if !self.channel3.dac_enabled {
                    self.channel3.enabled = false;
                }
            }
            // Channel 3: Length
            0xFF1B => {
                self.channel3.length_counter = 256 - value as u16;
            }
            // Channel 3: Volume
            0xFF1C => {
                self.channel3.volume_code = (value >> 5) & 0x03;
            }
            // Channel 3: Frequency lo
            0xFF1D => {
                self.channel3.frequency = (self.channel3.frequency & 0x700) | value as u16;
            }
            // Channel 3: Frequency hi + Trigger + Length enable
            0xFF1E => {
                self.channel3.frequency =
                    (self.channel3.frequency & 0xFF) | ((value as u16 & 0x07) << 8);
                let new_len_enable = value & 0x40 != 0;
                let on_length_step = self.frame_step_clocks_length();
                if new_len_enable && !self.channel3.length_enabled && on_length_step {
                    self.channel3.length_enabled = true;
                    self.channel3.clock_length();
                }
                self.channel3.length_enabled = new_len_enable;
                if value & 0x80 != 0 {
                    let len_was_zero = self.channel3.length_counter == 0;
                    self.channel3.trigger();
                    if len_was_zero && new_len_enable && on_length_step {
                        self.channel3.clock_length();
                    }
                }
            }
            // Channel 4: Length
            0xFF20 => {
                self.channel4.length_counter = 64 - (value & 0x3F) as u16;
            }
            // Channel 4: Volume Envelope
            0xFF21 => {
                self.channel4.volume_initial = (value >> 4) & 0x0F;
                self.channel4.envelope_add = value & 0x08 != 0;
                self.channel4.envelope_period = value & 0x07;
                self.channel4.dac_enabled = value & 0xF8 != 0;
                if !self.channel4.dac_enabled {
                    self.channel4.enabled = false;
                }
            }
            // Channel 4: Polynomial counter
            0xFF22 => {
                self.channel4.clock_shift = (value >> 4) & 0x0F;
                self.channel4.width_mode = value & 0x08 != 0;
                self.channel4.divisor_code = value & 0x07;
            }
            // Channel 4: Trigger + Length enable
            0xFF23 => {
                let new_len_enable = value & 0x40 != 0;
                let on_length_step = self.frame_step_clocks_length();
                if new_len_enable && !self.channel4.length_enabled && on_length_step {
                    self.channel4.length_enabled = true;
                    self.channel4.clock_length();
                }
                self.channel4.length_enabled = new_len_enable;
                if value & 0x80 != 0 {
                    let len_was_zero = self.channel4.length_counter == 0;
                    self.channel4.trigger();
                    if len_was_zero && new_len_enable && on_length_step {
                        self.channel4.clock_length();
                    }
                }
            }
            // NR50, NR51: master volume/panning — just stored in regs[]
            0xFF24 | 0xFF25 => {}
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_SEQUENCER_PERIOD: u16 = 8192;

    #[test]
    fn test_read_masks() {
        let apu = ApuPeripheral::new();
        // Write-only registers should read with mask bits set
        assert_eq!(apu.read_register(0xFF13) & 0xFF, 0xFF); // NR13 fully write-only
        assert_eq!(apu.read_register(0xFF14) & 0xBF, 0xBF); // NR14 trigger bit reads as 1
        // Unused addresses read as 0xFF
        assert_eq!(apu.read_register(0xFF15), 0xFF);
        assert_eq!(apu.read_register(0xFF1F), 0xFF);
    }

    #[test]
    fn test_nr52_power_on_off() {
        let mut apu = ApuPeripheral::new();
        assert!(apu.powered);

        // Power off
        apu.write_register(NR52_ADDR, 0x00);
        assert!(!apu.powered);
        assert_eq!(apu.build_nr52() & 0x80, 0);

        // All NR registers should be zeroed
        for addr in 0xFF10u16..=0xFF25 {
            if addr == 0xFF15 || addr == 0xFF1F {
                continue; // unused
            }
            let offset = (addr - NR10_ADDR) as usize;
            assert_eq!(apu.regs[offset], 0, "register 0x{:04X} not zeroed", addr);
        }

        // Power on
        apu.write_register(NR52_ADDR, 0x80);
        assert!(apu.powered);
        assert_eq!(apu.build_nr52() & 0x80, 0x80);
    }

    #[test]
    fn test_writes_ignored_when_powered_off() {
        let mut apu = ApuPeripheral::new();
        apu.write_register(NR52_ADDR, 0x00); // power off

        // Writes to most registers should be ignored
        apu.write_register(0xFF12, 0xFF);
        assert_eq!(apu.regs[2], 0); // NR12 unchanged

        // But length counter writes are allowed on DMG
        apu.write_register(0xFF11, 0x3F);
        assert_eq!(apu.channel1.length_counter, 64 - 0x3F);
    }

    #[test]
    fn test_nr52_channel_status() {
        let mut apu = ApuPeripheral::new();
        // Enable ch1 DAC and trigger
        apu.write_register(0xFF12, 0xF0); // volume envelope, DAC on
        apu.write_register(0xFF14, 0x80); // trigger
        assert_eq!(apu.build_nr52() & 0x01, 0x01);

        // Disable DAC → channel off
        apu.write_register(0xFF12, 0x00);
        assert_eq!(apu.build_nr52() & 0x01, 0x00);
    }

    #[test]
    fn test_length_counter_disables_channel() {
        let mut apu = ApuPeripheral::new();
        // Ch1: set short length, enable length, trigger
        apu.write_register(0xFF12, 0xF0); // DAC on
        apu.write_register(0xFF11, 0x3F); // length = 64 - 63 = 1
        apu.write_register(0xFF14, 0xC0); // trigger + length enable

        assert!(apu.channel1.enabled);
        assert_eq!(apu.channel1.length_counter, 1);

        // Clock length counter once
        apu.channel1.clock_length();
        assert!(!apu.channel1.enabled);
    }

    #[test]
    fn test_envelope_volume_change() {
        let mut apu = ApuPeripheral::new();
        apu.write_register(0xFF12, 0xF1); // vol=15, add=false, period=1
        apu.write_register(0xFF14, 0x80); // trigger

        assert_eq!(apu.channel1.volume, 15);

        // Clock envelope — timer=1 decrements to 0, then volume adjusts
        apu.channel1.clock_envelope();
        assert_eq!(apu.channel1.volume, 14);
        apu.channel1.clock_envelope();
        assert_eq!(apu.channel1.volume, 13);
    }

    #[test]
    fn test_sweep_overflow_disables_channel() {
        let mut apu = ApuPeripheral::new();
        apu.write_register(0xFF12, 0xF0); // DAC on
        apu.write_register(0xFF10, 0x11); // period=1, negate=false, shift=1
        apu.write_register(0xFF13, 0xFF); // freq lo = 0xFF
        apu.write_register(0xFF14, 0x87); // freq hi = 7, trigger
        // frequency = 0x7FF = 2047
        // sweep: new_freq = 2047 + (2047 >> 1) = 2047 + 1023 = 3070 > 2047
        assert!(!apu.channel1.enabled);
    }

    #[test]
    fn test_wave_ram_access() {
        let mut apu = ApuPeripheral::new();
        // Write wave RAM while channel off
        apu.write_wave_ram(0, 0x12);
        apu.write_wave_ram(1, 0x34);
        assert_eq!(apu.read_wave_ram(0), 0x12);
        assert_eq!(apu.read_wave_ram(1), 0x34);
    }

    #[test]
    fn test_noise_lfsr() {
        let mut apu = ApuPeripheral::new();
        apu.write_register(0xFF21, 0xF0); // DAC on
        apu.write_register(0xFF22, 0x00); // clock_shift=0, width=15-bit, divisor=0
        apu.write_register(0xFF23, 0x80); // trigger
        assert_eq!(apu.channel4.lfsr, 0x7FFF);
        // frequency_timer = NOISE_DIVISORS[0] << 0 = 8
        // Clock 8 times to expire timer and advance LFSR once
        for _ in 0..8 {
            apu.channel4.clock_frequency();
        }
        // XOR of bits 0,1 of 0x7FFF: both 1, XOR = 0
        // Shift right: 0x3FFF, set bit 14 to 0 = 0x3FFF
        assert_eq!(apu.channel4.lfsr, 0x3FFF);
    }

    #[test]
    fn test_frame_sequencer_steps() {
        let mut apu = ApuPeripheral::new();
        apu.write_register(0xFF12, 0xF0); // ch1 DAC on
        apu.write_register(0xFF14, 0x80); // trigger

        // Frame sequencer clocks on falling edge of bit 12.
        // Simulate by advancing div_counter through 8 falling edges.
        let mut div: u16 = 0;
        for expected_step in 0..8u8 {
            assert_eq!(apu.frame_sequencer_step, expected_step);
            // Advance 8192 T-cycles — bit 12 will fall once
            div = div.wrapping_add(FRAME_SEQUENCER_PERIOD);
            apu.tick(FRAME_SEQUENCER_PERIOD, div);
        }
        // Should wrap to 0
        assert_eq!(apu.frame_sequencer_step, 0);
    }

    #[test]
    fn test_dac_disable_kills_channel() {
        let mut apu = ApuPeripheral::new();
        // Ch3: enable DAC, trigger
        apu.write_register(0xFF1A, 0x80); // DAC on
        apu.write_register(0xFF1E, 0x80); // trigger
        assert!(apu.channel3.enabled);

        // Disable DAC
        apu.write_register(0xFF1A, 0x00);
        assert!(!apu.channel3.enabled);
    }

    #[test]
    fn test_register_readback_with_masks() {
        let mut apu = ApuPeripheral::new();
        // Write NR12 (fully readable, mask=0x00)
        apu.write_register(0xFF12, 0xA5);
        assert_eq!(apu.read_register(0xFF12), 0xA5);

        // Write NR11 (mask=0x3F, only duty bits 6-7 readable)
        apu.write_register(0xFF11, 0xC0); // duty=3
        assert_eq!(apu.read_register(0xFF11), 0xC0 | 0x3F); // 0xFF
    }
}
