/// MPU configuration helpers for RP2350 / ARMv8-M.
///
/// Each core has its own PMSAv8-M MPU; these functions set up one region
/// apiece — core 0 protects its own `.data` / RAM-code region read-only,
/// core 1 protects core 0's MSP stack read-only.  Both fire DACCVIOL
/// (MemManage → HardFault) on an illicit write, with the stacked PC
/// identifying the exact corrupt-store instruction.

/// Bounds of the RAM-resident `.data` thunk/code region, as an MPU (base, limit)
/// pair — or `None` if the range is empty.
///
/// Start above the SEGGER RTT control block (0x30 bytes, legitimately written by
/// defmt), rounded up to the 32-byte MPU granule.
///
/// The end needs care. RLAR's LIMIT is INCLUSIVE and its low 5 bits are forced
/// to 0x1F, so a region always ends at `limit | 0x1F` — it rounds UP to the
/// enclosing granule. `(edata - 1) & !0x1F` therefore reaches PAST `__edata` and
/// marks the first bytes of `.bss` read-only, since `__edata` is essentially
/// never 32-byte aligned. That is a real fault generator, not a theoretical one:
/// with `-Z stack-protector=strong` (`__edata = 0x20003ab4`) it made the
/// firmware unbootable — a genuine DACCVIOL, MMFAR = 0x20003ab8 = `__sbss`
/// exactly, faulting in an ordinary early `atomic_store::<u8>` into `.bss`.
/// Stepping back a full granule keeps the region below `__edata`, giving up at
/// most 32 bytes of thunk coverage to stay off `.bss`.
///
/// Both cores call this so they fence exactly the same bytes; an asymmetric
/// fence would produce a MemManage fault on one core only, which is miserable to
/// diagnose.
#[cfg(target_arch = "arm")]
unsafe fn data_thunk_bounds() -> Option<(u32, u32)> {
    unsafe extern "C" {
        static _SEGGER_RTT: u8;
        static __edata: u8;
    }
    let rtt_addr = core::ptr::addr_of!(_SEGGER_RTT) as usize;
    let base = ((rtt_addr + 0x30 + 0x1F) & !0x1F) as u32;
    let edata = core::ptr::addr_of!(__edata) as u32;
    let limit = edata.saturating_sub(0x20) & !0x1F;
    (limit > base).then_some((base, limit))
}

/// Arm `region` as privileged read-only, executable (XN=0), over `[base, limit]`.
#[cfg(target_arch = "arm")]
unsafe fn arm_ro_exec_region(region: u32, base: u32, limit: u32) {
    const MPU_RNR: *mut u32 = 0xE000_ED98 as *mut u32;
    const MPU_RBAR: *mut u32 = 0xE000_ED9C as *mut u32;
    const MPU_RLAR: *mut u32 = 0xE000_EDA0 as *mut u32;
    MPU_RNR.write_volatile(region);
    MPU_RBAR.write_volatile(base | 0x1C); // SH=11, AP=10 priv-RO, XN=0
    MPU_RLAR.write_volatile(limit | 1);
}

