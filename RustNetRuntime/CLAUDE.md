# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Status: milestones 1-3 and 5 complete, 4 and 6 partly done, and it works

The runtime executes real Roslyn-compiled assemblies. `cargo test --workspace` runs 416 tests;
the conformance fixture prints `checks=361 failures=0` on RustCLR, byte-identical to `dotnet`,
and `ModernSyntax` prints `checks=35 failures=0`. Generic collections, LINQ and
`async`/`await` all run, `Span<T>` and `unsafe` pointers work, and reflection works on
real `System.Type` objects.

**The code generator has grown well past integers.** Integer *and* `double`
arithmetic, `int[]`/`double[]`/`bool[]` element access, allocation, and recursion all
compile to x86-64; a method whose body loops compiles on its first call rather than
waiting for a call count. `sort`, `sieve`, `matrix` and `fib` all went from
seconds to under a quarter-second.

**Objects that never escape are replaced by their fields** rather than read out of
the heap — `Value` is a Rust enum whose layout is not promised, so making it part of
the JIT ABI was refused. `crates/rustclr-jit/src/escape.rs` states the proof
obligations; anything it cannot follow is declined. That took `fields` from 3,173 ms
to 89 ms.

**Virtual calls are devirtualised when every answer is known** — see
`crates/rustclr-jit/src/devirt.rs`, which trusts class-hierarchy analysis only where
the declaring type cannot be extended from another assembly, and refuses a public
hierarchy even when it looks ideal. `virtual` went 1,923 ms → 103 ms. Compiled code
holds heap references *only* as `Handle`s: `repr(transparent)` over `u64`, opaque,
never dereferenced, with the receiver's type fetched by a helper rather than read
out of the object.

**A constructor that provably does nothing is not called** —
`Loader::constructor_does_nothing` is the single rule, shared by the interpreter,
scalar replacement and compiled `newobj`, because three copies of it would drift.
An object's starting field values are computed once per type rather than per
allocation. Together those took `alloc` 1,093 ms → 670 ms.

**The busiest two locals or arguments are pinned in callee-saved registers**
(R12/R13), worth −22% on `fields` and nothing on rows where short compiled leaves
are called from an interpreted loop. **A compiled method that cannot allocate gets
no allocation context**, and the marshalling buffers are reused — the transition,
not the emitted code, was most of what `kernels` and `inlined` measured.

What is still interpreted: **field access on an object that escapes**, virtual calls
through an open hierarchy, and calls to methods other than the caller itself.
**A field's slot is cached per field, not per receiver type** — base-first layout
makes it the same in every subclass, so `ldfld`/`stfld` no longer read the object's
type through the heap's lock.

**Compiled code reaches safe points.** A compiled method polls at every loop
back-edge — for another thread's collection *and* for its own allocation pressure,
since with one thread nothing else ever asks. The frame is described by byte offsets
fixed at compile time, which works because the collector never moves an object, a
`Handle` is `repr(transparent)` over `u64`, and the poll sits where the evaluation
stack is empty. A back-edge reached with a non-empty stack gets no safe point and
keeps the old rules. **Allocation inside a loop is therefore legal**, and each safe
point releases what the frame can no longer reach, so a loop's memory is flat rather
than proportional to its trip count — there is a test that fails if it is not.

**A field of a heap object is reached by a helper call, never an inline load** —
`Value` is a Rust enum with boxed payloads and no promised layout, so reading it
from emitted code would fail as a wrong number rather than a crash. `int`, `long`,
`double` and reference fields cross as one word; a struct field is refused, and
the declared kind is *checked* so a reference cannot be stored as a number and go
untraced. With that and safe points, `alloc` stopped being interpreted: 613 → 206 ms.

**The inliner flattens more than one level** — a callee that is itself built from
smaller callees is flattened before being spliced, which is how code is actually
written. That took `inlined` from 477 ms to 83 ms by turning 400,031 transitions
into compiled code into **one**. Depth and resulting size are both bounded.

**One compiled method calls another** through a table of entry addresses the tier
fills in before the caller runs — resolved in two passes, because an entry names
the callee *and the callee's own table*, and two methods may call each other. That
compiled `kernels` (350 → 142 ms), whose helpers loop and so can never be spliced.
Compiled calls are bounded by **bytes of machine stack**, not a frame count, and
the CLI reserves that stack rather than hoping the host provided it.

