#[cfg(feature = "trace")]
use alloc::boxed::Box;

use super::instructions::adc::opcode::Adc;
use super::instructions::add::opcode::{Add16, Add8, AddSP16};
use super::instructions::call::opcode::{Call, CallOp};
use super::instructions::cb::opcode::{CbInstruction, CbOp, CbTarget};
use super::instructions::cp::opcode::Cp8;
use super::instructions::inc_dec::opcode::{Dec16, Dec8, Inc16, Inc8};
use super::instructions::instructions::{Error as InstructionError, Instructions};
use super::instructions::jump::opcode::{Condition, Jump, JumpOp};
use super::instructions::ld::opcode::Ld8;
use super::instructions::ld16::opcode::{Ld16, Ld16Op};
use super::instructions::logic::opcode::{And8, Or8, Xor8};
use super::instructions::misc::opcode::{Misc, MiscOp};
use super::instructions::opcodes::{OpCodeDecoder, OpCodeTable};
use super::instructions::operand::{Memory, Operand, Register16, Register8};
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
#[cfg(feature = "perf")]
use super::perf::Sm83PerfRecorder;
use super::registers::{Flags, Registers};

use crate::memory::memory::{GameBoyMemory, Memory as MemoryTrait};
#[cfg(feature = "perf")]
pub use super::perf::Sm83PerfProfile;

/// Interrupt Master Enable state. EI has a 1-instruction delay before IME becomes active.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum ImeState {
    Disabled,
    /// EI was just executed — IME activates after the next instruction.
    Pending,
    Enabled,
}

/// Snapshot of CPU and key hardware state passed to trace hooks.
#[cfg(feature = "trace")]
pub struct TraceEvent<'a> {
    pub pc: u16,
    pub registers: &'a Registers,
    pub ime: bool,
    pub ly: u8,
    pub if_: u8,
    pub ie: u8,
    pub lcdc: u8,
}

