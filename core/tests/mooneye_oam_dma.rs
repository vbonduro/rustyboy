//! Integration tests using Mooneye's OAM DMA test ROMs.
//!
//! Pass/fail is detected via CPU register values after HALT:
//!   Pass: B=3, C=5, D=8, E=13, H=21, L=34 (Fibonacci)
//!   Fail: all registers set to 0x42

mod common;

use common::assert_mooneye_passed;

#[test]
fn test_mooneye_oam_dma_basic() {
    assert_mooneye_passed(
        "roms/mooneye/acceptance/oam_dma/basic.gb",
        "oam_dma/basic",
    );
}

#[test]
fn test_mooneye_oam_dma_reg_read() {
    assert_mooneye_passed(
        "roms/mooneye/acceptance/oam_dma/reg_read.gb",
        "oam_dma/reg_read",
    );
}

#[test]
#[ignore] // Requires MBC1 (ROM writes for bank switching)
fn test_mooneye_oam_dma_sources_gs() {
    assert_mooneye_passed(
        "roms/mooneye/acceptance/oam_dma/sources-GS.gb",
        "oam_dma/sources-GS",
    );
}
