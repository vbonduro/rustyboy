//! Shared test harness utilities for integration tests.
#![allow(dead_code)]

use rustyboy_core::cpu::cpu::Cpu;
use rustyboy_core::cpu::instructions::opcodes::OpCodeDecoder;
use rustyboy_core::cpu::peripheral::bus::Peripheral;
use rustyboy_core::cpu::peripheral::serial::SerialPort;
use rustyboy_core::cpu::registers::Registers;
use rustyboy_core::cpu::sm83::Sm83;
use rustyboy_core::memory::memory::{BusEvent, GameBoyMemory, Memory};
use std::cell::RefCell;
use std::rc::Rc;

/// Resolve a ROM path relative to the workspace root.
pub fn rom_path(relative: &str) -> std::path::PathBuf {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    workspace_root.join(relative)
}

/// Load a ROM file and return its bytes.
pub fn load_rom(path: &str) -> Vec<u8> {
    let full_path = rom_path(path);
    std::fs::read(&full_path)
        .unwrap_or_else(|_| panic!("ROM not found: {}", full_path.display()))
}

/// Wrapper to share a SerialPort via Rc<RefCell>.
struct SharedSerial(Rc<RefCell<SerialPort>>);

impl Peripheral for SharedSerial {
    fn handle(&mut self, event: &BusEvent, mem: &mut dyn Memory) {
        self.0.borrow_mut().handle(event, mem);
    }
}

/// Run a Blargg-style ROM that outputs results via serial port.
/// Returns the serial output as a string.
pub fn run_blargg_rom(path: &str) -> String {
    let rom_data = load_rom(path);
    let memory = Box::new(GameBoyMemory::with_rom(rom_data));
    let decoder = Box::new(OpCodeDecoder::new());
    let mut cpu = Sm83::new(memory, decoder).with_registers(Registers {
        pc: 0x0100,
        sp: 0xFFFE,
        ..Default::default()
    });

    let serial = Rc::new(RefCell::new(SerialPort::new()));
    cpu.subscribe_peripheral(0xFF02..=0xFF02, Box::new(SharedSerial(serial.clone())));

    const MAX_TICKS: u64 = 50_000_000;
    let mut ticks = 0u64;
    while ticks < MAX_TICKS {
        cpu.tick().unwrap();
        ticks += 1;
        if ticks % 1024 == 0 {
            let out = serial.borrow();
            let bytes = out.output();
            if bytes.ends_with(b"Passed\n") || bytes.ends_with(b"Failed\n") {
                break;
            }
        }
    }

    let output = serial.borrow().output().to_vec();
    String::from_utf8_lossy(&output).into_owned()
}

/// Assert that a Blargg ROM's serial output contains "Passed".
pub fn assert_blargg_passed(path: &str, name: &str) {
    let output = run_blargg_rom(path);
    assert!(
        output.contains("Passed"),
        "{}: expected 'Passed' in serial output, got: {:?}",
        name,
        output
    );
}

/// Run a Mooneye-style ROM that signals completion via HALT and
/// indicates pass/fail via Fibonacci register values.
///
/// Pass: B=3, C=5, D=8, E=13, H=21, L=34
/// Fail: B=0x42, C=0x42, D=0x42, E=0x42, H=0x42, L=0x42
pub fn run_mooneye_rom(path: &str) -> MooneyeResult {
    let rom_data = load_rom(path);
    let memory = Box::new(GameBoyMemory::with_rom(rom_data));
    let decoder = Box::new(OpCodeDecoder::new());
    let mut cpu = Sm83::new(memory, decoder).with_registers(Registers {
        pc: 0x0100,
        sp: 0xFFFE,
        ..Default::default()
    });

    // Mooneye tests complete within 120 emulated seconds.
    // At ~4 MHz that's ~480 million cycles, but most finish much faster.
    const MAX_TICKS: u64 = 100_000_000;
    let mut ticks = 0u64;
    while ticks < MAX_TICKS {
        cpu.tick().unwrap();
        ticks += 1;
        if cpu.is_halted() {
            break;
        }
    }

    let regs = cpu.registers();
    if regs.b == 3 && regs.c == 5 && regs.d == 8 && regs.e == 13 && regs.h == 21 && regs.l == 34
    {
        MooneyeResult::Pass
    } else if regs.b == 0x42
        && regs.c == 0x42
        && regs.d == 0x42
        && regs.e == 0x42
        && regs.h == 0x42
        && regs.l == 0x42
    {
        MooneyeResult::Fail
    } else if ticks >= MAX_TICKS {
        MooneyeResult::Timeout
    } else {
        MooneyeResult::Unknown(regs)
    }
}

