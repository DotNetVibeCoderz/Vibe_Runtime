# RustNetRuntime

**C# on a runtime rebuilt in Rust.**

*[Baca dalam Bahasa Indonesia →](README.id.md)*

RustNetRuntime replaces CoreCLR. C# stays the language you write; underneath it,
the runtime — garbage collector, loader, execution engine, interop — is Rust.
This is not a port of CoreCLR's C++. It is a re-implementation, built to take
the safety and concurrency model Rust offers rather than to reproduce the
original line by line.

Alongside the runtime is **CodeGen**, a desktop IDE whose assistant — *Jack, The
Code Bender* — scaffolds projects, writes code, and runs it on either runtime
without leaving the window.

---

## It runs real assemblies today

This is the check that matters. A C# program compiled by Roslyn — with
inheritance, interfaces, structs, enums, delegates, generics and exception
handling — produces identical output on both runtimes:

```console
$ dotnet Conformance.dll
checks=136 failures=0

$ rustnet run Conformance.dll --stats
checks=136 failures=0

─── execution ──────────────────────────────
  wall clock                  10.505 ms
  IL instructions              19,617
  throughput                 1,867,343 instr/s
  managed calls                 2,167
  native calls                    103
  peak frame depth                 16
─── heap ───────────────────────────────────
  collector               mark-sweep
  allocations                     194
  live bytes                   10,023
```

The 38 checks cover arithmetic and overflow, `long` maths, control flow,
strings, arrays, class inheritance, virtual and interface dispatch, properties,
structs, enums, boxing and casting, `try`/`catch`/`finally`, nested handlers,
divide-by-zero, delegates, and allocation under collection pressure. A second
suite, `tests/fixtures/ModernSyntax/`, does the same for 35 modern C# features.
Both are normal C# projects you can read and extend.

`cargo test --workspace` runs 163 tests across the eight crates.

---

## CodeGen

![CodeGen editing a project, with the RustCLR run output and live runtime counters](docs/images/codegen-main.png)

A three-pane workspace: file explorer, editor with syntax highlighting and line
numbers, and the assistant on the right. The status bar carries the thing this
project is actually about — what the runtime did on the last run:
`IL 4,812 · HEAP 3.1 KB · GC 0`.

![Jack changing code in response to a request, showing which tools he used](docs/images/codegen-chat.png)

Jack is not a chat window bolted onto an editor. He has tools: he reads and
writes files in the open project, edits precise blocks rather than rewriting
whole files, builds, runs on either runtime, disassembles IL, searches the web,
and does arithmetic. The line under each reply lists the tools he actually
called.

Four providers, one interface — **OpenAI**, **Claude**, **Gemini** and
**Ollama**. Everything goes through Semantic Kernel, so the assistant's tools
work identically whichever you pick.

![The New Project dialog, showing templates with a live preview of what will be created](docs/images/codegen-new-project.png)

Fourteen templates spanning console, web, desktop, mobile, IoT and library
projects, across business, science, education and games. Templates marked *runs
on RustCLR* stay inside the IL subset the runtime executes today.

![Settings, with every value stored in app.config](docs/images/codegen-settings.png)

Every setting lives in `app.config` and is editable here — model, key, endpoint,
temperature, system prompt, toolchain paths, editor preferences and layout.
There is no second configuration store.

---

## Speed

The same assembly on both runtimes. Best of three runs, wall clock including
process start; .NET built with tiered compilation off so the JIT runs at full
speed.

| Workload | .NET | RustCLR | Ratio |
| --- | ---: | ---: | ---: |
| Process start | 108 ms | **62 ms** | **0.6x** |
| Exceptions (50k throws) | 130 ms | **111 ms** | **0.9x** |
| Integer kernels — *compiled* | 149 ms | **268 ms** | **1.8x** |
| Strings (20k concats) | 96 ms | **190 ms** | **2.0x** |
| The same kernels via helpers — *compiled, inlined* | 106 ms | **310 ms** | **2.9x** |
| Recursion (fib 27) | 108 ms | 442 ms | 4.1x |
| Allocation (300k objects) | 112 ms | 1,014 ms | 9.1x |
| Sieve (1M) | 107 ms | 1,287 ms | 12.0x |
| Matrix multiply (120 squared) | 110 ms | 1,470 ms | 13.4x |
| Virtual calls (2M) | 115 ms | 1,788 ms | 15.5x |
| Quicksort (200k) | 122 ms | 2,460 ms | 20.2x |
| Field access (3M) | 111 ms | 2,943 ms | 26.5x |

