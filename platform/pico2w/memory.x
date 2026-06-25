/*
 * Memory layout for Raspberry Pi Pico 2 / Pico 2W (RP2350A)
 *
 * Flash: 4MB QSPI, XIP-mapped at 0x10000000
 * RAM:   512KB striped across SRAM0-SRAM7 (8 × 64KB banks, best for general use)
 *        SRAM8/SRAM9: 4KB direct-mapped banks (dedicated use, e.g. per-core stacks)
 *
 * NOTE: the first 640 KiB of flash are reserved for the firmware image.
 * The remaining flash is available to runtime-managed data (currently a staged
 * ROM slot).
 *
 * Flash budget was expanded from 512K to 640K to accommodate ratatui/mousefood
 * UI layer added in the mousefood migration (adds ~82 KiB to .text/.rodata).
 * The firmware slot constant in flash_rom.rs was updated to match.
 *
 * Bead 1 uses a single-app layout (no bootloader).
 * Bead 8 (OTA) will restructure this into dual-bank partitions
 * for embassy-boot-rp:
 *   BOOTLOADER : ORIGIN = 0x10000000, LENGTH = 32K
 *   APP_A      : ORIGIN = 0x10008000, LENGTH = ~2M   <- active slot
 *   APP_B      : ORIGIN = 0x10200000, LENGTH = ~2M   <- OTA staging slot
 */
MEMORY {
    FLASH       : ORIGIN = 0x10000000, LENGTH = 640K
    RAM         : ORIGIN = 0x20000000, LENGTH = 512K
    CORE1_STACK : ORIGIN = 0x20080000, LENGTH = 8K
}

SECTIONS {
    /*
     * Boot ROM info block — IMAGE_DEF recognized by the RP2350 bootrom.
     * Must be within the first 4K of flash (after .vector_table) so the
     * bootrom and picotool can find it.
     */
    .start_block : ALIGN(4)
    {
        __start_block_addr = .;
        KEEP(*(.start_block));
        KEEP(*(.boot_info));
    } > FLASH

} INSERT AFTER .vector_table;

/* Move .text to start after the boot info block. */
_stext = ADDR(.start_block) + SIZEOF(.start_block);

SECTIONS {
    /*
     * Picotool / probe-rs binary info entries.
     * Pointers in the start_block header direct tools to this table.
     */
    .bi_entries : ALIGN(4)
    {
        __bi_entries_start = .;
        KEEP(*(.bi_entries));
        . = ALIGN(4);
        __bi_entries_end = .;
    } > FLASH

} INSERT AFTER .text;

SECTIONS {
    /*
     * Dedicated core 1 stack storage in SRAM8/SRAM9 so the publish seam can
     * use main striped RAM without also paying this 8 KiB out of the heap.
     */
    .core1_stack (NOLOAD) : ALIGN(32)
    {
        /* Lowest valid address of core 1's stack (it grows down from the top).
         * The crash handler compares the faulting SP against this to detect a
         * core 1 stack overflow, mirroring `_stack_end` for core 0. */
        __core1_stack_limit = .;
        KEEP(*(.core1_stack));
        __core1_stack_top = .;
    } > CORE1_STACK

} INSERT AFTER .uninit;

SECTIONS {
    /*
     * End block — can hold a signature for secure boot.
     * Placed last so it brackets the full image.
     */
    .end_block : ALIGN(4)
    {
        __end_block_addr = .;
        KEEP(*(.end_block));
    } > FLASH

} INSERT AFTER .uninit;

PROVIDE(start_to_end = __end_block_addr - __start_block_addr);
PROVIDE(end_to_start = __start_block_addr - __end_block_addr);