#[derive(Debug)]
pub enum MooneyeResult {
    Pass,
    Fail,
    Timeout,
    Unknown(Registers),
}

/// Assert that a Mooneye ROM passed.
pub fn assert_mooneye_passed(path: &str, name: &str) {
    let result = run_mooneye_rom(path);
    match result {
        MooneyeResult::Pass => {}
        MooneyeResult::Fail => panic!("{}: Mooneye test reported FAIL", name),
        MooneyeResult::Timeout => panic!("{}: Mooneye test timed out", name),
        MooneyeResult::Unknown(regs) => {
            panic!("{}: Mooneye test ended with unexpected registers: {:?}", name, regs)
        }
    }
}

/// Run a ROM for a given number of frames and return a copy of the framebuffer.
/// One frame = 70,224 dots = ~17,556 CPU ticks (at 4 cycles per tick average).
pub fn run_rom_frames(path: &str, frames: u32) -> Vec<u8> {
    let rom_data = load_rom(path);
    let memory = Box::new(GameBoyMemory::with_rom(rom_data));
    let decoder = Box::new(OpCodeDecoder::new());
    let mut cpu = Sm83::new(memory, decoder).with_registers(Registers {
        pc: 0x0100,
        sp: 0xFFFE,
        ..Default::default()
    });

    // Each frame is 154 scanlines * 456 dots = 70,224 dots.
    // CPU tick returns ~4-20 cycles per instruction.
    // Run enough ticks to cover the requested frames.
    let total_dots: u64 = frames as u64 * 70_224;
    let mut dots_elapsed: u64 = 0;
    while dots_elapsed < total_dots {
        let cycles = cpu.tick().unwrap() as u64;
        dots_elapsed += cycles;
    }

    cpu.framebuffer().to_vec()
}

/// Load a reference PNG image and return the pixels as 2-bit shade values.
/// The reference image should be 160x144 with 4 shades of gray.
pub fn load_reference_png(path: &str) -> Vec<u8> {
    let full_path = rom_path(path);
    let file = std::fs::File::open(&full_path)
        .unwrap_or_else(|_| panic!("Reference image not found: {}", full_path.display()));
    let mut decoder = png::Decoder::new(file);
    // Expand sub-byte grayscale (2-bit, 4-bit) to full 8-bit per pixel
    decoder.set_transformations(png::Transformations::EXPAND);
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).unwrap();
    let bytes = &buf[..info.buffer_size()];

    // Map grayscale pixel values to 2-bit shade indices.
    // After EXPAND, 2-bit values become: 0x00, 0x55, 0xAA, 0xFF
    // DMG shades: 0=lightest, 3=darkest
    let total_pixels = (info.width * info.height) as usize;
    let mut shades = Vec::with_capacity(total_pixels);

    for i in 0..total_pixels {
        let gray = bytes[i];
        let shade = match gray {
            0xC0..=0xFF => 0, // white
            0x80..=0xBF => 1, // light gray
            0x40..=0x7F => 2, // dark gray
            0x00..=0x3F => 3, // black
        };
        shades.push(shade);
    }

    shades
}
