//! Diagnostic test: trace LCDC in DKL2 level gameplay.
//! Run with: cargo test --test dkl2_lcdc_trace -- --nocapture --ignored

mod common;

use rustyboy_core::cpu::cpu::Cpu;
use rustyboy_core::cpu::peripheral::joypad::Button;
use rustyboy_core::cpu::registers::{Flags, Registers};
use rustyboy_core::cpu::sm83::Sm83;
use rustyboy_core::memory::GameBoyMemory;

const CYCLES_PER_FRAME: u32 = 70224;
const LCDC_ADDR: u16 = 0xFF40;
const STAT_ADDR: u16 = 0xFF41;
const LY_ADDR: u16 = 0xFF44;
const LYC_ADDR: u16 = 0xFF45;
const BGP_ADDR: u16 = 0xFF47;
const OBP0_ADDR: u16 = 0xFF48;
const SCX_ADDR: u16 = 0xFF43;
const SCY_ADDR: u16 = 0xFF42;

fn build_cpu() -> Sm83 {
    let rom_path = "/home/vbonduro/roms/extracted/Donkey Kong Land 2 (USA, Europe) (SGB Enhanced).gb";
    let rom_data = std::fs::read(rom_path).expect("DKL2 ROM not found");
    let memory = Box::new(GameBoyMemory::with_rom(rom_data));
    Sm83::new(memory)
        .with_registers(Registers {
            a: 0x01,
            f: Flags::from_bits_truncate(0xB0),
            b: 0x00,
            c: 0x13,
            d: 0x00,
            e: 0xD8,
            h: 0x01,
            l: 0x4D,
            pc: 0x0100,
            sp: 0xFFFE,
        })
        .with_dmg_state()
}

fn run_frame(cpu: &mut Sm83) {
    let start = cpu.cycle_counter();
    while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
        let _ = cpu.tick();
    }
}

fn dump_oam_summary(cpu: &Sm83) -> usize {
    let mut visible = 0;
    for i in 0..40 {
        let base = 0xFE00 + (i * 4) as u16;
        let y = cpu.read_memory(base).unwrap_or(0);
        let x = cpu.read_memory(base + 1).unwrap_or(0);
        let tile = cpu.read_memory(base + 2).unwrap_or(0);
        let attrs = cpu.read_memory(base + 3).unwrap_or(0);
        if y != 0 && y < 160 {
            eprintln!("  OAM[{:2}]: scr({:3},{:3}) tile=0x{:02X} pal={} bg_pri={}",
                i, x as i16 - 8, y as i16 - 16, tile, (attrs>>4)&1, (attrs>>7)&1);
            visible += 1;
        }
    }
    visible
}

const FFEB_ADDR: u16 = 0xFFEB;
const IE_ADDR: u16 = 0xFFFF;
const IF_ADDR: u16 = 0xFF0F;

