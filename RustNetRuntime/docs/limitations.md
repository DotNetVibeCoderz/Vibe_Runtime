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

## The native code generator only takes leaf integer methods

**What happens:** most methods are still interpreted. The x86-64 backend
compiles a method only when it makes no calls, allocates nothing, has no
exception handling, and works entirely in integers — arguments, locals, results
and every intermediate value.

That is narrow on purpose: a partial backend that is honest about its reach is
useful immediately, and everything it declines runs exactly as it did before.

```bash
rustnet jit <assembly>       # what compiles, and why the rest does not
rustnet run --no-jit …       # interpret everything; output must be identical
rustnet run --no-inline …    # compile, but splice nothing; output must match too
```

**What it is worth where it applies.** The `kernels` benchmark — integer
arithmetic written out longhand — runs in 2971 ms interpreted and 269 ms
compiled: **11.0× faster**, which moves it from 20× slower than .NET to 1.8×.
The `inlined` benchmark is the same arithmetic factored into small helpers:
1629 ms interpreted, 400 ms compiled, **4.1×** — of which **2.9× is the inliner
alone**, since with `--no-inline` the same run takes 1148 ms.

**What it does not reach.** Every other workload in the benchmark suite. They
use arrays and calls, so `rustnet jit` compiles none of them and the figures are
unchanged. The immediate blocker is arrays: handles here are not pointers, so
reading `a[i]` from machine code means resolving a handle through the handle
table, which needs a call back into the runtime and therefore a calling
convention the backend does not have yet.

**Inlining is one level deep and branch-free.** A `call` no longer disqualifies
a method: a small static callee is spliced into its caller, which is often the
difference between a real method compiling and being declined. But the callee
must contain no branches at all — a helper with an `if` in it is not inlined —
and the splice is not applied recursively, so a helper that itself calls another
helper is left alone. Instance methods are never inlined. `--no-inline` turns
the whole thing off, and the output must be identical either way.

**The AArch64 and RISC-V backends emit code that has never been executed.**
Both encode the same IL the x86-64 backend does, through the same shared
translation, and both are checked by disassembling their output and reading it.
Neither has ever run a compiled method: this host is x86-64, and only the
x86-64 backend is dispatched to at runtime. Treat them as reviewed encoders,
not as working backends — an unexecuted backend that claims to work is worse
than no backend at all.

**Code memory is write-xor-execute.** A page is mapped readable and writable,
filled, and only then flipped to readable and executable. It is never both at
once. In an environment that forbids executable mappings outright, compilation
fails and everything is interpreted.

This is [Milestone 4](../Plan.md).

---

## Reflection works, except for attributes

**What works.** `System.Type` is a real object, interned one per runtime type,
so `typeof(T) == typeof(T)` is reference equality as .NET guarantees.

```csharp
Type t = value.GetType();
Console.WriteLine(t.Name + " : " + t.BaseType.Name);
foreach (FieldInfo f in t.GetFields()) Console.WriteLine(f.Name);
MethodInfo m = t.GetMethod("Compute");
object result = m.Invoke(value, new object[] { 21 });
object made = Activator.CreateInstance(typeof(Widget));
```

All of that runs: names and namespaces, base types, the `IsValueType` /
`IsClass` / `IsInterface` / `IsEnum` / `IsArray` / `IsPrimitive` / `IsAbstract` /
`IsSealed` family, `IsAssignableFrom`, `IsInstanceOfType`, member enumeration,
`MethodInfo.Invoke`, `FieldInfo` get and set, and `Activator.CreateInstance`. A
boxed value reports the type it holds rather than `System.Object`.

**Custom attributes are decoded**, including constructor arguments, named
fields and named properties:

```csharp
var mark = (MarkAttribute)typeof(Widget).GetCustomAttributes(typeof(MarkAttribute), false)[0];
Console.WriteLine(mark.Text + " " + mark.Order);
```

An argument this runtime cannot decode — an array, a `Type`, a boxed object —
omits that attribute from the result rather than building it with an invented
value. "Not found" is an answer a caller can act on; a wrong value is not.

**`typeof(T)` on a generic parameter throws.** The type argument was erased, so
there is no type to name. The two honest options were `System.Object` — a
plausible-looking wrong answer that nobody would notice — or a clear
`NotSupportedException`. It throws, and the message says why. The same applies
to `Activator.CreateInstance<T>()`; pass the type explicitly instead.

**Not implemented:** `PropertyInfo` accessors, `MethodInfo` parameter lists,
`Assembly` and `Module` enumeration, and constructing generic types at run time.

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

## Embedded: the interpreter runs on hardware, within a memory budget

**C# executes on a microcontroller.** On an ESP32-C3 (RISC-V, 400 KB SRAM,
no operating system) the loader builds a type registry, RustBCL registers all
766 of its native bindings, and the interpreter runs `HelloWorld.Main`:

```
-- il interpreter --
heap budget      294912 bytes
bcl tier         full (260702 bytes needed)
native bindings  766
types registered 204

--- program output ---
Hello from RustCLR
42
120
--- end ---
exit code        0
il executed      68
calls            6
```

Those three lines are byte-identical to `dotnet HelloWorld.dll` on a desktop,
CRLF included, and so are the counters: 68 IL instructions and 6 calls on
x86-64 and on RISC-V alike. Full capture:
[ESP32-C3, executing](logs/esp32c3-interpreter.log). Earlier metadata-and-GC
captures: [Xtensa](logs/esp32-wroom32.log) · [RISC-V](logs/esp32c3.log) ·
[Arm](logs/meadow-f7.log).

**The whole runtime builds without `std`** — `rustclr-metadata`, `rustclr-gc`,
`rustclr-core` and `rustclr-bcl`, for `thumbv7em-none-eabihf`,
`thumbv6m-none-eabi`, `riscv32imc-unknown-none-elf` and
`riscv64gc-unknown-none-elf`. `bash tests/embedded.sh` checks all sixteen
combinations. Three things had to change and each is a real difference, not a
polyfill:

* **Maps.** `HashMap` becomes `BTreeMap` without `std`. Every key the runtime
  uses — integer ids, tuples of them, names — is already `Ord`, so this costs
  an iteration order that nothing depends on, and avoids pulling a hasher onto
  a microcontroller.
* **`Arc` becomes `Rc`.** RISC-V `imc` — the ESP32-C3's core — has no atomics
  extension, so `Arc` does not exist on that target. The interpreter is
  single-threaded on a chip, which is exactly when `Rc` is correct anyway.
* **Float maths comes from `libm`.** `core` has no `sqrt`, no `sin`, not even
  `abs` for `f64`, and `System.Math` is largely a libm. This is the only
  external dependency anywhere in the runtime, it is optional, and a default
  (`std`) build does not pull it in.

Only the filesystem was irreducible: `Loader::load_from_file` is gated on
`std`, and without it an assembly arrives as bytes.

**How much of RustBCL fits depends on the board**, and the firmware decides
from a measured number rather than a guess. Peak allocation to load the runtime
and run a program is **260,702 bytes** with every binding, or **192,045** with
console, strings and maths only. `Tier::for_budget` compares those against the
board's heap and picks; a board that clears neither says so in a line of text
instead of dying inside the allocator.

| Board | Core | RAM | Heap given | Tier | State |
| --- | --- | ---: | ---: | --- | --- |
| ESP32-C3 | RISC-V 32 | 400 K | 288 K | full | **executes IL on hardware** |
| ESP32-WROOM-32 | Xtensa LX6 | 520 K | 176 K + 96 K | full | builds; last flashed pre-interpreter |
| Meadow F7 Micro | Arm Cortex-M7 | 384 K | 288 K | full | builds; last flashed pre-interpreter |
| Sipeed Maix Go K210 | RISC-V 64 | 6 M | 1 M | full | **builds; never flashed** — no board |
| Raspberry Pi Pico | Arm Cortex-M0+ | 256 K | 192 K | minimal | **builds; never flashed** — no board |

Only the first row has been run on hardware since the interpreter landed. The
rest are builds, and are worth reading as exactly that.

**The WROOM-32 needs two heap regions to get there**, which is the most
instructive thing in that table. Its main `dram_seg` tops out at 176 KB — found
by bisecting until the link succeeded — which is below even the reduced binding
set. The ESP32 has a second bank of 98,768 bytes past the ROM's data and stacks
that the linker will not place ordinary statics in; `esp-alloc` accepts regions
rather than one arena, so the firmware adds both. A single allocation still
cannot span them, and the largest the runtime makes is 67,584 bytes, which
clears either.

**The heap is a static array, so the linker enforces the budget.** Asking the
C3 for 320 KB produces `.bss will not fit in region DRAM, overflowed by 13844
bytes` at link time rather than a hard fault at run time. That is the argument
for sizing it statically.

`Heap::embedded(n)` is a **hard ceiling**: `try_alloc` returns `None` when full
rather than growing past the budget. On a device whose RAM was allocated up
front, a heap that quietly grows has not been bounded at all.

**What does not run on hardware.**

| | |
| --- | --- |
| IL execution | `rustclr-core` needs a hash map, a clock and a way to read an assembly |
| Ahead-of-time compilation | Needs Arm and RISC-V code generators; only x86-64 exists |
| The Pico and K210 | Their images build; neither has been flashed |

That first row is the honest boundary: the chip can *read* a .NET assembly and
manage a heap, but it cannot execute a method. This is
[Milestone 6](../Plan.md).

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
