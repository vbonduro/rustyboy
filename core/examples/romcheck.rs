//! Headless reproduction of the web client's run loop, used to find ROMs that
//! freeze the browser. Mirrors `EmulatorHandle::{new,run_frame}` in
//! platform/web/client/src/lib.rs exactly (same DMG post-boot register state,
//! same CYCLES_PER_FRAME budget), but adds a per-frame tick cap so a non-
//! advancing cycle counter is reported as STALL instead of hanging forever.
//!
//! Usage:
//!   cargo run -p rustyboy-core --example romcheck -- <rom-or-dir> [frames]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use rustyboy_core::{
    cpu::registers::{Flags, Registers},
    GameBoy,
};

const CYCLES_PER_FRAME: u32 = 70224;
// Generous ceiling: if a single frame needs >8× its cycle budget in ticks and
// still hasn't advanced the counter past the target, the CPU is stuck.
const MAX_TICKS_PER_FRAME: u64 = CYCLES_PER_FRAME as u64 * 8;

fn build(rom: Vec<u8>) -> GameBoy {
    GameBoy::new(rom)
        .with_registers(Registers {
            a: 0x01,
            f: Flags::from_bits_truncate(0xB0),
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            pc: 0x0100,
            sp: 0xFFFE,
        })
        .with_dmg_state()
}

enum Outcome {
    Ok,
    Stall { frame: u32, pc: u16 },
    Panic(String),
    BuildPanic(String),
}

fn run_rom(bytes: Vec<u8>, frames: u32) -> Outcome {
    let mut gb = match catch_unwind(AssertUnwindSafe(|| build(bytes))) {
        Ok(gb) => gb,
        Err(e) => return Outcome::BuildPanic(panic_msg(e)),
    };

    let result = catch_unwind(AssertUnwindSafe(|| {
        for f in 0..frames {
            let start = gb.cycle_counter();
            let mut ticks = 0u64;
            while gb.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
                gb.tick();
                ticks += 1;
                if ticks > MAX_TICKS_PER_FRAME {
                    return Some((f, gb.registers().pc));
                }
            }
        }
        None
    }));

    match result {
        Ok(None) => Outcome::Ok,
        Ok(Some((frame, pc))) => Outcome::Stall { frame, pc },
        Err(e) => Outcome::Panic(panic_msg(e)),
    }
}

fn panic_msg(e: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = e.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = e.downcast_ref::<String>() {
        s.clone()
    } else {
        "<non-string panic>".to_string()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let target = args.get(1).map(PathBuf::from).unwrap_or_else(|| {
        eprintln!("usage: romcheck <rom-or-dir> [frames]");
        std::process::exit(2);
    });
    let frames: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(600);

    let mut roms: Vec<PathBuf> = Vec::new();
    if target.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&target)
            .unwrap_or_else(|e| panic!("read_dir {}: {e}", target.display()))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                matches!(
                    p.extension().and_then(|x| x.to_str()),
                    Some("gb") | Some("gbc")
                )
            })
            .collect();
        entries.sort();
        roms = entries;
    } else {
        roms.push(target);
    }

    println!("Checking {} ROM(s), {frames} frames each:\n", roms.len());
    let mut bad = 0;
    for rom in &roms {
        let name = rom.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let bytes = match std::fs::read(rom) {
            Ok(b) => b,
            Err(e) => {
                println!("  [READ-ERR] {name}: {e}");
                bad += 1;
                continue;
            }
        };
        match run_rom(bytes, frames) {
            Outcome::Ok => println!("  [ok]    {name}"),
            Outcome::Stall { frame, pc } => {
                println!("  [STALL] {name}  — cycle counter stuck at frame {frame}, PC={pc:04X}");
                bad += 1;
            }
            Outcome::Panic(m) => {
                println!("  [PANIC] {name}  — {m}");
                bad += 1;
            }
            Outcome::BuildPanic(m) => {
                println!("  [PANIC@new] {name}  — {m}");
                bad += 1;
            }
        }
    }
    println!("\n{} ok, {} problematic", roms.len() - bad, bad);
    if bad > 0 {
        std::process::exit(1);
    }
}