/// Run DKL2 into level gameplay, then trace STAT interrupt firing with HRAM[0xFFEB] value.
/// This helps diagnose whether the two-phase STAT interrupt technique is executing correctly.
#[test]
#[ignore]
fn trace_dkl2_stat_interrupt_ffeb() {
    let mut cpu = build_cpu();

    // Navigate to level entry (same sequence as trace_dkl2_level_lcdc)
    for f in 0..710u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);
    }

    eprintln!("=== Tracing STAT interrupts with 0xFFEB value ===");
    eprintln!("At frame ~710 (level gameplay)");

    // Now tick-by-tick for 3 frames, tracking every STAT/VBlank interrupt dispatch
    // We detect interrupts by watching IF register transitions
    let mut prev_if: u8 = cpu.read_memory(IF_ADDR).unwrap_or(0);
    let mut prev_ly: u8 = cpu.read_memory(LY_ADDR).unwrap_or(0);
    let mut prev_lcdc: u8 = cpu.read_memory(LCDC_ADDR).unwrap_or(0);

    for frame_offset in 0..5u32 {
        let start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();

            let cur_if = cpu.read_memory(IF_ADDR).unwrap_or(0);
            let cur_ly = cpu.read_memory(LY_ADDR).unwrap_or(0);
            let cur_lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
            let ffeb = cpu.read_memory(FFEB_ADDR).unwrap_or(0);
            let ie = cpu.read_memory(IE_ADDR).unwrap_or(0);
            let lyc = cpu.read_memory(LYC_ADDR).unwrap_or(0);

            // Detect IF bit 1 (STAT) rising edge
            if (cur_if & 0x02) != 0 && (prev_if & 0x02) == 0 {
                eprintln!("  F+{} STAT INT PENDING: LY={:3} LYC={:3} LCDC=0x{:02X}(OBJ={}) IE=0x{:02X} FFEB=0x{:02X}",
                    frame_offset, cur_ly, lyc, cur_lcdc, (cur_lcdc>>1)&1, ie, ffeb);
            }
            // Detect IF bit 0 (VBlank) rising edge
            if (cur_if & 0x01) != 0 && (prev_if & 0x01) == 0 {
                eprintln!("  F+{} VBLK INT PENDING: LY={:3} LCDC=0x{:02X}(OBJ={}) FFEB=0x{:02X}",
                    frame_offset, cur_ly, cur_lcdc, (cur_lcdc>>1)&1, ffeb);
            }
            // Track LCDC changes
            if cur_lcdc != prev_lcdc {
                eprintln!("  F+{} LCDC CHANGE: LY={:3} 0x{:02X}->0x{:02X} OBJ {}->{}  FFEB=0x{:02X}",
                    frame_offset, cur_ly,
                    prev_lcdc, cur_lcdc,
                    if prev_lcdc & 2 != 0 {"ON "} else {"OFF"},
                    if cur_lcdc & 2 != 0 {"ON"} else {"OFF"},
                    ffeb);
            }
            // Track LY changes (show key scanlines)
            if cur_ly != prev_ly && (cur_ly == 0 || cur_ly == 128 || cur_ly == 144) {
                let stat = cpu.read_memory(STAT_ADDR).unwrap_or(0);
                eprintln!("  F+{} LY={:3}: LCDC=0x{:02X} STAT=0x{:02X} LYC={:3} FFEB=0x{:02X}",
                    frame_offset, cur_ly, cur_lcdc, stat, lyc, ffeb);
            }

            prev_if = cur_if;
            prev_ly = cur_ly;
            prev_lcdc = cur_lcdc;
        }
        eprintln!("--- end frame {} ---", frame_offset);
    }
}

/// Trace HRAM[0xFFEB] and HRAM[0xFFE6] values every scanline boundary during level play.
/// Check if 0xFFEB ever becomes non-zero and when 0xFFE6 is set.
#[test]
#[ignore]
fn trace_dkl2_ffeb_timeline() {
    let mut cpu = build_cpu();

    for f in 0..710u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);
    }

    eprintln!("=== Tracking 0xFFEB and 0xFFE6 across level gameplay ===");

    let mut prev_ffeb: u8 = cpu.read_memory(FFEB_ADDR).unwrap_or(0);
    let mut prev_ffe6: u8 = cpu.read_memory(0xFFE6).unwrap_or(0);
    let mut prev_ffee: u8 = cpu.read_memory(0xFFEE).unwrap_or(0);
    let mut prev_lyc: u8 = cpu.read_memory(LYC_ADDR).unwrap_or(0);
    let mut prev_pc: u16 = 0;

    // Track for 10 frames tick by tick
    for frame_offset in 0..10u32 {
        let start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();

            let ffeb = cpu.read_memory(FFEB_ADDR).unwrap_or(0);
            let ffe6 = cpu.read_memory(0xFFE6).unwrap_or(0);
            let ffee = cpu.read_memory(0xFFEE).unwrap_or(0);
            let lyc = cpu.read_memory(LYC_ADDR).unwrap_or(0);
            let ly = cpu.read_memory(LY_ADDR).unwrap_or(0);
            let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
            let pc = cpu.registers().pc;

            if ffeb != prev_ffeb {
                eprintln!("  F+{} FFEB: 0x{:02X}->0x{:02X}  LY={:3} LCDC=0x{:02X} PC=0x{:04X}",
                    frame_offset, prev_ffeb, ffeb, ly, lcdc, pc);
                prev_ffeb = ffeb;
            }
            if ffe6 != prev_ffe6 {
                eprintln!("  F+{} FFE6: 0x{:02X}->0x{:02X}  LY={:3} PC=0x{:04X}",
                    frame_offset, prev_ffe6, ffe6, ly, pc);
                prev_ffe6 = ffe6;
            }
            if ffee != prev_ffee {
                eprintln!("  F+{} FFEE: 0x{:02X}->0x{:02X}  LY={:3} PC=0x{:04X}",
                    frame_offset, prev_ffee, ffee, ly, pc);
                prev_ffee = ffee;
            }
            if lyc != prev_lyc {
                eprintln!("  F+{} LYC: {}->{}  LY={:3} PC=0x{:04X}",
                    frame_offset, prev_lyc, lyc, ly, pc);
                prev_lyc = lyc;
            }
            prev_pc = pc;
        }
        eprintln!("--- end frame {} ---", frame_offset);
    }

    // Print final HRAM state
    eprintln!("\nFinal HRAM state:");
    for addr in [0xFFE6u16, 0xFFE7, 0xFFE8, 0xFFE9, 0xFFEA, 0xFFEB, 0xFFEC, 0xFFED, 0xFFEE, 0xFFEF] {
        eprintln!("  0x{:04X} = 0x{:02X}", addr, cpu.read_memory(addr).unwrap_or(0));
    }
}

