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

/// Target sample rate for PCM output (Hz).
pub const SAMPLE_RATE: u32 = 48_000;

/// DMG T-cycle frequency.
const CPU_FREQ: u32 = 4_194_304;

/// Numerator/denominator for the downsampling accumulator.
/// We emit one stereo sample every CPU_FREQ/SAMPLE_RATE T-cycles.
/// Using fixed-point: accumulator counts in units of SAMPLE_RATE.
const SAMPLE_PERIOD_NUM: u32 = CPU_FREQ;
const SAMPLE_PERIOD_DEN: u32 = SAMPLE_RATE;
/// About 804 stereo pairs are produced per Game Boy frame at 48 kHz.
/// Reserve some headroom so the hot audio path doesn't regrow this buffer.
const SAMPLE_BUFFER_CAPACITY_HINT: usize = 2048;
/// Max possible mixer output before normalization: 4 channels * level 15 * volume 8.
const MIXER_MAX: u32 = 4 * 15 * 8;

/// Result of an APU tick.
pub struct ApuOutput {
    pub nr52: u8,
}

#[cfg(feature = "perf")]
#[derive(Default)]
pub struct ApuPerfProfile {
    pub frame_seq: u32,
    pub pulse: u32,
    pub wave: u32,
    pub noise: u32,
    pub mix: u32,
}

/// Pulse (square wave) channel — used by ch1 (with sweep) and ch2.
#[derive(Default)]
struct SquareChannel {
    /// Channel output is active (DAC on and not silenced by length).
    enabled: bool,
    /// DAC powered. When false, channel is immediately disabled.
    dac_enabled: bool,
    /// Counts down to 0; channel disables when it reaches 0 (if length_enabled).
    length_counter: u16,
    /// Whether the length counter is active (NRx4 bit 6).
    length_enabled: bool,
    /// Current output volume (0–15). Modified by the envelope.
    volume: u8,
    /// Initial volume loaded on trigger (NRx2 bits 7–4).
    volume_initial: u8,
    /// Envelope direction: true = increase, false = decrease.
    envelope_add: bool,
    /// Envelope sweep period in frame sequencer ticks (0 = disabled).
    envelope_period: u8,
    /// Counts down each frame sequencer envelope step.
    envelope_timer: u8,
    /// 11-bit frequency value (not the actual Hz, used to derive timer period).
    frequency: u16,
    /// Cached timer reload period `(2048 - frequency) * 4`.
    frequency_period: u16,
    /// Counts down T-cycles; reloads to `(2048 - frequency) * 4` at 0.
    frequency_timer: u16,
    /// Duty pattern index (0–3).
    duty: u8,
    /// Current step within the 8-bit duty waveform (0–7).
    duty_position: u8,
}

impl SquareChannel {
    fn sync_frequency_period(&mut self) {
        self.frequency_period = (2048 - self.frequency) * 4;
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.sync_frequency_period();
        self.frequency_timer = self.frequency_period;
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

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_frequency(&mut self, cycles: u16) {
        if cycles == 0 { return; }
        if self.frequency_timer > cycles {
            self.frequency_timer -= cycles;
            return;
        }
        if self.frequency_period == 0 {
            self.sync_frequency_period();
        }
        let period = self.frequency_period;
        let (fires, final_timer) = if self.frequency_timer == 0 {
            // timer=0: fires immediately without decrement, then `cycles` normal steps follow
            let n = cycles as u32;
            let p = period as u32;
            let fires = 1 + (n - 1) / p;
            let rem = (n - 1) % p;
            (fires, if rem == 0 { period } else { period - rem as u16 })
        } else {
            let remaining = (cycles - self.frequency_timer) as u32;
            let p = period as u32;
            let fires = 1 + remaining / p;
            let rem = remaining % p;
            (fires, if rem == 0 { period } else { period - rem as u16 })
        };
        self.frequency_timer = final_timer;
        if fires > 0 {
            self.duty_position = self.duty_position.wrapping_add(fires as u8) & 0x07;
        }
    }

    fn digital_output(&self) -> u8 {
        if !self.enabled { return 0; }
        let high = DUTY_TABLE[self.duty as usize][self.duty_position as usize];
        if high != 0 { self.volume } else { 0 }
    }
}

/// Frequency sweep unit, attached to ch1 only.
///
/// Each frame sequencer sweep step adjusts ch1's frequency by a fraction of
/// itself, and can disable ch1 on overflow.
#[derive(Default)]
struct SweepState {
    /// Sweep unit is active (period != 0 or shift != 0 at trigger time).
    enabled: bool,
    /// Sweep period in frame sequencer ticks (0 = disabled).
    period: u8,
    /// Counts down each frame sequencer sweep step.
    timer: u8,
    /// Direction: true = subtract (down-sweep), false = add (up-sweep).
    negate: bool,
    /// Right-shift amount for the frequency delta calculation.
    shift: u8,
    /// Shadow copy of ch1's frequency, updated each sweep iteration.
    shadow_frequency: u16,
    /// Set when negate mode was used during the sweep. Used for the
    /// "negate-then-positive" disable quirk.
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
                    channel.sync_frequency_period();
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

/// Wave channel (ch3) — plays arbitrary 4-bit PCM samples from wave RAM.
///
/// The wave channel clocks at 2 MHz (once every 2 T-cycles) rather than 4 MHz.
/// Wave RAM is only accessible from the CPU during the 2 T-cycle window immediately
/// after a sample position advance (`just_read` is true). Outside this window,
/// reads return 0xFF and writes are ignored while ch3 is active.
#[derive(Default)]
struct WaveChannel {
    /// Channel output is active (DAC on and not silenced by length).
    enabled: bool,
    /// DAC powered (NR30 bit 7). When false, channel is immediately disabled.
    dac_enabled: bool,
    /// Counts down to 0; channel disables when it reaches 0 (if length_enabled).
    length_counter: u16,
    /// Whether the length counter is active (NR34 bit 6).
    length_enabled: bool,
    /// Output volume shift code (NR32 bits 6–5): 0=mute, 1=100%, 2=50%, 3=25%.
    volume_code: u8,
    /// 11-bit frequency value. Timer period = `2048 - frequency` in 2MHz ticks.
    frequency: u16,
    /// Cached timer reload period `2048 - frequency`.
    frequency_period: u16,
    /// Counts down 2MHz ticks; reloads to `2048 - frequency` at 0.
    frequency_timer: u16,
    /// Current sample position within the 32-nibble wave table (0–31).
    position: u8,
    /// The wave RAM byte read at the current position (both nibbles).
    sample_buffer: u8,
    /// 16-byte wave RAM table, each byte encoding two 4-bit samples.
    wave_ram: [u8; 16],
    /// True for 2 T-cycles after each position advance. During this window,
    /// CPU reads return `sample_buffer` and CPU writes redirect to the current
    /// position instead of the requested offset (DMG wave RAM access quirk).
    just_read: bool,
}

impl WaveChannel {
    fn sync_frequency_period(&mut self) {
        self.frequency_period = 2048 - self.frequency;
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 256;
        }
        self.sync_frequency_period();
        // Timer counts in 2 MHz cycles. DMG quirk: trigger adds 3 extra 2MHz-cycles
        // to the initial timer reload (does NOT apply to clock_frequency() reload).
        self.frequency_timer = self.frequency_period + 3;
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