Subtract the process-start row before drawing conclusions: it is most of .NET's
figure on the shorter workloads, so the *compute* ratio is worse than the wall
clock suggests, closer to 100x on the tightest loops. That is the cost of
interpreting rather than compiling, and it is what [Milestone 4](Plan.md) is for.

Several rows go the other way, for three different reasons. **RustCLR starts in
little over half the time** (no JIT, no warm-up), which matters for short-lived
CLI tools and for microcontrollers with no room for a code cache. **Exceptions
and strings stay close** because that work happens in native Rust inside
RustBCL, not in interpreted IL. And the two **kernel rows are compiled to
machine code** rather than interpreted at all — the second of them only because
the inliner splices its helpers in first.

Every row's checksum is compared between the runtimes before it is timed; a
mismatch prints `MISMATCH` instead of a number.

```bash
cd benchmarks && bash run.sh
```

Detail: [docs/benchmarks.md](docs/benchmarks.md).

---

## Install

```bash
./packaging/build.sh              # for this machine
./packaging/build.sh linux-arm64  # or a Raspberry Pi, win-x64, osx-arm64
```

Then, from the unpacked package:

```bash
./install.sh          # Linux and macOS, into ~/.local, no root needed
install.ps1           # Windows, into %LOCALAPPDATA%, no elevation needed
```

Both take `--uninstall`. A package carries the toolchain, a self-contained
CodeGen, the docs and the samples: everything except the .NET SDK, which you
still need to *compile* C#.

Full guide: [docs/installation.md](docs/installation.md).

---

## Getting started

**Prerequisites:** Rust 1.85+ and the .NET SDK 10.

```bash
# Build the runtime and toolchain
cargo build --release

# Compile a C# program with the .NET SDK, then run it on RustCLR
cd tests/fixtures/HelloWorld
dotnet build -c Release
../../../target/release/rustnet run bin/Release/net10.0/HelloWorld.dll
```

```bash
# Launch the IDE
dotnet run --project src/CodeGen
```

Add an API key under **Settings → Providers** to wake Jack up. Ollama needs no
key; point it at your local server and pick a model.

Full walkthrough: [docs/getting-started.md](docs/getting-started.md).

---

## What is in the box

| Component | What it is |
| --- | --- |
| **RustCLR** | The runtime: garbage collector, type system, assembly loader, IL execution engine, exception handling |
| **RustBCL** | The base class library, implemented natively in Rust — `Console`, `String`, `Math`, `Convert`, `StringBuilder`, `Array`, `GC`, and more |
| **RustNet Toolchain** | `rustnet` — build, run, inspect, disassemble and verify assemblies |
| **Interop Bridge** | P/Invoke into native libraries, with marshalling and safe handle wrappers |
| **CodeGen** | The Avalonia IDE and its assistant |

Eight Rust crates:

```
rustclr-metadata   PE/COFF and ECMA-335 reader
rustclr-gc         Managed heap, pluggable collectors
rustclr-core       Type system, loader, IL interpreter
rustclr-bcl        Native base class library
rustclr-sched      Lock-free queue, channels, thread pool
rustclr-interop    P/Invoke and marshalling
rustclr-jit        Compilation interface and IL verifier
rustnet-cli        The toolchain binary
```

Architecture in detail: [docs/architecture.md](docs/architecture.md).

---

## The toolchain

```bash
rustnet run <assembly> [--stats] [--trace]   # execute on RustCLR
rustnet info <assembly> [--verbose]          # metadata summary
rustnet disasm <assembly> [filter]           # disassemble to IL
rustnet verify <assembly>                    # report what will not resolve
rustnet build [project] [--run]              # compile, then run here
rustnet capabilities                         # what this runtime implements
```

`verify` is the one to reach for first when porting: it names every member a
program references that RustCLR cannot yet supply, before you run it.

Reference: [docs/cli.md](docs/cli.md).

---

## What works, and what does not

Being straight about this is more useful than a feature list.

**Works.** Classes, interfaces, inheritance, virtual and interface dispatch,
value types, enums, delegates (unicast and multicast), arrays, strings with
correct UTF-16 semantics, boxing, casting, `try`/`catch`/`finally`, static
constructors, P/Invoke, and a garbage collector that handles cycles.

**Modern C# works too.** String interpolation, tuples and deconstruction, ranges
and indices (`a[^1]`, `a[1..4]`), nullable value types, init-only properties,
records, pattern matching, switch expressions, target-typed `new`, local
functions and `out` variables. `tests/fixtures/ModernSyntax/` exercises 35 of
them and reports `failures=0` on both runtimes.

**Collections and LINQ.** `List<T>`, `Dictionary<K,V>`, `HashSet<T>`,
`Queue<T>` and `Stack<T>` are implemented natively, and so is LINQ — about forty
`Enumerable` operators including `GroupBy` and `OrderBy`/`ThenBy`.

