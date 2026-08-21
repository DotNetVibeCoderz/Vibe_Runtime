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

**21 of 21 probes produce identical output on both runtimes.**

Identical *output* is what a probe can check, and for a while that was worth a
warning here: `async`, threading and the TPL all produced .NET's answers without
any concurrency at all, and a probe measuring wall-clock overlap would have
failed three of them. That is no longer true — threads spawn, `Task.Run` starts
elsewhere, `Parallel.*` splits across cores and `await` suspends — and the
conformance fixture now includes checks whose answers *do* depend on work really
running at once. What remains missing is named in the notes below rather than in
this number.

### Asynchronous and parallel programming

| Feature | RustCLR | Why |
| --- | --- | --- |
| `async` / `await` | ✅ | `await` on a pending task suspends and returns; the completing thread resumes it. See the note below |
| `Task`, `Task<T>`, `WhenAll`, `TaskCompletionSource` | ✅ | Results, ordering and exception propagation match .NET |
| Task Parallel Library | ✅ **real** | `Task.Run` starts on another thread; `Parallel.For`/`ForEach`/`Invoke` split across cores; `WaitAll` waits |
| Threading, `lock`, `Interlocked` | ✅ **real threads** | `Thread.Start` spawns; `lock` excludes; `Interlocked` does not lose updates. See the note below |

### Memory and resource management

| Feature | RustCLR | Why |
| --- | --- | --- |
| Garbage collection | ✅ | Mark-sweep, handles cycles, pluggable |
| `IDisposable` / `using` | ✅ | Interface dispatch finds the concrete `Dispose` |
| `IAsyncDisposable` / `await using` | ✅ | Works, with `ValueTask` underneath. Disposal runs after the body |
| `Span<T>`, `Memory<T>` | ✅ | Over an array, a string or `stackalloc`. The element width for raw memory comes from the call site's `TypeSpec` |

### Modern language features

| Feature | RustCLR | Why |
| --- | --- | --- |
| Primary constructors (C# 12) | ✅ | Compile to an ordinary constructor and fields |
| Collection expressions — arrays | ✅ | `[1, 2, 3]` is `newarr` plus `InitializeArray` |
| Collection expressions — spread | ✅ | `[..a, b]` lowers through `Span<T>` over an array |
| Collection expressions — spans | ✅ | Through `RuntimeHelpers.CreateSpan` |
| Extension members (C# 14) | ✅ | Static methods with a receiver parameter |
| Interceptors | ✅ | Compile-time rewriting; the runtime sees ordinary IL |
| Union types | **not in .NET 10** | The compiler parses it; `System.Runtime.CompilerServices.IUnion` does not exist |
| Closed hierarchies | **not in .NET 10** | Same — `IsClosedTypeAttribute` is missing from the BCL |
| Extension indexers | ⚠️ | Part of C# 14 extension members; the property and method forms are verified, indexers are not |

### Advanced interop

| Feature | RustCLR | Why |
| --- | --- | --- |
| P/Invoke | ✅ | Real dynamic loading; the probe reads its own process id |
| Type marshalling | ✅ | Blittable structs round-trip. `AllocHGlobal` uses the managed heap, so the pointer cannot go to native code |
| Unsafe code, pointers | ✅ | `stackalloc`, `fixed`, arithmetic, comparison and dereference all run. A pointer is a buffer plus a byte offset, not an address |

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

`Task<T>` turned out to need nothing from erasure either — a task carries its
result as a runtime value, so one `Task` type serves every `T`. What remains
blocked is what genuinely needs the argument at run time: `Span<T>` is a ref
struct the runtime would have to model, and `Marshal.SizeOf<T>` needs a layout
for a `T` it does not have.

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

## Threads, tasks and `await` are real

`Thread.Start()` spawns an OS thread and `Join()` waits for it. Four threads
incrementing one static reach four thousand, `lock` genuinely excludes, and a
consumer can block on a flag set by a producer started *afterwards* — the case
that hangs forever under serialisation. Those four are checks in the
conformance fixture, because each has an answer that differs if the threads do
not really overlap.

**How it works.** A spawned thread gets a *worker* interpreter: the same heap,
the same static storage, the same native bindings, its own frame stack. It
allocates into the same object graph and a collection stops it like any other
mutator.

The part worth understanding is the loader. It is not shared and not locked —
each thread gets an identical **copy**. That works because a loader is finished
before the first instruction runs: types are registered, tokens are resolved,
and closed generic constructions are built eagerly at load. A copy taken
afterwards has the same `TypeId`s and `MethodId`s as the original, so two
threads reading their own tables behave exactly as if they shared one, and
neither pays a lock on the path that runs every instruction. Static storage is
the exception and is genuinely shared, because `static int Total` must be one
slot.

**`await` suspends.** An `async` method that awaits a pending task copies its
state machine to the heap, queues it on that task and *returns* — so the caller
gets a pending task back, and whichever thread completes the awaited one runs
the continuation. Two tasks started and then awaited genuinely overlap.

What that does *not* do is make anything concurrent by itself: awaiting in a
loop is sequential here exactly as it is on .NET, because that is what awaiting
in a loop means. Where the work starts is what decides.

**There is a pool.** `Task.Run` and `Parallel.*` queue onto one worker per core,
each holding its own long-lived interpreter, and a thread waiting on a task runs
queued work rather than idling — which is what stops a task that awaits another
task from deadlocking the pool. `Task.Delay` arms a timer instead of sleeping.
`Thread.Start` still gets a dedicated thread, as `Thread` should.

**The bug this found is worth recording.** A thread waiting in `Join` announces
itself blocked so the collector does not wait for it — and the first version
contributed *none of its roots* while it was away. A blocked thread was assumed
to hold no references, which is true of a thread parked at a safe point and
false of one sitting in `Join` holding the array of threads it is joining. A
collection swept it, and an array with four elements came back with zero.
Blocked threads now hand their roots over on the way out.

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
