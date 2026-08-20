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
| `noop` | 126 | **65** | **0.5×** | Process start — nothing else |
| `exceptions` | 125 | **128** | **1.0×** | 50k throws through `try`/`catch`/`finally` |
| `strings` | 134 | **176** | **1.3×** | 20k concatenations, `Length`, `IndexOf` |
| `fib` | 119 | 427 | 3.6× | Recursive call overhead, fib(27) |
| `alloc` | 119 | 992 | 8.3× | 300k allocations with collection pressure |
| `sieve` | 114 | 1,187 | 10.4× | 1M-element sieve: array writes in a tight loop |
| `matrix` | 110 | 1,305 | 11.9× | 120×120 multiply: floating-point array maths |
| `virtual` | 110 | 1,632 | 14.8× | 2M virtual calls through a base reference |
| `sort` | 120 | 2,242 | 18.7× | Quicksort over 200k ints |
| `fields` | 144 | 2,764 | 19.2× | 3M instance field reads and writes |

Every row's checksum is compared between the two runtimes before it is timed. A
mismatch prints `MISMATCH` instead of a number — a benchmark that computed a
different answer is not a benchmark.

---

## Reading these numbers

**Subtract the `noop` row.** Both figures include process start, which is most
of .NET's time on the shorter workloads. `sieve` at 114 ms on .NET is roughly
110 ms of startup and a few milliseconds of work; RustCLR's 1,187 ms is 65 ms of
startup and about 1.1 s of work. The compute ratio is therefore far worse than
the wall-clock ratio suggests — closer to 100× on the tightest loops.

That is what interpretation costs. .NET compiles IL to machine code; RustCLR
walks it instruction by instruction. Roughly 1.8 million IL instructions per
second is the current throughput.

**RustCLR starts in half the time.** No JIT, no tiered compilation, no warm-up:
65 ms against 126 ms. For a CLI tool that runs briefly and exits, or a
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
