/// PPU register addresses.
pub(crate) const LCDC_ADDR: u16 = 0xFF40;
pub(crate) const STAT_ADDR: u16 = 0xFF41;
pub(crate) const LY_ADDR: u16 = 0xFF44;
pub(crate) const BGP_ADDR: u16 = 0xFF47;
pub(crate) const OBP0_ADDR: u16 = 0xFF48;
pub(crate) const OBP1_ADDR: u16 = 0xFF49;

pub(crate) const VBLANK_INTERRUPT_BIT: u8 = 0;
pub(crate) const STAT_INTERRUPT_BIT: u8 = 1;

/// IO array offsets (io[i] = memory at 0xFF00 + i).
const LCDC_IO: usize = 0x40;
const STAT_IO: usize = 0x41;
const SCY_IO: usize = 0x42;
const SCX_IO: usize = 0x43;
const LY_IO: usize = 0x44;
const LYC_IO: usize = 0x45;
const BGP_IO: usize = 0x47;
const OBP0_IO: usize = 0x48;
const OBP1_IO: usize = 0x49;
const WY_IO: usize = 0x4A;
const WX_IO: usize = 0x4B;

const DOTS_PER_SCANLINE: u16 = 456;
const OAM_SCAN_DOTS: u16 = 80;
const PIXEL_TRANSFER_DOTS: u16 = 172;
const VISIBLE_SCANLINES: u8 = 144;
const TOTAL_SCANLINES: u8 = 154;

const SCREEN_WIDTH: usize = 160;
const SCREEN_HEIGHT: usize = 144;
pub const FRAMEBUFFER_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT;

/// PPU rendering mode.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum PpuMode {
    HBlank = 0,
    VBlank = 1,
    OamScan = 2,
    PixelTransfer = 3,
}

// SAFETY: HBlank = 0 is a valid discriminant; all-zero bytes produce a valid PpuMode.
unsafe impl bytemuck::Zeroable for PpuMode {}

/// Bitfield accessor for the LCDC register.
#[derive(Clone, Copy)]
struct Lcdc(u8);

impl Lcdc {
    fn lcd_enabled(self) -> bool {
        self.0 & 0x80 != 0
    }
    fn window_tilemap_high(self) -> bool {
        self.0 & 0x40 != 0
    }
    fn window_enabled(self) -> bool {
        self.0 & 0x20 != 0
    }
    fn bg_tile_data_unsigned(self) -> bool {
        self.0 & 0x10 != 0
    }
    fn bg_tilemap_high(self) -> bool {
        self.0 & 0x08 != 0
    }
    fn obj_tall(self) -> bool {
        self.0 & 0x04 != 0
    }
    fn obj_enabled(self) -> bool {
        self.0 & 0x02 != 0
    }
    fn bg_enabled(self) -> bool {
        self.0 & 0x01 != 0
    }
}

/// Result of a PPU tick.
pub struct PpuOutput {
    pub vblank_interrupt: bool,
    pub stat_interrupt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PpuRasterRequest {
    pub ly: u8,
    pub window_line_counter: u8,
}

pub struct PpuTimingOutput {
    pub vblank_interrupt: bool,
    pub stat_interrupt: bool,
    pub render_scanline: Option<PpuRasterRequest>,
    pub lcd_reset: bool,
}

/// Scanline-based PPU peripheral.
///
/// Config registers (LCDC, STAT, SCY, SCX, LYC, BGP, OBP0, OBP1, WY, WX) live
/// in `memory.io[]` — the single source of truth. `tick()` receives an `io: &mut [u8]`
/// slice and reads config from it, writing LY and STAT back after each tick so CPU
/// reads see current values.
pub struct PpuPeripheral {
    dot: u16,
    ly: u8,
    pub(crate) mode: PpuMode,
    window_line_counter: u8,
    prev_stat_line: bool,
    framebuffer: [u8; FRAMEBUFFER_SIZE],
    /// Raw BG/window color indices (0–3) for the current scanline, used for sprite priority.
    bg_color_indices: [u8; SCREEN_WIDTH],
}

// SAFETY: Every field is a primitive, bool, or byte array — all valid when zeroed.
// mode = 0 is HBlank, a valid PpuMode discriminant. Callers must set mode = OamScan
// after construction if that is the intended initial state.
unsafe impl bytemuck::Zeroable for PpuPeripheral {}

impl PpuPeripheral {
    pub fn new() -> Self {
        Self {
            dot: 0,
            ly: 0,
            mode: PpuMode::OamScan,
            window_line_counter: 0,
            prev_stat_line: false,
            framebuffer: [0u8; FRAMEBUFFER_SIZE],
            bg_color_indices: [0u8; SCREEN_WIDTH],
        }
    }

