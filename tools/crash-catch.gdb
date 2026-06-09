# crash-catch.gdb — catch the recurring Core 1 audio-enqueue HardFault.
#
# Background: the crash log shows a HardFault whose PC symbolizes to
# heapless::mpmc::enqueue (the AUDIO_QUEUE) inside run_core1_worker's
# DrainAudio handler. The faulting store targets ~0x01338e80, which is NOT
# the real AUDIO_QUEUE (0x20002f74) — so the `self`/audio_tx pointer (or
# Core 1's stack frame holding it) was corrupted. Prime suspect: Core 1's
# 8 KiB stack (0x20080000..0x20082000) overflowing — note a PpuSnapshot is
# 8480 bytes, larger than the whole stack.
#
# Usage (two terminals):
#   T1:  probe-rs gdb --chip RP235x
#   T2:  gdb-multiarch -x tools/crash-catch.gdb \
#          /tmp/rb-22f3352/target/thumbv8m.main-none-eabihf/release/rustyboy-pico2w
# Then load+run the repro ROM (id=21f712e2) and play until it faults.

set pagination off
set print pretty on
target remote :1337

# Break the instant the firmware enters its HardFault handler, BEFORE it
# records the snapshot and calls sys_reset().
break rustyboy_pico2w::crash::handler::__cortex_m_rt_HardFault

# Convenience: dump everything we need to distinguish the two hypotheses.
define faultdump
  echo \n==== FAULT STATE ====\n
  echo -- core registers (R0 is usually `self`/audio_tx for the enqueue) --\n
  info registers r0 r1 r2 r3 sp lr pc
  echo \n-- is SP inside Core 1's 8 KiB stack [0x20080000,0x20082000)? --\n
  printf "SP = 0x%08x  (overflow if < 0x20080000)\n", $sp
  echo \n-- CFSR / HFSR / BFAR (fault status) --\n
  x/1xw 0xE000ED28
  x/1xw 0xE000ED2C
  x/1xw 0xE000ED38
  echo \n-- AUDIO_QUEUE (expected base 0x20002f74) enqueue/dequeue positions --\n
  print rustyboy_pico2w::multicore::AUDIO_QUEUE
  echo \n-- backtrace --\n
  bt
end

commands
  faultdump
end

echo \nReady. In the probe-rs gdb terminal the target is halted; type:  load   then  continue\n
echo Then run the repro ROM and play until the HardFault breakpoint fires.\n
