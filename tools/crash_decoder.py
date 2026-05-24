#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "rich>=13",
# ]
# ///
"""
rustyboy crash decoder
======================

Reads a 4 KiB crash log sector image (from internal flash) and produces a
human-readable or JSON report.

Usage examples
--------------
# Read raw sector from a file captured by picotool:
#   picotool save -o crash.bin --range 0x103FF000 +0x1000
./tools/crash_decoder.py --raw crash.bin

# Symbolize addresses with a firmware ELF (requires arm-none-eabi-addr2line):
./tools/crash_decoder.py --raw crash.bin --elf target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w

# Machine-readable JSON (for agents / CI):
./tools/crash_decoder.py --raw crash.bin --json

# Read directly via probe-rs (requires connected debug probe):
./tools/crash_decoder.py --probe --elf path/to/firmware.elf
"""

from __future__ import annotations

import argparse
import json
import struct
import subprocess
import sys
from pathlib import Path
from typing import Optional

# ---------------------------------------------------------------------------
# Wire-format constants (must match crash/mod.rs)
# ---------------------------------------------------------------------------

RECORD_SIZE = 128
SECTOR_SIZE = 4096
MAX_RECORDS_PER_SECTOR = 31

RECORD_MAGIC = b"RCRP"
SECTOR_MAGIC = b"RCLG"
CRASH_MAGIC = 0xCF_4A_53_11

CRASH_KIND_NAMES = {0: "HardFault", 1: "Panic"}

FLAG_HAS_ARM_REGS = 0x01
FLAG_HAS_GB_STATE = 0x02
FLAG_HAS_ROM_INFO = 0x04
FLAG_HAS_PANIC_LOC = 0x08

# Flash offset of the crash log sector inside the 4 MiB address space.
CRASH_LOG_FLASH_OFFSET = 0x3FF000  # relative to flash base 0x10000000
CRASH_LOG_ADDR = 0x10000000 + CRASH_LOG_FLASH_OFFSET

# ---------------------------------------------------------------------------
# Record layout  (all LE, matches crash/mod.rs byte map)
# ---------------------------------------------------------------------------
# [0..4]   magic           b"RCRP"
# [4]      schema_ver
# [5]      crash_kind
# [6]      flags
# [7]      slot_seq
# [8..11]  fw_version[3]
# [11]     _pad0
# [12..16] git_hash        u32 LE
# [16..20] arm_pc          u32 LE
# [20..24] arm_lr          u32 LE
# [24..28] arm_cfsr        u32 LE
# [28..32] arm_hfsr        u32 LE
# [32..36] arm_fault_addr  u32 LE
# [36..40] _pad1
# [40..44] rom_id_prefix   4 bytes
# [44..46] rom_bank        u16 LE
# [46]     ram_bank
# [47]     _pad2
# [48..56] gb_a,f,b,c,d,e,h,l  (8 × u8)
# [56..58] gb_sp           u16 LE
# [58..60] gb_pc           u16 LE
# [60..64] gb_cycle_lo     u32 LE
# [64]     ppu_ly
# [65]     ppu_lcdc
# [66]     ppu_stat
# [67]     _pad3
# [68..80] panic_loc       12 bytes, null-terminated
# [80..82] panic_line      u16 LE
# [82..120] _reserved
# [120..124] crc32         u32 LE
# [124..128] _pad4

_RECORD_FMT = (
    "<"
    "4s"   # magic
    "B"    # schema_ver
    "B"    # crash_kind
    "B"    # flags
    "B"    # slot_seq
    "3s"   # fw_version
    "x"    # _pad0
    "I"    # git_hash
    "I"    # arm_pc
    "I"    # arm_lr
    "I"    # arm_cfsr
    "I"    # arm_hfsr
    "I"    # arm_fault_addr
    "4x"   # _pad1
    "4s"   # rom_id_prefix
    "H"    # rom_bank
    "B"    # ram_bank
    "x"    # _pad2
    "BBBBBBBB"  # gb_a, gb_f, gb_b, gb_c, gb_d, gb_e, gb_h, gb_l
    "H"    # gb_sp
    "H"    # gb_pc
    "I"    # gb_cycle_lo
    "B"    # ppu_ly
    "B"    # ppu_lcdc
    "B"    # ppu_stat
    "x"    # _pad3
    "12s"  # panic_loc
    "H"    # panic_line
    "38x"  # _reserved + _pad4
    "I"    # crc32
    "4x"   # _pad4
)
assert struct.calcsize(_RECORD_FMT) == RECORD_SIZE, (
    f"Record struct size mismatch: {struct.calcsize(_RECORD_FMT)} != {RECORD_SIZE}"
)

