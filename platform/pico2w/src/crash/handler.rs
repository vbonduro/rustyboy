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
//!       or stack-protector failure LR when HAS_STACK_CHK_FAIL_LR is set
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
//!          stack_headroom: bytes between SP-at-fault and the faulting core's
//!            stack limit — core 0's `_stack_end`, or core 1's
//!            `__core1_stack_limit` when the FAULT_ON_CORE1 flag is set
//!            (HAS_STACK_OVERFLOW set → overflow depth, clear → remaining headroom)
//!          panic_line_or_r1: panic source line (panics) or stacked R1 lo16
//!            (UNALIGNED HardFaults)
//! [6] = panic_file[0..4]             first 4 ASCII bytes of source file
//!       or, for UNALIGNED HardFaults, the captured pre-handler R4
//! [7] = panic_file[4..8]             next 4 ASCII bytes
//!       or, for UNALIGNED HardFaults, the stacked R12
//! ```

use cortex_m_rt::ExceptionFrame;
use rp_pac::{POWMAN, SIO, WATCHDOG};

use super::{flags, CrashKind, CRASH_CONTEXT, CRASH_MAGIC};

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
static mut HARDFAULT_EXTRA_REGS: [u32; 8] = [0; 8];

// cortex-m-rt's normal HardFault trampoline only exposes the hardware-stacked
// registers. Crash #5 needs the multiword-store base register, which is callee
// saved (`r4` in the current route_bus_events restore), so capture r4-r11 before
// a Rust prologue can reuse them.
#[cfg(target_arch = "arm")]
core::arch::global_asm!(
    r#"
    .section .HardFaultTrampoline,"ax",%progbits
    .global HardFault
    .type HardFault,%function
    .thumb_func
HardFault:
    mov r0, lr
    movs r1, #4
    tst r0, r1
    bne 1f
    mrs r0, msp
    b 2f
1:
    mrs r0, psp
2:
    ldr r1, ={extra}
    stmia r1!, {{r4, r5, r6, r7}}
    mov r2, r8
    str r2, [r1]
    adds r1, #4
    mov r2, r9
    str r2, [r1]
    adds r1, #4
    mov r2, r10
    str r2, [r1]
    adds r1, #4
    mov r2, r11
    str r2, [r1]
    mov r1, lr
    b.w {handler}
    .size HardFault, . - HardFault
    "#,
    extra = sym HARDFAULT_EXTRA_REGS,
    handler = sym hard_fault_rust,
);

// ---------------------------------------------------------------------------
// Linker-defined stack-limit symbols (lowest valid address of each stack).
// Declared in memory.x; the linker resolves them to concrete addresses.
//   _stack_end          — bottom of the core 0 MSP stack.
//   __core1_stack_limit — bottom of core 1's dedicated 8 KiB stack.
// ---------------------------------------------------------------------------
extern "C" {
    static _stack_end: u32;
    static __core1_stack_limit: u32;
}

// ---------------------------------------------------------------------------
// ARM System Control Block register addresses (Cortex-M33 architectural).
// ---------------------------------------------------------------------------
const SCB_CFSR: *const u32 = 0xE000_ED28 as *const u32;
const SCB_HFSR: *const u32 = 0xE000_ED2C as *const u32;
const SCB_MMFAR: *const u32 = 0xE000_ED34 as *const u32;
const SCB_BFAR: *const u32 = 0xE000_ED38 as *const u32;

// CFSR bit positions.
const CFSR_MMARVALID: u32 = 1 << 7; // MMFAR holds a valid address
const CFSR_BFARVALID: u32 = 1 << 15; // BFAR holds a valid address
/// UFSR.UNDEFINSTR (bit 16). BFAR/MMFAR are not updated for this fault, so for
/// diagnostics we store the faulting instruction halfwords in `arm_fault_addr`.
const CFSR_UNDEFINSTR: u32 = 1 << 16;
/// UFSR.UNALIGNED (bit 24) — neither BFAR nor MMFAR is valid for this fault;
/// we repurpose `arm_fault_addr` to hold the stacked R0 from the exception
/// frame so the decoder can display the actual misaligned address.
const CFSR_UNALIGNED: u32 = 1 << 24;

