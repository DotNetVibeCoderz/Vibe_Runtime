# Progress

Tracking for RustNetRuntime. Updated 2026-08-20.

**Verified this session:** `cargo test --workspace` → 141 passed, 0 failed.
Five programs — two conformance suites, a sample, and two written or edited by
the assistant against a live LLM — produce byte-identical output on RustCLR and
.NET.

```
Conformance    IDENTICAL — checks=285 failures=0
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
| `rustclr-gc` | Handle-based heap, pluggable collectors, mark-sweep, **shared heap + stop-the-world safepoints** | 19 tests; 200k-deep graph marks without stack overflow |
| `rustclr-core` | Type system, loader, IL interpreter, exception handling | 18 unit + 3 multi-threaded |
| `rustclr-bcl` | Native BCL: Console, String, Math, interpolation, tuples, ranges, Nullable, generic collections, LINQ, Task and async, reflection | 20 unit + 8 integration; 836 bindings |
| `rustclr-sched` | Lock-free MS queue, MPMC channel, thread pool | 16 tests |
| `rustclr-interop` | P/Invoke, dynamic loading, marshalling | 9 tests; calls the real `GetCurrentProcessId` |
| `rustclr-jit` | Compiler trait, IL verifier, analysis, **x86-64 code generator**, W^X pages, tiering | 25 unit + 3 differential |
| `rustnet-cli` | `run` / `info` / `disasm` / `verify` / `build` / `capabilities` | Drives every fixture |

### A thread pool, and `rustclr-sched` finally earns its keep

`Task.Run` and `Parallel.*` queue onto one worker per core instead of taking a
thread each. Two thousand tasks is a conformance check; it used to mean two
thousand OS threads, each paying for a copy of the loader on the way up.

**The workers hold interpreters**, which is the whole reason the pool could not
be `rustclr-sched`'s as it stood. A pool of plain closures would have to build
an interpreter per job — and copying a loader is the expensive part of starting
a managed thread, not the thread. So each worker builds one at startup and
reuses it, and a job is `FnOnce(&mut Interpreter)`. The lock-free queue
underneath *is* `rustclr-sched`'s, tested since the scheduler milestone and
until now unused by anything.

**A waiting thread helps.** A thread blocked on a task runs queued jobs rather
than idling. Without that the pool deadlocks in the familiar way — every worker
blocked on a task whose job is still queued behind it — and `Task.Run(async () =>
await Task.Run(…))` is exactly that shape. It is also why the pool never needs
to grow under load.

`Task.Delay` arms a timer and returns a pending task, so a delay overlaps the
work after it. One timer thread serves every outstanding delay, and it owns no
interpreter: when a deadline passes it queues the completion like any other job,
because a continuation is managed code and belongs on a worker.

**The bug this turned up was not about threads at all.** A non-generic
`TaskAwaiter.GetResult()` returns void, and this runtime was handing back a
value for it — which leaves that value on the evaluation stack, so everything
the caller reads afterwards is off by one. It surfaced as a null reference two
statements later, on a `Stopwatch` that was fine. It had been wrong the whole
time and stayed invisible while `Task.Delay` returned an already-completed task
that nothing awaited.

### await suspends

An `async` method that awaits a pending task returns to its caller, and the
thread completing the task resumes it. Two tasks started and then awaited
overlap; awaiting in a loop stays sequential, as it is on .NET.

**Almost none of this was new code, and one line of it was mine to undo.** The
continuation machinery had been right all along: `AwaitUnsafeOnCompleted` copies
the state machine to the heap — which is what makes a suspension a suspension,
since it has to outlive the frame it was a local of — and queues it on the task.
What blocked `await` was that I had made the *awaiter's* `IsCompleted` wait and
answer `true`, back when `Task.Run` ran inline and a pending task meant a bug.
With `Task.Run` on a real thread, answering honestly sends the state machine
down the path that was already there.

**Two things did need building.** Waiting on a task nobody owns: the task an
`async` method returns has no thread of its own — it is completed by whichever
thread finishes the *inner* task — so `Result` cannot join anything. It waits
while another mutator is registered and reports a clear error when none is,
which is the difference between waiting and hanging.

And a race worth naming: `await_on_completed` checked whether the task was
pending and *then* joined the queue. A task completing between those two steps
drained a queue the continuation had not yet entered, and the `await` would
never resume. Settling a task and taking its waiting list now happen under the
same gate as checking and queueing. Resuming runs managed code, so it happens
after the gate is released, never under it.

`Stopwatch` had to become a real object on the way past: it was the number it
started at, which reads `ElapsedMilliseconds` correctly and cannot express
`Restart`, because `this` for a class arrives by value and there is nothing to
write back to.

### Task.Run and Parallel.* run on threads too

`Task.Run` starts its delegate on another thread; `Parallel.For`, `ForEach` and
`Invoke` split their work one thread per core, capped by the iteration count.
Two 120 ms tasks finish in 123 ms rather than 240.

Both fell out of the thread work rather than needing anything new: `spawn` and
`join` were already there, and a parallel loop is a partition plus a join.
`Task.Run` needed one more idea — the task carries the id of the thread running
it, and every way of *observing* a task settles it first. That is `Result`,
`Wait`, `WaitAll`, and an awaiter's `IsCompleted`, which is the gate `await`
branches on: answering "not yet" would send the state machine down the
resume-inline path before the body had finished.

**`WaitAll` was waiting for nothing.** It read a params array, and .NET 10 does
not pass one — `Task.WaitAll(a, b)` lowers through an `InlineArray2<Task>` and a
`ReadOnlySpan<Task>`. Reading only arrays meant it found no tasks and returned
at once. It reads a span now.

**A timing check is the wrong test, and the fixture caught me writing one.**
`elapsed < 200 ms` for two 120 ms tasks passed standalone and failed inside the
conformance run, where the machine is busy — a margin generous enough to be
reliable is too generous to prove anything. It is a rendezvous now: each task
announces itself and waits for the other, with a bounded wait so a runtime that
serialises them reports `false` rather than hanging. Whether two things
overlapped is not a question about how long they took.

**`Environment.CurrentManagedThreadId` returned 1 for every thread.** True while
threads were serialised, and a silent wrong answer once they were not — an
iterator's state machine uses it to notice cross-thread enumeration, so every
thread sharing an id would have had them sharing one state machine.

### Threads run at the same time

`Thread.Start` spawns an OS thread and `Join` waits for it. Four threads
incrementing one static reach four thousand, `lock` excludes, `Interlocked` does
not lose updates, and a consumer can block on a flag set by a producer started
*afterwards* — the case that hangs forever under serialisation. All four are
conformance checks, because each has an answer that differs if the threads do
not really overlap, and the fixture ran identical to `dotnet` twelve times in a
row.

**The design is that the loader is copied, not shared.** That is the thing that
made this tractable, and the earlier conclusion — that it needed either unsafe
concurrent data structures or a redesign of stub synthesis — was wrong for a
reason worth writing down. `registry.ty()` is called on nearly every instruction
and hands back a reference, so a lock underneath it would cost more than the
heap's did. But a `Loader` is *finished* before the first instruction runs, and
an identical copy behaves exactly like a shared immutable one, at no cost and
with no borrow problem at all. Only the genuinely mutable part — static field
storage — is shared, behind a lock.

The correctness condition is precise and checkable: the copies must not diverge.
A loader grows in two places after load, an interface stub and
`MakeGenericType`, and a `MethodId` minted on one thread names nothing on
another. A thread that grew its registry is caught on `Join` and reported.

**Three bugs, and the second is the one that matters.**

`lock` was a no-op, which was harmless while nothing overlapped and is a data
race the moment something does. `lock (gate) counter++` on two threads gave
1,863 of 2,000 — both threads reached the same static, which was the point, and
neither excluded the other, which was not. `Interlocked` had the same problem
one layer down: read, add and write are three separate lock acquisitions, so two
threads interleave between them. It reached 18,828 of 20,000, and only after
`Increment` was gated as well as `Add` — gating one and not the other is exactly
the kind of half-fix that looks finished.

**The GC bug is the one to remember.** A thread waiting in `Join` announces
itself blocked so the collector does not wait for it, and the first version
contributed *none of its roots* while it was away. "Blocked threads hold no
references into the heap" is true of a thread parked at a safe point and false
of one sitting in `Join` holding the array of threads it is joining. A
collection swept it, and `new Thread[4]` came back with length zero. Blocked
threads hand their roots over on the way out now, and there is a test.

That bug was invisible until threads were real *and* allocated enough to force a
collection. The earlier multi-threaded tests passed because nothing was live
across the wait.

### Every probe passes, and that is not the same as parallelism

`Task.WaitAll(a, b)` written in C# runs, and the advanced-feature matrix is
**21 of 21**.

The last failure was never about the TPL. `WaitAll(a, b)` does not call a params
array on .NET 10: it fills a `System.Runtime.CompilerServices.InlineArray2<Task>`
and makes a `ReadOnlySpan<Task>` over it. That was recorded as "the lowering is
what this runtime cannot follow" — true when it was written, and the spans and
raw pointers built since had already removed most of it.

What was left was three small things. `InlineArray<N><T>` is a *framework* type
on .NET 10, not compiler-generated, so it had to be registered — a struct of N
slots, and the arity is in the name. `Unsafe.As` is the identity on a managed
reference, because a reference here is a path to a slot and reinterpreting it
changes only the type it is read at. `Unsafe.Add` walks that path: to the next
array element, the next struct field, or — when the reference names something
whose fields *are* the elements, which is exactly an inline array — to its nth
slot. The pointer kind and `ByRef::StructField` already existed; nothing new was
needed to express any of it.

`MemoryMarshal.CreateReadOnlySpan` is the one honest compromise, and it is
written down where it is done: it copies the elements out rather than making a
view onto the caller's storage. The callers that reach it build the buffer
immediately before and never touch it again, and a `ReadOnlySpan` cannot be
written through, so nothing can observe the difference — but anything needing a
genuine view over a struct's fields is not served by it.

**21 of 21 is a statement about output, not capability.** `tpl` passes because
its answers are order-independent: `Task.Run` runs its delegate where it is
created and `Parallel.For` iterates in order, and the sums come out the same. A
probe measuring wall-clock overlap would fail. Three rows of the matrix are
marked with a warning for exactly this reason, and the caveat now sits above the
table rather than below it, so a reader meets it before the number.

### The call site knows what the type does not

Two probe failures — a span over `stackalloc`, and `Memory<T>` — both came down
to one missing fact, and the fact was never actually missing. The matrix
reached **20 of 21**.

A `Span<int>` over `stackalloc` memory needs to know that an element is four
bytes. The buffer cannot say: `localloc` allocates bytes. The type cannot say
either, because framework generics are erased here — `List<int>` and
`List<string>` are deliberately one runtime type, so that native bindings stay
reachable by declaring-type name. That was recorded as the reason the feature
could not be built.

It was the wrong conclusion. Erasure applies to the *runtime type*; the call
site still spells the arguments out, and the loader was already parsing that
`TypeSpec` to build a name. `new Span<int>(ptr, 4)` says `int` right there.
Recording the resolved arguments of every member reference — framework generics
included, which `member_ref_owner` deliberately skipped — and staging them for
the duration of one native call was the whole change.

That is a general capability, not a span fix. Any native implementation on a
framework generic can now ask what the call site named.

**A span is three shapes now**, and every accessor reads all three: a window
onto an array where start counts elements, a window onto raw bytes where start
counts bytes and the window carries the element width, and a string that *is*
the span. Indexing raw memory yields a raw pointer rather than a managed
reference — and the `ldind`/`stind` that follows already knows its own width
from the instruction, so nothing had to be threaded through.

`Memory<T>` came free: it is the same window. The difference in .NET is that a
span may not be stored in a field or held across an `await`, and Roslyn has
enforced that before any of this runs.

### Marshalling blittable structs

`Marshal.SizeOf<T>()`, `AllocHGlobal`, `StructureToPtr` and `PtrToStructure<T>`
work for a struct of primitive fields, widths intact — the matrix reached
**19 of 21**.

Nothing here was hard, and that is the point: both halves of marshalling had
been blocked on things finished earlier in this session. Turning a value into
bytes needs somewhere to put them, which the raw pointer gave; `SizeOf<T>()`
needs to know what `T` is, which generic methods already record on their
instantiation, so the native implementation reads it back rather than being
handed it. Two features that were built for other reasons met.

**`AllocHGlobal` allocates on the managed heap**, so `FreeHGlobal` does nothing
and the collector reclaims the buffer when the last pointer to it is gone. A
program that frees correctly cannot tell; one that uses memory after freeing it
finds it still valid here and crashes on .NET, which is the safe direction to
differ in. The pointer cannot go to native code — that needs a real address —
and `docs/limitations.md` says so rather than leaving it to be discovered.

A struct with a reference field is refused by name. Its field is a handle into
the GC's table, and writing those bits would produce a number that looks like a
pointer and is not one.

### Raw pointers, and `unsafe` C# runs

`stackalloc`, `fixed`, pointer arithmetic, comparison and dereference all work,
along with `cpblk` and `initblk`. The advanced-feature matrix went 16 → 18 of
21; `unsafe-fixed` and `unsafe-stackalloc` both pass.

**A pointer is not an address.** It is a buffer on the managed heap plus a byte
offset, which is enough because C# only ever gets one from `stackalloc` — a
byte range — or from `fixed`, which pins an array. Both name memory this
runtime already owns. Arithmetic moves the offset, so nothing can point outside
a buffer the runtime knows about, and the pointer *roots* that buffer, so
`stackalloc` memory outlives every reference to it instead of dying with a
frame. That is a stronger guarantee than the native stack would give, and
nothing observable depends on the difference.

It also explains why this is a separate kind from `ByRef` rather than a widening
of it. A `ByRef` is a *path* — to a local, a field, an element — and has no byte
offset. `conv.u` on a `ByRef::ArrayElement` produces a pointer, which is exactly
what `fixed` compiles to; `conv.u` on any other `ByRef` still refuses.

**Three bugs, and the second is the one worth remembering.**

Pointer *comparison* was missing, so `for (int* p = start; p < start + n; p++)`
read both sides as zero and the loop body never ran. The probe reported 0 where
`dotnet` said 10 — a wrong answer, not a refusal.

The access width was being read from the *buffer* rather than the instruction.
`int* p` and `byte* q` are the same value here, and `*p` versus `*q` is the
difference between `ldind.i4` and `ldind.u1`, so every `stackalloc int[]` write
was truncated to one byte. `unsafe-stackalloc` passed anyway, because its
values are 7, 8 and 9 and all three fit. Writing 300 and 70000 gave back zero.
A probe passing is not the same as a thing working.

The third: `p[0] + ","` does not emit `ldind.i4` at all — it calls
`Int32::ToString()` with the *pointer* as `this`. A value type's receiver
arriving as a raw pointer had no unwrapping path, so the method read the pointer
itself and every such value rendered as zero. The width for that one comes from
the declaring type, which is the only place it is written down.

### Spans work over arrays

`Span<T>` and `ReadOnlySpan<T>` over an array: `Length`, `IsEmpty`, indexing,
`Slice`, `CopyTo`, `ToArray`, the implicit conversion from `T[]`, and both
collection-expression forms. The advanced-feature matrix went 14 → 16 of 21.

**The reframing is the useful part.** A span was recorded as unimplementable
because it is a generic ref struct. It is not the ref-struct-ness that was the
obstacle — a span is a window: something to look at, an offset, and a length,
and all three are representable whenever the thing being looked at is a managed
array. Only *raw* memory has no representation here, because a managed pointer
in this runtime is a path to a slot rather than an address. So `stackalloc`
still refuses and everything else does not, which is a much narrower line than
the one that was written down.

Indexing returns `ByRef::ArrayElement` — a reference to the element, which the
caller loads or stores through. That is what makes `span[1] = 20` visible in
the array behind it, and it needed no new machinery: the pointer kind already
existed.

**A span is now one of two shapes**, and both had to keep working. Over an array
it is a window object with three slots; over a string it *is* the string, which
is the older representation and the one ordinary C# depends on — `string + char`
lowers through `ReadOnlySpan<char>` on .NET 10. Every accessor reads both.

**The blob length was the real trap.** `ReadOnlySpan<char> x = ['a', 'b']` puts
its characters in the image and calls `CreateSpan`, and metadata does not record
an RVA field's length — `InitializeArray` gets away with that because the array
it fills bounds the copy. The size is in the name of the synthetic type Roslyn
emits per distinct size, `__StaticArrayInitTypeSize=4_Align=2`, and taking the
digits after the *last* `=` reads the alignment instead: a two-character span
came back with a length of one. Bounded by nothing at all it was 276.

### await foreach over an async iterator runs

`async IAsyncEnumerable<int>` with `await foreach` produces the same answer as
`dotnet`, including `yield break`, an empty sequence, and breaking out early.
That completes the async surface: what is still missing is overlap, not syntax.

The compiler lowers an async iterator into a state machine that is its own
`IAsyncEnumerable<T>`, `IAsyncEnumerator<T>` *and* `IValueTaskSource<bool>` —
all IL this runtime already executes. Two pieces were the runtime's:
`AsyncIteratorMethodBuilder`, and `ManualResetValueTaskSourceCore<T>`, the
promise `MoveNextAsync` hands back. Both are simple here for the same reason the
rest of `async` is: by the time `MoveNextAsync` returns, the body has already
run to the next `yield return`, so the promise is always settled and the
"reset, await, complete later" cycle it exists to manage never happens.

**Three bugs, each found by running it.**

`start_state_machine` passed the *pointer* to the state machine. That is right
for an async method, whose state machine is a struct, and wrong for an async
iterator, whose state machine is a class because it has to outlive the call —
there `this` is the object. Passing a pointer-to-local made every `ldfld` in the
body read from the wrong thing, surfacing as a null receiver two frames away in
`DisposeAsync`.

`ValueTask<T>`'s two-argument constructor takes an `IValueTaskSource<T>`, and
was wrapping the *source object itself* as the result. It asks the source now
instead — sound only because nothing overlaps, which is written down where it
is done rather than left to be inferred.

The third ended every `await foreach` at the closing brace. Once an enumeration
finishes, `DisposeAsync` returns `default(ValueTask)` — all zeroes, so null
here — and the shared awaiter bindings refuse a null receiver, which is correct
for a `TaskAwaiter` and wrong for this one. The `ValueTask` awaiters now read
null as completed, and are registered *after* the shared loop so they win.

### await using and ValueTask run

`await using` works, `IAsyncDisposable` with it, and `ValueTask`/`ValueTask<T>`
underneath — the advanced-feature matrix is 13 → 14 of 21.

`AsyncValueTaskMethodBuilder` had been registered all along; what was missing
was `ValueTask` itself and its awaiters. A `ValueTask` is represented by the
task it stands for, which loses the one thing the type exists for — avoiding an
allocation when a method completes synchronously — and keeps everything a
program can observe. `default(ValueTask)` is the case worth naming: a struct's
default is all zeroes, which arrives here as null, and in .NET it means an
*already completed* task rather than an absent one, so null reads as completed.

**Two bugs, both found by running it rather than reasoning about it.**

`ConfiguredValueTaskAwaitable` was in the framework type table but
`ValueTaskAwaiter` was not, so `await using` reached `get_IsCompleted` on a type
the registry did not have and the member reference would not resolve. The
binding existed; the type did not. Same shape as the `ParameterInfo` bug earlier
in this project.

The second was subtler and the trace found it, not the reading. The constructor
returned the constructed value, which is what `newobj` on a value type takes —
and `ValueTask<int> v = new ValueTask<int>(11)` does not compile to `newobj` at
all. Roslyn emits `ldloca v; ldc.i4 11; call .ctor`, so the return value went
nowhere and the local stayed null, which then read as `default(ValueTask)` —
*completed*, with no result. `IsCompleted` passed and `Result` returned zero,
which is exactly the plausible-looking wrong answer that is worth catching.
Writing through the `this` pointer serves both shapes, since `newobj` reads the
same slot back out of its cell.

### A static method on a generic type knows its type argument

`Tally<int>.ArgumentName()` returns `"Int32"`. This was the last generic-type
gap, described as needing something the frame did not have: no receiver exists
in a static method, and the body is shared by every construction.

It needed nothing new. The call site is a `MemberRef` whose owner is the
*construction*, and the loader had been recording that since `newobj` on a
constructed generic needed it — the information was already there, one frame
away. `do_call` reads it before entering and sets it on the new frame;
`frame_generic_argument` uses it for `!N` when there is no receiver, and still
refuses when a call site genuinely names the open definition.

Verified by disabling it: without the change the fixture stops with
``NotSupportedException ... at Conformance.Tally`1.ArgumentName``, which is the
proof the six new checks are load-bearing rather than decorative.

