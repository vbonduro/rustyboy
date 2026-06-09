//! `rb-flash` — the `cargo run` runner for the pico2w firmware.
//!
//! probe-rs `--verify` has been observed to false-pass a corrupt `.data` page
//! on this board (XIP-cache coherency), letting the firmware boot into corrupt
//! RAM-resident code and crash deep in the emulator. To catch that, the
//! firmware self-checks its `.data` load image against `EXPECTED_DATA_CRC` on
//! boot (see `integrity` in main.rs).
//!
//! This runner computes that CRC from the freshly built ELF, patches it into
//! the image, then flashes the patched copy with probe-rs. A flash that
//! corrupts `.data` makes the firmware exit via semihosting, so probe-rs (and
//! therefore `cargo run`) returns non-zero — the user re-runs to retry. We do
//! not auto-reflash, to avoid needless flash wear.
//!
//! ## Watchdog
//!
//! Before programming, the runner clears the target's hardware watchdog
//! (`WATCHDOG.CTRL.ENABLE`). The firmware arms a watchdog at boot; an armed
//! 10 s watchdog outlives the 30-40 s flash and resets the chip mid-write —
//! corrupting the image (which surfaces as wild-PC boot faults that look like
//! "flash corruption"). probe-rs's `pause_on_debug` only pauses the watchdog
//! while the core is *halted*, not while it's running the flash loader, so it
//! isn't sufficient on its own. Clearing it here — on whatever firmware is
//! currently running, before we touch flash — fixes this on every flash,
//! including the first. The firmware re-arms it on the next boot as normal
//! (`cargo run` keeps the cores under debug, so the freshly flashed image's
//! watchdog never bites the *next* flash either, because we clear it again).
//!
//! Build once: `cargo build -p xtask --release` (produces target/release/rb-flash).
//! Wired up via `runner` in platform/pico2w/.cargo/config.toml.

use anyhow::{anyhow, Context, Result};
use object::read::elf::ProgramHeader;
use object::{Object, ObjectSection, ObjectSymbol};
use std::process::Command;

/// RP2350 `WATCHDOG.CTRL` register (base 0x400d_8000, offset 0). `ENABLE` is
/// bit 30; writing 0 clears it (and the pause bits — irrelevant once disabled;
/// the `TIME` field is read-only).
const WATCHDOG_CTRL_ADDR: u32 = 0x400d_8000;

