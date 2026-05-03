use alloc::{boxed::Box, sync::Arc, vec, vec::Vec};

use super::arg;
use super::cb::decoder::CbDecoder;
use super::adc::decoder::AdcDecoder;
use super::add::decoder::{Add16Decoder, Add8Decoder, AddSP16Decoder};
use super::call::decoder::CallDecoder;
use super::cp::decoder::Cp8Decoder;
use super::decoder::{Decoder, Error};
use super::inc_dec::decoder::{Dec16Decoder, Dec8Decoder, Inc16Decoder, Inc8Decoder};
use super::jump::decoder::JumpDecoder;
use super::ld::decoder::Ld8Decoder;
use super::ld16::decoder::Ld16Decoder;
use super::logic::and::decoder::And8Decoder;
use super::logic::or::decoder::Or8Decoder;
use super::logic::xor::decoder::Xor8Decoder;
use super::misc::decoder::MiscDecoder;
use super::opcode::OpCode;
use super::ret::decoder::RetDecoder;
use super::rotate::decoder::RotateDecoder;
use super::rst::decoder::RstDecoder;
use super::sbc::decoder::Sbc8Decoder;
use super::stack::decoder::{Pop16Decoder, Push16Decoder};
use super::sub::decoder::Sub8Decoder;

use crate::cpu::sm83::Sm83;
use crate::cpu::instructions::instructions::Error as InstructionError;

/// A function pointer to a handler method on `Sm83`.
pub type Handler = fn(&mut Sm83, u32) -> Result<(), InstructionError>;

/// One entry in the flat opcode dispatch table.
/// Two words, `Copy` — no heap, no atomics.
#[derive(Clone, Copy)]
pub struct OpcodeEntry {
    pub handler: Handler,
    pub data:    u32,
}

impl OpcodeEntry {
    /// Sentinel for unimplemented/illegal opcodes.
    const INVALID: Self = Self {
        handler: Sm83::handle_invalid,
        data:    0,
    };
}

pub struct OpCodeDecoder {
    opcodes: Vec<Box<dyn Decoder>>,
}

impl OpCodeDecoder {
    pub fn new() -> Self {
        OpCodeDecoder {
            opcodes: vec![
                Box::new(Ld8Decoder {}),
                Box::new(Add8Decoder {}),
                Box::new(Add16Decoder {}),
                Box::new(AddSP16Decoder {}),
                Box::new(AdcDecoder {}),
                Box::new(Sub8Decoder {}),
                Box::new(Sbc8Decoder {}),
                Box::new(Cp8Decoder {}),
                Box::new(Ld16Decoder {}),
                Box::new(Inc8Decoder {}),
                Box::new(Dec8Decoder {}),
                Box::new(Inc16Decoder {}),
                Box::new(Dec16Decoder {}),
                Box::new(RotateDecoder {}),
                Box::new(JumpDecoder {}),
                Box::new(And8Decoder {}),
                Box::new(Or8Decoder {}),
                Box::new(Xor8Decoder {}),
                Box::new(MiscDecoder {}),
                Box::new(Push16Decoder {}),
                Box::new(Pop16Decoder {}),
                Box::new(CallDecoder),
                Box::new(RetDecoder),
                Box::new(RstDecoder),
            ],
        }
    }
}

impl Decoder for OpCodeDecoder {
    fn decode(&self, opcode: u8) -> Result<Box<dyn OpCode>, Error> {
        self.opcodes
            .iter()
            .find_map(|decoder| decoder.decode(opcode).ok())
            .ok_or_else(|| Error::InvalidOpcode(opcode))
    }
}

/// Pre-decoded opcode table: 256-entry `Arc` arrays built once at startup.
/// `get()` / `get_cb()` are O(1) array index + one `Arc::clone` (atomic increment)
/// — no heap allocation per call.
///
/// Also contains a flat `[OpcodeEntry; 512]` function-pointer table for the hot
/// dispatch path: indices 0–255 are main opcodes, 256–511 are CB-prefixed opcodes.
pub struct OpCodeTable {
    pub(crate) main: Vec<Option<Arc<dyn OpCode>>>,
    pub(crate) cb:   Vec<Option<Arc<dyn OpCode>>>,
    /// Flat function-pointer table. Indices 0–255 = main; 256–511 = CB-prefixed.
    pub flat: alloc::boxed::Box<[OpcodeEntry; 512]>,
}