### Generic types can be built at run time, and the fixture caught a wrong answer

`MakeGenericType` works. It was recorded as blocked by erasure, and the
per-construction generic types built earlier in this session removed the block
without that being noticed — a closed construction is already a real runtime
type with its own identity and static storage, so `MakeGenericType` only had to
call the loader path a `TypeSpec` already calls, cache included. That sharing is
the point: `typeof(Cell<>).MakeGenericType(typeof(int))` returns the *identical
instance* as `typeof(Cell<int>)`, so reference equality on types stays reliable,
and `Activator.CreateInstance` on a type built at run time gives an object whose
methods run normally. `IsGenericType`, `IsGenericTypeDefinition`,
`ContainsGenericParameters` and `GetGenericTypeDefinition` came with it.

**The fixture found a divergence the other way round.** A new check asserted
that an open definition has no type arguments. `dotnet` failed it: .NET returns
the type *parameter* — `typeof(Cell<>).GetGenericArguments()` is `[T]`, not
`[]` — and this runtime had been quietly answering zero. It refuses now, because
it records only the arity of a definition and has no runtime type for a
parameter, and an empty array is exactly the plausible-looking wrong answer the
project refuses to give. No conformance check covers it: the two runtimes
deliberately differ there, so a check could not match both.

That is the second time writing the check found the bug rather than confirming
the fix. Conformance is 204 → 222 checks, byte-identical.