/// Trace what PC values are active during VBlank/STAT ISR execution in level gameplay.
/// Also log all PC values in the range 0x3550-0x38FF (STAT ISR region) and 0x0040-0x0060.
#[test]
#[ignore]
fn trace_dkl2_isr_pc_trace() {
    let mut cpu = build_cpu();

    for f in 0..710u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);
    }

    eprintln!("=== Tracing ISR PC values during level gameplay ===");

    let mut prev_if: u8 = cpu.read_memory(IF_ADDR).unwrap_or(0);
    let mut logged_pcs: std::collections::HashSet<u16> = std::collections::HashSet::new();

    for frame_offset in 0..3u32 {
        let start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();

            let cur_if = cpu.read_memory(IF_ADDR).unwrap_or(0);
            let pc = cpu.registers().pc;
            let ly = cpu.read_memory(LY_ADDR).unwrap_or(0);
            let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
            let ffeb = cpu.read_memory(FFEB_ADDR).unwrap_or(0);

            // Track any PC in VBlank ISR range (0x0040-0x006F) or STAT-related range
            let in_vblank_isr = pc >= 0x0040 && pc <= 0x006F;
            let in_stat_region = (pc >= 0x3550 && pc <= 0x3600) || (pc >= 0x38C0 && pc <= 0x38F0);
            let in_14720_region = pc >= 0x4720 && pc <= 0x4730; // bank-switched

            if (in_vblank_isr || in_stat_region || in_14720_region) && !logged_pcs.contains(&pc) {
                eprintln!("  F+{} PC=0x{:04X} LY={:3} LCDC=0x{:02X}(OBJ={}) IF=0x{:02X} FFEB=0x{:02X}",
                    frame_offset, pc, ly, lcdc, (lcdc>>1)&1, cur_if, ffeb);
                logged_pcs.insert(pc);
            }

            // Detect IF bit changes (interrupt pending)
            if (cur_if & 0x03) != (prev_if & 0x03) {
                eprintln!("  F+{} IF: 0x{:02X}->0x{:02X}  LY={:3} PC=0x{:04X} FFEB=0x{:02X}",
                    frame_offset, prev_if, cur_if, ly, pc, ffeb);
            }
            prev_if = cur_if;
        }
        eprintln!("--- end frame {} ---", frame_offset);
    }
}

