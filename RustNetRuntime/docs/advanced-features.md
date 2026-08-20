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

**13 of 21 probes produce identical output on both runtimes.**

### Asynchronous and parallel programming

| Feature | RustCLR | Why |
| --- | --- | --- |
| `async` / `await` | ⚠️ **works, synchronous** | The builders and awaiters are implemented; see the note below |
| `Task`, `Task<T>`, `WhenAll`, `TaskCompletionSource` | ✅ | Results, ordering and exception propagation match .NET |
| Task Parallel Library | ❌ | `Parallel.For` is unimplemented |
| Threading, `lock`, `Interlocked` | ⚠️ **works, serialised** | See the note below |

### Memory and resource management

| Feature | RustCLR | Why |
| --- | --- | --- |
| Garbage collection | ✅ | Mark-sweep, handles cycles, pluggable |
| `IDisposable` / `using` | ✅ | Interface dispatch finds the concrete `Dispose` |
| `IAsyncDisposable` / `await using` | ❌ | Unimplemented; `async` itself works |
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
| LINQ | ⚠️ **works, eager** | ~40 `Enumerable` operators natively; see the note below |
| Generic collections | ✅ | `List`, `Dictionary`, `HashSet`, `Queue`, `Stack`, natively |
| `foreach` over `IEnumerable<T>` | ✅ | Including `yield return` iterators and user enumerators |
| Pattern matching, switch expressions | ✅ | Type, relational, logical and property patterns |
| Records | ✅ | Needed `EqualityComparer<T>.Default`, which now exists |
| Source generators | ✅ | Compile-time; the runtime sees ordinary IL |

---

## What erasure still costs

Generic type arguments are erased: `List<int>` and `List<string>` are one
runtime type. That used to block everything on this page built on a generic
type. It no longer does, because the collections are implemented natively over
storage that is self-describing — a runtime value already knows whether it holds
an integer or a reference, so one implementation serves every `T`.

What remains blocked is what genuinely needs the argument at run time:
`Span<T>` and `Task<T>` are ref structs and state-machine types the runtime
would have to model, and `Marshal.SizeOf<T>` needs a layout for a `T` it does
not have. Those are [Milestone 3](../Plan.md) and [Milestone 4](../Plan.md)
work, not one blocked milestone.

For user-written generic code, the measured effects of erasure — `typeof(T)`,
`is T`, statics per instantiation — are tabulated in
[limitations.md](limitations.md).

---

## LINQ is eager

Every operator materialises its result at once instead of returning a lazy
iterator. `Where(…).Select(…).ToList()` walks the source twice more than .NET
would, and three behaviours differ: side effects inside a predicate happen at
the LINQ call rather than at consumption; an infinite sequence never terminates;
and a source mutated after the call is not reflected in the result.

Ordering compares numbers and strings. Any other key type is **refused** with a
clear error rather than sorted arbitrarily, and a custom `IComparer<T>` argument
is accepted but ignored — the erased type argument is what a real comparer
implementation would need.

---

## async is synchronous

`await` works, and an async method's results, ordering and exception
propagation match .NET exactly — including an exception thrown across an
`await` and caught by the caller. What does not happen is *overlap*: a task runs
to completion at the point it is created, because there is one interpreter
thread. `Task.Run` invokes its delegate immediately; `Task.Delay` sleeps.

The suspend-and-resume path is real, not bypassed: a `TaskCompletionSource`
completed after its awaiter has suspended genuinely parks the state machine on
the heap and resumes it on completion. That is what the conformance check
`resumed continuation` exercises.

What this costs is any program that depends on two tasks progressing together,
and any wall-clock speedup from parallelism. It arrives with a re-entrant
interpreter — see the note on threads below, which has the same cause.

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
several OS threads could drive at once. That is the one piece both this and
`async` are waiting on.

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