### Several threads share one heap, and the test found a race

The runtime beneath C# can now be driven by more than one OS thread. Three
things had to hold, and `crates/rustclr-core/tests/parallel.rs` checks each:
threads allocate into one heap, an object made on one is visible on another,
and a collection stops everyone and gathers their roots before it sweeps.

**The order was the point.** The safepoint handshake went first, alone, because
it was the only genuine unknown — if that protocol did not hold, nothing built
on it would either. The mechanical half came second: 56 call sites that read a
managed object were moved behind closures so no borrow escaped, one batch at a
time, each verified against the conformance fixture. Only once none of them held
a borrow did a lock go underneath, and at that point no call site had to change.
Attempted together, a failure could have been either half.

The compiler found the one place that could not be mechanical: `with_values` in
`collections.rs` returned `values_mut()` and applied the edit afterwards, a
borrow that would have outlived the lock. It does the edit inside the closure
now. One site out of 56 is roughly the rate the closure conversion was betting
on.

**The race the test found.** The first multi-threaded test passed, and passed
for the wrong reason: the interpreter never called `Mutators::register`, so the
collector waited for nobody. Registering it exposed the real defect. `poll`
decrements `parked` on the way out, but a thread woken from collection *N* and
not yet rescheduled is still counted — so a collector starting *N+1* immediately
afterwards saw four parked threads, concluded everyone had arrived at *this*
safe point, and swept without their roots. All four workers lost their rooted
strings. `stop_the_world` now waits for the previous round to drain before
starting the next. Forty consecutive runs clean afterwards; the same test failed
four-for-four before.