    fn digital_output(&self) -> u8 {
        if !self.enabled { return 0; }
        let nibble = if self.position & 1 == 0 {
            self.sample_buffer >> 4
        } else {
            self.sample_buffer & 0x0F
        };
        match self.volume_code {
            1 => nibble,
            2 => nibble >> 1,
            3 => nibble >> 2,
            _ => 0,
        }
    }

    /// Advance wave frequency timer by `n_ticks` 2 MHz ticks (batch path).
    /// Mirrors the per-tick `clock_frequency` logic with skip-ahead arithmetic.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_frequency_wave(&mut self, n_ticks: u16) {
        if n_ticks == 0 { return; }
        // Every 2MHz tick clears just_read at its start; only the last tick can leave it set.
        self.just_read = false;
        if !self.enabled { return; }
        if self.frequency_timer > n_ticks {
            self.frequency_timer -= n_ticks;
            return;
        }
        if self.frequency_period == 0 {
            self.sync_frequency_period();
        }
        let period = self.frequency_period;
        let (fires, final_timer) = if self.frequency_timer == 0 {
            let n = n_ticks as u32;
            let p = period as u32;
            let fires = 1 + (n - 1) / p;
            let rem = (n - 1) % p;
            (fires, if rem == 0 { period } else { period - rem as u16 })
        } else {
            let remaining = (n_ticks - self.frequency_timer) as u32;
            let p = period as u32;
            let fires = 1 + remaining / p;
            let rem = remaining % p;
            (fires, if rem == 0 { period } else { period - rem as u16 })
        };
        self.frequency_timer = final_timer;
        if fires > 0 {
            self.position = self.position.wrapping_add(fires as u8) & 0x1F;
            let byte_index = (self.position / 2) as usize;
            self.sample_buffer = self.wave_ram[byte_index];
            // just_read is true only if the last 2MHz tick fired (timer reloaded at the end)
            self.just_read = final_timer == period;
        }
    }
}

/// Noise channel (ch4) — generates pseudo-random noise via a linear feedback
/// shift register (LFSR).
#[derive(Default)]
struct NoiseChannel {
    /// Channel output is active (DAC on and not silenced by length).
    enabled: bool,
    /// DAC powered. When false, channel is immediately disabled.
    dac_enabled: bool,
    /// Counts down to 0; channel disables when it reaches 0 (if length_enabled).
    length_counter: u16,
    /// Whether the length counter is active (NR44 bit 6).
    length_enabled: bool,
    /// Current output volume (0–15). Modified by the envelope.
    volume: u8,
    /// Initial volume loaded on trigger (NR42 bits 7–4).
    volume_initial: u8,
    /// Envelope direction: true = increase, false = decrease.
    envelope_add: bool,
    /// Envelope sweep period in frame sequencer ticks (0 = disabled).
    envelope_period: u8,
    /// Counts down each frame sequencer envelope step.
    envelope_timer: u8,
    /// LFSR clock shift (NR43 bits 7–4). Timer period = `divisor << clock_shift`.
    clock_shift: u8,
    /// LFSR width mode (NR43 bit 3): false = 15-bit, true = 7-bit.
    width_mode: bool,
    /// Index into `NOISE_DIVISORS` table (NR43 bits 2–0).
    divisor_code: u8,
    /// Cached timer reload period `NOISE_DIVISORS[divisor_code] << clock_shift`.
    /// A value of 0 represents the wrapped overflow case used by the current model.
    frequency_period: u16,
    /// Counts down T-cycles until the next LFSR step.
    frequency_timer: u16,
    /// 15-bit (or 7-bit in width mode) linear feedback shift register.
    /// Bit 0 is the output bit; XOR of bits 0 and 1 feeds back into bit 14 (and 6).
    lfsr: u16,
}

impl NoiseChannel {
    fn sync_frequency_period(&mut self) {
        self.frequency_period = NOISE_DIVISORS[self.divisor_code as usize] << self.clock_shift;
    }