/// Trace HRAM[0xFFE6-0xFFEB] and LYC around the frame 680 level transition.
/// Check what value LYC gets set to when entering the level, and what FFEB becomes.
#[test]
#[ignore]
fn trace_dkl2_level_entry_lyc() {
    let mut cpu = build_cpu();

    for f in 0..650u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);
    }

    eprintln!("=== Level entry HRAM/LYC trace (frames 650-720) ===");

    let mut prev_lyc = cpu.read_memory(LYC_ADDR).unwrap_or(0);
    let mut prev_ffe6 = cpu.read_memory(0xFFE6).unwrap_or(0);
    let mut prev_ffeb = cpu.read_memory(FFEB_ADDR).unwrap_or(0);
    let mut prev_stat = cpu.read_memory(STAT_ADDR).unwrap_or(0);
    let mut prev_lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);

    for f in 650u32..720 {
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);

        let lyc = cpu.read_memory(LYC_ADDR).unwrap_or(0);
        let ffe6 = cpu.read_memory(0xFFE6).unwrap_or(0);
        let ffeb = cpu.read_memory(FFEB_ADDR).unwrap_or(0);
        let stat = cpu.read_memory(STAT_ADDR).unwrap_or(0);
        let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);

        if lyc != prev_lyc || ffe6 != prev_ffe6 || ffeb != prev_ffeb || stat != prev_stat || lcdc != prev_lcdc || f % 10 == 0 {
            eprintln!("  Frame {:4}: LYC={:3} FFE6=0x{:02X} FFEB=0x{:02X} STAT=0x{:02X} LCDC=0x{:02X}(OBJ={})",
                f, lyc, ffe6, ffeb, stat, lcdc, (lcdc>>1)&1);
            prev_lyc = lyc;
            prev_ffe6 = ffe6;
            prev_ffeb = ffeb;
            prev_stat = stat;
            prev_lcdc = lcdc;
        }
    }

    // Now tick-level trace for 2 frames to catch any LYC/HRAM changes
    eprintln!("\n--- Tick-level trace for 2 frames ---");
    let mut prev_lyc_t: u8 = cpu.read_memory(LYC_ADDR).unwrap_or(0);
    let mut prev_ffe6_t: u8 = cpu.read_memory(0xFFE6).unwrap_or(0);
    let mut prev_ffeb_t: u8 = cpu.read_memory(FFEB_ADDR).unwrap_or(0);

    for _ in 0..2 {
        let start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();
            let lyc = cpu.read_memory(LYC_ADDR).unwrap_or(0);
            let ffe6 = cpu.read_memory(0xFFE6).unwrap_or(0);
            let ffeb = cpu.read_memory(FFEB_ADDR).unwrap_or(0);
            let ly = cpu.read_memory(LY_ADDR).unwrap_or(0);
            let pc = cpu.registers().pc;
            if lyc != prev_lyc_t {
                eprintln!("  LYC: {}->{}  LY={} PC=0x{:04X} FFE6=0x{:02X} FFEB=0x{:02X}",
                    prev_lyc_t, lyc, ly, pc, ffe6, ffeb);
                prev_lyc_t = lyc;
            }
            if ffe6 != prev_ffe6_t {
                eprintln!("  FFE6: 0x{:02X}->0x{:02X}  LY={} PC=0x{:04X} FFEB=0x{:02X}",
                    prev_ffe6_t, ffe6, ly, pc, ffeb);
                prev_ffe6_t = ffe6;
            }
            if ffeb != prev_ffeb_t {
                eprintln!("  FFEB: 0x{:02X}->0x{:02X}  LY={} PC=0x{:04X}",
                    prev_ffeb_t, ffeb, ly, pc);
                prev_ffeb_t = ffeb;
            }
        }
    }
}