pub struct Sm83 {
    // ── Pure CPU state ───────────────────────────────────────────────────────
    pub(crate) registers: Registers,
    pub(crate) ime: ImeState,
    pub(crate) halted: bool,
    /// Pre-decoded opcode table; built once at construction time.
    opcodes: OpCodeTable,
    /// Thin raw pointer to GameBoyMemory, valid only during step().
    /// Set to non-null at the start of step() and cleared before returning.
    _memory: *mut GameBoyMemory,
    /// Per-instruction trace hook, enabled by the `trace` feature.
    #[cfg(feature = "trace")]
    trace_hook: Option<Box<dyn FnMut(TraceEvent<'_>)>>,
    #[cfg(feature = "perf")]
    pub(crate) perf: Sm83PerfRecorder,
}

impl Sm83 {
    /// Create a pure CPU state machine without peripheral state.
    /// Used by `GameBoy` which owns the peripheral state itself.
    pub(crate) fn new_pure() -> Self {
        Self {
            registers: Registers::default(),
            ime: ImeState::Disabled,
            halted: false,
            opcodes: OpCodeTable::from_decoder(&OpCodeDecoder::new()),
            _memory: core::ptr::null_mut(),
            #[cfg(feature = "trace")]
            trace_hook: None,
            #[cfg(feature = "perf")]
            perf: Sm83PerfRecorder::default(),
        }
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

    /// Install a per-instruction trace hook (only available with `--features trace`).
    /// The hook is called after every instruction with a snapshot of CPU and hardware state.
    #[cfg(feature = "trace")]
    pub fn set_trace_hook<F>(&mut self, hook: F)
    where
        F: FnMut(TraceEvent<'_>) + 'static,
    {
        self.trace_hook = Some(Box::new(hook));
    }

    #[cfg(feature = "perf")]
    pub fn take_perf_profile(&mut self) -> Sm83PerfProfile {
        self.perf.take_profile()
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_ime(&mut self) {
        if self.ime == ImeState::Pending {
            self.ime = ImeState::Enabled;
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn has_pending_interrupt(&self) -> bool {
        let ie = unsafe { (*self._memory).ie() };
        let if_ = unsafe { (*self._memory).read_io(0xFF0F) };
        ie & if_ != 0
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn take_pending_interrupt(&mut self) -> Option<u8> {
        let ie = unsafe { (*self._memory).ie() };
        let if_ = unsafe { (*self._memory).read_io(0xFF0F) };
        let pending = ie & if_;
        if pending == 0 {
            return None;
        }
        let bit = pending.trailing_zeros() as u8;
        // Clear the IF bit in memory
        let new_if = if_ & !(1 << bit);
        unsafe { (*self._memory).write_io(0xFF0F, new_if); }
        Some(bit)
    }

    // ── Register encoding helpers ─────────────────────────────────────────────

    /// r16 pairs for stack instructions: 0=BC 1=DE 2=HL 3=AF.
    fn r16stk_get(&self, r: u8) -> u16 {
        match r {
            0 => self.registers.bc(), 1 => self.registers.de(),
            2 => self.registers.hl(),
            _ => {
                let a = self.registers.a as u16;
                let f = self.registers.f.bits() as u16;
                (a << 8) | f
            }
        }
    }

    fn r16stk_set(&mut self, r: u8, v: u16) {
        match r {
            0 => self.registers.set_bc(v), 1 => self.registers.set_de(v),
            2 => self.registers.set_hl(v),
            _ => {
                self.registers.a = (v >> 8) as u8;
                self.registers.f = Flags::from_bits_truncate(v as u8);
            }
        }
    }

    // ── Bus access helpers ────────────────────────────────────────────────────

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn bus_read(&self, addr: u16) -> u8 {
        unsafe { (*self._memory).read_fast(addr) }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn bus_write(&mut self, addr: u16, val: u8) {
        unsafe { (*self._memory).write(addr, val).ok(); }
    }

    fn fetch_byte(&mut self) -> u8 {
        let addr = self.registers.pc;
        self.registers.pc = self.registers.pc.wrapping_add(1);
        self.bus_read(addr)
    }

    // ── Instruction-level step interface ─────────────────────────────────────

    /// Execute one complete instruction. Returns T-cycles elapsed.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn step(&mut self, memory: &mut GameBoyMemory) -> u32 {
        self._memory = memory as *mut GameBoyMemory;
        let t = self.step_inner();
        self._memory = core::ptr::null_mut();
        t
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn step_inner(&mut self) -> u32 {
        self.advance_ime();

        if self.halted {
            if self.has_pending_interrupt() {
                self.halted = false;
                if self.ime == ImeState::Enabled {
                    if let Some(bit) = self.take_pending_interrupt() {
                        return self.dispatch_isr(bit);
                    }
                }
                // IME=false: unhalt, fall through to execute next instruction
            } else {
                return 4; // one NOP-equivalent cycle while halted
            }
        }

        let opcode = self.fetch_byte();
        if opcode == 0xCB {
            let cb_opcode = self.fetch_byte();
            let handler = match self.opcodes.get_cb(cb_opcode) {
                Ok(h) => h,
                Err(_) => return 8,
            };
            let cycles = handler.execute(self).unwrap_or(8) as u32;
            if self.ime == ImeState::Enabled {
                if let Some(bit) = self.take_pending_interrupt() {
                    return cycles + self.dispatch_isr(bit);
                }
            }
            return cycles;
        }

        let handler = match self.opcodes.get(opcode) {
            Ok(h) => h,
            Err(_) => return 4,
        };
        let cycles = handler.execute(self).unwrap_or(4) as u32;
        // Post-instruction: check for interrupt dispatch
        if self.ime == ImeState::Enabled {
            if let Some(bit) = self.take_pending_interrupt() {
                return cycles + self.dispatch_isr(bit);
            }
        }
        cycles
    }

    /// Dispatch an interrupt service routine. Returns T-cycles for the ISR dispatch (20).
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn dispatch_isr(&mut self, bit: u8) -> u32 {
        self.ime = ImeState::Disabled;
        let sp = self.registers.sp;
        let pc = self.registers.pc;
        self.bus_write(sp.wrapping_sub(1), (pc >> 8) as u8);
        self.bus_write(sp.wrapping_sub(2), pc as u8);
        self.registers.sp = sp.wrapping_sub(2);
        self.registers.pc = 0x0040u16.wrapping_add((bit as u16) * 8);
        20 // 5 M-cycles = 20 T-cycles
    }

    // ── Register enum helpers ─────────────────────────────────────────────────

    fn get_r8_enum(&self, r: Register8) -> u8 {
        match r {
            Register8::A => self.registers.a,
            Register8::B => self.registers.b,
            Register8::C => self.registers.c,
            Register8::D => self.registers.d,
            Register8::E => self.registers.e,
            Register8::H => self.registers.h,
            Register8::L => self.registers.l,
        }
    }

    fn set_r8_enum(&mut self, r: Register8, val: u8) {
        match r {
            Register8::A => self.registers.a = val,
            Register8::B => self.registers.b = val,
            Register8::C => self.registers.c = val,
            Register8::D => self.registers.d = val,
            Register8::E => self.registers.e = val,
            Register8::H => self.registers.h = val,
            Register8::L => self.registers.l = val,
        }
    }

    fn get_r16_enum(&self, r: Register16) -> u16 {
        match r {
            Register16::AF => {
                let a = self.registers.a as u16;
                let f = self.registers.f.bits() as u16;
                (a << 8) | f
            }
            Register16::BC => self.registers.bc(),
            Register16::DE => self.registers.de(),
            Register16::HL => self.registers.hl(),
            Register16::SP => self.registers.sp,
        }
    }

    fn set_r16_enum(&mut self, r: Register16, val: u16) {
        match r {
            Register16::AF => {
                self.registers.a = (val >> 8) as u8;
                self.registers.f = Flags::from_bits_truncate(val as u8);
            }
            Register16::BC => self.registers.set_bc(val),
            Register16::DE => self.registers.set_de(val),
            Register16::HL => self.registers.set_hl(val),
            Register16::SP => self.registers.sp = val,
        }
    }

    fn get_operand8(&mut self, op: &Operand) -> u8 {
        match op {
            Operand::Register8(r) => self.get_r8_enum(*r),
            Operand::Memory(Memory::HL) => {
                let a = self.registers.hl();
                self.bus_read(a)
            }
            Operand::Memory(Memory::BC) => {
                let a = self.registers.bc();
                self.bus_read(a)
            }
            Operand::Memory(Memory::DE) => {
                let a = self.registers.de();
                self.bus_read(a)
            }
            Operand::Memory(Memory::HLI) => {
                let a = self.registers.hl();
                let v = self.bus_read(a);
                self.registers.set_hl(a.wrapping_add(1));
                v
            }
            Operand::Memory(Memory::HLD) => {
                let a = self.registers.hl();
                let v = self.bus_read(a);
                self.registers.set_hl(a.wrapping_sub(1));
                v
            }
            Operand::Imm8 | Operand::ImmSigned8 => self.fetch_byte(),
            _ => panic!("get_operand8: unsupported {:?}", op),
        }
    }

    fn set_operand8(&mut self, op: &Operand, val: u8) {
        match op {
            Operand::Register8(r) => self.set_r8_enum(*r, val),
            Operand::Memory(Memory::HL) => {
                let a = self.registers.hl();
                self.bus_write(a, val);
            }
            Operand::Memory(Memory::BC) => {
                let a = self.registers.bc();
                self.bus_write(a, val);
            }
            Operand::Memory(Memory::DE) => {
                let a = self.registers.de();
                self.bus_write(a, val);
            }
            Operand::Memory(Memory::HLI) => {
                let a = self.registers.hl();
                self.bus_write(a, val);
                self.registers.set_hl(a.wrapping_add(1));
            }
            Operand::Memory(Memory::HLD) => {
                let a = self.registers.hl();
                self.bus_write(a, val);
                self.registers.set_hl(a.wrapping_sub(1));
            }
            _ => panic!("set_operand8: unsupported {:?}", op),
        }
    }

    fn check_condition(&self, cond: &Condition) -> bool {
        match cond {
            Condition::NZ => !self.registers.f.contains(Flags::Z),
            Condition::Z  =>  self.registers.f.contains(Flags::Z),
            Condition::NC => !self.registers.f.contains(Flags::C),
            Condition::C  =>  self.registers.f.contains(Flags::C),
        }
    }

    fn pop16_inner(&mut self) -> u16 {
        let lo = self.bus_read(self.registers.sp);
        let hi = self.bus_read(self.registers.sp.wrapping_add(1));
        self.registers.sp = self.registers.sp.wrapping_add(2);
        u16::from_le_bytes([lo, hi])
    }

    fn push16_inner(&mut self, val: u16) {
        self.registers.sp = self.registers.sp.wrapping_sub(2);
        self.bus_write(self.registers.sp.wrapping_add(1), (val >> 8) as u8);
        self.bus_write(self.registers.sp, val as u8);
    }

    // ── CB helpers ────────────────────────────────────────────────────────────

    fn set_cb_result(&mut self, target: CbTarget, val: u8, flags: Flags) {
        self.write_cb_target(target, val);
        self.registers.f = flags;
    }

    fn write_cb_target(&mut self, target: CbTarget, val: u8) {
        match target {
            CbTarget::Reg(r) => self.set_r8_enum(r, val),
            CbTarget::HLMem => {
                let a = self.registers.hl();
                self.bus_write(a, val);
            }
        }
    }
}

// ── Instructions implementation ───────────────────────────────────────────────

impl Instructions for Sm83 {
    fn add8(&mut self, op: &Add8) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.operand);
        let (r, f) = add_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn add16(&mut self, op: &Add16) -> Result<u8, InstructionError> {
        let rr = match op.operand {
            Operand::Register16(r) => self.get_r16_enum(r),
            _ => return Err(InstructionError::InvalidOperand(alloc::format!("{:?}", op.operand))),
        };
        let hl = self.registers.hl();
        let (new_hl, new_flags) = add_u16(hl, rr);
        // ADD HL,rr preserves Z flag
        let mut f = self.registers.f;
        f.remove(Flags::N);
        f.set(Flags::H, new_flags.contains(Flags::H));
        f.set(Flags::C, new_flags.contains(Flags::C));
        self.registers.set_hl(new_hl);
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn add_sp16(&mut self, op: &AddSP16) -> Result<u8, InstructionError> {
        let e = self.fetch_byte() as i8;
        let (res, flags) = add_sp_u16(self.registers.sp, e);
        self.registers.sp = res;
        self.registers.f = flags;
        Ok(op.cycles)
    }

    fn adc(&mut self, op: &Adc) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.operand);
        let cy = self.registers.f.contains(Flags::C) as u8;
        let (r, f) = adc_u8(self.registers.a, val, cy);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn sub8(&mut self, op: &Sub8) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.operand);
        let (r, f) = sub_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn sbc8(&mut self, op: &Sbc8) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.operand);
        let cy = self.registers.f.contains(Flags::C) as u8;
        let (r, f) = sbc_u8(self.registers.a, val, cy);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn cp8(&mut self, op: &Cp8) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.operand);
        self.registers.f = cp_u8(self.registers.a, val);
        Ok(op.cycles)
    }

    fn ld8(&mut self, op: &Ld8) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.src);
        self.set_operand8(&op.dest, val);
        Ok(op.cycles)
    }