_SECTOR_HEADER_FMT = "<4sIB"  # magic, erase_count, next_slot

# ---------------------------------------------------------------------------
# CRC32 (IEEE 802.3, matches crc32() in crash/mod.rs)
# ---------------------------------------------------------------------------

def _crc32(data: bytes) -> int:
    crc = 0xFFFF_FFFF
    for byte in data:
        crc ^= byte
        for _ in range(8):
            if crc & 1:
                crc = (crc >> 1) ^ 0xEDB8_8320
            else:
                crc >>= 1
    return (~crc) & 0xFFFF_FFFF


# ---------------------------------------------------------------------------
# Data classes
# ---------------------------------------------------------------------------

class CrashRecord:
    def __init__(self, raw: bytes, slot_index: int):
        if len(raw) != RECORD_SIZE:
            raise ValueError(f"Expected {RECORD_SIZE} bytes, got {len(raw)}")

        fields = struct.unpack(_RECORD_FMT, raw)
        (
            self.magic,
            self.schema_ver,
            self.crash_kind,
            self.flags,
            self.slot_seq,
            fw,
            self.git_hash,
            self.arm_pc,
            self.arm_lr,
            self.arm_cfsr,
            self.arm_hfsr,
            self.arm_fault_addr,
            self.rom_id_prefix,
            self.rom_bank,
            self.ram_bank,
            self.gb_a, self.gb_f, self.gb_b, self.gb_c,
            self.gb_d, self.gb_e, self.gb_h, self.gb_l,
            self.gb_sp,
            self.gb_pc,
            self.gb_cycle_lo,
            self.ppu_ly,
            self.ppu_lcdc,
            self.ppu_stat,
            self.panic_loc_raw,
            self.panic_line,
            self.crc32_stored,
        ) = fields

        self.fw_version = tuple(fw)
        self.slot_index = slot_index
        self.crc32_computed = _crc32(raw[:120])
        self.crc_ok = self.crc32_stored == self.crc32_computed
        self.panic_loc = self.panic_loc_raw.rstrip(b"\x00").decode("ascii", errors="replace")

    @property
    def valid_magic(self) -> bool:
        return self.magic == RECORD_MAGIC

    @property
    def crash_kind_name(self) -> str:
        return CRASH_KIND_NAMES.get(self.crash_kind, f"Unknown({self.crash_kind})")

    @property
    def has_arm_regs(self) -> bool:
        return bool(self.flags & FLAG_HAS_ARM_REGS)

    @property
    def has_gb_state(self) -> bool:
        return bool(self.flags & FLAG_HAS_GB_STATE)

    @property
    def has_rom_info(self) -> bool:
        return bool(self.flags & FLAG_HAS_ROM_INFO)

    @property
    def has_panic_loc(self) -> bool:
        return bool(self.flags & FLAG_HAS_PANIC_LOC)

    @property
    def rom_id_hex(self) -> str:
        return self.rom_id_prefix.hex()

    @property
    def fw_version_str(self) -> str:
        return ".".join(str(v) for v in self.fw_version)

    def to_dict(self) -> dict:
        return {
            "slot_index": self.slot_index,
            "slot_seq": self.slot_seq,
            "crc_ok": self.crc_ok,
            "schema_ver": self.schema_ver,
            "crash_kind": self.crash_kind_name,
            "flags": self.flags,
            "fw_version": self.fw_version_str,
            "git_hash": f"{self.git_hash:08x}",
            "arm": {
                "pc": f"0x{self.arm_pc:08x}",
                "lr": f"0x{self.arm_lr:08x}",
                "cfsr": f"0x{self.arm_cfsr:08x}",
                "hfsr": f"0x{self.arm_hfsr:08x}",
                "fault_addr": f"0x{self.arm_fault_addr:08x}",
            } if self.has_arm_regs else None,
            "rom": {
                "id_prefix": self.rom_id_hex,
                "bank": self.rom_bank,
                "ram_bank": self.ram_bank,
            } if self.has_rom_info else None,
            "gb": {
                "pc": f"0x{self.gb_pc:04x}",
                "sp": f"0x{self.gb_sp:04x}",
                "af": f"0x{self.gb_a:02x}{self.gb_f:02x}",
                "bc": f"0x{self.gb_b:02x}{self.gb_c:02x}",
                "de": f"0x{self.gb_d:02x}{self.gb_e:02x}",
                "hl": f"0x{self.gb_h:02x}{self.gb_l:02x}",
                "cycle_lo": self.gb_cycle_lo,
            } if self.has_gb_state else None,
            "ppu": {
                "ly": self.ppu_ly,
                "lcdc": f"0x{self.ppu_lcdc:02x}",
                "stat": f"0x{self.ppu_stat:02x}",
            } if self.has_gb_state else None,
            "panic": {
                "file": self.panic_loc,
                "line": self.panic_line,
            } if self.has_panic_loc else None,
        }


