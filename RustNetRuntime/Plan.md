# Plan

The roadmap for RustNetRuntime. Milestones 1 to 3 are done; everything below
them is ordered by what unblocks the most real programs.

---

## Milestone 1 — Execute real C# ✅

**Goal:** take an assembly Roslyn produced and run it correctly.

Done. `tests/fixtures/Conformance/` reports `failures=0` on RustCLR, identical
to `dotnet`, and has grown with every capability since.

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

## Milestone 2 — Generics ✅

**Why first:** almost every non-trivial C# program uses `List<T>` or
`Dictionary<K,V>`, and until this landed that was the single largest gap
between "runs a test program" and "runs real code".

Done. `tests/fixtures/Conformance/` reports `checks=80 failures=0` on RustCLR,
identical to `dotnet`; the advanced-feature matrix went from 10 of 21 probes to
12 of 21, gaining records and LINQ.

| Delivered | |
| --- | --- |
| Generic collections | `List<T>`, `Dictionary<K,V>`, `HashSet<T>`, `Queue<T>`, `Stack<T>`, `KeyValuePair<K,V>` |
| Enumeration | `foreach` over any of them, over `IEnumerable<T>`, and over user iterators |
| LINQ | ~40 `Enumerable` operators, including `GroupBy` and `OrderBy`/`ThenBy` |
| Comparers | `EqualityComparer<T>.Default` and `Comparer<T>.Default` — which is what makes records work |
| Nested type names | `List`1+Enumerator` resolves instead of colliding with every other `Enumerator` |
| `constrained.` on reference types | ECMA-335 III.2.1's other half, which every `foreach` needs |
| Interior pointers into value types | `ByRef::StructField`, so `p.X` where `p` is a struct local has an address |

**The approach, and what it does not do.** Generic *types* are still erased:
`List<int>` and `List<string>` share one `RuntimeType`. That is sound for the
collections because their storage is `Value`, which already carries its own
shape — an `I32` slot and an `Obj` slot are distinguishable without a type
argument. What it does *not* give is user-written generic code that depends on
`T` at run time: `default(T)`, `typeof(T)`, or a generic type with a static
field per instantiation. Real instantiation — one `RuntimeType` per closed
construction — is still the eventual answer, and is now a smaller job than it
was, because the collections no longer depend on it.

Also still open: custom `IEqualityComparer<T>`/`IComparer<T>` arguments are
accepted and ignored, LINQ is eager rather than lazy, and ordering compares
numbers and strings only. Each is stated in `rustnet capabilities` and
`docs/limitations.md`.

---

## Milestone 3 — Tasks and async ✅

Done for the language feature. `tests/fixtures/Conformance/` reports
`checks=90 failures=0` on RustCLR, identical to `dotnet`, and the
advanced-feature matrix went from 12 of 21 probes to 13, gaining `async-await`.

| Delivered | |
| --- | --- |
| `Task`, `Task<T>` | Status, result, exception, continuations — all on the traced heap |
| The builders | `AsyncTaskMethodBuilder`, its generic and void and `ValueTask` forms |
| Awaiters | `TaskAwaiter`, `ConfiguredTaskAwaitable`, `YieldAwaitable` |
| Statics | `Run`, `Delay`, `Yield`, `FromResult`, `CompletedTask`, `WhenAll`, `WhenAny`, `Wait` |
| `TaskCompletionSource` | Including a genuine suspend and resume |

**An `async` method is not special to the runtime.** Roslyn lowers it to an
ordinary struct plus calls into a builder; implementing that builder is the
whole of `await` support, and the state machine is IL the interpreter already
ran. Suspension copies the machine into a one-field heap cell and resumes it
through a managed pointer — which only works because `ByRef::StructField` and
the `MethodImpl` table landed in Milestone 2.

**Asynchrony is synchronous.** There is one interpreter thread, so a task runs
to completion where it is created. Results, ordering and exception propagation
match .NET exactly; what is absent is *overlap*. This is the same honesty as
`Thread`, and for the same reason.

Still open, and the reason this milestone is not the end of the story:

- **Real concurrency.** `rustclr-sched` has the substrate — a lock-free run
  queue, channels, a thread pool, all tested. What is missing is a re-entrant
  interpreter several OS threads can drive at once. Until then `Task.Run` and
  `Thread.Start` both run their body inline.
- **TPL**: `Parallel.For`, `Parallel.ForEach`.
- **`IAsyncDisposable` / `await using`**, and `IAsyncEnumerable<T>`.

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
