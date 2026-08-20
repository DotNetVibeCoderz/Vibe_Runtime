# Advanced C# features on RustCLR

Which of C#'s advanced features run on RustCLR, measured rather than claimed.

*[Bahasa Indonesia →](id/fitur-lanjutan.md)*

Every row below comes from `tests/fixtures/AdvancedFeatures/`, which exercises
each feature in its own process and compares RustCLR's output with .NET's. A
feature counts as supported only when the two produce **identical output** —
running without crashing is not enough, because a wrong answer is worse than a
clear failure.

```bash
cd tests/fixtures/AdvancedFeatures
dotnet build -c Release
bash probe.sh
```

---

## The matrix

**10 of 21 probes produce identical output on both runtimes.**

### Asynchronous and parallel programming

| Feature | RustCLR | Why |
| --- | --- | --- |
| `async` / `await` | ❌ | The state machine needs `AsyncTaskMethodBuilder<T>` and `TaskAwaiter<T>` |
| Task Parallel Library | ❌ | `Task<T>`, `Parallel.For` over generic delegates |
| Threading, `lock`, `Interlocked` | ⚠️ **works, serialised** | See the note below |

### Memory and resource management

| Feature | RustCLR | Why |
| --- | --- | --- |
| Garbage collection | ✅ | Mark-sweep, handles cycles, pluggable |
| `IDisposable` / `using` | ✅ | Interface dispatch finds the concrete `Dispose` |
| `IAsyncDisposable` / `await using` | ❌ | Needs `async` |
| `Span<T>`, `Memory<T>` | ❌ | Generic ref structs, and `stackalloc` needs `localloc` |

### Modern language features

| Feature | RustCLR | Why |
| --- | --- | --- |
| Primary constructors (C# 12) | ✅ | Compile to an ordinary constructor and fields |
| Collection expressions — arrays | ✅ | `[1, 2, 3]` is `newarr` plus `InitializeArray` |
| Collection expressions — spread | ❌ | `[..a, b]` is lowered through `Span<T>` |
| Collection expressions — spans | ❌ | `ReadOnlySpan<char> x = ['a']` needs `Span<T>` |
| Extension members (C# 14) | ✅ | Static methods with a receiver parameter |
| Interceptors | ✅ | Compile-time rewriting; the runtime sees ordinary IL |
| Union types | **not in .NET 10** | The compiler parses it; `System.Runtime.CompilerServices.IUnion` does not exist |
| Closed hierarchies | **not in .NET 10** | Same — `IsClosedTypeAttribute` is missing from the BCL |
| Extension indexers | ⚠️ | Part of C# 14 extension members; the property and method forms are verified, indexers are not |

### Advanced interop

| Feature | RustCLR | Why |
| --- | --- | --- |
| P/Invoke | ✅ | Real dynamic loading; the probe reads its own process id |
| Type marshalling | ❌ | `Marshal.SizeOf<T>` / `PtrToStructure<T>` are generic |
| Unsafe code, pointers | ❌ | Managed pointers here are structural and have no address |

### High-level abstractions

| Feature | RustCLR | Why |
| --- | --- | --- |
| LINQ | ❌ | `IEnumerable<T>`, `Where`, `Select` — all generic |
| Pattern matching, switch expressions | ✅ | Type, relational, logical and property patterns |
| Records | ❌ | The generated `Equals` uses `EqualityComparer<T>` |
| Source generators | ✅ | Compile-time; the runtime sees ordinary IL |

---

## One cause behind almost every ❌

Nine of the eleven failures come from a single gap: **generic types are erased
rather than instantiated.** `Span<T>`, `Task<T>`, `List<T>`,
`EqualityComparer<T>` and `IEnumerable<T>` are all generic, so anything built on
them cannot resolve.

That is [Milestone 2](../Plan.md), and it is the highest-value work remaining —
it is not eleven separate problems, it is one problem with eleven symptoms.

Generic **methods** already work: an instantiation binds by its type argument,
which is what made string interpolation, tuples, ranges and `Nullable<T>`
possible. Generic **types** are the piece still missing.

---

## Threads are serialised

`Thread.Start()` runs the delegate **synchronously on the calling thread**, and
`Join()` returns immediately because the work is already done. `lock` is a no-op
for the same reason: with no concurrent execution there is nothing to exclude.

This is correct for the common start-then-join shape, and for code that uses
threads to organise work rather than to gain parallelism. It is **wrong** for a
program that depends on two threads running at the same time — a consumer
blocking on a producer started afterwards will hang.

The alternative was to refuse `Thread` outright. Serialising it makes more
programs run, so it is offered with this limitation stated here, in
`rustnet capabilities`, and in the source, rather than left to be discovered.

`rustclr-sched` already has the real substrate — a lock-free run queue, channels
and a thread pool, all tested. What is missing is a re-entrant interpreter that
several OS threads could drive at once. That arrives with
[Milestone 3](../Plan.md).

---

## Compile-time features need no runtime support

**Source generators** and **interceptors** work, and the reason is worth being
explicit about: both run inside the compiler. By the time RustCLR sees the
assembly, generated code and rewritten call sites are ordinary IL.

The fixture proves this rather than assuming it. `Generator/` is a real
incremental generator that emits a class whose contents are computed from the
compilation, and separately rewrites one call site with
`InterceptsLocationAttribute`. Both probes pass on RustCLR with the same output
as .NET.

The practical consequence: any library built on source generation — many
serialisers, mappers and DI containers — has a good chance of running, provided
what it *generates* stays inside the supported subset.

---

## Two features do not exist yet anywhere

**Union types** and **closed hierarchies** are C# proposals. The compiler in
.NET 10 parses their syntax, but the runtime types they need
(`IUnion`, `IsClosedTypeAttribute`) are not in the BCL, so they fail to compile
on .NET itself:

```
error CS0518: Predefined type 'System.Runtime.CompilerServices.IUnion' is not defined
error CS0656: Missing compiler required member 'IsClosedTypeAttribute..ctor'
```

They cannot be supported by any runtime until the BCL ships them.

---

## Checking your own program

```bash
dotnet build -c Release
rustnet verify bin/Release/net10.0/YourApp.dll
```

`verify` names every member your program references that RustCLR cannot supply,
and every method whose IL fails verification, before you run anything. A line
reading `<generic instantiation>` is the gap described above.
