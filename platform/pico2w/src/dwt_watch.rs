#![cfg(target_arch = "arm")]

//! Firmware-side DWT write-watchpoints for the #5 memory-corruption hunt.
//!
//! RP2350 has one DWT bank per Cortex-M33 core, so both cores must program the
//! same victim address if we want to catch a cross-core rogue write.

use core::sync::atomic::{AtomicU32, Ordering};

use cortex_m::asm;
use rp_pac::SIO;

const DEMCR: *mut u32 = 0xE000_EDFC as *mut u32;
const DFSR: *mut u32 = 0xE000_ED30 as *mut u32;
const DWT_CTRL: *const u32 = 0xE000_1000 as *const u32;
const DWT_COMP0: usize = 0xE000_1020;
const DWT_STRIDE: usize = 0x10;

const DEMCR_MON_EN: u32 = 1 << 16;
const DEMCR_TRCENA: u32 = 1 << 24;
const DFSR_CLEAR_ALL: u32 = 0x1F;

const DWT_FUNCTION_DISABLED: u32 = 0;
// ARMv8-M DWT v2, matching OpenOCD's cortex_m_set_watchpoint path:
// write watchpoint + linked data-address comparison + 4-byte access size.
const DWT_FUNCTION_WRITE_WORD: u32 = 5 | (1 << 4) | (2 << 10); // 0x815
                                                               // ARMv8-M v2 DWT: the access size lives in FUNCTION's DATAVSIZE field (the
                                                               // `2 << 10` above = 4 bytes); DWT_MASKn must be 0 for an exact-address match.
                                                               // Verified empirically: OpenOCD's working `wp` leaves DWT_MASK0 = 0, and a
                                                               // firmware-armed `COMP=addr, MASK=0, FUNCTION=0x815` traps on the exact word.
                                                               // The previous value `3` (OpenOCD's internal struct field, never written to the
                                                               // +4 register in the v2.x path) suppressed all matches → DebugMonitor never fired.
const DWT_MASK_WORD: u32 = 0;
const DWT_FUNCTION_MATCHED: u32 = 1 << 24;
const WATCH_WORDS: usize = 4;
const WATCH_MODE_QUEUE_HEADER: u32 = 0;
const WATCH_MODE_RAW_WORD: u32 = 1;
const WATCH_MODE_RAW_DMA_WORDS: u32 = 2;

static WATCH_ADDR0: AtomicU32 = AtomicU32::new(0);
static WATCH_ADDR1: AtomicU32 = AtomicU32::new(0);
static WATCH_ADDR2: AtomicU32 = AtomicU32::new(0);
static WATCH_ADDR3: AtomicU32 = AtomicU32::new(0);
static WATCH_MODE: AtomicU32 = AtomicU32::new(WATCH_MODE_QUEUE_HEADER);
static CORE0_ARMED_KEY: AtomicU32 = AtomicU32::new(0);
static CORE1_ARMED_KEY: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy)]
pub struct WatchHit {
    pub address: u32,
    pub function: u32,
}

#[inline(always)]
pub fn publish_and_arm_watch_words(base: usize) {
    let base = base as u32;
    WATCH_ADDR0.store(base, Ordering::Release);
    WATCH_ADDR1.store(base.wrapping_add(4), Ordering::Release);
    WATCH_ADDR2.store(base.wrapping_add(8), Ordering::Release);
    WATCH_ADDR3.store(base.wrapping_add(12), Ordering::Release);
    WATCH_MODE.store(WATCH_MODE_QUEUE_HEADER, Ordering::Release);
    arm_watch_for_current_core(load_watch_addresses());
}

#[inline(always)]
pub fn publish_and_arm_raw_word(address: usize) {
    publish_and_arm_raw_slot(0, address);
}

#[inline(always)]
pub fn publish_and_arm_raw_slot(slot: usize, address: usize) {
    store_watch_address(slot, address as u32);
    WATCH_MODE.store(WATCH_MODE_RAW_WORD, Ordering::Release);
    arm_watch_for_current_core(load_watch_addresses());
}

#[inline(always)]
pub fn publish_and_arm_raw_words(addresses: [usize; WATCH_WORDS]) {
    WATCH_ADDR0.store(addresses[0] as u32, Ordering::Release);
    WATCH_ADDR1.store(addresses[1] as u32, Ordering::Release);
    WATCH_ADDR2.store(addresses[2] as u32, Ordering::Release);
    WATCH_ADDR3.store(addresses[3] as u32, Ordering::Release);
    WATCH_MODE.store(WATCH_MODE_RAW_WORD, Ordering::Release);
    arm_watch_for_current_core(load_watch_addresses());
}

#[inline(always)]
pub fn publish_and_arm_dma_words(addresses: [usize; WATCH_WORDS]) {
    WATCH_ADDR0.store(addresses[0] as u32, Ordering::Release);
    WATCH_ADDR1.store(addresses[1] as u32, Ordering::Release);
    WATCH_ADDR2.store(addresses[2] as u32, Ordering::Release);
    WATCH_ADDR3.store(addresses[3] as u32, Ordering::Release);
    WATCH_MODE.store(WATCH_MODE_RAW_DMA_WORDS, Ordering::Release);
    arm_watch_for_current_core(load_watch_addresses());
}

#[inline(always)]
pub fn arm_published_watch_words_for_current_core() {
    arm_watch_for_current_core(load_watch_addresses());
}

pub fn current_watch_base() -> u32 {
    WATCH_ADDR0.load(Ordering::Acquire)
}

#[inline(always)]
pub fn current_watch_addresses() -> [u32; WATCH_WORDS] {
    load_watch_addresses()
}