    fn ld16(&mut self, op: &Ld16) -> Result<u8, InstructionError> {
        match &op.op {
            Ld16Op::RrImm16 { dest } => {
                let lo = self.fetch_byte();
                let hi = self.fetch_byte();
                self.set_r16_enum(*dest, u16::from_le_bytes([lo, hi]));
            }
            Ld16Op::NnSp => {
                let lo = self.fetch_byte();
                let hi = self.fetch_byte();
                let addr = u16::from_le_bytes([lo, hi]);
                let sp = self.registers.sp;
                self.bus_write(addr, sp as u8);
                self.bus_write(addr.wrapping_add(1), (sp >> 8) as u8);
            }
            Ld16Op::SpHl => {
                self.registers.sp = self.registers.hl();
            }
            Ld16Op::HlSpE => {
                let e = self.fetch_byte() as i8;
                let (res, flags) = add_sp_u16(self.registers.sp, e);
                self.registers.set_hl(res);
                self.registers.f = flags;
            }
            Ld16Op::BcA => {
                let a = self.registers.bc();
                self.bus_write(a, self.registers.a);
            }
            Ld16Op::DeA => {
                let a = self.registers.de();
                self.bus_write(a, self.registers.a);
            }
            Ld16Op::ABc => {
                let a = self.registers.bc();
                self.registers.a = self.bus_read(a);
            }
            Ld16Op::ADe => {
                let a = self.registers.de();
                self.registers.a = self.bus_read(a);
            }
            Ld16Op::HliA => {
                let a = self.registers.hl();
                self.bus_write(a, self.registers.a);
                self.registers.set_hl(a.wrapping_add(1));
            }
            Ld16Op::HldA => {
                let a = self.registers.hl();
                self.bus_write(a, self.registers.a);
                self.registers.set_hl(a.wrapping_sub(1));
            }
            Ld16Op::AHli => {
                let a = self.registers.hl();
                self.registers.a = self.bus_read(a);
                self.registers.set_hl(a.wrapping_add(1));
            }
            Ld16Op::AHld => {
                let a = self.registers.hl();
                self.registers.a = self.bus_read(a);
                self.registers.set_hl(a.wrapping_sub(1));
            }
            Ld16Op::NnA => {
                let lo = self.fetch_byte();
                let hi = self.fetch_byte();
                let addr = u16::from_le_bytes([lo, hi]);
                self.bus_write(addr, self.registers.a);
            }
            Ld16Op::ANn => {
                let lo = self.fetch_byte();
                let hi = self.fetch_byte();
                let addr = u16::from_le_bytes([lo, hi]);
                self.registers.a = self.bus_read(addr);
            }
            Ld16Op::LdhNA => {
                let n = self.fetch_byte();
                self.bus_write(0xFF00 | (n as u16), self.registers.a);
            }
            Ld16Op::LdhAN => {
                let n = self.fetch_byte();
                self.registers.a = self.bus_read(0xFF00 | (n as u16));
            }
            Ld16Op::LdCA => {
                let c = self.registers.c;
                self.bus_write(0xFF00 | (c as u16), self.registers.a);
            }
            Ld16Op::LdAC => {
                let c = self.registers.c;
                self.registers.a = self.bus_read(0xFF00 | (c as u16));
            }
        }
        Ok(op.cycles)
    }

