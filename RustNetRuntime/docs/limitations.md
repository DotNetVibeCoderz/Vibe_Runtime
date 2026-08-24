# Limitations

What RustCLR does not do yet, and why. Being straight about this is more useful
than a feature list — and `rustnet capabilities` prints the same information
from the runtime itself, so it cannot drift from the code.

---

## Generic type arguments are known, for user types and methods

**A user generic type gets one runtime type per closed construction.**
`Cell<int>` and `Cell<string>` are two types. They share one body — generics
are still erased for *execution*, and one compiled method serves every argument
— but each carries its own type arguments and its own static storage:

```csharp
sealed class Cell<T>
{
    public string ArgumentName() => typeof(T).Name;   // "Int32", then "String"
    public bool Accepts(object o) => o is T;          // true for 7, false for "seven"
    public T Empty() => default(T);
}

static class Tally<T> { public static int Count; }    // one slot per construction
```

All of that runs and agrees with .NET. A class type parameter is answered
through the **receiver**: `this` is an instance of `Cell<int>` or of
`Cell<string>`, and those are different runtime types carrying different
arguments.

**Framework generics stay erased, deliberately.** `List<int>` and `List<string>`
remain one runtime type. Every native binding is keyed by its declaring type's
name, so giving `List<int>` a type of its own would put `List`1::Add` out of
reach of the implementation behind it — and nothing in the collections needs `T`
at run time, because their storage is a managed array of runtime *values* and a
value already carries its own shape.

**A generic method** knows its arguments from a different source. Every call
site emits a `MethodSpec` carrying them, and each instantiation records them —
so although `M<int>` and `M<string>` share one body, the body can ask what `T`
was:

```csharp
static string NameOf<T>() => typeof(T).Name;   // "Int32", then "String"
static T Default<T>() => default(T);           // 0, then null
static bool Holds<T>(object o) => o is T;      // true for 5, false for "five"
```

All three run and agree with .NET. What makes it work is that the argument was
never lost for a method — the `MethodSpec` had it all along and the loader was
discarding it.

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
| A framework generic — `List<int>` vs `List<string>` | One runtime type, by choice; see above |
| `GetGenericArguments()` on an **open** definition | .NET returns the type parameter `T`; this runtime has no runtime type for a parameter, so it refuses rather than return the empty array it could |

`MakeGenericType` **works.** It calls the same loader path a `TypeSpec` does and
shares its cache, so `typeof(Cell<>).MakeGenericType(typeof(int))` is the
identical instance as `typeof(Cell<int>)` — reference equality on types stays
reliable, and an object made from a type built at run time runs its methods
normally. `IsGenericType`, `IsGenericTypeDefinition`,
`ContainsGenericParameters` and `GetGenericTypeDefinition` answer alongside it.
The wrong-arity, already-closed and not-generic cases throw what .NET throws.

That row above was found by the fixture, not reasoned about: the check asserted
an open definition had no type arguments, and `dotnet` disagreed by returning
one. The runtime had been quietly answering zero.

Everything else works. `typeof(T)`, `x is T` and `default(T)` answer for both a
class type parameter (through the receiver) and a method one (through the
instantiation), and a static field belongs to its construction. A custom
`IComparer<T>` is no longer ignored either; see the sorting section below.

**The three cases needed different things.** A method instantiation only had to
record what the call site already said — the loader was parsing the arguments to
build a name and then dropping them. A type instantiation needed a runtime type
of its own per closed construction, with fresh static slots and an identity the
receiver could be asked for.

The third, a class type parameter in a **static** method, has no receiver and so
looked like it needed something new. It did not. `Tally<int>.ArgumentName()`
compiles to a `MemberRef` whose owner is the *construction*, and the loader was
already recording that — it had been added so `newobj` on a constructed generic
could know its arguments. Carrying it onto the frame is the whole change, and it
falls back to refusing when a call site genuinely names the definition.

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

## async, await and threads run, and overlap

**What happens:** `async`/`await` works. `Task`, `Task<T>`,
`TaskCompletionSource`, `Task.Run`, `Task.Delay`, `Task.WhenAll` and the awaiter
pattern are all implemented, and an async method's results, ordering and
exception propagation match .NET exactly.

`Task.Run` starts its delegate on another thread, and `await` on a pending task
suspends the async method and returns to its caller. Two tasks started and then
awaited genuinely run at once.

**What breaks:** the rows below, and they are narrower than they were.

| | |
| --- | --- |
| Awaiting tasks started earlier so they overlap | **Works** — `Task.Run` starts on another thread, so several started and then awaited run at once |
| A producer/consumer pair joined by a `TaskCompletionSource` the producer completes *later* | Works — that path genuinely suspends and resumes |
| `Parallel.For`, `Parallel.ForEach`, `Parallel.Invoke` | **Run on several threads**, one chunk each. Iteration order is not preserved, which is the contract a parallel loop already has |
| `Task.WaitAll(a, b)` written in C# | **Works, and genuinely waits.** .NET 10 lowers it through an `InlineArray2<Task>` and a `ReadOnlySpan<Task>`, so it had to learn to read a span — reading only arrays made it wait for nothing at all |
| `IAsyncDisposable` / `await using` | **Works**, including `async ValueTask DisposeAsync()`; disposal runs after the body and an `await` inside it does not reorder that |
| `ValueTask`, `ValueTask<T>` | Work. Represented by the task they stand for, so the allocation `ValueTask` exists to avoid happens anyway — everything observable is the same |
| `IAsyncEnumerable<T>` / `await foreach` | **Work**, including `yield break`, an empty sequence, and `break` out of the loop (which runs the enumerator's `DisposeAsync`) |
| Wall-clock speedup | **Yes**, from `Thread`, `Task.Run` and `Parallel.*`. Not from `await`: see below |

**What overlaps.** `Thread.Start` spawns; `Task.Run` starts its delegate on
another thread; `Parallel.For`, `ForEach` and `Invoke` split their work across
one thread per core, capped by the number of iterations. `lock` excludes and
`Interlocked` does not lose updates. **`await` suspends** — an `async` method
that awaits a pending task returns to its caller, and the thread that completes
the task runs the continuation. Each of those has a conformance check whose
answer differs if the work does not really run at once.

**Where the work starts is still what decides whether it overlaps.** `await` no
longer serialises anything, but it also does not make anything concurrent:

```csharp
var a = Task.Run(Work);   // starts now, on another thread
var b = Task.Run(Work);   // starts now, on another
await a; await b;         // both were already running: overlapped
```

```csharp
foreach (var item in items)
    await ProcessAsync(item);   // sequential, exactly as on .NET
