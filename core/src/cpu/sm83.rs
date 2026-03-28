use alloc::{boxed::Box, format};

use super::cpu::{Cpu, CpuError};
use super::instructions::adc::opcode::Adc;
use super::instructions::add::opcode::{Add16, Add8, AddSP16};
use super::instructions::call::opcode::{Call, CallOp};
use super::instructions::cb::decoder::CbDecoder;
use super::instructions::cb::opcode::{CbInstruction, CbOp, CbTarget};
use super::instructions::cp::opcode::Cp8;
use super::instructions::decoder::Decoder;
use super::instructions::inc_dec::opcode::{Dec16, Dec8, Inc16, Inc8};
use super::instructions::instructions::{Error as InstructionError, Instructions};
use super::instructions::jump::opcode::{Condition, Jump, JumpOp};
use super::instructions::ld::opcode::Ld8;
use super::instructions::ld16::opcode::Ld16;
use super::instructions::logic::opcode::{And8, Or8, Xor8};
use super::instructions::misc::opcode::Misc;
use super::instructions::operand::*;
use super::instructions::ret::opcode::{Ret, RetOp};
use super::instructions::rotate::opcode::{Rotate, RotateOp};
use super::instructions::rst::opcode::Rst;
use super::instructions::sbc::opcode::Sbc8;
use super::instructions::stack::opcode::{Pop16, Push16};
use super::instructions::sub::opcode::Sub8;
use super::operations::add::*;
use super::operations::cb::{
    bit_u8, res_u8, rl_u8, rlc_u8, rr_u8, rrc_u8, set_u8, sla_u8, sra_u8, srl_u8, swap_u8,
};
use super::operations::inc_dec::{dec_u8, inc_u8};
use super::operations::logic::{and_u8, or_u8, xor_u8};
use super::operations::misc::daa_u8;
use super::operations::sub::*;
use super::peripheral::apu::{
    ApuPeripheral, NR10_ADDR, NR52_ADDR, WAVE_RAM_START, WAVE_RAM_END,
};
use super::peripheral::joypad::{Button, JoypadPeripheral, JOYP_ADDR, JOYPAD_INTERRUPT_BIT};
use super::peripheral::serial::SerialPort;
use super::peripheral::ppu::{
    PpuInput, PpuPeripheral, FRAMEBUFFER_SIZE, LCDC_ADDR, STAT_ADDR, SCY_ADDR, SCX_ADDR,
    LY_ADDR, LYC_ADDR, BGP_ADDR, OBP0_ADDR, OBP1_ADDR, WY_ADDR, WX_ADDR,
    VBLANK_INTERRUPT_BIT, STAT_INTERRUPT_BIT,
};
use super::peripheral::timer::{
    TimerInput, TimerPeripheral, DIV_ADDR, TIMA_ADDR, TIMER_INTERRUPT_BIT, TMA_ADDR, TAC_ADDR,
};
use super::registers::{Flags, Registers};

use crate::memory::memory::{Error as MemoryError, GameBoyMemory, Memory as MemoryBus};

impl From<MemoryError> for InstructionError {
    fn from(error: MemoryError) -> Self {
        InstructionError::Failed(format!("Failed to access memory: {}", error))
    }
}

/// Interrupt Master Enable state. EI has a 1-instruction delay before IME becomes active.
#[derive(Debug, PartialEq, Clone, Copy)]
enum ImeState {
    Disabled,
    /// EI was just executed — IME activates after the next instruction.
    Pending,
    Enabled,
}

const IF_ADDR: u16 = 0xFF0F;
const DMA_ADDR: u16 = 0xFF46;
const IE_ADDR: u16 = 0xFFFF;
const SB_ADDR: u16 = 0xFF01;
const SC_ADDR: u16 = 0xFF02;

pub struct Sm83 {
    memory: Box<GameBoyMemory>,
    registers: Registers,
    opcodes: Box<dyn Decoder>,
    serial: SerialPort,
    timer: TimerPeripheral,
    ppu: PpuPeripheral,
    apu: ApuPeripheral,
    joypad: JoypadPeripheral,
    ime: ImeState,
    halted: bool,
    /// Cycle counter incremented by 4 on each M-cycle (bus_read/bus_write/tick_cycle).
    cycle_counter: u64,
}

impl Sm83 {
    pub fn new(memory: Box<GameBoyMemory>, opcode_decoder: Box<dyn Decoder>) -> Self {
        let joypad = JoypadPeripheral::new();
        let mut sm83 = Self {
            memory,
            registers: Registers::default(),
            opcodes: opcode_decoder,
            serial: SerialPort::new(),
            timer: TimerPeripheral::new(),
            ppu: PpuPeripheral::new(),
            apu: ApuPeripheral::new(),
            joypad,
            ime: ImeState::Disabled,
            halted: false,
            cycle_counter: 0,
        };
        // Seed JOYP with no buttons pressed (all lines high).
        sm83.memory.write_io(JOYP_ADDR, sm83.joypad.read());
        // Seed IO memory with initial APU register read values so games
        // reading registers before any write see correct masked values.
        for addr in NR10_ADDR..=NR52_ADDR {
            sm83.memory.write_io(addr, sm83.apu.read_register(addr));
        }
        // Unused APU addresses always read as 0xFF
        for addr in 0xFF27u16..WAVE_RAM_START {
            sm83.memory.write_io(addr, 0xFF);
        }
        for addr in WAVE_RAM_START..=WAVE_RAM_END {
            let offset = (addr - WAVE_RAM_START) as u8;
            sm83.memory.write_io(addr, sm83.apu.read_wave_ram(offset));
        }
        sm83
    }

