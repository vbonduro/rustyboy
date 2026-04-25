# SD Card Module (ACEIRMC) — SPI Behaviour Notes

## Module circuit

The ACEIRMC micro-SD module is a clone of the Catalex design.  Key components:

| Component     | Part         | Role |
|---------------|--------------|------|
| LDO regulator | AMS1117-3.3  | VBUS (5 V) → 3.3 V for SD card |
| Level buffer  | 74LVC125A    | Quad 3-state buffer, Vcc tied to **3.3 V rail** (AMS1117 output) |
| Pull-ups      | ~10 kΩ       | On all four SPI lines between SD socket and buffer inputs |

Because the 74LVC125A is powered from 3.3 V, all buffer outputs are 3.3 V.
The module is safe to connect directly to the RP2350 (3.3 V GPIO).

### Signal routing through the buffer

| Signal | Direction   | Buffer path |
|--------|-------------|-------------|
| CLK    | MCU → card  | A = MCU CLK,  Y = card CLK  |
| MOSI   | MCU → card  | A = MCU MOSI, Y = card MOSI |
| CS     | MCU → card  | A = MCU CS,   Y = card CS   |
| MISO   | card → MCU  | A = card MISO, Y = MCU MISO |

### /OE pins

All four /OE (output enable, active-LOW) pins are **permanently tied to GND**.
This means every buffer is always enabled — the MISO buffer never tri-states,
even when CS is HIGH.  This is a known design issue in this class of modules
(documented in the Catalex/Open-Smart errata; a fix is to lift pin 13 of the
74LVC125A and connect it to the CS net instead of GND, so MISO tri-states when
the card is deselected).

For a single-device SPI bus this does not cause bus contention, but it does
mean the MCU always sees the buffered SD card MISO regardless of CS state.

---

## Why MISO reads 0xFF

### 1. SD card MISO is open-collector / tristate (SD spec behaviour)

The SD Physical Layer spec defines the card's DO (MISO) pin as open-drain.
The card only **actively drives MISO LOW** (for 0-bits and R1 response bytes
with bit 7 = 0).  It either weakly drives or releases MISO for 1-bits and
during idle periods.

Confirmed by carlk3's well-tested Pico SD library:
> "The SPI MISO (DO on SD card) is open collector (or tristate).
>  It should be pulled up.  The Pico internal pull-up is weak (~56 kΩ / 60 µA).
>  It's best to add an external pull-up of around 5 kΩ to 3.3 V."

### 2. RP2350 MISO pull-up is disabled by embassy-rp

Embassy-rp's `Spi::new_blocking` explicitly disables both pull-up and
pull-down on the MISO pin (spi.rs, pad_ctrl write for miso):

```rust
w.set_pue(false);   // pull-up disabled
w.set_pde(false);   // pull-down disabled
```

The only pull-up on the MISO line is therefore the module's ~10 kΩ resistor.

### 3. What 0xFF means in practice

| MISO state             | Who is driving it | MCU reads |
|------------------------|-------------------|-----------|
| Card actively LOW      | SD card           | 0x00 – 0x7F (valid R1) |
| Card releases / high-Z | 10 kΩ module pull-up | 0xFF (filler / not responding) |
| Card stuck / non-responsive | 10 kΩ module pull-up | 0xFF on every byte |

When the SD card is functioning correctly it pulls MISO LOW for R1 response
bytes.  When it enters a stuck or non-responsive state it stops driving MISO
at all, the module pull-up holds the line HIGH, and the MCU reads 0xFF on
every SPI transfer → `TimeoutCommand` in embedded-sdmmc.

---

## Observed SPI / SD init behaviour in this project

| Session state                        | CMD0 response | Meaning |
|--------------------------------------|--------------|---------|
| Card not powered (3.3 V VCC typo)   | 0xFF timeout  | AMS1117 output ~2.3 V, card off |
| Clean power-on (VBUS), first run     | **0x01**      | R1_IDLE_STATE — correct, card entered SPI mode |
| Soft MCU reset, card still init'd   | **0x00**      | R1_READY_STATE — card stayed in SPI mode from previous session |
| After many CS-toggle retry cycles   | 0xFF timeout  | Card stopped driving MISO — stuck state |

