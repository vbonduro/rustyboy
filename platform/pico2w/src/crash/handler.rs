//! Fault and panic handlers that capture crash state into RP2350 scratch
//! registers and then trigger a clean reset.
//!
//! # Why scratch registers?
//!
//! Writing to flash during a fault is problematic:
//! - Core 1 may be executing code from XIP flash → disabling XIP is required
//!   but risky in a fault context.
//! - A full sector erase takes ~45 ms — unacceptable in a hard-fault path.
//!
//! Instead, we write 32 bytes to the Watchdog scratch registers and 32 bytes
//! to the POWMAN scratch registers — both are MMIO and survive soft resets.
//! On the next boot, `crash::storage::check_and_commit` reads the scratch
//! registers, constructs the full 128-byte [`CrashRecord`], and commits it
//! to flash safely on a single core.
//!
//! # Watchdog scratch layout (8 × u32)
//!
//! ```text
//! [0] = CRASH_MAGIC                  sentinel — only written here
//! [1] = arm_pc                       program counter at fault
//! [2] = arm_lr                       link register at fault
//! [3] = arm_cfsr                     Configurable Fault Status Register
//! [4] = arm_hfsr                     HardFault Status Register
//! [5] = arm_fault_addr               MMFAR or BFAR (whichever is valid)
//! [6] = [crash_kind:8 | flags:8 | ram_bank:8 | _:8]
//! [7] = [rom_bank:16 | gb_pc:16]
//! ```
//!
//! # POWMAN scratch layout (8 × u32)
//!
//! ```text
//! [0] = rom_id_prefix                first 4 bytes of ROM SHA-256
//! [1] = [gb_a:8 | gb_f:8 | gb_b:8 | gb_c:8]
//! [2] = [gb_d:8 | gb_e:8 | gb_h:8 | gb_l:8]
//! [3] = [gb_sp:16 | ppu_ly:8 | ppu_stat:8]
//! [4] = gb_cycle_lo                  lower 32 bits of cycle counter
//! [5] = [stack_headroom:16 | panic_line_or_r1:16]
//!          stack_headroom: bytes between MSP-at-fault and _stack_end
//!            (HAS_STACK_OVERFLOW set → overflow depth, clear → remaining headroom)
//!          panic_line_or_r1: panic source line (panics) or stacked R1 lo16
//!            (UNALIGNED HardFaults)
//! [6] = panic_file[0..4]             first 4 ASCII bytes of source file
//! [7] = panic_file[4..8]             next 4 ASCII bytes
//! ```

use cortex_m_rt::ExceptionFrame;
use rp_pac::{POWMAN, WATCHDOG};

use super::{flags, CrashKind, CRASH_CONTEXT, CRASH_MAGIC};

// ---------------------------------------------------------------------------
// Linker-defined symbol: bottom of the Core 0 MSP stack.
// Declared in memory.x; the linker resolves it to a concrete address.
// ---------------------------------------------------------------------------
extern "C" {
    static _stack_end: u32;
}

// ---------------------------------------------------------------------------
// ARM System Control Block register addresses (Cortex-M33 architectural).
// ---------------------------------------------------------------------------
const SCB_CFSR: *const u32 = 0xE000_ED28 as *const u32;
const SCB_HFSR: *const u32 = 0xE000_ED2C as *const u32;
const SCB_MMFAR: *const u32 = 0xE000_ED34 as *const u32;
const SCB_BFAR: *const u32 = 0xE000_ED38 as *const u32;

// CFSR bit positions.
const CFSR_MMARVALID: u32 = 1 << 7;  // MMFAR holds a valid address
const CFSR_BFARVALID: u32 = 1 << 15; // BFAR holds a valid address
/// UFSR.UNALIGNED (bit 24) — neither BFAR nor MMFAR is valid for this fault;
/// we repurpose `arm_fault_addr` to hold the stacked R0 from the exception
/// frame so the decoder can display the actual misaligned address.
const CFSR_UNALIGNED: u32 = 1 << 24;

// ---------------------------------------------------------------------------
// HardFault handler
// ---------------------------------------------------------------------------