// ---------------------------------------------------------------------------
// HardFault handler
// ---------------------------------------------------------------------------

#[cfg(target_arch = "arm")]
#[inline(never)]
unsafe extern "C" fn hard_fault_rust(ef: *const ExceptionFrame, exc_return: u32) -> ! {
    let ef = unsafe { &*ef };

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
    } else if cfsr & CFSR_UNDEFINSTR != 0 {
        fault_instruction_word(ef.pc()).unwrap_or(0)
    } else if cfsr & CFSR_UNALIGNED != 0 {
        ef.r0()
    } else {
        0
    };

    // Compute stack headroom / overflow depth from the pre-exception SP.
    // NOTE: `ef + 32` is only correct for the basic 8-word frame with no
    // alignment pad. On this hard-float target the frame can be 104 bytes
    // (FP extended frame) plus a 4-byte aligner — see `sp_before_exception`.
    let sp_before = sp_before_exception(ef, exc_return);

    // Store sp_before (the faulted thread's SP) rather than pre-handler r4.
    // For INVSTATE from trigger's `pop {r7,pc}`, sp_before - 4 is the exact
    // address of the corrupted saved-LR slot, giving us the DWT watchpoint
    // address for the next crash hunt iteration.
    let hardfault_tail = Some([sp_before as u32, ef.r12()]);

    // For UNALIGNED faults, also capture R1 into POWMAN scratch[5] lo16.
    // Pattern B is `lda r1, [r1]` — stacked R1 = faulting address.
    let arm_r1_for_unaligned = if cfsr & CFSR_UNALIGNED != 0 {
        ef.r1()
    } else {
        0
    };

    commit_crash_and_reset(
        CrashKind::HardFault,
        flags::HAS_ARM_REGS | flags::HAS_HARDFAULT_EXTENDED_REGS,
        sp_before,
        ef.pc(),
        ef.lr(),
        cfsr,
        hfsr,
        fault_addr,
        None,
        hardfault_tail,
        arm_r1_for_unaligned,
    )
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

    // Read the current SP directly — in a panic handler there's no exception frame.
    let sp = cortex_m::register::msp::read() as usize;

    // Software panics do not have an exception frame, so ARM register values
    // are all zero.
    commit_crash_and_reset(
        CrashKind::Panic,
        flags::HAS_PANIC_LOC,
        sp,
        0,
        0,
        0,
        0,
        0,
        Some(file_bytes),
        None,
        line,
    )
}

// ---------------------------------------------------------------------------
// Shared crash-commit helper
// ---------------------------------------------------------------------------

