# Progress

Tracking for RustNetRuntime. Updated 2026-08-20.

**Verified this session:** `cargo test --workspace` → 111 passed, 0 failed.
Five programs — two conformance suites, a sample, and two written or edited by
the assistant against a live LLM — produce byte-identical output on RustCLR and
.NET.

```
Conformance    IDENTICAL — checks=38 failures=0
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
| `rustclr-bcl` | Native BCL: Console, String, Math, interpolation, tuples, ranges, Nullable | 9 unit + 8 integration; 317 bindings |
| `rustclr-sched` | Lock-free MS queue, MPMC channel, thread pool | 16 tests |
| `rustclr-interop` | P/Invoke, dynamic loading, marshalling | 9 tests; calls the real `GetCurrentProcessId` |
| `rustclr-jit` | Compiler trait, IL verifier, basic-block analysis | 9 tests |
| `rustnet-cli` | `run` / `info` / `disasm` / `verify` / `build` / `capabilities` | Drives every fixture |

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

Generic *types* are still erased — see [Plan.md](Plan.md) Milestone 2.

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

Two are worth dwelling on.

**The GC safepoint bug** is the classic one: `newobj` allocated, then collected
before anything referenced the new object. The handle design turned what would
be heap corruption in a pointer-based runtime into a clean, diagnosable panic —
but the fix is discipline about *when* collection may run, and there is now a
conformance check that forces a collection mid-construction.

**The verifier crying wolf** was worse than a missing feature. `docs/limitations.md`
tells people to trust `rustnet verify`; a tool that reports problems in correct
code teaches them not to. It now resolves call targets for exact stack effects,
seeds only real handler offsets, stops trusting depth past an unresolved call,
and reports unbindable references — but only those IL actually reaches, so
attribute constructors do not bury the real findings.

---

## Not done

Ordered as in [Plan.md](Plan.md):

1. **Generic types** — still erased to `object`. Generic *methods* now bind by
   type argument, which is what unblocked modern C#, but `List<T>` and LINQ need
   real instantiation.
2. **async/await** — the scheduler exists and is tested; nothing drives managed
   state machines through it.
3. **Native code generation** — `rustclr-jit` has the interface, the verifier
   and the analysis; there is no backend. This is what the benchmark gap is.
4. **Reflection** — `GetType()` returns a name, not a `System.Type`.
5. **Embedded targets** — designed for, never flashed to hardware.
6. **Exception filters** — `catch when` is treated as non-matching.

`rustnet capabilities` prints this from the runtime itself, and
`rustnet verify <assembly>` names what a specific program would hit.

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
