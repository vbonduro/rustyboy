use std::error::Error;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use ratatui::backend::TestBackend;
use ratatui::prelude::{Buffer, Color};
use ratatui::Terminal;
use rustyboy_pico2w::display::font;
use rustyboy_pico2w::display::ui::draw_menu;
use rustyboy_pico2w::menu::{
    MainMenu, MenuEffect, MenuFrame, MenuInput, MenuLogic, RomListEffect, RomListLogic,
    SettingsMenu,
};

const COLS: u16 = 30;
const ROWS: u16 = 24;
const CELL_W: usize = 8;
const CELL_H: usize = 13;
const PNG_W: usize = COLS as usize * CELL_W;
const PNG_H: usize = ROWS as usize * CELL_H;
const OUT_DIR: &str = "/tmp/rustyboy-menu-preview";

const ROM_ITEMS: [&str; 7] = [
    "ADVENTURE_ISLAND_II_SUPER_LONG_NAME.GB",
    "CASTLEVANIA_THE_ADVENTURE.GB",
    "DR_MARIO.GB",
    "KIRBYS_DREAM_LAND.GB",
    "LEGEND_OF_ZELDA_LINKS_AWAKENING_DX.GBC",
    "METROID_II_RETURN_OF_SAMUS.GB",
    "TETRIS.GB",
];

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "--dump") {
        dump_screens(Path::new(OUT_DIR))?;
        return Ok(());
    }

    let mut app = PreviewApp::new();
    let mut terminal = HostTerminal::enter()?;
    app.render(&mut terminal, "ready")?;

    loop {
        let key = read_key()?;
        match key {
            Key::Quit => break,
            Key::Screenshot => {
                let path = app.save_screenshot(Path::new(OUT_DIR))?;
                app.render(&mut terminal, &format!("saved {}", path.display()))?;
            }
            Key::None => {}
            _ => {
                app.handle_key(key);
                app.render(&mut terminal, "")?;
            }
        }
    }

    Ok(())
}

struct PreviewApp {
    screen: Screen,
    shots: usize,
}

impl PreviewApp {
    fn new() -> Self {
        Self {
            screen: Screen::Main(MainMenu::new()),
            shots: 0,
        }
    }

    fn handle_key(&mut self, key: Key) {
        let input = key.input();
        match &mut self.screen {
            Screen::Main(menu) => match menu.handle_main(input, false) {
                MenuEffect::ShowRoms => {
                    self.screen = Screen::Roms(RomListLogic::new(ROM_ITEMS.len()))
                }
                MenuEffect::ShowSettings => self.screen = Screen::Settings(SettingsMenu::new()),
                _ => {}
            },
            Screen::Roms(logic) => match logic.handle(input) {
                RomListEffect::Back => self.screen = Screen::Main(MainMenu::new()),
                RomListEffect::NextPage => logic.reset(ROM_ITEMS.len()),
                RomListEffect::PrevPage => logic.select_last(),
                _ => {}
            },
            Screen::Settings(menu) => match menu.handle(input) {
                MenuEffect::Back => self.screen = Screen::Main(MainMenu::new()),
                MenuEffect::ShowWifiMenu => self.screen = Screen::WifiPortal,
                _ => {}
            },
            Screen::WifiPortal => {
                if input.back {
                    self.screen = Screen::Settings(SettingsMenu::new());
                }
            }
            Screen::WifiConfigured => {
                if input.back {
                    self.screen = Screen::Settings(SettingsMenu::new());
                } else if input.confirm {
                    self.screen = Screen::WifiPortal;
                }
            }
        }
    }

    fn render(&self, terminal: &mut HostTerminal, status: &str) -> io::Result<()> {
        let buf = self.render_buffer()?;
        terminal.draw(&buf, status)
    }

    fn render_buffer(&self) -> io::Result<Buffer> {
        render_screen(&self.screen)
    }

    fn save_screenshot(&mut self, dir: &Path) -> io::Result<PathBuf> {
        fs::create_dir_all(dir)?;
        let name = format!("menu-preview-{:02}.png", self.shots);
        self.shots += 1;
        let path = dir.join(name);
        let buf = self.render_buffer()?;
        save_png(&buf, &path)?;
        save_text(&buf, &path.with_extension("txt"))?;
        Ok(path)
    }
}

enum Screen {
    Main(MainMenu),
    Roms(RomListLogic),
    Settings(SettingsMenu),
    WifiPortal,
    WifiConfigured,
}