```

The second is sequential on .NET too — that is what awaiting in a loop means.
An `async` method that never reaches `Task.Run` still runs its body on the
calling thread, which is also what .NET does until something yields.

**There is a thread pool.** `Task.Run` and `Parallel.*` queue onto one worker
per core, each holding a long-lived interpreter — copying a loader is the
expensive part of starting a managed thread, so it happens once per worker
rather than once per task. Two thousand tasks is a conformance check.
`Thread.Start` still takes a dedicated thread, which is what `Thread` means.

`Task.Delay` arms a timer and returns a pending task, so
`var d = Task.Delay(500); Work(); await d;` overlaps. One timer thread serves
every outstanding delay.

**`Monitor.Wait` and `Pulse` work**, so a producer/consumer queue built on
`lock`/`Wait`/`Pulse` runs — a conformance check moves 500 items through one.
`Wait` releases the monitor at whatever depth it was held and retakes it at that
depth, because a method that locked three deep and then waits must come back
three deep or its own `finally` blocks release a lock it no longer holds.

`Wait(obj, ms)` honours its timeout and reports whether it was pulsed, and it
comes back holding the monitor either way — a caller whose `finally` released a
lock it never retook would corrupt the count for every other thread. A negative
timeout is `Infinite`, as .NET defines it.

One narrower thing: `Pulse` wakes *every* waiter rather than one. That is a
weaker promise, and a `Wait` written in the re-check loop the API requires
already tolerates it — a spurious wake-up is always possible — whereas waking
one and picking the wrong one would not be.

**The pool grows when its workers block.** A worker that sleeps, waits on a
lock or joins a thread is a worker the pool has lost for the duration, and if
that leaves nobody able to take queued work another is started, up to a ceiling.
Thirty-two tasks that each sleep 400 ms took 1,610 ms before this and 469 ms
after — four rounds against roughly one.

That fix began as a *collector* bug: `Thread.Sleep` slept without announcing
itself blocked, so a collection had to wait out the whole sleep for it, and
several sleeping threads stalled every collection. Announcing it fixed both.

**A thread waiting on a task runs queued work while it waits.** Without that a
pool deadlocks in the familiar way — every worker blocked on a task whose job is
still in the queue behind it — and a task that awaits another task is exactly
that shape. It is also why the pool never needs to grow under load.

### How the runtime runs on several threads

`Thread.Start` spawns. The machinery underneath, with tests in
`crates/rustclr-core/tests/parallel.rs` and checks in the conformance fixture:

| | |
| --- | --- |
| Several OS threads allocating into one heap | Yes — `SharedHeap` serialises on a lock |
| An object made on one thread, read on another | Yes |
| A static written on one thread, read on another | Yes — static storage is shared, everything else in a loader is copied |
| Collecting while other threads run | Yes — they stop at safe points and hand in their roots first |
| A thread blocked in `Join` or on a `lock` | Counts as stopped *and* hands over its roots |
| `lock`, `Interlocked` | Real exclusion; four threads bumping a counter reach the right number |

**The loader is copied, not shared, and that is the whole design.** A `Loader`
is finished before the first instruction runs — types registered, tokens
resolved, closed generic constructions built eagerly at load — so a copy taken
afterwards is *identical*: same `TypeId`s, same `MethodId`s, same tables. Two
threads reading their own copies behave exactly as if they shared one, and
neither pays a lock on the path that runs every instruction. That is what made
this tractable at all: `registry.ty()` is called on nearly every instruction and
hands back a reference, so putting it behind a lock would have cost far more
than the heap's did.

Static storage is the exception, and is genuinely shared behind a lock, because
`static int Total` must be one slot however many threads reach it.

The correctness condition is precise: **the copies must not diverge.** A loader
grows in exactly two places after load — an interface-dispatch stub synthesised
for a native implementation, and `MakeGenericType`. A `MethodId` minted on one
thread means nothing on another, so a thread that grew its registry is detected
on `Join` and reported, rather than being allowed to hand back an id that names
a different method. That check has not fired in practice; it is there because
the alternative to noticing is a wrong answer.

**What is still serialised:** `Task` and `Parallel.For`. They need a scheduler
handing work to these threads and a frame stack per task, and neither is built.

**The measured cost so far**, best of five, against the same binary built before
the heap moved behind a lock:

| workload | before | after | |
| --- | --- | --- | --- |
| `alloc` | 1022 ms | 1059 ms | +4% |
| `fields` | 3021 ms | 3092 ms | +2% |
| `virtual` | 1852 ms | 1893 ms | +2% |
| `arrays` | 411 ms | 418 ms | +2% |
| `strings` | 203 ms | 222 ms | +9% |
| `calls` | 77 ms | 80 ms | *below the noise floor* |

The `calls` row is there for completeness and should not be read as +4%. That
workload runs for well under a tenth of a second, and repeating it later gave
57 ms and 93 ms on an unchanged binary — it cannot resolve a difference that
size. The rows above it run long enough to mean something, and were measured
interleaved against the pre-lock binary in one session, which is what makes
them comparable at all. See [benchmarks.md](benchmarks.md) on drift.

On a microcontroller it costs 1,732 bytes of flash and **no RAM at all** —
`bss` and `data` are byte-identical, because without `std` the lock is a
`RefCell` and the registry compiles away to nothing. An M5Stack Tough runs
`HelloWorld` with the shared heap in place, output identical to `dotnet`.

---

## Exception filters are evaluated

`catch (Exception e) when (e.Message.Contains("x"))` runs its filter during the
unwind and takes the exception when the filter returns 1, exactly as ECMA-335
III.19 describes. Filters run outermost-last in table order, the first to accept
wins, and a filter that throws declines — the exception already in flight keeps
travelling rather than being replaced by one from the code asking about it.

A filter runs in a frame of its own that shares the unwinding frame's method and
arguments and takes a copy of its locals, written back at `endfilter`. That
write-back is what makes `catch (E e) when (Log(ref buffer))` work: the `ref`
points into the filter's copy, and without it the append would vanish and the
filter would look as though it never ran.

**One narrowing remains.** A filter that throws skips the write-back, because its
locals are then in a state nothing has reasoned about.

`try`/`catch`/`finally` works alongside this, including nesting, rethrow, and
`finally` blocks running during unwind.

---

## What the native code generator takes

**What happens:** the x86-64 backend takes a good deal now — `int` and `double`
arithmetic, `int[]`/`double[]`/`bool[]` element access, allocation of both
arrays and objects, field access, guarded devirtualisation, recursion, and
calls between compiled methods. Every benchmark row in this repository compiles
or is native.

**What it still declines**, and each for a stated reason rather than because
nobody got to it:

| | |
| --- | --- |
| Exception handling | a compiled frame cannot be unwound by the interpreter |
| Instance methods | the calling convention carries no receiver |
| `float[]` | four-byte elements would widen on every load and round on every store |
| `rem` on doubles | IEEE remainder, with no single x86 instruction since SSE replaced x87's `fprem` |
| A virtual call through an open hierarchy | another assembly could add an override — see the devirtualisation section |
| A field of a type it cannot lay out | a struct field has no one-word representation |

Everything declined runs exactly as it did before, which is what makes a partial
backend useful rather than dangerous. `rustnet jit <assembly>` prints the
decision for every method, with the reason.

```bash
rustnet jit <assembly>       # what compiles, and why the rest does not
rustnet run --no-jit …       # interpret everything; output must be identical
rustnet run --no-inline …    # compile, but splice nothing; output must match too
```

**What it is worth**, measured by running the same workload with `--no-jit` so
the comparison is the same code either way:

| Workload | interpreted | compiled | speedup |
| --- | ---: | ---: | ---: |
| `arrays` — an `int[]` passed in and walked | 18,401 ms | 205 ms | **89.8×** |
| `fields` — 3M reads and writes of an object's fields | 3,123 ms | 120 ms | **26.0×** |
| `virtual` — 2M calls through a base reference | 1,923 ms | 96 ms | **20.0×** |
| `alloc` — 300k allocations with collection pressure | 1,093 ms | 206 ms | **5.3×** |
| `kernels` — arithmetic written out longhand | 2,971 ms | 269 ms | **11.0×** |
| `inlined` — the same arithmetic via helpers | 1,629 ms | 400 ms | **4.1×** |

Several of these understate the gap rather than flattering it: the compiled
column is close to the process-start floor, so nearly all of what remains is
starting up rather than the loop. Each pair was measured in one session; they
are not all from the same run as the table in
[benchmarks.md](benchmarks.md), and should not be read against it.

`arrays` is the largest because the interpreter pays most for element access:
a handle resolution and a `Value` per element, against a bounds check and a
scaled-index load.
The `inlined` benchmark is the same arithmetic factored into small helpers:
1629 ms interpreted, 400 ms compiled, **4.1×** — of which **2.9× is the inliner
alone**, since with `--no-inline` the same run takes 1148 ms.

**Arrays compile**, as a parameter or as a local. An `int[]`, `double[]` or
`bool[]` is handed to compiled code as a two-word descriptor — a data pointer
and a length — so `a[i]` becomes a bounds check and a scaled-index load rather
than a handle lookup. The `arrays` benchmark runs **89.8× faster** compiled than
interpreted, the largest gap in the suite.

**One compiled method can call another.** The call is indirect, through a table
of entry addresses the runtime fills in before the caller first runs. It has to
be: a callee's address does not exist when its caller is compiled, and two
methods may call each other, so there is no order in which direct addresses
could be baked in.

Each table entry names two things — the callee's entry *and the callee's own
table* — because a callee reads its table out of the argument array its caller
builds. Getting that wrong is not a wrong answer, it is a jump through whatever
the stack happened to hold, and it was wrong twice: once by not writing the
slot at all, and once by filling tables in a way that moved them after another
table had recorded their address. Both now have tests that crash without the
fix.

Compiled calls run on the **machine stack**, which cannot be grown or unwound
the way the interpreter's frame budget can. So the runtime counts the *bytes*
those frames occupy and refuses past a budget rather than overrunning. Bytes
rather than frames is the correction that mattered: frames differ in size, so
any frame count is simultaneously too generous for a large frame and too mean
for a small one, and the count that was there before turned into a segfault the
moment frames grew.

Because managed recursion needs machine stack, the runtime reserves a large one
for managed code rather than hoping the host provided it — a megabyte, the
Windows default, is less than the interpreter's own 4096-frame budget needs, and
a program that ran interpreted must not fail compiled. An embedder calling the
interpreter directly is responsible for the same thing.

What is still refused: an **instance** method as the callee, which this
convention does not carry a receiver for, and any callee the backend would not
compile — settled by shape when the caller is compiled, since compiling the
callee to find out would not terminate when two methods call each other.

**A field of a heap object is reached by a helper call.** This is the one place
the design was decided by what could go *wrong* rather than by what would be
fast. Compiling `ldfld` the obvious way means emitting machine code that reads a
`Value` out of a `Vec<Value>`, which requires knowing that enum's discriminant
and payload offsets. Rust promises neither without `repr`, the enum has boxed
variants, and the failure would be a **wrong number on a toolchain upgrade with
no source change** — the worst failure profile in the whole runtime. It was
refused three times on those grounds.

A call has none of that. The helper does the reading in Rust, and compiled code
sees one 64-bit word: an integer, a double's bit pattern, or a handle. `int`,
`long`, `double` and reference fields cross; a struct field has no one-word
representation and is refused.

The declared kind is **checked**, not merely used to convert, and that is worth
stating because it was wrong first. An early version accepted an integer for any
kind, which meant a reference stored as a number round-tripped perfectly through
compiled code — and was invisible to the collector, which traces a reference and
not an integer, so the chain under it would be swept while compiled code still
walked it. A mutation test caught it: the helper now refuses a shape that does
not match what the field was declared to hold.

Two smaller things fall out. An object allocated by compiled code gets its
fields' **typed zeros** rather than `Value::Null`, because a field declared
`int` must not read back as a null. And its field count is the **flattened**
one, base included, or a subclass instance would be allocated too small for the
slots its own code addresses.

**Compiled code reaches safe points, and may therefore allocate in a loop.**
This was the last structural restriction on the code generator, and it was
there for a real reason: compiled code had no place where it could be stopped,
so a collection could not run while it executed, so a loop that allocated could
exhaust the heap where the interpreter would not.

A compiled method now polls at every loop back-edge. Two things can ask it to
stop, and both are needed: another thread waiting to collect — without which a
long compiled loop would hold up a collection indefinitely — and this call's own
allocation pressure, which with a single thread is the only thing that will ever
ask.

What makes the frame describable at that point is a deliberate narrowing. The
poll goes where the **evaluation stack is empty**, which is where Roslyn puts
its back-edges, so the only slots holding anything are locals and arguments and
their kinds are known from declared types — no inference over the instruction
stream, and no way for that inference to be subtly wrong. A back-edge reached
with something still on the stack gets no safe point at all, and such a method
keeps the old rule.

Two other things fall out of the design. A `Handle` is `repr(transparent)` over
`u64` and this collector never moves an object, so a safe point only has to
*describe* the frame, never rewrite it. And because a reference or array slot
must be readable from the frame, those slots are excluded from the register
pinning that speeds up integer loops — what the collector cannot see, it
reclaims.

The property worth stating plainly, because the first attempt at this got it
wrong: **a compiled loop's memory does not grow with its trip count.** An
earlier version kept every allocation pinned for the length of the call, which
passed every correctness test and was strictly worse than refusing to compile —
a million iterations meant a million live arrays. Each safe point now releases
what the frame can no longer reach. There is a test that runs the same loop ten
times longer and fails if the peak heap follows.

**A constructor that provably does nothing is not called at all.** A class
declaring no constructor still gets one from the C# compiler: load `this`, call
`object`'s constructor, return. Running it costs two managed frames and a native
call to reach an empty body, and the fields it would leave untouched are already
at the zero the allocator gave them. Where the whole chain up to
`System.Object` does nothing, the interpreter allocates and pushes the instance
without entering a frame.

The chain is what decides, not the leaf: an empty constructor over a base that
assigns a field is *not* elided, and neither is one whose class has a field
initialiser, because that compiles into the constructor. The class constructor
still runs — eliding an instance constructor must never elide a static one.
`Loader::constructor_does_nothing` is the single rule, shared with scalar
replacement and with compiled `newobj`, which need the same assurance for the
same reason.

**A virtual call is compiled only when every answer it can give is known.**
Class-hierarchy analysis over the loaded types is normally a guess in this
runtime — `Assembly.Load` can bring in a subtype later — so it is trusted only
where that is impossible: where the declaring type is not visible outside its
own assembly and therefore cannot be derived from outside it. A `private`
nested class qualifies; a `public` one is refused even when it otherwise looks
ideal, and the test suite contains exactly such a type for the purpose of
watching it be turned down. Each override must additionally fold to a constant,
because emitting anything else would need a general calling convention between
compiled methods, which does not exist.

Where all that holds, the call becomes a helper call for the receiver's runtime
type and a chain of compares. No object memory is read, so no heap layout is
assumed; the only thing compiled code holds is a handle, an opaque 64-bit token
it cannot dereference. A null receiver throws `NullReferenceException` as it
should, and a receiver matching no guard **traps** — there is no on-stack
replacement, so a body that has already allocated cannot re-run interpreted,
and a hierarchy proof that turned out to be wrong has to report itself rather
than return a number nobody can account for.

That is a narrow capability, described narrowly on purpose. It is not "the
backend compiles virtual calls".

**Objects that never escape stop existing.** Compiled code does not read fields
out of a heap object, and deliberately never will: fields live in a `Vec<Value>`
and `Value` is a Rust enum with boxed payloads whose layout the language does
not promise, so emitting code that reads it would make that layout part of this
backend's ABI — and the failure mode would be a wrong number, not a crash.
Instead, when an object provably never leaves the method that made it, each
field becomes an ordinary local and `ldfld`/`stfld` become `ldloc`/`stloc`.

The proof is deliberately narrow, because being wrong about it is silent. The
object must be created by a `newobj` outside any loop, have a constructor that
does nothing but chain to `System.Object`'s, derive directly from `Object` with
only scalar fields, live in exactly one local, and be used *only* as the target
of `ldfld`, `stfld`, or a `dup` feeding one. Passing it to a method, returning
it, boxing it, locking on it or taking a field's address all disqualify it, and
anything the analysis cannot follow is refused rather than guessed at. An
object that escapes is reached through the field helper below instead.

That works because of an invariant worth naming: the backend declines any method
that allocates, so no collection can run while compiled code executes, and this
collector never moves an object in any case. Both would have to change together
for the pointer to go stale.

**An array created *inside* the method is still declined.** `new int[n]`
allocates, which is exactly what the invariant above forbids — so `sieve` and
`sort` in the benchmark suite, which allocate their arrays as locals, are
interpreted. That is why `arrays` was added beside them rather than instead of
them: one measures what the backend does with an array, the others measure how
often a real program hands it one it cannot take.

**A bounds failure cannot throw from compiled code**, which has no frame for the
interpreter to unwind. It writes a flag into a slot past the arguments and
returns, and the tier raises `IndexOutOfRangeException` on its behalf. Stores
that already happened stay done — the same as .NET, where the exception aborts
the method rather than rolling it back.

**What it still does not reach.** Allocation, exception handling, floating point
and object field access. `double[]` is not taken either: only `int[]`, because a
wider set means more element widths in the emitter for no more coverage of the
loops that matter.

**Inlining is branch-free, and reaches through several levels of call.** A
`call` no longer disqualifies a method: a small static callee is spliced into
its caller, which is often the difference between a real method compiling and
being declined. A callee that is *itself* built from smaller callees is
flattened before it is spliced, up to three levels deep.

That last part is not a refinement, it is the difference between the optimisation
working and not. One level handles a caller whose callees are leaves; code is
written with a helper built out of smaller helpers, and against that shape a
one-level inliner leaves the caller entirely interpreted. The `inlined`
benchmark was 3.6× .NET for exactly that reason and is 0.7× now.

What is still refused: a callee containing **any branch** — a helper with an
`if` in it is not spliced — and any instance method. Both the depth and the
size of the result are bounded, so a method cannot quietly compile to something
enormous; past the budget the splicing is abandoned and the method is compiled,
or declined, as written. `--no-inline` turns the whole thing off, and the output
must be identical either way.

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


### Floating point compiles

`double` arithmetic, comparison, negation and conversion all compile:
`a + b`, `a < b`, `-a`, `(double)n` and `(int)d` on scalars.

**IL's arithmetic opcodes do not say which kind they are.** `add` is `add`
whether its operands are integers or doubles, and the difference lives in the
types that reached it. An interpreter can look at the values it holds; a
compiler cannot. So a pass walks the instruction stream keeping an abstract
stack of kinds and records, for each arithmetic instruction, which one it is —
and **declines the method** when it cannot follow the stream, rather than
picking a kind. An integer `add` emitted for a double produces a number,
silently, and the wrong one.

**Doubles need no new register class.** A slot is eight bytes and so is a
`double`, so floats travel through slots and general-purpose registers as bit
patterns; only the arithmetic moves them into xmm and straight back out. That
is why this was a type-inference problem rather than a register-allocation one.

**`double[]` compiles too**, when the array arrives as a *parameter* — a dot
product over 512 elements runs **37× faster** compiled than interpreted, with
the same answer. An eight-byte element needs no sign extension, so the load is
a plain 64-bit move where `int[]` has to widen 32 bits to 64; the bounds check
is the same one, and still throws `IndexOutOfRangeException`.

**A compiled method can allocate.** `new int[n]`, `new double[n]` and
`new bool[n]` inside a compiled method work, which is what finally makes
`matrix` and `sieve` compile — **21.5×** and **20.3×** faster, same checksums.
Both allocate their arrays as locals, and that used to disqualify them outright.

One caveat about those numbers: they are measured with `--jit-threshold 1`.
`Sieve` and `Matrix` are each called *once* by the benchmark, so at the default
threshold they never compile at all and the suite still reports the interpreted
time. The speedup is real and the tiering policy is what decides whether a given
program sees it.

Three facts make it sound, and it needs all three:

* **`Heap::alloc` does not collect.** It allocates and records pressure. A
  collection happens where the runtime asks for one — between two interpreted
  instructions, or at a compiled method's loop back-edge — never underneath an
  allocation. So an allocation can never pull the ground out from under the
  code that requested it.
* **A compiled method cannot return a reference.** Its return type must be an
  integer or a float, so every object it allocates is unreachable the moment
  the call ends and nothing has to be handed back.
* **Allocation is bounded, but by pruning rather than by refusal.** A `newarr`
  inside a loop used to be declined outright, because compiled code never
  collected and an unbounded loop would exhaust a heap the interpreter would
  not. It is allowed where the method reaches a safe point: each poll releases
  the allocations the frame can no longer reach, so memory tracks what is
  *live* rather than what has been allocated. A method that cannot poll — a
  back-edge with a non-empty evaluation stack — is still declined on the old
  rule.
* **A raw pointer into array storage stays valid.** It used to rest on "nothing
  collects during a call", which is no longer true. It now rests on two things
  that are: this collector never moves an object, and any array the frame can
  still reach is named by the safe point's frame map and therefore rooted.

A program that runs interpreted and fails compiled is the one difference this
runtime will not accept, so the bound is measured rather than argued: a test
runs the same compiled loop at ten times the trip count and fails if the peak
heap follows it.

When the heap genuinely cannot satisfy a request, compiled code takes the same
trap path a failed bounds check takes and the tier raises the exception.

**What is still declined:** `float[]`, whose four-byte elements would have to
widen on every load and round on every store; `rem` on doubles, which is IEEE
remainder and has had no single x86 instruction since SSE replaced x87's
`fprem`; `decimal`; and anything else the integer path already declines.

---

## Sorting with your own comparer works, and is stable

`list.Sort((a, b) => ...)` and `list.Sort(myComparer)` both run. The comparator
is managed code — it can allocate, call back into the BCL, and throw — so it
cannot be handed to Rust's `sort_by`, whose comparator returns an `Ordering` and
can do none of those things. The sort is a merge sort written out, calling into
the interpreter for each comparison and propagating an exception the moment one
escapes.

**One difference from .NET, and it is worth stating.** This sort is *stable*:
equal elements keep their original order. `List<T>.Sort` on .NET is an introsort
and documents the order of equal elements as unspecified, so the two runtimes
can disagree there. A program that depends on the difference is depending on
something .NET does not promise — but it is the one place output is not
byte-identical by construction. `OrderBy`, which .NET *does* document as stable,
agrees exactly.

`Sort()` with no comparer still orders numbers and strings only, and refuses
anything else rather than leaving it in an arbitrary order.

---

## `Span<T>` and `Memory<T>` work

A span is a window: something to look at, an offset into it, and a length. All
three are representable here whether the thing is a **managed array**, a
**string**, or a **`stackalloc` byte range**.

**What works.** `Length`, `IsEmpty`, indexing, `Slice`, `CopyTo`, `ToArray`, the
implicit conversion from `T[]`, `AsSpan`, `AsMemory`, `Memory<T>.Span`,
`foreach`, and both collection-expression forms — `[..xs, 4]` and
`ReadOnlySpan<char> x = ['a', 'b']`, the latter through
`RuntimeHelpers.CreateSpan`. Indexing yields a *reference* to the element, so
`span[1] = 20` is visible in the array behind it, which is the point of the type
rather than a detail of it.

`Memory<T>` is the same window. The difference in .NET is that a span is a ref
struct and may not be stored in a field or held across an `await`; that is a
compile-time restriction, and Roslyn has already enforced it before any of this
runs.

**Three shapes, one type.** A span here is over an array (start counts
elements), over raw `stackalloc` bytes (start counts bytes, and the window
records how many make an element), or *is* a string — the original
representation, still what `string + char` uses:

```text
call     ReadOnlySpan<char> String::op_Implicit(string)
newobj   ReadOnlySpan<char>::.ctor(ref char)
call     string String::Concat(ReadOnlySpan<char>, ReadOnlySpan<char>)
```

A line like `text += "0123456789ABCDEF"[nibble]`, which never mentions `Span`,
failed with *no implementation for System.String::op_Implicit* before that
existed, and three of CodeGen's own templates hit it. Every accessor reads all
three shapes.

**The element width was the hard part**, and it is worth saying where it comes
from in each case. Over an array, the array knows. Over raw memory it does not
— `localloc` allocates bytes — and neither does the type, because framework
generics are erased here so that native bindings stay reachable by
declaring-type name. It comes from the **call site**: `new Span<int>(ptr, 4)`
spells `int` out in its `TypeSpec`, and the loader records the arguments of
every member reference, framework generics included. Indexing raw memory yields
a raw pointer rather than a managed reference, and the `ldind`/`stind` that
follows already knows its own width from the instruction.

**Slicing a span that stands for a string** refuses: the string is the whole
representation and there is no offset in it.

**The blob length was the one real trap.** `ReadOnlySpan<char> x = ['a', 'b']`
puts its characters in the image and calls `CreateSpan`, but metadata does not
record an RVA field's length — `InitializeArray` gets away with that because the
array it fills bounds the copy, and a span has nothing to bound it. The size is
in the name of the synthetic type Roslyn emits per distinct size,
`__StaticArrayInitTypeSize=4_Align=2`. Taking the digits after the *last* `=`
reads the alignment, and gives a two-character span a length of one.

---

## Reflection works, except for attributes

**What works.** `System.Type` is a real object, interned one per runtime type,
so `typeof(T) == typeof(T)` is reference equality as .NET guarantees.

```csharp
Type t = value.GetType();
Console.WriteLine(t.Name + " : " + t.BaseType.Name);
foreach (FieldInfo f in t.GetFields()) Console.WriteLine(f.Name);
foreach (PropertyInfo p in t.GetProperties()) Console.WriteLine(p.Name + " " + p.CanWrite);
t.GetProperty("Celsius").SetValue(value, 100.0);
MethodInfo m = t.GetMethod("Compute");
Console.WriteLine(m.GetParameters().Length);
object result = m.Invoke(value, new object[] { 21 });
object made = Activator.CreateInstance(typeof(Widget));
```

All of that runs: names and namespaces, base types, the `IsValueType` /
`IsClass` / `IsInterface` / `IsEnum` / `IsArray` / `IsPrimitive` / `IsAbstract` /
`IsSealed` family, `IsAssignableFrom`, `IsInstanceOfType`, member enumeration,
`MethodInfo.Invoke`, `FieldInfo` get and set, `PropertyInfo` get and set, and
`Activator.CreateInstance`. A boxed value reports the type it holds rather than
`System.Object`.

**Properties come from metadata, not from method names.** C# compiles `p.X` to a
call to `get_X`, so nothing is needed to *run* a property — what reflection needs
is to know the accessors are halves of one member, and the loader reads
`PropertyMap` and `MethodSemantics` to find out. A method called `get_Total` is
not necessarily an accessor, and a property whose accessors were renamed still
pairs correctly.

**Parameter names come from the `Param` table**, which is separate from the
signature: a signature carries types, and only that table carries what the
author called them. A method with no rows there — a native binding, or an
assembly compiled without them — reports `arg0`, `arg1` and so on rather than
inventing a name.

**`Assembly` and `Module` enumerate.** `GetExecutingAssembly`,
`GetEntryAssembly`, `GetTypes`, `GetType(name)`, `GetName`, `Type.Assembly` and
`Type.Module` all work. `Module.Name` reports the file name with its extension,
read from the `Module` table rather than assembled from the assembly name and a
guessed suffix — .NET answers `Conformance.dll` where the assembly is
`Conformance`, and a byte-for-byte comparison notices the difference.

**`Assembly.Load` works on a host, and resolves differently from .NET.** It
searches beside the assemblies already loaded, then any path given to
`Loader::add_search_path`. .NET does not probe: it resolves through the
`AssemblyLoadContext` and the `deps.json` the SDK emits.

For the ordinary case — a project reference, so the SDK writes the dependency
into the manifest and copies the DLL next to the app — the two agree exactly.
They diverge when a DLL is simply *present* beside the app without being
referenced: .NET refuses it, and this runtime loads it.

That is the uncomfortable direction for a divergence. A program that works here
may fail on .NET, which is the opposite of the usual risk and worth saying
plainly rather than leaving to be discovered. Probing is the only mechanism
available without implementing load contexts and reading the dependency
manifest.

Without `std` there is no filesystem to search and `Assembly.Load` refuses,
saying so.

**Constructing a generic type at run time** is still absent, and that is
blocked by generic erasure rather than by reflection.

**Custom attributes are decoded**, including constructor arguments, named
fields and named properties:

```csharp
var mark = (MarkAttribute)typeof(Widget).GetCustomAttributes(typeof(MarkAttribute), false)[0];
Console.WriteLine(mark.Text + " " + mark.Order);
```

An argument this runtime cannot decode — an array, a `Type`, a boxed object —
omits that attribute from the result rather than building it with an invented
value. "Not found" is an answer a caller can act on; a wrong value is not.

**`typeof(T)` on a *class* type parameter throws.** A method type parameter
answers — the instantiation records its arguments. For a class parameter the
argument was erased, so
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

`arglist` · `mkrefany` · `refanyval` · `refanytype` · `jmp`

Multi-dimensional arrays parse but only single-dimension zero-based arrays are
allocated and indexed.

### `localloc`, `cpblk` and `initblk` work, and so does `unsafe`

`stackalloc`, `fixed`, pointer arithmetic, comparison and dereference all run.
`Unsafe.InitBlock` and `Unsafe.CopyBlock` are bound to the same code as the
instructions they stand for.

**A pointer here is not an address.** It is a buffer on the managed heap plus a
byte offset. That is enough because C# only ever gets one from `stackalloc`,
which asks for a byte range, or from `fixed`, which pins an array — both name
memory this runtime already owns. Arithmetic moves the offset, so nothing can
be made to point at memory the runtime does not own, and the pointer *roots*
its buffer, so `stackalloc` memory outlives every reference to it rather than
dying with a stack frame. Everything a program can observe is unchanged.

This is why a raw pointer and a `ByRef` are different things. A `ByRef` is a
*path* — to a local, a field, an element — and cannot be byte-addressed or
advanced past its target. `conv.u` on one is exactly what `fixed` compiles to,
and it is the conversion that turns the path into a pointer. `conv.u` on any
other `ByRef` still refuses: a pointer to a local has no byte offset to give.

**The access width comes from the instruction, not the pointer.** `int* p` and
`byte* q` are the same value here; `*p` versus `*q` is the difference between
`ldind.i4` and `ldind.u1`. Reading the width from the buffer instead truncated
every `stackalloc int[]` write to a single byte — invisible for values below
256, and wrong for everything else.

**What still refuses:** an unaligned pointer into typed storage, a pointer into
an array of references (its elements are handles, not bytes), `cpblk` between
anything other than two byte buffers, and a `Span<T>` over `stackalloc` memory
— that last because the element width lives in `T`, and framework generics are
erased here by choice, so it cannot be known. Inferring it by dividing the
buffer size by the length would be right whenever a span covers its whole
allocation and quietly wrong otherwise.

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

**Structs do not cross the P/Invoke boundary.** Only strings and blittable
primitives.

### `Marshal` works, but on the managed heap

`Marshal.SizeOf<T>()`, `AllocHGlobal`, `FreeHGlobal`, `StructureToPtr` and
`PtrToStructure<T>` all run for a **blittable** struct — one whose fields are
primitives at their natural widths, which is what a C# struct is by default.
A `struct { int A; short B; long C; }` round-trips with its widths intact.

Two things had to exist first, and both arrived above: a raw pointer, so there
is somewhere to put the bytes, and a generic method that knows its type
arguments, so `SizeOf<T>()` can ask what `T` is.

**`AllocHGlobal` does not allocate unmanaged memory.** It allocates a byte
buffer on the managed heap, so `FreeHGlobal` does nothing — the collector owns
the buffer and reclaims it when the last pointer to it is gone. A program that
frees correctly cannot tell the difference. One that uses memory *after*
freeing it finds it still valid here and crashes on .NET, which is the safe
direction to differ in. What it cannot do is hand that pointer to native code:
that needs a real address, and P/Invoke still takes only strings and blittable
primitives.

**A struct with a reference field is refused**, by name, rather than
marshalled. The field is a handle into the GC's table, and writing its bits
would produce a number that looks like a pointer and is not one.

---

## Embedded: the interpreter runs on hardware, within a memory budget

**C# executes on a microcontroller.** On an ESP32-C3 (RISC-V, 400 KB SRAM,
no operating system) the loader builds a type registry, RustBCL registers all
988 of its native bindings, and the interpreter runs `HelloWorld.Main`:

```
-- il interpreter --
heap budget      294912 bytes
bcl tier         full (260702 bytes needed)
native bindings  821
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
x86-64 and on RISC-V alike. That capture is from the last time the board was attached; RustBCL has since
grown to 988 bindings. The count in the excerpt is what the chip actually
printed, not what a rebuild would print today.

