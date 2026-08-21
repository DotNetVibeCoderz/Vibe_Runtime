# RustCLR on a Kendryte K210 (Sipeed Maix Go)

Firmware that runs RustCLR's metadata reader and garbage collector on a
**Kendryte K210** — dual RV64GC.

The second RISC-V board and the first 64-bit one; the ESP32-C3 is RV32IMC. It
prints the same report as every other board.

**It executes IL.** The K210 has 6 MB of SRAM against a peak need of 261 KB,
so it is the one board of the seven with room to spare: it gets the full set of
RustBCL bindings without anything being trimmed. See
[docs/limitations.md](../../docs/limitations.md).

**Status: built, not yet flashed.** No K210 was connected when this was
written. The image builds; the run is the step outstanding.

---

## Build and flash

`riscv64gc-unknown-none-elf` is an upstream target, so stable Rust builds it:

```bash
cd embedded/k210
cargo build --release
cargo objcopy --release -- -O binary fw.bin

kflash -p COM<n> -b 1500000 fw.bin
```

The Maix Go bridges UARTHS to USB through an on-board STM32F103, so the console
is the same port `kflash` uses — 115200 baud, and the report prints on boot.

---

## No internal flash, so the layout is RAM-only

The K210 has none. Its mask ROM reads the image out of the board's SPI NOR part,
copies it to `0x80000000` and jumps there — so text, rodata, data, bss, heap and
stack all live in SRAM, with no load-address/run-address split to arrange.

**6 MB, not 8.** The general-purpose SRAM is two banks — 4 MB at `0x80000000`
and 2 MB at `0x80400000` — which are contiguous and described as one region. The
2 MB above that is the KPU's AI RAM, usable as ordinary memory only after the AI
clock domain is ungated, so it is left out rather than handed to the linker on
trust.

Both harts leave the mask ROM; `_max_hart_id = 1` lets the second reach
`_mp_hook` and park in `wfi` rather than being sent to a busy-loop `abort`.

This layout, and the reasoning, come from the
[RustNet K210 port](../../../RustNet/runtime/firmware-k210).

## The clock is read, not assumed

UARTHS derives its baud divisor straight from the core clock:
`div = cpu_hz / baud - 1`. A firmware that assumes 26 MHz on a board the ROM
left running from PLL0 produces a port that opens and prints nothing but noise.

So `cpu_hz()` reads PLL0 and the clock selector and works the frequency out —
`in0 / (clkr+1) * (clkf+1) / (clkod+1)`, or the bare crystal when the PLL is
bypassed — and the banner prints what it found:

```
clock            <n> MHz core, read from PLL0; UARTHS at 115200 baud
```

The pads are deliberately left alone: the mask ROM already muxes IO4/IO5 to
UARTHS on every Maix board, because it used them itself to report.

## Memory

| | |
| --- | --- |
| `HEAP_BYTES` | 512 KB of the 6 MB SRAM, a static array in `.bss` |
| `MANAGED_SLOTS` | 128 — a **ceiling**, not a hint |

The heap being a static array rather than the linker's `.heap` region keeps
`_heap_size` at zero, which gives `.stack` everything between the end of `.bss`
and the top of SRAM.
