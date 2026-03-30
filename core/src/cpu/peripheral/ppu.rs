/// PPU register addresses.
pub(crate) const LCDC_ADDR: u16 = 0xFF40;
pub(crate) const STAT_ADDR: u16 = 0xFF41;
pub(crate) const SCY_ADDR: u16 = 0xFF42;
pub(crate) const SCX_ADDR: u16 = 0xFF43;
pub(crate) const LY_ADDR: u16 = 0xFF44;
pub(crate) const LYC_ADDR: u16 = 0xFF45;
pub(crate) const BGP_ADDR: u16 = 0xFF47;
pub(crate) const OBP0_ADDR: u16 = 0xFF48;
pub(crate) const OBP1_ADDR: u16 = 0xFF49;
pub(crate) const WY_ADDR: u16 = 0xFF4A;
pub(crate) const WX_ADDR: u16 = 0xFF4B;

pub(crate) const VBLANK_INTERRUPT_BIT: u8 = 0;
pub(crate) const STAT_INTERRUPT_BIT: u8 = 1;

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

/// Input snapshot passed to the PPU each tick.
pub struct PpuInput<'a> {
    pub lcdc: u8,
    pub stat: u8,
    pub scy: u8,
    pub scx: u8,
    pub lyc: u8,
    pub bgp: u8,
    pub obp0: u8,
    pub obp1: u8,
    pub wy: u8,
    pub wx: u8,
    pub vram: &'a [u8],
    pub oam: &'a [u8],
}

/// Result of a PPU tick — CPU writes these back to IO registers.
pub struct PpuOutput {
    pub ly: u8,
    pub stat: u8,
    pub vblank_interrupt: bool,
    pub stat_interrupt: bool,
}

/// Event produced by a single dot advance.
enum DotEvent {
    None,
    RenderScanline,
    EnterVBlank,
}

