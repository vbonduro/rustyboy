// Animated title-screen background generated from gemini_generated_video_C5B695AC.mov.
// Source frames are 160x144 at 12 fps with a shared 64-color palette.

pub const FRAME_COUNT: usize = 120;
pub const FRAME_SIZE: usize = 23040; // 160 * 144
pub const PALETTE_COLORS: usize = 64;
pub const FRAME_DURATION_MS: u32 = 83;
const PALETTE_BYTES: usize = PALETTE_COLORS * 4;

pub const LANDSCAPE_BG_DATA: &[u8] = include_bytes!("landscape_bg.bin");

pub fn get_frame_rgba(frame_index: usize) -> Vec<u8> {
    let idx = frame_index % FRAME_COUNT;
    let start = PALETTE_BYTES + idx * FRAME_SIZE;
    let end = start + FRAME_SIZE;

    let mut rgba = Vec::with_capacity(FRAME_SIZE * 4);
    for &palette_idx in &LANDSCAPE_BG_DATA[start..end] {
        let color_start = palette_idx as usize * 4;
        let color_end = color_start + 4;
        rgba.extend_from_slice(&LANDSCAPE_BG_DATA[color_start..color_end]);
    }
    rgba
}