    /// Press or release a button. Fires the joypad interrupt if the button is
    /// newly pressed and its select line is active.
    pub fn set_button(&mut self, button: Button, pressed: bool) {
        let interrupt = self.joypad.set_button(button, pressed);
        self.memory.write_io(JOYP_ADDR, self.joypad.read());
        if interrupt {
            let if_val = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_val | (1 << JOYPAD_INTERRUPT_BIT));
        }
    }

    /// Subscribe a peripheral to receive bus events for the given address range.
    /// Returns all bytes captured by the serial port (SB transfers via SC).
    pub fn serial_output(&self) -> &[u8] {
        self.serial.output()
    }

    // Retrieve a copy of the CPU registers.
    pub fn registers(&self) -> Registers {
        self.registers.clone()
    }

    /// Returns true if the interrupt master enable flag is active.
    pub fn ime(&self) -> bool {
        self.ime == ImeState::Enabled
    }

    /// Returns true if the CPU is halted (waiting for an interrupt).
    pub fn is_halted(&self) -> bool {
        self.halted
    }

    /// Read a byte from the memory bus (for test/debug access).
    pub fn read_memory(&self, address: u16) -> Result<u8, MemoryError> {
        self.memory.read(address)
    }

    /// Returns a reference to the PPU framebuffer (160x144 pixels, 2-bit shade per pixel).
    pub fn cycle_counter(&self) -> u64 {
        self.cycle_counter
    }

    pub fn framebuffer(&self) -> &[u8; FRAMEBUFFER_SIZE] {
        self.ppu.framebuffer()
    }

    /// Drain accumulated PCM audio samples since the last call.
    /// Returns interleaved stereo f32 samples [L, R, L, R, ...] at 48,000 Hz.
    pub fn drain_audio_samples(&mut self) -> alloc::vec::Vec<f32> {
        self.apu.drain_samples()
    }

    /// Builder method to set initial register state. Used to skip the boot ROM
    /// by setting PC to 0x0100 and SP to 0xFFFE.
    pub fn with_registers(mut self, registers: Registers) -> Self {
        self.registers = registers;
        self
    }

    /// Seed IO registers to their DMG post-boot-ROM state so games that poll
    /// LY or check LCDC before enabling the LCD work correctly without a boot ROM.
    pub fn with_dmg_state(mut self) -> Self {
        // Post-boot IO register values (DMG, SGB, MGB verified against Pan Docs)
        self.memory.write_io(LCDC_ADDR, 0x91); // LCD on, BG on, sprites off, window off
        self.memory.write_io(STAT_ADDR, 0x85); // PPU in VBlank (mode 1), LYC=LY
        self.memory.write_io(BGP_ADDR,  0xFC); // background palette: 3,3,3,0
        self.memory.write_io(OBP0_ADDR, 0xFF); // obj palette 0
        self.memory.write_io(OBP1_ADDR, 0xFF); // obj palette 1
        // APU post-boot state (Pan Docs)
        self.write_apu_register(0xFF26, 0xF1); // NR52: APU on, ch1 active
        self.write_apu_register(0xFF25, 0xF3); // NR51: ch1-3 right, ch1-4 left
        self.write_apu_register(0xFF24, 0x77); // NR50: max volume both sides
        self
    }

    // ── M-cycle–accurate bus access ─────────────────────────────────────────

    /// Perform a bus read: advance all peripherals by one M-cycle (4 T-cycles),
    /// process any pending bus events, then read from memory.
    /// Wave RAM reads (0xFF30-0xFF3F) are routed through the APU so that
    /// reads while ch3 is on return the current sample buffer.
    fn bus_read(&mut self, addr: u16) -> Result<u8, MemoryError> {
        if (WAVE_RAM_START..=WAVE_RAM_END).contains(&addr) {
            // Wave RAM reads require T-cycle precision. The sample is latched
            // at T3 within the M-cycle on real hardware.
            self.tick_cycle_to_t3();
            let offset = (addr - WAVE_RAM_START) as u8;
            let value = self.apu.read_wave_ram(offset);
            self.advance_timer_apu(1);
            return Ok(value);
        }
        self.tick_cycle();
        self.memory.read(addr)
    }

    /// Perform a bus write: advance all peripherals by one M-cycle (4 T-cycles),
    /// process any pending bus events, then write to memory.
    ///
    /// APU register and wave RAM writes are applied at T3 within the M-cycle
    /// for T-cycle accurate timing.
    fn bus_write(&mut self, addr: u16, value: u8) -> Result<(), MemoryError> {
        if (NR10_ADDR..=NR52_ADDR).contains(&addr) {
            self.tick_cycle_to_t3();
            self.write_apu_register(addr, value);
            self.advance_timer_apu(1);
            return Ok(());
        }
        if (WAVE_RAM_START..=WAVE_RAM_END).contains(&addr) {
            self.tick_cycle_to_t3();
            self.write_wave_ram(addr, value);
            self.advance_timer_apu(1);
            return Ok(());
        }
        self.tick_cycle();
        self.memory.write(addr, value)
    }

    /// Advance peripherals by one M-cycle (4 T-cycles) without a bus access.
    /// Used for internal M-cycles (e.g. ALU operations, SP adjustment).
    fn tick_cycle(&mut self) {
        self.cycle_counter += 4;
        self.route_bus_events();
        self.advance_peripherals(4);
    }

    fn advance_peripherals(&mut self, cycles: u16) {
        self.advance_ppu(cycles);
        // Timer and APU advance per T-cycle so the APU sees the correct
        // intermediate DIV counter at each step. This gives T-cycle accurate
        // wave channel position tracking needed for dmg_sound tests 09/10/12.
        for _ in 0..cycles {
            self.advance_timer_apu(1);
        }
    }

    /// Tick peripherals through the first 3 T-cycles of an M-cycle (T1–T3),
    /// stopping so the caller can perform a time-sensitive APU read or write at T3.
    /// The caller must call `advance_timer_apu(1)` afterwards to complete T4.
    ///
    /// PPU advances for the full M-cycle up front; only timer+APU need per-T-cycle
    /// precision for wave channel position tracking.
    fn tick_cycle_to_t3(&mut self) {
        self.cycle_counter += 4;
        self.route_bus_events();
        self.advance_ppu(4);
        for _ in 0..3 {
            self.advance_timer_apu(1);
        }
    }

    /// Advance timer and APU together by `cycles` T-cycles.
    fn advance_timer_apu(&mut self, cycles: u16) {
        for _ in 0..cycles {
            self.advance_timer(1);
            self.advance_apu(1);
        }
    }

    // ── Tick phase helpers ──────────────────────────────────────────────────

    fn advance_ime(&mut self) {
        if self.ime == ImeState::Pending {
            self.ime = ImeState::Enabled;
        }
    }

    fn route_bus_events(&mut self) {
        let events = self.memory.drain_events();
        for event in &events {
            self.handle_bus_event(event.address, event.value);
        }
    }

    fn handle_bus_event(&mut self, addr: u16, value: u8) {
        match addr {
            a if a == JOYP_ADDR => {
                self.joypad.write(value);
                self.memory.write_io(JOYP_ADDR, self.joypad.read());
            }
            a if a == SC_ADDR => {
                let sb = self.memory.read_io(SB_ADDR);
                self.serial.handle_sc_write(value, sb);
            }
            a if a == DIV_ADDR => self.timer.reset_div(),
            a if a == LY_ADDR => self.ppu.reset_ly(),
            a if a == DMA_ADDR => self.memory.dma_to_oam(value),
            a if (NR10_ADDR..=NR52_ADDR).contains(&a) => self.write_apu_register(a, value),
            // Unused APU addresses 0xFF27-0xFF2F always read as 0xFF
            a if (0xFF27u16..WAVE_RAM_START).contains(&a) => self.memory.write_io(a, 0xFF),
            a if (WAVE_RAM_START..=WAVE_RAM_END).contains(&a) => self.write_wave_ram(a, value),
            _ => {}
        }
    }

    /// Write an APU register and sync the masked read-back value to IO memory.
    /// NR52 writes may power off all channels, so all registers are resynced.
    fn write_apu_register(&mut self, addr: u16, value: u8) {
        self.apu.write_register(addr, value);
        if addr == NR52_ADDR {
            for a in NR10_ADDR..=NR52_ADDR {
                self.memory.write_io(a, self.apu.read_register(a));
            }
        } else {
            self.memory.write_io(addr, self.apu.read_register(addr));
        }
    }

    /// Write to wave RAM through the APU and sync the result to IO memory.
    fn write_wave_ram(&mut self, addr: u16, value: u8) {
        let offset = (addr - WAVE_RAM_START) as u8;
        self.apu.write_wave_ram(offset, value);
        self.memory.write_io(addr, self.apu.read_wave_ram(offset));
    }

    fn advance_ppu(&mut self, cycles: u16) {
        let output = self.ppu.tick(
            cycles,
            PpuInput {
                lcdc: self.memory.read_io(LCDC_ADDR),
                stat: self.memory.read_io(STAT_ADDR),
                scy: self.memory.read_io(SCY_ADDR),
                scx: self.memory.read_io(SCX_ADDR),
                lyc: self.memory.read_io(LYC_ADDR),
                bgp: self.memory.read_io(BGP_ADDR),
                obp0: self.memory.read_io(OBP0_ADDR),
                obp1: self.memory.read_io(OBP1_ADDR),
                wy: self.memory.read_io(WY_ADDR),
                wx: self.memory.read_io(WX_ADDR),
                vram: self.memory.vram(),
                oam: self.memory.oam(),
            },
        );
        self.memory.write_io(LY_ADDR, output.ly);
        self.memory.write_io(STAT_ADDR, output.stat);
        if output.vblank_interrupt {
            let if_val = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_val | (1 << VBLANK_INTERRUPT_BIT));
        }
        if output.stat_interrupt {
            let if_val = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_val | (1 << STAT_INTERRUPT_BIT));
        }
    }

    fn advance_timer(&mut self, cycles: u16) {
        let output = self.timer.tick(
            cycles,
            TimerInput {
                tima: self.memory.read_io(TIMA_ADDR),
                tma: self.memory.read_io(TMA_ADDR),
                tac: self.memory.read_io(TAC_ADDR),
            },
        );
        self.memory.write_io(TIMA_ADDR, output.tima);
        self.memory.write_io(DIV_ADDR, output.div);
        if output.interrupt {
            let if_val = self.memory.read_io(IF_ADDR);
            self.memory.write_io(IF_ADDR, if_val | (1 << TIMER_INTERRUPT_BIT));
        }
    }

    fn advance_apu(&mut self, cycles: u16) {
        let output = self.apu.tick(cycles, self.timer.internal_counter());
        self.memory.write_io(NR52_ADDR, output.nr52);
    }

    fn has_pending_interrupt(&self) -> bool {
        let ie = self.memory.read_io(IE_ADDR);
        let if_val = self.memory.read_io(IF_ADDR);
        ie & if_val != 0
    }

    fn take_pending_interrupt(&mut self) -> Option<u8> {
        let ie = self.memory.read_io(IE_ADDR);
        let if_val = self.memory.read_io(IF_ADDR);
        let pending = ie & if_val;
        if pending == 0 {
            return None;
        }
        let bit = pending.trailing_zeros() as u8;
        self.memory.write_io(IF_ADDR, if_val & !(1 << bit));
        Some(bit)
    }

    fn dispatch_interrupt(&mut self, bit: u8) -> Result<(), InstructionError> {
        self.ime = ImeState::Disabled;
        // Interrupt dispatch: 2 internal + push PC (2 writes) + 1 internal = 5 M-cycles
        self.tick_cycle(); // internal
        self.tick_cycle(); // internal
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.bus_write(self.registers.sp, (self.registers.pc >> 8) as u8)?;
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.bus_write(self.registers.sp, self.registers.pc as u8)?;
        self.tick_cycle(); // internal — load ISR address
        self.registers.pc = 0x0040 + (bit as u16) * 8;
        Ok(())
    }

    fn read_next_pc(&mut self) -> Result<u8, MemoryError> {
        let byte = self.bus_read(self.registers.pc)?;
        self.registers.pc = self.registers.pc.wrapping_add(1);
        Ok(byte)
    }

    fn get_8bit_operand(&mut self, operand: Operand) -> Result<u8, InstructionError> {
        match operand {
            Operand::Register8(reg) => Ok(self.get_register8_operand(reg)),
            Operand::Memory(Memory::HL) => {
                let address = self.registers.hl();
                Ok(self.bus_read(address)?)
            }
            Operand::Imm8 => Ok(self.read_next_pc()?),
            _ => {
                return Err(InstructionError::InvalidOperand(format!(
                    "{} for instruction Add8",
                    operand
                )))
            }
        }
    }

    fn get_register8_operand(&self, operand: Register8) -> u8 {
        match operand {
            Register8::A => self.registers.a,
            Register8::B => self.registers.b,
            Register8::C => self.registers.c,
            Register8::D => self.registers.d,
            Register8::E => self.registers.e,
            Register8::H => self.registers.h,
            Register8::L => self.registers.l,
        }
    }

    fn get_register16_operand(&self, operand: Register16) -> u16 {
        match operand {
            Register16::AF => self.registers.af(),
            Register16::BC => self.registers.bc(),
            Register16::DE => self.registers.de(),
            Register16::HL => self.registers.hl(),
            Register16::SP => self.registers.sp,
        }
    }

    fn set_register8_operand(&mut self, operand: Register8, value: u8) {
        match operand {
            Register8::A => self.registers.a = value,
            Register8::B => self.registers.b = value,
            Register8::C => self.registers.c = value,
            Register8::D => self.registers.d = value,
            Register8::E => self.registers.e = value,
            Register8::H => self.registers.h = value,
            Register8::L => self.registers.l = value,
        }
    }

    fn set_register16_operand(&mut self, operand: Register16, value: u16) {
        match operand {
            Register16::AF => self.registers.set_af(value),
            Register16::BC => self.registers.set_bc(value),
            Register16::DE => self.registers.set_de(value),
            Register16::HL => self.registers.set_hl(value),
            Register16::SP => self.registers.sp = value,
        }
    }

    fn set_8bit_operand(&mut self, operand: Operand, value: u8) -> Result<(), InstructionError> {
        match operand {
            Operand::Register8(reg) => {
                self.set_register8_operand(reg, value);
                Ok(())
            }
            Operand::Memory(Memory::HL) => {
                let address = self.registers.hl();
                Ok(self.bus_write(address, value)?)
            }
            _ => Err(InstructionError::InvalidOperand(format!(
                "{} for write operand",
                operand
            ))),
        }
    }

    fn write_cb_target(&mut self, target: CbTarget, value: u8) -> Result<(), InstructionError> {
        match target {
            CbTarget::Reg(reg) => {
                self.set_register8_operand(reg, value);
                Ok(())
            }
            CbTarget::HLMem => {
                self.bus_write(self.registers.hl(), value)?;
                Ok(())
            }
        }
    }

    fn push_pc(&mut self) -> Result<(), MemoryError> {
        let pc = self.registers.pc;
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.bus_write(self.registers.sp, (pc >> 8) as u8)?;
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.bus_write(self.registers.sp, (pc & 0xFF) as u8)?;
        Ok(())
    }

    fn pop_pc(&mut self) -> Result<u16, MemoryError> {
        let lo = self.bus_read(self.registers.sp)? as u16;
        self.registers.sp = self.registers.sp.wrapping_add(1);
        let hi = self.bus_read(self.registers.sp)? as u16;
        self.registers.sp = self.registers.sp.wrapping_add(1);
        Ok((hi << 8) | lo)
    }

    fn check_condition(&self, cond: &Condition) -> bool {
        match cond {
            Condition::NZ => !self.registers.f.contains(Flags::Z),
            Condition::Z => self.registers.f.contains(Flags::Z),
            Condition::NC => !self.registers.f.contains(Flags::C),
            Condition::C => self.registers.f.contains(Flags::C),
        }
    }
}

impl Cpu for Sm83 {
    fn tick(&mut self) -> Result<u8, CpuError> {
        let start_cycles = self.cycle_counter;

        if self.halted {
            self.tick_cycle(); // 1 M-cycle while halted
            if self.has_pending_interrupt() {
                self.halted = false;
            } else {
                return Ok((self.cycle_counter - start_cycles) as u8);
            }
        }

        self.advance_ime();

        // Opcode fetch is the first M-cycle (via bus_read inside read_next_pc)
        let opcode = self.read_next_pc()?;
        if opcode == 0xCB {
            let cb_opcode = self.read_next_pc()?;
            CbDecoder.decode(cb_opcode)?.execute(self)?;
        } else {
            self.opcodes.decode(opcode)?.execute(self)?;
        };

        // Peripherals and bus events are already advanced per M-cycle
        // inside bus_read/bus_write/tick_cycle — no bulk advance needed.

        if self.ime == ImeState::Enabled {
            if let Some(bit) = self.take_pending_interrupt() {
                self.dispatch_interrupt(bit)?;
            }
        }

        Ok((self.cycle_counter - start_cycles) as u8)
    }
}

impl Instructions for Sm83 {
    fn add8(&mut self, opcode: &Add8) -> Result<u8, InstructionError> {
        (self.registers.a, self.registers.f) =
            add_u8(self.registers.a, self.get_8bit_operand(opcode.operand)?);
        Ok(opcode.cycles)
    }

    fn add16(&mut self, opcode: &Add16) -> Result<u8, InstructionError> {
        let operand: u16 = match opcode.operand {
            Operand::Register16(reg) => self.get_register16_operand(reg),
            _ => {
                return Err(InstructionError::InvalidOperand(format!(
                    "{} for instruction Add16",
                    opcode.operand
                )))
            }
        };

        let (hl, new_flags) = add_u16(self.registers.hl(), operand);
        // ADD HL,rr preserves Z — merge new H/C/N flags into existing flags
        self.registers.f.set(Flags::N, false);
        self.registers.f.set(Flags::H, new_flags.contains(Flags::H));
        self.registers.f.set(Flags::C, new_flags.contains(Flags::C));
        self.registers.set_hl(hl);
        self.tick_cycle(); // internal — 16-bit ALU
        Ok(opcode.cycles)
    }

    fn add_sp16(&mut self, opcode: &AddSP16) -> Result<u8, InstructionError> {
        if opcode.operand != Operand::ImmSigned8 {
            return Err(InstructionError::InvalidOperand(format!(
                "{} for instruction AddSP16",
                opcode.operand
            )));
        }

        // fetch + read e8 + internal + internal = 4 M-cycles
        let offset = self.read_next_pc()? as i8;
        (self.registers.sp, self.registers.f) = add_sp_u16(self.registers.sp, offset);
        self.tick_cycle(); // internal
        self.tick_cycle(); // internal

        Ok(opcode.cycles)
    }

