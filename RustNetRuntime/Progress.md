# Progress

Tracking for RustNetRuntime. Updated 2026-08-20.

**Verified this session:** `cargo test --workspace` → 141 passed, 0 failed.
Five programs — two conformance suites, a sample, and two written or edited by
the assistant against a live LLM — produce byte-identical output on RustCLR and
.NET.

```
Conformance    IDENTICAL — checks=134 failures=0
ModernSyntax   IDENTICAL — checks=35 failures=0
UserDirectory  IDENTICAL
PrimeSieve     IDENTICAL   (written by Jack from a prompt)
SensorGateway  IDENTICAL   (edited by Jack from a prompt)
```

---

## Done — Rust runtime

| Crate | State | Evidence |
| --- | --- | --- |
| `rustclr-metadata` | PE/COFF + ECMA-335 reader, signatures, IL bodies | 27 tests, incl. 9 against a real Roslyn-built assembly |
| `rustclr-gc` | Handle-based heap, pluggable collectors, mark-sweep | 10 tests; 200k-deep graph marks without stack overflow |
| `rustclr-core` | Type system, loader, IL interpreter, exception handling | 18 tests |
| `rustclr-bcl` | Native BCL: Console, String, Math, interpolation, tuples, ranges, Nullable, generic collections, LINQ, Task and async, reflection | 20 unit + 8 integration; 757 bindings |
| `rustclr-sched` | Lock-free MS queue, MPMC channel, thread pool | 16 tests |
| `rustclr-interop` | P/Invoke, dynamic loading, marshalling | 9 tests; calls the real `GetCurrentProcessId` |
| `rustclr-jit` | Compiler trait, IL verifier, analysis, **x86-64 code generator**, W^X pages, tiering | 25 unit + 3 differential |
| `rustnet-cli` | `run` / `info` / `disasm` / `verify` / `build` / `capabilities` | Drives every fixture |

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

**IL did not execute there**, and that distinction matters. The chip can read a
.NET assembly and manage a heap; it cannot yet run a method, because
`rustclr-core` still needs `std`.

Two bugs stood between "compiles" and "runs": the metadata crate's `use alloc::…`
reached only `lib.rs`, and `Image` was gated on `std` in its entirety when only
`from_file` and `path()` need a filesystem.

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
covers: 2484 ms interpreted, 232 ms compiled — **10.7× faster**, and 1.6× .NET
rather than 17.5×.

**What it does not cover, and why that matters.** Running `rustnet jit` on the
existing benchmark suite compiles *nothing*: every workload there uses arrays or
calls. That is a real finding, not a footnote — the backend's reach is narrow,
and the honest way to show it was to add a workload of the covered shape beside
the others rather than in place of them.

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

Ordered as in [Plan.md](Plan.md):

1. **Generic types** — still erased to `object`. The collections and LINQ work
   anyway, because their storage is self-describing; what erasure still costs is
   user-written generic code that reads `T` at run time (`default(T)`,
   `typeof(T)`, per-instantiation statics), and custom comparers.
2. **Real concurrency** — `async`/`await` and `Thread` both run, but neither
   overlaps: a task runs to completion where it is created and `Thread.Start`
   runs its body inline. `rustclr-sched` has the substrate; what is missing is a
   re-entrant interpreter several OS threads can drive at once. TPL
   (`Parallel.For`) and `await using` are unimplemented.
3. **Native code generation beyond leaf integer methods** — the x86-64 backend
   exists and is 10.7× faster where it applies, but it declines anything using
   arrays, calls, allocation or exception handling, which is most of a real
   program. Arrays are the next piece; AArch64, RISC-V and inlining are untouched.
4. **Reflection breadth** — `PropertyInfo` accessors, `MethodInfo` parameter
   lists, `Assembly`/`Module` enumeration, and constructing generic types at run
   time. Types, members, invocation and attributes all work.
5. **IL execution on hardware** — the metadata reader and collector run on an
   ESP32; the interpreter does not, because `rustclr-core` still needs a hash
   map, a clock and file IO. Ahead-of-time compilation additionally needs Arm
   and RISC-V backends.
6. **Exception filters** — `catch when` is treated as non-matching.

`rustnet capabilities` prints this from the runtime itself, and
`rustnet verify <assembly>` names what a specific program would hit.

### The advanced-feature matrix is measured, not asserted

`tests/fixtures/AdvancedFeatures/` is 21 single-feature probes plus a real
incremental source generator. `probe.sh` runs each on both runtimes and diffs —
**13 of 21 pass on RustCLR today**, and the failures print the runtime's own
error rather than a guess. Results and reasoning:
[docs/advanced-features.md](docs/advanced-features.md) ·
[Bahasa Indonesia](docs/id/fitur-lanjutan.md).

Two findings from that run were not obvious beforehand:

- **Source generators and interceptors need no runtime support at all.** Both
  are Roslyn-side; the runtime only ever sees ordinary IL. Proven with a
  generator that actually intercepts a call, not by reasoning about it.
- **`Thread` works, but serialised.** `Thread.Start` runs the body on the
  calling thread and `Join` returns immediately. Start-then-join programs are
  correct; anything needing two threads to make progress together hangs. Said
  in exactly those words in `capabilities`, because "supported" would be a lie
  and "unsupported" would be wrong.

Also measured: **union types and closed hierarchies do not exist in .NET 10** —
`IUnion` and `IsClosedTypeAttribute` are absent from the BCL, so there is
nothing for any runtime to support yet.

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
- **Unsupported interop shapes are refused, not guessed.**
- **Strings are `Vec<u16>`.** .NET defines `Length` and indexing in UTF-16 code
  units; UTF-8 would make them wrong or O(n).
- **Collection only happens where every live value is rooted.** Between
  instructions, before an allocation, before a return value is popped.