#[cortex_m_rt::exception]
unsafe fn HardFault(ef: &ExceptionFrame) -> ! {
    // Disable interrupts so nothing else interferes while we write scratch.
    cortex_m::interrupt::disable();

    // Read fault classification registers.
    let cfsr = unsafe { core::ptr::read_volatile(SCB_CFSR) };
    let hfsr = unsafe { core::ptr::read_volatile(SCB_HFSR) };
    let mmfar = unsafe { core::ptr::read_volatile(SCB_MMFAR) };
    let bfar = unsafe { core::ptr::read_volatile(SCB_BFAR) };

    // Pick the most relevant fault address.
    // For UNALIGNED faults, BFAR/MMFAR are not updated by hardware.  Instead,
    // save the stacked R0 from the exception frame: for `ldr r0, [r0]` and
    // `lda r1, [r1]` patterns, R0 holds the address that triggered the fault.
    let fault_addr = if cfsr & CFSR_BFARVALID != 0 {
        bfar
    } else if cfsr & CFSR_MMARVALID != 0 {
        mmfar
    } else if cfsr & CFSR_UNALIGNED != 0 {
        ef.r0()
    } else {
        0
    };

    // Attempt to read emulator state from the global context.
    let ctx = CRASH_CONTEXT.snapshot();

    // Compute stack headroom / overflow depth from the pre-exception MSP.
    // The hardware pushes 8 words (32 bytes) before entering the exception handler,
    // so MSP before the fault = exception frame pointer + 32.
    let (stack_headroom, overflowed) = compute_stack_info_from_ef(ef);

    let f = flags::HAS_ARM_REGS
        | if ctx.is_some() { flags::HAS_GB_STATE | flags::HAS_ROM_INFO } else { 0 }
        | if overflowed { flags::HAS_STACK_OVERFLOW } else { 0 };

    // Build the packed word for scratch[6].
    let ram_bank = ctx.as_ref().map(|c| c.ram_bank).unwrap_or(0);
    let packed6 = (CrashKind::HardFault as u32)
        | ((f as u32) << 8)
        | ((ram_bank as u32) << 16);

    // Build the packed word for scratch[7].
    let rom_bank = ctx.as_ref().map(|c| c.rom_bank).unwrap_or(0);
    let gb_pc = ctx.as_ref().map(|c| c.gb_pc).unwrap_or(0);
    let packed7 = ((rom_bank as u32) << 16) | (gb_pc as u32);

    // For UNALIGNED faults, also capture R1 into POWMAN scratch[5] lo16.
    // Pattern B is `lda r1, [r1]` — stacked R1 = faulting address.
    let arm_r1_for_unaligned = if cfsr & CFSR_UNALIGNED != 0 { ef.r1() } else { 0 };

    write_watchdog_scratch(ef.pc(), ef.lr(), cfsr, hfsr, fault_addr, packed6, packed7);
    write_powman_scratch_from_context(ctx.as_ref(), None, arm_r1_for_unaligned, stack_headroom);

    cortex_m::peripheral::SCB::sys_reset()
}

// ---------------------------------------------------------------------------
// Panic handler (replaces panic-probe)
// ---------------------------------------------------------------------------

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    // Disable interrupts immediately.  `disable()` is safe in this context
    // (single-core panic handler, no RTOS invariants apply).
    cortex_m::interrupt::disable();

    // Extract the file and line from the panic location.
    let (file_bytes, line) = if let Some(loc) = info.location() {
        let f = loc.file();
        // Take the last path component for compactness.
        let filename = f.rfind('/').map(|i| &f[i + 1..]).unwrap_or(f);
        let fb = filename.as_bytes();
        let mut buf = [0u8; 8];
        let n = fb.len().min(8);
        buf[..n].copy_from_slice(&fb[..n]);
        (buf, loc.line())
    } else {
        ([0u8; 8], 0)
    };

    let ctx = CRASH_CONTEXT.snapshot();

    // Read MSP directly — in a panic handler there's no exception frame.
    let (stack_headroom, overflowed) = compute_stack_info_from_msp();

    let f = flags::HAS_PANIC_LOC
        | if ctx.is_some() { flags::HAS_GB_STATE | flags::HAS_ROM_INFO } else { 0 }
        | if overflowed { flags::HAS_STACK_OVERFLOW } else { 0 };

    let ram_bank = ctx.as_ref().map(|c| c.ram_bank).unwrap_or(0);
    let packed6 = (CrashKind::Panic as u32) | ((f as u32) << 8) | ((ram_bank as u32) << 16);

    let rom_bank = ctx.as_ref().map(|c| c.rom_bank).unwrap_or(0);
    let gb_pc = ctx.as_ref().map(|c| c.gb_pc).unwrap_or(0);
    let packed7 = ((rom_bank as u32) << 16) | (gb_pc as u32);

    // No meaningful ARM PC/LR for a software panic.
    write_watchdog_scratch(0, 0, 0, 0, 0, packed6, packed7);
    write_powman_scratch_from_context(ctx.as_ref(), Some(file_bytes), line, stack_headroom);

    cortex_m::peripheral::SCB::sys_reset()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Compute `(headroom_or_depth: u16, overflowed: bool)` from the exception frame.