Full captures of the interpreter running:
[ESP32-C3, RISC-V](logs/esp32c3-interpreter.log) ·
[M5Stack Tough, Xtensa](logs/m5stack-tough.log). Earlier metadata-and-GC
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

| Board | Core | RAM | Heap | Tier | State |
| --- | --- | ---: | ---: | --- | --- |
| ESP32-C3 | RISC-V 32 | 400 K | 288 K | full | **executes IL on hardware** |
| M5Stack Tough | Xtensa LX6 | 520 K | 176 K + 96 K | full | **executes IL on hardware** |
| ESP32-WROOM-32 | Xtensa LX6 | 520 K | 176 K + 96 K | full | same image; last flashed pre-interpreter |
| Meadow F7 Micro | Arm Cortex-M7 | 384 K | 288 K | full | builds; last flashed pre-interpreter |
| Sipeed Maix Go K210 | RISC-V 64 | 6 M | 1 M | full | **runs** — `HelloWorld.Main` executes, output identical to `dotnet` |
| Netduino 3 WiFi | Arm Cortex-M4F | 192 K + 64 K CCM | 192 K | minimal | builds; never flashed — no board |
| Raspberry Pi Pico | Arm Cortex-M0+ | 256 K | 192 K | minimal | builds; never flashed — no board |
| Nucleo-F401RE | Arm Cortex-M4F | 96 K | 64 K | **none** | builds; never flashed — no board |