/// Trace IME state during level gameplay to understand interrupt dispatch timing.
#[test]
#[ignore]
fn trace_dkl2_ime_timing() {
    let mut cpu = build_cpu();

    for f in 0..710u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);
    }

    eprintln!("=== IME timing trace during level play ===");

    let mut prev_ime = cpu.ime();
    let mut prev_ly: u8 = 0;

    for frame_offset in 0..3u32 {
        let start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();

            let cur_ime = cpu.ime();
            let ly = cpu.read_memory(LY_ADDR).unwrap_or(0);
            let pc = cpu.registers().pc;
            let cur_if = cpu.read_memory(IF_ADDR).unwrap_or(0);
            let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);

            // Log IME state changes
            if cur_ime != prev_ime {
                eprintln!("  F+{} IME: {}->{}  LY={:3} PC=0x{:04X} IF=0x{:02X} LCDC=0x{:02X}(OBJ={})",
                    frame_offset,
                    if prev_ime {"ON"} else {"off"},
                    if cur_ime {"ON"} else {"off"},
                    ly, pc, cur_if, lcdc, (lcdc>>1)&1);
                prev_ime = cur_ime;
            }
            // Log at key scanlines
            if ly != prev_ly && (ly == 0 || ly == 128 || ly == 144) {
                eprintln!("  F+{} LY={:3}: IME={} IF=0x{:02X} PC=0x{:04X} LCDC=0x{:02X}(OBJ={})",
                    frame_offset, ly,
                    if cur_ime {"ON"} else {"off"},
                    cur_if, pc, lcdc, (lcdc>>1)&1);
                prev_ly = ly;
            }
        }
        eprintln!("--- end frame {} ---", frame_offset);
    }
}

/// Trace IE and LCDC changes around level entry (frames 648-690) tick-by-tick.
/// In PyBoy, IE transitions 0x00->0x03 at frame ~683 and LCDC goes 0x45->0xE7.
/// This test checks whether our emulator does the same.
#[test]
#[ignore]
fn trace_dkl2_ie_level_entry() {
    let mut cpu = build_cpu();

    for f in 0..648u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 647 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);
    }

    eprintln!("=== IE/LCDC trace around level entry (frames 648-700) ===");

    let mut prev_ie   = cpu.read_memory(IE_ADDR).unwrap_or(0);
    let mut prev_lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
    let mut prev_if   = cpu.read_memory(IF_ADDR).unwrap_or(0);
    let mut prev_stat = cpu.read_memory(STAT_ADDR).unwrap_or(0);

    for f in 648u32..700 {
        if f == 650 { cpu.set_button(Button::A, false); }

        // Tick frame, watching every instruction
        let start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();

            let ie   = cpu.read_memory(IE_ADDR).unwrap_or(0);
            let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
            let if_  = cpu.read_memory(IF_ADDR).unwrap_or(0);
            let stat = cpu.read_memory(STAT_ADDR).unwrap_or(0);
            let ly   = cpu.read_memory(LY_ADDR).unwrap_or(0);
            let lyc  = cpu.read_memory(LYC_ADDR).unwrap_or(0);
            let pc   = cpu.registers().pc;

            if ie != prev_ie {
                eprintln!("  F{:04} IE:   0x{:02X}->0x{:02X}  LY={:3} LYC={:3} LCDC=0x{:02X} PC=0x{:04X}",
                    f, prev_ie, ie, ly, lyc, lcdc, pc);
                prev_ie = ie;
            }
            if lcdc != prev_lcdc {
                eprintln!("  F{:04} LCDC: 0x{:02X}->0x{:02X} OBJ {}->{}  LY={:3} IE=0x{:02X} PC=0x{:04X}",
                    f, prev_lcdc, lcdc,
                    if prev_lcdc & 2 != 0 {"ON "} else {"OFF"},
                    if lcdc & 2 != 0 {"ON"} else {"OFF"},
                    ly, ie, pc);
                prev_lcdc = lcdc;
            }
            if if_ != prev_if {
                eprintln!("  F{:04} IF:   0x{:02X}->0x{:02X}  LY={:3} LCDC=0x{:02X} PC=0x{:04X}",
                    f, prev_if, if_, ly, lcdc, pc);
                prev_if = if_;
            }
            if stat != prev_stat {
                eprintln!("  F{:04} STAT: 0x{:02X}->0x{:02X}  LY={:3} PC=0x{:04X}",
                    f, prev_stat, stat, ly, pc);
                prev_stat = stat;
            }
        }
    }
}

