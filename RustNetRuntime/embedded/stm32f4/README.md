# RustCLR on STM32F4

Two boards from one source file: a **Nucleo-F401RE** and a **Netduino 3 WiFi**.
Both are Cortex-M4F; they differ in how much memory they have, and that
difference decides whether C# runs.

| | Nucleo-F401RE | Netduino 3 WiFi |
| --- | --- | --- |
| Part | STM32F401RET6 | STM32F427VIT6 |
| Flash | 512 KB | 2 MB |
| RAM | 96 KB | 192 KB + 64 KB CCM |
| Clock | 84 MHz from the HSI | 168 MHz from a 25 MHz HSE |
| Console | USART2 on PA2 — the ST-Link's virtual COM port | UART7 on PE8, goPort2 header |
| LED | LD2, PA5 | USR_LED, PA10 |
| Heap given | 64 KB | 192 KB |
| RustBCL tier | **none** | **minimal** |
| Image `.text` | 21 KB | 282 KB |

```bash
cargo build --release --no-default-features --features nucleo-f401re
cargo build --release --no-default-features --features netduino3-f427vi
```

**Neither board has been flashed.** No F401RE or F427VI was connected when this
was written. Both images build and the layouts are verified by reading the
linked ELF, but the run is the step outstanding. Flashing needs a probe — there
is no bootloader in the picture:

```bash
probe-rs run --chip STM32F401RETx target/thumbv7em-none-eabihf/release/rustclr-stm32f4
probe-rs run --chip STM32F427VITx target/thumbv7em-none-eabihf/release/rustclr-stm32f4
```

The Nucleo has an ST-Link on board, so it needs no extra hardware. The Netduino
needs a probe wired to SWD, and a USB-serial adapter on PE8 to see the console
at 115200 baud.

---

## The F401RE cannot run a C# program, and says so

Loading the runtime costs **192,045 bytes** with the smallest useful set of
RustBCL bindings — console, strings and maths. The F401RE has 96 KB of RAM in
total. There is no arrangement of that memory that runs a program, so the
firmware reports the shortfall instead of discovering it as an allocator panic:

```
-- il interpreter --
heap budget      65536 bytes
bcl tier         none - SKIPPED
                 the runtime needs 192045 bytes
                 to load at all, and this board has
                 65536. Metadata and the collector
                 still run; IL execution does not.
```

It still reads a real Roslyn-built assembly out of flash and still exercises the
collector, which is what the board *can* do.

**The image does not carry the interpreter it cannot use.** `Tier::for_budget`
is a `const fn` and `HEAP_BYTES` is a constant, so LTO folds the decision at
compile time, finds the `Full` and `Minimal` arms unreachable, and strips the
loader and all 766 native bindings. That is why `.text` is 21 KB here against
282 KB on the F427VI. It was not designed for — it fell out of making the tier
a constant expression — but it is the right outcome, and it is worth knowing
that a board below the threshold pays no flash for being below it.

---

## The F427VI runs C#, but only by swapping its memories around

The part advertises 256 KB of RAM. It is in two pieces that are not adjacent:

- **192 KB at `0x20000000`** — SRAM1 (112K) + SRAM2 (16K) + SRAM3 (64K),
  contiguous, DMA-reachable.
- **64 KB of CCM at `0x10000000`** — reachable by the core, not by DMA.

Give the allocator only the first and it gets 192 KB *minus* whatever `.data`,
`.bss` and the stack take out of it. The threshold is 192,045 bytes. A few
kilobytes of statics is the difference between this board running a program and
not running one.

So `memory-f427vi.x` swaps the roles you would expect:

```
RAM  -> CCM  (0x10000000, 64K)   .data, .bss and the stack
SRAM -> SRAM (0x20000000, 192K)  the managed heap, in one unbroken piece
```

`cortex-m-rt`'s `link.x` hardcodes `> RAM` for `.data`, `.bss` and the stack, so
naming CCM `RAM` is what moves them. Nothing in this firmware uses DMA, which is
the only thing CCM cannot do.

The heap gets its own `(NOLOAD)` output section rather than being a `static` in
`.bss` — a `static` in `.bss` would follow `.bss` into CCM, which is the one
place it must not go. `(NOLOAD)` also keeps 192 KB of zeros out of the image and
out of the startup memset; the allocator does not need its arena zeroed.

Verified by reading the linked ELF rather than by assertion:

```
Idx Name            Size     VMA
  4 .data           00000008 10000000     <- CCM
  6 .bss            00000020 10000008     <- CCM
  7 .sram_heap      00030000 20000000     <- all 196,608 bytes of SRAM
_stack_start        10010000              <- top of CCM
```

196,608 clears the 192,045 threshold by **4,563 bytes**. That is a real margin
but a thin one; if RustBCL's binding set grows, re-measure before assuming this
board still fits.

This is the same shape of problem as the ESP32-WROOM-32's second DRAM bank, and
has the same cause: the runtime wants one large contiguous arena, and these
parts do not offer one by default.

---

## Why the memory map comes from `build.rs`

The two parts differ in flash size, RAM size **and** in which physical memory
holds `.bss`. Picking the wrong `memory.x` produces a firmware that links and
then misbehaves at run time. Selecting it from the board feature in `build.rs`
means it cannot be chosen wrongly, and selecting both features is a build error
rather than a coin toss.

---

## Reference

The board facts — pins, clocks, part numbers — come from the RustNet STM32 port
in `../../../RustNet/runtime/firmware-stm32`, which established them on real
hardware. The memory layout here is different: that firmware reserves a flash
sector for storage and keeps CCM out of the allocator deliberately, because it
does use DMA. This one writes no flash and does no DMA, so it can spend both.
