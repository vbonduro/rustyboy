#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "pytest>=8",
#   "rich>=13",
# ]
# ///
"""
Tests for crash_decoder.py.

Run with:
    uv run pytest tools/test_crash_decoder.py -v
or:
    ./tools/test_crash_decoder.py  (runs pytest via uv shebang)

The `write_test_fixture` Rust test must have been run at least once to produce
tools/fixtures/test_crash.bin before the fixture round-trip test can pass.
"""

from __future__ import annotations

import io
import json
import struct
import sys
from pathlib import Path
from typing import Optional
from unittest.mock import patch

import pytest

# ---------------------------------------------------------------------------
# Import the decoder module from the same directory.
# ---------------------------------------------------------------------------

_TOOLS_DIR = Path(__file__).parent
sys.path.insert(0, str(_TOOLS_DIR))
import crash_decoder as cd

# ---------------------------------------------------------------------------
# Test fixture helpers — construct binary sector images in Python.
#
# These helpers mirror the Rust serialization exactly, making the Python tests
# fully self-contained (no Rust build required).
# ---------------------------------------------------------------------------

# CRC32 is already tested via the decoder module itself.
_RECORD_MAGIC = b"RCRP"
_SECTOR_MAGIC = b"RCLG"
RECORD_SIZE = 128
SECTOR_SIZE = 4096

FLAG_HAS_ARM_REGS = 0x01
FLAG_HAS_GB_STATE = 0x02
FLAG_HAS_ROM_INFO = 0x04
FLAG_HAS_PANIC_LOC = 0x08


def _pack_record(
    *,
    crash_kind: int = 0,
    flags: int = FLAG_HAS_ARM_REGS | FLAG_HAS_GB_STATE | FLAG_HAS_ROM_INFO,
    slot_seq: int = 0,
    fw_version: tuple[int, int, int] = (0, 1, 0),
    git_hash: int = 0xDEAD_C0DE,
    arm_pc: int = 0x1002_34A8,
    arm_lr: int = 0x1002_2BC4,
    arm_cfsr: int = 0x0000_0200,
    arm_hfsr: int = 0x4000_0000,
    arm_fault_addr: int = 0xDEAD_BEEF,
    rom_id_prefix: bytes = b"\xab\xcd\xef\x01",
    rom_bank: int = 7,
    ram_bank: int = 0,
    gb_a: int = 0x01, gb_f: int = 0xB0,
    gb_b: int = 0x00, gb_c: int = 0x13,
    gb_d: int = 0x00, gb_e: int = 0xD8,
    gb_h: int = 0x01, gb_l: int = 0x4D,
    gb_sp: int = 0xCFE0,
    gb_pc: int = 0x4A31,
    gb_cycle_lo: int = 12_345_678,
    ppu_ly: int = 88,
    ppu_lcdc: int = 0x91,
    ppu_stat: int = 0x83,
    panic_loc: bytes = b"",
    panic_line: int = 0,
) -> bytes:
    """Pack a 128-byte crash record matching the Rust layout."""
    loc_padded = (panic_loc + b"\x00" * 12)[:12]
    # Build payload (bytes 0..120, no CRC yet).
    payload = struct.pack(
        "<"
        "4s"    # magic
        "B"     # schema_ver
        "B"     # crash_kind
        "B"     # flags
        "B"     # slot_seq
        "3s"    # fw_version
        "x"     # _pad0
        "I"     # git_hash
        "I"     # arm_pc
        "I"     # arm_lr
        "I"     # arm_cfsr
        "I"     # arm_hfsr
        "I"     # arm_fault_addr
        "4x"    # _pad1
        "4s"    # rom_id_prefix
        "H"     # rom_bank
        "B"     # ram_bank
        "x"     # _pad2
        "BBBBBBBB"  # gb registers
        "H"     # gb_sp
        "H"     # gb_pc
        "I"     # gb_cycle_lo
        "B"     # ppu_ly
        "B"     # ppu_lcdc
        "B"     # ppu_stat
        "x"     # _pad3
        "12s"   # panic_loc
        "H"     # panic_line
        "38x",  # _reserved + _pad4 placeholder (no CRC yet)
        _RECORD_MAGIC,
        1,  # schema_ver
        crash_kind,
        flags,
        slot_seq,
        bytes(fw_version),
        git_hash,
        arm_pc,
        arm_lr,
        arm_cfsr,
        arm_hfsr,
        arm_fault_addr,
        rom_id_prefix,
        rom_bank,
        ram_bank,
        gb_a, gb_f, gb_b, gb_c, gb_d, gb_e, gb_h, gb_l,
        gb_sp,
        gb_pc,
        gb_cycle_lo,
        ppu_ly,
        ppu_lcdc,
        ppu_stat,
        loc_padded,
        panic_line,
    )
    assert len(payload) == 120, f"payload wrong size: {len(payload)}"
    crc = cd._crc32(payload)
    # Append crc32 (4 bytes) + _pad4 (4 bytes).
    return payload + struct.pack("<I4x", crc)