/// Configure Core 0's PMSAv8-M MPU with one privileged-read-only region:
/// - region 0: .data RAM code from `__sdata` to immediately before `_SEGGER_RTT`
///
/// Core 0 never legitimately writes this range after startup. Any write
/// fires DACCVIOL (MemManage → HardFault) with the exact writer PC stacked.
///
/// The upper bound is derived from `_SEGGER_RTT` so adding another RAM-resident
/// function cannot silently leave the end of `.data` writable.
#[cfg(target_arch = "arm")]
#[inline(never)]
pub unsafe fn setup_core0_data_mpu() {
    unsafe extern "C" {
        static _SEGGER_RTT: u8;
        static __edata: u8;
    }

    const MPU_TYPE: *mut u32 = 0xE000_ED90 as *mut u32;
    const MPU_CTRL: *mut u32 = 0xE000_ED94 as *mut u32;
    const MPU_RNR: *mut u32 = 0xE000_ED98 as *mut u32;
    const MPU_RBAR: *mut u32 = 0xE000_ED9C as *mut u32;
    const MPU_RLAR: *mut u32 = 0xE000_EDA0 as *mut u32;
    const MPU_MAIR0: *mut u32 = 0xE000_EDC0 as *mut u32;

    let dregion = unsafe { (MPU_TYPE.read_volatile() >> 8) & 0xFF };
    defmt::info!("core0 MPU setup: DREGION={=u32}", dregion);
    if dregion == 0 {
        defmt::warn!("core0 MPU: no MPU regions available — .data protection disabled");
        return;
    }

    let rtt_addr = core::ptr::addr_of!(_SEGGER_RTT) as usize;
    let data_limit = (rtt_addr - 1) & !0x1F;

    unsafe {
        MPU_CTRL.write_volatile(0); // disable MPU before configuring regions

        // Configure MAIR0 index 0: Normal memory, write-back, read/write-allocate.
        MPU_MAIR0.write_volatile(0xFF);

        // Region 0: __sdata through the final 32-byte block below _SEGGER_RTT,
        // privileged-read-only, execute OK.
        //   RBAR: BASE=0x20000000, SH=11=inner-shareable, AP=10=priv-RO,
        //         XN=0 (execution allowed) → 0x2000_001C
        MPU_RNR.write_volatile(0);
        MPU_RBAR.write_volatile(0x2000_001C);
        MPU_RLAR.write_volatile(data_limit as u32 | 1);

        // Region 1: the REST of `.data`, above the RTT control block.
        //
        // Region 0's comment claims deriving the bound from `_SEGGER_RTT` stops
        // the end of `.data` being silently writable. It does the opposite: the
        // linker places `_SEGGER_RTT` in the MIDDLE of `.data`, so region 0 ends
        // at 0x2000373f while `.data` actually runs to `__edata` (0x20003920) —
        // leaving ~480 bytes unprotected, and those bytes are exactly the
        // `__Thumbv7ABSLongThunk_*` long-branch table. That is the one part of
        // `.data` we have direct crash evidence against (a fault whose branch
        // target register held the CORRECT address for `cb::sla_u8`, whose thunk
        // lives at 0x200037d4, yet landed at a wild heap PC — i.e. the thunk's
        // own instructions/literal pool were corrupted). It is also why every
        // "Core N never writes .data" result so far was vacuous for that range.
        //
        // Bounds and the RLAR rounding hazard are documented on
        // `data_thunk_bounds()`.
        if dregion > 1 {
            if let Some((base, limit)) = data_thunk_bounds() {
                arm_ro_exec_region(1, base, limit);
                defmt::info!(
                    "core0 MPU region 1 armed: .data thunks RO [{=u32:#010x}..={=u32:#010x}]",
                    base,
                    limit + 0x1F
                );
            }
        }

        // Region 2: CORE 1's STACK, privileged-read-only *for core 0*.
        //
        // Core 0 must never write core 1's stack. The existing regions fence
        // core 1 out of core 0's stack, but the symmetric direction was never
        // covered — core 0 could scribble on CORE1_STACK with nothing to trap it.
        //
        // Evidence this matters (recovered 2026-08-05 from the WATCHDOG scratch
        // registers, an uncommitted core-1 crash record that the normal crash log
        // could never show because only CORE 1 reset, so no boot ever ran
        // check_and_commit): core 1 took a precise bus fault
        // (CFSR=0x00008200 BFARVALID|PRECISERR) at address 0x0c164015 with
        // LR=0x20000325 = ApuPeripheral::produce_samples. The sample_buffer Vec
        // was INTACT (cap=2048, ptr=0x2002b7bc), and ptr+len*2=0x2002b960 is
        // nowhere near the faulting address — so the buffer was fine and `self`
        // itself was garbage, i.e. the `worker` reference held on CORE 1's STACK
        // had been clobbered. Same fault-address family as two earlier core-1
        // crashes (0x0c0e0015, 0x0c160015).
        //
        // If core 0 is the writer, this region turns that silent clobber into an
        // immediate DACCVIOL with MMFAR = the exact address and the stacked PC =
        // the exact storing instruction. If it never fires, core 0 is excluded
        // and the writer is core 1 itself or DMA.
        //
        // XN=1 here (unlike the .data regions): core 0 never executes from core
        // 1's stack. CORE1_STACK is 0x20080000..0x20082000 (8 KiB, SRAM8/9), both
        // ends already 32-byte aligned so no granule rounding is needed.
        // Core 0 legitimately writes core 1's stack while setting up its initial
        // frame in `spawn_core1`, so arming it this early makes boot fault. That
        // is not hypothetical: doing so produced an immediate
        // CFSR=0x00000082 (MMARVALID|DACCVIOL), MMFAR=0x20081ffc (top word of
        // CORE1_STACK), PC=core::ptr::write — which also proves the region works.

        // ENABLE=1, PRIVDEFENA=1 (other addresses use default map), HFNMIENA=0
        // (MPU disabled during fault handlers so the crash handler can read freely).
        MPU_CTRL.write_volatile(0x0000_0005);
    }
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
    defmt::info!(
        "core0 MPU armed: r0=.data RO [0x20000000..={=usize:#010x}]",
        data_limit + 0x1F
    );
}