The first two rows have been run on hardware since the interpreter landed —
**two architectures**, RISC-V 32 and Xtensa LX6, from the same source. The rest
are builds, and are worth reading as exactly that. (The WROOM-32 runs the same
image as the M5Stack Tough, feature for feature; it has simply not been
reflashed since.)

**One board cannot run a program at all, and that is reported rather than
discovered.** The Nucleo-F401RE has 96 KB of RAM against a 192,045-byte floor.
No arrangement of that memory loads the runtime, so the firmware prints the
shortfall and carries on with the metadata reader and the collector. It also
does not pay flash for what it cannot use: `Tier::for_budget` is a `const fn`
over a constant, so LTO proves the `Full` and `Minimal` arms unreachable and
strips the loader and all 988 bindings — 21 KB of `.text` against the F427VI's
282 KB from the same source file.

**Two boards reach their tier only by using memory their part does not offer
by default**, which is the most instructive thing in that table.

The **Netduino 3 WiFi** advertises 256 KB of RAM in two pieces that are not
adjacent: 192 KB of DMA-reachable SRAM at `0x20000000`, and 64 KB of CCM at
`0x10000000` that the core can reach but DMA cannot. Giving the allocator only
the first leaves 192 KB *minus* `.data`, `.bss` and the stack — and the floor is
192,045 bytes, so a few kilobytes of statics decides it. So the roles are
swapped: `.data`, `.bss` and the stack go to CCM (this firmware does no DMA,
which is the only thing CCM cannot do), and the whole 192 KB of SRAM becomes the
heap in one unbroken piece. `cortex-m-rt`'s `link.x` hardcodes `> RAM`, so
naming CCM `RAM` is what moves them. 196,608 bytes clears the floor by 4,563 —
a real margin, but thin enough to re-measure if RustBCL grows.

