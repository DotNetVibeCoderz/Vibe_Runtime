# RustCLR on a Kendryte K210 (Sipeed Maix Go)

Firmware that runs RustCLR's metadata reader and garbage collector on a
**Kendryte K210** — dual RV64GC.

The second RISC-V board and the first 64-bit one; the ESP32-C3 is RV32IMC. It
prints the same report as every other board.

**It executes IL.** The K210 has 6 MB of SRAM against a peak need of 261 KB,
so it is the one board of the seven with room to spare: it gets the full set of
RustBCL bindings without anything being trimmed. See
[docs/limitations.md](../../docs/limitations.md).

**Status: builds; boots from SRAM on a HuskyLens, but prints nothing there.**

A Sipeed **HuskyLens** — a K210 vision module — was tried on 2026-08-21. The
useful finding is about *how* to try one safely:

**`kflash -s` boots from SRAM and never touches flash.** The K210 has no
internal flash, so its firmware lives on an external SPI part, and on a
commercial module that part holds the product's own firmware. `--sram` loads
into RAM and jumps there; a power cycle brings the product back. The HuskyLens
was booted this way several times and its firmware — camera bring-up, UI, face
recognition — returned intact each time. Nothing was erased.

```bash
kflash -p COM<n> -b 1500000 -s fw.bin     # SRAM only; flash untouched
```

**What did not work, and what was ruled out.** The ISP handshake, the download
and `Boot user code from SRAM` all succeed, and then the board is silent —
nothing at 115200, 230400, 921600 or 1500000 baud. Four things were checked and
none of them is the cause:

* **Not the baud.** The divisor is computed from a PLL0 read, so a wrong clock
  would give a wrong baud — but a build that skips programming the divisor
  entirely, inheriting the one `kflash` just used at 1.5 Mbaud, is equally
  silent.
* **Not the UART.** `kflash`'s ISP talks over UARTHS and checks the replies, so
  transmit demonstrably reaches the host moments earlier.
* **Not the entry address.** The ELF's entry is `0x80000000`, `.text` starts
  there, and `.bss`/`.stack` are `NOLOAD`, so the 563 KB image is exactly what
  should sit at the boot address.
* **`main` is never reached.** A raw write to `HS_TXDATA` as the first statement
  of `main` — no clock, no heap, no allocator — produced nothing either. The
  failure is in the hand-off or in `riscv-rt`'s startup, before any of this
  crate's code runs.

That is as far as it could be taken without a second K210 to compare against or
a way to see the chip's state. It is recorded here rather than guessed at.

So RISC-V **64** is still unverified on hardware. RISC-V **32** is verified — the
ESP32-C3 runs the interpreter.

**One thing changed as a result.** The report now repeats every few seconds
rather than printing once. On a board with flash, a single print is fine: the
firmware is still there after a host attaches and resets it. Here a reset
discards SRAM and boots the *other* firmware, so a report printed once is
unobservable by construction. That change is itself unverified, for the same
reason everything else here is.

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
