//! Display viewer — renders display output to PNG files on the host.
//!
//! Usage:
//!   cargo run -p display-viewer -- splash         # all splash frames → /tmp/splash_NNN.png
//!   cargo run -p display-viewer -- splash --last  # final splash frame only → /tmp/splash_final.png
//!   cargo run -p display-viewer -- frame          # test GB framebuffer → /tmp/frame.png

use rustyboy_pico2w::display::{
    fb::FbDisplay,
    Display, SCREEN_H, SCREEN_W,
};

fn make_display() -> Display<FbDisplay> {
    Display::from_draw_target(FbDisplay::new(SCREEN_W as u32, SCREEN_H as u32))
}

fn cmd_splash(last_only: bool) {
    let mut disp = make_display();
    let mut frame = 0u32;
    let mut saved = 0u32;

    loop {
        let done = disp.splash_step(frame);

        if !last_only {
            let path = format!("/tmp/splash_{:03}.png", frame);
            disp.save_png(&path).expect("failed to write PNG");
            println!("wrote {path}");
            saved += 1;
        }

        if done {
            let path = "/tmp/splash_final.png".to_string();
            disp.save_png(&path).expect("failed to write PNG");
            println!("wrote {path}");
            saved += 1;
            break;
        }

        frame += 1;
    }

    println!("{saved} frame(s) written.");
}

fn cmd_frame() {
    let mut disp = make_display();

    // Checkerboard pattern cycling through all 4 DMG palette entries.
    let mut fb = [0u8; 23040];
    for y in 0..144usize {
        for x in 0..160usize {
            fb[y * 160 + x] = ((x / 8 + y / 8) % 4) as u8;
        }
    }

    disp.render_frame(&fb);
    disp.save_png("/tmp/frame.png").expect("failed to write PNG");
    println!("wrote /tmp/frame.png");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("splash") => {
            let last_only = args.iter().any(|a| a == "--last");
            cmd_splash(last_only);
        }
        Some("frame") => cmd_frame(),
        _ => {
            eprintln!("usage: display-viewer <splash [--last] | frame>");
            std::process::exit(1);
        }
    }
}