Back-to-back collections are what make it reproducible. A single collection
never shows it, which is why the first version of the test — spawn four threads,
collect once — passed while the bug was live. It also finished in 0.01s with the
workers already unregistered, so it was measuring nothing.

**What it costs.** Best of five against the same binary built before the lock:
+2% on `fields`, `virtual` and `arrays`, +4% on `alloc` and `calls`, +9% on
`strings`. On a microcontroller, 1,732 bytes of flash and no RAM at all — `bss`
and `data` are byte-identical, because without `std` the lock is a `RefCell` and
the mutator registry is a shim that compiles away. An M5Stack Tough on COM5 runs
`HelloWorld` with all of it in place, output identical to `dotnet`.

**No C# program can use any of it yet**, and it would be wrong to imply
otherwise. The loader is still per-interpreter, and it holds three things that
change while a program runs: static field storage, the interface-dispatch stub
cache, and the closed generic constructions. Two threads would see two sets of
statics. Sharing it is not a smaller version of the heap job — `registry.ty()`
runs on nearly every instruction and hands back a reference, so a lock there
would cost far more than the heap's did. It needs splitting into a read-only
part shared without a lock and a mutable part behind one. That is the next
piece of work. `docs/limitations.md` states the line as it stands.

### Seven board firmwares, one demonstration

`embedded/demo-common` holds the on-chip report and each firmware supplies a
`core::fmt::Write` to receive it. That refactor was the point of adding the
fourth and fifth boards: "they all print the same thing" is only true if there
is one copy of it, and four copies would have drifted.

| Board | Core | Target | Tier | State |
| --- | --- | --- | --- | --- |
| ESP32-C3 | RISC-V 32 | `riscv32imc-unknown-none-elf` | full | **executes IL on hardware** |
| ESP32-WROOM-32 | Xtensa LX6 | `xtensa-esp32-none-elf` | full | run on hardware (pre-interpreter) |
| Meadow F7 Micro | Arm Cortex-M7 | `thumbv7em-none-eabihf` | full | run on hardware (pre-interpreter) |
| Sipeed Maix Go | RISC-V 64 | `riscv64gc-unknown-none-elf` | full | builds; **not yet flashed** |
| Netduino 3 WiFi | Arm Cortex-M4F | `thumbv7em-none-eabihf` | minimal | builds; **not yet flashed** |
| Raspberry Pi Pico | Arm Cortex-M0+ | `thumbv6m-none-eabi` | minimal | builds; **not yet flashed** |
| Nucleo-F401RE | Arm Cortex-M4F | `thumbv7em-none-eabihf` | **none** | builds; **not yet flashed** |

`tests/firmware.sh` builds all seven. That catches a class `tests/embedded.sh`
cannot: a change to a shared type breaks a board firmware long before it breaks
the host build, and otherwise nobody notices until they reach for the hardware.

### Generic types get one runtime type per construction