    pub fn ly(&self) -> u8 {
        self.ly
    }

    /// Sync `prev_stat_line` to the current STAT line state, preventing a
    /// spurious rising-edge interrupt when seeding the PPU to a mid-frame
    /// state (e.g. after DMG post-boot register initialization or save-state load).
    pub fn sync_prev_stat_line(&mut self, io: &[u8]) {
        let lyc = io[LYC_IO];
        let stat = io[STAT_IO];
        let lyc_match = self.ly == lyc;
        let stat_line = (lyc_match && (stat & 0x40 != 0))
            || (self.mode == PpuMode::HBlank && (stat & 0x08 != 0))
            || (self.mode == PpuMode::VBlank && (stat & 0x10 != 0))
            || (self.mode == PpuMode::OamScan && (stat & 0x20 != 0));
        self.prev_stat_line = stat_line;
    }

    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        &self.framebuffer
    }

    pub fn clear_framebuffer(&mut self) {
        self.framebuffer = [0u8; FRAMEBUFFER_SIZE];
        self.bg_color_indices = [0u8; SCREEN_WIDTH];
    }

    /// Extract PPU state for serialization. Reads config registers from `io`.
    pub fn to_save_state(&self, io: &[u8]) -> crate::cpu::save_state::PpuState {
        crate::cpu::save_state::PpuState {
            dot: self.dot,
            ly: self.ly,
            mode: self.mode,
            window_line_counter: self.window_line_counter,
            lcdc: io[LCDC_IO],
            stat: io[STAT_IO],
            scy: io[SCY_IO],
            scx: io[SCX_IO],
            lyc: io[LYC_IO],
            bgp: io[BGP_IO],
            obp0: io[OBP0_IO],
            obp1: io[OBP1_IO],
            wy: io[WY_IO],
            wx: io[WX_IO],
        }
    }

    /// Apply PPU internal state from a parsed [`PpuState`].
    /// Config registers (lcdc, stat, etc.) are restored via memory.load_state().
    pub fn load_state(&mut self, state: crate::cpu::save_state::PpuState) {
        self.dot = state.dot;
        self.ly = state.ly;
        self.mode = state.mode;
        self.window_line_counter = state.window_line_counter;
    }

    /// Reset LY to 0 (triggered by CPU write to the LY register).
    pub fn reset_ly(&mut self) {
        self.ly = 0;
    }

    pub fn render_scanline_from_snapshot(
        &mut self,
        io: &[u8],
        vram: &[u8],
        oam: &[u8],
        ly: u8,
        window_line_counter: u8,
    ) {
        self.ly = ly;
        self.window_line_counter = window_line_counter;
        self.render_scanline(io, vram, oam);
    }

    /// Advance the PPU by `cycles` T-cycles.
    ///
    /// `io` is the IO register slice (0xFF00–0xFF7F). Config registers are read
    /// from it; LY and STAT are written back so CPU reads see current values.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn tick(&mut self, cycles: u16, io: &mut [u8], vram: &[u8], oam: &[u8]) -> PpuOutput {
        let lcdc = Lcdc(io[LCDC_IO]);

        if !lcdc.lcd_enabled() {
            self.reset_lcd(io);
            return PpuOutput {
                vblank_interrupt: false,
                stat_interrupt: false,
            };
        }

        let mut vblank_interrupt = false;
        let mut remaining = cycles;