/// Scanline-based PPU peripheral.
pub struct PpuPeripheral {
    dot: u16,
    ly: u8,
    mode: PpuMode,
    window_line_counter: u8,
    prev_stat_line: bool,
    framebuffer: [u8; FRAMEBUFFER_SIZE],
    /// Raw BG/window color indices (0-3) for the current scanline, used for sprite priority.
    bg_color_indices: [u8; SCREEN_WIDTH],
}

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

    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        &self.framebuffer
    }

    /// Serialize PPU state into `out`. 5 bytes: dot(LE u16) ly mode window_line_counter.
    pub fn save_state(&self, out: &mut alloc::vec::Vec<u8>) {
        out.extend_from_slice(&self.dot.to_le_bytes());
        out.push(self.ly);
        out.push(self.mode as u8);
        out.push(self.window_line_counter);
    }

    /// Deserialize PPU state from `data` at byte `offset`. Returns the number of bytes consumed.
    pub fn load_state(&mut self, data: &[u8], offset: usize) -> usize {
        let start = offset;
        let mut cur = offset;
        self.dot = u16::from_le_bytes([data[cur], data[cur + 1]]); cur += 2;
        self.ly = data[cur]; cur += 1;
        self.mode = match data[cur] {
            0 => PpuMode::HBlank,
            1 => PpuMode::VBlank,
            2 => PpuMode::OamScan,
            _ => PpuMode::PixelTransfer,
        }; cur += 1;
        self.window_line_counter = data[cur]; cur += 1;
        cur - start
    }

    /// Advance the PPU by `cycles` T-cycles.
    pub fn tick(&mut self, cycles: u16, input: PpuInput) -> PpuOutput {
        let lcdc = Lcdc(input.lcdc);

        if !lcdc.lcd_enabled() {
            self.reset_lcd();
            let stat = (input.stat & 0x78) | (PpuMode::HBlank as u8);
            return PpuOutput {
                ly: 0,
                stat,
                vblank_interrupt: false,
                stat_interrupt: false,
            };
        }

        let mut vblank_interrupt = false;
        for _ in 0..cycles {
            match self.advance_dot(&input) {
                DotEvent::RenderScanline => self.render_scanline(&input),
                DotEvent::EnterVBlank => vblank_interrupt = true,
                DotEvent::None => {}
            }
        }

        let (stat, stat_interrupt) = self.build_stat(&input);

        PpuOutput {
            ly: self.ly,
            stat,
            vblank_interrupt,
            stat_interrupt,
        }
    }

    /// Reset LY to 0 (triggered by CPU write to LY register).
    pub fn reset_ly(&mut self) {
        self.ly = 0;
    }

    /// Advance the dot counter by one and handle mode transitions.
    fn advance_dot(&mut self, _input: &PpuInput) -> DotEvent {
        self.dot += 1;

        match self.mode {
            PpuMode::OamScan => {
                if self.dot >= OAM_SCAN_DOTS {
                    self.mode = PpuMode::PixelTransfer;
                }
                DotEvent::None
            }
            PpuMode::PixelTransfer => {
                if self.dot >= OAM_SCAN_DOTS + PIXEL_TRANSFER_DOTS {
                    self.mode = PpuMode::HBlank;
                    DotEvent::RenderScanline
                } else {
                    DotEvent::None
                }
            }
            PpuMode::HBlank => {
                if self.dot >= DOTS_PER_SCANLINE {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly >= VISIBLE_SCANLINES {
                        self.mode = PpuMode::VBlank;
                        DotEvent::EnterVBlank
                    } else {
                        self.mode = PpuMode::OamScan;
                        DotEvent::None
                    }
                } else {
                    DotEvent::None
                }
            }
            PpuMode::VBlank => {
                if self.dot >= DOTS_PER_SCANLINE {
                    self.dot = 0;
                    self.ly += 1;
                    if self.ly >= TOTAL_SCANLINES {
                        self.ly = 0;
                        self.mode = PpuMode::OamScan;
                        self.window_line_counter = 0;
                    }
                }
                DotEvent::None
            }
        }
    }

    /// Build the STAT register value and detect STAT interrupt rising edge.
    fn build_stat(&mut self, input: &PpuInput) -> (u8, bool) {
        let lyc_match = self.ly == input.lyc;
        let stat = (input.stat & 0x78)
            | if lyc_match { 0x04 } else { 0x00 }
            | (self.mode as u8);

        let stat_line = (lyc_match && (input.stat & 0x40 != 0))
            || (self.mode == PpuMode::HBlank && (input.stat & 0x08 != 0))
            || (self.mode == PpuMode::VBlank && (input.stat & 0x10 != 0))
            || (self.mode == PpuMode::OamScan && (input.stat & 0x20 != 0));

        let interrupt = stat_line && !self.prev_stat_line;
        self.prev_stat_line = stat_line;

        (stat, interrupt)
    }

    /// Reset all PPU state when the LCD is disabled.
    fn reset_lcd(&mut self) {
        self.dot = 0;
        self.ly = 0;
        self.mode = PpuMode::HBlank;
        self.window_line_counter = 0;
        self.prev_stat_line = false;
    }

    fn render_scanline(&mut self, input: &PpuInput) {
        let lcdc = Lcdc(input.lcdc);
        let ly = self.ly as usize;
        if ly >= SCREEN_HEIGHT {
            return;
        }

        let row_start = ly * SCREEN_WIDTH;

        if lcdc.bg_enabled() {
            self.render_bg_scanline(input, lcdc, row_start);
        } else {
            for x in 0..SCREEN_WIDTH {
                self.framebuffer[row_start + x] = 0;
                self.bg_color_indices[x] = 0;
            }
        }

        if lcdc.window_enabled() && lcdc.bg_enabled() {
            self.render_window_scanline(input, lcdc, row_start);
        }

        if lcdc.obj_enabled() {
            self.render_sprite_scanline(input, lcdc, row_start);
        }
    }

    fn render_bg_scanline(&mut self, input: &PpuInput, lcdc: Lcdc, row_start: usize) {
        let tilemap_base: usize = if lcdc.bg_tilemap_high() {
            0x1C00
        } else {
            0x1800
        };
        let y = input.scy.wrapping_add(self.ly);
        let tile_row = (y / 8) as usize;
        let fine_y = (y % 8) as usize;

        for screen_x in 0..SCREEN_WIDTH {
            let x = input.scx.wrapping_add(screen_x as u8);
            let tile_col = (x / 8) as usize;
            let fine_x = 7 - (x % 8);

            let color = fetch_tile_pixel(input.vram, lcdc, tilemap_base, tile_col, tile_row, fine_x, fine_y);
            self.bg_color_indices[screen_x] = color;
            self.framebuffer[row_start + screen_x] = apply_palette(input.bgp, color);
        }
    }

    fn render_window_scanline(&mut self, input: &PpuInput, lcdc: Lcdc, row_start: usize) {
        if self.ly < input.wy || input.wx > 166 {
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

        let screen_x_start = if input.wx < 7 { 0 } else { (input.wx - 7) as usize };

        for screen_x in screen_x_start..SCREEN_WIDTH {
            let win_x = screen_x - screen_x_start;
            let tile_col = win_x / 8;
            let fine_x = 7 - (win_x % 8) as u8;

            let color = fetch_tile_pixel(input.vram, lcdc, tilemap_base, tile_col, tile_row, fine_x as u8, fine_y);
            self.bg_color_indices[screen_x] = color;
            self.framebuffer[row_start + screen_x] = apply_palette(input.bgp, color);
        }

        self.window_line_counter += 1;
    }

    fn render_sprite_scanline(&mut self, input: &PpuInput, lcdc: Lcdc, row_start: usize) {
        let sprite_height: u8 = if lcdc.obj_tall() { 16 } else { 8 };
        let ly = self.ly as i16;

        // Collect visible sprites on this scanline (max 10)
        let mut sprites: [(u8, u8, u8, u8, usize); 10] = [(0, 0, 0, 0, 0); 10];
        let mut count = 0usize;

        for i in 0..40 {
            if count >= 10 {
                break;
            }
            let oam_addr = i * 4;
            let sprite_y = input.oam[oam_addr] as i16 - 16;
            let sprite_x = input.oam[oam_addr + 1];
            let tile = input.oam[oam_addr + 2];
            let attrs = input.oam[oam_addr + 3];

            if ly >= sprite_y && ly < sprite_y + sprite_height as i16 {
                sprites[count] = (sprite_y as u8, sprite_x, tile, attrs, i);
                count += 1;
            }
        }

        // Sort by X coordinate; insertion sort preserves OAM index order for ties
        for i in 1..count {
            let key = sprites[i];
            let mut j = i;
            while j > 0 && sprites[j - 1].1 > key.1 {
                sprites[j] = sprites[j - 1];
                j -= 1;
            }
            sprites[j] = key;
        }

        // Draw in reverse priority order so higher-priority sprites overwrite
        for idx in (0..count).rev() {
            self.draw_sprite(input, lcdc, row_start, sprite_height, &sprites[idx]);
        }
    }

    fn draw_sprite(
        &mut self,
        input: &PpuInput,
        lcdc: Lcdc,
        row_start: usize,
        sprite_height: u8,
        sprite: &(u8, u8, u8, u8, usize),
    ) {
        let (_, sprite_x, tile, attrs, oam_index) = *sprite;
        let sprite_screen_x = sprite_x as i16 - 8;
        let sprite_y_pos = (input.oam[oam_index * 4] as i16) - 16;
        let ly = self.ly as i16;

        let y_flip = attrs & 0x40 != 0;
        let x_flip = attrs & 0x20 != 0;
        let bg_priority = attrs & 0x80 != 0;
        let palette = if attrs & 0x10 != 0 { input.obp1 } else { input.obp0 };

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
        let lo = input.vram[tile_addr];
        let hi = input.vram[tile_addr + 1];

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

/// Fetch a single tile pixel from VRAM given tilemap coordinates.
fn fetch_tile_pixel(
    vram: &[u8],
    lcdc: Lcdc,
    tilemap_base: usize,
    tile_col: usize,
    tile_row: usize,
    fine_x: u8,
    fine_y: usize,
) -> u8 {
    let tilemap_addr = tilemap_base + tile_row * 32 + tile_col;
    let tile_index = vram[tilemap_addr];
    let tile_data_addr = tile_data_address(lcdc, tile_index, fine_y);
    let lo = vram[tile_data_addr];
    let hi = vram[tile_data_addr + 1];
    decode_2bpp_pixel(lo, hi, fine_x)
}

/// Decode a single pixel from a 2bpp tile row.
fn decode_2bpp_pixel(lo: u8, hi: u8, bit: u8) -> u8 {
    ((hi >> bit) & 1) << 1 | ((lo >> bit) & 1)
}

/// Compute the VRAM address for a tile row given the tile index and addressing mode.
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
fn apply_palette(palette: u8, color_index: u8) -> u8 {
    (palette >> (color_index * 2)) & 0x03
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_input<'a>(vram: &'a [u8], oam: &'a [u8]) -> PpuInput<'a> {
        PpuInput {
            lcdc: 0x91, // LCD on, BG on, BG tile data unsigned
            stat: 0x00,
            scy: 0,
            scx: 0,
            lyc: 0xFF,
            bgp: 0xE4, // standard palette: 3,2,1,0
            obp0: 0xE4,
            obp1: 0xE4,
            wy: 0,
            wx: 7,
            vram,
            oam,
        }
    }

    fn tick_dots(ppu: &mut PpuPeripheral, dots: u32, input: &PpuInput) -> PpuOutput {
        let mut output = PpuOutput {
            ly: 0,
            stat: 0,
            vblank_interrupt: false,
            stat_interrupt: false,
        };
        // Tick one at a time to get correct mode transitions
        for _ in 0..dots {
            let o = ppu.tick(1, PpuInput {
                lcdc: input.lcdc,
                stat: input.stat,
                scy: input.scy,
                scx: input.scx,
                lyc: input.lyc,
                bgp: input.bgp,
                obp0: input.obp0,
                obp1: input.obp1,
                wy: input.wy,
                wx: input.wx,
                vram: input.vram,
                oam: input.oam,
            });
            if o.vblank_interrupt {
                output.vblank_interrupt = true;
            }
            if o.stat_interrupt {
                output.stat_interrupt = true;
            }
            output.ly = o.ly;
            output.stat = o.stat;
        }
        output
    }

    #[test]
    fn test_mode_transitions_single_scanline() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = PpuPeripheral::new();
        let input = default_input(&vram, &oam);

        assert_eq!(ppu.mode, PpuMode::OamScan);

        // After 80 dots: transition to PixelTransfer
        let output = tick_dots(&mut ppu, OAM_SCAN_DOTS as u32, &input);
        assert_eq!(ppu.mode, PpuMode::PixelTransfer);
        assert_eq!(output.ly, 0);

        // After 172 more dots: transition to HBlank
        let output = tick_dots(&mut ppu, PIXEL_TRANSFER_DOTS as u32, &input);
        assert_eq!(ppu.mode, PpuMode::HBlank);
        assert_eq!(output.ly, 0);

        // After 204 more dots (456 total): transition to next scanline
        let output = tick_dots(&mut ppu, (DOTS_PER_SCANLINE - OAM_SCAN_DOTS - PIXEL_TRANSFER_DOTS) as u32, &input);
        assert_eq!(ppu.mode, PpuMode::OamScan);
        assert_eq!(output.ly, 1);
    }

    #[test]
    fn test_ly_counts_to_vblank() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = PpuPeripheral::new();
        let input = default_input(&vram, &oam);

        // Tick through 144 scanlines
        let output = tick_dots(&mut ppu, VISIBLE_SCANLINES as u32 * DOTS_PER_SCANLINE as u32, &input);
        assert_eq!(ppu.mode, PpuMode::VBlank);
        assert_eq!(output.ly, 144);
        assert!(output.vblank_interrupt);
    }

    #[test]
    fn test_ly_wraps_after_frame() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = PpuPeripheral::new();
        let input = default_input(&vram, &oam);

        // Full frame: 154 scanlines
        let output = tick_dots(&mut ppu, TOTAL_SCANLINES as u32 * DOTS_PER_SCANLINE as u32, &input);
        assert_eq!(ppu.mode, PpuMode::OamScan);
        assert_eq!(output.ly, 0);
    }

    #[test]
    fn test_vblank_interrupt_fires_at_ly_144() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = PpuPeripheral::new();
        let input = default_input(&vram, &oam);

        // Tick through 143 scanlines — no vblank yet
        let output = tick_dots(&mut ppu, 143 * DOTS_PER_SCANLINE as u32, &input);
        assert!(!output.vblank_interrupt);

        // Tick through scanline 143 into 144 — vblank fires
        let output = tick_dots(&mut ppu, DOTS_PER_SCANLINE as u32, &input);
        assert!(output.vblank_interrupt);
        assert_eq!(output.ly, 144);
    }

    #[test]
    fn test_stat_lyc_interrupt() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = PpuPeripheral::new();
        let mut input = default_input(&vram, &oam);
        input.lyc = 5;
        input.stat = 0x40; // LYC=LY interrupt enable

        // Tick to scanline 5
        let output = tick_dots(&mut ppu, 5 * DOTS_PER_SCANLINE as u32, &input);
        assert!(output.stat_interrupt);
        assert_eq!(output.stat & 0x04, 0x04); // LYC=LY flag set
    }

    #[test]
    fn test_stat_mode_bits() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = PpuPeripheral::new();
        let input = default_input(&vram, &oam);

        // OAM scan = mode 2
        let output = tick_dots(&mut ppu, 1, &input);
        assert_eq!(output.stat & 0x03, 2);

        // Pixel transfer = mode 3
        let output = tick_dots(&mut ppu, 79, &input);
        assert_eq!(output.stat & 0x03, 3);

        // HBlank = mode 0
        let output = tick_dots(&mut ppu, 172, &input);
        assert_eq!(output.stat & 0x03, 0);
    }

    #[test]
    fn test_lcd_disabled_resets_state() {
        let vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];
        let mut ppu = PpuPeripheral::new();
        let mut input = default_input(&vram, &oam);

        // Advance to mid-frame
        tick_dots(&mut ppu, 5 * DOTS_PER_SCANLINE as u32 + 100, &input);

        // Disable LCD
        input.lcdc = 0x00;
        let output = ppu.tick(1, input);
        assert_eq!(output.ly, 0);
        assert_eq!(ppu.mode, PpuMode::HBlank);
        assert_eq!(ppu.dot, 0);
    }

    #[test]
    fn test_apply_palette() {
        // Standard palette: color 0→0, 1→1, 2→2, 3→3
        assert_eq!(apply_palette(0xE4, 0), 0);
        assert_eq!(apply_palette(0xE4, 1), 1);
        assert_eq!(apply_palette(0xE4, 2), 2);
        assert_eq!(apply_palette(0xE4, 3), 3);

        // Inverted palette: color 0→3, 1→2, 2→1, 3→0
        assert_eq!(apply_palette(0x1B, 0), 3);
        assert_eq!(apply_palette(0x1B, 1), 2);
        assert_eq!(apply_palette(0x1B, 2), 1);
        assert_eq!(apply_palette(0x1B, 3), 0);
    }

    #[test]
    fn test_bg_tile_rendering() {
        let mut vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        // Set up tile 0 at 0x0000: first row = all color 3
        vram[0] = 0xFF; // lo plane: all 1s
        vram[1] = 0xFF; // hi plane: all 1s

        // Set tilemap entry at (0,0) to tile 0
        vram[0x1800] = 0;

        let mut ppu = PpuPeripheral::new();
        let input = default_input(&vram, &oam);

        // Tick through one scanline (LY=0)
        tick_dots(&mut ppu, DOTS_PER_SCANLINE as u32, &input);

        // First 8 pixels should be palette color 3
        for x in 0..8 {
            assert_eq!(
                ppu.framebuffer[x],
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

        // Set up sprite tile 1: first row = all color 1
        vram[16] = 0xFF; // lo plane
        vram[17] = 0x00; // hi plane

        // OAM entry 0: Y=16 (screen Y=0), X=8 (screen X=0), tile=1, attrs=0
        oam[0] = 16;
        oam[1] = 8;
        oam[2] = 1;
        oam[3] = 0;

        let mut ppu = PpuPeripheral::new();
        let mut input = default_input(&vram, &oam);
        input.lcdc = 0x93; // LCD on, BG on, OBJ on, unsigned tile data

        tick_dots(&mut ppu, DOTS_PER_SCANLINE as u32, &input);

        // First 8 pixels should be sprite color 1 (applied through OBP0)
        for x in 0..8 {
            assert_eq!(
                ppu.framebuffer[x],
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

        // BG tile 0, row 0: all color 2
        vram[0] = 0x00; // lo
        vram[1] = 0xFF; // hi
        vram[0x1800] = 0;

        // Sprite tile 1, row 0: alternating color 0 (transparent) and color 1
        vram[16] = 0xAA; // lo: 10101010
        vram[17] = 0x00; // hi: 00000000

        oam[0] = 16;
        oam[1] = 8;
        oam[2] = 1;
        oam[3] = 0;

        let mut ppu = PpuPeripheral::new();
        let mut input = default_input(&vram, &oam);
        input.lcdc = 0x93;

        tick_dots(&mut ppu, DOTS_PER_SCANLINE as u32, &input);

        // Pixels where sprite is color 0 should show BG color 2
        // Pixels where sprite is color 1 should show sprite color 1
        assert_eq!(ppu.framebuffer[0], apply_palette(0xE4, 1)); // sprite
        assert_eq!(ppu.framebuffer[1], apply_palette(0xE4, 2)); // BG (transparent sprite)
    }

    #[test]
    fn test_window_rendering() {
        let mut vram = [0u8; 0x2000];
        let oam = [0u8; 0xA0];

        // Window tile 0 at 0x0000: first row = all color 1
        vram[0] = 0xFF; // lo
        vram[1] = 0x00; // hi

        // Window tilemap at 0x1800 (LCDC bit 6 = 0), tile 0
        vram[0x1800] = 0;

        let mut ppu = PpuPeripheral::new();
        let mut input = default_input(&vram, &oam);
        input.lcdc = 0xB1; // LCD on, window on, BG on, unsigned, window tilemap low
        input.wy = 0;
        input.wx = 7; // window starts at screen X=0

        tick_dots(&mut ppu, DOTS_PER_SCANLINE as u32, &input);

        // First 8 pixels should be window color 1
        for x in 0..8 {
            assert_eq!(
                ppu.framebuffer[x],
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
        let mut ppu = PpuPeripheral::new();
        let mut input = default_input(&vram, &oam);
        input.stat = 0x20; // Mode 2 (OAM scan) interrupt enable

        // First tick into OAM scan should fire STAT
        let output = tick_dots(&mut ppu, 1, &input);
        assert!(output.stat_interrupt);

        // Subsequent ticks in same mode should NOT re-fire
        let output = tick_dots(&mut ppu, 1, &input);
        assert!(!output.stat_interrupt);
    }

    #[test]
    fn test_signed_tile_addressing() {
        // LCDC bit 4 = 0: signed addressing, tile 0 at 0x1000
        let lcdc = Lcdc(0x81); // LCD on, BG on, signed tile data
        let addr = tile_data_address(lcdc, 0, 0);
        assert_eq!(addr, 0x1000);

        // Tile index 0x80 = -128 as i8 → 0x1000 + (-128)*16 = 0x1000 - 2048 = 0x0800
        let addr = tile_data_address(lcdc, 0x80, 0);
        assert_eq!(addr, 0x0800);

        // Tile index 127 → 0x1000 + 127*16 = 0x17F0
        let addr = tile_data_address(lcdc, 127, 0);
        assert_eq!(addr, 0x17F0);
    }

    #[test]
    fn test_decode_2bpp_pixel() {
        // Both bits set = color 3
        assert_eq!(decode_2bpp_pixel(0xFF, 0xFF, 7), 3);
        // Only lo set = color 1
        assert_eq!(decode_2bpp_pixel(0xFF, 0x00, 0), 1);
        // Only hi set = color 2
        assert_eq!(decode_2bpp_pixel(0x00, 0xFF, 0), 2);
        // Neither set = color 0
        assert_eq!(decode_2bpp_pixel(0x00, 0x00, 0), 0);
    }
}