    fn adc(&mut self, opcode: &Adc) -> Result<u8, InstructionError> {
        let carry: u8 = self.registers.f.contains(Flags::C) as u8;
        (self.registers.a, self.registers.f) = adc_u8(
            self.registers.a,
            self.get_8bit_operand(opcode.operand)?,
            carry,
        );
        Ok(opcode.cycles)
    }

    fn sub8(&mut self, opcode: &Sub8) -> Result<u8, InstructionError> {
        (self.registers.a, self.registers.f) =
            sub_u8(self.registers.a, self.get_8bit_operand(opcode.operand)?);
        Ok(opcode.cycles)
    }

    fn sbc8(&mut self, opcode: &Sbc8) -> Result<u8, InstructionError> {
        let carry: u8 = self.registers.f.contains(Flags::C) as u8;
        (self.registers.a, self.registers.f) = sbc_u8(
            self.registers.a,
            self.get_8bit_operand(opcode.operand)?,
            carry,
        );
        Ok(opcode.cycles)
    }

    fn cp8(&mut self, opcode: &Cp8) -> Result<u8, InstructionError> {
        self.registers.f = cp_u8(self.registers.a, self.get_8bit_operand(opcode.operand)?);
        Ok(opcode.cycles)
    }

    fn ld8(&mut self, opcode: &Ld8) -> Result<u8, InstructionError> {
        // Read the source value
        let value = match opcode.src {
            Operand::Register8(reg) => self.get_register8_operand(reg),
            Operand::Memory(Memory::HL) => {
                let address = self.registers.hl();
                self.bus_read(address)?
            }
            Operand::Imm8 => self.read_next_pc()?,
            _ => {
                return Err(InstructionError::InvalidOperand(format!(
                    "{} for instruction Ld8 src",
                    opcode.src
                )))
            }
        };

        // Write to the destination
        match opcode.dest {
            Operand::Register8(reg) => self.set_register8_operand(reg, value),
            Operand::Memory(Memory::HL) => {
                let address = self.registers.hl();
                self.bus_write(address, value)?;
            }
            _ => {
                return Err(InstructionError::InvalidOperand(format!(
                    "{} for instruction Ld8 dest",
                    opcode.dest
                )))
            }
        }

        Ok(opcode.cycles)
    }

    fn inc8(&mut self, opcode: &Inc8) -> Result<u8, InstructionError> {
        let val = self.get_8bit_operand(opcode.operand)?;
        let (result, flags) = inc_u8(val, self.registers.f);
        self.registers.f = flags;
        self.set_8bit_operand(opcode.operand, result)?;
        Ok(opcode.cycles)
    }

    fn dec8(&mut self, opcode: &Dec8) -> Result<u8, InstructionError> {
        let val = self.get_8bit_operand(opcode.operand)?;
        let (result, flags) = dec_u8(val, self.registers.f);
        self.registers.f = flags;
        self.set_8bit_operand(opcode.operand, result)?;
        Ok(opcode.cycles)
    }

    fn inc16(&mut self, opcode: &Inc16) -> Result<u8, InstructionError> {
        let val = self.get_register16_operand(opcode.operand);
        self.set_register16_operand(opcode.operand, val.wrapping_add(1));
        self.tick_cycle(); // internal
        Ok(opcode.cycles)
    }

    fn dec16(&mut self, opcode: &Dec16) -> Result<u8, InstructionError> {
        let val = self.get_register16_operand(opcode.operand);
        self.set_register16_operand(opcode.operand, val.wrapping_sub(1));
        self.tick_cycle(); // internal
        Ok(opcode.cycles)
    }

    fn rotate_accumulator(&mut self, opcode: &Rotate) -> Result<u8, InstructionError> {
        let a = self.registers.a;
        let carry_in = self.registers.f.contains(Flags::C) as u8;

        let (result, carry_out) = match opcode.op {
            RotateOp::Rlca => {
                let bit7 = a >> 7;
                ((a << 1) | bit7, bit7 != 0)
            }
            RotateOp::Rla => {
                let bit7 = a >> 7;
                ((a << 1) | carry_in, bit7 != 0)
            }
            RotateOp::Rrca => {
                let bit0 = a & 1;
                ((a >> 1) | (bit0 << 7), bit0 != 0)
            }
            RotateOp::Rra => {
                let bit0 = a & 1;
                ((a >> 1) | (carry_in << 7), bit0 != 0)
            }
        };

        self.registers.a = result;
        // Z=0, N=0, H=0 always; only C is affected
        self.registers.f = Flags::empty();
        self.registers.f.set(Flags::C, carry_out);

        Ok(opcode.cycles)
    }

    fn ld16(&mut self, opcode: &Ld16) -> Result<u8, InstructionError> {
        use super::instructions::ld16::opcode::Ld16Op;
        match &opcode.op {
            Ld16Op::RrImm16 { dest } => {
                // fetch + read lo + read hi = 3 M-cycles
                let lo = self.read_next_pc()? as u16;
                let hi = self.read_next_pc()? as u16;
                let val = (hi << 8) | lo;
                self.set_register16_operand(*dest, val);
            }
            Ld16Op::NnSp => {
                // fetch + read lo + read hi + write lo + write hi = 5 M-cycles
                let lo = self.read_next_pc()? as u16;
                let hi = self.read_next_pc()? as u16;
                let addr = (hi << 8) | lo;
                let sp = self.registers.sp;
                self.bus_write(addr, (sp & 0xFF) as u8)?;
                self.bus_write(addr.wrapping_add(1), (sp >> 8) as u8)?;
            }
            Ld16Op::SpHl => {
                // fetch + internal = 2 M-cycles
                self.registers.sp = self.registers.hl();
                self.tick_cycle(); // internal
            }
            Ld16Op::HlSpE => {
                // fetch + read e8 + internal = 3 M-cycles
                let offset = self.read_next_pc()? as i8;
                let (result, flags) = add_sp_u16(self.registers.sp, offset);
                self.registers.set_hl(result);
                self.registers.f = flags;
                self.tick_cycle(); // internal
            }
            Ld16Op::BcA => {
                let addr = self.registers.bc();
                self.bus_write(addr, self.registers.a)?;
            }
            Ld16Op::DeA => {
                let addr = self.registers.de();
                self.bus_write(addr, self.registers.a)?;
            }
            Ld16Op::ABc => {
                let addr = self.registers.bc();
                self.registers.a = self.bus_read(addr)?;
            }
            Ld16Op::ADe => {
                let addr = self.registers.de();
                self.registers.a = self.bus_read(addr)?;
            }
            Ld16Op::HliA => {
                let addr = self.registers.hl();
                self.bus_write(addr, self.registers.a)?;
                self.registers.set_hl(addr.wrapping_add(1));
            }
            Ld16Op::HldA => {
                let addr = self.registers.hl();
                self.bus_write(addr, self.registers.a)?;
                self.registers.set_hl(addr.wrapping_sub(1));
            }
            Ld16Op::AHli => {
                let addr = self.registers.hl();
                self.registers.a = self.bus_read(addr)?;
                self.registers.set_hl(addr.wrapping_add(1));
            }
            Ld16Op::AHld => {
                let addr = self.registers.hl();
                self.registers.a = self.bus_read(addr)?;
                self.registers.set_hl(addr.wrapping_sub(1));
            }
            Ld16Op::NnA => {
                let lo = self.read_next_pc()? as u16;
                let hi = self.read_next_pc()? as u16;
                let addr = (hi << 8) | lo;
                self.bus_write(addr, self.registers.a)?;
            }
            Ld16Op::ANn => {
                let lo = self.read_next_pc()? as u16;
                let hi = self.read_next_pc()? as u16;
                let addr = (hi << 8) | lo;
                self.registers.a = self.bus_read(addr)?;
            }
            Ld16Op::LdhNA => {
                let offset = self.read_next_pc()? as u16;
                let addr = 0xFF00 | offset;
                self.bus_write(addr, self.registers.a)?;
            }
            Ld16Op::LdhAN => {
                let offset = self.read_next_pc()? as u16;
                let addr = 0xFF00 | offset;
                self.registers.a = self.bus_read(addr)?;
            }
            Ld16Op::LdCA => {
                let addr = 0xFF00 | (self.registers.c as u16);
                self.bus_write(addr, self.registers.a)?;
            }
            Ld16Op::LdAC => {
                let addr = 0xFF00 | (self.registers.c as u16);
                self.registers.a = self.bus_read(addr)?;
            }
        }
        Ok(opcode.cycles)
    }

    fn jump(&mut self, opcode: &Jump) -> Result<u8, InstructionError> {
        match &opcode.op {
            JumpOp::Jp => {
                // fetch + read lo + read hi + internal = 4 M-cycles
                let lo = self.read_next_pc()? as u16;
                let hi = self.read_next_pc()? as u16;
                self.registers.pc = (hi << 8) | lo;
                self.tick_cycle(); // internal — load new PC
                Ok(opcode.cycles)
            }
            JumpOp::JpHl => {
                // fetch only = 1 M-cycle
                self.registers.pc = self.registers.hl();
                Ok(opcode.cycles)
            }
            JumpOp::JpCc(cond) => {
                // fetch + read lo + read hi [+ internal if taken] = 3 or 4 M-cycles
                let lo = self.read_next_pc()? as u16;
                let hi = self.read_next_pc()? as u16;
                let target = (hi << 8) | lo;
                if self.check_condition(cond) {
                    self.registers.pc = target;
                    self.tick_cycle(); // internal — branch taken
                    Ok(opcode.cycles)
                } else {
                    Ok(12)
                }
            }
            JumpOp::Jr => {
                // fetch + read e8 + internal = 3 M-cycles
                let offset = self.read_next_pc()? as i8 as i16;
                self.registers.pc = self.registers.pc.wrapping_add(offset as u16);
                self.tick_cycle(); // internal — apply offset
                Ok(opcode.cycles)
            }
            JumpOp::JrCc(cond) => {
                // fetch + read e8 [+ internal if taken] = 2 or 3 M-cycles
                let offset = self.read_next_pc()? as i8 as i16;
                if self.check_condition(cond) {
                    self.registers.pc = self.registers.pc.wrapping_add(offset as u16);
                    self.tick_cycle(); // internal — branch taken
                    Ok(opcode.cycles)
                } else {
                    Ok(8)
                }
            }
        }
    }

    fn and8(&mut self, opcode: &And8) -> Result<u8, InstructionError> {
        let val = self.get_8bit_operand(opcode.operand)?;
        (self.registers.a, self.registers.f) = and_u8(self.registers.a, val);
        Ok(opcode.cycles)
    }

    fn or8(&mut self, opcode: &Or8) -> Result<u8, InstructionError> {
        let val = self.get_8bit_operand(opcode.operand)?;
        (self.registers.a, self.registers.f) = or_u8(self.registers.a, val);
        Ok(opcode.cycles)
    }

    fn xor8(&mut self, opcode: &Xor8) -> Result<u8, InstructionError> {
        let val = self.get_8bit_operand(opcode.operand)?;
        (self.registers.a, self.registers.f) = xor_u8(self.registers.a, val);
        Ok(opcode.cycles)
    }

    fn misc(&mut self, opcode: &Misc) -> Result<u8, InstructionError> {
        use super::instructions::misc::opcode::MiscOp;
        match opcode.op {
            MiscOp::Nop => {
                // No operation
            }
            MiscOp::Halt => {
                self.halted = true;
            }
            MiscOp::Stop => {
                // Consume the next byte (should be 0x00)
                let _ = self.read_next_pc()?;
            }
            MiscOp::Daa => {
                (self.registers.a, self.registers.f) = daa_u8(self.registers.a, self.registers.f);
            }
            MiscOp::Cpl => {
                // Complement A: A = ~A, N=1, H=1, Z and C unchanged
                self.registers.a = !self.registers.a;
                self.registers.f.insert(Flags::N);
                self.registers.f.insert(Flags::H);
            }
            MiscOp::Scf => {
                // Set carry flag: N=0, H=0, C=1, Z unchanged
                self.registers.f.remove(Flags::N);
                self.registers.f.remove(Flags::H);
                self.registers.f.insert(Flags::C);
            }
            MiscOp::Ccf => {
                // Complement carry flag: N=0, H=0, C=!C, Z unchanged
                let c = self.registers.f.contains(Flags::C);
                self.registers.f.remove(Flags::N);
                self.registers.f.remove(Flags::H);
                self.registers.f.set(Flags::C, !c);
            }
            MiscOp::Di => {
                self.ime = ImeState::Disabled;
            }
            MiscOp::Ei => {
                self.ime = ImeState::Pending;
            }
        }
        Ok(opcode.cycles)
    }

