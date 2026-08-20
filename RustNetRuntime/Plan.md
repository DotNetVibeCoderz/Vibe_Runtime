# Plan

The roadmap for RustNetRuntime. Milestone 1 is done; everything below it is
ordered by what unblocks the most real programs.

---

## Milestone 1 — Execute real C# ✅

**Goal:** take an assembly Roslyn produced and run it correctly.

Done. `tests/fixtures/Conformance/` reports `checks=37 failures=0` on RustCLR,
identical to `dotnet`. 108 tests pass across the workspace.

| Delivered | |
| --- | --- |
| PE/COFF and ECMA-335 metadata reader | signatures, heaps, tables, method bodies |
| Managed heap | handle table, mark-sweep, pinning, swappable collectors |
| Type system and loader | types, methods, fields, vtables, interfaces |
| IL interpreter | ~180 opcodes, iterative frame stack |
| Exception handling | `try`/`catch`/`finally`, nested, `leave`/`endfinally` |
| Native BCL | Console, String, Math, Convert, StringBuilder, Array, GC, Random, time |
| P/Invoke | dynamic loading, marshalling, up to six arguments |
| Toolchain | `run`, `info`, `disasm`, `verify`, `build`, `capabilities` |
| CodeGen | three-pane IDE, four LLM providers, 14 templates |

---

## Milestone 2 — Generics

**Why first:** almost every non-trivial C# program uses `List<T>` or
`Dictionary<K,V>`. Generics are erased to `object` today, which is the single
largest gap between "runs a test program" and "runs real code".

- Instantiate generic types rather than erasing them: one `RuntimeType` per
  closed construction, with a shared open definition.
- Resolve `MethodSpec` tokens to instantiated methods.
- Generic virtual dispatch and constrained calls on value types.
- Then implement `List<T>`, `Dictionary<K,V>`, `Queue<T>`, `Stack<T>` and
  `IEnumerable<T>` in RustBCL.

**Done when:** a conformance program using generic collections and `foreach`
over `IEnumerable<T>` reports `failures=0`.

---

## Milestone 3 — Tasks and async

The scheduler exists (`rustclr-sched`: lock-free queue, channels, thread pool);
nothing drives managed state machines through it yet.

- Recognise compiler-generated `IAsyncStateMachine` types.
- `Task`, `Task<T>`, `TaskCompletionSource` and the awaiter pattern in RustBCL.
- Drive `MoveNext` from the thread pool; continuations onto the run queue.
- `Thread`, `Monitor` (`lock`), `Interlocked`.

**Done when:** a program that awaits several tasks and joins their results
produces the same output on both runtimes.

---

## Milestone 4 — Native code generation

`rustclr-jit` already supplies the compilation interface, the IL verifier and
basic-block analysis. What is missing is a backend.

- x86-64 emitter for leaf integer methods, with W^X code pages.
- Tiering: interpret, then compile on call count.
- AArch64, then RISC-V.
- Inlining, using the existing `is_inline_candidate` analysis.

Interpretation stays the fallback: a partial backend must be useful, so
`Compiler::can_compile` decides per method.

---

## Milestone 5 — Reflection and metadata at run time

- `System.Type` as a real object rather than a name string.
- `GetType()`, `typeof`, member enumeration, attribute reading.
- `Activator.CreateInstance`.

---

## Milestone 6 — Embedded targets

The pieces are in place — `no_std`-friendly crates, an `embedded` collector
profile, an `embedded` Cargo profile, RISC-V and thumb targets recognised — but
nothing has been run on real hardware.

- Build the core crates for `thumbv7em-none-eabihf` and
  `riscv32imc-unknown-none-elf` without `std`.
- A fixed-size heap with no allocator dependency.
- Ahead-of-time compilation, since a JIT needs writable code pages.
- Flash and run the `iot-gateway` template on an ESP32 and an STM32.

---

## Milestone 7 — Exception filters and the remaining IL

- Evaluate `catch when` filters during the first unwind pass.
- `localloc`, `cpblk`, `initblk`, `calli`, `arglist`.
- Multi-dimensional arrays with non-zero lower bounds.

---

## Ongoing

**Conformance.** Every capability added gets a check in the fixture that fails
without it. The suite is the definition of "works".

**Honesty about gaps.** `rustnet capabilities` prints what is implemented
directly from the runtime, and `rustnet verify` names what a given assembly
would hit. Neither is allowed to drift from the code.

**CodeGen.** Track the runtime: as the supported IL subset grows, the templates
marked *runs on RustCLR* grow with it.