/// Run DKL2 into level gameplay and track all LCDC/OBJ changes.
/// Also track which specific LCDC VALUES are written at each LY transition,
/// to understand the game's mid-frame split screen technique.
#[test]
#[ignore]
fn trace_dkl2_level_lcdc() {
    let mut cpu = build_cpu();

    // Navigate to level entry then run for a while
    let mut prev_lcdc: u8 = cpu.read_memory(LCDC_ADDR).unwrap_or(0);

    for f in 0..1000u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        // Press Right to move character in level
        if f >= 710 && f < 800 { cpu.set_button(Button::Right, true); }
        if f == 800 { cpu.set_button(Button::Right, false); }
        // Press start at 850 to try to pause the game
        if f == 850 { cpu.set_button(Button::Start, true); }
        if f == 860 { cpu.set_button(Button::Start, false); }

        run_frame(&mut cpu);

        let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
        if f >= 680 && (lcdc != prev_lcdc || f % 20 == 0) {
            let bgp = cpu.read_memory(BGP_ADDR).unwrap_or(0);
            let stat = cpu.read_memory(STAT_ADDR).unwrap_or(0);
            let lyc = cpu.read_memory(LYC_ADDR).unwrap_or(0);
            let scx = cpu.read_memory(SCX_ADDR).unwrap_or(0);
            let scy = cpu.read_memory(SCY_ADDR).unwrap_or(0);
            let fb = cpu.framebuffer();
            let dark = fb.iter().filter(|&&p| p > 0).count();
            eprintln!("Frame {:4}: LCDC=0x{:02X}(OBJ={}) BGP=0x{:02X} STAT=0x{:02X} LYC={:3} SCX={:3} SCY={:3} dark={}",
                f, lcdc, (lcdc>>1)&1, bgp, stat, lyc, scx, scy, dark);
            prev_lcdc = lcdc;
        }
    }

    eprintln!("\n=== Final state (frame 1000) ===");
    let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
    let bgp = cpu.read_memory(BGP_ADDR).unwrap_or(0);
    let stat = cpu.read_memory(STAT_ADDR).unwrap_or(0);
    let lyc = cpu.read_memory(LYC_ADDR).unwrap_or(0);
    eprintln!("LCDC=0x{:02X} OBJ={} BGP=0x{:02X} STAT=0x{:02X} LYC={}", lcdc, (lcdc>>1)&1, bgp, stat, lyc);
    let n = dump_oam_summary(&cpu);
    eprintln!("Active sprites: {}", n);

    // Mid-frame LCDC trace for final 3 frames
    let mut prev = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
    let mut changes: Vec<(u8, u8, u8)> = Vec::new();
    for _ in 0..3 {
        let start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();
            let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
            let ly = cpu.read_memory(LY_ADDR).unwrap_or(0);
            if lcdc != prev {
                changes.push((ly, prev, lcdc));
                prev = lcdc;
            }
        }
    }
    eprintln!("\nMid-frame LCDC changes:");
    for (ly, old, new) in &changes {
        eprintln!("  LY={:3}: 0x{:02X}→0x{:02X}  OBJ {}→{}", ly, old, new,
            if old & 2 != 0 {"ON "} else {"OFF"}, if new & 2 != 0 {"ON"} else {"OFF"});
    }

    // Show framebuffer
    let fb = cpu.framebuffer();
    eprintln!("\nFramebuffer:");
    for row in (0..144usize).step_by(2) {
        let row_str: String = fb[row*160..(row+1)*160].iter().map(|&p| match p {
            0 => ' ', 1 => '.', 2 => 'o', 3 => '#', _ => '?',
        }).collect();
        if row_str.chars().any(|c| c != ' ') {
            eprintln!("  r{:3}: {}", row, row_str);
        }
    }
}

