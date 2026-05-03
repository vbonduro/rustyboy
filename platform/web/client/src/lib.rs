use wasm_bindgen::prelude::*;

use rustyboy_core::{
    cpu::{
        cpu::Cpu,
        peripheral::joypad::Button,
        registers::{Flags, Registers},
        save_state::SaveState,
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
        // Start at 0x100 with DMG post-boot-ROM state (skips boot ROM).
        let cpu = Sm83::new(Box::new(memory))
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

    /// Drain accumulated PCM audio samples since the last call.
    /// Returns interleaved stereo f32 samples [L, R, L, R, ...] at 48,000 Hz.
    /// Pass to an AudioContext for playback.
    pub fn drain_audio_samples(&mut self) -> Vec<f32> {
        self.cpu.drain_audio_samples()
    }

    /// Returns a debug string with key PPU/interrupt state for on-screen display.
    /// Only available when compiled with the `debug-overlay` feature.
    #[cfg(feature = "debug-overlay")]
    pub fn debug_state(&self) -> String {
        let read = |a: u16| self.cpu.read_memory(a).unwrap_or(0);
        let lcdc = read(0xFF40);
        let stat = read(0xFF41);
        let ly   = read(0xFF44);
        let lyc  = read(0xFF45);
        let scx  = read(0xFF43);
        let scy  = read(0xFF42);
        let if_  = read(0xFF0F);
        let ie   = read(0xFFFF);
        let bgp  = read(0xFF47);
        let obp0 = read(0xFF48);
        let ffeb = read(0xFFEB);
        let ffe6 = read(0xFFE6);
        let ime  = if self.cpu.ime() { "1" } else { "0" };
        let bank = self.cpu.current_rom_bank();
        let pc = self.cpu.registers().pc;
        format!(
            "PC={:04X} ROM={:02}\nLY={:3} LYC={:3} SCX={:3} SCY={:3}\nLCDC={:02X} OBJ={} WIN={} BG={}\nSTAT={:02X} IF={:02X} IE={:02X} IME={}\nBGP={:02X} OBP0={:02X}\nFFEB={:02X} FFE6={:02X}",
            pc, bank,
            ly, lyc, scx, scy,
            lcdc, (lcdc >> 1) & 1, (lcdc >> 5) & 1, lcdc & 1,
            stat, if_, ie, ime,
            bgp, obp0,
            ffeb, ffe6
        )
    }

    /// Serialize the full emulator state to a byte blob (save state).
    pub fn save_state(&self) -> Vec<u8> {
        self.cpu.save_state()
    }

    /// Restore emulator state from a blob produced by `save_state`.
    pub fn load_state(&mut self, data: Vec<u8>) -> Result<(), JsValue> {
        let state = SaveState::from_blob(data).map_err(|e| JsValue::from_str(e))?;
        self.cpu.load_state(state).map_err(|e| JsValue::from_str(e))
    }

    /// Returns the cartridge external RAM (battery save) as bytes, or an empty Vec
    /// if this cartridge has no external RAM.
    pub fn get_battery_save(&self) -> Vec<u8> {
        self.cpu.external_ram().map(|s| s.to_vec()).unwrap_or_default()
    }

    /// Writes battery save data into the cartridge external RAM.
    /// No-op if this cartridge has no external RAM.
    pub fn set_battery_save(&mut self, data: Vec<u8>) {
        self.cpu.set_external_ram(&data);
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
