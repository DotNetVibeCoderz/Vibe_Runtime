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
checks=37 failures=0

$ rustnet run Conformance.dll --stats
checks=37 failures=0

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

The 37 checks cover arithmetic and overflow, `long` maths, control flow,
strings, arrays, class inheritance, virtual and interface dispatch, properties,
structs, enums, boxing and casting, `try`/`catch`/`finally`, nested handlers,
divide-by-zero, and delegates. The suite lives in
`tests/fixtures/Conformance/` — it is a normal C# project you can read and
extend.

`cargo test --workspace` runs 108 tests across the eight crates.

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

## Getting started

**Prerequisites:** Rust 1.85+ and the .NET SDK 9 or 10.

```bash
# Build the runtime and toolchain
cargo build --release

# Compile a C# program with the .NET SDK, then run it on RustCLR
cd tests/fixtures/HelloWorld
dotnet build -c Release
../../../target/release/rustnet run bin/Release/net9.0/HelloWorld.dll
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

**Does not work yet.** Generics are erased to `object` rather than
instantiated. Exception filters (`catch when`) are not evaluated. `async`/`await`
state machines are not driven by the scheduler. Reflection is minimal. There is
no native code generator — `rustclr-jit` supplies the compilation interface and
the IL verifier, and execution is interpreted.

`rustnet capabilities` prints this list from the runtime itself, so it cannot
drift from reality. Detail: [docs/limitations.md](docs/limitations.md).

---

## Targets

The metadata reader recognises x86, x64, Arm, Arm64, RISC-V 32 and RISC-V 64.
The core crates are written to be `no_std`-friendly, and the collector has an
`embedded` profile with a small allocation trigger for microcontroller targets
(ESP32, STM32, RISC-V). The `embedded` Cargo profile builds for size.

---

## Documentation

- [Getting started](docs/getting-started.md) · [Bahasa Indonesia](docs/id/memulai.md)
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