    fn inc8(&mut self, op: &Inc8) -> Result<u8, InstructionError> {
        match &op.operand {
            Operand::Register8(r) => {
                let (v, f) = inc_u8(self.get_r8_enum(*r), self.registers.f);
                self.set_r8_enum(*r, v);
                self.registers.f = f;
            }
            Operand::Memory(Memory::HL) => {
                let addr = self.registers.hl();
                let old = self.bus_read(addr);
                let (v, f) = inc_u8(old, self.registers.f);
                self.bus_write(addr, v);
                self.registers.f = f;
            }
            _ => return Err(InstructionError::InvalidOperand(alloc::format!("{:?}", op.operand))),
        }
        Ok(op.cycles)
    }

    fn dec8(&mut self, op: &Dec8) -> Result<u8, InstructionError> {
        match &op.operand {
            Operand::Register8(r) => {
                let (v, f) = dec_u8(self.get_r8_enum(*r), self.registers.f);
                self.set_r8_enum(*r, v);
                self.registers.f = f;
            }
            Operand::Memory(Memory::HL) => {
                let addr = self.registers.hl();
                let old = self.bus_read(addr);
                let (v, f) = dec_u8(old, self.registers.f);
                self.bus_write(addr, v);
                self.registers.f = f;
            }
            _ => return Err(InstructionError::InvalidOperand(alloc::format!("{:?}", op.operand))),
        }
        Ok(op.cycles)
    }