impl OpCodeTable {
    /// Build the complete table: flat function-pointer entries for the hot path
    /// AND `Arc`-based entries for the test/compatibility path.
    pub fn new() -> Self {
        let decoder = OpCodeDecoder::new();
        let mut main: Vec<Option<Arc<dyn OpCode>>> = Vec::with_capacity(256);
        for i in 0..=255u8 {
            main.push(decoder.decode(i).ok().map(Arc::from));
        }
        let mut cb: Vec<Option<Arc<dyn OpCode>>> = Vec::with_capacity(256);
        for i in 0..=255u8 {
            cb.push(CbDecoder.decode(i).ok().map(Arc::from));
        }

        let flat = Self::build_flat();
        Self { main, cb, flat }
    }

    /// Build the table from any `Decoder` for the main (non-CB) opcodes.
    /// CB opcodes are always decoded via `CbDecoder`.
    pub fn from_decoder(decoder: &dyn Decoder) -> Self {
        let mut main: Vec<Option<Arc<dyn OpCode>>> = Vec::with_capacity(256);
        for i in 0..=255u8 {
            main.push(decoder.decode(i).ok().map(Arc::from));
        }
        let mut cb: Vec<Option<Arc<dyn OpCode>>> = Vec::with_capacity(256);
        for i in 0..=255u8 {
            cb.push(CbDecoder.decode(i).ok().map(Arc::from));
        }
        let flat = Self::build_flat();
        Self { main, cb, flat }
    }

