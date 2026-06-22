==============================================================================
 OpenOCD hardware write-watchpoint runbook — catch the #5 wild writer
==============================================================================
Goal: halt the CPU at the exact store instruction that corrupts memory, with no
firmware change (so no stack-layout perturbation — the thing that confounds every
software tripwire in this bug). When it halts, `reg` PC = the corrupting store.

probe-rs's GDB stub CANNOT set DWT watchpoints on RP235x (reports 0 comparators).
Use the RaspberryPi OpenOCD fork, which correctly reports 4 watchpoints per core.

------------------------------------------------------------------------------
 CHOOSE THE WATCH ADDRESS  (read this first — wrong target = useless run)
------------------------------------------------------------------------------
Watch a COLD word (written ~once, at construction) that the corruptor smashes.
DO NOT watch a stack slot — it is written by every push/pop and will halt
immediately on legitimate traffic.

  *** DO NOT WATCH 0x2007EACC ***  (the unmasked-build victim; it is a hot
      stack LR slot — a write-watchpoint there fires on every leaf call).

Recommended cold targets (pick one; verify value against the boot-log checkpoints
each run, since addresses shift slightly per build):

  PRIMARY  — cartridge vtable pointer
    addr  = GameBoyMemory_base + 0x4124
          = 0x20026184 + 0x4124  = 0x2002A2A8     <-- VERIFY base in boot log
    healthy value = 0x1003263c   (boot log "vtable=...")
    why: dominant crash mechanism — bus_write loads this fat-pointer vtable word
         and blx's through it; a smashed value -> IBUSERR. Cold during gameplay.

  SECONDARY — audio_tx (heapless spsc) length metadata word
    addr  = 0x20081F44
    healthy value = 0x00000801   (2049; boot log "watch@0x20081f44=...")
    why: smashed to 0 in the spsc.rs:185 crashes. Exact address is stable/known.

If a run halts immediately at construction time, that is the ONE legitimate write
-- resume once and let the poll loop continue; the next halt is the corruptor.

------------------------------------------------------------------------------
 PRE-FLIGHT
------------------------------------------------------------------------------
- Flash the UNMASKED build first (shim reverted; plain self.gb.tick()):
      cd platform/pico2w && cargo run --release   # let it boot, then detach:
      pkill -f probe-rs
- OpenOCD fork: built last session at /tmp/ocd-build/openocd (v0.12.0+dev, has
  target/rp2350.cfg). /tmp may be wiped on reboot -- if missing, rebuild from the
  raspberrypi/openocd fork. Confirm `./src/openocd --version` shows 0.12.0+dev.
- probe-rs and OpenOCD cannot share the probe. Always kill probe-rs first.

------------------------------------------------------------------------------
 LAUNCH  (single-core cm0 attach — the writer is on core 0)
------------------------------------------------------------------------------
pkill -9 -x probe-rs; pkill -9 -x gdb-multiarch
cd /tmp/ocd-build/openocd
./src/openocd -s tcl -f interface/cmsis-dap.cfg -c "adapter speed 5000" \
    -c "set USE_CORE cm0" -f target/rp2350.cfg -f ./watch.tcl

  - USE_CORE (not _USE_CORE; rp2350.cfg overwrites the internal one). cm0 only:
    arming the wp on a single core avoids the SMP "other core running" abort that
    silently leaves the wp un-armed (you then catch nothing).
  - Confirm OpenOCD prints that the watchpoint was SET. If it errors on insert,
    nothing is armed -- fix before walking away.

------------------------------------------------------------------------------
 watch.tcl   (save next to the openocd binary, or pass an absolute path above)
------------------------------------------------------------------------------
init
halt

# Disable the firmware's 10 s watchdog so a long debug-halt cannot reset the chip
# out from under us. WATCHDOG.CTRL.ENABLE @ 0x400d8000.
mww 0x400d8000 0

# ---- arm the write-watchpoint -------------------------------------------------
# 4-byte WRITE watch. Change the address per the "CHOOSE THE WATCH ADDRESS" notes.
set WATCH 0x2002A2A8        ;# cartridge vtable ptr (PRIMARY). Or 0x20081F44.
wp $WATCH 4 w

resume

# Open-ended wait. wait_halt has a timeout cap, so poll manually in a TCL loop;
# the event loop does NOT auto-poll inside a blocking loop, so call poll yourself.
while { 1 } {
    sleep 300
    catch { poll }
    if { [string compare [rp2350.cm0 curstate] "halted"] == 0 } { break }
}

# ---- HALTED AT THE CORRUPTING STORE ------------------------------------------
echo "=== WATCHPOINT HIT ==="
reg                         ;# PC = the store instruction; r0..r12 = base/operands
mdw $WATCH 1                ;# the value just written (the smashed word)
mdw 0x2007EAB0 24           ;# stack context around the fault
shutdown

------------------------------------------------------------------------------
 AFTER IT HALTS
------------------------------------------------------------------------------
- Resolve the store PC to source:
      arm-none-eabi-addr2line -e \
        target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w 0x<pc>
  (or plain addr2line). Do NOT feed `reg pc force` strings to expr/mdh -- they are
  formatted, not integers; just read the reg dump and addr2line the PC afterward.
- Read which base register held the wild destination (the value == $WATCH at the
  store) to name the overrunning code path.

------------------------------------------------------------------------------
 IF IT DOESN'T HIT
------------------------------------------------------------------------------
- The bug needs the poisoned save state to drive the GB CPU into the trigger
  (cycle_lo ~2.38B). Standalone reboot reaches it within ~10 s; let it run minutes.
- If the unmasked build's victim is landing on the hot stack slot instead of the
  cold word you're watching, the cold word may not get hit. The writer is the same
  regardless -- to relocate the victim onto a watchable cold word, flash a
  layout-perturbed build (e.g. the mask-wrapper shim) which pushes the victim onto
  0x20081F44, and watch that. Same store, just an observable victim.
- Positive control: before trusting a no-hit result, confirm the comparator works
  -- watch a deliberately hot address briefly (e.g. an active stack word) and
  verify OpenOCD halts. A comparator that never halts on ANYTHING is mis-armed.

------------------------------------------------------------------------------
 GOTCHAS (each cost a run last time)
------------------------------------------------------------------------------
- SMP replicates the wp to both cores and aborts if cm1 isn't halted -> wp fails
  to arm -> firmware runs free -> next crash sys_resets -> "external reset
  detected" and you catch nothing. Fix: USE_CORE cm0 (single-core attach).
- Re-disable the watchdog (mww 0x400d8000 0) AFTER halting; the firmware re-arms it
  every boot.
- Use the poll+sleep loop, not wait_halt (timeout cap), for an open-ended wait.
==============================================================================