/// Best-effort: clear the target's watchdog so it can't reset the chip
/// mid-flash. Failure is non-fatal — we warn and let the flash proceed (worst
/// case the flash is as flaky as before, and a retry recovers).
fn disable_target_watchdog() {
    let addr = format!("{WATCHDOG_CTRL_ADDR:#010x}");
    // probe-rs write --chip RP235x b32 0x400d8000 0x0
    let result = Command::new("probe-rs")
        .args(["write", "--chip", "RP235x", "b32", &addr, "0x0"])
        .status();
    match result {
        Ok(s) if s.success() => {
            eprintln!("rb-flash: watchdog disabled (WATCHDOG.CTRL.ENABLE cleared) before flash");
        }
        Ok(s) => {
            eprintln!(
                "rb-flash: warning: could not disable target watchdog (probe-rs write exited {}); \
                 flash may be interrupted by a watchdog reset — retry if it fails",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            eprintln!("rb-flash: warning: could not spawn probe-rs to disable watchdog: {e}");
        }
    }
}

/// probe-rs invocation. Mirrors the historical runner (single-buffered +
/// verify); the CRC guard is the backstop for what verify misses.
const PROBE_RS_ARGS: &[&str] = &[
    "run",
    "--chip",
    "RP235x",
    "--speed",
    "1000",
    "--disable-double-buffering",
    "--verify",
];

/// CRC-32/ISO-HDLC (reflected, poly 0xEDB88320). Must stay byte-for-byte
/// identical to `integrity::crc32` in the firmware.
fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn main() -> Result<()> {
    // cargo invokes the runner as `rb-flash <path-to-elf> [extra args...]`.
    let mut args = std::env::args().skip(1).peekable();
    let elf_path = args
        .next()
        .ok_or_else(|| anyhow!("usage: rb-flash <elf> [probe-rs extra args]"))?;
    let extra: Vec<String> = args.collect();

    let bytes = std::fs::read(&elf_path).with_context(|| format!("reading {elf_path}"))?;
    let mut patched = bytes.clone();

    let file = object::File::parse(&*bytes).context("parsing ELF")?;

    // Reconstruct the exact bytes that will live in flash across the whole
    // firmware image — [__start_block_addr, IMAGE_CRC) — and CRC them. This is
    // the same region the firmware re-reads from XIP flash at boot
    // (`integrity::verify_image`), so it catches a corrupt .text/.rodata page,
    // not just .data.
    let sym_addr = |name: &str| -> Result<u64> {
        file.symbols()
            .find(|s| s.name() == Ok(name))
            .map(|s| s.address())
            .ok_or_else(|| anyhow!("symbol {name} not found — is this the pico2w firmware ELF?"))
    };
    let img_start = sym_addr("__start_block_addr")?;
    let crc_addr = sym_addr("IMAGE_CRC")?; // end of CRC-covered region (start of .end_block)
    let sidata = sym_addr("__sidata")?; // .data load address (LMA) in flash
    if crc_addr <= img_start {
        return Err(anyhow!("IMAGE_CRC {crc_addr:#x} <= start {img_start:#x}"));
    }

    let _ = sidata; // (kept for clarity; LMA now comes from the program headers)

    // Reconstruct from ELF *LOAD segments* (program headers), not sections:
    // probe-rs flashes whole segments, whose `p_filesz` includes inter-section
    // alignment padding as 0x00 bytes. A section-based reconstruction misses
    // that padding (it would leave 0xFF there) and the CRC would falsely
    // mismatch the real flash. Gaps *between* segments are never written, so
    // NOR flash reads them as 0xFF — initialise the buffer to match.
    let elf = object::read::elf::ElfFile32::<object::Endianness>::parse(&*bytes)
        .context("parsing ELF32 program headers")?;
    let endian = elf.endian();
    let mut image = vec![0xFFu8; (crc_addr - img_start) as usize];
    for ph in elf.elf_program_headers() {
        if ph.p_type(endian) != object::elf::PT_LOAD {
            continue;
        }
        let paddr = ph.p_paddr(endian) as u64; // flash (load) address
        let offset = ph.p_offset(endian) as usize;
        let filesz = ph.p_filesz(endian) as usize;
        if filesz == 0 {
            continue;
        }
        let seg = bytes
            .get(offset..offset + filesz)
            .ok_or_else(|| anyhow!("segment file range {offset:#x}+{filesz:#x} out of bounds"))?;
        // Place the overlap of [paddr, paddr+filesz) with [img_start, crc_addr).
        let seg_end = paddr + filesz as u64;
        if seg_end <= img_start || paddr >= crc_addr {
            continue;
        }
        let dst_start = paddr.max(img_start);
        let dst_end = seg_end.min(crc_addr);
        let buf_off = (dst_start - img_start) as usize;
        let seg_off = (dst_start - paddr) as usize;
        let n = (dst_end - dst_start) as usize;
        image[buf_off..buf_off + n].copy_from_slice(&seg[seg_off..seg_off + n]);
    }
    let crc = crc32(&image);

    // Patch the IMAGE_CRC word (in .end_block) in the output file.
    let sec = file
        .sections()
        .find(|s| {
            s.file_range().is_some() && crc_addr >= s.address() && crc_addr < s.address() + s.size()
        })
        .ok_or_else(|| anyhow!("no section with file bytes contains IMAGE_CRC {crc_addr:#x}"))?;
    let (sec_off, _) = sec.file_range().unwrap();
    let file_off = sec_off as usize + (crc_addr - sec.address()) as usize;
    patched
        .get_mut(file_off..file_off + 4)
        .ok_or_else(|| anyhow!("CRC slot {file_off:#x} out of bounds"))?
        .copy_from_slice(&crc.to_le_bytes());

    let out_path = format!("{elf_path}.rbcrc");
    std::fs::write(&out_path, &patched).with_context(|| format!("writing {out_path}"))?;
    eprintln!("rb-flash: image crc {crc:#010x} over [{img_start:#010x}, {crc_addr:#010x}) -> IMAGE_CRC");

    // Kill any live watchdog before programming so it can't reset the chip
    // mid-flash (see module docs). Must happen right before the flash, on
    // whatever firmware is currently running.
    disable_target_watchdog();

    let status = Command::new("probe-rs")
        .args(PROBE_RS_ARGS)
        .args(&extra)
        .arg(&out_path)
        .status()
        .context("spawning probe-rs (is probe-rs-tools installed?)")?;

    std::process::exit(status.code().unwrap_or(1));
}