`Cell<int>` and `Cell<string>` are now two runtime types. They share one body —
generics remain erased for *execution* — but each carries its own type arguments
and its own static storage, so `typeof(T)`, `x is T`, `default(T)` and a static
field per construction all agree with .NET.

**Three pieces had to line up:**

* **Constructions are built at load time**, from the `TypeSpec` that names them.
  `resolve_type_sig` is `&self` and cannot intern a new type, so building them
  lazily was never available; a pass beside the existing ones was.
* **`newobj` had to stop using the constructor's declaring type.** A member
  reference on a construction resolves to the *definition's* `.ctor`, because
  one body serves every construction — so the instance it built was a `Cell<T>`
  whose `T` was unknowable. The member reference now remembers which
  construction was named.
* **A class type parameter is answered through the receiver.** The body is
  shared, so the method cannot know; `this` can.

**Framework generics are left erased on purpose.** Every native binding is keyed
by its declaring type's name, and giving `List<int>` a name of its own would put
`List`1::Add` out of reach of the implementation behind it. Nothing in the
collections needs `T` at run time anyway — their storage is a managed array of
runtime values, and a value already carries its shape.

**One bug, and it was the kind that only shows up under test.** Static storage is
indexed by `FieldId`, not by the `slot` an instance field carries. The first
version set `slot` on each cloned static and grew the table to match, which put
every value at one index while every read looked at another — a panic in the
fixture rather than a wrong answer, which is the good outcome.

### Generic methods know their type arguments

`typeof(T)`, `default(T)` and `x is T` now answer inside a generic method, and
match .NET at every instantiation.

**Nothing had to be inferred — the argument was being thrown away.** Every call
to `M<int>` emits a `MethodSpec` carrying `int`, and the loader already gave each
spec its own `MethodInfo` so the *name* could distinguish
`AppendFormatted<bool>` from `AppendFormatted<int>`. It parsed the arguments,
used them to build a name, and dropped them. Recording them on the instantiation
and reading them when a `!!N` token is resolved is the whole change.

**The refusal that was there stays, narrowed.** `typeof(T)` on a *class* type
parameter still throws rather than answering `System.Object`, because that
argument really was erased. The check now asks whether the executing frame can
answer first, and only refuses when it cannot — so the error means "this one is
genuinely unknown" rather than "generic parameters are unknown".

**The type half is a different size of job**, and is not started. A method
instantiation only had to record what the call site already said; a type
instantiation needs a distinct runtime type per closed construction, with its
own static slots, its own identity, and a shared body whose `!0` resolves
through the receiver. That touches layout, vtables, statics and identity at
once.

### Arrays compile, at 89.8×

An `int[]` that arrives as a **parameter** is now handed to compiled code as a
two-word descriptor — data pointer and length — so `a[i]` becomes a bounds check
and a scaled-index load. The `arrays` benchmark goes from 18,401 ms interpreted
to 205 ms compiled: the largest gap in the suite, because element access is
where the interpreter pays most — a handle resolution and a `Value` per element.

**The design turned on one discovery.** `ArrayStorage` keeps `int[]` as a
contiguous `Vec<i32>`, not as boxed values. That is what made a pointer viable
at all; the plan before checking was a call out of compiled code into a runtime
helper for every element, with Win64 shadow space and volatile-register
discipline around each one. Reading the representation first turned a risky ABI
change into two loads.

**Holding a raw pointer into the managed heap is sound for a stated reason.**
The backend declines any method that allocates, so no collection can run while
compiled code executes; and this collector never moves an object in any case.
Both would have to change together for the pointer to go stale. That same
invariant is why an array created *inside* the method is still declined —
`new int[n]` allocates, which is exactly what it forbids.

**A bounds failure cannot throw.** Compiled code has no frame for the
interpreter to unwind, so it writes a flag one slot past the arguments and
returns, and the tier raises `IndexOutOfRangeException`. Declining and
re-running interpreted would have been wrong: stores already made stay made,
which is also what .NET does — the exception aborts the method, it does not roll
it back. Making that work meant widening `NativeTier::try_execute` to return a
`Result`, because "ran and faulted" is not the same as "declined".

**Two things the benchmarks caught.** The array workloads already in the suite
did not speed up at all, because `sieve` and `sort` allocate their arrays as
locals — the feature was working and the benchmarks could not see it. And
`rustnet jit` explained the decline of `Fill(int[],int)` as "parameter 0 is not
an integer", which was false: the explanation tested integers only while the
backend had moved on. A wrong explanation is worse than none, because it sends
the reader after the wrong thing. The real reason was `conv.u8`, from a `long`
in the benchmark I had just written.

### The interpreter runs on Xtensa

An **M5Stack Tough** — ESP32-D0WD rev v3.1, dual core, 16 MB flash — executes
`HelloWorld.Main` with all 836 native bindings:
[docs/logs/m5stack-tough.log](docs/logs/m5stack-tough.log). That is the second
architecture, and the same source file as the RISC-V ESP32-C3: 68 IL
instructions and 6 calls on x86-64, on RISC-V 32 and on Xtensa LX6 alike.

**The two-region heap was theory until now.** The ESP32's main `dram_seg` tops
out at 176 KB and the full binding set needs 260,702 bytes, so the firmware adds
the 96 KB bank at `0x3ffe7e30` that sits past the ROM's data and stacks and that
the linker will not place ordinary statics in. That reaches 278,528. The
arrangement was designed for the WROOM-32 during the STM32 work and never run;
this board is the first hardware confirmation that a heap split across two
regions actually carries the runtime.

**The banner was asserting something it could not know.** It read
"ESP32-WROOM-32" because that is the board the `esp32` feature was written for —
but a firmware knows which *chip* it was compiled for and has no way to tell
which board that chip is soldered to. Running it on a different ESP32 made the
report wrong. It now names the chip, and which board a capture came from lives
in the log header where a person writes it.

The board's previous contents — the user's own RustNet ESP-IDF firmware, on
`esp-idf-svc 0.51` — were read back to `backups/m5tough-flash-backup.bin`
(16,777,216 bytes) before anything was written. Same discipline as the Meadow
F7: a flash that replaces someone's firmware should be reversible before it
starts, not after.

### Sorting with a comparer, and a docs claim that was wrong

`list.Sort((a, b) => ...)` and `list.Sort(myComparer)` run. Both bind through
the same arity key and are told apart by what the object *is* — a
`Comparison<T>` is a delegate, an `IComparer<T>` is an object with a `Compare`
method — because binding by arity means both arrive at the same native.

The comparator is managed code: it allocates, calls back into the BCL, and can
throw. That rules out Rust's `sort_by`, whose comparator returns an `Ordering`
and can do none of those. So it is a merge sort written out, calling
`Interpreter::invoke` per comparison and propagating an exception the moment one
escapes.

**Merge sort for two reasons, both deliberate.** It makes a predictable number
of comparisons whatever the input, so a comparator with side effects is not at
the mercy of a pivot choice; and it is stable. .NET's `List<T>.Sort` is an
unstable introsort and documents the order of equal elements as unspecified, so
this is the one place the two runtimes can legitimately disagree — stated in
`docs/limitations.md` rather than left to be discovered.

**The claim in the README was wrong in both directions.** It said custom
comparers were *ignored*, which would have meant silently wrong output — the
worst failure mode this project has a convention against. They were in fact
refused with "no implementation", which is the right behaviour for something
unimplemented. Reading the code to implement the feature is what turned up the
inaccuracy; the docs now say what the code does.

`String.CompareOrdinal` came with it, because a comparison lambda almost always
reaches for it and it was unbound.

### Reflection is finished, apart from loading

`Assembly` and `Module` enumerate: `GetExecutingAssembly`, `GetEntryAssembly`,
`GetTypes`, `GetType(name)`, `GetName`, `Type.Assembly`, `Type.Module`. Each is
an object holding an assembly id, the same shape a `MethodInfo` uses for a
method id — three types rather than one because `Type.Assembly` and
`Type.Module` are distinct properties in C#, and returning the wrong one would
compile and then misbehave.

Two things the reference runtime settled rather than reasoning:

* **`GetEntryAssembly` is not assembly 0.** Slot 0 is RustBCL's synthetic
  assembly, so the first version reported `RustBCL` as the program's entry
  assembly. It now finds the one with an entry point.
* **`Module.Name` keeps the extension.** .NET answers `Conformance.dll` where
  the assembly is `Conformance`. Both the test expectation and the
  implementation were written the other way and a byte-for-byte comparison
  caught it; the name now comes from the `Module` table rather than from the
  assembly name plus a guessed suffix.

**Parameter names came with it.** The `Param` table is separate from the
signature — a signature carries types, and only that table carries what the
author called them — so `MethodInfo` now stores them and `ParameterInfo.Name`
reports the real one. A method with no rows there still answers `argN`, which
says "not recorded" rather than inventing one.

### Exception filters, and properties for reflection

**`catch when` runs.** The obstacle was never the matching rule — it was that a
filter is managed code executing *during* the unwind, before the frames below
it are discarded, which is the whole reason `when` can see state a catch block
would arrive too late to observe.

A filter now gets a frame of its own, pushed onto the frame being unwound and
sharing its method and arguments, with a copy of its locals. `endfilter` writes
those locals back and returns the `int32` verdict through the same frame-floor
mechanism a native-to-managed call uses — the mechanism that already existed for
`ToString` called from a native.

The write-back was not in the first version, and a test caught it.
`catch (E e) when (Log(ref buffer))` came back with the log empty: the `ref`
pointed into the filter's copy of the locals, so the append vanished and the
filter looked as though it had never run. Five conformance checks cover filters
now, including ordering across nested clauses and the spec's rule that a
throwing filter *declines* rather than replacing the exception in flight.

**Properties are materialised from metadata.** C# compiles `p.X` to a call to
`get_X`, so nothing was needed to run a property — what was missing was knowing
that `get_X` and `set_X` are two halves of one member, which is what
`GetProperties()` has to answer. The loader now reads `PropertyMap` and
`MethodSemantics` and interns a `PropertyInfo` per property.

Reconstructed from those tables rather than guessed from method names, which
matters twice: a method called `get_Total` is not necessarily an accessor, and a
property whose accessors an obfuscator renamed still pairs correctly.

`MethodBase.GetParameters()` came with it. A parameter has no id of its own — it
is the *n*th entry of a signature — so a `ParameterInfo` carries the method id
and the position packed into one word.

**A one-line bug worth recording**: the first run reported position 0 and type
`Object` for every parameter. `ParameterInfo` was not in the loader's list of
pre-registered framework types, so `new_member` could not find the type to
allocate and returned null for every entry. The array had the right length and
nothing else.

### CodeGen: boards, deploy, and a check that found four bugs

**Devices** (`Ctrl+D`) lists the seven boards, scans for what is attached, and
flashes firmware or a program. The panel is organised around the memory budget
because that is what decides everything else: each board's heap is drawn on one
scale against 192,045 and 260,702 bytes, so a tier reads as arithmetic. The
Nucleo-F401RE's bar stopping 126,509 bytes short of the first mark is the entire
explanation for why it cannot run C#.

Two decisions there were deliberate rather than convenient:

* **Scanning never touches a board.** It lists ports and probes. `Identify` is a
  separate action because `espflash board-info` resets the part into its
  bootloader, which should not be a side effect of opening a window.
* **Ambiguity is reported.** A CH340 bridge looks identical whether an ESP32 or
  a sewing machine is behind it, and a probe says nothing about what is on the
  far end of SWD. Those read "possibly connected". The next step writes to
  flash, and a confident wrong answer there is expensive.

**Deploy needed the firmware to stop hard-coding its assembly.** These boards
have no filesystem, so an application is not copied onto one — it is compiled
into the image. Each firmware's `build.rs` now resolves `RUSTCLR_APP` and hands
the path to `include_bytes!`, so `RUSTCLR_APP=MyApp.dll cargo build` embeds a
different program without editing the crate. Verified by swapping a 4,608-byte
assembly for an 11,264-byte one and watching the image grow.

**Six embedded templates, written to the reduced binding set** — blink
scheduler, ring-buffer logger, Modbus RTU frames, PID control, edge classifier,
Morse beacon. They stay inside `Console`, `String`, `Math` and arrays, because a
template that will not run on the board it was written for is worse than no
template.

**`--verify-templates` is the part worth keeping.** `RunsOnRustClr` was a
property nothing checked, and the convention says templates carrying it must
actually run there. The command scaffolds all 20 templates, builds them, runs
them on both runtimes and diffs the output — board templates against
`--bcl minimal`, since passing with all 836 bindings says nothing about a 192 KB
board. It found four real things on its first run:

1. **`Console.ReadLine()` hung the runner.** `ProcessRunner` redirected stdout
   and stderr but left stdin inherited, so two templates waited forever on a
   console nobody was typing at. Any child reading stdin would have wedged the
   IDE the same way. Stdin is now redirected and closed at once, which is EOF.
2. **`string + char` does not run.** .NET 10 lowers it to a span-based
   `String.Concat`, and `Span<T>` is unimplemented — so `text += "0123ABC"[i]`
   fails at *every* tier, not just the reduced one. Three of my own templates
   did it. Fixed in the templates, and documented there, because the runtime gap
   is the documented kind.
3. **`char.ToUpperInvariant` was missing** while `ToUpper` sat registered beside
   it. Added, along with `ToLowerInvariant`.
4. **`"a b".Split(' ')` failed.** It looks like a one-argument call and is not:
   C# resolves it to `Split(char, StringSplitOptions)`, whose typed key carries
   a token that means nothing outside the calling assembly. `Split/1`, `/2` and
   `/3` now bind through arity, the same fix `Join/2` needed earlier.

**And a formatting bug worth its own note.** `console-numerics` printed a
solver's residual as `0.000000000291990431…` where .NET prints
`2.9199043183325557E-10`. .NET switches to scientific notation outside a band
and Rust never does. The first fix guessed the band as `[-4, 15)` and `1e15`
disagreed; reading the boundaries off the reference runtime gave `[-4, 17)` for
double and `[-4, 9)` for float. Eight conformance checks cover it now — the
fixture is at 144.

Neither of the two boards this was built against was connected, so Deploy and
Flash are exercised only as far as building the image. Scanning, tier reporting
and the minimal-BCL pre-check run without hardware.

### The two STM32F4 boards bracket the memory question

`embedded/stm32f4` builds for a **Nucleo-F401RE** and a **Netduino 3 WiFi** from
one source file. They were not added for another Cortex-M — they were added
because they sit either side of the line the interpreter draws.

**The F427VI runs a program only after its memories swap roles.** The part
advertises 256 KB in two pieces that are not adjacent: 192 KB of DMA-reachable
SRAM at `0x20000000`, and 64 KB of CCM at `0x10000000` that the core reaches but
DMA cannot. Handing the allocator only the SRAM leaves 192 KB minus `.data`,
`.bss` and the stack, against a 192,045-byte floor — so a few kilobytes of
statics decides whether the board runs C#. `memory-f427vi.x` therefore names
**CCM as `RAM`**, which is what moves `.data`, `.bss` and the stack there
(`cortex-m-rt`'s `link.x` hardcodes `> RAM`), and gives the whole SRAM to the
heap through its own `(NOLOAD)` section. The heap could not be an ordinary
`static`: `.bss` is in CCM now, and a `static` would follow it there. Verified
by reading the linked ELF rather than by assertion — `.data` at `0x10000000`,
`.sram_heap` 196,608 bytes at `0x20000000`, `_stack_start` at `0x10010000`.
That clears the floor by 4,563 bytes, which is thin enough to re-measure if
RustBCL grows.

**The F401RE is the first board that cannot run one at all.** 96 KB against a
192,045-byte floor is not close, so the firmware prints the shortfall and
carries on with the metadata reader and the collector.

**An unplanned result worth recording: it does not pay flash for what it cannot
use.** `Tier::for_budget` is a `const fn` and `HEAP_BYTES` is a constant, so LTO
folds the decision, finds the `Full` and `Minimal` arms unreachable, and strips
the loader and all 836 native bindings. `.text` is 21 KB on the F401RE against
282 KB on the F427VI, from the same source file. That fell out of making the
tier a constant expression rather than a runtime check — worth knowing, because
it means adding a board below the threshold costs nothing but its own bring-up.

Neither board was connected, so both rows are builds.

**Two facts worth not rediscovering**, both taken from the sibling RustNet ports
rather than worked out again. The RP2040's ROM checks a CRC over the first 256
bytes of flash, so the second-stage bootloader is taken prebuilt — a wrong CRC
is a board that silently returns to BOOTSEL. And the K210 derives its UART baud
straight from the core clock, so the firmware *reads* PLL0 and the clock
selector rather than assuming 26 MHz; assuming would give a port that opens and
prints noise.

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

Two bugs stood between "compiles" and "runs": the metadata crate's `use alloc::…`
reached only `lib.rs`, and `Image` was gated on `std` in its entirety when only
`from_file` and `path()` need a filesystem.

### The interpreter runs on the chip

**C# executes on an ESP32-C3** — RISC-V, 400 KB of SRAM, no operating system.
The loader builds a type registry, RustBCL registers all 836 of its native
bindings, and `HelloWorld.Main` prints the same three lines `dotnet` prints,
CRLF included, with the same 68 IL instructions and 6 calls:
[docs/logs/esp32c3-interpreter.log](docs/logs/esp32c3-interpreter.log).

**The blocker was written down wrong.** The note here said `rustclr-core`
needed "a hash map, a clock and file IO", and that had gone unchallenged for
three milestones. Two thirds of it was false: every map key in the runtime is
an integer id, a tuple of them, or a name — all `Ord`, so `BTreeMap` serves
without `std` — and the clock had been behind the `Host` trait since the trait
existed. Only the filesystem was real, and it is now the one thing gated.
`rustclr-bcl` went the same way: 20 of its 24 `std::` paths were
`std::cmp::Ordering`.

Two differences are genuine rather than cosmetic:

* **`Arc` becomes `Rc` without `std`.** RISC-V `imc` has no atomics extension,
  so `Arc` does not exist on the ESP32-C3 at all. `Rc` is correct there anyway:
  the interpreter is single-threaded on a chip.
* **Float maths comes from `libm`.** `core` has no `sqrt`, no `sin`, not even
  `abs` for `f64`, and `System.Math` is largely a libm. This is the only
  external dependency anywhere in the runtime; it is optional and a default
  build does not pull it in.

**The memory budget is the whole story on these boards**, and guessing at it
wasted two flash cycles. The first attempt asked for a single 98,304-byte
allocation and died — `Heap::embedded(4096)` reserves its slot table up front,
deliberately, and 4,096 was a number I picked rather than measured. The second
died inside `rustclr_bcl::install` at 192 KB. Measuring it with a counting
allocator settled it: **260,702 bytes** peak with every binding, **192,045**
with console, strings and maths only. `Tier::for_budget` now compares a board's
heap against those, and a board that clears neither says so in a line of text
instead of faulting inside the allocator.

**The WROOM-32 needed two heap regions**, which is the most transferable thing
here. Its `dram_seg` tops out at 176 KB — found by bisecting until the link
succeeded — below even the reduced set. The ESP32 has a second bank of 98,768
bytes past the ROM's data and stacks that the linker will not put ordinary
statics in; `esp-alloc` takes regions rather than one arena, so the firmware
adds both and reaches 272 KB. A single allocation cannot span them, and the
largest the runtime makes is 67,584 bytes, which clears either.

**Sizing the heap as a static array is what made these failures cheap.** Asking
the C3 for 320 KB produces `.bss will not fit in region DRAM, overflowed by
13844 bytes` at link time. A dynamically grown heap would have found the same
limit as a hard fault on the wire.

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
covers: 2971 ms interpreted, 269 ms compiled — **11.0× faster**, and 1.8× .NET
rather than 20×.

**Then inlining, and a second benchmark to measure it honestly.** A `call` used
to disqualify a method outright, which meant the backend only took code written
to suit it. `crates/rustclr-jit/src/inline.rs` splices branch-free static
callees into their callers — one level, arguments spilled to fresh locals,
`ldarg.N` rewritten to `ldloc`, the trailing `ret` dropped. The `inlined`
workload is the same arithmetic as `kernels` factored into helpers:

| | interpreted | compiled, `--no-inline` | compiled |
| --- | ---: | ---: | ---: |
| `inlined` | 1629 ms | 1148 ms | **400 ms** |

**2.9× of that is the inliner alone**, and it is worth exactly 1.0× on
`kernels`, whose callees all contain loops. Adding `--no-inline` was what made
the claim checkable rather than asserted — the same reasoning that produced
`--no-jit`.

**A bug worth recording: synthetic offsets do not work.** The first design gave
spliced instructions offsets in a high range (`0x8000_0000+`) so the caller's
offsets would never need renumbering. It was wrong. `analyse` reaches an
instruction's successor as `offset + length`, so the chain broke at each splice
boundary, `depth_at` came back sparse, and the translator read a missing entry
as depth zero — surfacing as `attempt to subtract with overflow` at
`translate.rs:416`, nowhere near the cause. The fix was to stop pretending
offsets are byte positions: renumber the whole stream one instruction per
offset and remap branch targets through a table. Nothing downstream cares,
because everything downstream treats offsets as identifiers.

**A metric that lied, caught by writing it down.** The first test asserted that
inlining compiles *more* methods across the fixture. It failed at 9 vs 10 — and
the inliner was working. Inlining `Scale` into `Blend` means `Scale` is never
called, so it never gets hot enough to compile, and the total goes *down*. The
test now names `Blend` and asks the backend about it directly.

**What it does not cover, and why that matters.** Running `rustnet jit` on the
existing benchmark suite still compiles nothing outside those two workloads:
every other one uses arrays. That is a real finding, not a footnote — the
backend's reach is narrow, and the honest way to show it was to add workloads of
the covered shape beside the others rather than in place of them.

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

1. **Generic types** — done for user types and methods, including a class type
   parameter in a *static* method: the call site's `MemberRef` names the
   construction, so the frame carries it. What is left is framework generics,
   which stay erased by choice because native bindings are keyed by declaring
   type name.
2. **Real concurrency** — done. `Thread.Start`, `Task.Run` and `Parallel.*` run
   on real threads sharing the heap and static storage; `lock` and `Interlocked`
   exclude; `await` suspends and the completing thread resumes; tasks run on a
   pool of one worker per core, and `Task.Delay` arms a timer. `rustclr-sched`
   is no longer unused — its lock-free queue is the pool's. `await using`, `ValueTask` and now `await foreach` over
   an `async IAsyncEnumerable<T>` all work, so the async surface itself is
   complete — what is missing is overlap, not syntax.

   The runtime beneath it has moved. The thread-safe heap and collector that
   this was waiting on are done and tested: several OS threads allocate into one
   heap, share objects, and stop at safe points so a collection sees every
   root — see "Several threads share one heap" above. What now blocks the C#
   surface is one specific thing, the per-interpreter `Loader`: two threads
   would see two sets of static fields. It needs splitting into a read-only part
   shared without a lock and a mutable part behind one, because a lock across
   `registry.ty()` — called on nearly every instruction — would cost more than
   the whole heap change did.
3. **Native code generation beyond integer methods and `int[]`** — the x86-64
   backend is 11.0× faster on arithmetic and 89.8× on an array walk. It still
   declines allocation, exception handling, floating point and object field
   access; an array created *inside* a method falls under allocation, which is
   why `sieve` and `sort` are still interpreted. The AArch64 and RISC-V backends
   emit and disassemble correctly but **have never executed a single
   instruction**, and deliberately decline arrays — adding untested encodings to
   an untested backend would be stacking one unknown on another.
4. **Reflection breadth** — constructing a generic type at run time now works;
   `MakeGenericType` calls the same loader path a `TypeSpec` does and shares its
   cache, so it returns the identical instance as `typeof`. The gap that
   replaced it is narrower: `GetGenericArguments()` on an *open* definition,
   where .NET returns the type parameter `T` and this runtime has no runtime
   type for one — so it refuses. `Assembly.Load` works on
   a host but resolves by probing where .NET reads `deps.json`, so an
   unreferenced DLL beside the app loads here and does not there — noted in
   `docs/limitations.md` because it is a divergence in the risky direction.
   Everything else works: types, members, properties, parameters and their
   names, assemblies, modules, invocation and attributes.
5. **IL execution on more hardware** — an ESP32-C3 (RISC-V 32) and an M5Stack
   Tough (Xtensa LX6) run the interpreter with the whole of RustBCL. The
   remaining images build but have not been flashed since; the Pico and the
   Netduino clear only the reduced binding set, the Nucleo-F401RE clears
   neither, and no board was connected for any of them. Ahead-of-time
   compilation additionally needs the Arm and RISC-V backends to execute.
6. **`Span<T>`** — done. Over an array, over a string, and over `stackalloc`
   memory; `Memory<T>`, `AsSpan` and `AsMemory` too. Slicing a span that
   *stands for* a string still refuses, because the string is the whole
   representation and there is no offset in it.
7. **The remaining IL** — `localloc`, `cpblk` and `initblk` work: `Value` has a
   raw-pointer kind now, and it is a buffer plus a byte offset rather than an
   address. What is left is `arglist` (varargs), `mkrefany`/`refanyval`/
   `refanytype`, `jmp`, and multi-dimensional arrays with non-zero lower
   bounds. Exception filters and `calli` work.

`rustnet capabilities` prints this from the runtime itself, and
`rustnet verify <assembly>` names what a specific program would hit.

### The advanced-feature matrix is measured, not asserted

`tests/fixtures/AdvancedFeatures/` is 21 single-feature probes plus a real
incremental source generator. `probe.sh` runs each on both runtimes and diffs —
**21 of 21 pass on RustCLR today**.

That measures *output*, and output is not capability. `async`, threading and the
TPL all produce the answers .NET produces while nothing overlaps, so a probe
that measured wall-clock time would fail three of these. The matrix marks those
rows with a warning rather than a tick for that reason. Results and reasoning:
[docs/advanced-features.md](docs/advanced-features.md) ·
[Bahasa Indonesia](docs/id/fitur-lanjutan.md).

Two findings from that run were not obvious beforehand:

- **Source generators and interceptors need no runtime support at all.** Both
  are Roslyn-side; the runtime only ever sees ordinary IL. Proven with a
  generator that actually intercepts a call, not by reasoning about it.
- **`Thread` spawns for real.** `Thread.Start` starts an OS thread and `Join`
  waits for it; `lock` excludes and `Interlocked` does not lose updates. `Task`
  and `Parallel.For` still run their work where it is created, so a
  `Thread`-based program overlaps and a task-based one does not.

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