    /// Build the flat 512-entry function-pointer table.
    ///
    /// Indices 0–255: main opcodes.
    /// Indices 256–511: CB-prefixed opcodes (index = 256 + cb_byte).
    fn build_flat() -> alloc::boxed::Box<[OpcodeEntry; 512]> {
        let mut t = alloc::boxed::Box::new([OpcodeEntry::INVALID; 512]);

        // ── Helper aliases ──────────────────────────────────────────────────
        use arg::*;

        // ─── Main opcode table (indices 0–255) ──────────────────────────────

        // NOP
        t[0x00] = OpcodeEntry { handler: Sm83::handle_nop,  data: pack0(4) };
        // STOP 0x00 (consumes next byte)
        t[0x10] = OpcodeEntry { handler: Sm83::handle_stop, data: pack0(4) };
        // HALT
        t[0x76] = OpcodeEntry { handler: Sm83::handle_halt, data: pack0(4) };
        // DAA/CPL/SCF/CCF/DI/EI
        t[0x27] = OpcodeEntry { handler: Sm83::handle_daa,  data: pack0(4) };
        t[0x2F] = OpcodeEntry { handler: Sm83::handle_cpl,  data: pack0(4) };
        t[0x37] = OpcodeEntry { handler: Sm83::handle_scf,  data: pack0(4) };
        t[0x3F] = OpcodeEntry { handler: Sm83::handle_ccf,  data: pack0(4) };
        t[0xF3] = OpcodeEntry { handler: Sm83::handle_di,   data: pack0(4) };
        t[0xFB] = OpcodeEntry { handler: Sm83::handle_ei,   data: pack0(4) };

        // RLCA / RRCA / RLA / RRA
        t[0x07] = OpcodeEntry { handler: Sm83::handle_rot_acc, data: pack1(RLCA, 4) };
        t[0x0F] = OpcodeEntry { handler: Sm83::handle_rot_acc, data: pack1(RRCA, 4) };
        t[0x17] = OpcodeEntry { handler: Sm83::handle_rot_acc, data: pack1(RLA,  4) };
        t[0x1F] = OpcodeEntry { handler: Sm83::handle_rot_acc, data: pack1(RRA,  4) };

        // ADD A, r8 / (HL) / imm8  (0x80–0x87, 0xC6)
        t[0x80] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(B,      4) };
        t[0x81] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(C,      4) };
        t[0x82] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(D,      4) };
        t[0x83] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(E,      4) };
        t[0x84] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(H,      4) };
        t[0x85] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(L,      4) };
        t[0x86] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(MEM_HL, 8) };
        t[0x87] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(A,      4) };
        t[0xC6] = OpcodeEntry { handler: Sm83::handle_add8, data: pack1(IMM8,   8) };

        // ADC A, r8 / (HL) / imm8  (0x88–0x8F, 0xCE)
        t[0x88] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(B,      4) };
        t[0x89] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(C,      4) };
        t[0x8A] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(D,      4) };
        t[0x8B] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(E,      4) };
        t[0x8C] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(H,      4) };
        t[0x8D] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(L,      4) };
        t[0x8E] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(MEM_HL, 8) };
        t[0x8F] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(A,      4) };
        t[0xCE] = OpcodeEntry { handler: Sm83::handle_adc, data: pack1(IMM8,   8) };

        // SUB A, r8 / (HL) / imm8  (0x90–0x97, 0xD6)
        t[0x90] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(B,      4) };
        t[0x91] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(C,      4) };
        t[0x92] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(D,      4) };
        t[0x93] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(E,      4) };
        t[0x94] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(H,      4) };
        t[0x95] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(L,      4) };
        t[0x96] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(MEM_HL, 8) };
        t[0x97] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(A,      4) };
        t[0xD6] = OpcodeEntry { handler: Sm83::handle_sub8, data: pack1(IMM8,   8) };

        // SBC A, r8 / (HL) / imm8  (0x98–0x9F, 0xDE)
        t[0x98] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(B,      4) };
        t[0x99] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(C,      4) };
        t[0x9A] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(D,      4) };
        t[0x9B] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(E,      4) };
        t[0x9C] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(H,      4) };
        t[0x9D] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(L,      4) };
        t[0x9E] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(MEM_HL, 8) };
        t[0x9F] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(A,      4) };
        t[0xDE] = OpcodeEntry { handler: Sm83::handle_sbc8, data: pack1(IMM8,   8) };

        // AND A, r8 / (HL) / imm8  (0xA0–0xA7, 0xE6)
        t[0xA0] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(B,      4) };
        t[0xA1] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(C,      4) };
        t[0xA2] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(D,      4) };
        t[0xA3] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(E,      4) };
        t[0xA4] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(H,      4) };
        t[0xA5] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(L,      4) };
        t[0xA6] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(MEM_HL, 8) };
        t[0xA7] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(A,      4) };
        t[0xE6] = OpcodeEntry { handler: Sm83::handle_and8, data: pack1(IMM8,   8) };

        // XOR A, r8 / (HL) / imm8  (0xA8–0xAF, 0xEE)
        t[0xA8] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(B,      4) };
        t[0xA9] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(C,      4) };
        t[0xAA] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(D,      4) };
        t[0xAB] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(E,      4) };
        t[0xAC] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(H,      4) };
        t[0xAD] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(L,      4) };
        t[0xAE] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(MEM_HL, 8) };
        t[0xAF] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(A,      4) };
        t[0xEE] = OpcodeEntry { handler: Sm83::handle_xor8, data: pack1(IMM8,   8) };

        // OR A, r8 / (HL) / imm8  (0xB0–0xB7, 0xF6)
        t[0xB0] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(B,      4) };
        t[0xB1] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(C,      4) };
        t[0xB2] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(D,      4) };
        t[0xB3] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(E,      4) };
        t[0xB4] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(H,      4) };
        t[0xB5] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(L,      4) };
        t[0xB6] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(MEM_HL, 8) };
        t[0xB7] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(A,      4) };
        t[0xF6] = OpcodeEntry { handler: Sm83::handle_or8, data: pack1(IMM8,   8) };

        // CP A, r8 / (HL) / imm8  (0xB8–0xBF, 0xFE)
        t[0xB8] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(B,      4) };
        t[0xB9] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(C,      4) };
        t[0xBA] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(D,      4) };
        t[0xBB] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(E,      4) };
        t[0xBC] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(H,      4) };
        t[0xBD] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(L,      4) };
        t[0xBE] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(MEM_HL, 8) };
        t[0xBF] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(A,      4) };
        t[0xFE] = OpcodeEntry { handler: Sm83::handle_cp8, data: pack1(IMM8,   8) };

        // INC r8 / (HL)
        t[0x04] = OpcodeEntry { handler: Sm83::handle_inc8, data: pack1(B,       4) };
        t[0x0C] = OpcodeEntry { handler: Sm83::handle_inc8, data: pack1(C,       4) };
        t[0x14] = OpcodeEntry { handler: Sm83::handle_inc8, data: pack1(D,       4) };
        t[0x1C] = OpcodeEntry { handler: Sm83::handle_inc8, data: pack1(E,       4) };
        t[0x24] = OpcodeEntry { handler: Sm83::handle_inc8, data: pack1(H,       4) };
        t[0x2C] = OpcodeEntry { handler: Sm83::handle_inc8, data: pack1(L,       4) };
        t[0x34] = OpcodeEntry { handler: Sm83::handle_inc8, data: pack1(MEM_HL, 12) };
        t[0x3C] = OpcodeEntry { handler: Sm83::handle_inc8, data: pack1(A,       4) };

        // DEC r8 / (HL)
        t[0x05] = OpcodeEntry { handler: Sm83::handle_dec8, data: pack1(B,       4) };
        t[0x0D] = OpcodeEntry { handler: Sm83::handle_dec8, data: pack1(C,       4) };
        t[0x15] = OpcodeEntry { handler: Sm83::handle_dec8, data: pack1(D,       4) };
        t[0x1D] = OpcodeEntry { handler: Sm83::handle_dec8, data: pack1(E,       4) };
        t[0x25] = OpcodeEntry { handler: Sm83::handle_dec8, data: pack1(H,       4) };
        t[0x2D] = OpcodeEntry { handler: Sm83::handle_dec8, data: pack1(L,       4) };
        t[0x35] = OpcodeEntry { handler: Sm83::handle_dec8, data: pack1(MEM_HL, 12) };
        t[0x3D] = OpcodeEntry { handler: Sm83::handle_dec8, data: pack1(A,       4) };

        // INC r16
        t[0x03] = OpcodeEntry { handler: Sm83::handle_inc16, data: pack1(BC, 8) };
        t[0x13] = OpcodeEntry { handler: Sm83::handle_inc16, data: pack1(DE, 8) };
        t[0x23] = OpcodeEntry { handler: Sm83::handle_inc16, data: pack1(HL, 8) };
        t[0x33] = OpcodeEntry { handler: Sm83::handle_inc16, data: pack1(SP, 8) };

        // DEC r16
        t[0x0B] = OpcodeEntry { handler: Sm83::handle_dec16, data: pack1(BC, 8) };
        t[0x1B] = OpcodeEntry { handler: Sm83::handle_dec16, data: pack1(DE, 8) };
        t[0x2B] = OpcodeEntry { handler: Sm83::handle_dec16, data: pack1(HL, 8) };
        t[0x3B] = OpcodeEntry { handler: Sm83::handle_dec16, data: pack1(SP, 8) };

        // ADD HL, r16
        t[0x09] = OpcodeEntry { handler: Sm83::handle_add16, data: pack1(BC, 8) };
        t[0x19] = OpcodeEntry { handler: Sm83::handle_add16, data: pack1(DE, 8) };
        t[0x29] = OpcodeEntry { handler: Sm83::handle_add16, data: pack1(HL, 8) };
        t[0x39] = OpcodeEntry { handler: Sm83::handle_add16, data: pack1(SP, 8) };

        // ADD SP, e8
        t[0xE8] = OpcodeEntry { handler: Sm83::handle_add_sp_e8, data: pack0(16) };

        // LD r8, r8 / LD r8, (HL) / LD (HL), r8  (0x40–0x7F, skip 0x76 = HALT)
        // Encoded as pack2(dst_arg, src_arg, cycles)
        for dst in 0u8..8 {
            for src in 0u8..8 {
                let op = 0x40u8 | (dst << 3) | src;
                if op == 0x76 { continue; } // HALT already set above
                let dst_arg = Self::reg_bits_to_arg(dst);
                let src_arg = Self::reg_bits_to_arg(src);
                let cycles: u8 = if dst == 6 || src == 6 { 8 } else { 4 };
                t[op as usize] = OpcodeEntry {
                    handler: Sm83::handle_ld8,
                    data: pack2(dst_arg, src_arg, cycles),
                };
            }
        }

        // LD r8, imm8  (0x06,0x0E,0x16,0x1E,0x26,0x2E,0x36,0x3E)
        t[0x06] = OpcodeEntry { handler: Sm83::handle_ld8, data: pack2(B,      IMM8,  8) };
        t[0x0E] = OpcodeEntry { handler: Sm83::handle_ld8, data: pack2(C,      IMM8,  8) };
        t[0x16] = OpcodeEntry { handler: Sm83::handle_ld8, data: pack2(D,      IMM8,  8) };
        t[0x1E] = OpcodeEntry { handler: Sm83::handle_ld8, data: pack2(E,      IMM8,  8) };
        t[0x26] = OpcodeEntry { handler: Sm83::handle_ld8, data: pack2(H,      IMM8,  8) };
        t[0x2E] = OpcodeEntry { handler: Sm83::handle_ld8, data: pack2(L,      IMM8,  8) };
        t[0x36] = OpcodeEntry { handler: Sm83::handle_ld8, data: pack2(MEM_HL, IMM8, 12) };
        t[0x3E] = OpcodeEntry { handler: Sm83::handle_ld8, data: pack2(A,      IMM8,  8) };

        // LD rr, nn
        t[0x01] = OpcodeEntry { handler: Sm83::handle_ld16_rr_imm16, data: pack1(BC, 12) };
        t[0x11] = OpcodeEntry { handler: Sm83::handle_ld16_rr_imm16, data: pack1(DE, 12) };
        t[0x21] = OpcodeEntry { handler: Sm83::handle_ld16_rr_imm16, data: pack1(HL, 12) };
        t[0x31] = OpcodeEntry { handler: Sm83::handle_ld16_rr_imm16, data: pack1(SP, 12) };

        // LD (nn), SP
        t[0x08] = OpcodeEntry { handler: Sm83::handle_ld_nn_sp,  data: pack0(20) };
        // LD SP, HL
        t[0xF9] = OpcodeEntry { handler: Sm83::handle_ld_sp_hl,  data: pack0(8) };
        // LD HL, SP+e
        t[0xF8] = OpcodeEntry { handler: Sm83::handle_ld_hl_sp_e, data: pack0(12) };
        // LD (BC), A
        t[0x02] = OpcodeEntry { handler: Sm83::handle_ld_bc_a,   data: pack0(8) };
        // LD (DE), A
        t[0x12] = OpcodeEntry { handler: Sm83::handle_ld_de_a,   data: pack0(8) };
        // LD A, (BC)
        t[0x0A] = OpcodeEntry { handler: Sm83::handle_ld_a_bc,   data: pack0(8) };
        // LD A, (DE)
        t[0x1A] = OpcodeEntry { handler: Sm83::handle_ld_a_de,   data: pack0(8) };
        // LD (HL+), A
        t[0x22] = OpcodeEntry { handler: Sm83::handle_ld_hli_a,  data: pack0(8) };
        // LD (HL-), A
        t[0x32] = OpcodeEntry { handler: Sm83::handle_ld_hld_a,  data: pack0(8) };
        // LD A, (HL+)
        t[0x2A] = OpcodeEntry { handler: Sm83::handle_ld_a_hli,  data: pack0(8) };
        // LD A, (HL-)
        t[0x3A] = OpcodeEntry { handler: Sm83::handle_ld_a_hld,  data: pack0(8) };
        // LD (nn), A
        t[0xEA] = OpcodeEntry { handler: Sm83::handle_ld_nn_a,   data: pack0(16) };
        // LD A, (nn)
        t[0xFA] = OpcodeEntry { handler: Sm83::handle_ld_a_nn,   data: pack0(16) };
        // LDH (n), A
        t[0xE0] = OpcodeEntry { handler: Sm83::handle_ldh_n_a,   data: pack0(12) };
        // LDH A, (n)
        t[0xF0] = OpcodeEntry { handler: Sm83::handle_ldh_a_n,   data: pack0(12) };
        // LD (C), A
        t[0xE2] = OpcodeEntry { handler: Sm83::handle_ld_c_a,    data: pack0(8) };
        // LD A, (C)
        t[0xF2] = OpcodeEntry { handler: Sm83::handle_ld_a_c,    data: pack0(8) };

        // PUSH rr
        t[0xC5] = OpcodeEntry { handler: Sm83::handle_push16, data: pack1(BC, 16) };
        t[0xD5] = OpcodeEntry { handler: Sm83::handle_push16, data: pack1(DE, 16) };
        t[0xE5] = OpcodeEntry { handler: Sm83::handle_push16, data: pack1(HL, 16) };
        t[0xF5] = OpcodeEntry { handler: Sm83::handle_push16, data: pack1(AF, 16) };

        // POP rr
        t[0xC1] = OpcodeEntry { handler: Sm83::handle_pop16, data: pack1(BC, 12) };
        t[0xD1] = OpcodeEntry { handler: Sm83::handle_pop16, data: pack1(DE, 12) };
        t[0xE1] = OpcodeEntry { handler: Sm83::handle_pop16, data: pack1(HL, 12) };
        t[0xF1] = OpcodeEntry { handler: Sm83::handle_pop16, data: pack1(AF, 12) };

        // JP nn (unconditional, 16 cycles)
        t[0xC3] = OpcodeEntry { handler: Sm83::handle_jp_nn,  data: pack0(16) };
        // JP HL (4 cycles)
        t[0xE9] = OpcodeEntry { handler: Sm83::handle_jp_hl,  data: pack0(4) };
        // JR e (12 cycles)
        t[0x18] = OpcodeEntry { handler: Sm83::handle_jr,     data: pack0(12) };

        // JP cc, nn  (taken=16, not-taken=12)
        t[0xC2] = OpcodeEntry { handler: Sm83::handle_jp_cc, data: pack_cc(NZ, 16, 12) };
        t[0xCA] = OpcodeEntry { handler: Sm83::handle_jp_cc, data: pack_cc(Z,  16, 12) };
        t[0xD2] = OpcodeEntry { handler: Sm83::handle_jp_cc, data: pack_cc(NC, 16, 12) };
        t[0xDA] = OpcodeEntry { handler: Sm83::handle_jp_cc, data: pack_cc(CC, 16, 12) };

        // JR cc, e  (taken=12, not-taken=8)
        t[0x20] = OpcodeEntry { handler: Sm83::handle_jr_cc, data: pack_cc(NZ, 12, 8) };
        t[0x28] = OpcodeEntry { handler: Sm83::handle_jr_cc, data: pack_cc(Z,  12, 8) };
        t[0x30] = OpcodeEntry { handler: Sm83::handle_jr_cc, data: pack_cc(NC, 12, 8) };
        t[0x38] = OpcodeEntry { handler: Sm83::handle_jr_cc, data: pack_cc(CC, 12, 8) };

        // CALL nn (24 cycles)
        t[0xCD] = OpcodeEntry { handler: Sm83::handle_call,    data: pack0(24) };

        // CALL cc, nn  (taken=24, not-taken=12)
        t[0xC4] = OpcodeEntry { handler: Sm83::handle_call_cc, data: pack_cc(NZ, 24, 12) };
        t[0xCC] = OpcodeEntry { handler: Sm83::handle_call_cc, data: pack_cc(Z,  24, 12) };
        t[0xD4] = OpcodeEntry { handler: Sm83::handle_call_cc, data: pack_cc(NC, 24, 12) };
        t[0xDC] = OpcodeEntry { handler: Sm83::handle_call_cc, data: pack_cc(CC, 24, 12) };

        // RET (16 cycles)
        t[0xC9] = OpcodeEntry { handler: Sm83::handle_ret,     data: pack0(16) };
        // RETI (16 cycles)
        t[0xD9] = OpcodeEntry { handler: Sm83::handle_reti,    data: pack0(16) };

        // RET cc  (taken=20, not-taken=8)
        t[0xC0] = OpcodeEntry { handler: Sm83::handle_ret_cc, data: pack_cc(NZ, 20, 8) };
        t[0xC8] = OpcodeEntry { handler: Sm83::handle_ret_cc, data: pack_cc(Z,  20, 8) };
        t[0xD0] = OpcodeEntry { handler: Sm83::handle_ret_cc, data: pack_cc(NC, 20, 8) };
        t[0xD8] = OpcodeEntry { handler: Sm83::handle_ret_cc, data: pack_cc(CC, 20, 8) };

        // RST n  (16 cycles each; arg0 = target vector / 8, actual addr = arg0 * 8)
        t[0xC7] = OpcodeEntry { handler: Sm83::handle_rst, data: pack1(0x00, 16) };
        t[0xCF] = OpcodeEntry { handler: Sm83::handle_rst, data: pack1(0x08, 16) };
        t[0xD7] = OpcodeEntry { handler: Sm83::handle_rst, data: pack1(0x10, 16) };
        t[0xDF] = OpcodeEntry { handler: Sm83::handle_rst, data: pack1(0x18, 16) };
        t[0xE7] = OpcodeEntry { handler: Sm83::handle_rst, data: pack1(0x20, 16) };
        t[0xEF] = OpcodeEntry { handler: Sm83::handle_rst, data: pack1(0x28, 16) };
        t[0xF7] = OpcodeEntry { handler: Sm83::handle_rst, data: pack1(0x30, 16) };
        t[0xFF] = OpcodeEntry { handler: Sm83::handle_rst, data: pack1(0x38, 16) };

        // ── CB-prefixed opcode table (indices 256–511) ───────────────────────
        // CB opcode byte layout:
        //   bits [7:6]: group (00=shift/rot, 01=BIT, 10=RES, 11=SET)
        //   bits [5:3]: op or bit index
        //   bits [2:0]: register target (0=B..5=L,6=(HL),7=A)
        for cb in 0u8..=255u8 {
            let idx = 256 + cb as usize;
            let tgt = cb & 0x07;           // 0–7 (same as arg::B..=arg::A)
            let is_hl = tgt == MEM_HL;
            match cb >> 6 {
                0x00 => {
                    // Shift/rotate group — bits [5:3] = op
                    let op_field = (cb >> 3) & 0x07;
                    let cycles: u8 = if is_hl { 16 } else { 8 };
                    // pack2(target, shift_op, cycles)
                    t[idx] = OpcodeEntry {
                        handler: Sm83::handle_cb_shift,
                        data: pack2(tgt, op_field, cycles),
                    };
                }
                0x01 => {
                    // BIT b, r — bits [5:3] = bit index
                    let bit = (cb >> 3) & 0x07;
                    let cycles: u8 = if is_hl { 12 } else { 8 };
                    // pack2(target, bit, cycles)
                    t[idx] = OpcodeEntry {
                        handler: Sm83::handle_cb_bit,
                        data: pack2(tgt, bit, cycles),
                    };
                }
                0x02 => {
                    // RES b, r
                    let bit = (cb >> 3) & 0x07;
                    let cycles: u8 = if is_hl { 16 } else { 8 };
                    t[idx] = OpcodeEntry {
                        handler: Sm83::handle_cb_res,
                        data: pack2(tgt, bit, cycles),
                    };
                }
                0x03 => {
                    // SET b, r
                    let bit = (cb >> 3) & 0x07;
                    let cycles: u8 = if is_hl { 16 } else { 8 };
                    t[idx] = OpcodeEntry {
                        handler: Sm83::handle_cb_set,
                        data: pack2(tgt, bit, cycles),
                    };
                }
                _ => unreachable!(),
            }
        }

        t
    }

    /// Convert the 3-bit register field from the LD r8,r8 opcode to an `arg` constant.
    const fn reg_bits_to_arg(bits: u8) -> u8 {
        match bits {
            0 => arg::B,
            1 => arg::C,
            2 => arg::D,
            3 => arg::E,
            4 => arg::H,
            5 => arg::L,
            6 => arg::MEM_HL,
            7 => arg::A,
            _ => 0xFF, // unreachable in well-formed opcodes
        }
    }

    /// Return a clone of the pre-decoded handler for this opcode.
    /// The returned `Arc` has a lifetime independent of `self`, so callers can
    /// hold it while mutably borrowing the CPU.
    pub fn get(&self, opcode: u8) -> Result<Arc<dyn OpCode>, Error> {
        self.main[opcode as usize]
            .as_ref()
            .map(Arc::clone)
            .ok_or(Error::InvalidOpcode(opcode))
    }

    pub fn get_cb(&self, opcode: u8) -> Result<Arc<dyn OpCode>, Error> {
        self.cb[opcode as usize]
            .as_ref()
            .map(Arc::clone)
            .ok_or(Error::InvalidOpcode(opcode))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::instructions::operand::*;
    use crate::cpu::instructions::test::util::FakeCpu;

    #[test]
    fn test_from_add8() {
        let opcode = 0x80;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        )
    }

    #[test]
    fn test_from_add16_bc() {
        let opcode = 0x09;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::BC);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_add16_de() {
        let opcode = 0x19;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::DE);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_add16_hl() {
        let opcode = 0x29;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::HL);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_add16_sp() {
        let opcode = 0x39;
        let expected_cycles = 8;
        let expected_operand = Operand::Register16(Register16::SP);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_adc_a() {
        let opcode = 0x8F;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::A);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_sub8_b() {
        let opcode = 0x90;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_sbc8_b() {
        let opcode = 0x98;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_from_cp8_b() {
        let opcode = 0xB8;
        let expected_cycles = 4;
        let expected_operand = Operand::Register8(Register8::B);

        FakeCpu::new().test_decode_operand(
            opcode,
            &OpCodeDecoder::new(),
            expected_cycles,
            expected_operand,
        );
    }

    #[test]
    fn test_invalid_opcode() {
        let opcode = 0xFC; // Truly unimplemented opcode
        assert!(OpCodeDecoder::new().decode(opcode).is_err());
    }
}