    fn inc16(&mut self, op: &Inc16) -> Result<u8, InstructionError> {
        let v = self.get_r16_enum(op.operand);
        self.set_r16_enum(op.operand, v.wrapping_add(1));
        Ok(op.cycles)
    }

    fn dec16(&mut self, op: &Dec16) -> Result<u8, InstructionError> {
        let v = self.get_r16_enum(op.operand);
        self.set_r16_enum(op.operand, v.wrapping_sub(1));
        Ok(op.cycles)
    }

    fn rotate_accumulator(&mut self, op: &Rotate) -> Result<u8, InstructionError> {
        let a = self.registers.a;
        let carry = self.registers.f.contains(Flags::C);
        let (result, mut f) = match op.op {
            RotateOp::Rlca => {
                let b7 = a >> 7;
                let r = (a << 1) | b7;
                let mut f = Flags::empty();
                f.set(Flags::C, b7 != 0);
                (r, f)
            }
            RotateOp::Rrca => {
                let b0 = a & 1;
                let r = (a >> 1) | (b0 << 7);
                let mut f = Flags::empty();
                f.set(Flags::C, b0 != 0);
                (r, f)
            }
            RotateOp::Rla => rl_u8(a, carry),
            RotateOp::Rra => rr_u8(a, carry),
        };
        // Accumulator rotates always clear Z
        f.remove(Flags::Z);
        self.registers.a = result;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn and8(&mut self, op: &And8) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.operand);
        let (r, f) = and_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn or8(&mut self, op: &Or8) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.operand);
        let (r, f) = or_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn xor8(&mut self, op: &Xor8) -> Result<u8, InstructionError> {
        let val = self.get_operand8(&op.operand);
        let (r, f) = xor_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn jump(&mut self, op: &Jump) -> Result<u8, InstructionError> {
        match &op.op {
            JumpOp::Jp => {
                let lo = self.fetch_byte();
                let hi = self.fetch_byte();
                self.registers.pc = u16::from_le_bytes([lo, hi]);
            }
            JumpOp::JpHl => {
                self.registers.pc = self.registers.hl();
            }
            JumpOp::JpCc(cond) => {
                let lo = self.fetch_byte();
                let hi = self.fetch_byte();
                if self.check_condition(cond) {
                    self.registers.pc = u16::from_le_bytes([lo, hi]);
                    return Ok(op.cycles); // taken: 16 cycles
                }
                return Ok(12); // not taken: 12 cycles
            }
            JumpOp::Jr => {
                let e = self.fetch_byte() as i8 as i16 as u16;
                self.registers.pc = self.registers.pc.wrapping_add(e);
            }
            JumpOp::JrCc(cond) => {
                let e = self.fetch_byte() as i8 as i16 as u16;
                if self.check_condition(cond) {
                    self.registers.pc = self.registers.pc.wrapping_add(e);
                    return Ok(op.cycles); // taken: 12 cycles
                }
                return Ok(8); // not taken: 8 cycles
            }
        }
        Ok(op.cycles)
    }

    fn misc(&mut self, op: &Misc) -> Result<u8, InstructionError> {
        match op.op {
            MiscOp::Nop => {}
            MiscOp::Halt => {
                self.halted = true;
            }
            MiscOp::Stop => {
                // Simplified: treat as halt
                self.halted = true;
            }
            MiscOp::Daa => {
                let (r, f) = daa_u8(self.registers.a, self.registers.f);
                self.registers.a = r;
                self.registers.f = f;
            }
            MiscOp::Cpl => {
                self.registers.a = !self.registers.a;
                self.registers.f.insert(Flags::N | Flags::H);
            }
            MiscOp::Scf => {
                self.registers.f.remove(Flags::N | Flags::H);
                self.registers.f.insert(Flags::C);
            }
            MiscOp::Ccf => {
                let c = self.registers.f.contains(Flags::C);
                self.registers.f.remove(Flags::N | Flags::H);
                self.registers.f.set(Flags::C, !c);
            }
            MiscOp::Di => {
                self.ime = ImeState::Disabled;
            }
            MiscOp::Ei => {
                self.ime = ImeState::Pending;
            }
        }
        Ok(op.cycles)
    }

    fn push16(&mut self, op: &Push16) -> Result<u8, InstructionError> {
        let idx = match op.operand {
            Register16::BC => 0,
            Register16::DE => 1,
            Register16::HL => 2,
            Register16::AF => 3,
            Register16::SP => unreachable!("PUSH SP invalid"),
        };
        let val = self.r16stk_get(idx);
        self.push16_inner(val);
        Ok(op.cycles)
    }

    fn pop16(&mut self, op: &Pop16) -> Result<u8, InstructionError> {
        let val = self.pop16_inner();
        let idx = match op.operand {
            Register16::BC => 0,
            Register16::DE => 1,
            Register16::HL => 2,
            Register16::AF => 3,
            Register16::SP => 3,
        };
        self.r16stk_set(idx, val);
        Ok(op.cycles)
    }

    fn call(&mut self, op: &Call) -> Result<u8, InstructionError> {
        let lo = self.fetch_byte();
        let hi = self.fetch_byte();
        let target = u16::from_le_bytes([lo, hi]);
        let take = match &op.op {
            CallOp::Call => true,
            CallOp::CallCc(cond) => self.check_condition(cond),
        };
        if take {
            let ret = self.registers.pc;
            self.push16_inner(ret);
            self.registers.pc = target;
            Ok(op.cycles) // taken: 24 cycles
        } else {
            Ok(12) // not taken: 12 cycles
        }
    }

    fn ret(&mut self, op: &Ret) -> Result<u8, InstructionError> {
        match &op.op {
            RetOp::Ret => {
                let addr = self.pop16_inner();
                self.registers.pc = addr;
            }
            RetOp::RetCc(cond) => {
                if self.check_condition(cond) {
                    let addr = self.pop16_inner();
                    self.registers.pc = addr;
                    return Ok(op.cycles); // taken: 20 cycles
                }
                return Ok(8); // not taken: 8 cycles
            }
            RetOp::Reti => {
                let addr = self.pop16_inner();
                self.registers.pc = addr;
                self.ime = ImeState::Enabled; // RETI re-enables immediately (no delay)
            }
        }
        Ok(op.cycles)
    }

    fn rst(&mut self, op: &Rst) -> Result<u8, InstructionError> {
        let ret = self.registers.pc;
        self.push16_inner(ret);
        self.registers.pc = op.vector as u16;
        Ok(op.cycles)
    }

    fn cb(&mut self, op: &CbInstruction) -> Result<u8, InstructionError> {
        let val = match op.target {
            CbTarget::Reg(r) => self.get_r8_enum(r),
            CbTarget::HLMem => {
                let a = self.registers.hl();
                self.bus_read(a)
            }
        };
        let carry = self.registers.f.contains(Flags::C);
        match op.op {
            CbOp::Rlc  => { let (r, f) = rlc_u8(val);       self.set_cb_result(op.target, r, f); }
            CbOp::Rrc  => { let (r, f) = rrc_u8(val);       self.set_cb_result(op.target, r, f); }
            CbOp::Rl   => { let (r, f) = rl_u8(val, carry); self.set_cb_result(op.target, r, f); }
            CbOp::Rr   => { let (r, f) = rr_u8(val, carry); self.set_cb_result(op.target, r, f); }
            CbOp::Sla  => { let (r, f) = sla_u8(val);       self.set_cb_result(op.target, r, f); }
            CbOp::Sra  => { let (r, f) = sra_u8(val);       self.set_cb_result(op.target, r, f); }
            CbOp::Swap => { let (r, f) = swap_u8(val);      self.set_cb_result(op.target, r, f); }
            CbOp::Srl  => { let (r, f) = srl_u8(val);       self.set_cb_result(op.target, r, f); }
            CbOp::Bit(b) => {
                // BIT does not write back; only updates flags
                let f = bit_u8(val, b, self.registers.f);
                self.registers.f = f;
            }
            CbOp::Res(b) => {
                let r = res_u8(val, b);
                self.write_cb_target(op.target, r);
            }
            CbOp::Set(b) => {
                let r = set_u8(val, b);
                self.write_cb_target(op.target, r);
            }
        }
        Ok(op.cycles)
    }
}

#[cfg(test)]
mod tests {
    use alloc::{vec, vec::Vec};
    use crate::cpu::registers::{Flags, Registers};
    use crate::gameboy::GameBoy;
    use crate::memory::memory::{GameBoyMemory, Memory};

    fn make_test_cpu(rom_data: Vec<u8>) -> GameBoy {
        GameBoy::for_test(rom_data)
    }

    fn make_test_cpu_with_memory(
        setup: impl FnOnce(&mut GameBoyMemory),
        rom_data: Vec<u8>,
    ) -> GameBoy {
        GameBoy::for_test_with_setup(setup, rom_data)
    }

    /// Add a constant to the accumulator register and expect the register's value to be the
    /// appropriate value.
    #[test]
    fn test_add8_imm8_to_accumlator() {
        let mut cpu = make_test_cpu(vec![0xC6, 0x03]);
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x03);
        assert_eq!(cpu.registers().f, Flags::empty())
    }

    #[test]
    fn test_add8_imm8_to_accumlator_sum_zero() {
        let mut cpu = make_test_cpu(vec![0xC6, 0x00]);
        let cycles = cpu.step().unwrap();

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
        let cycles = cpu.step().unwrap();

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
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().a, 0x0A);
        assert_eq!(cpu.registers().f, Flags::empty());
    }
}
