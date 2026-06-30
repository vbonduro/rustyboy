use ratatui::style::Color;

// Menu palette, sampled from the title screen's own colors so every menu shares
// the muted cream-on-dark-green look rather than the classic bright DMG lime.
// C0 = background, C1 = bars / selection fill, C2 = text, C3 = bright/highlight.
pub(super) const C0: [u8; 4] = [0x18, 0x2A, 0x20, 0xFF]; // (24,42,32) dark green
pub(super) const C1: [u8; 4] = [0x3E, 0x5A, 0x46, 0xFF]; // (62,90,70) mid green
pub(super) const C2: [u8; 4] = [0xA6, 0xBC, 0x96, 0xFF]; // (166,188,150) cream text
pub(super) const C3: [u8; 4] = [0xB0, 0xC6, 0xA0, 0xFF]; // (176,198,160) bright cream

// Aliases for the main-menu text drawn over the landscape: cream fill with the
// title's dark-green outline.
pub(super) const MENU_TEXT_OUTLINE: [u8; 4] = [0x1F, 0x37, 0x28, 0xFF]; // (31,55,40)
pub(super) const MENU_TEXT_NORMAL: [u8; 4] = C2;
pub(super) const MENU_TEXT_SELECTED: [u8; 4] = C3;

/// Map a ratatui `Color` to an RGBA pixel.  All menu styles use `Color::Rgb`;
/// any other variant falls back to `fallback`.
pub(super) fn color_to_rgba(color: Color, fallback: [u8; 4]) -> [u8; 4] {
    match color {
        Color::Rgb(r, g, b) => [r, g, b, 0xFF],
        _ => fallback,
    }
}

pub(super) fn gb_c0() -> Color {
    Color::Rgb(C0[0], C0[1], C0[2])
}
pub(super) fn gb_c1() -> Color {
    Color::Rgb(C1[0], C1[1], C1[2])
}
pub(super) fn gb_c2() -> Color {
    Color::Rgb(C2[0], C2[1], C2[2])
}
pub(super) fn gb_c3() -> Color {
    Color::Rgb(C3[0], C3[1], C3[2])
}
