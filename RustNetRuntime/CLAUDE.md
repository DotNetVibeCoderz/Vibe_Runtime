# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Status: milestones 1-3 and 5 complete, 4 and 6 partly done, and it works

The runtime executes real Roslyn-compiled assemblies. `cargo test --workspace` runs 141 tests;
the conformance fixture prints `checks=134 failures=0` on RustCLR, byte-identical to `dotnet`,
and `ModernSyntax` prints `checks=35 failures=0`. Generic collections, LINQ and
`async`/`await` all run, hot integer methods compile to x86-64 machine code, and reflection works on real
`System.Type` objects.
CodeGen builds and runs. Not a git repository.

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
cargo test --workspace                     # 141 tests; must stay green
bash tests/embedded.sh                     # bare-metal builds; must stay green
cargo test -p rustclr-core                 # one crate
cargo test -p rustclr-gc a_cycle_is        # one test by substring

# CodeGen
dotnet build src/CodeGen -c Release
dotnet run --project src/CodeGen

# Screenshots for README and docs — re-run after any UI change
dotnet run --project src/CodeGen -c Release -- --screenshot docs/images
```

### The check that matters

Before claiming a runtime change works, run a real assembly through both runtimes and diff:

```bash
cd tests/fixtures/Conformance && dotnet build -c Release
dotnet bin/Release/net10.0/Conformance.dll                    # expect: checks=134 failures=0
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
  exception filters all produce a clear error instead of a plausible-looking wrong answer.
- **Templates marked `RunsOnRustClr = true` must actually run there.** Generic collections
  and LINQ landed with Milestone 2 and `async`/`await` with Milestone 3, so all three are
  fair game; `Span<T>`, TPL and exception filters are not.