def _pack_sector_header(*, erase_count: int = 1, next_slot: int = 0) -> bytes:
    """Pack a 128-byte sector header."""
    buf = bytearray(RECORD_SIZE)
    struct.pack_into("<4sIB", buf, 0, _SECTOR_MAGIC, erase_count, next_slot)
    return bytes(buf)


def _make_sector(*records: bytes, erase_count: int = 1) -> bytes:
    """Build a 4 KiB sector image with the given records."""
    header = _pack_sector_header(erase_count=erase_count, next_slot=len(records))
    sector = bytearray(SECTOR_SIZE)
    sector[0:RECORD_SIZE] = header
    for i, rec in enumerate(records):
        off = RECORD_SIZE + i * RECORD_SIZE
        sector[off : off + RECORD_SIZE] = rec
    return bytes(sector)


# ---------------------------------------------------------------------------
# CRC32 sanity
# ---------------------------------------------------------------------------

def test_crc32_known_vector():
    """CRC32 of b'123456789' must equal 0xCBF43926 (canonical test vector)."""
    assert cd._crc32(b"123456789") == 0xCBF4_3926


# ---------------------------------------------------------------------------
# CrashRecord parsing
# ---------------------------------------------------------------------------

def test_decode_single_hardfault():
    rec_bytes = _pack_record(
        arm_pc=0xDEAD_C0DE,
        rom_id_prefix=b"\xab\xcd\xef\x01",
    )
    sector = _make_sector(rec_bytes)
    header, records = cd.parse_sector(sector)

    assert header is not None
    assert header.valid
    assert len(records) == 1

    r = records[0]
    assert r.crc_ok, "CRC32 mismatch"
    assert r.crash_kind_name == "HardFault"
    assert r.arm_pc == 0xDEAD_C0DE
    assert r.rom_id_hex == "abcdef01"
    assert r.gb_pc == 0x4A31
    assert r.has_arm_regs
    assert r.has_gb_state
    assert r.has_rom_info


def test_decode_multiple_records():
    r0 = _pack_record(slot_seq=0, arm_pc=0x1000_0001)
    r1 = _pack_record(slot_seq=1, arm_pc=0x1000_0002)
    r2 = _pack_record(slot_seq=2, arm_pc=0x1000_0003)
    sector = _make_sector(r0, r1, r2)
    _, records = cd.parse_sector(sector)
    assert len(records) == 3
    assert records[0].arm_pc == 0x1000_0001
    assert records[1].arm_pc == 0x1000_0002
    assert records[2].arm_pc == 0x1000_0003


def test_decode_panic_record():
    rec_bytes = _pack_record(
        crash_kind=1,
        flags=FLAG_HAS_PANIC_LOC | FLAG_HAS_GB_STATE | FLAG_HAS_ROM_INFO,
        arm_pc=0, arm_lr=0, arm_cfsr=0, arm_hfsr=0, arm_fault_addr=0,
        panic_loc=b"storage.rs",
        panic_line=47,
    )
    sector = _make_sector(rec_bytes)
    _, records = cd.parse_sector(sector)
    assert len(records) == 1
    r = records[0]
    assert r.crash_kind_name == "Panic"
    assert r.panic_loc == "storage.rs"
    assert r.panic_line == 47
    assert r.has_panic_loc
    assert not r.has_arm_regs


def test_no_crashes_empty_sector():
    """A sector with header but zero records should return an empty list."""
    sector = _make_sector()  # next_slot = 0
    header, records = cd.parse_sector(sector)
    assert header is not None
    assert records == []


def test_bad_sector_magic():
    """Sectors with corrupt magic return None header."""
    sector = b"\x00" * SECTOR_SIZE
    header, records = cd.parse_sector(sector)
    assert header is None
    assert records == []


def test_corrupt_crc_flagged():
    """A record with a bit-flipped byte should report crc_ok == False."""
    rec_bytes = bytearray(_pack_record(slot_seq=0))
    rec_bytes[42] ^= 0xFF  # corrupt data byte
    sector = _make_sector(bytes(rec_bytes))
    _, records = cd.parse_sector(sector)
    assert len(records) == 1
    assert not records[0].crc_ok


