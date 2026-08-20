# RustCLR on a Meadow F7 Micro

Firmware that runs RustCLR's metadata reader and garbage collector on a
Wilderness Labs **Meadow F7 Micro v1.0** — STM32F777, Cortex-M7 at 216 MHz.

Verified on hardware: [docs/logs/meadow-f7.log](../../docs/logs/meadow-f7.log).
This is the third architecture; the same two crates run on Xtensa LX6 and
RISC-V from [embedded/esp32-demo](../esp32-demo), and all three print
**byte-identical** reports.

**It does not execute IL.** `rustclr-core` still needs `std`. See
[docs/limitations.md](../../docs/limitations.md).

---

## Restoring the board

**Flashing this replaces whatever was in internal flash.** On the board this
was developed against that was the RustNet Meadow F7 firmware, which is backed
up here byte-for-byte:

```bash
dfu-util -a 0 -s 0x08000000:leave -D backup/fw.bin.original
```

`backup/meadow-f7-flash-before.bin` is a direct read of the board's flash taken
before anything was written, and it matches `fw.bin.original` exactly for the
313,344 bytes the read completed — which is what makes the restore trustworthy
rather than hopeful.

---

## Building and flashing

`thumbv7em-none-eabihf` is an upstream target, so stable Rust builds it:

```bash
cd embedded/meadow-f7
cargo build --release
cargo objcopy --release -- -O binary fw.bin

dfu-util -a 0 -s 0x08000000:leave -D fw.bin
```

Enter DFU by unplugging, holding **BOOT**, replugging, then releasing.

Reading the console — the board appears as a CDC serial port
(`RustCLR Meadow F7`, VID 16C0 / PID 27DD):

```bash
# any terminal at 115200; the report prints when the port is opened
```

---

## Two things that are easy to get wrong

**The crystal frequency is not published.** USB needs exactly 48 MHz, which
needs the crystal, and the Meadow's is documented nowhere. The
[RustNet Meadow F7 port](../../../RustNet/runtime/firmware-meadow-f7)
established it the hard way — sweeping candidates and letting a USB host
adjudicate — and recorded the answer: **25 MHz**, an Abracon ABM12W-25 at X401
on PH0/PH1. This firmware uses that directly, and prints what it actually
locked to so the claim stays checkable:

```
clock            216 MHz from HSE (25 MHz crystal)
```

The divisors follow from it: `/M=25` to a 1 MHz PLL input, `xN=432` to a
432 MHz VCO, `/P=2` for the core and `/Q=9` for exactly 48 MHz.

**Enumeration is not the same as someone reading.** A configured CDC device
with no terminal attached accepts writes into the endpoint and drops them when
it fills. The first build of this firmware printed on enumeration and lost most
of its report that way. It now waits for the host to assert **DTR** — a
terminal actually opening the port — and reports once per session, so
reconnecting gives a fresh report rather than silence.

---

## Memory

| | |
| --- | --- |
| `FLASH` | 1 MB of the part's 2 MB, at `0x08000000` |
| `RAM` | 384 KB at `0x20020000` — SRAM1 + SRAM2 |
| `HEAP_BYTES` | 64 KB for the allocator |
| `MANAGED_SLOTS` | 128 — a **ceiling**, not a hint |

RAM deliberately starts at SRAM1 rather than `0x20000000`. The 128 KB below is
DTCM: the fastest memory on the part, but tightly coupled to the core and **not
reachable by DMA**. Handing it to the allocator would work until the first
driver DMA'd into a buffer that happened to land there. That layout, and the
reasoning, come from the RustNet port.

`Heap::embedded(n)` refuses to allocate past `n` rather than growing — the
firmware fills it to exactly 128 slots and prints `refused past it  true`.
