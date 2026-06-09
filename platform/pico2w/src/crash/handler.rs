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

use super::{flags, CrashKind, CRASH_CONTEXT, CRASH_MAGIC, TRANSPORT_SMASH_DIAG};
use crate::dwt_watch;

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
static mut HARDFAULT_EXTRA_REGS: [u32; 8] = [0; 8];

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
static mut DEBUG_MONITOR_EXTRA_REGS: [u32; 8] = [0; 8];

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
    b.w {handler}
    .size HardFault, . - HardFault
    "#,
    extra = sym HARDFAULT_EXTRA_REGS,
    handler = sym hard_fault_rust,
);

// DWT data watchpoints enter through DebugMonitor, not HardFault. Capture the
// callee-saved registers the same way as the HardFault trampoline: for the
// route_bus_events `stm r4!, {...}` path, pre-handler r4 is the store
// destination.
#[cfg(target_arch = "arm")]
core::arch::global_asm!(
    r#"
    .section .DebugMonitorTrampoline,"ax",%progbits
    .global DebugMonitor
    .type DebugMonitor,%function
    .thumb_func
DebugMonitor:
    mov r12, lr
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
    push {{r12, lr}}
    bl {handler}
    pop {{r12, lr}}
    bx r12
    .size DebugMonitor, . - DebugMonitor
    "#,
    extra = sym DEBUG_MONITOR_EXTRA_REGS,
    handler = sym debug_monitor_rust,
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
/// Sentinel stored in `arm_cfsr` for firmware DWT/DebugMonitor watchpoint hits.
/// It is intentionally outside the architectural CFSR bit pattern space.
const CFSR_DWT_WATCHPOINT: u32 = 0xD717_0001;
/// Sentinel stored in `arm_cfsr` when `route_bus_events` sees an impossible
/// `GameBoyMemory.events` VecDeque header before calling `drain_into`.
const CFSR_ROUTE_DRAIN_GUARD: u32 = 0xD917_0001;
/// Sentinel stored in `arm_cfsr` for a core-1 `shared`/`worker` pointer
/// tripwire. `arm_hfsr`/`arm_fault_addr` hold the observed pointers.
const CFSR_CORE1_POINTER_GUARD: u32 = 0xC011_0001;
/// Sentinel stored in `arm_cfsr` for a failed `live_ppu_snapshot` RefCell borrow
/// on core 1. `arm_hfsr` is the raw borrow word; `arm_fault_addr` is render ver.
const CFSR_LIVE_PPU_BORROW_GUARD: u32 = 0xC011_0002;
/// Sentinel stored in `arm_cfsr` when OAM DMA is about to form an impossible
/// destination slice. `arm_hfsr` packs source/progress/count.
const CFSR_DMA_OAM_GUARD: u32 = 0xD6A0_0001;
/// Sentinel stored in `arm_cfsr` when the main `GameBoy.memory` Box pointer
/// differs from the pointer captured at initialization.
const CFSR_GAMEBOY_MEMORY_POINTER_GUARD: u32 = 0xC011_0003;
/// Sentinel stored in `arm_cfsr` when an explicit stack-canary checkpoint sees
/// that the protected function's canary word changed.
const CFSR_STACK_CANARY_CHANGE_GUARD: u32 = 0xC011_0004;
/// Sentinel stored in `arm_cfsr` by the guarded global allocator when an
/// allocation fails or returns an impossible pointer.
const CFSR_HEAP_ALLOC_GUARD: u32 = 0xC011_0005;

// ---------------------------------------------------------------------------
// HardFault handler
// ---------------------------------------------------------------------------

#[cfg(target_arch = "arm")]
#[inline(never)]
unsafe extern "C" fn hard_fault_rust(ef: *const ExceptionFrame) -> ! {
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

    // Attempt to read emulator state from the global context.
    let ctx = CRASH_CONTEXT.snapshot();

    // Identify the faulting core so we measure against the correct stack.
    let core = current_core();

    // Compute stack headroom / overflow depth from the pre-exception SP.
    // The hardware pushes 8 words (32 bytes) before entering the exception handler,
    // so the pre-fault SP = exception frame pointer + 32.
    let sp_before = ef as *const ExceptionFrame as usize + 32;
    let (stack_headroom, overflowed) = compute_stack_info(sp_before, core);

    let f = flags::HAS_ARM_REGS
        | if ctx.is_some() {
            flags::HAS_GB_STATE | flags::HAS_ROM_INFO
        } else {
            0
        }
        | if overflowed {
            flags::HAS_STACK_OVERFLOW
        } else {
            0
        }
        | if core == 1 { flags::FAULT_ON_CORE1 } else { 0 };

    let hardfault_tail = {
        let r4 =
            unsafe { (core::ptr::addr_of!(HARDFAULT_EXTRA_REGS) as *const u32).read_volatile() };
        Some([r4, ef.r12()])
    };

    let f = f | if hardfault_tail.is_some() {
        flags::HAS_HARDFAULT_EXTENDED_REGS
    } else {
        0
    };

    // Build the packed word for scratch[6].
    let ram_bank = ctx.as_ref().map(|c| c.ram_bank).unwrap_or(0);
    let packed6 = (CrashKind::HardFault as u32) | ((f as u32) << 8) | ((ram_bank as u32) << 16);

    // Build the packed word for scratch[7].
    let rom_bank = ctx.as_ref().map(|c| c.rom_bank).unwrap_or(0);
    let gb_pc = ctx.as_ref().map(|c| c.gb_pc).unwrap_or(0);
    let packed7 = ((rom_bank as u32) << 16) | (gb_pc as u32);

    // For UNALIGNED faults, also capture R1 into POWMAN scratch[5] lo16.
    // Pattern B is `lda r1, [r1]` — stacked R1 = faulting address.
    let arm_r1_for_unaligned = if cfsr & CFSR_UNALIGNED != 0 {
        ef.r1()
    } else {
        0
    };

    write_watchdog_scratch(ef.pc(), ef.lr(), cfsr, hfsr, fault_addr, packed6, packed7);
    write_powman_scratch_from_context(
        ctx.as_ref(),
        None,
        hardfault_tail,
        arm_r1_for_unaligned,
        stack_headroom,
    );

    cortex_m::peripheral::SCB::sys_reset()
}

#[cfg(target_arch = "arm")]
/// A watched `VecDeque<BusEvent>` header write is legitimate only while its
/// header invariants hold: small cap, heap/POOL buffer pointer, `head < cap`,
/// and `len <= cap`. The #5 corruptor can leave individually small words that
/// still make the deque impossible, so checking each word independently misses
/// the `drain_into` variant.
#[inline(always)]
fn is_plausible_queue_ptr(v: u32) -> bool {
    (0x2002_0000..0x2006_0000).contains(&v)
}

#[cfg(target_arch = "arm")]
#[inline(always)]
unsafe fn read_watched_header_words(base: u32) -> [u32; 4] {
    [
        unsafe { (base as *const u32).read_volatile() },
        unsafe { (base.wrapping_add(4) as *const u32).read_volatile() },
        unsafe { (base.wrapping_add(8) as *const u32).read_volatile() },
        unsafe { (base.wrapping_add(12) as *const u32).read_volatile() },
    ]
}

#[cfg(target_arch = "arm")]
#[inline(always)]
fn first_bad_queue_header_word(words: &[u32; 4]) -> Option<usize> {
    const MAX_REASONABLE_CAP: u32 = 0x1000;

    let cap = words[0];
    let ptr = words[1];
    let head = words[2];
    let len = words[3];

    if cap > MAX_REASONABLE_CAP {
        return Some(0);
    }

    if cap == 0 {
        if ptr >= 0x1000 && !is_plausible_queue_ptr(ptr) {
            return Some(1);
        }
        if head != 0 {
            return Some(2);
        }
        if len != 0 {
            return Some(3);
        }
        return None;
    }

    if !is_plausible_queue_ptr(ptr) {
        return Some(1);
    }
    if head >= cap {
        return Some(2);
    }
    if len > cap {
        return Some(3);
    }
    None
}

#[cfg(target_arch = "arm")]
unsafe extern "C" fn debug_monitor_rust(ef: *const ExceptionFrame) {
    let ef = unsafe { &*ef };

    let hit = dwt_watch::watch_hit();
    // Multiword header stores can trip the comparator on a small cap/head/len
    // word. Snapshot the whole VecDeque header and record if the combined
    // invariants are impossible.
    let base = dwt_watch::current_watch_base();
    if base == 0 {
        dwt_watch::clear_debug_status();
        return;
    }
    if dwt_watch::current_watch_is_raw_word() {
        let bad_address = hit.address;
        if dwt_watch::current_watch_uses_dma_filter()
            && should_ignore_valid_dma_watch_hit(bad_address)
        {
            dwt_watch::clear_debug_status();
            return;
        }

        cortex_m::interrupt::disable();
        dwt_watch::clear_debug_status();

        let ctx = CRASH_CONTEXT.snapshot();
        let core = current_core();
        let sp_before = ef as *const ExceptionFrame as usize + 32;
        let (stack_headroom, overflowed) = compute_stack_info(sp_before, core);

        let diagnostic_tail = {
            let r4 = unsafe {
                (core::ptr::addr_of!(DEBUG_MONITOR_EXTRA_REGS) as *const u32).read_volatile()
            };
            Some([r4, ef.r12()])
        };

        let f = flags::HAS_ARM_REGS
            | flags::HAS_HARDFAULT_EXTENDED_REGS
            | if ctx.is_some() {
                flags::HAS_GB_STATE | flags::HAS_ROM_INFO
            } else {
                0
            }
            | if overflowed {
                flags::HAS_STACK_OVERFLOW
            } else {
                0
            }
            | if core == 1 { flags::FAULT_ON_CORE1 } else { 0 };

        let ram_bank = ctx.as_ref().map(|c| c.ram_bank).unwrap_or(0);
        let packed6 = (CrashKind::HardFault as u32) | ((f as u32) << 8) | ((ram_bank as u32) << 16);

        let rom_bank = ctx.as_ref().map(|c| c.rom_bank).unwrap_or(0);
        let gb_pc = ctx.as_ref().map(|c| c.gb_pc).unwrap_or(0);
        let packed7 = ((rom_bank as u32) << 16) | (gb_pc as u32);

        write_watchdog_scratch(
            ef.pc(),
            ef.lr(),
            CFSR_DWT_WATCHPOINT,
            hit.function,
            bad_address,
            packed6,
            packed7,
        );
        write_powman_scratch_from_context(
            ctx.as_ref(),
            None,
            diagnostic_tail,
            ef.r1(),
            stack_headroom,
        );

        cortex_m::peripheral::SCB::sys_reset()
    }
    let header_words = unsafe { read_watched_header_words(base) };
    let bad_index = if let Some(index) = first_bad_queue_header_word(&header_words) {
        index
    } else {
        dwt_watch::clear_debug_status();
        return;
    };
    let bad_address = base.wrapping_add((bad_index as u32) * 4);

    cortex_m::interrupt::disable();
    dwt_watch::clear_debug_status();

    let ctx = CRASH_CONTEXT.snapshot();
    let core = current_core();
    let sp_before = ef as *const ExceptionFrame as usize + 32;
    let (stack_headroom, overflowed) = compute_stack_info(sp_before, core);

    let diagnostic_tail = {
        let r4 = unsafe {
            (core::ptr::addr_of!(DEBUG_MONITOR_EXTRA_REGS) as *const u32).read_volatile()
        };
        Some([r4, ef.r12()])
    };

    let f = flags::HAS_ARM_REGS
        | flags::HAS_HARDFAULT_EXTENDED_REGS
        | if ctx.is_some() {
            flags::HAS_GB_STATE | flags::HAS_ROM_INFO
        } else {
            0
        }
        | if overflowed {
            flags::HAS_STACK_OVERFLOW
        } else {
            0
        }
        | if core == 1 { flags::FAULT_ON_CORE1 } else { 0 };

    let ram_bank = ctx.as_ref().map(|c| c.ram_bank).unwrap_or(0);
    let packed6 = (CrashKind::HardFault as u32) | ((f as u32) << 8) | ((ram_bank as u32) << 16);

    let rom_bank = ctx.as_ref().map(|c| c.rom_bank).unwrap_or(0);
    let gb_pc = ctx.as_ref().map(|c| c.gb_pc).unwrap_or(0);
    let packed7 = ((rom_bank as u32) << 16) | (gb_pc as u32);

    write_watchdog_scratch(
        ef.pc(),
        ef.lr(),
        CFSR_DWT_WATCHPOINT,
        hit.function,
        bad_address,
        packed6,
        packed7,
    );
    write_powman_scratch_from_context(ctx.as_ref(), None, diagnostic_tail, ef.r1(), stack_headroom);

    cortex_m::peripheral::SCB::sys_reset()
}

#[cfg(target_arch = "arm")]
fn should_ignore_valid_dma_watch_hit(hit_address: u32) -> bool {
    let addresses = dwt_watch::current_watch_addresses();
    let dma_tag_word = addresses[0];
    let dma_payload_word = addresses[1];
    if dma_tag_word == 0
        || dma_payload_word == 0
        || (hit_address != dma_tag_word && hit_address != dma_payload_word)
    {
        return false;
    }

    let tag = unsafe { (dma_tag_word as *const u32).read_volatile() };
    let payload = unsafe { (dma_payload_word as *const u32).read_volatile() };
    dma_words_are_valid_for_diagnostics(tag, payload)
}

#[cfg(target_arch = "arm")]
fn dma_words_are_valid_for_diagnostics(tag: u32, payload: u32) -> bool {
    if tag == 0 {
        return true;
    }
    if tag != 1 {
        return false;
    }

    let source = payload & 0xFFFF;
    let progress = (payload >> 16) & 0xFF;
    source & 0x00FF == 0 && progress <= 160
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyboy_route_drain_guard(
    cap: u32,
    ptr: u32,
    head: u32,
    len: u32,
    bad_index: u32,
) -> ! {
    let route_lr: u32;
    unsafe {
        core::arch::asm!(
            "mov {}, lr",
            out(reg) route_lr,
            options(nomem, nostack, preserves_flags),
        );
    }

    record_synthetic_hardfault(
        route_lr,
        bad_index,
        CFSR_ROUTE_DRAIN_GUARD,
        cap,
        ptr,
        Some([head, len]),
        bad_index,
    )
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyboy_core1_pointer_guard(
    site: u32,
    shared: u32,
    worker: u32,
    want_shared: u32,
    want_worker: u32,
) -> ! {
    let guard_lr: u32;
    unsafe {
        core::arch::asm!(
            "mov {}, lr",
            out(reg) guard_lr,
            options(nomem, nostack, preserves_flags),
        );
    }

    record_synthetic_hardfault(
        guard_lr,
        site,
        CFSR_CORE1_POINTER_GUARD,
        shared,
        worker,
        Some([want_shared, want_worker]),
        site,
    )
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyboy_live_ppu_borrow_guard(
    site: u32,
    borrow_word: u32,
    render_version: u32,
    shared: u32,
    worker: u32,
) -> ! {
    let guard_lr: u32;
    unsafe {
        core::arch::asm!(
            "mov {}, lr",
            out(reg) guard_lr,
            options(nomem, nostack, preserves_flags),
        );
    }

    record_synthetic_hardfault(
        guard_lr,
        site,
        CFSR_LIVE_PPU_BORROW_GUARD,
        borrow_word,
        render_version,
        Some([shared, worker]),
        site,
    )
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyboy_dma_oam_guard(
    site: u32,
    packed_dma: u32,
    actual_src: u32,
    dst: u32,
    count: u32,
) -> ! {
    let guard_lr: u32;
    unsafe {
        core::arch::asm!(
            "mov {}, lr",
            out(reg) guard_lr,
            options(nomem, nostack, preserves_flags),
        );
    }

    record_synthetic_hardfault(
        guard_lr,
        site,
        CFSR_DMA_OAM_GUARD,
        packed_dma,
        actual_src,
        Some([dst, count]),
        site,
    )
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyboy_gameboy_memory_pointer_guard(
    site: u32,
    gameboy: u32,
    memory: u32,
    want_memory: u32,
    field_addr: u32,
) -> ! {
    let guard_lr: u32;
    unsafe {
        core::arch::asm!(
            "mov {}, lr",
            out(reg) guard_lr,
            options(nomem, nostack, preserves_flags),
        );
    }

    record_synthetic_hardfault(
        guard_lr,
        site,
        CFSR_GAMEBOY_MEMORY_POINTER_GUARD,
        gameboy,
        memory,
        Some([want_memory, field_addr]),
        site,
    )
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub extern "C" fn rustyboy_find_stack_canary(caller_sp: u32, _site: u32) -> u32 {
    let guard = crate::stack_chk::__stack_chk_guard as u32;
    let mut addr = caller_sp;
    let end = caller_sp.saturating_add(128);
    while addr < end {
        let value = unsafe { (addr as *const u32).read_volatile() };
        if value == guard {
            return addr;
        }
        addr = addr.wrapping_add(core::mem::size_of::<u32>() as u32);
    }
    0
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rustyboy_stack_canary_change_guard(
    site: u32,
    canary_addr: u32,
    before: u32,
    after: u32,
    bus_addr: u32,
    memory: u32,
) -> ! {
    let guard_lr: u32;
    unsafe {
        core::arch::asm!(
            "mov {}, lr",
            out(reg) guard_lr,
            options(nomem, nostack, preserves_flags),
        );
    }

    record_synthetic_hardfault(
        guard_lr,
        site,
        CFSR_STACK_CANARY_CHANGE_GUARD,
        canary_addr,
        after,
        Some([before, memory]),
        bus_addr,
    )
}

#[cfg(target_arch = "arm")]
#[unsafe(no_mangle)]
pub extern "C" fn rustyboy_heap_alloc_guard(
    site: u32,
    ptr: u32,
    size: u32,
    align: u32,
    heap_start: u32,
    heap_end: u32,
) -> ! {
    let guard_lr: u32;
    unsafe {
        core::arch::asm!(
            "mov {}, lr",
            out(reg) guard_lr,
            options(nomem, nostack, preserves_flags),
        );
    }

    record_synthetic_hardfault(
        guard_lr,
        site,
        CFSR_HEAP_ALLOC_GUARD,
        size,
        align,
        Some([ptr, heap_start]),
        heap_end,
    )
}

#[cfg(target_arch = "arm")]
#[inline(never)]
fn record_synthetic_hardfault(
    arm_pc: u32,
    arm_lr: u32,
    arm_cfsr: u32,
    arm_hfsr: u32,
    arm_fault_addr: u32,
    diagnostic_words: Option<[u32; 2]>,
    panic_line_or_r1: u32,
) -> ! {
    cortex_m::interrupt::disable();
    dwt_watch::clear_debug_status();

    let ctx = CRASH_CONTEXT.snapshot();
    let core = current_core();
    let sp = cortex_m::register::msp::read() as usize;
    let (stack_headroom, overflowed) = compute_stack_info(sp, core);

    let f = flags::HAS_ARM_REGS
        | if diagnostic_words.is_some() {
            flags::HAS_HARDFAULT_EXTENDED_REGS
        } else {
            0
        }
        | if ctx.is_some() {
            flags::HAS_GB_STATE | flags::HAS_ROM_INFO
        } else {
            0
        }
        | if overflowed {
            flags::HAS_STACK_OVERFLOW
        } else {
            0
        }
        | if core == 1 { flags::FAULT_ON_CORE1 } else { 0 };

    let ram_bank = ctx.as_ref().map(|c| c.ram_bank).unwrap_or(0);
    let packed6 = (CrashKind::HardFault as u32) | ((f as u32) << 8) | ((ram_bank as u32) << 16);

    let rom_bank = ctx.as_ref().map(|c| c.rom_bank).unwrap_or(0);
    let gb_pc = ctx.as_ref().map(|c| c.gb_pc).unwrap_or(0);
    let packed7 = ((rom_bank as u32) << 16) | (gb_pc as u32);

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
        None,
        diagnostic_words,
        panic_line_or_r1,
        stack_headroom,
    );

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

    // Identify the panicking core so we measure against the correct stack.
    let core = current_core();

    // Read the current SP directly — in a panic handler there's no exception frame.
    let sp = cortex_m::register::msp::read() as usize;
    let (stack_headroom, overflowed) = compute_stack_info(sp, core);

    let stack_chk_lr = crate::stack_chk::last_fail_lr() as u32;
    let transport_smash = TRANSPORT_SMASH_DIAG.take();

    let f = flags::HAS_PANIC_LOC
        | if ctx.is_some() {
            flags::HAS_GB_STATE | flags::HAS_ROM_INFO
        } else {
            0
        }
        | if overflowed {
            flags::HAS_STACK_OVERFLOW
        } else {
            0
        }
        | if core == 1 { flags::FAULT_ON_CORE1 } else { 0 }
        | if transport_smash.is_some() {
            flags::HAS_ARM_REGS
        } else if stack_chk_lr != 0 {
            flags::HAS_ARM_REGS | flags::HAS_STACK_CHK_FAIL_LR
        } else {
            0
        };

    let ram_bank = ctx.as_ref().map(|c| c.ram_bank).unwrap_or(0);
    let crash_kind = if transport_smash.is_some() {
        CrashKind::TransportSmash
    } else {
        CrashKind::Panic
    };
    let packed6 = (crash_kind as u32) | ((f as u32) << 8) | ((ram_bank as u32) << 16);

    let rom_bank = ctx.as_ref().map(|c| c.rom_bank).unwrap_or(0);
    let gb_pc = ctx.as_ref().map(|c| c.gb_pc).unwrap_or(0);
    let packed7 = ((rom_bank as u32) << 16) | (gb_pc as u32);

    let (arm_pc, arm_lr, arm_cfsr, arm_hfsr, arm_fault_addr) = if let Some(smash) = transport_smash
    {
        (
            smash.base,
            smash.cmd,
            smash.aud,
            smash.shr,
            smash.source_triplet,
        )
    } else {
        // Software panics do not have an exception frame. Stack-protector
        // panics are the exception: `__stack_chk_fail` captured LR before
        // calling into defmt.
        (0, stack_chk_lr, 0, 0, 0)
    };
    write_watchdog_scratch(
        arm_pc,
        arm_lr,
        arm_cfsr,
        arm_hfsr,
        arm_fault_addr,
        packed6,
        packed7,
    );
    write_powman_scratch_from_context(ctx.as_ref(), Some(file_bytes), None, line, stack_headroom);

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
