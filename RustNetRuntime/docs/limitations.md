# Limitations

What RustCLR does not do yet, and why. Being straight about this is more useful
than a feature list — and `rustnet capabilities` prints the same information
from the runtime itself, so it cannot drift from the code.

---

## Generic type arguments are erased

**What happens:** a generic type is loaded as its open definition and every type
argument becomes `object`. `List<int>` and `List<string>` are the same runtime
type.

**What still works — nearly everything you would reach for.** The collections
and LINQ are implemented natively, and erasure costs them nothing, because their
storage is a managed array of runtime *values* and a value already carries its
own shape. One implementation serves every `T`, and a `List<int>` holds unboxed
integers.

```csharp
var totals = orders
    .Where(o => o.Paid)
    .GroupBy(o => o.Region)
    .OrderBy(g => g.Key)
    .ToDictionary(g => g.Key, g => g.Sum(o => o.Amount));
```

That runs. So do `foreach` over `IEnumerable<T>`, over a user type with an
iterator method, and over anything else that implements the enumerator pattern.
So do records, which need `EqualityComparer<T>.Default` for their generated
`Equals`.

**What breaks:** user-written generic code that needs `T` at run time. Each row
below was measured on both runtimes, not inferred.

| | |
| --- | --- |
| `typeof(T)` | Yields an opaque token, so `.Name` and `.ToString()` do not report the type |
| `value is T` inside a generic method | Always false — there is no argument to test against |
| A static field on a generic type | One slot shared by every instantiation. `Box<int>.Count` and `Box<string>.Count` are the same field |
| `where T : new()` construction | `Activator.CreateInstance` is not implemented |
| A custom `IEqualityComparer<T>` or `IComparer<T>` argument | Accepted and ignored; the default is used |

`default(T)` *does* work, for both value and reference type arguments — the
compiler emits `initobj` against the erased slot, and the zero it produces is
the right one.

**Why it is like this:** instantiating generics properly means one runtime type
per closed construction, a shared open definition, and generic virtual dispatch.
It remains the eventual answer — but it is a smaller job now than it was, since
nothing in the collections depends on it.

---

## LINQ is eager, not lazy

**What happens:** every `Enumerable` operator materialises its result
immediately rather than returning an iterator that runs as it is consumed.

**What breaks:** three things, and only these three.

| | |
| --- | --- |
| Side-effect timing | A `Console.WriteLine` inside a predicate runs at the LINQ call, not at the `foreach` that consumes it |
| Infinite sequences | `Enumerable.Range(0, int.MaxValue).Where(…).First()` never returns; on .NET it stops at the first match |
| Mutating the source afterwards | The result was taken as a snapshot and does not see the change |

For everything else the results are identical, which is what the conformance
suite checks.

**Ordering** compares numbers and strings. A key of any other type is *refused*
with a clear error rather than ordered arbitrarily — a silently wrong sort is
much harder to notice than a failed one.

---

## async and await run, but nothing overlaps

**What happens:** `async`/`await` works. `Task`, `Task<T>`,
`TaskCompletionSource`, `Task.Run`, `Task.Delay`, `Task.WhenAll` and the awaiter
pattern are all implemented, and an async method's results, ordering and
exception propagation match .NET exactly.

What is absent is *concurrency*. There is one interpreter thread, so a task runs
to completion at the point it is created: `Task.Run` invokes its delegate
immediately and `Task.Delay` sleeps.

**What breaks:** code that depends on two tasks making progress at the same
time.

| | |
| --- | --- |
| Awaiting a task started earlier so the two overlap | The first ran to completion before the second was created |
| A producer/consumer pair joined by a `TaskCompletionSource` the producer completes *later* | Works — that path genuinely suspends and resumes |
| `Parallel.For`, `Parallel.ForEach` | Not implemented |
| `IAsyncDisposable` / `await using`, `IAsyncEnumerable<T>` | Not implemented |
| Wall-clock speedup from parallelism | There is none; the work is serialised |

**Why it is like this:** `rustclr-sched` already has the substrate — a lock-free
run queue, channels and a thread pool, all tested. What is missing is a
re-entrant interpreter several OS threads could drive at once. Running tasks
inline makes far more programs produce the right answer than refusing `Task`
would, so it is offered with the limitation stated here, in
`rustnet capabilities`, and in the source.