```csharp
var totals = orders
    .Where(o => o.Paid)
    .GroupBy(o => o.Region)
    .OrderBy(g => g.Key)
    .ToDictionary(g => g.Key, g => g.Sum(o => o.Amount));
```

That runs, byte-identically to .NET. So does `foreach` over `IEnumerable<T>`,
over a `yield return` iterator, and over any type implementing the enumerator
pattern.

Two caveats, both stated by `rustnet capabilities`: **LINQ is eager**, not lazy,
so side effects in a predicate happen at the call rather than at consumption;
and ordering compares numbers and strings, refusing any other key type rather
than sorting it arbitrarily.

**async and await.** `Task`, `Task<T>`, `TaskCompletionSource`, `Task.Run`,
`Task.WhenAll` and the awaiter pattern are implemented, so `async` methods run —
including an exception thrown across an `await` and caught by the caller.

```csharp
static async Task<int> Chain(int n)
{
    int a = await Doubled(n);
    int b = await Doubled(a);
    return a + b;
}
```

An `async` method is not special to the runtime: Roslyn lowers it to an ordinary
struct plus calls into a *builder*, and implementing that builder is the whole of
`await`. The caveat is the same one `Thread` carries — **there is no overlap**. A
task runs to completion where it is created, so results and ordering are correct
but nothing runs in parallel.

**Reflection works on real `Type` objects.** `typeof(T)`, `GetType()`, base
types, `IsAssignableFrom`, member enumeration, `MethodInfo.Invoke`, `FieldInfo`
get and set, and `Activator.CreateInstance`. Type objects are interned one per
runtime type, so `typeof(int) == typeof(int)` is reference equality. Custom
attributes are decoded too — constructor arguments, named fields and named
properties. `typeof(T)` on an *erased* generic parameter throws rather than
answering `System.Object`.

**Some methods are compiled to machine code.** `rustclr-jit` emits x86-64 into
write-xor-execute pages for methods doing integer arithmetic, after 32 calls
have shown them to be worth compiling. On the `kernels` benchmark that is
**11.0× faster** than interpreting — 269 ms against 2,971 ms, which is 1.8× .NET
rather than 20×.

**Small callees are inlined**, so a `call` no longer disqualifies a method. The
`inlined` benchmark is the same arithmetic as `kernels` but factored into
helpers, the way real code is written: 1,629 ms interpreted, 400 ms compiled,
of which **2.9× is the inliner alone** — with `--no-inline` the same run takes
1,148 ms because the method around the helpers stays interpreted. Only
branch-free static callees are spliced, one level deep.

The reach is still narrow and `rustnet jit <assembly>` says exactly how narrow:
anything using arrays, allocation or exception handling is interpreted.
`rustnet run --no-jit` interprets everything and `rustnet run --no-inline`
compiles without splicing; both must print the same bytes — there are
differential tests that assert it.

**AArch64 and RISC-V backends exist and have never been executed.** They encode
the same IL through the same shared translation as x86-64 and are checked by
disassembling their output, but no compiled method has ever run on either. Only
x86-64 is dispatched to at runtime.

**Advanced C# features.** 13 of 21 probed features produce identical output on
both runtimes: garbage collection, `IDisposable`/`using`, `async`/`await`,
threading with `lock` and `Interlocked` (both serialised — see below), primary
constructors, collection expressions over arrays, extension members, P/Invoke,
pattern matching, records, LINQ, source generators and interceptors.

The remaining gaps are `Span<T>` and struct marshalling, which need generic type
arguments the runtime has erased; TPL and `await using`, which are simply
unimplemented; and unsafe pointers, which structural managed references cannot
express. Union types and closed hierarchies are not in .NET 10 at all — the
compiler parses them but the BCL types they need do not exist yet.

The full matrix, with why each row lands where it does:
[docs/advanced-features.md](docs/advanced-features.md).


**Does not work yet.** Nothing runs concurrently: `async` tasks and `Thread`
bodies both execute inline, so results are right but there is no parallelism.
Generic type *arguments* are erased, so user generic code that reads `T` at run
time — `typeof(T)`, `is T`, a static field per instantiation — does not behave
correctly, and custom comparers are ignored. Exception filters (`catch when`)
are not evaluated. The native code generator takes only
integer methods — 11.0× faster where it applies, and inlining lets it accept
calls to small helpers, but it declines anything using arrays, which is most of
a real program.

`rustnet capabilities` prints this list from the runtime itself, so it cannot
drift from reality. Detail: [docs/limitations.md](docs/limitations.md).

---

## Targets