    fn push16(&mut self, opcode: &Push16) -> Result<u8, InstructionError> {
        // fetch + internal + write hi + write lo = 4 M-cycles
        let value = self.get_register16_operand(opcode.operand);
        self.tick_cycle(); // internal
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.bus_write(self.registers.sp, (value >> 8) as u8)?;
        self.registers.sp = self.registers.sp.wrapping_sub(1);
        self.bus_write(self.registers.sp, (value & 0xFF) as u8)?;
        Ok(opcode.cycles)
    }

    fn pop16(&mut self, opcode: &Pop16) -> Result<u8, InstructionError> {
        // fetch + read lo + read hi = 3 M-cycles
        let lo = self.bus_read(self.registers.sp)? as u16;
        self.registers.sp = self.registers.sp.wrapping_add(1);
        let hi = self.bus_read(self.registers.sp)? as u16;
        self.registers.sp = self.registers.sp.wrapping_add(1);
        self.set_register16_operand(opcode.operand, (hi << 8) | lo);
        Ok(opcode.cycles)
    }

    fn call(&mut self, opcode: &Call) -> Result<u8, InstructionError> {
        // fetch + read lo + read hi [+ internal + push hi + push lo] = 3 or 6 M-cycles
        let lo = self.read_next_pc()? as u16;
        let hi = self.read_next_pc()? as u16;
        let target = (hi << 8) | lo;
        match &opcode.op {
            CallOp::Call => {
                self.tick_cycle(); // internal
                self.push_pc()?;  // 2 bus writes
                self.registers.pc = target;
                Ok(opcode.cycles)
            }
            CallOp::CallCc(cond) => {
                if self.check_condition(cond) {
                    self.tick_cycle(); // internal
                    self.push_pc()?;  // 2 bus writes
                    self.registers.pc = target;
                    Ok(opcode.cycles)
                } else {
                    Ok(12)
                }
            }
        }
    }

    fn ret(&mut self, opcode: &Ret) -> Result<u8, InstructionError> {
        match &opcode.op {
            RetOp::Ret => {
                // fetch + pop lo + pop hi + internal = 4 M-cycles
                self.registers.pc = self.pop_pc()?;
                self.tick_cycle(); // internal
                Ok(opcode.cycles)
            }
            RetOp::RetCc(cond) => {
                // fetch + internal [+ pop lo + pop hi + internal] = 2 or 5 M-cycles
                self.tick_cycle(); // internal — condition eval
                if self.check_condition(cond) {
                    self.registers.pc = self.pop_pc()?;
                    self.tick_cycle(); // internal
                    Ok(opcode.cycles)
                } else {
                    Ok(8)
                }
            }
            RetOp::Reti => {
                // fetch + pop lo + pop hi + internal = 4 M-cycles
                self.registers.pc = self.pop_pc()?;
                self.tick_cycle(); // internal
                self.ime = ImeState::Enabled;
                Ok(opcode.cycles)
            }
        }
    }

    fn rst(&mut self, opcode: &Rst) -> Result<u8, InstructionError> {
        // fetch + internal + push hi + push lo = 4 M-cycles
        self.tick_cycle(); // internal
        self.push_pc()?;  // 2 bus writes
        self.registers.pc = opcode.vector as u16;
        Ok(opcode.cycles)
    }

    fn cb(&mut self, opcode: &CbInstruction) -> Result<u8, InstructionError> {
        let carry_in = self.registers.f.contains(Flags::C);

        let val = match opcode.target {
            CbTarget::Reg(reg) => self.get_register8_operand(reg),
            CbTarget::HLMem => self.bus_read(self.registers.hl())?,
        };

        match opcode.op {
            CbOp::Bit(bit) => {
                self.registers.f = bit_u8(val, bit, self.registers.f);
            }
            CbOp::Res(bit) => {
                let result = res_u8(val, bit);
                self.write_cb_target(opcode.target, result)?;
            }
            CbOp::Set(bit) => {
                let result = set_u8(val, bit);
                self.write_cb_target(opcode.target, result)?;
            }
            _ => {
                let (result, flags) = match opcode.op {
                    CbOp::Rlc => rlc_u8(val),
                    CbOp::Rrc => rrc_u8(val),
                    CbOp::Rl => rl_u8(val, carry_in),
                    CbOp::Rr => rr_u8(val, carry_in),
                    CbOp::Sla => sla_u8(val),
                    CbOp::Sra => sra_u8(val),
                    CbOp::Swap => swap_u8(val),
                    CbOp::Srl => srl_u8(val),
                    _ => unreachable!(),
                };
                self.registers.f = flags;
                self.write_cb_target(opcode.target, result)?;
            }
        }

        Ok(opcode.cycles)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{boxed::Box, vec, vec::Vec};
    use crate::cpu::instructions::opcodes::OpCodeDecoder;
    use crate::cpu::registers::Flags;

    use crate::memory::memory::GameBoyMemory;

    pub fn make_test_cpu(rom_data: Vec<u8>) -> Sm83 {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::with_rom(rom_data));
        let decoder = Box::new(OpCodeDecoder::new());

        Sm83::new(memory, decoder)
    }

    pub fn make_test_cpu_with_memory(
        setup: impl FnOnce(&mut GameBoyMemory),
        rom_data: Vec<u8>,
    ) -> Sm83 {
        let mut mem = GameBoyMemory::with_rom(rom_data);
        setup(&mut mem);
        let decoder = Box::new(OpCodeDecoder::new());
        Sm83::new(Box::new(mem), decoder)
    }

    /// Add a constant to the accumulator register and expect the register's value to be the
    /// appropriate value.
    #[test]
    fn test_add8_imm8_to_accumlator() {
        let mut cpu = make_test_cpu(vec![0xC6, 0x03]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x03);
        assert_eq!(cpu.registers().f, Flags::empty())
    }

    #[test]
    fn test_add8_imm8_to_accumlator_sum_zero() {
        let mut cpu = make_test_cpu(vec![0xC6, 0x00]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z)
    }

    // Set the expected value for register b and confirm the add operation takes place as expected.
    #[test]
    fn test_add8_regb_to_accumulator() {
        let mut cpu = make_test_cpu(vec![0x80]).with_registers(Registers {
            b: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x05);
    }

    /// ADD A, (HL) — opcode 0x86 — reads from memory at address pointed to by HL.
    /// HL=0xC000, memory[0xC000]=0x07, A=0x03 → A should become 0x0A.
    #[test]
    fn test_add8_memory_hl_to_accumulator() {
        let mut cpu = make_test_cpu_with_memory(
            |m| { m.write(0xC000, 0x07).unwrap(); },
            vec![0x86],
        ).with_registers(Registers {
            a: 0x03,
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0A);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_add8_invalid_opcode() {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::new());
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(memory, decoder));
        assert!(cpu
            .add8(&Add8 {
                operand: Operand::Imm16,
                cycles: 4
            })
            .is_err());
    }

    // Load up all 8-bit registers with some test values, add them all to the accumulator register, the add the accumulator
    // register to itself, then confirm that it has the expected value.
    #[test]
    fn test_add8_all_reg8s_to_accumulator() {
        let registers = Registers {
            b: 0x01,
            c: 0x02,
            d: 0x03,
            e: 0x04,
            h: 0x05,
            l: 0x06,
            ..Default::default()
        };
        let rom_data = vec![0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x87];
        let num_instructions = rom_data.len();
        let mut cpu = make_test_cpu(rom_data).with_registers(registers.clone());

        let total_cycles: u8 = (0..num_instructions).map(|_| cpu.tick().unwrap()).sum();

        let mut expected_accumlator_value =
            registers.b + registers.c + registers.d + registers.e + registers.h + registers.l;
        expected_accumlator_value += expected_accumlator_value;

        assert_eq!(total_cycles, num_instructions as u8 * 4);
        assert_eq!(cpu.registers().a, expected_accumlator_value);
    }

    #[test]
    fn test_add8_rollover_flags() {
        let mut cpu = make_test_cpu(vec![0xC6, 0xFF]).with_registers(Registers {
            a: 0x01,
            ..Default::default()
        }); // Add 0xFF to accumulator
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00); // Accumulator should have rolled over to 0.
        assert_eq!(cpu.registers().f, Flags::Z | Flags::C | Flags::H); // Flags should indicate zero, carry, and half-carry
    }

