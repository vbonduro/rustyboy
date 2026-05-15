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
use super::perf::{cyccnt, Sm83PerfRecorder};
use super::registers::{Flags, Registers};

#[cfg(feature = "perf")]
pub use super::perf::Sm83PerfProfile;
use crate::memory::map::{IO_REG_BASE, IO_REG_END, OAM_BASE, OAM_END, VRAM_BASE, VRAM_END};
use crate::memory::memory::GameBoyMemory;

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

    #[cfg(feature = "perf")]
    #[inline(always)]
    fn finish_step(&mut self, total_start: u32, cycles: u32) -> u32 {
        self.perf.record_total(cyccnt().wrapping_sub(total_start));
        cycles
    }

    #[cfg(not(feature = "perf"))]
    #[inline(always)]
    fn finish_step(&mut self, _total_start: (), cycles: u32) -> u32 {
        cycles
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn advance_ime(&mut self) {
        if self.ime == ImeState::Pending {
            self.ime = ImeState::Enabled;
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn has_pending_interrupt(&self, memory: &GameBoyMemory) -> bool {
        memory.ie() & memory.read_io(0xFF0F) != 0
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn take_pending_interrupt(&mut self, memory: &mut GameBoyMemory) -> Option<u8> {
        let ie = memory.ie();
        let if_ = memory.read_io(0xFF0F);
        let pending = ie & if_;
        if pending == 0 {
            return None;
        }
        let bit = pending.trailing_zeros() as u8;
        let new_if = if_ & !(1 << bit);
        memory.write_io(0xFF0F, new_if);
        Some(bit)
    }

    // ── Register encoding helpers ─────────────────────────────────────────────

    /// r16 pairs for stack instructions: 0=BC 1=DE 2=HL 3=AF.
    fn r16stk_get(&self, r: u8) -> u16 {
        match r {
            0 => self.registers.bc(),
            1 => self.registers.de(),
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
            0 => self.registers.set_bc(v),
            1 => self.registers.set_de(v),
            2 => self.registers.set_hl(v),
            _ => {
                self.registers.a = (v >> 8) as u8;
                self.registers.f = Flags::from_bits_truncate(v as u8);
            }
        }
    }

    // ── Bus access helpers ────────────────────────────────────────────────────

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn bus_read(&mut self, memory: &GameBoyMemory, addr: u16) -> u8 {
        #[cfg(feature = "perf")]
        let read_start = cyccnt();
        #[cfg(feature = "perf")]
        let nested = self.perf.nested_snapshot();

        let value = memory.read_fast(addr);

        #[cfg(feature = "perf")]
        {
            let dt = cyccnt()
                .wrapping_sub(read_start)
                .wrapping_sub(self.perf.nested_cycles_since(nested));
            self.perf.record_mem_read(dt);
            self.perf.record_bus_read(dt);
        }

        value
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn bus_write(&mut self, memory: &mut GameBoyMemory, addr: u16, val: u8) {
        #[cfg(feature = "perf")]
        let write_start = cyccnt();
        #[cfg(feature = "perf")]
        let nested = self.perf.nested_snapshot();

        match addr {
            IO_REG_BASE..=IO_REG_END | 0xFFFF => {
                #[cfg(feature = "perf")]
                let io_start = cyccnt();
                memory.write_io(addr, val);
                #[cfg(feature = "perf")]
                self.perf
                    .record_mem_write_io(cyccnt().wrapping_sub(io_start));

                if matches!(addr, IO_REG_BASE..=IO_REG_END | 0xFFFF) {
                    #[cfg(feature = "perf")]
                    let enqueue_start = cyccnt();
                    memory.enqueue_bus_event(addr, val);
                    #[cfg(feature = "perf")]
                    self.perf
                        .record_mem_write_enqueue(cyccnt().wrapping_sub(enqueue_start));
                }
            }
            VRAM_BASE..=VRAM_END | OAM_BASE..=OAM_END => {
                #[cfg(feature = "perf")]
                let fast_start = cyccnt();
                memory.write_fast(addr, val);
                #[cfg(feature = "perf")]
                self.perf
                    .record_mem_write_fast(addr, cyccnt().wrapping_sub(fast_start));

                #[cfg(feature = "perf")]
                let enqueue_start = cyccnt();
                memory.enqueue_bus_event(addr, val);
                #[cfg(feature = "perf")]
                self.perf
                    .record_mem_write_enqueue(cyccnt().wrapping_sub(enqueue_start));
            }
            _ => {
                #[cfg(feature = "perf")]
                let fast_start = cyccnt();
                memory.write_fast(addr, val);
                #[cfg(feature = "perf")]
                self.perf
                    .record_mem_write_fast(addr, cyccnt().wrapping_sub(fast_start));
            }
        }

        #[cfg(feature = "perf")]
        {
            let dt = cyccnt()
                .wrapping_sub(write_start)
                .wrapping_sub(self.perf.nested_cycles_since(nested));
            self.perf.record_mem_write(dt);
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn fetch_byte(&mut self, memory: &mut GameBoyMemory) -> u8 {
        let addr = self.registers.pc;
        #[cfg(feature = "perf")]
        let fetch_start = cyccnt();
        #[cfg(feature = "perf")]
        let nested = self.perf.nested_snapshot();

        let value = if addr <= 0x7FFF {
            #[cfg(feature = "perf")]
            let idle_start = cyccnt();
            #[cfg(feature = "perf")]
            let read_start = cyccnt();

            let value = memory.read_rom_fast(addr);

            #[cfg(feature = "perf")]
            {
                self.perf
                    .record_pc_fetch_rom_read(cyccnt().wrapping_sub(read_start));
                self.perf
                    .record_pc_fetch_rom_idle(cyccnt().wrapping_sub(idle_start));
            }

            value
        } else {
            memory.read_fast(addr)
        };

        #[cfg(feature = "perf")]
        let wrap_start = cyccnt();
        self.registers.pc = self.registers.pc.wrapping_add(1);

        #[cfg(feature = "perf")]
        {
            self.perf
                .record_pc_fetch_wrapper(cyccnt().wrapping_sub(wrap_start));
            let dt = cyccnt()
                .wrapping_sub(fetch_start)
                .wrapping_sub(self.perf.nested_cycles_since(nested));
            self.perf.record_pc_fetch(addr, dt);
        }

        value
    }

    // ── Instruction-level step interface ─────────────────────────────────────

    /// Execute one complete instruction. Returns T-cycles elapsed.
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    pub fn step(&mut self, memory: &mut GameBoyMemory) -> u32 {
        self.step_inner(memory)
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn step_inner(&mut self, memory: &mut GameBoyMemory) -> u32 {
        #[cfg(feature = "perf")]
        let total_start = cyccnt();
        #[cfg(not(feature = "perf"))]
        let total_start = ();

        self.advance_ime();

        if self.halted {
            if self.has_pending_interrupt(memory) {
                self.halted = false;
                if self.ime == ImeState::Enabled {
                    if let Some(bit) = self.take_pending_interrupt(memory) {
                        let isr_cycles = self.dispatch_isr(memory, bit);
                        return self.finish_step(total_start, isr_cycles);
                    }
                }
                // IME=false: unhalt, fall through to execute next instruction
            } else {
                return self.finish_step(total_start, 4); // one NOP-equivalent cycle while halted
            }
        }

        let opcode = self.fetch_byte(memory);
        if opcode == 0xCB {
            #[cfg(feature = "perf")]
            let cb_start = cyccnt();
            let cb_opcode = self.fetch_byte(memory);
            #[cfg(feature = "perf")]
            let dispatch_start = cyccnt();
            let handler = match self.opcodes.get_cb(cb_opcode) {
                Ok(h) => h,
                Err(_) => return self.finish_step(total_start, 8),
            };
            #[cfg(feature = "perf")]
            {
                self.perf
                    .record_opcode_dispatch(cyccnt().wrapping_sub(dispatch_start));
                self.perf.record_cb_prefix(cyccnt().wrapping_sub(cb_start));
            }
            #[cfg(feature = "perf")]
            let cb_exec_start = cyccnt();
            #[cfg(feature = "perf")]
            let cb_exec_nested = self.perf.nested_snapshot();
            let cycles = handler.execute(self, memory).unwrap_or(8) as u32;
            #[cfg(feature = "perf")]
            {
                let dt = cyccnt()
                    .wrapping_sub(cb_exec_start)
                    .wrapping_sub(self.perf.nested_cycles_since(cb_exec_nested));
                self.perf.record_cb_opcode_exec(cb_opcode, dt);
            }
            if self.ime == ImeState::Enabled {
                if let Some(bit) = self.take_pending_interrupt(memory) {
                    let isr_cycles = self.dispatch_isr(memory, bit);
                    return self.finish_step(total_start, cycles + isr_cycles);
                }
            }
            return self.finish_step(total_start, cycles);
        }

        #[cfg(feature = "perf")]
        let dispatch_start = cyccnt();
        let handler = match self.opcodes.get(opcode) {
            Ok(h) => h,
            Err(_) => return self.finish_step(total_start, 4),
        };
        #[cfg(feature = "perf")]
        self.perf
            .record_opcode_dispatch(cyccnt().wrapping_sub(dispatch_start));
        #[cfg(feature = "perf")]
        let exec_start = cyccnt();
        #[cfg(feature = "perf")]
        let exec_nested = self.perf.nested_snapshot();
        let cycles = handler.execute(self, memory).unwrap_or(4) as u32;
        #[cfg(feature = "perf")]
        {
            let dt = cyccnt()
                .wrapping_sub(exec_start)
                .wrapping_sub(self.perf.nested_cycles_since(exec_nested));
            self.perf.record_opcode_exec(opcode, dt);
        }
        // Post-instruction: check for interrupt dispatch
        if self.ime == ImeState::Enabled {
            if let Some(bit) = self.take_pending_interrupt(memory) {
                let isr_cycles = self.dispatch_isr(memory, bit);
                return self.finish_step(total_start, cycles + isr_cycles);
            }
        }
        self.finish_step(total_start, cycles)
    }

    /// Dispatch an interrupt service routine. Returns T-cycles for the ISR dispatch (20).
    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn dispatch_isr(&mut self, memory: &mut GameBoyMemory, bit: u8) -> u32 {
        self.ime = ImeState::Disabled;
        let sp = self.registers.sp;
        let pc = self.registers.pc;
        self.bus_write(memory, sp.wrapping_sub(1), (pc >> 8) as u8);
        self.bus_write(memory, sp.wrapping_sub(2), pc as u8);
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

    fn get_operand8(&mut self, op: &Operand, memory: &mut GameBoyMemory) -> u8 {
        #[cfg(feature = "perf")]
        let operand_start = cyccnt();
        #[cfg(feature = "perf")]
        let nested = self.perf.nested_snapshot();

        let value = match op {
            Operand::Register8(r) => self.get_r8_enum(*r),
            Operand::Memory(Memory::HL) => {
                let a = self.registers.hl();
                self.bus_read(memory, a)
            }
            Operand::Memory(Memory::BC) => {
                let a = self.registers.bc();
                self.bus_read(memory, a)
            }
            Operand::Memory(Memory::DE) => {
                let a = self.registers.de();
                self.bus_read(memory, a)
            }
            Operand::Memory(Memory::HLI) => {
                let a = self.registers.hl();
                let v = self.bus_read(memory, a);
                self.registers.set_hl(a.wrapping_add(1));
                v
            }
            Operand::Memory(Memory::HLD) => {
                let a = self.registers.hl();
                let v = self.bus_read(memory, a);
                self.registers.set_hl(a.wrapping_sub(1));
                v
            }
            Operand::Imm8 | Operand::ImmSigned8 => self.fetch_byte(memory),
            _ => panic!("get_operand8: unsupported {:?}", op),
        };

        #[cfg(feature = "perf")]
        {
            let dt = cyccnt()
                .wrapping_sub(operand_start)
                .wrapping_sub(self.perf.nested_cycles_since(nested));
            match op {
                Operand::Register8(_) => self.perf.record_operand8_reg(dt),
                Operand::Imm8 | Operand::ImmSigned8 => self.perf.record_operand8_imm(dt),
                Operand::Memory(_) => self.perf.record_operand8_mem(dt),
                _ => unreachable!("get_operand8 perf classification drifted from operand match"),
            }
        }

        value
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn fetch_operand8_immediate(&mut self, memory: &mut GameBoyMemory) -> u8 {
        #[cfg(feature = "perf")]
        let operand_start = cyccnt();
        #[cfg(feature = "perf")]
        let nested = self.perf.nested_snapshot();

        let value = self.fetch_byte(memory);

        #[cfg(feature = "perf")]
        {
            let dt = cyccnt()
                .wrapping_sub(operand_start)
                .wrapping_sub(self.perf.nested_cycles_since(nested));
            self.perf.record_operand8_imm(dt);
        }

        value
    }

    fn set_operand8(&mut self, op: &Operand, val: u8, memory: &mut GameBoyMemory) {
        match op {
            Operand::Register8(r) => self.set_r8_enum(*r, val),
            Operand::Memory(Memory::HL) => {
                let a = self.registers.hl();
                self.bus_write(memory, a, val);
            }
            Operand::Memory(Memory::BC) => {
                let a = self.registers.bc();
                self.bus_write(memory, a, val);
            }
            Operand::Memory(Memory::DE) => {
                let a = self.registers.de();
                self.bus_write(memory, a, val);
            }
            Operand::Memory(Memory::HLI) => {
                let a = self.registers.hl();
                self.bus_write(memory, a, val);
                self.registers.set_hl(a.wrapping_add(1));
            }
            Operand::Memory(Memory::HLD) => {
                let a = self.registers.hl();
                self.bus_write(memory, a, val);
                self.registers.set_hl(a.wrapping_sub(1));
            }
            _ => panic!("set_operand8: unsupported {:?}", op),
        }
    }

    fn check_condition(&self, cond: &Condition) -> bool {
        match cond {
            Condition::NZ => !self.registers.f.contains(Flags::Z),
            Condition::Z => self.registers.f.contains(Flags::Z),
            Condition::NC => !self.registers.f.contains(Flags::C),
            Condition::C => self.registers.f.contains(Flags::C),
        }
    }

    fn pop16_inner(&mut self, memory: &mut GameBoyMemory) -> u16 {
        let lo = self.bus_read(memory, self.registers.sp);
        let hi = self.bus_read(memory, self.registers.sp.wrapping_add(1));
        self.registers.sp = self.registers.sp.wrapping_add(2);
        u16::from_le_bytes([lo, hi])
    }

    fn push16_inner(&mut self, val: u16, memory: &mut GameBoyMemory) {
        self.registers.sp = self.registers.sp.wrapping_sub(2);
        self.bus_write(memory, self.registers.sp.wrapping_add(1), (val >> 8) as u8);
        self.bus_write(memory, self.registers.sp, val as u8);
    }

    // ── CB helpers ────────────────────────────────────────────────────────────

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn cb_reg(&mut self, op: CbOp, reg: Register8) {
        let val = self.get_r8_enum(reg);
        let carry = self.registers.f.contains(Flags::C);
        match op {
            CbOp::Rlc => {
                let (r, f) = rlc_u8(val);
                self.set_r8_enum(reg, r);
                self.registers.f = f;
            }
            CbOp::Rrc => {
                let (r, f) = rrc_u8(val);
                self.set_r8_enum(reg, r);
                self.registers.f = f;
            }
            CbOp::Rl => {
                let (r, f) = rl_u8(val, carry);
                self.set_r8_enum(reg, r);
                self.registers.f = f;
            }
            CbOp::Rr => {
                let (r, f) = rr_u8(val, carry);
                self.set_r8_enum(reg, r);
                self.registers.f = f;
            }
            CbOp::Sla => {
                let (r, f) = sla_u8(val);
                self.set_r8_enum(reg, r);
                self.registers.f = f;
            }
            CbOp::Sra => {
                let (r, f) = sra_u8(val);
                self.set_r8_enum(reg, r);
                self.registers.f = f;
            }
            CbOp::Swap => {
                let (r, f) = swap_u8(val);
                self.set_r8_enum(reg, r);
                self.registers.f = f;
            }
            CbOp::Srl => {
                let (r, f) = srl_u8(val);
                self.set_r8_enum(reg, r);
                self.registers.f = f;
            }
            CbOp::Bit(b) => {
                self.registers.f = bit_u8(val, b, self.registers.f);
            }
            CbOp::Res(b) => {
                self.set_r8_enum(reg, res_u8(val, b));
            }
            CbOp::Set(b) => {
                self.set_r8_enum(reg, set_u8(val, b));
            }
        }
    }

    #[cfg_attr(target_arch = "arm", link_section = ".data")]
    fn cb_hlmem(&mut self, op: CbOp, memory: &mut GameBoyMemory) {
        let addr = self.registers.hl();
        let val = self.bus_read(memory, addr);
        let carry = self.registers.f.contains(Flags::C);
        match op {
            CbOp::Rlc => {
                let (r, f) = rlc_u8(val);
                self.bus_write(memory, addr, r);
                self.registers.f = f;
            }
            CbOp::Rrc => {
                let (r, f) = rrc_u8(val);
                self.bus_write(memory, addr, r);
                self.registers.f = f;
            }
            CbOp::Rl => {
                let (r, f) = rl_u8(val, carry);
                self.bus_write(memory, addr, r);
                self.registers.f = f;
            }
            CbOp::Rr => {
                let (r, f) = rr_u8(val, carry);
                self.bus_write(memory, addr, r);
                self.registers.f = f;
            }
            CbOp::Sla => {
                let (r, f) = sla_u8(val);
                self.bus_write(memory, addr, r);
                self.registers.f = f;
            }
            CbOp::Sra => {
                let (r, f) = sra_u8(val);
                self.bus_write(memory, addr, r);
                self.registers.f = f;
            }
            CbOp::Swap => {
                let (r, f) = swap_u8(val);
                self.bus_write(memory, addr, r);
                self.registers.f = f;
            }
            CbOp::Srl => {
                let (r, f) = srl_u8(val);
                self.bus_write(memory, addr, r);
                self.registers.f = f;
            }
            CbOp::Bit(b) => {
                self.registers.f = bit_u8(val, b, self.registers.f);
            }
            CbOp::Res(b) => {
                self.bus_write(memory, addr, res_u8(val, b));
            }
            CbOp::Set(b) => {
                self.bus_write(memory, addr, set_u8(val, b));
            }
        }
    }
}

// ── Instructions implementation ───────────────────────────────────────────────

impl Instructions for Sm83 {
    fn add8(&mut self, op: &Add8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = match op.operand {
            Operand::Imm8 => self.fetch_operand8_immediate(memory),
            _ => self.get_operand8(&op.operand, memory),
        };
        let (r, f) = add_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn add16(&mut self, op: &Add16) -> Result<u8, InstructionError> {
        let rr = match op.operand {
            Operand::Register16(r) => self.get_r16_enum(r),
            _ => {
                return Err(InstructionError::InvalidOperand(alloc::format!(
                    "{:?}", op.operand
                )))
            }
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

    fn add_sp16(
        &mut self,
        op: &AddSP16,
        memory: &mut GameBoyMemory,
    ) -> Result<u8, InstructionError> {
        let e = self.fetch_byte(memory) as i8;
        let (res, flags) = add_sp_u16(self.registers.sp, e);
        self.registers.sp = res;
        self.registers.f = flags;
        Ok(op.cycles)
    }

    fn adc(&mut self, op: &Adc, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = match op.operand {
            Operand::Imm8 => self.fetch_operand8_immediate(memory),
            _ => self.get_operand8(&op.operand, memory),
        };
        let cy = self.registers.f.contains(Flags::C) as u8;
        let (r, f) = adc_u8(self.registers.a, val, cy);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn sub8(&mut self, op: &Sub8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = match op.operand {
            Operand::Imm8 => self.fetch_operand8_immediate(memory),
            _ => self.get_operand8(&op.operand, memory),
        };
        let (r, f) = sub_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn sbc8(&mut self, op: &Sbc8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = match op.operand {
            Operand::Imm8 => self.fetch_operand8_immediate(memory),
            _ => self.get_operand8(&op.operand, memory),
        };
        let cy = self.registers.f.contains(Flags::C) as u8;
        let (r, f) = sbc_u8(self.registers.a, val, cy);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn cp8(&mut self, op: &Cp8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = match op.operand {
            Operand::Imm8 => self.fetch_operand8_immediate(memory),
            _ => self.get_operand8(&op.operand, memory),
        };
        self.registers.f = cp_u8(self.registers.a, val);
        Ok(op.cycles)
    }

    fn ld8(&mut self, op: &Ld8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        if op.src == Operand::Imm8 {
            let val = self.fetch_operand8_immediate(memory);
            match op.dest {
                Operand::Register8(r) => self.set_r8_enum(r, val),
                Operand::Memory(Memory::HL) => {
                    let addr = self.registers.hl();
                    self.bus_write(memory, addr, val);
                }
                _ => {
                    return Err(InstructionError::InvalidOperand(alloc::format!(
                        "{:?}", op.dest
                    )))
                }
            }
            return Ok(op.cycles);
        }

        let val = self.get_operand8(&op.src, memory);
        self.set_operand8(&op.dest, val, memory);
        Ok(op.cycles)
    }

    fn ld16(&mut self, op: &Ld16, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        match &op.op {
            Ld16Op::RrImm16 { dest } => {
                let lo = self.fetch_byte(memory);
                let hi = self.fetch_byte(memory);
                self.set_r16_enum(*dest, u16::from_le_bytes([lo, hi]));
            }
            Ld16Op::NnSp => {
                let lo = self.fetch_byte(memory);
                let hi = self.fetch_byte(memory);
                let addr = u16::from_le_bytes([lo, hi]);
                let sp = self.registers.sp;
                self.bus_write(memory, addr, sp as u8);
                self.bus_write(memory, addr.wrapping_add(1), (sp >> 8) as u8);
            }
            Ld16Op::SpHl => {
                self.registers.sp = self.registers.hl();
            }
            Ld16Op::HlSpE => {
                let e = self.fetch_byte(memory) as i8;
                let (res, flags) = add_sp_u16(self.registers.sp, e);
                self.registers.set_hl(res);
                self.registers.f = flags;
            }
            Ld16Op::BcA => {
                let a = self.registers.bc();
                self.bus_write(memory, a, self.registers.a);
            }
            Ld16Op::DeA => {
                let a = self.registers.de();
                self.bus_write(memory, a, self.registers.a);
            }
            Ld16Op::ABc => {
                let a = self.registers.bc();
                self.registers.a = self.bus_read(memory, a);
            }
            Ld16Op::ADe => {
                let a = self.registers.de();
                self.registers.a = self.bus_read(memory, a);
            }
            Ld16Op::HliA => {
                let a = self.registers.hl();
                self.bus_write(memory, a, self.registers.a);
                self.registers.set_hl(a.wrapping_add(1));
            }
            Ld16Op::HldA => {
                let a = self.registers.hl();
                self.bus_write(memory, a, self.registers.a);
                self.registers.set_hl(a.wrapping_sub(1));
            }
            Ld16Op::AHli => {
                let a = self.registers.hl();
                self.registers.a = self.bus_read(memory, a);
                self.registers.set_hl(a.wrapping_add(1));
            }
            Ld16Op::AHld => {
                let a = self.registers.hl();
                self.registers.a = self.bus_read(memory, a);
                self.registers.set_hl(a.wrapping_sub(1));
            }
            Ld16Op::NnA => {
                let lo = self.fetch_byte(memory);
                let hi = self.fetch_byte(memory);
                let addr = u16::from_le_bytes([lo, hi]);
                self.bus_write(memory, addr, self.registers.a);
            }
            Ld16Op::ANn => {
                let lo = self.fetch_byte(memory);
                let hi = self.fetch_byte(memory);
                let addr = u16::from_le_bytes([lo, hi]);
                self.registers.a = self.bus_read(memory, addr);
            }
            Ld16Op::LdhNA => {
                let n = self.fetch_byte(memory);
                self.bus_write(memory, IO_REG_BASE | (n as u16), self.registers.a);
            }
            Ld16Op::LdhAN => {
                let n = self.fetch_byte(memory);
                self.registers.a = self.bus_read(memory, IO_REG_BASE | (n as u16));
            }
            Ld16Op::LdCA => {
                let c = self.registers.c;
                self.bus_write(memory, IO_REG_BASE | (c as u16), self.registers.a);
            }
            Ld16Op::LdAC => {
                let c = self.registers.c;
                self.registers.a = self.bus_read(memory, IO_REG_BASE | (c as u16));
            }
        }
        Ok(op.cycles)
    }

    fn inc8(&mut self, op: &Inc8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        match &op.operand {
            Operand::Register8(r) => {
                let (v, f) = inc_u8(self.get_r8_enum(*r), self.registers.f);
                self.set_r8_enum(*r, v);
                self.registers.f = f;
            }
            Operand::Memory(Memory::HL) => {
                let addr = self.registers.hl();
                let old = self.bus_read(memory, addr);
                let (v, f) = inc_u8(old, self.registers.f);
                self.bus_write(memory, addr, v);
                self.registers.f = f;
            }
            _ => {
                return Err(InstructionError::InvalidOperand(alloc::format!(
                    "{:?}", op.operand
                )))
            }
        }
        Ok(op.cycles)
    }

    fn dec8(&mut self, op: &Dec8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        match &op.operand {
            Operand::Register8(r) => {
                let (v, f) = dec_u8(self.get_r8_enum(*r), self.registers.f);
                self.set_r8_enum(*r, v);
                self.registers.f = f;
            }
            Operand::Memory(Memory::HL) => {
                let addr = self.registers.hl();
                let old = self.bus_read(memory, addr);
                let (v, f) = dec_u8(old, self.registers.f);
                self.bus_write(memory, addr, v);
                self.registers.f = f;
            }
            _ => {
                return Err(InstructionError::InvalidOperand(alloc::format!(
                    "{:?}", op.operand
                )))
            }
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

    fn and8(&mut self, op: &And8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = match op.operand {
            Operand::Imm8 => self.fetch_operand8_immediate(memory),
            _ => self.get_operand8(&op.operand, memory),
        };
        let (r, f) = and_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn or8(&mut self, op: &Or8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = match op.operand {
            Operand::Imm8 => self.fetch_operand8_immediate(memory),
            _ => self.get_operand8(&op.operand, memory),
        };
        let (r, f) = or_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn xor8(&mut self, op: &Xor8, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = match op.operand {
            Operand::Imm8 => self.fetch_operand8_immediate(memory),
            _ => self.get_operand8(&op.operand, memory),
        };
        let (r, f) = xor_u8(self.registers.a, val);
        self.registers.a = r;
        self.registers.f = f;
        Ok(op.cycles)
    }

    fn jump(&mut self, op: &Jump, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        match &op.op {
            JumpOp::Jp => {
                let lo = self.fetch_byte(memory);
                let hi = self.fetch_byte(memory);
                self.registers.pc = u16::from_le_bytes([lo, hi]);
            }
            JumpOp::JpHl => {
                self.registers.pc = self.registers.hl();
            }
            JumpOp::JpCc(cond) => {
                let lo = self.fetch_byte(memory);
                let hi = self.fetch_byte(memory);
                if self.check_condition(cond) {
                    self.registers.pc = u16::from_le_bytes([lo, hi]);
                    return Ok(op.cycles); // taken: 16 cycles
                }
                return Ok(12); // not taken: 12 cycles
            }
            JumpOp::Jr => {
                let e = self.fetch_byte(memory) as i8 as i16 as u16;
                self.registers.pc = self.registers.pc.wrapping_add(e);
            }
            JumpOp::JrCc(cond) => {
                let e = self.fetch_byte(memory) as i8 as i16 as u16;
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

    fn push16(&mut self, op: &Push16, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let idx = match op.operand {
            Register16::BC => 0,
            Register16::DE => 1,
            Register16::HL => 2,
            Register16::AF => 3,
            Register16::SP => unreachable!("PUSH SP invalid"),
        };
        let val = self.r16stk_get(idx);
        self.push16_inner(val, memory);
        Ok(op.cycles)
    }

    fn pop16(&mut self, op: &Pop16, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let val = self.pop16_inner(memory);
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

    fn call(&mut self, op: &Call, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let lo = self.fetch_byte(memory);
        let hi = self.fetch_byte(memory);
        let target = u16::from_le_bytes([lo, hi]);
        let take = match &op.op {
            CallOp::Call => true,
            CallOp::CallCc(cond) => self.check_condition(cond),
        };
        if take {
            let ret = self.registers.pc;
            self.push16_inner(ret, memory);
            self.registers.pc = target;
            Ok(op.cycles) // taken: 24 cycles
        } else {
            Ok(12) // not taken: 12 cycles
        }
    }

    fn ret(&mut self, op: &Ret, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        match &op.op {
            RetOp::Ret => {
                let addr = self.pop16_inner(memory);
                self.registers.pc = addr;
            }
            RetOp::RetCc(cond) => {
                if self.check_condition(cond) {
                    let addr = self.pop16_inner(memory);
                    self.registers.pc = addr;
                    return Ok(op.cycles); // taken: 20 cycles
                }
                return Ok(8); // not taken: 8 cycles
            }
            RetOp::Reti => {
                let addr = self.pop16_inner(memory);
                self.registers.pc = addr;
                self.ime = ImeState::Enabled; // RETI re-enables immediately (no delay)
            }
        }
        Ok(op.cycles)
    }

    fn rst(&mut self, op: &Rst, memory: &mut GameBoyMemory) -> Result<u8, InstructionError> {
        let ret = self.registers.pc;
        self.push16_inner(ret, memory);
        self.registers.pc = op.vector as u16;
        Ok(op.cycles)
    }

    fn cb(
        &mut self,
        op: &CbInstruction,
        memory: &mut GameBoyMemory,
    ) -> Result<u8, InstructionError> {
        #[cfg(feature = "perf")]
        let cb_exec_start = cyccnt();
        #[cfg(feature = "perf")]
        let nested = self.perf.nested_snapshot();
        #[cfg(feature = "perf")]
        let is_reg_target = match op.target {
            CbTarget::Reg(r) => {
                self.cb_reg(op.op, r);
                true
            }
            CbTarget::HLMem => {
                self.cb_hlmem(op.op, memory);
                false
            }
        };

        #[cfg(not(feature = "perf"))]
        match op.target {
            CbTarget::Reg(r) => self.cb_reg(op.op, r),
            CbTarget::HLMem => self.cb_hlmem(op.op, memory),
        }

        #[cfg(feature = "perf")]
        {
            let dt = cyccnt()
                .wrapping_sub(cb_exec_start)
                .wrapping_sub(self.perf.nested_cycles_since(nested));
            if is_reg_target {
                self.perf.record_cb_exec_reg(dt);
            } else {
                self.perf.record_cb_exec_hlmem(dt);
            }
        }

        Ok(op.cycles)
    }
}

#[cfg(test)]
mod tests {
    use crate::cpu::registers::{Flags, Registers};
    use crate::gameboy::GameBoy;
    use crate::memory::memory::{GameBoyMemory, Memory};
    use alloc::{vec, vec::Vec};

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
            |m| {
                m.write(0xC000, 0x07).unwrap();
            },
            vec![0x86],
        )
        .with_registers(Registers {
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

    #[test]
    fn test_ld8_imm8_to_register_updates_target() {
        let mut cpu = make_test_cpu(vec![0x06, 0x7B]);
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().b, 0x7B);
    }

    #[test]
    fn test_ld8_imm8_to_memory_hl_updates_memory() {
        let mut cpu = make_test_cpu(vec![0x36, 0xA5]).with_registers(Registers {
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.read_memory(0xC000).unwrap(), 0xA5);
    }

    #[test]
    fn test_ld16_imm16_to_hl_updates_target() {
        let mut cpu = make_test_cpu(vec![0x21, 0x34, 0x12]);
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().hl(), 0x1234);
    }

    #[test]
    fn test_jr_negative_offset_loops_back_to_self() {
        let mut cpu = make_test_cpu(vec![0x18, 0xFE]);
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().pc, 0x0000);
    }

    #[test]
    fn test_jr_nz_taken_updates_pc() {
        let mut cpu = make_test_cpu(vec![0x20, 0x02]).with_registers(Registers {
            f: Flags::empty(),
            ..Default::default()
        });
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.registers().pc, 0x0004);
    }

    #[test]
    fn test_jr_z_not_taken_only_advances_pc() {
        let mut cpu = make_test_cpu(vec![0x28, 0x02]).with_registers(Registers {
            f: Flags::empty(),
            ..Default::default()
        });
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 8);
        assert_eq!(cpu.registers().pc, 0x0002);
    }

    #[test]
    fn test_cb_swap_hl_memory_updates_memory_and_flags() {
        let mut cpu = make_test_cpu_with_memory(
            |m| {
                m.write(0xC000, 0x3C).unwrap();
            },
            vec![0xCB, 0x36],
        )
        .with_registers(Registers {
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 16);
        assert_eq!(cpu.read_memory(0xC000).unwrap(), 0xC3);
        assert_eq!(cpu.registers().f, Flags::empty());
    }

    #[test]
    fn test_cb_bit_hl_memory_updates_flags_without_writeback() {
        let mut cpu = make_test_cpu_with_memory(
            |m| {
                m.write(0xC000, 0x00).unwrap();
            },
            vec![0xCB, 0x46],
        )
        .with_registers(Registers {
            f: Flags::C,
            h: 0xC0,
            l: 0x00,
            ..Default::default()
        });
        let cycles = cpu.step().unwrap();

        assert_eq!(cycles, 12);
        assert_eq!(cpu.read_memory(0xC000).unwrap(), 0x00);
        assert_eq!(cpu.registers().f, Flags::Z | Flags::H | Flags::C);
    }

    #[cfg(feature = "perf")]
    #[test]
    fn perf_counters_cover_fetch_decode_and_memory_paths() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x0147] = 0x00;
        rom[0x0148] = 0x00;
        rom[0x0149] = 0x00;
        rom[0x0100] = 0x3E; // LD A, d8
        rom[0x0101] = 0x12;
        rom[0x0102] = 0xEA; // LD (a16), A
        rom[0x0103] = 0x00;
        rom[0x0104] = 0x80;
        rom[0x0105] = 0xFA; // LD A, (a16)
        rom[0x0106] = 0x00;
        rom[0x0107] = 0x80;
        rom[0x0108] = 0x26; // LD H, d8
        rom[0x0109] = 0x80;
        rom[0x010A] = 0x2E; // LD L, d8
        rom[0x010B] = 0x00;
        rom[0x010C] = 0x86; // ADD A, (HL)

        let mut gb = GameBoy::new(rom);
        gb.tick();
        gb.tick();
        gb.tick();
        gb.tick();
        gb.tick();
        gb.tick();

        let profile = gb.take_perf_profile();
        assert!(profile.total > 0);
        assert!(profile.pc_fetch_calls >= 8);
        assert!(profile.pc_fetch_rom_calls >= 8);
        assert!(profile.pc_fetch_rom_idle > 0);
        assert!(profile.pc_fetch_rom_read > 0);
        assert!(profile.pc_fetch_wrapper > 0);
        assert!(profile.opcode_dispatch_calls >= 3);
        assert!(profile.operand8_calls >= 1);
        assert!(profile.operand8_imm_calls >= 1);
        assert!(profile.operand8_mem_calls >= 1);
        assert!(profile.mem_write > 0);
        assert!(profile.mem_write_fast_vram > 0);
        assert!(profile.mem_read > 0);
        assert!(profile.bus_read_calls >= 1);
        assert!(profile.mem_write_route > 0);
    }
}
