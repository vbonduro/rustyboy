//! Integration tests using Blargg's cpu_instrs test ROMs.
//!
//! These tests load a real Game Boy ROM, run it to completion (until HALT),
//! and assert the expected serial output. ROM files live under roms/blargg/.

use rustyboy::cpu::cpu::Cpu;
use rustyboy::cpu::instructions::opcodes::OpCodeDecoder;
use rustyboy::cpu::peripheral::bus::Peripheral;
use rustyboy::cpu::peripheral::serial::SerialPort;
use rustyboy::cpu::sm83::Sm83;
use rustyboy::memory::memory::{BusEvent, GameBoyMemory, Memory};
use std::cell::RefCell;
use std::rc::Rc;

/// Wraps SerialPort behind Rc<RefCell> so we can inspect output after the run.
struct SharedSerial(Rc<RefCell<SerialPort>>);

impl Peripheral for SharedSerial {
    fn handle(&mut self, event: &BusEvent, mem: &mut dyn Memory) {
        self.0.borrow_mut().handle(event, mem);
    }
}

fn run_rom(path: &str) -> String {
    let rom_data = std::fs::read(path)
        .unwrap_or_else(|_| panic!("ROM not found: {} — run tests from repo root", path));

    let memory = Box::new(GameBoyMemory::with_rom(rom_data));
    let decoder = Box::new(OpCodeDecoder::new());
    let mut cpu = Sm83::new(memory, decoder);

    // Skip the boot ROM — jump straight to the cartridge entry point.
    cpu.set_pc(0x0100);
    cpu.set_sp(0xFFFE);

    let serial = Rc::new(RefCell::new(SerialPort::new()));
    cpu.subscribe_peripheral(0xFF02..=0xFF02, Box::new(SharedSerial(serial.clone())));

    const MAX_TICKS: u64 = 50_000_000;
    let mut ticks = 0u64;
    while !cpu.is_halted() && ticks < MAX_TICKS {
        cpu.tick().unwrap();
        ticks += 1;
    }

    let output = serial.borrow().output().to_vec();
    String::from_utf8_lossy(&output).into_owned()
}

#[test]
fn test_blargg_01_special() {
    let output = run_rom("roms/blargg/cpu_instrs/individual/01-special.gb");
    assert!(
        output.contains("Passed"),
        "Expected 'Passed' in serial output, got: {:?}",
        output
    );
}