# ---------------------------------------------------------------------------
# JSON output
# ---------------------------------------------------------------------------

def test_json_output_schema():
    rec_bytes = _pack_record()
    sector = _make_sector(rec_bytes)
    header, records = cd.parse_sector(sector)

    buf = io.StringIO()
    with patch("sys.stdout", buf):
        cd.print_json(header, records, elf_path=None)

    data = json.loads(buf.getvalue())
    assert "sector" in data
    assert "crashes" in data
    assert len(data["crashes"]) == 1

    crash = data["crashes"][0]
    assert crash["crash_kind"] == "HardFault"
    assert crash["arm"] is not None
    assert crash["arm"]["pc"] == f"0x{0x1002_34A8:08x}"
    assert crash["rom"] is not None
    assert crash["rom"]["id_prefix"] == "abcdef01"
    assert crash["gb"] is not None
    assert crash["gb"]["pc"] == "0x4a31"
    assert crash["panic"] is None  # not a panic record


def test_json_panic_record():
    rec_bytes = _pack_record(
        crash_kind=1,
        flags=FLAG_HAS_PANIC_LOC,
        arm_pc=0, arm_lr=0, arm_cfsr=0, arm_hfsr=0, arm_fault_addr=0,
        panic_loc=b"main.rs",
        panic_line=99,
    )
    sector = _make_sector(rec_bytes)
    _, records = cd.parse_sector(sector)
    d = records[0].to_dict()
    assert d["crash_kind"] == "Panic"
    assert d["panic"] == {"file": "main.rs", "line": 99}
    assert d["arm"] is None
    assert d["gb"] is None


# ---------------------------------------------------------------------------
# CFSR / HFSR description helpers
# ---------------------------------------------------------------------------

def test_cfsr_preciserr():
    desc = cd._cfsr_description(0x0000_0200)
    assert "PRECISERR" in desc


def test_cfsr_unaligned():
    desc = cd._cfsr_description(1 << 24)
    assert "UNALIGNED" in desc


def test_hfsr_forced():
    desc = cd._hfsr_description(1 << 30)
    assert "FORCED" in desc


# ---------------------------------------------------------------------------
# Fixture round-trip (cross-language integration test)
#
# This test reads tools/fixtures/test_crash.bin which is generated by the Rust
# `write_test_fixture` test in crash/mod.rs.  Run that test once first:
#
#   cargo test-host -- write_test_fixture
# ---------------------------------------------------------------------------

FIXTURE_PATH = Path(__file__).parent / "fixtures" / "test_crash.bin"


@pytest.mark.skipif(not FIXTURE_PATH.exists(), reason="Rust fixture not yet generated; run: cargo test-host -- write_test_fixture")
def test_fixture_roundtrip():
    """Decode the Rust-generated fixture and verify known field values."""
    data = FIXTURE_PATH.read_bytes()
    header, records = cd.parse_sector(data)

    assert header is not None, "sector header missing in fixture"
    assert header.erase_count == 1
    assert header.next_slot == 2
    assert len(records) == 2, f"expected 2 records, got {len(records)}"

    # Record 0: HardFault with well-known values (from sample_record(0) in Rust)
    r0 = records[0]
    assert r0.crc_ok, "fixture record 0 CRC mismatch — struct layout out of sync"
    assert r0.crash_kind_name == "HardFault"
    assert r0.arm_pc == 0x1002_34A8, f"arm_pc wrong: {r0.arm_pc:#x}"
    assert r0.arm_lr == 0x1002_2BC4
    assert r0.arm_cfsr == 0x0000_0200
    assert r0.arm_fault_addr == 0xDEAD_BEEF
    assert r0.rom_id_hex == "abcdef01"
    assert r0.rom_bank == 7
    assert r0.gb_pc == 0x4A31
    assert r0.gb_a == 0x01
    assert r0.gb_f == 0xB0
    assert r0.gb_sp == 0xCFE0
    assert r0.ppu_ly == 88
    assert r0.gb_cycle_lo == 12_345_678

    # Record 1: Panic
    r1 = records[1]
    assert r1.crc_ok, "fixture record 1 CRC mismatch"
    assert r1.crash_kind_name == "Panic"
    assert r1.has_panic_loc
    assert r1.panic_line == 47
    assert not r1.has_arm_regs  # panics have no exception frame


# ---------------------------------------------------------------------------
# Entry point — allow running as a script via uv shebang.
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    sys.exit(pytest.main([__file__, "-v", *sys.argv[1:]]))