    #[test]
    fn test_add16_bc_to_hl() {
        let mut registers = Registers::default();
        registers.set_bc(0xbeef);
        let mut cpu = make_test_cpu(vec![0x09]).with_registers(registers);

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().hl(), 0xbeef); // Expected value after adding BC to HL
    }

    #[test]
    fn test_add16_de_to_hl() {
        let mut registers = Registers::default();
        registers.set_de(0xbeef);
        let mut cpu = make_test_cpu(vec![0x19]).with_registers(registers);

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().hl(), 0xbeef); // Expected value after adding DE to HL
    }

    #[test]
    fn test_add16_hl_to_hl() {
        let mut registers = Registers::default();
        registers.set_hl(0xffff);
        let mut cpu = make_test_cpu(vec![0x29]).with_registers(registers);

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().hl(), 0xfffe); // Expected value after adding HL to HL
        assert_eq!(cpu.registers().f, Flags::H | Flags::C);
    }

    #[test]
    fn test_add16_sp_to_hl() {
        let mut registers = Registers::default();
        registers.sp = 0xffff;
        let mut cpu = make_test_cpu(vec![0x39]).with_registers(registers);

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().hl(), 0xffff); // Expected value after adding SP to HL
    }

    #[test]
    fn test_add16_invalid_opcode() {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::new());
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(memory, decoder));
        assert!(cpu
            .add16(&Add16 {
                operand: Operand::Imm8,
                cycles: 4
            })
            .is_err());
    }

    // TODO: This is NOT a useful test. Will have to revisit later.
    #[test]
    fn test_add_sp16_imm8() {
        let mut cpu = make_test_cpu(vec![0xE8, 0x05]);

        assert_eq!(cpu.tick().unwrap(), 16);
        assert_eq!(cpu.registers().sp, 0x0005); // Expected value after adding signed immediate 8-bit value to SP
    }

    #[test]
    fn test_add_sp16_invalid_opcode() {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::new());
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(memory, decoder));
        assert!(cpu
            .add_sp16(&AddSP16 {
                operand: Operand::Imm8,
                cycles: 4
            })
            .is_err());
    }

    #[test]
    fn test_adc_b() {
        let mut cpu = make_test_cpu(vec![0x88]).with_registers(Registers {
            a: 0x05,
            b: 0x03,
            ..Default::default()
        });

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding B to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_c() {
        let mut cpu = make_test_cpu(vec![0x89]).with_registers(Registers {
            a: 0x05,
            c: 0x03,
            ..Default::default()
        });

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding C to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_d() {
        let mut cpu = make_test_cpu(vec![0x8A]).with_registers(Registers {
            a: 0x05,
            d: 0x03,
            ..Default::default()
        });

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding D to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_e() {
        let mut cpu = make_test_cpu(vec![0x8B]).with_registers(Registers {
            a: 0x05,
            e: 0x03,
            ..Default::default()
        });

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding E to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_h() {
        let mut cpu = make_test_cpu(vec![0x8C]).with_registers(Registers {
            a: 0x05,
            h: 0x03,
            ..Default::default()
        });

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding H to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_l() {
        let mut cpu = make_test_cpu(vec![0x8D]).with_registers(Registers {
            a: 0x05,
            l: 0x03,
            ..Default::default()
        });

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding L to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_a() {
        let mut cpu = make_test_cpu(vec![0x8F]).with_registers(Registers {
            a: 0x05,
            ..Default::default()
        });

        assert_eq!(cpu.tick().unwrap(), 4);
        assert_eq!(cpu.registers().a, 0x0A); // Expected value after adding A to A
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    /// ADC A, (HL) — opcode 0x8E — reads from memory at address in HL, adds with carry.
    /// HL=0xC001, memory[0xC001]=0x04, A=0x05, carry=0 → A should become 0x09.
    #[test]
    fn test_adc_memhl() {
        let mut cpu = make_test_cpu_with_memory(
            |m| { m.write(0xC001, 0x04).unwrap(); },
            vec![0x8E],
        ).with_registers(Registers {
            a: 0x05,
            h: 0xC0,
            l: 0x01,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x09);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_adc_imm8() {
        let mut cpu = make_test_cpu(vec![0xCE, 0x03]).with_registers(Registers {
            a: 0x05,
            ..Default::default()
        });

        assert_eq!(cpu.tick().unwrap(), 8);
        assert_eq!(cpu.registers().a, 0x08); // Expected value after adding immediate 8-bit value to A
    }

    #[test]
    fn test_adc_invalid_operand() {
        let memory: Box<GameBoyMemory> = Box::new(GameBoyMemory::new());
        let decoder = Box::new(OpCodeDecoder::new());

        let mut cpu: Box<dyn Instructions> = Box::new(Sm83::new(memory, decoder));
        assert!(cpu
            .adc(&Adc {
                operand: Operand::Register16(Register16::BC),
                cycles: 4
            })
            .is_err());
    }

    #[test]
    fn test_sub8_imm8() {
        let mut cpu = make_test_cpu(vec![0xD6, 0x03]).with_registers(Registers {
            a: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x02);
        assert_eq!(cpu.registers().f, Flags::N);
    }

    #[test]
    fn test_sub8_zero_result() {
        let mut cpu = make_test_cpu(vec![0xD6, 0x05]).with_registers(Registers {
            a: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::N);
    }

    #[test]
    fn test_sub8_regb() {
        let mut cpu = make_test_cpu(vec![0x90]).with_registers(Registers {
            a: 0x10,
            b: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x0B);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H); // H flag should be set for 0x10 - 0x05
    }

    #[test]
    fn test_sub8_borrow() {
        let mut cpu = make_test_cpu(vec![0xD6, 0x10]).with_registers(Registers {
            a: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0xF5);
        assert_eq!(cpu.registers().f, Flags::N | Flags::C);
    }

    #[test]
    fn test_sub8_half_borrow() {
        let mut cpu = make_test_cpu(vec![0xD6, 0x01]).with_registers(Registers {
            a: 0x10,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0F);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H);
    }

    #[test]
    fn test_sbc8_no_carry() {
        let mut cpu = make_test_cpu(vec![0xDE, 0x03]).with_registers(Registers {
            a: 0x10,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0D);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H); // Half-borrow: 0x0 < 0x3
    }

    #[test]
    fn test_sbc8_with_carry() {
        let mut cpu = make_test_cpu(vec![0xDE, 0x03]).with_registers(Registers {
            a: 0x10,
            f: Flags::C,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0C);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H); // Half-borrow: 0x0 < 0x3 + 1
    }

    #[test]
    fn test_sbc8_zero_result() {
        let mut cpu = make_test_cpu(vec![0xDE, 0x04]).with_registers(Registers {
            a: 0x05,
            f: Flags::C,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::N);
    }

    #[test]
    fn test_sbc8_regb() {
        let mut cpu = make_test_cpu(vec![0x98]).with_registers(Registers {
            a: 0x10,
            b: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x0B);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H);
    }

    #[test]
    fn test_cp8_imm8() {
        let mut cpu = make_test_cpu(vec![0xFE, 0x05]).with_registers(Registers {
            a: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x05); // A register unchanged
        assert_eq!(cpu.registers().f, Flags::Z | Flags::N);
    }

    #[test]
    fn test_cp8_regb() {
        let mut cpu = make_test_cpu(vec![0xB8]).with_registers(Registers {
            a: 0x05,
            b: 0x10,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x05); // A register unchanged
        assert_eq!(cpu.registers().f, Flags::N | Flags::C);
    }

    // --- LD 8-bit integration tests ---

    /// LD B, C — register to register: copies C into B.
    /// C=0x42, opcode 0x41 (LD B,C), expect B=0x42, 4 cycles.
    #[test]
    fn test_ld8_reg_to_reg() {
        let mut cpu = make_test_cpu(vec![0x41]).with_registers(Registers {
            c: 0x42,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().b, 0x42);
    }

    /// LD A, (HL) — load from memory at HL into A.
    /// HL=0xC000, memory[0xC000]=0x55, opcode 0x7E, expect A=0x55, 8 cycles.
    #[test]
    fn test_ld8_a_from_mem_hl() {
        let mut cpu = make_test_cpu_with_memory(
            |m| { m.write(0xC000, 0x55).unwrap(); },
            vec![0x7E],
        ).with_registers(Registers {
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x55);
    }

    /// LD (HL), A — store A into memory at HL, then LD A, (HL) to verify write.
    /// ROM: [0x77 (LD (HL),A), 0x7E (LD A,(HL))].
    /// A=0xCD, HL=0xC000 initially. After store A=0 is zeroed then restored via the read-back.
    #[test]
    fn test_ld8_mem_hl_from_a_verify_memory() {
        // ROM: store A to (HL), then load A from (HL); HL=0xC000, A=0xCD
        // After tick 1 (LD (HL),A): memory[0xC000]=0xCD, cycles=8
        // After tick 2 (LD A,(HL)): A=0xCD, cycles=8
        let memory = GameBoyMemory::with_rom(vec![0x77, 0x7E]);
        let decoder = Box::new(OpCodeDecoder::new());
        let mut cpu = Sm83::new(Box::new(memory), decoder).with_registers(Registers {
            a: 0xCD,
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });

        // Store A into (HL)
        let cycles1 = cpu.tick().unwrap();
        assert_eq!(cycles1, 8);

        // Zero out A (keep HL pointing to 0xC000) so we know the next tick loads from memory
        let regs_after_store = cpu.registers();
        cpu = cpu.with_registers(Registers {
            a: 0x00,
            ..regs_after_store
        });

        // Load A from (HL) — should restore 0xCD from memory
        let cycles2 = cpu.tick().unwrap();
        assert_eq!(cycles2, 8);
        assert_eq!(cpu.registers().a, 0xCD);
    }

    /// LD B, n — load immediate byte into B.
    /// ROM: [0x06, 0x7F], expect B=0x7F, 8 cycles.
    #[test]
    fn test_ld8_b_imm8() {
        let mut cpu = make_test_cpu(vec![0x06, 0x7F]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().b, 0x7F);
    }

    /// LD (HL), n — store immediate byte into memory at HL, then LD A,(HL) to verify.
    /// ROM: [0x36, 0x99, 0x7E], HL=0xC000.
    /// Tick 1: store 0x99 to (HL), 12 cycles.
    /// Tick 2: load A from (HL), expect A=0x99, 8 cycles.
    #[test]
    fn test_ld8_mem_hl_imm8() {
        let memory = GameBoyMemory::with_rom(vec![0x36, 0x99, 0x7E]);
        let decoder = Box::new(OpCodeDecoder::new());
        let mut cpu = Sm83::new(Box::new(memory), decoder).with_registers(Registers {
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });

        // LD (HL), 0x99
        let cycles1 = cpu.tick().unwrap();
        assert_eq!(cycles1, 12);

        // LD A, (HL) — verify that 0x99 was stored
        let regs = cpu.registers();
        cpu = cpu.with_registers(Registers { a: 0x00, ..regs });
        let cycles2 = cpu.tick().unwrap();
        assert_eq!(cycles2, 8);
        assert_eq!(cpu.registers().a, 0x99);
    }

    /// Integration test: ADD A, (HL) via GameBoyMemory.
    /// ROM: [0x86] at address 0x0000.
    /// Memory at 0xC010 = 0x0F. A = 0x01, HL = 0xC010.
    /// Expected: A = 0x10, H flag set (lower nibble overflow 1+F=10).
    #[test]
    fn test_integration_add8_memory_hl_gameboy_memory() {
        let mut memory = GameBoyMemory::with_rom(vec![0x86]);
        // Pre-populate the memory location HL will point to
        memory.write(0xC010, 0x0F).unwrap();

        let decoder = Box::new(OpCodeDecoder::new());
        let mut cpu = Sm83::new(Box::new(memory), decoder).with_registers(Registers {
            a: 0x01,
            h: 0xC0,
            l: 0x10,
            ..Default::default()
        });

        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x10);
        assert_eq!(cpu.registers().f, Flags::H); // half-carry: low nibble 1 + F = 10
    }

    // --- Rotate Accumulator integration tests ---

    /// RLCA (0x07) with bit 7 set: carry should be set, bit 0 of result should be set, Z=0
    #[test]
    fn test_rlca_bit7_set() {
        // A = 0x80 (1000_0000): bit7=1, rotate left → result=0x01, C=1
        let mut cpu = make_test_cpu(vec![0x07]).with_registers(Registers {
            a: 0x80,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x01); // bit7 wraps to bit0
        assert_eq!(cpu.registers().f, Flags::C); // carry set, Z=0
    }

    /// RLCA (0x07) with bit 7 clear: carry should be clear, Z=0
    #[test]
    fn test_rlca_bit7_clear() {
        // A = 0x01 (0000_0001): bit7=0, rotate left → result=0x02, C=0
        let mut cpu = make_test_cpu(vec![0x07]).with_registers(Registers {
            a: 0x01,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x02);
        assert_eq!(cpu.registers().f, Flags::empty()); // C=0, Z=0
    }

    /// RLCA with result 0x00: Z must still be 0 (not set)
    #[test]
    fn test_rlca_z_always_zero() {
        // A = 0x00: result = 0x00, but Z must NOT be set
        let mut cpu = make_test_cpu(vec![0x07]).with_registers(Registers {
            a: 0x00,
            ..Default::default()
        });
        cpu.tick().unwrap();

        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::empty()); // Z=0 even when result is 0
    }

    /// RLA (0x17) with carry set: old carry should go to bit 0
    #[test]
    fn test_rla_carry_goes_to_bit0() {
        // A = 0x00, carry=1: result = (0x00 << 1) | 1 = 0x01, C=0
        let mut cpu = make_test_cpu(vec![0x17]).with_registers(Registers {
            a: 0x00,
            f: Flags::C,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x01); // old carry goes to bit0
        assert_eq!(cpu.registers().f, Flags::empty()); // C=0, Z=0
    }

    /// RLA (0x17): bit7 of A goes to carry
    #[test]
    fn test_rla_bit7_goes_to_carry() {
        // A = 0x80, carry=0: result = 0x00, C=1
        let mut cpu = make_test_cpu(vec![0x17]).with_registers(Registers {
            a: 0x80,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::C); // bit7 went to carry; Z still 0
    }

    /// RRCA (0x0F) with bit 0 set: carry should be set, bit 7 of result should be set, Z=0
    #[test]
    fn test_rrca_bit0_set() {
        // A = 0x01 (0000_0001): bit0=1, rotate right → result=0x80, C=1
        let mut cpu = make_test_cpu(vec![0x0F]).with_registers(Registers {
            a: 0x01,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x80); // bit0 wraps to bit7
        assert_eq!(cpu.registers().f, Flags::C); // carry set, Z=0
    }

    /// RRCA (0x0F) with bit 0 clear: carry should be clear, Z=0
    #[test]
    fn test_rrca_bit0_clear() {
        // A = 0x80: bit0=0, rotate right → result=0x40, C=0
        let mut cpu = make_test_cpu(vec![0x0F]).with_registers(Registers {
            a: 0x80,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x40);
        assert_eq!(cpu.registers().f, Flags::empty()); // C=0, Z=0
    }

    /// RRA (0x1F) with carry clear and bit 0 set: C set, bit 7 of result clear (carry was 0)
    #[test]
    fn test_rra_carry_clear_bit0_set() {
        // A = 0x01, carry=0: result = (0x01 >> 1) | (0 << 7) = 0x00, C=1
        let mut cpu = make_test_cpu(vec![0x1F]).with_registers(Registers {
            a: 0x01,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x00); // bit7 = old carry = 0
        assert_eq!(cpu.registers().f, Flags::C); // C = old bit0 = 1; Z=0
    }

    /// RRA (0x1F): old carry goes to bit 7
    #[test]
    fn test_rra_carry_goes_to_bit7() {
        // A = 0x00, carry=1: result = (0x00 >> 1) | (1 << 7) = 0x80, C=0
        let mut cpu = make_test_cpu(vec![0x1F]).with_registers(Registers {
            a: 0x00,
            f: Flags::C,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x80); // old carry goes to bit7
        assert_eq!(cpu.registers().f, Flags::empty()); // C=0, Z=0
    }

    /// Verify Z is always 0 even when rotate result is 0
    #[test]
    fn test_rra_z_always_zero_when_result_zero() {
        // A = 0x00, carry=0: result = 0x00, C=0; Z must be 0
        let mut cpu = make_test_cpu(vec![0x1F]).with_registers(Registers {
            a: 0x00,
            ..Default::default()
        });
        cpu.tick().unwrap();

        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::empty()); // Z=0 even when result is 0
    }

    // --- JP/JR Jump instruction integration tests ---

    /// JP nn (0xC3) — unconditional absolute jump to 16-bit immediate.
    /// ROM: [0xC3, 0x05, 0x00] — jump to address 0x0005, 16 cycles.
    #[test]
    fn test_jp_nn_sets_pc() {
        let mut cpu = make_test_cpu(vec![0xC3, 0x05, 0x00]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 16);
        assert_eq!(cpu.registers().pc, 0x0005);
    }

    /// JP HL (0xE9) — jump to address in HL. HL=0x1234, expect PC=0x1234, 4 cycles.
    #[test]
    fn test_jp_hl_sets_pc_to_hl() {
        let mut cpu = make_test_cpu(vec![0xE9]).with_registers(Registers {
            h: 0x12,
            l: 0x34,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().pc, 0x1234);
    }

    /// JP Z, nn (0xCA) with Z flag set — jump taken, 16 cycles, PC = target.
    #[test]
    fn test_jp_z_nn_taken_when_z_set() {
        let mut cpu = make_test_cpu(vec![0xCA, 0x08, 0x00]).with_registers(Registers {
            f: Flags::Z,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 16);
        assert_eq!(cpu.registers().pc, 0x0008);
    }

    /// JP Z, nn (0xCA) with Z flag clear — jump not taken, 12 cycles, PC past instruction.
    #[test]
    fn test_jp_z_nn_not_taken_when_z_clear() {
        let mut cpu = make_test_cpu(vec![0xCA, 0x08, 0x00]).with_registers(Registers {
            f: Flags::empty(),
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().pc, 0x0003);
    }

    /// JR e (0x18) with positive offset +5. After reading opcode (PC=1) and offset byte (PC=2),
    /// PC += 5 → PC = 7. 12 cycles.
    #[test]
    fn test_jr_positive_offset() {
        let mut cpu = make_test_cpu(vec![0x18, 0x05]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().pc, 0x0007);
    }

    /// JR e (0x18) with negative offset -2 (0xFE). After reading (PC=2), PC += -2 → PC = 0.
    #[test]
    fn test_jr_negative_offset() {
        let mut cpu = make_test_cpu(vec![0x18, 0xFE]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().pc, 0x0000);
    }

    /// JR NZ, e (0x20) — Z clear, condition true, jump taken. PC = 2 + 3 = 5. 12 cycles.
    #[test]
    fn test_jr_nz_taken_when_z_clear() {
        let mut cpu = make_test_cpu(vec![0x20, 0x03]).with_registers(Registers {
            f: Flags::empty(),
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().pc, 0x0005);
    }

    /// JR NZ, e (0x20) — Z set, condition false, jump not taken. PC = 2. 8 cycles.
    #[test]
    fn test_jr_nz_not_taken_when_z_set() {
        let mut cpu = make_test_cpu(vec![0x20, 0x03]).with_registers(Registers {
            f: Flags::Z,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().pc, 0x0002);
    }

    // --- AND/OR/XOR integration tests ---

    /// AND B: A=0xFF, B=0x0F -> A=0x0F, H=1, Z=0, N=0, C=0
    #[test]
    fn test_and8_reg_b() {
        let mut cpu = make_test_cpu(vec![0xA0]).with_registers(Registers {
            a: 0xFF,
            b: 0x0F,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x0F);
        assert_eq!(cpu.registers().f, Flags::H);
    }

    /// AND A when A=0: A = 0 & 0 = 0, Z=1, H=1, N=0, C=0
    #[test]
    fn test_and8_a_zero_result() {
        let mut cpu = make_test_cpu(vec![0xA7]).with_registers(Registers {
            a: 0x00,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::H);
    }

    /// AND n (immediate): A=0xF0, n=0x3C -> A=0x30, H=1, Z=0
    #[test]
    fn test_and8_imm8() {
        let mut cpu = make_test_cpu(vec![0xE6, 0x3C]).with_registers(Registers {
            a: 0xF0,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x30);
        assert_eq!(cpu.registers().f, Flags::H);
    }

    /// OR C: A=0xF0, C=0x0F -> A=0xFF, H=0, Z=0, N=0, C=0
    #[test]
    fn test_or8_reg_c() {
        let mut cpu = make_test_cpu(vec![0xB1]).with_registers(Registers {
            a: 0xF0,
            c: 0x0F,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0xFF);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    /// OR n with result 0: A=0x00, n=0x00 -> Z=1, all others clear
    #[test]
    fn test_or8_imm8_zero_result() {
        let mut cpu = make_test_cpu(vec![0xF6, 0x00]).with_registers(Registers {
            a: 0x00,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z);
    }

    /// XOR A (self): always produces 0, Z=1, N=0, H=0, C=0
    #[test]
    fn test_xor8_a_self() {
        let mut cpu = make_test_cpu(vec![0xAF]).with_registers(Registers {
            a: 0x42,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z);
    }

    /// XOR (HL): reads from memory, A=0xFF, (HL)=0x0F -> A=0xF0, flags empty
    #[test]
    fn test_xor8_mem_hl() {
        let mut cpu = make_test_cpu_with_memory(
            |m| { m.write(0xC000, 0x0F).unwrap(); },
            vec![0xAE],
        ).with_registers(Registers {
            a: 0xFF,
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0xF0);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    // --- LD16 integration tests ---

    /// LD BC, nn — opcode 0x01, load 16-bit immediate 0x1234 into BC (little-endian).
    #[test]
    fn test_ld16_bc_nn() {
        let mut cpu = make_test_cpu(vec![0x01, 0x34, 0x12]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().bc(), 0x1234);
    }

    /// LD DE, nn — opcode 0x11.
    #[test]
    fn test_ld16_de_nn() {
        let mut cpu = make_test_cpu(vec![0x11, 0xEF, 0xBE]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().de(), 0xBEEF);
    }

    /// LD HL, nn — opcode 0x21.
    #[test]
    fn test_ld16_hl_nn() {
        let mut cpu = make_test_cpu(vec![0x21, 0x00, 0xC0]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().hl(), 0xC000);
    }

    /// LD SP, nn — opcode 0x31.
    #[test]
    fn test_ld16_sp_nn() {
        let mut cpu = make_test_cpu(vec![0x31, 0xFF, 0xFF]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().sp, 0xFFFF);
    }

    /// LD (nn), SP — opcode 0x08, store SP to memory at address.
    /// ROM: [0x08, 0x00, 0xC0] SP=0xBEEF -> memory[0xC000]=0xEF, memory[0xC001]=0xBE.
    #[test]
    fn test_ld16_nn_sp() {
        let mut cpu = make_test_cpu(vec![0x08, 0x00, 0xC0]).with_registers(Registers {
            sp: 0xBEEF,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 20);
        assert_eq!(cpu.memory.read(0xC000).unwrap(), 0xEF); // low byte
        assert_eq!(cpu.memory.read(0xC001).unwrap(), 0xBE); // high byte
    }

    /// LD SP, HL — opcode 0xF9, copy HL into SP.
    #[test]
    fn test_ld16_sp_hl() {
        let mut registers = Registers::default();
        registers.set_hl(0xDEAD);
        let mut cpu = make_test_cpu(vec![0xF9]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().sp, 0xDEAD);
    }

    /// LD (BC), A — opcode 0x02, store A to memory at BC.
    #[test]
    fn test_ld16_bc_a() {
        let mut registers = Registers::default();
        registers.a = 0x42;
        registers.set_bc(0xC000);
        let mut cpu = make_test_cpu(vec![0x02]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.memory.read(0xC000).unwrap(), 0x42);
    }

    /// LD A, (DE) — opcode 0x1A, load A from memory at DE.
    #[test]
    fn test_ld16_a_de() {
        let mut registers = Registers::default();
        registers.set_de(0xC005);
        let mut cpu = make_test_cpu_with_memory(
            |m| { m.write(0xC005, 0x77).unwrap(); },
            vec![0x1A],
        ).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x77);
    }

    /// LD (HL+), A — opcode 0x22, store A to (HL), then HL++.
    #[test]
    fn test_ld16_hli_a() {
        let mut registers = Registers::default();
        registers.a = 0xAB;
        registers.set_hl(0xC010);
        let mut cpu = make_test_cpu(vec![0x22]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.memory.read(0xC010).unwrap(), 0xAB);
        assert_eq!(cpu.registers().hl(), 0xC011);
    }

    /// LD A, (HL-) — opcode 0x3A, load A from (HL), then HL--.
    #[test]
    fn test_ld16_a_hld() {
        let mut registers = Registers::default();
        registers.set_hl(0xC020);
        let mut cpu = make_test_cpu_with_memory(
            |m| { m.write(0xC020, 0x55).unwrap(); },
            vec![0x3A],
        ).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x55);
        assert_eq!(cpu.registers().hl(), 0xC01F);
    }

    /// LD (nn), A — opcode 0xEA, store A to direct 16-bit address.
    #[test]
    fn test_ld16_nn_a() {
        let mut registers = Registers::default();
        registers.a = 0xCC;
        let mut cpu = make_test_cpu(vec![0xEA, 0x30, 0xC0]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 16);
        assert_eq!(cpu.memory.read(0xC030).unwrap(), 0xCC);
    }

    /// LDH (n), A — opcode 0xE0, store A to 0xFF00+n.
    /// n=0x80 -> address=0xFF80 (HRAM), A=0x11.
    #[test]
    fn test_ld16_ldh_n_a() {
        let mut registers = Registers::default();
        registers.a = 0x11;
        let mut cpu = make_test_cpu(vec![0xE0, 0x80]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.memory.read(0xFF80).unwrap(), 0x11);
    }

    /// LD (C), A — opcode 0xE2, store A to 0xFF00+C.
    /// C=0x80 -> address=0xFF80, A=0x22.
    #[test]
    fn test_ld16_ld_c_a() {
        let mut registers = Registers::default();
        registers.a = 0x22;
        registers.c = 0x80;
        let mut cpu = make_test_cpu(vec![0xE2]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.memory.read(0xFF80).unwrap(), 0x22);
    }

    // --- Misc instruction integration tests ---

    /// NOP (0x00): PC advances by 1, no registers changed.
    #[test]
    fn test_nop_advances_pc() {
        let mut cpu = make_test_cpu(vec![0x00]).with_registers(Registers {
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().pc, 1);
        assert_eq!(cpu.registers().a, 0x00);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    /// HALT (0x76): returns 4 cycles without crashing.
    #[test]
    fn test_halt_returns_cycles() {
        let mut cpu = make_test_cpu(vec![0x76]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
    }

    /// STOP (0x10): next byte (0x00) is consumed, so PC advances by 2.
    #[test]
    fn test_stop_consumes_next_byte() {
        let mut cpu = make_test_cpu(vec![0x10, 0x00]);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8); // fetch + consume next byte
        assert_eq!(cpu.registers().pc, 2); // consumed both 0x10 and 0x00
    }

    /// CPL (0x2F): A = ~A, N=1, H=1.
    #[test]
    fn test_cpl_complements_a() {
        let mut cpu = make_test_cpu(vec![0x2F]).with_registers(Registers {
            a: 0b10110011,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0b01001100);
        assert!(cpu.registers().f.contains(Flags::N));
        assert!(cpu.registers().f.contains(Flags::H));
    }

    /// CPL preserves Z and C flags.
    #[test]
    fn test_cpl_preserves_z_and_c_flags() {
        let mut cpu = make_test_cpu(vec![0x2F]).with_registers(Registers {
            a: 0xFF,
            f: Flags::Z | Flags::C,
            ..Default::default()
        });
        let _cycles = cpu.tick().unwrap();

        assert_eq!(cpu.registers().a, 0x00);
        assert!(cpu.registers().f.contains(Flags::Z));
        assert!(cpu.registers().f.contains(Flags::C));
        assert!(cpu.registers().f.contains(Flags::N));
        assert!(cpu.registers().f.contains(Flags::H));
    }

    /// SCF (0x37): C=1, N=0, H=0, Z unchanged.
    #[test]
    fn test_scf_sets_carry() {
        let mut cpu = make_test_cpu(vec![0x37]).with_registers(Registers {
            f: Flags::Z | Flags::N | Flags::H,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert!(cpu.registers().f.contains(Flags::C));
        assert!(!cpu.registers().f.contains(Flags::N));
        assert!(!cpu.registers().f.contains(Flags::H));
        assert!(cpu.registers().f.contains(Flags::Z)); // Z unchanged
    }

    /// CCF with C set (0x3F): C should be cleared.
    #[test]
    fn test_ccf_clears_carry_when_set() {
        let mut cpu = make_test_cpu(vec![0x3F]).with_registers(Registers {
            f: Flags::C,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert!(!cpu.registers().f.contains(Flags::C));
        assert!(!cpu.registers().f.contains(Flags::N));
        assert!(!cpu.registers().f.contains(Flags::H));
    }

    /// CCF with C clear (0x3F): C should be set.
    #[test]
    fn test_ccf_sets_carry_when_clear() {
        let mut cpu = make_test_cpu(vec![0x3F]).with_registers(Registers {
            f: Flags::empty(),
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert!(cpu.registers().f.contains(Flags::C));
    }

    /// DAA after BCD addition: A=0x08, B=0x09 -> ADD -> A=0x11, H=1 -> DAA -> A=0x17.
    #[test]
    fn test_daa_after_bcd_addition() {
        // ADD A, B (0x80) then DAA (0x27)
        // A=0x08, B=0x09: (0x8+0x9=0x11), (8&0xF)+(9&0xF)=17>15 => H flag set
        // DAA: H=1 so add 0x06 -> 0x11+0x06=0x17
        let mut cpu = make_test_cpu(vec![0x80, 0x27]).with_registers(Registers {
            a: 0x08,
            b: 0x09,
            ..Default::default()
        });
        cpu.tick().unwrap(); // ADD A, B -> A=0x11, H=1
        let cycles = cpu.tick().unwrap(); // DAA

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().a, 0x17); // 8+9=17 in BCD
        assert!(!cpu.registers().f.contains(Flags::H));
    }

    /// DI (0xF3): returns 4 cycles.
    #[test]
    fn test_di_returns_cycles() {
        let mut cpu = make_test_cpu(vec![0xF3]);
        let cycles = cpu.tick().unwrap();
        assert_eq!(cycles, 4);
    }

    /// EI (0xFB): returns 4 cycles.
    #[test]
    fn test_ei_returns_cycles() {
        let mut cpu = make_test_cpu(vec![0xFB]);
        let cycles = cpu.tick().unwrap();
        assert_eq!(cycles, 4);
    }

    // --- INC/DEC integration tests ---

    /// INC B — opcode 0x04.
    /// B=0x05. Expected: B=0x06, Z=0, N=0, H=0, C unchanged.
    #[test]
    fn test_inc8_b() {
        let mut cpu = make_test_cpu(vec![0x04]).with_registers(Registers {
            b: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().b, 0x06);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    /// INC B rollover (0xFF → 0x00): Z set, H set, C unchanged (C preserved).
    #[test]
    fn test_inc8_b_rollover() {
        let mut cpu = make_test_cpu(vec![0x04]).with_registers(Registers {
            b: 0xFF,
            f: Flags::C,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().b, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::H | Flags::C); // Z, H set; C preserved
    }

    /// INC B half-carry: lower nibble 0x0F → 0x10 sets H.
    #[test]
    fn test_inc8_b_half_carry() {
        let mut cpu = make_test_cpu(vec![0x04]).with_registers(Registers {
            b: 0x0F,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().b, 0x10);
        assert_eq!(cpu.registers().f, Flags::H);
    }

    /// DEC C — opcode 0x0D.
    /// C=0x05. Expected: C=0x04, N=1, Z=0, H=0.
    #[test]
    fn test_dec8_c() {
        let mut cpu = make_test_cpu(vec![0x0D]).with_registers(Registers {
            c: 0x05,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().c, 0x04);
        assert_eq!(cpu.registers().f, Flags::N);
    }

    /// DEC C to zero: Z set.
    #[test]
    fn test_dec8_c_to_zero() {
        let mut cpu = make_test_cpu(vec![0x0D]).with_registers(Registers {
            c: 0x01,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().c, 0x00);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::N);
    }

    /// DEC C half-borrow: lower nibble 0x10 → 0x0F. H set.
    #[test]
    fn test_dec8_c_half_borrow() {
        let mut cpu = make_test_cpu(vec![0x0D]).with_registers(Registers {
            c: 0x10,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().c, 0x0F);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H);
    }

    /// DEC C preserves C flag.
    #[test]
    fn test_dec8_c_preserves_carry() {
        let mut cpu = make_test_cpu(vec![0x0D]).with_registers(Registers {
            c: 0x05,
            f: Flags::C,
            ..Default::default()
        });
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().c, 0x04);
        assert_eq!(cpu.registers().f, Flags::N | Flags::C); // C preserved
    }

    /// INC (HL) — opcode 0x34.
    /// HL=0xC000, memory[0xC000]=0x07. Expected: memory[0xC000]=0x08, 12 cycles.
    /// Verify by reading back with LD A,(HL).
    #[test]
    fn test_inc8_mem_hl() {
        let mut memory = GameBoyMemory::with_rom(vec![0x34, 0x7E]);
        memory.write(0xC000, 0x07).unwrap();
        let decoder = Box::new(OpCodeDecoder::new());
        let mut cpu = Sm83::new(Box::new(memory), decoder).with_registers(Registers {
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });

        let cycles = cpu.tick().unwrap();
        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().f, Flags::empty());

        // Verify memory was updated by loading via LD A,(HL)
        let regs = cpu.registers();
        cpu = cpu.with_registers(Registers { a: 0x00, ..regs });
        let cycles2 = cpu.tick().unwrap();
        assert_eq!(cycles2, 8);
        assert_eq!(cpu.registers().a, 0x08);
    }

    /// DEC (HL) — opcode 0x35.
    /// HL=0xC000, memory[0xC000]=0x07. Expected: memory[0xC000]=0x06, 12 cycles.
    #[test]
    fn test_dec8_mem_hl() {
        let mut memory = GameBoyMemory::with_rom(vec![0x35, 0x7E]);
        memory.write(0xC000, 0x07).unwrap();
        let decoder = Box::new(OpCodeDecoder::new());
        let mut cpu = Sm83::new(Box::new(memory), decoder).with_registers(Registers {
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });

        let cycles = cpu.tick().unwrap();
        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().f, Flags::N);

        // Verify memory was updated by loading via LD A,(HL)
        let regs = cpu.registers();
        cpu = cpu.with_registers(Registers { a: 0x00, ..regs });
        let cycles2 = cpu.tick().unwrap();
        assert_eq!(cycles2, 8);
        assert_eq!(cpu.registers().a, 0x06);
    }

    /// INC BC — opcode 0x03. No flags affected.
    /// BC=0x00FF. Expected: BC=0x0100, all flags unchanged.
    #[test]
    fn test_inc16_bc() {
        let mut registers = Registers::default();
        registers.set_bc(0x00FF);
        registers.f = Flags::Z | Flags::N | Flags::H | Flags::C; // all flags set
        let mut cpu = make_test_cpu(vec![0x03]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().bc(), 0x0100);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::N | Flags::H | Flags::C);
    }

    /// INC BC rollover: 0xFFFF → 0x0000. No flags affected.
    #[test]
    fn test_inc16_bc_rollover() {
        let mut registers = Registers::default();
        registers.set_bc(0xFFFF);
        let mut cpu = make_test_cpu(vec![0x03]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().bc(), 0x0000);
        assert_eq!(cpu.registers().f, Flags::empty()); // no flags changed
    }

    /// DEC SP — opcode 0x3B. No flags affected.
    /// SP=0x0100. Expected: SP=0x00FF, flags unchanged.
    #[test]
    fn test_dec16_sp() {
        let mut registers = Registers::default();
        registers.sp = 0x0100;
        registers.f = Flags::Z | Flags::C;
        let mut cpu = make_test_cpu(vec![0x3B]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().sp, 0x00FF);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::C); // flags unchanged
    }

    /// INC HL — opcode 0x23. No flags affected.
    #[test]
    fn test_inc16_hl() {
        let mut registers = Registers::default();
        registers.set_hl(0x1234);
        let mut cpu = make_test_cpu(vec![0x23]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().hl(), 0x1235);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    /// DEC HL — opcode 0x2B. No flags affected.
    #[test]
    fn test_dec16_hl() {
        let mut registers = Registers::default();
        registers.set_hl(0x1234);
        registers.f = Flags::N | Flags::H;
        let mut cpu = make_test_cpu(vec![0x2B]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().hl(), 0x1233);
        assert_eq!(cpu.registers().f, Flags::N | Flags::H); // flags unchanged
    }

    // --- PUSH / POP ---

    #[test]
    fn test_push_bc_writes_to_stack() {
        let mut registers = Registers::default();
        registers.set_bc(0xABCD);
        registers.sp = 0xC010;
        // PUSH BC = 0xC5, then POP DE = 0xD1 to verify round-trip via registers
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xC5, 0xD1])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap(); // PUSH BC
        assert_eq!(cycles, 16);
        assert_eq!(cpu.registers().sp, 0xC00E);
        cpu.tick().unwrap(); // POP DE
        assert_eq!(cpu.registers().de(), 0xABCD);
        assert_eq!(cpu.registers().sp, 0xC010);
    }

    #[test]
    fn test_push_af_writes_to_stack() {
        let mut registers = Registers::default();
        registers.a = 0x12;
        registers.f = Flags::Z | Flags::C;
        registers.sp = 0xC010;
        // PUSH AF = 0xF5, then POP AF = 0xF1 round-trip
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xF5, 0xF1])
            .with_registers(registers);
        cpu.tick().unwrap(); // PUSH AF
        assert_eq!(cpu.registers().sp, 0xC00E);
        // Clear A and F to confirm POP restores them
        let mut cleared = cpu.registers();
        cleared.a = 0x00;
        cleared.f = Flags::empty();
        let cpu = cpu.with_registers(cleared);
        let mut cpu = cpu;
        cpu.tick().unwrap(); // POP AF
        assert_eq!(cpu.registers().a, 0x12);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::C);
    }

    #[test]
    fn test_pop_bc_reads_from_stack() {
        // PUSH HL with known value, then POP BC to verify
        let mut registers = Registers::default();
        registers.set_hl(0xABCD);
        registers.sp = 0xC010;
        // PUSH HL = 0xE5, POP BC = 0xC1
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xE5, 0xC1])
            .with_registers(registers);
        cpu.tick().unwrap(); // PUSH HL
        let cycles = cpu.tick().unwrap(); // POP BC

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().sp, 0xC010);
        assert_eq!(cpu.registers().bc(), 0xABCD);
    }

    #[test]
    fn test_pop_af_restores_flags() {
        let mut registers = Registers::default();
        registers.a = 0x42;
        registers.f = Flags::Z | Flags::H;
        registers.sp = 0xC010;
        // PUSH AF = 0xF5, clear registers, POP AF = 0xF1
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xF5, 0xF1])
            .with_registers(registers);
        cpu.tick().unwrap(); // PUSH AF
        let mut cleared = cpu.registers();
        cleared.a = 0x00;
        cleared.f = Flags::empty();
        let mut cpu = cpu.with_registers(cleared);
        cpu.tick().unwrap(); // POP AF

        assert_eq!(cpu.registers().a, 0x42);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::H);
        assert_eq!(cpu.registers().sp, 0xC010);
    }

    #[test]
    fn test_pop_af_ignores_low_nibble() {
        let mut registers = Registers::default();
        registers.sp = 0xC00E;
        let mut cpu = make_test_cpu_with_memory(
            |m| {
                m.write(0xC00E, 0x9F).unwrap(); // Z|C|garbage_low_nibble
                m.write(0xC00F, 0x00).unwrap(); // A
            },
            vec![0xF1], // POP AF
        ).with_registers(registers);
        cpu.tick().unwrap();

        assert_eq!(cpu.registers().f.bits() & 0x0F, 0x00);
    }

    #[test]
    fn test_push_then_pop_roundtrip() {
        let mut registers = Registers::default();
        registers.set_hl(0x1234);
        registers.sp = 0xC010;
        // PUSH HL = 0xE5, POP BC = 0xC1
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xE5, 0xC1])
            .with_registers(registers);
        cpu.tick().unwrap(); // PUSH HL
        cpu.tick().unwrap(); // POP BC

        assert_eq!(cpu.registers().bc(), 0x1234);
        assert_eq!(cpu.registers().sp, 0xC010);
    }

    // --- CALL / RET ---

    #[test]
    fn test_call_nn_pushes_pc_and_jumps() {
        // CALL nn = 0xCD, target = 0x0050
        // ROM: [0xCD, 0x50, 0x00, ...]
        // PC starts at 0, after fetch opcode PC=1, after fetch lo PC=2, after fetch hi PC=3
        let mut registers = Registers::default();
        registers.sp = 0xC010;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCD, 0x50, 0x00])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 24);
        assert_eq!(cpu.registers().pc, 0x0050);
        assert_eq!(cpu.registers().sp, 0xC00E);
        // Return address (0x0003) should be on stack
        assert_eq!(cpu.registers().sp, 0xC00E);
    }

    #[test]
    fn test_call_then_ret_returns_to_caller() {
        // ROM: CALL 0x0005, NOP, NOP, NOP, RET
        // Addr: 0x0000 0xCD 0x05 0x00  (CALL 0x0005)
        // Addr: 0x0003 0x00            (NOP — never reached)
        // Addr: 0x0004 0x00            (NOP — never reached)
        // Addr: 0x0005 0xC9            (RET)
        let mut registers = Registers::default();
        registers.sp = 0xC010;
        let rom = vec![0xCD, 0x05, 0x00, 0x00, 0x00, 0xC9];
        let mut cpu = make_test_cpu_with_memory(|_| {}, rom).with_registers(registers);
        cpu.tick().unwrap(); // CALL 0x0005 — PC becomes 0x0005
        assert_eq!(cpu.registers().pc, 0x0005);
        cpu.tick().unwrap(); // RET — PC becomes 0x0003
        assert_eq!(cpu.registers().pc, 0x0003);
        assert_eq!(cpu.registers().sp, 0xC010);
    }

    #[test]
    fn test_call_cc_not_taken() {
        // CALL NZ with Z flag set — should not jump, 12 cycles
        // 0xC4 = CALL NZ, nn
        let mut registers = Registers::default();
        registers.f = Flags::Z;
        registers.sp = 0xC010;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xC4, 0x50, 0x00])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().pc, 0x0003); // advanced past operands
        assert_eq!(cpu.registers().sp, 0xC010); // SP unchanged
    }

    #[test]
    fn test_call_cc_taken() {
        // CALL Z with Z flag set — should jump, 24 cycles
        // 0xCC = CALL Z, nn
        let mut registers = Registers::default();
        registers.f = Flags::Z;
        registers.sp = 0xC010;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCC, 0x50, 0x00])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 24);
        assert_eq!(cpu.registers().pc, 0x0050);
    }

    #[test]
    fn test_ret_cc_not_taken() {
        // RET NZ with Z set — not taken, 8 cycles
        // 0xC0 = RET NZ
        let mut registers = Registers::default();
        registers.f = Flags::Z;
        registers.sp = 0xC010;
        let mut cpu =
            make_test_cpu_with_memory(|_| {}, vec![0xC0]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().sp, 0xC010); // SP unchanged
    }

    #[test]
    fn test_rst_pushes_pc_and_jumps_to_vector() {
        // RST 0x08 = 0xCF
        let mut registers = Registers::default();
        registers.sp = 0xC010;
        let mut cpu =
            make_test_cpu_with_memory(|_| {}, vec![0xCF]).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 16);
        assert_eq!(cpu.registers().pc, 0x0008);
        assert_eq!(cpu.registers().sp, 0xC00E);
    }

    #[test]
    fn test_cb_rlc_b() {
        // RLC B = 0xCB 0x00, B = 0b10110001 => result = 0b01100011, carry = 1
        let mut registers = Registers::default();
        registers.b = 0b10110001;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCB, 0x00])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().b, 0b01100011);
        assert!(cpu.registers().f.contains(Flags::C));
        assert!(!cpu.registers().f.contains(Flags::Z));
        assert!(!cpu.registers().f.contains(Flags::N));
        assert!(!cpu.registers().f.contains(Flags::H));
    }

    #[test]
    fn test_cb_rlc_b_zero() {
        // RLC B = 0xCB 0x00, B = 0 => result = 0, zero flag set
        let mut registers = Registers::default();
        registers.b = 0;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCB, 0x00])
            .with_registers(registers);
        cpu.tick().unwrap();

        assert_eq!(cpu.registers().b, 0);
        assert!(cpu.registers().f.contains(Flags::Z));
        assert!(!cpu.registers().f.contains(Flags::C));
    }

    #[test]
    fn test_cb_swap_a() {
        // SWAP A = 0xCB 0x37, A = 0xAB => result = 0xBA
        let mut registers = Registers::default();
        registers.a = 0xAB;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCB, 0x37])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0xBA);
        assert!(!cpu.registers().f.contains(Flags::Z));
        assert!(!cpu.registers().f.contains(Flags::C));
        assert!(!cpu.registers().f.contains(Flags::N));
        assert!(!cpu.registers().f.contains(Flags::H));
    }

    #[test]
    fn test_cb_bit_3_b_clear() {
        // BIT 3, B = 0xCB 0x58, B = 0b11110111 (bit 3 = 0) => Z flag set, H flag set, N clear
        let mut registers = Registers::default();
        registers.b = 0b11110111;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCB, 0x58])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert!(cpu.registers().f.contains(Flags::Z));
        assert!(cpu.registers().f.contains(Flags::H));
        assert!(!cpu.registers().f.contains(Flags::N));
        // B unchanged
        assert_eq!(cpu.registers().b, 0b11110111);
    }

    #[test]
    fn test_cb_bit_3_b_set() {
        // BIT 3, B = 0xCB 0x58, B = 0b00001000 (bit 3 = 1) => Z flag clear, H flag set
        let mut registers = Registers::default();
        registers.b = 0b00001000;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCB, 0x58])
            .with_registers(registers);
        cpu.tick().unwrap();

        assert!(!cpu.registers().f.contains(Flags::Z));
        assert!(cpu.registers().f.contains(Flags::H));
        assert!(!cpu.registers().f.contains(Flags::N));
    }

    #[test]
    fn test_cb_res_3_b() {
        // RES 3, B = 0xCB 0x98, B = 0xFF => result = 0xF7
        let mut registers = Registers::default();
        registers.b = 0xFF;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCB, 0x98])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().b, 0xF7);
    }

    #[test]
    fn test_cb_set_3_b() {
        // SET 3, B = 0xCB 0xD8, B = 0x00 => result = 0x08
        let mut registers = Registers::default();
        registers.b = 0x00;
        let mut cpu = make_test_cpu_with_memory(|_| {}, vec![0xCB, 0xD8])
            .with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().b, 0x08);
    }

    #[test]
    fn test_cb_rlc_hl_mem() {
        // RLC (HL) = 0xCB 0x06, (HL) = 0b10110001 => result = 0b01100011, carry = 1
        let mut registers = Registers::default();
        registers.set_hl(0xC000);
        let mut cpu = make_test_cpu_with_memory(
            |m| { m.write(0xC000, 0b10110001).unwrap(); },
            vec![0xCB, 0x06],
        ).with_registers(registers);
        let cycles = cpu.tick().unwrap();

        assert_eq!(cycles, 16);
        assert_eq!(cpu.memory.read(0xC000).unwrap(), 0b01100011);
        assert!(cpu.registers().f.contains(Flags::C));
        assert!(!cpu.registers().f.contains(Flags::Z));
    }

    // --- IME and HALT ---

    #[test]
    fn test_ime_starts_false() {
        let cpu = make_test_cpu(vec![0x00]);
        assert!(!cpu.is_halted());
    }

    #[test]
    fn test_di_clears_ime() {
        // DI = 0xF3
        let mut cpu = make_test_cpu(vec![0xFB, 0xF3]); // EI then DI
        cpu.tick().unwrap(); // EI
        cpu.tick().unwrap(); // DI
        assert!(!cpu.ime());
    }

    #[test]
    fn test_ei_sets_ime() {
        // EI = 0xFB, NOP = 0x00: IME becomes active after the instruction following EI
        let mut cpu = make_test_cpu(vec![0xFB, 0x00]);
        cpu.tick().unwrap(); // EI — ime_pending set, IME still false
        assert!(!cpu.ime());
        cpu.tick().unwrap(); // NOP — IME activates at start of this tick
        assert!(cpu.ime());
    }

    #[test]
    fn test_halt_sets_halted() {
        // HALT = 0x76
        let mut cpu = make_test_cpu(vec![0x76]);
        assert!(!cpu.is_halted());
        cpu.tick().unwrap();
        assert!(cpu.is_halted());
    }

    #[test]
    fn test_halted_cpu_returns_4_cycles_without_advancing_pc() {
        // HALT then NOP — halted CPU should not execute NOP
        let mut cpu = make_test_cpu(vec![0x76, 0x00]);
        cpu.tick().unwrap(); // executes HALT, sets halted
        let pc_after_halt = cpu.registers().pc;

        let cycles = cpu.tick().unwrap(); // should return early, not execute NOP
        assert_eq!(cycles, 4);
        assert_eq!(cpu.registers().pc, pc_after_halt); // PC did not advance
    }

    // --- Interrupt dispatch ---

    #[test]
    fn test_ei_delays_one_instruction() {
        // EI (0xFB) then NOP (0x00): IME should NOT be set until after the NOP
        let mut cpu = make_test_cpu(vec![0xFB, 0x00]);
        cpu.tick().unwrap(); // EI — ime_pending, but IME still false
        assert!(!cpu.ime()); // not yet
        cpu.tick().unwrap(); // NOP — now IME becomes true
        assert!(cpu.ime());
    }

    #[test]
    fn test_interrupt_dispatch_jumps_to_vector_and_pushes_pc() {
        // Set up: IE=1 (VBlank enabled), IF=1 (VBlank pending), IME=true
        // ROM: EI (0xFB), NOP (0x00), then padding
        let mut cpu = make_test_cpu(vec![0xFB, 0x00, 0x00, 0x00]);
        let mut regs = Registers::default();
        regs.sp = 0xDFFE;
        cpu = cpu.with_registers(regs);
        cpu.memory.write_io(IE_ADDR, 0x01); // IE: VBlank enabled
        cpu.memory.write_io(IF_ADDR, 0x01); // IF: VBlank pending

        cpu.tick().unwrap(); // EI — ime_pending=true
        cpu.tick().unwrap(); // NOP — IME becomes true, interrupt dispatched after

        assert_eq!(cpu.registers().pc, 0x0040); // VBlank vector
        assert_eq!(cpu.registers().sp, 0xDFFC); // SP decremented by 2
        assert_eq!(cpu.memory.read_io(IF_ADDR) & 0x01, 0); // IF bit 0 cleared
        assert!(!cpu.ime()); // IME cleared during dispatch
    }

    #[test]
    fn test_halt_resumes_when_interrupt_pending() {
        // HALT with IE=1, IF=1 but IME=false: CPU wakes but doesn't dispatch
        let mut cpu = make_test_cpu(vec![0x76, 0x00]); // HALT, NOP
        cpu.memory.write_io(IE_ADDR, 0x01); // IE: VBlank enabled
        cpu.memory.write_io(IF_ADDR, 0x01); // IF: VBlank pending

        cpu.tick().unwrap(); // HALT — sets halted=true
        assert!(cpu.is_halted()); // still halted after HALT instruction
        // Next tick: 1 halted M-cycle (4 T-cycles) then wakes (IE&IF!=0), executes NOP (4 T-cycles)
        let cycles = cpu.tick().unwrap();
        assert!(!cpu.is_halted());
        assert_eq!(cycles, 8); // halted M-cycle + NOP
    }
}