        while remaining > 0 {
            let threshold = match self.mode {
                PpuMode::OamScan => OAM_SCAN_DOTS,
                PpuMode::PixelTransfer => OAM_SCAN_DOTS + PIXEL_TRANSFER_DOTS,
                PpuMode::HBlank | PpuMode::VBlank => DOTS_PER_SCANLINE,
            };
            let dots_to_threshold = threshold.saturating_sub(self.dot);

            if dots_to_threshold > 0 && remaining < dots_to_threshold {
                self.dot += remaining;
                break;
            }

            self.dot += dots_to_threshold;
            remaining -= dots_to_threshold;

            match self.mode {
                PpuMode::OamScan => {
                    self.mode = PpuMode::PixelTransfer;
                }
                PpuMode::PixelTransfer => {
                    self.mode = PpuMode::HBlank;
                    self.render_scanline(io, vram, oam);
                }
                PpuMode::HBlank => {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly >= VISIBLE_SCANLINES {
                        self.mode = PpuMode::VBlank;
                        vblank_interrupt = true;
                    } else {
                        self.mode = PpuMode::OamScan;
                    }
                }
                PpuMode::VBlank => {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly >= TOTAL_SCANLINES {
                        self.ly = 0;
                        self.mode = PpuMode::OamScan;
                        self.window_line_counter = 0;
                    }
                }
            }
        }

        let stat_interrupt = self.build_stat(io);

        io[LY_IO] = self.ly;