The metadata reader recognises x86, x64, Arm, Arm64, RISC-V 32 and RISC-V 64.
The **whole runtime builds without `std`** — `rustclr-metadata`, `rustclr-gc`,
`rustclr-core` and `rustclr-bcl` — for `thumbv7em-none-eabihf`,
`thumbv6m-none-eabi`, `riscv32imc-unknown-none-elf` and
`riscv64gc-unknown-none-elf`. `bash tests/embedded.sh` checks all sixteen
combinations.

**C# runs on a microcontroller.** On an ESP32-C3 — RISC-V, 400 KB of SRAM, no
operating system — the loader builds a type registry, RustBCL registers all 766
of its native bindings, and the interpreter executes `HelloWorld.Main`:

```
-- il interpreter --
heap budget      294912 bytes
bcl tier         full (260702 bytes needed)
native bindings  766

--- program output ---
Hello from RustCLR
42
120
--- end ---
il executed      68
calls            6
```

Those three lines are byte-identical to what `dotnet HelloWorld.dll` prints on
a desktop, CRLF included, and so are the counters — 68 IL instructions and 6
calls on x86-64 and on RISC-V alike. Capture:
[ESP32-C3, executing](docs/logs/esp32c3-interpreter.log).

Three things had to change, and each is a real difference rather than a
polyfill: maps become `BTreeMap` (every key the runtime uses is already `Ord`),
`Arc` becomes `Rc` (RISC-V `imc` has no atomics extension, and the interpreter
is single-threaded on a chip anyway), and float maths comes from `libm` —
the only external dependency anywhere in the runtime, optional, and absent from
a default build. Only the filesystem was irreducible: `load_from_file` is gated
on `std`, and an assembly on a chip arrives as bytes.

**How much of RustBCL fits depends on the board.** Peak allocation is 260,702
bytes with every binding, or 192,045 with console, strings and maths only —
measured, not estimated. Each firmware picks from its heap budget, and a board
that clears neither says so in a line of text instead of dying inside the
allocator.

| Board | Core | RAM | Tier | State |
| --- | --- | ---: | --- | --- |
| [ESP32-C3](embedded/esp32-demo) | RISC-V 32 | 400 K | full | **executes IL on hardware** |
| [ESP32-WROOM-32](embedded/esp32-demo) | Xtensa LX6 | 520 K | full | builds; last flashed pre-interpreter |
| [Meadow F7](embedded/meadow-f7) | Cortex-M7 | 384 K | full | builds; last flashed pre-interpreter |
| [Maix Go K210](embedded/k210) | RISC-V 64 | 6 M | full | builds; never flashed — no board |
| [Pico](embedded/rp2040) | Cortex-M0+ | 256 K | minimal | builds; never flashed — no board |

All five share one demonstration ([embedded/demo-common](embedded/demo-common))
and `bash tests/firmware.sh` builds them. Only the first row has run on
hardware since the interpreter landed; earlier metadata-and-GC captures:
[Xtensa](docs/logs/esp32-wroom32.log) · [RISC-V](docs/logs/esp32c3.log) ·
[Arm](docs/logs/meadow-f7.log).

`Heap::embedded(n)` is a hard ceiling rather than a hint: allocation past it
fails instead of growing, which is the only kind of bound worth having on a
device whose RAM was budgeted up front. The `embedded` Cargo profile builds for
size.

---

## Documentation

- [Getting started](docs/getting-started.md) · [Bahasa Indonesia](docs/id/memulai.md)
- [Installation](docs/installation.md) · [Bahasa Indonesia](docs/id/instalasi.md)
- [Benchmarks](docs/benchmarks.md)
- [Advanced C# features](docs/advanced-features.md) · [Bahasa Indonesia](docs/id/fitur-lanjutan.md)
- [Architecture](docs/architecture.md)
- [The runtime in depth](docs/runtime.md)
- [CodeGen guide](docs/codegen.md) · [Bahasa Indonesia](docs/id/codegen.md)
- [Toolchain reference](docs/cli.md)
- [Templates](docs/templates.md)
- [Limitations](docs/limitations.md)
- [Roadmap](Plan.md) · [Progress](Progress.md)

---

## Contributing

The tests are the contract. `cargo test --workspace` must stay green, and the
conformance fixture must keep reporting `failures=0` on both runtimes. When you
add a runtime capability, add a check to `tests/fixtures/Conformance/Program.cs`
that fails without it.

Screenshots in this README are generated, not captured by hand:

```bash
dotnet run --project src/CodeGen -c Release -- --screenshot docs/images
```

---

## Credits

Built by **Gravicode Studios**, led by **Kang Fadhil**.

MIT licensed.