///
/// At the point where the `HardFault` handler executes, the hardware has already
/// pushed 8 words (32 bytes) onto the MSP.  So the pre-fault MSP is:
///   `ef as *const ExceptionFrame as usize + 32`
///
/// We compare that against `_stack_end` (the linker-defined bottom of the MSP
/// stack) to determine whether the stack has overflowed:
/// - If `msp_before_fault >= _stack_end`: `headroom = msp_before_fault - _stack_end`
/// - If `msp_before_fault <  _stack_end`: `depth    = _stack_end - msp_before_fault`
///
/// The return value is clamped to `u16::MAX`.
#[inline(always)]
fn compute_stack_info_from_ef(ef: &ExceptionFrame) -> (u16, bool) {
    // SAFETY: `_stack_end` is a linker symbol with no associated data; taking its
    // address is safe.  We cast to usize for arithmetic only.
    let stack_end = unsafe { &_stack_end as *const u32 as usize };
    // Exception hardware frame size = 8 registers × 4 bytes = 32 bytes.
    let msp_before = ef as *const ExceptionFrame as usize + 32;
    if msp_before >= stack_end {
        let headroom = (msp_before - stack_end).min(u16::MAX as usize) as u16;
        (headroom, false)
    } else {
        let depth = (stack_end - msp_before).min(u16::MAX as usize) as u16;
        (depth, true)
    }
}

/// Same as [`compute_stack_info_from_ef`] but reads the current MSP directly.
///
/// Used by the panic handler, which runs as a normal function (no exception
/// frame is pushed by hardware).
#[inline(always)]
fn compute_stack_info_from_msp() -> (u16, bool) {
    let stack_end = unsafe { &_stack_end as *const u32 as usize };
    let msp = cortex_m::register::msp::read() as usize;
    if msp >= stack_end {
        let headroom = (msp - stack_end).min(u16::MAX as usize) as u16;
        (headroom, false)
    } else {
        let depth = (stack_end - msp).min(u16::MAX as usize) as u16;
        (depth, true)
    }
}

/// Write the 8 Watchdog scratch registers.
#[inline(never)]
fn write_watchdog_scratch(
    arm_pc: u32,
    arm_lr: u32,
    arm_cfsr: u32,
    arm_hfsr: u32,
    arm_fault_addr: u32,
    packed6: u32,
    packed7: u32,
) {
    let wd = WATCHDOG;
    wd.scratch0().write_value(CRASH_MAGIC);
    wd.scratch1().write_value(arm_pc);
    wd.scratch2().write_value(arm_lr);
    wd.scratch3().write_value(arm_cfsr);
    wd.scratch4().write_value(arm_hfsr);
    wd.scratch5().write_value(arm_fault_addr);
    wd.scratch6().write_value(packed6);
    wd.scratch7().write_value(packed7);
}

/// Write the 8 POWMAN scratch registers with emulator and panic context.
///
/// `panic_line_or_r1` — for panics: the source line number; for UNALIGNED
///   HardFaults: the stacked R1 value; otherwise 0.  Only the low 16 bits
///   are stored (panic line numbers and stacked register lo16 both fit).
///
/// `stack_headroom` — bytes between the pre-fault MSP and `_stack_end`.
///   Packed into the upper 16 bits of scratch[5].
#[inline(never)]
fn write_powman_scratch_from_context(
    ctx: Option<&super::CrashContextSnapshot>,
    panic_file: Option<[u8; 8]>,
    panic_line_or_r1: u32,
    stack_headroom: u16,
) {
    let pm = POWMAN;

    let rom_id = ctx.map(|c| u32::from_le_bytes(c.rom_id_prefix)).unwrap_or(0);
    pm.scratch(0).write_value(rom_id);

    let af_bc = ctx
        .map(|c| {
            (c.gb_a as u32)
                | ((c.gb_f as u32) << 8)
                | ((c.gb_b as u32) << 16)
                | ((c.gb_c as u32) << 24)
        })
        .unwrap_or(0);
    pm.scratch(1).write_value(af_bc);

    let de_hl = ctx
        .map(|c| {
            (c.gb_d as u32)
                | ((c.gb_e as u32) << 8)
                | ((c.gb_h as u32) << 16)
                | ((c.gb_l as u32) << 24)
        })
        .unwrap_or(0);
    pm.scratch(2).write_value(de_hl);

    let sp_ly = ctx
        .map(|c| {
            (c.gb_sp as u32) | ((c.ppu_ly as u32) << 16) | ((c.ppu_stat as u32) << 24)
        })
        .unwrap_or(0);
    pm.scratch(3).write_value(sp_ly);

    pm.scratch(4).write_value(ctx.map(|c| c.gb_cycle_lo).unwrap_or(0));

    // Pack: upper 16 bits = stack_headroom, lower 16 bits = panic_line_or_r1.
    let scratch5 = ((stack_headroom as u32) << 16) | (panic_line_or_r1 & 0xFFFF);
    pm.scratch(5).write_value(scratch5);

    let file = panic_file.unwrap_or([0u8; 8]);
    pm.scratch(6).write_value(u32::from_le_bytes(file[0..4].try_into().unwrap()));
    pm.scratch(7).write_value(u32::from_le_bytes(file[4..8].try_into().unwrap()));
}