        PpuOutput {
            vblank_interrupt,
            stat_interrupt,
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn tick_timing_only(&mut self, cycles: u16, io: &mut [u8]) -> PpuTimingOutput {
        let lcdc = Lcdc(io[LCDC_IO]);

        if !lcdc.lcd_enabled() {
            self.reset_lcd(io);
            return PpuTimingOutput {
                vblank_interrupt: false,
                stat_interrupt: false,
                render_scanline: None,
                lcd_reset: true,
            };
        }

        let mut vblank_interrupt = false;
        let mut render_scanline = None;
        let mut remaining = cycles;

        while remaining > 0 {
            let threshold = match self.mode {
                PpuMode::OamScan => OAM_SCAN_DOTS,
                PpuMode::PixelTransfer => OAM_SCAN_DOTS + PIXEL_TRANSFER_DOTS,
                PpuMode::HBlank | PpuMode::VBlank => DOTS_PER_SCANLINE,
            };
            let dots_to_threshold = threshold.saturating_sub(self.dot);

            if dots_to_threshold > 0 && remaining < dots_to_threshold {
                self.dot += remaining;
                break;
            }

            self.dot += dots_to_threshold;
            remaining -= dots_to_threshold;

            match self.mode {
                PpuMode::OamScan => {
                    self.mode = PpuMode::PixelTransfer;
                }
                PpuMode::PixelTransfer => {
                    self.mode = PpuMode::HBlank;
                    render_scanline = Some(PpuRasterRequest {
                        ly: self.ly,
                        window_line_counter: self.window_line_counter,
                    });
                    if self.window_participates_on_current_line(io) {
                        self.window_line_counter = self.window_line_counter.wrapping_add(1);
                    }
                }
                PpuMode::HBlank => {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly >= VISIBLE_SCANLINES {
                        self.mode = PpuMode::VBlank;
                        vblank_interrupt = true;
                    } else {
                        self.mode = PpuMode::OamScan;
                    }
                }
                PpuMode::VBlank => {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly >= TOTAL_SCANLINES {
                        self.ly = 0;
                        self.mode = PpuMode::OamScan;
                        self.window_line_counter = 0;
                    }
                }
            }
        }

        let stat_interrupt = self.build_stat(io);
        io[LY_IO] = self.ly;

        PpuTimingOutput {
            vblank_interrupt,
            stat_interrupt,
            render_scanline,
            lcd_reset: false,
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn reset_lcd(&mut self, io: &mut [u8]) {
        self.dot = 0;
        self.ly = 0;
        self.mode = PpuMode::HBlank;
        self.window_line_counter = 0;
        self.prev_stat_line = false;
        io[LY_IO] = 0;
        io[STAT_IO] = (io[STAT_IO] & 0x78) | (PpuMode::HBlank as u8);
    }

    /// Update STAT register and detect STAT interrupt rising edge.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn build_stat(&mut self, io: &mut [u8]) -> bool {
        let lyc = io[LYC_IO];
        let lyc_match = self.ly == lyc;
        let new_stat =
            (io[STAT_IO] & 0x78) | if lyc_match { 0x04 } else { 0x00 } | (self.mode as u8);
        io[STAT_IO] = new_stat;

        let stat_line = (lyc_match && (new_stat & 0x40 != 0))
            || (self.mode == PpuMode::HBlank && (new_stat & 0x08 != 0))
            || (self.mode == PpuMode::VBlank && (new_stat & 0x10 != 0))
            || (self.mode == PpuMode::OamScan && (new_stat & 0x20 != 0));

        let interrupt = stat_line && !self.prev_stat_line;
        self.prev_stat_line = stat_line;
        interrupt
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn render_scanline(&mut self, io: &[u8], vram: &[u8], oam: &[u8]) {
        let lcdc = Lcdc(io[LCDC_IO]);
        let ly = self.ly as usize;
        if ly >= SCREEN_HEIGHT {
            return;
        }

        let row_start = ly * SCREEN_WIDTH;
        let scy = io[SCY_IO];
        let scx = io[SCX_IO];
        let bgp = io[BGP_IO];
        let obp0 = io[OBP0_IO];
        let obp1 = io[OBP1_IO];
        let wy = io[WY_IO];
        let wx = io[WX_IO];

        if lcdc.bg_enabled() {
            self.render_bg_scanline(vram, lcdc, row_start, scy, scx, bgp);
        } else {
            for x in 0..SCREEN_WIDTH {
                self.framebuffer[row_start + x] = 0;
                self.bg_color_indices[x] = 0;
            }
        }

        if lcdc.window_enabled() && lcdc.bg_enabled() {
            self.render_window_scanline(vram, lcdc, row_start, wy, wx, bgp);
        }

        if lcdc.obj_enabled() {
            self.render_sprite_scanline(vram, oam, lcdc, row_start, obp0, obp1);
        }
    }

    fn window_participates_on_current_line(&self, io: &[u8]) -> bool {
        let lcdc = Lcdc(io[LCDC_IO]);
        lcdc.window_enabled()
            && lcdc.bg_enabled()
            && self.ly < SCREEN_HEIGHT as u8
            && self.ly >= io[WY_IO]
            && io[WX_IO] <= 166
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn render_bg_scanline(
        &mut self,
        vram: &[u8],
        lcdc: Lcdc,
        row_start: usize,
        scy: u8,
        scx: u8,
        bgp: u8,
    ) {
        let tilemap_base: usize = if lcdc.bg_tilemap_high() {
            0x1C00
        } else {
            0x1800
        };
        let y = scy.wrapping_add(self.ly);
        let tile_row = (y / 8) as usize;
        let fine_y = (y % 8) as usize;

        let mut current_tile_col = usize::MAX;
        let mut lo = 0u8;
        let mut hi = 0u8;

        for screen_x in 0..SCREEN_WIDTH {
            let x = scx.wrapping_add(screen_x as u8);
            let tile_col = (x / 8) as usize;
            let fine_x = 7 - (x % 8);

            if tile_col != current_tile_col {
                current_tile_col = tile_col;
                let tilemap_addr = tilemap_base + tile_row * 32 + tile_col;
                let tile_index = vram[tilemap_addr];
                let tile_data_addr = tile_data_address(lcdc, tile_index, fine_y);
                lo = vram[tile_data_addr];
                hi = vram[tile_data_addr + 1];
            }

            let color = decode_2bpp_pixel(lo, hi, fine_x);
            self.bg_color_indices[screen_x] = color;
            self.framebuffer[row_start + screen_x] = apply_palette(bgp, color);
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn render_window_scanline(
        &mut self,
        vram: &[u8],
        lcdc: Lcdc,
        row_start: usize,
        wy: u8,
        wx: u8,
        bgp: u8,
    ) {
        if self.ly < wy || wx > 166 {
            return;
        }

        let tilemap_base: usize = if lcdc.window_tilemap_high() {
            0x1C00
        } else {
            0x1800
        };
        let win_y = self.window_line_counter as usize;
        let tile_row = win_y / 8;
        let fine_y = win_y % 8;

        let screen_x_start = if wx < 7 { 0 } else { (wx - 7) as usize };

        let mut current_tile_col = usize::MAX;
        let mut lo = 0u8;
        let mut hi = 0u8;

        for screen_x in screen_x_start..SCREEN_WIDTH {
            let win_x = screen_x - screen_x_start;
            let tile_col = win_x / 8;
            let fine_x = 7 - (win_x % 8) as u8;

            if tile_col != current_tile_col {
                current_tile_col = tile_col;
                let tilemap_addr = tilemap_base + tile_row * 32 + tile_col;
                let tile_index = vram[tilemap_addr];
                let tile_data_addr = tile_data_address(lcdc, tile_index, fine_y);
                lo = vram[tile_data_addr];
                hi = vram[tile_data_addr + 1];
            }

            let color = decode_2bpp_pixel(lo, hi, fine_x);
            self.bg_color_indices[screen_x] = color;
            self.framebuffer[row_start + screen_x] = apply_palette(bgp, color);
        }

        self.window_line_counter += 1;
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn render_sprite_scanline(
        &mut self,
        vram: &[u8],
        oam: &[u8],
        lcdc: Lcdc,
        row_start: usize,
        obp0: u8,
        obp1: u8,
    ) {
        let sprite_height: u8 = if lcdc.obj_tall() { 16 } else { 8 };
        let ly = self.ly as i16;

        let mut sprites: [(u8, u8, u8, u8, usize); 10] = [(0, 0, 0, 0, 0); 10];
        let mut count = 0usize;

        for i in 0..40 {
            if count >= 10 {
                break;
            }
            let oam_addr = i * 4;
            let sprite_y = oam[oam_addr] as i16 - 16;
            let sprite_x = oam[oam_addr + 1];
            let tile = oam[oam_addr + 2];
            let attrs = oam[oam_addr + 3];

            if ly >= sprite_y && ly < sprite_y + sprite_height as i16 {
                sprites[count] = (sprite_y as u8, sprite_x, tile, attrs, i);
                count += 1;
            }
        }

        for i in 1..count {
            let key = sprites[i];
            let mut j = i;
            while j > 0 && sprites[j - 1].1 > key.1 {
                sprites[j] = sprites[j - 1];
                j -= 1;
            }
            sprites[j] = key;
        }

        for idx in (0..count).rev() {
            self.draw_sprite(
                vram,
                oam,
                lcdc,
                row_start,
                sprite_height,
                &sprites[idx],
                obp0,
                obp1,
            );
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn draw_sprite(
        &mut self,
        vram: &[u8],
        oam: &[u8],
        lcdc: Lcdc,
        row_start: usize,
        sprite_height: u8,
        sprite: &(u8, u8, u8, u8, usize),
        obp0: u8,
        obp1: u8,
    ) {
        let (_, sprite_x, tile, attrs, oam_index) = *sprite;
        let sprite_screen_x = sprite_x as i16 - 8;
        let sprite_y_pos = (oam[oam_index * 4] as i16) - 16;
        let ly = self.ly as i16;

        let y_flip = attrs & 0x40 != 0;
        let x_flip = attrs & 0x20 != 0;
        let bg_priority = attrs & 0x80 != 0;
        let palette = if attrs & 0x10 != 0 { obp1 } else { obp0 };

        let mut row_in_sprite = (ly - sprite_y_pos) as u8;
        let tile_index = if lcdc.obj_tall() {
            if y_flip {
                row_in_sprite = sprite_height - 1 - row_in_sprite;
            }
            if row_in_sprite < 8 {
                tile & 0xFE
            } else {
                row_in_sprite -= 8;
                tile | 0x01
            }
        } else {
            if y_flip {
                row_in_sprite = 7 - row_in_sprite;
            }
            tile
        };

        let tile_addr = (tile_index as usize) * 16 + (row_in_sprite as usize) * 2;
        let lo = vram[tile_addr];
        let hi = vram[tile_addr + 1];

        for pixel in 0..8u8 {
            let screen_x = sprite_screen_x + pixel as i16;
            if screen_x < 0 || screen_x >= SCREEN_WIDTH as i16 {
                continue;
            }
            let sx = screen_x as usize;
            let bit = if x_flip { pixel } else { 7 - pixel };
            let color_index = decode_2bpp_pixel(lo, hi, bit);
            if color_index == 0 {
                continue;
            }
            if bg_priority && self.bg_color_indices[sx] != 0 {
                continue;
            }
            self.framebuffer[row_start + sx] = apply_palette(palette, color_index);
        }
    }
}

/// Decode a single pixel from a 2bpp tile row.
#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn decode_2bpp_pixel(lo: u8, hi: u8, bit: u8) -> u8 {
    ((hi >> bit) & 1) << 1 | ((lo >> bit) & 1)
}

/// Compute the VRAM address for a tile row given the tile index and addressing mode.
#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn tile_data_address(lcdc: Lcdc, tile_index: u8, fine_y: usize) -> usize {
    let base = if lcdc.bg_tile_data_unsigned() {
        (tile_index as usize) * 16
    } else {
        let signed_index = tile_index as i8 as i16;
        (0x1000 + signed_index * 16) as usize
    };
    base + fine_y * 2
}

/// Apply a 4-shade palette (BGP/OBP0/OBP1) to a 2-bit color index.
#[cfg_attr(target_arch = "arm", link_section = ".data")]
fn apply_palette(palette: u8, color_index: u8) -> u8 {
    (palette >> (color_index * 2)) & 0x03
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Last scanline index that is drawn to the screen (LY 0–143 are visible).
    const LAST_VISIBLE_LINE: u8 = VISIBLE_SCANLINES - 1;

    fn default_ppu() -> PpuPeripheral {
        PpuPeripheral::new()
    }

    fn default_io() -> [u8; 0x80] {
        let mut io = [0u8; 0x80];
        io[LCDC_IO] = 0x91; // LCD on, BG on, BG tile data unsigned
        io[LYC_IO] = 0xFF;
        io[BGP_IO] = 0xE4; // standard palette: 3,2,1,0
        io[OBP0_IO] = 0xE4;
        io[OBP1_IO] = 0xE4;
        io[WX_IO] = 7;
        io
    }

    fn tick_dots(
        ppu: &mut PpuPeripheral,
        io: &mut [u8],
        dots: u32,
        vram: &[u8],
        oam: &[u8],
    ) -> PpuOutput {
        let mut vblank = false;
        let mut stat_irq = false;
        for _ in 0..dots {
            let o = ppu.tick(1, io, vram, oam);
            vblank |= o.vblank_interrupt;
            stat_irq |= o.stat_interrupt;
        }
        PpuOutput {
            vblank_interrupt: vblank,
            stat_interrupt: stat_irq,
        }
    }

    #[test]
    fn test_mode_transitions_single_scanline() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = default_ppu();
        let mut io = default_io();

        assert_eq!(ppu.mode, PpuMode::OamScan);

        tick_dots(&mut ppu, &mut io, OAM_SCAN_DOTS as u32, &vram, &oam);
        assert_eq!(ppu.mode, PpuMode::PixelTransfer);
        assert_eq!(ppu.ly(), 0);

        tick_dots(&mut ppu, &mut io, PIXEL_TRANSFER_DOTS as u32, &vram, &oam);
        assert_eq!(ppu.mode, PpuMode::HBlank);
        assert_eq!(ppu.ly(), 0);

        tick_dots(
            &mut ppu,
            &mut io,
            (DOTS_PER_SCANLINE - OAM_SCAN_DOTS - PIXEL_TRANSFER_DOTS) as u32,
            &vram,
            &oam,
        );
        assert_eq!(ppu.mode, PpuMode::OamScan);
        assert_eq!(ppu.ly(), 1);
    }

    #[test]
    fn test_ly_counts_to_vblank() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = default_ppu();
        let mut io = default_io();

        let output = tick_dots(
            &mut ppu,
            &mut io,
            VISIBLE_SCANLINES as u32 * DOTS_PER_SCANLINE as u32,
            &vram,
            &oam,
        );
        assert_eq!(ppu.mode, PpuMode::VBlank);
        assert_eq!(ppu.ly(), 144);
        assert!(output.vblank_interrupt);
    }

    #[test]
    fn test_ly_wraps_after_frame() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = default_ppu();
        let mut io = default_io();

        tick_dots(
            &mut ppu,
            &mut io,
            TOTAL_SCANLINES as u32 * DOTS_PER_SCANLINE as u32,
            &vram,
            &oam,
        );
        assert_eq!(ppu.mode, PpuMode::OamScan);
        assert_eq!(ppu.ly(), 0);
    }

    #[test]
    fn test_vblank_interrupt_fires_at_ly_144() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = default_ppu();
        let mut io = default_io();

        let output = tick_dots(
            &mut ppu,
            &mut io,
            LAST_VISIBLE_LINE as u32 * DOTS_PER_SCANLINE as u32,
            &vram,
            &oam,
        );
        assert!(!output.vblank_interrupt);

        let output = tick_dots(&mut ppu, &mut io, DOTS_PER_SCANLINE as u32, &vram, &oam);
        assert!(output.vblank_interrupt);
        assert_eq!(ppu.ly(), 144);
    }

    #[test]
    fn test_stat_lyc_interrupt() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = default_ppu();
        let mut io = default_io();
        io[LYC_IO] = 5;
        io[STAT_IO] = 0x40; // LYC=LY interrupt enable

        // Advance exactly 5 full scanlines so LY reaches 5, matching LYC.
        let output = tick_dots(&mut ppu, &mut io, 5 * DOTS_PER_SCANLINE as u32, &vram, &oam);
        assert!(output.stat_interrupt);
        assert_eq!(io[STAT_IO] & 0x04, 0x04); // LYC=LY flag set
    }

    #[test]
    fn test_stat_mode_bits() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = default_ppu();
        let mut io = default_io();

        tick_dots(&mut ppu, &mut io, 1, &vram, &oam);
        assert_eq!(io[STAT_IO] & 0x03, 2); // OAM scan = mode 2

        tick_dots(&mut ppu, &mut io, 79, &vram, &oam);
        assert_eq!(io[STAT_IO] & 0x03, 3); // Pixel transfer = mode 3

        tick_dots(&mut ppu, &mut io, 172, &vram, &oam);
        assert_eq!(io[STAT_IO] & 0x03, 0); // HBlank = mode 0
    }

    #[test]
    fn test_lcd_disabled_resets_state() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = default_ppu();
        let mut io = default_io();

        tick_dots(
            &mut ppu,
            &mut io,
            5 * DOTS_PER_SCANLINE as u32 + 100,
            &vram,
            &oam,
        );

        io[LCDC_IO] = 0x00;
        ppu.tick(1, &mut io, &vram, &oam);
        assert_eq!(ppu.ly(), 0);
        assert_eq!(ppu.mode, PpuMode::HBlank);
        assert_eq!(ppu.dot, 0);
    }

