#!/usr/bin/env bash
# Wrapper around probe-rs run that clears all DWT comparators before flashing.
#
# DWT comparator registers are NOT cleared by a warm system reset (SYSRESETREQ).
# An armed write-watchpoint from the previous boot persists into the next Reset
# handler, which copies .data from flash to SRAM and writes the watched address,
# triggering an immediate "Watchpoint" exit before main() is reached.
#
# Writing 0 to DWT_FUNCTIONn (0xE000_1028/38/48/58) disables comparators 0–3.
# probe-rs halts the chip, writes the registers, then probe-rs run reflashes.
#
# Usage (set as runner in .cargo/config.toml):
#   runner = "path/to/rb-run.sh"
set -euo pipefail

ELF="${1:?Usage: rb-run.sh <elf>}"

# Best-effort: disarm all 4 comparators. If probe-rs write isn't available or
# fails (e.g. chip already erased / first-ever flash), continue anyway.
probe-rs write --chip RP235x b32 0xE0001028 0 2>/dev/null || true
probe-rs write --chip RP235x b32 0xE0001038 0 2>/dev/null || true
probe-rs write --chip RP235x b32 0xE0001048 0 2>/dev/null || true
probe-rs write --chip RP235x b32 0xE0001058 0 2>/dev/null || true

exec probe-rs run --chip RP235x "$ELF"
