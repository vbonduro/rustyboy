//! Integration tests using Blargg's cpu_instrs test ROMs.
//!
//! These tests load a real Game Boy ROM, run it to completion (until HALT),
//! and assert the expected serial output. ROM files live under roms/blargg/.
//!
//! Failing tests are marked #[ignore] — they document known gaps and serve as
//! the red phase for future TDD work:
//!   02-interrupts: requires interrupt dispatch (IME/IE/IF)
//!   03-op sp,hl:   ADD HL,SP (opcode 0x39) produces wrong flags
//!   05-op rp:      ADD HL,rr (09/19/29) produce wrong flags

use rustyboy::cpu::cpu::Cpu;
use rustyboy::cpu::instructions::opcodes::OpCodeDecoder;
use rustyboy::cpu::peripheral::bus::Peripheral;
use rustyboy::cpu::peripheral::serial::SerialPort;
use rustyboy::cpu::sm83::Sm83;
use rustyboy::memory::memory::{BusEvent, GameBoyMemory, Memory};
use std::cell::RefCell;
use std::rc::Rc;

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
    let mut cpu = Sm83::new(memory, decoder).with_registers(rustyboy::cpu::registers::Registers {
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
        // Check serial output periodically for completion markers.
        // ROMs that use interrupts (e.g. 02-interrupts) HALT and wake multiple
        // times, so we can't use is_halted() as a termination condition.
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

fn assert_passed(output: &str, rom: &str) {
    assert!(
        output.contains("Passed"),
        "{}: expected 'Passed' in serial output, got: {:?}",
        rom,
        output
    );
}

// --- Passing tests ---

#[test]
fn test_blargg_01_special() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/01-special.gb"),
        "01-special",
    );
}

#[test]
fn test_blargg_04_op_r_imm() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/04-op r,imm.gb"),
        "04-op r,imm",
    );
}

#[test]
fn test_blargg_06_ld_r_r() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/06-ld r,r.gb"),
        "06-ld r,r",
    );
}

#[test]
fn test_blargg_07_jr_jp_call_ret_rst() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/07-jr,jp,call,ret,rst.gb"),
        "07-jr,jp,call,ret,rst",
    );
}

#[test]
fn test_blargg_08_misc_instrs() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/08-misc instrs.gb"),
        "08-misc instrs",
    );
}

#[test]
fn test_blargg_09_op_r_r() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/09-op r,r.gb"),
        "09-op r,r",
    );
}

#[test]
fn test_blargg_10_bit_ops() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/10-bit ops.gb"),
        "10-bit ops",
    );
}

#[test]
fn test_blargg_11_op_a_hl() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/11-op a,(hl).gb"),
        "11-op a,(hl)",
    );
}

// --- Known failing tests (red phase for future TDD) ---
#[test]
fn test_blargg_02_interrupts() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/02-interrupts.gb"),
        "02-interrupts",
    );
}

#[test]
fn test_blargg_03_op_sp_hl() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/03-op sp,hl.gb"),
        "03-op sp,hl",
    );
}

#[test]
fn test_blargg_05_op_rp() {
    assert_passed(
        &run_rom("roms/blargg/cpu_instrs/individual/05-op rp.gb"),
        "05-op rp",
    );
}