    #[test]
    fn test_apply_palette() {
        assert_eq!(apply_palette(0xE4, 0), 0);
        assert_eq!(apply_palette(0xE4, 1), 1);
        assert_eq!(apply_palette(0xE4, 2), 2);
        assert_eq!(apply_palette(0xE4, 3), 3);

        assert_eq!(apply_palette(0x1B, 0), 3);
        assert_eq!(apply_palette(0x1B, 1), 2);
        assert_eq!(apply_palette(0x1B, 2), 1);
        assert_eq!(apply_palette(0x1B, 3), 0);
    }

    #[test]
    fn test_bg_tile_rendering() {
        let mut vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        vram[0] = 0xFF;
        vram[1] = 0xFF;
        vram[0x1800] = 0;

        let mut ppu = default_ppu();
        let mut io = default_io();

        tick_dots(&mut ppu, &mut io, DOTS_PER_SCANLINE as u32, &vram, &oam);

        for x in 0..8 {
            assert_eq!(
                ppu.framebuffer()[x],
                apply_palette(0xE4, 3),
                "pixel {} expected color 3",
                x
            );
        }
    }

    #[test]
    fn test_sprite_rendering() {
        let mut vram = [0u8; 0x2000];
        let mut oam = [0u8; 0xA0];

        vram[16] = 0xFF;
        vram[17] = 0x00;

        oam[0] = 16;
        oam[1] = 8;
        oam[2] = 1;
        oam[3] = 0;

        let mut ppu = default_ppu();
        let mut io = default_io();
        io[LCDC_IO] = 0x93; // LCD on, BG on, OBJ on, unsigned tile data

        tick_dots(&mut ppu, &mut io, DOTS_PER_SCANLINE as u32, &vram, &oam);

        for x in 0..8 {
            assert_eq!(
                ppu.framebuffer()[x],
                apply_palette(0xE4, 1),
                "sprite pixel {} expected color 1",
                x
            );
        }
    }