**Every benchmark row is now compiled or native**, none slower than 2.0×.

**Concurrency is real.** `Thread.Start` spawns an OS thread; `Task.Run` and
`Parallel.*` run on a pool of one worker per core; `await` suspends and the completing
thread resumes; `lock`, `Interlocked` and `Monitor.Wait`/`Pulse` all do what they say.
Threads share one heap and one set of static fields, and each gets an identical *copy*
of the loader — which is why no lock sits on the path that runs every instruction.

**The whole runtime builds without `std`, and the interpreter runs on microcontrollers** —
an ESP32-C3 (RISC-V 32), a Sipeed Maix Go (K210, RISC-V **64**) and an M5Stack Tough
(Xtensa LX6) all execute `HelloWorld.Main` with the whole of RustBCL, printing bytes
identical to `dotnet`. Three architectures, one source file. The K210 is booted into
SRAM (`kflash -s`, which never touches its flash) and watched with DTR and RTS
cleared, because either one resets the chip and discards the image. `bash tests/embedded.sh` checks metadata, gc, core and bcl on four
bare-metal targets. Memory is the binding constraint there: 260,702 bytes peak with every
binding, 192,045 with a console-only subset, and each firmware picks from its heap budget —
the Nucleo-F401RE's 96 KB clears neither, which it reports rather than faulting.

CodeGen builds and runs. Not a git repository.

**An editor can attach.** `rustnet debug --dap` is a Debug Adapter Protocol
server — JSON over stdin/stdout — with breakpoints by source line, the call
stack as `file:line`, and locals by name, all read out of the portable PDB the
SDK writes beside the assembly. `rustnet debug` alone drives the same hooks
from a terminal. Its JSON is hand-written because the workspace has no external
dependencies. Under `--dap` **stdout belongs to the protocol**: the program's
console becomes `output` events. The hook is `#[cold]` and never inlined
because without that it cost 8% on the interpreter. What is missing is stated
in `docs/limitations.md`: no editor extension, no `evaluate`, and no pause
while running, because the hook is synchronous.

**The profiler samples itself.** `rustnet profile` reports self and inclusive
time per method, exact call counts, and what was allocated by type — arrays
charged to their element type, not lumped under `System.Array`. There is no
sampling thread: the interpreter counts down 512 instructions, reads the clock,
and walks its own frames, which is why it needs no synchronisation and works on
a board. It charges *measured* elapsed time rather than one tick per sample, so
a late sample does not shorten the run it reports. It measures **wall clock**,
profiles **one thread**, and interprets by default because compiled code runs no
IL. A program reads the same counters through `GC.CollectionCount` and
`GC.GetTotalAllocatedBytes`, so the two cannot disagree.

**A C# program can drive hardware.** `RustNet.Hal.Gpio.Write(2, true)` reaches a
pin: C# → `managed/RustNet.Device` → a native binding → a `Board` trait object →
the chip. The board is a *swappable module* like the collector and the
scheduler — `crates/rustclr-hal` is traits only, `crates/rustclr-hal-host` is a
simulator, and `rustnet run --board host` installs it. **No board is an error,
not a no-op.** `RustNet.IO.Storage` is whole-file over a log-structured flash
store, because NOR cannot set bits and a seekable stream would erase a sector
per write. `RustNet.Security` is SHA-256, HMAC, AES-CTR and CRC-32 **written out
rather than depended on**, checked against the vectors FIPS 180-4, FIPS 197,
SP 800-38A and RFC 4231 publish — no RSA, no RNG, and `docs/limitations.md` says
why. `tests/fixtures/Device` is the hardware sibling of the Conformance fixture.

**The managed libraries are seventeen assemblies under `managed/`, and that is
load-bearing.** A loaded assembly's types are *all* registered whether the
program mentions them or not, so one combined library made a program that only
draws also carry CAN, MQTT, a camera and a database — measured at 320 types and
498 methods against 253 and 68 for the two it actually used. Every template
references only the libraries its source names. Porting from the reference
requires exactly one managed change: `[InternalCall] ... => throw new
RuntimeOnlyException()` must become `extern` + `[MethodImpl(InternalCall)]`,
because RustCLR dispatches a native only for a method with no IL body.