The same reasoning, and the same caveat, applies to `Thread`: `Thread.Start`
runs the body on the calling thread and `Join` returns at once. See
[advanced-features.md](advanced-features.md#threads-are-serialised).

---

## Exception filters are not evaluated

**What happens:** `catch (Exception e) when (e.Message.Contains("x"))` is treated
as non-matching. The exception passes it by rather than being caught.

**Why it is like this:** a filter runs managed code during the first unwind pass,
before the stack is unwound. That needs a re-entrant execution mode mid-dispatch.
Treating filters as non-matching lets the exception escape to an outer handler —
noisy, but correct — rather than swallowing it, which would be silently wrong.

`try`/`catch`/`finally` without a filter works, including nesting, rethrow, and
`finally` blocks running during unwind.

---

## There is no native code generator

**What happens:** every method is interpreted. `rustclr-jit` provides the
`Compiler` trait, the IL verifier and basic-block analysis, but
`InterpreterTier` is the only implementation and it reports every method as
interpreted.

**What this costs:** roughly 1.8 million IL instructions per second on the
conformance suite. Fine for scripts, tools and IoT control loops; not fine for
anything numerically heavy.

The tiering design already accounts for a partial backend —
`Compiler::can_compile` decides per method, so the first emitter only has to
handle leaf integer methods to be useful. This is [Milestone 4](../Plan.md).

---

## Reflection is minimal

`GetType()` returns a type *name*, not a `System.Type` object. There is no
member enumeration, no attribute reading, no `Activator.CreateInstance`.

`ldtoken` pushes the raw metadata token, which is enough for
`RuntimeHelpers.InitializeArray` — the mechanism behind `new int[] { 1, 2, 3 }` —
but not for anything that inspects types at run time.

This is [Milestone 5](../Plan.md).

---

## Unimplemented IL instructions

These are decoded and then reported as unsupported, rather than being silently
skipped:

`localloc` · `cpblk` · `initblk` · `arglist` · `mkrefany` · `refanyval` ·
`refanytype` · `calli` · `jmp`

Multi-dimensional arrays parse but only single-dimension zero-based arrays are
allocated and indexed.

---

## Interop constraints

**Argument count.** Up to six. Raising it means adding arms to the dispatch
table, because each arity needs its own concrete function type.

**Mixed argument kinds.** A call taking both integers and floats is refused. The
table does not model per-position ABI slots, and mis-ordering registers is
undefined behaviour — an error is strictly better than a guess.

**Strings marshal as UTF-8 `char*`.** Wide-string marshalling is not
implemented. A declaration expecting `wchar_t*` will receive the wrong encoding,
so it is refused rather than mis-encoded.

**Structs do not cross the boundary.** Only strings and blittable primitives.

---

## Embedded targets are designed for, not proven

The core crates are written to be `no_std`-friendly, the collector has an
`embedded` profile, the Cargo `embedded` profile optimises for size, and the
metadata reader recognises RISC-V and Arm machine types.

**Nothing has been flashed to real hardware.** Until the core crates build for
`thumbv7em-none-eabihf` and `riscv32imc-unknown-none-elf` without `std`, and the
`iot-gateway` template runs on an actual ESP32, this is a design intent rather
than a claim. This is [Milestone 6](../Plan.md).

---

## CodeGen

**Format Code** is a brace-depth reindenter, not a C# formatter. It fixes
indentation and trailing whitespace; it does not wrap lines, sort usings or
normalise spacing. Anything more needs Roslyn.

**Chat streaming** is not incremental for Claude. The tool loop needs a complete
message before it can run a tool, so the panel shows a working indicator rather
than partial text.

**Image attachments** are passed as file paths, not inlined bitmaps. Jack's
tools read from disk.

**Mobile templates** produce touch-first Avalonia layouts that run on the
desktop. Deploying to Android or iOS needs the Avalonia mobile heads added to
the project — the template says so.

---

## How to find out for yourself

```bash
rustnet capabilities              # what the runtime implements
rustnet verify <assembly>         # what your program would hit
```

`verify` is the honest answer for any specific program: it names every framework
member referenced but not implemented, before you run anything.

For the advanced language and framework features specifically — async/await,
`Span<T>`, primary constructors, collection expressions, source generators,
threading — [advanced-features.md](advanced-features.md) has a feature-by-feature
matrix, each row produced by running a probe on both runtimes and comparing.
[Bahasa Indonesia](id/fitur-lanjutan.md).