/// Capture the crash context, build the packed scratch words, write both sets
/// of scratch registers, and trigger a clean `sys_reset`.
///
/// Both `hard_fault_rust` and the `#[panic_handler]` funnel through here;
/// the caller supplies the kind-specific inputs and the two extra flag bits
/// that distinguish a HardFault from a Panic:
///
/// - `kind_flags` — additional flags that are always set for this crash kind
///   (e.g. `HAS_ARM_REGS | HAS_HARDFAULT_EXTENDED_REGS` for HardFault,
///    `HAS_PANIC_LOC` for Panic).
/// - `sp` — pre-fault stack pointer (used to compute stack headroom).
/// - `arm_*` — ARM exception frame fields; all zero for panics.
/// - `panic_file` — 8-byte filename buffer; `None` for HardFaults.
/// - `diagnostic_words` — `[sp_before, r12]` for HardFaults; `None` for panics.
/// - `panic_line_or_r1` — panic source line (panics) or stacked R1 lo16
///   (UNALIGNED HardFaults); 0 otherwise.
#[inline(never)]
fn commit_crash_and_reset(
    kind: CrashKind,
    kind_flags: u8,
    sp: usize,
    arm_pc: u32,
    arm_lr: u32,
    arm_cfsr: u32,
    arm_hfsr: u32,
    arm_fault_addr: u32,
    panic_file: Option<[u8; 8]>,
    diagnostic_words: Option<[u32; 2]>,
    panic_line_or_r1: u32,
) -> ! {
    // Attempt to read emulator state from the global context.
    let ctx = CRASH_CONTEXT.snapshot();

    // Identify the faulting core so we measure against the correct stack.
    let core = current_core();

    let (stack_headroom, overflowed) = compute_stack_info(sp, core);

    // Common flag bits: emulator state availability, stack overflow, core ID.
    let common_flags = if ctx.is_some() {
        flags::HAS_GB_STATE | flags::HAS_ROM_INFO
    } else {
        0
    } | if overflowed {
        flags::HAS_STACK_OVERFLOW
    } else {
        0
    } | if core == 1 { flags::FAULT_ON_CORE1 } else { 0 };

    let f = kind_flags | common_flags;

    // Build scratch[6]: [crash_kind:8 | flags:8 | ram_bank:8 | _:8]
    let ram_bank = ctx.as_ref().map(|c| c.ram_bank).unwrap_or(0);
    let packed6 = (kind as u32) | ((f as u32) << 8) | ((ram_bank as u32) << 16);

    // Build scratch[7]: [rom_bank:16 | gb_pc:16]
    let rom_bank = ctx.as_ref().map(|c| c.rom_bank).unwrap_or(0);
    let gb_pc = ctx.as_ref().map(|c| c.gb_pc).unwrap_or(0);
    let packed7 = ((rom_bank as u32) << 16) | (gb_pc as u32);

    // Capture a stack window for the crash record. `commit_crash_and_reset` does
    // not build the record here — it stashes state in the watchdog/POWMAN
    // scratch registers and resets, and the record is assembled on the NEXT
    // boot — so the window goes to `.uninit`, which survives the reset the same
    // way the scratch registers do.
    //
    // `sp` is ALREADY the pre-fault stack pointer: `hard_fault_rust` passes
    // `sp_before_exception()`, which has itself added the frame size (32 or 104
    // for an FP-extended frame) and the STKALIGN pad. Do not re-add them — an
    // earlier version did, which pushed the window 32 bytes past the return-
    // address slot it exists to capture, and made its own xPSR probe read an
    // unrelated word.
    #[cfg(target_arch = "arm")]
    unsafe {
        crate::crash::stack_snapshot::capture_stack_window(sp);
    }

    write_watchdog_scratch(
        arm_pc,
        arm_lr,
        arm_cfsr,
        arm_hfsr,
        arm_fault_addr,
        packed6,
        packed7,
    );
    write_powman_scratch_from_context(
        ctx.as_ref(),
        panic_file,
        diagnostic_words,
        panic_line_or_r1,
        stack_headroom,
    );

    cortex_m::peripheral::SCB::sys_reset()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// ID of the core currently executing this handler (0 = core 0, 1 = core 1).
///
/// The fault/panic handler runs on whichever core faulted, so `SIO.CPUID`
/// identifies the core whose stack we must measure against.
#[inline(always)]
fn current_core() -> u32 {
    SIO.cpuid().read()
}

fn fault_instruction_word(pc: u32) -> Option<u32> {
    let addr = (pc & !1) as usize;
    if !is_readable_code_addr(addr) || !is_readable_code_addr(addr + 2) {
        return None;
    }

    // Safety: restricted to RP2350 XIP/SRAM windows; Thumb instructions are
    // halfword-aligned, so two volatile halfword reads avoid unaligned u32 loads.
    let lo = unsafe { core::ptr::read_volatile(addr as *const u16) as u32 };
    let hi = unsafe { core::ptr::read_volatile((addr + 2) as *const u16) as u32 };
    Some(lo | (hi << 16))
}

fn is_readable_code_addr(addr: usize) -> bool {
    const XIP_START: usize = 0x1000_0000;
    const XIP_END: usize = 0x1040_0000;
    const SRAM_START: usize = 0x2000_0000;
    const SRAM_END: usize = 0x2008_2000;

    (XIP_START..XIP_END).contains(&addr) || (SRAM_START..SRAM_END).contains(&addr)
}

/// Lowest valid address (the limit) of `core`'s stack.
///
/// Each stack grows downward, so an overflow means the SP has dropped *below*
/// this limit.  Core 0 uses the linker's `_stack_end`; core 1 uses its
/// dedicated `__core1_stack_limit`.
#[inline(always)]
fn stack_limit_for_core(core: u32) -> usize {
    // SAFETY: both are linker symbols with no associated data; taking their
    // address is safe.  We cast to usize for arithmetic only.
    unsafe {
        if core == 1 {
            &__core1_stack_limit as *const u32 as usize
        } else {
            &_stack_end as *const u32 as usize
        }
    }
}

/// Reconstruct the pre-exception stack pointer from the exception frame.
///
/// `ef + 32` is correct ONLY for the basic 8-word frame with no alignment pad.
/// On this hard-float (`thumbv8m.main-none-eabihf`) target two things enlarge
/// the frame and must be added back, or every absolute spill-slot address
/// derived from this SP is wrong (this exact error mis-named a watchpoint slot
/// during the G4 hunt — see docs/investigations/compiler-codegen-investigation.md):
///   * FP context: if EXC_RETURN.FType (bit 4) is 0, the hardware reserved an
///     extended frame (26 words = 104 bytes) instead of the basic 32 bytes.
///   * 8-byte realignment: if stacked xPSR bit 9 is set, the hardware inserted
///     a 4-byte pad word below the frame to keep it 8-byte aligned.
///
/// `exc_return` is the handler-entry LR (EXC_RETURN), forwarded by the
/// HardFault trampoline.
#[cfg(target_arch = "arm")]
#[inline(always)]
fn sp_before_exception(ef: &ExceptionFrame, exc_return: u32) -> usize {
    let ef_addr = ef as *const ExceptionFrame as usize;
    // EXC_RETURN bit 4 (FType): 1 = basic frame, 0 = extended (FP) frame.
    let frame_size = if exc_return & (1 << 4) == 0 { 104 } else { 32 };
    // Stacked xPSR bit 9: a 4-byte aligner pad was inserted below the frame.
    let pad = if ef.xpsr() & (1 << 9) != 0 { 4 } else { 0 };
    ef_addr + frame_size + pad
}

/// Compute `(headroom_or_depth: u16, overflowed: bool)` for a pre-fault stack
/// pointer, measured against the faulting `core`'s stack limit:
/// - If `sp_before >= limit`: `headroom = sp_before - limit`, not overflowed.
/// - If `sp_before <  limit`: `depth    = limit - sp_before`, overflowed.
///
/// The return value is clamped to `u16::MAX`.
#[inline(always)]
fn compute_stack_info(sp_before: usize, core: u32) -> (u16, bool) {
    let limit = stack_limit_for_core(core);
    if sp_before >= limit {
        let headroom = (sp_before - limit).min(u16::MAX as usize) as u16;
        (headroom, false)
    } else {
        let depth = (limit - sp_before).min(u16::MAX as usize) as u16;
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
    // Capture DMA channel write-addresses before the reset wipes them.
    // Stored in a .uninit static so the bytes survive the soft reset and can
    // be read back in check_and_commit on the next boot.
    capture_dma_snapshot();

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

/// Snapshot DMA channels 0-7 WRITE_ADDR + a busy-channel bitmask into the
/// `.uninit` static below.  Called once per crash, before `sys_reset`, so the
/// data survives into the next boot's `check_and_commit`.
fn capture_dma_snapshot() {
    // RP2350 DMA register layout (channels 0-15):
    //   Base: 0x5000_0000
    //   Per channel: stride 0x40
    //   +0x00 = READ_ADDR, +0x04 = WRITE_ADDR, +0x08 = TRANS_COUNT,
    //   +0x0C = CTRL_TRIG  (bit 24 = BUSY)
    const DMA_BASE: usize = 0x5000_0000;
    const WRITE_ADDR_OFF: usize = 0x04;
    const CTRL_TRIG_OFF: usize = 0x0C;
    const CHANNEL_STRIDE: usize = 0x40;
    const BUSY_BIT: u32 = 1 << 24;

    let mut busy_mask: u32 = 0;
    let mut write_addrs = [0u32; 16];
    for i in 0usize..16 {
        let base = DMA_BASE + i * CHANNEL_STRIDE;
        let ctrl = unsafe { ((base + CTRL_TRIG_OFF) as *const u32).read_volatile() };
        if ctrl & BUSY_BIT != 0 {
            busy_mask |= 1 << i;
        }
        write_addrs[i] = unsafe { ((base + WRITE_ADDR_OFF) as *const u32).read_volatile() };
    }
    unsafe {
        DMA_CRASH_SNAPSHOT[0] = DMA_SNAPSHOT_SENTINEL;
        DMA_CRASH_SNAPSHOT[1] = busy_mask;
        // ch0-ch6 go into the crash record (slots 2-8)
        for i in 0..7usize {
            DMA_CRASH_SNAPSHOT[2 + i] = write_addrs[i];
        }
        // ch7-ch15 (9 channels) stored in slots 9-17 for boot-time defmt logging
        for i in 7..16usize {
            DMA_CRASH_SNAPSHOT[2 + i] = write_addrs[i];
        }
    }
}

/// Sentinel value in DMA_CRASH_SNAPSHOT[0] that distinguishes a populated
/// snapshot from uninitialised memory left over from a previous boot.
pub const DMA_SNAPSHOT_SENTINEL: u32 = 0xD4A0_C12A;

/// DMA channel write-address snapshot, populated at crash time and consumed
/// at boot by `check_and_commit`.  Lives in `.uninit` so it is not zeroed by
/// the cortex-m-rt reset handler and survives the `sys_reset()` soft reboot.
///
/// Layout: [0] = sentinel, [1] = busy_mask, [2..18] = WRITE_ADDR ch0-15.
/// Slots 2-8 (ch0-6) are also committed to the flash crash record.
/// Slots 9-17 (ch7-15) are logged via defmt only — see check_and_commit.
#[unsafe(no_mangle)]
#[link_section = ".uninit"]
pub static mut DMA_CRASH_SNAPSHOT: [u32; 18] = [0; 18];

/// Write the 8 POWMAN scratch registers with emulator and panic context.
///
/// `panic_line_or_r1` — for panics: the source line number; for UNALIGNED
///   HardFaults: the stacked R1 value; otherwise 0.  Only the low 16 bits
///   are stored (panic line numbers and stacked register lo16 both fit).
///
/// `stack_headroom` — bytes between the pre-fault SP and the faulting core's
///   stack limit. Packed into the upper 16 bits of scratch[5].
#[inline(never)]
fn write_powman_scratch_from_context(
    ctx: Option<&super::CrashContextSnapshot>,
    panic_file: Option<[u8; 8]>,
    diagnostic_words: Option<[u32; 2]>,
    panic_line_or_r1: u32,
    stack_headroom: u16,
) {
    let pm = POWMAN;

    let rom_id = ctx
        .map(|c| u32::from_le_bytes(c.rom_id_prefix))
        .unwrap_or(0);
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
        .map(|c| (c.gb_sp as u32) | ((c.ppu_ly as u32) << 16) | ((c.ppu_stat as u32) << 24))
        .unwrap_or(0);
    pm.scratch(3).write_value(sp_ly);

    pm.scratch(4)
        .write_value(ctx.map(|c| c.gb_cycle_lo).unwrap_or(0));

    // Pack: upper 16 bits = stack_headroom, lower 16 bits = panic_line_or_r1.
    let scratch5 = ((stack_headroom as u32) << 16) | (panic_line_or_r1 & 0xFFFF);
    pm.scratch(5).write_value(scratch5);

    let (tail0, tail1) = if let Some(words) = diagnostic_words {
        (words[0], words[1])
    } else {
        let file = panic_file.unwrap_or([0u8; 8]);
        (
            u32::from_le_bytes(file[0..4].try_into().unwrap()),
            u32::from_le_bytes(file[4..8].try_into().unwrap()),
        )
    };
    pm.scratch(6).write_value(tail0);
    pm.scratch(7).write_value(tail1);
}