### Normal expected SD init sequence (for reference)

```
CS HIGH + ≥80 CLK pulses    → card internal state machine wakes (native → SPI receptive)
CS LOW  + CMD0 (CRC=0x95)   → R1 = 0x01  (idle state)           ← only seen once cleanly
CMD8    (0x1AA)             → R1 = 0x01 + 4 bytes voltage echo
CMD55 + ACMD41 loop         → R1 = 0x01 while initialising, 0x00 when ready
CMD58                       → OCR register, check CCS bit for SDHC/SDXC
```

---

## Known causes of getting stuck in 0xFF

1. **Repeated CS-toggle retries** — toggling CS HIGH/LOW 50+ times while the
   card is in an intermediate state can confuse the SPI state machine.
   The SdFat reference implementation keeps CS LOW throughout all CMD0 retries
   (clocking 0xFF bytes with CS asserted) rather than deasserting between
   retries.  Our CS-toggle patch was intended to help the "already active" case
   but likely caused the deeper stuck state.

2. **Card in mid-transaction after MCU reset** — after a watchdog or
   probe-rs-triggered reset the card retains its SPI state.  If the card was
   mid-block-read or mid-write, it may not respond to CMD0 until the
   in-progress block is clocked out (515 bytes max).

3. **Card in native SD mode** — per spec this should not happen without a
   power cycle, but some cheap cards behave non-conformantly.

---

## Fixes / mitigations

### Software — enable RP2350 internal pull-up on MISO (GP16)

After `Spi::new_blocking`, add:

```rust
// embassy-rp disables pull-ups on SPI pins; re-enable on MISO (GP16)
// because SD card MISO is open-collector and needs a pull-up.
embassy_rp::pac::PADS_BANK0.gpio(16).modify(|w| w.set_pue(true));
```

This puts the RP2350 ~56 kΩ pull-up in parallel with the module's ~10 kΩ,
giving ~9 kΩ total — closer to the recommended 5 kΩ and better for
open-collector signal integrity.

### Software — keep CS LOW during CMD0 retries (revert CS-toggle patch)

The original embedded-sdmmc / SdFat behaviour: flush 0xFF bytes with CS
**asserted** (LOW) between CMD0 retries.  This clocks out any pending
data block without confusing the card with CS transitions.  Only raise CS
between full `acquire()` attempts, not between individual CMD0 commands.

### Hardware — GP20 power switch (planned, Bead 4)

Wire GP20 to a P-MOSFET or load switch (AP2112 / TPS22860) that gates VBUS
to the module VCC pin.  The firmware can then:

```
GP20 LOW  for ≥100 ms  → module off, capacitors discharge
GP20 HIGH + 250 ms     → module on, card power-on reset
re-initialise SdCard object
```

Circuit and logic level details are in CLAUDE.md (SD module power switch
section).  This is the definitive fix for stuck-card recovery without manual
power cycling.

### Hardware — external 4.7 kΩ pull-up on MISO (optional improvement)

Add a 4.7 kΩ resistor from GP16 (MISO) to the 3.3 V rail.  Together with the
module's 10 kΩ this gives ~3.2 kΩ, well within the recommended range and
robust for SDXC cards at higher SPI speeds.

---

## References

- [SD Card Reader MISO Hack — Hackaday.io](https://hackaday.io/project/164296-sd-card-reader-miso-hack)
- [MicroSD Card Hardware Fault Fixed — Arduino Forum](https://forum.arduino.cc/t/microsd-card-hardware-fault-fixed/625442)
- [no-OS-FatFS-SD-SPI-RPi-Pico README — carlk3/GitHub](https://github.com/carlk3/no-OS-FatFS-SD-SPI-RPi-Pico)
- [SD Card Not Working Without Pull-up on MISO — Arduino Forum](https://forum.arduino.cc/t/sd-card-not-working-unless-i-enable-pullup-on-miso-is-this-okay/147327)
- [In-Depth Tutorial: Micro SD Card Module — Last Minute Engineers](https://lastminuteengineers.com/arduino-micro-sd-card-module-tutorial/)
- [SD Level Shifting Using 74HC125 — Arduino Forum](https://forum.arduino.cc/t/sd-card-level-shifting-using-74hc125-ic/174703)