    #[test]
    fn test_sprite_transparency() {
        let mut vram = [0u8; 0x2000];
        let mut oam = [0u8; 0xA0];

        vram[0] = 0x00;
        vram[1] = 0xFF;
        vram[0x1800] = 0;

        vram[16] = 0xAA;
        vram[17] = 0x00;

        oam[0] = 16;
        oam[1] = 8;
        oam[2] = 1;
        oam[3] = 0;

        let mut ppu = default_ppu();
        let mut io = default_io();
        io[LCDC_IO] = 0x93;

        tick_dots(&mut ppu, &mut io, DOTS_PER_SCANLINE as u32, &vram, &oam);

        assert_eq!(ppu.framebuffer()[0], apply_palette(0xE4, 1)); // sprite
        assert_eq!(ppu.framebuffer()[1], apply_palette(0xE4, 2)); // BG (transparent sprite)
    }

    #[test]
    fn test_window_rendering() {
        let mut vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        vram[0] = 0xFF;
        vram[1] = 0x00;
        vram[0x1800] = 0;

        let mut ppu = default_ppu();
        let mut io = default_io();
        io[LCDC_IO] = 0xB1; // LCD on, window on, BG on, unsigned, window tilemap low
        io[WY_IO] = 0;
        // WX already 7 from default_io()

        tick_dots(&mut ppu, &mut io, DOTS_PER_SCANLINE as u32, &vram, &oam);

        for x in 0..8 {
            assert_eq!(
                ppu.framebuffer()[x],
                apply_palette(0xE4, 1),
                "window pixel {} expected color 1",
                x
            );
        }
    }