fn render_screen(screen: &Screen) -> io::Result<Buffer> {
    let mut terminal =
        Terminal::new(TestBackend::new(COLS, ROWS)).expect("test backend is infallible");
    terminal
        .draw(|f| match screen {
            Screen::Main(menu) => {
                let mut frame = menu.frame(false);
                frame.crash_pending = true;
                draw_menu(f, &frame);
            }
            Screen::Roms(logic) => {
                let enabled = [true; 7];
                let frame = MenuFrame {
                    title: "ROMS",
                    items: &ROM_ITEMS,
                    selected: logic.selected(),
                    marquee_frame: 0,
                    enabled: &enabled,
                    marked: Some(4),
                    crash_pending: false,
                };
                draw_menu(f, &frame);
            }
            Screen::Settings(menu) => {
                let frame = menu.frame(false);
                draw_menu(f, &frame);
            }
            Screen::WifiPortal => {
                let items = ["JOIN WIFI", "RustyBoy", "OPEN", "192.168.4.1"];
                let enabled = [true; 4];
                let frame = MenuFrame {
                    title: "WIFI SETUP",
                    items: &items,
                    selected: usize::MAX,
                    marquee_frame: 0,
                    enabled: &enabled,
                    marked: None,
                    crash_pending: false,
                };
                draw_menu(f, &frame);
            }
            Screen::WifiConfigured => {
                let items = ["SSID: HomeNetwork-With-A-Long-Name.", "FORGET"];
                let enabled = [false, true];
                let frame = MenuFrame {
                    title: "WIFI",
                    items: &items,
                    selected: 1,
                    marquee_frame: 0,
                    enabled: &enabled,
                    marked: None,
                    crash_pending: false,
                };
                draw_menu(f, &frame);
            }
        })
        .expect("test backend draw is infallible");
    Ok(terminal.backend().buffer().clone())
}

fn dump_screens(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;

    let mut roms = RomListLogic::new(ROM_ITEMS.len());
    for _ in 0..4 {
        let _ = roms.handle(MenuInput {
            down: true,
            ..MenuInput::default()
        });
    }

    let screens = [
        ("main", Screen::Main(MainMenu::new())),
        ("roms", Screen::Roms(roms)),
        ("settings", Screen::Settings(SettingsMenu::new())),
        ("wifi-setup", Screen::WifiPortal),
        ("wifi-configured", Screen::WifiConfigured),
    ];

    for (name, screen) in screens {
        let buf = render_screen(&screen)?;
        let png = dir.join(format!("{name}.png"));
        save_png(&buf, &png)?;
        save_text(&buf, &dir.join(format!("{name}.txt")))?;
        println!("{}", png.display());
    }

    Ok(())
}

#[derive(Clone, Copy)]
enum Key {
    Up,
    Down,
    Confirm,
    Back,
    Screenshot,
    Quit,
    None,
}

impl Key {
    fn input(self) -> MenuInput {
        match self {
            Self::Up => MenuInput {
                up: true,
                ..MenuInput::default()
            },
            Self::Down => MenuInput {
                down: true,
                ..MenuInput::default()
            },
            Self::Confirm => MenuInput {
                confirm: true,
                ..MenuInput::default()
            },
            Self::Back => MenuInput {
                back: true,
                ..MenuInput::default()
            },
            _ => MenuInput::default(),
        }
    }
}

fn read_key() -> io::Result<Key> {
    let mut byte = [0u8; 1];
    io::stdin().read_exact(&mut byte)?;
    Ok(match byte[0] {
        b'q' | 0x03 => Key::Quit,
        b's' => Key::Screenshot,
        b'k' | b'w' => Key::Up,
        b'j' => Key::Down,
        b'a' | b'\r' | b'\n' => Key::Confirm,
        b'b' | 0x7f | 0x08 => Key::Back,
        0x1b => read_escape_key()?,
        _ => Key::None,
    })
}

fn read_escape_key() -> io::Result<Key> {
    let mut seq = [0u8; 2];
    if io::stdin().read_exact(&mut seq).is_err() {
        return Ok(Key::None);
    }
    Ok(match seq {
        [b'[', b'A'] => Key::Up,
        [b'[', b'B'] => Key::Down,
        [b'[', b'D'] => Key::Back,
        [b'[', b'C'] => Key::Confirm,
        _ => Key::None,
    })
}

struct HostTerminal;

impl HostTerminal {
    fn enter() -> io::Result<Self> {
        let _ = Command::new("stty").args(["raw", "-echo"]).status();
        let mut out = io::stdout().lock();
        write!(out, "\x1b[?1049h\x1b[?25l\x1b[2J\x1b[H")?;
        out.flush()?;
        Ok(Self)
    }

