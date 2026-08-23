# Benchmarks

RustCLR against the reference .NET runtime, on the same assembly.

```bash
cd benchmarks
bash run.sh
```

---

## Results

Windows 11, x64. `rustnet` built with `cargo build --release`; the benchmark
assembly built `-c Release` with tiered compilation **off**, so .NET runs at
full JIT speed rather than warming up. Best of three runs, wall clock including
process start.

| Workload | .NET (ms) | RustCLR (ms) | Ratio | What it stresses |
| --- | ---: | ---: | ---: | --- |
| `fib` | 128 | **75** | **0.6×** | Recursive call overhead, fib(27) — **compiled** |
| `matrix` | 87 | **60** | **0.7×** | 120×120 multiply over `double[]` — **compiled** |
| `noop` | 124 | **93** | **0.8×** | Process start — nothing else |
| `sieve` | 119 | **92** | **0.8×** | 1M-element sieve over `bool[]` — **compiled** |
| `exceptions` | 125 | **129** | **1.0×** | 50k throws through `try`/`catch`/`finally` |
| `strings` | 94 | **169** | **1.8×** | 20k concatenations, `Length`, `IndexOf` |
| `arrays` | 117 | **232** | **2.0×** | An `int[]` passed in and walked — **compiled** |
| `sort` | 112 | **236** | **2.1×** | Quicksort over 200k ints — **compiled, recursive** |
| `kernels` | 127 | **339** | **2.7×** | Integer arithmetic written longhand — **compiled** |
| `inlined` | 94 | **442** | **4.7×** | The same arithmetic in helpers — **compiled, inlined** |
| `alloc` | 101 | 1,072 | 10.6× | 300k allocations with collection pressure |
| `virtual` | 112 | 1,962 | 17.5× | 2M virtual calls through a base reference |
| `fields` | 109 | 3,173 | 29.1× | 3M instance field reads and writes |

`sieve` and `matrix` are the two worth reading carefully. Both dropped from
around 1,500 ms to under 100 ms when the tiering heuristic started compiling
methods that loop, and both are now *at the process-start floor* — `noop` is
70 ms, so almost everything left in those two rows is starting the process. The
ratio against .NET is therefore mostly RustCLR's faster startup rather than
faster computation, and reading them as "twice as fast as .NET at numerics"
would be wrong.

### How much of a difference is real

**About ten percent of run-to-run drift, on this machine, within one session.**
That is measured, not assumed: `fields` gave 2,856 ms, then 3,097 ms, then
3,200 ms across one afternoon on the same binary, drifting slower as the machine
warmed. Two full suite runs an hour apart put every row 5–10% above the table
above.

So a 5% difference between two runs of this suite means nothing. Before
attributing one to a change, measure both binaries **in the same session,
interleaved** — which is how the shared-heap cost in
[limitations.md](limitations.md) was measured, and why that comparison is
trustworthy where a comparison against this table would not be.

The check that catches a real regression is the reverse experiment: remove the
suspect code and measure again. A per-call lookup added for generic type
arguments looked like an 8% cost on `virtual` until deleting it measured
*slower* still — the drift was the whole signal.

`noop` is the row to distrust most: it is process start and nothing else, so it
measures the machine rather than the runtime. Registering the full BCL rather
than the console-only subset accounts for about 3 ms of it, measured with
`--bcl minimal`.

---

## Reading these numbers

**Subtract the `noop` row.** Both figures include process start, which is most
of .NET's time on the shorter workloads and most of RustCLR's on the compiled
ones. `fields` at 112 ms on .NET is roughly 110 ms of startup and a couple of
milliseconds of work; RustCLR's 3,256 ms is 70 ms of startup and about 3.2 s of
work. The compute ratio on an interpreted row is therefore far worse than the
wall-clock ratio suggests — closer to 100× on the tightest loops.

The same subtraction cuts the other way on `sieve` and `matrix`: at 60 ms and
84 ms against a 70 ms floor, almost nothing is left to attribute to
computation, and their sub-1× ratios say more about process start than about
arithmetic.

That is what interpretation costs. .NET compiles IL to machine code; RustCLR
walks it instruction by instruction — around 33 million IL instructions per
second on this machine.

**Except where the code generator reaches.** Six rows are compiled to machine
code rather than interpreted — `sieve`, `matrix`, `arrays`, `kernels`,
`inlined`, and parts of others.

`kernels` writes it out longhand, in methods that call nothing and allocate
nothing — the shape the backend has always taken. Compiled it runs in 269 ms
against 2,971 ms interpreted: **11.0× faster**, moving it from 20× slower than
.NET to 1.8×.

`arrays` is the one with the widest gap. An `int[]` that arrives
as a *parameter* is handed to compiled code as a data pointer and a length, so
element access is a bounds check and a scaled-index load. Interpreted it takes
18,401 ms; compiled, 205 ms — **89.8×**, because element access is exactly where
the interpreter pays most: a handle resolution and a `Value` for every element.

