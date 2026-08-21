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
| `noop` | 109 | **58** | **0.5×** | Process start — nothing else |
| `exceptions` | 130 | **111** | **0.9×** | 50k throws through `try`/`catch`/`finally` |
| `kernels` | 149 | **268** | **1.8×** | Integer arithmetic written longhand — **compiled** |
| `strings` | 96 | **190** | **2.0×** | 20k concatenations, `Length`, `IndexOf` |
| `inlined` | 113 | **327** | **2.9×** | The same arithmetic in helpers — **compiled, inlined** |
| `arrays` | 128 | **418** | **3.3×** | An `int[]` passed in and walked — **compiled** |
| `fib` | 108 | 442 | 4.1× | Recursive call overhead, fib(27) |
| `alloc` | 112 | 1,014 | 9.1× | 300k allocations with collection pressure |
| `sieve` | 107 | 1,287 | 12.0× | 1M-element sieve: array writes in a tight loop |
| `matrix` | 110 | 1,470 | 13.4× | 120×120 multiply: floating-point array maths |
| `virtual` | 115 | 1,788 | 15.5× | 2M virtual calls through a base reference |
| `sort` | 122 | 2,460 | 20.2× | Quicksort over 200k ints |
| `fields` | 111 | 2,943 | 26.5× | 3M instance field reads and writes |

Every row's checksum is compared between the two runtimes before it is timed. A
mismatch prints `MISMATCH` instead of a number — a benchmark that computed a
different answer is not a benchmark.

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
of .NET's time on the shorter workloads. `sieve` at 114 ms on .NET is roughly
110 ms of startup and a few milliseconds of work; RustCLR's 1,187 ms is 65 ms of
startup and about 1.1 s of work. The compute ratio is therefore far worse than
the wall-clock ratio suggests — closer to 100× on the tightest loops.

That is what interpretation costs. .NET compiles IL to machine code; RustCLR
walks it instruction by instruction — around 33 million IL instructions per
second on this machine.

**Except where the code generator reaches.** Two rows are compiled to machine
code rather than interpreted, and they are deliberately the same arithmetic
written two ways.

`kernels` writes it out longhand, in methods that call nothing and allocate
nothing — the shape the backend has always taken. Compiled it runs in 269 ms
against 2,971 ms interpreted: **11.0× faster**, moving it from 20× slower than
.NET to 1.8×.

`arrays` is the third, and the one with the widest gap. An `int[]` that arrives
as a *parameter* is handed to compiled code as a data pointer and a length, so
element access is a bounds check and a scaled-index load. Interpreted it takes
18,401 ms; compiled, 205 ms — **89.8×**, because element access is exactly where
the interpreter pays most: a handle resolution and a `Value` for every element.

Note what `arrays` does *not* measure. `sieve` and `sort` are array-bound too
and did not move at all, because they allocate their arrays as locals and the
backend declines any method that allocates. That is not an oversight in them —
it is the invariant that makes a raw pointer into the managed heap safe for the
length of a call, and the reason `arrays` was added beside them rather than
instead of them.

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
