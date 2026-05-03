/// Flat operand constants and pack helpers for the function-pointer dispatch table.
///
/// Data word layout (32-bit):
///   bits  [7:0]  = arg0  (operand, condition, CB op, etc.)
///   bits [15:8]  = arg1  (dst operand for LD, bit index for CB, 0 otherwise)
///   bits [23:16] = cycles_taken
///   bits [31:24] = cycles_not_taken (conditional insns only; 0 for unconditional)
///
/// For pack_cc:
///   bits  [7:0]  = cond
///   bits [15:8]  = (unused, 0)
///   bits [23:16] = cycles_taken
///   bits [31:24] = cycles_not_taken

// ── r8 arg constants ─────────────────────────────────────────────────────────
// Also used as CB target encoding: matches SM83 CB r8 slot order
pub const B:      u8 = 0;
pub const C:      u8 = 1;
pub const D:      u8 = 2;
pub const E:      u8 = 3;
pub const H:      u8 = 4;
pub const L:      u8 = 5;
pub const MEM_HL: u8 = 6;  // (HL) memory — matches SM83 r8 encoding slot 6
pub const A:      u8 = 7;
pub const IMM8:   u8 = 8;
pub const IMMS8:  u8 = 9;  // signed immediate (for ADD SP, e)

// ── r16 arg constants ─────────────────────────────────────────────────────────
pub const BC: u8 = 10;
pub const DE: u8 = 11;
pub const HL: u8 = 12;
pub const SP: u8 = 13;
pub const AF: u8 = 14;

// ── condition constants ───────────────────────────────────────────────────────
pub const NZ: u8 = 0;
pub const Z:  u8 = 1;
pub const NC: u8 = 2;
pub const CC: u8 = 3;  // named CC to avoid clash with Flags::C

// ── CB shift/rotate op constants (bits [7:3] of CB opcode >> 3) ──────────────
pub const RLC:  u8 = 0;
pub const RRC:  u8 = 1;
pub const RL:   u8 = 2;
pub const RR:   u8 = 3;
pub const SLA:  u8 = 4;
pub const SRA:  u8 = 5;
pub const SWAP: u8 = 6;
pub const SRL:  u8 = 7;

// ── rotate-accumulator op constants ──────────────────────────────────────────
pub const RLCA: u8 = 0;
pub const RRCA: u8 = 1;
pub const RLA:  u8 = 2;
pub const RRA:  u8 = 3;

// ── Pack helpers ──────────────────────────────────────────────────────────────

/// Pack: no operand — just cycles.
///   bits [23:16] = cycles
#[inline(always)]
pub const fn pack0(cycles: u8) -> u32 {
    (cycles as u32) << 16
}

/// Pack: one operand in arg0.
///   bits [7:0] = arg0, bits [23:16] = cycles
#[inline(always)]
pub const fn pack1(arg0: u8, cycles: u8) -> u32 {
    (arg0 as u32) | ((cycles as u32) << 16)
}

/// Pack: two operands.
///   bits [7:0] = arg0, bits [15:8] = arg1, bits [23:16] = cycles
#[inline(always)]
pub const fn pack2(arg0: u8, arg1: u8, cycles: u8) -> u32 {
    (arg0 as u32) | ((arg1 as u32) << 8) | ((cycles as u32) << 16)
}

/// Pack: condition-code branch.
///   bits [7:0] = cond, bits [23:16] = cycles_taken, bits [31:24] = cycles_not_taken
#[inline(always)]
pub const fn pack_cc(cond: u8, cycles_taken: u8, cycles_not_taken: u8) -> u32 {
    (cond as u32) | ((cycles_taken as u32) << 16) | ((cycles_not_taken as u32) << 24)
}

// ── Unpack helpers ────────────────────────────────────────────────────────────

#[inline(always)]
pub const fn unpack_arg0(data: u32) -> u8 {
    data as u8
}

#[inline(always)]
pub const fn unpack_arg1(data: u32) -> u8 {
    (data >> 8) as u8
}

#[inline(always)]
pub const fn unpack_cycles(data: u32) -> u8 {
    (data >> 16) as u8
}

#[inline(always)]
pub const fn unpack_cycles_not_taken(data: u32) -> u8 {
    (data >> 24) as u8
}