**Fourteen templates in `templates/`** (`bash tests/templates.sh`), and three
host tools in `tools/` — Deploy, Workbench, Designer. The reference's
`rustnet-mdp` (DLL→RNX) is deliberately absent: RustCLR reads PE/ECMA-335, so
what gets signed and flashed is exactly what Roslyn produced.

**`graphics-primitives` runs on the Maix Go**, drawing into a 320x240 RGB565
canvas through an ST7789V. A firmware carries its libraries via
`RUSTCLR_LIB1`/`LIB2` because a board has no filesystem to resolve an
`AssemblyRef` against; the program's console **streams** rather than being
captured, because a device program loops forever and a captured console prints
nothing; and attaching the panel is what raises the backlight, since the
backlight rail hangs off the panel's reset line.

The loader now **follows assembly references** from the directory a program sits
in, skipping framework names so a self-contained publish cannot supply a second
`System.Object`.

**The device layers are all in**, ported from the Gravicode RustNet reference:
`hal` + four chip HALs (K210, ESP32-C3, RP2040, STM32), `flashfs`, `crypto`,
`secureboot`, `ota`, `gfx`, `mqtt`, `usb`, `db`, `diag`, `net`. Every one but
`net` builds on all four bare-metal targets — `tests/embedded.sh` checks
fourteen crates. **Each of the seven firmwares installs its real board** and
reports what the chip offers; nothing in that report drives an output, because
a demo that wrote an arbitrary pin on unknown wiring is a short or a relay.

Three things the port could not copy, each with the reason in
`docs/limitations.md`: the **font** is now a 1 KB table generated from readable
pixel art (a test renders it back to text); **secure boot** verifies an HMAC
rather than RSA, so the verifying key can also sign and that is stated rather
than glossed; and **`rustclr-diag`'s lock** is a try-borrow that drops a record,
because thumbv6m and rv32imc have no compare-and-swap and a spin lock would hang
any board whose interrupt handler logged.

`requirements.md` is the single source of truth for scope. Read it before starting any work —
this file summarizes its binding decisions but does not replace it. `Plan.md` holds the
roadmap and `Progress.md` the state; **update `Progress.md` as work lands.**

Read `Progress.md` before starting: it records which bugs were found and how, which is usually
faster than rediscovering them.

## What is being built

Two things live under one repo:

1. **RustNetRuntime** — a from-scratch reimplementation of the CLR in **Rust**, replacing CoreCLR.
   C# remains the surface language; the runtime beneath it is entirely Rust. This is a redefinition
   of the runtime, not a port of CoreCLR's C++.