The **ESP32** has the same shape of problem, and this is no longer theory: the
M5Stack Tough needed 260,702 bytes for the full binding set and its main
`dram_seg` tops out at 176 KB. Adding the second bank took it to 278,528 and it
ran. The arrangement was designed for the WROOM-32 and had never been executed
until that board was flashed. Its main `dram_seg` tops out at 176 KB — found
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
| Ahead-of-time compilation | Needs Arm and RISC-V code generators that *execute*; only x86-64 does |
| The Pico, Netduino and Nucleo-F401RE | Their images build; none has been flashed, because no board was connected |
| The RISC-V and AArch64 code generators on a board | Both emit, and both have their exact bytes asserted against a disassembler, but neither has executed an instruction. The Maix Go is now an RV64 board that could run one — what is in the way is that `rustclr-jit` is `std`-only (`cargo build -p rustclr-jit --target riscv64gc-unknown-none-elf` cannot find `std`) and that `codepage.rs` takes its executable pages from `mmap`/`VirtualAlloc`, which bare metal has neither of |

This table used to have a first row saying IL execution did not run on hardware
at all — that the chip could read an assembly and manage a heap but not execute
a method. That stopped being true when the ESP32-C3 and the M5Stack Tough ran
`HelloWorld.Main` with the whole of RustBCL. A limitations page that understates
a runtime misleads exactly as much as one that overstates it, so the row is
gone rather than softened. This is [Milestone 6](../Plan.md).