class SectorHeader:
    def __init__(self, raw: bytes):
        magic, self.erase_count, self.next_slot = struct.unpack_from(_SECTOR_HEADER_FMT, raw)
        self.valid = magic == SECTOR_MAGIC


# ---------------------------------------------------------------------------
# Flash sector parsing
# ---------------------------------------------------------------------------

def parse_sector(data: bytes) -> tuple[Optional[SectorHeader], list[CrashRecord]]:
    if len(data) < SECTOR_SIZE:
        raise ValueError(f"Sector data too short: {len(data)} < {SECTOR_SIZE}")
    data = data[:SECTOR_SIZE]

    header = SectorHeader(data[:RECORD_SIZE])
    records: list[CrashRecord] = []

    if not header.valid:
        return None, records

    # Scan all 31 slots by RCRP magic rather than relying on header.next_slot.
    #
    # NOR flash can only clear bits (1→0); updating next_slot requires setting
    # bits (0→1) after the first commit, which is impossible without an erase.
    # The result is next_slot gets AND-corrupted on the second write, making it
    # unreliable as a record count on real hardware.  Scanning by magic is
    # always correct: an empty (erased) slot reads 0xFF×128, which never matches
    # RCRP (0x52 0x43 0x52 0x50).
    for i in range(MAX_RECORDS_PER_SECTOR):
        offset = RECORD_SIZE + i * RECORD_SIZE
        raw = data[offset : offset + RECORD_SIZE]
        if raw[:4] != RECORD_MAGIC:
            continue  # skip empty or corrupt slots; don't stop early
        try:
            records.append(CrashRecord(raw, i))
        except Exception:
            continue

    return header, records


# ---------------------------------------------------------------------------
# Address symbolization
# ---------------------------------------------------------------------------

def _symbolize(addr: int, elf_path: str) -> str:
    """Resolve a hex address to file:line via addr2line."""
    for tool in ("arm-none-eabi-addr2line", "llvm-addr2line", "addr2line"):
        try:
            result = subprocess.run(
                [tool, "-e", elf_path, "-f", "-C", f"0x{addr:08x}"],
                capture_output=True, text=True, timeout=5,
            )
            if result.returncode == 0:
                lines = result.stdout.strip().splitlines()
                func = lines[0] if lines else "?"
                loc = lines[1] if len(lines) > 1 else "?"
                return f"{func}  ({loc})"
        except FileNotFoundError:
            continue
        except Exception:
            continue
    return "(addr2line not available)"


def symbolize_record(record: CrashRecord, elf_path: Optional[str]) -> dict[str, str]:
    if not elf_path:
        return {}
    syms: dict[str, str] = {}
    if record.has_arm_regs and record.arm_pc:
        syms["pc"] = _symbolize(record.arm_pc, elf_path)
    if record.has_arm_regs and record.arm_lr:
        syms["lr"] = _symbolize(record.arm_lr, elf_path)
    return syms


# ---------------------------------------------------------------------------
# Flash acquisition
# ---------------------------------------------------------------------------