    fn trigger(&mut self) {
        self.enabled = self.dac_enabled;
        if self.length_counter == 0 {
            self.length_counter = 64;
        }
        self.sync_frequency_period();
        self.frequency_timer = self.frequency_period;
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

    fn clock_lfsr(&mut self) {
        let xor_bit = (self.lfsr & 1) ^ ((self.lfsr >> 1) & 1);
        self.lfsr >>= 1;
        self.lfsr |= xor_bit << 14;
        if self.width_mode {
            self.lfsr &= !(1 << 6);
            self.lfsr |= xor_bit << 6;
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_frequency_noise(&mut self, cycles: u16) {
        if cycles == 0 { return; }
        if self.frequency_timer > cycles {
            self.frequency_timer -= cycles;
            return;
        }
        if self.frequency_period == 0 {
            self.sync_frequency_period();
        }
        let period = self.frequency_period;
        if period == 0 {
            // u16 overflow: treat as period=65536, effectively no firing in short batches
            self.frequency_timer = self.frequency_timer.saturating_sub(cycles);
            return;
        }
        let fires = if self.frequency_timer == 0 {
            let n = cycles as u32;
            let p = period as u32;
            let fires = 1 + (n - 1) / p;
            let rem = (n - 1) % p;
            self.frequency_timer = if rem == 0 { period } else { period - rem as u16 };
            fires
        } else {
            let remaining = (cycles - self.frequency_timer) as u32;
            let p = period as u32;
            let fires = 1 + remaining / p;
            let rem = remaining % p;
            self.frequency_timer = if rem == 0 { period } else { period - rem as u16 };
            fires
        };
        for _ in 0..fires { self.clock_lfsr(); }
    }

    fn digital_output(&self) -> u8 {
        if !self.enabled { return 0; }
        // LFSR bit 0 low = high output
        if self.lfsr & 1 == 0 { self.volume } else { 0 }
    }
}

/// Bit 12 of the timer's internal counter (= bit 4 of DIV).
/// The frame sequencer clocks on the falling edge of this bit.
const FRAME_SEQ_BIT: u16 = 1 << 12;

/// Game Boy APU (Audio Processing Unit) peripheral.
///
/// Contains the four sound channels (pulse×2, wave, noise), the frame sequencer
/// that drives length/envelope/sweep clocking, and raw register storage for
/// read-back through the IO bus.
///
/// `tick()` must be called once per T-cycle. The frame sequencer is driven by
/// the falling edge of bit 12 of the timer's internal counter (DIV bit 4).
pub struct ApuPeripheral {
    /// Whether the APU is powered on (NR52 bit 7). When false, all registers
    /// except NRx1 (length) are frozen and channels are silent.
    powered: bool,
    /// Previous state of FRAME_SEQ_BIT used to detect the falling edge.
    prev_div_bit: bool,
    /// Current frame sequencer step (0–7). Advances on each falling edge of
    /// FRAME_SEQ_BIT. Drives length (steps 0,2,4,6), sweep (steps 2,6),
    /// and envelope (step 7) clocking.
    frame_sequencer_step: u8,
    /// Phase divider for ch3's 2 MHz clock. Toggles every T-cycle; ch3's
    /// frequency timer only ticks when this is `true`.
    wave_2mhz_phase: bool,

    channel1: SquareChannel,
    sweep: SweepState,
    channel2: SquareChannel,
    channel3: WaveChannel,
    channel4: NoiseChannel,

    /// Raw register bytes for NR10–NR51 (indices 0–21), stored as written.
    /// Read-back ORs these with `READ_MASKS` to expose write-only bits as 1.
    regs: [u8; 23], // NR10 (0xFF10) through NR52 (0xFF26)

    /// Downsampling accumulator. Incremented by SAMPLE_PERIOD_DEN each T-cycle;
    /// when it reaches SAMPLE_PERIOD_NUM a stereo sample is emitted and the
    /// remainder is kept to avoid pitch drift.
    sample_acc: u32,
    /// Interleaved stereo PCM output buffer: [L, R, L, R, ...], i16 PCM words.
    sample_buffer: alloc::vec::Vec<i16>,
    /// Cached NR50 master-volume gains scaled for direct integer PCM output.
    left_scale: u16,
    right_scale: u16,
    /// Cached NR51 routing bits normalized to channel bits 0..3.
    left_routes: u8,
    right_routes: u8,
    #[cfg(feature = "perf")]
    perf_profile: ApuPerfProfile,
}

impl ApuPeripheral {
    pub fn new() -> Self {
        Self {
            powered: true,
            prev_div_bit: false,
            frame_sequencer_step: 0,
            wave_2mhz_phase: false,
            channel1: SquareChannel::default(),
            sweep: SweepState::default(),
            channel2: SquareChannel::default(),
            channel3: WaveChannel::default(),
            channel4: NoiseChannel::default(),
            regs: [0u8; 23],
            sample_acc: 0,
            sample_buffer: alloc::vec::Vec::with_capacity(SAMPLE_BUFFER_CAPACITY_HINT),
            left_scale: 0,
            right_scale: 0,
            left_routes: 0,
            right_routes: 0,
            #[cfg(feature = "perf")]
            perf_profile: ApuPerfProfile::default(),
        }
    }

    /// Drain and return accumulated PCM samples since the last call.
    /// Returns interleaved stereo f32 samples: [L, R, L, R, ...] in [-1.0, 1.0].
    pub fn drain_samples(&mut self) -> alloc::vec::Vec<f32> {
        let mut raw = alloc::vec::Vec::new();
        core::mem::swap(&mut raw, &mut self.sample_buffer);

        let mut out = alloc::vec::Vec::with_capacity(raw.len());
        for sample in raw.iter().copied() {
            out.push(sample as f32 / 32767.0);
        }

        raw.clear();
        core::mem::swap(&mut raw, &mut self.sample_buffer);
        out
    }

    /// Drain accumulated PCM samples into a caller-owned buffer so the hot
    /// producer path can keep reusing the same allocation across frames.
    pub fn drain_samples_into(&mut self, out: &mut alloc::vec::Vec<i16>) {
        out.clear();
        core::mem::swap(out, &mut self.sample_buffer);
    }

    pub fn clear_samples(&mut self) {
        self.sample_buffer.clear();
        self.sample_acc = 0;
    }

    #[cfg(feature = "perf")]
    pub fn take_perf_profile(&mut self) -> ApuPerfProfile {
        core::mem::take(&mut self.perf_profile)
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
    ///
    /// On DMG, reading wave RAM while ch3 is active only returns valid data
    /// during the 2 T-cycle window when the wave channel reads a new sample
    /// (`just_read` is true). Any other time returns 0xFF, and the returned
    /// byte is always the one at the wave channel's current position (not the
    /// requested offset).
    pub fn read_wave_ram(&self, offset: u8) -> u8 {
        if self.channel3.enabled {
            if self.channel3.just_read {
                self.channel3.sample_buffer
            } else {
                0xFF
            }
        } else {
            self.channel3.wave_ram[offset as usize]
        }
    }

    /// Write wave RAM byte.
    ///
    /// On DMG, writing wave RAM while ch3 is active is ignored unless it
    /// coincides with the wave channel's sample read window (`just_read`).
    /// When it does coincide, the write goes to the wave channel's current
    /// position (not the requested offset).
    pub fn write_wave_ram(&mut self, offset: u8, value: u8) {
        if self.channel3.enabled {
            if self.channel3.just_read {
                let byte_index = (self.channel3.position / 2) as usize;
                self.channel3.wave_ram[byte_index] = value;
            }
            // else: write ignored on DMG when ch3 is active outside read window
        } else {
            self.channel3.wave_ram[offset as usize] = value;
        }
    }

    /// Advance the APU by `cycles` T-cycles.
    ///
    /// `div_counter` is the timer's internal 16-bit counter *after* the timer
    /// has been advanced for these cycles. The frame sequencer clocks on the
    /// falling edge of bit 12 (DIV bit 4).
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn tick(&mut self, cycles: u16, div_counter: u16) -> ApuOutput {
        if !self.powered {
            self.prev_div_bit = div_counter & FRAME_SEQ_BIT != 0;
            return ApuOutput { nr52: self.build_nr52() };
        }

        // Frame sequencer: check O(1) if bit 12 fell anywhere in (div_start, div_counter].
        // A falling edge of bit 12 occurs at every counter value that is a multiple of 8192.
        // Distance from div_start+1 to the next multiple of 8192:
        //   k = (8192 - ((div_start+1) & 8191)) & 8191
        // Edge occurred in batch iff k < cycles.
        let cur_div_bit = div_counter & FRAME_SEQ_BIT != 0;
        let frame_seq_fell = if cycles <= 4 {
            self.prev_div_bit && !cur_div_bit
        } else {
            let div_start = div_counter.wrapping_sub(cycles);
            let k = (0x2000u16.wrapping_sub(div_start.wrapping_add(1) & 0x1FFF)) & 0x1FFF;
            (k as u32) < (cycles as u32)
        };
        if frame_seq_fell {
            #[cfg(feature = "perf")]
            let t0 = crate::cpu::perf::cyccnt();
            self.clock_frame_sequencer();
            #[cfg(feature = "perf")]
            {
                let dt = crate::cpu::perf::cyccnt().wrapping_sub(t0);
                self.perf_profile.frame_seq = self.perf_profile.frame_seq.wrapping_add(dt);
            }
        }
        self.prev_div_bit = cur_div_bit;

        // Square channel frequency timers: skip-ahead arithmetic.
        #[cfg(feature = "perf")]
        let t0 = crate::cpu::perf::cyccnt();
        self.channel1.advance_frequency(cycles);
        self.channel2.advance_frequency(cycles);
        #[cfg(feature = "perf")]
        {
            let dt = crate::cpu::perf::cyccnt().wrapping_sub(t0);
            self.perf_profile.pulse = self.perf_profile.pulse.wrapping_add(dt);
        }

        // Wave channel clocks at 2 MHz (once per 2 T-cycles).
        // Number of 2MHz ticks depends on current phase:
        //   phase=false: fires on odd ticks → ceil(cycles/2)
        //   phase=true:  fires on even ticks → floor(cycles/2)
        let n_wave_ticks = if self.wave_2mhz_phase {
            cycles / 2
        } else {
            (cycles + 1) / 2
        };
        #[cfg(feature = "perf")]
        let t0 = crate::cpu::perf::cyccnt();
        self.channel3.advance_frequency_wave(n_wave_ticks);
        if cycles % 2 != 0 {
            self.wave_2mhz_phase = !self.wave_2mhz_phase;
        }
        #[cfg(feature = "perf")]
        {
            let dt = crate::cpu::perf::cyccnt().wrapping_sub(t0);
            self.perf_profile.wave = self.perf_profile.wave.wrapping_add(dt);
        }

        #[cfg(feature = "perf")]
        let t0 = crate::cpu::perf::cyccnt();
        self.channel4.advance_frequency_noise(cycles);
        #[cfg(feature = "perf")]
        {
            let dt = crate::cpu::perf::cyccnt().wrapping_sub(t0);
            self.perf_profile.noise = self.perf_profile.noise.wrapping_add(dt);
        }

        // Downsample to 48 kHz. The Pico runtime almost always calls this with
        // cycles=1/3/4, so a single-sample fast path avoids a 64-bit divide/mod
        // in the common case while keeping the generic batch path for tests and
        // any larger callers.
        let sample_inc = cycles as u32 * SAMPLE_PERIOD_DEN;
        if cycles <= 4 {
            self.sample_acc += sample_inc;
            if self.sample_acc >= SAMPLE_PERIOD_NUM {
                self.sample_acc -= SAMPLE_PERIOD_NUM;
                #[cfg(feature = "perf")]
                let t0 = crate::cpu::perf::cyccnt();
                let (left, right) = self.mix_sample();
                self.sample_buffer.push(left);
                self.sample_buffer.push(right);
                #[cfg(feature = "perf")]
                {
                    let dt = crate::cpu::perf::cyccnt().wrapping_sub(t0);
                    self.perf_profile.mix = self.perf_profile.mix.wrapping_add(dt);
                }
            }
        } else {
            let acc = self.sample_acc as u64 + sample_inc as u64;
            let n_samples = acc / SAMPLE_PERIOD_NUM as u64;
            self.sample_acc = (acc % SAMPLE_PERIOD_NUM as u64) as u32;
            if n_samples != 0 {
                #[cfg(feature = "perf")]
                let t0 = crate::cpu::perf::cyccnt();
                for _ in 0..n_samples {
                    let (left, right) = self.mix_sample();
                    self.sample_buffer.push(left);
                    self.sample_buffer.push(right);
                }
                #[cfg(feature = "perf")]
                {
                    let dt = crate::cpu::perf::cyccnt().wrapping_sub(t0);
                    self.perf_profile.mix = self.perf_profile.mix.wrapping_add(dt);
                }
            }
        }

        ApuOutput { nr52: self.build_nr52() }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
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

    /// Mix all four channels into a stereo sample pair using NR50/NR51.
    /// Returns signed 16-bit PCM values packed as an interleaved stereo pair.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn mix_sample(&self) -> (i16, i16) {
        let ch1 = self.channel1.digital_output() as u16;
        let ch2 = self.channel2.digital_output() as u16;
        let ch3 = self.channel3.digital_output() as u16;
        let ch4 = self.channel4.digital_output() as u16;

        let mut left = 0u16;
        let mut right = 0u16;

        if self.left_routes & 0x01 != 0 { left += ch1; }
        if self.left_routes & 0x02 != 0 { left += ch2; }
        if self.left_routes & 0x04 != 0 { left += ch3; }
        if self.left_routes & 0x08 != 0 { left += ch4; }

        if self.right_routes & 0x01 != 0 { right += ch1; }
        if self.right_routes & 0x02 != 0 { right += ch2; }
        if self.right_routes & 0x04 != 0 { right += ch3; }
        if self.right_routes & 0x08 != 0 { right += ch4; }

        (
            (left as u32 * self.left_scale as u32) as i16,
            (right as u32 * self.right_scale as u32) as i16,
        )
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
            self.sync_mixer_cache();
        } else if !was_powered && self.powered {
            // Power on: reset frame sequencer
            self.frame_sequencer_step = 0;
            self.sync_mixer_cache();
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

    fn sync_mixer_cache(&mut self) {
        let nr50 = self.regs[20];
        let left_vol = ((nr50 >> 4) & 0x07) as u32 + 1;
        let right_vol = (nr50 & 0x07) as u32 + 1;
        self.left_scale = ((left_vol * 32767) / MIXER_MAX) as u16;
        self.right_scale = ((right_vol * 32767) / MIXER_MAX) as u16;

        let nr51 = self.regs[21];
        self.left_routes = (nr51 >> 4) & 0x0F;
        self.right_routes = nr51 & 0x0F;
    }

    fn apply_register_write(&mut self, address: u16, value: u8) {
        match address {
            // ── Channel 1 (pulse + sweep) ─────────────────────────────────
            0xFF10 => self.write_nr10_sweep(value),
            0xFF11 => {
                // NR11: duty pattern (bits 7–6) and length counter (bits 5–0)
                self.channel1.duty = (value >> 6) & 0x03;
                self.channel1.length_counter = 64 - (value & 0x3F) as u16;
            }
            0xFF12 => self.write_ch1_envelope(value),
            0xFF13 => {
                // NR13: frequency low byte
                self.channel1.frequency = (self.channel1.frequency & 0x700) | value as u16;
                self.channel1.sync_frequency_period();
            }
            0xFF14 => self.write_ch1_trigger(value),

            // ── Channel 2 (pulse) ─────────────────────────────────────────
            0xFF16 => {
                // NR21: duty pattern (bits 7–6) and length counter (bits 5–0)
                self.channel2.duty = (value >> 6) & 0x03;
                self.channel2.length_counter = 64 - (value & 0x3F) as u16;
            }
            0xFF17 => self.write_ch2_envelope(value),
            0xFF18 => {
                // NR23: frequency low byte
                self.channel2.frequency = (self.channel2.frequency & 0x700) | value as u16;
                self.channel2.sync_frequency_period();
            }
            0xFF19 => self.write_ch2_trigger(value),

            // ── Channel 3 (wave) ──────────────────────────────────────────
            0xFF1A => {
                // NR30: DAC enable (bit 7)
                self.channel3.dac_enabled = value & 0x80 != 0;
                if !self.channel3.dac_enabled {
                    self.channel3.enabled = false;
                }
            }
            0xFF1B => {
                // NR31: length counter (full byte, max 256)
                self.channel3.length_counter = 256 - value as u16;
            }
            0xFF1C => {
                // NR32: output volume code (bits 6–5): 0=mute, 1=100%, 2=50%, 3=25%
                self.channel3.volume_code = (value >> 5) & 0x03;
            }
            0xFF1D => {
                // NR33: frequency low byte
                self.channel3.frequency = (self.channel3.frequency & 0x700) | value as u16;
                self.channel3.sync_frequency_period();
            }
            0xFF1E => self.write_ch3_trigger(value),

            // ── Channel 4 (noise) ─────────────────────────────────────────
            0xFF20 => {
                // NR41: length counter (bits 5–0, max 64)
                self.channel4.length_counter = 64 - (value & 0x3F) as u16;
            }
            0xFF21 => self.write_ch4_envelope(value),
            0xFF22 => {
                // NR43: LFSR clock shift (bits 7–4), width mode (bit 3), divisor (bits 2–0)
                self.channel4.clock_shift = (value >> 4) & 0x0F;
                self.channel4.width_mode = value & 0x08 != 0;
                self.channel4.divisor_code = value & 0x07;
                self.channel4.sync_frequency_period();
            }
            0xFF23 => self.write_ch4_trigger(value),

            // NR50, NR51: master volume / stereo panning — stored in regs[] only
            0xFF24 | 0xFF25 => self.sync_mixer_cache(),
            _ => {}
        }
    }

    // ── NRx2 volume envelope + DAC helpers ──────────────────────────────────

    /// NR12: ch1 volume envelope and DAC. DAC is disabled (and channel silenced)
    /// when the upper 5 bits are all zero (no initial volume and no add mode).
    fn write_ch1_envelope(&mut self, value: u8) {
        self.channel1.volume_initial = (value >> 4) & 0x0F;
        self.channel1.envelope_add = value & 0x08 != 0;
        self.channel1.envelope_period = value & 0x07;
        self.channel1.dac_enabled = value & 0xF8 != 0;
        if !self.channel1.dac_enabled {
            self.channel1.enabled = false;
        }
    }

    /// NR22: ch2 volume envelope and DAC.
    fn write_ch2_envelope(&mut self, value: u8) {
        self.channel2.volume_initial = (value >> 4) & 0x0F;
        self.channel2.envelope_add = value & 0x08 != 0;
        self.channel2.envelope_period = value & 0x07;
        self.channel2.dac_enabled = value & 0xF8 != 0;
        if !self.channel2.dac_enabled {
            self.channel2.enabled = false;
        }
    }

    /// NR42: ch4 volume envelope and DAC.
    fn write_ch4_envelope(&mut self, value: u8) {
        self.channel4.volume_initial = (value >> 4) & 0x0F;
        self.channel4.envelope_add = value & 0x08 != 0;
        self.channel4.envelope_period = value & 0x07;
        self.channel4.dac_enabled = value & 0xF8 != 0;
        if !self.channel4.dac_enabled {
            self.channel4.enabled = false;
        }
    }

    // ── NRx4 trigger helpers ─────────────────────────────────────────────────

    /// NR10: sweep period, direction, and shift. The "negate-then-positive" quirk
    /// disables ch1 if sweep was used in subtract mode and is now switched to add.
    fn write_nr10_sweep(&mut self, value: u8) {
        self.sweep.period = (value >> 4) & 0x07;
        let new_negate = value & 0x08 != 0;
        if self.sweep.negate_used && self.sweep.negate && !new_negate {
            self.channel1.enabled = false;
        }
        self.sweep.negate = new_negate;
        self.sweep.shift = value & 0x07;
    }

    /// NR14: ch1 frequency high bits, length enable, and trigger.
    ///
    /// Trigger initialises the channel and fires the sweep unit. The sweep unit
    /// does an immediate overflow check when shift is nonzero.
    fn write_ch1_trigger(&mut self, value: u8) {
        self.channel1.frequency =
            (self.channel1.frequency & 0xFF) | ((value as u16 & 0x07) << 8);
        self.channel1.sync_frequency_period();
        let new_len_enable = value & 0x40 != 0;
        let on_length_step = self.frame_step_clocks_length();
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
            if len_was_zero && new_len_enable && on_length_step {
                self.channel1.clock_length();
            }
        }
    }

    /// NR19: ch2 frequency high bits, length enable, and trigger.
    fn write_ch2_trigger(&mut self, value: u8) {
        self.channel2.frequency =
            (self.channel2.frequency & 0xFF) | ((value as u16 & 0x07) << 8);
        self.channel2.sync_frequency_period();
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

    /// NR1E: ch3 frequency high bits, length enable, and trigger.
    ///
    /// Includes the DMG wave RAM corruption quirk: retriggering while ch3 is
    /// active and the wave timer is about to fire (`frequency_timer == 1`)
    /// corrupts the first 4 bytes of wave RAM based on the upcoming position.
    fn write_ch3_trigger(&mut self, value: u8) {
        self.channel3.frequency =
            (self.channel3.frequency & 0xFF) | ((value as u16 & 0x07) << 8);
        self.channel3.sync_frequency_period();
        let new_len_enable = value & 0x40 != 0;
        let on_length_step = self.frame_step_clocks_length();
        if new_len_enable && !self.channel3.length_enabled && on_length_step {
            self.channel3.length_enabled = true;
            self.channel3.clock_length();
        }
        self.channel3.length_enabled = new_len_enable;
        if value & 0x80 != 0 {
            let len_was_zero = self.channel3.length_counter == 0;
            self.apply_ch3_retrigger_corruption();
            self.channel3.trigger();
            if len_was_zero && new_len_enable && on_length_step {
                self.channel3.clock_length();
            }
        }
    }

    /// NR23: ch4 length enable and trigger.
    fn write_ch4_trigger(&mut self, value: u8) {
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

    /// DMG wave RAM corruption on retrigger: when ch3 is active and its frequency
    /// timer is 1 (about to fire on the next 2MHz tick), retriggering corrupts
    /// the first 4 bytes of wave RAM. If the upcoming position byte is in the first
    /// block (bytes 0–3), only that byte is copied to byte 0. Otherwise the entire
    /// 4-byte block containing that position is copied into bytes 0–3.
    fn apply_ch3_retrigger_corruption(&mut self) {
        if !self.channel3.enabled || self.channel3.frequency_timer != 1 {
            return;
        }
        let next_pos_byte = ((self.channel3.position + 1) / 2) as usize & 0x0F;
        if next_pos_byte < 4 {
            self.channel3.wave_ram[0] = self.channel3.wave_ram[next_pos_byte];
        } else {
            let block_start = next_pos_byte & !3;
            self.channel3.wave_ram[0] = self.channel3.wave_ram[block_start];
            self.channel3.wave_ram[1] = self.channel3.wave_ram[block_start + 1];
            self.channel3.wave_ram[2] = self.channel3.wave_ram[block_start + 2];
            self.channel3.wave_ram[3] = self.channel3.wave_ram[block_start + 3];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME_SEQUENCER_PERIOD: u16 = 8192;

    fn advance_square_reference(channel: &mut SquareChannel, cycles: u16) {
        if cycles == 0 {
            return;
        }
        let period = (2048 - channel.frequency) * 4;
        let (fires, final_timer) = if channel.frequency_timer == 0 {
            let n = cycles as u32;
            let p = period as u32;
            let fires = 1 + (n - 1) / p;
            let rem = (n - 1) % p;
            (fires, if rem == 0 { period } else { period - rem as u16 })
        } else if channel.frequency_timer > cycles {
            (0u32, channel.frequency_timer - cycles)
        } else {
            let remaining = (cycles - channel.frequency_timer) as u32;
            let p = period as u32;
            let fires = 1 + remaining / p;
            let rem = remaining % p;
            (fires, if rem == 0 { period } else { period - rem as u16 })
        };
        channel.frequency_timer = final_timer;
        if fires > 0 {
            channel.duty_position = ((channel.duty_position as u32 + fires) % 8) as u8;
        }
    }

    fn advance_wave_reference(channel: &mut WaveChannel, n_ticks: u16) {
        if n_ticks == 0 {
            return;
        }
        channel.just_read = false;
        if !channel.enabled {
            return;
        }
        let period = 2048 - channel.frequency;
        let (fires, final_timer) = if channel.frequency_timer == 0 {
            let n = n_ticks as u32;
            let p = period as u32;
            let fires = 1 + (n - 1) / p;
            let rem = (n - 1) % p;
            (fires, if rem == 0 { period } else { period - rem as u16 })
        } else if channel.frequency_timer > n_ticks {
            (0u32, channel.frequency_timer - n_ticks)
        } else {
            let remaining = (n_ticks - channel.frequency_timer) as u32;
            let p = period as u32;
            let fires = 1 + remaining / p;
            let rem = remaining % p;
            (fires, if rem == 0 { period } else { period - rem as u16 })
        };
        channel.frequency_timer = final_timer;
        if fires > 0 {
            channel.position = ((channel.position as u32 + fires) % 32) as u8;
            let byte_index = (channel.position / 2) as usize;
            channel.sample_buffer = channel.wave_ram[byte_index];
            channel.just_read = final_timer == period;
        }
    }

    fn advance_noise_reference(channel: &mut NoiseChannel, cycles: u16) {
        if cycles == 0 {
            return;
        }
        let period = NOISE_DIVISORS[channel.divisor_code as usize] << channel.clock_shift;
        if period == 0 {
            channel.frequency_timer = channel.frequency_timer.saturating_sub(cycles);
            return;
        }
        let fires = if channel.frequency_timer == 0 {
            let n = cycles as u32;
            let p = period as u32;
            let fires = 1 + (n - 1) / p;
            let rem = (n - 1) % p;
            channel.frequency_timer = if rem == 0 { period } else { period - rem as u16 };
            fires
        } else if channel.frequency_timer > cycles {
            channel.frequency_timer -= cycles;
            0u32
        } else {
            let remaining = (cycles - channel.frequency_timer) as u32;
            let p = period as u32;
            let fires = 1 + remaining / p;
            let rem = remaining % p;
            channel.frequency_timer = if rem == 0 { period } else { period - rem as u16 };
            fires
        };
        for _ in 0..fires {
            channel.clock_lfsr();
        }
    }

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
        // Advance 8 T-cycles to expire timer and advance LFSR once
        apu.channel4.advance_frequency_noise(8);
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

    /// Verifies wave channel phase at the read point for test 09 iteration 1.
    ///
    /// Iteration 1: a=0x99, freq=0x799, initial_timer = (2048-0x799)+3 = 106.
    /// T-cycle sequence (trigger → freq change → delay → read):
    ///   NR34 bus_write:  3T advance + trigger + 1T = 4T total
    ///   wreg NR33,-2:    4 tick_cycles (ld a + ldh opcode + ldh read_n) + 3T advance + write + 1T = 20T
    ///   delay_clocks 176: 44 tick_cycles = 176T
    ///   lda WAVE:        2 tick_cycles (opcode + read_n) + 3T advance + read = 11T
    ///
    /// 2MHz-ticks from trigger to freq change: 20T/2 = 10
    /// Timer at freq change: 106 - 10 = 96
    /// 2MHz-ticks from freq change to read: (1+176+8+3)T / 2 = 188T/2 = 94
    /// Since 96 > 94: timer never fires → position stays 0, just_read=false, read returns 0xFF.
    #[test]
    fn probe_wave_phase_at_read() {
        let wave_data: [u8; 16] = [
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
            0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
        ];
        let a: u8 = 0x99;

        // ---- Simulate from a fresh APU, matching bus_write(NR34)/bus_write(NR33)/delay/bus_read ----
        // Each bus_write for APU regs: advance_apu(1) × 3, write, advance_apu(1) × 1
        // Each tick_cycle (non-APU): advance_apu(1) × 4
        // bus_read for WAVE: advance_apu(1) × 3, read, advance_apu(1) × 1

        let mut apu = ApuPeripheral::new();
        let mut div: u16 = 0;

        // Write wave RAM while ch3 disabled (load_wave)
        for i in 0..16u8 {
            apu.write_wave_ram(i, wave_data[i as usize]);
        }
        apu.write_register(0xFF1A, 0x80); // NR30: DAC on
        apu.write_register(0xFF1C, 0x00); // NR32: silent
        apu.write_register(0xFF1D, a);    // NR33: initial freq lo

        // bus_write NR34 = 0x87 (trigger):  3 advance + write + 1 advance = 4T
        for _ in 0..3 { div = div.wrapping_add(1); apu.tick(1, div); }
        apu.write_register(0xFF1E, 0x87);
        div = div.wrapping_add(1); apu.tick(1, div);

        // wreg NR33,-2 overhead: ld a,$FE (2M=8T) + ldh opcode(4T) + ldh read_n(4T) = 4 tick_cycles = 16T
        // Then bus_write NR33: 3 advance + write + 1 advance = 4T
        // Total: 20T (10 2MHz-ticks) from trigger to freq change
        for _ in 0..16 { div = div.wrapping_add(1); apu.tick(1, div); } // 4 tick_cycles × 4T
        for _ in 0..3 { div = div.wrapping_add(1); apu.tick(1, div); }
        apu.write_register(0xFF1D, 0xFE); // freq = 0x7FE
        div = div.wrapping_add(1); apu.tick(1, div);

        // delay_clocks 176 = 44 M-cycles = 176 T-cycles = 176 tick calls
        for _ in 0..176 { div = div.wrapping_add(1); apu.tick(1, div); }

        // bus_read WAVE: ldh opcode(4T) + ldh read_n(4T) = 8 tick calls, then 3T advance + read + 1T
        for _ in 0..8 { div = div.wrapping_add(1); apu.tick(1, div); }
        for _ in 0..3 { div = div.wrapping_add(1); apu.tick(1, div); }
        let value = apu.read_wave_ram(0);
        div = div.wrapping_add(1); apu.tick(1, div);

        let pos = apu.channel3.position;
        let timer = apu.channel3.frequency_timer;
        let just_read = apu.channel3.just_read;
        // iteration 1 is non-coincident: timer 96 > 94 ticks available → no advance
        // position stays 0, just_read=false, value=0xFF
        assert_eq!(
            (just_read, pos, timer, value),
            (false, 0u8, 2u16, 0xFFu8),
            "wave state: value={:02X} pos={} timer={} just_read={}",
            value, pos, timer, just_read
        );
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

    #[test]
    fn test_square_small_cycle_path_matches_reference() {
        for &frequency in &[0u16, 1, 511, 1024, 2047] {
            for &frequency_timer in &[0u16, 1, 2, 3, 4, 7, 32, 255] {
                for &duty_position in &[0u8, 3, 7] {
                    for &cycles in &[1u16, 2, 3, 4] {
                        let mut fast = SquareChannel {
                            frequency,
                            frequency_period: (2048 - frequency) * 4,
                            frequency_timer,
                            duty_position,
                            ..Default::default()
                        };
                        let mut reference = SquareChannel {
                            frequency,
                            frequency_period: (2048 - frequency) * 4,
                            frequency_timer,
                            duty_position,
                            ..Default::default()
                        };

                        fast.advance_frequency(cycles);
                        advance_square_reference(&mut reference, cycles);

                        assert_eq!(
                            (fast.frequency_timer, fast.duty_position),
                            (reference.frequency_timer, reference.duty_position),
                            "square mismatch: freq={frequency} timer={frequency_timer} duty={duty_position} cycles={cycles}",
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn test_wave_small_cycle_path_matches_reference() {
        let mut wave_ram = [0u8; 16];
        for (idx, byte) in wave_ram.iter_mut().enumerate() {
            *byte = (idx as u8).wrapping_mul(0x11);
        }

        for &enabled in &[false, true] {
            for &frequency in &[0u16, 1, 1024, 2047] {
                for &frequency_timer in &[0u16, 1, 2, 3, 4, 7] {
                    for &position in &[0u8, 1, 14, 31] {
                        for &n_ticks in &[1u16, 2] {
                            let mut fast = WaveChannel {
                                enabled,
                                frequency,
                                frequency_period: 2048 - frequency,
                                frequency_timer,
                                position,
                                sample_buffer: 0xAA,
                                wave_ram,
                                just_read: true,
                                ..Default::default()
                            };
                            let mut reference = WaveChannel {
                                enabled,
                                frequency,
                                frequency_period: 2048 - frequency,
                                frequency_timer,
                                position,
                                sample_buffer: 0xAA,
                                wave_ram,
                                just_read: true,
                                ..Default::default()
                            };

                            fast.advance_frequency_wave(n_ticks);
                            advance_wave_reference(&mut reference, n_ticks);

                            assert_eq!(
                                (
                                    fast.frequency_timer,
                                    fast.position,
                                    fast.sample_buffer,
                                    fast.just_read,
                                ),
                                (
                                    reference.frequency_timer,
                                    reference.position,
                                    reference.sample_buffer,
                                    reference.just_read,
                                ),
                                "wave mismatch: enabled={enabled} freq={frequency} timer={frequency_timer} pos={position} ticks={n_ticks}",
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_noise_small_cycle_path_matches_reference() {
        for &clock_shift in &[0u8, 1, 7, 15] {
            for &divisor_code in &[0u8, 1, 7] {
                for &width_mode in &[false, true] {
                    for &frequency_timer in &[0u16, 1, 2, 3, 8, 32] {
                        for &cycles in &[1u16, 2, 3, 4] {
                            let mut fast = NoiseChannel {
                                clock_shift,
                                divisor_code,
                                width_mode,
                                frequency_period: NOISE_DIVISORS[divisor_code as usize] << clock_shift,
                                frequency_timer,
                                lfsr: 0x5A5A,
                                ..Default::default()
                            };
                            let mut reference = NoiseChannel {
                                clock_shift,
                                divisor_code,
                                width_mode,
                                frequency_period: NOISE_DIVISORS[divisor_code as usize] << clock_shift,
                                frequency_timer,
                                lfsr: 0x5A5A,
                                ..Default::default()
                            };

                            fast.advance_frequency_noise(cycles);
                            advance_noise_reference(&mut reference, cycles);

                            assert_eq!(
                                (fast.frequency_timer, fast.lfsr),
                                (reference.frequency_timer, reference.lfsr),
                                "noise mismatch: shift={clock_shift} divisor={divisor_code} width={width_mode} timer={frequency_timer} cycles={cycles}",
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_sample_generation() {
        let mut apu = ApuPeripheral::new();
        // Seed DMG post-boot state
        apu.write_register(0xFF26, 0xF1); // NR52: APU on, ch1 active
        apu.write_register(0xFF25, 0xF3); // NR51: panning
        apu.write_register(0xFF24, 0x77); // NR50: max volume
        // Trigger ch1 with audible settings
        apu.write_register(0xFF12, 0xF3); // NR12: volume=15, envelope up, period=3
        apu.write_register(0xFF11, 0x80); // NR11: duty=2 (50%), length=0
        apu.write_register(0xFF13, 0x00); // NR13: freq lo
        apu.write_register(0xFF14, 0x87); // NR14: trigger, freq hi=7

        let mut div: u16 = 0;
        for _ in 0..70224u32 {
            div = div.wrapping_add(1);
            apu.tick(1, div);
        }
        let samples = apu.drain_samples();
        assert!(samples.len() > 0, "no samples generated");
        let nonzero = samples.iter().any(|&s| s != 0.0);
        let max = samples.iter().cloned().fold(0.0f32, f32::max);
        assert!(nonzero, "all samples zero: max={} nr50={:#04x} nr51={:#04x}",
            max, apu.regs[20], apu.regs[21]);
    }
}