---

## Debugging

**Hooks, source lines and a debug adapter all exist.** The runtime can stop
before a chosen instruction, step into, over and out of calls, and hand a
debugger every frame's arguments, locals and evaluation stack while the program
is stopped. A debugger may also set breakpoints *while* stopped, and they take
effect on the same run. `rustnet debug` drives it from a terminal;
`rustnet debug --dap` speaks the Debug Adapter Protocol over stdin and stdout,
so VSCode and Rider can attach.

Source-line breakpoints and named locals come from the **portable PDB** the
.NET SDK writes beside the assembly — the same `BSJB` metadata format the
assembly reader already parses, with the debug tables (`Document`,
`MethodDebugInformation`, `LocalScope`, `LocalVariable`) now in the schema.

What remains is stated rather than hidden:

| | |
| --- | --- |
| No PDB, no lines | without one, breakpoints are `Method:offset` and locals are numbered — the command says so rather than offering a syntax that never matches |
| No pause button | the hook is synchronous, so while the program runs no DAP request is serviced; an editor's pause does nothing until the next breakpoint |
| One thread is debugged | the thread that runs `Main`. A thread the program starts gets its own interpreter and no debugger, so a breakpoint inside it never fires — its console is still redirected, so its output reaches the editor |
| Objects are not expanded | a variable that holds a reference shows the handle; expanding it means reading the heap, which the adapter does not have while stopped |
| No editor extension | the adapter is a stdio DAP server; something has to contribute the debug type and launch it |
| No `evaluate` | there is no expression evaluator, so an editor's watch window and debug console cannot compute anything |
| Debugging is interpreted-only | `rustnet debug` does not enable the JIT: compiled code has no IL offset to stop at, and a program that ran half compiled would hit breakpoints in only half its methods |

