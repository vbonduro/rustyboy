#!/usr/bin/env -S uv run --with numpy
# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy"]
# ///
"""
encode_landscape.py — Re-encode landscape_bg.bin into the compact RBG1 format.

Old format
----------
  offset 0       : 256 bytes  — palette, 64 RGBA entries (palette_len=64)
  offset 256     : 120 × 23040 bytes — per-frame palette indices (1 byte / pixel,
                   160×144 pixels per frame, row-major left-to-right top-to-bottom)
  total          : 2 765 056 bytes

New format (RBG1, little-endian throughout)
-------------------------------------------
  Header (14 bytes):
    bytes  0– 3  : magic "RBG1"
    bytes  4– 5  : width        u16  (160)
    bytes  6– 7  : height       u16  (144)
    bytes  8– 9  : frame_count  u16  (120)
    byte  10     : palette_len  u8   (64)
    byte  11     : bits_per_index u8  (3)
    bytes 12–13  : static_rows  u16  (number of top rows identical across all frames)

  Palette (palette_len × 4 bytes):
    bytes 14 .. 14+palette_len*4 — RGBA bytes, indexed 0..palette_len-1

  Static region (one copy of the frozen top rows, 3-bit-packed):
    ceil(static_rows * width * 3 / 8) bytes
    Bit layout: indices are concatenated LSB-first into a flat bit stream.
    Pixel p's index occupies bits [p*3 .. p*3+2] in the stream, where
    bit i is stored as (stream[i//8] >> (i%8)) & 1.

  Animated region (frame_count frames, each 3-bit-packed):
    frame_count × ceil(anim_rows * width * 3 / 8) bytes
    Each frame stores only the bottom anim_rows = height - static_rows rows,
    using the same LSB-first 3-bit packing.

Usage:
  uv run --with numpy tools/encode_landscape.py
  (run from the platform/web/client/ directory, or edit the paths below)
"""
import struct
import sys
from pathlib import Path

import numpy as np

# ---------------------------------------------------------------------------
# Paths — adjust if running from a different cwd
# ---------------------------------------------------------------------------
REPO_BASE = Path(__file__).resolve().parent.parent  # platform/web/client/
OLD_BIN   = REPO_BASE / "src" / "landscape_bg.bin"
NEW_BIN   = REPO_BASE / "src" / "landscape_bg.bin"  # overwrite in-place

WIDTH        = 160
HEIGHT       = 144
FRAME_COUNT  = 120
PALETTE_COLORS = 64
PALETTE_BYTES  = PALETTE_COLORS * 4
FRAME_SIZE     = WIDTH * HEIGHT  # pixels (old: 1 byte each)
BITS_PER_INDEX = 3

def pack_indices_3bit(indices: np.ndarray) -> bytes:
    """Pack a flat array of 3-bit indices (values 0..7) into an LSB-first bit stream."""
    n = len(indices)
    out_bytes = (n * 3 + 7) // 8
    buf = bytearray(out_bytes)
    for i, val in enumerate(indices):
        bit_pos = i * 3
        byte_idx = bit_pos >> 3
        bit_off  = bit_pos & 7
        # Write 3 bits across at most 2 bytes
        val = int(val) & 0x7
        buf[byte_idx] |= (val << bit_off) & 0xFF
        carry = val >> (8 - bit_off)
        if carry and byte_idx + 1 < out_bytes:
            buf[byte_idx + 1] |= carry
    return bytes(buf)

def unpack_indices_3bit(data: bytes, n_pixels: int) -> np.ndarray:
    """Unpack n_pixels 3-bit indices from an LSB-first bit stream."""
    out = np.empty(n_pixels, dtype=np.uint8)
    for i in range(n_pixels):
        bit_pos  = i * 3
        byte_idx = bit_pos >> 3
        bit_off  = bit_pos & 7
        lo = (data[byte_idx] >> bit_off) & 0xFF
        if bit_off <= 5:
            val = lo & 0x7
        else:
            # spans two bytes
            hi = data[byte_idx + 1] if byte_idx + 1 < len(data) else 0
            val = (lo | (hi << (8 - bit_off))) & 0x7
        out[i] = val
    return out

