# Progress

Tracking for RustNetRuntime. Updated 2026-08-24.

**Verified this session:** `cargo test --workspace` → 416 passed, 0 failed.
`bash tests/embedded.sh` → 14 crates × 4 bare-metal targets, all built without
`std`. `bash tests/firmware.sh` → seven firmwares, each installing its real board.
`bash tests/templates.sh` → all fourteen application templates build.
Five programs — two conformance suites, a sample, and two written or edited by
the assistant against a live LLM — produce byte-identical output on RustCLR and
.NET.

```
Device         64 checks, 0 failures  (HAL, storage, crypto, graphics)
Conformance    IDENTICAL — checks=361 failures=0
ModernSyntax   IDENTICAL — checks=35 failures=0
UserDirectory  IDENTICAL
PrimeSieve     IDENTICAL   (written by Jack from a prompt)
SensorGateway  IDENTICAL   (edited by Jack from a prompt)
```

## 2026-10-11 — fixes found by Rustzor's apps

Rustzor (C# web apps on this runtime in WebAssembly) drove these out with real
apps; each has a check that runs on .NET and here.

**Verified:** `cargo test --workspace` → 471 passed, 0 failed. Conformance
`checks=463 failures=0` on .NET and on RustCLR. Cross-assembly fixture
`checks=8 failures=0`. `bash tests/embedded.sh` → all targets built without `std`.

- New: `System.Decimal` (`decimal.rs`), `System.Text.RegularExpressions`
  (`regex_engine.rs`, `regex.rs`), `DateTimeOffset`, `TimeSpan.Parse`, Guid
  formats, `CancellationToken`/`CancellationTokenSource` (`cancellation.rs`),
  `OperationCanceledException`, surrogate helpers on `char`,
  `Enumerable.AsEnumerable`, `Encoding.GetString(byte[], int, int)`,
  `Enum.CompareTo`, non-generic `IList`/`IDictionary`, reflection over
  framework generics.
- Fixed: `await` in a Debug build resumed a class state machine through a
  pointer; interface dispatch skipped an override taking a library struct;
  quadratic `string.Length`/`Substring`; GC holes around `newobj` arguments,
  native arguments and callback results; `Random(seed)` ignored the seed;
  enum arrays from an image blob came back as zeros; boxed enums, chars and
  bools compared, hashed or printed by reference or as numbers; half a
  surrogate pair vanished in `char.ToString` and `StringBuilder`;
  `Activator.CreateInstance` skipped a framework collection's native
  constructor; overloads that differ only in generic arity bound to the wrong
  one across assemblies.
- New tests: `tests/gc_natives.rs`, `tests/cross_assembly.rs` (fixture in
  `tests/fixtures/CrossAssembly`), and conformance files `RegexChecks`,
  `RandomChecks`, `DecimalChecks`, `GcChecks`, `EnumChecks`,
  `ActivatorChecks`, `TextChecks`, `CancellationChecks`.

---

## Done — Rust runtime

| Crate | State | Evidence |
| --- | --- | --- |
| `rustclr-metadata` | PE/COFF + ECMA-335 reader, signatures, IL bodies | 28 tests, incl. 9 against a real Roslyn-built assembly |
| `rustclr-gc` | Handle-based heap, pluggable collectors, mark-sweep, **shared heap + stop-the-world safepoints** | 22 tests; 200k-deep graph marks without stack overflow |
| `rustclr-core` | Type system, loader, IL interpreter, exception handling, **threads, monitors, task pool**, debugger hooks, **sampling profiler** | 31 tests, incl. multi-threaded |
| `rustclr-bcl` | Native BCL: Console, String, Math, interpolation, tuples, ranges, Nullable, generic collections, LINQ, Task and async, spans, marshalling, reflection | 29 tests; 988 bindings |
| `rustclr-sched` | Lock-free MS queue, MPMC channel, thread pool | 17 tests; the queue is the task pool's |
| `rustclr-interop` | P/Invoke, dynamic loading, marshalling | 9 tests; calls the real `GetCurrentProcessId` |
| `rustclr-jit` | Compiler trait, IL verifier, analysis, **x86-64 code generator** with floats, arrays, allocation and recursion, W^X pages, tiering | 62 tests, incl. differential against the interpreter |
| `rustclr-hal` | Board traits: GPIO, I2C, SPI, UART, I2S, PWM, ADC, CAN, 1-Wire, RTC, watchdog, extmem, netif, signals, camera | Zero dependencies; builds on four bare-metal targets |
| `rustclr-hal-host` | A simulated board, so hardware code is testable on a desktop | 15 tests |
| `rustclr-flashfs` | Log-structured store for raw NOR flash: named blobs, newest wins | 13 tests |
| `rustclr-crypto` | SHA-256, HMAC-SHA256, AES-CTR, CRC-32 — written out, not depended on | 14 tests, all against published vectors |
| `rustclr-hal-k210` · `-esp32c3` · `-rp2040` · `-stm32` | Real peripherals for four chips | 68 tests; each builds for its own bare-metal target |
| `rustclr-secureboot` | Signed image containers, verified by MAC | 8 tests |
| `rustclr-ota` | A/B slots, verified images, automatic rollback | 7 tests |
| `rustclr-gfx` | RGB565 framebuffer, primitives, an embedded 8x8 font | 14 tests, including one that renders the font back to text |
| `rustclr-mqtt` | An MQTT 3.1.1 codec, transport-free so it builds bare-metal | 4 tests |
| `rustclr-usb` | USB descriptors and the CDC/HID classes a board presents | 4 tests |
| `rustclr-db` | An embedded record store with a small SQL surface | 4 tests |
| `rustclr-diag` | Structured logging and health counters | 9 tests |
| `rustclr-net` | HTTP, MQTT, Modbus, a small web server | 7 tests; `std`-only — its transport is sockets |
| `rustnet-cli` | `run` / `info` / `disasm` / `verify` / `build` / `capabilities` / `debug` / `debug --dap` / **`profile`** | Drives every fixture; 5 tests speak DAP to a real process |

### graphics-primitives runs on an M5Stack Tough

An ESP32 draws now. `graphics-primitives` cycles its scenes on a Tough's
ILI9342C panel out of a 320x240 RGB565 canvas, with the **full** binding tier.

```
heap budget      4234220 bytes
bcl tier         full (385070 bytes needed)
native bindings  1067
library 0        RustNet.Core
library 1        RustNet.Graphics
types registered 253            <- the same number as the desktop and the K210
```

Three things had to be true at once, and the board found each of them by
failing differently:

| | |
| --- | --- |
| `one-bank` | The 96 KB reclaimed from `dram2_seg` is not entirely free. It corrupts a loaded assembly once RustBCL has its share — on a Tough as an `Image` whose bytes were byte-perfect and whose stored metadata range had been overwritten, on a Core2 as `LoadProhibited` |
| `psram` | 8 MB registered **behind** internal DRAM. Allocation walks regions in order, so internal is spent first and PSRAM catches the canvas. It is opt-in because `esp-alloc` documents ESP32 atomics as broken in PSRAM; what makes it survivable here is that the `no_std` prelude maps `Arc` to `Rc`, so the refcounts are not atomic — an argument, written down, not a proof |
| 64 KB arena | Down from 176 KB. **esp-alloc's arena is a `.bss` static and the stack is whatever `dram_seg` has left after it**, so asking for 176 KB of a ~200 KB segment left the stack a sliver and the board died on its stack guard. That read like runaway recursion and was linker arithmetic |

Capture: [docs/logs/m5stack-tough-graphics.log](docs/logs/m5stack-tough-graphics.log).

**The console had to stream, again.** Output was buffered into a `CaptureHost`
and printed when the entry point returned, and a graphics demo never returns —
so a board that was drawing correctly looked like one that had failed to boot.
The same trap the K210 fell into, in a firmware that had not been given the
same fix.

### The tools reach the hardware

`RustNet.Deploy`'s catalog now drives a real build and flash of the Tough,
which is the code path `RustNet.Workbench` and CodeGen both call:

```
BUILD OK  .../xtensa-esp32-none-elf/release/rustclr-esp32-demo (998 KiB)
-- espflash -> COM5
App/part. size:    675,968/2,097,152 bytes, 32.23%
FLASH OK
```

Four things were wrong and are fixed:

- **Repository detection looked for `runtime/`**, the reference repo's layout.
  It looks for `crates/` and `embedded/` together now — a lone `Cargo.toml`
  matches every crate on the way up.
- **The recipes were stale.** Xtensa is not an upstream Rust target, so the
  toolchain, the target and `-Z build-std` are part of the recipe rather than
  assumed; and the M5 entries now carry the three features that actually work.
- **The partition table was mandatory.** `--partition-table partitions.csv`
  was passed unconditionally, and a firmware that carries its program in
  `.rodata` has no such file — the flash failed outright. It is passed when
  the file is there. `embedded/esp32-demo/partitions.csv` now exists, widened
  from the reference's 4 MB layout to the 16 MB these boards carry.
- **`kflash` is invoked with `-s`**, so a K210 boots from SRAM and its own SPI
  part is left alone.

**A flash through the tools replaced the running demo with `HelloWorld`,** and
the board sat on the green diagnostic fill with nothing drawn over it. Not a
graphics fault: `BoardFlasher.Build` ran cargo with no `RUSTCLR_APP`, so the
image carried the `HelloWorld.dll` checked in beside the firmware crate. The
flash succeeded, the board booted, and it ran a different program.

`DeviceApp` fixes that — an application and the libraries it references,
turned into the `RUSTCLR_APP` and `RUSTCLR_LIB1..3` the firmware build reads.
`FromBuildOutput` takes the `RustNet.*` assemblies the SDK copied beside the
program, which is already the right list because the device libraries are
separate assemblies and a project only gets what it references. The build now
says what it is carrying either way:

```
  app  Primitives.dll + RustNet.Core.dll, RustNet.Graphics.dll
BUILD OK  .../rustclr-esp32-demo (1015 KiB)
FLASH OK
```

and the board comes up on `assembly Primitives`, `library 0 RustNet.Core`,
`library 1 RustNet.Graphics`, animating.

**CodeGen keeps a second catalog**, and it had drifted exactly as
`BoardCatalog`'s own comment warns. Its tier thresholds were the pre-device
numbers (192,045 / 260,702 against the measured 252,092 / 385,070), which made
it claim an ESP32-C3 clears the full tier and a WROOM clears it on a memory
bank that corrupts. Both corrected, the two M5 boards added, and the K210
marked verified.

### The ESP32's second RAM bank is not entirely ours

An M5Stack Tough (COM5) and an M5Stack Core2 (COM14) both run RustCLR and both
crash when a program loads a second assembly. The symptoms differ — the Core2
dies as `LoadProhibited`, the Tough as `MissingHeap("#~")` from the metadata
reader — and the cause is one thing.

**The bytes were never the problem.** Instrumenting the firmware to print what
it actually holds:

```
library 0        4608 bytes, magic 4D5A
                 owned copy: 4608 bytes, BSJB at Some(620)
```

Byte-perfect, and `BSJB` at exactly the offset the host file has it. The image
is intact; `Metadata::parse` succeeds inside `Image::from_bytes` and then fails
on the *same stored range* one call later. What is being overwritten is the
`Image`'s own range, not the assembly.

**The decisive test was to stop using the bank.** `embedded/esp32-demo` gives
the allocator 176 KB of `dram_seg` plus 96 KB reclaimed from `dram2_seg` — the
memory "past the ROM's data and stacks", where the boundary was inherited from
the segment size rather than measured. Built with the new `one-bank` feature,
the same board on the same program says:

```
heap budget      180224 bytes
bcl tier         none - SKIPPED
                 the runtime needs 252092 bytes to load at all
```

No panic. A clean refusal instead of a corrupted heap. **If a crash goes away
with one bank, the second bank is the crash** — that is what the feature is for.

The bank is not useless: the Tough ran `HelloWorld` out of it months ago and
the log is still accurate. It corrupts under *pressure*, once RustBCL has taken
its share and a program starts loading assemblies of its own. 96 KB is too
much and what is actually free has not been measured, so this is documented in
`embedded/esp32-demo/src/main.rs` as a hazard rather than papered over.

**What it means for these boards.** 176 KB of trustworthy DRAM against a
252,092-byte minimal tier: an ESP32 can read metadata and collect garbage, and
cannot currently execute IL without the risky bank. Both boards have PSRAM,
which would settle it — except that `esp-alloc` documents ESP32 atomics as
broken in PSRAM, and this runtime's collector and safepoints are built on
atomics. Stating that beats a heap that works until it does not.

### The M5 panel driver covers both boards

`embedded/esp32-demo/src/m5.rs` — AXP192 power sequencing and an ILI9342C over
SPI2 — ported from the Gravicode reference's ESP-IDF firmware. The register
sequences carry over unchanged; the plumbing is rewritten against `esp-hal`
because this firmware is bare-metal.

The two boards differ in exactly one thing, and it is worth the type that
carries it: **LDO3 is an ordinary rail on a Tough and the vibration motor on a
Core2**, so a Core2 configured the Tough's way sits there buzzing. `Variant`
holds that difference; everything else is shared.

The green fill lands on the glass on a Core2, which is what proved the whole
path — PMIC rails, backlight, SPI, panel — before any managed code ran. It is
kept for that reason: a panel that is not being written and a framebuffer that
never reaches `present` look identical on a screen still showing the previous
firmware's picture.

### mqtt-dashboard publishes to a real broker, and a stale constant nearly killed a board

The `mqtt-dashboard` template runs, connects to `broker.hivemq.com:1883`,
subscribes to its command topic and publishes telemetry. Confirmed from
outside: a separate raw-socket MQTT client subscribed to the broker and
received three of the dashboard's messages.

```
tx {"seq":3,"uptime_ms":27934,"rx":2,"chip":"host"}
  RECEIVED: rustnet/MqttDashboard/telemetry -> {"seq":3,...}
```

**`RustNet.Net` had no bindings at all.** WiFi and MQTT are bound now, over
`rustclr-net`'s real sockets. `Wifi.Connect` does two honest things rather than
one dishonest one: with a board it associates through the HAL's
`NetInterface`; without one it reports the machine's existing connectivity and
`GetSsid` answers `host`, because a desktop's radio belongs to the operating
system and a made-up network name would be worse than the truth.

**A five-argument `string.Concat` did not work, and that is a .NET 9 change.**
`params string[]` is gone from the reference assembly; Roslyn binds
`params ReadOnlySpan<string>`, builds the arguments into an inline-array local
and hands over a span. The declaring type is a `TypeSpec` this loader renders
as a raw token, so the qualified-name key cannot be written down — the arity
key can, and `String::Concat/1` now reads a span window through
`spans::window_values` rather than guessing. The first attempt bound
`Unsafe.As`, `Unsafe.Add` and `MemoryMarshal.CreateReadOnlySpan` again; they
were already there and already correct, and the duplicates were removed.

### The threshold that lied

An **M5Stack Core2** — ESP32 rev v3.0, 16 MB, on COM14 — booted RustCLR,
reported `bcl tier full (260702 bytes needed)` against its 278,528-byte budget,
and panicked mid-registration on a 46-byte allocation.

`FULL_BCL_BYTES` was measured when RustBCL had 826 bindings. It now has 1,060,
and the constant had not moved. A threshold that says a board will fit and then
kills it is worse than no threshold: the failure lands on a chip nobody can
attach a debugger to.

`cargo test -p rustclr-bcl --test footprint` now measures both tiers through a
counting `GlobalAlloc` and **fails if either constant drifts below what the
bindings cost**:

| tier | was declared | measured |
| --- | ---: | ---: |
| full | 260,702 | **385,070** |
| minimal | 192,045 | **252,092** |

With the real numbers the Core2 takes the minimal tier — 435 bindings — and
runs `HelloWorld.Main` cleanly, printing what `dotnet` prints. Capture:
[docs/logs/m5stack-core2.log](docs/logs/m5stack-core2.log).

**Two things that made the measurement itself wrong before it was right.** The
first run had the tests sharing one process-wide counter, so whichever ran
first paid for lazy statics and reported several kilobytes more — an
order-dependent number, and a threshold guarded by one fails at random. The
second had the serialising lock around the measurement but *not* the warm-up,
so one test's frees landed inside another's window and the live total wrapped
to 18,446,744,073,709,498,820 bytes. Both are fixed, and the figures now
reproduce exactly.

### Templates, tools, and a graphics demo on real glass

Fourteen application templates and three host tools came across from the
Gravicode RustNet reference, and `graphics-primitives` now runs on a Maix Go.

**The managed library is seventeen assemblies, not one.** The first attempt
merged them, on the reasoning that a board pays metadata per assembly. That was
backwards, and measuring said so: a loaded assembly's types are *all* registered
whether the program mentions them or not, so one combined library made a program
that only draws also carry CAN, Modbus, MQTT, a camera, a VNC server and a
database.

| `graphics-primitives` | split (Core + Graphics) | one library |
| --- | ---: | ---: |
| assembly bytes | **23,552** across 3 | 62,976 |
| types registered | **253** | 320 |
| methods registered | **68** | 498 |

430 extra methods for a program that draws. Each template's project references
only the libraries its source names, derived from the file rather than guessed —
the first pass read `using` lines alone and missed the fully-qualified uses,
which is why seven templates would not build.

**One managed change was required, and it is the same one every port needs.**
RustCLR dispatches a native only for a method with **no IL body**, so the
reference's `[InternalCall] ... => throw new RuntimeOnlyException()` would run
the body and the binding would never fire. Every one became `extern` plus
`[MethodImpl(MethodImplOptions.InternalCall)]`, which is the real .NET
convention and what the metadata reader classifies correctly.

**`rustnet-mdp` did not come across, and that is a feature.** The reference
compiles a DLL to an intermediate `RNX` before deploying it. RustCLR reads
PE/ECMA-335 directly, so what is signed and flashed is exactly what Roslyn
produced — one fewer metadata writer to keep in step with the compiler, and one
fewer place for the two to disagree. `RustNet.Deploy`, `RustNet.Workbench` and
`RustNet.Designer` build with that step removed.

`bash tests/templates.sh` instantiates all fourteen the way `rustnet new` would
and builds them. A template that does not compile is not a template, and the
way that gets noticed otherwise is somebody starting a project with one.

### Four things the board found

Getting `graphics-primitives` onto the Maix Go took four fixes, and none of
them was the one that looked obvious.

**A board has no filesystem to resolve an `AssemblyRef` against.** The firmware
carries the libraries and loads them first, named by `RUSTCLR_LIB1`/`LIB2` at
build time. On the chip: `library 0 RustNet.Core`, `library 1
RustNet.Graphics`, then 253 types — the same number as the desktop.

**The board was described and then dropped on the floor.** `exercise_board`
reported what the chip had and the interpreter never got it, so a program that
reached for a pin was told there was none on a board that plainly had one. It
is owned now and moves in.

**The program's console was captured, not streamed.** Output went into a
`CaptureHost` and was printed when the entry point returned — and a device
program loops forever, so nothing was ever printed. The board looked dead while
it was in fact running the demo. This was the one that cost the most time, and
it was diagnosed by noticing the earlier build only printed because it *crashed*.

**A panel that is never reset stays dark.** `Display.Present()` was reaching a
board with no panel attached, which is a documented no-op. Attaching the
ST7789V driver also brings the backlight up: on a MaixLCD the backlight rail
hangs off a PMOS switched by the panel's reset line. `[panel] ST7789V 320x240
on SPI0 octal @ 15 MHz`, and `spi bus 0` correctly starts reporting
`not on this chip` — the panel has claimed the controller.

**And one that was not a bug.** A 320x240 RGB565 canvas is 153,600 bytes and
the K210 firmware claimed 1 MB of its 6 MB SRAM; the allocation failed. Raising
the budget to 2 MB fixed it. 4 MB was tried first and the board went silent —
recorded in `embedded/k210/README.md` rather than left as a number somebody
will try again.

The demo runs indefinitely: gradients, a 3D cube, bouncing balls and matrix
rain, 38 scenes observed without a panic.

### The device layers, and the firmwares that install them

Twelve crates ported or written from the Gravicode RustNet reference, and every
one of them builds on all four bare-metal targets — `tests/embedded.sh` now
checks fourteen crates rather than four, because a crate that only builds on a
desktop is a crate a board cannot use and the way that gets noticed is a
firmware build failing weeks later.

| Crate | What it is | Evidence |
| --- | --- | --- |
| `rustclr-hal` | Board traits — the seam to the hardware | Zero dependencies |
| `rustclr-hal-host` | A simulator, so device code runs on a desktop | 15 tests |
| `rustclr-hal-k210` · `-esp32c3` · `-rp2040` · `-stm32` | Real peripherals, four chips | 68 tests; each builds for its own target |
| `rustclr-flashfs` | Log-structured store for raw NOR flash | 13 tests |
| `rustclr-crypto` | SHA-256, HMAC, AES-CTR, CRC-32 | 14 tests against published vectors |
| `rustclr-secureboot` | Signed image containers | 8 tests |
| `rustclr-ota` | A/B slots, verified images, automatic rollback | 7 tests |
| `rustclr-gfx` | RGB565 framebuffer, primitives, an 8x8 font | 14 tests |
| `rustclr-mqtt` · `-usb` · `-db` · `-diag` | Codecs and stores a device needs | 21 tests |
| `rustclr-net` | HTTP, MQTT, Modbus, a small web server | 7 tests; `std`-only, and the manifest says why |

**Every firmware now installs its real board.** The shared demo takes one and
reports what the chip actually offers. On the Maix Go, over the wire:

```
-- hardware abstraction layer --
board            Sipeed Maix Go
gpio pin 0       yes
adc channel 0    not supported
i2c bus 0        yes
spi bus 0        yes
uart port 0      yes
pwm channel 0    not on this chip
external memory  not on this chip
watchdog         idle
monotonic clock  moves forward
```

**Nothing in that report drives an output**, and that is the point. A firmware
demo that wrote a pin would be writing an arbitrary pin on hardware whose
wiring it does not know — a short across a driver, or a relay clicking in a
cabinet nobody is watching. Asking the HAL to *resolve* a peripheral proves the
whole path and cannot damage anything. The ADC is read because reading one is
passive.

### Three things the port could not simply copy

**The font.** `rustclr-gfx` used `font8x8`. The glyphs are now a 1 KB table in
`crates/rustclr-gfx/src/font.rs`, generated from pixel art that a person can
read — and the test renders the table *back* to text, so a wrong byte shows up
as a misshapen letter rather than as a number nobody can check. Another test
proves all 94 printable glyphs are distinct, which is what catches a
copy-paste slip in a 128-entry table.

**Secure boot.** The reference verifies an RSA PKCS#1 v1.5 signature. RSA needs
bignum arithmetic, this workspace has no external dependencies, and a
hand-rolled bignum with a timing side channel is *worse* than no signature
check — it is a check that reports success. So the container verifies an
HMAC-SHA256 against a key provisioned at flash time. What that still gives:
nobody without the key can produce an image the device accepts. What it does
not: the verifying key is also a signing key, so extracting it from one device
lets an attacker sign for all of them. Both halves are in the crate docs and in
`docs/limitations.md`, because a MAC honestly described beats a signature that
is not really one.

**A lock for a chip with no atomics.** `rustclr-diag` shares a logger, which
needs mutual exclusion — and **thumbv6m and rv32imc have no compare-and-swap at
all**, so `AtomicBool::compare_exchange` does not compile there and
`alloc::sync::Arc` is configured out for the same reason. The first attempt was
a spin lock and would have hung any board where an interrupt handler logged
while the main loop held it. It is now a *try*-borrow that drops the record and
counts it. Losing a log line is a cost; a hung board is not one anybody would
accept to avoid it.

### The blocker, and one BCL gap it found

`Device.dll` references `RustNet.Device.dll` beside it, and the loader had
never followed an `AssemblyRef` — a `TypeRef` binds only to a type already in
the registry, so every `RustNet.Hal` token failed to resolve.
`load_from_file` now loads referenced assemblies from the directory first and
recurses. Framework names are skipped deliberately: loading a real
`System.Runtime.dll` from a self-contained publish would give the program a
second, conflicting `System.Object`.

The device fixture then needed `Encoding.UTF8.GetBytes`, which did not exist.
UTF-8 and ASCII now do, with the code page carried on the instance so the two
are genuinely different objects rather than one that behaves like whichever was
asked for last.

### A C# program can drive hardware

`RustNet.Hal.Gpio.Write(2, true)` now reaches a pin. The path is C# →
`RustNet.Device.dll` → a native binding → a `Board` trait object → the chip,
and every link of it is exercised by `tests/fixtures/Device`, which reports
**43 checks, 0 failures** against the simulator.

**The board is a module, not a wiring.** `requirements.md` asks for the
collector and the scheduler to be swappable; a peripheral bolted into the
interpreter would have been the one place that was not honoured. `Board` hangs
off the interpreter the way the host and the collector do, `crates/rustclr-hal`
is traits and nothing else, and `crates/rustclr-hal-host` is a simulator so
hardware code is testable on a desktop. `run --board host` installs it; an
unknown name lists what exists rather than falling back to the simulator,
because a program that quietly ran against a simulation when it meant to drive
a relay has done something worse than fail.

**No board is an error.** Every binding throws, naming what was missing and
what to do. Only `Board.Name` and `Board.IsPresent` answer without one.

**The blocker was assembly references.** `Device.dll` references
`RustNet.Device.dll` sitting beside it, and the loader had never followed an
`AssemblyRef` — a `TypeRef` binds only to a type already in the registry, so
every `RustNet.Hal` token failed to resolve. `load_from_file` now loads the
referenced assemblies from the directory first and recurses. Framework names
are skipped on purpose: loading a real `System.Runtime.dll` from a
self-contained publish would give the program a second, conflicting
`System.Object`.

**Storage is whole-file, and that is the shape rather than a subset.**
`RustNet.IO.Storage` over a log-structured store: writes append, newest wins,
deletes leave a tombstone. NOR flash clears bits and cannot set them, so a
seekable stream would mean read-erase-write of a whole sector per write — slow,
and a power-loss window over data that was previously fine.

### Crypto, written out rather than depended on

The reference implementation reaches for `sha2`, `hmac`, `aes`, `ctr` and
`rsa`. This workspace has no external dependencies, and that posture is why the
PE reader, the metadata reader, the portable-PDB reader and the debug adapter's
JSON are all here in full. So `crates/rustclr-crypto` writes out SHA-256,
HMAC-SHA256, AES-128/256-CTR and CRC-32 — and checks them against the vectors
their specifications publish:

| | |
| --- | --- |
| SHA-256 | FIPS 180-4, including the million-`a` case that catches a 32-bit length counter |
| AES | FIPS 197 Appendix B and C.3, both key sizes |
| AES-CTR | NIST SP 800-38A §F.5.1 |
| HMAC-SHA256 | RFC 4231, including the over-long key that must be hashed rather than truncated |
| CRC-32 | the `123456789` check value every implementation publishes |

All fourteen passed on the first run, which is the point: a published vector is
a stronger claim about correctness than a resolved dependency is.

**What is refused, and why.** No RSA and no elliptic curves — signature
verification needs bignum arithmetic, and a hand-rolled bignum with a side
channel is worse than no signature check, because it is a check that reports
success. No random number generator: entropy belongs to the chip and comes
through the HAL. AES is counter mode only, because ECB encrypts equal blocks to
equal ciphertext and offering it beside CTR would be offering a foot-gun with
the same shape as the safe option. A key that is not 16 or 32 bytes is refused
rather than padded.

**`Hmac.Verify` exists so nobody writes `computed == expected`.** That
comparison exits at the first differing byte, and how long it took says where
that byte was — enough to forge a tag one byte at a time. The binding compares
in constant time, and the managed doc comment says why the method is there.

### System.Text.Encoding

A real BCL gap, found by a fixture that needed `Encoding.UTF8.GetBytes`. UTF-8
and ASCII, with the code page carried on the instance so the two are genuinely
different objects rather than one that behaves like whichever was asked for
last. Nine Conformance checks, byte-identical to `dotnet` — **361 checks now**
— and mutation-checked: an encoding that ignored its code page fails two of
them.

ASCII substitutes `?` rather than throwing, and invalid UTF-8 decodes lossily
to U+FFFD, both matching .NET. The strict `throwOnInvalidBytes` form is not
offered rather than offered and quietly behaving like the lossy one.

### RISC-V 64 runs, on a Maix Go

A Sipeed Maix Go arrived on COM10 and settled two open questions at once.

**The interpreter runs on RV64GC.** `HelloWorld.Main` executes on the K210 and
prints the same three lines `dotnet` does, with the full RustBCL tier — 992
native bindings, 245 types, nothing trimmed, because 6 MB of SRAM against a
261 KB peak is the one board of seven with room to spare. The collector suite
passes beside it. Capture: [docs/logs/maixgo-k210.log](docs/logs/maixgo-k210.log).
That makes **three architectures** that have run the interpreter on hardware —
RISC-V 32, RISC-V 64 and Xtensa LX6 — from one source file.

**The HuskyLens was the HuskyLens.** The same firmware, built the same way, had
been silent on a Sipeed HuskyLens on 2026-08-21, and four causes had been ruled
out without reaching a verdict. The Maix Go is the second K210 that was needed:
the firmware is fine, and the failure is in that product's hand-off or its
`riscv-rt` startup.

**The board found a bug in five minutes.** The sixth report died with
`memory allocation of 73728 bytes failed` on a 1 MB heap the collector said held
nothing. Not a leak — **fragmentation**: each report builds the whole runtime
(260 KB of bindings, a 98 KB slot table) and tears it down, and after five rounds
a linked-list allocator has no contiguous 72 KB block left. The allocator is now
re-initialised before each report, which is sound because nothing that outlives
an iteration is on the heap — `run` drops everything it made, `Detail` is a fixed
96-byte array, `Uarths` is a unit struct. Verified the way it was found: **45
consecutive reports, no panic**, where the previous build died at six.

**How to do it again safely.** `kflash -p COM10 -b 1500000 -s fw.bin` boots into
SRAM and never touches the SPI flash — which matters on a commercial K210 module
whose flash holds the product's own firmware. Watch the console with **DTR and
RTS cleared**: on this board they drive ISP and reset, and asserting either
discards the SRAM image and boots whatever is on flash instead.

**The RISC-V code generator still has not run.** `crates/rustclr-jit` is
`std`-only — `cargo build -p rustclr-jit --target riscv64gc-unknown-none-elf`
fails with "can't find crate for `std`" and 463 more, because the crate has no
`no_std` attribute and therefore no prelude — and `codepage.rs` gets its
executable pages from `mmap`/`VirtualAlloc`, neither of which exists on bare
metal. The backend itself is the small part: `riscv64.rs` emits RV64I/M and its
tests assert the exact bytes. What stands between it and this board is a
`no_std` port of the crate around it and a `CodePage` that is a static aligned
buffer plus a `fence.i`. The K210 makes that worth doing — it is the only
RV64 board here, and the ESP32-C3 is RV32, which this backend does not target.

### The profiler samples itself

`rustnet profile` reports where a program's time went, by method, and what it
allocated, by type. Self time and inclusive time separately — the two orders
rarely agree, and the difference is usually the answer — with exact call counts
beside them.

**There is no sampling thread.** Reading another thread's frame stack means
stopping it first, and the only machinery for that is the collector's. Instead
the interpreter counts down 512 instructions, reads the clock itself, and walks
its own frames. That means no synchronisation on the path that runs every
instruction, a stack that is never caught mid-update, and a profiler that works
on a microcontroller, where there is no second thread to spare and the clock is
a hardware timer.

**Measured elapsed time is charged, not one tick per sample.**
`Thread::sleep(1ms)` on Windows is anywhere from 1 to 15 ms depending on what
timer resolution the process happens to have. A profiler that assumed its
period was honoured would report a program as fifteen times faster than it was;
charging what the clock says removes the question, and a unit test fires a
sample 50× late and asserts the whole gap is charged.

**Calls are counted, not sampled.** `fib(20)` reports exactly 21,891. A call is
rare enough that an increment on it distorts nothing, and "called four million
times" is often the whole answer. The tables are `Vec`s indexed by `MethodId`
rather than maps keyed by it — a profiler that hashed on the call path would be
measuring itself.

**Arrays are charged to their element type.** The first run reported
`System.Array 45.5%`, which is not something anybody can act on: `new int[n]`
needs no `TypeSpec` row, so every runtime-made array fell back to the base type.
A second table indexed by *element* type turned that into `System.Int32[] 43.5%`.

**The hook costs nothing when nothing is attached** — one `Option::is_some` per
instruction, cold and never inlined, the same shape as the debugger's.
Interleaved best-of-nine against a build with the check removed: `fields`
2,478.8 ms against 2,480.3 median, `fib` 446.3 against 433.4. Within the spread.

**A test found an allocation site that was invisible.** The totals have to add
up — what the profiler charges to types must equal what the heap accounted for —
and on `HelloWorld` they did not. `await`'s state-machine cell went straight to
`heap.alloc`, skipping both the allocation counter and the attribution. It now
goes through `Interpreter::alloc_clr_object`, and the test says so byte for
byte. On a *threaded* program the totals legitimately differ, because a worker
thread has no profiler; that test asserts >99% instead, which is exactly the
gap a threaded fixture leaves and far smaller than a missing site would.

**Seven Conformance checks, all invariants.** How many bytes a runtime allocates
for the same program is its own business, so nothing pins a number: what both
runtimes must agree on is that `GC.GetTotalAllocatedBytes` never decreases and
does move when the program allocates, that `GC.Collect()` shows up in
`GC.CollectionCount`, and that cumulative allocation outgrows live bytes once
garbage has been dropped. Mutation-checked: returning live bytes under the
allocated name fails two of them, and returning a constant fails another.

### VSCode can attach

`rustnet debug --dap` is a Debug Adapter Protocol server: JSON over stdin and
stdout, framed with `Content-Length:` headers exactly as LSP is. An editor sets
breakpoints by source line, sees the call stack with file and line, reads
arguments and locals by name, and steps.

**The runtime's hook was already the right shape.** `Debugger::stopped` is
called with the program stopped and does not return until it says what to do
next, which is exactly what an adapter wants: send `stopped`, then keep
answering `stackTrace`, `scopes` and `variables` until the editor sends a step
or a continue, and return that. The consequence is stated in the docs rather
than left to be discovered — while the program *runs*, no request is serviced,
so an editor's pause button does nothing until the next breakpoint.

**The JSON is hand-written.** This workspace has no external dependencies, and
DAP messages are flat objects of strings, numbers and short arrays. Pulling in
`serde_json` to carry them would have been a larger change to the project than
the feature it serves. 4 unit tests, including one that checks malformed input
is refused rather than guessed at.

**stdout had to be taken away from the program.** A `Console.WriteLine` written
straight to stdout lands *inside* a framed message and the editor drops the
connection. The debuggee gets a host whose writes become `output` events — where
an editor shows them anyway, and live rather than at the next stop — sharing one
locked writer with the adapter so a message is never interleaved. `Console.ReadLine`
answers `null`: stdin carries the editor's requests, and handing the program a
line of protocol would be worse than telling it there is no console.

**Every thread's console, not just the first.** `Thread.Start` and the task
pool each built a `SystemHost` outright, so a worker thread's output went to the
real stdout however the main thread had been redirected — silent corruption of
the protocol stream for any program that prints from a thread. The interpreter
now carries a *host factory* the embedder sets, `worker()` copies it, and every
place that starts a managed thread goes through it. A test in
`crates/rustclr-core/tests/parallel.rs` fails if a spawned thread writes
anywhere else.

**The tests drive a real process.** `crates/rustnet-cli/tests/dap.rs` spawns
`rustnet debug --dap`, speaks framed JSON to it over a pipe, and asserts on what
comes back. Each was mutation-checked: reporting every breakpoint `verified`
fails the unverified-line test, an off-by-one in the sequence-point search fails
the stack-line test, and writing the program's console to real stdout fails the
output-event test by corrupting the stream the test reads.

### Source-line breakpoints, from a real PDB

`crates/rustclr-metadata/src/pdb.rs` reads the portable PDB the .NET SDK writes
beside an assembly. It is not a separate format so much as a separate *file* in
the one already parsed — the same `BSJB` header, with tables 0x30–0x37 carrying
documents, sequence points and local names.

`rustnet debug --break Program.cs:2189` now resolves to
`Conformance.Program::Triple(int) at IL_0000`, stops there, and prints every
frame as `file:line`. Locals render by name. A line with **no** code is refused
(`no code on Program.cs:1`) rather than moved to a nearby line that has some,
and a line is resolved to *every* place it has code, because a lambda and its
enclosing method share lines.

**Two bugs the fixture found.** The absolute line and column belong to the first
*non-hidden* sequence point, not the first record — reading them from the first
record produced a `negative sequence point` error on the first real PDB tried.
And my own test asserted that `checks` and `failures` were locals of `Main`;
they are static fields, so the test was wrong and the reader was right.

### Recursion compiles

`sort` went from 2,757 ms to **236 ms** and `fib` from 460 ms to **75 ms**.
Fifteen methods in the benchmark suite compile now, and `alloc`, `virtual` and
`fields` are the only rows still dominated by interpretation.

Recursion is the one call shape the inliner cannot remove — splicing a method
into itself does not terminate — so it is the one that needed a calling
convention. The arguments go into an outgoing frame the prologue reserves, and
the callee is entered exactly as the tier enters the method: same
argument-array shape, same trap slot, same allocation context. The call target
is the first byte of the method's own code, so the displacement is known at emit
time and needs no fixup.

Only *self*-calls, at the time: a call to another method needs an address that
may not exist when the caller is compiled. **Superseded** — calls between
compiled methods go through a table the tier fills in before the caller runs,
and `Sort` compiles.

**Compiled recursion runs on the machine stack**, which cannot be grown or
unwound the way the interpreter's frame budget can — so overrunning it would be
a crash rather than an exception, and this runtime does not accept that anywhere
else. Compiled code counts its own depth and traps past a limit. A 200,000-deep
recursion reports the same catchable exception interpreted and compiled; .NET
stack-overflows and dies.

**Two bugs, and the second was caught by a test I had already broken.**

The verifier rejected `QuickSort` with "reached with stack depth 3, but 0 on
another path". That was the verifier being right: the depth walk treated a
`call` as moving the stack by zero, when it pops its arguments and may push a
result. The method was fine; the analysis was not.

The second was mine from an hour earlier. Two gate functions share an identical
`.all(is_supported(...))` line, and a one-shot replace had put an earlier edit
in the wrong one — so the *no-inlining* screen started waving calls through. The
differential test said exactly that: "Blend contains calls, so without inlining
the backend must decline it". I had noticed the mis-targeted edit at the time,
fixed the intended function, and left the accidental one in place.

### A method that loops compiles on its first call

`sieve` went from 1,439 ms to **60 ms** and `matrix` from 1,731 ms to **84 ms**,
with no flag and no change to either benchmark. `noop` is unchanged at 70 ms, so
this cost nothing at startup.

A call count is a proxy for "worth compiling", and it is simply the wrong proxy
for a method called *once* that runs for seconds. `Sieve` and `Matrix` are
exactly that shape: they had been compilable for a while, and the previous
entries had to say "measured with `--jit-threshold 1`" because at the default
threshold they never compiled at all. The signal that was missing is in the body
— a backward branch — and it is available before the call is made.

So: a method whose body loops compiles on its first call; a loop-free method
still waits for the count, because compiling one that is called once is pure
loss. Compilation costs microseconds against the seconds a loop can run.

**This is not on-stack replacement.** The decision happens *before* the call, so
a method already running cannot be swapped for a compiled one. A single call
that enters a long loop still runs interpreted if the decision went the other
way — which is why the heuristic is deliberately eager rather than clever.

**Both rows are now at the process-start floor**, and that is worth saying
plainly: `noop` is 70 ms, so most of what is left in `sieve` and `matrix` is
starting the process, and their ratios against .NET are dominated by RustCLR's
faster startup rather than by faster arithmetic. Reading them as "twice as fast
as .NET at numerics" would be wrong.

### bool[] compiles, and sieve joins matrix

`bool[]` elements — one byte, zero-extended on the way in — compile, so `sieve`
does too: **20.3×**. Thirteen methods in the benchmark suite compile now.

The element work was small; the gates were the obstacle. Three separate screens
all rejected an array-typed *local*, each for the same reason, and that reason
had expired: a local declared `int[]` used to be ambiguous between a descriptor
the tier passed in and the result of a `newarr` the backend could not emit.
Both are descriptor addresses now. Widening one screen and not the others left
`sieve` declined with no visible change, twice.

The CLI's explanation was stale in the same way — it still said "local 0 is not
an integer" for a method the backend would now take. A stale explanation is
worse than none: it sends a reader looking for the wrong thing, which is exactly
what it did to me here.

**The benchmark suite still shows the interpreted times for both.** `Sieve` and
`Matrix` are each called once, so at the default tiering threshold they never
compile; the 20× and 21× figures come from `--jit-threshold 1`. I have not
changed the threshold or the benchmark to make the table look better.

### Compiled code can allocate, and matrix is 21× faster

`new int[n]` and `new double[n]` inside a compiled method work. `matrix` — the
benchmark that has been the motivating example for two sessions — compiles at
last: **1,757 ms interpreted against 83 ms, 21.2×**, same checksum.

**The reason it was declined was never "allocation is hard".** It was that
compiled code reaches no safe point, so a collection during the call would have
to walk a frame the collector cannot read, and would free the arrays whose raw
pointers compiled code holds. Three facts dissolve that, and the design needs
all three:

`Heap::alloc` does not collect — it allocates and records pressure, and the
*interpreter* collects between instructions. Compiled code has no
between-instructions, so allocating from it cannot trigger a collection at all.

A compiled method cannot return a reference, because its return type must be an
integer or a float. So every array it allocates is unreachable the moment the
call ends: the sink owns them for the call and drops them after, and nothing
needs rooting.

And a `newarr` inside a loop is **declined**. Without that, a compiled loop
allocating would exhaust a heap the interpreter would have collected — a program
that runs interpreted and fails compiled, which is the one difference this
runtime will not accept.

**The ABI turned out to be the easy part.** `RBX`, `R14` and `R15` — the
argument pointer and the two cached stack slots — are callee-saved in both the
Windows and System V conventions, so a C call preserves them; the temps are
volatile and nothing is live across an instruction boundary anyway. The
allocation context rides in the argument array, one past the trap slot, so
nothing about how compiled code is *entered* had to change.

**Two bugs.** `find_sz_array` returns the constructions something has already
named, and `System.Double[]` often has not been, so every allocation failed
until the base `System.Array` stood in — the same fallback the interpreter
already uses.

The second was not mine and was worth catching: the full workspace test run
failed intermittently with "this task never completed", while passing in
isolation every time. `settle` reads counters without a lock, and a job taken
off the queue but not yet finished leaves a window where they say idle. Treating
one sample as final turned a slow task into a spurious error. It now wants the
idle reading to persist. Three consecutive clean workspace runs afterwards.

### double[] compiles

Element access on a `double[]` that arrives as a parameter compiles: a dot
product over 512 elements is **37× faster** than interpreted, same answer.
Conformance 294 → 299 checks.

Most of it already existed. The descriptor the tier builds for an `int[]` —
pointer and length — is the same shape for a `double[]`, and the assembler
already had scaled-index moves with a scale parameter, sitting unused behind a
dead-code warning. An eight-byte element is in fact *simpler* than a four-byte
one: `int[]` has to sign-extend 32 bits to 64 or a negative element comes back
as a huge positive number, and a `double` is already the width of a slot, so the
load is a plain move of the bit pattern.

**`matrix` still does not compile**, and it is worth being exact about why,
because "double arrays are supported" would imply otherwise. It allocates its
arrays as locals — `double[] a = new double[n * n]` — and allocation is
disqualifying whatever the element type: compiled code runs with no safepoint,
so a collection must not be able to happen inside it. An array passed in is
fine; one created inside the method is not. The 37× figure comes from a shape
that does compile, not from the benchmark.

I did not restructure the benchmark to make the number look better. It measures
what it measures.

### The code generator compiles floating point

`double` arithmetic, comparison, negation and conversion compile to SSE2.
Conformance is 289 → 294 checks, identical interpreted and compiled.

**The problem was type inference, not code generation.** IL's `add` is `add`
whether its operands are integers or doubles — the difference is in the types
that reached it, which an interpreter can see in the values it holds and a
compiler cannot. `floats::infer` walks the stream keeping an abstract stack of
kinds and records which instructions are floating-point, declining the method
when it cannot follow rather than picking a kind.

Doubles needed no register class of their own: a slot is eight bytes and so is
a `double`, so they travel as bit patterns through the same slots and
general-purpose registers, and only the arithmetic moves them into xmm and back.

**Three bugs, and the shape of all three is the same.**

The differential run passed with **zero methods compiled** — an `accepts` screen
rejected float signatures before `compile` was ever reached, so identical output
proved only that the interpreter agrees with itself. Checking the compile count
is what caught it. That is the second time this session a test passed while
measuring nothing.

Then the numbers came back wrong in a very particular way: `ConstOnly()`
returning 1.0 gave 4.6071824188000174E+18, which is *exactly the bit pattern of
1.0*. The compiled code was right; the tier's `widen` had no float case, so the
correct bits came back as `Value::I64`. The round trip is now a test in its own
right, because getting one direction right and not the other is precisely how
this looked.

And `(int)3.5` gave 0. `conv.i4` on a double was narrowing the bit pattern
instead of converting first — the low 32 bits of 3.5's representation are zero.
Only the conformance check found that one, which is what conformance checks are
for.

**`matrix` is not faster, and that was measured rather than assumed.** It uses
`double[]`, and array support covered `int[]` only, so the method was still
declined — 1,773 ms interpreted against 1,734 ms compiled is noise. `double[]`
landed next; `matrix` still does not compile, for a different reason given
below.

### The pool grows, and a sleeping thread stops stalling collections

Thirty-two tasks that each sleep 400 ms: **1,610 ms before, 469 ms after** —
four rounds against roughly one. A worker that blocks tells the pool it is gone
for the duration, and if that leaves queued work with nobody to run it, another
worker starts, up to a ceiling. Unbounded growth would turn a program that
blocks a lot into one that runs out of threads.

**It began as a collector bug, and that is the part worth keeping.** The growth
code was in place and never fired, because `Thread.Sleep` slept without
announcing itself blocked. That is not only a scheduling matter: a sleeping
thread reaches no safe point, so a collection waited out the entire sleep for
it, and several sleeping threads stalled every collection. One `blocking`
wrapper fixed the collector and the pool at once.

Worth noting what the first version of the test proved, which was nothing: 32
tasks sleeping 200 ms on 8 workers is four rounds of 200 ms, comfortably inside
the 1,200 ms bound it checked. Doubling the sleep made the two outcomes 1,600 ms
and 400 ms, which a threshold can actually tell apart.

### Monitor.Wait and Pulse

A producer/consumer queue on `lock`/`Wait`/`Pulse` runs; a conformance check
moves 500 items through one, with the consumer made to reach `Wait` on an empty
queue before anything is produced, so the answer depends on it being woken
rather than on timing.

`Wait` releases the monitor at whatever depth it was held and retakes it at that
depth. That detail is the whole of it: `lock` is re-entrant, so a method that
locked three deep and then waits must come back three deep, or its own `finally`
blocks release a lock it no longer holds.

`Wait(obj, ms)` honours its timeout and reports whether it was pulsed. It comes
back holding the monitor either way, which is the part worth being careful
about: `Wait` is defined to return holding it, and a timeout does not change
that — a caller whose `finally` released a lock it never retook would corrupt
the count for every other thread.

The bug there was mine and was silent: `Monitor.Wait` is *static*, so the object
is argument zero and the timeout argument one — two in total. Testing for three
meant the timeout was never read and every timed wait was infinite. It hung
rather than answering wrongly, which is the better failure of the two.

`Pulse` wakes every waiter rather than one, and that is deliberate rather than a
shortcut. It is a *weaker* promise than the API makes, and a `Wait` written in
the re-check loop the API requires already tolerates it, because a spurious
wake-up is always possible. Waking exactly one and picking the wrong one would
not be tolerable. The pulse is a counter rather than a flag, so a waiter that
had not yet reached the wait when the pulse arrived does not miss it.

### A thread pool, and `rustclr-sched` finally earns its keep

`Task.Run` and `Parallel.*` queue onto one worker per core instead of taking a
thread each. Two thousand tasks is a conformance check; it used to mean two
thousand OS threads, each paying for a copy of the loader on the way up.

**The workers hold interpreters**, which is the whole reason the pool could not
be `rustclr-sched`'s as it stood. A pool of plain closures would have to build
an interpreter per job — and copying a loader is the expensive part of starting
a managed thread, not the thread. So each worker builds one at startup and
reuses it, and a job is `FnOnce(&mut Interpreter)`. The lock-free queue
underneath *is* `rustclr-sched`'s, tested since the scheduler milestone and
until now unused by anything.

**A waiting thread helps.** A thread blocked on a task runs queued jobs rather
than idling. Without that the pool deadlocks in the familiar way — every worker
blocked on a task whose job is still queued behind it — and `Task.Run(async () =>
await Task.Run(…))` is exactly that shape. It is also why the pool never needs
to grow under load.

`Task.Delay` arms a timer and returns a pending task, so a delay overlaps the
work after it. One timer thread serves every outstanding delay, and it owns no
interpreter: when a deadline passes it queues the completion like any other job,
because a continuation is managed code and belongs on a worker.

**The bug this turned up was not about threads at all.** A non-generic
`TaskAwaiter.GetResult()` returns void, and this runtime was handing back a
value for it — which leaves that value on the evaluation stack, so everything
the caller reads afterwards is off by one. It surfaced as a null reference two
statements later, on a `Stopwatch` that was fine. It had been wrong the whole
time and stayed invisible while `Task.Delay` returned an already-completed task
that nothing awaited.

### await suspends

An `async` method that awaits a pending task returns to its caller, and the
thread completing the task resumes it. Two tasks started and then awaited
overlap; awaiting in a loop stays sequential, as it is on .NET.

**Almost none of this was new code, and one line of it was mine to undo.** The
continuation machinery had been right all along: `AwaitUnsafeOnCompleted` copies
the state machine to the heap — which is what makes a suspension a suspension,
since it has to outlive the frame it was a local of — and queues it on the task.
What blocked `await` was that I had made the *awaiter's* `IsCompleted` wait and
answer `true`, back when `Task.Run` ran inline and a pending task meant a bug.
With `Task.Run` on a real thread, answering honestly sends the state machine
down the path that was already there.

**Two things did need building.** Waiting on a task nobody owns: the task an
`async` method returns has no thread of its own — it is completed by whichever
thread finishes the *inner* task — so `Result` cannot join anything. It waits
while another mutator is registered and reports a clear error when none is,
which is the difference between waiting and hanging.

And a race worth naming: `await_on_completed` checked whether the task was
pending and *then* joined the queue. A task completing between those two steps
drained a queue the continuation had not yet entered, and the `await` would
never resume. Settling a task and taking its waiting list now happen under the
same gate as checking and queueing. Resuming runs managed code, so it happens
after the gate is released, never under it.

`Stopwatch` had to become a real object on the way past: it was the number it
started at, which reads `ElapsedMilliseconds` correctly and cannot express
`Restart`, because `this` for a class arrives by value and there is nothing to
write back to.

### Task.Run and Parallel.* run on threads too

`Task.Run` starts its delegate on another thread; `Parallel.For`, `ForEach` and
`Invoke` split their work one thread per core, capped by the iteration count.
Two 120 ms tasks finish in 123 ms rather than 240.

Both fell out of the thread work rather than needing anything new: `spawn` and
`join` were already there, and a parallel loop is a partition plus a join.
`Task.Run` needed one more idea — the task carries the id of the thread running
it, and every way of *observing* a task settles it first. That is `Result`,
`Wait`, `WaitAll`, and an awaiter's `IsCompleted`, which is the gate `await`
branches on: answering "not yet" would send the state machine down the
resume-inline path before the body had finished.

**`WaitAll` was waiting for nothing.** It read a params array, and .NET 10 does
not pass one — `Task.WaitAll(a, b)` lowers through an `InlineArray2<Task>` and a
`ReadOnlySpan<Task>`. Reading only arrays meant it found no tasks and returned
at once. It reads a span now.

**A timing check is the wrong test, and the fixture caught me writing one.**
`elapsed < 200 ms` for two 120 ms tasks passed standalone and failed inside the
conformance run, where the machine is busy — a margin generous enough to be
reliable is too generous to prove anything. It is a rendezvous now: each task
announces itself and waits for the other, with a bounded wait so a runtime that
serialises them reports `false` rather than hanging. Whether two things
overlapped is not a question about how long they took.

**`Environment.CurrentManagedThreadId` returned 1 for every thread.** True while
threads were serialised, and a silent wrong answer once they were not — an
iterator's state machine uses it to notice cross-thread enumeration, so every
thread sharing an id would have had them sharing one state machine.

### Threads run at the same time

`Thread.Start` spawns an OS thread and `Join` waits for it. Four threads
incrementing one static reach four thousand, `lock` excludes, `Interlocked` does
not lose updates, and a consumer can block on a flag set by a producer started
*afterwards* — the case that hangs forever under serialisation. All four are
conformance checks, because each has an answer that differs if the threads do
not really overlap, and the fixture ran identical to `dotnet` twelve times in a
row.

**The design is that the loader is copied, not shared.** That is the thing that
made this tractable, and the earlier conclusion — that it needed either unsafe
concurrent data structures or a redesign of stub synthesis — was wrong for a
reason worth writing down. `registry.ty()` is called on nearly every instruction
and hands back a reference, so a lock underneath it would cost more than the
heap's did. But a `Loader` is *finished* before the first instruction runs, and
an identical copy behaves exactly like a shared immutable one, at no cost and
with no borrow problem at all. Only the genuinely mutable part — static field
storage — is shared, behind a lock.

The correctness condition is precise and checkable: the copies must not diverge.
A loader grows in two places after load, an interface stub and
`MakeGenericType`, and a `MethodId` minted on one thread names nothing on
another. A thread that grew its registry is caught on `Join` and reported.

**Three bugs, and the second is the one that matters.**

`lock` was a no-op, which was harmless while nothing overlapped and is a data
race the moment something does. `lock (gate) counter++` on two threads gave
1,863 of 2,000 — both threads reached the same static, which was the point, and
neither excluded the other, which was not. `Interlocked` had the same problem
one layer down: read, add and write are three separate lock acquisitions, so two
threads interleave between them. It reached 18,828 of 20,000, and only after
`Increment` was gated as well as `Add` — gating one and not the other is exactly
the kind of half-fix that looks finished.

**The GC bug is the one to remember.** A thread waiting in `Join` announces
itself blocked so the collector does not wait for it, and the first version
contributed *none of its roots* while it was away. "Blocked threads hold no
references into the heap" is true of a thread parked at a safe point and false
of one sitting in `Join` holding the array of threads it is joining. A
collection swept it, and `new Thread[4]` came back with length zero. Blocked
threads hand their roots over on the way out now, and there is a test.

That bug was invisible until threads were real *and* allocated enough to force a
collection. The earlier multi-threaded tests passed because nothing was live
across the wait.

### Every probe passes, and that is not the same as parallelism

`Task.WaitAll(a, b)` written in C# runs, and the advanced-feature matrix is
**21 of 21**.

The last failure was never about the TPL. `WaitAll(a, b)` does not call a params
array on .NET 10: it fills a `System.Runtime.CompilerServices.InlineArray2<Task>`
and makes a `ReadOnlySpan<Task>` over it. That was recorded as "the lowering is
what this runtime cannot follow" — true when it was written, and the spans and
raw pointers built since had already removed most of it.

What was left was three small things. `InlineArray<N><T>` is a *framework* type
on .NET 10, not compiler-generated, so it had to be registered — a struct of N
slots, and the arity is in the name. `Unsafe.As` is the identity on a managed
reference, because a reference here is a path to a slot and reinterpreting it
changes only the type it is read at. `Unsafe.Add` walks that path: to the next
array element, the next struct field, or — when the reference names something
whose fields *are* the elements, which is exactly an inline array — to its nth
slot. The pointer kind and `ByRef::StructField` already existed; nothing new was
needed to express any of it.

`MemoryMarshal.CreateReadOnlySpan` is the one honest compromise, and it is
written down where it is done: it copies the elements out rather than making a
view onto the caller's storage. The callers that reach it build the buffer
immediately before and never touch it again, and a `ReadOnlySpan` cannot be
written through, so nothing can observe the difference — but anything needing a
genuine view over a struct's fields is not served by it.

**21 of 21 is a statement about output, not capability.** `tpl` passes because
its answers are order-independent: `Task.Run` runs its delegate where it is
created and `Parallel.For` iterates in order, and the sums come out the same. A
probe measuring wall-clock overlap would fail. Three rows of the matrix are
marked with a warning for exactly this reason, and the caveat now sits above the
table rather than below it, so a reader meets it before the number.

### The call site knows what the type does not

Two probe failures — a span over `stackalloc`, and `Memory<T>` — both came down
to one missing fact, and the fact was never actually missing. The matrix
reached **20 of 21**.

A `Span<int>` over `stackalloc` memory needs to know that an element is four
bytes. The buffer cannot say: `localloc` allocates bytes. The type cannot say
either, because framework generics are erased here — `List<int>` and
`List<string>` are deliberately one runtime type, so that native bindings stay
reachable by declaring-type name. That was recorded as the reason the feature
could not be built.

It was the wrong conclusion. Erasure applies to the *runtime type*; the call
site still spells the arguments out, and the loader was already parsing that
`TypeSpec` to build a name. `new Span<int>(ptr, 4)` says `int` right there.
Recording the resolved arguments of every member reference — framework generics
included, which `member_ref_owner` deliberately skipped — and staging them for
the duration of one native call was the whole change.

That is a general capability, not a span fix. Any native implementation on a
framework generic can now ask what the call site named.

**A span is three shapes now**, and every accessor reads all three: a window
onto an array where start counts elements, a window onto raw bytes where start
counts bytes and the window carries the element width, and a string that *is*
the span. Indexing raw memory yields a raw pointer rather than a managed
reference — and the `ldind`/`stind` that follows already knows its own width
from the instruction, so nothing had to be threaded through.

`Memory<T>` came free: it is the same window. The difference in .NET is that a
span may not be stored in a field or held across an `await`, and Roslyn has
enforced that before any of this runs.

### Marshalling blittable structs

`Marshal.SizeOf<T>()`, `AllocHGlobal`, `StructureToPtr` and `PtrToStructure<T>`
work for a struct of primitive fields, widths intact — the matrix reached
**19 of 21**.

Nothing here was hard, and that is the point: both halves of marshalling had
been blocked on things finished earlier in this session. Turning a value into
bytes needs somewhere to put them, which the raw pointer gave; `SizeOf<T>()`
needs to know what `T` is, which generic methods already record on their
instantiation, so the native implementation reads it back rather than being
handed it. Two features that were built for other reasons met.

**`AllocHGlobal` allocates on the managed heap**, so `FreeHGlobal` does nothing
and the collector reclaims the buffer when the last pointer to it is gone. A
program that frees correctly cannot tell; one that uses memory after freeing it
finds it still valid here and crashes on .NET, which is the safe direction to
differ in. The pointer cannot go to native code — that needs a real address —
and `docs/limitations.md` says so rather than leaving it to be discovered.

A struct with a reference field is refused by name. Its field is a handle into
the GC's table, and writing those bits would produce a number that looks like a
pointer and is not one.

### Raw pointers, and `unsafe` C# runs

`stackalloc`, `fixed`, pointer arithmetic, comparison and dereference all work,
along with `cpblk` and `initblk`. The advanced-feature matrix went 16 → 18 of
21; `unsafe-fixed` and `unsafe-stackalloc` both pass.

**A pointer is not an address.** It is a buffer on the managed heap plus a byte
offset, which is enough because C# only ever gets one from `stackalloc` — a
byte range — or from `fixed`, which pins an array. Both name memory this
runtime already owns. Arithmetic moves the offset, so nothing can point outside
a buffer the runtime knows about, and the pointer *roots* that buffer, so
`stackalloc` memory outlives every reference to it instead of dying with a
frame. That is a stronger guarantee than the native stack would give, and
nothing observable depends on the difference.

It also explains why this is a separate kind from `ByRef` rather than a widening
of it. A `ByRef` is a *path* — to a local, a field, an element — and has no byte
offset. `conv.u` on a `ByRef::ArrayElement` produces a pointer, which is exactly
what `fixed` compiles to; `conv.u` on any other `ByRef` still refuses.

**Three bugs, and the second is the one worth remembering.**

Pointer *comparison* was missing, so `for (int* p = start; p < start + n; p++)`
read both sides as zero and the loop body never ran. The probe reported 0 where
`dotnet` said 10 — a wrong answer, not a refusal.

The access width was being read from the *buffer* rather than the instruction.
`int* p` and `byte* q` are the same value here, and `*p` versus `*q` is the
difference between `ldind.i4` and `ldind.u1`, so every `stackalloc int[]` write
was truncated to one byte. `unsafe-stackalloc` passed anyway, because its
values are 7, 8 and 9 and all three fit. Writing 300 and 70000 gave back zero.
A probe passing is not the same as a thing working.

The third: `p[0] + ","` does not emit `ldind.i4` at all — it calls
`Int32::ToString()` with the *pointer* as `this`. A value type's receiver
arriving as a raw pointer had no unwrapping path, so the method read the pointer
itself and every such value rendered as zero. The width for that one comes from
the declaring type, which is the only place it is written down.

### Spans work over arrays

`Span<T>` and `ReadOnlySpan<T>` over an array: `Length`, `IsEmpty`, indexing,
`Slice`, `CopyTo`, `ToArray`, the implicit conversion from `T[]`, and both
collection-expression forms. The advanced-feature matrix went 14 → 16 of 21.

**The reframing is the useful part.** A span was recorded as unimplementable
because it is a generic ref struct. It is not the ref-struct-ness that was the
obstacle — a span is a window: something to look at, an offset, and a length,
and all three are representable whenever the thing being looked at is a managed
array. Only *raw* memory has no representation here, because a managed pointer
in this runtime is a path to a slot rather than an address. So `stackalloc`
still refuses and everything else does not, which is a much narrower line than
the one that was written down.

Indexing returns `ByRef::ArrayElement` — a reference to the element, which the
caller loads or stores through. That is what makes `span[1] = 20` visible in
the array behind it, and it needed no new machinery: the pointer kind already
existed.

**A span is now one of two shapes**, and both had to keep working. Over an array
it is a window object with three slots; over a string it *is* the string, which
is the older representation and the one ordinary C# depends on — `string + char`
lowers through `ReadOnlySpan<char>` on .NET 10. Every accessor reads both.

**The blob length was the real trap.** `ReadOnlySpan<char> x = ['a', 'b']` puts
its characters in the image and calls `CreateSpan`, and metadata does not record
an RVA field's length — `InitializeArray` gets away with that because the array
it fills bounds the copy. The size is in the name of the synthetic type Roslyn
emits per distinct size, `__StaticArrayInitTypeSize=4_Align=2`, and taking the
digits after the *last* `=` reads the alignment instead: a two-character span
came back with a length of one. Bounded by nothing at all it was 276.

### await foreach over an async iterator runs

`async IAsyncEnumerable<int>` with `await foreach` produces the same answer as
`dotnet`, including `yield break`, an empty sequence, and breaking out early.
That completes the async surface: what is still missing is overlap, not syntax.

The compiler lowers an async iterator into a state machine that is its own
`IAsyncEnumerable<T>`, `IAsyncEnumerator<T>` *and* `IValueTaskSource<bool>` —
all IL this runtime already executes. Two pieces were the runtime's:
`AsyncIteratorMethodBuilder`, and `ManualResetValueTaskSourceCore<T>`, the
promise `MoveNextAsync` hands back. Both are simple here for the same reason the
rest of `async` is: by the time `MoveNextAsync` returns, the body has already
run to the next `yield return`, so the promise is always settled and the
"reset, await, complete later" cycle it exists to manage never happens.

**Three bugs, each found by running it.**

`start_state_machine` passed the *pointer* to the state machine. That is right
for an async method, whose state machine is a struct, and wrong for an async
iterator, whose state machine is a class because it has to outlive the call —
there `this` is the object. Passing a pointer-to-local made every `ldfld` in the
body read from the wrong thing, surfacing as a null receiver two frames away in
`DisposeAsync`.

`ValueTask<T>`'s two-argument constructor takes an `IValueTaskSource<T>`, and
was wrapping the *source object itself* as the result. It asks the source now
instead — sound only because nothing overlaps, which is written down where it
is done rather than left to be inferred.

The third ended every `await foreach` at the closing brace. Once an enumeration
finishes, `DisposeAsync` returns `default(ValueTask)` — all zeroes, so null
here — and the shared awaiter bindings refuse a null receiver, which is correct
for a `TaskAwaiter` and wrong for this one. The `ValueTask` awaiters now read
null as completed, and are registered *after* the shared loop so they win.

### await using and ValueTask run

`await using` works, `IAsyncDisposable` with it, and `ValueTask`/`ValueTask<T>`
underneath — the advanced-feature matrix is 13 → 14 of 21.

`AsyncValueTaskMethodBuilder` had been registered all along; what was missing
was `ValueTask` itself and its awaiters. A `ValueTask` is represented by the
task it stands for, which loses the one thing the type exists for — avoiding an
allocation when a method completes synchronously — and keeps everything a
program can observe. `default(ValueTask)` is the case worth naming: a struct's
default is all zeroes, which arrives here as null, and in .NET it means an
*already completed* task rather than an absent one, so null reads as completed.

**Two bugs, both found by running it rather than reasoning about it.**

`ConfiguredValueTaskAwaitable` was in the framework type table but
`ValueTaskAwaiter` was not, so `await using` reached `get_IsCompleted` on a type
the registry did not have and the member reference would not resolve. The
binding existed; the type did not. Same shape as the `ParameterInfo` bug earlier
in this project.

The second was subtler and the trace found it, not the reading. The constructor
returned the constructed value, which is what `newobj` on a value type takes —
and `ValueTask<int> v = new ValueTask<int>(11)` does not compile to `newobj` at
all. Roslyn emits `ldloca v; ldc.i4 11; call .ctor`, so the return value went
nowhere and the local stayed null, which then read as `default(ValueTask)` —
*completed*, with no result. `IsCompleted` passed and `Result` returned zero,
which is exactly the plausible-looking wrong answer that is worth catching.
Writing through the `this` pointer serves both shapes, since `newobj` reads the
same slot back out of its cell.

### A static method on a generic type knows its type argument

`Tally<int>.ArgumentName()` returns `"Int32"`. This was the last generic-type
gap, described as needing something the frame did not have: no receiver exists
in a static method, and the body is shared by every construction.

It needed nothing new. The call site is a `MemberRef` whose owner is the
*construction*, and the loader had been recording that since `newobj` on a
constructed generic needed it — the information was already there, one frame
away. `do_call` reads it before entering and sets it on the new frame;
`frame_generic_argument` uses it for `!N` when there is no receiver, and still
refuses when a call site genuinely names the open definition.

Verified by disabling it: without the change the fixture stops with
``NotSupportedException ... at Conformance.Tally`1.ArgumentName``, which is the
proof the six new checks are load-bearing rather than decorative.

### Generic types can be built at run time, and the fixture caught a wrong answer

`MakeGenericType` works. It was recorded as blocked by erasure, and the
per-construction generic types built earlier in this session removed the block
without that being noticed — a closed construction is already a real runtime
type with its own identity and static storage, so `MakeGenericType` only had to
call the loader path a `TypeSpec` already calls, cache included. That sharing is
the point: `typeof(Cell<>).MakeGenericType(typeof(int))` returns the *identical
instance* as `typeof(Cell<int>)`, so reference equality on types stays reliable,
and `Activator.CreateInstance` on a type built at run time gives an object whose
methods run normally. `IsGenericType`, `IsGenericTypeDefinition`,
`ContainsGenericParameters` and `GetGenericTypeDefinition` came with it.

**The fixture found a divergence the other way round.** A new check asserted
that an open definition has no type arguments. `dotnet` failed it: .NET returns
the type *parameter* — `typeof(Cell<>).GetGenericArguments()` is `[T]`, not
`[]` — and this runtime had been quietly answering zero. It refuses now, because
it records only the arity of a definition and has no runtime type for a
parameter, and an empty array is exactly the plausible-looking wrong answer the
project refuses to give. No conformance check covers it: the two runtimes
deliberately differ there, so a check could not match both.

That is the second time writing the check found the bug rather than confirming
the fix. Conformance is 204 → 222 checks, byte-identical.

### Several threads share one heap, and the test found a race

The runtime beneath C# can now be driven by more than one OS thread. Three
things had to hold, and `crates/rustclr-core/tests/parallel.rs` checks each:
threads allocate into one heap, an object made on one is visible on another,
and a collection stops everyone and gathers their roots before it sweeps.

**The order was the point.** The safepoint handshake went first, alone, because
it was the only genuine unknown — if that protocol did not hold, nothing built
on it would either. The mechanical half came second: 56 call sites that read a
managed object were moved behind closures so no borrow escaped, one batch at a
time, each verified against the conformance fixture. Only once none of them held
a borrow did a lock go underneath, and at that point no call site had to change.
Attempted together, a failure could have been either half.

The compiler found the one place that could not be mechanical: `with_values` in
`collections.rs` returned `values_mut()` and applied the edit afterwards, a
borrow that would have outlived the lock. It does the edit inside the closure
now. One site out of 56 is roughly the rate the closure conversion was betting
on.

**The race the test found.** The first multi-threaded test passed, and passed
for the wrong reason: the interpreter never called `Mutators::register`, so the
collector waited for nobody. Registering it exposed the real defect. `poll`
decrements `parked` on the way out, but a thread woken from collection *N* and
not yet rescheduled is still counted — so a collector starting *N+1* immediately
afterwards saw four parked threads, concluded everyone had arrived at *this*
safe point, and swept without their roots. All four workers lost their rooted
strings. `stop_the_world` now waits for the previous round to drain before
starting the next. Forty consecutive runs clean afterwards; the same test failed
four-for-four before.

Back-to-back collections are what make it reproducible. A single collection
never shows it, which is why the first version of the test — spawn four threads,
collect once — passed while the bug was live. It also finished in 0.01s with the
workers already unregistered, so it was measuring nothing.

**What it costs.** Best of five against the same binary built before the lock:
+2% on `fields`, `virtual` and `arrays`, +4% on `alloc` and `calls`, +9% on
`strings`. On a microcontroller, 1,732 bytes of flash and no RAM at all — `bss`
and `data` are byte-identical, because without `std` the lock is a `RefCell` and
the mutator registry is a shim that compiles away. An M5Stack Tough on COM5 runs
`HelloWorld` with all of it in place, output identical to `dotnet`.

**No C# program can use any of it yet**, and it would be wrong to imply
otherwise. The loader is still per-interpreter, and it holds three things that
change while a program runs: static field storage, the interface-dispatch stub
cache, and the closed generic constructions. Two threads would see two sets of
statics. Sharing it is not a smaller version of the heap job — `registry.ty()`
runs on nearly every instruction and hands back a reference, so a lock there
would cost far more than the heap's did. It needs splitting into a read-only
part shared without a lock and a mutable part behind one. That is the next
piece of work. `docs/limitations.md` states the line as it stands.

### Seven board firmwares, one demonstration

`embedded/demo-common` holds the on-chip report and each firmware supplies a
`core::fmt::Write` to receive it. That refactor was the point of adding the
fourth and fifth boards: "they all print the same thing" is only true if there
is one copy of it, and four copies would have drifted.

| Board | Core | Target | Tier | State |
| --- | --- | --- | --- | --- |
| ESP32-C3 | RISC-V 32 | `riscv32imc-unknown-none-elf` | full | **executes IL on hardware** |
| ESP32-WROOM-32 | Xtensa LX6 | `xtensa-esp32-none-elf` | full | run on hardware (pre-interpreter) |
| Meadow F7 Micro | Arm Cortex-M7 | `thumbv7em-none-eabihf` | full | run on hardware (pre-interpreter) |
| Sipeed Maix Go | RISC-V 64 | `riscv64gc-unknown-none-elf` | full | **runs on hardware** — COM10, SRAM boot |
| Netduino 3 WiFi | Arm Cortex-M4F | `thumbv7em-none-eabihf` | minimal | builds; **not yet flashed** |
| Raspberry Pi Pico | Arm Cortex-M0+ | `thumbv6m-none-eabi` | minimal | builds; **not yet flashed** |
| Nucleo-F401RE | Arm Cortex-M4F | `thumbv7em-none-eabihf` | **none** | builds; **not yet flashed** |

`tests/firmware.sh` builds all seven. That catches a class `tests/embedded.sh`
cannot: a change to a shared type breaks a board firmware long before it breaks
the host build, and otherwise nobody notices until they reach for the hardware.

### Generic types get one runtime type per construction

`Cell<int>` and `Cell<string>` are now two runtime types. They share one body —
generics remain erased for *execution* — but each carries its own type arguments
and its own static storage, so `typeof(T)`, `x is T`, `default(T)` and a static
field per construction all agree with .NET.

**Three pieces had to line up:**

* **Constructions are built at load time**, from the `TypeSpec` that names them.
  `resolve_type_sig` is `&self` and cannot intern a new type, so building them
  lazily was never available; a pass beside the existing ones was.
* **`newobj` had to stop using the constructor's declaring type.** A member
  reference on a construction resolves to the *definition's* `.ctor`, because
  one body serves every construction — so the instance it built was a `Cell<T>`
  whose `T` was unknowable. The member reference now remembers which
  construction was named.
* **A class type parameter is answered through the receiver.** The body is
  shared, so the method cannot know; `this` can.

**Framework generics are left erased on purpose.** Every native binding is keyed
by its declaring type's name, and giving `List<int>` a name of its own would put
`List`1::Add` out of reach of the implementation behind it. Nothing in the
collections needs `T` at run time anyway — their storage is a managed array of
runtime values, and a value already carries its shape.

**One bug, and it was the kind that only shows up under test.** Static storage is
indexed by `FieldId`, not by the `slot` an instance field carries. The first
version set `slot` on each cloned static and grew the table to match, which put
every value at one index while every read looked at another — a panic in the
fixture rather than a wrong answer, which is the good outcome.

### Generic methods know their type arguments

`typeof(T)`, `default(T)` and `x is T` now answer inside a generic method, and
match .NET at every instantiation.

**Nothing had to be inferred — the argument was being thrown away.** Every call
to `M<int>` emits a `MethodSpec` carrying `int`, and the loader already gave each
spec its own `MethodInfo` so the *name* could distinguish
`AppendFormatted<bool>` from `AppendFormatted<int>`. It parsed the arguments,
used them to build a name, and dropped them. Recording them on the instantiation
and reading them when a `!!N` token is resolved is the whole change.

**The refusal that was there stays, narrowed.** `typeof(T)` on a *class* type
parameter still throws rather than answering `System.Object`, because that
argument really was erased. The check now asks whether the executing frame can
answer first, and only refuses when it cannot — so the error means "this one is
genuinely unknown" rather than "generic parameters are unknown".

**The type half is a different size of job**, and is not started. A method
instantiation only had to record what the call site already said; a type
instantiation needs a distinct runtime type per closed construction, with its
own static slots, its own identity, and a shared body whose `!0` resolves
through the receiver. That touches layout, vtables, statics and identity at
once.

### Arrays compile, at 89.8×

An `int[]` that arrives as a **parameter** is now handed to compiled code as a
two-word descriptor — data pointer and length — so `a[i]` becomes a bounds check
and a scaled-index load. The `arrays` benchmark goes from 18,401 ms interpreted
to 205 ms compiled: the largest gap in the suite, because element access is
where the interpreter pays most — a handle resolution and a `Value` per element.

**The design turned on one discovery.** `ArrayStorage` keeps `int[]` as a
contiguous `Vec<i32>`, not as boxed values. That is what made a pointer viable
at all; the plan before checking was a call out of compiled code into a runtime
helper for every element, with Win64 shadow space and volatile-register
discipline around each one. Reading the representation first turned a risky ABI
change into two loads.

**Holding a raw pointer into the managed heap is sound for a stated reason.**
At the time it rested on two facts: the backend declined any method that
allocates, so no collection could run while compiled code executed, and this
collector never moves an object. That same invariant was why an array created
*inside* a method was declined — `new int[n]` allocates, which is exactly what
it forbade.

**Superseded.** The first fact stopped being true when compiled code reached
safe points and could allocate in a loop. What replaced it: the collector still
never moves an object, and any array the frame can still reach is named by the
safe point's frame map and therefore rooted. The pointer is sound for a
different reason now, and the reason is written down where the code is.

**A bounds failure cannot throw.** Compiled code has no frame for the
interpreter to unwind, so it writes a flag one slot past the arguments and
returns, and the tier raises `IndexOutOfRangeException`. Declining and
re-running interpreted would have been wrong: stores already made stay made,
which is also what .NET does — the exception aborts the method, it does not roll
it back. Making that work meant widening `NativeTier::try_execute` to return a
`Result`, because "ran and faulted" is not the same as "declined".

**Two things the benchmarks caught.** The array workloads already in the suite
did not speed up at all, because `sieve` and `sort` allocate their arrays as
locals — the feature was working and the benchmarks could not see it. And
`rustnet jit` explained the decline of `Fill(int[],int)` as "parameter 0 is not
an integer", which was false: the explanation tested integers only while the
backend had moved on. A wrong explanation is worse than none, because it sends
the reader after the wrong thing. The real reason was `conv.u8`, from a `long`
in the benchmark I had just written.

### The interpreter runs on Xtensa

An **M5Stack Tough** — ESP32-D0WD rev v3.1, dual core, 16 MB flash — executes
`HelloWorld.Main` with all 988 native bindings:
[docs/logs/m5stack-tough.log](docs/logs/m5stack-tough.log). That is the second
architecture, and the same source file as the RISC-V ESP32-C3: 68 IL
instructions and 6 calls on x86-64, on RISC-V 32 and on Xtensa LX6 alike.

**The two-region heap was theory until now.** The ESP32's main `dram_seg` tops
out at 176 KB and the full binding set needs 260,702 bytes, so the firmware adds
the 96 KB bank at `0x3ffe7e30` that sits past the ROM's data and stacks and that
the linker will not place ordinary statics in. That reaches 278,528. The
arrangement was designed for the WROOM-32 during the STM32 work and never run;
this board is the first hardware confirmation that a heap split across two
regions actually carries the runtime.

**The banner was asserting something it could not know.** It read
"ESP32-WROOM-32" because that is the board the `esp32` feature was written for —
but a firmware knows which *chip* it was compiled for and has no way to tell
which board that chip is soldered to. Running it on a different ESP32 made the
report wrong. It now names the chip, and which board a capture came from lives
in the log header where a person writes it.

The board's previous contents — the user's own RustNet ESP-IDF firmware, on
`esp-idf-svc 0.51` — were read back to `backups/m5tough-flash-backup.bin`
(16,777,216 bytes) before anything was written. Same discipline as the Meadow
F7: a flash that replaces someone's firmware should be reversible before it
starts, not after.

### Sorting with a comparer, and a docs claim that was wrong

`list.Sort((a, b) => ...)` and `list.Sort(myComparer)` run. Both bind through
the same arity key and are told apart by what the object *is* — a
`Comparison<T>` is a delegate, an `IComparer<T>` is an object with a `Compare`
method — because binding by arity means both arrive at the same native.

The comparator is managed code: it allocates, calls back into the BCL, and can
throw. That rules out Rust's `sort_by`, whose comparator returns an `Ordering`
and can do none of those. So it is a merge sort written out, calling
`Interpreter::invoke` per comparison and propagating an exception the moment one
escapes.

**Merge sort for two reasons, both deliberate.** It makes a predictable number
of comparisons whatever the input, so a comparator with side effects is not at
the mercy of a pivot choice; and it is stable. .NET's `List<T>.Sort` is an
unstable introsort and documents the order of equal elements as unspecified, so
this is the one place the two runtimes can legitimately disagree — stated in
`docs/limitations.md` rather than left to be discovered.

**The claim in the README was wrong in both directions.** It said custom
comparers were *ignored*, which would have meant silently wrong output — the
worst failure mode this project has a convention against. They were in fact
refused with "no implementation", which is the right behaviour for something
unimplemented. Reading the code to implement the feature is what turned up the
inaccuracy; the docs now say what the code does.

`String.CompareOrdinal` came with it, because a comparison lambda almost always
reaches for it and it was unbound.

### Reflection is finished, apart from loading

`Assembly` and `Module` enumerate: `GetExecutingAssembly`, `GetEntryAssembly`,
`GetTypes`, `GetType(name)`, `GetName`, `Type.Assembly`, `Type.Module`. Each is
an object holding an assembly id, the same shape a `MethodInfo` uses for a
method id — three types rather than one because `Type.Assembly` and
`Type.Module` are distinct properties in C#, and returning the wrong one would
compile and then misbehave.

Two things the reference runtime settled rather than reasoning:

* **`GetEntryAssembly` is not assembly 0.** Slot 0 is RustBCL's synthetic
  assembly, so the first version reported `RustBCL` as the program's entry
  assembly. It now finds the one with an entry point.
* **`Module.Name` keeps the extension.** .NET answers `Conformance.dll` where
  the assembly is `Conformance`. Both the test expectation and the
  implementation were written the other way and a byte-for-byte comparison
  caught it; the name now comes from the `Module` table rather than from the
  assembly name plus a guessed suffix.

**Parameter names came with it.** The `Param` table is separate from the
signature — a signature carries types, and only that table carries what the
author called them — so `MethodInfo` now stores them and `ParameterInfo.Name`
reports the real one. A method with no rows there still answers `argN`, which
says "not recorded" rather than inventing one.

### Exception filters, and properties for reflection

**`catch when` runs.** The obstacle was never the matching rule — it was that a
filter is managed code executing *during* the unwind, before the frames below
it are discarded, which is the whole reason `when` can see state a catch block
would arrive too late to observe.

A filter now gets a frame of its own, pushed onto the frame being unwound and
sharing its method and arguments, with a copy of its locals. `endfilter` writes
those locals back and returns the `int32` verdict through the same frame-floor
mechanism a native-to-managed call uses — the mechanism that already existed for
`ToString` called from a native.

The write-back was not in the first version, and a test caught it.
`catch (E e) when (Log(ref buffer))` came back with the log empty: the `ref`
pointed into the filter's copy of the locals, so the append vanished and the
filter looked as though it had never run. Five conformance checks cover filters
now, including ordering across nested clauses and the spec's rule that a
throwing filter *declines* rather than replacing the exception in flight.

**Properties are materialised from metadata.** C# compiles `p.X` to a call to
`get_X`, so nothing was needed to run a property — what was missing was knowing
that `get_X` and `set_X` are two halves of one member, which is what
`GetProperties()` has to answer. The loader now reads `PropertyMap` and
`MethodSemantics` and interns a `PropertyInfo` per property.

Reconstructed from those tables rather than guessed from method names, which
matters twice: a method called `get_Total` is not necessarily an accessor, and a
property whose accessors an obfuscator renamed still pairs correctly.

`MethodBase.GetParameters()` came with it. A parameter has no id of its own — it
is the *n*th entry of a signature — so a `ParameterInfo` carries the method id
and the position packed into one word.

**A one-line bug worth recording**: the first run reported position 0 and type
`Object` for every parameter. `ParameterInfo` was not in the loader's list of
pre-registered framework types, so `new_member` could not find the type to
allocate and returned null for every entry. The array had the right length and
nothing else.

### CodeGen: boards, deploy, and a check that found four bugs

**Devices** (`Ctrl+D`) lists the seven boards, scans for what is attached, and
flashes firmware or a program. The panel is organised around the memory budget
because that is what decides everything else: each board's heap is drawn on one
scale against 192,045 and 260,702 bytes, so a tier reads as arithmetic. The
Nucleo-F401RE's bar stopping 126,509 bytes short of the first mark is the entire
explanation for why it cannot run C#.

Two decisions there were deliberate rather than convenient:

* **Scanning never touches a board.** It lists ports and probes. `Identify` is a
  separate action because `espflash board-info` resets the part into its
  bootloader, which should not be a side effect of opening a window.
* **Ambiguity is reported.** A CH340 bridge looks identical whether an ESP32 or
  a sewing machine is behind it, and a probe says nothing about what is on the
  far end of SWD. Those read "possibly connected". The next step writes to
  flash, and a confident wrong answer there is expensive.

**Deploy needed the firmware to stop hard-coding its assembly.** These boards
have no filesystem, so an application is not copied onto one — it is compiled
into the image. Each firmware's `build.rs` now resolves `RUSTCLR_APP` and hands
the path to `include_bytes!`, so `RUSTCLR_APP=MyApp.dll cargo build` embeds a
different program without editing the crate. Verified by swapping a 4,608-byte
assembly for an 11,264-byte one and watching the image grow.

**Six embedded templates, written to the reduced binding set** — blink
scheduler, ring-buffer logger, Modbus RTU frames, PID control, edge classifier,
Morse beacon. They stay inside `Console`, `String`, `Math` and arrays, because a
template that will not run on the board it was written for is worse than no
template.

**`--verify-templates` is the part worth keeping.** `RunsOnRustClr` was a
property nothing checked, and the convention says templates carrying it must
actually run there. The command scaffolds all 20 templates, builds them, runs
them on both runtimes and diffs the output — board templates against
`--bcl minimal`, since passing with all 988 bindings says nothing about a 192 KB
board. It found four real things on its first run:

1. **`Console.ReadLine()` hung the runner.** `ProcessRunner` redirected stdout
   and stderr but left stdin inherited, so two templates waited forever on a
   console nobody was typing at. Any child reading stdin would have wedged the
   IDE the same way. Stdin is now redirected and closed at once, which is EOF.
2. **`string + char` does not run.** .NET 10 lowers it to a span-based
   `String.Concat`, and `Span<T>` is unimplemented — so `text += "0123ABC"[i]`
   fails at *every* tier, not just the reduced one. Three of my own templates
   did it. Fixed in the templates, and documented there, because the runtime gap
   is the documented kind.
3. **`char.ToUpperInvariant` was missing** while `ToUpper` sat registered beside
   it. Added, along with `ToLowerInvariant`.
4. **`"a b".Split(' ')` failed.** It looks like a one-argument call and is not:
   C# resolves it to `Split(char, StringSplitOptions)`, whose typed key carries
   a token that means nothing outside the calling assembly. `Split/1`, `/2` and
   `/3` now bind through arity, the same fix `Join/2` needed earlier.

**And a formatting bug worth its own note.** `console-numerics` printed a
solver's residual as `0.000000000291990431…` where .NET prints
`2.9199043183325557E-10`. .NET switches to scientific notation outside a band
and Rust never does. The first fix guessed the band as `[-4, 15)` and `1e15`
disagreed; reading the boundaries off the reference runtime gave `[-4, 17)` for
double and `[-4, 9)` for float. Eight conformance checks cover it now — the
fixture is at 144.

Neither of the two boards this was built against was connected, so Deploy and
Flash are exercised only as far as building the image. Scanning, tier reporting
and the minimal-BCL pre-check run without hardware.

### The two STM32F4 boards bracket the memory question

`embedded/stm32f4` builds for a **Nucleo-F401RE** and a **Netduino 3 WiFi** from
one source file. They were not added for another Cortex-M — they were added
because they sit either side of the line the interpreter draws.

**The F427VI runs a program only after its memories swap roles.** The part
advertises 256 KB in two pieces that are not adjacent: 192 KB of DMA-reachable
SRAM at `0x20000000`, and 64 KB of CCM at `0x10000000` that the core reaches but
DMA cannot. Handing the allocator only the SRAM leaves 192 KB minus `.data`,
`.bss` and the stack, against a 192,045-byte floor — so a few kilobytes of
statics decides whether the board runs C#. `memory-f427vi.x` therefore names
**CCM as `RAM`**, which is what moves `.data`, `.bss` and the stack there
(`cortex-m-rt`'s `link.x` hardcodes `> RAM`), and gives the whole SRAM to the
heap through its own `(NOLOAD)` section. The heap could not be an ordinary
`static`: `.bss` is in CCM now, and a `static` would follow it there. Verified
by reading the linked ELF rather than by assertion — `.data` at `0x10000000`,
`.sram_heap` 196,608 bytes at `0x20000000`, `_stack_start` at `0x10010000`.
That clears the floor by 4,563 bytes, which is thin enough to re-measure if
RustBCL grows.

**The F401RE is the first board that cannot run one at all.** 96 KB against a
192,045-byte floor is not close, so the firmware prints the shortfall and
carries on with the metadata reader and the collector.

**An unplanned result worth recording: it does not pay flash for what it cannot
use.** `Tier::for_budget` is a `const fn` and `HEAP_BYTES` is a constant, so LTO
folds the decision, finds the `Full` and `Minimal` arms unreachable, and strips
the loader and all 988 native bindings. `.text` is 21 KB on the F401RE against
282 KB on the F427VI, from the same source file. That fell out of making the
tier a constant expression rather than a runtime check — worth knowing, because
it means adding a board below the threshold costs nothing but its own bring-up.

Neither board was connected, so both rows are builds.

**Two facts worth not rediscovering**, both taken from the sibling RustNet ports
rather than worked out again. The RP2040's ROM checks a CRC over the first 256
bytes of flash, so the second-stage bootloader is taken prebuilt — a wrong CRC
is a board that silently returns to BOOTSEL. And the K210 derives its UART baud
straight from the core clock, so the firmware *reads* PLL0 and the clock
selector rather than assuming 26 MHz; assuming would give a port that opens and
prints noise.

### The bare-metal crates actually build now

Milestone 6, the buildable half. `rustclr-metadata` and `rustclr-gc` compile
without `std` for four targets — thumbv7em, thumbv6m, riscv32imc, riscv64gc —
and `tests/embedded.sh` checks all of them.

**The `no_std` claim was half true before.** `rustclr-gc` built; the metadata
crate did not, because its `use alloc::…` sat in `lib.rs` and reached no
submodule, so every module failed at once the moment `std` went away. A shared
prelude fixed it. The script exists so this cannot rot silently — which is
exactly what happened to three integration tests left pointing at `net9.0`.

**A fixed heap has to be able to say no.** `Heap::embedded(n)` *reserved* n
slots and then grew past them on demand, which on a device whose RAM was
budgeted up front is not a bound at all. It is a ceiling now: `try_alloc`
returns `None` when full, and there is a test that fills one and checks it
refuses.

**And it runs on three architectures.** An ESP32-WROOM-32 (Xtensa LX6), an
ESP32-C3 (RISC-V) and a Meadow F7 Micro (STM32F777, Arm Cortex-M7) were all
flashed and run, and their metadata and collector output is **byte-identical**.
On each chip:

- `rustclr-metadata` parsed a Roslyn-built `HelloWorld.dll` out of flash and
  reported its assembly name, metadata version, table counts, entry point and
  both declared types;
- `rustclr-gc` allocated a three-node ring, kept it while rooted, **collected it
  once unrooted** (`live=0` — cycles really are reclaimed), detected a stale
  handle, and filled its heap to exactly the 128-slot ceiling before refusing.

Captured output: [Xtensa](docs/logs/esp32-wroom32.log) ·
[RISC-V](docs/logs/esp32c3.log) · [Arm](docs/logs/meadow-f7.log).

**Difficulty tracked how upstream the target is.** RISC-V and Arm are both
upstream Rust targets: stable toolchain, prebuilt `core` and `alloc`, no
`build-std`. Xtensa needed the forked toolchain from `espup`, an ESP-IDF app
descriptor the bootloader would accept, and a physically held BOOT button.

**The Meadow's obstacle was different in kind.** Its crystal frequency is
published nowhere and USB will not enumerate without it. The sibling RustNet
Meadow F7 port had already established it — 25 MHz, an Abracon ABM12W-25 at
X401 — by sweeping candidates and letting a USB host adjudicate. Reusing that
answer rather than rediscovering it is the difference between a board that
boots and one that spends its first seconds searching; the firmware prints what
it actually locked to, so the claim stays checkable.

A second lesson from that board: **enumeration is not the same as someone
reading**. The first build printed on enumeration and lost most of its report
into an endpoint nothing was draining. It now waits for `DTR` — a terminal
actually opening the port — and reports once per session.

Two bugs stood between "compiles" and "runs": the metadata crate's `use alloc::…`
reached only `lib.rs`, and `Image` was gated on `std` in its entirety when only
`from_file` and `path()` need a filesystem.

### The interpreter runs on the chip

**C# executes on an ESP32-C3** — RISC-V, 400 KB of SRAM, no operating system.
The loader builds a type registry, RustBCL registers all 988 of its native
bindings, and `HelloWorld.Main` prints the same three lines `dotnet` prints,
CRLF included, with the same 68 IL instructions and 6 calls:
[docs/logs/esp32c3-interpreter.log](docs/logs/esp32c3-interpreter.log).

**The blocker was written down wrong.** The note here said `rustclr-core`
needed "a hash map, a clock and file IO", and that had gone unchallenged for
three milestones. Two thirds of it was false: every map key in the runtime is
an integer id, a tuple of them, or a name — all `Ord`, so `BTreeMap` serves
without `std` — and the clock had been behind the `Host` trait since the trait
existed. Only the filesystem was real, and it is now the one thing gated.
`rustclr-bcl` went the same way: 20 of its 24 `std::` paths were
`std::cmp::Ordering`.

Two differences are genuine rather than cosmetic:

* **`Arc` becomes `Rc` without `std`.** RISC-V `imc` has no atomics extension,
  so `Arc` does not exist on the ESP32-C3 at all. `Rc` is correct there anyway:
  the interpreter is single-threaded on a chip.
* **Float maths comes from `libm`.** `core` has no `sqrt`, no `sin`, not even
  `abs` for `f64`, and `System.Math` is largely a libm. This is the only
  external dependency anywhere in the runtime; it is optional and a default
  build does not pull it in.

**The memory budget is the whole story on these boards**, and guessing at it
wasted two flash cycles. The first attempt asked for a single 98,304-byte
allocation and died — `Heap::embedded(4096)` reserves its slot table up front,
deliberately, and 4,096 was a number I picked rather than measured. The second
died inside `rustclr_bcl::install` at 192 KB. Measuring it with a counting
allocator settled it: **260,702 bytes** peak with every binding, **192,045**
with console, strings and maths only. `Tier::for_budget` now compares a board's
heap against those, and a board that clears neither says so in a line of text
instead of faulting inside the allocator.

**The WROOM-32 needed two heap regions**, which is the most transferable thing
here. Its `dram_seg` tops out at 176 KB — found by bisecting until the link
succeeded — below even the reduced set. The ESP32 has a second bank of 98,768
bytes past the ROM's data and stacks that the linker will not put ordinary
statics in; `esp-alloc` takes regions rather than one arena, so the firmware
adds both and reaches 272 KB. A single allocation cannot span them, and the
largest the runtime makes is 67,584 bytes, which clears either.

**Sizing the heap as a static array is what made these failures cheap.** Asking
the C3 for 320 KB produces `.bss will not fit in region DRAM, overflowed by
13844 bytes` at link time. A dynamically grown heap would have found the same
limit as a hard fault on the wire.

### Reflection works on real Type objects

Milestone 5, three quarters of it. `System.Type` is an object holding a type id,
interned one per runtime type — so `typeof(int) == typeof(int)` is reference
equality, which .NET guarantees and real code relies on. `ldtoken` resolves a
type token where it executes rather than carrying it as a number, because a
metadata token means nothing outside the assembly that emitted it.

Delivered: names, namespace, base type, the `IsXxx` family, `IsAssignableFrom`,
`IsInstanceOfType`, `GetMethods`/`GetFields`/`GetMethod`/`GetField`,
`MethodInfo.Invoke`, `FieldInfo` get and set, and `Activator.CreateInstance`.

**Custom attributes are decoded on demand.** The blob is kept at load time and
read the first time someone asks, because building an instance means running its
constructor and nothing can run mid-load. Constructor arguments, named fields
and named properties all work.

**Three refusals rather than plausible answers.** `typeof(T)` on a generic
parameter throws `NotSupportedException` — the argument was erased, and
answering `System.Object` would be wrong in a way nobody would notice. Same for
`Activator.CreateInstance<T>()`. And an attribute whose argument shape cannot be
decoded is omitted rather than built with an invented value.

### Some methods now run as machine code

Milestone 4, half of it. `rustclr-jit` emits real x86-64 for leaf methods doing
integer arithmetic, into pages that are mapped writable, filled, and only then
flipped to executable — never both at once. A tiering policy compiles a method
after 32 calls; everything the backend declines is interpreted exactly as
before.

**What it is worth.** On the `kernels` benchmark, which is the shape the backend
covers: 2971 ms interpreted, 269 ms compiled — **11.0× faster**, and 1.8× .NET
rather than 20×.

**Then inlining, and a second benchmark to measure it honestly.** A `call` used
to disqualify a method outright, which meant the backend only took code written
to suit it. `crates/rustclr-jit/src/inline.rs` splices branch-free static
callees into their callers — one level, arguments spilled to fresh locals,
`ldarg.N` rewritten to `ldloc`, the trailing `ret` dropped. The `inlined`
workload is the same arithmetic as `kernels` factored into helpers:

| | interpreted | compiled, `--no-inline` | compiled |
| --- | ---: | ---: | ---: |
| `inlined` | 1629 ms | 1148 ms | **400 ms** |

**2.9× of that is the inliner alone**, and it is worth exactly 1.0× on
`kernels`, whose callees all contain loops. Adding `--no-inline` was what made
the claim checkable rather than asserted — the same reasoning that produced
`--no-jit`.

**A bug worth recording: synthetic offsets do not work.** The first design gave
spliced instructions offsets in a high range (`0x8000_0000+`) so the caller's
offsets would never need renumbering. It was wrong. `analyse` reaches an
instruction's successor as `offset + length`, so the chain broke at each splice
boundary, `depth_at` came back sparse, and the translator read a missing entry
as depth zero — surfacing as `attempt to subtract with overflow` at
`translate.rs:416`, nowhere near the cause. The fix was to stop pretending
offsets are byte positions: renumber the whole stream one instruction per
offset and remap branch targets through a table. Nothing downstream cares,
because everything downstream treats offsets as identifiers.

**A metric that lied, caught by writing it down.** The first test asserted that
inlining compiles *more* methods across the fixture. It failed at 9 vs 10 — and
the inliner was working. Inlining `Scale` into `Blend` means `Scale` is never
called, so it never gets hot enough to compile, and the total goes *down*. The
test now names `Blend` and asks the backend about it directly.

**What it does not cover, and why that matters.** Running `rustnet jit` on the
existing benchmark suite still compiles nothing outside those two workloads:
every other one uses arrays. That is a real finding, not a footnote — the
backend's reach is narrow, and the honest way to show it was to add workloads of
the covered shape beside the others rather than in place of them.

The next step is arrays, and it is a real one. Handles are not pointers, so
reading `a[i]` from machine code means resolving a handle through the handle
table — which needs a call back into the runtime, and therefore a calling
convention the backend does not have yet.

**The bug worth remembering.** The first working version passed every unit test
and then corrupted memory in release builds only. The prologue pushed `rbx`,
`r14` and `r15` *after* establishing `rbp`, so local 0 at `[rbp-8]` sat exactly
on top of saved `rbx`: a compiled method returned having destroyed its caller's
registers. It never showed up in the emitter's own tests, because the corruption
lands in whoever called it. There is now an assertion on the frame layout, and a
differential test that runs the whole conformance fixture both ways and compares
byte for byte.

### async and await now run

Milestone 3. `Task`, `Task<T>`, the async method builders, the awaiters,
`TaskCompletionSource`, and the `Task` statics — `Run`, `Delay`, `Yield`,
`FromResult`, `WhenAll`, `WhenAny`.

**An `async` method is not special to the runtime.** That was the finding that
made this small. Roslyn lowers `async` to an ordinary struct — the state
machine — plus calls into a *builder*, and the state machine is IL the
interpreter already ran. Implementing the builder is the whole of `await`.

**Suspension needs a heap copy.** In a release build the state machine is a
struct in the caller's local, and that local is gone the moment the method
suspends. `AwaitUnsafeOnCompleted` copies it into a one-field heap cell and
resumes it through a managed pointer at that cell — the same device `newobj`
uses for value-type constructors. This only works because `ByRef::StructField`
and the `MethodImpl` table landed in Milestone 2; the conformance check
`resumed continuation` is the one that exercises it, by completing a
`TaskCompletionSource` after its awaiter has already suspended.

**Asynchrony is synchronous.** One interpreter thread means a task runs to
completion where it is created. Results, ordering and exception propagation
match .NET exactly; overlap does not exist. Stated in `capabilities` and
`docs/limitations.md` in those words, as `Thread` already is.

### Generic collections and LINQ now run

Milestone 2. `List<T>`, `Dictionary<K,V>`, `HashSet<T>`, `Queue<T>`, `Stack<T>`
and about forty `Enumerable` operators, all natively implemented, all pinned by
conformance checks. Records work as a consequence: a record's generated
`Equals` calls `EqualityComparer<T>.Default`, so it could not run at all before.

**The design decision that made this small.** Generic *types* are still erased —
`List<int>` and `List<string>` share one `RuntimeType`. That would be fatal if
the storage were typed, but a collection here is backed by a managed array of
`Value`, and a `Value` already carries its own shape. An `I32` slot and an `Obj`
slot are distinguishable without ever consulting a type argument, so one
implementation serves every `T`, `List<int>` holds unboxed integers, and the
collector traces the elements with no special case.

What that does *not* buy is user-written generic code that depends on `T` at run
time — `default(T)`, `typeof(T)`, a static field per instantiation. Real
instantiation is still the eventual answer; it is now a smaller job, because
nothing in the collections depends on it.

**LINQ is eager.** Every operator materialises a `List<T>` immediately rather
than returning a lazy iterator. Results match .NET for ordinary code; the three
cases where they do not — side-effect timing, infinite sequences, and a source
mutated after the call — are named in `capabilities` and `docs/limitations.md`.

**Ordering keeps its levels.** `OrderBy(...).ThenBy(...)` stores one key array
per level and applies them in order when the result is read. Sorting eagerly at
each step and re-sorting on `ThenBy` would discard the primary ordering — a
wrong answer that looks entirely plausible, which is the kind worth designing
against.

### Modern C# now runs

`tests/fixtures/ModernSyntax/` covers 35 features and reports `failures=0`:
string interpolation (including alignment and nesting), tuples and
deconstruction, ranges and indices, nullable value types, init-only properties,
records, pattern matching, switch expressions, target-typed `new`, local
functions, `out` variables, digit separators and binary literals.

Getting there needed five pieces of runtime work:

| Added | Why it was needed |
| --- | --- |
| `DefaultInterpolatedStringHandler` | C# 10+ compiles every `$"…"` through it |
| `MethodSpec` resolution, specialised by type argument | Generic method calls; lets `AppendFormatted<bool>` print `True` rather than `1` |
| `ValueTuple`1..`8` with real field slots | Tuples are read with `ldfld Item1` |
| `System.Index`, `System.Range`, `GetSubArray` | `a[^1]` and `a[1..4]` |
| `System.Nullable`1` | `int?` and `?.` |

Generic *types* are still erased, which no longer blocks the collections — see
the section above, and [Plan.md](Plan.md) Milestone 2.

---

## Done — CodeGen

Builds, runs, and has been driven against a live LLM end to end.

| Area | State |
| --- | --- |
| Shell | Three-pane workspace, menu, toolbar, log panel, status bar with live runtime telemetry |
| Editor | AvaloniaEdit, TextMate highlighting, tabs, line-number toggle, go-to-line, brace-depth formatter |
| Chat | Jack, four providers, model picker, attachments, clear thread, `Ctrl+Enter` |
| Kernel functions | 8 workspace · 6 toolchain · 2 web · 3 utility |
| Providers | Claude via the official Anthropic SDK behind a custom `IChatCompletionService`; OpenAI, Gemini and Ollama via the OpenAI-protocol connector — and any OpenAI-compatible endpoint by setting the base address |
| Templates | 14, across console/web/desktop/mobile/IoT/library × business/science/education/games |
| Configuration | Everything in `app.config`, editable from Settings or `--set`, no second store |
| Headless modes | `--set`, `--chat`, `--screenshot` — the same services the UI uses |

**Assistant verified against a real model.** Driven through an OpenAI-protocol
endpoint with tool calling, Jack scaffolded a project, wrote a Sieve of
Eratosthenes, built it, ran it on RustCLR, hit a real gap, worked around it, and
reported what he changed. A second run added a `Median()` to an existing project
and verified it on both runtimes. The screenshots in the docs render that actual
transcript, not an invented one.

---

## Done — packaging, benchmarks, documentation

**Packaging.** `packaging/build.sh` produces a package for any of eight runtime
identifiers; `install.sh` and `install.ps1` install per-user without privileges
and both support `--uninstall`. The full build → install → run → uninstall cycle
was exercised on Windows: a 49 MB package, clean `PATH` and Start Menu afterwards.

**Benchmarks.** `benchmarks/run.sh` compares both runtimes over ten workloads
and refuses to print a figure when their results disagree. Headline: RustCLR
starts in half the time, is at parity on exceptions and close on strings, and is
4–19× slower on compute-bound work — around 100× once process start is
subtracted. See [docs/benchmarks.md](docs/benchmarks.md).

**Documentation.** README in English and Bahasa Indonesia; `docs/` with index,
getting started, installation, architecture, runtime, CLI, CodeGen, templates,
benchmarks and limitations; `docs/id/` for the user-facing guides. Every
screenshot is rendered from the real windows by `--screenshot`, so the docs
cannot drift from the product.

---
## Bugs found and fixed, with how

Each was found by running something real, not by reading code.

| Bug | Found by |
| --- | --- |
| PE optional-header skip was 44/56 bytes instead of 60/76 — **every** image failed to load | Parsing a real Roslyn assembly instead of a hand-built fixture |
| The K210 firmware exhausted a 1 MB heap after five reports — heap fragmentation from building and tearing down the whole runtime, not a leak | Running it on a real Maix Go and watching past the first report |
| `await`'s state-machine cell went straight to `heap.alloc`, so it was counted by neither the allocation statistic nor the profiler | Asserting that what the profiler charges to types equals what the heap accounted for. On `HelloWorld` it was 4 objects short |
| Under `--dap`, a thread the program started wrote to the **real** stdout: `Thread.Start` and the task pool both built a `SystemHost` outright, so one `Console.WriteLine` from a worker would land inside a framed message and the editor would drop the connection | Trying to verify a sentence before writing it into `rustnet capabilities` — the claim was about worker threads, and reading `Interpreter::worker` showed it copied neither the host nor the debugger |
| The absolute line and column of a portable PDB's sequence points belong to the first **non-hidden** point, not the first record | The first real PDB tried reported a `negative sequence point` |
| The x86-64 verifier gave every `call` the *enclosing* method's stack delta, so any non-recursive call drifted the depth and reported a stack error in valid IL | `rustnet jit` on the benchmark assembly: two methods said "IL verification failed" where `rustnet verify` said the same assembly was fine |
| Scalar replacement bailed on every method, because `ret` is signature-dependent and has no static stack effect to look up — the catch-all arm refused it | `rustnet jit` still listing `FieldAccess` as declined after the pass was wired in |
| A devirtualised call on a **null** receiver reported "the hierarchy analysis was wrong" instead of throwing `NullReferenceException` | Deleting the null guard and finding the Conformance check still passed — the method under test was compilable but never *compiled*, because the tier only takes a method that loops or is called 32 times |
| The devirtualisation check could not tell a correct guard from a swapped one: `300*3 + 300*4` equals `300*4 + 300*3` | Reversing the case list on purpose and watching the suite stay green |
| The closed-world refusal was untested — no loaded type would have been devirtualised even with the check removed | Adding a `public` hierarchy that is otherwise an ideal candidate, purely so the refusal has something to refuse |
| The class-constructor check for elided constructors was untested: the fixture read a static field of the same class, which runs the class constructor by itself | Deleting `ensure_cctor` and watching the suite stay green; the witness now lives on a *different* class |
| `alloc_object` walked the base chain, built two vectors and cloned a signature per field — on every allocation rather than once per type | `--stats` on `alloc`: 600,009 managed calls and 300,006 native calls for 300,006 allocations |
| The interpreter-to-compiled transition built an allocation context on every call — three types resolved by name, three array constructions searched — for methods that never allocate | `--stats` on `kernels`: 239,969 of its 240,009 managed calls are calls *into* compiled code, so the measurement was the transition rather than the code |
| Register pinning was measured as "no effect" and nearly reverted; it is worth −22% on `fields` | Measuring one run per build, on a machine that drifts by ten percent. An interleaved best-of-eleven settled it — and intermediate sample sizes showed +2% and +9% before converging on zero |
| Every `ldfld` and `stfld` read the receiver's type through the heap's lock, purely to find a slot that does not depend on it | Pricing `alloc`'s candidates by deleting each one: allocation removed saved nothing, field access removed saved nothing, so the cost had to be spread across the ordinary path |
| **A frame's `pending_newobj` and in-flight exception were never traced as roots.** An object between allocation and its constructor returning, and an exception being dispatched, were both invisible to a collection | Running the fixture under a collector with a 16 KB trigger. Neither window could be reached before, because a collection only happens where the interpreter asks for one and nothing asked during either |
| **`dispatch_exception` held the exception object in a Rust local while running filters** — which are managed code, and allocate. The object could be swept mid-dispatch, surfacing as a type index of -1 | Same run. The fix registers it as a root for the length of the dispatch, on every path — hence the wrapper rather than a push and a pop around a body full of `?` |
| The first attempt at allocation-in-a-loop pinned every allocation for the length of the call: correct, and strictly worse than refusing to compile | Not by a correctness test — every one passed. By measuring peak heap at ten times the trip count and watching it follow exactly |
| Objects allocated by *compiled* code got `Value::Null` in every field, so a field declared `int` read back as the wrong shape; and their field count came from the declaring type only, so a subclass instance was allocated too small | Compiling the whole fixture at threshold 1 — the field-access helper refused the shape rather than returning a plausible zero, which is what it is for |
| `field_load` accepted an integer for *any* declared kind, so a reference stored as a number round-tripped perfectly through compiled code and was invisible to the collector | Mutating `field_store` to write a reference as `Value::I64` and watching the suite stay green. The kind is now checked, not merely used to convert |
| The inliner was one level deep, so a helper built out of smaller helpers could not be spliced and its caller stayed interpreted — 400,031 transitions on one benchmark row | `--stats` again: almost every managed call was a call *into* compiled code, with only 5.2M instructions interpreted. Flattening a callee's own calls first took that to one call and the row from 477 ms to 83 ms |
| A compiled callee read its own call table from a slot the caller never wrote, so it jumped through whatever the stack happened to hold | A segfault that appeared only at a low JIT threshold, and vanished with `--no-inline`. Tracing table resolution named the method: mutual recursion, where the callee is the only one that *has* a table to read |
| Filling a call table by *replacing* the box moved it, leaving the address another method's table already recorded pointing at freed memory | The same crash, one layer down. Tables are now allocated at their final size first and filled in place |
| The compiled stack guard counted *frames*, which is unsound for any frame size but the one it was calibrated against — larger frames turned it from a refusal into a segfault | Frames grew by ~50 bytes and 4096 of them stopped fitting a 1 MB stack. It counts bytes now, and the CLI reserves a stack for managed code rather than hoping for one |
| `stfld` through a managed pointer did not handle unboxed structs | Conformance check `struct` |
| Value-type locals were zeroed as scalars, losing their fields | Same |
| Lock-free queue took the node value *before* winning the CAS — heap corruption | The 4-producer/2-consumer stress test |
| `RuntimeHelpers.InitializeArray` unimplemented, so `new int[] { … }` failed | Conformance check `loop` |
| `Console.WriteLine` emitted LF where .NET emits CRLF on Windows | Diffing a sample's output between runtimes |
| `SettingsWindow` looked up a `TextBlock` as a `TextBox` — the dialog threw on open | The screenshot harness, which opens every window |
| `init` accessors return `void modreq(IsExternalInit)`, so `returns_void` said false and `ret` popped an empty stack | The modern-syntax fixture |
| **Collection ran between allocating an object and rooting it**, freeing the object under construction | The allocation benchmark, which panicked |
| The IL verifier reported false positives: `ret` assumed to pop always, `call` assumed neutral, every block leader seeded at depth 1 | Running `verify` on a clean assembly and not believing the output |
| `verify` reported OK for assemblies that failed at run time, because unresolvable `MemberRef`s never became methods | A runtime failure the tool had just cleared |
| **A native method calling back into managed code always got `None`, and the result was pushed onto an unrelated frame's evaluation stack** | LINQ: `Where(n => true)` kept nothing |
| `constrained.` handled value types but not reference types, so every `foreach` failed at the `Dispose` in its `finally` | The first `List<T>` probe |
| `callvirt` on `System.Object::ToString` never reached a user override, because a native stub has no vtable slot | Printing objects from a `List<T>` |
| `ldflda` required a heap object, so `p.X` where `p` is a struct local threw `NullReferenceException` | A `List<Point>` probe — the bug had nothing to do with lists |
| Nested `TypeRef`s resolved by their bare name, so `List`1+Enumerator` collided with every other `Enumerator` | Trying to resolve the enumerator `foreach` asks for |
| The loader ignored the `MethodImpl` table, so **every explicit interface implementation was invisible to dispatch** | A `yield return` iterator, whose state machine implements `IEnumerable<T>` explicitly |
| **`e.Message` was empty for every exception a program constructed itself** — the setter wrote field 0, the getter only ever read a `ClrException` | Chasing what looked like an async bug; the synchronous case failed the same way |
| `string.Join` bound only the `string[]` overload, so joining any other sequence failed to resolve | `Task.WhenAll` results, which come back as a list |
| **Compiled code destroyed its caller's callee-saved registers** — locals overlapped the saved `rbx`/`r14`/`r15` | A release-only crash; the emitter's own tests all passed |
| Three integration tests pointed at `net9.0` and had been silently *skipping* since the .NET 10 migration | Reading them while adding a fourth |
| **`"" + boxedInt` printed 0.** Improving virtual dispatch routed `object.ToString()` on a boxed `int` to `Int32::ToString`, whose receiver then arrived boxed | The advanced-feature matrix dropping 13 → 10 in one run |
| `MethodInfo.Invoke` and `FieldInfo.SetValue` passed the *box* to a primitive parameter, so every later read returned zero | The reflection conformance checks |
| The virtual-dispatch fallback checked only the *typed* native key, not the arity one, so `Type::GetCustomAttributes` lost to `MemberInfo`'s | An attribute probe panicking on a type id read as a method id |

Two are worth dwelling on.

**The GC safepoint bug** is the classic one: `newobj` allocated, then collected
before anything referenced the new object. The handle design turned what would
be heap corruption in a pointer-based runtime into a clean, diagnosable panic —
but the fix is discipline about *when* collection may run, and there is now a
conformance check that forces a collection mid-construction.

**The re-entrancy bug** is the one that had been there longest and was hardest
to see. `do_return` handed a value to the caller only when the frame stack
emptied completely; any other return pushed it onto whatever frame happened to
be underneath. For a native calling back into managed code — `ToString` on a
user type, a LINQ predicate — that meant the result vanished *and* an unrelated
evaluation stack silently gained an entry. Nothing failed loudly: `ToString`
quietly printed the type name instead. The fix gives each invocation a frame
floor, and there is now a conformance check (`ToString from native`) that fails
without it.

**The verifier crying wolf** was worse than a missing feature. `docs/limitations.md`
tells people to trust `rustnet verify`; a tool that reports problems in correct
code teaches them not to. It now resolves call targets for exact stack effects,
seeds only real handler offsets, stops trusting depth past an unresolved call,
and reports unbindable references — but only those IL actually reaches, so
attribute constructors do not bury the real findings.

---

## Not done

Ordered as in [Plan.md](Plan.md). Everything here is a deliberate refusal, a
piece of work with no hardware to run it on, or a gap still open — each said as
which, because those are not the same thing.

1. **Framework generics stay erased, by choice.** User generics are not:
   `Cell<int>` and `Cell<string>` are two runtime types with their own static
   slots, and a class type parameter answers in a *static* method because the
   call site's `MemberRef` names the construction. `List<int>` and
   `List<string>` remain one type because every native binding is keyed by its
   declaring type's name, and giving `List<int>` a name of its own would put
   ``List`1::Add`` out of reach. `GetGenericArguments()` on an *open* definition
   refuses: .NET returns the parameter `T`, and this runtime has no runtime type
   for one.

2. **Concurrency is done.** `Thread.Start` spawns an OS thread; `Task.Run` and
   `Parallel.*` run on a pool of one worker per core; `await` suspends and the
   completing thread resumes; `lock`, `Interlocked` and `Monitor.Wait`/`Pulse`
   all do what they say, and threads share one heap and one set of static
   fields. What remains is a weaker promise, stated rather than hidden:
   `Monitor.Pulse` wakes *every* waiter rather than one, which a `Wait` written
   in the required re-check loop already tolerates.

3. **Native code generation** now covers `int` and `double` arithmetic,
   `int[]`/`double[]`/`bool[]` element access, allocation — including inside a
   loop, since compiled code reaches safe points — object allocation, field
   access through a helper, guarded devirtualisation, recursion, and calls
   between compiled methods. Every benchmark row is compiled or native.

   Still declined, and both are code rather than hardware: **exception
   handling** and **instance methods**. Separately, the AArch64 and RISC-V
   backends **have never executed a single instruction**. They encode the same
   IL through the same shared translation and are checked by disassembly, but no
   board has been connected — and an unexecuted backend that claims to work is
   worse than no backend at all.

4. **Reflection is broad.** Types, members, properties, parameters and their
   names, assemblies, modules, invocation, attributes, and constructing a
   generic type at run time all work. Two divergences are documented rather than
   papered over: the open-definition case in (1), and `Assembly.Load` resolving
   by probing where .NET reads `deps.json` — so an unreferenced DLL beside the
   app loads here and does not there, which is a divergence in the risky
   direction.

5. **IL runs on hardware; ahead-of-time compilation does not.** An ESP32-C3
   (RISC-V 32) and an M5Stack Tough (Xtensa LX6) both execute `HelloWorld.Main`
   with the whole of RustBCL, printing bytes identical to `dotnet`. The Pico,
   Netduino and Nucleo-F401RE images build but have never been flashed —
   no board was connected. AOT additionally needs the Arm and RISC-V code
   generators to execute, which is (3).

6. **`Span<T>` is done** — over an array, over a string, and over `stackalloc`
   memory, with `Memory<T>`, `AsSpan` and `AsMemory`. Slicing a span that
   *stands for* a string still refuses: the string is the whole representation
   and there is no offset in it.

7. **Five IL opcodes are refused**, each returning "not implemented by this
   runtime" rather than doing something plausible: `arglist` (varargs),
   `mkrefany`, `refanyval`, `refanytype` and `jmp`. Multi-dimensional arrays
   with non-zero lower bounds are also absent. `localloc`, `cpblk`, `initblk`,
   exception filters and `calli` all work — the first three were once listed
   here as blocked on the value model, and stopped being blocked when `Value`
   gained a raw-pointer kind that is a buffer plus a byte offset rather than an
   address.

8. **Toolchain surface named in `requirements.md` that does not exist yet.**
   Listed because scope is measured by the requirement, not by what was built:

   | | |
   | --- | --- |
   | Editor extension for the debug adapter | `rustnet debug --dap` is a stdio DAP server and VSCode or Rider can attach to it, but nothing contributes the debug type and launches it |
   | Expression evaluation while stopped | no `evaluate` request, so an editor's watch window and debug console cannot compute anything |
   | `rustnet deploy` | CodeGen has a Deploy menu and a device flasher; the CLI has no `deploy` command |
   | A call graph, and CPU time | `rustnet profile` reports self and inclusive time per method, exact call counts and allocation by type — but not caller/callee edges, and it measures wall clock, so time blocked in a wait counts against the waiter |
   | Ahead-of-time compilation | only the JIT exists — see (3) and (5) |
   | The RISC-V and AArch64 code generators have never executed | both emit and both are byte-checked against a disassembler, but `can_compile` answers false off a matching host. The Maix Go could now run the RISC-V one — what is in the way is that `rustclr-jit` is `std`-only and `codepage.rs` needs `mmap`/`VirtualAlloc` |

### Advanced C# features

Roslyn's own feature list for C# 12, 13 and 14, probed one feature at a time and
diffed against `dotnet`: **21 of 21 pass on RustCLR**. Results and reasoning:
[docs/advanced-features.md](docs/advanced-features.md) ·
[Bahasa Indonesia](docs/id/fitur-lanjutan.md).

That measures *output*, and output is not capability — though it means more now
than when this was written, because the things it could not then distinguish
have since been built. `Task` and `Parallel.For` do overlap now, on a real pool.

Two findings from that run were not obvious beforehand:

- **Source generators and interceptors need no runtime support at all.** Both
  are Roslyn-side; the runtime only ever sees ordinary IL. Proven with a
  generator that actually intercepts a call, not by reasoning about it.
- **Union types and closed hierarchies do not exist in .NET 10.** `IUnion` and
  `IsClosedTypeAttribute` are absent from the BCL, so there is nothing for any
  runtime to support yet.

---

## Design decisions worth remembering

- **Handles, not pointers, in the GC.** A stale reference is detected, not
  dereferenced. Costs one array load; removes a whole bug class — and turned the
  safepoint bug above into a diagnosable panic instead of corruption.
- **Framework types are a contract, not managed code.** The loader turns
  unresolved `MemberRef`s into internal-call stubs keyed by
  `Namespace.Type::Method(params)`; RustBCL supplies the Rust implementation.
- **The interpreter loop is iterative.** Managed recursion exhausts a frame
  budget and throws `StackOverflowException` rather than killing the process.
- **Managed pointers are structural, not addresses.** They cannot dangle and
  never need collector fix-ups — which is also what makes value-type
  constructors and the interpolation handler work.
- **A safe point in compiled code is a description, not a rewrite.** Three
  things make that true and all three are load-bearing: this collector never
  moves an object, a `Handle` is `repr(transparent)` over `u64`, and the poll
  sits where the evaluation stack is empty. Together they reduce "find this
  frame's roots" to reading a list of byte offsets computed from declared
  types — no inference over the instruction stream, and nothing to write back.
  Anything that would need more than that is declined instead.
- **A permission and its obligation are one change.** Allowing allocation
  inside a compiled loop is only sound *because* the loop can be stopped there
  and pruned there. Shipping the permission without the pruning would have been
  worse than shipping neither: it compiles methods that can exhaust a heap the
  interpreter would not. The test that guards it measures memory at two trip
  counts rather than checking any answer.
- **A field's slot does not depend on the receiver's type.** The flattened
  layout is base-first, so a field declared in `T` sits at the same index in `T`
  and in every subclass. That is what makes the slot cacheable by field alone,
  and what let both field paths stop reading the object's type through the
  heap's lock. The bounds check inside the accessor is what keeps a receiver
  that somehow lacks the field an error rather than a read of the wrong slot.
- **Price a bottleneck by deleting it, not by reading the code.** `alloc` looked
  like an allocation problem and is not: making every `new` return the same
  object saves nothing, and so does removing field access entirely. Only the
  whole `newobj` path moves it, by 14%; the rest is plain interpretation. Three
  cheap experiments were worth more than any amount of staring at the
  allocator.
- **An "off" feature is only free if it is measured.** The debugger hook is one
  branch per instruction and looked obviously free. Interleaved against a build
  with it removed it cost **8%** on the densest interpreted workload — the
  branch and its registers were being hoisted into the hot loop. Marking the
  poll `#[cold]` and `#[inline(never)]` took it to nothing. The first
  measurement that suggested otherwise compared two different sessions, which
  is the mistake this file already records once.
- **A budget must be denominated in the resource it protects.** The compiled
  recursion guard counted frames because frames are what recursion produces; the
  resource is *bytes of machine stack*, and the two only agree for one frame
  size. It was latently wrong from the day it was written and only failed when
  frames grew — as a segfault, which is precisely what it existed to prevent.
- **A default is not a guarantee.** Managed recursion runs on the machine stack,
  so how deep a program can go was decided by whatever stack the host happened
  to give the thread — a megabyte on Windows, less than the interpreter's own
  frame budget needs. A runtime that requires a resource should ask for it, so
  the CLI now runs managed code on a stack it reserves itself.
- **An optimisation's reach is part of its design, not a detail.** The inliner
  was correct from the day it was written and still left the benchmark it
  existed for entirely interpreted, because real code factors a helper out of
  smaller helpers and one level cannot see through that. What it *could* do was
  never the question; what the code in front of it looks like was.
- **Compiled code being slow is not the same as compiled code being bad.** The
  `kernels` and `inlined` rows were both compiled and both slow, which read as a
  code-quality problem and was not: a quarter of a million *transitions* into
  compiled code, each doing setup the method had no use for. Ask `--stats` what
  a benchmark is actually doing before reading its number as a verdict on the
  thing you were thinking about.
- **One rule for "this constructor does nothing", on the loader.** The
  interpreter skips the call, scalar replacement retires the object, and
  compiled `newobj` allocates without a frame — three consumers, one reason
  (none of them runs a constructor), and therefore one implementation. It lives
  on `Loader` rather than in the JIT because the interpreter asks it on every
  `newobj`, and it is memoised per interpreter because decoding IL to answer
  costs more than the frames it saves.
- **A `Handle` may cross into compiled code; a `Value` may not.** The two look
  like the same question and are not. `Handle` is one `u64` in a newtype, so
  `repr(transparent)` is the language's own promise about its representation —
  and it is an opaque token rather than an address, so compiled code cannot
  dereference one and a stale value fails a generation check instead of reading
  freed memory. `Value` is a multi-variant enum with boxed payloads and no
  promise at all. Anything compiled code needs to *know* about an object — its
  type, its fields — comes from a helper call into Rust, never from reading the
  heap.
- **`Value`'s memory layout is not part of the JIT ABI, and did not become one
  — field access went through a helper instead.** Compiling `ldfld` the obvious way means emitting machine code that
  reads a `Value` out of a `Vec<Value>` — which requires knowing the enum's
  discriminant and payload offsets. Rust promises neither without `repr`, the
  enum has boxed variants, and a future compiler is free to reorder them. The
  failure would be a *wrong number*, not a crash, and it would appear on a
  toolchain upgrade with no source change. Scalar replacement gets the same
  benchmark win with no layout assumption at all, so the layout route was
  declined rather than deferred. If field access on escaping objects is ever
  needed, the sound version is a helper call — the shape `alloc_array` already
  uses — not inline layout knowledge. That is what was eventually built: a call
  per access, `int`/`long`/`double`/reference crossing as one 64-bit word, and
  a struct field refused. It costs more than a load and cannot be wrong in the
  way an inline load could, and it took `alloc` from 613 ms to 206 ms — so the
  refusal cost nothing in the end except the time to find the right shape.
- **Unsupported interop shapes are refused, not guessed.**
- **Strings are `Vec<u16>`.** .NET defines `Length` and indexing in UTF-16 code
  units; UTF-8 would make them wrong or O(n).
- **Collection only happens where every live value is rooted.** Between
  instructions, before an allocation, before a return value is popped.

## The three K210 demos, ported and run on the Maix Go (2026-08-24)

`Samples/Showcase`, `Samples/WifiCheck` and `Samples/Kiosk`, from the
reference's `firmware-k210/demo`. Two of the three run on the hardware; the
third is characterised below rather than claimed.

| Sample | Libraries | On the Maix Go |
| --- | --- | --- |
| `Showcase` | 3 | **runs indefinitely** — `docs/logs/maixgo-showcase.log` |
| `WifiCheck` | 3 | **runs** — camera captured, radio reports honestly |
| `Kiosk` | 8 | loads and executes; **cannot allocate its 320x240 camera frame** |

### What each one exposed

Porting three real programs found five defects, all of which are fixed:

- **A bounded heap could exhaust its slots without ever asking to collect.**
  `has_room()` bounds by *slots*, `should_collect()` measured only *bytes*, so
  a loop of small short-lived objects filled 512 slots while live bytes stayed
  under the 8 KB `minimum_heap` and the allocator panicked on a heap that was
  almost entirely garbage. `HeapPressure` now carries `slot_limit` and
  `MarkSweep` collects at three-quarters full. That is what killed `Showcase`
  after two passes through its scene loop; the regression test fails on
  allocation 512 without the fix.
- **A struct or enum parameter could not bind across an assembly reference.**
  A `ValueType`/`Class` in a signature is a *token*, and `type_sigs_match`
  compared two of them raw — tokens from different assemblies' tables, which
  are never equal. So `Camera.Configure(int, int, PixelFormat)` failed to
  resolve, was synthesised as an internal call, and died at the call site with
  `no implementation for ...(int,int,#16777256)` despite having a perfectly
  good IL body. `find_method_on_type_from` resolves each side's tokens in its
  own assembly first. This affected *any* managed library API taking an enum,
  not only the camera.
- **The full BCL tier registered fewer bindings than the minimal one on a
  board.** `install` guarded networking on `std`; `install_minimal` registered
  the offline bindings unconditionally. A board with plenty of RAM therefore
  reported `bcl tier full` and then failed `Wifi.Connect` with "no
  implementation" — on hardware where the *smaller* tier worked.
- **The firmware library ceiling was three, and `Kiosk` needs eight.** Widened
  across all five firmware crates and `RustNet.Deploy`. `RustNet.Media`'s
  reference to `RustNet.Graphics` also went: it existed only for a doc link,
  and it made every program that captures an image load a whole extra assembly.
- **`rustnet profile` could not take `--board`,** so a device program could not
  be profiled at all — it cannot get past its first `Display` call.

### New capability: the camera

`Board::camera()` (defaulted to `NotSupported`, so boards without a sensor are
unchanged), `SimCamera` reachable on the host board, `K210Camera` over the
existing DVP driver, and `RustNet.Media.Camera` bindings. Nine checks in the
Device fixture, which is now at **73**.

On the Maix Go this captures real frames: `camera: 320x240, 153600 bytes,
30% lit`. The sensor owns I²C2 exclusively — `Board::i2c` deliberately never
handed it out — and a capture takes the DVP data pads from the panel and gives
them back, so a program can capture and then `Present` without knowing.

### What `Kiosk` cannot do, and why

It loads all eight libraries (299 types, 359 methods), runs, reports no
network honestly, and then fails to allocate 153,600 bytes for its first
camera frame. The heap is **3 MB, fully free when the program starts** — the
figure is now printed at boot — so this is the interpreter, the full binding
table, nine assemblies' metadata and a 150 KB canvas consuming the rest,
against a linked-list first-fit allocator.

3 MB is what fits: the heap array must not cross the SRAM bank boundary at
`0x80400000` (a 4 MB one wedged the board silently), and with `Kiosk`'s eight
libraries compiled in the array starts at `0x800C4210`, leaving 3.23 MB below
it. A runtime check now prints that refusal instead of going silent.

Two things `Kiosk` also cannot do on this board, stated rather than worked
around: the Maix Go's radio is an ESP AT module on UART1 and `Board::netif`
still returns `NotSupported` — `rustclr-espat` is protocol parsing with no
transport — so MQTT has nothing to reach.

### Ported deliberately differently

`Kiosk`'s three firmware-provided `[InternalCall]` button methods became
constants plus the ordinary `Gpio` bindings. An internal call has to be
registered by the firmware, which means a program using one cannot run under
`--board host` and cannot be tried without flashing; constants run in the
simulator and on the board from one source, and a different pinout is three
numbers rather than a firmware build. The values are the reference firmware's.

`Showcase`'s run counter is now optional: a Maix Go booted into SRAM has no
filesystem bound, and the scenes are the point.
