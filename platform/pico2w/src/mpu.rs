/// MPU configuration helpers for RP2350 / ARMv8-M.
///
/// Each core has its own PMSAv8-M MPU; these functions set up one region
/// apiece — core 0 protects its own `.data` / RAM-code region read-only,
/// core 1 protects core 0's MSP stack read-only.  Both fire DACCVIOL
/// (MemManage → HardFault) on an illicit write, with the stacked PC
/// identifying the exact corrupt-store instruction.

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
/// Configure Core 1's PMSAv8-M MPU to mark Core 0's stack as privileged-read-only.
///
/// Region 0: 0x20066B60–0x2007FFFF, AP=10 (priv RO), XN=1, SH=inner-shareable.
/// PRIVDEFENA=1 leaves all other addresses with full default access.
///
/// When Core 1 writes anywhere in this range → MemManage fault (CFSR.DACCVIOL=1,
/// MMFAR=faulting address) → escalates to HardFault → existing handler records
/// stacked PC = the exact corrupt store instruction.
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
    // crash-looped on MMFAR DACCVIOL. Aligning the base DOWN to the 32-byte MPU
    // granule keeps it above the RTT buffer (which lives below _stack_end in
    // .uninit) while still covering the entire core-0 stack.
    extern "C" {
        static _stack_end: u32;
    }
    let stack_bottom = core::ptr::addr_of!(_stack_end) as u32;
    let base = stack_bottom & !0x1F; // 32-byte aligned MPU region base
                                     //   RBAR: BASE | SH=11(b4:3) | AP=10(b2:1) | XN=1(b0) = base | 0x1D
                                     //   RLAR: LIMIT=0x2007FFE0 | AttrIndx=0(b3:1) | EN=1(b0) = 0x2007FFE1
    MPU_RNR.write_volatile(0);
    MPU_RBAR.write_volatile(base | 0x1D);
    MPU_RLAR.write_volatile(0x2007_FFE1);
    defmt::info!(
        "core1 MPU region 0 base (from _stack_end) = 0x{=u32:08x}",
        base
    );

    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    // Enable: PRIVDEFENA(b2)=1 background full-access for non-covered addresses,
    // HFNMIENA(b1)=0 MPU off in HardFault so our handler can read crash info,
    // ENABLE(b0)=1.
    MPU_CTRL.write_volatile(0x0000_0005);

    cortex_m::asm::dsb();
    cortex_m::asm::isb();

    defmt::info!("core1 MPU armed: region 0 = [0x20066B60, 0x2007FFFF] priv-RO (Core 0 stack)");
}
