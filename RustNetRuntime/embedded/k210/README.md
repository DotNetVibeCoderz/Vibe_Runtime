# RustCLR on a Kendryte K210 (Sipeed Maix Go)

Firmware that runs RustCLR's metadata reader, IL interpreter and garbage
collector on a **Kendryte K210** — dual RV64GC.

The second RISC-V board and the first 64-bit one; the ESP32-C3 is RV32IMC. It
prints the same report as every other board.

**Status: it runs.** Verified on a Sipeed **Maix Go** on 2026-08-24, booted into
SRAM over COM10. `HelloWorld.Main` executes and prints the same three lines
`dotnet` does:

```
-- il interpreter --
heap budget      1048576 bytes
bcl tier         full (260702 bytes needed)
native bindings  992
types registered 245

--- program output ---
Hello from RustCLR
42
120

--- end ---
exit code        0
il executed      68
```

The collector suite passes too: a rooted cycle survives, an unrooted one is
reclaimed, a stale handle reports invalid, and the slot ceiling is refused past
rather than exceeded. The K210 has 6 MB of SRAM against a peak need of 261 KB,
so it is the one board of the seven with room to spare — it gets the full set of
RustBCL bindings with nothing trimmed.

**RISC-V 64 is therefore verified on hardware.** What is *not* verified is the
RISC-V code generator: `crates/rustclr-jit` is `std`-only, and its executable
pages come from `mmap`/`VirtualAlloc`, neither of which exists here. See
`docs/limitations.md`.

---

## Heap: 2 MB, and why not 4

`HEAP_BYTES` is 2 MB of the 6 MB SRAM. 1 MB was generous for a console program
and not enough for a graphical one — the `graphics-primitives` template asks
for a 320x240 RGB565 canvas, 153,600 bytes on top of RustBCL's 261 KB, and the
allocator refused it.

**4 MB was tried and the board went silent**: it booted, printed nothing, and
looked exactly like a firmware that had failed to start. Recorded here rather
than left as a number somebody will try again. 2 MB keeps the array inside the
first SRAM bank, with the image below it and the stack above, and it prints.

## The panel, and the backlight

`Display.Present()` reaches glass only if the firmware attaches a panel. It is
a decision the firmware makes because attaching takes SPI0 out of general
circulation — after `attach_panel`, the HAL report correctly says
`spi bus 0  not on this chip`.

```
[panel] ST7789V 320x240 on SPI0 octal @ 15 MHz (SPI0 source 780 MHz)
```

**Initialising the panel is also what turns the backlight on.** The MaixLCD's
backlight rail hangs off a PMOS switched by the panel's reset line, so a panel
that is never reset stays dark however much is drawn into it — a running demo
on a black screen.

## Carrying an application and its libraries

A board has no filesystem, so an `AssemblyRef` cannot be resolved by looking
beside the program the way `rustnet run` does. The firmware carries what the
program references, in dependency order:

```bash
RUSTCLR_APP=MyApp.dll RUSTCLR_LIB1=RustNet.Core.dll RUSTCLR_LIB2=RustNet.Graphics.dll cargo build --release
```

Two is the current ceiling — `include_bytes!` needs one literal per file. A
program needing more says so at build time rather than failing to resolve a
token on the board.

## The bug the board found

The fifth report was fine and the sixth died:

```
panic: memory allocation of 73728 bytes failed
```

On a 1 MB heap, with the collector reporting nothing live. Not a leak —
**fragmentation**. Each report builds the whole runtime (260 KB of bindings, a
98 KB slot table) and tears it down again, and after five rounds a linked-list
allocator has no contiguous 72 KB block left.

The fix is to reset the allocator before each report, which is sound here
because nothing that outlives an iteration is on the heap: `run` drops
everything it made, `Detail` is a fixed 96-byte array, and `Uarths` is a unit
struct. Those three facts are the whole proof obligation, and a heap-backed
local added outside the loop would break it.

Verified the same way it was found: **45 consecutive reports, no panic**, where
the previous build died at six.

---

## A HuskyLens is silent, and that is the HuskyLens

A Sipeed **HuskyLens** — a different K210 product — was tried first, on
2026-08-21. The ISP handshake, the download and `Boot user code from SRAM` all
succeeded, and then nothing came back at any baud. Four causes were ruled out:
not the baud (a build that skips the divisor entirely is equally silent), not
the UART (`kflash`'s own ISP had just talked over it), not the entry address
(`0x80000000`, where `.text` starts), and `main` is never reached — a raw write
to `HS_TXDATA` as its first statement produced nothing either.

**The Maix Go settles it.** The same firmware, built the same way, prints on a
Maix Go. The failure is specific to the HuskyLens' hand-off or its `riscv-rt`
startup, not to this crate.

---

## Booting it safely

**`kflash -s` boots from SRAM and never touches flash.** The K210 has no
internal flash, so its firmware lives on an external SPI part — and on a
commercial module that part holds the product's own firmware. `-s` loads into
RAM and jumps there; a power cycle brings the product back. The HuskyLens was
booted this way several times and its camera bring-up, UI and face recognition
returned intact each time.

```bash
cd embedded/k210
cargo build --release
cargo objcopy --release -- -O binary fw.bin

kflash -p COM10 -b 1500000 -s fw.bin      # SRAM only; flash untouched
```

Then watch the console — **with DTR and RTS cleared**. On this board they drive
ISP and reset, and asserting either discards the SRAM image and boots whatever
is on the SPI flash instead:

```python
s = serial.Serial(); s.port = "COM10"; s.baudrate = 115200
s.dtr = False; s.rts = False; s.open()
```

That is also why the report **repeats** rather than printing once. On a board
with flash a single print is fine — the firmware is still there after a host
attaches and resets it. Here a reset discards SRAM, so a report printed once
would be unobservable by construction.

The Maix Go bridges UARTHS to USB through its on-board FTDI FT2232, so the
console is the same port `kflash` uses, at 115200 baud.

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
| `HEAP_BYTES` | 2 MB of the 6 MB SRAM, a static array in `.bss`, re-initialised before each report |
| `MANAGED_SLOTS` | 128 — a **ceiling**, not a hint |

The heap being a static array rather than the linker's `.heap` region keeps
`_heap_size` at zero, which gives `.stack` everything between the end of `.bss`
and the top of SRAM.