def main():
    print(f"Reading {OLD_BIN} ...")
    raw = OLD_BIN.read_bytes()
    old_size = len(raw)
    print(f"  Old size: {old_size:,} bytes")

    # -----------------------------------------------------------------------
    # Parse old format
    # -----------------------------------------------------------------------
    expected_size = PALETTE_BYTES + FRAME_COUNT * FRAME_SIZE
    assert len(raw) == expected_size, (
        f"Unexpected size: got {len(raw)}, expected {expected_size}"
    )

    palette = raw[:PALETTE_BYTES]

    # frames[f, row, col] = palette index (uint8)
    frames_flat = np.frombuffer(raw[PALETTE_BYTES:], dtype=np.uint8)
    frames = frames_flat.reshape(FRAME_COUNT, HEIGHT, WIDTH)

    # -----------------------------------------------------------------------
    # Assert ≤ 8 distinct indices
    # -----------------------------------------------------------------------
    distinct = np.unique(frames_flat)
    print(f"  Distinct palette indices used: {distinct.tolist()}")
    assert len(distinct) <= 8, f"More than 8 distinct indices: {len(distinct)}"
    assert all(v < 8 for v in distinct), f"Index ≥ 8 found: {distinct}"
    print(f"  OK — fits in {BITS_PER_INDEX} bits")

    # -----------------------------------------------------------------------
    # Find static_rows: largest prefix of rows identical across all frames
    # -----------------------------------------------------------------------
    static_rows = 0
    for row in range(HEIGHT):
        row_data = frames[:, row, :]   # shape (FRAME_COUNT, WIDTH)
        if np.all(row_data == row_data[0]):
            static_rows += 1
        else:
            break

    print(f"  Static (frozen) top rows: {static_rows}")
    assert static_rows > 0, "No static rows found!"

    anim_rows = HEIGHT - static_rows

    # -----------------------------------------------------------------------
    # Encode new format
    # -----------------------------------------------------------------------
    # Header
    magic = b"RBG1"
    header = magic
    header += struct.pack("<HHH", WIDTH, HEIGHT, FRAME_COUNT)
    header += struct.pack("<BB", PALETTE_COLORS, BITS_PER_INDEX)
    header += struct.pack("<H", static_rows)
    assert len(header) == 14

    # Palette (unchanged)
    palette_section = palette  # 256 bytes

    # Static region: top static_rows of frame 0 (all frames identical here)
    static_indices = frames[0, :static_rows, :].flatten()
    static_packed = pack_indices_3bit(static_indices)
    static_pixel_count = static_rows * WIDTH
    static_bytes_needed = (static_pixel_count * 3 + 7) // 8
    assert len(static_packed) == static_bytes_needed

    # Animated region: bottom anim_rows for each frame
    anim_pixel_count = anim_rows * WIDTH
    anim_bytes_per_frame = (anim_pixel_count * 3 + 7) // 8
    anim_sections = []
    for f in range(FRAME_COUNT):
        anim_indices = frames[f, static_rows:, :].flatten()
        packed = pack_indices_3bit(anim_indices)
        assert len(packed) == anim_bytes_per_frame
        anim_sections.append(packed)

    new_data = header + palette_section + static_packed + b"".join(anim_sections)
    new_size = len(new_data)

    # -----------------------------------------------------------------------
    # Round-trip verification
    # -----------------------------------------------------------------------
    print("  Verifying round-trip ...")
    # Re-parse new_data
    assert new_data[:4] == b"RBG1"
    w, h, fc = struct.unpack_from("<HHH", new_data, 4)
    pl, bpi, sr = struct.unpack_from("<BBH", new_data, 10)
    assert (w, h, fc, pl, bpi, sr) == (WIDTH, HEIGHT, FRAME_COUNT, PALETTE_COLORS, BITS_PER_INDEX, static_rows)

    ar = h - sr
    off = 14 + pl * 4  # after header + palette

    static_byte_count = (sr * w * 3 + 7) // 8
    static_raw = new_data[off:off + static_byte_count]
    static_decoded = unpack_indices_3bit(static_raw, sr * w).reshape(sr, w)
    off += static_byte_count

    abpf = (ar * w * 3 + 7) // 8
    for f in range(FRAME_COUNT):
        anim_raw = new_data[off:off + abpf]
        anim_decoded = unpack_indices_3bit(anim_raw, ar * w).reshape(ar, w)
        off += abpf

        # Reconstruct full frame
        decoded_frame = np.vstack([static_decoded, anim_decoded])
        expected_frame = frames[f]
        if not np.array_equal(decoded_frame, expected_frame):
            print(f"  MISMATCH at frame {f}!")
            sys.exit(1)

    assert off == len(new_data), f"Consumed {off} bytes but data is {len(new_data)}"
    print("  Round-trip: PASSED (all 120 frames bit-identical)")

    # -----------------------------------------------------------------------
    # Write new bin
    # -----------------------------------------------------------------------
    NEW_BIN.write_bytes(new_data)
    print(f"\nWrote {NEW_BIN}")
    print(f"  Old size : {old_size:>10,} bytes  ({old_size/1024/1024:.2f} MB)")
    print(f"  New size : {new_size:>10,} bytes  ({new_size/1024/1024:.2f} MB)")
    print(f"  Reduction: {100*(1 - new_size/old_size):.1f}%")
    print(f"  static_rows={static_rows}, anim_rows={anim_rows}")
    print(f"  static packed: {len(static_packed):,} bytes")
    print(f"  anim per frame: {anim_bytes_per_frame:,} bytes × {FRAME_COUNT} = {anim_bytes_per_frame*FRAME_COUNT:,} bytes")

if __name__ == "__main__":
    main()