A compiler-generated local the PDB does not name is identified by its slot
number rather than given an invented one: `[3]=…` in the terminal, `local3` in
an editor's variables pane.

**The hook costs nothing when nothing is attached.** That is measured rather
than assumed: interleaved best-of-eleven against a build with the hook removed
puts it within noise. It only holds because the poll is marked cold and never
inlined — without those, the same measurement showed **8%** on the most
instruction-dense interpreted workload, because the branch and its registers
were being hoisted into the hot loop.

---

## Device libraries

**`RustNet.Hal` reaches a board, and the board is a module.** GPIO, ADC, PWM,
I2C, SPI, UART, watchdog and delay, over traits in `crates/rustclr-hal`. Which
board is underneath is decided at firmware build time, the way the collector
and the scheduler are chosen — `requirements.md` asks for exactly that, and a
peripheral wired into the interpreter would be the one place it was not
honoured. `rustnet run --board host` installs a simulator, so hardware code is
testable on a desktop.

**No board is an error.** A program that writes a pin on a machine that has
none is not doing something harmless; it is doing something that will not
happen. Every binding throws, naming what was missing and what to do about it.
Only `Board.Name` and `Board.IsPresent` answer without one, because whether
there *is* a board is the one hardware question a program running anywhere is
entitled to ask.

| | |
| --- | --- |
| Only the peripherals listed above are bound | The trait crate also describes I2S, CAN, 1-Wire, RTC, external memory, network interfaces, timed signals and cameras. They have no managed bindings yet, so a C# program cannot reach them |
| One board per interpreter | A worker thread's interpreter does not carry the board across, so a `Task.Run` that touches a pin throws. The same rule as the debugger and the profiler |
| The simulator is not a chip | Timings are not real, an ADC reads what was written to it, and an I2C bus with nothing attached NACKs. It catches "this program calls a peripheral this board does not have" and it does not catch a race with a sensor's conversion time |

### Storage

**`RustNet.IO.Storage` reads and writes whole files, and that is the shape, not
a subset.** NOR flash clears bits and cannot set them; the only way back is
erasing a whole sector. A `Stream` with `Seek` and partial writes over that
would mean read-erase-write of the surrounding sector on every write — slow,
and a power-loss window over data that was previously fine. Writes append and
the newest record wins; deletes append a tombstone.

| | |
| --- | --- |
| No handles, no seeking, no partial writes | `AppendAllText` reads, concatenates and writes a new record. Right for a log line, wrong for a megabyte |
| The store only grows until it is compacted | A superseded record still occupies its bytes. `Storage.UsedBytes` is what a program watches; compaction is not exposed yet |
| No `FileStream`, no `Path`, no `DirectoryInfo` | Directories are prefixes rather than objects. `CreateDirectory` writes an empty marker so an empty one can still be listed |
| Region 0 only | Application files live in external-memory region 0. A board with no external memory throws rather than writing to RAM that disappears at the next reset |

### Cryptography

**The algorithms are written out rather than depended on**, and checked against
the vectors their specifications publish: FIPS 180-4 for SHA-256 (including the
million-character case that catches a 32-bit length counter), FIPS 197 and
NIST SP 800-38A for AES-CTR, RFC 4231 for HMAC-SHA256. That is a stronger claim
than a dependency gives, and it is why four bare-metal targets build without a
vendoring step.