#[cfg_attr(target_arch = "arm", link_section = ".data")]
/// Configure Core 1's PMSAv8-M MPU with two privileged-read-only regions.
///
/// Region 0: Core 0's MSP stack, AP=10 (priv RO), XN=1, SH=inner-shareable.
/// Region 1: `.data` RAM code + long-branch thunk table, AP=10 (priv RO), **XN=0**
///           (Core 1 *executes* `.data`-resident code such as
///           `PpuPeripheral::render_scanline`, so execution must stay permitted —
///           XN=1 here would fault instantly).
/// PRIVDEFENA=1 leaves all other addresses with full default access.
///
/// When Core 1 writes anywhere in either range → MemManage fault (CFSR.DACCVIOL=1,
/// MMFAR=faulting address) → escalates to HardFault → existing handler records
/// stacked PC = the exact corrupt store instruction.
///
/// # Why region 1 exists (bug #5)
///
/// Core 0 has protected `.data` since the region-0 setup in
/// [`setup_core0_data_mpu`], and has never once tripped DACCVIOL — so Core 0 is
/// not the writer. Core 1, however, was only ever fenced off from *Core 0's
/// stack*; `.data` was left fully writable from Core 1, so the 2026-06-12
/// "Core 1 definitively ruled out" result only ruled it out for stack writes and
/// never covered `.data` at all.
///
/// That gap matters because `.data` (all code + literal pools, no mutable
/// statics — any post-startup write to it is by definition the bug) holds the
/// `__Thumbv7ABSLongThunk_*` long-branch table at its top. A live capture
/// (git a8ea0259, cycle ~2.58B) faulted with `LR` = `Sm83::cb_reg`, `r12` =
/// `cb::sla_u8` — i.e. the branch *target register was correct* — yet landed at
/// `PC = 0x2002b5ec` (heap, INVSTATE). A correct target register with a wrong
/// destination means the thunk's own instructions/literal pool were corrupted,
/// not the register: the writer hit the thunk table itself.
#[cfg(target_arch = "arm")]
pub unsafe fn setup_core1_mpu() {
    const MPU_TYPE: *mut u32 = 0xE000_ED90 as *mut u32;
    const MPU_CTRL: *mut u32 = 0xE000_ED94 as *mut u32;
    const MPU_RNR: *mut u32 = 0xE000_ED98 as *mut u32;
    const MPU_RBAR: *mut u32 = 0xE000_ED9C as *mut u32;
    const MPU_RLAR: *mut u32 = 0xE000_EDA0 as *mut u32;
    const MPU_MAIR0: *mut u32 = 0xE000_EDC0 as *mut u32;

    let dregion = (MPU_TYPE.read_volatile() >> 8) & 0xFF;
    defmt::info!("core1 MPU setup: DREGION={=u32}", dregion);
    if dregion == 0 {
        defmt::warn!("core1 MPU: no MPU regions present — protection disabled");
        return;
    }

    // Disable before reconfiguring.
    MPU_CTRL.write_volatile(0);
    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    // Attr 0 = Normal memory, outer+inner write-back, read/write-allocate (0xFF).
    MPU_MAIR0.write_volatile(0xFF);

    // Region 0: Core 0 stack [_stack_end, _stack_start]. Derive the base from the
    // linker symbol (NOT a hardcode) so the region tracks SRAM layout shifts. §G11:
    // a stale hardcode (0x20066B60) sat ~1 KB below the real stack bottom, inside
    // the defmt RTT buffer (0x20066b3c–0x20066f3c); after the copy_dma_step .data
    // fix grew SRAM, legit core-1 RTT logging wrote into the covered range and
    // crash-looped on MMFAR DACCVIOL.
    //
    // Round the base UP to the 32-byte MPU granule, not down. `_stack_end` is the
    // end of `.uninit` (`__euninit`) and is essentially never 32-byte aligned —
    // it is 0x20066b08 as of this writing. Rounding DOWN puts the region base
    // BELOW `__euninit`, which makes the tail of the last `.uninit` object
    // privileged read-only for core 1. That object is `DMA_CRASH_SNAPSHOT`, and
    // `capture_dma_snapshot()` writes all 18 of its words unconditionally from
    // core 1's panic handler — which runs in thread mode with the MPU live,
    // unlike the HardFault path (MPU_CTRL.HFNMIENA=0 disables the MPU there).
    // The store took a DACCVIOL that escalated to HardFault, so a core-1 panic
    // was recorded as a bogus HardFault and lost its file/line.
    //
    // Rounding up cannot do this: the base lands at or above `__euninit`, so no
    // `.uninit` byte is ever inside the region. The cost is that up to 31 bytes
    // at the very bottom of core 0's stack go unfenced by core 1 — the deepest
    // point, reached only at near-overflow, which MSPLIM already traps.
    extern "C" {
        static _stack_end: u32;
    }
    let stack_bottom = core::ptr::addr_of!(_stack_end) as u32;
    let base = (stack_bottom + 0x1F) & !0x1F; // 32-byte aligned, at or above __euninit
                                              //   RBAR: BASE | SH=11(b4:3) | AP=10(b2:1) | XN=1(b0) = base | 0x1D
                                              //   RLAR: LIMIT=0x2007FFE0 | AttrIndx=0(b3:1) | EN=1(b0) = 0x2007FFE1
    MPU_RNR.write_volatile(0);
    MPU_RBAR.write_volatile(base | 0x1D);
    MPU_RLAR.write_volatile(0x2007_FFE1);
    defmt::info!(
        "core1 MPU region 0 base (from _stack_end) = 0x{=u32:08x}",
        base
    );

    // Region 1: `.data` RAM code + thunk table, priv-RO but still executable.
    // Same encoding and same `_SEGGER_RTT`-derived limit as Core 0's region 0
    // (see `setup_core0_data_mpu`) so both cores fence exactly the same bytes.
    //   RBAR: BASE=0x20000000 | SH=11 | AP=10 (priv-RO) | XN=0 → 0x2000_001C
    //   RLAR: LIMIT | EN=1
    let data_limit = if dregion > 1 {
        unsafe extern "C" {
            static _SEGGER_RTT: u8;
            static __edata: u8;
        }
        let rtt_addr = core::ptr::addr_of!(_SEGGER_RTT) as usize;
        let limit = ((rtt_addr - 1) & !0x1F) as u32;
        MPU_RNR.write_volatile(1);
        MPU_RBAR.write_volatile(0x2000_001C);
        MPU_RLAR.write_volatile(limit | 1);

        // Region 2: the thunk table above the RTT control block — see the long
        // rationale on Core 0's region 1 in `setup_core0_data_mpu`. Region 1
        // stops at the RTT block (which defmt legitimately writes), leaving the
        // `__Thumbv7ABSLongThunk_*` table unprotected; that gap is why "Core 1
        // never writes .data" did not actually cover the range we have crash
        // evidence against.
        if dregion > 2 {
            if let Some((base, limit)) = data_thunk_bounds() {
                arm_ro_exec_region(2, base, limit);
                defmt::info!(
                    "core1 MPU region 2 armed: .data thunks RO [{=u32:#010x}..={=u32:#010x}]",
                    base,
                    limit + 0x1F
                );
            }
        }
        limit
    } else {
        defmt::warn!("core1 MPU: <2 regions available — .data protection disabled");
        0
    };

    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    // Enable: PRIVDEFENA(b2)=1 background full-access for non-covered addresses,
    // HFNMIENA(b1)=0 MPU off in HardFault so our handler can read crash info,
    // ENABLE(b0)=1.
    MPU_CTRL.write_volatile(0x0000_0005);

    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    defmt::info!(
        "core1 MPU armed: region 0 = [{=u32:#010x}, 0x2007FFFF] priv-RO (Core 0 stack), \
         region 1 = [0x20000000, {=u32:#010x}] priv-RO+exec (.data RAM code/thunks)",
        base,
        data_limit + 0x1F
    );
}
