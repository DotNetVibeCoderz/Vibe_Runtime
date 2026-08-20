# Plan

The roadmap for RustNetRuntime. Milestones 1-3 and 5 are done; 4 and 6 are
partly done. What remains is ordered by what unblocks the most real programs.

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

Done. The conformance fixture grew from 38 checks to 80 and stayed at
`failures=0`, identical to `dotnet`; the advanced-feature matrix went from 10 of
21 probes to 12, gaining records and LINQ.

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

Done for the language feature. The conformance fixture grew from 80 checks to
90 and stayed at `failures=0`, identical to `dotnet`; the advanced-feature
matrix went from 12 of 21 probes to 13, gaining `async-await`.

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

## Milestone 4 — Native code generation ◐

Half done, and useful now. The x86-64 emitter and the tiering policy exist; the
other two architectures and inlining do not.

| | |
| --- | --- |
| x86-64 emitter for leaf integer methods | ✅ |
| W^X code pages | ✅ mapped RW, filled, then flipped to RX — never both |
| Tiering: interpret, then compile on call count | ✅ default 32 calls |
| AArch64, then RISC-V | ❌ |
| Inlining, using `is_inline_candidate` | ❌ the analysis exists; nothing consumes it |

**What it compiles.** Integer arithmetic, comparison, branching, arguments and
locals, in methods that make no calls, allocate nothing and have no exception
handling. `rustnet jit <assembly>` lists what is taken and why the rest is not.

**What it is worth.** On the `kernels` benchmark — the shape the backend covers
— 2484 ms interpreted, 232 ms compiled: **10.7× faster**, and 1.6× .NET rather
than 17.5×. On the other benchmark workloads it changes nothing, because they
use arrays and calls and are declined; measuring that was the point of adding
`kernels` beside them rather than instead of them.

**The next step is arrays**, and it is a real one. Handles are not pointers, so
reading `a[i]` from machine code means resolving a handle through the handle
table — which needs a call back into the runtime, and therefore a calling
convention the backend does not yet have. That single piece unblocks most of the
existing benchmark suite.

Interpretation stays the fallback: a partial backend must be useful, so
`Compiler::can_compile` decides per method, and `--no-jit` must always produce
identical output. There is a differential test that asserts exactly that.

---

## Milestone 5 — Reflection and metadata at run time ✅

| | |
| --- | --- |
| `System.Type` as a real object | ✅ interned one per runtime type, so `typeof(T) == typeof(T)` is reference equality |
| `GetType()`, `typeof` | ✅ including boxed values, which report the type they hold |
| Member enumeration | ✅ `GetMethods`, `GetFields`, `GetMethod`, `GetField`, `MethodInfo.Invoke`, `FieldInfo` get/set |
| `Activator.CreateInstance` | ✅ by `Type`; the generic form is refused, see below |
| Attribute reading | ✅ constructor arguments, named fields and named properties |

**`typeof(T)` on a generic parameter is refused, not answered.** The type
argument was erased, so the honest options were `System.Object` — a
plausible-looking wrong answer — or a clear `NotSupportedException`. It throws,
and the message says why. The same applies to `Activator.CreateInstance<T>()`.

**Attributes are decoded on demand, not at load.** Building an instance means
running its constructor, and nothing can run while an assembly is still loading —
so the blob is kept and decoded the first time someone asks. An argument shape
this runtime cannot read — an array, a `Type`, a boxed object — omits that
attribute from the result rather than constructing it with an invented value:
"not found" is an answer a caller can act on, a wrong value is not.

Still absent: `PropertyInfo` accessors, `MethodInfo` parameter lists, `Assembly`
and `Module` enumeration, and constructing generic types at run time.

---

## Milestone 6 — Embedded targets ◐

**It runs on real hardware, on three architectures.** Verified on an
ESP32-WROOM-32 (Xtensa LX6), an ESP32-C3 (RISC-V) and a Meadow F7 Micro
(STM32F777, Arm Cortex-M7). The metadata and collector output is
**byte-identical** on all three:
[Xtensa](docs/logs/esp32-wroom32.log) · [RISC-V](docs/logs/esp32c3.log) ·
[Arm](docs/logs/meadow-f7.log).

| | |
| --- | --- |
| Core crates build without `std` | ✅ `rustclr-metadata` and `rustclr-gc`, four upstream targets plus Xtensa |
| A fixed-size heap | ✅ `Heap::embedded(n)` is a hard ceiling, not a hint |
| Flash and run on an ESP32 | ✅ Xtensa **and** RISC-V |
| Flash and run on an STM32 | ✅ Meadow F7 Micro, over USB DFU |
| `rustclr-core` without `std` | ❌ needs a hash map, a clock and file IO |
| Ahead-of-time compilation | ❌ blocked on Arm and RISC-V backends, which do not exist |

On each chip, `rustclr-metadata` read a Roslyn-built `HelloWorld.dll` out of
flash — PE header, CLI header, metadata tables, string heap — and reported its
assembly name, table counts, entry point and declared types. Then `rustclr-gc`
allocated a three-node ring, kept it alive while rooted, **collected it once
unrooted** (`live=0`, so cycles really are reclaimed), detected a stale handle,
and filled the heap to exactly its 128-slot ceiling before refusing to grow.

```
assembly         HelloWorld
metadata version v4.0.30319
entry point      Main
cycle unrooted   live=0
refused past it  true
```

**Difficulty was inversely related to how upstream the target is.** RISC-V and
Arm are both upstream Rust targets: stable toolchain, prebuilt `core` and
`alloc`, no `build-std`. Xtensa needed the forked toolchain from `espup`. The
Meadow's own obstacle was different again — its crystal frequency is published
nowhere, and USB will not enumerate without it. The
[RustNet Meadow F7 port](../RustNet/runtime/firmware-meadow-f7) had already
established it (25 MHz, Abracon ABM12W-25 at X401) by sweeping candidates and
letting a host adjudicate; this firmware uses that answer and prints what it
actually locked to, so the claim stays checkable.

```bash
bash tests/embedded.sh                 # four upstream targets, build only
cd embedded/meadow-f7 && cargo build --release      # STM32, stable
cd embedded/esp32-demo
# ESP32-C3, stable Rust:
cargo build --release --no-default-features       --features esp32c3 --target riscv32imc-unknown-none-elf
# ESP32-WROOM-32, forked toolchain:
cargo +esp build --release --no-default-features       --features esp32 --target xtensa-esp32-none-elf -Z build-std=core,alloc
```

**What still does not run on hardware is IL.** `rustclr-core` is the
interpreter, and it needs a hash map, a clock and a way to read an assembly.
Until that lands, a chip can *read* a .NET assembly and manage a heap, but not
execute a method. Ahead-of-time compilation needs Arm and RISC-V code
generators, which [Milestone 4](#milestone-4--native-code-generation-) has not
written — and the Arm one now has an obvious first customer.

**Two bugs stood between "compiles" and "runs".** `rustclr-metadata` did not
actually build without `std` — its `use alloc::…` sat in `lib.rs` and reached no
submodule. And `Image` was gated on `std` in its entirety when only `from_file`
and `path()` need a filesystem; everything else works from a byte slice, which
is exactly what reading an assembly out of flash requires.

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