pub fn current_watch_is_raw_word() -> bool {
    matches!(
        WATCH_MODE.load(Ordering::Acquire),
        WATCH_MODE_RAW_WORD | WATCH_MODE_RAW_DMA_WORDS
    )
}

pub fn current_watch_uses_dma_filter() -> bool {
    WATCH_MODE.load(Ordering::Acquire) == WATCH_MODE_RAW_DMA_WORDS
}

pub fn watch_hit() -> WatchHit {
    let addresses = load_watch_addresses();
    let mut fallback_function = 0;
    let mut fallback_address = 0;
    let mut index = 0usize;
    while index < WATCH_WORDS {
        let address = addresses[index];
        if fallback_address == 0 {
            fallback_address = address;
        }
        let function = unsafe { read_reg(dwt_function_addr(index)) };
        if index == 0 {
            fallback_function = function;
        }
        if function & DWT_FUNCTION_MATCHED != 0 {
            return WatchHit { address, function };
        }
        index += 1;
    }
    WatchHit {
        address: fallback_address,
        function: fallback_function,
    }
}

pub fn clear_debug_status() {
    unsafe { write_reg(DFSR as usize, DFSR_CLEAR_ALL) };
}

/// Disable this core's DWT comparators (FUNCTION=0) and reset the armed-base
/// cache so the next `arm_*` reprograms them.
///
/// Kept for experiments that need to mask legitimate writes around a known hot
/// region. The current #5 capture path leaves the watch armed and filters by
/// value in DebugMonitor instead, because disarming around `gb.tick()` shifted
/// the layout-sensitive bug.
#[inline(always)]
pub fn disarm_for_current_core() {
    let armed_key = if current_core() == 1 {
        &CORE1_ARMED_KEY
    } else {
        &CORE0_ARMED_KEY
    };
    unsafe {
        let comparators = ((read_reg(DWT_CTRL as usize) >> 28) & 0xF).min(WATCH_WORDS as u32);
        let mut index = 0usize;
        while index < comparators as usize {
            write_reg(dwt_function_addr(index), DWT_FUNCTION_DISABLED);
            index += 1;
        }
        asm::dsb();
        asm::isb();
    }
    armed_key.store(0, Ordering::Release);
}

#[inline(always)]
fn arm_watch_for_current_core(addresses: [u32; WATCH_WORDS]) {
    let armed_key = if current_core() == 1 {
        &CORE1_ARMED_KEY
    } else {
        &CORE0_ARMED_KEY
    };
    let key = watch_key(addresses);
    if key == 0 {
        return;
    }
    if armed_key.load(Ordering::Acquire) == key {
        return;
    }

    unsafe { program_watch_words(addresses) };
    armed_key.store(key, Ordering::Release);
}

unsafe fn program_watch_words(addresses: [u32; WATCH_WORDS]) {
    let demcr = unsafe { read_reg(DEMCR as usize) };
    unsafe { write_reg(DEMCR as usize, demcr | DEMCR_TRCENA | DEMCR_MON_EN) };
    clear_debug_status();

    let comparators =
        ((unsafe { read_reg(DWT_CTRL as usize) } >> 28) & 0xF).min(WATCH_WORDS as u32);
    let mut index = 0usize;
    while index < comparators as usize {
        let address = addresses[index];
        if address != 0 {
            unsafe { program_word_write_watch(index, address) };
        } else {
            unsafe { write_reg(dwt_function_addr(index), DWT_FUNCTION_DISABLED) };
        }
        index += 1;
    }

    asm::dsb();
    asm::isb();
}

#[inline(always)]
fn load_watch_addresses() -> [u32; WATCH_WORDS] {
    [
        WATCH_ADDR0.load(Ordering::Acquire),
        WATCH_ADDR1.load(Ordering::Acquire),
        WATCH_ADDR2.load(Ordering::Acquire),
        WATCH_ADDR3.load(Ordering::Acquire),
    ]
}

#[inline(always)]
fn store_watch_address(slot: usize, address: u32) {
    match slot {
        0 => WATCH_ADDR0.store(address, Ordering::Release),
        1 => WATCH_ADDR1.store(address, Ordering::Release),
        2 => WATCH_ADDR2.store(address, Ordering::Release),
        3 => WATCH_ADDR3.store(address, Ordering::Release),
        _ => {}
    }
}

#[inline(always)]
fn watch_key(addresses: [u32; WATCH_WORDS]) -> u32 {
    addresses[0]
        ^ addresses[1].rotate_left(7)
        ^ addresses[2].rotate_left(13)
        ^ addresses[3].rotate_left(19)
}

unsafe fn program_word_write_watch(index: usize, address: u32) {
    unsafe { write_reg(dwt_function_addr(index), DWT_FUNCTION_DISABLED) };
    unsafe { write_reg(dwt_comp_addr(index), address) };
    unsafe { write_reg(dwt_mask_addr(index), DWT_MASK_WORD) };
    unsafe { write_reg(dwt_function_addr(index), DWT_FUNCTION_WRITE_WORD) };
}

#[inline(always)]
fn current_core() -> u32 {
    SIO.cpuid().read()
}

#[inline(always)]
fn dwt_comp_addr(index: usize) -> usize {
    DWT_COMP0 + index * DWT_STRIDE
}

#[inline(always)]
fn dwt_mask_addr(index: usize) -> usize {
    dwt_comp_addr(index) + 4
}

#[inline(always)]
fn dwt_function_addr(index: usize) -> usize {
    dwt_comp_addr(index) + 8
}

#[inline(always)]
unsafe fn read_reg(addr: usize) -> u32 {
    unsafe { (addr as *const u32).read_volatile() }
}

#[inline(always)]
unsafe fn write_reg(addr: usize, value: u32) {
    unsafe { (addr as *mut u32).write_volatile(value) };
}
