use wasm_bindgen::prelude::*;

use rustyboy_core::{
    cpu::{
        cpu::Cpu,
        instructions::opcodes::OpCodeDecoder,
        peripheral::joypad::Button,
        registers::{Flags, Registers},
        sm83::Sm83,
    },
    memory::GameBoyMemory,
};

const CYCLES_PER_FRAME: u32 = 70224;
const SCREEN_WIDTH: usize = 160;
const SCREEN_HEIGHT: usize = 144;
const RGBA_FRAMEBUFFER_SIZE: usize = SCREEN_WIDTH * SCREEN_HEIGHT * 4;

// DMG green palette: palette index → RGBA
const PALETTE: [[u8; 4]; 4] = [
    [0xE0, 0xF8, 0xD0, 0xFF], // 0 - lightest green
    [0x88, 0xC0, 0x70, 0xFF], // 1
    [0x34, 0x68, 0x56, 0xFF], // 2
    [0x08, 0x18, 0x20, 0xFF], // 3 - darkest
];

#[wasm_bindgen]
pub struct EmulatorHandle {
    cpu: Sm83,
    rgba_buf: Vec<u8>,
}

#[wasm_bindgen]
impl EmulatorHandle {
    #[wasm_bindgen(constructor)]
    pub fn new(rom: Vec<u8>) -> EmulatorHandle {
        let memory = GameBoyMemory::with_rom(rom);
        let decoder = Box::new(OpCodeDecoder::new());
        // Start at 0x100 with DMG post-boot-ROM state (skips boot ROM).
        let cpu = Sm83::new(Box::new(memory), decoder)
            .with_registers(Registers {
                a: 0x01, f: Flags::from_bits_truncate(0xB0),
                b: 0x00, c: 0x13,
                d: 0x00, e: 0xD8,
                h: 0x01, l: 0x4D,
                pc: 0x0100,
                sp: 0xFFFE,
            })
            .with_dmg_state();
        EmulatorHandle {
            cpu,
            rgba_buf: vec![0u8; RGBA_FRAMEBUFFER_SIZE],
        }
    }

    pub fn run_frame(&mut self) {
        let start = self.cpu.cycle_counter();
        while self.cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = self.cpu.tick();
        }
    }

    /// Returns the framebuffer as an RGBA8 Vec for use in JS as Uint8ClampedArray.
    pub fn framebuffer_rgba(&mut self) -> Vec<u8> {
        let fb = self.cpu.framebuffer();
        for (i, &pixel) in fb.iter().enumerate() {
            let color = PALETTE[(pixel & 3) as usize];
            self.rgba_buf[i * 4..i * 4 + 4].copy_from_slice(&color);
        }
        self.rgba_buf.clone()
    }

    /// button: 0=Right 1=Left 2=Up 3=Down 4=A 5=B 6=Select 7=Start
    pub fn set_button(&mut self, button: u8, pressed: bool) {
        let btn = match button {
            0 => Button::Right,
            1 => Button::Left,
            2 => Button::Up,
            3 => Button::Down,
            4 => Button::A,
            5 => Button::B,
            6 => Button::Select,
            7 => Button::Start,
            _ => return,
        };
        self.cpu.set_button(btn, pressed);
    }
}
