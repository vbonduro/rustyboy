#![cfg(target_arch = "arm")]

const AUDIO_BUF_SIZE: usize = 1024;

// Double-buffer for I2S DMA: two back-to-back static arrays in .bss.
static mut AUDIO_BUF_A: [u32; AUDIO_BUF_SIZE] = [0u32; AUDIO_BUF_SIZE];
static mut AUDIO_BUF_B: [u32; AUDIO_BUF_SIZE] = [0u32; AUDIO_BUF_SIZE];

pub const SAMPLE_RATE: u32 = 48_000;

pub struct AudioBuffers {
    use_a_as_front: bool,
    front_n: usize,
}

impl AudioBuffers {
    pub const fn new() -> Self {
        Self {
            use_a_as_front: true,
            front_n: 0,
        }
    }

    pub fn front_back_buffers(&self) -> (&'static [u32], &'static mut [u32]) {
        // Safety: A and B are separate statics and are never aliased — one is
        // read-only (DMA) and the other is write-only (APU fill) per iteration.
        unsafe {
            if self.use_a_as_front {
                (
                    core::slice::from_raw_parts(
                        core::ptr::addr_of!(AUDIO_BUF_A).cast::<u32>(),
                        self.front_n,
                    ),
                    core::slice::from_raw_parts_mut(
                        core::ptr::addr_of_mut!(AUDIO_BUF_B).cast::<u32>(),
                        AUDIO_BUF_SIZE,
                    ),
                )
            } else {
                (
                    core::slice::from_raw_parts(
                        core::ptr::addr_of!(AUDIO_BUF_B).cast::<u32>(),
                        self.front_n,
                    ),
                    core::slice::from_raw_parts_mut(
                        core::ptr::addr_of_mut!(AUDIO_BUF_A).cast::<u32>(),
                        AUDIO_BUF_SIZE,
                    ),
                )
            }
        }
    }

    pub fn queue_next_frame(&mut self, samples: &[f32], back_buf: &mut [u32]) {
        let back_n = samples_to_i2s(samples, back_buf);
        self.use_a_as_front = !self.use_a_as_front;
        self.front_n = back_n;
    }
}

/// Pack interleaved stereo f32 samples [L, R, L, R …] into I2S u32 words.
///
/// Each word carries one stereo pair: left channel in bits 31:16, right in
/// bits 15:0.  Converts f32 [-1.0, 1.0] → i16 and reinterprets as u16.
fn samples_to_i2s(samples: &[f32], buf: &mut [u32]) -> usize {
    let pairs = (samples.len() / 2).min(buf.len());
    for i in 0..pairs {
        let l = (samples[i * 2] * 32767.0) as i16;
        let r = (samples[i * 2 + 1] * 32767.0) as i16;
        buf[i] = ((l as u16 as u32) << 16) | (r as u16 as u32);
    }
    pairs
}