2. **CodeGen** — an Avalonia UI desktop code editor whose built-in AI assistant ("Jack - The Code
   Bender") generates applications from prompts.

### Runtime component names (use these exact names)

| Name | Role |
| --- | --- |
| `RustCLR` | Core runtime: GC, JIT/AOT interface, threading, async/await scheduler |
| `RustBCL` | Base class library: collections, IO, numerics, DateTime, Regex |
| `RustNet Toolchain` | Compiler driver, CLI (build/run/deploy), debugger hooks, profiler |
| `Interop Bridge` | P/Invoke and FFI layer, ABI compatibility, safe pointer/handle wrappers |

### Layering

C# source → IL → metadata reader → `RustCLR` loader → JIT/AOT → native. `RustBCL` sits on
`RustCLR`; `Interop Bridge` is how both reach native/OS code. The GC and the scheduler must be
swappable modules — do not hard-wire either into the rest of `RustCLR`.

## Hard constraints from the spec

- **Rust, not C++**, for everything in the runtime. Memory safety and Rust's concurrency model
  (ownership, channels, lock-free queues) are the reason the project exists.
- **Backward compatibility**: existing C# must run without significant modification.
- **Targets**: Windows, Linux, macOS across x86, x64, Arm, Arm64, RISC-V — plus microcontrollers
  (ESP32, STM32, RISC-V). Assume `no_std`-friendly design in core crates where feasible.
- **Interop-friendly**: calling into C/C++ libraries stays possible.
- Performance is a stated requirement: fast, low memory, lightweight.

## CodeGen tool constraints

- **Avalonia UI**, three-pane layout: file explorer (VSCode-like) left, code editor center
  (line numbers with show/hide toggle, syntax highlighting), chat panel right.
- Chat panel: resizable width, hide/show, image attachments, `Ctrl+Enter` or Send button to
  submit, clear-thread button, LLM model picker at the top of the panel.
- Menu/toolbar: New Project (Folder), Open Project/File, Close Project, Go To Line Number,
  Format Code, Build, Run, Deploy, Exit.
- New Project offers **Blank** and **From Template**; templates should span web, cross-platform
  desktop, console, mobile, and IoT, with use cases across business/industry, science, education,
  and games.
- Status bar plus a logs panel along the bottom for process output.
- **LLM access goes through Semantic Kernel.** Supported providers: OpenAI, Claude, Gemini, Ollama.
- **All configuration lives in `app.config`** (model, API key, endpoint, temperature, system
  prompt, and every other setting) and must be editable from the UI. Do not introduce a second
  config store.
- Kernel functions the assistant needs: project scaffolding, create/modify code, run, debug,
  compile — plus common functions `SearchInternet` (Tavily), `ScrapeWebPage`, `MathCalculation`,
  and date/time.
- Use the `frontend-design` skill (in `.claude/skills/`) when building or reshaping UI — the spec
  explicitly calls for it.

## Documentation requirements

- `README.md` in **both English and Bahasa Indonesia**, with screenshots.
- Full documentation under `docs/`, with screenshots.
- Ship plentiful sample data and sample users.
- Attribution, in both the docs and the app itself: *dibuat oleh Gravicode Studios, dipimpin oleh
  Kang Fadhil* (built by Gravicode Studios, led by Kang Fadhil).

## Commands

Rust `1.97.1` / cargo `1.97.1`, .NET SDK `10.0.400`; every project targets `net10.0`. There is no `.sln`;
the C# side is one project built by path.

```bash
# Runtime
cargo build --release                      # produces target/release/rustnet
cargo test --workspace                     # 416 tests; must stay green
bash tests/embedded.sh                     # 14 crates x 4 bare-metal targets; must stay green
bash tests/firmware.sh                     # seven board firmwares; must stay green
cargo test -p rustclr-core                 # one crate
cargo test -p rustclr-gc a_cycle_is        # one test by substring

# CodeGen
dotnet build src/CodeGen -c Release
dotnet run --project src/CodeGen
dotnet run --project src/CodeGen -- --verify-templates   # every template on both runtimes

# Screenshots for README and docs — re-run after any UI change
dotnet run --project src/CodeGen
dotnet run --project src/CodeGen -- --verify-templates   # every template on both runtimes -c Release -- --screenshot docs/images
```

### The check that matters

Before claiming a runtime change works, run a real assembly through both runtimes and diff:

```bash
cd tests/fixtures/Conformance && dotnet build -c Release
dotnet bin/Release/net10.0/Conformance.dll                    # expect: checks=361 failures=0
../../../target/debug/rustnet.exe run bin/Release/net10.0/Conformance.dll
```

The same applies to `samples/UserDirectory`. Output must match byte for byte — a CRLF/LF
mismatch in `Console.WriteLine` was caught exactly this way.

When you add a runtime capability, add a check to `tests/fixtures/Conformance/Program.cs`
that fails without it. That file is the definition of "works".

## Layout

```
crates/          8 Rust crates — see the table in Progress.md
src/CodeGen/     the Avalonia IDE (Models, Services, Plugins, ViewModels, Views, Themes)
tests/fixtures/  HelloWorld and Conformance — real C# projects, built with the .NET SDK
samples/         sample data, sample users, and a worked example
docs/            documentation and generated screenshots
```

## Conventions that are load-bearing

- **State limitations plainly.** `rustnet capabilities` prints them from the runtime, and
  `docs/limitations.md` explains each with its reason. Do not quietly widen a claim.
- **Refuse rather than guess.** Unsupported interop shapes, unresolvable tokens and unmatched
  unsupported IL opcodes all produce a clear error instead of a plausible-looking wrong answer.
- **Templates marked `RunsOnRustClr = true` must actually run there.** Generic collections
  and LINQ landed with Milestone 2 and `async`/`await` with Milestone 3, so all three are
  fair game, as are exception filters; `Span<T>` and TPL are not.
