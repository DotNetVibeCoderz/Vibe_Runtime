# RustCLR on an RP2040 (Raspberry Pi Pico)

Firmware that runs RustCLR's metadata reader and garbage collector on a
**Raspberry Pi RP2040** — Cortex-M0+ at 125 MHz.

The smallest core of the family: ARMv6-M, no divide instruction, no
compare-and-swap, 264 KB of SRAM for everything. It prints the same report as
every other board, which is the point of including it.

**It executes IL, but only just.** The RP2040 is the tightest of the five
boards: 192 KB of its 256 KB goes to the allocator, which clears the reduced
binding set (192,045 bytes) by under 5 KB and does not come close to the full
one (260,702). So this board runs with console, strings and maths — no LINQ,
collections, reflection or tasks. The firmware works that out from its heap
budget rather than being told. See
[docs/limitations.md](../../docs/limitations.md).

**Status: built, not yet flashed.** No RP2040 was connected when this was
written. The image builds and the UF2 is produced; the run is the step
outstanding.

---

## Build and flash

`thumbv6m-none-eabi` is an upstream target, so stable Rust builds it:

```bash
cd embedded/rp2040
cargo build --release
python tools/elf2uf2.py \
    target/thumbv6m-none-eabi/release/rustclr-rp2040 rustclr-pico.uf2
```

Hold **BOOTSEL** while plugging the board in, then copy the UF2 to the
`RPI-RP2` drive that appears:

```bash
cp rustclr-pico.uf2 /d/          # whatever letter RPI-RP2 mounts as
```

The board reboots into the firmware and appears as a CDC serial port
(`RustCLR RP2040`, VID 16C0 / PID 27DD). Open it at any baud; the report prints
when the port is opened.

---

## Boot, and the 256 bytes that decide everything

The RP2040 has no internal flash. Its ROM reads 256 bytes from offset 0 of the
QSPI part, checks their CRC and runs them; that stage sets up execute-in-place
and the image proper starts immediately after it.

`rp2040-boot2` supplies those bytes prebuilt. A wrong CRC is a board that
enumerates as BOOTSEL again on the next power-up and never runs anything — not
a thing worth hand-assembling. `memory.x` reserves the region:

```
BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
RAM   : ORIGIN = 0x20000000, LENGTH = 256K
```

Both the layout and the reasoning come from the
[RustNet RP2040 port](../../../RustNet/runtime/firmware-rp2040).

## Why a HAL here and not on the STM32

`rp2040-hal` supplies the clock tree and the USB bus. The Meadow port drives its
STM32 at the register level, so taking a HAL here is worth justifying: the
crystal-to-*exactly*-48 MHz path USB needs has no tolerance, and a maintained
implementation of it is cheaper engineering than rediscovering the PLL
constants. The RP2040's `init_clocks_and_plls` does that in one call.

## Memory

| | |
| --- | --- |
| `HEAP_BYTES` | 96 KB of the chip's 264 KB SRAM |
| `MANAGED_SLOTS` | 128 — a **ceiling**, not a hint |