`sieve` and `matrix` were the two this table used to embarrass. Both are
array-bound, both allocate their arrays as *locals*, and the backend declined
any method that allocated — so `arrays` was added beside them to measure the
part that did compile. Both compile now: allocation from compiled code is
sound because allocating does not itself collect and a compiled method cannot
return a reference, so nothing it allocates outlives the call.

Neither moved until the *tiering* changed as well. Each is called once, so a
call-count threshold never fired however long it ran; a method whose body loops
now compiles on its first call. Two separate limits, and the benchmark showed
no improvement until both were gone — which is why they are described together
here.

`sort` compiles now too. `QuickSort` is recursive, which is the one call shape
the inliner cannot remove — splicing a method into itself does not terminate —
so it needed a calling convention of its own: the arguments go into an outgoing
frame the prologue reserves, and the callee is entered exactly as the tier
enters the method, same argument-array shape and same trap slot.

Compiled recursion runs on the *machine* stack rather than the interpreter's
frame budget, which cannot be grown or unwound the same way, so compiled code
counts its own depth and traps past a limit. `Down(200000)` reports the same
catchable exception interpreted and compiled; .NET stack-overflows and dies.

`Sort` itself — the wrapper that calls `QuickSort` — is still interpreted,
because a call to *another* method needs an address that may not exist when the
caller is compiled. Only self-calls are emitted.

`inlined` factors the identical arithmetic into small static helpers, which is
what code in the wild actually looks like. Every one of those calls used to
disqualify the calling method outright. With the inliner splicing them in:

| | interpreted | compiled, `--no-inline` | compiled |
| --- | ---: | ---: | ---: |
| `inlined` | 1,629 ms | 1,148 ms | **400 ms** |

**2.9× of that is the inliner alone.** Without it, only the leaf helpers compile
and the method around them stays interpreted. Inlining is worth nothing at all
on `kernels` — 1.0×, because its callees contain loops and none are eligible —
and that contrast is the point. The gain is not in compiling more instructions;
it is in compiling code that was written normally.

Every other row is unchanged by the JIT, because every other row uses arrays and
`rustnet jit` declines them all. That is why these two were added *beside* the
existing workloads rather than replacing one: the interesting number is not just
what compilation is worth where it applies, but how narrow "where it applies"
still is.

```bash
rustnet jit benchmarks/Benchmarks/bin/Release/net10.0/Benchmarks.dll

# The two figures above, isolated:
rustnet run --no-jit    …/Benchmarks.dll -- inlined   # interpreted
rustnet run --no-inline …/Benchmarks.dll -- inlined   # compiled, nothing spliced
rustnet run            …/Benchmarks.dll -- inlined    # compiled and inlined
```

**RustCLR starts in half the time.** Its tiering compiles nothing until a method
has been called 32 times, and emits a few hundred bytes when it does, so there
is no warm-up to pay for: 65 ms against 126 ms. For a CLI tool that runs briefly and exits, or a
microcontroller that has no room for a code cache, that matters more than steady
-state throughput.

**Exceptions are a wash, and strings are close.** Throwing is cheap here because
RustCLR does not build a full managed stack trace object on every throw, and
string work is close because `Concat`, `Length` and `IndexOf` are native Rust —
the interpreter dispatches once and the work happens at native speed. The same
is true of anything that spends its time inside RustBCL rather than in IL.

**Field access is the worst case, and that is the right shape.** `fields` does
almost nothing per instruction, so the interpreter's per-instruction overhead is
essentially the whole measurement. It is the honest upper bound on what a JIT
would recover.

---

## What would change these

[Milestone 4](../Plan.md) is a native code generator. The analysis it needs
already exists — `rustclr-jit` computes basic blocks, per-instruction stack
depth and which methods a simple backend could take. Every workload above
except `noop` and `exceptions` is dominated by interpreter dispatch, which is
exactly what compilation removes.

The tiering design means a partial backend helps immediately:
`Compiler::can_compile` decides per method, so an emitter that only handles leaf
integer methods would already take `fib`, `sieve` and `fields`.

---

## Running them yourself

```bash
cargo build --release              # rustnet must be a release build
cd benchmarks
bash run.sh                        # all workloads, best of 3

RUNS=10 bash run.sh                # more runs, less noise
WORKLOADS="sieve sort" bash run.sh # just these two
```

The harness refuses to report a figure when the two runtimes disagree on the
result, and warns loudly if it finds only a debug build of `rustnet` — a debug
interpreter is several times slower again and would make the comparison
meaningless.

The workloads live in `benchmarks/Benchmarks/Program.cs`. They use arrays and
explicit loops — no LINQ, no generic collections — because a benchmark that only
runs on one of the two runtimes measures nothing useful. That was a hard
constraint when they were written; since Milestone 2 both would run, but keeping
the workloads allocation-light is what makes them measure the interpreter rather
than the collection implementations.