| | |
| --- | --- |
| No RSA, no elliptic curves | Signature verification needs bignum arithmetic, and a hand-rolled bignum with a side channel is worse than no signature check — it is a check that reports success. Secure boot therefore verifies an HMAC over a key provisioned at flash time, which is a real guarantee with a real limitation |
| No random number generator | Entropy is a property of the hardware. It comes from the board through the HAL, and a board without one says so rather than returning a counter dressed up as randomness |
| AES is counter mode only | No ECB, deliberately: equal blocks encrypt to equal ciphertext, and offering it beside CTR would be offering a foot-gun with the same shape as the safe option |
| A (key, counter) pair must never be reused | Reusing one XORs two plaintexts together and both are recoverable. Nothing here can check that — it does not know what came before — so it is stated rather than implied away |
| Not hardened against physical attack | The AES here is straightforward and table-free. It is not written to resist power analysis by somebody holding the board |
| Keys are 16 or 32 bytes | A key of another length is refused, not padded. A five-byte password stretched into a 128-bit key is a five-byte password |

### Secure boot and OTA

**The image container is verified with a MAC, not a signature.** The reference
this was ported from uses RSA PKCS#1 v1.5. RSA needs bignum arithmetic, this
workspace has no external dependencies, and a hand-rolled bignum with a timing
side channel is *worse* than no signature check — it is a check that reports
success.

| | |
| --- | --- |
| What it still gives you | Nobody without the key can produce an image the device accepts. A corrupted download, a truncated write, a man-in-the-middle on the update channel and an image built for another chip are all rejected before a byte of payload runs |
| What it does not | The verifying key is also a signing key. Extracting it from any one device lets an attacker sign images for all of them — which is exactly the property public-key cryptography has and this does not. Per-device keys narrow it to one board |
| No rollback protection | A device will accept an *older* correctly-signed image. There is no version counter in the container yet, so a downgrade attack against a fixed vulnerability is possible |
| Confirmation is the new firmware's job | An image can verify perfectly and still be broken. Only the new firmware knows whether it works, so silence for three boots is treated as failure and the old slot comes back |

### Networking

`rustclr-net` — HTTP, Modbus, a small web server — is **`std`-only**, and that
is a fact about its transport rather than an oversight: the sockets are
`std::net`, the timeouts are `std::time`, the server spawns `std::thread`. A
board reaches a network through the HAL's `netif` trait or an AT-command modem
instead. The codec a device actually needs, MQTT, is `rustclr-mqtt` — a
separate crate precisely so it builds bare-metal, and it is transport-free:
packets in, packets out, and the caller moves the bytes.

There are also **no managed bindings for networking yet**, so a C# program
cannot open a socket. That is the next layer, not a design position.

### Graphics

One RGB565 canvas per interpreter. A 240×240 buffer is 115 KB, which on most
of these boards is the entire heap budget, so a second one would not be a
feature — it would be an out-of-memory.

| | |
| --- | --- |
| One built-in font | 8×8, ASCII only, integer scaling. Non-ASCII draws as a gap rather than a substitute glyph: a device showing `?` where a degree sign belongs is showing a wrong reading |
| No anti-aliasing, no sub-pixel positioning | Text and lines are drawn on whole pixels |
| `Present` needs a board | Drawing does not, which is what lets a UI be tested headlessly. A board with no wired panel accepts the frame and does nothing with it |
| No image decoding | `DrawImage` takes RGB565 the caller already has. There is no PNG or JPEG decoder |

### Logging on a chip with no atomics

`rustclr-diag` shares a logger, which needs mutual exclusion — and **thumbv6m
(Cortex-M0+) and rv32imc have no compare-and-swap at all**, so
`AtomicBool::compare_exchange` does not compile there and `alloc::sync::Arc` is
configured out for the same reason. Without `std` the logger therefore *tries*
its borrow and **drops the record** if the logger is busy, counting what it
dropped.

**Do not log from an interrupt handler on such a chip.** The borrow flag is a
plain cell there, so a handler that preempts a logging main loop can corrupt
it. Queue the record and log it from the main loop.

### Text encodings

**UTF-8 and ASCII, and nothing else.** Every wire format an embedded program
meets — an MQTT payload, an HTTP body, JSON, a sensor's serial protocol — is one
of those two, and .NET's remaining code-page tables are larger than the whole
heap on the boards this targets. ASCII substitutes `?` for what it cannot
encode, matching .NET, so a program that round-trips through it gets the same
answer here as there. Invalid UTF-8 decodes lossily to U+FFFD, as
`Encoding.UTF8` does by default; the strict `throwOnInvalidBytes` form is not
offered rather than being offered and quietly behaving like the lossy one.

---

## Profiling

**`rustnet profile` samples; it does not instrument.** Every 512 IL
instructions the interpreter reads the clock, and when a sampling period has
passed it walks its own frame stack and charges the *measured* elapsed time to
what it finds. Calls are counted exactly; time is sampled.

What that buys is stated in `docs/cli.md`. What it costs:

| | |
| --- | --- |
| Wall clock, not CPU time | time blocked in a `Task.Delay`, a `Join` or a lock is charged to the method that waited. Right for "why is this slow", wrong for "what is burning the processor" |
| One thread | a thread the program starts gets its own interpreter and no profiler; its time and its allocations go unattributed, so on a threaded program the per-type table sums to less than the heap totals beside it |
| Compiled code is invisible | a compiled method executes no IL, so `rustnet profile` interprets everything unless `--jit` is given. With `--jit` a compiled method's time lands on the interpreted frame that called it and its allocations on nothing — the command says so on stderr rather than printing it quietly |
| A profile is not a benchmark | interpreting changes the absolute numbers. Use `run --stats` for speed and this for shape |
| No call graph | self and inclusive time per method, not caller/callee edges. A tree needs every sampled stack kept rather than folded into per-method counters |
| No line-level attribution | the frame knows its method, and the sample is charged there. Charging an IL offset would need the PDB the debugger reads, and a sample lands between instructions rather than on one |
| Resolution is the host clock | `Host::monotonic_nanos` on a desktop is `Instant`; a board that has only milliseconds gets milliseconds, and a run shorter than one period reports no samples rather than a made-up distribution |

**The hook costs nothing when nothing is attached.** One `Option::is_some` per
instruction, the same shape as the debugger's. Measured rather than assumed:
interleaved best-of-nine against a build with the check removed puts `fields`
— the most instruction-dense workload — at 2,478.8 ms against 2,480.3 ms
median, and `fib` at 446.3 against 433.4. Both within the run-to-run spread.

**A program's own diagnostics come from the same counters.**
`GC.GetTotalAllocatedBytes`, `GC.GetTotalMemory` and `GC.CollectionCount` read
the heap the profiler reads, so a program cannot disagree with its own profile.
Two consequences worth naming: `GC.GetAllocatedBytesForCurrentThread()` returns
the **process** total, because there is one heap and no per-thread allocation
context; and `GC.CollectionCount` ignores its generation argument, because the
default collector is not generational and `GC.MaxGeneration` returns 0.

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