    #[test]
    fn test_stat_rising_edge_only() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = default_ppu();
        let mut io = default_io();
        io[STAT_IO] = 0x20; // Mode 2 (OAM scan) interrupt enable

        let output = tick_dots(&mut ppu, &mut io, 1, &vram, &oam);
        assert!(output.stat_interrupt);

        let output = tick_dots(&mut ppu, &mut io, 1, &vram, &oam);
        assert!(!output.stat_interrupt);
    }

    #[test]
    fn test_signed_tile_addressing() {
        let lcdc = Lcdc(0x81);
        let addr = tile_data_address(lcdc, 0, 0);
        assert_eq!(addr, 0x1000);

        let addr = tile_data_address(lcdc, 0x80, 0);
        assert_eq!(addr, 0x0800);

        let addr = tile_data_address(lcdc, 127, 0);
        assert_eq!(addr, 0x17F0);
    }

    #[test]
    fn test_decode_2bpp_pixel() {
        assert_eq!(decode_2bpp_pixel(0xFF, 0xFF, 7), 3);
        assert_eq!(decode_2bpp_pixel(0xFF, 0x00, 0), 1);
        assert_eq!(decode_2bpp_pixel(0x00, 0xFF, 0), 2);
        assert_eq!(decode_2bpp_pixel(0x00, 0x00, 0), 0);
    }
}