    fn draw(&mut self, buf: &Buffer, status: &str) -> io::Result<()> {
        let mut out = io::stdout().lock();
        write!(out, "\x1b[H")?;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = buf
                    .cell(ratatui::layout::Position { x, y })
                    .expect("cell inside buffer");
                let fg = rgb(cell.fg);
                let bg = rgb(cell.bg);
                write!(
                    out,
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m{}",
                    fg.0,
                    fg.1,
                    fg.2,
                    bg.0,
                    bg.1,
                    bg.2,
                    cell.symbol()
                )?;
            }
            write!(out, "\x1b[0m\r\n")?;
        }
        write!(
            out,
            "\x1b[0m\r\narrows/j/k move  enter/a select  b/backspace back  s screenshot  q quit\r\n{status}\x1b[K"
        )?;
        out.flush()
    }
}

impl Drop for HostTerminal {
    fn drop(&mut self) {
        let _ = Command::new("stty").arg("sane").status();
        let mut out = io::stdout().lock();
        let _ = write!(out, "\x1b[0m\x1b[?25h\x1b[?1049l");
        let _ = out.flush();
    }
}

fn save_png(buf: &Buffer, path: &Path) -> io::Result<()> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, PNG_W as u32, PNG_H as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut png = encoder
        .write_header()
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))?;
    let data = buffer_pixels(buf);
    png.write_image_data(&data)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e.to_string()))
}

fn save_text(buf: &Buffer, path: &Path) -> io::Result<()> {
    let mut file = File::create(path)?;
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let symbol = buf
                .cell(ratatui::layout::Position { x, y })
                .map(|c| c.symbol())
                .unwrap_or(" ");
            write!(file, "{}", ascii_symbol(symbol))?;
        }
        writeln!(file)?;
    }
    Ok(())
}

fn buffer_pixels(buf: &Buffer) -> Vec<u8> {
    let mut pixels = vec![0u8; PNG_W * PNG_H * 3];
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = buf
                .cell(ratatui::layout::Position { x, y })
                .expect("cell inside buffer");
            draw_cell(
                &mut pixels,
                x as usize * CELL_W,
                y as usize * CELL_H,
                ascii_symbol(cell.symbol()),
                rgb(cell.fg),
                rgb(cell.bg),
            );
        }
    }
    pixels
}

fn draw_cell(
    pixels: &mut [u8],
    x0: usize,
    y0: usize,
    ch: char,
    fg: (u8, u8, u8),
    bg: (u8, u8, u8),
) {
    for y in 0..CELL_H {
        for x in 0..CELL_W {
            put_pixel(pixels, x0 + x, y0 + y, bg);
        }
    }

    if ch == ' ' {
        return;
    }

    let glyph = if ch.is_ascii() { ch as u8 } else { b'?' };
    for y in 0..8 {
        let row = font::glyph_row(glyph, y);
        for x in 0..CELL_W {
            let glyph_x = x * 8 / CELL_W;
            if (row >> glyph_x) & 1 != 0 {
                put_pixel(pixels, x0 + x, y0 + y + 1, fg);
            }
        }
    }
}

fn put_pixel(pixels: &mut [u8], x: usize, y: usize, color: (u8, u8, u8)) {
    if x >= PNG_W || y >= PNG_H {
        return;
    }
    let offset = (y * PNG_W + x) * 3;
    pixels[offset] = color.0;
    pixels[offset + 1] = color.1;
    pixels[offset + 2] = color.2;
}

fn ascii_symbol(symbol: &str) -> char {
    match symbol {
        "\u{2500}" => '-',
        "\u{2502}" => '|',
        "\u{250c}" | "\u{2510}" | "\u{2514}" | "\u{2518}" => '+',
        _ => symbol.chars().next().unwrap_or(' '),
    }
}

fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Black | Color::Reset => (0x08, 0x18, 0x20),
        Color::Gray | Color::DarkGray => (0x34, 0x68, 0x56),
        Color::Green | Color::LightGreen => (0x88, 0xC0, 0x70),
        Color::White => (0xE0, 0xF8, 0xD0),
        Color::Yellow | Color::LightYellow => (0xE0, 0xF8, 0xD0),
        Color::Red | Color::LightRed => (0xE0, 0x20, 0x20),
        Color::Blue | Color::LightBlue => (0x40, 0x80, 0xF0),
        Color::Magenta | Color::LightMagenta => (0xC0, 0x80, 0xC0),
        Color::Cyan | Color::LightCyan => (0x88, 0xC0, 0x70),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(_) => (0x88, 0xC0, 0x70),
    }
}