/// Trace the B register and LY when the CPU is at PC=0x35C2 (LY-wait loop)
/// and when EI fires at 0x35C6. This tells us what scanline the game is waiting for.
#[test]
#[ignore]
fn trace_dkl2_ly_wait_b_register() {
    let mut cpu = build_cpu();

    for f in 0..686u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);
    }

    eprintln!("=== Tracing B register at 0x35C2 LY-wait loop ===");

    let mut prev_pc: u16 = 0;
    let mut in_loop = false;
    let mut loop_b: u8 = 0;

    for frame_offset in 0..5u32 {
        let start = cpu.cycle_counter();
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();

            let pc = cpu.registers().pc;
            let b  = cpu.registers().b;
            let ly = cpu.read_memory(LY_ADDR).unwrap_or(0);
            let if_ = cpu.read_memory(IF_ADDR).unwrap_or(0);
            let ime = cpu.ime();

            // Detect entry into the LY-wait loop
            if pc == 0x35C2 && !in_loop {
                in_loop = true;
                loop_b = b;
                eprintln!("  F+{} ENTER loop 0x35C2: B={:3} LY={:3} IF={:02X} IME={}",
                    frame_offset, b, ly, if_, ime);
            }
            // Detect exit from loop (EI at 0x35C6)
            if pc == 0x35C6 && in_loop {
                in_loop = false;
                eprintln!("  F+{} EXIT  loop -> EI: B={:3} LY={:3} IF={:02X} IME={}",
                    frame_offset, b, ly, if_, ime);
            }
            // If in loop and B changed
            if in_loop && b != loop_b {
                eprintln!("  F+{} LOOP B changed: {}->{}  LY={:3} PC=0x{:04X}",
                    frame_offset, loop_b, b, ly, pc);
                loop_b = b;
            }

            prev_pc = pc;
        }
        eprintln!("--- end frame {} ---", frame_offset);
    }
}

/// Side-by-side instruction trace for comparison with PyBoy.
/// Dumps PC, registers, and key hardware state at every instruction
/// when PC is in the range 0x34A0-0x35D0 during level gameplay.
#[test]
#[ignore]
fn trace_dkl2_insn_trace() {
    let mut cpu = build_cpu();

    for f in 0..686u32 {
        if f == 120 { cpu.set_button(Button::Start, true); }
        if f == 130 { cpu.set_button(Button::Start, false); }
        if f == 350 { cpu.set_button(Button::A, true); }
        if f == 360 { cpu.set_button(Button::A, false); }
        if f == 450 { cpu.set_button(Button::Start, true); }
        if f == 460 { cpu.set_button(Button::Start, false); }
        if f == 560 { cpu.set_button(Button::A, true); }
        if f == 570 { cpu.set_button(Button::A, false); }
        if f == 640 { cpu.set_button(Button::A, true); }
        if f == 650 { cpu.set_button(Button::A, false); }
        run_frame(&mut cpu);
    }

    eprintln!("PC     A  B  C  D  E  HL   SP   F  LY IF IE IME LCDC");

    // Trace for 2 frames tick-by-tick
    for _ in 0..2u32 {
        let start = cpu.cycle_counter();
        let mut prev_pc = cpu.registers().pc;
        while cpu.cycle_counter().wrapping_sub(start) < CYCLES_PER_FRAME as u64 {
            let _ = cpu.tick();

            let r   = cpu.registers();
            let pc  = r.pc;

            // Only log when PC is in the region of interest
            if pc >= 0x34A0 && pc <= 0x35D0 {
                let ly   = cpu.read_memory(LY_ADDR).unwrap_or(0);
                let if_  = cpu.read_memory(IF_ADDR).unwrap_or(0);
                let ie   = cpu.read_memory(IE_ADDR).unwrap_or(0);
                let lcdc = cpu.read_memory(LCDC_ADDR).unwrap_or(0);
                let ime  = if cpu.ime() { 1u8 } else { 0u8 };
                let f_bits = r.f.bits();
                eprintln!("{:04X}  {:02X} {:02X} {:02X} {:02X} {:02X} {:04X} {:04X} {:02X} {:3} {:02X} {:02X}  {}  {:02X}",
                    pc,
                    r.a, r.b, r.c, r.d, r.e, r.hl(), r.sp,
                    f_bits, ly, if_, ie, ime, lcdc);
            }
            prev_pc = pc;
        }
        eprintln!("--- frame boundary ---");
    }
}
