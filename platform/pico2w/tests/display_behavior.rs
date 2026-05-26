use rustyboy_pico2w::display::{
    loading_bar_window, menu_item_text_window, render_loading_row, render_menu_row, LoadingFrame,
    LoadingProgress,
};
use rustyboy_pico2w::menu::MenuFrame;

fn render_menu_scanline(frame: &MenuFrame<'_>, y: u16) -> [u8; 480] {
    let mut row = [0; 480];
    render_menu_row(frame, y, &mut row);
    row
}

fn render_loading_scanline(frame: &LoadingFrame<'_>, y: u16) -> [u8; 480] {
    let mut row = [0; 480];
    render_loading_row(frame, y, &mut row);
    row
}

fn pixel(row: &[u8; 480], x: usize) -> [u8; 2] {
    [row[x * 2], row[x * 2 + 1]]
}

fn rom_menu_frame<'a>(
    items: &'a [&'a str],
    enabled: &'a [bool],
    marquee_frame: u32,
) -> MenuFrame<'a> {
    MenuFrame {
        title: "ROMS",
        items,
        selected: 0,
        marquee_frame,
        enabled,
        marked: None,
        crash_pending: false,
    }
}

#[test]
fn selected_long_menu_item_scrolls_but_unselected_long_items_stay_static() {
    let items = [
        "THIS-IS-A-VERY-LONG-ROM-NAME-A.GB",
        "THIS-IS-A-VERY-LONG-ROM-NAME-B.GB",
    ];
    let enabled = [true, true];
    let selected_text = menu_item_text_window(0, false).unwrap();
    let unselected_text = menu_item_text_window(1, false).unwrap();

    let first_frame = rom_menu_frame(&items, &enabled, 0);
    let scrolled_frame = rom_menu_frame(&items, &enabled, 64);

    let selected_y = selected_text.y_start + 4;
    let selected_first = render_menu_scanline(&first_frame, selected_y);
    let selected_scrolled = render_menu_scanline(&scrolled_frame, selected_y);
    assert_ne!(
        &selected_first[selected_text.byte_range()],
        &selected_scrolled[selected_text.byte_range()]
    );

    let unselected_y = unselected_text.y_start + 4;
    let unselected_first = render_menu_scanline(&first_frame, unselected_y);
    let unselected_later = render_menu_scanline(&scrolled_frame, unselected_y);
    assert_eq!(
        &unselected_first[unselected_text.byte_range()],
        &unselected_later[unselected_text.byte_range()]
    );
}

#[test]
fn marked_menu_item_keeps_label_out_of_marker_space() {
    let unmarked = menu_item_text_window(0, false).unwrap();
    let marked = menu_item_text_window(0, true).unwrap();

    assert_eq!(marked.x_start, unmarked.x_start);
    assert_eq!(marked.y_start, unmarked.y_start);
    assert!(marked.x_end < unmarked.x_end);
}

#[test]
fn loading_progress_bar_fills_the_completed_fraction() {
    let window = loading_bar_window();
    let frame = LoadingFrame::new("ROM.GB", LoadingProgress::new(1, 2), 0);
    let row = render_loading_scanline(&frame, window.y_start);

    let first_filled = pixel(&row, 20);
    assert_eq!(first_filled, pixel(&row, 119));
    assert_ne!(first_filled, pixel(&row, 120));
    assert_eq!(pixel(&row, 120), pixel(&row, 219));
}

#[test]
fn loading_progress_bar_caps_at_full_width() {
    let window = loading_bar_window();
    let frame = LoadingFrame::new("ROM.GB", LoadingProgress::new(3, 2), 0);
    let row = render_loading_scanline(&frame, window.y_start);

    assert_eq!(pixel(&row, 20), pixel(&row, 219));
}