def read_from_probe() -> bytes:
    """Read the crash sector directly from the target via probe-rs.

    The crash log lives at the XIP-mapped flash address 0x103FF000 on RP2350.
    Despite probe-rs documenting 'NOTE: Only supports RAM addresses', the
    XIP-mapped region (0x10000000+) is fully accessible via the Cortex-M33 DAP
    AHB-AP while the device is halted.  Validated on RP235x with probe-rs 0.29.

    Writes to a temporary binary file (the -o flag bypasses hex-parsing issues).
    """
    import tempfile, os

    addr = CRASH_LOG_ADDR
    # probe-rs 0.29 flag format: read --chip CHIP -o FILE -f FORMAT WIDTH ADDR WORDS
    with tempfile.NamedTemporaryFile(suffix=".bin", delete=False) as tmp:
        tmp_path = tmp.name
    try:
        result = subprocess.run(
            [
                "probe-rs", "read",
                "--chip", "RP235x",
                "-o", tmp_path,
                "-f", "binary",
                "b8",
                f"0x{addr:08x}",
                str(SECTOR_SIZE),
            ],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if result.returncode != 0:
            raise RuntimeError(
                f"probe-rs read failed (exit {result.returncode}):\n"
                f"  {result.stderr.strip()}\n\n"
                f"Manual alternative:\n"
                f"  probe-rs read --chip RP235x -o crash.bin -f binary b8 "
                f"0x{addr:08x} {SECTOR_SIZE}\n"
                f"  ./tools/crash_decoder.py --raw crash.bin"
            )
        data = Path(tmp_path).read_bytes()
        if len(data) != SECTOR_SIZE:
            raise RuntimeError(
                f"Expected {SECTOR_SIZE} bytes from probe-rs, got {len(data)}"
            )
        return data
    finally:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass


# ---------------------------------------------------------------------------
# Rich output
# ---------------------------------------------------------------------------

def print_report(header: Optional[SectorHeader], records: list[CrashRecord],
                 elf_path: Optional[str]) -> None:
    from rich.console import Console
    from rich.panel import Panel
    from rich.table import Table
    from rich import box
    from rich.text import Text

    console = Console()

    if header is None:
        console.print("[bold red]✗ No valid crash log sector found (bad sector magic).[/]")
        return

    title = (
        f"[bold cyan]RUSTYBOY CRASH DUMP[/]  "
        f"[dim]erase_count={header.erase_count}  next_slot={header.next_slot}[/]"
    )
    console.print(Panel(title, expand=False))

    if not records:
        console.print("[green]✓ No crash records in this sector.[/]")
        return

    console.print(f"[bold]{len(records)} crash record(s) found:[/]\n")

    for rec in records:
        syms = symbolize_record(rec, elf_path)
        crc_badge = "[green]CRC OK[/]" if rec.crc_ok else "[bold red]CRC MISMATCH[/]"
        kind_color = "red" if rec.crash_kind == 0 else "yellow"
        kind_str = f"[bold {kind_color}]{rec.crash_kind_name}[/]"

        header_line = (
            f"[bold]Crash #{rec.slot_index + 1}[/]  {kind_str}  "
            f"[dim]fw={rec.fw_version_str}  git={rec.git_hash:08x}  "
            f"slot={rec.slot_seq}[/]  {crc_badge}"
        )
        console.print(Panel(header_line, expand=False, box=box.SIMPLE_HEAVY))

        # ARM state
        if rec.has_arm_regs:
            pc_sym = syms.get("pc", "")
            lr_sym = syms.get("lr", "")
            console.print(f"  [bold]ARM PC [/]  [cyan]0x{rec.arm_pc:08x}[/]"
                          + (f"  → {pc_sym}" if pc_sym else ""))
            console.print(f"  [bold]ARM LR [/]  [cyan]0x{rec.arm_lr:08x}[/]"
                          + (f"  → {lr_sym}" if lr_sym else ""))
            console.print(f"  [bold]CFSR   [/]  0x{rec.arm_cfsr:08x}  "
                          + _cfsr_description(rec.arm_cfsr))
            if rec.arm_hfsr:
                console.print(f"  [bold]HFSR   [/]  0x{rec.arm_hfsr:08x}  "
                              + _hfsr_description(rec.arm_hfsr))
            if rec.arm_fault_addr:
                console.print(f"  [bold]Fault@ [/]  [red]0x{rec.arm_fault_addr:08x}[/]")

        # Panic location
        if rec.has_panic_loc and rec.panic_loc:
            console.print(f"  [bold]Panic  [/]  [yellow]{rec.panic_loc}:{rec.panic_line}[/]")

        # ROM info
        if rec.has_rom_info:
            console.print(
                f"\n  [bold]ROM    [/]  id=[magenta]{rec.rom_id_hex}[/]  "
                f"bank={rec.rom_bank}  ram_bank={rec.ram_bank}"
            )

        # GB CPU
        if rec.has_gb_state:
            console.print(
                f"  [bold]GB CPU [/]  "
                f"PC=[cyan]0x{rec.gb_pc:04x}[/]  "
                f"SP=0x{rec.gb_sp:04x}  "
                f"AF=0x{rec.gb_a:02x}{rec.gb_f:02x}  "
                f"BC=0x{rec.gb_b:02x}{rec.gb_c:02x}  "
                f"DE=0x{rec.gb_d:02x}{rec.gb_e:02x}  "
                f"HL=0x{rec.gb_h:02x}{rec.gb_l:02x}"
            )
            console.print(
                f"  [bold]Cycles [/]  {rec.gb_cycle_lo:,}"
            )
            console.print(
                f"  [bold]PPU    [/]  "
                f"LY={rec.ppu_ly}  "
                f"LCDC=0x{rec.ppu_lcdc:02x}  "
                f"STAT=0x{rec.ppu_stat:02x}"
            )

        console.print()


# ---------------------------------------------------------------------------
# CFSR / HFSR human-readable descriptions
# ---------------------------------------------------------------------------

def _cfsr_description(cfsr: int) -> str:
    parts = []
    # UsageFault (bits 16-25)
    if cfsr & (1 << 25): parts.append("DIVBYZERO")
    if cfsr & (1 << 24): parts.append("UNALIGNED")
    if cfsr & (1 << 19): parts.append("NOCP")
    if cfsr & (1 << 18): parts.append("INVPC")
    if cfsr & (1 << 17): parts.append("INVSTATE")
    if cfsr & (1 << 16): parts.append("UNDEFINSTR")
    # BusFault (bits 8-15)
    if cfsr & (1 << 15): parts.append("BFARVALID")
    if cfsr & (1 << 12): parts.append("STKERR")
    if cfsr & (1 << 11): parts.append("UNSTKERR")
    if cfsr & (1 << 10): parts.append("IMPRECISERR")
    if cfsr & (1 <<  9): parts.append("PRECISERR")
    if cfsr & (1 <<  8): parts.append("IBUSERR")
    # MemManage (bits 0-7)
    if cfsr & (1 <<  7): parts.append("MMARVALID")
    if cfsr & (1 <<  4): parts.append("MSTKERR")
    if cfsr & (1 <<  3): parts.append("MUNSTKERR")
    if cfsr & (1 <<  1): parts.append("DACCVIOL")
    if cfsr & (1 <<  0): parts.append("IACCVIOL")
    return "  ".join(parts) if parts else ""


def _hfsr_description(hfsr: int) -> str:
    parts = []
    if hfsr & (1 << 31): parts.append("DEBUGEVT")
    if hfsr & (1 << 30): parts.append("FORCED")
    if hfsr & (1 <<  1): parts.append("VECTTBL")
    return "  ".join(parts) if parts else ""


# ---------------------------------------------------------------------------
# JSON output
# ---------------------------------------------------------------------------

def print_json(header: Optional[SectorHeader], records: list[CrashRecord],
               elf_path: Optional[str]) -> None:
    out = {
        "sector": {
            "valid": header is not None,
            "erase_count": header.erase_count if header else None,
            "next_slot": header.next_slot if header else None,
        },
        "crashes": [rec.to_dict() for rec in records],
    }
    # Append symbolization if elf provided.
    if elf_path:
        for entry, rec in zip(out["crashes"], records):
            syms = symbolize_record(rec, elf_path)
            if syms:
                entry["symbols"] = syms
    print(json.dumps(out, indent=2))


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main() -> None:
    ap = argparse.ArgumentParser(
        description="Decode rustyboy crash records from an RP2350 flash sector image."
    )
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--raw", metavar="FILE",
                     help="Path to a 4 KiB binary sector image (from picotool save)")
    src.add_argument("--probe", action="store_true",
                     help="Read sector directly from the target via probe-rs")
    ap.add_argument("--elf", metavar="FILE",
                    help="Firmware ELF for address symbolisation (arm-none-eabi-addr2line)")
    ap.add_argument("--json", dest="json_out", action="store_true",
                    help="Output machine-readable JSON instead of rich text")

    args = ap.parse_args()

    # Acquire raw sector bytes.
    if args.probe:
        data = read_from_probe()
    else:
        raw_path = Path(args.raw)
        if not raw_path.exists():
            sys.exit(f"File not found: {raw_path}")
        data = raw_path.read_bytes()

    if len(data) < SECTOR_SIZE:
        # Pad to sector size if the file is shorter (e.g. truncated picotool output).
        data = data + b"\xff" * (SECTOR_SIZE - len(data))

    header, records = parse_sector(data)

    if args.json_out:
        print_json(header, records, args.elf)
    else:
        print_report(header, records, args.elf)


if __name__ == "__main__":
    main()
